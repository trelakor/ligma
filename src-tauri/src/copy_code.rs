//! Copy as code without Dev Mode (Settings > Features > Code): in Figma's
//! right-click menu, Copy/Paste as > Copy as code, which asks for Dev Mode
//! on plans without it, gives way to Copy as CSS and Copy as Tailwind.
//! Each copies the selected layers' CSS, as Dev Mode shows it, from the
//! plugin API's `getCSSAsync`, which every plan has. Tailwind is made from
//! that CSS, with arbitrary values where Tailwind's scale has none. On by
//! default
//!
//! The CSS is read through Figma's console plugin API, `window.figma`,
//! which Figma only offers in files that can be edited, so view-only files
//! keep Figma's own item. Menu items are found by role and Figma's English
//! labels, Copy as code or else Copy as PNG to go next to, as seen on
//! 2026-10-29

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::settings::Settings;

/// Runs in Figma tabs. `__LIGMA_COPY_CODE__` is `{ on, rev }`: the setting
/// when the tab was made. A page loaded later in the same tab may find a
/// newer one in `localStorage`; the higher `rev` wins
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  const KEY = "ligma-copy-code";
  let state = __LIGMA_COPY_CODE__;
  try {
    const stored = JSON.parse(localStorage.getItem(KEY));
    if (stored?.rev > state.rev) state = stored;
  } catch {}

  // The plugin API, if the file can be edited
  const api = () => {
    const figma = window.figma;
    try {
      return figma?.currentPage && figma.editorType === "figma" ? figma : null;
    } catch {
      return null;
    }
  };
  let toast;
  const notify = (figma, message) => {
    try {
      toast?.cancel();
    } catch {}
    toast = figma.notify(message, { timeout: 2000 });
  };

  // Tailwind's spacing scale, in steps of 4px
  const SCALE = new Set([0.5, 1, 1.5, 2, 2.5, 3, 3.5, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64, 72, 80, 96]);
  const RADII = { "0": "none", "2px": "sm", "4px": "", "6px": "md", "8px": "lg", "12px": "xl", "16px": "2xl", "24px": "3xl", "9999px": "full" };
  const WEIGHTS = { 100: "thin", 200: "extralight", 300: "light", 400: "normal", 500: "medium", 600: "semibold", 700: "bold", 800: "extrabold", 900: "black" };
  const ALIGN = { "flex-start": "start", "flex-end": "end", start: "start", end: "end", center: "center", baseline: "baseline", stretch: "stretch" };
  const JUSTIFY = { ...ALIGN, "space-between": "between", "space-around": "around", "space-evenly": "evenly" };
  const KEYWORDS = {
    display: { flex: "flex", "inline-flex": "inline-flex", grid: "grid", "inline-grid": "inline-grid", block: "block", "inline-block": "inline-block", inline: "inline", none: "hidden" },
    "flex-direction": { row: "flex-row", column: "flex-col", "row-reverse": "flex-row-reverse", "column-reverse": "flex-col-reverse" },
    "flex-wrap": { wrap: "flex-wrap", nowrap: "flex-nowrap", "wrap-reverse": "flex-wrap-reverse" },
    position: { absolute: "absolute", relative: "relative", fixed: "fixed", sticky: "sticky", static: "static" },
    "text-align": { left: "text-left", center: "text-center", right: "text-right", justify: "text-justify", start: "text-start", end: "text-end" },
    "text-transform": { uppercase: "uppercase", lowercase: "lowercase", capitalize: "capitalize", none: "normal-case" },
    "text-decoration-line": { underline: "underline", "line-through": "line-through", none: "no-underline" },
    "font-style": { italic: "italic", normal: "" },
    overflow: { hidden: "overflow-hidden", visible: "overflow-visible", auto: "overflow-auto", scroll: "overflow-scroll", clip: "overflow-clip" },
    "box-sizing": { "border-box": "box-border", "content-box": "box-content" },
    flex: { "1 1 0%": "flex-1", "1 1 auto": "flex-auto", "0 1 auto": "flex-initial", none: "flex-none" },
    "flex-grow": { 0: "grow-0", 1: "grow" },
    "flex-shrink": { 0: "shrink-0", 1: "shrink" },
  };
  KEYWORDS["text-decoration"] = KEYWORDS["text-decoration-line"];

  // A value in Tailwind's brackets: underscores for spaces
  const bracket = (value) => `[${value.replace(/_/g, "\\_").replace(/\s*,\s*/g, ",").replace(/\s+/g, "_")}]`;
  const space = (value) => {
    const px = /^(\d*\.?\d+)px$/.exec(value)?.[1];
    if (value === "0" || px === "0") return "0";
    if (px === "1") return "px";
    if (px && SCALE.has(Number(px) / 4)) return String(Number(px) / 4);
    return bracket(value);
  };
  const size = (value) => ({ "100%": "full", auto: "auto", "fit-content": "fit", "min-content": "min", "max-content": "max" })[value] ?? space(value);
  // A color, told from other values by Tailwind unless it's a `var()`
  const color = (value) => (/^var\(/.test(value) ? bracket(`color:${value}`) : bracket(value));
  const isColor = (value) => /^(#[\da-f]{3,8}|(rgba?|hsla?)\([^)]*\)|var\([^)]*\)|[a-z]+)$/i.test(value);
  // Sides as CSS lists them: 1 to 4 values
  const sides = (prefix, value) => {
    const [top, right = top, bottom = top, left = right] = value.split(/\s+/).map(space);
    if (top === right && top === bottom && top === left) return [`${prefix}-${top}`];
    if (top === bottom && left === right) return [`${prefix}y-${top}`, `${prefix}x-${left}`];
    return [`${prefix}t-${top}`, `${prefix}r-${right}`, `${prefix}b-${bottom}`, `${prefix}l-${left}`];
  };

  const tailwind = (property, value) => {
    const keyword = KEYWORDS[property]?.[value];
    if (keyword !== undefined) return keyword ? [keyword] : [];
    const side = { top: "top", right: "right", bottom: "bottom", left: "left" }[property];
    if (side) return [/^-/.test(value) ? `${side}-${bracket(value)}` : `${side}-${space(value)}`];
    switch (property) {
      case "width":
        return [`w-${size(value)}`];
      case "height":
        return [`h-${size(value)}`];
      case "min-width":
        return [`min-w-${size(value)}`];
      case "max-width":
        return [`max-w-${size(value)}`];
      case "min-height":
        return [`min-h-${size(value)}`];
      case "max-height":
        return [`max-h-${size(value)}`];
      case "padding":
        return sides("p", value);
      case "margin":
        return sides("m", value);
      case "padding-top":
      case "padding-right":
      case "padding-bottom":
      case "padding-left":
      case "margin-top":
      case "margin-right":
      case "margin-bottom":
      case "margin-left":
        return [`${property[0]}${property.split("-")[1][0]}-${space(value)}`];
      case "gap": {
        const [row, column = row] = value.split(/\s+/);
        return row === column ? [`gap-${space(row)}`] : [`gap-y-${space(row)}`, `gap-x-${space(column)}`];
      }
      case "row-gap":
        return [`gap-y-${space(value)}`];
      case "column-gap":
        return [`gap-x-${space(value)}`];
      case "justify-content":
        if (JUSTIFY[value]) return [`justify-${JUSTIFY[value]}`];
        break;
      case "align-items":
        if (ALIGN[value]) return [`items-${ALIGN[value]}`];
        break;
      case "align-self":
        if (ALIGN[value]) return [`self-${ALIGN[value]}`];
        break;
      case "align-content":
        if (JUSTIFY[value]) return [`content-${JUSTIFY[value]}`];
        break;
      case "flex":
      case "flex-grow":
      case "flex-shrink":
        return [`${{ flex: "flex", "flex-grow": "grow", "flex-shrink": "shrink" }[property]}-${bracket(value)}`];
      case "flex-basis":
        return [`basis-${size(value)}`];
      case "border-radius":
        if (value in RADII) return [RADII[value] ? `rounded-${RADII[value]}` : "rounded"];
        return [`rounded-${bracket(value)}`];
      case "background":
      case "background-color":
        if (isColor(value)) return [`bg-${color(value)}`];
        break;
      case "color":
        return [`text-${color(value)}`];
      case "font-family":
        return [`font-${bracket(`'${value.split(",")[0].trim().replace(/^["']|["']$/g, "")}'`)}`];
      case "font-size":
        return [`text-${bracket(value)}`];
      case "font-weight":
        return [WEIGHTS[value] ? `font-${WEIGHTS[value]}` : `font-${bracket(value)}`];
      case "line-height":
        return [value === "normal" ? "leading-normal" : `leading-${bracket(value)}`];
      case "letter-spacing":
        return [`tracking-${bracket(value)}`];
      case "opacity":
        return [`opacity-${bracket(value)}`];
      case "box-shadow":
        return [`shadow-${bracket(value)}`];
      case "z-index":
        return [`z-${bracket(value)}`];
      case "aspect-ratio":
        return [`aspect-${bracket(value.replace(/\s*\/\s*/, "/"))}`];
      case "mix-blend-mode":
        return [`mix-blend-${value}`];
      case "border": {
        const match = /^(\S+)\s+(solid|dashed|dotted|double)\s+(.+)$/.exec(value);
        if (match) return [`border-${bracket(match[1])}`, `border-${match[2]}`, `border-${color(match[3])}`];
        break;
      }
    }
    // Anything else as an arbitrary property
    return [`[${property}:${bracket(value).slice(1)}`];
  };

  // `{ name, css }` for each selected layer, `css` as `getCSSAsync` gives it
  // with Dev Mode's `/* … */` notes left out. Layers it can't read are
  // skipped
  const read = async (nodes) => {
    const layers = [];
    for (const node of nodes) {
      let raw;
      try {
        raw = await node.getCSSAsync();
      } catch {
        continue;
      }
      const css = Object.entries(raw)
        .map(([property, value]) => [property, String(value).replace(/\/\*.*?\*\//g, "").trim()])
        .filter(([, value]) => value);
      layers.push({ name: node.name, css });
    }
    return layers;
  };
  const format = (layers, kind) =>
    layers
      .map(({ name, css }) => {
        const body =
          kind === "tailwind"
            ? css.flatMap(([property, value]) => tailwind(property, value)).join(" ")
            : css.map(([property, value]) => `${property}: ${value};`).join("\n");
        return layers.length > 1 ? `/* ${name.replace(/\*\//g, "*\\/")} */\n${body}` : body;
      })
      .join("\n\n");

  const LABELS = { css: "CSS", tailwind: "Tailwind" };
  // Started from the click, for the clipboard to accept text that's still
  // being read. WebKit and Chromium both take a promise in a ClipboardItem
  const copy = (kind) => {
    const figma = api();
    if (!figma) return;
    if (!figma.currentPage.selection.length) {
      notify(figma, "Nothing selected");
      return;
    }
    const text = read([...figma.currentPage.selection]).then((layers) => {
      if (!layers.length) throw new Error("no CSS");
      return format(layers, kind);
    });
    const written =
      typeof ClipboardItem === "function"
        ? navigator.clipboard.write([new ClipboardItem({ "text/plain": text.then((text) => new Blob([text], { type: "text/plain" })) })])
        : text.then((text) => navigator.clipboard.writeText(text));
    written
      .catch(() => text.then((text) => navigator.clipboard.writeText(text)))
      .then(() => notify(figma, `Copied as ${LABELS[kind]}`))
      .catch(() => notify(figma, `Couldn't copy as ${LABELS[kind]}`));
  };

  // Figma's menu items, by role and class. `label` reads an item's name
  // without its shortcut
  const ITEM = '[role="menuitem"], [class*="multilevel--option"]';
  const nameOf = (item) => {
    const walker = document.createTreeWalker(item, NodeFilter.SHOW_TEXT);
    for (let text = walker.nextNode(); text; text = walker.nextNode()) {
      if (text.data.trim()) return text;
    }
    return null;
  };
  const label = (item) => nameOf(item)?.data.trim() ?? "";
  const HIDDEN = "data-ligma-code-hidden";

  // An item like `like`, with its spacing and name but without its
  // shortcut, badge or arrow
  const makeItem = (like, kind) => {
    const item = like.cloneNode(true);
    item.removeAttribute(HIDDEN);
    item.removeAttribute("id");
    item.removeAttribute("aria-haspopup");
    item.removeAttribute("aria-expanded");
    item.removeAttribute("data-testid");
    item.dataset.ligmaCode = kind;
    const name = nameOf(item);
    if (name) {
      name.data = `Copy as ${LABELS[kind]}`;
      for (let node = name; node !== item; node = node.parentNode) {
        while (node.nextSibling) node.nextSibling.remove();
      }
    } else {
      item.textContent = `Copy as ${LABELS[kind]}`;
    }
    return item;
  };

  const decorate = () => {
    const on = state.on && !!api();
    if (!on) {
      for (const node of document.querySelectorAll("[data-ligma-code]")) node.remove();
      for (const node of document.querySelectorAll(`[${HIDDEN}]`)) node.removeAttribute(HIDDEN);
      return;
    }
    const items = [...document.querySelectorAll(ITEM)].filter((item) => !item.dataset.ligmaCode);
    // Figma's Copy as code, or else Copy as PNG to follow
    const code = items.find((item) => /^Copy as code\b/i.test(label(item)));
    const anchor = code ?? items.find((item) => /^Copy as PNG\b/i.test(label(item)));
    if (!anchor || anchor.parentElement.querySelector(":scope > [data-ligma-code]")) return;
    if (code) code.setAttribute(HIDDEN, "");
    const css = makeItem(anchor, "css");
    const tailwind = makeItem(anchor, "tailwind");
    if (code) code.before(css, tailwind);
    else anchor.after(css, tailwind);
  };

  // Figma closes its menu on Escape, sent from within the menu so it
  // doesn't reach the canvas first
  const closeMenu = (item) => {
    const target = item.closest('[role="menu"]') ?? item.parentElement;
    for (const type of ["keydown", "keyup"]) {
      target.dispatchEvent(new KeyboardEvent(type, { key: "Escape", code: "Escape", keyCode: 27, which: 27, bubbles: true, cancelable: true }));
    }
  };
  // Ligma's items keep Figma's handlers out, and copy on click
  for (const type of ["pointerdown", "mousedown", "pointerup", "mouseup", "click"]) {
    addEventListener(
      type,
      (event) => {
        const item = event.target.closest?.("[data-ligma-code]");
        if (!item) return;
        event.preventDefault();
        event.stopImmediatePropagation();
        if (type !== "click" || event.button !== 0) return;
        copy(item.dataset.ligmaCode);
        closeMenu(item);
      },
      true,
    );
  }

  const style = document.createElement("style");
  style.textContent = `
    [${HIDDEN}] { display: none !important; }
    [data-ligma-code]:hover { background-color: var(--color-bg-menu-selected, var(--color-bg-brand, #0d99ff)) !important; color: var(--color-text-menu-selected, var(--color-text-onbrand, #fff)) !important; }
  `;
  const mount = () => {
    if (style.isConnected) return true;
    const parent = document.head ?? document.documentElement;
    parent?.append(style);
    return !!parent;
  };
  if (!mount()) new MutationObserver((_, observer) => mount() && observer.disconnect()).observe(document, { childList: true });

  let queued = false;
  new MutationObserver(() => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      decorate();
    });
  }).observe(document.documentElement, { childList: true, subtree: true });

  // From Ligma, when the setting changes
  window.__ligmaCopyCode = (next) => {
    try {
      localStorage.setItem(KEY, JSON.stringify(next));
    } catch {}
    state = next;
    decorate();
  };
  // From another tab, or while this one was frozen and missed it
  const sync = () => {
    try {
      const stored = JSON.parse(localStorage.getItem(KEY));
      if (stored?.rev > state.rev) {
        state = stored;
        decorate();
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
    ON.store(settings.copy_as_code, Ordering::Relaxed);
    REV.store(now(), Ordering::Relaxed);
}

/// `PAGE_JS`, for a new Figma tab
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_COPY_CODE__", &state())
}

/// Sends a changed setting to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    if ON.swap(settings.copy_as_code, Ordering::Relaxed) == settings.copy_as_code {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaCopyCode?.({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
