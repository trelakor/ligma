//! Effects library (Settings > Features > Toolbar): a button in Figma
//! Design's toolbar, after Actions, that lists effects Ligma makes in the
//! file, each with a panel drawn as Figma's own are, using its colors,
//! type and controls. On by default
//!
//! Effects run in the page through Figma's console plugin API,
//! `window.figma`, rather than as plugins, so there's no plugin window:
//! what they make shows on the canvas as the panel changes it, as real
//! layers, and Cancel takes it away. Applying is one undo step. Figma only
//! offers that API in files that can be edited, so the button isn't added
//! in others
//!
//! Repeater repeats the selected layer: along a trail, each copy a step
//! on, turned and curving as it goes; around a ring; or in a grid. The
//! copies can grow or shrink, fade and blend toward a color from the first
//! to the last, evenly or easing in or out. With Keep editable, the
//! original and its copies are kept in a group Repeater knows: select it
//! and open Repeater to change it, and the copies are made again from the
//! original as it is then, so they follow changes to it

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::settings::Settings;

/// Runs in Figma tabs. `__LIGMA_EFFECTS__` is `{ on, rev }`: the setting
/// when the tab was made. A page loaded later in the same tab may find a
/// newer one in `localStorage`; the higher `rev` wins
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  const KEY = "ligma-effects";
  let state = __LIGMA_EFFECTS__;
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

  const BUTTON = "data-ligma-effects";
  const ROOT = "data-ligma-effects-ui";
  const STROKE = 'stroke="var(--fpl-icon-color, var(--color-icon))" stroke-linecap="round" stroke-linejoin="round"';
  // Sparkles, drawn as Figma's toolbar icons are: one-pixel outlines
  const ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24" data-fpl-icon-size="24"><path ${STROKE} d="M10 6.5c.55 3.9 1.6 4.95 5.5 5.5-3.9.55-4.95 1.6-5.5 5.5-.55-3.9-1.6-4.95-5.5-5.5 3.9-.55 4.95-1.6 5.5-5.5Z"/><path ${STROKE} d="M17 4.5c.24 1.6.9 2.26 2.5 2.5-1.6.24-2.26.9-2.5 2.5-.24-1.6-.9-2.26-2.5-2.5 1.6-.24 2.26-.9 2.5-2.5Z"/></svg>`;
  // Three cards, each a step on from the last
  const REPEATER_ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><rect ${STROKE} x="4.5" y="4.5" width="9" height="9" rx="1.5"/><path ${STROKE} d="M16.5 7.5v7a2 2 0 0 1-2 2h-7"/><path ${STROKE} d="M19.5 10.5v7a2 2 0 0 1-2 2h-7"/></svg>`;
  const CHEVRON = `<svg width="16" height="16" fill="none" viewBox="0 0 16 16"><path ${STROKE} d="m5.5 7 2.5 2.5L10.5 7"/></svg>`;
  const CLOSE = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><path ${STROKE} d="m8 8 8 8m0-8-8 8"/></svg>`;
  const CHECK = `<svg width="16" height="16" fill="none" viewBox="0 0 16 16"><path ${STROKE} d="m4.5 8.5 2.5 2.5 4.5-5.5"/></svg>`;
  // Field glyphs, 16 pixels, in the field's secondary text color
  const glyph = (d) => `<svg width="16" height="16" fill="none" viewBox="0 0 16 16"><path stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" d="${d}"/></svg>`;
  const GLYPHS = {
    copies: glyph("M3.5 3.5h6v6h-6zM6.5 12.5h6v-6"),
    turn: glyph("M12.5 8a4.5 4.5 0 1 1-1.3-3.2M12.5 3v2.5H10"),
    curve: glyph("M3.5 12.5C3.5 7 7 3.5 12.5 3.5"),
    radius: glyph("M8 8h4.5M8 8m-1 0a1 1 0 1 0 2 0a1 1 0 1 0-2 0M3.5 8a4.5 4.5 0 0 1 9 0"),
    start: glyph("M3.5 12.5h9M3.5 12.5l6-7.5"),
    sweep: glyph("M3.5 10a4.5 4.5 0 0 1 9 0M12.5 10l-1.5-1.5M12.5 10l1-1.8"),
    columns: glyph("M3.5 3.5v9M8 3.5v9M12.5 3.5v9"),
    rows: glyph("M3.5 3.5h9M3.5 8h9M3.5 12.5h9"),
    gapX: glyph("M2.5 4v8M13.5 4v8M5 8h6M6.5 6.5 5 8l1.5 1.5M9.5 6.5 11 8l-1.5 1.5"),
    gapY: glyph("M4 2.5h8M4 13.5h8M8 5v6M6.5 6.5 8 5l1.5 1.5M6.5 9.5 8 11l1.5-1.5"),
    size: glyph("M3.5 6.5v-3h3M12.5 9.5v3h-3M3.5 3.5l3.5 3.5M12.5 12.5 9 9"),
    opacity: glyph("M8 3.5a4.5 4.5 0 1 0 0 9zM8 3.5a4.5 4.5 0 1 1 0 9"),
    amount: glyph("M3.5 12.5l9-9M5 4.5m-1 0a1 1 0 1 0 2 0a1 1 0 1 0-2 0M11 11.5m-1 0a1 1 0 1 0 2 0a1 1 0 1 0-2 0"),
  };
  const PRESETS_ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><path ${STROKE} d="M6.5 8.5h11M6.5 12h11M6.5 15.5h7"/></svg>`;

  const style = document.createElement("style");
  style.textContent = `
    [${BUTTON}][aria-expanded="true"] { background: var(--color-bg-pressed, #444) !important; }
    [${ROOT}] {
      font-family: var(--font-family-default, Inter, sans-serif);
      font-size: 11px;
      line-height: 16px;
      letter-spacing: 0.005em;
      color: var(--color-text);
      -webkit-font-smoothing: antialiased;
    }
    [${ROOT}] * { box-sizing: border-box; }
    [${ROOT}] button { font: inherit; color: inherit; }
    /* The library, as Figma's menus are */
    [${ROOT}="menu"] {
      position: fixed;
      z-index: 9;
      width: 240px;
      padding: 8px;
      border-radius: var(--radius-large, 13px);
      background: var(--color-bg-menu, #1e1e1e);
      color: var(--color-text-menu, #fff);
      box-shadow: var(--elevation-500);
    }
    [${ROOT}="menu"] .heading { padding: 0 8px; height: 24px; display: flex; align-items: center; color: var(--color-text-menu-secondary, #ffffffb2); }
    [${ROOT}="menu"] .item {
      display: grid;
      grid-template-columns: 24px 1fr;
      column-gap: 4px;
      width: 100%;
      padding: 4px 8px 4px 0;
      border: 0;
      border-radius: 5px;
      background: none;
      text-align: left;
      cursor: default;
    }
    [${ROOT}="menu"] .item:hover, [${ROOT}="menu"] .item:focus-visible { background: var(--color-bg-menu-selected, #0c8ce9); outline: 0; }
    [${ROOT}="menu"] .item svg { grid-row: span 2; margin: -4px 0 0 4px; }
    [${ROOT}="menu"] .item svg [stroke] { stroke: currentColor; }
    [${ROOT}="menu"] .item .name { font-weight: 500; }
    [${ROOT}="menu"] .item .about { color: var(--color-text-menu-secondary, #ffffffb2); }
    [${ROOT}="menu"] .item:hover .about { color: inherit; }
    /* An effect's panel, as Figma's own panels are */
    [${ROOT}="panel"] {
      position: fixed;
      z-index: 8;
      width: 248px;
      max-height: calc(100vh - 120px);
      display: flex;
      flex-direction: column;
      border-radius: var(--radius-large, 13px);
      background: var(--color-bg);
      box-shadow: var(--elevation-500);
      overflow: hidden;
    }
    [${ROOT}="panel"] .title {
      display: flex;
      align-items: center;
      height: 40px;
      padding: 0 8px 0 16px;
      border-bottom: 1px solid var(--color-border);
      font-weight: 600;
      cursor: grab;
      user-select: none;
    }
    [${ROOT}="panel"] .title span { flex: 1; }
    [${ROOT}="panel"] .scroll { overflow-y: auto; }
    [${ROOT}="panel"] .icon-button {
      display: grid;
      place-items: center;
      width: 24px;
      height: 24px;
      padding: 0;
      border: 0;
      border-radius: 5px;
      background: none;
    }
    [${ROOT}="panel"] .icon-button:hover, [${ROOT}="panel"] .icon-button[aria-expanded="true"] { background: var(--color-bg-hover); }
    [${ROOT}="panel"] section { padding: 8px 16px 12px; border-bottom: 1px solid var(--color-border); }
    [${ROOT}="panel"] section:last-of-type { border-bottom: 0; }
    [${ROOT}="panel"] .section-title { display: flex; align-items: center; justify-content: space-between; height: 24px; margin-bottom: 4px; font-weight: 600; }
    [${ROOT}="panel"] .grid { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
    [${ROOT}="panel"] .grid + .grid, [${ROOT}="panel"] .grid + .line, [${ROOT}="panel"] .line + .grid, [${ROOT}="panel"] .segments + .grid { margin-top: 8px; }
    [${ROOT}="panel"] .field {
      display: flex;
      align-items: center;
      height: 24px;
      min-width: 0;
      border: 1px solid transparent;
      border-radius: 5px;
      background: var(--color-bg-secondary);
    }
    [${ROOT}="panel"] .field:hover { border-color: var(--color-border); }
    [${ROOT}="panel"] .field:focus-within { border-color: var(--color-border-selected, #0d99ff); }
    [${ROOT}="panel"] .field .glyph {
      flex: none;
      display: grid;
      place-items: center;
      width: 24px;
      height: 100%;
      color: var(--color-text-secondary);
      cursor: ew-resize;
      user-select: none;
    }
    [${ROOT}="panel"] .field input {
      flex: 1;
      min-width: 0;
      height: 100%;
      padding: 0 6px 0 0;
      border: 0;
      outline: 0;
      background: none;
      color: inherit;
      font: inherit;
      font-variant-numeric: tabular-nums;
    }
    [${ROOT}="panel"] .field input::selection { background: var(--color-bg-selected-strong, #0d99ff66); }
    [${ROOT}="panel"] .segments { display: flex; padding: 2px; gap: 2px; border-radius: 5px; background: var(--color-bg-secondary); }
    [${ROOT}="panel"] .segments button {
      flex: 1;
      height: 20px;
      padding: 0 6px;
      border: 0;
      border-radius: 4px;
      background: none;
      color: var(--color-text-secondary);
    }
    [${ROOT}="panel"] .segments button:hover { color: var(--color-text); }
    [${ROOT}="panel"] .segments button[aria-checked="true"] { background: var(--color-bg); color: var(--color-text); box-shadow: 0 0 0 1px var(--color-border); }
    [${ROOT}="panel"] .line { display: flex; align-items: center; gap: 8px; min-height: 24px; }
    [${ROOT}="panel"] .line .label { flex: 1; color: var(--color-text-secondary); }
    [${ROOT}="panel"] .select {
      display: flex;
      align-items: center;
      justify-content: space-between;
      width: 112px;
      height: 24px;
      padding: 0 2px 0 8px;
      border: 1px solid transparent;
      border-radius: 5px;
      background: var(--color-bg-secondary);
      text-align: left;
    }
    [${ROOT}="panel"] .select:hover, [${ROOT}="panel"] .select[aria-expanded="true"] { border-color: var(--color-border); }
    [${ROOT}="panel"] .check { display: flex; align-items: center; gap: 8px; min-height: 24px; cursor: default; user-select: none; }
    [${ROOT}="panel"] .check input { width: 12px; height: 12px; margin: 0; accent-color: var(--color-bg-brand, #0d99ff); }
    [${ROOT}="panel"] .swatch {
      flex: none;
      width: 16px;
      height: 16px;
      margin-left: 4px;
      padding: 0;
      border: 0;
      border-radius: 3px;
      box-shadow: inset 0 0 0 1px var(--color-bordertranslucent, #0000001a);
    }
    [${ROOT}="panel"] .field.color { gap: 4px; }
    [${ROOT}="panel"] .field.color input[type="color"] { position: absolute; width: 0; height: 0; opacity: 0; pointer-events: none; }
    [${ROOT}="panel"] .footer {
      display: flex;
      align-items: center;
      gap: 8px;
      padding: 12px 16px;
      border-top: 1px solid var(--color-border);
    }
    [${ROOT}="panel"] .footer .count { flex: 1; color: var(--color-text-secondary); font-variant-numeric: tabular-nums; }
    [${ROOT}="panel"] .button {
      height: 24px;
      padding: 0 8px;
      border: 1px solid var(--color-border);
      border-radius: 5px;
      background: none;
    }
    [${ROOT}="panel"] .button:hover { background: var(--color-bg-hover); }
    [${ROOT}="panel"] .button.primary { border-color: transparent; background: var(--color-bg-brand, #0d99ff); color: var(--color-text-onbrand, #fff); }
    [${ROOT}="panel"] .button.primary:hover { background: var(--color-bg-brand-hover, var(--color-bg-brand, #0d99ff)); }
    [${ROOT}="panel"] :focus-visible { outline: 1px solid var(--color-border-selected, #0d99ff); outline-offset: -1px; }
    [${ROOT}="panel"] [hidden] { display: none !important; }
  `;

  // The effects the library lists
  const EFFECTS = [
    {
      id: "repeater",
      name: "Repeater",
      about: "Repeat a layer in a trail, ring or grid",
      icon: REPEATER_ICON,
      open: (figma) => openRepeater(figma),
    },
  ];

  // ---------- The toolbar button and the library ----------

  let menu = null;
  const button = () => document.querySelector(`[${BUTTON}]`);
  const closeMenu = () => {
    menu?.remove();
    menu = null;
    button()?.setAttribute("aria-expanded", "false");
    removeEventListener("pointerdown", outsideMenu, true);
  };
  const outsideMenu = (event) => {
    if (menu && !menu.contains(event.target) && !button()?.contains(event.target)) closeMenu();
  };
  const openMenu = () => {
    const figma = api();
    const anchor = button();
    if (!figma || !anchor) return;
    // A Repeater group selected opens it to change
    const selected = figma.currentPage.selection;
    if (selected.length === 1 && repeaterOf(selected[0])) {
      openRepeater(figma);
      return;
    }
    menu = document.createElement("div");
    menu.setAttribute(ROOT, "menu");
    menu.setAttribute("role", "menu");
    menu.setAttribute("aria-label", "Effects");
    menu.innerHTML = `<div class="heading">Effects</div>`;
    for (const effect of EFFECTS) {
      const item = menu.appendChild(document.createElement("button"));
      item.type = "button";
      item.className = "item";
      item.setAttribute("role", "menuitem");
      item.innerHTML = `${effect.icon}<span class="name">${effect.name}</span><span class="about">${effect.about}</span>`;
      item.addEventListener("click", () => {
        closeMenu();
        effect.open(figma);
      });
    }
    document.body.append(menu);
    const box = anchor.getBoundingClientRect();
    menu.style.left = `${Math.max(8, Math.min(innerWidth - menu.offsetWidth - 8, box.left + box.width / 2 - menu.offsetWidth / 2))}px`;
    menu.style.top = `${box.top - menu.offsetHeight - 12}px`;
    anchor.setAttribute("aria-expanded", "true");
    addEventListener("pointerdown", outsideMenu, true);
    menu.querySelector(".item")?.focus();
  };

  // Keeps the button after Actions, as Figma draws its toolbar again
  const place = () => {
    const actions = document.querySelector('[data-testid="Actions-tool"]');
    const mine = button();
    if (!state.on || !actions || !api()) {
      mine?.remove();
      return;
    }
    if (mine && mine.previousElementSibling === actions) return;
    const made = mine ?? document.createElement("button");
    if (!mine) {
      made.type = "button";
      made.setAttribute(BUTTON, "");
      made.setAttribute("aria-label", "Effects");
      made.setAttribute("aria-haspopup", "menu");
      made.setAttribute("aria-expanded", "false");
      Object.assign(made.dataset, { tooltip: "Effects", tooltipType: "text", tooltipShowAbove: "true" });
      made.innerHTML = ICON;
      made.addEventListener("click", () => (menu ? closeMenu() : openMenu()));
    }
    made.className = actions.className;
    actions.after(made);
  };

  // ---------- Controls drawn as Figma's ----------

  const element = (tag, props = {}, ...children) => {
    const node = Object.assign(document.createElement(tag), props);
    node.append(...children);
    return node;
  };

  // A number field: a glyph that scrubs it when dragged, as Figma's do,
  // and the value with its unit. Arrow keys step it, Shift by 10
  const numberField = ({ glyph, title, unit = "", min = -Infinity, max = Infinity, step = 1, get, set }) => {
    const input = element("input", { type: "text", inputMode: "decimal", spellcheck: false });
    input.setAttribute("aria-label", title);
    const mark = element("span", { className: "glyph", title });
    if (glyph.startsWith("<")) mark.innerHTML = glyph;
    else mark.textContent = glyph;
    const field = element("label", { className: "field", title }, mark, input);
    const clamp = (value) => Math.min(max, Math.max(min, value));
    const show = () => {
      if (document.activeElement !== input) input.value = `${Math.round(get() * 100) / 100}${unit}`;
    };
    const commit = (value) => {
      if (!Number.isFinite(value)) return show();
      set(clamp(Math.round(value / step) * step));
      show();
    };
    input.addEventListener("change", () => commit(parseFloat(input.value)));
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter") input.blur();
      else if (event.key === "Escape") {
        event.stopPropagation();
        input.blur();
        show();
      } else if (event.key === "ArrowUp" || event.key === "ArrowDown") {
        event.preventDefault();
        commit(get() + (event.key === "ArrowUp" ? 1 : -1) * step * (event.shiftKey ? 10 : 1));
        input.select();
      }
    });
    input.addEventListener("blur", show);
    mark.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      event.preventDefault();
      mark.setPointerCapture(event.pointerId);
      const [from, start] = [event.clientX, get()];
      const move = (event) => commit(start + Math.round((event.clientX - from) / 2) * step * (event.shiftKey ? 10 : 1));
      mark.addEventListener("pointermove", move);
      mark.addEventListener("lostpointercapture", () => mark.removeEventListener("pointermove", move), { once: true });
    });
    field.show = show;
    return field;
  };

  // A segmented control: one of a few, side by side
  const segments = (label, options, get, set) => {
    const group = element("div", { className: "segments" });
    group.setAttribute("role", "radiogroup");
    group.setAttribute("aria-label", label);
    for (const [value, text, tip] of options) {
      const option = group.appendChild(element("button", { type: "button", textContent: text }));
      option.setAttribute("role", "radio");
      option.dataset.value = value;
      if (tip) option.title = tip;
      option.addEventListener("click", () => set(value));
    }
    group.show = () => {
      for (const option of group.children) option.setAttribute("aria-checked", String(option.dataset.value === String(get())));
    };
    return group;
  };

  // A dropdown, opening a menu as Figma's do, the current choice checked
  const select = (label, options, get, set) => {
    const text = element("span");
    const trigger = element("button", { type: "button", className: "select" }, text);
    trigger.insertAdjacentHTML("beforeend", CHEVRON);
    trigger.setAttribute("aria-haspopup", "listbox");
    trigger.setAttribute("aria-label", label);
    let list = null;
    const close = () => {
      list?.remove();
      list = null;
      trigger.setAttribute("aria-expanded", "false");
      removeEventListener("pointerdown", outside, true);
    };
    const outside = (event) => list && !list.contains(event.target) && !trigger.contains(event.target) && close();
    trigger.addEventListener("click", () => {
      if (list) return close();
      list = element("div");
      list.setAttribute(ROOT, "menu");
      list.setAttribute("role", "listbox");
      list.style.width = "auto";
      list.style.minWidth = `${trigger.offsetWidth + 32}px`;
      for (const [value, name] of options) {
        const item = list.appendChild(element("button", { type: "button", className: "item" }));
        item.style.gridTemplateColumns = "16px 1fr";
        item.style.padding = "4px 8px";
        item.innerHTML = `${value === get() ? CHECK : "<span></span>"}<span class="name">${name}</span>`;
        item.querySelector("svg")?.style.setProperty("margin", "0");
        item.addEventListener("click", () => {
          close();
          set(value);
        });
      }
      document.body.append(list);
      const box = trigger.getBoundingClientRect();
      const at = Math.max(0, options.findIndex(([value]) => value === get()));
      list.style.left = `${box.left - 24}px`;
      list.style.top = `${Math.max(8, Math.min(innerHeight - list.offsetHeight - 8, box.top - 8 - at * 24))}px`;
      trigger.setAttribute("aria-expanded", "true");
      addEventListener("pointerdown", outside, true);
    });
    trigger.show = () => (text.textContent = options.find(([value]) => value === get())?.[1] ?? "");
    trigger.close = close;
    return trigger;
  };

  // ---------- Repeater ----------

  const PLUGIN = "ligma";
  const SETTINGS = "repeater";
  const ROLE = "repeater-role";
  const MAX_COPIES = 400;

  const DEFAULTS = {
    mode: "trail",
    copies: 8,
    stepX: 24,
    stepY: 0,
    curve: 0,
    turn: 0,
    radius: 120,
    start: 0,
    sweep: 360,
    orient: true,
    columns: 4,
    rows: 3,
    gapX: 16,
    gapY: 16,
    endScale: 100,
    endOpacity: 100,
    ease: "even",
    blend: false,
    blendColor: "#0d99ff",
    blendAmount: 100,
    below: true,
    editable: true,
  };
  // Distances are in sizes of the layer, `u` its larger side, so a
  // preset fits a small icon and a large frame alike
  const PRESETS = [
    ["Trail", (u) => ({ mode: "trail", copies: 8, stepX: u * 0.6, stepY: 0, curve: 0, turn: 0, endScale: 100, endOpacity: 100 })],
    ["Fade out", (u) => ({ mode: "trail", copies: 6, stepX: u * 0.2, stepY: u * 0.1, curve: 0, turn: 0, endScale: 100, endOpacity: 0 })],
    ["Spiral", (u) => ({ mode: "trail", copies: 30, stepX: u * 0.45, stepY: 0, curve: 20, turn: 20, endScale: 25, endOpacity: 100, ease: "out" })],
    ["Arc", (u) => ({ mode: "trail", copies: 10, stepX: u * 0.5, stepY: 0, curve: 8, turn: 8, endScale: 100, endOpacity: 100 })],
    ["Ring", (u) => ({ mode: "radial", copies: 11, radius: u * 1.2, start: 0, sweep: 360, orient: true, endScale: 100, endOpacity: 100 })],
    ["Fan", (u) => ({ mode: "radial", copies: 6, radius: u * 2, start: -90, sweep: 90, orient: true, endScale: 100, endOpacity: 100 })],
    ["Grid", (u) => ({ mode: "grid", columns: 4, rows: 3, gapX: u * 0.15, gapY: u * 0.15, endScale: 100, endOpacity: 100 })],
    ["Echo", () => ({ mode: "trail", copies: 5, stepX: 0, stepY: 0, curve: 0, turn: 0, endScale: 160, endOpacity: 0, ease: "out" })],
  ];
  // Whole pixels for distances, as Figma's fields have them
  const fit = (values) => {
    for (const key of ["stepX", "stepY", "radius", "gapX", "gapY"]) if (key in values) values[key] = Math.round(values[key]);
    return values;
  };
  const EASES = {
    even: (t) => t,
    in: (t) => t * t,
    out: (t) => 1 - (1 - t) * (1 - t),
    inOut: (t) => (t < 0.5 ? 2 * t * t : 1 - 2 * (1 - t) * (1 - t)),
  };

  const repeaterOf = (node) => {
    try {
      return node?.type === "GROUP" && node.getSharedPluginData(PLUGIN, SETTINGS) ? node : null;
    } catch {
      return null;
    }
  };

  let repeating = null;

  const openRepeater = (figma) => {
    if (repeating) return;
    const selected = figma.currentPage.selection;
    if (selected.length !== 1) {
      figma.notify(selected.length ? "Select one layer to repeat" : "Select a layer to repeat", { timeout: 2000 });
      return;
    }
    let group = repeaterOf(selected[0]);
    let source;
    let settings = { ...DEFAULTS };
    const unit = () => Math.max(1, Math.round(Math.max(selected[0].width ?? 100, selected[0].height ?? 100)));
    if (group) {
      source = group.children.find((child) => child.getSharedPluginData(PLUGIN, ROLE) === "source") ?? group.children.at(-1);
      try {
        settings = { ...DEFAULTS, ...JSON.parse(group.getSharedPluginData(PLUGIN, SETTINGS)) };
      } catch {}
    } else {
      source = selected[0];
      if (source.type === "SECTION" || !("clone" in source)) {
        figma.notify("Sections can't be repeated", { timeout: 2000 });
        return;
      }
      // Starting as the Trail preset, sized to the layer
      Object.assign(settings, fit(PRESETS[0][1](unit())));
    }
    try {
      figma.commitUndo();
    } catch {}
    const current = {
      figma,
      group,
      source,
      settings,
      before: group ? JSON.stringify(settings) : null,
      // What's kept for an existing Repeater, to put back on Cancel
      kept: group ? group.children.filter((child) => child !== source) : [],
      clones: [],
      scales: [],
      queued: false,
    };
    repeating = current;
    // An existing Repeater's copies are hidden while new ones preview,
    // made again from the original as it is now
    for (const node of current.kept) node.visible = false;
    buildPanel(current);
    rebuild(current, true);
  };

  // Where each copy goes, from the settings: its offset from the original's
  // center, its turn in degrees, as Figma's rotation counts them, its size
  // and its opacity, each a fraction of the original's
  const layout = (settings, width, height) => {
    const s = settings;
    const count = s.mode === "grid" ? Math.max(0, s.columns * s.rows - 1) : s.copies;
    const ease = EASES[s.ease] ?? EASES.even;
    const placed = [];
    let x = 0;
    let y = 0;
    for (let i = 1; i <= count; i++) {
      const t = ease(count === 1 ? 1 : i / count);
      const scale = 1 + (s.endScale / 100 - 1) * t;
      const opacity = 1 + (s.endOpacity / 100 - 1) * t;
      let turn = 0;
      if (s.mode === "trail") {
        // Each step turns by the curve, so the trail bends
        const angle = ((s.curve * (i - 1)) * Math.PI) / 180;
        x += s.stepX * Math.cos(angle) - s.stepY * Math.sin(angle);
        y += s.stepX * Math.sin(angle) + s.stepY * Math.cos(angle);
        turn = -s.turn * i;
      } else if (s.mode === "radial") {
        // Around a ring through the original, which sits at the start
        const full = Math.abs(s.sweep) >= 360;
        const a0 = (s.start * Math.PI) / 180;
        const a = a0 + ((s.sweep * Math.PI) / 180) * (i / (full ? count + 1 : count));
        x = s.radius * (Math.cos(a) - Math.cos(a0));
        y = s.radius * (Math.sin(a) - Math.sin(a0));
        if (s.orient) turn = -((a - a0) * 180) / Math.PI;
      } else {
        const column = i % s.columns;
        const row = Math.floor(i / s.columns);
        x = column * (width + s.gapX);
        y = row * (height + s.gapY);
      }
      placed.push({ x, y, turn, scale: Math.max(0.01, scale), opacity: Math.min(1, Math.max(0, opacity)), t });
    }
    return placed;
  };

  const hexToRgb = (hex) => {
    const value = parseInt(hex.slice(1), 16);
    return { r: ((value >> 16) & 255) / 255, g: ((value >> 8) & 255) / 255, b: (value & 255) / 255 };
  };
  // Solid fills and strokes of `node` and what's in it, a fraction of the
  // way toward `color`
  const blendToward = (node, color, amount) => {
    const mix = (paints) =>
      paints === figma.mixed || !Array.isArray(paints)
        ? paints
        : paints.map((paint) =>
            paint.type === "SOLID"
              ? { ...paint, boundVariables: {}, color: { r: paint.color.r + (color.r - paint.color.r) * amount, g: paint.color.g + (color.g - paint.color.g) * amount, b: paint.color.b + (color.b - paint.color.b) * amount } }
              : paint,
          );
    const figma = window.figma;
    const nodes = "findAll" in node ? [node, ...node.findAll(() => true)] : [node];
    for (const each of nodes) {
      try {
        if ("fills" in each && each.fills !== figma.mixed && each.fills.length) each.fills = mix(each.fills);
        if ("strokes" in each && each.strokes.length) each.strokes = mix(each.strokes);
      } catch {}
    }
  };

  // Lays the copies out, making or removing them as the count changes, or
  // all of them again with `fresh`, as when blending changes
  const rebuild = (current, fresh = false) => {
    const { figma, source, settings } = current;
    if (source.removed) return closeRepeater(current, false);
    const width = source.width;
    const height = source.height;
    const placed = layout(settings, width, height);
    if (fresh) {
      for (const clone of current.clones) if (!clone.removed) clone.remove();
      current.clones = [];
      current.scales = [];
    }
    const parent = source.parent;
    while (current.clones.length > placed.length) {
      current.clones.pop().remove();
      current.scales.pop();
    }
    while (current.clones.length < placed.length) {
      const clone = source.clone();
      clone.setSharedPluginData(PLUGIN, ROLE, "copy");
      current.clones.push(clone);
      current.scales.push(1);
      if (settings.blend) {
        const t = placed[current.clones.length - 1].t;
        blendToward(clone, hexToRgb(settings.blendColor), (settings.blendAmount / 100) * t);
      }
    }
    // The original's place, its turn and center, in its parent
    const [[a, c, e], [b, d, f]] = source.relativeTransform;
    const center = { x: e + (a * width) / 2 + (c * height) / 2, y: f + (b * width) / 2 + (d * height) / 2 };
    // In auto layout, copies sit where they're put rather than in the flow
    const flowing = "layoutMode" in parent && parent.layoutMode !== "NONE";
    placed.forEach((spot, i) => {
      const clone = current.clones[i];
      if (clone.parent !== parent) parent.appendChild(clone);
      if (flowing && "layoutPositioning" in clone && clone.layoutPositioning !== "ABSOLUTE") clone.layoutPositioning = "ABSOLUTE";
      const ratio = spot.scale / current.scales[i];
      if (Math.abs(ratio - 1) > 1e-6 && "rescale" in clone) {
        try {
          clone.rescale(ratio);
          current.scales[i] = spot.scale;
        } catch {}
      }
      const w = width * current.scales[i];
      const h = height * current.scales[i];
      const r = (spot.turn * Math.PI) / 180;
      const [cos, sin] = [Math.cos(r), Math.sin(r)];
      // The original's turn, then the copy's own on top
      const m = [cos * a + sin * b, cos * c + sin * d, -sin * a + cos * b, -sin * c + cos * d];
      const cx = center.x + spot.x;
      const cy = center.y + spot.y;
      clone.relativeTransform = [
        [m[0], m[1], cx - (m[0] * w) / 2 - (m[1] * h) / 2],
        [m[2], m[3], cy - (m[2] * w) / 2 - (m[3] * h) / 2],
      ];
      clone.opacity = source.opacity * spot.opacity;
      clone.visible = true;
    });
    // Stacked from the farthest to the nearest, the nearest next to the
    // original, under or over it
    for (let i = current.clones.length - 1; i >= 0; i--) {
      const index = parent.children.indexOf(source);
      parent.insertChild(settings.below ? index : index + 1, current.clones[i]);
    }
    current.panel?.update();
  };

  // Lays them out again once a frame, however many changes come in it
  const schedule = (current, fresh = false) => {
    current.fresh ||= fresh;
    if (current.queued) return;
    current.queued = true;
    requestAnimationFrame(() => {
      current.queued = false;
      const again = current.fresh;
      current.fresh = false;
      if (repeating === current) rebuild(current, again);
    });
  };

  const apply = (current) => {
    const { figma, source, settings } = current;
    for (const node of current.kept) if (!node.removed) node.remove();
    let result = current.group;
    if (settings.editable) {
      const parent = source.parent;
      const nodes = [source, ...current.clones];
      if (!result || result.removed) {
        result = figma.group(nodes, parent, parent.children.indexOf(source));
        result.name = `Repeater · ${source.name}`;
      } else {
        for (const clone of current.clones) if (clone.parent !== result) result.appendChild(clone);
      }
      source.setSharedPluginData(PLUGIN, ROLE, "source");
      result.setSharedPluginData(PLUGIN, SETTINGS, JSON.stringify(settings));
      figma.currentPage.selection = [result];
    } else {
      // Plain copies, which Repeater forgets
      for (const clone of current.clones) clone.setSharedPluginData(PLUGIN, ROLE, "");
      if (result && !result.removed) {
        result.setSharedPluginData(PLUGIN, SETTINGS, "");
        source.setSharedPluginData(PLUGIN, ROLE, "");
      }
      figma.currentPage.selection = [source, ...current.clones];
    }
    try {
      figma.commitUndo();
    } catch {}
    const count = current.clones.length;
    closeRepeater(current, true);
    figma.notify(`${count} ${count === 1 ? "copy" : "copies"} made`, { timeout: 1500 });
  };

  const closeRepeater = (current, applied) => {
    if (repeating !== current) return;
    repeating = null;
    if (!applied) {
      for (const clone of current.clones) if (!clone.removed) clone.remove();
      for (const node of current.kept) if (!node.removed) node.visible = true;
      try {
        if (current.group && !current.group.removed) current.figma.currentPage.selection = [current.group];
        else if (!current.source.removed) current.figma.currentPage.selection = [current.source];
      } catch {}
    }
    current.panel?.close();
    current.root?.remove();
    removeEventListener("keydown", current.keys, true);
  };

  const buildPanel = (current) => {
    const s = current.settings;
    const root = element("div");
    root.setAttribute(ROOT, "panel");
    root.setAttribute("role", "dialog");
    root.setAttribute("aria-label", "Repeater");
    const change = (key, value, fresh = false) => {
      s[key] = value;
      schedule(current, fresh);
      show();
    };
    const field = (key, glyph, title, options = {}) =>
      numberField({ glyph, title, ...options, get: () => s[key], set: (value) => change(key, value, key === "blendAmount" && s.blend) });

    // Title, with presets and close
    const presets = element("button", { type: "button", className: "icon-button", title: "Presets" });
    presets.innerHTML = PRESETS_ICON;
    presets.setAttribute("aria-haspopup", "menu");
    const close = element("button", { type: "button", className: "icon-button", title: "Cancel" });
    close.innerHTML = CLOSE;
    close.addEventListener("click", () => closeRepeater(current, false));
    const title = element("div", { className: "title" }, element("span", { textContent: "Repeater" }), presets, close);

    // Layout
    const mode = segments(
      "Layout",
      [
        ["trail", "Trail", "Each copy a step on from the last"],
        ["radial", "Radial", "Around a ring"],
        ["grid", "Grid", "In rows and columns"],
      ],
      () => s.mode,
      (value) => change("mode", value),
    );
    const trail = element(
      "div",
      {},
      element("div", { className: "grid" }, field("copies", GLYPHS.copies, "Copies", { min: 1, max: MAX_COPIES }), field("turn", GLYPHS.turn, "Turn each step", { unit: "°" })),
      element("div", { className: "grid" }, field("stepX", "X", "Step right", { unit: "" }), field("stepY", "Y", "Step down", { unit: "" })),
      element("div", { className: "grid" }, field("curve", GLYPHS.curve, "Curve each step", { unit: "°" })),
    );
    for (const grid of trail.children) grid.style.marginTop = "8px";
    const radial = element(
      "div",
      {},
      element("div", { className: "grid" }, field("copies", GLYPHS.copies, "Copies", { min: 1, max: MAX_COPIES }), field("radius", GLYPHS.radius, "Radius", { min: 0 })),
      element("div", { className: "grid" }, field("start", GLYPHS.start, "Start angle", { unit: "°" }), field("sweep", GLYPHS.sweep, "Sweep", { unit: "°", min: -360, max: 360 })),
    );
    for (const grid of radial.children) grid.style.marginTop = "8px";
    const orient = element("input", { type: "checkbox" });
    orient.addEventListener("change", () => change("orient", orient.checked));
    radial.append(element("label", { className: "check" }, orient, "Turn copies to follow the ring"));
    radial.lastChild.style.marginTop = "8px";
    const grid = element(
      "div",
      {},
      element("div", { className: "grid" }, field("columns", GLYPHS.columns, "Columns", { min: 1, max: 40 }), field("rows", GLYPHS.rows, "Rows", { min: 1, max: 40 })),
      element("div", { className: "grid" }, field("gapX", GLYPHS.gapX, "Gap across", {}), field("gapY", GLYPHS.gapY, "Gap down", {})),
    );
    for (const each of grid.children) each.style.marginTop = "8px";
    const layoutSection = element("section", {}, element("div", { className: "section-title", textContent: "Layout" }), mode, trail, radial, grid);

    // From the first copy to the last
    const ease = select(
      "Progression",
      [
        ["even", "Even"],
        ["in", "Ease in"],
        ["out", "Ease out"],
        ["inOut", "Ease in and out"],
      ],
      () => s.ease,
      (value) => change("ease", value, s.blend),
    );
    const blend = element("input", { type: "checkbox" });
    blend.addEventListener("change", () => change("blend", blend.checked, true));
    const picker = element("input", { type: "color" });
    picker.addEventListener("input", () => change("blendColor", picker.value, true));
    const swatch = element("button", { type: "button", className: "swatch", title: "Blend color" });
    swatch.addEventListener("click", () => picker.click());
    const hex = element("input", { type: "text", spellcheck: false });
    hex.setAttribute("aria-label", "Blend color");
    hex.addEventListener("change", () => {
      const value = hex.value.trim().replace(/^#?/, "#");
      if (/^#[0-9a-f]{6}$/i.test(value)) change("blendColor", value.toLowerCase(), true);
      show();
    });
    hex.addEventListener("keydown", (event) => event.key === "Enter" && hex.blur());
    const color = element("label", { className: "field color" }, swatch, hex, picker);
    const amount = field("blendAmount", GLYPHS.amount, "Blend amount", { min: 0, max: 100, unit: "%" });
    const blendRow = element("div", { className: "grid" }, color, amount);
    const toEnd = element(
      "section",
      {},
      element("div", { className: "section-title", textContent: "To the last copy" }),
      element("div", { className: "grid" }, field("endScale", GLYPHS.size, "Size", { min: 1, max: 1000, unit: "%" }), field("endOpacity", GLYPHS.opacity, "Opacity", { min: 0, max: 100, unit: "%" })),
      element("div", { className: "line" }, element("span", { className: "label", textContent: "Progression" }), ease),
      element("label", { className: "check" }, blend, "Blend toward a color"),
      blendRow,
    );

    // The result
    const order = segments(
      "Stack",
      [
        ["below", "Copies below"],
        ["above", "Copies above"],
      ],
      () => (s.below ? "below" : "above"),
      (value) => change("below", value === "below"),
    );
    const editable = element("input", { type: "checkbox" });
    editable.addEventListener("change", () => change("editable", editable.checked));
    const result = element(
      "section",
      {},
      element("div", { className: "section-title", textContent: "Result" }),
      order,
      element("label", { className: "check", title: "Kept in a group Repeater can change later, the copies made again from the original" }, editable, "Keep editable"),
    );
    result.lastChild.style.marginTop = "8px";

    // Footer
    const count = element("span", { className: "count" });
    const cancel = element("button", { type: "button", className: "button", textContent: "Cancel" });
    cancel.addEventListener("click", () => closeRepeater(current, false));
    const go = element("button", { type: "button", className: "button primary", textContent: current.group ? "Update" : "Apply" });
    go.addEventListener("click", () => apply(current));
    const footer = element("div", { className: "footer" }, count, cancel, go);

    const scroll = element("div", { className: "scroll" }, layoutSection, toEnd, result);
    root.append(title, scroll, footer);
    document.body.append(root);

    // Presets menu
    let presetMenu = null;
    const closePresets = () => {
      presetMenu?.remove();
      presetMenu = null;
      presets.setAttribute("aria-expanded", "false");
    };
    presets.addEventListener("click", () => {
      if (presetMenu) return closePresets();
      presetMenu = element("div");
      presetMenu.setAttribute(ROOT, "menu");
      presetMenu.setAttribute("role", "menu");
      presetMenu.style.width = "160px";
      presetMenu.innerHTML = `<div class="heading">Presets</div>`;
      for (const [name, values] of PRESETS) {
        const item = presetMenu.appendChild(element("button", { type: "button", className: "item" }));
        item.style.gridTemplateColumns = "1fr";
        item.style.padding = "4px 8px";
        item.innerHTML = `<span class="name">${name}</span>`;
        item.addEventListener("click", () => {
          closePresets();
          Object.assign(s, { ease: "even" }, fit(values(Math.max(current.source.width, current.source.height))));
          schedule(current, s.blend);
          show();
        });
      }
      document.body.append(presetMenu);
      const box = presets.getBoundingClientRect();
      presetMenu.style.left = `${Math.min(innerWidth - presetMenu.offsetWidth - 8, box.left)}px`;
      presetMenu.style.top = `${box.bottom + 4}px`;
      presets.setAttribute("aria-expanded", "true");
      const outside = (event) => {
        if (presetMenu && !presetMenu.contains(event.target) && !presets.contains(event.target)) {
          closePresets();
          removeEventListener("pointerdown", outside, true);
        }
      };
      addEventListener("pointerdown", outside, true);
    });

    // Placed over the canvas's top right, clear of the right panel, and
    // dragged by its title
    const canvas = document.querySelector("canvas")?.getBoundingClientRect() ?? { right: innerWidth, top: 48 };
    root.style.left = `${Math.max(8, canvas.right - root.offsetWidth - 16)}px`;
    root.style.top = `${Math.max(8, canvas.top + 16)}px`;
    title.addEventListener("pointerdown", (event) => {
      if (event.button !== 0 || event.target.closest("button")) return;
      title.setPointerCapture(event.pointerId);
      title.style.cursor = "grabbing";
      const box = root.getBoundingClientRect();
      const [dx, dy] = [event.clientX - box.left, event.clientY - box.top];
      const move = (event) => {
        root.style.left = `${Math.min(innerWidth - 48, Math.max(8 - box.width + 48, event.clientX - dx))}px`;
        root.style.top = `${Math.min(innerHeight - 40, Math.max(8, event.clientY - dy))}px`;
      };
      title.addEventListener("pointermove", move);
      title.addEventListener(
        "lostpointercapture",
        () => {
          title.style.cursor = "";
          title.removeEventListener("pointermove", move);
        },
        { once: true },
      );
    });

    const fields = [...root.querySelectorAll(".field:not(.color)")];
    const show = () => {
      mode.show();
      trail.hidden = s.mode !== "trail";
      radial.hidden = s.mode !== "radial";
      grid.hidden = s.mode !== "grid";
      orient.checked = s.orient;
      for (const each of fields) each.show?.();
      ease.show();
      blend.checked = s.blend;
      blendRow.hidden = !s.blend;
      swatch.style.background = s.blendColor;
      picker.value = s.blendColor;
      if (document.activeElement !== hex) hex.value = s.blendColor.slice(1).toUpperCase();
      order.show();
      editable.checked = s.editable;
    };
    show();
    current.root = root;
    current.panel = {
      update: () => {
        const n = current.clones.length;
        count.textContent = `${n} ${n === 1 ? "copy" : "copies"}`;
      },
      close: () => {
        ease.close();
        closePresets();
      },
    };
    // Escape cancels, Enter applies, while the panel's open and not typing
    current.keys = (event) => {
      if (repeating !== current) return;
      const typing = event.target.closest?.("input:not([type=checkbox]), textarea, [contenteditable]");
      if (event.key === "Escape" && !typing) {
        event.preventDefault();
        event.stopImmediatePropagation();
        closeRepeater(current, false);
      } else if (event.key === "Enter" && !typing && root.contains(event.target)) {
        event.preventDefault();
        apply(current);
      }
    };
    addEventListener("keydown", current.keys, true);
    go.focus();
  };

  // ---------- Setting ----------

  const tick = () => {
    if (!state.on) {
      button()?.remove();
      closeMenu();
      if (repeating) closeRepeater(repeating, false);
      return;
    }
    if (!style.isConnected && document.head) document.head.append(style);
    place();
  };
  setInterval(tick, 500);

  const applyState = (next) => {
    state = next;
    tick();
  };
  // From Ligma, when the setting changes
  window.__ligmaEffects = (next) => {
    try {
      localStorage.setItem(KEY, JSON.stringify(next));
    } catch {}
    applyState(next);
  };
  // From another tab, or while this one was frozen and missed it
  const sync = () => {
    try {
      const stored = JSON.parse(localStorage.getItem(KEY));
      if (stored?.rev > state.rev) applyState(stored);
    } catch {}
  };
  addEventListener("storage", (event) => event.key === KEY && sync());
  document.addEventListener("visibilitychange", () => document.visibilityState === "visible" && sync());
})()"##;

/// The setting,
static ON: AtomicBool = AtomicBool::new(true);
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
    ON.store(settings.effects_library, Ordering::Relaxed);
    REV.store(now(), Ordering::Relaxed);
}

/// `PAGE_JS`, for a new Figma tab
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_EFFECTS__", &state())
}

/// Sends a changed setting to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    if ON.swap(settings.effects_library, Ordering::Relaxed) == settings.effects_library {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaEffects?.({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
