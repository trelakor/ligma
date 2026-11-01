//! Split view: two file tabs of a window share its canvas, each in its own
//! half, under one set of Figma's panels and one toolbar, the focused tab's
//!
//! Both tabs' webviews fill the window below the tab bar, as any tab's
//! does, and are clipped to their parts of it: the other tab to its half of
//! the focused tab's canvas, less the toolbar floating over it, and the
//! focused tab to everything else. So the other tab's panels and toolbar
//! are clipped away, and the focused tab's show where they always do. A
//! click in the other half reaches that tab's page, which tells Ligma
//! (`PAGE_JS`), and the two swap. The halves sit side by side, or stacked
//! when the canvas is no wider than 10:9
//!
//! Clipping takes a window region on Windows, which clips clicks too. On
//! macOS it takes a layer mask, with `hitTest:` patched to match (see
//! wkwebview.rs)
//!
//! Which tabs are split lives with the tabs (see tabs.rs). A tab dragged
//! over the far half of a window's canvas splits it with the tab there;
//! `show_hint` marks where it would go

use std::collections::HashMap;
use std::sync::Mutex;

use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Url, Webview, WebviewUrl, WebviewWindowBuilder, Window};

pub const SIGNAL_HOST: &str = "split.ligma.invalid";

/// In every tab. Ligma marks the tab in the half that isn't focused
/// (`__ligmaSplitOther`); pressing in it asks Ligma to focus it: through
/// `window.open` on Windows, and on macOS as an iframe load, which WKWebView
/// reports to `on_navigation`
pub const PAGE_JS: &str = r#"(() => {
  addEventListener(
    "pointerdown",
    () => {
      if (!window.__ligmaSplitOther) return;
      window.__ligmaSplitOther = false;
      const url = "https://split.ligma.invalid/focus";
      if (navigator.userAgent.includes("Mac")) {
        const frame = document.createElement("iframe");
        frame.style.display = "none";
        frame.src = url;
        document.documentElement.appendChild(frame);
        setTimeout(() => frame.remove(), 1000);
      } else {
        window.open(url, "_blank");
      }
    },
    true,
  );
})();"#;

/// Where Figma's canvas is, and what floats over the middle of its bottom
/// edge: the toolbar, and any bar above it (the image bar, for one), as
/// `{ canvas, toolbar }`, each `[left, top, right, bottom]` in CSS pixels
/// (toolbar null if there's none). Null outside an editor. The canvas is
/// the largest `<canvas>`, as in `CROP_JS` (tabs.rs)
const GEOMETRY_JS: &str = r#"(() => {
  const area = (r) => r.width * r.height;
  let canvas = null, r = null;
  for (const c of document.querySelectorAll("canvas")) {
    const b = c.getBoundingClientRect();
    if (!r || area(b) > area(r)) [canvas, r] = [c, b];
  }
  if (!canvas || area(r) < innerWidth * innerHeight * 0.2) return null;
  const left = Math.max(0, r.left), top = Math.max(0, r.top);
  const right = Math.min(innerWidth, r.right), bottom = Math.min(innerHeight, r.bottom);
  const spans = (el) => {
    if (el === canvas || el.contains(canvas)) return true;
    const e = el.getBoundingClientRect();
    return e.width >= (right - left) * 0.8;
  };
  const cx = (left + right) / 2;
  let box = null, open = 0;
  for (let y = bottom - 2; y > bottom - 240 && open < 40; y -= 4) {
    const el = document.elementFromPoint(cx, y);
    if (!el || spans(el)) {
      if (box) open += 4;
      continue;
    }
    open = 0;
    let node = el;
    while (node.parentElement && !spans(node.parentElement)) node = node.parentElement;
    const e = node.getBoundingClientRect();
    box = box
      ? [Math.min(box[0], e.left), Math.min(box[1], e.top), Math.max(box[2], e.right), Math.max(box[3], e.bottom)]
      : [e.left, e.top, e.right, e.bottom];
  }
  return { canvas: [left, top, right, bottom], toolbar: box };
})()"#;

/// Room left around the toolbar, in CSS pixels
const TOOLBAR_MARGIN: f64 = 6.0;

/// `[left, top, right, bottom]`
pub type Rect = [f64; 4];

#[derive(Clone, Copy, serde::Deserialize)]
struct Geometry {
    canvas: Rect,
    toolbar: Option<Rect>,
}

fn is_empty(r: &Rect) -> bool {
    r[2] <= r[0] || r[3] <= r[1]
}

/// What's left of `a` without `b`, as up to four rects
fn subtract(a: Rect, b: Rect) -> Vec<Rect> {
    let overlap = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    if is_empty(&overlap) {
        return vec![a];
    }
    let parts = [
        [a[0], a[1], a[2], overlap[1]],
        [a[0], overlap[3], a[2], a[3]],
        [a[0], overlap[1], overlap[0], overlap[3]],
        [overlap[2], overlap[1], a[2], overlap[3]],
    ];
    parts.into_iter().filter(|part| !is_empty(part)).collect()
}

fn subtract_all(rects: Vec<Rect>, cut: &[Rect]) -> Vec<Rect> {
    cut.iter().fold(rects, |rects, &b| rects.into_iter().flat_map(|a| subtract(a, b)).collect())
}

/// The two halves of `canvas`, first then second: side by side, or stacked
/// when it's no wider than 10:9
pub fn halves(canvas: Rect) -> [Rect; 2] {
    let [l, t, r, b] = canvas;
    if (r - l) * 9.0 <= (b - t) * 10.0 {
        let mid = (t + b) / 2.0;
        [[l, t, r, mid], [l, mid, r, b]]
    } else {
        let mid = (l + r) / 2.0;
        [[l, t, mid, b], [mid, t, r, b]]
    }
}

/// The parts of a tab's webview each of a split's tabs shows, in logical
/// pixels: the focused one, then the other, in the `other_half`
fn regions(size: (f64, f64), geometry: Option<Geometry>, zoom: f64, other_half: usize) -> (Vec<Rect>, Vec<Rect>) {
    let full = [0.0, 0.0, size.0, size.1];
    let scaled = |r: Rect| r.map(|v| v * zoom);
    let canvas = geometry.map_or(full, |g| scaled(g.canvas));
    let toolbar = geometry.and_then(|g| g.toolbar).map(|t| {
        let m = TOOLBAR_MARGIN;
        scaled([t[0] - m, t[1] - m, t[2] + m, t[3] + m])
    });
    let other = subtract_all(vec![halves(canvas)[other_half]], toolbar.as_slice());
    let focused = subtract_all(vec![full], &other);
    (focused, other)
}

/// Regions last applied, by tab, so an unchanged one isn't set again: that
/// repaints the page
static APPLIED: Mutex<Option<HashMap<u32, Option<Vec<Rect>>>>> = Mutex::new(None);

/// Clips tab `id`'s `webview` to `rects`, in logical pixels, or unclips it
fn clip(id: u32, webview: &Webview, rects: Option<Vec<Rect>>) {
    {
        let mut applied = APPLIED.lock().unwrap();
        let applied = applied.get_or_insert_with(HashMap::new);
        if applied.get(&id) == Some(&rects) || (rects.is_none() && !applied.contains_key(&id)) {
            return;
        }
        match &rects {
            Some(_) => applied.insert(id, rects.clone()),
            None => applied.remove(&id),
        };
    }
    #[cfg(windows)]
    {
        let factor = webview.window().scale_factor().unwrap_or(1.0);
        crate::webview2::clip(webview, rects.map(|rects| rects.into_iter().map(|r| r.map(|v| v * factor)).collect()));
    }
    #[cfg(target_os = "macos")]
    crate::wkwebview::clip(webview, rects);
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = (webview, rects);
}

/// Unclips tab `id`, which left a split, and stops treating its page as
/// the other half
pub fn unclip(id: u32, webview: &Webview) {
    clip(id, webview, None);
    let _ = webview.eval("window.__ligmaSplitOther = false");
}

/// Lays out a split: `focused` and `other` are the tabs' ids, and the
/// other's in the first half if `other_first`. Measures the focused tab's
/// canvas first
pub fn layout(app: &AppHandle, focused: u32, other: u32, other_first: bool) {
    let (Some(focused_view), Some(other_view)) =
        (crate::tabs::scriptable(app, focused), crate::tabs::scriptable(app, other))
    else {
        return;
    };
    let zoom = crate::tabs::scale(app);
    let _ = focused_view.clone().eval_with_callback(GEOMETRY_JS, move |json| {
        let geometry = serde_json::from_str::<Option<Geometry>>(&json).ok().flatten();
        let Ok(size) = focused_view.size() else {
            return;
        };
        let factor = focused_view.window().scale_factor().unwrap_or(1.0);
        let size = size.to_logical::<f64>(factor);
        let (mine, theirs) = regions((size.width, size.height), geometry, zoom, if other_first { 0 } else { 1 });
        clip(focused, &focused_view, Some(mine));
        clip(other, &other_view, Some(theirs));
        let _ = focused_view.eval("window.__ligmaSplitOther = false");
        let _ = other_view.eval("window.__ligmaSplitOther = true");
    });
}

/// The canvas of the active tab of `window`, in screen pixels, for a tab
/// dragged over it. Waits for the page, briefly
pub fn canvas_on_screen(app: &AppHandle, window: &Window, active: u32) -> Option<Rect> {
    let webview = crate::tabs::scriptable(app, active)?;
    let (tx, rx) = std::sync::mpsc::channel();
    webview
        .eval_with_callback(GEOMETRY_JS, move |json| {
            let _ = tx.send(serde_json::from_str::<Option<Geometry>>(&json).ok().flatten());
        })
        .ok()?;
    let geometry = rx.recv_timeout(std::time::Duration::from_millis(400)).ok()??;
    let origin = window.inner_position().ok()?;
    let factor = window.scale_factor().ok()?;
    let px = factor * crate::tabs::scale(app);
    let top = origin.y as f64 + crate::tabs::bar_height(app) * factor;
    let [l, t, r, b] = geometry.canvas;
    Some([origin.x as f64 + l * px, top + t * px, origin.x as f64 + r * px, top + b * px])
}

/// The window marking where a dragged tab would go in a split
const HINT: &str = "split-hint";

/// Marks `rect`, in screen pixels, over `owner`, as where a dragged tab
/// would go
pub fn show_hint(app: &AppHandle, owner: &Window, rect: Rect) {
    let window = match app.get_webview_window(HINT) {
        Some(window) => window,
        None => {
            #[allow(unused_mut)]
            let mut builder = WebviewWindowBuilder::new(app, HINT, WebviewUrl::App("split-hint.html".into()))
                .title("Split View")
                .visible(false)
                .focused(false)
                .focusable(false)
                .decorations(false)
                .resizable(false)
                .skip_taskbar(true)
                .shadow(false);
            // See-through on Windows; macOS needs private API for that, so
            // there the window is faded instead
            #[cfg(windows)]
            {
                builder = builder.transparent(true);
            }
            #[cfg(not(windows))]
            {
                builder = builder.background_color(tauri::window::Color(13, 153, 255, 255));
            }
            let Ok(window) = builder.build() else {
                return;
            };
            let _ = window.set_ignore_cursor_events(true);
            #[cfg(windows)]
            crate::frame::refresh(&window);
            #[cfg(target_os = "macos")]
            {
                let faded = window.clone();
                let _ = window.run_on_main_thread(move || {
                    if let Ok(ns_window) = faded.ns_window() {
                        unsafe {
                            let _: () = objc2::msg_send![ns_window as *mut objc2::runtime::AnyObject, setAlphaValue: 0.3f64];
                        }
                    }
                });
            }
            window
        }
    };
    let [l, t, r, b] = rect;
    let _ = window.set_position(PhysicalPosition::new(l.round() as i32, t.round() as i32));
    let _ = window.set_size(PhysicalSize::new((r - l).round().max(1.0) as u32, (b - t).round().max(1.0) as u32));
    crate::tab_menu::set_owner(&window, owner);
    #[cfg(windows)]
    crate::frame::show_inactive(&window, true);
    #[cfg(not(windows))]
    let _ = window.show();
}

pub fn hide_hint(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(HINT) {
        #[cfg(windows)]
        crate::frame::show_inactive(&window, false);
        #[cfg(not(windows))]
        let _ = window.hide();
    }
}

/// Closes the hint's window, for when the main window goes
pub fn close(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(HINT) {
        let _ = window.destroy();
    }
}

/// A tab's page asks to be focused: it's the other half of a split
pub fn on_signal(app: &AppHandle, tab: u32, url: &Url) {
    if url.path() == "/focus" {
        let app = app.clone();
        std::thread::spawn(move || {
            let _ = crate::tabs::select(&app, tab);
        });
    }
}
