//! Split view: two file tabs of a window share its canvas, each in its own
//! half, under one set of Figma's panels and one toolbar, the focused tab's
//!
//! The focused tab's webview fills the window below the tab bar, as any
//! tab's does. The other tab's is sized so its canvas covers its half of
//! the focused tab's canvas exactly, its panels beyond it, so its rulers,
//! scrollbars and zoom to fit are its half's. Each is clipped to its part:
//! the other tab to its half, less the focused tab's toolbar and the menus
//! and tooltips it has open, and the focused tab to everything else. Figma
//! keeps its canvas between its panels, so the focused tab's canvas still
//! spans both halves. A tab resized as the two swap is panned to match
//! (`__ligmaSplitKeep` and `__ligmaSplitRestore`), so what it shows stays
//! put. So
//! there's one set of panels and one toolbar, the focused tab's, where they
//! always are, and the other tab's toolbar is hidden in its page. A click
//! in the other half reaches that tab's page, which tells Ligma
//! (`PAGE_JS`), and the two swap. The halves sit side by side, or stacked
//! when the canvas is no wider than 10:9. `layout` runs every
//! `LAYOUT_EVERY` while a window is split, to follow the toolbar, image bar,
//! menus and tooltips as they come and go. Those are cut out to their own
//! rounded shapes, so nothing of the page around them shows
//!
//! A gap between the halves shows the tab bar's page, which fills the
//! window behind the tabs while it's split, and draws the divider there
//! (`showDivider` in index.html). Dragging it sets how the canvas is shared
//! (`set_split_ratio` in tabs.rs)
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
/// (`__ligmaSplit`), which hides its toolbar and image bar; pressing in it
/// asks Ligma to focus it: through `window.open` on Windows, and on macOS
/// as an iframe load, which WKWebView reports to `on_navigation`
pub const PAGE_JS: &str = r#"(() => {
  if (window.top !== window) return;
  const style = document.createElement("style");
  style.textContent = `
    .ligma-split-other [data-testid="design-toolbelt-wrapper"],
    .ligma-split-other [data-secondary-toolbelt] { visibility: hidden !important; }
  `;
  const mount = () => (document.head ? (document.head.append(style), true) : false);
  if (!mount()) document.addEventListener("DOMContentLoaded", mount, { once: true });
  // Keeps what shows at the middle of `rect`, `[left, top, right,
  // bottom]` in CSS pixels, there while the page is resized, by panning
  // Figma's viewport once it has laid out its canvas again: the world point
  // there is kept first, then moved to the middle of the rect it takes after
  const figmaViewport = () => {
    try {
      return window.figma?.viewport ?? null;
    } catch {
      return null;
    }
  };
  const canvasMiddle = () => {
    let best = null;
    for (const c of document.querySelectorAll("canvas")) {
      const r = c.getBoundingClientRect();
      if (!best || r.width * r.height > best.width * best.height) best = r;
    }
    return best && [best.left + best.width / 2, best.top + best.height / 2];
  };
  const middle = (rect) => [(rect[0] + rect[2]) / 2, (rect[1] + rect[3]) / 2];
  let anchor = null;
  Object.defineProperty(window, "__ligmaSplitKeep", {
    value: (rect) => {
      const viewport = figmaViewport();
      const canvas = canvasMiddle();
      if (!viewport || !canvas) return (anchor = null);
      const [x, y] = middle(rect);
      const { center, zoom } = viewport;
      anchor = { x: center.x + (x - canvas[0]) / zoom, y: center.y + (y - canvas[1]) / zoom, size: [innerWidth, innerHeight] };
    },
  });
  Object.defineProperty(window, "__ligmaSplitRestore", {
    value: (rect) => {
      const kept = anchor;
      anchor = null;
      if (!kept) return;
      const started = performance.now();
      const pan = () => {
        const viewport = figmaViewport();
        const canvas = canvasMiddle();
        if (!viewport || !canvas) return;
        const [x, y] = middle(rect);
        viewport.center = { x: kept.x - (x - canvas[0]) / viewport.zoom, y: kept.y - (y - canvas[1]) / viewport.zoom };
      };
      // Once the page has its new size, and Figma a frame or two to follow
      const wait = () => {
        const resized = innerWidth !== kept.size[0] || innerHeight !== kept.size[1];
        if (!resized && performance.now() - started < 600) return requestAnimationFrame(wait);
        requestAnimationFrame(() => requestAnimationFrame(pan));
      };
      wait();
    },
  });
  let other = false;
  Object.defineProperty(window, "__ligmaSplit", {
    value: (value) => {
      other = value;
      document.documentElement.classList.toggle("ligma-split-other", value);
    },
  });
  addEventListener(
    "pointerdown",
    () => {
      if (!other) return;
      other = false;
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

/// Where Figma's canvas is, as `{ canvas, floating, size }` in CSS pixels, or
/// null outside an editor. The canvas is the largest `<canvas>`, as in
/// `CROP_JS` (tabs.rs), as `[left, top, right, bottom]`. What floats over it
/// is the toolbar, the bars above it, like the image bar, and open menus
/// and tooltips, each as `[left, top, right, bottom, corner radius]`. The
/// size is the page's
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
  const floating = [];
  const add = (el) => {
    const style = getComputedStyle(el);
    const e = el.getBoundingClientRect();
    if (style.display === "none" || style.visibility === "hidden" || !area(e)) return;
    if (area(e) > innerWidth * innerHeight * 0.4) return;
    floating.push([e.left, e.top, e.right, e.bottom, parseFloat(style.borderTopLeftRadius) || 0]);
  };
  const SHAPES = [
    '[data-testid="design-toolbelt-wrapper"] [role="toolbar"]',
    '[data-secondary-toolbelt="true"] [class*="secondary_toolbelt--secondaryToolbeltContainer"]',
    "[data-is-the-tooltip]",
    '[role="menu"]',
    '[role="listbox"]',
  ];
  for (const el of document.querySelectorAll(SHAPES.join(", "))) add(el);
  return { canvas: [left, top, right, bottom], floating, size: [innerWidth, innerHeight] };
})()"#;

/// The gap between the halves, where the divider shows, in CSS pixels
const GAP: f64 = 6.0;

/// How far either half may be dragged to: its share of the canvas
const RATIO: (f64, f64) = (0.15, 0.85);

/// `[left, top, right, bottom]`
pub type Rect = [f64; 4];

/// A rect with rounded corners: `[left, top, right, bottom, radius]`
pub type Shape = [f64; 5];

#[derive(serde::Deserialize)]
struct Geometry {
    canvas: Rect,
    floating: Vec<Shape>,
    size: [f64; 2],
}

/// What of a tab's webview shows: the union of `add` less `cut`, in its
/// logical pixels from its top left
#[derive(Clone, PartialEq)]
pub struct Region {
    pub add: Vec<Shape>,
    pub cut: Vec<Shape>,
}

impl Region {
    /// As rects alone, corners square
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn rects(&self) -> Vec<Rect> {
        let rect = |s: &Shape| [s[0], s[1], s[2], s[3]];
        let cut: Vec<Rect> = self.cut.iter().map(rect).collect();
        subtract_all(self.add.iter().map(rect).collect(), &cut)
    }

    fn scaled(&self, by: f64) -> Region {
        let scale = |shapes: &[Shape]| shapes.iter().map(|s| s.map(|v| v * by)).collect();
        Region { add: scale(&self.add), cut: scale(&self.cut) }
    }
}

fn square(r: Rect) -> Shape {
    [r[0], r[1], r[2], r[3], 0.0]
}

/// How often a split window's layout is checked
pub const LAYOUT_EVERY: std::time::Duration = std::time::Duration::from_millis(100);

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

/// Whether `canvas` splits side by side: unless it's no wider than 10:9
fn side_by_side(canvas: Rect) -> bool {
    (canvas[2] - canvas[0]) * 9.0 > (canvas[3] - canvas[1]) * 10.0
}

/// The two halves of `canvas`, first then second, the first taking `ratio`
/// of it, side by side or stacked, and the `gap` between them
pub fn halves(canvas: Rect, ratio: f64, gap: f64) -> ([Rect; 2], Rect) {
    let [l, t, r, b] = canvas;
    let ratio = ratio.clamp(RATIO.0, RATIO.1);
    if side_by_side(canvas) {
        let mid = l + (r - l) * ratio;
        let (a, z) = (mid - gap / 2.0, mid + gap / 2.0);
        ([[l, t, a, b], [z, t, r, b]], [a, t, z, b])
    } else {
        let mid = t + (b - t) * ratio;
        let (a, z) = (mid - gap / 2.0, mid + gap / 2.0);
        ([[l, t, r, a], [l, z, r, b]], [l, a, r, z])
    }
}

fn offset(r: Rect, by: (f64, f64)) -> Rect {
    [r[0] + by.0, r[1] + by.1, r[2] + by.0, r[3] + by.1]
}

fn same(a: &Rect, b: &Rect) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.5)
}

/// Where each split tab's webview is and what of it shows, in logical
/// pixels from the top left of where tabs go, by tab. A tab missing from
/// `BOUNDS` fills it, as any tab does
static BOUNDS: Mutex<Option<HashMap<u32, Rect>>> = Mutex::new(None);
static VISIBLE: Mutex<Option<HashMap<u32, Rect>>> = Mutex::new(None);

fn get(map: &Mutex<Option<HashMap<u32, Rect>>>, id: u32) -> Option<Rect> {
    map.lock().unwrap().as_ref()?.get(&id).copied()
}

fn set(map: &Mutex<Option<HashMap<u32, Rect>>>, id: u32, rect: Option<Rect>) {
    let mut map = map.lock().unwrap();
    let map = map.get_or_insert_with(HashMap::new);
    match rect {
        Some(rect) => map.insert(id, rect),
        None => map.remove(&id),
    };
}

/// True if tab `id`'s webview is placed by its split, not as any tab's
pub fn places(id: u32) -> bool {
    get(&BOUNDS, id).is_some()
}

/// Moves tab `id`'s `webview` to `to`, in logical pixels from the top left
/// of where tabs go, or back to filling it, keeping what shows at the
/// middle of `visible` there, from where it was
fn place(id: u32, webview: &Webview, to: Option<Rect>, visible: Rect) {
    let app = webview.app_handle().clone();
    let Ok(full) = crate::tabs::tab_bounds(&app, &webview.window()) else {
        return;
    };
    let zoom = crate::tabs::scale(&app);
    let top = crate::tabs::bar_height(&app);
    let from = get(&BOUNDS, id);
    set(&BOUNDS, id, to);
    let page = |at: Option<Rect>| offset(visible, at.map_or((0.0, 0.0), |at| (-at[0], -at[1]))).map(|v| v / zoom);
    let (before, after) = (page(from), page(to));
    let moved = webview.clone();
    let _ = webview.eval_with_callback(format!("window.__ligmaSplitKeep?.({before:?})"), move |_| {
        let bounds = match to {
            Some(r) => tauri::Rect {
                position: tauri::LogicalPosition::new(r[0], r[1] + top).into(),
                size: tauri::LogicalSize::new(r[2] - r[0], r[3] - r[1]).into(),
            },
            None => full,
        };
        let _ = moved.set_bounds(bounds);
        let _ = moved.eval(format!("window.__ligmaSplitRestore?.({after:?})"));
    });
}

/// Regions last applied, by tab, so an unchanged one isn't set again: that
/// repaints the page
static APPLIED: Mutex<Option<HashMap<u32, Option<Region>>>> = Mutex::new(None);

/// Clips tab `id`'s `webview` to `region`, or unclips it. On Windows with
/// its rounded corners; on macOS square
fn clip(id: u32, webview: &Webview, region: Option<Region>) {
    {
        let mut applied = APPLIED.lock().unwrap();
        let applied = applied.get_or_insert_with(HashMap::new);
        if applied.get(&id) == Some(&region) || (region.is_none() && !applied.contains_key(&id)) {
            return;
        }
        match &region {
            Some(_) => applied.insert(id, region.clone()),
            None => applied.remove(&id),
        };
    }
    #[cfg(windows)]
    {
        let factor = webview.window().scale_factor().unwrap_or(1.0);
        crate::webview2::clip(webview, region.map(|region| region.scaled(factor)));
    }
    #[cfg(target_os = "macos")]
    crate::wkwebview::clip(webview, region.map(|region| region.rects()));
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = (webview, region);
}

/// Unclips tab `id`, which left a split, fills where tabs go with it
/// again, and stops treating its page as the other half
pub fn unclip(id: u32, webview: &Webview) {
    clip(id, webview, None);
    if let Some(visible) = get(&VISIBLE, id).filter(|_| places(id)) {
        place(id, webview, None, visible);
    }
    set(&VISIBLE, id, None);
    let _ = webview.eval("window.__ligmaSplit?.(false)");
}

/// Lays out a split: `focused` and `other` are the tabs' ids, the other's
/// in the first half if `other_first`, and the first half takes `ratio` of
/// the canvas. Measures both tabs' pages first, then places and clips them
/// and has the tab bar draw the divider
pub fn layout(app: &AppHandle, focused: u32, other: u32, other_first: bool, ratio: f64) {
    let (Some(focused_view), Some(other_view)) =
        (crate::tabs::scriptable(app, focused), crate::tabs::scriptable(app, other))
    else {
        return;
    };
    let measured = focused_view.clone();
    let _ = measured.eval_with_callback(GEOMETRY_JS, move |json| {
        let focused_geometry = serde_json::from_str::<Option<Geometry>>(&json).ok().flatten();
        let (focused_view, other_view) = (focused_view.clone(), other_view.clone());
        let measured = other_view.clone();
        let _ = measured.eval_with_callback(GEOMETRY_JS, move |json| {
            let other_geometry = serde_json::from_str::<Option<Geometry>>(&json).ok().flatten();
            let tabs = [(focused, &focused_view), (other, &other_view)];
            let geometry = [focused_geometry.as_ref(), other_geometry.as_ref()];
            arrange(tabs, geometry, other_first, ratio);
        });
    });
}

/// `layout`, once both pages are measured: the focused tab's, then the
/// other's
fn arrange(tabs: [(u32, &Webview); 2], geometry: [Option<&Geometry>; 2], other_first: bool, ratio: f64) {
    let [(focused, focused_view), (other, other_view)] = tabs;
    let app = focused_view.app_handle().clone();
    let Ok(full) = crate::tabs::tab_bounds(&app, &focused_view.window()) else {
        return;
    };
    let size = match full.size {
        tauri::Size::Logical(size) => (size.width, size.height),
        tauri::Size::Physical(size) => {
            let factor = focused_view.window().scale_factor().unwrap_or(1.0);
            (size.width as f64 / factor, size.height as f64 / factor)
        }
    };
    let zoom = crate::tabs::scale(&app);
    // The tab that was the other one fills where tabs go again first, and
    // the split is laid out from it once it has
    if let Some(visible) = get(&VISIBLE, focused).filter(|_| places(focused)) {
        place(focused, focused_view, None, visible);
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(80));
            crate::tabs::refresh_splits(&app);
        });
        return;
    }
    let all = [0.0, 0.0, size.0, size.1];
    let canvas = geometry[0].map_or(all, |g| g.canvas.map(|v| v * zoom));
    let floating: Vec<Shape> = geometry[0].iter().flat_map(|g| g.floating.iter().map(|s| s.map(|v| v * zoom))).collect();
    let (halves, gap) = halves(canvas, ratio, GAP * zoom);
    let (theirs, mine) = if other_first { (halves[0], halves[1]) } else { (halves[1], halves[0]) };
    set(&VISIBLE, focused, Some(mine));
    // The other tab's canvas on its half, its panels around it
    let placed = get(&BOUNDS, other);
    let at = placed.unwrap_or(all);
    let wanted = geometry[1].map(|g| {
        let c = g.canvas.map(|v| v * zoom);
        let (w, h) = (g.size[0] * zoom, g.size[1] * zoom);
        [theirs[0] - c[0], theirs[1] - c[1], theirs[2] + (w - c[2]), theirs[3] + (h - c[3])].map(f64::round)
    });
    let at = match wanted {
        Some(wanted) if !same(&wanted, &at) || placed.is_none() => {
            let visible = get(&VISIBLE, other).unwrap_or(theirs);
            place(other, other_view, Some(wanted), visible);
            wanted
        }
        _ => at,
    };
    set(&VISIBLE, other, Some(theirs));
    let mine_region = Region {
        add: subtract_all(vec![all], &[theirs, gap]).into_iter().map(square).chain(floating.iter().copied()).collect(),
        cut: Vec::new(),
    };
    let local = |s: &Shape| [s[0] - at[0], s[1] - at[1], s[2] - at[0], s[3] - at[1], s[4]];
    let theirs_region = Region { add: vec![local(&square(theirs))], cut: floating.iter().map(local).collect() };
    clip(focused, focused_view, Some(mine_region));
    clip(other, other_view, Some(theirs_region));
    let _ = focused_view.eval("window.__ligmaSplit?.(false)");
    let _ = other_view.eval("window.__ligmaSplit?.(true)");
    // In the tab bar's CSS pixels: it's at the window's top, at the same
    // zoom, and the tabs are below it
    let bar = |r: Rect| [r[0] / zoom, r[1] / zoom + crate::tabs::TAB_BAR_HEIGHT, r[2] / zoom, r[3] / zoom + crate::tabs::TAB_BAR_HEIGHT];
    let divider = serde_json::json!({ "gap": bar(gap), "canvas": bar(canvas), "sideBySide": side_by_side(canvas) });
    if let Some(bar) = app.get_webview(focused_view.window().label()) {
        let _ = bar.eval(format!("window.showDivider?.({divider})"));
    }
}

/// Takes the divider away from window `window`'s tab bar, once it's not
/// split
pub fn hide_divider(app: &AppHandle, window: &str) {
    if let Some(bar) = app.get_webview(window) {
        let _ = bar.eval("window.hideDivider?.()");
    }
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
