//! Discord Rich Presence: shows the open file and the selected tool, as
//! "Ligma". The tool's icon is the small image, blue while the window has
//! focus and grey while it doesn't. Discord names the activity after the Discord application, so
//! the application behind `CLIENT_ID` must be called "Ligma".
//!
//! Talking to Discord blocks, so a worker thread owns the connection. The
//! app sends it the wanted status every second and it forwards changes,
//! reconnecting whenever Discord is (re)started.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use discord_rich_presence::activity::{Activity, Assets, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use tauri::{AppHandle, Manager};

/// Application ID from the Discord Developer Portal. Empty: presence off.
const CLIENT_ID: &str = "1563151879788167309";
const LOGO_URL: &str = "https://raw.githubusercontent.com/trelakor/ligma/main/assets/logo.png";
/// Tool icons, `assets/tools/{focused,unfocused}/<slug>.png`.
const TOOLS_URL: &str = "https://raw.githubusercontent.com/trelakor/ligma/main/assets/tools";

/// Discord allows 5 activity updates per 20 seconds.
const MIN_INTERVAL: Duration = Duration::from_secs(4);
/// Wait between attempts to reach Discord when it isn't running.
const RETRY_AFTER: Duration = Duration::from_secs(15);

/// Name of the selected tool in the toolbar, or null. Figma marks the
/// active tool button with `aria-pressed` or `aria-checked`; its label may
/// carry a shortcut ("Pen – P") or name a tool group ("Move tools").
const TOOL_JS: &str = r#"(() => {
  const tools = [
    "Move", "Hand", "Scale", "Frame", "Section", "Slice", "Rectangle", "Line",
    "Arrow", "Ellipse", "Polygon", "Star", "Image", "Pen", "Pencil", "Text",
    "Comment", "Sticky", "Shape", "Connector", "Marker", "Highlighter",
    "Eraser", "Washi tape", "Stamp", "Table", "Text on path", "Annotation",
    "Measurement",
  ];
  const byName = new Map(tools.map((tool) => [tool.toLowerCase(), tool]));
  // Dev Mode names these after the action.
  byName.set("annotate", "Annotation");
  byName.set("measure", "Measurement");
  for (const el of document.querySelectorAll('[aria-pressed="true"], [aria-checked="true"]')) {
    const label = (el.getAttribute("aria-label") || el.getAttribute("data-tooltip") || "")
      .replace(/\s+[–—-]\s+.*$/, "")
      .replace(/\s+tools?$/i, "")
      .trim()
      .toLowerCase();
    if (byName.has(label)) return byName.get(label);
  }
  return null;
})()"#;

#[derive(Clone, Default, PartialEq)]
struct Status {
    /// Name of the open file. None outside a file (Home, other pages).
    file: Option<String>,
    tool: Option<String>,
    /// Whether the Ligma window has focus.
    focused: bool,
}

/// Channel to the worker. None when presence is off.
pub struct Presence(Mutex<Option<Sender<Status>>>);

fn worker(settings: &crate::settings::Settings) -> Option<Sender<Status>> {
    if CLIENT_ID.is_empty() || !settings.discord_rich_presence {
        return None;
    }
    let (tx, rx) = channel();
    std::thread::spawn(move || run(rx));
    Some(tx)
}

pub fn start(settings: &crate::settings::Settings) -> Presence {
    Presence(Mutex::new(worker(settings)))
}

/// Turns presence on or off. Dropping the channel stops the worker, which
/// disconnects from Discord and so clears the status.
pub fn apply_settings(app: &AppHandle, settings: &crate::settings::Settings) {
    let mut tx = app.state::<Presence>().inner().0.lock().unwrap();
    if tx.is_some() != settings.discord_rich_presence {
        *tx = worker(settings);
    }
}

/// Reports the active tab to Discord. Call periodically.
pub fn sync(app: &AppHandle) {
    let Some(tx) = app.state::<Presence>().0.lock().unwrap().clone() else {
        return;
    };
    // Not `get_webview_window`: that returns None once the window holds
    // more than one webview, i.e. whenever a file tab is open.
    let focused = app
        .get_window("main")
        .and_then(|window| window.is_focused().ok())
        .unwrap_or(false);
    let Some((file, webview)) = crate::tabs::active_file(app) else {
        let _ = tx.send(Status { focused, ..Status::default() });
        return;
    };
    let _ = webview.eval_with_callback(TOOL_JS, move |json| {
        let tool = serde_json::from_str::<Option<String>>(&json).ok().flatten();
        let _ = tx.send(Status { file: Some(file.clone()), tool, focused });
    });
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_millis() as i64)
}

/// File name of the tool's icon in `assets/tools`, if it has one.
fn tool_icon(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "Move" => "move",
        "Hand" => "hand",
        "Scale" => "scale",
        "Frame" => "frame",
        "Section" => "section",
        "Slice" => "slice",
        "Rectangle" => "rectangle",
        "Line" => "line",
        "Arrow" => "arrow",
        "Ellipse" => "ellipse",
        "Polygon" => "polygon",
        "Star" => "star",
        "Pen" => "pen",
        "Pencil" => "pencil",
        "Text" => "text",
        "Text on path" => "text-on-path",
        "Comment" => "comment",
        "Annotation" => "annotation",
        "Measurement" => "measurement",
        _ => return None,
    })
}

fn activity(status: &Status, started: i64) -> Activity<'_> {
    let details = match status.file.as_deref() {
        // "Figma" is the title while the file is still loading.
        Some("" | "Figma") => "Editing a file".to_owned(),
        Some(file) => format!("Editing {file}"),
        None => "Browsing files".to_owned(),
    };
    let mut assets = Assets::new().large_image(LOGO_URL).large_text("Ligma");
    let mut activity = Activity::new()
        .name("Ligma")
        .details(details)
        .timestamps(Timestamps::new().start(started));
    if let Some(tool) = &status.tool {
        let label = format!("{tool} tool");
        if let Some(icon) = tool_icon(tool) {
            let state = if status.focused { "focused" } else { "unfocused" };
            assets = assets
                .small_image(format!("{TOOLS_URL}/{state}/{icon}.png"))
                .small_text(label.clone());
        }
        activity = activity.state(label);
    }
    activity.assets(assets)
}

fn run(rx: Receiver<Status>) {
    let mut client: Option<DiscordIpcClient> = None;
    let mut retry_at = Instant::now();
    let mut wanted = Status::default();
    // What Discord shows. None after connecting, so it gets sent again.
    let mut shown: Option<Status> = None;
    let mut sent_at: Option<Instant> = None;
    // The elapsed time restarts with each file.
    let mut started = (None, now_ms());
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(status) => wanted = status,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(mut client) = client {
                    let _ = client.clear_activity();
                    let _ = client.close();
                }
                return;
            }
        }
        while let Ok(status) = rx.try_recv() {
            wanted = status;
        }
        if shown.as_ref() == Some(&wanted) || sent_at.is_some_and(|at| at.elapsed() < MIN_INTERVAL) {
            continue;
        }
        if client.is_none() {
            if Instant::now() < retry_at {
                continue;
            }
            let mut new = DiscordIpcClient::new(CLIENT_ID);
            if new.connect().is_err() {
                retry_at = Instant::now() + RETRY_AFTER;
                continue;
            }
            client = Some(new);
        }
        if started.0 != wanted.file {
            started = (wanted.file.clone(), now_ms());
        }
        let ok = client
            .as_mut()
            .is_some_and(|client| client.set_activity(activity(&wanted, started.1)).is_ok());
        sent_at = Some(Instant::now());
        if ok {
            shown = Some(wanted.clone());
        } else {
            // Discord quit. Reconnect on the next change or tick.
            client = None;
            shown = None;
        }
    }
}
