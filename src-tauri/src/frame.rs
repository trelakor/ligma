//! Window borders of Ligma's own frameless windows (Windows only): the
//! settings window, the tab menu and prompts. Windows 11 outlines them in
//! the accent color when "Show accent color on title bars and window
//! borders" is on; Figma's windows have a quiet border in their theme
//! instead. Windows 10 has no border to color and ignores this.

use tauri::{AppHandle, Manager, WebviewWindow};

/// DWMWA_BORDER_COLOR
const BORDER_COLOR: u32 = 34;
/// DWMWA_COLOR_NONE: no border at all.
const COLOR_NONE: u32 = 0xFFFF_FFFE;

#[link(name = "dwmapi")]
extern "system" {
    fn DwmSetWindowAttribute(hwnd: isize, attribute: u32, value: *const core::ffi::c_void, size: u32) -> i32;
}

/// Gives `window` the active theme's border color, or none until there's a
/// theme.
pub fn match_theme(window: &WebviewWindow) {
    let theme = crate::tabs::theme(window.app_handle());
    let color = theme
        .as_ref()
        .and_then(|theme| crate::settings::theme_color(theme, "border"))
        // COLORREF: 0x00bbggrr.
        .map_or(COLOR_NONE, |color| u32::from_le_bytes([color.0, color.1, color.2, 0]));
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    unsafe {
        DwmSetWindowAttribute(hwnd.0 as isize, BORDER_COLOR, (&color as *const u32).cast(), 4);
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
