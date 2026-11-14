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
  // A square breaking into flakes toward its top right
  const DISINTEGRATE_ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><path ${STROKE} d="M12.5 6.5h-6a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h9a2 2 0 0 0 2-2v-5"/><path ${STROKE} d="m15 9 1.5-1M17.5 5.5l1-1M19.5 9.5h1M14.5 4.5l.5-1M17 12l1 .5"/></svg>`;
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
        linear-gradient(var(--color-bg-brand, #0d99ff), var(--color-bg-brand, #0d99ff)) var(--from, 0%) 50% / var(--fill, 0%) 2px no-repeat,
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
    [${ROOT}="panel"] .value:not(:focus) { cursor: ew-resize; }
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
    {
      id: "disintegrate",
      name: "Disintegrate",
      about: "Snap it into dust, or shatter it like glass",
      icon: DISINTEGRATE_ICON,
      open: (figma) => openDisintegrate(figma),
    },
  ];

  // The wheel over a panel or menu is the panel's: Figma's listeners,
  // which pan and zoom the canvas, don't hear it
  addEventListener(
    "wheel",
    (event) => {
      if (event.target instanceof Element && event.target.closest(`[${ROOT}]`)) event.stopImmediatePropagation();
    },
    { capture: true, passive: true },
  );

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
    if (selected.length === 1 && disintegrateOf(selected[0])) {
      openDisintegrate(figma);
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

  // A labeled slider row for `key` of the settings `s`, with its value
  // beside it, which can be typed in, or dragged left and right to change
  // it, as Figma's fields are. `spec` is [key, label, unit, range], range
  // giving its ends from `unit`, the layer's larger side; `limits` bound
  // typing and dragging; `scrub` is how much a pixel's drag changes it;
  // `fresh` says whether a change has to make things again
  const makeSlider = ({ s, unit, rows, change, limits = {}, scrub = {}, fresh = () => false, digits = 0 }) => ([key, label, suffix, range]) => {
    const shape = range(unit);
    const [floor, ceiling] = limits[key] ?? [-Infinity, Infinity];
    const round = (number) => (digits ? Math.round(number * 10 ** digits) / 10 ** digits : Math.round(number));
    const clamp = (number) => Math.min(ceiling, Math.max(floor, round(number)));
    const input = element("input", { type: "range", min: 0, max: 1000, step: 1 });
    input.setAttribute("aria-label", label);
    const value = element("input", { type: "text", className: "value", inputMode: "decimal", spellcheck: false });
    value.setAttribute("aria-label", `${label} value`);
    input.addEventListener("input", () => change(key, clamp(toValue(shape, Number(input.value) / 1000)), fresh(key)));
    value.addEventListener("change", () => {
      const number = parseFloat(value.value);
      if (Number.isFinite(number)) change(key, clamp(number), fresh(key));
      else row.show();
    });
    value.addEventListener("keydown", (event) => {
      if (event.key === "Enter") value.blur();
      else if (event.key === "ArrowUp" || event.key === "ArrowDown") {
        event.preventDefault();
        change(key, clamp(s[key] + (event.key === "ArrowUp" ? 1 : -1) * (event.shiftKey ? 10 : 1)), fresh(key));
        value.select();
      }
    });
    value.addEventListener("blur", () => row.show());
    // Dragging the value scrubs it, with the pointer locked so it goes on
    // past the screen's edge; a click without a drag types in it
    value.addEventListener("pointerdown", (event) => {
      if (event.button !== 0 || document.activeElement === value) return;
      event.preventDefault();
      const start = s[key];
      let moved = 0;
      let scrubbing = false;
      const rate = scrub[key] ?? 1;
      const move = (event) => {
        moved += event.movementX;
        if (!scrubbing && Math.abs(moved) < 3) return;
        if (!scrubbing) {
          scrubbing = true;
          value.requestPointerLock?.()?.catch?.(() => {});
        }
        change(key, clamp(start + moved * rate * (event.shiftKey ? 10 : 1)), fresh(key));
      };
      const up = () => {
        removeEventListener("pointermove", move, true);
        removeEventListener("pointerup", up, true);
        if (document.pointerLockElement === value) document.exitPointerLock();
        if (!scrubbing) {
          value.focus();
          value.select();
        }
      };
      addEventListener("pointermove", move, true);
      addEventListener("pointerup", up, true);
    });
    const row = element("label", { className: "row" }, element("span", { className: "label", textContent: label }), input, value);
    // A click on the label doesn't put the cursor in the value
    row.addEventListener("click", (event) => event.target === row.firstChild && event.preventDefault());
    row.show = () => {
      const at = toPlace(shape, s[key]);
      input.value = String(Math.round(at * 1000));
      // Filled from the neutral value: the middle where it goes both
      // ways, or 100% is, and otherwise the start
      const from = shape.min < 0 || shape.middle !== undefined ? 0.5 : 0;
      const [left, width] = [Math.min(from, at), Math.abs(at - from)];
      // A background's position counts from the space it leaves, so it's
      // given as a fraction of that
      input.style.setProperty("--from", width >= 1 ? "0%" : `${(left / (1 - width)) * 100}%`);
      input.style.setProperty("--fill", `${width * 100}%`);
      if (document.activeElement !== value) value.value = `${round(s[key])}${suffix}`;
    };
    rows.push(row);
    return row;
  };

  // An effect's panel: its title, which drags it, with a close button, the
  // rest given, placed over the canvas's top right, clear of the right
  // panel
  const makePanel = (name, onClose, ...parts) => {
    const root = element("div");
    root.setAttribute(ROOT, "panel");
    root.setAttribute("role", "dialog");
    root.setAttribute("aria-label", name);
    const close = element("button", { type: "button", className: "icon-button", title: "Cancel" });
    close.innerHTML = CLOSE;
    close.addEventListener("click", onClose);
    const title = element("div", { className: "title" }, element("span", { textContent: name }), close);
    root.append(title, ...parts);
    document.body.append(root);
    // The wheel scrolls the panel, rather than reaching Figma, which
    // would pan or zoom the canvas
    root.addEventListener(
      "wheel",
      (event) => {
        event.preventDefault();
        event.stopPropagation();
        const scroll = event.target.closest?.(".scroll") ?? root.querySelector(".scroll");
        if (scroll) scroll.scrollTop += event.deltaMode === 1 ? event.deltaY * 16 : event.deltaY;
      },
      { passive: false },
    );
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
    return root;
  };

  // Preset chips, each setting values from `presets` [name, (unit) => values]
  const makeChips = (presets, pick) => {
    const chips = element("div", { className: "chips" });
    for (const [name, values] of presets) {
      const chip = chips.appendChild(element("button", { type: "button", className: "chip", textContent: name }));
      chip.addEventListener("click", () => pick(values));
    }
    return element("section", {}, element("div", { className: "caption", textContent: "Presets" }), chips);
  };

  // A row with a label and a checkbox, and anything else given
  const checkRow = (label, input, title, ...extra) => element("label", { className: "row check", title: title ?? "" }, element("span", { className: "label", textContent: label }), ...extra, input);

  // Escape cancels and Enter applies while `active()`, unless typing
  const panelKeys = (root, active, cancel, go) => (event) => {
    if (!active()) return;
    const typing = event.target.closest?.("input[type=text], textarea, [contenteditable]");
    if (event.key === "Escape" && !typing) {
      event.preventDefault();
      event.stopImmediatePropagation();
      cancel();
    } else if (event.key === "Enter" && !typing && root.contains(event.target)) {
      event.preventDefault();
      go();
    }
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
    for (const key of ["stepX", "stepY", "radius", "gapX", "gapY", "distance"]) if (key in values) values[key] = Math.round(values[key]);
    if ("piece" in values) values.piece = Math.round(values.piece * 10) / 10;
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
    if (repeating || disintegrating) return;
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
    // A new one's preview is grouped from the start, so the layers panel
    // shows one group rather than every copy. In auto layout it isn't, as
    // a group there would move the layer into the flow
    const parent = source.parent;
    if (!group && parent && !("layoutMode" in parent && parent.layoutMode !== "NONE")) {
      current.home = { parent, index: parent.children.indexOf(source) };
      try {
        current.wrap = figma.group([source], parent, current.home.index);
        current.wrap.name = `Repeater · ${source.name}`;
        figma.currentPage.selection = [current.wrap];
      } catch {
        current.wrap = null;
      }
    }
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
    let result = current.group ?? current.wrap;
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
      // Plain copies, which Repeater forgets, out of the preview's group
      for (const clone of current.clones) clone.setSharedPluginData(PLUGIN, ROLE, "");
      if (result && !result.removed) {
        result.setSharedPluginData(PLUGIN, SETTINGS, "");
        source.setSharedPluginData(PLUGIN, ROLE, "");
      }
      if (current.wrap && !current.wrap.removed) figma.ungroup(current.wrap);
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
      // The original back where it was, and the preview's group with it
      // gone, as Figma removes empty groups
      const { home, wrap, source } = current;
      if (wrap && home && !source.removed && !home.parent.removed) {
        try {
          home.parent.insertChild(Math.min(home.index, home.parent.children.length), source);
          if (!wrap.removed) wrap.remove();
        } catch {}
      }
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
  // `u` the layer's larger side, so distances suit it. Distances and
  // turns ease out from zero, so small values get most of the slider,
  // and End size has 100% in the middle. Typing and dragging the value go
  // past the slider's ends
  const near = (u, least, times) => Math.max(least, Math.round(u * times));
  const ROWS = {
    trail: [
      ["copies", "Copies", "", () => ({ min: 1, max: 60 })],
      ["stepX", "Step right", "px", (u) => ({ min: -near(u, 50, 1.5), max: near(u, 50, 1.5), shape: "curve" })],
      ["stepY", "Step down", "px", (u) => ({ min: -near(u, 50, 1.5), max: near(u, 50, 1.5), shape: "curve" })],
      ["turn", "Turn each step", "°", () => ({ min: -180, max: 180, shape: "curve" })],
      ["curve", "Curve each step", "°", () => ({ min: -90, max: 90, shape: "curve" })],
    ],
    radial: [
      ["copies", "Copies", "", () => ({ min: 1, max: 60 })],
      ["radius", "Radius", "px", (u) => ({ min: 0, max: near(u, 100, 4), shape: "curve" })],
      ["start", "Start angle", "°", () => ({ min: -180, max: 180 })],
      ["sweep", "Sweep", "°", () => ({ min: -360, max: 360 })],
    ],
    grid: [
      ["columns", "Columns", "", () => ({ min: 1, max: 20 })],
      ["rows", "Rows", "", () => ({ min: 1, max: 20 })],
      ["gapX", "Gap across", "px", (u) => ({ min: 0, max: near(u, 40, 1), shape: "curve" })],
      ["gapY", "Gap down", "px", (u) => ({ min: 0, max: near(u, 40, 1), shape: "curve" })],
    ],
    end: [
      ["endScale", "End size", "%", () => ({ min: 1, max: 300, middle: 100 })],
      ["endOpacity", "End opacity", "%", () => ({ min: 0, max: 100 })],
    ],
    blend: [["blendAmount", "Blend amount", "%", () => ({ min: 0, max: 100 })]],
  };
  // How much dragging a value changes it, a pixel at a time
  const SCRUB = { copies: 0.2, columns: 0.1, rows: 0.1 };
  // From a slider's place, 0 to 1, to its value and back
  const CURVE = 2.5;
  const toValue = ({ min, max, shape, middle }, at) => {
    if (middle !== undefined) return at < 0.5 ? min + (middle - min) * (at / 0.5) : middle + (max - middle) * ((at - 0.5) / 0.5);
    if (shape !== "curve") return min + (max - min) * at;
    if (min < 0) {
      const q = 2 * at - 1;
      return Math.sign(q) * Math.abs(q) ** CURVE * max;
    }
    return min + (max - min) * at ** CURVE;
  };
  const toPlace = ({ min, max, shape, middle }, value) => {
    const v = Math.min(max, Math.max(min, value));
    if (middle !== undefined) return v < middle ? ((v - min) / (middle - min)) * 0.5 : 0.5 + ((v - middle) / (max - middle)) * 0.5;
    if (shape !== "curve") return (v - min) / (max - min);
    if (min < 0) return (Math.sign(v) * Math.abs(v / max) ** (1 / CURVE) + 1) / 2;
    return ((v - min) / (max - min)) ** (1 / CURVE);
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
    const rows = [];
    const change = (key, value, fresh = false) => {
      s[key] = value;
      schedule(current, fresh);
      show();
    };
    // Blended copies are made again when how far each blends changes
    const slider = makeSlider({ s, unit, rows, change, limits: LIMITS, scrub: SCRUB, fresh: (key) => s.blend && ["blendAmount", "copies", "columns", "rows"].includes(key) });

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
    const presets = makeChips(PRESETS, (values) => {
      Object.assign(s, { ease: "even" }, fit(values(unit)));
      schedule(current, s.blend);
      show();
    });

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
    groups.radial.append(checkRow("Turn copies with the ring", orient));
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
      checkRow("Keep editable", editable, "Kept in a group Repeater can change later, the copies made again from the original as it is then"),
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

    const root = makePanel(current.group ? `Repeater · ${current.source.name}` : "Repeater", () => closeRepeater(current, false), canvas, scroll, footer);

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
    current.keys = panelKeys(root, () => repeating === current, () => closeRepeater(current, false), () => apply(current));
    addEventListener("keydown", current.keys, true);
    go.focus();
  };

  // ---------- Disintegrate ----------

  // Breaks the layer into pieces, as After Effects' Shatter does, and
  // moves them: Shatter as AE does, the pieces within each force's radius
  // thrown out from it by its strength, turning, slowed by viscosity and
  // pulled by gravity, the rest left whole; Snap as dust, a ragged front
  // spreading out from each force as far as its radius, the pieces it
  // passes blown off with the wind, shrinking, fading and lightening.
  // Pieces are polygons, cut from the layer's picture with smooth, straight
  // edges, drawn here in the page and put back as an image, the original
  // kept under it, hidden
  const DISINTEGRATE = "disintegrate";
  const MAX_SIDE = 4096;

  const force = (x, y, radius, strength, on = true) => ({ x, y, radius, strength, on });
  const DISINTEGRATE_DEFAULTS = {
    mode: "snap",
    pattern: "glass",
    pieces: 1500,
    seed: 1,
    force1: force(85, 50, 70, 40),
    force2: force(20, 30, 30, 30, false),
    time: 50,
    // Shatter's physics
    rotation: 90,
    randomness: 30,
    viscosity: 20,
    gravity: 0,
    gravityDirection: 90,
    // Snap's wind
    wind: 0,
    scatter: 40,
    ragged: 55,
    softness: 35,
    fade: 80,
    shrink: 45,
    lighten: 30,
    editable: true,
  };
  const DISINTEGRATE_PRESETS = [
    ["Drift", () => ({ mode: "snap", pattern: "glass", pieces: 2500, force1: force(90, 50, 80, 45), force2: force(20, 30, 30, 30, false), time: 50, wind: 0, scatter: 40, ragged: 55, softness: 35, fade: 80, shrink: 45, lighten: 30 })],
    ["Dust", () => ({ mode: "snap", pattern: "glass", pieces: 5000, force1: force(80, 30, 90, 70), force2: force(30, 80, 40, 40), time: 60, wind: -20, scatter: 70, ragged: 75, softness: 60, fade: 92, shrink: 65, lighten: 55 })],
    ["Glass", () => ({ mode: "shatter", pattern: "glass", pieces: 160, force1: force(50, 50, 45, 35), force2: force(20, 30, 30, 30, false), time: 50, rotation: 90, randomness: 30, viscosity: 20, gravity: 20, gravityDirection: 90 })],
    ["Crack", () => ({ mode: "shatter", pattern: "glass", pieces: 220, force1: force(60, 45, 35, 8), force2: force(30, 60, 20, 6, false), time: 40, rotation: 25, randomness: 50, viscosity: 60, gravity: 10, gravityDirection: 90 })],
    ["Explode", () => ({ mode: "shatter", pattern: "glass", pieces: 300, force1: force(50, 50, 100, 80), force2: force(20, 30, 30, 30, false), time: 55, rotation: 180, randomness: 40, viscosity: 10, gravity: 25, gravityDirection: 90 })],
    ["Tiles", () => ({ mode: "shatter", pattern: "squares", pieces: 120, force1: force(85, 50, 60, 30), force2: force(20, 30, 30, 30, false), time: 50, rotation: 60, randomness: 25, viscosity: 30, gravity: 40, gravityDirection: 90 })],
  ];
  const PATTERNS = [
    ["glass", "Glass"],
    ["squares", "Squares"],
    ["bricks", "Bricks"],
    ["triangles", "Triangles"],
  ];
  // Rows, each [key, label, unit, range]; a force's keys are "force1.x"
  // and so on
  const forceRows = (n) => [
    [`force${n}.x`, "Position X", "%", () => ({ min: 0, max: 100 })],
    [`force${n}.y`, "Position Y", "%", () => ({ min: 0, max: 100 })],
    [`force${n}.radius`, "Radius", "%", () => ({ min: 0, max: 150, shape: "curve" })],
    [`force${n}.strength`, "Strength", "%", () => ({ min: 0, max: 100, shape: "curve" })],
  ];
  const DISINTEGRATE_ROWS = {
    shape: [["pieces", "Pieces", "", () => ({ min: 10, max: 6000, shape: "curve" })]],
    time: [["time", "Time", "%", () => ({ min: 0, max: 100 })]],
    physics: [
      ["rotation", "Rotation speed", "°", () => ({ min: 0, max: 720, shape: "curve" })],
      ["randomness", "Randomness", "%", () => ({ min: 0, max: 100 })],
      ["viscosity", "Viscosity", "%", () => ({ min: 0, max: 100 })],
      ["gravity", "Gravity", "%", () => ({ min: 0, max: 100, shape: "curve" })],
      ["gravityDirection", "Gravity direction", "°", () => ({ min: -180, max: 180 })],
    ],
    wind: [
      ["wind", "Wind direction", "°", () => ({ min: -180, max: 180 })],
      ["scatter", "Scatter", "%", () => ({ min: 0, max: 100 })],
      ["ragged", "Raggedness", "%", () => ({ min: 0, max: 100 })],
      ["softness", "Edge softness", "%", () => ({ min: 0, max: 100 })],
      ["fade", "Fade", "%", () => ({ min: 0, max: 100 })],
      ["shrink", "Shrink", "%", () => ({ min: 0, max: 100 })],
      ["lighten", "Lighten", "%", () => ({ min: 0, max: 100 })],
    ],
  };
  const DISINTEGRATE_LIMITS = {
    pieces: [2, 20000],
    time: [0, 100],
    rotation: [0, 3600],
    randomness: [0, 100],
    viscosity: [0, 100],
    gravity: [0, 400],
    scatter: [0, 100],
    ragged: [0, 100],
    softness: [0, 100],
    fade: [0, 100],
    shrink: [0, 100],
    lighten: [0, 100],
    "force1.x": [-100, 200],
    "force1.y": [-100, 200],
    "force1.radius": [0, 400],
    "force1.strength": [0, 400],
    "force2.x": [-100, 200],
    "force2.y": [-100, 200],
    "force2.radius": [0, 400],
    "force2.strength": [0, 400],
  };
  // The settings with a force's fields readable as "force1.x" and so on,
  // for the shared slider rows
  const flat = (s) =>
    new Proxy(s, {
      get: (target, key) => (typeof key === "string" && key.includes(".") ? target[key.split(".")[0]][key.split(".")[1]] : target[key]),
      set: (target, key, value) => {
        if (typeof key === "string" && key.includes(".")) target[key.split(".")[0]][key.split(".")[1]] = value;
        else target[key] = value;
        return true;
      },
    });

  // Seeded randomness, the same for the same seed, so a result can be made
  // again
  const hash = (x, y, seed) => {
    let h = Math.imul(x | 0, 374761393) + Math.imul(y | 0, 668265263) + Math.imul(seed | 0, 2147483647);
    h = Math.imul(h ^ (h >>> 13), 1274126177);
    return ((h ^ (h >>> 16)) >>> 0) / 4294967296;
  };
  // Smooth noise, 0 to 1, for a ragged front
  const noise = (x, y, seed) => {
    const [ix, iy] = [Math.floor(x), Math.floor(y)];
    const [fx, fy] = [x - ix, y - iy];
    const [u, v] = [fx * fx * (3 - 2 * fx), fy * fy * (3 - 2 * fy)];
    const a = hash(ix, iy, seed);
    const b = hash(ix + 1, iy, seed);
    const c = hash(ix, iy + 1, seed);
    const d = hash(ix + 1, iy + 1, seed);
    return a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v;
  };

  // `polygon` cut to the side of the line through `point` facing away
  // from `normal`, as a convex polygon's clipped by a half-plane
  const clipPolygon = (polygon, point, normal) => {
    const out = [];
    const side = ([x, y]) => (x - point[0]) * normal[0] + (y - point[1]) * normal[1];
    for (let i = 0; i < polygon.length; i++) {
      const a = polygon[i];
      const b = polygon[(i + 1) % polygon.length];
      const [sa, sb] = [side(a), side(b)];
      if (sa <= 0) out.push(a);
      if (sa <= 0 !== sb <= 0) {
        const t = sa / (sa - sb);
        out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
      }
    }
    return out;
  };

  // The pieces of a `width` by `height` picture in `pattern`, about
  // `count` of them: polygons, each with its center and randomness
  const cutPieces = (width, height, pattern, count, seed) => {
    const cell = Math.max(2, Math.sqrt((width * height) / Math.max(1, count)));
    const pieces = [];
    const add = (polygon) => {
      // Inside the picture
      for (const [point, normal] of [
        [[0, 0], [-1, 0]],
        [[0, 0], [0, -1]],
        [[width, 0], [1, 0]],
        [[0, height], [0, 1]],
      ])
        polygon = clipPolygon(polygon, point, normal);
      if (polygon.length < 3) return;
      let [cx, cy, area] = [0, 0, 0];
      for (let i = 0; i < polygon.length; i++) {
        const [a, b] = [polygon[i], polygon[(i + 1) % polygon.length]];
        const cross = a[0] * b[1] - b[0] * a[1];
        area += cross;
        cx += (a[0] + b[0]) * cross;
        cy += (a[1] + b[1]) * cross;
      }
      if (Math.abs(area) < 1e-6) return;
      const i = pieces.length;
      pieces.push({ polygon, x: cx / (3 * area), y: cy / (3 * area), random: [0, 1, 2, 3].map((k) => hash(i, k, seed + 101)) });
    };
    const across = Math.ceil(width / cell);
    const down = Math.ceil(height / cell);
    if (pattern === "squares" || pattern === "triangles") {
      for (let gy = 0; gy < down; gy++) {
        for (let gx = 0; gx < across; gx++) {
          const [x0, y0, x1, y1] = [gx * cell, gy * cell, (gx + 1) * cell, (gy + 1) * cell];
          if (pattern === "squares") add([[x0, y0], [x1, y0], [x1, y1], [x0, y1]]);
          else if (hash(gx, gy, seed) < 0.5) {
            add([[x0, y0], [x1, y0], [x1, y1]]);
            add([[x0, y0], [x1, y1], [x0, y1]]);
          } else {
            add([[x0, y0], [x1, y0], [x0, y1]]);
            add([[x1, y0], [x1, y1], [x0, y1]]);
          }
        }
      }
    } else if (pattern === "bricks") {
      const [w, h] = [cell * 1.6, cell * 0.65];
      for (let row = 0; row * h < height; row++) {
        const shift = row % 2 ? w / 2 : 0;
        for (let x = -shift; x < width; x += w) add([[x, row * h], [x + w, row * h], [x + w, (row + 1) * h], [x, (row + 1) * h]]);
      }
    } else {
      // Glass: Voronoi cells around a seed jittered in each square of a
      // grid, each cut by the lines halfway to the seeds around it
      const gw = across + 4;
      const gh = down + 4;
      const seedAt = (gx, gy) => [(gx - 2 + 0.5 + (hash(gx, gy, seed) - 0.5) * 0.9) * cell, (gy - 2 + 0.5 + (hash(gx, gy, seed + 7) - 0.5) * 0.9) * cell];
      for (let gy = 2; gy < gh - 2; gy++) {
        for (let gx = 2; gx < gw - 2; gx++) {
          const [sx, sy] = seedAt(gx, gy);
          let polygon = [
            [sx - cell * 2, sy - cell * 2],
            [sx + cell * 2, sy - cell * 2],
            [sx + cell * 2, sy + cell * 2],
            [sx - cell * 2, sy + cell * 2],
          ];
          for (let dy = -2; dy <= 2; dy++) {
            for (let dx = -2; dx <= 2; dx++) {
              if (!dx && !dy) continue;
              const [ox, oy] = seedAt(gx + dx, gy + dy);
              polygon = clipPolygon(polygon, [(sx + ox) / 2, (sy + oy) / 2], [ox - sx, oy - sy]);
              if (polygon.length < 3) break;
            }
          }
          if (polygon.length >= 3) add(polygon);
        }
      }
    }
    return pieces;
  };

  // Where each piece goes at the settings' time: its move, turn (radians),
  // size and opacity, or null where it stays whole. `size` is the layer's
  // larger side, in the picture's pixels
  const simulate = (pieces, s, width, height, size) => {
    const forces = [s.force1, s.force2]
      .filter((f) => f.on && f.radius > 0)
      .map((f) => ({ x: (f.x / 100) * width, y: (f.y / 100) * height, radius: (f.radius / 100) * size, strength: f.strength / 100 }));
    const time = s.time / 100;
    const moves = new Array(pieces.length).fill(null);
    if (!forces.length || time <= 0) return moves;
    if (s.mode === "shatter") {
      // Thrown out from each force, more near its middle, then slowed by
      // viscosity and pulled by gravity, as AE's Shatter
      const viscosity = (s.viscosity / 100) * 4;
      const travel = viscosity > 0 ? (1 - Math.exp(-viscosity * time)) / viscosity : time;
      const fall = ((s.gravity / 100) * size * 1.5 * time * time) / 2;
      const ga = (s.gravityDirection * Math.PI) / 180;
      const randomness = s.randomness / 100;
      pieces.forEach((piece, i) => {
        let [vx, vy, hit] = [0, 0, 0];
        for (const f of forces) {
          const [dx, dy] = [piece.x - f.x, piece.y - f.y];
          const d = Math.hypot(dx, dy);
          if (d >= f.radius) continue;
          const push = f.strength * (1 - d / f.radius);
          const [ux, uy] = d > 1e-6 ? [dx / d, dy / d] : [Math.cos(piece.random[0] * 6.283), Math.sin(piece.random[0] * 6.283)];
          vx += ux * push;
          vy += uy * push;
          hit = Math.max(hit, push);
        }
        if (!hit) return;
        const [r0, r1, r2] = piece.random;
        // Randomness turns and speeds each piece a little differently
        const turn = (r0 - 0.5) * 2 * randomness * 1.2;
        const speed = (1 + (r1 - 0.5) * 2 * randomness) * size * 1.5;
        const [cos, sin] = [Math.cos(turn), Math.sin(turn)];
        const [mx, my] = [(vx * cos - vy * sin) * speed * travel, (vx * sin + vy * cos) * speed * travel];
        moves[i] = {
          x: mx + Math.cos(ga) * fall,
          y: my + Math.sin(ga) * fall,
          turn: (((r2 - 0.5) * 2 * s.rotation * time * Math.min(1, hit * 2 + 0.3)) * Math.PI) / 180,
          scale: 1,
          opacity: 1,
          light: 0,
        };
      });
    } else {
      // Dust: a ragged front spreads out from each force as time goes, as
      // far as its radius; what it passes is blown off with the wind
      const wa = (s.wind * Math.PI) / 180;
      const [wx, wy] = [Math.cos(wa), Math.sin(wa)];
      const soft = 0.03 + (s.softness / 100) * 0.6;
      const rag = s.ragged / 100;
      const scale = size / 8;
      pieces.forEach((piece, i) => {
        let life = 0;
        let strength = 0;
        for (const f of forces) {
          const reach = Math.hypot(piece.x - f.x, piece.y - f.y) / f.radius;
          if (reach >= 1) continue;
          const rough = rag * (noise(piece.x / scale, piece.y / scale, s.seed) * 0.7 + noise((piece.x * 3) / scale, (piece.y * 3) / scale, s.seed + 3) * 0.3);
          const when = reach * (1 - rag * 0.6) + rough * 0.6 + (piece.random[3] - 0.5) * 0.06;
          const gone = Math.min(1, Math.max(0, (time * (1 + soft) - when) / soft));
          if (gone > life) [life, strength] = [gone, f.strength];
        }
        if (life <= 0) return;
        const [r0, r1, r2] = piece.random;
        // Slow to leave, then faster
        const distance = strength * size * 1.5 * life * life * (0.4 + r0 * 1.2);
        const side = (r1 - 0.5) * 2 * (s.scatter / 100) * distance;
        moves[i] = {
          x: wx * distance - wy * side,
          y: wy * distance + wx * side,
          turn: (r2 - 0.5) * 2 * Math.PI * life,
          scale: Math.max(0.05, 1 - (s.shrink / 100) * life * (0.6 + piece.random[3] * 0.8)),
          opacity: 1 - (s.fade / 100) * life * life,
          light: (s.lighten / 100) * life,
        };
      });
    }
    return moves;
  };

  // Draws the broken layer from `picture`, a canvas of it, `k` its pixels
  // per layer pixel. Gives a canvas cropped to what's drawn, and where it
  // sits relative to the picture's top left, in its pixels
  const drawBroken = (picture, pieces, s, k) => {
    const { width, height } = picture;
    const moves = simulate(pieces, s, width, height, Math.max(width, height));
    // The bounds of everything drawn
    let [left, top, right, bottom] = [0, 0, width, height];
    const corners = [];
    pieces.forEach((piece, i) => {
      const move = moves[i];
      if (!move || move.opacity <= 0.02) return;
      const [cos, sin] = [Math.cos(move.turn), Math.sin(move.turn)];
      for (const [px, py] of piece.polygon) {
        const [dx, dy] = [(px - piece.x) * move.scale, (py - piece.y) * move.scale];
        const x = piece.x + move.x + dx * cos - dy * sin;
        const y = piece.y + move.y + dx * sin + dy * cos;
        if (x < left) left = x;
        if (x > right) right = x;
        if (y < top) top = y;
        if (y > bottom) bottom = y;
      }
      corners.push(i);
    });
    [left, top, right, bottom] = [Math.floor(left) - 1, Math.floor(top) - 1, Math.ceil(right) + 1, Math.ceil(bottom) + 1];
    const canvas = Object.assign(document.createElement("canvas"), { width: right - left, height: bottom - top });
    const context = canvas.getContext("2d");
    const path = (polygon) => {
      context.beginPath();
      polygon.forEach(([x, y], j) => (j ? context.lineTo(x, y) : context.moveTo(x, y)));
      context.closePath();
    };
    // The picture, with the moving pieces cut out of it
    context.drawImage(picture, -left, -top);
    context.globalCompositeOperation = "destination-out";
    context.setTransform(1, 0, 0, 1, -left, -top);
    context.beginPath();
    pieces.forEach((piece, i) => {
      if (!moves[i]) return;
      piece.polygon.forEach(([x, y], j) => (j ? context.lineTo(x, y) : context.moveTo(x, y)));
      context.closePath();
    });
    context.fill();
    context.globalCompositeOperation = "source-over";
    // Each moving piece, those gone least first, so the farthest are on top
    corners.sort((a, b) => Math.hypot(moves[a].x, moves[a].y) - Math.hypot(moves[b].x, moves[b].y));
    for (const i of corners) {
      const piece = pieces[i];
      const move = moves[i];
      const [cos, sin] = [Math.cos(move.turn), Math.sin(move.turn)];
      context.save();
      // To where it's going, turned and sized about its center
      context.setTransform(1, 0, 0, 1, -left, -top);
      context.translate(piece.x + move.x, piece.y + move.y);
      context.transform(cos * move.scale, sin * move.scale, -sin * move.scale, cos * move.scale, 0, 0);
      context.translate(-piece.x, -piece.y);
      path(piece.polygon);
      context.clip();
      context.globalAlpha = move.opacity;
      let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
      for (const [x, y] of piece.polygon) [x0, y0, x1, y1] = [Math.min(x0, x), Math.min(y0, y), Math.max(x1, x), Math.max(y1, y)];
      [x0, y0] = [Math.max(0, Math.floor(x0)), Math.max(0, Math.floor(y0))];
      [x1, y1] = [Math.min(width, Math.ceil(x1)), Math.min(height, Math.ceil(y1))];
      if (x1 > x0 && y1 > y0) context.drawImage(picture, x0, y0, x1 - x0, y1 - y0, x0, y0, x1 - x0, y1 - y0);
      if (move.light > 0) {
        context.globalCompositeOperation = "source-atop";
        context.globalAlpha = move.light * move.opacity;
        context.fillStyle = "#fff";
        context.fill();
      }
      context.restore();
    }
    return { canvas, x: left, y: top };
  };

  // A picture of `bitmap` at most `most` pixels across, as a canvas, and
  // its pixels per layer pixel given the full one's `k`
  const scaledPicture = (bitmap, most, k) => {
    const ratio = Math.min(1, most / Math.max(bitmap.width, bitmap.height));
    const canvas = Object.assign(document.createElement("canvas"), { width: Math.max(1, Math.round(bitmap.width * ratio)), height: Math.max(1, Math.round(bitmap.height * ratio)) });
    canvas.getContext("2d").drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    return { picture: canvas, k: k * ratio };
  };

  // Pieces cut once for each picture, pattern, count and seed
  const piecesFor = (current, level) => {
    const s = current.settings;
    const { picture } = current.levels[level];
    const key = `${s.pattern}:${s.pieces}:${s.seed}`;
    current.cuts ??= {};
    if (current.cuts[level]?.key !== key) current.cuts[level] = { key, pieces: cutPieces(picture.width, picture.height, s.pattern, s.pieces, s.seed) };
    return current.cuts[level].pieces;
  };

  const renderAt = (current, level) => {
    const { picture, k } = current.levels[level];
    return drawBroken(picture, piecesFor(current, level), current.settings, k);
  };

  // From the page to the coordinates of what's in `parent`: a frame's
  // own, or for a group, which has none, those of what holds it
  const toParent = (parent, x, y) => {
    while (parent && (parent.type === "GROUP" || parent.type === "BOOLEAN_OPERATION")) parent = parent.parent;
    if (!parent || parent.type === "PAGE") return { x, y, linear: [1, 0, 0, 1] };
    const [[a, c, e], [b, d, f]] = parent.absoluteTransform;
    const det = a * d - b * c;
    const [ia, ib, ic, id] = [d / det, -b / det, -c / det, a / det];
    return { x: ia * (x - e) + ic * (y - f), y: ib * (x - e) + id * (y - f), linear: [ia, ic, ib, id] };
  };

  // Puts `drawn`, from `renderAt`, on the result layer, as an image the
  // size it covers in the file
  const placeDrawn = async (current, drawn, k) => {
    const { figma, result, bounds } = current;
    if (!drawn || result.removed) return;
    const blob = await new Promise((resolve) => drawn.canvas.toBlob(resolve, "image/png"));
    const bytes = new Uint8Array(await blob.arrayBuffer());
    if (result.removed) return;
    const hash = figma.createImage(bytes).hash;
    const place = toParent(result.parent, bounds.x + drawn.x / k, bounds.y + drawn.y / k);
    result.resizeWithoutConstraints(Math.max(0.01, drawn.canvas.width / k), Math.max(0.01, drawn.canvas.height / k));
    result.relativeTransform = [
      [place.linear[0], place.linear[1], place.x],
      [place.linear[2], place.linear[3], place.y],
    ];
    result.fills = [{ type: "IMAGE", imageHash: hash, scaleMode: "FILL" }];
    result.visible = true;
  };

  const disintegrateOf = (node) => {
    try {
      return node?.type === "GROUP" && node.getSharedPluginData(PLUGIN, DISINTEGRATE) ? node : null;
    } catch {
      return null;
    }
  };

  let disintegrating = null;

  const openDisintegrate = async (figma) => {
    if (repeating || disintegrating) return;
    const selected = figma.currentPage.selection;
    if (selected.length !== 1) {
      figma.notify(selected.length ? "Select one layer to disintegrate" : "Select a layer to disintegrate", { timeout: 2000 });
      return;
    }
    let group = disintegrateOf(selected[0]);
    let source;
    let result = null;
    const settings = structuredClone(DISINTEGRATE_DEFAULTS);
    if (group) {
      source = group.children.find((child) => child.getSharedPluginData(PLUGIN, ROLE) === "source");
      result = group.children.find((child) => child.getSharedPluginData(PLUGIN, ROLE) === "result") ?? null;
      try {
        const saved = JSON.parse(group.getSharedPluginData(PLUGIN, DISINTEGRATE));
        // Settings from before forces, which aren't kept
        if (saved.force1) Object.assign(settings, saved);
      } catch {}
      if (!source) {
        group = null;
        source = selected[0];
      }
    } else {
      source = selected[0];
    }
    if (source.type === "SECTION" || !("exportAsync" in source)) {
      figma.notify("Sections can't be disintegrated", { timeout: 2000 });
      return;
    }
    if (!group) Object.assign(settings, structuredClone(DISINTEGRATE_PRESETS[0][1]()));
    const current = {
      figma,
      group,
      source,
      settings,
      unit: Math.max(1, Math.max(source.width ?? 100, source.height ?? 100)),
      result,
      kept: result ? { fills: JSON.stringify(result.fills), width: result.width, height: result.height, transform: result.relativeTransform } : null,
      levels: {},
      wasVisible: source.visible,
    };
    disintegrating = current;
    try {
      figma.commitUndo();
    } catch {}
    // The layer as a picture, as large as an image can be in Figma
    const visible = source.visible;
    source.visible = true;
    try {
      current.bounds = source.absoluteRenderBounds ?? source.absoluteBoundingBox;
      const most = Math.min(2, MAX_SIDE / 2 / Math.max(1, current.bounds.width, current.bounds.height));
      const bytes = await source.exportAsync({ format: "PNG", constraint: { type: "SCALE", value: most } });
      current.bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
    } catch {
      source.visible = visible;
      disintegrating = null;
      figma.notify("Couldn't read the layer", { timeout: 2000 });
      return;
    }
    if (disintegrating !== current) return;
    const k = current.bitmap.width / current.bounds.width;
    current.levels = {
      preview: scaledPicture(current.bitmap, 360, k),
      live: scaledPicture(current.bitmap, 900, k),
      full: scaledPicture(current.bitmap, MAX_SIDE, k),
    };
    // A new one's grouped from the start, as Repeater's are, the result
    // over the original, which is hidden while it shows
    const parent = source.parent;
    if (!group && parent && !("layoutMode" in parent && parent.layoutMode !== "NONE")) {
      current.home = { parent, index: parent.children.indexOf(source) };
      try {
        current.wrap = figma.group([source], parent, current.home.index);
        current.wrap.name = `Disintegrate · ${source.name}`;
      } catch {
        current.wrap = null;
      }
    }
    if (!current.result) {
      current.result = figma.createRectangle();
      current.result.name = `${source.name} disintegrated`;
      const holder = source.parent;
      holder.insertChild(holder.children.indexOf(source) + 1, current.result);
      if ("layoutMode" in holder && holder.layoutMode !== "NONE") current.result.layoutPositioning = "ABSOLUTE";
      current.result.setSharedPluginData(PLUGIN, ROLE, "result");
      current.result.visible = false;
    }
    source.visible = false;
    figma.currentPage.selection = [current.wrap ?? current.group ?? current.result];
    buildDisintegratePanel(current);
    refreshDisintegrate(current);
  };

  // The panel's preview at once, and the file's a moment after the last
  // change, at a size quick to make
  const refreshDisintegrate = (current) => {
    drawDisintegratePreview(current);
    clearTimeout(current.timer);
    current.timer = setTimeout(async () => {
      if (disintegrating !== current) return;
      await placeDrawn(current, renderAt(current, "live"), current.levels.live.k);
    }, 120);
  };

  // Where the preview put the picture last, to place the forces' handles
  const drawDisintegratePreview = (current) => {
    const canvas = current.preview;
    if (!canvas || !current.levels.preview) return;
    const drawn = renderAt(current, "preview");
    const ratio = devicePixelRatio || 1;
    const box = canvas.getBoundingClientRect();
    canvas.width = Math.round(box.width * ratio);
    canvas.height = Math.round(box.height * ratio);
    const context = canvas.getContext("2d");
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, box.width, box.height);
    const { width, height } = current.levels.preview.picture;
    // The layer framed with room around it for what flies off, the same
    // however far that goes, so the view holds still while dragging
    const room = Math.max(width, height) * 0.35;
    const [left, top, right, bottom] = [-room, -room, width + room, height + room];
    const fit = Math.min((box.width - 8) / (right - left), (box.height - 8) / (bottom - top));
    current.view = { fit, ox: (box.width - (right - left) * fit) / 2 - left * fit, oy: (box.height - (bottom - top) * fit) / 2 - top * fit, width, height };
    const { ox, oy } = current.view;
    context.imageSmoothingQuality = "high";
    context.drawImage(drawn.canvas, ox + drawn.x * fit, oy + drawn.y * fit, drawn.canvas.width * fit, drawn.canvas.height * fit);
    // The forces: a dot to drag, and a ring for its radius
    const s = current.settings;
    const size = Math.max(width, height);
    [s.force1, s.force2].forEach((f, n) => {
      if (!f.on) return;
      const x = ox + (f.x / 100) * width * fit;
      const y = oy + (f.y / 100) * height * fit;
      const r = (f.radius / 100) * size * fit;
      context.strokeStyle = "rgba(13, 153, 255, 0.9)";
      context.fillStyle = "#fff";
      context.lineWidth = 1;
      context.setLineDash([4, 3]);
      context.beginPath();
      context.arc(x, y, Math.max(2, r), 0, Math.PI * 2);
      context.stroke();
      context.setLineDash([]);
      context.beginPath();
      context.arc(x, y, 5, 0, Math.PI * 2);
      context.fill();
      context.stroke();
      context.fillStyle = "rgba(13, 153, 255, 1)";
      context.font = "600 9px Inter, sans-serif";
      context.textAlign = "center";
      context.textBaseline = "middle";
      context.fillText(String(n + 1), x, y + 0.5);
    });
  };

  // Dragging a force's dot moves it, and its ring resizes it
  const dragForces = (current, canvas, change) => {
    const hit = (event) => {
      const v = current.view;
      if (!v) return null;
      const box = canvas.getBoundingClientRect();
      const [px, py] = [event.clientX - box.left, event.clientY - box.top];
      const size = Math.max(v.width, v.height);
      for (const n of [2, 1]) {
        const f = current.settings[`force${n}`];
        if (!f.on) continue;
        const x = v.ox + (f.x / 100) * v.width * v.fit;
        const y = v.oy + (f.y / 100) * v.height * v.fit;
        const r = (f.radius / 100) * size * v.fit;
        const d = Math.hypot(px - x, py - y);
        if (d <= 8) return { n, what: "move", px, py, x, y };
        if (Math.abs(d - r) <= 6) return { n, what: "size", px, py, x, y };
      }
      return null;
    };
    canvas.addEventListener("pointermove", (event) => {
      if (canvas.hasPointerCapture(event.pointerId)) return;
      const at = hit(event);
      canvas.style.cursor = at ? (at.what === "move" ? "move" : "ew-resize") : "";
    });
    canvas.addEventListener("pointerdown", (event) => {
      const at = hit(event);
      if (!at || event.button !== 0) return;
      event.preventDefault();
      canvas.setPointerCapture(event.pointerId);
      const v = current.view;
      const size = Math.max(v.width, v.height);
      const move = (event) => {
        const box = canvas.getBoundingClientRect();
        const [px, py] = [event.clientX - box.left, event.clientY - box.top];
        if (at.what === "move") {
          change(`force${at.n}.x`, Math.round(((px - v.ox) / (v.width * v.fit)) * 100));
          change(`force${at.n}.y`, Math.round(((py - v.oy) / (v.height * v.fit)) * 100));
        } else {
          change(`force${at.n}.radius`, Math.round((Math.hypot(px - at.x, py - at.y) / (size * v.fit)) * 100));
        }
      };
      canvas.addEventListener("pointermove", move);
      canvas.addEventListener("lostpointercapture", () => canvas.removeEventListener("pointermove", move), { once: true });
    });
  };

  const applyDisintegrate = async (current) => {
    const { figma, source, settings } = current;
    clearTimeout(current.timer);
    current.busy = true;
    current.panel?.busy(true);
    await placeDrawn(current, renderAt(current, "full"), current.levels.full.k);
    if (disintegrating !== current) return;
    let result = current.group ?? current.wrap;
    if (settings.editable) {
      if (!result || result.removed) {
        const parent = source.parent;
        result = figma.group([source, current.result], parent, parent.children.indexOf(source));
        result.name = `Disintegrate · ${source.name}`;
      }
      source.setSharedPluginData(PLUGIN, ROLE, "source");
      result.setSharedPluginData(PLUGIN, DISINTEGRATE, JSON.stringify(settings));
      figma.currentPage.selection = [result];
    } else {
      // Only the image, the original gone
      current.result.setSharedPluginData(PLUGIN, ROLE, "");
      if (result && !result.removed) result.setSharedPluginData(PLUGIN, DISINTEGRATE, "");
      const image = current.result;
      const holder = current.wrap ?? current.group;
      if (holder && !holder.removed) figma.ungroup(holder);
      if (!source.removed) source.remove();
      image.name = image.name.replace(/ disintegrated$/, "");
      figma.currentPage.selection = [image];
    }
    try {
      figma.commitUndo();
    } catch {}
    closeDisintegrate(current, true);
  };

  const closeDisintegrate = (current, applied) => {
    if (disintegrating !== current) return;
    disintegrating = null;
    clearTimeout(current.timer);
    if (!applied) {
      const { figma, source, result, home, wrap } = current;
      if (current.group) {
        // What it was before: its image, size and place
        try {
          if (current.kept && !result.removed) {
            result.resizeWithoutConstraints(current.kept.width, current.kept.height);
            result.relativeTransform = current.kept.transform;
            result.fills = JSON.parse(current.kept.fills);
            result.visible = true;
          }
        } catch {}
      } else {
        if (result && !result.removed) result.remove();
        if (!source.removed) source.visible = current.wasVisible;
        if (wrap && home && !source.removed && !home.parent.removed) {
          try {
            home.parent.insertChild(Math.min(home.index, home.parent.children.length), source);
            if (!wrap.removed) wrap.remove();
          } catch {}
        }
      }
      try {
        if (current.group && !current.group.removed) figma.currentPage.selection = [current.group];
        else if (!source.removed) figma.currentPage.selection = [source];
      } catch {}
    }
    current.bitmap?.close?.();
    current.panel?.close();
    current.root?.remove();
    removeEventListener("keydown", current.keys, true);
  };

  const buildDisintegratePanel = (current) => {
    const s = current.settings;
    const view = flat(s);
    const rows = [];
    const change = (key, value) => {
      view[key] = value;
      refreshDisintegrate(current);
      show();
    };
    const slider = makeSlider({ s: view, unit: current.unit, rows, change, limits: DISINTEGRATE_LIMITS, scrub: { pieces: 5 } });
    const canvas = element("canvas", { className: "preview" });
    canvas.setAttribute("aria-label", "Preview: drag a force's dot to move it, or its ring to change its radius");
    current.preview = canvas;
    dragForces(current, canvas, (key, value) => {
      view[key] = Math.min(DISINTEGRATE_LIMITS[key][1], Math.max(DISINTEGRATE_LIMITS[key][0], value));
      refreshDisintegrate(current);
      show();
    });

    const pick = (values) => {
      Object.assign(s, structuredClone(values()));
      refreshDisintegrate(current);
      show();
    };
    const presets = makeChips(DISINTEGRATE_PRESETS, pick);
    // Picking a mode starts from its first preset, keeping the forces
    const mode = segments(
      "Mode",
      [
        ["snap", "Snap"],
        ["shatter", "Shatter"],
      ],
      () => s.mode,
      (value) => {
        if (value === s.mode) return;
        const [force1, force2] = [s.force1, s.force2];
        const first = DISINTEGRATE_PRESETS.find(([, values]) => values().mode === value);
        Object.assign(s, structuredClone(first[1]()), { force1, force2 });
        refreshDisintegrate(current);
        show();
      },
    );
    const pattern = select("Pattern", PATTERNS, () => s.pattern, (value) => change("pattern", value));
    const shuffle = element("button", { type: "button", className: "chip", textContent: "Shuffle" });
    shuffle.addEventListener("click", () => change("seed", (s.seed % 9999) + 1));
    shuffle.style.gridColumn = "span 2";
    shuffle.style.justifySelf = "start";
    const shape = element(
      "section",
      {},
      element("div", { className: "row" }, element("span", { className: "label", textContent: "Mode" }), mode),
      element("label", { className: "row" }, element("span", { className: "label", textContent: "Pattern" }), pattern),
      element("div", { className: "rows" }, ...DISINTEGRATE_ROWS.shape.map(slider)),
      element("div", { className: "row" }, element("span", { className: "label", textContent: "Variation" }), shuffle),
    );
    const force1 = element("section", {}, element("div", { className: "caption", textContent: "Force 1" }), element("div", { className: "rows" }, ...forceRows(1).map(slider)));
    const second = element("input", { type: "checkbox" });
    second.addEventListener("change", () => {
      s.force2.on = second.checked;
      refreshDisintegrate(current);
      show();
    });
    const force2Rows = element("div", { className: "rows" }, ...forceRows(2).map(slider));
    const force2 = element("section", {}, checkRow("Force 2", second), force2Rows);
    const time = element("section", {}, element("div", { className: "rows" }, ...DISINTEGRATE_ROWS.time.map(slider)));
    const physics = element("section", {}, element("div", { className: "caption", textContent: "Physics" }), element("div", { className: "rows" }, ...DISINTEGRATE_ROWS.physics.map(slider)));
    const wind = element("section", {}, element("div", { className: "caption", textContent: "Dust" }), element("div", { className: "rows" }, ...DISINTEGRATE_ROWS.wind.map(slider)));
    const editable = element("input", { type: "checkbox" });
    editable.addEventListener("change", () => change("editable", editable.checked));
    const result = element("section", {}, checkRow("Keep editable", editable, "The original kept, hidden, in a group Disintegrate can change later"));

    const reset = element("button", { type: "button", className: "link", textContent: "Reset" });
    reset.addEventListener("click", () => pick(DISINTEGRATE_PRESETS.find(([, values]) => values().mode === s.mode)[1]));
    const scroll = element("div", { className: "scroll" }, presets, shape, time, force1, force2, physics, wind, result, element("div", { className: "under" }, reset));

    const name = element("div", { className: "source", textContent: current.source.name });
    const cancel = element("button", { type: "button", className: "button", textContent: "Cancel" });
    cancel.addEventListener("click", () => closeDisintegrate(current, false));
    const go = element("button", { type: "button", className: "button primary", textContent: current.group ? "Update" : "Disintegrate" });
    go.addEventListener("click", () => !current.busy && applyDisintegrate(current));
    const footer = element("div", { className: "footer" }, name, element("div", { className: "actions" }, cancel, go));

    const root = makePanel(current.group ? `Disintegrate · ${current.source.name}` : "Disintegrate", () => closeDisintegrate(current, false), canvas, scroll, footer);
    const show = () => {
      mode.show();
      pattern.show();
      for (const row of rows) row.show();
      second.checked = s.force2.on;
      force2Rows.hidden = !s.force2.on;
      physics.hidden = s.mode !== "shatter";
      wind.hidden = s.mode !== "snap";
      editable.checked = s.editable;
      drawDisintegratePreview(current);
    };
    show();
    current.root = root;
    current.panel = {
      busy: (on) => {
        go.disabled = on;
        go.textContent = on ? "Making…" : current.group ? "Update" : "Disintegrate";
      },
      close: () => pattern.close(),
    };
    current.keys = panelKeys(root, () => disintegrating === current, () => closeDisintegrate(current, false), () => !current.busy && applyDisintegrate(current));
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
