//! Frame colors (Settings > Features > Layers): each frame in the layers
//! panel gets a color, the rainbow over again down the list, red, orange,
//! yellow, green, blue and purple, and everything in it a lighter tint of
//! it, each frame inside a step darker, so it's clear where one frame, or
//! a frame in it, starts and ends. Sections and groups hold frames without
//! being one, so the frames in them each start a color of their own. Off
//! by default
//!
//! The panel draws only the rows in view, each with its layer's ID, so
//! `PAGE_JS` colors those as they're drawn, reading the frames each layer
//! is in from Figma's plugin API. Selected rows keep Figma's own highlight

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::settings::Settings;

/// Runs in Figma tabs. `__LIGMA_FRAME_COLORS__` is `{ on, rev }`: the
/// setting when the tab was made. A page loaded later in the same tab may
/// find a newer one in `localStorage`; the higher `rev` wins
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  const KEY = "ligma-frame-colors";
  let state = __LIGMA_FRAME_COLORS__;
  try {
    const stored = JSON.parse(localStorage.getItem(KEY));
    if (stored?.rev > state.rev) state = stored;
  } catch {}

  // The rainbow, in OKLCH hues: red, orange, yellow, green, blue, purple
  const HUES = [25, 60, 100, 150, 245, 305];
  // What counts as a frame. Sections and groups hold frames without being
  // one, so the frames in them each get a color of their own
  const FRAMES = new Set(["FRAME", "COMPONENT", "COMPONENT_SET", "INSTANCE"]);
  const ROW = '[class*="object_row--row--"]';
  const ID = '[data-testid$="-layers-panel-row"]';
  const COLORED = "data-ligma-frame-color";
  const TINT = "--ligma-frame-tint";

  // Under what's in the row, and under Figma's highlight on selected rows
  const style = document.createElement("style");
  style.textContent = `
    ${ROW}[${COLORED}]:not([class*="object_row--selected--"]) {
      background-image: linear-gradient(var(${TINT}), var(${TINT})) !important;
    }`;

  // A frame `depth` frames deep in the one `index` frames down its list:
  // its hue, a step darker for each frame it's in. Frames' own rows are
  // stronger than what's in them, so each one's start stands out
  const tint = (index, depth, own) => {
    const lightness = Math.max(0.42, 0.74 - depth * 0.08);
    return `oklch(${lightness} 0.15 ${HUES[index % HUES.length]} / ${own ? 0.3 : 0.12})`;
  };

  // Where each outermost frame comes among the frames beside it, top to
  // bottom as the panel lists them, read again at most once a second
  const orders = new Map();
  const indexOf = (node) => {
    const parent = node.parent;
    if (!parent) return 0;
    let order = orders.get(parent.id);
    if (!order || performance.now() - order.at > 1000) {
      const index = new Map();
      let at = 0;
      const children = parent.children;
      for (let i = children.length - 1; i >= 0; i--) if (FRAMES.has(children[i].type)) index.set(children[i].id, at++);
      order = { at: performance.now(), index };
      orders.set(parent.id, order);
    }
    return order.index.get(node.id) ?? 0;
  };

  // The color for the layer with `id`: of the frame it's in, or its own,
  // or null outside any frame
  const colorOf = (figma, id) => {
    let node;
    try {
      node = figma.getNodeById(id);
    } catch {}
    if (!node) return null;
    // The frames it's in, itself first if it's one, out to the outermost
    const frames = [];
    for (let at = node; at && at.type !== "PAGE" && at.type !== "DOCUMENT"; at = at.parent) {
      if (FRAMES.has(at.type)) frames.push(at);
    }
    if (!frames.length) return null;
    return tint(indexOf(frames.at(-1)), frames.length - 1, FRAMES.has(node.type));
  };

  const paint = () => {
    const figma = window.figma;
    for (const row of document.querySelectorAll(`${ROW}, ${ROW}[${COLORED}]`)) {
      const id = row.querySelector(ID)?.dataset.testid?.slice(0, -"-layers-panel-row".length);
      const color = state.on && figma && id ? colorOf(figma, id) : null;
      if (color) {
        if (row.style.getPropertyValue(TINT) !== color) row.style.setProperty(TINT, color);
        if (!row.hasAttribute(COLORED)) row.setAttribute(COLORED, "");
      } else if (row.hasAttribute(COLORED)) {
        row.removeAttribute(COLORED);
        row.style.removeProperty(TINT);
      }
    }
  };

  // Colors the rows as the panel draws them, scrolls or changes, once a
  // frame
  let panel = null;
  let queued = false;
  const queue = () => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      paint();
    });
  };
  const observer = new MutationObserver(queue);
  const watch = () => {
    const found = state.on ? document.querySelector('[data-testid="objects-panel"]') : null;
    if (found === panel) return;
    observer.disconnect();
    panel = found;
    if (panel) {
      observer.observe(panel, { childList: true, subtree: true, attributes: true, attributeFilter: ["data-testid", "class"] });
      queue();
    }
  };
  // The panel comes and goes with the file, the page and the left sidebar
  setInterval(watch, 1000);

  const apply = (next) => {
    state = next;
    if (state.on && !style.isConnected) document.head.append(style);
    watch();
    paint();
    if (!state.on) style.remove();
  };
  // From Ligma, when the setting changes
  window.__ligmaFrameColors = (next) => {
    try {
      localStorage.setItem(KEY, JSON.stringify(next));
    } catch {}
    apply(next);
  };
  // From another tab, or while this one was frozen and missed it
  const sync = () => {
    try {
      const stored = JSON.parse(localStorage.getItem(KEY));
      if (stored?.rev > state.rev) apply(stored);
    } catch {}
  };
  addEventListener("storage", (event) => event.key === KEY && sync());
  document.addEventListener("visibilitychange", () => document.visibilityState === "visible" && sync());
  const start = () => apply(state);
  if (document.head) start();
  else document.addEventListener("DOMContentLoaded", start, { once: true });
})()"##;

/// The setting,
static ON: AtomicBool = AtomicBool::new(false);
/// and when it last changed, for `PAGE_JS` to tell it from an older one in
/// `localStorage`. Launch counts as a change
static REV: AtomicU64 = AtomicU64::new(0);

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_millis() as u64)
}

fn state() -> String {
    let (on, rev) = (ON.load(Ordering::Relaxed), REV.load(Ordering::Relaxed));
    format!(r#"{{"on":{on},"rev":{rev}}}"#)
}

/// Call at launch, before any tab
pub fn init(settings: &Settings) {
    ON.store(settings.frame_colors, Ordering::Relaxed);
    REV.store(now(), Ordering::Relaxed);
}

/// `PAGE_JS`, for a new Figma tab
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_FRAME_COLORS__", &state())
}

/// Sends a changed setting to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    if ON.swap(settings.frame_colors, Ordering::Relaxed) == settings.frame_colors {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaFrameColors?.({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
