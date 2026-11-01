//! Split view: two file tabs of a window share its canvas, each in its own
//! half, under one set of Figma's panels and one toolbar, the focused tab's
//!
//! Both tabs' webviews fill the window below the tab bar, as any tab's
//! does, and are clipped to their parts of it: the other tab to its half of
//! the focused tab's canvas, less the focused tab's toolbar and the menus
//! and tooltips it has open, and the focused tab to everything else. So
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
//! over either half of a window's canvas splits it with the tab there;
//! `show_hint` marks where it would go
//!
//! Objects dragged from one half into the other move files (`CARRY_JS`):
//! as the drag crosses over, the page they're in copies them, through
//! Figma's own copy, to a clipboard of its own rather than the system's,
//! and asks Ligma to `carry` them. The other page pastes them, by the same
//! means, focused, as Figma only pastes then. Until the button's let go,
//! both pages keep their copy under the pointer, Ligma telling them where
//! it is, so it shows on whichever side of the split it's over, and the
//! half the pointer's in is focused. Let go, the page whose half it's in
//! keeps it and the other deletes its own

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
  let other = false;
  // This page's half and the other's, `[left, top, right, bottom]` in CSS
  // pixels, while split
  let halves = null;
  Object.defineProperty(window, "__ligmaSplit", {
    value: (value, shared = null) => {
      other = value;
      halves = shared;
      document.documentElement.classList.toggle("ligma-split-other", value);
    },
  });
  Object.defineProperty(window, "__ligmaSplitHalves", { get: () => halves });
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

/// In every tab, after `PAGE_JS`: objects dragged across a split (see the
/// module's doc). Uses the Plugin API Figma exposes on the page, and its
/// `focus-target` input, which its copy and paste listen on
pub const CARRY_JS: &str = r#"(() => {
  if (window.top !== window) return;
  const api = () => {
    try {
      return window.figma?.currentPage && window.figma.editorType === "figma" ? window.figma : null;
    } catch {
      return null;
    }
  };
  const inside = (r, x, y) => r && x >= r[0] && x < r[2] && y >= r[1] && y < r[3];
  const canvasMiddle = () => {
    let best = null;
    for (const c of document.querySelectorAll("canvas")) {
      const r = c.getBoundingClientRect();
      if (!best || r.width * r.height > best.width * best.height) best = r;
    }
    return best && [best.left + best.width / 2, best.top + best.height / 2];
  };
  // The point of the canvas at `x`, `y` in CSS pixels
  const world = (figma, x, y) => {
    const middle = canvasMiddle();
    const { center, zoom } = figma.viewport;
    return middle && { x: center.x + (x - middle[0]) / zoom, y: center.y + (y - middle[1]) / zoom };
  };
  const box = (nodes) => {
    const boxes = nodes.map((n) => n.absoluteBoundingBox ?? { x: n.x, y: n.y, width: n.width, height: n.height });
    return { x: Math.min(...boxes.map((b) => b.x)), y: Math.min(...boxes.map((b) => b.y)) };
  };
  const clipboard = (type, data) => {
    const target = document.querySelector("input.focus-target");
    if (!target) return false;
    target.focus();
    return !target.dispatchEvent(new ClipboardEvent(type, { clipboardData: data, bubbles: true, cancelable: true }));
  };
  const signal = (path) => {
    const url = "https://split.ligma.invalid/" + path;
    if (navigator.userAgent.includes("Mac")) {
      const frame = document.createElement("iframe");
      frame.style.display = "none";
      frame.src = url;
      document.documentElement.appendChild(frame);
      setTimeout(() => frame.remove(), 1000);
    } else {
      window.open(url, "_blank");
    }
  };

  // Out: a press on something selected on the canvas, as Figma sees it,
  // starts a drag of the selection. Crossing into the other half carries it
  // over
  let press = null;
  let out = null;
  let held = false;
  addEventListener("pointerdown", (e) => {
    const figma = api();
    press = null;
    held = e.button === 0;
    if (e.button !== 0 || !figma || !inside(window.__ligmaSplitHalves?.mine, e.clientX, e.clientY)) return;
    const selection = figma.currentPage.selection;
    const pointer = world(figma, e.clientX, e.clientY);
    if (!pointer || !selection.length) return;
    // A frame at the top level is dragged by its name too, above it
    const label = 24 / figma.viewport.zoom;
    const on = (n) => {
      const b = n.absoluteBoundingBox;
      const above = n.parent?.type === "PAGE" && ["FRAME", "SECTION", "COMPONENT", "COMPONENT_SET"].includes(n.type) ? label : 0;
      return b && pointer.x >= b.x && pointer.x <= b.x + b.width && pointer.y >= b.y - above && pointer.y <= b.y + b.height;
    };
    press = { nodes: selection, at: box(selection), pointer, on: selection.some(on) };
  }, true);
  addEventListener("pointermove", (e) => {
    const figma = api();
    const halves = window.__ligmaSplitHalves;
    if (!press || !figma || !halves || !(e.buttons & 1)) return;
    if (!inside(halves.theirs, e.clientX, e.clientY)) return;
    const nodes = press.nodes.filter((n) => !n.removed);
    // Pressed on the selection, or Figma's moving it: not a selection box
    // or a pan
    const now = nodes.length && box(nodes);
    const moving = now && (Math.abs(now.x - press.at.x) > 0.01 || Math.abs(now.y - press.at.y) > 0.01);
    if (!press.on && !moving) return;
    const data = new DataTransfer();
    if (!nodes.length || !clipboard("copy", data) || !data.getData("text/html")) return;
    // Where it was grabbed, from where it was when pressed
    const offset = { x: press.at.x - press.pointer.x, y: press.at.y - press.pointer.y };
    out = { nodes, html: data.getData("text/html"), offset };
    press = null;
    signal("carry");
  }, true);
  addEventListener("pointerup", () => {
    press = null;
    held = false;
  }, true);
  Object.defineProperty(window, "__ligmaCarryOut", {
    value: () => out && { html: out.html, offset: out.offset },
  });
  // Puts `nodes` with `offset` from the point of the canvas at `x`, `y`
  const follow = (figma, nodes, offset, x, y) => {
    const pointer = world(figma, x, y);
    nodes = nodes.filter((n) => !n.removed);
    if (!pointer || !nodes.length) return;
    const now = box(nodes);
    const dx = pointer.x + offset.x - now.x;
    const dy = pointer.y + offset.y - now.y;
    if (Math.abs(dx) < 1e-6 && Math.abs(dy) < 1e-6) return;
    for (const node of nodes) {
      node.x += dx;
      node.y += dy;
    }
  };
  // Keeps what's carried out under the pointer too, for it coming back
  Object.defineProperty(window, "__ligmaCarryFollow", {
    value: (x, y) => {
      const figma = api();
      if (figma && out) follow(figma, out.nodes, out.offset, x, y);
    },
  });
  // Once they're in the other file, after Figma has finished the drag here:
  // it won't take them away mid-drag
  Object.defineProperty(window, "__ligmaCarryDone", {
    value: (moved) => {
      const nodes = out?.nodes ?? [];
      out = null;
      if (!moved) {
        const figma = api();
        const kept = nodes.filter((n) => !n.removed);
        if (figma && kept.length) figma.currentPage.selection = kept;
        return;
      }
      // Tried until they're gone: once the drag here has ended, or after a
      // while if this page never hears of the button going up
      const started = performance.now();
      const remove = () => {
        const waiting = held && performance.now() - started < 1500;
        if (!waiting) {
          for (const node of nodes) {
            try {
              if (!node.removed) node.remove();
            } catch {}
          }
        }
        if (nodes.some((n) => !n.removed) && performance.now() - started < 5000) setTimeout(remove, 100);
      };
      setTimeout(remove, 50);
    },
  });

  // In: pasted at the top level, then kept under the pointer. What came
  // in is what's new at the top level
  let carried = null;
  Object.defineProperty(window, "__ligmaCarryIn", {
    value: ({ html, offset }) => {
      const figma = api();
      carried = null;
      if (!figma) return;
      figma.currentPage.selection = [];
      const data = new DataTransfer();
      data.setData("text/html", html);
      data.setData("text/plain", "");
      const before = new Set(figma.currentPage.children.map((n) => n.id));
      // Figma doesn't always mark the paste as handled: what lands tells
      clipboard("paste", data);
      carried = { offset, before, nodes: null, at: null, started: performance.now() };
    },
  });
  // Pasting can take a moment
  const landed = (figma) => {
    if (!carried.nodes) {
      const fresh = figma.currentPage.children.filter((n) => !carried.before.has(n.id));
      if (fresh.length) carried.nodes = fresh;
    }
    return carried.nodes;
  };
  Object.defineProperty(window, "__ligmaCarryMove", {
    value: (x, y) => {
      const figma = api();
      if (!figma || !carried) return;
      carried.at = [x, y];
      if (landed(figma)) follow(figma, carried.nodes, carried.offset, x, y);
    },
  });
  // With `keep`, true if what was carried came in, false if not; without,
  // it's deleted again. Null while the paste is still landing, for a while
  Object.defineProperty(window, "__ligmaCarryEnd", {
    value: (keep) => {
      const figma = api();
      if (!carried) return false;
      // Figma's API can be out of reach for a moment while it pastes
      if (!figma) return performance.now() - carried.started < 3000 ? null : ((carried = null), false);
      if (!landed(figma)) {
        if (performance.now() - carried.started < 3000) return null;
      } else if (carried.at) {
        window.__ligmaCarryMove(...carried.at);
      }
      const nodes = carried.nodes?.filter((n) => !n.removed) ?? [];
      carried = null;
      if (!keep) {
        for (const node of nodes) node.remove();
        return false;
      }
      if (nodes.length) figma.currentPage.selection = nodes;
      return nodes.length > 0;
    },
  });
})();"#;

/// Where Figma's canvas is, as `{ canvas, floating }` in CSS pixels, or
/// null outside an editor. The canvas is the largest `<canvas>`, as in
/// `CROP_JS` (tabs.rs), as `[left, top, right, bottom]`. What floats over it
/// is the toolbar, the bars above it, like the image bar, and open menus
/// and tooltips, each as `[left, top, right, bottom, corner radius]`
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
  return { canvas: [left, top, right, bottom], floating };
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

/// What each of a split's tabs shows, in logical pixels from its top left,
/// the focused one's then the other's, and the gap: the other gets its half
/// of the canvas, less what of the focused tab's floats over it, and the
/// focused one the rest but the gap
fn regions(size: (f64, f64), geometry: Option<Geometry>, zoom: f64, ratio: f64, other_half: usize) -> (Region, Region, Rect, Rect) {
    let full = [0.0, 0.0, size.0, size.1];
    let canvas = geometry.as_ref().map_or(full, |g| g.canvas.map(|v| v * zoom));
    let floating: Vec<Shape> = geometry.iter().flat_map(|g| g.floating.iter().map(|s| s.map(|v| v * zoom))).collect();
    let (halves, gap) = halves(canvas, ratio, GAP * zoom);
    let half = halves[other_half];
    let theirs = Region { add: vec![square(half)], cut: floating.clone() };
    let around = subtract_all(vec![full], &[half, gap]);
    let mine = Region { add: around.into_iter().map(square).chain(floating).collect(), cut: Vec::new() };
    (mine, theirs, gap, canvas)
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

/// Unclips tab `id`, which left a split, and stops treating its page as
/// the other half
pub fn unclip(id: u32, webview: &Webview) {
    clip(id, webview, None);
    let _ = webview.eval("window.__ligmaSplit?.(false)");
}

/// Lays out a split: `focused` and `other` are the tabs' ids, the other's
/// in the first half if `other_first`, and the first half takes `ratio` of
/// the canvas. Measures the focused tab's page first, then has the tab bar
/// draw the divider
pub fn layout(app: &AppHandle, focused: u32, other: u32, other_first: bool, ratio: f64) {
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
        let other_half = if other_first { 0 } else { 1 };
        let (mine, theirs, gap, canvas) = regions((size.width, size.height), geometry, zoom, ratio, other_half);
        clip(focused, &focused_view, Some(mine));
        clip(other, &other_view, Some(theirs));
        // Each page's half and the other's, in its CSS pixels: the same for
        // both, as both fill where tabs go
        let (both, _) = halves(canvas, ratio, GAP * zoom);
        {
            let mut placed = HALVES.lock().unwrap();
            let placed = placed.get_or_insert_with(HashMap::new);
            placed.insert(other, both[other_half]);
            placed.insert(focused, both[1 - other_half]);
        }
        let both = both.map(|half| half.map(|v| v / zoom));
        let shared = |mine: usize| serde_json::json!({ "mine": both[mine], "theirs": both[1 - mine] });
        let _ = focused_view.eval(format!("window.__ligmaSplit?.(false, {})", shared(1 - other_half)));
        let _ = other_view.eval(format!("window.__ligmaSplit?.(true, {})", shared(other_half)));
        // In the tab bar's CSS pixels: it's at the window's top, at the same
        // zoom, and the tabs are below it
        let bar = |r: Rect| [r[0] / zoom, r[1] / zoom + crate::tabs::TAB_BAR_HEIGHT, r[2] / zoom, r[3] / zoom + crate::tabs::TAB_BAR_HEIGHT];
        let divider = serde_json::json!({ "gap": bar(gap), "canvas": bar(canvas), "sideBySide": side_by_side(canvas) });
        if let Some(bar) = focused_view.app_handle().get_webview(focused_view.window().label()) {
            let _ = bar.eval(format!("window.showDivider?.({divider})"));
        }
    });
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
    let app = app.clone();
    match url.path() {
        "/focus" => {
            std::thread::spawn(move || {
                let _ = crate::tabs::select(&app, tab);
            });
        }
        "/carry" => {
            std::thread::spawn(move || carry(&app, tab));
        }
        _ => {}
    }
}

/// Carries what's being dragged in tab `from` into the other tab of its
/// split, which keeps it under the pointer until the button's let go (see
/// `CARRY_JS`)
fn carry(app: &AppHandle, from: u32) {
    let Some(to) = crate::tabs::split_partner(app, from) else {
        return;
    };
    let (Some(source), Some(target)) = (crate::tabs::scriptable(app, from), crate::tabs::scriptable(app, to)) else {
        return;
    };
    // Through a channel: the page's answer comes on the main thread
    let (tx, rx) = std::sync::mpsc::channel();
    let _ = source.eval_with_callback("window.__ligmaCarryOut?.() ?? null", move |json| {
        let _ = tx.send(json);
    });
    let Ok(json) = rx.recv_timeout(std::time::Duration::from_secs(2)) else {
        return;
    };
    if json == "null" || json.is_empty() {
        return;
    }
    // Figma only takes a paste with the page focused: the other half is
    // focused as the drag crosses into it
    let _ = crate::tabs::select(app, to);
    let _ = target.eval(format!("window.__ligmaCarryIn?.({json})"));
    let window = target.window();
    let zoom = crate::tabs::scale(app);
    let top = crate::tabs::bar_height(app);
    let (from_half, to_half) = (half_of(from), half_of(to));
    let mut last = None;
    // In the target's half, or else the source's
    let mut there = true;
    while crate::tabs::mouse_down() {
        let (Ok(at), Ok(origin), Ok(factor)) = (app.cursor_position(), window.inner_position(), window.scale_factor()) else {
            break;
        };
        if last != Some(at) {
            last = Some(at);
            // Where tabs go, in logical pixels, then in the pages' CSS pixels
            let (lx, ly) = ((at.x - origin.x as f64) / factor, (at.y - origin.y as f64) / factor - top);
            let (x, y) = (lx / zoom, ly / zoom);
            let inside = |half: Option<Rect>| half.is_some_and(|r| lx >= r[0] && lx < r[2] && ly >= r[1] && ly < r[3]);
            let now = if inside(to_half) {
                true
            } else if inside(from_half) {
                false
            } else {
                there
            };
            if now != there {
                there = now;
                let _ = crate::tabs::select(app, if there { to } else { from });
            }
            let _ = target.eval(format!("window.__ligmaCarryMove?.({x}, {y})"));
            let _ = source.eval(format!("window.__ligmaCarryFollow?.({x}, {y})"));
        }
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    // Asked again while the paste is still landing
    let moved = loop {
        let (tx, rx) = std::sync::mpsc::channel();
        // Not `?? false`: null is the page's "not yet"
        let script = format!("window.__ligmaCarryEnd ? window.__ligmaCarryEnd({there}) : false");
        let _ = target.eval_with_callback(script, move |json| {
            let _ = tx.send(json);
        });
        match rx.recv_timeout(std::time::Duration::from_secs(2)).as_deref() {
            Ok("null") => std::thread::sleep(std::time::Duration::from_millis(50)),
            Ok(answer) => break answer == "true",
            Err(_) => break false,
        }
    };
    let _ = source.eval(format!("window.__ligmaCarryDone?.({moved})"));
    let _ = crate::tabs::select(app, if moved { to } else { from });
}

/// Where tab `id`'s half of its split was last laid out, in logical pixels
/// from the top left of where tabs go
fn half_of(id: u32) -> Option<Rect> {
    HALVES.lock().unwrap().as_ref()?.get(&id).copied()
}

/// Each split tab's half, as `layout` last placed it
static HALVES: Mutex<Option<HashMap<u32, Rect>>> = Mutex::new(None);
