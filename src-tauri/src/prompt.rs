//! Prompts (join requests, updates, errors) drawn by Ligma in Figma's style
//! (`dist/prompt.html`) rather than as system message boxes. Each is a small
//! frameless window centered over the main window, like the tab menu: the
//! page fetches its text (`get_prompt`), reports its size (`prompt_ready`)
//! and is shown, then sends the chosen button (`prompt_answer`). Closing it
//! any other way answers None

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, LogicalSize, Manager, PhysicalPosition, Webview, WebviewUrl, WebviewWindowBuilder, WindowEvent};

const LABEL_PREFIX: &str = "prompt-";

type Done = Box<dyn FnOnce(Option<usize>) + Send>;

#[derive(Clone, Serialize)]
struct Content {
    title: String,
    message: String,
    /// Left to right; the last is the default
    buttons: Vec<String>,
}

struct Open {
    content: Content,
    done: Option<Done>,
    /// Centered and shown; a rescale keeps it where it is
    placed: bool,
}

static OPEN: Mutex<Option<HashMap<String, Open>>> = Mutex::new(None);
static NEXT: AtomicU32 = AtomicU32::new(0);

/// Shows `message` under `title` with `buttons`, left to right, the last
/// being the default. `done` gets the index of the one chosen, or None if
/// the prompt was dismissed (Esc, or the app closing)
pub fn show(
    app: &AppHandle,
    title: &str,
    message: impl Into<String>,
    buttons: &[&str],
    done: impl FnOnce(Option<usize>) + Send + 'static,
) {
    let label = format!("{LABEL_PREFIX}{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let content = Content {
        title: title.to_owned(),
        message: message.into(),
        buttons: buttons.iter().map(|&button| button.to_owned()).collect(),
    };
    OPEN.lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(label.clone(), Open { content, done: Some(Box::new(done)), placed: false });
    // Off the caller's thread: making a window on the main thread, from a
    // webview's callback, would wait on itself
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(error) = build(&app, &label) {
            log::error!("Couldn't show a prompt: {error}");
            answer(&app, &label, None);
        }
    });
}

fn build(app: &AppHandle, label: &str) -> tauri::Result<()> {
    let title = OPEN
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|open| open.get(label))
        .map(|open| open.content.title.clone())
        .unwrap_or_default();
    let main = app.get_window("main");
    #[allow(unused_mut)]
    let mut builder = WebviewWindowBuilder::new(app, label, WebviewUrl::App("prompt.html".into()))
        .title(title)
        .inner_size(360.0, 140.0)
        .visible(false)
        .decorations(false)
        // Rounded corners and a shadow on Windows 11
        .shadow(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .skip_taskbar(main.is_some());
    if let Some(color) = crate::tabs::theme(app).as_ref().and_then(|theme| crate::settings::theme_color(theme, "bg")) {
        builder = builder.background_color(color);
    }
    // Kept above the main window
    if let Some(main) = &main {
        #[cfg(windows)]
        {
            builder = builder.owner_raw(main.hwnd()?);
        }
        #[cfg(target_os = "macos")]
        {
            builder = builder.parent_raw(main.ns_window()?);
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        let _ = main;
    }
    let window = builder.build()?;
    #[cfg(windows)]
    {
        crate::frame::refresh(&window);
        crate::frame::match_theme(&window);
    }
    #[cfg(target_os = "macos")]
    {
        let ns_window = window.clone();
        window.run_on_main_thread(move || {
            if let Ok(ns_window) = ns_window.ns_window() {
                unsafe { crate::tab_menu::round_corners(ns_window.cast()) };
            }
        })?;
    }
    let label = label.to_owned();
    window.on_window_event(move |event| {
        if let WindowEvent::Destroyed = event {
            finish(&label, None);
        }
    });
    Ok(())
}

/// Runs the prompt's `done` with `choice`, once, and closes its window
fn answer(app: &AppHandle, label: &str, choice: Option<usize>) {
    if let Some(window) = app.get_webview_window(label) {
        let _ = window.destroy();
    }
    finish(label, choice);
}

/// Runs the prompt's `done` with `choice`, unless it has already run
fn finish(label: &str, choice: Option<usize>) {
    let done = OPEN
        .lock()
        .unwrap()
        .as_mut()
        .and_then(|open| open.remove(label))
        .and_then(|open| open.done);
    if let Some(done) = done {
        done(choice);
    }
}

/// Dismisses every open prompt, for when the main window goes
pub fn close_all(app: &AppHandle) {
    let labels: Vec<String> = OPEN.lock().unwrap().iter().flat_map(|open| open.keys().cloned()).collect();
    for label in labels {
        answer(app, &label, None);
    }
}

/// Passes the active tab's new theme to open prompts
pub fn apply_theme(app: &AppHandle, theme: &serde_json::Value) {
    for (label, window) in app.webview_windows() {
        if label.starts_with(LABEL_PREFIX) {
            let _ = window.eval(format!("window.applyTheme?.({theme})"));
        }
    }
}

/// Rescales open prompts to the interface scale
pub fn apply_scale(app: &AppHandle, scale: f64) {
    for (label, window) in app.webview_windows() {
        if label.starts_with(LABEL_PREFIX) {
            let _ = window.set_zoom(scale);
            let _ = window.eval("window.measure?.()");
        }
    }
}

fn is_prompt(webview: &Webview) -> bool {
    webview.label().starts_with(LABEL_PREFIX)
}

#[derive(Serialize)]
pub struct Prompt {
    #[serde(flatten)]
    content: Content,
    theme: Option<serde_json::Value>,
}

/// What this prompt says
#[tauri::command]
pub async fn get_prompt(app: AppHandle, webview: Webview) -> Option<Prompt> {
    if !is_prompt(&webview) {
        return None;
    }
    let content = OPEN.lock().unwrap().as_ref()?.get(webview.label())?.content.clone();
    // Zoomed now, before the page measures: a zoom set before it loaded
    // doesn't last
    let _ = webview.set_zoom(crate::tabs::scale(&app));
    Some(Prompt { content, theme: crate::tabs::theme(&app) })
}

/// The page has drawn the prompt at `width` by `height` CSS pixels: size
/// it, center it over the main window (or the screen) and show it
#[tauri::command]
pub async fn prompt_ready(app: AppHandle, webview: Webview, width: f64, height: f64) {
    if !is_prompt(&webview) {
        return;
    }
    let Some(window) = app.get_webview_window(webview.label()) else {
        return;
    };
    let scale = crate::tabs::scale(&app);
    let size = LogicalSize::new(width * scale, height * scale);
    let _ = window.set_size(size);
    // Only placed the first time; a rescale keeps it where it is
    let first = OPEN
        .lock()
        .unwrap()
        .as_mut()
        .and_then(|open| open.get_mut(webview.label()))
        .is_some_and(|open| !std::mem::replace(&mut open.placed, true));
    if first {
        let factor = window.scale_factor().unwrap_or(1.0);
        let (w, h) = (size.width * factor, size.height * factor);
        let area = match app.get_window("main") {
            Some(main) if !main.is_minimized().unwrap_or(false) => {
                main.outer_position().ok().zip(main.outer_size().ok())
            }
            _ => None,
        };
        let area = area.or_else(|| {
            let monitor = window.current_monitor().ok().flatten()?;
            let area = monitor.work_area();
            Some((area.position, area.size))
        });
        if let Some((position, outer)) = area {
            let x = position.x as f64 + (outer.width as f64 - w) / 2.0;
            // A little above the middle, like system alerts
            let y = position.y as f64 + (outer.height as f64 - h) / 3.0;
            let _ = window.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
        }
        let _ = window.show();
    }
    let _ = window.set_focus();
}

/// The user chose button `index`, or dismissed the prompt (None)
#[tauri::command]
pub async fn prompt_answer(app: AppHandle, webview: Webview, index: Option<usize>) {
    if is_prompt(&webview) {
        answer(&app, webview.label(), index);
    }
}
