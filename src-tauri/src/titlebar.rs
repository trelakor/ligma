//! Traffic light placement (macOS only).
//!
//! Tauri's `traffic_light_position` has no effect here: tao and wry apply it
//! from a `drawRect:` that never runs once webviews cover the window, and
//! AppKit keeps the buttons a fixed distance from the window's top edge
//! anyway, undoing any frame change. What AppKit does support is a taller
//! title bar: with an empty toolbar in the compact style, it centers the
//! buttons 19pt from the top, the middle of the 38pt tab bar. The toolbar is
//! transparent and lets clicks through to the tab bar.

use objc2::runtime::AnyObject;
use objc2::{class, msg_send};

/// NSWindowToolbarStyleUnifiedCompact
const UNIFIED_COMPACT: isize = 4;

/// Lowers the traffic lights of `ns_window` to the middle of the tab bar.
/// Call once, on the main thread, with a live NSWindow.
pub unsafe fn center_traffic_lights(ns_window: *mut AnyObject) {
    let identifier: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: c"ligma".as_ptr()];
    let toolbar: *mut AnyObject = msg_send![class!(NSToolbar), alloc];
    let toolbar: *mut AnyObject = msg_send![toolbar, initWithIdentifier: identifier];
    if toolbar.is_null() {
        return;
    }
    let _: () = msg_send![ns_window, setToolbar: toolbar];
    let _: () = msg_send![ns_window, setToolbarStyle: UNIFIED_COMPACT];
    // The window keeps the toolbar.
    let _: () = msg_send![toolbar, release];
}
