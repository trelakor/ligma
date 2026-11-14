//! Frame colors (Settings > Features > Layers): frames in the layers panel
//! get a color each, down the list, from a palette: the rainbow over again,
//! red, orange, yellow, green, blue and purple, or a colorway, Sunset, Ocean,
//! Forest, Aurora, Candy or Ember, stepping from one hue to another and
//! back, neighbors lighter and darker by turns. Each shows on the frame's
//! icon, and a guide in that color runs down the indent of everything in them, as a code editor's
//! indent guides do, so it's clear where each frame starts and ends. The
//! frames inside a frame are a step darker. Off by default
//!
//! A design's colors start at the list that holds the most frames side by
//! side, as a board of designs does, so the designs on a board each get
//! their own color rather than the board's. Sections and groups hold frames
//! without being one. The panel draws only the rows in view, each with its
//! layer's ID, so `PAGE_JS` colors those as they're drawn, reading the
//! frames each layer is in from Figma's plugin API. Without Tint rows only
//! the icons and the indents are drawn on, so hover and selection stay
//! Figma's, and the icons
//! of what's in a frame, like images and vectors, take its color too
//!
//! Tint rows, off by default, also washes each row with its frame's color,
//! lightly, from the guides' side, fading toward its name

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::settings::Settings;

/// Runs in Figma tabs. `__LIGMA_FRAME_COLORS__` is `{ on, tint, palette, rev }`:
/// the settings when the tab was made. A page loaded later in the same tab may
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
  // The other palettes step from one hue to another over 4 frames, and
  // back, each step a third of the way, and frames beside each other
  // lighter and darker by turns, so they stay apart
  const SWEEPS = {
    sunset: [345, 445],
    ocean: [170, 280],
    forest: [95, 185],
    aurora: [150, 310],
    candy: [280, 380],
    ember: [15, 100],
  };
  const STEPS = 4;
  const SHIFT = 0.06;
  // The hue for the frame `index` frames down, and how much lighter or
  // darker it is than the frames around it
  const hueAt = (index) => {
    const sweep = SWEEPS[state.palette];
    if (!sweep) return { hue: HUES[index % HUES.length], shift: 0 };
    const cycle = 2 * (STEPS - 1);
    const step = index % cycle;
    const t = (step < STEPS ? step : cycle - step) / (STEPS - 1);
    return { hue: (sweep[0] + (sweep[1] - sweep[0]) * t) % 360, shift: index % 2 ? -SHIFT : SHIFT / 2 };
  };
  // What counts as a frame. Sections and groups hold frames without being
  // one
  const FRAMES = new Set(["FRAME", "COMPONENT", "COMPONENT_SET", "INSTANCE"]);
  const ROW = '[class*="object_row--row--"]';
  const ID = '[data-testid$="-layers-panel-row"]';
  const INDENT = '[class*="object_row--indent--"]';
  // The strip of indents at the start of each row, full height, under the
  // row's name and over Figma's hover and selection, where guides go
  const INDENTS = '[class*="object_row--indents--"]';
  const ICON = '[class*="object_row--layerIcon--"]';
  const CARET = '[data-testid="layers-panel-expand-caret"]';
  const MARKED = "data-ligma-frame-color";

  // Shapes' icons are outlines Figma colors through `stroke`, the rest
  // through `--fpl-icon-color`
  const style = document.createElement("style");
  // With Tint rows, Figma's boxes behind the row's name take the row's
  // color in place of grey and blue: hover paler and whiter, the selected
  // row brighter still, and the band down what's in a selection softly
  style.textContent = `
    ${ICON}[${MARKED}] .svg-container svg { stroke: var(--fpl-icon-color); }
    ${ROW}[${MARKED}][class*="object_row--hovered--"][style*="--ligma-hover"]::after {
      background-color: var(--ligma-hover) !important;
    }
    ${ROW}[${MARKED}][class*="object_row--selected--"][style*="--ligma-hover"]::after {
      background-color: var(--ligma-selected) !important;
    }
    ${ROW}[${MARKED}][style*="--ligma-hover"]::before {
      background-color: var(--ligma-band) !important;
    }`;

  // A frame's expand arrow is centered this far past where its row's last
  // indent starts, and its guide drawn there, this wide
  const ARROW = 16;
  const GUIDE = 2;

  // How many frames are directly in `node`, read again at most once a
  // second. Cleared when the file or page changes, as IDs repeat across
  // files
  const counts = new Map();
  let shown = "";
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
    const { hue, shift } = hueAt(first.parent ? framesIn(first.parent).index.get(first.id) ?? 0 : 0);
    // Each step darker, from light enough to read on the panel
    const lightness = shift + (dark ? Math.max(0.48, 0.8 - start * 0.11) : Math.max(0.34, 0.62 - start * 0.09));
    const color = `oklch(${lightness.toFixed(3)} ${dark ? 0.15 : 0.17} ${Math.round(hue)})`;
    shades.set(frame.id, color);
    return color;
  };

  // The async lookup, as the sync one throws with dynamic page loading.
  // The rows are on the current page, which is loaded, so it's quick
  const nodeOf = (figma, row) => {
    const id = row.querySelector(ID)?.dataset.testid?.slice(0, -"-layers-panel-row".length);
    try {
      return id ? figma.getNodeByIdAsync(id).catch(() => null) : null;
    } catch {
      return null;
    }
  };

  const LOOKS = ["--ligma-hover", "--ligma-selected", "--ligma-band"];
  const GUIDES = ["background-image", "background-position", "background-size", "background-repeat"];
  const clear = (row) => {
    row.removeAttribute(MARKED);
    row.style.removeProperty("background-image");
    for (const property of LOOKS) row.style.removeProperty(property);
    const strip = row.querySelector(`${INDENTS}[${MARKED}]`);
    if (strip) {
      for (const property of GUIDES) strip.style.removeProperty(property);
      strip.removeAttribute(MARKED);
    }
    const icon = row.querySelector(`${ICON}[${MARKED}]`);
    if (icon) {
      icon.style.removeProperty("--fpl-icon-color");
      icon.removeAttribute(MARKED);
    }
  };

  // Each row: its icon in its frame's color, or the color of the frame
  // it's in, a guide down the column of each frame it's in, in that frame's
  // color, and below an open frame's arrow, the start of its own
  const paintRows = async () => {
    const figma = window.figma;
    if (!state.on || !figma || !panel) {
      for (const row of document.querySelectorAll(`${ROW}[${MARKED}]`)) clear(row);
      return;
    }
    let nodes;
    try {
      const where = `${location.pathname} ${figma.currentPage.id}`;
      if (where !== shown) {
        shown = where;
        counts.clear();
      }
      nodes = await Promise.all([...panel.querySelectorAll(ROW)].map(async (row) => [row, await nodeOf(figma, row)]));
    } catch {
      return;
    }
    if (!state.on) return;
    shades.clear();
    // Everything is measured before anything is written, so the rows lay
    // out once rather than once each
    const rows = nodes.filter(([row]) => row.isConnected);
    // Light text on the panel means a dark theme
    const text = rows[0] && getComputedStyle(rows[0][0]).color.match(/[\d.]+/g)?.map(Number);
    if (text) dark = 0.299 * text[0] + 0.587 * text[1] + 0.114 * text[2] > 128;
    const measured = rows.map(([row, node]) => {
      const strip = node && row.querySelector(INDENTS);
      if (!strip) return { row, node };
      const box = strip.getBoundingClientRect();
      const indents = [...strip.querySelectorAll(INDENT)].map((indent) => indent.getBoundingClientRect().left - box.left);
      const caret = row.querySelector(CARET);
      return { row, node, strip, indents, top: caret && Math.round(caret.getBoundingClientRect().bottom - box.top - 2) };
    });
    for (const { row, node, strip, indents, top } of measured) {
      if (!node) {
        if (row.hasAttribute(MARKED)) clear(row);
        continue;
      }
      if (!strip) continue;
      const layers = [];
      // The nearest frame it's in, for its icon
      let within = null;
      // The frames it's in, `steps` levels up
      let steps = 1;
      for (let at = node.parent; at && at.type !== "PAGE" && at.type !== "DOCUMENT"; at = at.parent, steps++) {
        const column = indents.length - steps - 1;
        if (column < 0) break;
        if (!FRAMES.has(at.type)) continue;
        const x = Math.round(indents[column] + ARROW - GUIDE / 2);
        const color = colorOf(at);
        within ??= color;
        layers.push({ color, position: `${x}px 0`, size: `${GUIDE}px 100%` });
      }
      const own = FRAMES.has(node.type) ? colorOf(node) : null;
      const tint = own ?? within;
      if (own && top != null && /object_row--expanded--/.test(row.className) && indents.length) {
        const x = Math.round(indents.at(-1) + ARROW - GUIDE / 2);
        layers.push({ color: own, position: `${x}px ${top}px`, size: `${GUIDE}px calc(100% - ${top}px)` });
      }
      const set = (property, value) => {
        if (strip.style.getPropertyValue(property) !== value) strip.style.setProperty(property, value);
      };
      if (layers.length) {
        set(GUIDES[0], layers.map(({ color }) => `linear-gradient(${color}, ${color})`).join(", "));
        set(GUIDES[1], layers.map(({ position }) => position).join(", "));
        set(GUIDES[2], layers.map(({ size }) => size).join(", "));
        set(GUIDES[3], "no-repeat");
        strip.setAttribute(MARKED, "");
      } else if (strip.hasAttribute(MARKED)) {
        for (const property of GUIDES) strip.style.removeProperty(property);
        strip.removeAttribute(MARKED);
      }
      const icon = row.querySelector(ICON);
      if (icon) {
        if (tint) {
          if (icon.style.getPropertyValue("--fpl-icon-color") !== tint) icon.style.setProperty("--fpl-icon-color", tint);
          icon.setAttribute(MARKED, "");
        } else if (icon.hasAttribute(MARKED)) {
          icon.style.removeProperty("--fpl-icon-color");
          icon.removeAttribute(MARKED);
        }
      }
      // With Tint rows, a wash of the frame's color from the guides' side,
      // fading toward the name. It's the row's own background, under
      // Figma's hover and selection, which cover it as they would the row
      const wash = state.tint && tint ? `linear-gradient(90deg, ${tint.replace(")", " / 0.13)")}, ${tint.replace(")", " / 0.03)")} 70%)` : "";
      if (row.style.getPropertyValue("background-image") !== wash) {
        if (wash) row.style.setProperty("background-image", wash);
        else row.style.removeProperty("background-image");
      }
      const hue = wash && tint.match(/([\d.]+)\)$/)?.[1];
      const looks = hue
        ? dark
          ? { "--ligma-hover": `oklch(0.93 0.07 ${hue} / 0.16)`, "--ligma-selected": `oklch(0.86 0.13 ${hue} / 0.36)`, "--ligma-band": `oklch(0.86 0.12 ${hue} / 0.14)` }
          : { "--ligma-hover": `oklch(0.95 0.05 ${hue})`, "--ligma-selected": `oklch(0.87 0.1 ${hue})`, "--ligma-band": `oklch(0.95 0.045 ${hue})` }
        : {};
      for (const property of LOOKS) {
        const value = looks[property] ?? "";
        if (row.style.getPropertyValue(property) === value) continue;
        if (value) row.style.setProperty(property, value);
        else row.style.removeProperty(property);
      }
      if (layers.length || tint) row.setAttribute(MARKED, "");
      else row.removeAttribute(MARKED);
    }
  };

  // One paint at a time, and one more a little after, once `counts` can be
  // read again, for frames added, moved or reordered since. `late` is -1
  // while that one is on its way, so it doesn't queue another
  let painting = false;
  let again = false;
  let late = 0;
  const paint = async () => {
    if (painting) {
      again = true;
      return;
    }
    painting = true;
    try {
      await paintRows();
    } finally {
      painting = false;
      if (again) {
        again = false;
        queue();
      } else if (late === -1) {
        late = 0;
      } else if (state.on && !late) {
        late = setTimeout(() => {
          late = -1;
          queue();
        }, 1100);
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
  let watching = 0;

  const apply = (next) => {
    state = next;
    if (state.on && !style.isConnected) document.head.append(style);
    if (state.on && !watching) watching = setInterval(watch, 1000);
    else if (!state.on && watching) {
      clearInterval(watching);
      watching = 0;
    }
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

/// The setting, whether rows are tinted too, and the palette, by its place
/// in `PALETTES`
static ON: AtomicBool = AtomicBool::new(false);
static TINT: AtomicBool = AtomicBool::new(false);
static PALETTE: AtomicUsize = AtomicUsize::new(0);

/// The palettes `PAGE_JS` knows, Rainbow first and the default
const PALETTES: [&str; 7] = ["rainbow", "sunset", "ocean", "forest", "aurora", "candy", "ember"];

/// When the settings last changed, for `PAGE_JS` to tell them from older
/// ones in `localStorage`. Launch counts as a change
static REV: AtomicU64 = AtomicU64::new(0);

fn palette(settings: &Settings) -> usize {
    PALETTES.iter().position(|&name| name == settings.frame_palette).unwrap_or(0)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_millis() as u64)
}

fn state() -> String {
    let (on, rev) = (ON.load(Ordering::Relaxed), REV.load(Ordering::Relaxed));
    let palette = PALETTES[PALETTE.load(Ordering::Relaxed).min(PALETTES.len() - 1)];
    let tint = TINT.load(Ordering::Relaxed);
    format!(r#"{{"on":{on},"tint":{tint},"palette":"{palette}","rev":{rev}}}"#)
}

/// Call at launch, before any tab
pub fn init(settings: &Settings) {
    ON.store(settings.frame_colors, Ordering::Relaxed);
    TINT.store(settings.frame_tint, Ordering::Relaxed);
    PALETTE.store(palette(settings), Ordering::Relaxed);
    REV.store(now(), Ordering::Relaxed);
}

/// `PAGE_JS`, for a new Figma tab
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_FRAME_COLORS__", &state())
}

/// Sends a changed setting to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    let on = ON.swap(settings.frame_colors, Ordering::Relaxed) != settings.frame_colors;
    let palette = PALETTE.swap(palette(settings), Ordering::Relaxed) != palette(settings);
    let tint = TINT.swap(settings.frame_tint, Ordering::Relaxed) != settings.frame_tint;
    if !on && !palette && !tint {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaFrameColors?.({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
