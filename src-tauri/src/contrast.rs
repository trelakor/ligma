//! Color contrast (Settings > Features > Colors): while the color picker of
//! a solid fill or stroke is open, a row at its bottom shows the color's
//! WCAG 2 contrast ratio against what's behind the layer, and the level it
//! passes: AAA and AA at 7:1 and 4.5:1 for text, 4.5:1 and 3:1 for large
//! text (24px, or 18.66px bold), and AA at 3:1 for other layers. On by
//! default
//!
//! What's behind is the page's background, then going down to the layer,
//! each parent's fills and the siblings below it that cover the whole
//! layer, then the layer's own fills below the one being edited. Over an
//! image or gradient there's no single color, so the row says so. Blend
//! modes, effects and the parents' opacity are left out
//!
//! Paints are read through Figma's console plugin API, `window.figma`. The
//! paint being edited is the Fill or Stroke row whose picker is open, found
//! by Figma's test ids and class names as seen on 2026-10-29 (see
//! adjustments.rs)

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

  // The color behind `node`, or null over an image or gradient
  const behind = (figma, node) => {
    const page = figma.currentPage;
    let color = overAll({ r: 1, g: 1, b: 1 }, page.backgrounds ?? []);
    const chain = [];
    for (let at = node; at && at !== page; at = at.parent) chain.unshift(at);
    const target = box(node);
    let parent = page;
    for (const child of chain) {
      for (const sibling of parent.children) {
        if (sibling === child) break;
        if (!sibling.visible || sibling.isMask || !covers(box(sibling), target)) continue;
        color = overAll(color, paintsOf(sibling, "fills"), sibling.opacity ?? 1);
      }
      if (child !== node) color = overAll(color, paintsOf(child, "fills"));
      parent = child;
    }
    return color;
  };

  const luminance = ({ r, g, b }) => {
    const linear = (c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
    return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
  };
  const large = (node) => {
    if (node.type !== "TEXT") return false;
    const size = typeof node.fontSize === "number" ? node.fontSize : 0;
    const weight = typeof node.fontWeight === "number" ? node.fontWeight : 400;
    return size >= 24 || (size >= 18.66 && weight >= 700);
  };
  const level = (node, ratio) => {
    if (node.type !== "TEXT") return ratio >= 3 ? "AA" : null;
    const [aaa, aa] = large(node) ? [4.5, 3] : [7, 4.5];
    return ratio >= aaa ? "AAA" : ratio >= aa ? "AA" : null;
  };

  // What the row shows: `{ ratio, level }`, `{ note }`, or null for no row
  const measure = () => {
    const figma = api();
    const node = figma?.currentPage.selection[0];
    if (!node) return null;
    const at = openPaint(node);
    const list = at && paintsOf(node, at.list);
    const paint = list?.[at.index];
    if (paint?.type !== "SOLID") return null;
    let color = behind(figma, node);
    if (color && at.list === "fills") color = overAll(color, list.slice(0, at.index));
    if (!color) return { note: "No contrast over an image or gradient" };
    const front = over(color, { ...paint, visible: true }, node.opacity ?? 1);
    const [light, dark] = [luminance(front), luminance(color)].sort((a, b) => b - a);
    const ratio = (light + 0.05) / (dark + 0.05);
    return { ratio, level: level(node, ratio) };
  };

  const PICKER = '[data-testid="paint-picker-v2-modal"]';
  const ROW_ATTR = "data-ligma-contrast";
  const render = () => {
    const picker = state.on && document.querySelector(PICKER);
    let row = document.querySelector(`[${ROW_ATTR}]`);
    let result = null;
    if (picker) {
      try {
        result = measure();
      } catch {}
    }
    if (!result) {
      row?.remove();
      return;
    }
    if (!row || !picker.contains(row)) {
      row?.remove();
      row = document.createElement("div");
      row.setAttribute(ROW_ATTR, "");
      row.dataset.tooltip = "WCAG contrast against what's behind the layer";
      row.dataset.tooltipType = "text";
      const solid = picker.querySelector('fieldset[value="SOLID"]');
      if (solid) solid.after(row);
      else picker.append(row);
    }
    const text = result.note
      ? `<span>${result.note}</span>`
      : `<span>Contrast</span><b>${(Math.floor(result.ratio * 100) / 100).toFixed(2)}:1</b>` +
        `<span data-ligma-contrast-level="${result.level ? "pass" : "fail"}">${result.level ?? "Fails"}</span>`;
    if (row.innerHTML !== text) row.innerHTML = text;
  };

  const style = document.createElement("style");
  style.textContent = `
    [${ROW_ATTR}] {
      display: flex; align-items: center; gap: 8px; height: 32px; padding: 0 16px;
      border-top: 1px solid var(--color-border, rgba(0, 0, 0, 0.1));
      font-size: 11px; color: var(--color-text-secondary, rgba(0, 0, 0, 0.5));
    }
    [${ROW_ATTR}] b { font-weight: 500; color: var(--color-text, rgba(0, 0, 0, 0.9)); font-variant-numeric: tabular-nums; }
    [${ROW_ATTR}] [data-ligma-contrast-level] { margin-left: auto; font-weight: 500; }
    [${ROW_ATTR}] [data-ligma-contrast-level="pass"] { color: var(--color-text-success, #14ae5c); }
    [${ROW_ATTR}] [data-ligma-contrast-level="fail"] { color: var(--color-text-danger, #f24822); }
  `;
  const mount = () => {
    if (style.isConnected) return true;
    const parent = document.head ?? document.documentElement;
    parent?.append(style);
    return !!parent;
  };
  if (!mount()) new MutationObserver((_, observer) => mount() && observer.disconnect()).observe(document, { childList: true });

  // Dragging in the picker changes the paint without changing the page,
  // so while it's open the row is checked a few times a second
  let timer;
  const watch = () => {
    const open = state.on && !!document.querySelector(PICKER);
    if (open && !timer) timer = setInterval(render, 150);
    if (!open && timer) {
      clearInterval(timer);
      timer = undefined;
    }
    render();
  };
  let queued = false;
  new MutationObserver(() => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      watch();
    });
  }).observe(document.documentElement, { childList: true, subtree: true });

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
