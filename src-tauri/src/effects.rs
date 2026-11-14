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

  // Cracks the layer into pieces, Voronoi cells around a seed jittered in
  // each square of a grid, and moves each once a ragged front sweeping
  // across the layer reaches it: blown along, scattered, spun, shrunk,
  // faded and lightened as it goes. Snap is fine dust, as the layer
  // crumbles away; Shatter is glass, as After Effects' Shatter breaks it.
  // It's drawn on the layer's pixels, here in the page, and put back as an
  // image, the original kept under it, hidden
  const DISINTEGRATE = "disintegrate";
  const MAX_SIDE = 4096;

  const DISINTEGRATE_DEFAULTS = {
    mode: "snap",
    progress: 45,
    angle: 0,
    burst: false,
    softness: 35,
    ragged: 55,
    piece: 4,
    distance: 160,
    scatter: 45,
    spin: 180,
    gravity: 0,
    fade: 85,
    shrink: 55,
    lighten: 35,
    seed: 1,
    editable: true,
  };
  // Each mode's own look, which picking it, or its presets, start from
  const DISINTEGRATE_PRESETS = [
    ["Drift", (u) => ({ mode: "snap", progress: 45, angle: 0, burst: false, softness: 35, ragged: 55, piece: Math.max(2, u / 120), distance: u * 0.9, scatter: 40, spin: 180, gravity: 0, fade: 80, shrink: 45, lighten: 30 })],
    ["Dust", (u) => ({ mode: "snap", progress: 70, angle: -20, burst: false, softness: 70, ragged: 75, piece: Math.max(1.5, u / 220), distance: u * 1.2, scatter: 70, spin: 240, gravity: -10, fade: 95, shrink: 70, lighten: 55 })],
    ["Crumble", (u) => ({ mode: "snap", progress: 50, angle: 90, burst: false, softness: 30, ragged: 60, piece: Math.max(2, u / 90), distance: u * 0.6, scatter: 25, spin: 120, gravity: 60, fade: 60, shrink: 30, lighten: 10 })],
    ["Glass", (u) => ({ mode: "shatter", progress: 45, angle: 0, burst: false, softness: 25, ragged: 30, piece: Math.max(4, u / 14), distance: u * 0.7, scatter: 35, spin: 90, gravity: 25, fade: 25, shrink: 0, lighten: 0 })],
    ["Explode", (u) => ({ mode: "shatter", progress: 55, angle: 0, burst: true, softness: 30, ragged: 20, piece: Math.max(4, u / 18), distance: u * 0.9, scatter: 30, spin: 180, gravity: 15, fade: 30, shrink: 10, lighten: 0 })],
  ];
  const DISINTEGRATE_ROWS = {
    front: [
      ["progress", "Progress", "%", () => ({ min: 0, max: 100 })],
      ["angle", "Direction", "°", () => ({ min: -180, max: 180 })],
      ["softness", "Edge softness", "%", () => ({ min: 0, max: 100 })],
      ["ragged", "Raggedness", "%", () => ({ min: 0, max: 100 })],
    ],
    pieces: [
      ["piece", "Piece size", "px", (u) => ({ min: 1, max: Math.max(40, Math.round(u / 4)), shape: "curve" })],
      ["distance", "Distance", "px", (u) => ({ min: 0, max: near(u, 100, 3), shape: "curve" })],
      ["scatter", "Scatter", "%", () => ({ min: 0, max: 100 })],
      ["spin", "Spin", "°", () => ({ min: 0, max: 720, shape: "curve" })],
      ["gravity", "Gravity", "%", () => ({ min: -100, max: 100 })],
    ],
    look: [
      ["fade", "Fade", "%", () => ({ min: 0, max: 100 })],
      ["shrink", "Shrink", "%", () => ({ min: 0, max: 100 })],
      ["lighten", "Lighten", "%", () => ({ min: 0, max: 100 })],
    ],
  };
  const DISINTEGRATE_LIMITS = { progress: [0, 100], softness: [0, 100], ragged: [0, 100], piece: [1, 500], distance: [0, Infinity], scatter: [0, 100], spin: [0, 3600], gravity: [-100, 100], fade: [0, 100], shrink: [0, 100], lighten: [0, 100] };

  // Seeded randomness, the same for the same seed, so a result can be
  // made again
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

  // The pieces of a `width` by `height` picture with alpha `alpha`, cells
  // `cell` pixels across: which piece each pixel is in, and each piece's
  // center, box and randomness
  const crack = (width, height, alpha, cell, seed, jitter) => {
    const across = Math.ceil(width / cell) + 2;
    const down = Math.ceil(height / cell) + 2;
    const seedX = new Float32Array(across * down);
    const seedY = new Float32Array(across * down);
    for (let gy = 0; gy < down; gy++) {
      for (let gx = 0; gx < across; gx++) {
        const i = gy * across + gx;
        seedX[i] = (gx - 1 + 0.5 + (hash(gx, gy, seed) - 0.5) * jitter) * cell;
        seedY[i] = (gy - 1 + 0.5 + (hash(gx, gy, seed + 7) - 0.5) * jitter) * cell;
      }
    }
    const cellOf = new Int32Array(width * height).fill(-1);
    for (let y = 0; y < height; y++) {
      const cy = Math.floor(y / cell) + 1;
      for (let x = 0; x < width; x++) {
        if (alpha[(y * width + x) * 4 + 3] === 0) continue;
        const cx = Math.floor(x / cell) + 1;
        let best = -1;
        let nearest = Infinity;
        for (let dy = -1; dy <= 1; dy++) {
          for (let dx = -1; dx <= 1; dx++) {
            const i = (cy + dy) * across + cx + dx;
            const ddx = seedX[i] - x;
            const ddy = seedY[i] - y;
            const d = ddx * ddx + ddy * ddy;
            if (d < nearest) [nearest, best] = [d, i];
          }
        }
        cellOf[y * width + x] = best;
      }
    }
    // Only the cells with pixels in them, numbered from 0
    const number = new Int32Array(across * down).fill(-1);
    let count = 0;
    for (let i = 0; i < cellOf.length; i++) {
      const c = cellOf[i];
      if (c >= 0 && number[c] < 0) number[c] = count++;
    }
    const piece = new Int32Array(width * height).fill(-1);
    const sumX = new Float64Array(count);
    const sumY = new Float64Array(count);
    const size = new Uint32Array(count);
    const box = new Int32Array(count * 4);
    for (let p = 0; p < count; p++) box.set([width, height, -1, -1], p * 4);
    for (let y = 0; y < height; y++) {
      for (let x = 0; x < width; x++) {
        const c = cellOf[y * width + x];
        if (c < 0) continue;
        const p = number[c];
        piece[y * width + x] = p;
        sumX[p] += x;
        sumY[p] += y;
        size[p]++;
        const b = p * 4;
        if (x < box[b]) box[b] = x;
        if (y < box[b + 1]) box[b + 1] = y;
        if (x > box[b + 2]) box[b + 2] = x;
        if (y > box[b + 3]) box[b + 3] = y;
      }
    }
    const centerX = new Float32Array(count);
    const centerY = new Float32Array(count);
    const random = new Float32Array(count * 4);
    for (let p = 0; p < count; p++) {
      centerX[p] = sumX[p] / size[p];
      centerY[p] = sumY[p] / size[p];
      for (let k = 0; k < 4; k++) random[p * 4 + k] = hash(p, k, seed + 101);
    }
    return { width, height, piece, count, centerX, centerY, box, random };
  };

  // Draws the disintegrated picture: `image` ImageData, `pieces` from
  // `crack`, `k` its pixels per layer pixel. Gives the drawn pixels, as
  // ImageData, cropped to what's drawn, and where that sits relative to
  // the picture's top left, in its pixels
  const disintegrate = (image, pieces, s, k) => {
    const { width, height, data } = image;
    const { piece, count, centerX, centerY, box, random } = pieces;
    const travel = s.distance * k;
    const margin = Math.ceil(travel * (1.8 + Math.abs(s.gravity) / 100) + s.piece * k * 2 + 2);
    const outWidth = width + margin * 2;
    const outHeight = height + margin * 2;
    const out = new Uint8ClampedArray(outWidth * outHeight * 4);
    const angle = (s.angle * Math.PI) / 180;
    const [dirX, dirY] = [Math.cos(angle), Math.sin(angle)];
    // How far along the sweep each corner is, to put the front 0 to 1
    const corners = [0, width * dirX, height * dirY, width * dirX + height * dirY];
    const [low, high] = [Math.min(...corners), Math.max(...corners)];
    const [midX, midY] = [width / 2, height / 2];
    const reach = Math.hypot(midX, midY) || 1;
    const soft = 0.03 + (s.softness / 100) * 0.55;
    const rag = s.ragged / 100;
    const scale = 40 * k + s.piece * k * 2;
    const progress = (s.progress / 100) * (1 + soft);
    // How far each piece has gone, 0 still in place to 1 all the way
    const gone = new Float32Array(count);
    for (let p = 0; p < count; p++) {
      const [x, y] = [centerX[p], centerY[p]];
      const along = s.burst ? Math.hypot(x - midX, y - midY) / reach : 1 - (x * dirX + y * dirY - low) / Math.max(1e-6, high - low);
      const rough = rag * (noise(x / scale, y / scale, s.seed) * 0.7 + noise(x / (scale / 3), y / (scale / 3), s.seed + 3) * 0.3);
      const when = along * (1 - rag * 0.6) + rough * 0.6 + (random[p * 4 + 3] - 0.5) * 0.06;
      gone[p] = Math.min(1, Math.max(0, (progress - when) / soft));
    }
    // What's still in place, as it was
    for (let y = 0; y < height; y++) {
      for (let x = 0; x < width; x++) {
        const p = piece[y * width + x];
        if (p < 0 || gone[p] > 0) continue;
        const from = (y * width + x) * 4;
        const to = ((y + margin) * outWidth + x + margin) * 4;
        out[to] = data[from];
        out[to + 1] = data[from + 1];
        out[to + 2] = data[from + 2];
        out[to + 3] = data[from + 3];
      }
    }
    // The pieces on their way, the least gone first so the farthest are on
    // top, each drawn by finding where each pixel it covers came from
    const moving = [];
    for (let p = 0; p < count; p++) if (gone[p] > 0) moving.push(p);
    moving.sort((a, b) => gone[a] - gone[b]);
    const fade = s.fade / 100;
    const shrink = s.shrink / 100;
    const lighten = s.lighten / 100;
    const scatter = s.scatter / 100;
    for (const p of moving) {
      const t = gone[p];
      // Fading late, so a trail of dust stays near what's left
      const opacity = 1 - fade * t ** 2;
      if (opacity <= 0.02) continue;
      const r = p * 4;
      let [ux, uy] = [dirX, dirY];
      if (s.burst) {
        const [ox, oy] = [centerX[p] - midX, centerY[p] - midY];
        const length = Math.hypot(ox, oy) || 1;
        [ux, uy] = [ox / length, oy / length];
      }
      // Slow to leave, then faster, as dust caught by the wind
      const ease = t * t;
      const distance = travel * ease * (0.4 + random[r] * 1.2);
      const side = (random[r + 1] - 0.5) * 2 * scatter * distance;
      const moveX = ux * distance - uy * side;
      const moveY = uy * distance + ux * side + (s.gravity / 100) * travel * ease * t;
      const turn = (((random[r + 2] - 0.5) * 2 * s.spin * t) * Math.PI) / 180;
      const size = Math.max(0.05, 1 - shrink * t * (0.6 + random[r + 3] * 0.8));
      const light = lighten * t;
      const [cos, sin] = [Math.cos(turn), Math.sin(turn)];
      const [cx, cy] = [centerX[p], centerY[p]];
      const [x0, y0, x1, y1] = [box[r], box[r + 1], box[r + 2], box[r + 3]];
      // Its box where it's going
      const half = Math.max(cx - x0, x1 - cx, cy - y0, y1 - cy) * size * 1.42 + 1;
      const [tx, ty] = [cx + moveX + margin, cy + moveY + margin];
      const left = Math.max(0, Math.floor(tx - half));
      const right = Math.min(outWidth - 1, Math.ceil(tx + half));
      const top = Math.max(0, Math.floor(ty - half));
      const bottom = Math.min(outHeight - 1, Math.ceil(ty + half));
      for (let oy = top; oy <= bottom; oy++) {
        for (let ox = left; ox <= right; ox++) {
          // Back through the turn and size to the picture
          const [dx, dy] = [(ox - tx) / size, (oy - ty) / size];
          const sx = Math.round(cx + dx * cos + dy * sin);
          const sy = Math.round(cy - dx * sin + dy * cos);
          if (sx < x0 || sx > x1 || sy < y0 || sy > y1 || piece[sy * width + sx] !== p) continue;
          const from = (sy * width + sx) * 4;
          const a = (data[from + 3] / 255) * opacity;
          if (a <= 0) continue;
          const to = (oy * outWidth + ox) * 4;
          const under = out[to + 3] / 255;
          const total = a + under * (1 - a);
          for (let channel = 0; channel < 3; channel++) {
            const color = data[from + channel] + (255 - data[from + channel]) * light;
            out[to + channel] = (color * a + out[to + channel] * under * (1 - a)) / total;
          }
          out[to + 3] = total * 255;
        }
      }
    }
    // Cropped to what's drawn
    let [cropLeft, cropTop, cropRight, cropBottom] = [outWidth, outHeight, -1, -1];
    for (let y = 0; y < outHeight; y++) {
      for (let x = 0; x < outWidth; x++) {
        if (out[(y * outWidth + x) * 4 + 3] === 0) continue;
        if (x < cropLeft) cropLeft = x;
        if (x > cropRight) cropRight = x;
        if (y < cropTop) cropTop = y;
        if (y > cropBottom) cropBottom = y;
      }
    }
    if (cropRight < 0) return null;
    const cropWidth = cropRight - cropLeft + 1;
    const cropHeight = cropBottom - cropTop + 1;
    const cropped = new ImageData(cropWidth, cropHeight);
    for (let y = 0; y < cropHeight; y++) {
      const from = ((y + cropTop) * outWidth + cropLeft) * 4;
      cropped.data.set(out.subarray(from, from + cropWidth * 4), y * cropWidth * 4);
    }
    return { image: cropped, x: cropLeft - margin, y: cropTop - margin };
  };

  // A picture of `bitmap` at most `most` pixels across, as ImageData, and
  // its pixels per layer pixel given the full one's `k`
  const scaledPixels = (bitmap, most, k) => {
    const ratio = Math.min(1, most / Math.max(bitmap.width, bitmap.height));
    const canvas = Object.assign(document.createElement("canvas"), { width: Math.max(1, Math.round(bitmap.width * ratio)), height: Math.max(1, Math.round(bitmap.height * ratio)) });
    const context = canvas.getContext("2d", { willReadFrequently: true });
    context.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    return { image: context.getImageData(0, 0, canvas.width, canvas.height), k: k * ratio };
  };

  // Pieces made once for each picture, size and seed
  const piecesFor = (current, level, image, cell) => {
    const key = `${level}:${cell.toFixed(2)}:${current.settings.seed}:${current.settings.mode}`;
    current.cracks ??= {};
    if (current.cracks[level]?.key !== key) {
      current.cracks[level] = { key, pieces: crack(image.width, image.height, image.data, cell, current.settings.seed, current.settings.mode === "shatter" ? 0.95 : 1) };
    }
    return current.cracks[level].pieces;
  };

  const renderAt = (current, level) => {
    const { image, k } = current.levels[level];
    const cell = Math.max(1.5, current.settings.piece * k);
    return disintegrate(image, piecesFor(current, level, image, cell), current.settings, k);
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
    if (!drawn || result.removed) {
      if (!result.removed) result.visible = false;
      return;
    }
    const canvas = Object.assign(document.createElement("canvas"), { width: drawn.image.width, height: drawn.image.height });
    canvas.getContext("2d").putImageData(drawn.image, 0, 0);
    const blob = await new Promise((resolve) => canvas.toBlob(resolve, "image/png"));
    const bytes = new Uint8Array(await blob.arrayBuffer());
    if (result.removed) return;
    const hash = figma.createImage(bytes).hash;
    const width = drawn.image.width / k;
    const height = drawn.image.height / k;
    const place = toParent(result.parent, bounds.x + drawn.x / k, bounds.y + drawn.y / k);
    result.resizeWithoutConstraints(Math.max(0.01, width), Math.max(0.01, height));
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
    let settings = { ...DISINTEGRATE_DEFAULTS };
    if (group) {
      source = group.children.find((child) => child.getSharedPluginData(PLUGIN, ROLE) === "source");
      result = group.children.find((child) => child.getSharedPluginData(PLUGIN, ROLE) === "result") ?? null;
      try {
        settings = { ...settings, ...JSON.parse(group.getSharedPluginData(PLUGIN, DISINTEGRATE)) };
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
    const unit = Math.max(1, Math.max(source.width ?? 100, source.height ?? 100));
    if (!group) Object.assign(settings, fit(DISINTEGRATE_PRESETS[0][1](unit)));
    const current = { figma, group, source, settings, unit, result, kept: result ? { fills: JSON.stringify(result.fills), width: result.width, height: result.height, transform: result.relativeTransform } : null, levels: {}, wasVisible: source.visible };
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
      preview: scaledPixels(current.bitmap, 360, k),
      live: scaledPixels(current.bitmap, 900, k),
      full: scaledPixels(current.bitmap, MAX_SIDE, k),
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
      const level = current.levels.live;
      const drawn = renderAt(current, "live");
      await placeDrawn(current, drawn, level.k);
    }, 120);
  };

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
    if (!drawn) return;
    const picture = Object.assign(document.createElement("canvas"), { width: drawn.image.width, height: drawn.image.height });
    picture.getContext("2d").putImageData(drawn.image, 0, 0);
    // Fitted around the layer and what flies off it
    const { width, height } = current.levels.preview.image;
    const [left, top] = [Math.min(0, drawn.x), Math.min(0, drawn.y)];
    const [right, bottom] = [Math.max(width, drawn.x + drawn.image.width), Math.max(height, drawn.y + drawn.image.height)];
    const pad = 16;
    const fit = Math.min((box.width - pad * 2) / (right - left), (box.height - pad * 2) / (bottom - top));
    const ox = (box.width - (right - left) * fit) / 2 - left * fit;
    const oy = (box.height - (bottom - top) * fit) / 2 - top * fit;
    context.imageSmoothingQuality = "high";
    context.drawImage(picture, ox + drawn.x * fit, oy + drawn.y * fit, drawn.image.width * fit, drawn.image.height * fit);
  };

  const applyDisintegrate = async (current) => {
    const { figma, source, settings } = current;
    clearTimeout(current.timer);
    current.busy = true;
    current.panel?.busy(true);
    const level = current.levels.full;
    await placeDrawn(current, renderAt(current, "full"), level.k);
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
    current.root?.remove();
    removeEventListener("keydown", current.keys, true);
  };

  const buildDisintegratePanel = (current) => {
    const s = current.settings;
    const rows = [];
    const change = (key, value) => {
      s[key] = value;
      refreshDisintegrate(current);
      show();
    };
    const slider = makeSlider({ s, unit: current.unit, rows, change, limits: DISINTEGRATE_LIMITS, scrub: { piece: 0.2 }, digits: 1 });
    const canvas = element("canvas", { className: "preview" });
    canvas.setAttribute("aria-hidden", "true");
    current.preview = canvas;

    const presets = makeChips(DISINTEGRATE_PRESETS, (values) => {
      Object.assign(s, fit(values(current.unit)));
      refreshDisintegrate(current);
      show();
    });
    // Picking a mode starts from its first preset, keeping where the front is
    const mode = segments(
      "Mode",
      [
        ["snap", "Snap"],
        ["shatter", "Shatter"],
      ],
      () => s.mode,
      (value) => {
        if (value === s.mode) return;
        const first = DISINTEGRATE_PRESETS.find(([, values]) => values(current.unit).mode === value);
        Object.assign(s, fit(first[1](current.unit)), { progress: s.progress, angle: s.angle });
        refreshDisintegrate(current);
        show();
      },
    );
    const burst = element("input", { type: "checkbox" });
    burst.addEventListener("change", () => change("burst", burst.checked));
    const front = element(
      "section",
      {},
      element("div", { className: "row" }, element("span", { className: "label", textContent: "Mode" }), mode),
      element("div", { className: "rows" }, ...DISINTEGRATE_ROWS.front.map(slider)),
      checkRow("Burst from the middle", burst, "Pieces fly out from the middle, the middle going first"),
    );
    const pieces = element("section", {}, element("div", { className: "caption", textContent: "Pieces" }), element("div", { className: "rows" }, ...DISINTEGRATE_ROWS.pieces.map(slider)));
    const look = element("section", {}, element("div", { className: "caption", textContent: "As they go" }), element("div", { className: "rows" }, ...DISINTEGRATE_ROWS.look.map(slider)));

    const shuffle = element("button", { type: "button", className: "chip", textContent: "Shuffle" });
    shuffle.addEventListener("click", () => change("seed", (s.seed % 9999) + 1));
    const editable = element("input", { type: "checkbox" });
    editable.addEventListener("change", () => change("editable", editable.checked));
    const result = element(
      "section",
      {},
      element("div", { className: "row" }, element("span", { className: "label", textContent: "Pattern" }), shuffle),
      checkRow("Keep editable", editable, "The original kept, hidden, in a group Disintegrate can change later"),
    );
    shuffle.style.gridColumn = "span 2";
    shuffle.style.justifySelf = "start";

    const reset = element("button", { type: "button", className: "link", textContent: "Reset" });
    reset.addEventListener("click", () => {
      const first = DISINTEGRATE_PRESETS.find(([, values]) => values(current.unit).mode === s.mode);
      Object.assign(s, fit(first[1](current.unit)));
      refreshDisintegrate(current);
      show();
    });
    const scroll = element("div", { className: "scroll" }, presets, front, pieces, look, result, element("div", { className: "under" }, reset));

    const name = element("div", { className: "source", textContent: current.source.name });
    const cancel = element("button", { type: "button", className: "button", textContent: "Cancel" });
    cancel.addEventListener("click", () => closeDisintegrate(current, false));
    const go = element("button", { type: "button", className: "button primary", textContent: current.group ? "Update" : "Disintegrate" });
    go.addEventListener("click", () => !current.busy && applyDisintegrate(current));
    const footer = element("div", { className: "footer" }, name, element("div", { className: "actions" }, cancel, go));

    const root = makePanel(current.group ? `Disintegrate · ${current.source.name}` : "Disintegrate", () => closeDisintegrate(current, false), canvas, scroll, footer);
    const show = () => {
      mode.show();
      for (const row of rows) row.show();
      burst.checked = s.burst;
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
