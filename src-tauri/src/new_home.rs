//! Ligma's home page (Settings > Features > Home, experimental, off by
//! default): the Home tab shows `dist/home.html` in place of Figma's file
//! browser. Files by when you last worked on them, search, the inbox and
//! organizing files, read from and sent to Figma with the user's session
//!
//! The page needs www.figma.com as its origin, for the session's cookies,
//! but none of Figma's home page, which is megabytes of script. So Home
//! loads `URL`, a tiny text file on www.figma.com with no
//! Content-Security-Policy, and `PAGE_JS` swaps the page in once that has
//! loaded. Files opened from it go to their own tabs like any link from
//! Home (see `tabs::on_navigation`)

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Url};

use crate::settings::{ColorScheme, Settings};

/// Where Home loads while on. The query marks it for `PAGE_JS`
pub const URL: &str = "https://www.figma.com/.well-known/security.txt?ligma-home";

const HTML: &str = include_str!("../../dist/home.html");
const THEMES_JS: &str = include_str!("../../dist/themes.js");

/// Runs in the Home tab. Replaces the text file's page with
/// `__LIGMA_HOME_HTML__` and runs its scripts, which the page's lack of a
/// Content-Security-Policy allows
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || location.pathname !== "/.well-known/security.txt" || location.search !== "?ligma-home" || window.top !== window) return;
  const html = __LIGMA_HOME_HTML__;
  const swap = () => {
    const doc = new DOMParser().parseFromString(html, "text/html");
    document.replaceChild(document.adoptNode(doc.documentElement), document.documentElement);
    // Scripts parsed elsewhere don't run; fresh ones do, in order
    for (const old of document.querySelectorAll("script")) {
      const script = document.createElement("script");
      script.textContent = old.textContent;
      old.replaceWith(script);
    }
  };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", swap, { once: true });
  else swap();
})()"##;

static ON: AtomicBool = AtomicBool::new(false);

/// Call at launch, before any tab
pub fn init(settings: &Settings) {
    ON.store(settings.new_home, Ordering::Relaxed);
}

/// What Home loads: Ligma's home page or Figma's file browser
pub fn home_url() -> Url {
    if ON.load(Ordering::Relaxed) { URL } else { crate::tabs::HOME_URL }.parse().unwrap()
}

/// Whether `url` is Ligma's home page
pub fn is_url(url: &Url) -> bool {
    url.as_str() == URL
}

/// `PAGE_JS`, for the Home tab. Its themes come inline: the page can't load
/// Ligma's files from figma.com
pub fn page_js() -> String {
    let html = HTML.replace(r#"<script src="themes.js"></script>"#, &format!("<script>{THEMES_JS}</script>"));
    PAGE_JS.replace("__LIGMA_HOME_HTML__", &serde_json::to_string(&html).unwrap())
}

/// Origin of the links the page opens to ask Ligma for something: a color
/// scheme picked in its account menu's Change theme, or Settings
pub const SIGNAL_HOST: &str = "home.ligma.invalid";

/// A request from the page in tab `tab`, taken only from Home. Not from
/// within the webview's own callback, which is still deciding on the page
pub fn on_signal(app: &AppHandle, tab: u32, url: &Url) {
    if tab != crate::tabs::HOME_ID {
        return;
    }
    let path = url.path().to_owned();
    let value = url.query_pairs().find(|(key, _)| key == "value").map(|(_, value)| value.into_owned());
    let app = app.clone();
    std::thread::spawn(move || match path.as_str() {
        "/settings" => {
            if let Err(error) = crate::settings::open_window(&app) {
                log::warn!("Couldn't open Settings: {error}");
            }
        }
        "/scheme" => {
            let scheme = match value.as_deref() {
                Some("system") => ColorScheme::System,
                Some("light") => ColorScheme::Light,
                Some("dark") => ColorScheme::Dark,
                _ => return,
            };
            let mut settings = crate::settings::current(&app);
            if settings.color_scheme == scheme {
                return;
            }
            settings.color_scheme = scheme;
            if let Err(error) = crate::settings::change(&app, settings.clone()) {
                log::error!("Couldn't save settings: {error}");
                return;
            }
            crate::settings::show_in_window(&app, &settings);
        }
        _ => {}
    });
}

/// Moves Home to the other page when the setting changes
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    if ON.swap(settings.new_home, Ordering::Relaxed) == settings.new_home {
        return;
    }
    crate::tabs::set_home_url(app, home_url());
}
