//! Window frames of Ligma's own frameless windows (Windows only): the
//! settings window, the tab menu, tab previews and prompts. Windows 11
//! outlines them in the accent color when "Show accent color on title bars
//! and window borders" is on, and keeps a strip of frame a pixel or two
//! tall above the page in the caption color. Like Figma's windows, they get
//! no border, just the system shadow, and the strip takes the page's
//! background so it doesn't show. Windows 10 has neither and ignores this.

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

#[link(name = "user32")]
extern "system" {
    fn SetWindowPos(hwnd: isize, after: isize, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
    fn ShowWindow(hwnd: isize, command: i32) -> i32;
}

const SW_HIDE: i32 = 0;
const SW_SHOWNOACTIVATE: i32 = 4;

const SWP_NOSIZE: u32 = 0x0001;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOACTIVATE: u32 = 0x0010;
const SWP_FRAMECHANGED: u32 = 0x0020;

/// Has Windows work out `window`'s frame again. A window made hidden keeps
/// the default frame, title bar and all, until it's first shown, so a size
/// set meanwhile comes out a title bar too tall. Call right after building
/// a hidden frameless window.
pub fn refresh(window: &WebviewWindow) {
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    unsafe {
        SetWindowPos(
            hwnd.0 as isize,
            0,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// Shows or hides `window` without activating it, so the window that has
/// focus keeps it. Tauri's `show` only does that for a new window.
pub fn show_inactive(window: &WebviewWindow, shown: bool) {
    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    unsafe {
        ShowWindow(hwnd.0 as isize, if shown { SW_SHOWNOACTIVATE } else { SW_HIDE });
    }
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
