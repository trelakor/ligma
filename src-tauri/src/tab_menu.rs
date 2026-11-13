//! The tabs' right-click menu, drawn by Ligma in Figma's style
//! (`dist/menu.html`) rather than the system's. It's a small frameless
//! window, made on first use and hidden in between, so later menus open
//! right away
//!
//! `show` keeps the items and has the page `refresh`. The page fetches
//! them (`get_menu`), draws them, and reports its size (`menu_ready`);
//! only then is the window placed at the cursor and shown. It hides when
//! it loses focus, and `menu_pick` runs the chosen item through the
//! menu's `Pick`: `tabs::on_menu` for a tab's menu. Toolbar modes (see
//! toolbar.rs) use it too, with checked items, and image adjustments (see
//! adjustments.rs), with shortcuts and items that can't be picked

use std::sync::Mutex;

use serde::Serialize;
use tauri::{
    AppHandle, LogicalSize, Manager, PhysicalPosition, Webview, WebviewUrl, WebviewWindow, WebviewWindowBuilder, Window,
    WindowEvent,
};

pub const WINDOW: &str = "tab-menu";

/// A menu item: the action `tabs::on_menu` runs, and its text. None is a
/// separator
pub type Item = Option<(&'static str, &'static str)>;

/// Runs a menu's chosen item: the tab it was opened for and the action
pub type Pick = fn(&AppHandle, u32, &str);

/// What a menu shows besides its items
#[derive(Clone, Default, Serialize)]
pub struct Extras {
    /// The actions checked, if its items can be
    pub checked: Option<Vec<&'static str>>,
    /// The actions shown but not offered
    pub disabled: Vec<&'static str>,
    /// `[action, shortcut]`, shown right of the item
    pub shortcuts: Vec<(&'static str, &'static str)>,
}

/// The menu being opened or shown
struct Open {
    tab: u32,
    items: Vec<Item>,
    extras: Extras,
    pick: Pick,
    /// The cursor when it was asked for, in screen pixels
    at: PhysicalPosition<f64>,
    /// The window it's for, which it's kept above
    owner: Window,
}

static OPEN: Mutex<Option<Open>> = Mutex::new(None);

/// Opens a menu for tab `tab` at the cursor. With `extras.checked`, every
/// item can be checked, and those it names are
pub fn show(app: &AppHandle, tab: u32, items: Vec<Item>, extras: Extras, pick: Pick) -> tauri::Result<()> {
    // The menu takes the tab preview's place
    crate::preview::hide(app);
    let at = app.cursor_position()?;
    let Some(owner) = crate::tabs::window_of(app, tab).or_else(|| crate::tabs::focused_window(app)) else {
        return Ok(());
    };
    *OPEN.lock().unwrap() = Some(Open { tab, items, extras, pick, at, owner: owner.clone() });
    if let Some(window) = app.get_webview_window(WINDOW) {
        window.set_zoom(crate::tabs::scale(app))?;
        return window.eval("window.refresh?.()");
    }
    // The page fetches the items once it has loaded
    let main = owner;
    #[allow(unused_mut)]
    let mut builder = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("menu.html".into()))
        .title("Tab Menu")
        .inner_size(200.0, 100.0)
        .visible(false)
        .decorations(false)
        // Rounded corners on Windows 11, like Figma's menus
        .shadow(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .skip_taskbar(true);
    // Light or dark with the editor. Figma's own menus stay dark in light
    // themes, which looks out of place on a light tab bar
    let color = crate::tabs::theme(app).and_then(|theme| crate::settings::theme_color(&theme, "bg"));
    if let Some(color) = color {
        builder = builder.background_color(color);
    }
    // Kept above the window it's for, and closed with it on Windows
    #[cfg(windows)]
    {
        builder = builder.owner_raw(main.hwnd()?);
    }
    // Not a child window on macOS until shown (`set_owner`): AppKit puts a
    // child on screen as soon as it's attached, before it's been placed
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
                unsafe { round_corners(ns_window.cast()) };
            }
        })?;
    }
    let handle = app.clone();
    let menu = window.clone();
    window.on_window_event(move |event| {
        // Only a shown menu: the new window can lose focus before it's
        // shown, while the page is still asking for its items
        if let WindowEvent::Focused(false) = event {
            if menu.is_visible().unwrap_or(false) {
                hide(&handle);
            }
        }
    });
    Ok(())
}

/// True while a menu is being opened or shown
pub fn is_open() -> bool {
    OPEN.lock().unwrap().is_some()
}

/// Hides the menu, if shown
pub fn hide(app: &AppHandle) {
    OPEN.lock().unwrap().take();
    if let Some(window) = app.get_webview_window(WINDOW) {
        release(&window);
        let _ = window.hide();
    }
}

/// Closes the menu window, for when the main window goes
pub fn close(app: &AppHandle) {
    OPEN.lock().unwrap().take();
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.destroy();
    }
}

/// Clips a borderless window to Figma's menu radius; AppKit gives it a
/// matching shadow once it's see-through outside the corners
#[cfg(target_os = "macos")]
pub unsafe fn round_corners(ns_window: *mut objc2::runtime::AnyObject) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    let _: () = msg_send![ns_window, setOpaque: false];
    let clear: *mut AnyObject = msg_send![class!(NSColor), clearColor];
    let _: () = msg_send![ns_window, setBackgroundColor: clear];
    let view: *mut AnyObject = msg_send![ns_window, contentView];
    if view.is_null() {
        return;
    }
    let _: () = msg_send![view, setWantsLayer: true];
    let layer: *mut AnyObject = msg_send![view, layer];
    if !layer.is_null() {
        let _: () = msg_send![layer, setCornerRadius: 12.0f64];
        let _: () = msg_send![layer, setMasksToBounds: true];
    }
    let _: () = msg_send![ns_window, setHasShadow: true];
}

fn from_menu(webview: &Webview) -> bool {
    webview.label() == WINDOW
}

#[derive(Serialize)]
pub struct Menu {
    /// `[action, text]`, or null for a separator
    items: Vec<Item>,
    #[serde(flatten)]
    extras: Extras,
    theme: Option<serde_json::Value>,
}

/// The menu to draw, if one is being opened
#[tauri::command]
pub async fn get_menu(app: AppHandle, webview: Webview) -> Option<Menu> {
    if !from_menu(&webview) {
        return None;
    }
    let (items, extras) = OPEN.lock().unwrap().as_ref().map(|open| (open.items.clone(), open.extras.clone()))?;
    // A zoom set before the page loaded doesn't last
    let _ = webview.set_zoom(crate::tabs::scale(&app));
    Some(Menu { items, extras, theme: crate::tabs::theme(&app) })
}

/// The page has drawn the menu at `width` by `height` CSS pixels: place it
/// at the cursor, inside the screen, and show it
#[tauri::command]
pub async fn menu_ready(app: AppHandle, webview: Webview, width: f64, height: f64) {
    if !from_menu(&webview) {
        return;
    }
    let Some((at, owner)) = OPEN.lock().unwrap().as_ref().map(|open| (open.at, open.owner.clone())) else {
        return;
    };
    let Some(window) = app.get_webview_window(WINDOW) else {
        return;
    };
    // At the interface scale
    let zoom = crate::tabs::scale(&app);
    let (width, height) = (width * zoom, height * zoom);
    let monitor = app.monitor_from_point(at.x, at.y).ok().flatten();
    let scale = monitor.as_ref().map_or(1.0, |monitor| monitor.scale_factor());
    let (w, h) = (width * scale, height * scale);
    let (mut x, mut y) = (at.x, at.y);
    if let Some(monitor) = &monitor {
        let area = monitor.work_area();
        let (left, top) = (area.position.x as f64, area.position.y as f64);
        let (right, bottom) = (left + area.size.width as f64, top + area.size.height as f64);
        // Like system menus: to the cursor's left or above it when there's
        // no room right of or below it
        if x + w > right {
            x = (x - w).max(left);
        }
        if y + h > bottom {
            y = (y - h).max(top);
        }
    }
    // No accent-colored border
    #[cfg(windows)]
    crate::frame::match_theme(&window);
    let _ = window.set_size(LogicalSize::new(width, height));
    let _ = window.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
    set_owner(&window, &owner);
    let _ = window.show();
    let _ = window.set_focus();
}

/// Keeps `window`, the tab menu or preview, above `owner` from now on, for
/// a tab in another window than the last one
pub fn set_owner(window: &WebviewWindow, owner: &Window) {
    #[cfg(windows)]
    crate::frame::set_owner(window, owner);
    #[cfg(target_os = "macos")]
    {
        let (child, parent) = (window.clone(), owner.clone());
        let _ = window.run_on_main_thread(move || {
            if let (Ok(child), Ok(parent)) = (child.ns_window(), parent.ns_window()) {
                unsafe { set_parent(child.cast(), parent.cast()) };
            }
        });
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = (window, owner);
}

/// Undoes `set_owner`, for a window about to be hidden. On macOS a hidden
/// child window comes back on screen whenever its parent is ordered in, as
/// when the app is switched back to, so it can't stay a child while hidden
pub fn release(window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        let child = window.clone();
        let _ = window.run_on_main_thread(move || {
            if let Ok(child) = child.ns_window() {
                unsafe { remove_parent(child.cast()) };
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = window;
}

/// Call on the main thread
#[cfg(target_os = "macos")]
unsafe fn remove_parent(ns_window: *mut objc2::runtime::AnyObject) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    let current: *mut AnyObject = msg_send![ns_window, parentWindow];
    if !current.is_null() {
        let _: () = msg_send![current, removeChildWindow: ns_window];
    }
}

/// Makes `ns_window` a child window of `parent`, unless it is already.
/// Call on the main thread
#[cfg(target_os = "macos")]
unsafe fn set_parent(ns_window: *mut objc2::runtime::AnyObject, parent: *mut objc2::runtime::AnyObject) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    let current: *mut AnyObject = msg_send![ns_window, parentWindow];
    if current == parent {
        return;
    }
    if !current.is_null() {
        let _: () = msg_send![current, removeChildWindow: ns_window];
    }
    // NSWindowAbove
    let _: () = msg_send![parent, addChildWindow: ns_window, ordered: 1isize];
}

/// Runs `action` from the menu, or just closes it when None (Esc)
#[tauri::command]
pub async fn menu_pick(app: AppHandle, webview: Webview, action: Option<String>) {
    if !from_menu(&webview) {
        return;
    }
    let open = OPEN.lock().unwrap().take();
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.hide();
    }
    let (Some(open), Some(action)) = (open, action) else {
        return;
    };
    // Only the actions this menu offered
    if open.items.iter().flatten().any(|(item, _)| *item == action) && !open.extras.disabled.contains(&action.as_str()) {
        (open.pick)(&app, open.tab, &action);
    }
}
