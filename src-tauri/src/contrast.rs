//! Color contrast (Settings > Features > Colors): while the color picker of
//! a solid fill or stroke of a text layer or a vector is open, it shows
//! the color's WCAG 2 contrast against what's behind the layer
//!
//! A row under the hex field has a swatch with "Aa" in the color on what
//! it's measured against, the ratio, and the levels: AA and AAA at 4.5:1
//! and 7:1 for text, 3:1 and 4.5:1 for large text (24px, or 18.66px bold),
//! and AA at 3:1 for vectors. Lines on the picker's color square mark
//! where the color starts to pass each level at the current hue, solid for
//! AA and dashed for AAA, like Chrome's color picker. Hovering the row
//! names what's behind
//!
//! What's behind is the page's background, then going down to the layer,
//! each parent's fills and the siblings below it that cover the whole
//! layer, then the layer's own fills below the one being edited. Over an
//! image or gradient there's no single color, so nothing is shown. Blend
//! modes, effects and the parents' opacity are left out
//!
//! Paints are read through Figma's console plugin API, `window.figma`. The
//! paint being edited is the Fill or Stroke row whose picker is open, found
//! by Figma's test ids and class names as seen on 2026-10-29 (see
//! adjustments.rs). Inside the picker, the color square is told by its
//! shape and the hex field as the first inputs below it, so nothing is
//! drawn when they can't be found. The square is Figma's saturation (left
//! to right) by brightness (bottom to top)

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::settings::Settings;

/// Runs in Figma tabs. `__LIGMA_COLOR_CONTRAST__` is `{ on, rev }`: the
/// setting when the tab was made. A page loaded later in the same tab may
/// find a newer one in `localStorage`; the higher `rev` wins
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  const KEY = "ligma-color-contrast";
  let state = __LIGMA_COLOR_CONTRAST__;
  try {
    const stored = JSON.parse(localStorage.getItem(KEY));
    if (stored?.rev > state.rev) state = stored;
  } catch {}

  const api = () => {
    const figma = window.figma;
    try {
      return figma?.currentPage && figma.editorType === "figma" ? figma : null;
    } catch {
      return null;
    }
  };
  const paintsOf = (node, list) => {
    try {
      return Array.isArray(node[list]) ? node[list] : [];
    } catch {
      return [];
    }
  };
  const shown = (paint) => paint.visible !== false && (paint.opacity ?? 1) > 0;

  // The Fill or Stroke row whose picker is open, as `{ list, index }`. Its
  // `data-onboarding-key` is `paint-panel-row-paint-<section>-<index>`, and
  // Fill comes first on the page
  const ROW = '[data-testid="paint-panel-color-value-container"]';
  const KEY_PATTERN = /-(\d+)-(\d+)$/;
  const openPaint = (node) => {
    const row = document.querySelector(`${ROW}[class*="paintPanelValueSelected--"]`);
    const match = KEY_PATTERN.exec(row?.dataset.onboardingKey ?? "");
    if (!match) return null;
    const first = KEY_PATTERN.exec(document.querySelector(ROW)?.dataset.onboardingKey ?? "");
    const at = { list: first && first[1] !== match[1] ? "strokes" : "fills", index: Number(match[2]) };
    // A layer with strokes but no fills: Stroke comes first
    if (at.list === "fills" && !paintsOf(node, "fills").length && paintsOf(node, "strokes").length) at.list = "strokes";
    return at;
  };

  // `{ r, g, b }` from 0 to 1, with `paint` laid over it at `opacity`.
  // Null when the paint isn't one color
  const over = (color, paint, opacity = 1) => {
    if (!color || !shown(paint)) return color;
    if (paint.type !== "SOLID") return null;
    const alpha = (paint.opacity ?? 1) * opacity;
    const mix = (key) => paint.color[key] * alpha + color[key] * (1 - alpha);
    return { r: mix("r"), g: mix("g"), b: mix("b") };
  };
  const overAll = (color, paints, opacity) => paints.reduce((color, paint) => over(color, paint, opacity), color);
  const box = (node) => {
    try {
      return node.absoluteBoundingBox;
    } catch {
      return null;
    }
  };
  const covers = (outer, inner) =>
    !!outer && !!inner && outer.x <= inner.x && outer.y <= inner.y && outer.x + outer.width >= inner.x + inner.width && outer.y + outer.height >= inner.y + inner.height;

  // The color behind `node` and the name of what it's from, or null over
  // an image or gradient
  const behind = (figma, node) => {
    const page = figma.currentPage;
    let color = overAll({ r: 1, g: 1, b: 1 }, page.backgrounds ?? []);
    let from = "the page";
    const chain = [];
    for (let at = node; at && at !== page; at = at.parent) chain.unshift(at);
    const target = box(node);
    let parent = page;
    for (const child of chain) {
      for (const sibling of parent.children) {
        if (sibling === child) break;
        if (!sibling.visible || sibling.isMask || !covers(box(sibling), target)) continue;
        const fills = paintsOf(sibling, "fills");
        if (!fills.some(shown)) continue;
        color = overAll(color, fills, sibling.opacity ?? 1);
        from = sibling.name;
      }
      const fills = child === node ? [] : paintsOf(child, "fills");
      if (fills.some(shown)) {
        color = overAll(color, fills);
        from = child.name;
      }
      if (!color) return null;
      parent = child;
    }
    return { color, from };
  };

  const luminance = ({ r, g, b }) => {
    const linear = (c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
    return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
  };
  const contrast = (a, b) => (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
  const hsv = ({ r, g, b }) => {
    const max = Math.max(r, g, b);
    const range = max - Math.min(r, g, b);
    let h = 0;
    if (range) {
      if (max === r) h = ((g - b) / range + 6) % 6;
      else if (max === g) h = (b - r) / range + 2;
      else h = (r - g) / range + 4;
    }
    return { h: h / 6, s: max ? range / max : 0, v: max };
  };
  const rgb = (h, s, v) => {
    const channel = (n) => {
      const k = (n + h * 6) % 6;
      return v - v * s * Math.max(0, Math.min(k, 4 - k, 1));
    };
    return { r: channel(5), g: channel(3), b: channel(1) };
  };
  const css = ({ r, g, b }, alpha = 1) => `rgb(${r * 255} ${g * 255} ${b * 255} / ${alpha})`;
  const hex = (color) =>
    "#" + ["r", "g", "b"].map((key) => Math.round(color[key] * 255).toString(16).padStart(2, "0")).join("").toUpperCase();

  // The levels a layer is held to, lowest first. None for layers that
  // aren't text or vectors, like frames and shapes
  const GRAPHICS = new Set(["VECTOR", "BOOLEAN_OPERATION", "LINE", "STAR", "POLYGON"]);
  const levelsOf = (node) => {
    if (GRAPHICS.has(node.type)) return [{ name: "AA", min: 3 }];
    if (node.type !== "TEXT") return null;
    const size = typeof node.fontSize === "number" ? node.fontSize : 0;
    const weight = typeof node.fontWeight === "number" ? node.fontWeight : 400;
    const large = size >= 24 || (size >= 18.66 && weight >= 700);
    return [
      { name: "AA", min: large ? 3 : 4.5 },
      { name: "AAA", min: large ? 4.5 : 7 },
    ];
  };

  // What to show for the open picker, or null for nothing
  const measure = () => {
    const figma = api();
    const node = figma?.currentPage.selection[0];
    const levels = node && levelsOf(node);
    if (!levels) return null;
    const at = openPaint(node);
    const list = at && paintsOf(node, at.list);
    const paint = list?.[at.index];
    if (paint?.type !== "SOLID") return null;
    const back = behind(figma, node);
    const bg = back && (at.list === "fills" ? overAll(back.color, list.slice(0, at.index)) : back.color);
    if (!bg) return null;
    const alpha = (paint.opacity ?? 1) * (node.opacity ?? 1);
    const front = over(bg, { ...paint, visible: true }, node.opacity ?? 1);
    return { paint, alpha, bg, front, from: back.from, levels, ratio: contrast(luminance(front), luminance(bg)) };
  };

  const PICKER = '[data-testid="paint-picker-v2-modal"]';
  const visible = (node) => node.getClientRects().length > 0;
  // The color square: the smallest squarish area nearly as wide as the
  // picker without inputs or buttons in it, so not a box around it and
  // the sliders
  const squareOf = (picker) => {
    const width = picker.getBoundingClientRect().width;
    let best = null;
    let area = Infinity;
    for (const node of picker.querySelectorAll("div, canvas")) {
      const rect = node.getBoundingClientRect();
      if (rect.width < width * 0.6 || rect.height < rect.width * 0.4 || rect.height > rect.width * 1.2) continue;
      if (node.tagName === "DIV" && node.querySelector("input, button, select")) continue;
      if (rect.width * rect.height < area) {
        best = node;
        area = rect.width * rect.height;
      }
    }
    return best;
  };
  // The hex field's row: the first input below the square, and the boxes
  // around it that are still one row high
  const hexRowOf = (picker, square) => {
    const bottom = square.getBoundingClientRect().bottom;
    const input = [...picker.querySelectorAll("input")].find((input) => visible(input) && input.getBoundingClientRect().top >= bottom);
    if (!input) return null;
    let row = input;
    while (row.parentElement && row.parentElement !== picker && row.parentElement.getBoundingClientRect().height <= 48) row = row.parentElement;
    return row === input ? null : row;
  };
  // The hue the square shows: its base color, if it's painted with one,
  // else the paint's own, kept while the paint is grey and has none
  let lastHue = 0;
  const hueOf = (square, paint) => {
    for (const node of [square, ...square.querySelectorAll("div")]) {
      const match = /^rgba?\((\d+), (\d+), (\d+)(?:, ([\d.]+))?\)$/.exec(getComputedStyle(node).backgroundColor);
      if (!match || match[4] === "0") continue;
      const color = hsv({ r: match[1] / 255, g: match[2] / 255, b: match[3] / 255 });
      if (color.s > 0.5 && color.v > 0.5) return (lastHue = color.h);
    }
    const color = hsv(paint.color);
    if (color.s > 0 && color.v > 0) lastHue = color.h;
    return lastHue;
  };

  // Where a level starts to pass on the square at hue `h`: one path for
  // darker colors and one for lighter, as SVG path data in a unit box.
  // A color's luminance only grows with its brightness, so each column's
  // edge is found by halving
  const COLUMNS = 48;
  const edges = (h, bg, alpha, min) => {
    const back = luminance(bg);
    const lumAt = (s, v) => luminance(over(bg, { type: "SOLID", color: rgb(h, s, v) }, alpha));
    const solve = (s, target) => {
      if (target <= lumAt(s, 0) || target >= lumAt(s, 1)) return null;
      let [low, high] = [0, 1];
      for (let i = 0; i < 16; i++) {
        const mid = (low + high) / 2;
        if (lumAt(s, mid) < target) low = mid;
        else high = mid;
      }
      return (low + high) / 2;
    };
    let path = "";
    for (const target of [(back + 0.05) / min - 0.05, min * (back + 0.05) - 0.05]) {
      let drawing = false;
      for (let i = 0; i <= COLUMNS; i++) {
        const s = i / COLUMNS;
        const v = solve(s, target);
        if (v === null) {
          drawing = false;
          continue;
        }
        path += `${drawing ? "L" : "M"}${s.toFixed(4)} ${(1 - v).toFixed(4)}`;
        drawing = true;
      }
    }
    return path;
  };

  const ROW_ATTR = "data-ligma-contrast";
  const LINES_ATTR = "data-ligma-contrast-lines";
  const SVG = "http://www.w3.org/2000/svg";
  const sample = (dashed) =>
    `<svg width="12" height="2" viewBox="0 0 12 2" aria-hidden="true"><path d="M0 1h12" stroke="currentColor" stroke-width="1.5"${dashed ? ' stroke-dasharray="3 2"' : ""}/></svg>`;
  const MARKS = {
    pass: '<svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="m3 6.2 2 2L9 4" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/></svg>',
    fail: '<svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="m4 4 4 4m0-4L4 8" fill="none" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/></svg>',
  };

  const clear = () => {
    for (const node of document.querySelectorAll(`[${ROW_ATTR}], [${LINES_ATTR}]`)) node.remove();
  };

  // The row under the hex field
  const showRow = (picker, anchor, result) => {
    let row = picker.querySelector(`[${ROW_ATTR}]`);
    if (!row || row.previousElementSibling !== anchor) {
      row?.remove();
      row = document.createElement("div");
      row.setAttribute(ROW_ATTR, "");
      row.dataset.tooltipType = "text";
      anchor.after(row);
    }
    // Lined up with the hex row's first control
    const inset = Math.max(0, (anchor.firstElementChild ?? anchor).getBoundingClientRect().left - anchor.getBoundingClientRect().left);
    row.style.paddingInline = `${inset}px`;
    row.dataset.tooltip = `Against ${result.from}, ${hex(result.bg)}`;
    const ratio = (Math.floor(result.ratio * 100) / 100).toFixed(2);
    const levels = result.levels
      .map((level, i) => {
        const pass = result.ratio >= level.min;
        const mark = pass ? "pass" : i === 0 ? "fail" : "short";
        return `<span data-level="${mark}">${sample(i > 0)}${level.name}${MARKS[pass ? "pass" : "fail"]}</span>`;
      })
      .join("");
    const html =
      `<span data-swatch style="background:${css(result.bg)};color:${css(result.paint.color, result.alpha)}">Aa</span>` +
      `<span data-ratio>${ratio}:1</span><span data-levels>${levels}</span>`;
    if (row.innerHTML !== html) row.innerHTML = html;
  };

  // The lines on the square, laid over it inside the picker so they move
  // with it. Worked out again only when the hue, background, opacity or
  // levels change
  let drawn = "";
  const showLines = (picker, square, result) => {
    let lines = picker.querySelector(`[${LINES_ATTR}]`);
    if (!lines) {
      lines = document.createElement("div");
      lines.setAttribute(LINES_ATTR, "");
      lines.innerHTML = `<svg xmlns="${SVG}" viewBox="0 0 1 1" preserveAspectRatio="none" aria-hidden="true"></svg>`;
      picker.append(lines);
      drawn = "";
    }
    // Placed from what it's positioned against
    const host = lines.offsetParent ?? document.body;
    const origin = host.getBoundingClientRect();
    const rect = square.getBoundingClientRect();
    Object.assign(lines.style, {
      left: `${rect.left - origin.left - host.clientLeft + host.scrollLeft}px`,
      top: `${rect.top - origin.top - host.clientTop + host.scrollTop}px`,
      width: `${rect.width}px`,
      height: `${rect.height}px`,
      borderRadius: getComputedStyle(square).borderRadius,
    });
    const h = hueOf(square, result.paint);
    const key = [h.toFixed(3), hex(result.bg), result.alpha.toFixed(3), ...result.levels.map((level) => level.min)].join();
    if (key === drawn) return;
    drawn = key;
    lines.firstElementChild.innerHTML = result.levels
      .map((level, i) => {
        const d = edges(h, result.bg, result.alpha, level.min);
        if (!d) return "";
        const dash = i > 0 ? ' stroke-dasharray="4 3"' : "";
        return `<path d="${d}" data-shadow${dash}/><path d="${d}"${dash}/>`;
      })
      .join("");
  };

  const render = () => {
    const picker = state.on && document.querySelector(PICKER);
    let result = null;
    let square = null;
    let anchor = null;
    if (picker) {
      try {
        result = measure();
        square = result && squareOf(picker);
        anchor = square && hexRowOf(picker, square);
      } catch {
        result = null;
      }
    }
    if (!result || !anchor) {
      clear();
      return;
    }
    showRow(picker, anchor, result);
    showLines(picker, square, result);
  };

  const style = document.createElement("style");
  style.textContent = `
    [${ROW_ATTR}] {
      display: flex; align-items: center; gap: 8px; height: 32px; box-sizing: border-box;
      font-size: 11px; color: var(--color-text-secondary, rgba(0, 0, 0, 0.5));
    }
    [${ROW_ATTR}] [data-swatch] {
      flex: none; display: grid; place-items: center; width: 24px; height: 24px; border-radius: 4px;
      box-shadow: inset 0 0 0 1px var(--color-border, rgba(0, 0, 0, 0.1));
      font-size: 11px; font-weight: 600; letter-spacing: -0.01em;
    }
    [${ROW_ATTR}] [data-ratio] { font-weight: 500; color: var(--color-text, rgba(0, 0, 0, 0.9)); font-variant-numeric: tabular-nums; }
    [${ROW_ATTR}] [data-levels] { display: flex; gap: 10px; margin-left: auto; }
    [${ROW_ATTR}] [data-level] { display: flex; align-items: center; gap: 4px; }
    [${ROW_ATTR}] [data-level] > svg:first-child { opacity: 0.6; }
    [${ROW_ATTR}] [data-level="pass"] > svg:last-child { color: var(--color-icon-success, #14ae5c); }
    [${ROW_ATTR}] [data-level="fail"] > svg:last-child { color: var(--color-icon-danger, #f24822); }
    [${ROW_ATTR}] [data-level="short"] > svg:last-child { color: var(--color-icon-tertiary, rgba(0, 0, 0, 0.3)); }
    [${LINES_ATTR}] { position: absolute; z-index: 1; overflow: hidden; pointer-events: none; }
    [${LINES_ATTR}] svg { display: block; width: 100%; height: 100%; overflow: visible; }
    [${LINES_ATTR}] path { fill: none; stroke: #fff; stroke-width: 1.25px; vector-effect: non-scaling-stroke; }
    [${LINES_ATTR}] path[data-shadow] { stroke: rgba(0, 0, 0, 0.35); stroke-width: 3px; }
  `;
  const mount = () => {
    if (style.isConnected) return true;
    const parent = document.head ?? document.documentElement;
    parent?.append(style);
    return !!parent;
  };
  if (!mount()) new MutationObserver((_, observer) => mount() && observer.disconnect()).observe(document, { childList: true });

  // Dragging in the picker changes the paint without changing the page,
  // so while it's open it's checked every few frames
  let frame;
  let ticks = 0;
  const loop = () => {
    if (!(state.on && document.querySelector(PICKER))) {
      frame = undefined;
      clear();
      return;
    }
    if (ticks++ % 4 === 0) render();
    frame = requestAnimationFrame(loop);
  };
  const watch = () => {
    if (!frame) loop();
  };
  let queued = false;
  new MutationObserver(() => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      watch();
    });
  }).observe(document, { childList: true, subtree: true });

  // From Ligma, when the setting changes
  window.__ligmaColorContrast = (next) => {
    try {
      localStorage.setItem(KEY, JSON.stringify(next));
    } catch {}
    state = next;
    watch();
  };
  // From another tab, or while this one was frozen and missed it
  const sync = () => {
    try {
      const stored = JSON.parse(localStorage.getItem(KEY));
      if (stored?.rev > state.rev) {
        state = stored;
        watch();
      }
    } catch {}
  };
  addEventListener("storage", (event) => event.key === KEY && sync());
  document.addEventListener("visibilitychange", () => document.visibilityState === "visible" && sync());
})()"##;


/// The setting,
static ON: AtomicBool = AtomicBool::new(true);
/// and when it last changed, for `PAGE_JS` to tell it from older ones in
/// `localStorage`. Launch counts as a change
static REV: AtomicU64 = AtomicU64::new(0);

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_millis() as u64)
}

fn state() -> String {
    format!(r#"{{"on":{},"rev":{}}}"#, ON.load(Ordering::Relaxed), REV.load(Ordering::Relaxed))
}

/// Call at launch, before any tab
pub fn init(settings: &Settings) {
    ON.store(settings.color_contrast, Ordering::Relaxed);
    REV.store(now(), Ordering::Relaxed);
}

/// `PAGE_JS`, for a new Figma tab
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_COLOR_CONTRAST__", &state())
}

/// Sends a changed setting to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    if ON.swap(settings.color_contrast, Ordering::Relaxed) == settings.color_contrast {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaColorContrast?.({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
