//! Window frames of Ligma's own frameless windows (Windows only): the
//! settings window, the tab menu and prompts. Windows 11 outlines them in
//! the accent color when "Show accent color on title bars and window
//! borders" is on, and keeps a strip of frame a pixel or two tall above the
//! page in the caption color. Like Figma's windows, they get no border,
//! just the system shadow, and the strip takes the page's background so it
//! doesn't show. Windows 10 has neither and ignores this.

use tauri::{AppHandle, Manager, WebviewWindow};

/// DWMWA_BORDER_COLOR
const BORDER_COLOR: u32 = 34;
/// DWMWA_CAPTION_COLOR
const CAPTION_COLOR: u32 = 35;
/// DWMWA_COLOR_NONE: no border at all.
const COLOR_NONE: u32 = 0xFFFF_FFFE;
/// DWMWA_COLOR_DEFAULT: the system's color.
const COLOR_DEFAULT: u32 = 0xFFFF_FFFF;

#[link(name = "dwmapi")]
extern "system" {
    fn DwmSetWindowAttribute(hwnd: isize, attribute: u32, value: *const core::ffi::c_void, size: u32) -> i32;
}

/// Takes `window`'s border away and gives the strip above its page the
/// active theme's background.
pub fn match_theme(window: &WebviewWindow) {
    let theme = crate::tabs::theme(window.app_handle());
    let caption = theme
        .as_ref()
        .and_then(|theme| crate::settings::theme_color(theme, "bg"))
        // COLORREF: 0x00bbggrr.
        .map_or(COLOR_DEFAULT, |color| u32::from_le_bytes([color.0, color.1, color.2, 0]));
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    for (attribute, color) in [(BORDER_COLOR, COLOR_NONE), (CAPTION_COLOR, caption)] {
        unsafe {
            DwmSetWindowAttribute(hwnd.0 as isize, attribute, (&color as *const u32).cast(), 4);
        }
    }
}

/// `match_theme` for every window but the main one, after the theme changed.
pub fn match_theme_all(app: &AppHandle) {
    for (label, window) in app.webview_windows() {
        if label != "main" {
            match_theme(&window);
        }
    }
}
