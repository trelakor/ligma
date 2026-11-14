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
    /* An effect's panel: a preview up top, then labeled sliders, in
       Figma's colors and type */
    [${ROOT}="panel"] {
      position: fixed;
      z-index: 8;
      width: 300px;
      max-height: calc(100vh - 96px);
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
      gap: 8px;
      height: 40px;
      flex: none;
      padding: 0 8px 0 16px;
      font-weight: 600;
      cursor: grab;
      user-select: none;
    }
    [${ROOT}="panel"] .title span { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
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
    [${ROOT}="panel"] .icon-button:hover { background: var(--color-bg-hover); }
    [${ROOT}="panel"] .preview {
      display: block;
      flex: none;
      width: 100%;
      height: 200px;
      background: var(--color-bg-secondary);
      border-block: 1px solid var(--color-border);
    }
    [${ROOT}="panel"] .scroll { overflow-y: auto; min-height: 0; }
    [${ROOT}="panel"] section { padding: 8px 16px; border-bottom: 1px solid var(--color-border); }
    [${ROOT}="panel"] .caption { height: 24px; display: flex; align-items: center; color: var(--color-text-secondary); }
    [${ROOT}="panel"] .chips { display: flex; flex-wrap: wrap; gap: 6px; padding: 2px 0 6px; }
    [${ROOT}="panel"] .chip {
      height: 24px;
      padding: 0 10px;
      border: 1px solid var(--color-border);
      border-radius: 12px;
      background: none;
    }
    [${ROOT}="panel"] .chip:hover { background: var(--color-bg-hover); border-color: var(--color-text-tertiary, var(--color-border)); }
    [${ROOT}="panel"] .chip:active { background: var(--color-bg-pressed, var(--color-bg-hover)); }
    [${ROOT}="panel"] .rows { display: flex; flex-direction: column; }
    [${ROOT}="panel"] .row {
      display: grid;
      grid-template-columns: 96px 1fr 52px;
      align-items: center;
      column-gap: 8px;
      min-height: 28px;
    }
    [${ROOT}="panel"] .row > .label { color: var(--color-text-secondary); }
    [${ROOT}="panel"] .row > .select, [${ROOT}="panel"] .row > .segments { grid-column: span 2; width: auto; }
    [${ROOT}="panel"] .row.check { grid-template-columns: 1fr auto auto; cursor: default; user-select: none; }
    [${ROOT}="panel"] .row.check input[type="checkbox"] { width: 12px; height: 12px; margin: 0 2px; accent-color: var(--color-bg-brand, #0d99ff); }
    [${ROOT}="panel"] input[type="range"] {
      -webkit-appearance: none;
      appearance: none;
      width: 100%;
      height: 16px;
      margin: 0;
      background:
        linear-gradient(var(--color-bg-brand, #0d99ff), var(--color-bg-brand, #0d99ff)) 0 50% / var(--fill, 0%) 2px no-repeat,
        linear-gradient(var(--color-border), var(--color-border)) 0 50% / 100% 2px no-repeat;
      cursor: default;
    }
    [${ROOT}="panel"] input[type="range"]::-webkit-slider-thumb {
      -webkit-appearance: none;
      width: 12px;
      height: 12px;
      border-radius: 50%;
      background: #fff;
      box-shadow: 0 0 0 1px rgb(0 0 0 / 0.2), 0 1px 3px rgb(0 0 0 / 0.3);
    }
    [${ROOT}="panel"] input[type="range"]:focus-visible { outline: 0; }
    [${ROOT}="panel"] input[type="range"]:focus-visible::-webkit-slider-thumb { box-shadow: 0 0 0 2px var(--color-border-selected, #0d99ff); }
    [${ROOT}="panel"] .value {
      width: 100%;
      height: 22px;
      padding: 0 4px;
      border: 1px solid transparent;
      border-radius: 4px;
      outline: 0;
      background: none;
      color: inherit;
      font: inherit;
      font-variant-numeric: tabular-nums;
      text-align: right;
    }
    [${ROOT}="panel"] .value:hover { background: var(--color-bg-secondary); }
    [${ROOT}="panel"] .value:focus { border-color: var(--color-border-selected, #0d99ff); background: var(--color-bg-secondary); text-align: left; }
    [${ROOT}="panel"] .select {
      display: flex;
      align-items: center;
      justify-content: space-between;
      height: 24px;
      padding: 0 2px 0 8px;
      border: 1px solid transparent;
      border-radius: 5px;
      background: var(--color-bg-secondary);
      text-align: left;
    }
    [${ROOT}="panel"] .select:hover, [${ROOT}="panel"] .select[aria-expanded="true"] { border-color: var(--color-border); }
    [${ROOT}="panel"] .segments { display: flex; padding: 2px; gap: 2px; border-radius: 5px; background: var(--color-bg-secondary); }
    [${ROOT}="panel"] .segments button {
      flex: 1;
      height: 20px;
      padding: 0 6px;
      border: 0;
      border-radius: 4px;
      background: none;
      color: var(--color-text-secondary);
      white-space: nowrap;
    }
    [${ROOT}="panel"] .segments button:hover { color: var(--color-text); }
    [${ROOT}="panel"] .segments button[aria-checked="true"] { background: var(--color-bg); color: var(--color-text); box-shadow: 0 0 0 1px var(--color-border); }
    [${ROOT}="panel"] .swatch {
      width: 16px;
      height: 16px;
      padding: 0;
      border: 0;
      border-radius: 4px;
      box-shadow: inset 0 0 0 1px var(--color-bordertranslucent, #0000001a);
    }
    [${ROOT}="panel"] .hidden-picker { position: absolute; width: 0; height: 0; opacity: 0; pointer-events: none; }
    [${ROOT}="panel"] .under { display: flex; justify-content: flex-end; padding: 8px 16px 12px; }
    [${ROOT}="panel"] .link {
      padding: 0;
      border: 0;
      background: none;
      color: var(--color-text-secondary);
    }
    [${ROOT}="panel"] .link:hover { color: var(--color-text); text-decoration: underline; text-underline-offset: 2px; }
    [${ROOT}="panel"] .footer {
      flex: none;
      display: flex;
      flex-direction: column;
      gap: 8px;
      padding: 12px 16px 16px;
      border-top: 1px solid var(--color-border);
    }
    [${ROOT}="panel"] .footer .source { color: var(--color-text-secondary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    [${ROOT}="panel"] .footer .actions { display: flex; gap: 8px; }
    [${ROOT}="panel"] .button {
      height: 32px;
      padding: 0 12px;
      border: 1px solid var(--color-border);
      border-radius: 6px;
      background: none;
      font-weight: 500;
    }
    [${ROOT}="panel"] .button:hover { background: var(--color-bg-hover); }
    [${ROOT}="panel"] .button.primary {
      flex: 1;
      border-color: transparent;
      background: var(--color-bg-brand, #0d99ff);
      color: var(--color-text-onbrand, #fff);
    }
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
        // Each step turns by the curve, so the trail bends, and is as much
        // smaller or larger as the copies are, so shrinking copies spiral in
        const angle = ((s.curve * (i - 1)) * Math.PI) / 180;
        const size = Math.max(0.01, 1 + (s.endScale / 100 - 1) * ease(count === 1 ? 1 : (i - 1) / count));
        x += (s.stepX * Math.cos(angle) - s.stepY * Math.sin(angle)) * size;
        y += (s.stepX * Math.sin(angle) + s.stepY * Math.cos(angle)) * size;
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

  // The slider rows each layout shows: key, label, unit, and its range,
  // `u` the layer's larger side, so distances suit it. Typing goes past
  // the slider's ends
  const ROWS = {
    trail: [
      ["copies", "Copies", "", () => [1, 100, 1]],
      ["stepX", "Step right", "px", (u) => [-Math.round(u * 2), Math.round(u * 2), 1]],
      ["stepY", "Step down", "px", (u) => [-Math.round(u * 2), Math.round(u * 2), 1]],
      ["turn", "Turn each step", "°", () => [-180, 180, 1]],
      ["curve", "Curve each step", "°", () => [-90, 90, 1]],
    ],
    radial: [
      ["copies", "Copies", "", () => [1, 100, 1]],
      ["radius", "Radius", "px", (u) => [0, Math.round(u * 5), 1]],
      ["start", "Start angle", "°", () => [-180, 180, 1]],
      ["sweep", "Sweep", "°", () => [-360, 360, 1]],
    ],
    grid: [
      ["columns", "Columns", "", () => [1, 20, 1]],
      ["rows", "Rows", "", () => [1, 20, 1]],
      ["gapX", "Gap across", "px", (u) => [0, Math.round(u), 1]],
      ["gapY", "Gap down", "px", (u) => [0, Math.round(u), 1]],
    ],
    end: [
      ["endScale", "End size", "%", () => [1, 300, 1]],
      ["endOpacity", "End opacity", "%", () => [0, 100, 1]],
    ],
    blend: [["blendAmount", "Blend amount", "%", () => [0, 100, 1]]],
  };
  const LIMITS = { copies: [1, MAX_COPIES], columns: [1, 40], rows: [1, 40], radius: [0, Infinity], gapX: [-Infinity, Infinity], gapY: [-Infinity, Infinity], endScale: [1, 1000], endOpacity: [0, 100], blendAmount: [0, 100], sweep: [-360, 360] };

  // The layer as a picture, for the panel's preview, at most `PREVIEW`
  // pixels across
  const PREVIEW = 160;
  const snapshot = async (source) => {
    const scale = Math.min(2, PREVIEW / Math.max(1, source.width, source.height));
    const bytes = await source.exportAsync({ format: "PNG", constraint: { type: "SCALE", value: scale } });
    const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
    return { bitmap, scale };
  };

  // Draws the original and its copies into the preview, fitted to it
  const drawPreview = (current) => {
    const { canvas, image } = current.preview ?? {};
    if (!canvas || !image) return;
    const s = current.settings;
    const width = current.source.width;
    const height = current.source.height;
    const placed = [{ x: 0, y: 0, turn: 0, scale: 1, opacity: 1, t: 0, original: true }, ...layout(s, width, height)];
    // The bounds of them all, turned
    let [left, top, right, bottom] = [Infinity, Infinity, -Infinity, -Infinity];
    for (const spot of placed) {
      const r = (-spot.turn * Math.PI) / 180;
      const [hw, hh] = [(width * spot.scale) / 2, (height * spot.scale) / 2];
      const ex = Math.abs(hw * Math.cos(r)) + Math.abs(hh * Math.sin(r));
      const ey = Math.abs(hw * Math.sin(r)) + Math.abs(hh * Math.cos(r));
      left = Math.min(left, spot.x - ex);
      right = Math.max(right, spot.x + ex);
      top = Math.min(top, spot.y - ey);
      bottom = Math.max(bottom, spot.y + ey);
    }
    const ratio = devicePixelRatio || 1;
    const box = canvas.getBoundingClientRect();
    canvas.width = Math.round(box.width * ratio);
    canvas.height = Math.round(box.height * ratio);
    const context = canvas.getContext("2d");
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, box.width, box.height);
    const pad = 16;
    const fit = Math.min((box.width - pad * 2) / Math.max(1, right - left), (box.height - pad * 2) / Math.max(1, bottom - top));
    const ox = box.width / 2 - ((left + right) / 2) * fit;
    const oy = box.height / 2 - ((top + bottom) / 2) * fit;
    // The farthest drawn first when copies go under the original
    const order = s.below ? [...placed].reverse() : placed;
    const tint = hexToRgb(s.blendColor);
    for (const spot of order) {
      context.save();
      context.globalAlpha = spot.opacity;
      context.translate(ox + spot.x * fit, oy + spot.y * fit);
      context.rotate((-spot.turn * Math.PI) / 180);
      const [w, h] = [width * spot.scale * fit, height * spot.scale * fit];
      const amount = s.blend && !spot.original ? (s.blendAmount / 100) * spot.t : 0;
      if (amount > 0) {
        // Tinted on its own, then drawn
        const off = current.preview.scratch ?? (current.preview.scratch = document.createElement("canvas"));
        off.width = image.width;
        off.height = image.height;
        const c = off.getContext("2d");
        c.clearRect(0, 0, off.width, off.height);
        c.drawImage(image, 0, 0);
        c.globalCompositeOperation = "source-atop";
        c.fillStyle = `rgba(${tint.r * 255}, ${tint.g * 255}, ${tint.b * 255}, ${amount})`;
        c.fillRect(0, 0, off.width, off.height);
        context.drawImage(off, -w / 2, -h / 2, w, h);
      } else {
        context.drawImage(image, -w / 2, -h / 2, w, h);
      }
      context.restore();
    }
  };

  const buildPanel = (current) => {
    const s = current.settings;
    const unit = Math.max(1, Math.max(current.source.width, current.source.height));
    const root = element("div");
    root.setAttribute(ROOT, "panel");
    root.setAttribute("role", "dialog");
    root.setAttribute("aria-label", "Repeater");
    const rows = [];
    const change = (key, value, fresh = false) => {
      s[key] = value;
      schedule(current, fresh);
      show();
    };

    // A labeled slider, with its value beside it, which can be typed in
    const slider = ([key, label, suffix, range]) => {
      const [min, max, step] = range(unit);
      const [floor, ceiling] = LIMITS[key] ?? [-Infinity, Infinity];
      const input = element("input", { type: "range", min, max, step });
      input.setAttribute("aria-label", label);
      const value = element("input", { type: "text", className: "value", inputMode: "decimal", spellcheck: false });
      value.setAttribute("aria-label", `${label} value`);
      // Blended copies are made again when how far each blends changes
      const fresh = () => s.blend && ["blendAmount", "copies", "columns", "rows"].includes(key);
      input.addEventListener("input", () => change(key, Number(input.value), fresh()));
      const commit = () => {
        const number = parseFloat(value.value);
        if (Number.isFinite(number)) change(key, Math.min(ceiling, Math.max(floor, Math.round(number))), fresh());
        else show();
      };
      value.addEventListener("change", commit);
      value.addEventListener("keydown", (event) => {
        if (event.key === "Enter") value.blur();
        else if (event.key === "ArrowUp" || event.key === "ArrowDown") {
          event.preventDefault();
          const next = s[key] + (event.key === "ArrowUp" ? 1 : -1) * step * (event.shiftKey ? 10 : 1);
          change(key, Math.min(ceiling, Math.max(floor, next)), fresh());
          value.select();
        }
      });
      value.addEventListener("blur", () => show());
      const row = element("label", { className: "row" }, element("span", { className: "label", textContent: label }), input, value);
      row.show = () => {
        input.value = String(s[key]);
        // The filled part of the track, up to the thumb
        const t = (Math.min(max, Math.max(min, s[key])) - min) / Math.max(1e-9, max - min);
        input.style.setProperty("--fill", `${t * 100}%`);
        if (document.activeElement !== value) value.value = `${Math.round(s[key] * 100) / 100}${suffix}`;
      };
      rows.push(row);
      return row;
    };

    // Title
    const close = element("button", { type: "button", className: "icon-button", title: "Cancel" });
    close.innerHTML = CLOSE;
    close.addEventListener("click", () => closeRepeater(current, false));
    const title = element("div", { className: "title" }, element("span", { textContent: current.group ? `Repeater · ${current.source.name}` : "Repeater" }), close);

    // Preview
    const canvas = element("canvas", { className: "preview" });
    canvas.setAttribute("aria-hidden", "true");
    current.preview = { canvas, image: null };
    snapshot(current.source)
      .then(({ bitmap }) => {
        if (repeating !== current) return bitmap.close();
        current.preview.image = bitmap;
        drawPreview(current);
      })
      .catch(() => {});

    // Presets, as chips
    const chips = element("div", { className: "chips" });
    for (const [name, values] of PRESETS) {
      const chip = chips.appendChild(element("button", { type: "button", className: "chip", textContent: name }));
      chip.addEventListener("click", () => {
        Object.assign(s, { ease: "even" }, fit(values(unit)));
        schedule(current, s.blend);
        show();
      });
    }
    const presets = element("section", {}, element("div", { className: "caption", textContent: "Presets" }), chips);

    // How it repeats
    const mode = select(
      "Repeat as",
      [
        ["trail", "Trail, step by step"],
        ["radial", "Radial, around a ring"],
        ["grid", "Grid, rows and columns"],
      ],
      () => s.mode,
      (value) => change("mode", value),
    );
    mode.classList.add("wide");
    const groups = {};
    for (const name of ["trail", "radial", "grid"]) groups[name] = element("div", { className: "rows" }, ...ROWS[name].map(slider));
    const orient = element("input", { type: "checkbox" });
    orient.addEventListener("change", () => change("orient", orient.checked));
    groups.radial.append(element("label", { className: "row check" }, element("span", { className: "label", textContent: "Turn copies with the ring" }), orient));
    const repeat = element("section", {}, element("label", { className: "row" }, element("span", { className: "label", textContent: "Repeat as" }), mode), groups.trail, groups.radial, groups.grid);

    // Toward the last copy
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
    ease.classList.add("wide");
    const blend = element("input", { type: "checkbox" });
    blend.addEventListener("change", () => change("blend", blend.checked, true));
    const picker = element("input", { type: "color", className: "hidden-picker" });
    picker.addEventListener("input", () => change("blendColor", picker.value, true));
    const swatch = element("button", { type: "button", className: "swatch", title: "Blend color" });
    swatch.addEventListener("click", (event) => {
      event.preventDefault();
      picker.click();
    });
    const blendRow = element("label", { className: "row check" }, element("span", { className: "label", textContent: "Blend toward a color" }), swatch, picker, blend);
    const blendAmount = element("div", { className: "rows" }, ...ROWS.blend.map(slider));
    const toward = element(
      "section",
      {},
      element("div", { className: "rows" }, ...ROWS.end.map(slider)),
      element("label", { className: "row" }, element("span", { className: "label", textContent: "Progression" }), ease),
      blendRow,
      blendAmount,
    );

    // The result
    const order = segments(
      "Copies go",
      [
        ["below", "Below"],
        ["above", "Above"],
      ],
      () => (s.below ? "below" : "above"),
      (value) => change("below", value === "below"),
    );
    const editable = element("input", { type: "checkbox" });
    editable.addEventListener("change", () => change("editable", editable.checked));
    const result = element(
      "section",
      {},
      element("div", { className: "row" }, element("span", { className: "label", textContent: "Copies go" }), order),
      element(
        "label",
        { className: "row check", title: "Kept in a group Repeater can change later, the copies made again from the original as it is then" },
        element("span", { className: "label", textContent: "Keep editable" }),
        editable,
      ),
    );

    const reset = element("button", { type: "button", className: "link", textContent: "Reset" });
    reset.addEventListener("click", () => {
      Object.assign(s, { ...DEFAULTS, below: s.below, editable: s.editable }, fit(PRESETS[0][1](unit)));
      schedule(current, true);
      show();
    });
    const scroll = element("div", { className: "scroll" }, presets, repeat, toward, result, element("div", { className: "under" }, reset));

    // Footer
    const name = element("div", { className: "source", textContent: current.source.name });
    const cancel = element("button", { type: "button", className: "button", textContent: "Cancel" });
    cancel.addEventListener("click", () => closeRepeater(current, false));
    const go = element("button", { type: "button", className: "button primary" });
    go.addEventListener("click", () => apply(current));
    const footer = element("div", { className: "footer" }, name, element("div", { className: "actions" }, cancel, go));

    root.append(title, canvas, scroll, footer);
    document.body.append(root);

    // Placed over the canvas's top right, clear of the right panel, and
    // dragged by its title
    const area = document.querySelector("canvas")?.getBoundingClientRect() ?? { right: innerWidth, top: 48 };
    root.style.left = `${Math.max(8, area.right - root.offsetWidth - 16)}px`;
    root.style.top = `${Math.max(8, area.top + 16)}px`;
    title.addEventListener("pointerdown", (event) => {
      if (event.button !== 0 || event.target.closest("button")) return;
      title.setPointerCapture(event.pointerId);
      title.style.cursor = "grabbing";
      const box = root.getBoundingClientRect();
      const [dx, dy] = [event.clientX - box.left, event.clientY - box.top];
      const move = (event) => {
        root.style.left = `${Math.min(innerWidth - 48, Math.max(48 - box.width, event.clientX - dx))}px`;
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

    const show = () => {
      mode.show();
      for (const [key, group] of Object.entries(groups)) group.hidden = s.mode !== key;
      for (const row of rows) row.show();
      orient.checked = s.orient;
      ease.show();
      blend.checked = s.blend;
      blendAmount.hidden = !s.blend;
      swatch.style.background = s.blendColor;
      picker.value = s.blendColor;
      order.show();
      editable.checked = s.editable;
      for (const chip of chips.children) chip.setAttribute("aria-pressed", "false");
      drawPreview(current);
    };
    show();
    current.root = root;
    current.panel = {
      update: () => {
        const n = current.clones.length;
        go.textContent = current.group ? `Update ${n} ${n === 1 ? "copy" : "copies"}` : `Place ${n} ${n === 1 ? "copy" : "copies"}`;
        drawPreview(current);
      },
      close: () => {
        mode.close();
        ease.close();
        current.preview?.image?.close?.();
      },
    };
    current.panel.update();
    // Escape cancels, Enter applies, while the panel's open and not typing
    current.keys = (event) => {
      if (repeating !== current) return;
      const typing = event.target.closest?.("input[type=text], textarea, [contenteditable]");
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
