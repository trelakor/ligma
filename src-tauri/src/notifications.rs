//! Figma's notifications, the ones in its bell (comments, replies,
//! mentions, invites and the rest), shown as the system's own
//!
//! Figma's web push needs a push service the system webviews don't have, so
//! Ligma reads the bell instead: every `CHECK_EVERY` a loaded Figma tab
//! fetches the feed Figma's bell shows (`FETCH_JS`), and each unread entry
//! not seen before is shown. Entries already there at launch aren't. A
//! batch of comments that grows comes back with a new time, so it's shown
//! again. Clicking one opens its link in a tab
//!
//! Nothing is checked while every tab is asleep, as in game mode

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tauri::{AppHandle, Url, Webview};

use crate::settings::Settings;

const CHECK_EVERY: Duration = Duration::from_secs(30);
/// How long a fetch may take before the next check tries again
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
/// More new at once than this show as one notification
const MAX_SHOWN: usize = 3;

/// Fetches the feed of Figma's bell, for all of the user's plans ("-1",
/// the bell's own default), into `window.__ligmaBell`, read by `READ_JS`.
/// Text comes as HTML, which is turned to plain text here. Not through the
/// DOM: Figma's pages may require Trusted Types
const FETCH_JS: &str = r##"(() => {
  const state = (window.__ligmaBell = { done: false, items: null });
  const entities = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " " };
  const text = (blocks) =>
    (blocks ?? [])
      .map((block) => block?.html_text ?? "")
      .join("")
      .replace(/<br\s*\/?>/gi, " ")
      .replace(/<[^>]*>/g, "")
      .replace(/&(#x[\da-f]+|#\d+|\w+);/gi, (entity, name) => {
        if (name[0] !== "#") return entities[name.toLowerCase()] ?? entity;
        const code = name[1] === "x" || name[1] === "X" ? parseInt(name.slice(2), 16) : parseInt(name.slice(1), 10);
        try {
          return String.fromCodePoint(code);
        } catch {
          return entity;
        }
      })
      .replace(/\s+/g, " ")
      .trim();
  const params = new URLSearchParams({ currentView: "fullscreen", appVersion: "1", clientType: "web", current_plan_id: "-1", is_plan_org: "false" });
  fetch("/api/user_notifications/server_driven/plan?" + params, { credentials: "include" })
    .then((response) => (response.ok ? response.json() : null))
    .then((feed) => {
      if (!Array.isArray(feed?.meta)) return;
      state.items = feed.meta.map((item) => {
        const body = item?.preferred_attachments?.find((attachment) => attachment?.body)?.body ?? {};
        return {
          id: `${item?.notification_id}@${body.created_at ?? ""}`,
          unread: !!item?.is_unread,
          title: text(body.title),
          subtitle: text(body.subtitle),
          text: text(body.description),
          link: item?.deeplink?.href ?? "",
        };
      });
    })
    .catch(() => {})
    .finally(() => (state.done = true));
})()"##;

/// The feed `FETCH_JS` fetched, once it's done: `{ items }`, with `items`
/// null if it failed (signed out, offline)
const READ_JS: &str = "window.__ligmaBell?.done ? { items: window.__ligmaBell.items } : null";

#[derive(Deserialize)]
struct Reply {
    items: Option<Vec<Item>>,
}

/// An entry in Figma's bell
#[derive(Deserialize)]
struct Item {
    /// Figma's id and the entry's time, which changes as a batch grows
    id: String,
    unread: bool,
    /// Who it's from
    title: String,
    /// What happened, and where: "Commented · <file>"
    subtitle: String,
    /// The comment, if there's one
    text: String,
    link: String,
}

static ENABLED: AtomicBool = AtomicBool::new(true);
/// Entries seen so far. None until the first check, whose entries are
/// taken as seen, and again after notifications are turned back on
static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Call at launch
pub fn init(settings: &Settings) {
    ENABLED.store(settings.figma_notifications, Ordering::Relaxed);
}

pub fn apply_settings(settings: &Settings) {
    if !ENABLED.swap(settings.figma_notifications, Ordering::Relaxed) && settings.figma_notifications {
        // Not what came while they were off
        *SEEN.lock().unwrap() = None;
    }
}

/// Starts checking Figma's bell
pub fn start(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let _ = mac_notification_sys::set_application(&app.config().identifier);
    }
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(CHECK_EVERY);
        if ENABLED.load(Ordering::Relaxed) {
            check(&app);
        }
    });
}

/// A Figma tab that can fetch from Figma's API
fn figma_tab(app: &AppHandle) -> Option<Webview> {
    crate::tabs::scriptable_tabs(app)
        .into_iter()
        .find(|webview| webview.url().is_ok_and(|url| url.scheme() == "https" && url.host_str() == Some("www.figma.com")))
}

/// Runs `script` in `webview`, for its result as JSON. Through a channel:
/// the page's answer comes on the main thread
fn eval(webview: &Webview, script: &str) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    webview
        .eval_with_callback(script, move |json| {
            let _ = tx.send(json);
        })
        .ok()?;
    rx.recv_timeout(Duration::from_secs(2)).ok()
}

fn check(app: &AppHandle) {
    let Some(webview) = figma_tab(app) else {
        return;
    };
    let _ = webview.eval(FETCH_JS);
    let started = Instant::now();
    let items = loop {
        std::thread::sleep(Duration::from_millis(500));
        if started.elapsed() > FETCH_TIMEOUT {
            return;
        }
        // Gone, asleep or reloaded meanwhile
        let Some(json) = eval(&webview, READ_JS) else {
            return;
        };
        match serde_json::from_str::<Option<Reply>>(&json) {
            Ok(Some(reply)) => break reply.items,
            Ok(None) => continue,
            Err(_) => return,
        }
    };
    let Some(items) = items else {
        return;
    };
    // Turned off while fetching
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let new: Vec<Item> = {
        let mut seen = SEEN.lock().unwrap();
        let first = seen.is_none();
        let seen = seen.get_or_insert_with(HashSet::new);
        let new = items
            .into_iter()
            .filter(|item| seen.insert(item.id.clone()) && item.unread && !first)
            .collect();
        new
    };
    // Not for the file you're looking at, as Figma's own push doesn't
    let looking_at = crate::tabs::in_front(app).then(|| crate::tabs::active_file(app).map(|file| file.key)).flatten();
    let new: Vec<Item> = new
        .into_iter()
        .filter(|item| {
            let key = Url::parse(&item.link).ok().and_then(|url| crate::tabs::file_key(&url));
            looking_at.is_none() || key != looking_at
        })
        .collect();
    if new.len() > MAX_SHOWN {
        show(app, "Figma", &format!("{} new notifications", new.len()), "", None);
        return;
    }
    for item in new {
        let title = if item.title.is_empty() { "Figma" } else { &item.title };
        let link = Url::parse(&item.link).ok().filter(crate::tabs::is_figma);
        show(app, title, &item.subtitle, &item.text, link);
    }
}

/// Opens `link` in a tab, after a click on its notification, or just
/// brings Ligma to the front
fn on_click(app: &AppHandle, link: Option<Url>) {
    let app = app.clone();
    // Off the main thread: it may open a tab
    std::thread::spawn(move || {
        if let Some(window) = crate::tabs::focused_window(&app) {
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
        if let Some(link) = link {
            let _ = crate::tabs::open(&app, link);
        }
    });
}

/// Toasts shown, kept so a click still reaches `on_click` from the
/// notification center. The latest few
#[cfg(windows)]
static TOASTS: Mutex<Vec<windows::UI::Notifications::ToastNotification>> = Mutex::new(Vec::new());

#[cfg(windows)]
fn show(app: &AppHandle, title: &str, subtitle: &str, text: &str, link: Option<Url>) {
    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::TypedEventHandler;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};

    fn escape(text: &str) -> String {
        text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
    }
    let lines: String = [title, subtitle, text]
        .iter()
        .filter(|line| !line.is_empty())
        .map(|line| format!("<text>{}</text>", escape(line)))
        .collect();
    let xml = format!(r#"<toast><visual><binding template="ToastGeneric">{lines}</binding></visual></toast>"#);
    let result = (|| -> windows::core::Result<()> {
        let document = XmlDocument::new()?;
        document.LoadXml(&HSTRING::from(xml))?;
        let toast = ToastNotification::CreateToastNotification(&document)?;
        let clicked = app.clone();
        toast.Activated(&TypedEventHandler::new(move |_, _| {
            on_click(&clicked, link.clone());
            Ok(())
        }))?;
        // The installer's Start menu shortcut carries this id, which toasts
        // need to show as Ligma's
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(&app.config().identifier))?;
        notifier.Show(&toast)?;
        let mut toasts = TOASTS.lock().unwrap();
        toasts.push(toast);
        let excess = toasts.len().saturating_sub(20);
        toasts.drain(..excess);
        Ok(())
    })();
    if let Err(error) = result {
        log::warn!("Couldn't show a notification: {error}");
    }
}

#[cfg(target_os = "macos")]
fn show(app: &AppHandle, title: &str, subtitle: &str, text: &str, link: Option<Url>) {
    let (title, subtitle, text, app) = (title.to_owned(), subtitle.to_owned(), text.to_owned(), app.clone());
    // Waits for the click, so on its own thread
    std::thread::spawn(move || {
        let (subtitle, message) = if text.is_empty() { (None, subtitle.as_str()) } else { (Some(subtitle.as_str()), text.as_str()) };
        let response = mac_notification_sys::Notification::new()
            .title(&title)
            .maybe_subtitle(subtitle)
            .message(message)
            .default_sound()
            .wait_for_click(true)
            .send();
        match response {
            Ok(mac_notification_sys::NotificationResponse::Click) => on_click(&app, link),
            Ok(_) => {}
            Err(error) => log::warn!("Couldn't show a notification: {error}"),
        }
    });
}
