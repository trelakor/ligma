//! Google sign-in. Google refuses to sign in inside embedded webviews, so
//! Ligma does what the Figma desktop app does: the user signs in and
//! approves Ligma in their browser, which hands a one-time secret back
//! through a `ligma://` link.
//!
//! 1. The login page's "Continue with Google" opens `/start_google_sso`,
//!    as a popup or a redirect. Ligma cancels that and has the page start
//!    an app session (`START_JS`, `POST /api/session/app_auth`).
//! 2. The page reports the session's id by loading `SIGNAL_HOST`, which
//!    `tabs::on_navigation` passes to `on_signal`. Ligma opens Figma's grant
//!    page for it in the browser, with `desktop_protocol=ligma`.
//! 3. Once the user approves, Figma sends the browser to
//!    `ligma://app_auth/redeem?g_secret=…`, which `on_open_url` receives.
//!    The tab that asked loads Figma's own redeem page with that secret,
//!    which signs it in and reloads. Tabs share cookies, so all are signed in.
//!
//! A link is only redeemed while a sign-in Ligma started is waiting for
//! it, so a link from elsewhere can't sign Ligma into someone else's account.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, Url};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

/// The scheme Ligma registers, also in `tauri.conf.json`.
pub const SCHEME: &str = "ligma";

/// Loaded by the page to report back; never resolves.
pub const SIGNAL_HOST: &str = "auth.ligma.invalid";

/// How long a sign-in waits for the browser.
const WAIT: Duration = Duration::from_secs(15 * 60);

/// Defines `__ligmaSignIn`, which starts an app session and reports it to
/// `SIGNAL_HOST`, and sends the login page's Google popup there instead of
/// a window Ligma would deny. The page polls the popup for a result; the
/// stand-in never closes, so it waits quietly until the redeem page loads.
pub const PAGE_JS: &str = r#"(() => {
  if (location.hostname !== "www.figma.com") return;
  const signal = (params) => {
    location.href = "https://auth.ligma.invalid/?" + new URLSearchParams(params);
  };
  Object.defineProperty(window, "__ligmaSignIn", {
    value: () =>
      fetch("/api/session/app_auth", {
        method: "POST",
        credentials: "include",
        headers: { "Content-Type": "application/json", "X-Csrf-Bypass": "yes" },
        body: JSON.stringify({ app_type: "desktop" }),
      })
        .then((response) => (response.ok ? response.json() : Promise.reject()))
        .then(({ meta }) => signal({ grant: meta.id }))
        .catch(() => signal({ error: "start" })),
  });
  const open = window.open;
  window.open = function (url, ...rest) {
    try {
      const target = new URL(url, location.href);
      if (target.origin === location.origin && target.pathname === "/start_google_sso") {
        window.__ligmaSignIn();
        return { closed: false, close() {}, focus() {}, postMessage() {} };
      }
    } catch {}
    return open.call(this, url, ...rest);
  };
})();"#;

const START_JS: &str = "window.__ligmaSignIn?.()";

/// The sign-in waiting for the browser: the tab that started it, and when.
#[derive(Default)]
pub struct Pending(Mutex<Option<(u32, Instant)>>);

/// True if `url` starts Figma's Google sign-in.
pub fn is_google_sign_in(url: &Url) -> bool {
    url.host_str() == Some("www.figma.com") && url.path() == "/start_google_sso"
}

/// Starts signing in from a page that redirected to Google. The popup
/// variant starts from `PAGE_JS` on its own.
pub fn start(webview: &tauri::Webview) {
    let _ = webview.eval(START_JS);
}

/// The page in tab `id` reporting through `SIGNAL_HOST`.
pub fn on_signal(app: &AppHandle, id: u32, url: &Url) {
    let grant = url.query_pairs().find(|(key, _)| key == "grant").map(|(_, value)| value);
    match grant.filter(|grant| {
        !grant.is_empty() && grant.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    }) {
        Some(grant) => {
            *app.state::<Pending>().0.lock().unwrap() = Some((id, Instant::now()));
            let url = format!("https://www.figma.com/app_auth/{grant}/grant?desktop_protocol={SCHEME}");
            let _ = app.opener().open_url(url, None::<&str>);
        }
        None => {
            app.state::<Pending>().0.lock().unwrap().take();
            app.dialog()
                .message("Figma didn't start the sign-in. Check your connection and try again.")
                .title("Sign in with Google")
                .show(|_| {});
        }
    }
}

/// A `ligma://` link, from the browser or the system.
pub fn on_open_url(app: &AppHandle, url: &Url) {
    if url.scheme() != SCHEME || url.host_str() != Some("app_auth") || url.path() != "/redeem" {
        return;
    }
    let Some(secret) = url.query_pairs().find(|(key, _)| key == "g_secret").map(|(_, value)| value) else {
        return;
    };
    let pending = app.state::<Pending>().0.lock().unwrap().take();
    let Some((id, _)) = pending.filter(|(_, since)| since.elapsed() < WAIT) else {
        return;
    };
    let mut redeem: Url = "https://www.figma.com/app_auth/redeem".parse().unwrap();
    redeem.query_pairs_mut().append_pair("g_secret", &secret);
    if let Some(window) = app.get_window("main") {
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    crate::tabs::navigate(app, id, redeem);
}
