//! Frame colors (Settings > Features > Layers): frames in the layers panel
//! get a color each, the rainbow over again down the list, red, orange,
//! yellow, green, blue and purple, on their icons, and a guide in that
//! color runs down the indent of everything in them, as a code editor's
//! indent guides do, so it's clear where each frame starts and ends. The
//! frames inside a frame are a step darker. Off by default
//!
//! A design's colors start at the list that holds the most frames side by
//! side, as a board of designs does, so the designs on a board each get
//! their own color rather than the board's. Sections and groups hold frames
//! without being one. The panel draws only the rows in view, each with its
//! layer's ID, so `PAGE_JS` colors those as they're drawn, reading the
//! frames each layer is in from Figma's plugin API. Only the icons and the
//! indents are drawn on, so hover and selection stay Figma's

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
  const HUES = [25, 60, 95, 150, 240, 305];
  // What counts as a frame. Sections and groups hold frames without being
  // one
  const FRAMES = new Set(["FRAME", "COMPONENT", "COMPONENT_SET", "INSTANCE"]);
  const ROW = '[class*="object_row--row--"]';
  const ID = '[data-testid$="-layers-panel-row"]';
  const INDENT = '[class*="object_row--indent--"]';
  const ICON = '[class*="object_row--layerIcon--"]';
  const CARET = '[data-testid="layers-panel-expand-caret"]';
  const MARKED = "data-ligma-frame-color";
  // A frame's expand arrow is centered this far past where its row's last
  // indent starts, and its guide drawn there, this wide
  const ARROW = 16;
  const GUIDE = 2;

  // The guides are drawn over the row, above Figma's hover and selection,
  // and under nothing, as they keep to the indents
  const style = document.createElement("style");
  style.textContent = `
    ${ROW}[${MARKED}]::before {
      content: "";
      position: absolute;
      inset: 0;
      z-index: 2;
      pointer-events: none;
      background-image: var(--ligma-guides, none);
      background-position: var(--ligma-guides-at, 0 0);
      background-size: var(--ligma-guides-size, 0 0);
      background-repeat: no-repeat;
    }`;

  // How many frames are directly in `node`, read again at most once a
  // second
  const counts = new Map();
  const framesIn = (node) => {
    const known = counts.get(node.id);
    if (known && performance.now() - known.at < 1000) return known;
    const index = new Map();
    let at = 0;
    const children = "children" in node ? [...node.children] : [];
    // Top to bottom as the panel lists them: last first, but in auto
    // layout in order
    if (!node.layoutMode || node.layoutMode === "NONE") children.reverse();
    for (const child of children) if (FRAMES.has(child.type)) index.set(child.id, at++);
    const order = { at: performance.now(), count: at, index };
    counts.set(node.id, order);
    return order;
  };

  // A frame's color. The frames a design is made of start their colors at
  // the list that holds the most frames side by side, as a board of
  // designs does, each taking the next color of the rainbow, and the frames
  // in each a step darker. A frame holding that list, like the board
  // itself, takes its color from its own list
  let dark = true;
  const shades = new Map();
  const colorOf = (frame) => {
    if (shades.has(frame.id)) return shades.get(frame.id);
    const frames = [];
    for (let at = frame; at && at.type !== "PAGE" && at.type !== "DOCUMENT"; at = at.parent) if (FRAMES.has(at.type)) frames.push(at);
    let start = 0;
    let most = -1;
    // Outermost first, so the outer one wins a tie
    for (let i = frames.length - 1; i >= 0; i--) {
      const count = frames[i].parent ? framesIn(frames[i].parent).count : 0;
      if (count > most) [most, start] = [count, i];
    }
    const first = frames[start];
    const hue = HUES[(first.parent ? framesIn(first.parent).index.get(first.id) ?? 0 : 0) % HUES.length];
    // Each step darker, from light enough to read on the panel
    const lightness = dark ? Math.max(0.5, 0.78 - start * 0.09) : Math.max(0.36, 0.62 - start * 0.08);
    const color = `oklch(${lightness} ${dark ? 0.15 : 0.17} ${hue})`;
    shades.set(frame.id, color);
    return color;
  };

  const nodeOf = (figma, row) => {
    const id = row.querySelector(ID)?.dataset.testid?.slice(0, -"-layers-panel-row".length);
    try {
      return id ? figma.getNodeById(id) : null;
    } catch {
      return null;
    }
  };

  const GUIDES = ["--ligma-guides", "--ligma-guides-at", "--ligma-guides-size"];
  const clear = (row) => {
    row.removeAttribute(MARKED);
    for (const property of GUIDES) row.style.removeProperty(property);
    row.querySelector(`${ICON}[${MARKED}]`)?.style.removeProperty("--fpl-icon-color");
    row.querySelector(`${ICON}[${MARKED}]`)?.removeAttribute(MARKED);
  };

  // Each row: its frame's icon in its color, a guide down the column of
  // each frame it's in, in that frame's color, and below an open frame's
  // arrow, the start of its own
  const paint = () => {
    const figma = window.figma;
    const rows = document.querySelectorAll(ROW);
    if (!state.on || !figma) {
      for (const row of document.querySelectorAll(`${ROW}[${MARKED}]`)) clear(row);
      return;
    }
    shades.clear();
    // Light text on the panel means a dark theme
    const text = rows[0] && getComputedStyle(rows[0]).color.match(/[\d.]+/g)?.map(Number);
    if (text) dark = 0.299 * text[0] + 0.587 * text[1] + 0.114 * text[2] > 128;
    for (const row of rows) {
      const node = nodeOf(figma, row);
      if (!node) {
        if (row.hasAttribute(MARKED)) clear(row);
        continue;
      }
      const box = row.getBoundingClientRect();
      const indents = [...row.querySelectorAll(INDENT)].map((indent) => indent.getBoundingClientRect().left - box.left);
      const layers = [];
      // The frames it's in, `steps` levels up
      let steps = 1;
      for (let at = node.parent; at && at.type !== "PAGE" && at.type !== "DOCUMENT"; at = at.parent, steps++) {
        const column = indents.length - steps - 1;
        if (column < 0) break;
        if (!FRAMES.has(at.type)) continue;
        const x = Math.round(indents[column] + ARROW - GUIDE / 2);
        const color = colorOf(at);
        layers.push({ color, position: `${x}px 0`, size: `${GUIDE}px 100%` });
      }
      const own = FRAMES.has(node.type) ? colorOf(node) : null;
      const caret = row.querySelector(CARET);
      if (own && caret && /object_row--expanded--/.test(row.className) && indents.length) {
        const x = Math.round(indents.at(-1) + ARROW - GUIDE / 2);
        const top = Math.round(caret.getBoundingClientRect().bottom - box.top - 2);
        layers.push({ color: own, position: `${x}px ${top}px`, size: `${GUIDE}px calc(100% - ${top}px)` });
      }
      const set = (property, value) => {
        if (row.style.getPropertyValue(property) !== value) row.style.setProperty(property, value);
      };
      if (layers.length) {
        set(GUIDES[0], layers.map(({ color }) => `linear-gradient(${color}, ${color})`).join(", "));
        set(GUIDES[1], layers.map(({ position }) => position).join(", "));
        set(GUIDES[2], layers.map(({ size }) => size).join(", "));
      } else {
        for (const property of GUIDES) row.style.removeProperty(property);
      }
      const icon = row.querySelector(ICON);
      if (icon) {
        if (own) {
          icon.style.setProperty("--fpl-icon-color", own);
          icon.setAttribute(MARKED, "");
        } else if (icon.hasAttribute(MARKED)) {
          icon.style.removeProperty("--fpl-icon-color");
          icon.removeAttribute(MARKED);
        }
      }
      if (layers.length || own) row.setAttribute(MARKED, "");
      else row.removeAttribute(MARKED);
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
