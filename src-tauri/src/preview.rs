//! Tab previews, like the Figma app's: resting the pointer on a tab shows
//! its last image and its name in a card under it (`tabPreviews`). The card
//! is a small frameless window (`dist/preview.html`), made on first use and
//! hidden in between. It never takes focus and lets the mouse through to
//! the page below.
//!
//! The tab bar asks for a card with `show_tab_preview` and drops it with
//! `hide_tab_preview`. Like the tab menu, the page then fetches what to
//! draw (`get_tab_preview`) and reports its size (`tab_preview_ready`);
//! only then is the window placed under the tab and shown. Images come
//! from `tabs::capture`, taken while a tab was last on screen.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Webview, WebviewUrl, WebviewWindowBuilder};

pub const WINDOW: &str = "tab-preview";

/// Space between the tab bar and the card, in CSS pixels.
const GAP: f64 = 4.0;

/// The card being opened or shown.
struct Open {
    tab: u32,
    /// Tells a page's answer for this card from one for an earlier card.
    seq: u64,
    /// The tab's left edge in the tab bar, in CSS pixels.
    left: f64,
}

static OPEN: Mutex<Option<Open>> = Mutex::new(None);
static SEQ: AtomicU64 = AtomicU64::new(0);

/// Opens the card of tab `tab`, whose left edge is `left` CSS pixels into
/// the tab bar.
fn show(app: &AppHandle, tab: u32, left: f64) -> tauri::Result<()> {
    if !crate::tabs::can_preview(app, tab) {
        hide(app);
        return Ok(());
    }
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    *OPEN.lock().unwrap() = Some(Open { tab, seq, left });
    if let Some(window) = app.get_webview_window(WINDOW) {
        window.set_zoom(crate::tabs::scale(app))?;
        return window.eval("window.refresh?.()");
    }
    // The page fetches the card once it has loaded.
    let main = app.get_window("main").expect("main window exists");
    #[allow(unused_mut)]
    let mut builder = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("preview.html".into()))
        .title("Tab Preview")
        .inner_size(264.0, 220.0)
        .visible(false)
        .focused(false)
        .focusable(false)
        .decorations(false)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .skip_taskbar(true);
    // Windows gives a window the mouse goes through neither rounded corners
    // nor a shadow, so there it's see-through and the page draws both
    // (`drawn` in preview.html). It's also kept above the main window, and
    // closed with it.
    #[cfg(windows)]
    {
        builder = builder.transparent(true).shadow(false).owner_raw(main.hwnd()?);
    }
    #[cfg(not(windows))]
    {
        let color = crate::tabs::theme(app).and_then(|theme| crate::settings::theme_color(&theme, "bg"));
        if let Some(color) = color {
            builder = builder.background_color(color);
        }
    }
    #[cfg(target_os = "macos")]
    {
        builder = builder.shadow(true).parent_raw(main.ns_window()?);
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = main;
    let window = builder.build()?;
    // Clicks and hovers go to whatever is under the card.
    window.set_ignore_cursor_events(true)?;
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
    Ok(())
}

/// Hides the card, if shown.
pub fn hide(app: &AppHandle) {
    if OPEN.lock().unwrap().take().is_none() {
        return;
    }
    if let Some(window) = app.get_webview_window(WINDOW) {
        #[cfg(windows)]
        crate::frame::show_inactive(&window, false);
        #[cfg(not(windows))]
        let _ = window.hide();
    }
}

/// Closes the card's window, for when the main window goes.
pub fn close(app: &AppHandle) {
    OPEN.lock().unwrap().take();
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.destroy();
    }
}

fn from_tab_bar(webview: &Webview) -> bool {
    webview.label() == "main"
}

fn from_preview(webview: &Webview) -> bool {
    webview.label() == WINDOW
}

/// The tab bar's pointer rests on tab `id`, `left` CSS pixels into the bar.
#[tauri::command]
pub async fn show_tab_preview(app: AppHandle, webview: Webview, id: u32, left: f64) {
    if from_tab_bar(&webview) {
        let _ = show(&app, id, left);
    }
}

#[tauri::command]
pub async fn hide_tab_preview(app: AppHandle, webview: Webview) {
    if from_tab_bar(&webview) {
        hide(&app);
    }
}

/// The pointer came into the tab bar, likely on its way to another tab:
/// keep an image of the tab it would leave.
#[tauri::command]
pub async fn capture_active_tab(app: AppHandle, webview: Webview) {
    if from_tab_bar(&webview) {
        crate::tabs::capture_active(&app);
    }
}

#[derive(Serialize)]
pub struct Card {
    seq: u64,
    #[serde(flatten)]
    tab: crate::tabs::Preview,
    theme: Option<serde_json::Value>,
}

/// The card to draw, if one is being opened.
#[tauri::command]
pub async fn get_tab_preview(app: AppHandle, webview: Webview) -> Option<Card> {
    if !from_preview(&webview) {
        return None;
    }
    let (tab, seq) = OPEN.lock().unwrap().as_ref().map(|open| (open.tab, open.seq))?;
    let Some(preview) = crate::tabs::preview(&app, tab) else {
        hide(&app);
        return None;
    };
    // A zoom set before the page loaded doesn't last.
    let _ = webview.set_zoom(crate::tabs::scale(&app));
    Some(Card { seq, tab: preview, theme: crate::tabs::theme(&app) })
}

/// The page has drawn card `seq` at `width` by `height` CSS pixels, `inset`
/// pixels into the page on every side (room for the shadow it draws on
/// Windows): place the card under its tab, inside the main window, and
/// show it.
#[tauri::command]
pub async fn tab_preview_ready(app: AppHandle, webview: Webview, seq: u64, width: f64, height: f64, inset: f64) {
    if !from_preview(&webview) {
        return;
    }
    let Some(left) = OPEN.lock().unwrap().as_ref().filter(|open| open.seq == seq).map(|open| open.left) else {
        return;
    };
    let (Some(main), Some(window)) = (app.get_window("main"), app.get_webview_window(WINDOW)) else {
        return;
    };
    let (Ok(origin), Ok(size), Ok(scale)) = (main.inner_position(), main.inner_size(), main.scale_factor()) else {
        return;
    };
    // CSS pixels at the interface scale, to screen pixels.
    let px = scale * crate::tabs::scale(&app);
    let (w, h) = ((width * px).round(), (height * px).round());
    let margin = (GAP * px).round();
    let right = origin.x as f64 + size.width as f64 - margin - w;
    let x = (origin.x as f64 + left * px).min(right).max(origin.x as f64 + margin);
    let y = origin.y as f64 + ((crate::tabs::TAB_BAR_HEIGHT + GAP) * px).round();
    let inset = (inset * px).round();
    let _ = window.set_size(PhysicalSize::new((w + 2.0 * inset) as u32, (h + 2.0 * inset) as u32));
    let _ = window.set_position(PhysicalPosition::new((x.round() - inset) as i32, (y - inset) as i32));
    #[cfg(windows)]
    crate::frame::show_inactive(&window, true);
    #[cfg(not(windows))]
    let _ = window.show();
}
