//! Google sign-in. Google refuses to sign in inside embedded webviews, so
//! Ligma does what the Figma desktop app does: the user signs in and
//! approves Ligma in a real browser, and Figma hands a one-time secret back
//! through a `figma://app_auth/redeem` link. Figma only ever sends that link
//! to `figma://`, whatever app asked.
//!
//! 1. The login page's "Continue with Google" opens `/start_google_sso`,
//!    as a popup or a redirect. Ligma cancels that and has the page start
//!    an app session (`START_JS`, `POST /api/session/app_auth`).
//! 2. The page reports the session's id by loading `SIGNAL_HOST`, which
//!    `tabs::on_navigation` passes to `on_signal`. Ligma shows Figma's grant
//!    page for it: on macOS in Apple's sign-in sheet, which catches the
//!    `figma://` link itself (web_auth.rs); on Windows in the browser, with
//!    `figma://` pointed at Ligma meanwhile (figma_scheme.rs).
//! 3. Once the user approves, `on_figma_link` gets the link and hands its
//!    secret to the page that asked, still on Figma's login page, like
//!    Figma desktop does. The page redeems it
//!    (`POST /api/session/app_auth/redeem`), then opens the files of the
//!    account it added. Tabs share cookies, so all are signed in.
//!
//! Figma's "Add account" opens its login page in a new tab, and Google
//! sign-in there runs the same steps; the redeem adds the account to the
//! session. The page that asked must redeem: Figma's own `/app_auth/redeem`
//! page sends a signed-in session on to its files without redeeming, and a
//! signed-out session that loads the login page again can't redeem.
//!
//! A link is only redeemed while a sign-in Ligma started is waiting for
//! it, so a link from elsewhere can't sign Ligma into someone else's account.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, Url};
use tauri_plugin_dialog::DialogExt;

/// Loaded by the page to report back; never resolves.
pub const SIGNAL_HOST: &str = "auth.ligma.invalid";

/// Fragment key of the sign-in secret handed to `PAGE_JS` by loading a
/// page, when the page that asked was put to sleep meanwhile.
const REDEEM_KEY: &str = "ligma_redeem";

/// How long a woken or resuming tab gets before the secret is handed over
/// by loading a page instead.
const RESUME_WAIT: Duration = Duration::from_secs(10);

/// How long a sign-in waits for the browser.
const WAIT: Duration = Duration::from_secs(15 * 60);

/// Defines `__ligmaSignIn`, which starts an app session and reports it to
/// `SIGNAL_HOST`, and `__ligmaRedeem`, which redeems its secret and opens
/// the account it added; also redeems a secret in the fragment
/// (`REDEEM_KEY`). Sends the login page's Google popup to `__ligmaSignIn`
/// instead of a window Ligma would deny. The page polls the popup for a
/// result; the stand-in never closes, so it waits quietly for the secret.
pub const PAGE_JS: &str = r#"(() => {
  if (location.hostname !== "www.figma.com") return;
  const signal = (params) => {
    location.href = "https://auth.ligma.invalid/?" + new URLSearchParams(params);
  };
  const post = (path, body) =>
    fetch(path, {
      method: "POST",
      credentials: "include",
      headers: { "Content-Type": "application/json", "X-Csrf-Bypass": "yes" },
      body: JSON.stringify(body),
    }).then((response) => (response.ok ? response.json() : Promise.reject()));
  // Ids of the accounts signed in to this session.
  const accounts = () =>
    fetch("/api/session/state", { credentials: "include", headers: { "X-Csrf-Bypass": "yes" } })
      .then((response) => (response.ok ? response.json() : Promise.reject()))
      .then(({ meta }) => (meta?.users ?? []).map((user) => String(user.id)))
      .catch(() => []);
  Object.defineProperty(window, "__ligmaRedeem", {
    value: (secret) =>
      accounts().then((before) =>
        post("/api/session/app_auth/redeem", { g_secret: secret })
          .then(accounts, () => Promise.reject())
          .then((after) => {
            const added = after.find((id) => !before.includes(id));
            location.replace(added ? "/files?fuid=" + encodeURIComponent(added) : "/files");
          })
          .catch(() => signal({ error: "redeem" })),
      ),
  });
  const secret = new URLSearchParams(location.hash.slice(1)).get("ligma_redeem");
  if (secret) {
    history.replaceState(history.state, "", location.pathname + location.search);
    window.__ligmaRedeem(secret);
  }
  Object.defineProperty(window, "__ligmaSignIn", {
    value: () =>
      post("/api/session/app_auth", { app_type: "desktop" })
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
    let Some(grant) = grant.filter(|grant| {
        !grant.is_empty() && grant.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    }) else {
        let redeem = url.query_pairs().any(|(key, value)| key == "error" && value == "redeem");
        cancel(app, Some(if redeem { REDEEM_FAILED } else { START_FAILED }));
        return;
    };
    let since = Instant::now();
    *app.state::<Pending>().0.lock().unwrap() = Some((id, since));
    let url: Url = format!("https://www.figma.com/app_auth/{grant}/grant").parse().unwrap();
    #[cfg(target_os = "macos")]
    crate::web_auth::start(app, url);
    #[cfg(not(target_os = "macos"))]
    {
        #[cfg(windows)]
        crate::figma_scheme::borrow();
        use tauri_plugin_opener::OpenerExt;
        let _ = app.opener().open_url(url.as_str(), None::<&str>);
    }
    // Gives up, and `figma://` back, if the browser never answers.
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(WAIT);
        let pending = app.state::<Pending>();
        let mut pending = pending.0.lock().unwrap();
        if pending.is_some_and(|(_, started)| started == since) {
            *pending = None;
            #[cfg(windows)]
            crate::figma_scheme::restore();
        }
    });
}

pub const START_FAILED: &str = "Figma couldn't start the sign-in. Check your connection and try again.";
const REDEEM_FAILED: &str = "Figma didn't accept the sign-in. Try Continue with Google again.";

/// Ends the waiting sign-in, showing `error` if it failed rather than was
/// cancelled.
pub fn cancel(app: &AppHandle, error: Option<&str>) {
    app.state::<Pending>().0.lock().unwrap().take();
    #[cfg(windows)]
    crate::figma_scheme::restore();
    if let Some(error) = error {
        app.dialog().message(error).title("Sign in with Google").show(|_| {});
    }
}

/// A `figma://` link: the end of a sign-in, or on Windows, where Ligma may
/// handle the scheme, one of Figma's "Open in desktop app" links, opened in
/// a tab. Not on the main thread: it may open one.
pub fn on_figma_link(app: &AppHandle, url: &Url) {
    if let Some(window) = app.get_window("main") {
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    if url.host_str() == Some("app_auth") && url.path() == "/redeem" {
        if let Some(secret) = url.query_pairs().find(|(key, _)| key == "g_secret").map(|(_, value)| value) {
            redeem(app, &secret);
        }
        return;
    }
    let Some(host) = url.host_str().filter(|host| !host.is_empty()) else {
        return;
    };
    let Ok(mut page) = format!("https://www.figma.com/{host}{}", url.path()).parse::<Url>() else {
        return;
    };
    page.set_query(url.query());
    let _ = crate::tabs::open(app, page);
}

fn redeem(app: &AppHandle, secret: &str) {
    let pending = app.state::<Pending>().0.lock().unwrap().take();
    #[cfg(windows)]
    crate::figma_scheme::restore();
    let Some((id, _)) = pending.filter(|(_, since)| since.elapsed() < WAIT) else {
        return;
    };
    // Asleep tabs wake, frozen ones resume (on macOS, a moment later).
    let _ = crate::tabs::select(app, id);
    let fallback = redeem_page(secret);
    let started = Instant::now();
    while started.elapsed() < RESUME_WAIT {
        if let Some(webview) = crate::tabs::scriptable(app, id) {
            // A tab that woke may not have its page yet; it loads one.
            let _ = webview.eval(format!(
                "window.__ligmaRedeem ? window.__ligmaRedeem({}) : location.replace({})",
                serde_json::to_string(secret).unwrap(),
                serde_json::to_string(fallback.as_str()).unwrap(),
            ));
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    crate::tabs::navigate(app, id, fallback);
}

/// A page that redeems `secret` once loaded (see `REDEEM_KEY`). Its own
/// query keeps it from being a same-page fragment change, which wouldn't
/// run `PAGE_JS`.
fn redeem_page(secret: &str) -> Url {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let mut page: Url = format!("https://www.figma.com/files?ligma_signin={stamp}").parse().unwrap();
    let mut fragment: Url = "https://fragment.invalid/".parse().unwrap();
    fragment.query_pairs_mut().append_pair(REDEEM_KEY, secret);
    page.set_fragment(fragment.query());
    page
}
