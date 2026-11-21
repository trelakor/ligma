//! WebKitGTK on Linux. Tauri packs every webview in a window into the
//! window's GtkBox there, which splits the window between them, and ignores
//! where they're put. So each webview is moved into a GtkLayout in that box
//! the first time it's placed, and placed there by hand. A GtkLayout rather
//! than a GtkFixed, as its size doesn't grow with what's in it, which would
//! keep the window from shrinking

use gtk::prelude::*;
use tauri::{Rect, Webview};

/// Puts `webview` at `bounds` in its window, as `Webview::set_bounds` does
/// on the other platforms. Also after a `reparent`, which puts the webview
/// back in its new window's box
pub fn set_bounds(webview: &Webview, bounds: Rect) -> tauri::Result<()> {
    let factor = webview.window().scale_factor()?;
    let position = bounds.position.to_logical::<f64>(factor);
    let size = bounds.size.to_logical::<f64>(factor);
    let (x, y) = (position.x.round() as i32, position.y.round() as i32);
    let (width, height) = ((size.width.round() as i32).max(1), (size.height.round() as i32).max(1));
    webview.with_webview(move |platform| {
        let widget = platform.inner();
        let Some(layout) = layout_of(&widget) else {
            return;
        };
        widget.set_size_request(width, height);
        layout.move_(&widget, x, y);
    })
}

/// The GtkLayout `widget` is placed in, moving it there from the window's
/// box if it isn't yet
fn layout_of(widget: &webkit2gtk::WebView) -> Option<gtk::Layout> {
    let parent = widget.parent()?;
    if let Ok(layout) = parent.clone().downcast::<gtk::Layout>() {
        return Some(layout);
    }
    let vbox = parent.downcast::<gtk::Box>().ok()?;
    let layout = vbox.children().into_iter().find_map(|child| child.downcast::<gtk::Layout>().ok()).unwrap_or_else(|| {
        let layout = gtk::Layout::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
        vbox.pack_start(&layout, true, true, 0);
        layout.show();
        layout
    });
    vbox.remove(widget);
    layout.put(widget, 0, 0);
    Some(layout)
}

/// How far in from the window's edges a drag resizes it, in logical pixels
const RESIZE_INSET: i32 = 5;

/// Lets the window be resized from its edges over tab `webview`, as Tauri
/// lets it over the tab bar. The window has no frame of its own
pub fn allow_resizing(webview: &Webview) -> tauri::Result<()> {
    use gtk::gdk::EventMask;
    use gtk::glib::Propagation;

    webview.with_webview(|platform| {
        let widget = platform.inner();
        widget.add_events(EventMask::POINTER_MOTION_MASK | EventMask::BUTTON_PRESS_MASK);
        widget.connect_motion_notify_event(|widget, event| {
            let Some((_, edge)) = edge_at(widget, event.root()) else {
                return Propagation::Proceed;
            };
            if let Some(window) = widget.window() {
                let cursor = gtk::gdk::Cursor::for_display(&window.display(), cursor_for(edge));
                window.set_cursor(cursor.as_ref());
            }
            Propagation::Stop
        });
        widget.connect_button_press_event(|widget, event| {
            if event.button() != 1 {
                return Propagation::Proceed;
            }
            let (root_x, root_y) = event.root();
            let Some((window, edge)) = edge_at(widget, (root_x, root_y)) else {
                return Propagation::Proceed;
            };
            window.begin_resize_drag(edge, 1, root_x as i32, root_y as i32, event.time());
            Propagation::Stop
        });
    })
}

/// The edge of `widget`'s frameless, resizable window at screen point
/// `(x, y)`, if it's within `RESIZE_INSET` of one
fn edge_at(widget: &webkit2gtk::WebView, (x, y): (f64, f64)) -> Option<(gtk::Window, gtk::gdk::WindowEdge)> {
    use gtk::gdk::WindowEdge;

    let window = widget.toplevel()?.downcast::<gtk::Window>().ok()?;
    if window.is_decorated() || !window.is_resizable() || window.is_maximized() {
        return None;
    }
    let gdk_window = window.window()?;
    let (left, top) = gdk_window.position();
    let (x, y) = (x - left as f64, y - top as f64);
    let (width, height) = (gdk_window.width() as f64, gdk_window.height() as f64);
    let inset = (gdk_window.scale_factor() * RESIZE_INSET) as f64;
    let (west, east) = (x < inset, x >= width - inset);
    let (north, south) = (y < inset, y >= height - inset);
    let edge = match (north, south, west, east) {
        (true, _, true, _) => WindowEdge::NorthWest,
        (true, _, _, true) => WindowEdge::NorthEast,
        (_, true, true, _) => WindowEdge::SouthWest,
        (_, true, _, true) => WindowEdge::SouthEast,
        (true, ..) => WindowEdge::North,
        (_, true, ..) => WindowEdge::South,
        (_, _, true, _) => WindowEdge::West,
        (_, _, _, true) => WindowEdge::East,
        _ => return None,
    };
    Some((window, edge))
}

fn cursor_for(edge: gtk::gdk::WindowEdge) -> gtk::gdk::CursorType {
    use gtk::gdk::{CursorType, WindowEdge};

    match edge {
        WindowEdge::NorthWest => CursorType::TopLeftCorner,
        WindowEdge::NorthEast => CursorType::TopRightCorner,
        WindowEdge::SouthWest => CursorType::BottomLeftCorner,
        WindowEdge::SouthEast => CursorType::BottomRightCorner,
        WindowEdge::North => CursorType::TopSide,
        WindowEdge::South => CursorType::BottomSide,
        WindowEdge::West => CursorType::LeftSide,
        _ => CursorType::RightSide,
    }
}
