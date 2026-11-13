//! A file's emoji, picked under its tab. Figma shows a file name's leading
//! emoji ("🎨 Thumbnails") as the file's icon, and so does the tab bar, so
//! picking one renames the file with it in front, and removing it renames
//! the file without
//!
//! The tab bar opens the picker (`show_emoji_picker`) from the tab's menu or
//! a click on the active tab's icon. Like the tab menu, it's a small
//! frameless window (`dist/emoji.html`), made on first use and hidden in
//! between: the page fetches the tab's emoji (`get_emoji_picker`), reports
//! its size (`emoji_picker_ready`), and is placed under the tab. The choice
//! comes back through `emoji_pick`
//!
//! Figma has no public way to rename a file, so the page renames it through
//! the same API Figma's own file name field uses, from a Figma tab, and
//! retitles the file's tabs. Meanwhile the tab bar shows the new emoji, and
//! goes back if the rename fails

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, LogicalSize, Manager, PhysicalPosition, Webview, WebviewUrl, WebviewWindowBuilder, Window, WindowEvent,
};

pub const WINDOW: &str = "emoji-picker";

/// Space between the tab bar and the picker, in CSS pixels
const GAP: f64 = 4.0;

/// How long a rename may take before Ligma gives up on hearing back
const RENAME_TIMEOUT: Duration = Duration::from_secs(10);

/// Renames file `KEY` with `EMOJI` in front of its name, or without the
/// emoji it has when `EMOJI` is "", and leaves `{ status, name }` in
/// `__ligmaRenames[SEQ]`: the HTTP status (0 if the request failed) and the
/// new name. Reads the name from Figma
/// rather than the tab, whose title may be stale. A file with branches is
/// named by its repo, like Figma's own rename does. The leading emoji is
/// found as in the tab bar (`splitEmoji` in dist/index.html)
const RENAME_JS: &str = r#"(async (key, emoji, seq) => {
  const done = (status, name = null) => ((window.__ligmaRenames ??= {})[seq] = { status, name });
  try {
    const got = await fetch(`/api/files/${key}`);
    if (!got.ok) return done(got.status);
    const body = await got.json();
    const file = body.meta?.file ?? body.meta;
    const name = file.name;
    const first = new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(name)[Symbol.iterator]().next().value?.segment ?? "";
    const after = name.slice(first.length).trimStart();
    const rest = /\p{Emoji_Presentation}|\ufe0f|\p{Regional_Indicator}/u.test(first) && after ? after : name;
    const next = emoji ? `${emoji} ${rest}` : rest;
    if (next === name) return done(200, name);
    const path = file.file_repo_id ? `/api/repo/${file.file_repo_id}` : `/api/files/${key}`;
    const put = await fetch(path, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name: next }),
    });
    done(put.status, next);
  } catch {
    done(0);
  }
})"#;

/// What `RENAME_JS` leaves for Ligma
#[derive(Deserialize)]
struct Renamed {
    status: u16,
    name: Option<String>,
}

/// The picker being opened or shown
struct Open {
    tab: u32,
    /// The tab's left edge in the tab bar, in CSS pixels
    left: f64,
    /// The file's emoji now, or ""
    current: String,
    /// The tab's window, which it's kept above
    owner: Window,
}

static OPEN: Mutex<Option<Open>> = Mutex::new(None);
static SEQ: AtomicU64 = AtomicU64::new(0);

/// The tab whose picker a click outside it last closed, and when. A click
/// on the tab's icon closes it before asking for it again: that's a toggle
static CLOSED: Mutex<Option<(u32, Instant)>> = Mutex::new(None);

/// Opens the picker of tab `tab`, whose left edge is `left` CSS pixels into
/// the tab bar and whose file has emoji `current`
fn show(app: &AppHandle, tab: u32, left: f64, current: String) -> tauri::Result<()> {
    if crate::tabs::file_key_of(app, tab).is_none() {
        return Ok(());
    }
    crate::preview::hide(app);
    crate::tab_menu::hide(app);
    let Some(owner) = crate::tabs::window_of(app, tab) else {
        return Ok(());
    };
    *OPEN.lock().unwrap() = Some(Open { tab, left, current, owner: owner.clone() });
    if let Some(window) = app.get_webview_window(WINDOW) {
        window.set_zoom(crate::tabs::scale(app))?;
        return window.eval("window.refresh?.()");
    }
    // The page asks for the tab's emoji once it has loaded
    let main = owner;
    #[allow(unused_mut)]
    let mut builder = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("emoji.html".into()))
        .title("Emoji")
        .inner_size(288.0, 360.0)
        .visible(false)
        .decorations(false)
        // Rounded corners on Windows 11, like Figma's menus
        .shadow(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .skip_taskbar(true);
    let color = crate::tabs::theme(app).and_then(|theme| crate::settings::theme_color(&theme, "bg"));
    if let Some(color) = color {
        builder = builder.background_color(color);
    }
    // Kept above the window it's for, and closed with it on Windows
    #[cfg(windows)]
    {
        builder = builder.owner_raw(main.hwnd()?);
    }
    // Not a child window on macOS until shown, see tab_menu.rs
    #[cfg(not(windows))]
    let _ = main;
    let window = builder.build()?;
    #[cfg(windows)]
    crate::frame::refresh(&window);
    #[cfg(target_os = "macos")]
    {
        let ns_window = window.clone();
        window.run_on_main_thread(move || {
            if let Ok(ns_window) = ns_window.ns_window() {
                unsafe { crate::tab_menu::round_corners(ns_window.cast()) };
            }
        })?;
    }
    let handle = app.clone();
    let picker = window.clone();
    window.on_window_event(move |event| {
        // Only once shown: the new window can lose focus while loading
        if let WindowEvent::Focused(false) = event {
            if picker.is_visible().unwrap_or(false) {
                let tab = OPEN.lock().unwrap().as_ref().map(|open| open.tab);
                *CLOSED.lock().unwrap() = tab.map(|tab| (tab, Instant::now()));
                hide(&handle);
            }
        }
    });
    Ok(())
}

/// True while the picker is being opened or shown
pub fn is_open() -> bool {
    OPEN.lock().unwrap().is_some()
}

/// Hides the picker, if shown
pub fn hide(app: &AppHandle) {
    OPEN.lock().unwrap().take();
    if let Some(window) = app.get_webview_window(WINDOW) {
        crate::tab_menu::release(&window);
        let _ = window.hide();
    }
}

/// Closes the picker's window, for when the main window goes
pub fn close(app: &AppHandle) {
    OPEN.lock().unwrap().take();
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.destroy();
    }
}

fn from_picker(webview: &Webview) -> bool {
    webview.label() == WINDOW
}

/// An emoji the page may send: a short run of text with no spaces, not
/// all ASCII (keycaps like 1️⃣ start with a digit)
fn is_emoji(text: &str) -> bool {
    text.len() <= 32
        && !text.is_ascii()
        && !text.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Renames tab `tab`'s file with `emoji` in front, or without its emoji
/// when "". Runs on its own thread: it waits on Figma
fn rename(app: AppHandle, tab: u32, emoji: String) {
    let Some(key) = crate::tabs::file_key_of(&app, tab) else {
        return;
    };
    crate::tabs::show_pending_emoji(&app, tab, Some(&emoji));
    // The tab itself, or any Figma tab if it's asleep: they share a sign-in
    let webview = crate::tabs::scriptable(&app, tab).or_else(|| {
        crate::tabs::scriptable_tabs(&app)
            .into_iter()
            .find(|webview| webview.url().is_ok_and(|url| url.scheme() == "https" && url.host_str() == Some("www.figma.com")))
    });
    let Some(webview) = webview else {
        return failed(&app, tab, None);
    };
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let args = serde_json::to_string(&(&key, &emoji, seq)).unwrap();
    let args = &args[1..args.len() - 1];
    if webview.eval(format!("{RENAME_JS}({args})")).is_err() {
        return failed(&app, tab, None);
    }
    // The page's answer, once it has one. Async results don't come back
    // through `eval_with_callback`, so it's read again until there
    let started = Instant::now();
    let renamed = loop {
        std::thread::sleep(Duration::from_millis(150));
        if started.elapsed() > RENAME_TIMEOUT {
            break None;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let read = format!("window.__ligmaRenames?.[{seq}] ?? null");
        if webview.eval_with_callback(read, move |json| drop(tx.send(json))).is_err() {
            break None;
        }
        let reply = rx.recv_timeout(Duration::from_secs(2)).ok();
        match reply.and_then(|json| serde_json::from_str::<Option<Renamed>>(&json).ok()) {
            Some(Some(renamed)) => break Some(renamed),
            Some(None) => continue,
            None => break None,
        }
    };
    let _ = webview.eval(format!("delete window.__ligmaRenames?.[{seq}]"));
    match renamed {
        Some(Renamed { status: 200..=299, name: Some(name) }) => crate::tabs::renamed(&app, &key, &name),
        renamed => failed(&app, tab, renamed.map(|renamed| renamed.status)),
    }
}

/// The rename didn't happen: the tab shows the file's emoji again, and a
/// prompt says why
fn failed(app: &AppHandle, tab: u32, status: Option<u16>) {
    crate::tabs::show_pending_emoji(app, tab, None);
    let message = match status {
        Some(401 | 403) => "Only people who can edit this file can change its emoji",
        Some(404) => "Figma couldn't find this file. It may have been moved or deleted",
        _ => "Figma didn't take the new name. Check your connection and try again",
    };
    crate::prompt::show(app, "Couldn't change the emoji", message, &["OK"], |_| {});
}

/// The tab bar asks for tab `id`'s picker: its left edge is `left` CSS
/// pixels into the bar, and its file's emoji is `current` ("" for none)
#[tauri::command]
pub async fn show_emoji_picker(app: AppHandle, webview: Webview, id: u32, left: f64, current: String) {
    if !crate::tabs::from_tab_bar(&webview) || !left.is_finite() {
        return;
    }
    let toggled = CLOSED.lock().unwrap().take().is_some_and(|(tab, at)| tab == id && at.elapsed() < Duration::from_millis(400));
    if !toggled {
        let _ = show(&app, id, left, current);
    }
}

#[derive(Serialize)]
pub struct Picker {
    current: String,
    theme: Option<serde_json::Value>,
}

/// What the picker shows, if it's being opened
#[tauri::command]
pub async fn get_emoji_picker(app: AppHandle, webview: Webview) -> Option<Picker> {
    if !from_picker(&webview) {
        return None;
    }
    let current = OPEN.lock().unwrap().as_ref()?.current.clone();
    // A zoom set before the page loaded doesn't last
    let _ = webview.set_zoom(crate::tabs::scale(&app));
    Some(Picker { current, theme: crate::tabs::theme(&app) })
}

/// The page has drawn the picker at `width` by `height` CSS pixels: place
/// it under its tab, inside the tab's window where it fits, and show it
#[tauri::command]
pub async fn emoji_picker_ready(app: AppHandle, webview: Webview, width: f64, height: f64) {
    if !from_picker(&webview) {
        return;
    }
    let Some((left, main)) = OPEN.lock().unwrap().as_ref().map(|open| (open.left, open.owner.clone())) else {
        return;
    };
    let Some(window) = app.get_webview_window(WINDOW) else {
        return;
    };
    let (Ok(origin), Ok(size), Ok(scale)) = (main.inner_position(), main.inner_size(), main.scale_factor()) else {
        return;
    };
    // CSS pixels at the interface scale, to screen pixels
    let zoom = crate::tabs::scale(&app);
    let px = scale * zoom;
    let w = (width * px).round();
    let margin = (GAP * px).round();
    let right = origin.x as f64 + size.width as f64 - margin - w;
    let x = (origin.x as f64 + left * px).min(right).max(origin.x as f64 + margin);
    let y = origin.y as f64 + ((crate::tabs::TAB_BAR_HEIGHT + GAP) * px).round();
    // No accent-colored border
    #[cfg(windows)]
    crate::frame::match_theme(&window);
    let _ = window.set_size(LogicalSize::new(width * zoom, height * zoom));
    let _ = window.set_position(PhysicalPosition::new(x.round() as i32, y as i32));
    crate::tab_menu::set_owner(&window, &main);
    let _ = window.show();
    let _ = window.set_focus();
}

/// The picker's choice: an emoji, "" to remove the file's, or None to close
#[tauri::command]
pub async fn emoji_pick(app: AppHandle, webview: Webview, emoji: Option<String>) {
    if !from_picker(&webview) {
        return;
    }
    let open = OPEN.lock().unwrap().take();
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.hide();
    }
    let (Some(open), Some(emoji)) = (open, emoji) else {
        return;
    };
    if emoji == open.current || !(emoji.is_empty() || is_emoji(&emoji)) {
        return;
    }
    std::thread::spawn(move || rename(app, open.tab, emoji));
}
