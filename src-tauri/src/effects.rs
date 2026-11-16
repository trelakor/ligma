//! Effects library (Settings > Editor > Toolbar): a button in Figma
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
//!
//! Disintegrate breaks the selected layer into pieces and throws them, as
//! After Effects' Shatter does, and Bitcrush pixelates, posterizes,
//! dithers and glitches it. Dither, Halftone and Modulate are Dither Boy's
//! kinds of dithers, Chromatic Aberration and Fisheye are a lens's,
//! Liquify, Displace, Motion Blur and Glow warp, smear or light it, and
//! Shadow, Reflection and Rim Light are worked out from a cutout's own
//! shape, with no generated pixels. Each draws an image over the original, which is kept,
//! hidden, in a group to change them from
//!
//! Effects stack, and a Ligma Effects section under Figma's own Effects in
//! the right panel lists them to edit, hide, reorder or remove. See
//! docs/EFFECTS.md

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
  // A circle stepped into pixels
  const BITCRUSH_ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><path ${STROKE} d="M9.5 4.5h5v2h2v2h2v7h-2v2h-2v2h-5v-2h-2v-2h-2v-7h2v-2h2z"/><path ${STROKE} d="M10.5 10.5h2v2h-2z"/></svg>`;
  const CHEVRON = `<svg width="16" height="16" fill="none" viewBox="0 0 16 16"><path ${STROKE} d="m5.5 7 2.5 2.5L10.5 7"/></svg>`;
  const CLOSE = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><path ${STROKE} d="m8 8 8 8m0-8-8 8"/></svg>`;
  const SEARCH = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><circle ${STROKE} cx="11" cy="11" r="4.5"/><path ${STROKE} d="m14.5 14.5 3.5 3.5"/></svg>`;
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
    /* The library: a search field, then every effect as a tile, a
       picture of it at work over its name, in Figma's menu colors */
    [${ROOT}="menu"] {
      position: fixed;
      z-index: 9;
      width: 368px;
      max-height: calc(100vh - 16px);
      display: flex;
      flex-direction: column;
      border-radius: var(--radius-large, 13px);
      background: var(--color-bg-menu, #1e1e1e);
      color: var(--color-text-menu, #fff);
      box-shadow: var(--elevation-500);
      overflow: hidden;
    }
    [${ROOT}="menu"] .top { flex: none; padding: 8px 8px 4px; }
    [${ROOT}="menu"] .search {
      display: flex;
      align-items: center;
      height: 32px;
      padding: 0 8px 0 3px;
      border: 1px solid var(--color-border-menu, #383838);
      border-radius: 8px;
      color: var(--color-text-menu-secondary, #ffffffb2);
    }
    [${ROOT}="menu"] .search:focus-within { border-color: var(--color-border-selected, #0d99ff); }
    [${ROOT}="menu"] .search svg { flex: none; }
    [${ROOT}="menu"] .search svg [stroke] { stroke: currentColor; }
    [${ROOT}="menu"] .search input {
      flex: 1;
      min-width: 0;
      height: 100%;
      padding: 0 0 0 2px;
      border: 0;
      background: none;
      outline: 0;
      font: inherit;
      font-size: 13px;
      color: var(--color-text-menu, #fff);
      caret-color: var(--color-border-selected, #0d99ff);
    }
    [${ROOT}="menu"] .search input::placeholder { color: var(--color-text-menu-secondary, #ffffffb2); }
    [${ROOT}="menu"] .search input::-webkit-search-cancel-button { display: none; }
    [${ROOT}="menu"] .body {
      flex: 1;
      min-height: 0;
      padding: 4px 8px 8px;
      overflow-y: auto;
      scrollbar-width: thin;
      scrollbar-color: #ffffff33 transparent;
    }
    [${ROOT}="menu"] .separator { height: 1px; margin: 8px -8px; background: var(--color-border-menu, #383838); }
    [${ROOT}="menu"] .heading { padding: 0 4px; height: 24px; display: flex; align-items: center; color: var(--color-text-menu-secondary, #ffffffb2); }
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
    [${ROOT}="menu"] .item:hover .about, [${ROOT}="menu"] .item:focus-visible .about { color: inherit; }
    [${ROOT}="menu"] .grid { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 4px; }
    [${ROOT}="menu"] .tile {
      display: flex;
      flex-direction: column;
      gap: 6px;
      min-width: 0;
      padding: 4px 4px 6px;
      border: 0;
      border-radius: 8px;
      background: none;
      text-align: center;
      cursor: default;
    }
    [${ROOT}="menu"] .tile[hidden] { display: none; }
    [${ROOT}="menu"] .tile:hover { background: var(--color-bg-menu-hover, #ffffff14); }
    [${ROOT}="menu"] .tile:focus-visible { outline: 2px solid var(--color-border-selected, #0d99ff); outline-offset: -2px; }
    [${ROOT}="menu"] .tile .picture {
      position: relative;
      display: block;
      aspect-ratio: 1;
      border-radius: 6px;
      background: #ffffff0f;
      overflow: hidden;
    }
    [${ROOT}="menu"] .tile .picture::after { content: ""; position: absolute; inset: 0; border-radius: inherit; box-shadow: inset 0 0 0 1px #ffffff1a; pointer-events: none; }
    [${ROOT}="menu"] .tile canvas { display: block; width: 100%; height: 100%; opacity: 0; transition: opacity 180ms cubic-bezier(0.16, 1, 0.3, 1); }
    [${ROOT}="menu"] .tile canvas.drawn { opacity: 1; }
    [${ROOT}="menu"] .tile .name { display: -webkit-box; overflow: hidden; -webkit-box-orient: vertical; -webkit-line-clamp: 2; line-height: 14px; overflow-wrap: anywhere; }
    [${ROOT}="menu"] .empty { padding: 24px 8px; text-align: center; color: var(--color-text-menu-secondary, #ffffffb2); }
    @media (prefers-reduced-motion: reduce) {
      [${ROOT}="menu"] .tile canvas { transition: none; }
    }
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

  // Closes whichever effect is open as Cancel does, so one that failed
  // doesn't keep every effect from opening until the page reloads
  const cancelOpen = () => {
    if (repeating) closeRepeater(repeating, false);
    if (disintegrating) closeDisintegrate(disintegrating, false);
    if (crushing) closeBitcrush(crushing, false);
    if (filtering) closeFilter(filtering, false);
  };
  // An effect's `open`, undone if it throws. Run for a stack, the stack's
  // made again by `restack`, which hears of it and says so itself
  const guarded = (open) => async (figma, edit = false, run = null) => {
    try {
      return await open(figma, edit, run);
    } catch (error) {
      cancelOpen();
      if (run) throw error;
      figma.notify("Couldn't make the effect", { timeout: 2000 });
    }
  };

  // The effects the library lists
  const EFFECTS = [
    {
      id: "repeater",
      name: "Repeater",
      about: "Repeat a layer in a trail, ring or grid",
      icon: REPEATER_ICON,
      of: (node) => repeaterOf(node),
      saved: (group) => JSON.parse(group.getSharedPluginData(PLUGIN, SETTINGS) || "{}"),
      summary: (s) => (s.mode === "grid" ? `Grid · ${s.columns} × ${s.rows}` : `${s.mode === "radial" ? "Radial" : "Trail"} · ${s.copies} ${s.copies === 1 ? "copy" : "copies"}`),
      open: guarded((figma, edit, run) => openRepeater(figma, edit, run)),
      thumb: () => repeaterThumb(),
    },
    {
      id: "disintegrate",
      name: "Disintegrate",
      about: "Snap it into dust, or shatter it like glass",
      icon: DISINTEGRATE_ICON,
      of: (node) => disintegrateOf(node),
      saved: (group) => JSON.parse(group.getSharedPluginData(PLUGIN, DISINTEGRATE) || "{}"),
      summary: (s) => `${s.mode === "snap" ? "Snap" : "Shatter"} · ${s.pieces} pieces`,
      open: guarded((figma, edit, run) => openDisintegrate(figma, edit, run)),
      thumb: () => disintegrateThumb(),
    },
    {
      id: "bitcrush",
      name: "Bitcrush",
      about: "Pixelate, posterize, dither and glitch it",
      icon: BITCRUSH_ICON,
      of: (node) => bitcrushOf(node),
      saved: (group) => JSON.parse(group.getSharedPluginData(PLUGIN, BITCRUSH) || "{}"),
      summary: (s) => `${s.pixelSize}pt · ${s.bitDepth}-bit`,
      open: guarded((figma, edit, run) => openBitcrush(figma, edit, run)),
      thumb: () => bitcrushThumb(),
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

  // A layer's name without the effects' names an effect group starts with,
  // so stacked effects are named after the original rather than each other
  const plainName = (name) => name.replace(new RegExp(`^(?:(?:${EFFECTS.map((effect) => effect.name).join("|")}) · )+`), "");

  // ---------- The toolbar button and the library ----------

  let menu = null;
  const button = () => document.querySelector(`[${BUTTON}]`);
  let menuKeys = null;
  const closeMenu = () => {
    menu?.remove();
    menu = null;
    button()?.setAttribute("aria-expanded", "false");
    removeEventListener("pointerdown", outsideMenu, true);
    if (menuKeys) removeEventListener("keydown", menuKeys, true);
    menuKeys = null;
  };
  const outsideMenu = (event) => {
    if (menu && !menu.contains(event.target) && !button()?.contains(event.target)) closeMenu();
  };
  const COLUMNS = 4;
  const openMenu = (from = null) => {
    const figma = api();
    const anchor = from ?? button();
    if (!figma || !anchor) return;
    // A layer inside an effect's group, its original, copies or result,
    // stands for the group, so the effect it's in can be changed
    {
      const [only] = figma.currentPage.selection;
      const parent = only?.parent;
      // Unless it's an effect's group itself, stacked under another
      if (figma.currentPage.selection.length === 1 && parent && !EFFECTS.some((effect) => effect.of(only)) && EFFECTS.some((effect) => effect.of(parent))) {
        figma.currentPage.selection = [parent];
      }
    }
    // An effect's group selected: the effect to change it first, then the
    // rest, which apply another on top of it
    const [only] = figma.currentPage.selection;
    const applied = figma.currentPage.selection.length === 1 ? EFFECTS.find((effect) => effect.of(only)) : null;
    menu = document.createElement("div");
    menu.setAttribute(ROOT, "menu");
    menu.setAttribute("role", "dialog");
    menu.setAttribute("aria-label", "Effects");
    // Tabs for categories go under the search, in `top`, once there are
    // enough effects to sort
    const top = menu.appendChild(element("div", { className: "top" }));
    const search = top.appendChild(element("label", { className: "search", innerHTML: SEARCH }));
    const input = search.appendChild(element("input", { type: "search", placeholder: "Search effects", spellcheck: false, autocomplete: "off" }));
    input.setAttribute("aria-label", "Search effects");
    const body = menu.appendChild(element("div", { className: "body" }));
    const pick = (run) => () => {
      closeMenu();
      run();
    };
    let editing = null;
    if (applied) {
      editing = body.appendChild(element("div"));
      const item = element("button", { type: "button", className: "item", innerHTML: `${applied.icon}<span class="name">Edit ${applied.name}</span><span class="about">Change the effect already applied</span>` });
      item.addEventListener("click", pick(() => applied.open(figma, true)));
      editing.append(element("div", { className: "heading", textContent: "This layer" }), item, element("div", { className: "separator" }), element("div", { className: "heading", textContent: "Add an effect on top" }));
    }
    const grid = body.appendChild(element("div", { className: "grid" }));
    const empty = body.appendChild(element("div", { className: "empty", textContent: "No effects match", hidden: true }));
    const tiles = EFFECTS.map((effect) => {
      const canvas = element("canvas", { width: THUMB, height: THUMB });
      canvas.setAttribute("aria-hidden", "true");
      const tile = grid.appendChild(element("button", { type: "button", className: "tile", title: effect.about }, element("span", { className: "picture" }, canvas), element("span", { className: "name", textContent: effect.name })));
      tile.addEventListener("click", pick(() => effect.open(figma, false)));
      return { effect, tile, canvas, text: `${effect.name} ${effect.about}`.toLowerCase() };
    });
    const shown = () => tiles.filter(({ tile }) => !tile.hidden).map(({ tile }) => tile);
    const editItem = () => (editing && !editing.hidden ? editing.querySelector(".item") : null);
    input.addEventListener("input", () => {
      const words = input.value.trim().toLowerCase().split(/\s+/).filter(Boolean);
      for (const { tile, text } of tiles) tile.hidden = !words.every((word) => text.includes(word));
      if (editing) editing.hidden = words.length > 0;
      empty.hidden = shown().length > 0;
    });
    // Arrows move through the tiles a row of four at a time, up from the
    // first row to the search, and typing anywhere searches
    menu.addEventListener("keydown", (event) => {
      // Figma's shortcuts don't hear keys typed here
      event.stopPropagation();
      const list = shown();
      if (event.target === input) {
        if (event.key === "ArrowDown") {
          event.preventDefault();
          (editItem() ?? list[0])?.focus();
        } else if (event.key === "Enter") {
          event.preventDefault();
          list[0]?.click();
        }
        return;
      }
      if (event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey && event.key !== " ") {
        input.focus();
        return;
      }
      if (event.target === editItem()) {
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          event.preventDefault();
          (event.key === "ArrowDown" ? list[0] : input)?.focus();
        }
        return;
      }
      const at = list.indexOf(event.target);
      const step = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -COLUMNS, ArrowDown: COLUMNS }[event.key];
      if (at < 0 || !step) return;
      event.preventDefault();
      const next = at + step;
      if (next >= 0) list[Math.min(list.length - 1, next)].focus();
      else if (step === -COLUMNS) (editItem() ?? input).focus();
    });
    menuKeys = (event) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopImmediatePropagation();
      closeMenu();
      anchor.focus();
    };
    addEventListener("keydown", menuKeys, true);
    document.body.append(menu);
    const box = anchor.getBoundingClientRect();
    menu.style.left = `${Math.max(8, Math.min(innerWidth - menu.offsetWidth - 8, box.left + box.width / 2 - menu.offsetWidth / 2))}px`;
    menu.style.top = `${Math.max(8, from ? Math.min(innerHeight - menu.offsetHeight - 8, box.bottom + 4) : box.top - menu.offsetHeight - 12)}px`;
    if (from) menu.style.left = `${Math.max(8, box.right - menu.offsetWidth)}px`;
    // Held at its first height, so searching doesn't move it
    menu.style.height = `${menu.offsetHeight}px`;
    anchor.setAttribute("aria-expanded", "true");
    addEventListener("pointerdown", outsideMenu, true);
    input.focus();
    drawThumbs(tiles);
  };

  // ---------- The library's pictures ----------

  // Each tile shows its effect at work on a picture drawn here, one that
  // shows it best: a sunset's tones for the dithers and Bitcrush, rings
  // for Chromatic Aberration, checks for Fisheye and a pawn on a floor or
  // on water for Shadow and Reflection. A picture effect's `thumb` names
  // its sample, backdrop and settings. Each is made once, the first time
  // the library opens, one a frame so the library's up at once
  const THUMB = 160;
  const thumbs = new Map();
  const canvas2d = (width = THUMB, height = THUMB) => {
    const canvas = Object.assign(document.createElement("canvas"), { width, height });
    return [canvas, canvas.getContext("2d", { willReadFrequently: true })];
  };
  const linear = (g, x0, y0, x1, y1, stops) => {
    const gradient = g.createLinearGradient(x0, y0, x1, y1);
    for (const [at, color] of stops) gradient.addColorStop(at, color);
    return gradient;
  };
  // A chess pawn standing at `cx`, `base`, lit from the left
  const pawn = (g, cx, base) => {
    g.save();
    g.translate(cx, base);
    g.fillStyle = linear(g, -26, 0, 26, 0, [
      [0, "#ffcf7a"],
      [0.45, "#f28a3c"],
      [1, "#b8401f"],
    ]);
    g.beginPath();
    g.moveTo(-26, 0);
    g.lineTo(26, 0);
    g.lineTo(26, -7);
    g.quadraticCurveTo(25, -13, 16, -14);
    g.quadraticCurveTo(8, -30, 8, -50);
    g.lineTo(13, -52);
    g.quadraticCurveTo(14, -56, 10, -57);
    g.lineTo(-10, -57);
    g.quadraticCurveTo(-14, -56, -13, -52);
    g.lineTo(-8, -50);
    g.quadraticCurveTo(-8, -30, -16, -14);
    g.quadraticCurveTo(-25, -13, -26, -7);
    g.closePath();
    g.fill();
    const head = g.createRadialGradient(-5, -75, 2, 0, -70, 15);
    head.addColorStop(0, "#ffe0a3");
    head.addColorStop(0.5, "#f28a3c");
    head.addColorStop(1, "#b8401f");
    g.fillStyle = head;
    g.beginPath();
    g.arc(0, -70, 14, 0, Math.PI * 2);
    g.fill();
    g.restore();
  };
  const SAMPLES = {
    // A sun over hills at dusk: every tone, from the sky's dark to the
    // sun's light
    sunset: (g) => {
      g.fillStyle = linear(g, 0, 0, 0, THUMB, [
        [0, "#1b1f4b"],
        [0.55, "#d1497a"],
        [1, "#ffb36b"],
      ]);
      g.fillRect(0, 0, THUMB, THUMB);
      const sun = g.createRadialGradient(94, 58, 2, 104, 70, 32);
      sun.addColorStop(0, "#fff6d8");
      sun.addColorStop(0.5, "#ffc46b");
      sun.addColorStop(1, "#e2615a");
      g.fillStyle = sun;
      g.beginPath();
      g.arc(104, 70, 30, 0, Math.PI * 2);
      g.fill();
      for (const [color, y0, a, b, y1] of [
        ["#6a2d6e", 104, 84, 118, 96],
        ["#2a1640", 132, 112, 140, 124],
      ]) {
        g.fillStyle = color;
        g.beginPath();
        g.moveTo(0, y0);
        g.bezierCurveTo(40, a, 100, b, THUMB, y1);
        g.lineTo(THUMB, THUMB);
        g.lineTo(0, THUMB);
        g.fill();
      }
    },
    // Light rings on black, where colors split at every edge
    rings: (g) => {
      g.fillStyle = "#101014";
      g.fillRect(0, 0, THUMB, THUMB);
      g.strokeStyle = g.fillStyle = "#ffffff";
      for (const [r, width] of [
        [52, 12],
        [27, 8],
      ]) {
        g.lineWidth = width;
        g.beginPath();
        g.arc(80, 80, r, 0, Math.PI * 2);
        g.stroke();
      }
      g.beginPath();
      g.arc(80, 80, 7, 0, Math.PI * 2);
      g.fill();
    },
    // Checks, which show a lens's bulge best
    checks: (g) => {
      for (let j = 0; j < 8; j++)
        for (let i = 0; i < 8; i++) {
          g.fillStyle = (i + j) % 2 ? "#1e2233" : `hsl(${190 + (i + j) * 12} 75% 62%)`;
          g.fillRect(i * 20, j * 20, 20, 20);
        }
    },
    // Colored bands, which show every way they're pushed
    stripes: (g) => {
      ["#ff5a5f", "#ffb400", "#3ec300", "#00a6ed", "#7b2ff7", "#f4f1ea"].forEach((color, i, all) => {
        g.fillStyle = color;
        g.fillRect(0, (i * THUMB) / all.length, THUMB, THUMB / all.length + 1);
      });
    },
    // Neon tubes on a dark wall
    neon: (g) => {
      g.fillStyle = "#0d0c18";
      g.fillRect(0, 0, THUMB, THUMB);
      g.lineCap = "round";
      g.lineWidth = 5;
      g.strokeStyle = "#ff4fd8";
      g.beginPath();
      g.arc(64, 70, 30, 0, Math.PI * 2);
      g.stroke();
      g.strokeStyle = "#5ee7ff";
      g.beginPath();
      g.moveTo(42, 122);
      g.lineTo(124, 122);
      g.moveTo(112, 40);
      g.lineTo(112, 98);
      g.stroke();
    },
    // City lights at night, which streak as they move
    lights: (g) => {
      g.fillStyle = "#0e1020";
      g.fillRect(0, 0, THUMB, THUMB);
      const colors = ["#ffcf6b", "#ff6b8b", "#6bd8ff", "#fff3d6", "#ff9a4d"];
      for (let i = 0; i < 26; i++) {
        const [x, y, r] = [hash2(i, 1, 3) * 150 + 5, hash2(i, 2, 3) * 130 + 15, 3 + hash2(i, 3, 3) * 6];
        g.fillStyle = colors[i % colors.length];
        g.beginPath();
        g.arc(x, y, r, 0, Math.PI * 2);
        g.fill();
      }
    },
    pawnOnNight: (g) => pawn(g, 80, 126),
    pawnOnFloor: (g) => pawn(g, 54, 118),
    pawnOnWater: (g) => pawn(g, 80, 100),
  };
  const BACKDROPS = {
    floor: (g) => {
      g.fillStyle = linear(g, 0, 0, 0, THUMB, [
        [0, "#ebe6dd"],
        [1, "#cdc5b8"],
      ]);
      g.fillRect(0, 0, THUMB, THUMB);
    },
    night: (g) => {
      const sky = g.createRadialGradient(118, 30, 4, 100, 60, 130);
      sky.addColorStop(0, "#33456e");
      sky.addColorStop(1, "#0b0e1c");
      g.fillStyle = sky;
      g.fillRect(0, 0, THUMB, THUMB);
    },
    // Dusk over water, its edge at the pawn's foot
    water: (g) => {
      g.fillStyle = linear(g, 0, 0, 0, 100, [
        [0, "#f3dcc0"],
        [1, "#e9a983"],
      ]);
      g.fillRect(0, 0, THUMB, 100);
      g.fillStyle = linear(g, 0, 100, 0, THUMB, [
        [0, "#3a6b88"],
        [1, "#16324a"],
      ]);
      g.fillRect(0, 100, THUMB, THUMB - 100);
    },
  };
  const pictureThumb = (spec) => {
    const { sample, backdrop, settings = {} } = spec.thumb;
    const [, g] = canvas2d();
    SAMPLES[sample](g);
    const done = spec.process(g.getImageData(0, 0, THUMB, THUMB), { ...structuredClone(spec.defaults), ...settings }, 1);
    const [result, rg] = canvas2d(done.image.width, done.image.height);
    rg.putImageData(done.image, 0, 0);
    const [tile, tg] = canvas2d();
    BACKDROPS[backdrop]?.(tg);
    tg.drawImage(result, done.x, done.y);
    return tile;
  };
  const bitcrushThumb = () => {
    const [tile, g] = canvas2d();
    SAMPLES.sunset(g);
    g.putImageData(crushRows(g.getImageData(0, 0, THUMB, THUMB), { ...BITCRUSH_DEFAULTS, pixelSize: 7 }, 1, 0, THUMB), 0, 0);
    return tile;
  };
  // A card stepped along a curve, turning and shrinking as it goes
  const repeaterThumb = () => {
    const [tile, g] = canvas2d();
    g.fillStyle = linear(g, 0, 0, 0, THUMB, [
      [0, "#eeebfa"],
      [1, "#dad4f2"],
    ]);
    g.fillRect(0, 0, THUMB, THUMB);
    for (let i = 0; i < 6; i++) {
      const t = i / 5;
      const size = 44 - 16 * t;
      g.save();
      g.translate(36 + 88 * t, 46 + 70 * t - 30 * Math.sin(Math.PI * t));
      g.rotate((i * 16 * Math.PI) / 180);
      g.globalAlpha = 0.3 + 0.7 * t;
      g.fillStyle = "#6457f0";
      g.strokeStyle = "#ffffff";
      g.lineWidth = 2;
      g.beginPath();
      g.roundRect(-size / 2, -size / 2, size, size, size * 0.2);
      g.fill();
      g.stroke();
      g.restore();
    }
    return tile;
  };
  // A square whose right side has broken into flakes, blown up and away
  const disintegrateThumb = () => {
    const [tile, g] = canvas2d();
    g.fillStyle = "#15171d";
    g.fillRect(0, 0, THUMB, THUMB);
    const [x0, y0, cell, n] = [24, 46, 4, 17];
    for (let j = 0; j < n; j++)
      for (let i = 0; i < n; i++) {
        const [u, v] = [i / (n - 1), j / (n - 1)];
        const [r, r2] = [hash2(i, j, 5), hash2(i, j, 9)];
        g.fillStyle = `hsl(${18 + 30 * v - 10 * u} 90% ${60 - 10 * v}%)`;
        if (u + (r - 0.5) * 0.5 <= 0.55) {
          g.globalAlpha = 1;
          g.fillRect(x0 + i * cell, y0 + j * cell, cell, cell);
          continue;
        }
        const t = clamp01(((u - 0.3) / 0.7) * (0.4 + r2 * 0.9));
        const size = cell * (1 - 0.5 * t);
        g.globalAlpha = 1 - 0.75 * t;
        g.fillRect(x0 + i * cell + 6 + 72 * t + r * 8, y0 + j * cell - 36 * t * (0.5 + r) + (r2 - 0.5) * 8, size, size);
      }
    return tile;
  };
  const drawThumbs = (tiles) => {
    const paint = ({ canvas }, picture) => {
      canvas.getContext("2d").drawImage(picture, 0, 0, THUMB, THUMB);
      canvas.classList.add("drawn");
    };
    const waiting = [];
    for (const entry of tiles) {
      const picture = thumbs.get(entry.effect.id);
      if (picture) paint(entry, picture);
      else if (entry.effect.thumb) waiting.push(entry);
    }
    const next = () => {
      const entry = waiting.shift();
      if (!entry || !entry.canvas.isConnected) return;
      try {
        const picture = entry.effect.thumb();
        thumbs.set(entry.effect.id, picture);
        paint(entry, picture);
      } catch {}
      requestAnimationFrame(next);
    };
    requestAnimationFrame(next);
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
      made.setAttribute("aria-haspopup", "dialog");
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
    const places = shape.digits ?? digits;
    const round = (number) => (places ? Math.round(number * 10 ** places) / 10 ** places : Math.round(number));
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
    // past the screen's edge; a click without a drag types in it. Where
    // the pointer can't be locked, as in apps' web views on macOS, it's
    // captured instead, so the drag goes on outside the window, up to the
    // screen's edge
    value.addEventListener("pointerdown", (event) => {
      if (event.button !== 0 || document.activeElement === value) return;
      event.preventDefault();
      try {
        value.setPointerCapture(event.pointerId);
      } catch {}
      const start = s[key];
      let moved = 0;
      let scrubbing = false;
      const rate = scrub[key] ?? 1;
      const move = (event) => {
        moved += event.movementX;
        if (!scrubbing && Math.abs(moved) < 3) return;
        if (!scrubbing) {
          scrubbing = true;
          try {
            value.requestPointerLock?.()?.catch?.(() => {});
          } catch {}
        }
        change(key, clamp(start + moved * rate * (event.shiftKey ? 10 : 1)), fresh(key));
      };
      const up = (event) => {
        removeEventListener("pointermove", move, true);
        removeEventListener("pointerup", up, true);
        removeEventListener("pointercancel", up, true);
        try {
          if (value.hasPointerCapture(event.pointerId)) value.releasePointerCapture(event.pointerId);
        } catch {}
        if (document.pointerLockElement === value) document.exitPointerLock();
        if (!scrubbing && event.type === "pointerup") {
          value.focus();
          value.select();
        }
      };
      addEventListener("pointermove", move, true);
      addEventListener("pointerup", up, true);
      addEventListener("pointercancel", up, true);
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

  const openRepeater = (figma, edit = false, run = null) => {
    if (repeating || disintegrating || crushing || filtering) return;
    const selected = figma.currentPage.selection;
    if (selected.length !== 1) {
      figma.notify(selected.length ? "Select one layer to repeat" : "Select a layer to repeat", { timeout: 2000 });
      return;
    }
    let group = edit ? repeaterOf(selected[0]) : null;
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
    if (run) Object.assign(settings, structuredClone(run), { editable: true });
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
        current.wrap.name = `Repeater · ${plainName(source.name)}`;
        figma.currentPage.selection = [current.wrap];
      } catch {
        current.wrap = null;
      }
    }
    if (run) {
      rebuild(current, true);
      apply(current, true);
      return;
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

  // `quiet` when it's made for a stack, where `restack` says if it failed
  const apply = (current, quiet = false) => {
    try {
      placeRepeater(current, quiet);
    } catch (error) {
      if (repeating === current) closeRepeater(current, false);
      if (quiet) throw error;
      current.figma.notify("Couldn't make the copies", { timeout: 2000 });
    }
  };

  const placeRepeater = (current, quiet) => {
    const { figma, source, settings } = current;
    for (const node of current.kept) if (!node.removed) node.remove();
    let result = current.group ?? current.wrap;
    if (settings.editable) {
      const parent = source.parent;
      const nodes = [source, ...current.clones];
      if (!result || result.removed) {
        result = figma.group(nodes, parent, parent.children.indexOf(source));
        result.name = `Repeater · ${plainName(source.name)}`;
      } else {
        for (const clone of current.clones) if (clone.parent !== result) result.appendChild(clone);
      }
      source.setSharedPluginData(PLUGIN, ROLE, "source");
      result.setSharedPluginData(PLUGIN, SETTINGS, JSON.stringify(settings));
      // Shown as it's made, so one that was hidden is shown again
      result.setSharedPluginData(PLUGIN, OFF, "");
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
    if (current.group && settings.editable) restackAbove(figma, current.group);
    if (!quiet) figma.notify(`${count} ${count === 1 ? "copy" : "copies"} made`, { timeout: 1500 });
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
      // One that was hidden stays hidden
      try {
        if (current.group && !current.group.removed && isOff(current.group)) setOff(current.group, true);
      } catch {}
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

    const root = makePanel(current.group ? `Repeater · ${plainName(current.source.name)}` : "Repeater", () => closeRepeater(current, false), canvas, scroll, footer);

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

  // Breaks the layer into pieces and throws them, as After Effects'
  // Shatter does (see `simulate`), Snap the same with a wipe deciding when
  // each piece goes, as a Thanos snap is made in AE. Pieces are polygons,
  // cut from the layer's picture with smooth, straight edges, drawn here
  // in the page and put back as an image, the original kept under it,
  // hidden
  const DISINTEGRATE = "disintegrate";
  const MAX_SIDE = 4096;
  // The quick picture, and the one put in the file while it's changed, at
  // most this many pixels across
  const QUICK = 720;

  // A force, as AE's Force 1 and Force 2: where it is on the layer, in
  // percent, how far in front of it (Depth) and how large a sphere
  // (Radius), in percent of the layer's larger side, and how hard it
  // throws what's in it (Strength, negative pulls in)
  const force = (x, y, depth, radius, strength, on = true) => ({ x, y, depth, radius, strength, on });
  const DISINTEGRATE_DEFAULTS = {
    mode: "shatter",
    pattern: "glass",
    pieces: 400,
    seed: 1,
    force1: force(50, 50, 10, 40, 5),
    force2: force(25, 30, 10, 25, 5, false),
    time: 1,
    // Physics, as AE's
    rotation: 0.2,
    tumble: "free",
    randomness: 0.1,
    viscosity: 0.1,
    mass: 30,
    gravity: 3,
    gravityDirection: 90,
    // Snap: when each piece goes, as a gradient wipe out from the forces
    spread: 1.5,
    ragged: 55,
    softness: 30,
    fade: 80,
    shrink: 30,
    lighten: 30,
    editable: true,
  };
  const DISINTEGRATE_PRESETS = [
    // Matched against Thanos snaps made with AE's Shatter: a figure's side
    // dissolving into dust that lifts away, and a wipe across the top of
    // a phone mockup, sweeping off to the right
    ["Dissolve", () => ({ mode: "snap", pattern: "glass", pieces: 1800, force1: force(66, 36, 0, 23, 0.5), force2: force(64, 72, 0, 14, 0.4), time: 1, rotation: 1, tumble: "free", randomness: 0.5, viscosity: 0.4, mass: 50, gravity: 9, gravityDirection: -80, spread: 1.6, ragged: 40, softness: 12, fade: 15, shrink: 5, lighten: 0 })],
    ["Wipe", () => ({ mode: "snap", pattern: "cells", pieces: 7000, force1: force(32, 0, 0, 33, 0.4), force2: force(76, 14, 0, 35, 0.4), time: 1.25, rotation: 0.4, tumble: "free", randomness: 0.75, viscosity: 0.4, mass: 50, gravity: 9, gravityDirection: 15, spread: 1.3, ragged: 50, softness: 20, fade: 0, shrink: 0, lighten: 0 })],
    ["Snap", () => ({ mode: "snap", pattern: "glass", pieces: 6000, force1: force(100, 50, 0, 110, 0.3), force2: force(25, 30, 10, 25, 5, false), time: 1, rotation: 1, tumble: "free", randomness: 0.5, viscosity: 0.4, mass: 50, gravity: 5, gravityDirection: -15, spread: 1.8, ragged: 60, softness: 30, fade: 75, shrink: 30, lighten: 30 })],
    ["Dust", () => ({ mode: "snap", pattern: "glass", pieces: 12000, force1: force(80, 35, 0, 120, 0.6), force2: force(25, 70, 0, 60, 0.4), time: 1.4, rotation: 1.5, tumble: "free", randomness: 0.8, viscosity: 0.6, mass: 60, gravity: 3, gravityDirection: -40, spread: 1.8, ragged: 75, softness: 45, fade: 90, shrink: 50, lighten: 50 })],
    ["Glass", () => ({ mode: "shatter", pattern: "glass", pieces: 400, force1: force(50, 50, 10, 40, 5), force2: force(25, 30, 10, 25, 5, false), time: 1, rotation: 0.2, tumble: "free", randomness: 0.1, viscosity: 0.1, mass: 30, gravity: 3, gravityDirection: 90 })],
    ["Corner", () => ({ mode: "shatter", pattern: "glass", pieces: 200, force1: force(88, 22, 10, 50, 4), force2: force(25, 30, 10, 25, 5, false), time: 0.5, rotation: 0.2, tumble: "free", randomness: 0.3, viscosity: 0.1, mass: 30, gravity: 0, gravityDirection: 90 })],
    ["Crack", () => ({ mode: "shatter", pattern: "glass", pieces: 500, force1: force(55, 45, 10, 35, 1.5), force2: force(25, 30, 10, 25, 5, false), time: 0.4, rotation: 0.1, tumble: "free", randomness: 0.2, viscosity: 0.3, mass: 30, gravity: 1, gravityDirection: 90 })],
    ["Explode", () => ({ mode: "shatter", pattern: "glass", pieces: 600, force1: force(50, 50, 5, 90, 12), force2: force(25, 30, 10, 25, 5, false), time: 1, rotation: 0.6, tumble: "free", randomness: 0.3, viscosity: 0.05, mass: 30, gravity: 3, gravityDirection: 90 })],
    ["Bricks", () => ({ mode: "shatter", pattern: "bricks", pieces: 120, force1: force(50, 60, 10, 50, 4), force2: force(25, 30, 10, 25, 5, false), time: 1, rotation: 0.2, tumble: "free", randomness: 0.1, viscosity: 0.1, mass: 30, gravity: 6, gravityDirection: 90 })],
  ];
  const PATTERNS = [
    ["glass", "Glass"],
    ["cells", "Cells"],
    ["squares", "Squares"],
    ["triangles", "Triangles"],
    ["bricks", "Bricks"],
    ["planks", "Planks"],
    ["hexagons", "Hexagons"],
  ];
  const TUMBLES = [
    ["free", "Free"],
    ["spin", "Spin only"],
    ["none", "None"],
  ];
  // Rows, each [key, label, unit, range]; a force's keys are "force1.x"
  // and so on
  const forceRows = (n) => [
    [`force${n}.x`, "Position X", "%", () => ({ min: 0, max: 100 })],
    [`force${n}.y`, "Position Y", "%", () => ({ min: 0, max: 100 })],
    [`force${n}.depth`, "Depth", "%", () => ({ min: 0, max: 100, shape: "curve" })],
    [`force${n}.radius`, "Radius", "%", () => ({ min: 0, max: 150, shape: "curve" })],
    [`force${n}.strength`, "Strength", "", () => ({ min: -20, max: 20, shape: "curve", digits: 1 })],
  ];
  const DISINTEGRATE_ROWS = {
    shape: [["pieces", "Pieces", "", () => ({ min: 10, max: 20000, shape: "curve" })]],
    time: [["time", "Time", "s", () => ({ min: 0, max: 5, digits: 2 })]],
    physics: [
      ["rotation", "Rotation speed", "", () => ({ min: 0, max: 5, shape: "curve", digits: 2 })],
      ["randomness", "Randomness", "", () => ({ min: 0, max: 1, digits: 2 })],
      ["viscosity", "Viscosity", "", () => ({ min: 0, max: 1, digits: 2 })],
      ["mass", "Mass variance", "%", () => ({ min: 0, max: 100 })],
      ["gravity", "Gravity", "", () => ({ min: 0, max: 20, shape: "curve", digits: 1 })],
      ["gravityDirection", "Gravity direction", "°", () => ({ min: -180, max: 180 })],
    ],
    snap: [
      ["spread", "Spread time", "s", () => ({ min: 0.05, max: 5, digits: 2 })],
      ["ragged", "Raggedness", "%", () => ({ min: 0, max: 100 })],
      ["softness", "Edge softness", "%", () => ({ min: 0, max: 100 })],
      ["fade", "Fade", "%", () => ({ min: 0, max: 100 })],
      ["shrink", "Shrink", "%", () => ({ min: 0, max: 100 })],
      ["lighten", "Lighten", "%", () => ({ min: 0, max: 100 })],
    ],
  };
  const forceLimits = (n) => ({
    [`force${n}.x`]: [-100, 200],
    [`force${n}.y`]: [-100, 200],
    [`force${n}.depth`]: [0, 400],
    [`force${n}.radius`]: [0, 400],
    [`force${n}.strength`]: [-100, 100],
  });
  const DISINTEGRATE_LIMITS = {
    pieces: [2, 40000],
    time: [0, 30],
    rotation: [0, 50],
    randomness: [0, 1],
    viscosity: [0, 1],
    mass: [0, 100],
    gravity: [0, 200],
    spread: [0.05, 30],
    ragged: [0, 100],
    softness: [0, 100],
    fade: [0, 100],
    shrink: [0, 100],
    lighten: [0, 100],
    ...forceLimits(1),
    ...forceLimits(2),
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
  // Smooth noise, 0 to 1, for a ragged wipe
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
  // `count` of them over `covered`, the share of it that isn't
  // transparent: polygons, each with its center and randomness
  const cutPieces = (width, height, pattern, count, seed, covered = 1) => {
    // Cells splits each in two or three, so it starts with fewer
    const cells = pattern === "cells" ? count / 2.2 : pattern === "triangles" ? count / 2 : count;
    const cell = Math.max(1.5, Math.sqrt((width * height * covered) / Math.max(1, cells)));
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
      if (Math.abs(area) < 0.05) return;
      const i = pieces.length;
      pieces.push({ polygon, x: cx / (3 * area), y: cy / (3 * area), random: [0, 1, 2, 3, 4, 5].map((k) => hash(i, k, seed + 101)) });
    };
    const across = Math.ceil(width / cell);
    const down = Math.ceil(height / cell);
    if (pattern === "squares" || pattern === "triangles") {
      for (let gy = 0; gy < down; gy++) {
        for (let gx = 0; gx < across; gx++) {
          const [x0, y0, x1, y1] = [gx * cell, gy * cell, (gx + 1) * cell, (gy + 1) * cell];
          if (pattern === "squares") add([[x0, y0], [x1, y0], [x1, y1], [x0, y1]]);
          else if ((gx + gy) % 2) {
            add([[x0, y0], [x1, y0], [x1, y1]]);
            add([[x0, y0], [x1, y1], [x0, y1]]);
          } else {
            add([[x0, y0], [x1, y0], [x0, y1]]);
            add([[x1, y0], [x1, y1], [x0, y1]]);
          }
        }
      }
    } else if (pattern === "bricks" || pattern === "planks") {
      // Bricks offset by half each row; planks long, offset anywhere
      const [w, h] = pattern === "bricks" ? [cell * 1.6, cell * 0.62] : [cell * 4, cell * 0.5];
      for (let row = 0; row * h < height; row++) {
        const shift = pattern === "bricks" ? (row % 2 ? w / 2 : 0) : hash(row, 0, seed) * w;
        for (let x = -shift; x < width; x += w) add([[x, row * h], [x + w, row * h], [x + w, (row + 1) * h], [x, (row + 1) * h]]);
      }
    } else if (pattern === "hexagons") {
      const r = cell / Math.sqrt(1.5 * Math.sqrt(3));
      const [dx, dy] = [Math.sqrt(3) * r, 1.5 * r];
      for (let row = 0; row * dy - r < height; row++) {
        for (let column = 0; column * dx - dx < width; column++) {
          const [cx, cy] = [column * dx + (row % 2 ? dx / 2 : 0), row * dy];
          add([0, 1, 2, 3, 4, 5].map((k) => [cx + r * Math.cos(((60 * k - 30) * Math.PI) / 180), cy + r * Math.sin(((60 * k - 30) * Math.PI) / 180)]));
        }
      }
    } else if (pattern === "glass") {
      // Glass, as AE's: the layer split again and again by straight lines
      // at random angles, the largest piece each time, so cracks run on
      // across pieces and they come out as the long, sharp slivers broken
      // glass has, until there are as many as asked for over what isn't
      // transparent
      const target = Math.max(1, Math.round(count / covered));
      // A heap, the largest on top
      const parts = [];
      const push = (part) => {
        let i = parts.push(part) - 1;
        while (i > 0) {
          const up = (i - 1) >> 1;
          if (parts[up].area >= parts[i].area) break;
          [parts[up], parts[i]] = [parts[i], parts[up]];
          i = up;
        }
      };
      const pop = () => {
        const top = parts[0];
        const last = parts.pop();
        if (parts.length) {
          parts[0] = last;
          let i = 0;
          for (;;) {
            const [l, r] = [i * 2 + 1, i * 2 + 2];
            let big = i;
            if (l < parts.length && parts[l].area > parts[big].area) big = l;
            if (r < parts.length && parts[r].area > parts[big].area) big = r;
            if (big === i) break;
            [parts[big], parts[i]] = [parts[i], parts[big]];
            i = big;
          }
        }
        return top;
      };
      push({ polygon: [[0, 0], [width, 0], [width, height], [0, height]], area: width * height });
      let n = 0;
      while (parts.length < target && n < target * 4) {
        n++;
        const part = pop();
        let [mx, my] = [0, 0];
        for (const [x, y] of part.polygon) [mx, my] = [mx + x / part.polygon.length, my + y / part.polygon.length];
        const spread = Math.sqrt(part.area) * 0.35;
        const point = [mx + (hash(n, 1, seed) - 0.5) * spread, my + (hash(n, 2, seed) - 0.5) * spread];
        const angle = hash(n, 3, seed) * Math.PI;
        const normal = [Math.cos(angle), Math.sin(angle)];
        for (const side of [normal, [-normal[0], -normal[1]]]) {
          const polygon = clipPolygon(part.polygon, point, side);
          if (polygon.length < 3) continue;
          let area = 0;
          for (let i = 0; i < polygon.length; i++) {
            const [a, b] = [polygon[i], polygon[(i + 1) % polygon.length]];
            area += a[0] * b[1] - b[0] * a[1];
          }
          push({ polygon, area: Math.abs(area) / 2 });
        }
      }
      for (const part of parts) add(part.polygon);
    } else {
      // Cells: irregular cells around seeds anywhere in each
      // square of a grid, each cut again by a line or two at random, into
      // the sharp slivers broken glass has
      const gw = across + 4;
      const gh = down + 4;
      const seedAt = (gx, gy) => [(gx - 2 + hash(gx, gy, seed)) * cell, (gy - 2 + hash(gx, gy, seed + 7)) * cell];
      for (let gy = 2; gy < gh - 2; gy++) {
        for (let gx = 2; gx < gw - 2; gx++) {
          const [sx, sy] = seedAt(gx, gy);
          let polygon = [
            [sx - cell * 3, sy - cell * 3],
            [sx + cell * 3, sy - cell * 3],
            [sx + cell * 3, sy + cell * 3],
            [sx - cell * 3, sy + cell * 3],
          ];
          for (let dy = -2; dy <= 2 && polygon.length >= 3; dy++) {
            for (let dx = -2; dx <= 2 && polygon.length >= 3; dx++) {
              if (!dx && !dy) continue;
              const [ox, oy] = seedAt(gx + dx, gy + dy);
              polygon = clipPolygon(polygon, [(sx + ox) / 2, (sy + oy) / 2], [ox - sx, oy - sy]);
            }
          }
          if (polygon.length < 3) continue;
          // Cut by lines through a point near its middle, at random angles
          const parts = [polygon];
          const cuts = 1 + (hash(gx, gy, seed + 13) < 0.45 ? 1 : 0);
          for (let c = 0; c < cuts; c++) {
            const at = parts.length - 1 - Math.floor(hash(gx, gy, seed + 17 + c) * parts.length * 0.99);
            const part = parts.splice(at, 1)[0];
            let [mx, my] = [0, 0];
            for (const [x, y] of part) [mx, my] = [mx + x / part.length, my + y / part.length];
            const shift = (hash(gx, gy, seed + 19 + c) - 0.5) * cell * 0.5;
            const angle = hash(gx, gy, seed + 23 + c) * Math.PI;
            const normal = [Math.cos(angle), Math.sin(angle)];
            const point = [mx + normal[0] * shift, my + normal[1] * shift];
            const one = clipPolygon(part, point, normal);
            const two = clipPolygon(part, point, [-normal[0], -normal[1]]);
            for (const piece of [one, two]) if (piece.length >= 3) parts.push(piece);
          }
          for (const part of parts) add(part);
        }
      }
    }
    return pieces;
  };

  // Where each piece is at the settings' time, as AE's Shatter works it
  // out: a 2D transform of the piece, its depth and its look, or null
  // where it's still whole. The layer is lit in 3D: a force is a sphere
  // its Depth in front of the layer, and the pieces in it fly away from
  // its middle, faster the nearer they are to it, so those in the middle
  // mostly fly toward the camera, growing and tumbling, rather than out
  // to the sides. Viscosity slows them, gravity pulls them, each turns
  // about its own random axis, and the camera, as far as the layer is
  // across, gives the depth. With Snap, a ragged wipe spreads from each
  // force over Spread time, and each piece only starts once it's reached
  const simulate = (pieces, s, width, height, size) => {
    const forces = [s.force1, s.force2]
      .filter((f) => f.on && f.radius > 0)
      .map((f) => ({ x: (f.x / 100) * width, y: (f.y / 100) * height, z: -(f.depth / 100) * size, radius: (f.radius / 100) * size, strength: f.strength }));
    const moves = new Array(pieces.length).fill(null);
    if (!forces.length || s.time <= 0) return moves;
    // AE's units: speed and gravity in tenths of the layer's size a second
    const unit = size * 0.1;
    const drag = s.viscosity * 4;
    const ga = (s.gravityDirection * Math.PI) / 180;
    const [gx, gy] = [Math.cos(ga) * s.gravity * unit, Math.sin(ga) * s.gravity * unit];
    const focal = size * 2.2;
    const [cx, cy] = [width / 2, height / 2];
    const snap = s.mode === "snap";
    const soft = 0.03 + (s.softness / 100) * 0.6;
    const rag = s.ragged / 100;
    const scale = size / 8;
    pieces.forEach((piece, i) => {
      // The velocity the forces give it, and how far into each it is
      let [vx, vy, vz] = [0, 0, 0];
      let within = 1;
      for (const f of forces) {
        const [dx, dy, dz] = [piece.x - f.x, piece.y - f.y, -f.z];
        const d = Math.hypot(dx, dy, dz);
        if (d >= f.radius) continue;
        const power = f.strength * unit * (1 - d / f.radius);
        const n = d > 1e-6 ? 1 / d : 0;
        vx += dx * n * power;
        vy += dy * n * power;
        vz += (d > 1e-6 ? dz * n : 1) * power;
        within = Math.min(within, Math.hypot(dx, dy) / f.radius);
      }
      if (within >= 1) return;
      const [r0, r1, r2, r3, r4, r5] = piece.random;
      // When it breaks: at once, or with Snap once the wipe reaches it
      let start = 0;
      if (snap) {
        const rough = rag * (noise(piece.x / scale, piece.y / scale, s.seed) * 0.7 + noise((piece.x * 3) / scale, (piece.y * 3) / scale, s.seed + 3) * 0.3);
        start = (within * (1 - rag * 0.6) + rough * 0.6 + (r5 - 0.5) * soft) * s.spread;
      }
      const t = s.time - start;
      if (t <= 0) return;
      // Randomness turns and speeds each piece differently, and heavier
      // ones are slower
      const random = s.randomness;
      const speed = (1 + (r0 - 0.5) * 2 * random) / (1 + ((r1 - 0.5) * 2 * s.mass) / 100);
      const turn = (r2 - 0.5) * 2 * random * 1.5;
      const [cos, sin] = [Math.cos(turn), Math.sin(turn)];
      let [ux, uy, uz] = [(vx * cos - vy * sin) * speed, (vx * sin + vy * cos) * speed, vz * speed];
      // And a velocity of its own in any direction, as AE's Randomness
      // gives, in 3D, so with enough of it some pieces go against the
      // force and gravity, back over what's still whole or toward the
      // camera, and the cloud scatters every way rather than as one sheet
      {
        const along = hash(i, 7, s.seed + 31) * Math.PI * 2;
        const up = hash(i, 8, s.seed + 31) * 2 - 1;
        const across = Math.sqrt(1 - up * up);
        const own = random * unit * 2 * (0.25 + hash(i, 9, s.seed + 31));
        ux += Math.cos(along) * across * own;
        uy += Math.sin(along) * across * own;
        uz += up * own * 0.6;
      }
      // Moved by its speed, slowed by viscosity, and by gravity
      const travel = drag > 0 ? (1 - Math.exp(-drag * t)) / drag : t;
      const fall = (t * t) / 2;
      const [mx, my, mz] = [ux * travel + gx * fall, uy * travel + gy * fall, uz * travel];
      // Seen from the camera: nearer is larger, and farther from the middle
      if (focal - mz <= focal * 0.05) return;
      const depth = focal / (focal - mz);
      const [px, py] = [cx + (piece.x + mx - cx) * depth, cy + (piece.y + my - cy) * depth];
      // Tumbling about a random axis: as it turns, it thins along it
      const spin = (r3 - 0.5) * 2 * Math.PI * 2 * s.rotation * t * (0.5 + r4);
      const tilt = s.tumble === "free" ? Math.PI * 2 * s.rotation * t * (0.5 + r5) : 0;
      const roll = s.tumble === "none" ? 0 : spin;
      const axis = r1 * Math.PI;
      // Never quite edge-on, as AE's pieces have thickness that shows
      const flat = Math.sign(Math.cos(tilt) || 1) * Math.max(0.55, Math.abs(Math.cos(tilt)));
      // R(roll) · R(axis) · diag(1, flat) · R(-axis), sized by depth and shrink
      const life = snap ? Math.min(1, t / Math.max(0.2, s.spread * 0.5)) : 0;
      const size2 = depth * (snap ? Math.max(0.05, 1 - (s.shrink / 100) * life) : 1);
      const [ca, sa] = [Math.cos(axis), Math.sin(axis)];
      const m = [ca * ca + flat * sa * sa, ca * sa - flat * ca * sa, ca * sa - flat * sa * ca, sa * sa + flat * ca * ca];
      const [cr, sr] = [Math.cos(roll), Math.sin(roll)];
      const a = (cr * m[0] - sr * m[2]) * size2;
      const b = (sr * m[0] + cr * m[2]) * size2;
      const c = (cr * m[1] - sr * m[3]) * size2;
      const d = (sr * m[1] + cr * m[3]) * size2;
      moves[i] = {
        // Maps the piece's own points to where they're drawn
        matrix: [a, b, c, d, px - (a * piece.x + c * piece.y), py - (b * piece.x + d * piece.y)],
        depth: mz,
        opacity: snap ? 1 - (s.fade / 100) * life * life : 1,
        light: snap ? (s.lighten / 100) * life : 0,
      };
    });
    return moves;
  };

  // Pieces cut out of the picture in one go, as one path
  const CUT_BATCH = 256;

  // Draws the broken layer from `picture`, a canvas of it: `order`, the
  // pieces that moved, each drawn by `draw`, and `result`, a canvas
  // cropped to what's drawn, and where it sits relative to the picture's
  // top left, in its pixels
  const brokenFrame = (picture, pieces, s) => {
    const { width, height } = picture;
    const moves = simulate(pieces, s, width, height, Math.max(width, height));
    // The bounds of everything drawn
    let [left, top, right, bottom] = [0, 0, width, height];
    const drawn = [];
    pieces.forEach((piece, i) => {
      const move = moves[i];
      if (!move || move.opacity <= 0.02) return;
      const [a, b, c, d, e, f] = move.matrix;
      for (const [x, y] of piece.polygon) {
        const [px, py] = [a * x + c * y + e, b * x + d * y + f];
        if (px < left) left = px;
        if (px > right) right = px;
        if (py < top) top = py;
        if (py > bottom) bottom = py;
      }
      drawn.push(i);
    });
    // Kept to a sensible size, as pieces flying past the camera can be huge
    const most = Math.max(width, height) * 1.5;
    [left, top] = [Math.max(-most, Math.floor(left) - 1), Math.max(-most, Math.floor(top) - 1)];
    [right, bottom] = [Math.min(width + most, Math.ceil(right) + 1), Math.min(height + most, Math.ceil(bottom) + 1)];
    // Drawn no larger than Figma keeps an image, smaller if need be
    const shrink = Math.min(1, MAX_SIDE / Math.max(right - left, bottom - top));
    const canvas = Object.assign(document.createElement("canvas"), { width: Math.max(1, Math.round((right - left) * shrink)), height: Math.max(1, Math.round((bottom - top) * shrink)) });
    const context = canvas.getContext("2d");
    // The picture, with the pieces that moved cut out of it
    context.setTransform(shrink, 0, 0, shrink, -left * shrink, -top * shrink);
    context.drawImage(picture, 0, 0);
    context.globalCompositeOperation = "destination-out";
    const moved = pieces.filter((piece, i) => moves[i]);
    const trace = (target, list) => {
      target.beginPath();
      for (const piece of list) {
        piece.polygon.forEach(([x, y], j) => (j ? target.lineTo(x, y) : target.moveTo(x, y)));
        target.closePath();
      }
    };
    if (moved.length <= CUT_BATCH * 4) {
      trace(context, moved);
      context.fill();
    } else {
      // One path of thousands of pieces is slow to fill, its cost growing
      // with the square of their number, so they're filled in batches on
      // a mask over the picture, adding up. As pieces don't overlap, the
      // sum along the edges they share is what one path would give
      const [ox, oy] = [-left * shrink, -top * shrink];
      const [ix, iy] = [Math.floor(ox), Math.floor(oy)];
      const mask = Object.assign(document.createElement("canvas"), { width: Math.ceil(width * shrink) + 1, height: Math.ceil(height * shrink) + 1 });
      const ink = mask.getContext("2d");
      ink.setTransform(shrink, 0, 0, shrink, ox - ix, oy - iy);
      ink.globalCompositeOperation = "lighter";
      for (let at = 0; at < moved.length; at += CUT_BATCH) {
        trace(ink, moved.slice(at, at + CUT_BATCH));
        ink.fill();
      }
      context.setTransform(1, 0, 0, 1, 0, 0);
      context.drawImage(mask, ix, iy);
      mask.width = mask.height = 0;
      context.setTransform(shrink, 0, 0, shrink, -left * shrink, -top * shrink);
    }
    context.globalCompositeOperation = "source-over";
    // Each piece where it's gone, the farthest from the camera first
    drawn.sort((a, b) => moves[a].depth - moves[b].depth);
    const draw = (i) => {
      const piece = pieces[i];
      const move = moves[i];
      context.save();
      context.setTransform(shrink, 0, 0, shrink, -left * shrink, -top * shrink);
      context.transform(...move.matrix);
      context.beginPath();
      piece.polygon.forEach(([x, y], j) => (j ? context.lineTo(x, y) : context.moveTo(x, y)));
      context.closePath();
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
    };
    return { order: drawn, draw, result: { canvas, x: left, y: top, width: right - left, height: bottom - top } };
  };

  // For the panel and the file while it's changed
  const drawBroken = (picture, pieces, s) => {
    const frame = brokenFrame(picture, pieces, s);
    for (const i of frame.order) frame.draw(i);
    return frame.result;
  };

  // For applying, at full size, with breaks so Figma keeps up. Null once
  // `alive` says it's no longer wanted
  const drawBrokenFully = async (picture, pieces, s, alive) => {
    const frame = brokenFrame(picture, pieces, s);
    let until = performance.now() + 12;
    for (const i of frame.order) {
      frame.draw(i);
      if (performance.now() > until) {
        await new Promise((resolve) => setTimeout(resolve));
        if (!alive()) return null;
        until = performance.now() + 12;
      }
    }
    return frame.result;
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
    if (current.cuts[level]?.key !== key) {
      // How much of the picture isn't transparent, so the count is of
      // pieces with something in them
      const { data } = picture.getContext("2d").getImageData(0, 0, picture.width, picture.height);
      let solid = 0;
      for (let at = 3; at < data.length; at += 16) if (data[at] > 0) solid++;
      const covered = Math.max(0.02, solid / (data.length / 16));
      current.cuts[level] = { key, pieces: cutPieces(picture.width, picture.height, s.pattern, s.pieces, s.seed, covered) };
    }
    return current.cuts[level].pieces;
  };

  const renderAt = (current, level) => {
    const { picture } = current.levels[level];
    return drawBroken(picture, piecesFor(current, level), current.settings);
  };
  const renderFully = (current) => {
    const { picture } = current.levels.full;
    return drawBrokenFully(picture, piecesFor(current, "full"), structuredClone(current.settings), () => !current.closed);
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

  // The transform in `parent` that puts a layer at `absolute` on the page
  const relativeFor = (parent, absolute) => {
    const [[a, c, e], [b, d, f]] = absolute;
    const { x, y, linear } = toParent(parent, e, f);
    const [l0, l1, l2, l3] = linear;
    return [
      [l0 * a + l1 * b, l0 * c + l1 * d, x],
      [l2 * a + l3 * b, l2 * c + l3 * d, y],
    ];
  };

  // Puts `drawn`, from `renderAt`, on the result layer, as an image the
  // size it covers in the file
  // Reads `node` as a picture without changing what shows. Figma reads a
  // hidden layer, or one in a hidden group, as empty, so one that's hidden,
  // as the original under an effect is, is read from a copy at the bottom
  // of the page, out of the way, removed after
  const AWAY = 200000;
  const pictureReader = (figma, node) => {
    let shown = true;
    for (let at = node; at && at.type !== "PAGE"; at = at.parent) if (!at.visible) shown = false;
    let from = node;
    if (!shown) {
      from = node.clone();
      figma.currentPage.insertChild(0, from);
      const [[a, c, e], [b, d, f]] = node.absoluteTransform;
      from.relativeTransform = [
        [a, c, e + AWAY],
        [b, d, f],
      ];
      from.visible = true;
    }
    const box = from.absoluteRenderBounds ?? from.absoluteBoundingBox;
    return {
      bounds: { x: box.x - (from === node ? 0 : AWAY), y: box.y, width: box.width, height: box.height },
      read: async (scale) => createImageBitmap(new Blob([await from.exportAsync({ format: "PNG", constraint: { type: "SCALE", value: scale } })], { type: "image/png" })),
      done: () => {
        if (from !== node && !from.removed) from.remove();
      },
    };
  };

  // Whether a picture has anything to see at 8 pixels across, as `loaded`
  // reads it back
  const inked = (canvas) => {
    try {
      const context = Object.assign(document.createElement("canvas"), { width: 8, height: 8 }).getContext("2d", { willReadFrequently: true });
      context.drawImage(canvas, 0, 0, 8, 8);
      return context.getImageData(0, 0, 8, 8).data.some((value, at) => at % 4 === 3 && value > 0);
    } catch {
      return true;
    }
  };

  // Figma shows a new image only once it's loaded, empty until then, so
  // each is loaded on a small layer out of the way before it's put in place,
  // and the old one shows until then. Read back, a loaded one isn't empty
  const loaded = async (current, fill) => {
    const { figma, bounds } = current;
    if (current.closed) return;
    if (!current.loader || current.loader.removed) {
      current.loader = figma.createRectangle();
      current.loader.name = "Ligma loading";
      figma.currentPage.insertChild(0, current.loader);
      current.loader.resizeWithoutConstraints(8, 8);
      current.loader.x = bounds.x + AWAY;
      current.loader.y = bounds.y;
    }
    current.loader.fills = [fill];
    const until = performance.now() + 2000;
    while (performance.now() < until && !current.loader.removed) {
      try {
        const bytes = await current.loader.exportAsync({ format: "PNG", constraint: { type: "WIDTH", value: 8 } });
        const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
        const context = Object.assign(document.createElement("canvas"), { width: 8, height: 8 }).getContext("2d", { willReadFrequently: true });
        context.drawImage(bitmap, 0, 0, 8, 8);
        if (context.getImageData(0, 0, 8, 8).data.some((value, at) => at % 4 === 3 && value > 0)) return;
      } catch {
        return;
      }
    }
  };

  const placeDrawn = async (current, drawn, k) => {
    const { figma, result, bounds } = current;
    if (!drawn || result.removed) return;
    const blob = await new Promise((resolve) => drawn.canvas.toBlob(resolve, "image/png"));
    const bytes = new Uint8Array(await blob.arrayBuffer());
    if (result.removed) return;
    const fill = { type: "IMAGE", imageHash: figma.createImage(bytes).hash, scaleMode: "FILL" };
    // One with nothing to see never reads back as loaded, and showing it
    // empty meanwhile looks the same
    if (inked(drawn.canvas)) await loaded(current, fill);
    // Closed meanwhile, as it was is kept
    if (result.removed || current.closed) return;
    const place = toParent(result.parent, bounds.x + drawn.x / k, bounds.y + drawn.y / k);
    result.resizeWithoutConstraints(Math.max(0.01, drawn.width / k), Math.max(0.01, drawn.height / k));
    result.relativeTransform = [
      [place.linear[0], place.linear[1], place.x],
      [place.linear[2], place.linear[3], place.y],
    ];
    result.fills = [fill];
    result.visible = true;
    // The original's hidden once there's something in its place, so the
    // layer never shows empty
    if (!current.source.removed) current.source.visible = false;
  };

  const disintegrateOf = (node) => {
    try {
      return node?.type === "GROUP" && node.getSharedPluginData(PLUGIN, DISINTEGRATE) ? node : null;
    } catch {
      return null;
    }
  };

  let disintegrating = null;

  const openDisintegrate = async (figma, edit = false, run = null) => {
    if (repeating || disintegrating || crushing || filtering) return;
    const selected = figma.currentPage.selection;
    if (selected.length !== 1) {
      figma.notify(selected.length ? "Select one layer to disintegrate" : "Select a layer to disintegrate", { timeout: 2000 });
      return;
    }
    let group = edit ? disintegrateOf(selected[0]) : null;
    let source;
    let result = null;
    const settings = structuredClone(DISINTEGRATE_DEFAULTS);
    if (group) {
      source = group.children.find((child) => child.getSharedPluginData(PLUGIN, ROLE) === "source");
      result = group.children.find((child) => child.getSharedPluginData(PLUGIN, ROLE) === "result") ?? null;
      try {
        const saved = JSON.parse(group.getSharedPluginData(PLUGIN, DISINTEGRATE));
        // Settings from before forces, which aren't kept
        if (saved.force1 && "depth" in saved.force1) Object.assign(settings, saved);
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
    if (run) Object.assign(settings, structuredClone(run), { editable: true });
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
      run: Boolean(run),
    };
    disintegrating = current;
    try {
      figma.commitUndo();
    } catch {}
    // The layer as a picture: a small one first, quick to read, for the
    // panel's preview and the file while it's changed, so the panel shows
    // at once, then one as large as an image can be in Figma, for applying.
    const reader = pictureReader(figma, source);
    const { read } = reader;
    try {
      current.bounds = reader.bounds;
      const side = Math.max(1, current.bounds.width, current.bounds.height);
      const quick = await read(Math.min(2, QUICK / side));
      if (disintegrating !== current) return reader.done();
      const k = quick.width / current.bounds.width;
      current.levels = { preview: scaledPicture(quick, 360, k), live: scaledPicture(quick, QUICK, k) };
      quick.close?.();
      if (!run) {
        buildDisintegratePanel(current);
        refreshDisintegrate(current);
      }
      current.bitmap = await read(Math.min(2, MAX_SIDE / 2 / side));
      reader.done();
    } catch {
      reader.done();
      if (disintegrating === current) {
        current.root?.remove();
        removeEventListener("keydown", current.keys, true);
        disintegrating = null;
        figma.notify("Couldn't read the layer", { timeout: 2000 });
      }
      return;
    }
    if (disintegrating !== current) return;
    current.levels.full = scaledPicture(current.bitmap, MAX_SIDE, current.bitmap.width / current.bounds.width);
    // A new one's grouped from the start, as Repeater's are, the result
    // over the original, which is hidden while it shows
    const parent = source.parent;
    if (!group && parent && !("layoutMode" in parent && parent.layoutMode !== "NONE")) {
      current.home = { parent, index: parent.children.indexOf(source) };
      try {
        current.wrap = figma.group([source], parent, current.home.index);
        current.wrap.name = `Disintegrate · ${plainName(source.name)}`;
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
    figma.currentPage.selection = [current.wrap ?? current.group ?? current.result];
    current.ready = true;
    if (run) return applyDisintegrate(current);
    refreshDisintegrate(current);
  };

  // The panel's preview at once, and the file's a moment after the last
  // change, at a size quick to make
  const refreshDisintegrate = (current) => {
    if (!current.drawing) {
      current.drawing = true;
      requestAnimationFrame(() => {
        current.drawing = false;
        if (disintegrating === current) drawDisintegratePreview(current);
      });
    }
    if (!current.ready) return;
    clearTimeout(current.timer);
    current.timer = setTimeout(async () => {
      // Once it's being applied, only the full one goes in the file
      if (disintegrating !== current || current.busy) return;
      if (current.placing) {
        current.again = true;
        return;
      }
      current.placing = true;
      // One that fails is left for the next change to try again
      try {
        do {
          current.again = false;
          await placeDrawn(current, renderAt(current, "live"), current.levels.live.k);
        } while (current.again && !current.busy && disintegrating === current);
      } catch {}
      current.placing = false;
    }, 40);
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
    context.drawImage(drawn.canvas, ox + drawn.x * fit, oy + drawn.y * fit, drawn.width * fit, drawn.height * fit);
    // The forces: a dot to drag, and a ring for its radius
    const s = current.settings;
    const size = Math.max(width, height);
    [s.force1, s.force2].forEach((f, n) => {
      if (!f.on) return;
      const x = ox + (f.x / 100) * width * fit;
      const y = oy + (f.y / 100) * height * fit;
      // Where its sphere meets the layer
      const r = Math.sqrt(Math.max(0, f.radius ** 2 - f.depth ** 2)) / 100 * size * fit;
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
        const r = (Math.sqrt(Math.max(0, f.radius ** 2 - f.depth ** 2)) / 100) * size * v.fit;
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
          // The radius whose sphere meets the layer where the ring's dragged
          const f = current.settings[`force${at.n}`];
          const across = (Math.hypot(px - at.x, py - at.y) / (size * v.fit)) * 100;
          change(`force${at.n}.radius`, Math.round(Math.hypot(across, f.depth)));
        }
      };
      canvas.addEventListener("pointermove", move);
      canvas.addEventListener("lostpointercapture", () => canvas.removeEventListener("pointermove", move), { once: true });
    });
  };

  // Closed as Cancel closes it if it fails, rather than left making
  const applyDisintegrate = async (current) => {
    try {
      await finishDisintegrate(current);
    } catch (error) {
      if (disintegrating === current) closeDisintegrate(current, false);
      if (current.run) throw error;
      current.figma.notify("Couldn't make the effect", { timeout: 2000 });
    }
  };

  const finishDisintegrate = async (current) => {
    const { figma, source, settings } = current;
    clearTimeout(current.timer);
    current.busy = true;
    current.panel?.busy(true);
    // Until the full picture's read, and one being put in the file is in
    while (!current.ready || current.placing) {
      await new Promise((resolve) => setTimeout(resolve, 16));
      if (!current.ready && (disintegrating !== current && crushing !== current)) return;
    }
    current.again = false;
    await placeDrawn(current, await renderFully(current), current.levels.full.k);
    if (disintegrating !== current) return;
    let result = current.group ?? current.wrap;
    if (settings.editable) {
      if (!result || result.removed) {
        const parent = source.parent;
        result = figma.group([source, current.result], parent, parent.children.indexOf(source));
        result.name = `Disintegrate · ${plainName(source.name)}`;
      }
      source.setSharedPluginData(PLUGIN, ROLE, "source");
      result.setSharedPluginData(PLUGIN, DISINTEGRATE, JSON.stringify(settings));
      result.setSharedPluginData(PLUGIN, OFF, "");
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
    if (current.group && settings.editable) restackAbove(figma, current.group);
  };

  const closeDisintegrate = (current, applied) => {
    if (disintegrating !== current) return;
    disintegrating = null;
    clearTimeout(current.timer);
    current.closed = true;
    if (current.loader && !current.loader.removed) current.loader.remove();
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
          // One that was hidden stays hidden
          if (!current.group.removed && isOff(current.group)) setOff(current.group, true);
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
    // Picking a mode starts from its first preset
    const mode = segments(
      "Mode",
      [
        ["snap", "Snap"],
        ["shatter", "Shatter"],
      ],
      () => s.mode,
      (value) => {
        if (value === s.mode) return;
        const first = DISINTEGRATE_PRESETS.find(([, values]) => values().mode === value);
        Object.assign(s, structuredClone(first[1]()));
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
    const tumble = select("Tumble axis", TUMBLES, () => s.tumble, (value) => change("tumble", value));
    const physics = element(
      "section",
      {},
      element("div", { className: "caption", textContent: "Physics" }),
      element("div", { className: "rows" }, ...DISINTEGRATE_ROWS.physics.map(slider)),
      element("label", { className: "row" }, element("span", { className: "label", textContent: "Tumble axis" }), tumble),
    );
    const wind = element("section", {}, element("div", { className: "caption", textContent: "Snap" }), element("div", { className: "rows" }, ...DISINTEGRATE_ROWS.snap.map(slider)));
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

    const root = makePanel(current.group ? `Disintegrate · ${plainName(current.source.name)}` : "Disintegrate", () => closeDisintegrate(current, false), canvas, scroll, footer);
    const show = () => {
      mode.show();
      pattern.show();
      for (const row of rows) row.show();
      second.checked = s.force2.on;
      force2Rows.hidden = !s.force2.on;
      tumble.show();
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
      close: () => {
        pattern.close();
        tumble.close();
      },
    };
    current.keys = panelKeys(root, () => disintegrating === current, () => closeDisintegrate(current, false), () => !current.busy && applyDisintegrate(current));
    addEventListener("keydown", current.keys, true);
    go.focus();
  };

  // ---------- Bitcrush ----------

  // Pixelates, posterizes and dithers the layer, with glitch blocks, an
  // RGB split, noise, contrast and saturation: Figma's "Bitcrush lab"
  // shader effect, its WGSL ported here line for line to run on the
  // layer's picture in the page, as the plugin API can't make shader
  // effects. Sizes are in points, as the shader's are, so the look is the
  // same however large the picture. Put back as an image the layer's
  // size, the original kept under it, hidden
  const BITCRUSH = "bitcrush";
  const BITCRUSH_DEFAULTS = {
    pixelSize: 8,
    pixelAspect: 1,
    bitDepth: 3,
    colorMode: 0,
    ditherMode: 1,
    ditherStrength: 0.35,
    ditherScale: 3,
    glitchBlockSize: 48,
    glitchDisplacement: 24,
    glitchAmount: 0.06,
    splitDistance: 0,
    splitAngle: 0,
    noiseAmount: 0.015,
    randomSeed: 17,
    contrastAmount: 1.1,
    saturationAmount: 1,
    effectMix: 1,
    editable: true,
  };
  const BITCRUSH_PRESETS = [
    ["Classic", () => ({ ...BITCRUSH_DEFAULTS })],
    ["Game Boy", () => ({ ...BITCRUSH_DEFAULTS, pixelSize: 4, bitDepth: 2, colorMode: 1, ditherMode: 1, ditherStrength: 0.6, ditherScale: 4, glitchAmount: 0, noiseAmount: 0, contrastAmount: 1.2 })],
    ["8-bit", () => ({ ...BITCRUSH_DEFAULTS, pixelSize: 6, bitDepth: 2, ditherMode: 1, ditherStrength: 0.5, ditherScale: 6, glitchAmount: 0, noiseAmount: 0, saturationAmount: 1.3 })],
    ["Glitch", () => ({ ...BITCRUSH_DEFAULTS, pixelSize: 3, bitDepth: 5, ditherMode: 2, ditherStrength: 0.3, glitchBlockSize: 32, glitchDisplacement: 60, glitchAmount: 0.25, splitDistance: 6, splitAngle: 0, noiseAmount: 0.04 })],
    ["Dither", () => ({ ...BITCRUSH_DEFAULTS, pixelSize: 1, bitDepth: 1, colorMode: 1, ditherMode: 1, ditherStrength: 1, ditherScale: 2, glitchAmount: 0, noiseAmount: 0, contrastAmount: 1.3 })],
  ];
  const BITCRUSH_ROWS = {
    pixels: [
      ["pixelSize", "Pixel size", "pt", () => ({ min: 1, max: 100, shape: "curve" })],
      ["pixelAspect", "Pixel aspect", "", () => ({ min: 0.25, max: 4, middle: 1, digits: 2 })],
      ["bitDepth", "Color bit depth", "", () => ({ min: 1, max: 8 })],
    ],
    dither: [
      ["ditherStrength", "Strength", "", () => ({ min: 0, max: 2, digits: 2 })],
      ["ditherScale", "Scale", "pt", () => ({ min: 1, max: 48, shape: "curve" })],
    ],
    glitch: [
      ["glitchBlockSize", "Block size", "pt", () => ({ min: 4, max: 400, shape: "curve" })],
      ["glitchDisplacement", "Displacement", "pt", () => ({ min: 0, max: 400, shape: "curve" })],
      ["glitchAmount", "Amount", "", () => ({ min: 0, max: 1, digits: 2 })],
    ],
    split: [
      ["splitDistance", "Distance", "pt", () => ({ min: 0, max: 100, shape: "curve", digits: 1 })],
      ["splitAngle", "Angle", "°", () => ({ min: -180, max: 180 })],
    ],
    tone: [
      ["noiseAmount", "Noise", "", () => ({ min: 0, max: 1, shape: "curve", digits: 3 })],
      ["contrastAmount", "Contrast", "", () => ({ min: 0, max: 3, middle: 1, digits: 2 })],
      ["saturationAmount", "Saturation", "", () => ({ min: 0, max: 3, middle: 1, digits: 2 })],
      ["effectMix", "Mix", "", () => ({ min: 0, max: 1, digits: 2 })],
    ],
  };
  const BITCRUSH_LIMITS = {
    pixelSize: [1, 1000],
    pixelAspect: [0.05, 20],
    bitDepth: [1, 8],
    ditherStrength: [0, 10],
    ditherScale: [1, 500],
    glitchBlockSize: [1, 4000],
    glitchDisplacement: [0, 4000],
    glitchAmount: [0, 1],
    splitDistance: [0, 1000],
    splitAngle: [-360, 360],
    noiseAmount: [0, 1],
    contrastAmount: [0, 10],
    saturationAmount: [0, 10],
    effectMix: [0, 1],
  };

  // The shader's seededHash, bayer4 and fragment, on `image`, straight
  // ImageData, `k` its pixels a point
  const fract = (x) => x - Math.floor(x);
  const seededHash = (x, y, seed) => {
    let q0 = fract(x * 0.1031);
    let q1 = fract(y * 0.103);
    let q2 = fract(seed * 0.0973);
    const d = q0 * (q1 + 33.33) + q1 * (q2 + 33.33) + q2 * (q0 + 33.33);
    q0 += d;
    q1 += d;
    q2 += d;
    return fract((q0 + q1) * q2);
  };
  const bayer4 = (px, py) => {
    const x = ((px % 4) + 4) % 4;
    const y = ((py % 4) + 4) % 4;
    const low = ((x & 1) ^ (y & 1)) * 2 + (y & 1);
    const high = (((x >> 1) & 1) ^ ((y >> 1) & 1)) * 2 + ((y >> 1) & 1);
    return (low * 4 + high + 0.5) / 16 - 0.5;
  };
  // Rows `from` to `to` of it, as their own ImageData. It uses nothing of
  // the page's but `fract`, `seededHash` and `bayer4`, so workers run it
  // as it is
  const crushRows = (image, s, k, from, to) => {
    const { width: W, data } = image;
    const H = image.height;
    const out = new ImageData(W, Math.max(1, to - from));
    const o = out.data;
    const [dw, dh] = [W / k, H / k];
    // A pixel, premultiplied, 0 to 1, nearest to where `ux`, `uy` falls,
    // or nothing outside the picture with `zero`
    const sample = (ux, uy, zero, into) => {
      if (zero && (ux < 0 || uy < 0 || ux > 1 || uy > 1)) {
        into[0] = into[1] = into[2] = into[3] = 0;
        return into;
      }
      const ix = Math.min(W - 1, Math.max(0, Math.floor(Math.min(1, Math.max(0, ux)) * W)));
      const iy = Math.min(H - 1, Math.max(0, Math.floor(Math.min(1, Math.max(0, uy)) * H)));
      const at = (iy * W + ix) * 4;
      const a = data[at + 3] / 255;
      into[0] = (data[at] / 255) * a;
      into[1] = (data[at + 1] / 255) * a;
      into[2] = (data[at + 2] / 255) * a;
      into[3] = a;
      return into;
    };
    const [center, red, blue] = [new Float32Array(4), new Float32Array(4), new Float32Array(4)];
    const block = Math.max(s.glitchBlockSize, 1);
    const [cw, ch] = [Math.max(s.pixelSize * s.pixelAspect, 1), Math.max(s.pixelSize, 1)];
    const angle = s.splitAngle * 0.01745329252;
    const [sx, sy] = [(Math.cos(angle) * s.splitDistance) / dw, (Math.sin(angle) * s.splitDistance) / dh];
    const levels = 2 ** Math.min(8, Math.max(1, Math.round(s.bitDepth))) - 1;
    const ditherScale = Math.max(s.ditherScale, 1);
    const seed = s.randomSeed;
    for (let y = from; y < to; y++) {
      for (let x = 0; x < W; x++) {
        const at = (y * W + x) * 4;
        const put = ((y - from) * W + x) * 4;
        const px = (x + 0.5) / k;
        const py = (y + 0.5) / k;
        const [bx, by] = [Math.floor(px / block), Math.floor(py / block)];
        const enabled = seededHash(bx, by, seed) < s.glitchAmount ? 1 : 0;
        const dx = enabled ? (seededHash(bx + 19, by + 7, seed) * 2 - 1) * s.glitchDisplacement : 0;
        const dy = enabled ? (seededHash(bx + 3, by + 29, seed) * 2 - 1) * 0.2 * s.glitchDisplacement : 0;
        const [qx, qy] = [px + dx, py + dy];
        const [ox, oy] = [Math.floor(qx / cw) * cw, Math.floor(qy / ch) * ch];
        const ux = (ox + Math.min(cw, Math.max(dw - ox, 0)) * 0.5) / dw;
        const uy = (oy + Math.min(ch, Math.max(dh - oy, 0)) * 0.5) / dh;
        sample(ux, uy, true, center);
        sample(ux + sx, uy + sy, true, red);
        sample(ux - sx, uy - sy, true, blue);
        let r = red[0] / Math.max(red[3], 0.00001);
        let g = center[1] / Math.max(center[3], 0.00001);
        let b = blue[2] / Math.max(blue[3], 0.00001);
        let luma = r * 0.2126 + g * 0.7152 + b * 0.0722;
        r = luma + (r - luma) * s.saturationAmount;
        g = luma + (g - luma) * s.saturationAmount;
        b = luma + (b - luma) * s.saturationAmount;
        r = (r - 0.5) * s.contrastAmount + 0.5;
        g = (g - 0.5) * s.contrastAmount + 0.5;
        b = (b - 0.5) * s.contrastAmount + 0.5;
        if (s.colorMode === 1) r = g = b = r * 0.2126 + g * 0.7152 + b * 0.0722;
        const n = (seededHash(Math.floor(px), Math.floor(py), seed + 47) * 2 - 1) * s.noiseAmount;
        r += n;
        g += n;
        b += n;
        let dither = 0;
        if (s.ditherMode === 1) dither = bayer4(Math.floor(px / ditherScale), Math.floor(py / ditherScale));
        else if (s.ditherMode === 2) dither = seededHash(Math.floor(px / ditherScale), Math.floor(py / ditherScale), seed + 101) - 0.5;
        const d = (dither * s.ditherStrength) / levels;
        const q = (v) => Math.floor(Math.min(1, Math.max(0, v + d)) * levels + 0.5) / levels;
        const a = center[3];
        // The input, premultiplied, mixed toward the crushed
        const ia = data[at + 3] / 255;
        const m = s.effectMix;
        const outA = ia + (a - ia) * m;
        const mixOne = (inV, crushed) => ((data[at + inV] / 255) * ia * (1 - m) + crushed * a * m) / Math.max(outA, 0.00001);
        o[put] = Math.round(mixOne(0, q(r)) * 255);
        o[put + 1] = Math.round(mixOne(1, q(g)) * 255);
        o[put + 2] = Math.round(mixOne(2, q(b)) * 255);
        o[put + 3] = Math.round(outA * 255);
      }
    }
    return out;
  };

  // The pixel loop for the file's pictures runs in workers, a band of rows
  // each, so Figma keeps up while a large one's crushed. Two at most, as
  // each gets its own copy of the picture. Where workers can't start, it
  // runs in the page a few rows at a time, with breaks between
  const CRUSH_WORKER = [
    `const fract = ${fract};`,
    `const seededHash = ${seededHash};`,
    `const bayer4 = ${bayer4};`,
    `const crushRows = ${crushRows};`,
    `onmessage = ({ data: { id, image, s, k, from, to } }) => {`,
    `  const out = crushRows(image, s, k, from, to);`,
    `  postMessage({ id, out }, [out.data.buffer]);`,
    `};`,
    `postMessage({ ready: true });`,
  ].join("\n");
  let crushers = null;
  let crushersReady = null;
  let crushJob = 0;
  const crushJobs = new Map();
  const stopCrushers = () => {
    for (const worker of crushers ?? []) worker.terminate();
    crushers = [];
    for (const job of crushJobs.values()) job.reject(new Error("Worker stopped"));
    crushJobs.clear();
  };
  // The workers, once each has said it's loaded. One that can't load, as
  // where the page's rules forbid it, or that's slow to, stops them all,
  // and the page crushes from then on
  const crushWorkers = async () => {
    if (!crushers) {
      crushers = [];
      const loads = [];
      try {
        const url = URL.createObjectURL(new Blob([CRUSH_WORKER], { type: "text/javascript" }));
        for (let i = 0; i < Math.min(2, Math.max(1, (navigator.hardwareConcurrency || 2) - 1)); i++) {
          const worker = new Worker(url);
          loads.push(
            new Promise((resolve) => {
              worker.onmessage = ({ data }) => {
                if (data.ready) return resolve(true);
                const job = crushJobs.get(data.id);
                crushJobs.delete(data.id);
                job?.resolve(data.out);
              };
              worker.onerror = (event) => {
                event.preventDefault();
                resolve(false);
                stopCrushers();
              };
            }),
          );
          crushers.push(worker);
        }
      } catch {
        loads.push(false);
      }
      const late = new Promise((resolve) => setTimeout(() => resolve(false), 2000));
      crushersReady = Promise.race([Promise.all(loads).then((all) => all.every(Boolean)), late]).then((ready) => ready || stopCrushers());
    }
    await crushersReady;
    return crushers;
  };
  const inWorker = (worker, message) =>
    new Promise((resolve, reject) => {
      const id = ++crushJob;
      crushJobs.set(id, { resolve, reject });
      worker.postMessage({ id, ...message });
    });
  // Null once `alive` says it's no longer wanted
  const crushApart = async (image, s, k, alive) => {
    const { width: W, height: H } = image;
    const out = new ImageData(W, H);
    const workers = await crushWorkers();
    if (!alive()) return null;
    if (workers.length) {
      try {
        const band = Math.ceil(H / workers.length);
        const parts = await Promise.all(
          workers.map((worker, i) => {
            const [from, to] = [i * band, Math.min(H, (i + 1) * band)];
            return from < to ? inWorker(worker, { image, s, k, from, to }).then((part) => [from, part]) : null;
          }),
        );
        for (const part of parts) if (part) out.data.set(part[1].data, part[0] * W * 4);
        return out;
      } catch {}
    }
    const rows = Math.max(1, Math.floor(150000 / W));
    for (let from = 0; from < H; from += rows) {
      if (!alive()) return null;
      const to = Math.min(H, from + rows);
      out.data.set(crushRows(image, s, k, from, to).data, from * W * 4);
      await new Promise((resolve) => setTimeout(resolve));
    }
    return out;
  };

  const crushedPixels = (entry) => (entry.pixels ??= entry.picture.getContext("2d", { willReadFrequently: true }).getImageData(0, 0, entry.picture.width, entry.picture.height));
  const crushedCanvas = (image) => {
    const canvas = Object.assign(document.createElement("canvas"), { width: image.width, height: image.height });
    canvas.getContext("2d").putImageData(image, 0, 0);
    return { canvas, x: 0, y: 0, width: canvas.width, height: canvas.height };
  };
  // The crushed picture for `level`, as `drawBroken` gives its own: the
  // panel's small one at once, the file's apart from the page
  const crushAt = async (current, level) => {
    const entry = current.levels[level];
    const image = await crushApart(crushedPixels(entry), structuredClone(current.settings), entry.k, () => !current.closed);
    return image && crushedCanvas(image);
  };

  const drawBitcrushPreview = (current) => {
    const canvas = current.preview;
    if (!canvas || !current.levels.preview) return;
    const entry = current.levels.preview;
    const pixels = crushedPixels(entry);
    const drawn = crushedCanvas(crushRows(pixels, current.settings, entry.k, 0, pixels.height));
    const ratio = devicePixelRatio || 1;
    const box = canvas.getBoundingClientRect();
    canvas.width = Math.round(box.width * ratio);
    canvas.height = Math.round(box.height * ratio);
    const context = canvas.getContext("2d");
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, box.width, box.height);
    const fit = Math.min((box.width - 24) / drawn.width, (box.height - 24) / drawn.height);
    // Pixels stay sharp, as they're the point
    context.imageSmoothingEnabled = fit < 1;
    context.drawImage(drawn.canvas, (box.width - drawn.width * fit) / 2, (box.height - drawn.height * fit) / 2, drawn.width * fit, drawn.height * fit);
  };

  const buildBitcrushPanel = (current) => {
    const s = current.settings;
    const rows = [];
    const change = (key, value) => {
      s[key] = value;
      refreshBitcrush(current);
      show();
    };
    const slider = makeSlider({ s, unit: current.unit, rows, change, limits: BITCRUSH_LIMITS });
    const canvas = element("canvas", { className: "preview" });
    canvas.setAttribute("aria-hidden", "true");
    current.preview = canvas;
    const pick = (values) => {
      Object.assign(s, values(), { editable: s.editable });
      refreshBitcrush(current);
      show();
    };
    const presets = makeChips(BITCRUSH_PRESETS, pick);
    const colorMode = segments(
      "Color mode",
      [
        ["0", "RGB"],
        ["1", "Grayscale"],
      ],
      () => String(s.colorMode),
      (value) => change("colorMode", Number(value)),
    );
    const ditherMode = select(
      "Dither pattern",
      [
        ["0", "Off"],
        ["1", "Ordered Bayer"],
        ["2", "Noise"],
      ],
      () => String(s.ditherMode),
      (value) => change("ditherMode", Number(value)),
    );
    const caption = (text) => element("div", { className: "caption", textContent: text });
    const rowsOf = (list) => element("div", { className: "rows" }, ...list.map(slider));
    const labelled = (text, control) => element("div", { className: "row" }, element("span", { className: "label", textContent: text }), control);
    const shuffle = element("button", { type: "button", className: "chip", textContent: "Shuffle" });
    shuffle.addEventListener("click", () => change("randomSeed", Math.floor(Math.random() * 65536)));
    shuffle.style.gridColumn = "span 2";
    shuffle.style.justifySelf = "start";
    const pixels = element("section", {}, caption("Pixels"), rowsOf(BITCRUSH_ROWS.pixels), labelled("Color mode", colorMode));
    const dither = element("section", {}, caption("Dither"), labelled("Pattern", ditherMode), rowsOf(BITCRUSH_ROWS.dither));
    const glitch = element("section", {}, caption("Glitch"), rowsOf(BITCRUSH_ROWS.glitch), labelled("Random seed", shuffle));
    const split = element("section", {}, caption("RGB split"), rowsOf(BITCRUSH_ROWS.split));
    const tone = element("section", {}, caption("Tone"), rowsOf(BITCRUSH_ROWS.tone));
    const editable = element("input", { type: "checkbox" });
    editable.addEventListener("change", () => change("editable", editable.checked));
    const result = element("section", {}, checkRow("Keep editable", editable, "The original kept, hidden, in a group Bitcrush can change later"));
    const reset = element("button", { type: "button", className: "link", textContent: "Reset" });
    reset.addEventListener("click", () => pick(BITCRUSH_PRESETS[0][1]));
    const scroll = element("div", { className: "scroll" }, presets, pixels, dither, glitch, split, tone, result, element("div", { className: "under" }, reset));
    const name = element("div", { className: "source", textContent: current.source.name });
    const cancel = element("button", { type: "button", className: "button", textContent: "Cancel" });
    cancel.addEventListener("click", () => closeBitcrush(current, false));
    const go = element("button", { type: "button", className: "button primary", textContent: current.group ? "Update" : "Bitcrush" });
    go.addEventListener("click", () => !current.busy && applyBitcrush(current));
    const footer = element("div", { className: "footer" }, name, element("div", { className: "actions" }, cancel, go));
    const root = makePanel(current.group ? `Bitcrush · ${plainName(current.source.name)}` : "Bitcrush", () => closeBitcrush(current, false), canvas, scroll, footer);
    const show = () => {
      colorMode.show();
      ditherMode.show();
      for (const row of rows) row.show();
      editable.checked = s.editable;
      drawBitcrushPreview(current);
    };
    show();
    current.root = root;
    current.panel = {
      busy: (on) => {
        go.disabled = on;
        go.textContent = on ? "Making…" : current.group ? "Update" : "Bitcrush";
      },
      close: () => ditherMode.close(),
    };
    current.keys = panelKeys(root, () => crushing === current, () => closeBitcrush(current, false), () => !current.busy && applyBitcrush(current));
    addEventListener("keydown", current.keys, true);
    go.focus();
  };

  const bitcrushOf = (node) => {
    try {
      return node?.type === "GROUP" && node.getSharedPluginData(PLUGIN, BITCRUSH) ? node : null;
    } catch {
      return null;
    }
  };

  let crushing = null;

  const openBitcrush = async (figma, edit = false, run = null) => {
    if (repeating || disintegrating || crushing || filtering) return;
    const selected = figma.currentPage.selection;
    if (selected.length !== 1) {
      figma.notify(selected.length ? "Select one layer to bitcrush" : "Select a layer to bitcrush", { timeout: 2000 });
      return;
    }
    let group = edit ? bitcrushOf(selected[0]) : null;
    let source;
    let result = null;
    const settings = structuredClone(BITCRUSH_DEFAULTS);
    if (group) {
      source = group.children.find((child) => child.getSharedPluginData(PLUGIN, ROLE) === "source");
      result = group.children.find((child) => child.getSharedPluginData(PLUGIN, ROLE) === "result") ?? null;
      try {
        const saved = JSON.parse(group.getSharedPluginData(PLUGIN, BITCRUSH));
        Object.assign(settings, saved);
      } catch {}
      if (!source) {
        group = null;
        source = selected[0];
      }
    } else {
      source = selected[0];
    }
    if (source.type === "SECTION" || !("exportAsync" in source)) {
      figma.notify("Sections can't be bitcrushed", { timeout: 2000 });
      return;
    }
    if (run) Object.assign(settings, structuredClone(run), { editable: true });

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
      run: Boolean(run),
    };
    crushing = current;
    try {
      figma.commitUndo();
    } catch {}
    // The layer as a picture: a small one first, quick to read, for the
    // panel's preview and the file while it's changed, so the panel shows
    // at once, then one as large as an image can be in Figma, for applying.
    const reader = pictureReader(figma, source);
    const { read } = reader;
    try {
      current.bounds = reader.bounds;
      const side = Math.max(1, current.bounds.width, current.bounds.height);
      const quick = await read(Math.min(2, QUICK / side));
      if (crushing !== current) return reader.done();
      const k = quick.width / current.bounds.width;
      current.levels = { preview: scaledPicture(quick, 360, k), live: scaledPicture(quick, QUICK, k) };
      quick.close?.();
      if (!run) {
        buildBitcrushPanel(current);
        refreshBitcrush(current);
      }
      current.bitmap = await read(Math.min(2, MAX_SIDE / 2 / side));
      reader.done();
    } catch {
      reader.done();
      if (crushing === current) {
        current.root?.remove();
        removeEventListener("keydown", current.keys, true);
        crushing = null;
        figma.notify("Couldn't read the layer", { timeout: 2000 });
      }
      return;
    }
    if (crushing !== current) return;
    current.levels.full = scaledPicture(current.bitmap, MAX_SIDE, current.bitmap.width / current.bounds.width);
    // Grouped from the start, as Disintegrate's are
    const parent = source.parent;
    if (!group && parent && !("layoutMode" in parent && parent.layoutMode !== "NONE")) {
      current.home = { parent, index: parent.children.indexOf(source) };
      try {
        current.wrap = figma.group([source], parent, current.home.index);
        current.wrap.name = `Bitcrush · ${plainName(source.name)}`;
      } catch {
        current.wrap = null;
      }
    }
    if (!current.result) {
      current.result = figma.createRectangle();
      current.result.name = `${source.name} bitcrushed`;
      const holder = source.parent;
      holder.insertChild(holder.children.indexOf(source) + 1, current.result);
      if ("layoutMode" in holder && holder.layoutMode !== "NONE") current.result.layoutPositioning = "ABSOLUTE";
      current.result.setSharedPluginData(PLUGIN, ROLE, "result");
      current.result.visible = false;
    }
    figma.currentPage.selection = [current.wrap ?? current.group ?? current.result];
    current.ready = true;
    if (run) return applyBitcrush(current);
    refreshBitcrush(current);
  };

  // The panel's preview at once, and the file's a moment after
  const refreshBitcrush = (current) => {
    if (!current.drawing) {
      current.drawing = true;
      requestAnimationFrame(() => {
        current.drawing = false;
        if (crushing === current) drawBitcrushPreview(current);
      });
    }
    if (!current.ready) return;
    clearTimeout(current.timer);
    current.timer = setTimeout(async () => {
      // Once it's being applied, only the full one goes in the file
      if (crushing !== current || current.busy) return;
      if (current.placing) {
        current.again = true;
        return;
      }
      current.placing = true;
      // One that fails is left for the next change to try again
      try {
        do {
          current.again = false;
          await placeDrawn(current, await crushAt(current, "live"), current.levels.live.k);
        } while (current.again && !current.busy && crushing === current);
      } catch {}
      current.placing = false;
    }, 40);
  };

  // Closed as Cancel closes it if it fails, rather than left making
  const applyBitcrush = async (current) => {
    try {
      await finishBitcrush(current);
    } catch (error) {
      if (crushing === current) closeBitcrush(current, false);
      if (current.run) throw error;
      current.figma.notify("Couldn't make the effect", { timeout: 2000 });
    }
  };

  const finishBitcrush = async (current) => {
    const { figma, source, settings } = current;
    clearTimeout(current.timer);
    current.busy = true;
    current.panel?.busy(true);
    // Until the full picture's read, and one being put in the file is in
    while (!current.ready || current.placing) {
      await new Promise((resolve) => setTimeout(resolve, 16));
      if (!current.ready && (disintegrating !== current && crushing !== current)) return;
    }
    current.again = false;
    await placeDrawn(current, await crushAt(current, "full"), current.levels.full.k);
    if (crushing !== current) return;
    let result = current.group ?? current.wrap;
    if (settings.editable) {
      if (!result || result.removed) {
        const parent = source.parent;
        result = figma.group([source, current.result], parent, parent.children.indexOf(source));
        result.name = `Bitcrush · ${plainName(source.name)}`;
      }
      source.setSharedPluginData(PLUGIN, ROLE, "source");
      result.setSharedPluginData(PLUGIN, BITCRUSH, JSON.stringify(settings));
      result.setSharedPluginData(PLUGIN, OFF, "");
      figma.currentPage.selection = [result];
    } else {
      // Only the image, the original gone
      current.result.setSharedPluginData(PLUGIN, ROLE, "");
      if (result && !result.removed) result.setSharedPluginData(PLUGIN, BITCRUSH, "");
      const image = current.result;
      const holder = current.wrap ?? current.group;
      if (holder && !holder.removed) figma.ungroup(holder);
      if (!source.removed) source.remove();
      image.name = image.name.replace(/ bitcrushed$/, "");
      figma.currentPage.selection = [image];
    }
    try {
      figma.commitUndo();
    } catch {}
    closeBitcrush(current, true);
    if (current.group && settings.editable) restackAbove(figma, current.group);
  };

  const closeBitcrush = (current, applied) => {
    if (crushing !== current) return;
    crushing = null;
    clearTimeout(current.timer);
    current.closed = true;
    // The workers go with the panel, as their memory's only needed while
    // it's open
    stopCrushers();
    crushers = null;
    if (current.loader && !current.loader.removed) current.loader.remove();
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
          // One that was hidden stays hidden
          if (!current.group.removed && isOff(current.group)) setOff(current.group, true);
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



  // ---------- Picture effects ----------

  // Effects that make a new picture from the layer's, as Bitcrush does:
  // Dither, Halftone, Modulate, Chromatic Aberration, Fisheye, Liquify,
  // Displace, Motion Blur, Glow, Shadow, Reflection and Rim Light. Each
  // gives its settings, panel and a pixel function; opening, the live
  // picture in the file, applying, cancelling and stacking are shared. A
  // pixel function takes the picture as ImageData, the settings and the
  // picture's pixels a point, and gives back an ImageData and where its
  // top left sits from the picture's, as some draw past the layer. For the
  // file's pictures it runs in a worker, so it uses nothing of the page's
  // but the helpers in `PIXEL_HELPERS`. Nothing's generated: every result
  // is worked out from the layer's own pixels
  const clamp01 = (v) => (v < 0 ? 0 : v > 1 ? 1 : v);
  // Straight 0 to 255 to premultiplied 0 to 1, four floats a pixel
  const premultiplied = (image) => {
    const { data } = image;
    const f = new Float32Array(data.length);
    for (let i = 0; i < data.length; i += 4) {
      const a = data[i + 3] / 255;
      f[i] = (data[i] / 255) * a;
      f[i + 1] = (data[i + 1] / 255) * a;
      f[i + 2] = (data[i + 2] / 255) * a;
      f[i + 3] = a;
    }
    return f;
  };
  const toImage = (f, W, H) => {
    const out = new ImageData(W, H);
    const o = out.data;
    for (let i = 0; i < W * H * 4; i += 4) {
      const a = f[i + 3];
      if (a <= 0.0005) continue;
      const alpha = a > 1 ? 1 : a;
      o[i] = Math.round(Math.min(1, f[i] / a) * 255);
      o[i + 1] = Math.round(Math.min(1, f[i + 1] / a) * 255);
      o[i + 2] = Math.round(Math.min(1, f[i + 2] / a) * 255);
      o[i + 3] = Math.round(alpha * 255);
    }
    return out;
  };
  // Bilinear, premultiplied, at `x`, `y` in pixels, nothing outside
  const sampleAt = (f, W, H, x, y, into) => {
    x -= 0.5;
    y -= 0.5;
    const x0 = Math.floor(x);
    const y0 = Math.floor(y);
    const tx = x - x0;
    const ty = y - y0;
    into[0] = into[1] = into[2] = into[3] = 0;
    for (let j = 0; j < 2; j++) {
      const yy = y0 + j;
      if (yy < 0 || yy >= H) continue;
      const wy = j ? ty : 1 - ty;
      for (let i = 0; i < 2; i++) {
        const xx = x0 + i;
        if (xx < 0 || xx >= W) continue;
        const w = wy * (i ? tx : 1 - tx);
        if (!w) continue;
        const at = (yy * W + xx) * 4;
        into[0] += f[at] * w;
        into[1] += f[at + 1] * w;
        into[2] += f[at + 2] * w;
        into[3] += f[at + 3] * w;
      }
    }
    return into;
  };
  // The same for one float a pixel
  const channelAt = (c, W, H, x, y) => {
    x -= 0.5;
    y -= 0.5;
    const x0 = Math.floor(x);
    const y0 = Math.floor(y);
    const tx = x - x0;
    const ty = y - y0;
    let v = 0;
    for (let j = 0; j < 2; j++) {
      const yy = y0 + j;
      if (yy < 0 || yy >= H) continue;
      const wy = j ? ty : 1 - ty;
      for (let i = 0; i < 2; i++) {
        const xx = x0 + i;
        if (xx < 0 || xx >= W) continue;
        v += c[yy * W + xx] * wy * (i ? tx : 1 - tx);
      }
    }
    return v;
  };
  // A box blur of radius `r` along rows or columns, nothing past the edges
  const boxPass = (src, dst, W, H, r, horizontal) => {
    const [n, m, step, stride] = horizontal ? [W, H, 1, W] : [H, W, W, 1];
    const norm = 1 / (2 * r + 1);
    for (let j = 0; j < m; j++) {
      const base = j * stride;
      let sum = 0;
      for (let i = 0; i <= Math.min(r, n - 1); i++) sum += src[base + i * step];
      for (let i = 0; i < n; i++) {
        dst[base + i * step] = sum * norm;
        const add = i + r + 1;
        const sub = i - r;
        if (add < n) sum += src[base + add * step];
        if (sub >= 0) sum -= src[base + sub * step];
      }
    }
  };
  // About a Gaussian of `sigma` pixels: three box blurs each way
  const blurChannel = (c, W, H, sigma) => {
    const out = Float32Array.from(c);
    if (!(sigma > 0.3)) return out;
    const r = Math.max(1, Math.round((Math.sqrt(4 * sigma * sigma + 1) - 1) / 2));
    const tmp = new Float32Array(c.length);
    for (let pass = 0; pass < 3; pass++) {
      boxPass(out, tmp, W, H, r, true);
      boxPass(tmp, out, W, H, r, false);
    }
    return out;
  };
  const hexRgb = (hex) => {
    const n = parseInt(String(hex).replace("#", "").padEnd(6, "0").slice(0, 6), 16) || 0;
    return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
  };
  const lumaOf = (r, g, b) => r * 0.2126 + g * 0.7152 + b * 0.0722;
  const hash2 = (x, y, seed) => {
    let h = (Math.imul(x | 0, 374761393) + Math.imul(y | 0, 668265263) + Math.imul(seed | 0, 1442695041)) | 0;
    h = Math.imul(h ^ (h >>> 13), 1274126177);
    return ((h ^ (h >>> 16)) >>> 0) / 4294967296;
  };
  // The picture averaged in square cells of `cell` pixels, premultiplied
  const cellGrid = (f, W, H, cell) => {
    const size = Math.max(1, cell);
    const gw = Math.max(1, Math.ceil(W / size));
    const gh = Math.max(1, Math.ceil(H / size));
    const g = new Float32Array(gw * gh * 4);
    const n = new Float32Array(gw * gh);
    for (let y = 0; y < H; y++) {
      const gy = Math.min(gh - 1, Math.floor(y / size));
      for (let x = 0; x < W; x++) {
        const to = gy * gw + Math.min(gw - 1, Math.floor(x / size));
        const at = (y * W + x) * 4;
        g[to * 4] += f[at];
        g[to * 4 + 1] += f[at + 1];
        g[to * 4 + 2] += f[at + 2];
        g[to * 4 + 3] += f[at + 3];
        n[to]++;
      }
    }
    for (let i = 0; i < gw * gh; i++) if (n[i]) for (let c = 0; c < 4; c++) g[i * 4 + c] /= n[i];
    return { g, gw, gh, size };
  };
  // The cells' average at `x`, `y` in the picture's pixels, smoothly
  const gridAt = (grid, x, y, into) => {
    const gx = Math.min(grid.gw - 0.5, Math.max(0.5, x / grid.size));
    const gy = Math.min(grid.gh - 0.5, Math.max(0.5, y / grid.size));
    return sampleAt(grid.g, grid.gw, grid.gh, gx, gy, into);
  };
  // The first and last rows of what's solid, half opaque or more, where a
  // cutout stands on its ground
  const solidRows = (f, W, H) => {
    let top = -1;
    let bottom = -1;
    for (let y = 0; y < H && top < 0; y++) for (let x = 0; x < W; x++) if (f[(y * W + x) * 4 + 3] > 0.5) { top = y; break; }
    for (let y = H - 1; y >= 0 && bottom < 0; y--) for (let x = 0; x < W; x++) if (f[(y * W + x) * 4 + 3] > 0.5) { bottom = y + 1; break; }
    return top < 0 ? { top: 0, bottom: H } : { top, bottom };
  };
  // Whether none of it's see-through, a photo rather than a cutout, so
  // effects that move its pixels keep to its bounds and repeat its edges
  const isOpaque = (f) => {
    for (let i = 3; i < f.length; i += 4) if (f[i] < 0.999) return false;
    return true;
  };
  // `sampleAt` with its edge pixels repeated past it
  const edgeAt = (f, W, H, x, y, into) => sampleAt(f, W, H, Math.min(W - 0.5, Math.max(0.5, x)), Math.min(H - 0.5, Math.max(0.5, y)), into);
  const PIXEL_HELPERS = { clamp01, premultiplied, toImage, sampleAt, channelAt, boxPass, blurChannel, hexRgb, lumaOf, hash2, cellGrid, gridAt, solidRows, isOpaque, edgeAt };

  style.textContent += `
  `;

  let filtering = null;
  let filterJob = 0;
  const filterOf = (spec, node) => {
    try {
      return node?.type === "GROUP" && node.getSharedPluginData(PLUGIN, spec.id) ? node : null;
    } catch {
      return null;
    }
  };
  const pictureEffect = (spec) =>
    EFFECTS.push({
      id: spec.id,
      name: spec.name,
      about: spec.about,
      icon: spec.icon,
      of: (node) => filterOf(spec, node),
      saved: (group) => JSON.parse(group.getSharedPluginData(PLUGIN, spec.id) || "{}"),
      summary: (s) => spec.summary({ ...structuredClone(spec.defaults), ...s }),
      open: guarded((figma, edit, run) => openFilter(spec, figma, edit, run)),
      thumb: () => pictureThumb(spec),
    });

  // The open effect's worker, once it's said it's loaded, or null where
  // one can't start, and the page runs it instead
  const filterWorker = (current) =>
    (current.worker ??= new Promise((resolve) => {
      let ready = false;
      let worker = null;
      current.jobs = new Map();
      const fail = () => {
        if (ready) return;
        ready = true;
        worker?.terminate();
        resolve(null);
      };
      try {
        const source = [...Object.entries(PIXEL_HELPERS).map(([name, fn]) => `const ${name} = ${fn};`), `const process = ${current.spec.process};`, `onmessage = ({ data: { id, image, s, k } }) => {`, `  try {`, `    const made = process(image, s, k);`, `    postMessage({ id, out: made.image, x: made.x, y: made.y }, [made.image.data.buffer]);`, `  } catch (error) {`, `    postMessage({ id, error: String(error) });`, `  }`, `};`, `postMessage({ ready: true });`].join("\n");
        const url = URL.createObjectURL(new Blob([source], { type: "text/javascript" }));
        worker = new Worker(url);
        worker.onmessage = ({ data }) => {
          if (data.ready) {
            ready = true;
            URL.revokeObjectURL(url);
            return resolve(worker);
          }
          const job = current.jobs.get(data.id);
          current.jobs.delete(data.id);
          if (data.error) job?.reject(new Error(data.error));
          else job?.resolve({ image: data.out, x: data.x, y: data.y });
        };
        worker.onerror = (event) => {
          event.preventDefault();
          for (const job of current.jobs.values()) job.reject(new Error("Worker failed"));
          current.jobs.clear();
          fail();
        };
        setTimeout(fail, 2000);
      } catch {
        fail();
      }
    }));
  const stopFilterWorker = (current) => {
    current.worker?.then((worker) => worker?.terminate());
    for (const job of current.jobs?.values() ?? []) job.reject(new Error("Worker stopped"));
    current.jobs?.clear();
  };

  // What a pixel function made, as `drawBroken` gives its own: a canvas,
  // made smaller if it's past what an image can be in Figma, and where it
  // sits from the picture's top left, in the picture's pixels
  const drawnFrom = ({ image, x, y }) => {
    let canvas = Object.assign(document.createElement("canvas"), { width: image.width, height: image.height });
    canvas.getContext("2d").putImageData(image, 0, 0);
    const side = Math.max(image.width, image.height);
    if (side > MAX_SIDE) {
      const ratio = MAX_SIDE / side;
      const small = Object.assign(document.createElement("canvas"), { width: Math.max(1, Math.round(image.width * ratio)), height: Math.max(1, Math.round(image.height * ratio)) });
      small.getContext("2d").drawImage(canvas, 0, 0, small.width, small.height);
      canvas = small;
    }
    return { canvas, x, y, width: image.width, height: image.height };
  };

  // The picture for `level` made apart from the page
  const filterAt = async (current, level) => {
    const entry = current.levels[level];
    const image = crushedPixels(entry);
    const s = structuredClone(current.settings);
    const worker = await filterWorker(current);
    if (current.closed) return null;
    let made = null;
    if (worker) {
      try {
        made = await new Promise((resolve, reject) => {
          const id = ++filterJob;
          current.jobs.set(id, { resolve, reject });
          worker.postMessage({ id, image, s, k: entry.k });
        });
      } catch {}
    }
    if (current.closed) return null;
    if (!made) {
      await new Promise((resolve) => setTimeout(resolve));
      made = current.spec.process(image, s, entry.k);
    }
    return drawnFrom(made);
  };

  const drawFilterPreview = (current) => {
    const canvas = current.preview;
    if (!canvas || !current.levels.preview) return;
    const entry = current.levels.preview;
    let drawn;
    try {
      drawn = drawnFrom(current.spec.process(crushedPixels(entry), current.settings, entry.k));
    } catch {
      return;
    }
    const ratio = devicePixelRatio || 1;
    const box = canvas.getBoundingClientRect();
    canvas.width = Math.round(box.width * ratio);
    canvas.height = Math.round(box.height * ratio);
    const context = canvas.getContext("2d");
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, box.width, box.height);
    const fit = Math.min((box.width - 24) / drawn.width, (box.height - 24) / drawn.height);
    context.imageSmoothingEnabled = !current.spec.sharp || fit < 1;
    context.drawImage(drawn.canvas, (box.width - drawn.width * fit) / 2, (box.height - drawn.height * fit) / 2, drawn.width * fit, drawn.height * fit);
  };

  const buildFilterPanel = (current) => {
    const { spec } = current;
    const s = current.settings;
    const rows = [];
    const shows = [];
    const closes = [];
    const change = (key, value) => {
      s[key] = value;
      refreshFilter(current);
      show();
    };
    const slider = makeSlider({ s, unit: current.unit, rows, change, limits: spec.limits ?? {} });
    const labelled = (text, control) => element("div", { className: "row" }, element("span", { className: "label", textContent: text }), control);
    const ui = {
      s,
      unit: current.unit,
      change,
      element,
      section: (caption, ...parts) => element("section", {}, ...(caption ? [element("div", { className: "caption", textContent: caption })] : []), ...parts.filter(Boolean)),
      sliders: (list) => element("div", { className: "rows" }, ...list.map(slider)),
      segments: (key, label, options) => {
        const control = segments(label, options, () => s[key], (value) => change(key, value));
        shows.push(() => control.show());
        return labelled(label, control);
      },
      select: (key, label, options) => {
        const control = select(label, options, () => s[key], (value) => change(key, value));
        shows.push(() => control.show());
        closes.push(() => control.close());
        return labelled(label, control);
      },
      check: (key, label, title = "") => {
        const input = element("input", { type: "checkbox" });
        input.addEventListener("change", () => change(key, input.checked));
        shows.push(() => (input.checked = Boolean(s[key])));
        return checkRow(label, input, title);
      },
      swatch: (get, set, title) => {
        const picker = element("input", { type: "color", className: "hidden-picker" });
        picker.addEventListener("input", () => set(picker.value));
        const swatch = element("button", { type: "button", className: "swatch", title });
        swatch.addEventListener("click", (event) => {
          event.preventDefault();
          picker.click();
        });
        const paint = () => {
          swatch.style.background = get();
          picker.value = get();
        };
        shows.push(paint);
        return [swatch, picker, paint];
      },
      color: (key, label) => {
        const [swatch, picker] = ui.swatch(() => s[key], (value) => change(key, value), label);
        return element("label", { className: "row check" }, element("span", { className: "label", textContent: label }), swatch, picker);
      },
      shuffle: (key, label = "Random seed") => {
        const button = element("button", { type: "button", className: "chip", textContent: "Shuffle" });
        button.addEventListener("click", () => change(key, Math.floor(Math.random() * 65536)));
        button.style.gridColumn = "span 2";
        button.style.justifySelf = "start";
        return labelled(label, button);
      },
      // Shown only while `test()` holds
      when: (node, test) => {
        shows.push(() => (node.hidden = !test()));
        return node;
      },
    };
    const canvas = element("canvas", { className: "preview" });
    canvas.setAttribute("aria-hidden", "true");
    current.preview = canvas;
    const pick = (values) => {
      Object.assign(s, structuredClone(spec.defaults), values(current.unit), { editable: s.editable });
      refreshFilter(current);
      show();
    };
    const presets = spec.presets?.length ? makeChips(spec.presets, pick) : null;
    const sections = spec.panel(ui);
    const editable = element("input", { type: "checkbox" });
    editable.addEventListener("change", () => change("editable", editable.checked));
    const result = element("section", {}, checkRow("Keep editable", editable, `The original kept, hidden, in a group ${spec.name} can change later`));
    const reset = element("button", { type: "button", className: "link", textContent: "Reset" });
    reset.addEventListener("click", () => pick(() => ({})));
    const scroll = element("div", { className: "scroll" }, ...[presets, ...sections].filter(Boolean), result, element("div", { className: "under" }, reset));
    const name = element("div", { className: "source", textContent: current.source.name });
    const cancel = element("button", { type: "button", className: "button", textContent: "Cancel" });
    cancel.addEventListener("click", () => closeFilter(current, false));
    const label = current.group ? "Update" : spec.verb;
    const go = element("button", { type: "button", className: "button primary", textContent: label });
    go.addEventListener("click", () => !current.busy && applyFilter(current));
    const footer = element("div", { className: "footer" }, name, element("div", { className: "actions" }, cancel, go));
    const root = makePanel(current.group ? `${spec.name} · ${plainName(current.source.name)}` : spec.name, () => closeFilter(current, false), canvas, scroll, footer);
    const show = () => {
      for (const fn of shows) fn();
      for (const row of rows) row.show();
      editable.checked = s.editable;
      drawFilterPreview(current);
    };
    show();
    current.root = root;
    current.panel = {
      busy: (on) => {
        go.disabled = on;
        go.textContent = on ? "Making…" : label;
      },
      close: () => closes.forEach((fn) => fn()),
    };
    current.keys = panelKeys(root, () => filtering === current, () => closeFilter(current, false), () => !current.busy && applyFilter(current));
    addEventListener("keydown", current.keys, true);
    go.focus();
  };

  const openFilter = async (spec, figma, edit = false, run = null) => {
    if (repeating || disintegrating || crushing || filtering) return;
    const selected = figma.currentPage.selection;
    if (selected.length !== 1) {
      figma.notify(selected.length ? `Select one layer for ${spec.name}` : `Select a layer for ${spec.name}`, { timeout: 2000 });
      return;
    }
    let group = edit ? filterOf(spec, selected[0]) : null;
    let source;
    let result = null;
    const settings = structuredClone(spec.defaults);
    if (group) {
      source = sourceOf(group);
      result = group.children.find((child) => child.getSharedPluginData(PLUGIN, ROLE) === "result") ?? null;
      try {
        Object.assign(settings, JSON.parse(group.getSharedPluginData(PLUGIN, spec.id)));
      } catch {}
      if (!source) {
        group = null;
        source = selected[0];
      }
    } else {
      source = selected[0];
    }
    if (source.type === "SECTION" || !("exportAsync" in source)) {
      figma.notify("Sections can't take effects", { timeout: 2000 });
      return;
    }
    if (run) Object.assign(settings, structuredClone(run), { editable: true });

    const current = {
      spec,
      figma,
      group,
      source,
      settings,
      unit: Math.max(1, Math.max(source.width ?? 100, source.height ?? 100)),
      result,
      kept: result ? { fills: JSON.stringify(result.fills), width: result.width, height: result.height, transform: result.relativeTransform } : null,
      levels: {},
      wasVisible: source.visible,
      run: Boolean(run),
    };
    filtering = current;
    try {
      figma.commitUndo();
    } catch {}
    // A small picture first, for the panel and the file while it's
    // changed, then the full one, for applying, as Bitcrush reads them
    const reader = pictureReader(figma, source);
    try {
      current.bounds = reader.bounds;
      const side = Math.max(1, current.bounds.width, current.bounds.height);
      const quick = await reader.read(Math.min(2, QUICK / side));
      if (filtering !== current) return reader.done();
      const k = quick.width / current.bounds.width;
      current.levels = { preview: scaledPicture(quick, 360, k), live: scaledPicture(quick, QUICK, k) };
      quick.close?.();
      if (!run) {
        buildFilterPanel(current);
        refreshFilter(current);
      }
      current.bitmap = await reader.read(Math.min(2, MAX_SIDE / 2 / side));
      reader.done();
    } catch {
      reader.done();
      if (filtering === current) {
        current.root?.remove();
        removeEventListener("keydown", current.keys, true);
        stopFilterWorker(current);
        filtering = null;
        figma.notify("Couldn't read the layer", { timeout: 2000 });
      }
      return;
    }
    if (filtering !== current) return;
    current.levels.full = scaledPicture(current.bitmap, MAX_SIDE, current.bitmap.width / current.bounds.width);
    const parent = source.parent;
    if (!group && parent && !("layoutMode" in parent && parent.layoutMode !== "NONE")) {
      current.home = { parent, index: parent.children.indexOf(source) };
      try {
        current.wrap = figma.group([source], parent, current.home.index);
        current.wrap.name = `${spec.name} · ${plainName(source.name)}`;
      } catch {
        current.wrap = null;
      }
    }
    if (!current.result) {
      current.result = figma.createRectangle();
      current.result.name = `${source.name} (${spec.name})`;
      const holder = source.parent;
      holder.insertChild(holder.children.indexOf(source) + 1, current.result);
      if ("layoutMode" in holder && holder.layoutMode !== "NONE") current.result.layoutPositioning = "ABSOLUTE";
      current.result.setSharedPluginData(PLUGIN, ROLE, "result");
      current.result.visible = false;
    }
    figma.currentPage.selection = [current.wrap ?? current.group ?? current.result];
    current.ready = true;
    if (run) return applyFilter(current);
    refreshFilter(current);
  };

  // The panel's preview at once, and the file's a moment after
  const refreshFilter = (current) => {
    if (!current.drawing) {
      current.drawing = true;
      requestAnimationFrame(() => {
        current.drawing = false;
        if (filtering === current) drawFilterPreview(current);
      });
    }
    if (!current.ready) return;
    clearTimeout(current.timer);
    current.timer = setTimeout(async () => {
      if (filtering !== current || current.busy) return;
      if (current.placing) {
        current.again = true;
        return;
      }
      current.placing = true;
      try {
        do {
          current.again = false;
          await placeDrawn(current, await filterAt(current, "live"), current.levels.live.k);
        } while (current.again && !current.busy && filtering === current);
      } catch {}
      current.placing = false;
    }, 40);
  };

  // Closed as Cancel closes it if it fails, rather than left making
  const applyFilter = async (current) => {
    try {
      await finishFilter(current);
    } catch (error) {
      if (filtering === current) closeFilter(current, false);
      if (current.run) throw error;
      current.figma.notify("Couldn't make the effect", { timeout: 2000 });
    }
  };

  const finishFilter = async (current) => {
    const { figma, source, settings, spec } = current;
    clearTimeout(current.timer);
    current.busy = true;
    current.panel?.busy(true);
    while (!current.ready || current.placing) {
      await new Promise((resolve) => setTimeout(resolve, 16));
      if (filtering !== current) return;
    }
    current.again = false;
    await placeDrawn(current, await filterAt(current, "full"), current.levels.full.k);
    if (filtering !== current) return;
    let result = current.group ?? current.wrap;
    if (settings.editable) {
      if (!result || result.removed) {
        const parent = source.parent;
        result = figma.group([source, current.result], parent, parent.children.indexOf(source));
        result.name = `${spec.name} · ${plainName(source.name)}`;
      }
      source.setSharedPluginData(PLUGIN, ROLE, "source");
      result.setSharedPluginData(PLUGIN, spec.id, JSON.stringify(settings));
      result.setSharedPluginData(PLUGIN, OFF, "");
      figma.currentPage.selection = [result];
    } else {
      current.result.setSharedPluginData(PLUGIN, ROLE, "");
      if (result && !result.removed) result.setSharedPluginData(PLUGIN, spec.id, "");
      const image = current.result;
      const holder = current.wrap ?? current.group;
      if (holder && !holder.removed) figma.ungroup(holder);
      if (!source.removed) source.remove();
      image.name = image.name.replace(` (${spec.name})`, "");
      figma.currentPage.selection = [image];
    }
    try {
      figma.commitUndo();
    } catch {}
    closeFilter(current, true);
    if (current.group && settings.editable) restackAbove(figma, current.group);
  };

  const closeFilter = (current, applied) => {
    if (filtering !== current) return;
    filtering = null;
    clearTimeout(current.timer);
    current.closed = true;
    stopFilterWorker(current);
    if (current.loader && !current.loader.removed) current.loader.remove();
    if (!applied) {
      const { figma, source, result, home, wrap } = current;
      if (current.group) {
        try {
          if (current.kept && !result.removed) {
            result.resizeWithoutConstraints(current.kept.width, current.kept.height);
            result.relativeTransform = current.kept.transform;
            result.fills = JSON.parse(current.kept.fills);
            result.visible = true;
          }
          if (!current.group.removed && isOff(current.group)) setOff(current.group, true);
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

  // Sizes in points scale with the layer, `u` its larger side
  const upTo = (u, times, least) => Math.max(least, Math.round(u * times));

  // ---------- Dither ----------

  // Dither Boy's families of dithers: error diffusion (Floyd–Steinberg,
  // Atkinson, Jarvis–Judice–Ninke, Stucki, Burkes and Sierra's three),
  // ordered (Bayer 2, 4 and 8, clustered dots) and threshold and random,
  // on cells of the layer's picture, in a palette
  const DITHER_ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><rect ${STROKE} x="4.5" y="4.5" width="15" height="15" rx="1.5"/><path ${STROKE} d="M12 4.5v15M12 4.5h3.75v3.75H12zM15.75 8.25h3.75V12h-3.75zM12 12h3.75v3.75H12zM15.75 15.75h3.75v3.75h-3.75z"/></svg>`;
  const DITHER_ALGORITHMS = [
    ["floyd", "Floyd–Steinberg"],
    ["atkinson", "Atkinson"],
    ["jarvis", "Jarvis–Judice–Ninke"],
    ["stucki", "Stucki"],
    ["burkes", "Burkes"],
    ["sierra", "Sierra"],
    ["sierra2", "Two-row Sierra"],
    ["sierraLite", "Sierra Lite"],
    ["bayer2", "Bayer 2×2"],
    ["bayer4", "Bayer 4×4"],
    ["bayer8", "Bayer 8×8"],
    ["cluster", "Clustered dots"],
    ["random", "Random"],
    ["threshold", "Threshold"],
  ];
  const DITHER_PALETTES = [
    ["mono", "Black and white"],
    ["duo", "Two colors"],
    ["gray", "Grays"],
    ["green", "Handheld green"],
    ["sepia", "Sepia"],
    ["cga", "Cyan and magenta"],
    ["color", "Posterized color"],
  ];
  pictureEffect({
    id: "dither",
    thumb: { sample: "sunset", settings: { palette: "duo", pixelSize: 2.5 } },
    name: "Dither",
    about: "Error diffusion, Bayer and more, in a palette",
    icon: DITHER_ICON,
    verb: "Dither",
    sharp: true,
    defaults: { algorithm: "floyd", palette: "mono", levels: 4, pixelSize: 2, strength: 100, serpentine: true, brightness: 0, contrast: 100, gamma: 1, ink: "#1a1a2e", paper: "#f4efe1", seed: 7, editable: true },
    presets: [
      ["Classic", () => ({})],
      ["Atkinson", () => ({ algorithm: "atkinson", pixelSize: 1.5, contrast: 115 })],
      ["Bayer", () => ({ algorithm: "bayer4", pixelSize: 3 })],
      ["Handheld", () => ({ algorithm: "bayer4", palette: "green", pixelSize: 3, contrast: 110 })],
      ["Newsprint", () => ({ algorithm: "cluster", palette: "duo", pixelSize: 1.5, ink: "#222222", paper: "#ece6d6" })],
      ["Retro color", () => ({ algorithm: "floyd", palette: "color", levels: 3, pixelSize: 3 })],
      ["CGA", () => ({ algorithm: "bayer8", palette: "cga", pixelSize: 3 })],
    ],
    limits: { pixelSize: [0.25, 400], levels: [2, 16], strength: [0, 200], brightness: [-100, 100], contrast: [0, 400], gamma: [0.1, 5] },
    summary: (s) => `${DITHER_ALGORITHMS.find(([value]) => value === s.algorithm)?.[1] ?? "Dither"} · ${s.pixelSize}pt`,
    panel: (ui) => {
      const { s } = ui;
      const diffusing = () => !/^(bayer|cluster|random|threshold)/.test(s.algorithm);
      const levelled = () => s.palette === "gray" || s.palette === "color";
      return [
        ui.section(
          "Dither",
          ui.select("algorithm", "Algorithm", DITHER_ALGORITHMS),
          ui.sliders([
            ["pixelSize", "Pixel size", "pt", (u) => ({ min: 0.5, max: upTo(u, 0.1, 16), shape: "curve", digits: 1 })],
            ["strength", "Strength", "%", () => ({ min: 0, max: 200, middle: 100 })],
          ]),
          ui.when(ui.check("serpentine", "Serpentine", "Each row the other way from the last, which breaks up the diffusion's worms"), diffusing),
          ui.when(ui.shuffle("seed"), () => s.algorithm === "random"),
        ),
        ui.section(
          "Palette",
          ui.select("palette", "Colors", DITHER_PALETTES),
          ui.when(ui.sliders([["levels", "Levels", "", () => ({ min: 2, max: 16 })]]), levelled),
          ui.when(element("div", {}, ui.color("ink", "Ink"), ui.color("paper", "Paper")), () => s.palette === "duo"),
        ),
        ui.section(
          "Tone",
          ui.sliders([
            ["brightness", "Brightness", "", () => ({ min: -100, max: 100 })],
            ["contrast", "Contrast", "%", () => ({ min: 0, max: 300, middle: 100 })],
            ["gamma", "Gamma", "", () => ({ min: 0.2, max: 3, middle: 1, digits: 2 })],
          ]),
        ),
      ];
    },
    process: (image, s, k) => {
      const { width: W, height: H } = image;
      const f = premultiplied(image);
      const cell = Math.max(1, s.pixelSize * k);
      const { g, gw, gh } = cellGrid(f, W, H, cell);
      const n = gw * gh;
      const hex = (list) => list.map(hexRgb);
      const ramps = { mono: hex(["#000000", "#ffffff"]), duo: hex([s.ink, s.paper]), green: hex(["#0f380f", "#306230", "#8bac0f", "#9bbc0f"]), sepia: hex(["#2b1d0e", "#6b4a2b", "#b08d57", "#f1e3c6"]) };
      const levels = Math.max(2, Math.min(16, Math.round(s.levels)));
      ramps.gray = Array.from({ length: levels }, (_, i) => [i / (levels - 1), i / (levels - 1), i / (levels - 1)]);
      const ramp = ramps[s.palette];
      const colors = s.palette === "cga" ? hex(["#000000", "#55ffff", "#ff55ff", "#ffffff"]) : null;
      const channels = ramp ? 1 : 3;
      // Toned values, luma for a ramp of colors, or red, green and blue
      const v = new Float32Array(n * channels);
      const solid = new Uint8Array(n);
      const [bright, contrast, gamma] = [s.brightness / 100, s.contrast / 100, 1 / Math.max(0.05, s.gamma)];
      const tone = (x) => Math.max(0, (x - 0.5) * contrast + 0.5 + bright) ** gamma;
      for (let i = 0; i < n; i++) {
        const a = g[i * 4 + 3];
        solid[i] = a >= 0.5 ? 1 : 0;
        const [r, gg, b] = a > 0 ? [g[i * 4] / a, g[i * 4 + 1] / a, g[i * 4 + 2] / a] : [0, 0, 0];
        if (ramp) v[i] = tone(lumaOf(r, gg, b));
        else [v[i * 3], v[i * 3 + 1], v[i * 3 + 2]] = [tone(r), tone(gg), tone(b)];
      }
      const steps = ramp ? ramp.length - 1 : colors ? 1 : levels - 1;
      // Quantizes the cell's value, `bias` from the dither added, into
      // `into` the color, giving the error left
      const err = new Float32Array(3);
      const pickColor = (i, bias, into) => {
        if (ramp) {
          const x = v[i] + bias;
          const at = Math.max(0, Math.min(steps, Math.round(x * steps)));
          err[0] = v[i] - at / steps;
          const c = ramp[at];
          into[0] = c[0];
          into[1] = c[1];
          into[2] = c[2];
          return;
        }
        if (colors) {
          let best = 0;
          let least = Infinity;
          for (let c = 0; c < colors.length; c++) {
            const [dr, dg, db] = [v[i * 3] + bias - colors[c][0], v[i * 3 + 1] + bias - colors[c][1], v[i * 3 + 2] + bias - colors[c][2]];
            const d = dr * dr * 0.3 + dg * dg * 0.59 + db * db * 0.11;
            if (d < least) [least, best] = [d, c];
          }
          for (let c = 0; c < 3; c++) {
            into[c] = colors[best][c];
            err[c] = v[i * 3 + c] - into[c];
          }
          return;
        }
        for (let c = 0; c < 3; c++) {
          const at = Math.max(0, Math.min(steps, Math.round((v[i * 3 + c] + bias) * steps)));
          into[c] = at / steps;
          err[c] = v[i * 3 + c] - into[c];
        }
      };
      const out = new Float32Array(n * 3);
      const color = new Float32Array(3);
      const strength = s.strength / 100;
      const KERNELS = {
        floyd: [16, [1, 0, 7], [-1, 1, 3], [0, 1, 5], [1, 1, 1]],
        atkinson: [8, [1, 0, 1], [2, 0, 1], [-1, 1, 1], [0, 1, 1], [1, 1, 1], [0, 2, 1]],
        jarvis: [48, [1, 0, 7], [2, 0, 5], [-2, 1, 3], [-1, 1, 5], [0, 1, 7], [1, 1, 5], [2, 1, 3], [-2, 2, 1], [-1, 2, 3], [0, 2, 5], [1, 2, 3], [2, 2, 1]],
        stucki: [42, [1, 0, 8], [2, 0, 4], [-2, 1, 2], [-1, 1, 4], [0, 1, 8], [1, 1, 4], [2, 1, 2], [-2, 2, 1], [-1, 2, 2], [0, 2, 4], [1, 2, 2], [2, 2, 1]],
        burkes: [32, [1, 0, 8], [2, 0, 4], [-2, 1, 2], [-1, 1, 4], [0, 1, 8], [1, 1, 4], [2, 1, 2]],
        sierra: [32, [1, 0, 5], [2, 0, 3], [-2, 1, 2], [-1, 1, 4], [0, 1, 5], [1, 1, 4], [2, 1, 2], [-1, 2, 2], [0, 2, 3], [1, 2, 2]],
        sierra2: [16, [1, 0, 4], [2, 0, 3], [-2, 1, 1], [-1, 1, 2], [0, 1, 3], [1, 1, 2], [2, 1, 1]],
        sierraLite: [4, [1, 0, 2], [-1, 1, 1], [0, 1, 1]],
      };
      const kernel = KERNELS[s.algorithm];
      if (kernel) {
        const [divisor, ...taps] = kernel;
        for (let y = 0; y < gh; y++) {
          const back = s.serpentine && y % 2 === 1;
          for (let j = 0; j < gw; j++) {
            const x = back ? gw - 1 - j : j;
            const i = y * gw + x;
            if (!solid[i]) continue;
            pickColor(i, 0, color);
            out.set(color, i * 3);
            for (const [dx, dy, w] of taps) {
              const [tx, ty] = [x + (back ? -dx : dx), y + dy];
              if (tx < 0 || tx >= gw || ty >= gh) continue;
              const t = ty * gw + tx;
              const share = (w / divisor) * strength;
              for (let c = 0; c < channels; c++) v[t * channels + c] += err[c] * share;
            }
          }
        }
      } else {
        // An ordered matrix, its thresholds 0 to 1
        const bayer = (size) => {
          let m = [0];
          let side = 1;
          while (side < size) {
            const next = new Array(4 * side * side);
            for (let y = 0; y < side; y++)
              for (let x = 0; x < side; x++) {
                const at = m[y * side + x] * 4;
                next[y * 2 * side + x] = at;
                next[y * 2 * side + x + side] = at + 2;
                next[(y + side) * 2 * side + x] = at + 3;
                next[(y + side) * 2 * side + x + side] = at + 1;
              }
            m = next;
            side *= 2;
          }
          return { m: m.map((value) => (value + 0.5) / (side * side)), side };
        };
        const matrix = s.algorithm === "bayer2" ? bayer(2) : s.algorithm === "bayer8" ? bayer(8) : s.algorithm === "cluster" ? { m: [12, 5, 6, 13, 4, 0, 1, 7, 11, 3, 2, 8, 15, 10, 9, 14].map((value) => (value + 0.5) / 16), side: 4 } : bayer(4);
        const spread = (strength / steps) * (colors ? 1.5 : 1);
        for (let y = 0; y < gh; y++)
          for (let x = 0; x < gw; x++) {
            const i = y * gw + x;
            if (!solid[i]) continue;
            let bias = 0;
            if (s.algorithm === "random") bias = (hash2(x, y, s.seed) - 0.5) * spread;
            else if (s.algorithm !== "threshold") bias = (matrix.m[(y % matrix.side) * matrix.side + (x % matrix.side)] - 0.5) * spread;
            pickColor(i, bias, color);
            out.set(color, i * 3);
          }
      }
      const result = new ImageData(W, H);
      const o = result.data;
      for (let y = 0; y < H; y++) {
        const gy = Math.min(gh - 1, Math.floor(y / cell));
        for (let x = 0; x < W; x++) {
          const i = gy * gw + Math.min(gw - 1, Math.floor(x / cell));
          if (!solid[i]) continue;
          const at = (y * W + x) * 4;
          o[at] = Math.round(out[i * 3] * 255);
          o[at + 1] = Math.round(out[i * 3 + 1] * 255);
          o[at + 2] = Math.round(out[i * 3 + 2] * 255);
          o[at + 3] = 255;
        }
      }
      return { image: result, x: 0, y: 0 };
    },
  });

  // ---------- Halftone ----------

  // A print screen: dots, lines, squares, diamonds or crosses on a grid
  // turned to an angle, each as large as the layer's dark there, in one
  // ink or in four, cyan, magenta, yellow and black, each on its own angle
  const HALFTONE_ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><circle ${STROKE} cx="7.5" cy="7.5" r="3"/><circle ${STROKE} cx="16.5" cy="7.5" r="2"/><circle ${STROKE} cx="7.5" cy="16.5" r="2"/><circle ${STROKE} cx="16.5" cy="16.5" r="1"/></svg>`;
  // How much of a cell `fu`, `fv` from its middle (−0.5 to 0.5) is inked
  // for `value` of it, its edge `e` pixels soft in a cell `c` across
  const halftoneCoverage = (shape, fu, fv, value, c, e) => {
    if (value <= 0.002) return 0;
    if (value >= 0.998) return 1;
    let d;
    let r;
    const [au, av] = [Math.abs(fu), Math.abs(fv)];
    if (shape === "line") [d, r] = [av, value / 2];
    else if (shape === "square") [d, r] = [Math.max(au, av), Math.sqrt(value) / 2];
    else if (shape === "diamond") [d, r] = [au + av, value <= 0.5 ? Math.sqrt(value / 2) : 1 - Math.sqrt((1 - value) / 2)];
    else if (shape === "cross") [d, r] = [Math.min(au, av), (1 - Math.sqrt(1 - value)) / 2];
    else [d, r] = [Math.hypot(fu, fv), Math.min(0.7072, Math.sqrt(value / Math.PI))];
    const cover = clamp01(((r - d) * c) / e + 0.5);
    return cover * clamp01((r * c * 2) / e);
  };
  PIXEL_HELPERS.halftoneCoverage = halftoneCoverage;
  const HALFTONE_SHAPES = [
    ["dot", "Dots"],
    ["line", "Lines"],
    ["square", "Squares"],
    ["diamond", "Diamonds"],
    ["cross", "Crosses"],
  ];
  pictureEffect({
    id: "halftone",
    thumb: { sample: "sunset", settings: { mode: "cmyk", cell: 6 } },
    name: "Halftone",
    about: "Print it in dots, lines or four inks",
    icon: HALFTONE_ICON,
    verb: "Halftone",
    defaults: { mode: "mono", shape: "dot", cell: 6, angle: 45, size: 100, softness: 30, brightness: 0, contrast: 100, ink: "#111111", paper: "#ffffff", showPaper: true, editable: true },
    presets: [
      ["Dots", () => ({})],
      ["Comic", () => ({ mode: "cmyk", cell: 5, angle: 15, contrast: 120 })],
      ["Lines", () => ({ shape: "line", cell: 4, angle: 30 })],
      ["Newspaper", () => ({ cell: 3, angle: 45, ink: "#222222", paper: "#efe9db", contrast: 110 })],
      ["Pop", () => ({ cell: 12, angle: 0, ink: "#e4002b", paper: "#ffe14d", contrast: 130 })],
      ["Crosses", () => ({ shape: "cross", cell: 8, angle: 0 })],
    ],
    limits: { cell: [1, 1000], angle: [-360, 360], size: [0, 400], softness: [0, 100], brightness: [-100, 100], contrast: [0, 400] },
    summary: (s) => `${s.mode === "cmyk" ? "CMYK" : HALFTONE_SHAPES.find(([value]) => value === s.shape)?.[1]} · ${s.cell}pt`,
    panel: (ui) => [
      ui.section(
        "Screen",
        ui.segments("mode", "Inks", [
          ["mono", "One ink"],
          ["cmyk", "CMYK", "Cyan, magenta, yellow and black, each on its own angle"],
        ]),
        ui.select("shape", "Shape", HALFTONE_SHAPES),
        ui.sliders([
          ["cell", "Cell size", "pt", (u) => ({ min: 1, max: upTo(u, 0.15, 24), shape: "curve", digits: 1 })],
          ["angle", "Angle", "°", () => ({ min: -90, max: 90 })],
          ["size", "Dot size", "%", () => ({ min: 0, max: 200, middle: 100 })],
          ["softness", "Softness", "%", () => ({ min: 0, max: 100 })],
        ]),
      ),
      ui.section("Colors", ui.when(ui.color("ink", "Ink"), () => ui.s.mode === "mono"), ui.color("paper", "Paper"), ui.check("showPaper", "Show paper", "Paper under the ink, or only the ink, the rest left clear")),
      ui.section(
        "Tone",
        ui.sliders([
          ["brightness", "Brightness", "", () => ({ min: -100, max: 100 })],
          ["contrast", "Contrast", "%", () => ({ min: 0, max: 300, middle: 100 })],
        ]),
      ),
    ],
    process: (image, s, k) => {
      const { width: W, height: H } = image;
      const f = premultiplied(image);
      const cell = Math.max(1.5, s.cell * k);
      const grid = cellGrid(f, W, H, Math.max(1, cell / 2));
      const [ink, paper] = [hexRgb(s.ink), hexRgb(s.paper)];
      const [bright, contrast, gain] = [s.brightness / 100, s.contrast / 100, s.size / 100];
      const tone = (v) => clamp01((v - 0.5) * contrast + 0.5 + bright);
      const cmyk = s.mode === "cmyk";
      const screens = (cmyk ? [15, 75, 0, 45] : [0]).map((turn) => {
        const a = ((s.angle + turn) * Math.PI) / 180;
        return [Math.cos(a), Math.sin(a)];
      });
      const e = Math.max(0.6, (s.softness / 100) * cell * 0.5);
      const out = new Float32Array(W * H * 4);
      const at4 = new Float32Array(4);
      const cover = new Float32Array(4);
      for (let y = 0; y < H; y++)
        for (let x = 0; x < W; x++) {
          const at = (y * W + x) * 4;
          const srcA = f[at + 3];
          if (srcA <= 0) continue;
          const [px, py] = [x + 0.5, y + 0.5];
          for (let j = 0; j < screens.length; j++) {
            const [cos, sin] = screens[j];
            const u = (px * cos + py * sin) / cell;
            const v = (-px * sin + py * cos) / cell;
            const [cu, cv] = [Math.floor(u) + 0.5, Math.floor(v) + 0.5];
            gridAt(grid, (cu * cos - cv * sin) * cell, (cu * sin + cv * cos) * cell, at4);
            const a = at4[3];
            let value = 0;
            if (a > 0.001) {
              const [r, g, b] = [tone(at4[0] / a), tone(at4[1] / a), tone(at4[2] / a)];
              if (!cmyk) value = 1 - lumaOf(r, g, b);
              else {
                const black = 1 - Math.max(r, g, b);
                const rest = 1 - black;
                value = j === 3 ? black : rest < 0.0001 ? 0 : (1 - [r, g, b][j] - black) / rest;
              }
            }
            cover[j] = halftoneCoverage(s.shape, u - cu, v - cv, clamp01(value * gain), cell, e);
          }
          let [r, g, b, alpha] = [0, 0, 0, srcA];
          if (!cmyk) {
            const c = cover[0];
            if (s.showPaper) [r, g, b] = [paper[0] + (ink[0] - paper[0]) * c, paper[1] + (ink[1] - paper[1]) * c, paper[2] + (ink[2] - paper[2]) * c];
            else [r, g, b, alpha] = [ink[0], ink[1], ink[2], srcA * c];
          } else {
            const black = 1 - cover[3];
            const [cr, cg, cb] = [(1 - cover[0]) * black, (1 - cover[1]) * black, (1 - cover[2]) * black];
            if (s.showPaper) [r, g, b] = [paper[0] * cr, paper[1] * cg, paper[2] * cb];
            else {
              // The inks over white, with what of white shows taken out
              const union = 1 - (1 - cover[0]) * (1 - cover[1]) * (1 - cover[2]) * black;
              const clear = 1 - union;
              [r, g, b] = union > 0.001 ? [clamp01((cr - clear) / union), clamp01((cg - clear) / union), clamp01((cb - clear) / union)] : [0, 0, 0];
              alpha = srcA * union;
            }
          }
          out[at] = r * alpha;
          out[at + 1] = g * alpha;
          out[at + 2] = b * alpha;
          out[at + 3] = alpha;
        }
      return { image: toImage(out, W, H), x: 0, y: 0 };
    },
  });

  // ---------- Modulate ----------

  // Dither Boy's modulation dithers: the layer drawn in lines that thicken
  // where it's dark, waves that swing wider there, or rings
  const MODULATE_ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><path ${STROKE} d="M4.5 7.5c1.25 0 1.25-1 2.5-1s1.25 1 2.5 1 1.25-1 2.5-1 1.25 1 2.5 1 1.25-1 2.5-1 1.25 1 2.5 1M4.5 12c1.25 0 1.25-2 2.5-2s1.25 2 2.5 2 1.25-2 2.5-2 1.25 2 2.5 2 1.25-2 2.5-2 1.25 2 2.5 2M4.5 17.5c1.25 0 1.25-3 2.5-3s1.25 3 2.5 3 1.25-3 2.5-3 1.25 3 2.5 3 1.25-3 2.5-3 1.25 3 2.5 3"/></svg>`;
  pictureEffect({
    id: "modulate",
    thumb: { sample: "sunset", settings: { mode: "waves", spacing: 9 } },
    name: "Modulate",
    about: "Draw it in lines, waves or rings",
    icon: MODULATE_ICON,
    verb: "Modulate",
    defaults: { mode: "lines", spacing: 5, angle: 0, weight: 90, amplitude: 60, frequency: 0.5, centerX: 50, centerY: 50, softness: 30, brightness: 0, contrast: 100, ink: "#111111", paper: "#ffffff", showPaper: true, editable: true },
    presets: [
      ["Lines", () => ({})],
      ["Engraving", () => ({ spacing: 3, angle: 30, weight: 100, contrast: 120 })],
      ["Waves", () => ({ mode: "waves", spacing: 6, amplitude: 70, frequency: 0.6, weight: 70 })],
      ["Signal", () => ({ mode: "waves", spacing: 4, amplitude: 90, frequency: 1.4, weight: 50, ink: "#39ff7a", paper: "#06140b" })],
      ["Rings", () => ({ mode: "rings", spacing: 5 })],
    ],
    limits: { spacing: [1, 1000], angle: [-360, 360], weight: [1, 200], amplitude: [0, 400], frequency: [0.02, 20], centerX: [-200, 300], centerY: [-200, 300], softness: [0, 100], brightness: [-100, 100], contrast: [0, 400] },
    summary: (s) => `${s.mode === "waves" ? "Waves" : s.mode === "rings" ? "Rings" : "Lines"} · ${s.spacing}pt`,
    panel: (ui) => {
      const { s } = ui;
      return [
        ui.section(
          "Lines",
          ui.segments("mode", "Draw as", [
            ["lines", "Lines"],
            ["waves", "Waves"],
            ["rings", "Rings"],
          ]),
          ui.sliders([
            ["spacing", "Spacing", "pt", (u) => ({ min: 1, max: upTo(u, 0.15, 24), shape: "curve", digits: 1 })],
            ["weight", "Weight", "%", () => ({ min: 5, max: 150, middle: 90 })],
            ["softness", "Softness", "%", () => ({ min: 0, max: 100 })],
          ]),
          ui.when(ui.sliders([["angle", "Angle", "°", () => ({ min: -90, max: 90 })]]), () => s.mode !== "rings"),
          ui.when(
            ui.sliders([
              ["amplitude", "Amplitude", "%", () => ({ min: 0, max: 200 })],
              ["frequency", "Frequency", "", () => ({ min: 0.05, max: 4, shape: "curve", digits: 2 })],
            ]),
            () => s.mode === "waves",
          ),
          ui.when(
            ui.sliders([
              ["centerX", "Center x", "%", () => ({ min: 0, max: 100 })],
              ["centerY", "Center y", "%", () => ({ min: 0, max: 100 })],
            ]),
            () => s.mode === "rings",
          ),
        ),
        ui.section("Colors", ui.color("ink", "Ink"), ui.color("paper", "Paper"), ui.check("showPaper", "Show paper", "Paper under the lines, or only the lines, the rest left clear")),
        ui.section(
          "Tone",
          ui.sliders([
            ["brightness", "Brightness", "", () => ({ min: -100, max: 100 })],
            ["contrast", "Contrast", "%", () => ({ min: 0, max: 300, middle: 100 })],
          ]),
        ),
      ];
    },
    process: (image, s, k) => {
      const { width: W, height: H } = image;
      const f = premultiplied(image);
      const spacing = Math.max(1.5, s.spacing * k);
      const grid = cellGrid(f, W, H, Math.max(1, spacing / 2));
      const [ink, paper] = [hexRgb(s.ink), hexRgb(s.paper)];
      const [bright, contrast, weight] = [s.brightness / 100, s.contrast / 100, s.weight / 100];
      const tone = (v) => clamp01((v - 0.5) * contrast + 0.5 + bright);
      const turn = (s.angle * Math.PI) / 180;
      const [cos, sin] = [Math.cos(turn), Math.sin(turn)];
      const [cx, cy] = [(W * s.centerX) / 100, (H * s.centerY) / 100];
      const e = Math.max(0.6, (s.softness / 100) * spacing * 0.5);
      const [amp, freq] = [s.amplitude / 100, s.frequency];
      const out = new Float32Array(W * H * 4);
      const at4 = new Float32Array(4);
      const line = (d, half) => (half <= 0 ? 0 : clamp01(((half - d) * spacing) / e + 0.5) * clamp01((half * spacing * 4) / e));
      for (let y = 0; y < H; y++)
        for (let x = 0; x < W; x++) {
          const at = (y * W + x) * 4;
          const srcA = f[at + 3];
          if (srcA <= 0) continue;
          const [px, py] = [x + 0.5, y + 0.5];
          gridAt(grid, px, py, at4);
          const a = at4[3];
          const dark = a > 0.001 ? 1 - lumaOf(tone(at4[0] / a), tone(at4[1] / a), tone(at4[2] / a)) : 0;
          let c = 0;
          if (s.mode === "rings") {
            const r = Math.hypot(px - cx, py - cy) / spacing;
            c = line(Math.abs(r - Math.round(r)), (dark * weight) / 2);
          } else {
            const u = (px * cos + py * sin) / spacing;
            const v = (-px * sin + py * cos) / spacing;
            if (s.mode === "lines") c = line(Math.abs(v - Math.round(v)), (dark * weight) / 2);
            else {
              const phase = 2 * Math.PI * u * freq;
              const swing = amp * dark * 0.5;
              const offset = swing * Math.sin(phase);
              const slope = swing * 2 * Math.PI * freq * Math.cos(phase);
              const half = (weight * (0.2 + 0.8 * dark) * 0.25) / Math.sqrt(1 + slope * slope);
              for (let i = Math.round(v) - 1; i <= Math.round(v) + 1; i++) c = Math.max(c, line(Math.abs(v - i - offset), half));
            }
          }
          let [r, g, b, alpha] = [ink[0], ink[1], ink[2], srcA * c];
          if (s.showPaper) [r, g, b, alpha] = [paper[0] + (ink[0] - paper[0]) * c, paper[1] + (ink[1] - paper[1]) * c, paper[2] + (ink[2] - paper[2]) * c, srcA];
          out[at] = r * alpha;
          out[at + 1] = g * alpha;
          out[at + 2] = b * alpha;
          out[at + 3] = alpha;
        }
      return { image: toImage(out, W, H), x: 0, y: 0 };
    },
  });

  // ---------- Chromatic Aberration ----------

  // A lens's color fringes: red, green and blue drawn apart, out from a
  // center, more toward the edges, as a lens does, or all one way
  const CHROMATIC_ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><circle ${STROKE} cx="10" cy="12" r="5.5"/><circle ${STROKE} cx="14" cy="12" r="5.5"/></svg>`;
  const FRINGES = [
    ["rb", "Red and blue"],
    ["rc", "Red and cyan"],
    ["gm", "Green and magenta"],
    ["spectrum", "Spectrum"],
  ];
  pictureEffect({
    id: "chromatic",
    thumb: { sample: "rings", settings: { amount: 10 } },
    name: "Chromatic Aberration",
    about: "Split its colors as a lens does",
    icon: CHROMATIC_ICON,
    verb: "Split",
    defaults: { mode: "lens", fringe: "rb", amount: 4, angle: 0, centerX: 50, centerY: 50, falloff: 1.5, editable: true },
    presets: [
      ["Lens", () => ({})],
      ["Subtle", () => ({ amount: 1.5 })],
      ["Strong", (u) => ({ amount: upTo(u, 0.03, 8), falloff: 1 })],
      ["Shift", () => ({ mode: "shift", amount: 3, fringe: "rc" })],
      ["Spectrum", (u) => ({ fringe: "spectrum", amount: upTo(u, 0.025, 6) })],
    ],
    limits: { amount: [0, 2000], angle: [-360, 360], centerX: [-200, 300], centerY: [-200, 300], falloff: [0.2, 6] },
    summary: (s) => `${s.mode === "shift" ? "Shift" : "Lens"} · ${s.amount}pt`,
    panel: (ui) => {
      const { s } = ui;
      return [
        ui.section(
          "Aberration",
          ui.segments("mode", "Split", [
            ["lens", "From a center", "Out from a center, more toward the edges, as a lens"],
            ["shift", "One way"],
          ]),
          ui.select("fringe", "Colors", FRINGES),
          ui.sliders([["amount", "Amount", "pt", (u) => ({ min: 0, max: upTo(u, 0.08, 20), shape: "curve", digits: 1 })]]),
          ui.when(ui.sliders([["angle", "Angle", "°", () => ({ min: -180, max: 180 })]]), () => s.mode === "shift"),
          ui.when(
            ui.sliders([
              ["falloff", "Falloff", "", () => ({ min: 0.2, max: 4, middle: 1, digits: 2 })],
              ["centerX", "Center x", "%", () => ({ min: 0, max: 100 })],
              ["centerY", "Center y", "%", () => ({ min: 0, max: 100 })],
            ]),
            () => s.mode === "lens",
          ),
        ),
      ];
    },
    process: (image, s, k) => {
      const { width: W, height: H } = image;
      const f = premultiplied(image);
      const amount = s.amount * k;
      const pad = Math.ceil(amount) + 2;
      const [OW, OH] = [W + pad * 2, H + pad * 2];
      // Each sample a way along the split, -1 to 1, and how much of red,
      // green and blue it gives
      let samples = {
        rb: [[1, [1, 0, 0]], [0, [0, 1, 0]], [-1, [0, 0, 1]]],
        rc: [[1, [1, 0, 0]], [-1, [0, 1, 1]]],
        gm: [[1, [0, 1, 0]], [-1, [1, 0, 1]]],
      }[s.fringe];
      if (!samples) {
        const n = 9;
        samples = Array.from({ length: n }, (_, i) => {
          const hue = (i / (n - 1)) * 4;
          const rgb = [clamp01(Math.abs(hue - 3) - 1), clamp01(2 - Math.abs(hue - 2)), clamp01(2 - Math.abs(hue - 4))];
          return [1 - (2 * i) / (n - 1), rgb];
        });
        const sums = [0, 1, 2].map((c) => samples.reduce((sum, [, w]) => sum + w[c], 0));
        samples = samples.map(([t, w]) => [t, w.map((v, c) => v / (sums[c] || 1))]);
      }
      const [cx, cy] = [(W * s.centerX) / 100, (H * s.centerY) / 100];
      const R = Math.max(1, Math.hypot(Math.max(cx, W - cx), Math.max(cy, H - cy)));
      const turn = (s.angle * Math.PI) / 180;
      const [sx, sy] = [Math.cos(turn) * amount, Math.sin(turn) * amount];
      const out = new Float32Array(OW * OH * 4);
      const p = new Float32Array(4);
      for (let y = 0; y < OH; y++)
        for (let x = 0; x < OW; x++) {
          const [X, Y] = [x + 0.5 - pad, y + 0.5 - pad];
          let [vx, vy] = [sx, sy];
          if (s.mode !== "shift") {
            const [rx, ry] = [(X - cx) / R, (Y - cy) / R];
            const r = Math.hypot(rx, ry);
            const m = r > 0 ? (amount * r ** s.falloff) / r : 0;
            [vx, vy] = [rx * m, ry * m];
          }
          let [cr, cg, cb, ar, ag, ab] = [0, 0, 0, 0, 0, 0];
          for (const [t, w] of samples) {
            sampleAt(f, W, H, X - t * vx, Y - t * vy, p);
            cr += w[0] * p[0];
            cg += w[1] * p[1];
            cb += w[2] * p[2];
            ar += w[0] * p[3];
            ag += w[1] * p[3];
            ab += w[2] * p[3];
          }
          const at = (y * OW + x) * 4;
          out[at] = cr;
          out[at + 1] = cg;
          out[at + 2] = cb;
          out[at + 3] = Math.max(ar, ag, ab);
        }
      return { image: toImage(out, OW, OH), x: -pad, y: -pad };
    },
  });

  // ---------- Fisheye ----------

  // A fisheye lens's bulge, or a pinch, within a circle around a center,
  // its edge left where it is, and the circle cut out as a lens's picture
  const FISHEYE_ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><circle ${STROKE} cx="12" cy="12" r="7.5"/><path ${STROKE} d="M12 4.5c-2.5 2-2.5 13 0 15M12 4.5c2.5 2 2.5 13 0 15M4.5 12c2-2.5 13-2.5 15 0M4.5 12c2 2.5 13 2.5 15 0"/></svg>`;
  pictureEffect({
    id: "fisheye",
    thumb: { sample: "checks", settings: { strength: 80 } },
    name: "Fisheye",
    about: "Bulge or pinch it as a lens",
    icon: FISHEYE_ICON,
    verb: "Warp",
    defaults: { strength: 60, radius: 100, zoom: 100, centerX: 50, centerY: 50, circle: false, editable: true },
    presets: [
      ["Fisheye", () => ({})],
      ["Lens circle", () => ({ strength: 80, circle: true })],
      ["Bulge", () => ({ strength: 45, radius: 55 })],
      ["Pinch", () => ({ strength: -60, radius: 70 })],
      ["Peephole", () => ({ strength: 95, circle: true, zoom: 115 })],
    ],
    limits: { strength: [-100, 100], radius: [1, 1000], zoom: [10, 1000], centerX: [-200, 300], centerY: [-200, 300] },
    summary: (s) => `${s.strength < 0 ? "Pinch" : "Bulge"} · ${Math.abs(s.strength)}%`,
    panel: (ui) => [
      ui.section(
        "Lens",
        ui.sliders([
          ["strength", "Strength", "%", () => ({ min: -100, max: 100 })],
          ["radius", "Radius", "%", () => ({ min: 5, max: 200, middle: 100 })],
          ["zoom", "Zoom", "%", () => ({ min: 25, max: 400, middle: 100 })],
          ["centerX", "Center x", "%", () => ({ min: 0, max: 100 })],
          ["centerY", "Center y", "%", () => ({ min: 0, max: 100 })],
        ]),
        ui.check("circle", "Lens circle", "Only what's in the lens's circle, the rest left clear"),
      ),
    ],
    process: (image, s) => {
      const { width: W, height: H } = image;
      const f = premultiplied(image);
      const [cx, cy] = [(W * s.centerX) / 100, (H * s.centerY) / 100];
      const R = Math.max(1, ((s.radius / 100) * Math.max(W, H)) / 2);
      const zoom = Math.max(0.05, s.zoom / 100);
      // As a fisheye maps angles to a flat picture: tan for a bulge, its
      // inverse for a pinch, the circle's edge kept where it is
      const theta = (Math.abs(s.strength) / 100) * 1.45;
      const map = (r) => (theta < 0.001 ? r : s.strength > 0 ? Math.tan(r * theta) / Math.tan(theta) : Math.atan(r * Math.tan(theta)) / theta);
      const out = new Float32Array(W * H * 4);
      const p = new Float32Array(4);
      for (let y = 0; y < H; y++)
        for (let x = 0; x < W; x++) {
          const [dx, dy] = [(x + 0.5 - cx) / R, (y + 0.5 - cy) / R];
          const r = Math.hypot(dx, dy);
          if (s.circle && r > 1) continue;
          const m = r > 0 && r < 1 ? map(r) / r : 1;
          sampleAt(f, W, H, cx + (dx * m * R) / zoom, cy + (dy * m * R) / zoom, p);
          // The circle's edge a pixel soft
          const edge = s.circle ? clamp01((1 - r) * R + 0.5) : 1;
          out.set([p[0] * edge, p[1] * edge, p[2] * edge, p[3] * edge], (y * W + x) * 4);
        }
      return { image: toImage(out, W, H), x: 0, y: 0 };
    },
  });

  // ---------- Liquify ----------

  // Photoshop's distort filters: Twirl turns it more toward the center,
  // Pinch squeezes it in or puffs it out, Ripple sends rings out from the
  // center as on a pond, Zigzag twists those rings back and forth, all
  // within a radius, and Wave moves it across waves running one way. A
  // photo keeps its bounds, its edges repeated; a cutout grows to fit
  const LIQUIFY_ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><path ${STROKE} d="M12 12.5a1 1 0 1 1 1-1.5 2.5 2.5 0 0 1-2.5 3.5A3.5 3.5 0 0 1 7 11a5 5 0 0 1 5-5 6 6 0 0 1 6 6 7 7 0 0 1-7 7"/></svg>`;
  const LIQUIFY_MODES = [
    ["twirl", "Twirl"],
    ["pinch", "Pinch"],
    ["ripple", "Ripple"],
    ["zigzag", "Zigzag"],
    ["wave", "Wave"],
  ];
  pictureEffect({
    id: "liquify",
    thumb: { sample: "checks", settings: { twirl: 240 } },
    name: "Liquify",
    about: "Twirl, pinch, ripple or wave it",
    icon: LIQUIFY_ICON,
    verb: "Warp",
    defaults: { mode: "twirl", twirl: 180, pinch: 50, amplitude: 6, wavelength: 24, angle: 0, radius: 100, centerX: 50, centerY: 50, editable: true },
    presets: [
      ["Twirl", () => ({})],
      ["Whirlpool", () => ({ twirl: 540, radius: 80 })],
      ["Pinch", () => ({ mode: "pinch", pinch: 60 })],
      ["Puff", () => ({ mode: "pinch", pinch: -60 })],
      ["Pond", () => ({ mode: "ripple", amplitude: 4, wavelength: 16 })],
      ["Zigzag", () => ({ mode: "zigzag", amplitude: 10, wavelength: 20 })],
      ["Wave", () => ({ mode: "wave", amplitude: 8, wavelength: 60 })],
    ],
    limits: { twirl: [-3600, 3600], pinch: [-100, 100], amplitude: [0, 1000], wavelength: [1, 4000], angle: [-360, 360], radius: [1, 1000], centerX: [-200, 300], centerY: [-200, 300] },
    summary: (s) =>
      ({
        twirl: `Twirl · ${s.twirl}°`,
        pinch: `${s.pinch < 0 ? "Puff" : "Pinch"} · ${Math.abs(s.pinch)}%`,
        ripple: `Ripple · ${s.amplitude}pt`,
        zigzag: `Zigzag · ${s.amplitude}pt`,
        wave: `Wave · ${s.amplitude}pt`,
      })[s.mode] ?? "",
    panel: (ui) => {
      const { s } = ui;
      const waving = () => ["ripple", "zigzag", "wave"].includes(s.mode);
      return [
        ui.section(
          "Warp",
          ui.select("mode", "Warp", LIQUIFY_MODES),
          ui.when(ui.sliders([["twirl", "Angle", "°", () => ({ min: -720, max: 720 })]]), () => s.mode === "twirl"),
          ui.when(ui.sliders([["pinch", "Amount", "%", () => ({ min: -100, max: 100 })]]), () => s.mode === "pinch"),
          ui.when(
            ui.sliders([
              ["amplitude", "Amplitude", "pt", (u) => ({ min: 0, max: upTo(u, 0.1, 16), shape: "curve", digits: 1 })],
              ["wavelength", "Wavelength", "pt", (u) => ({ min: 2, max: upTo(u, 0.5, 64), shape: "curve" })],
            ]),
            waving,
          ),
          ui.when(ui.sliders([["angle", "Angle", "°", () => ({ min: -180, max: 180 })]]), () => s.mode === "wave"),
        ),
        ui.when(
          ui.section(
            "Area",
            ui.sliders([
              ["radius", "Radius", "%", () => ({ min: 5, max: 200, middle: 100 })],
              ["centerX", "Center x", "%", () => ({ min: 0, max: 100 })],
              ["centerY", "Center y", "%", () => ({ min: 0, max: 100 })],
            ]),
          ),
          () => s.mode !== "wave",
        ),
      ];
    },
    process: (image, s, k) => {
      const { width: W, height: H } = image;
      const f = premultiplied(image);
      const amp = s.amplitude * k;
      const opaque = isOpaque(f);
      const sample = opaque ? edgeAt : sampleAt;
      const pad = s.mode === "wave" && !opaque ? Math.ceil(amp) + 1 : 0;
      const [OW, OH] = [W + 2 * pad, H + 2 * pad];
      const [cx, cy] = [(W * s.centerX) / 100, (H * s.centerY) / 100];
      const R = Math.max(1, ((s.radius / 100) * Math.max(W, H)) / 2);
      const wave = Math.max(1, s.wavelength * k);
      const [ax, ay] = [Math.cos((s.angle * Math.PI) / 180), Math.sin((s.angle * Math.PI) / 180)];
      const twirl = (s.twirl * Math.PI) / 180;
      const pinch = s.pinch / 100;
      const power = pinch >= 0 ? 1 - 0.7 * pinch : 1 - 1.5 * pinch;
      const out = new Float32Array(OW * OH * 4);
      const p = new Float32Array(4);
      for (let y = 0; y < OH; y++)
        for (let x = 0; x < OW; x++) {
          let [X, Y] = [x - pad + 0.5, y - pad + 0.5];
          if (s.mode === "wave") {
            // Moved across the waves by a sine along them
            const shift = amp * Math.sin((2 * Math.PI * (X * ax + Y * ay)) / wave);
            X += ay * shift;
            Y -= ax * shift;
          } else {
            const [dx, dy] = [X - cx, Y - cy];
            const r = Math.hypot(dx, dy);
            if (r > 0 && r < R) {
              // Strongest at the center, nothing at the radius
              const t = r / R;
              const fall = (1 - t) * (1 - t);
              let [reach, turn] = [r, 0];
              if (s.mode === "twirl") turn = twirl * fall;
              else if (s.mode === "pinch") reach = R * t ** power;
              else if (s.mode === "ripple") reach = r + amp * Math.sin((2 * Math.PI * r) / wave) * fall;
              else turn = (amp / r) * Math.sin((2 * Math.PI * r) / wave) * fall;
              const angle = Math.atan2(dy, dx) + turn;
              [X, Y] = [cx + Math.cos(angle) * reach, cy + Math.sin(angle) * reach];
            }
          }
          out.set(sample(f, W, H, X, Y, p), (y * OW + x) * 4);
        }
      return { image: toImage(out, OW, OH), x: -pad, y: -pad };
    },
  });

  // ---------- Displace ----------

  // Each pixel taken from a little way off, as far and which way a map
  // says: smooth noise, rougher turbulence, or the layer's own brightness,
  // which pushes light parts one way and dark the other. Split moves red
  // and blue more and less than green, for a prism's fringes. A photo
  // keeps its bounds, its edges repeated; a cutout grows to fit
  const DISPLACE_ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><path ${STROKE} d="M4.5 8c2.5-2 5 2 7.5 0s5-2 7.5 0M4.5 12c2.5-2 5 2 7.5 0s5-2 7.5 0M4.5 16c2.5-2 5 2 7.5 0s5-2 7.5 0"/></svg>`;
  // Smooth noise in octaves, about −1 to 1, folded into turbulence's
  // ridges with `turbulent`
  const noiseAt = (x, y, seed, octaves, turbulent) => {
    let v = 0;
    let amp = 1;
    let total = 0;
    for (let o = 0; o < octaves; o++) {
      const n = valueNoise(x, y, seed + o * 131) * 2 - 1;
      v += (turbulent ? 1 - 2 * Math.abs(n) : n) * amp;
      total += amp;
      [x, y] = [x * 2.03 + 11.7, y * 2.03 + 5.3];
      amp *= 0.5;
    }
    return v / total;
  };
  Object.assign(PIXEL_HELPERS, { noiseAt });
  pictureEffect({
    id: "displace",
    thumb: { sample: "stripes", settings: { amount: 9, scale: 36, detail: 3 } },
    name: "Displace",
    about: "Push its pixels around with noise or its own tones",
    icon: DISPLACE_ICON,
    verb: "Displace",
    defaults: { map: "noise", amount: 8, scale: 40, detail: 2, stretch: 0, axis: "both", angle: 0, smooth: 4, split: 0, seed: 1, editable: true },
    presets: [
      ["Wobble", () => ({})],
      ["Heat haze", () => ({ amount: 3, scale: 12, detail: 3, stretch: -70, axis: "x" })],
      ["Turbulence", () => ({ map: "turbulence", amount: 14, scale: 60, detail: 5 })],
      ["Smear", () => ({ map: "self", amount: 12, smooth: 6 })],
      ["Prism", () => ({ amount: 6, scale: 30, detail: 3, split: 60 })],
    ],
    limits: { amount: [0, 1000], scale: [1, 4000], detail: [1, 6], stretch: [-100, 100], angle: [-360, 360], smooth: [0, 1000], split: [0, 100] },
    summary: (s) => `${{ noise: "Noise", turbulence: "Turbulence", self: "Its tones" }[s.map] ?? ""} · ${s.amount}pt`,
    panel: (ui) => {
      const { s } = ui;
      const noisy = () => s.map !== "self";
      return [
        ui.section(
          "Map",
          ui.segments("map", "Map", [
            ["noise", "Noise"],
            ["turbulence", "Turbulence", "Noise folded into sharp ridges"],
            ["self", "Its tones", "Light parts pushed one way, dark the other"],
          ]),
          ui.sliders([["amount", "Amount", "pt", (u) => ({ min: 0, max: upTo(u, 0.15, 24), shape: "curve", digits: 1 })]]),
          ui.when(
            ui.sliders([
              ["scale", "Scale", "pt", (u) => ({ min: 2, max: upTo(u, 0.6, 96), shape: "curve" })],
              ["detail", "Detail", "", () => ({ min: 1, max: 6 })],
              ["stretch", "Stretch", "%", () => ({ min: -100, max: 100 })],
            ]),
            noisy,
          ),
          ui.when(
            ui.segments("axis", "Moves", [
              ["both", "Both ways"],
              ["x", "Across"],
              ["y", "Up and down"],
            ]),
            noisy,
          ),
          ui.when(ui.shuffle("seed", "Noise"), noisy),
          ui.when(
            ui.sliders([
              ["angle", "Angle", "°", () => ({ min: -180, max: 180 })],
              ["smooth", "Smoothing", "pt", (u) => ({ min: 0, max: upTo(u, 0.1, 16), shape: "curve", digits: 1 })],
            ]),
            () => !noisy(),
          ),
        ),
        ui.section("Color", ui.sliders([["split", "Split", "%", () => ({ min: 0, max: 100 })]])),
      ];
    },
    process: (image, s, k) => {
      const { width: W, height: H } = image;
      const f = premultiplied(image);
      const amount = s.amount * k;
      const split = s.split / 100;
      const opaque = isOpaque(f);
      const sample = opaque ? edgeAt : sampleAt;
      const pad = opaque ? 0 : Math.ceil(amount * (1 + split) * 1.2) + 1;
      const [OW, OH] = [W + 2 * pad, H + 2 * pad];
      const self = s.map === "self";
      // Its tones, transparency a middle gray that moves nothing
      let tones = null;
      if (self) {
        tones = new Float32Array(W * H);
        for (let i = 0; i < W * H; i++) tones[i] = lumaOf(f[i * 4], f[i * 4 + 1], f[i * 4 + 2]) + (1 - f[i * 4 + 3]) * 0.5;
        tones = blurChannel(tones, W, H, s.smooth * k);
      }
      const [ux, uy] = [Math.cos((s.angle * Math.PI) / 180), Math.sin((s.angle * Math.PI) / 180)];
      const size = Math.max(1, s.scale * k);
      const st = s.stretch / 100;
      const [sx, sy] = [size * (1 + Math.max(0, st) * 3), size * (1 + Math.max(0, -st) * 3)];
      const [octaves, turbulent, seed] = [Math.round(s.detail), s.map === "turbulence", s.seed | 0];
      const out = new Float32Array(OW * OH * 4);
      const [p, red, blue] = [new Float32Array(4), new Float32Array(4), new Float32Array(4)];
      for (let y = 0; y < OH; y++)
        for (let x = 0; x < OW; x++) {
          const [X, Y] = [x - pad + 0.5, y - pad + 0.5];
          let [dx, dy] = [0, 0];
          if (self) {
            const m = (channelAt(tones, W, H, Math.min(W - 0.5, Math.max(0.5, X)), Math.min(H - 0.5, Math.max(0.5, Y))) - 0.5) * 2 * amount;
            [dx, dy] = [m * ux, m * uy];
          } else {
            // In points, so the look holds at any size
            if (s.axis !== "y") dx = noiseAt(X / sx, Y / sy, seed, octaves, turbulent) * amount * 1.6;
            if (s.axis !== "x") dy = noiseAt(X / sx, Y / sy, seed + 977, octaves, turbulent) * amount * 1.6;
          }
          const at = (y * OW + x) * 4;
          sample(f, W, H, X + dx, Y + dy, p);
          if (split > 0) {
            sample(f, W, H, X + dx * (1 + split), Y + dy * (1 + split), red);
            sample(f, W, H, X + dx * (1 - split), Y + dy * (1 - split), blue);
            out[at] = red[0];
            out[at + 1] = p[1];
            out[at + 2] = blue[2];
            out[at + 3] = Math.max(red[3], p[3], blue[3]);
          } else out.set(p, at);
        }
      return { image: toImage(out, OW, OH), x: -pad, y: -pad };
    },
  });

  // ---------- Motion Blur ----------

  // Smeared along its motion: one way (Linear), out from a center (Zoom)
  // or around it (Spin). Each pixel averages eight along the path, then
  // eight between those and eight between those again, so even a long
  // blur is smooth. Trail smears it one way only, behind its motion. A
  // photo keeps its bounds, its edges repeated; a cutout grows to fit
  const MOTION_ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><circle ${STROKE} cx="15" cy="12" r="4.5"/><path ${STROKE} d="M4.5 9.5h4M3.5 12h5M4.5 14.5h4"/></svg>`;
  pictureEffect({
    id: "motionBlur",
    thumb: { sample: "lights", settings: { distance: 26, angle: 0, trail: true } },
    name: "Motion Blur",
    about: "Smear it along a line, out from a point or around it",
    icon: MOTION_ICON,
    verb: "Blur",
    defaults: { mode: "linear", distance: 20, angle: 0, trail: false, zoom: 20, spin: 10, centerX: 50, centerY: 50, editable: true },
    presets: [
      ["Pan", () => ({})],
      ["Speed", () => ({ distance: 40, trail: true })],
      ["Shake", () => ({ distance: 8, angle: 90 })],
      ["Zoom burst", () => ({ mode: "zoom", zoom: 30 })],
      ["Spin", () => ({ mode: "spin", spin: 15 })],
    ],
    limits: { distance: [0, 4000], angle: [-360, 360], zoom: [0, 95], spin: [-360, 360], centerX: [-200, 300], centerY: [-200, 300] },
    summary: (s) => ({ linear: `Linear · ${s.distance}pt`, zoom: `Zoom · ${s.zoom}%`, spin: `Spin · ${s.spin}°` })[s.mode] ?? "",
    panel: (ui) => {
      const { s } = ui;
      return [
        ui.section(
          "Motion",
          ui.segments("mode", "Motion", [
            ["linear", "Linear"],
            ["zoom", "Zoom", "Out from a center"],
            ["spin", "Spin", "Around a center"],
          ]),
          ui.when(
            ui.sliders([
              ["distance", "Distance", "pt", (u) => ({ min: 0, max: upTo(u, 0.5, 64), shape: "curve" })],
              ["angle", "Angle", "°", () => ({ min: -180, max: 180 })],
            ]),
            () => s.mode === "linear",
          ),
          ui.when(ui.sliders([["zoom", "Amount", "%", () => ({ min: 0, max: 80 })]]), () => s.mode === "zoom"),
          ui.when(ui.sliders([["spin", "Angle", "°", () => ({ min: -90, max: 90 })]]), () => s.mode === "spin"),
          ui.check("trail", "Trail", "Smeared one way only, behind its motion"),
        ),
        ui.when(
          ui.section(
            "Center",
            ui.sliders([
              ["centerX", "Center x", "%", () => ({ min: 0, max: 100 })],
              ["centerY", "Center y", "%", () => ({ min: 0, max: 100 })],
            ]),
          ),
          () => s.mode !== "linear",
        ),
      ];
    },
    process: (image, s, k) => {
      const { width: W, height: H } = image;
      const f = premultiplied(image);
      const zoom = Math.min(0.95, s.zoom / 100);
      const spin = (s.spin * Math.PI) / 180;
      const length = s.distance * k;
      const opaque = isOpaque(f);
      const sample = opaque ? edgeAt : sampleAt;
      const pad = opaque
        ? 0
        : s.mode === "linear"
          ? Math.ceil(length) + 1
          : s.mode === "zoom"
            ? Math.ceil(Math.min(Math.max(W, H), (Math.max(W, H) / 2) * (zoom / (1 - zoom)))) + 1
            : Math.ceil((Math.hypot(W, H) / 2 - Math.min(W, H) / 2) * Math.min(1, Math.abs(s.spin) / 45)) + 1;
      const [OW, OH] = [W + 2 * pad, H + 2 * pad];
      let from = new Float32Array(OW * OH * 4);
      for (let y = 0; y < H; y++) from.set(f.subarray(y * W * 4, (y + 1) * W * 4), ((y + pad) * OW + pad) * 4);
      const [cx, cy] = [pad + (W * s.centerX) / 100, pad + (H * s.centerY) / 100];
      const [vx, vy] = [Math.cos((s.angle * Math.PI) / 180) * length, Math.sin((s.angle * Math.PI) / 180) * length];
      // How far along the path, `t` of it, a pixel at `X`, `Y` takes from
      const along = (X, Y, t, into) => {
        if (s.mode === "linear") return sample(from, OW, OH, X + vx * t, Y + vy * t, into);
        const [dx, dy] = [X - cx, Y - cy];
        if (s.mode === "zoom") return sample(from, OW, OH, cx + dx * (1 - zoom * t), cy + dy * (1 - zoom * t), into);
        const [c, n] = [Math.cos(spin * t), Math.sin(spin * t)];
        return sample(from, OW, OH, cx + dx * c - dy * n, cy + dx * n + dy * c, into);
      };
      // The longest the path gets, in pixels, to skip passes too fine to see
      const reach = s.mode === "linear" ? length : s.mode === "zoom" ? (zoom * Math.hypot(OW, OH)) / 2 : (Math.abs(spin) * Math.hypot(OW, OH)) / 2;
      const TAPS = 8;
      const p = new Float32Array(4);
      for (let pass = 0, span = 1; pass < 3 && (reach * span) / (TAPS - 1) >= 0.25 && reach >= 0.5; pass++, span /= TAPS - 1) {
        const to = new Float32Array(OW * OH * 4);
        for (let y = 0; y < OH; y++)
          for (let x = 0; x < OW; x++) {
            const at = (y * OW + x) * 4;
            for (let j = 0; j < TAPS; j++) {
              const t = (s.trail ? j / (TAPS - 1) : j / (TAPS - 1) - 0.5) * span;
              along(x + 0.5, y + 0.5, t, p);
              to[at] += p[0] / TAPS;
              to[at + 1] += p[1] / TAPS;
              to[at + 2] += p[2] / TAPS;
              to[at + 3] += p[3] / TAPS;
            }
          }
        from = to;
      }
      return { image: toImage(from, OW, OH), x: -pad, y: -pad };
    },
  });

  // ---------- Glow ----------

  // Light spilling past its brightest parts (Bloom), or past its whole
  // shape (Outer), blurred at three sizes and added over it as light is,
  // so it washes toward white where it's strongest. Tint turns the light
  // toward a color
  const GLOW_ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><circle ${STROKE} cx="12" cy="12" r="3.5"/><path ${STROKE} d="M12 4.5v2M12 17.5v2M4.5 12h2M17.5 12h2M6.7 6.7l1.4 1.4M15.9 15.9l1.4 1.4M6.7 17.3l1.4-1.4M15.9 8.1l1.4-1.4"/></svg>`;
  pictureEffect({
    id: "glow",
    thumb: { sample: "neon", settings: { threshold: 40, radius: 12, intensity: 150 } },
    name: "Glow",
    about: "Make its bright parts or its shape glow",
    icon: GLOW_ICON,
    verb: "Add glow",
    defaults: { mode: "bloom", threshold: 65, softness: 20, radius: 16, intensity: 120, tint: 0, color: "#ffd38a", only: false, editable: true },
    presets: [
      ["Bloom", () => ({})],
      ["Dreamy", () => ({ threshold: 35, radius: 30, intensity: 90 })],
      ["Highlights", () => ({ threshold: 80, softness: 10, radius: 8, intensity: 170 })],
      ["Neon", () => ({ mode: "outer", radius: 10, intensity: 160 })],
      ["Halo", () => ({ mode: "outer", radius: 24, intensity: 120, tint: 100, color: "#9ad7ff" })],
    ],
    limits: { threshold: [0, 100], softness: [0, 100], radius: [0, 1000], intensity: [0, 1000], tint: [0, 100] },
    summary: (s) => `${s.mode === "outer" ? "Outer" : "Bloom"} · ${s.radius}pt`,
    panel: (ui) => {
      const { s } = ui;
      return [
        ui.section(
          "Glow",
          ui.segments("mode", "From", [
            ["bloom", "Bright parts"],
            ["outer", "Its shape", "Light from all of it, past its edges"],
          ]),
          ui.when(
            ui.sliders([
              ["threshold", "Threshold", "%", () => ({ min: 0, max: 100 })],
              ["softness", "Softness", "%", () => ({ min: 0, max: 100 })],
            ]),
            () => s.mode === "bloom",
          ),
          ui.sliders([
            ["radius", "Radius", "pt", (u) => ({ min: 0, max: upTo(u, 0.3, 48), shape: "curve" })],
            ["intensity", "Intensity", "%", () => ({ min: 0, max: 300, middle: 100 })],
          ]),
        ),
        ui.section("Color", ui.sliders([["tint", "Tint", "%", () => ({ min: 0, max: 100 })]]), ui.color("color", "Color"), ui.check("only", "Glow only", "Only the glow, without the layer under it")),
      ];
    },
    process: (image, s, k) => {
      const { width: W, height: H } = image;
      const f = premultiplied(image);
      const r = Math.max(0.5, s.radius * k);
      const pad = isOpaque(f) ? 0 : Math.ceil(3 * r) + 1;
      const [OW, OH] = [W + 2 * pad, H + 2 * pad];
      // Worked out smaller, as it's blurred anyway, but never so small its
      // finest blur's lost
      const q = Math.min(1, Math.max(700 / Math.max(OW, OH), 4 / r));
      const [MW, MH] = [Math.max(1, Math.ceil(OW * q)), Math.max(1, Math.ceil(OH * q))];
      const [th, knee, tint] = [s.threshold / 100, Math.max(0.001, s.softness / 100), s.tint / 100];
      const color = hexRgb(s.color);
      const light = [0, 1, 2].map(() => new Float32Array(MW * MH));
      const p = new Float32Array(4);
      for (let v = 0; v < MH; v++)
        for (let u = 0; u < MW; u++) {
          sampleAt(f, W, H, (u + 0.5) / q - pad, (v + 0.5) / q - pad, p);
          if (p[3] <= 0.001) continue;
          const c = [p[0] / p[3], p[1] / p[3], p[2] / p[3]];
          let w = 1;
          if (s.mode === "bloom") {
            const t = clamp01((lumaOf(c[0], c[1], c[2]) - th + knee) / (2 * knee));
            w = t * t * (3 - 2 * t);
          }
          for (let ch = 0; ch < 3; ch++) light[ch][v * MW + u] = (c[ch] + (color[ch] - c[ch]) * tint) * w * p[3];
        }
      // Three sizes together, as light scatters near and far, stronger
      // than the light alone, so a thin line still glows
      const glow = new Float32Array(MW * MH * 4);
      for (let ch = 0; ch < 3; ch++)
        for (const [m, weight] of [
          [0.25, 0.6],
          [0.55, 0.8],
          [1, 1.2],
        ]) {
          const blurred = blurChannel(light[ch], MW, MH, r * m * q);
          for (let i = 0; i < MW * MH; i++) glow[i * 4 + ch] += blurred[i] * weight;
        }
      const intensity = s.intensity / 100;
      const out = new Float32Array(OW * OH * 4);
      for (let y = 0; y < OH; y++)
        for (let x = 0; x < OW; x++) {
          sampleAt(glow, MW, MH, (x + 0.5) * q, (y + 0.5) * q, p);
          const [gr, gg, gb] = [p[0] * intensity, p[1] * intensity, p[2] * intensity];
          const ga = Math.min(1, Math.max(gr, gg, gb));
          const at = (y * OW + x) * 4;
          const [sx, sy] = [x - pad, y - pad];
          if (!s.only && sx >= 0 && sy >= 0 && sx < W && sy < H) {
            const from = (sy * W + sx) * 4;
            const a = f[from + 3];
            out[at] = f[from] + gr;
            out[at + 1] = f[from + 1] + gg;
            out[at + 2] = f[from + 2] + gb;
            out[at + 3] = a + ga * (1 - a);
          } else {
            out[at] = gr;
            out[at + 1] = gg;
            out[at + 2] = gb;
            out[at + 3] = ga;
          }
        }
      return { image: toImage(out, OW, OH), x: -pad, y: -pad };
    },
  });

  // ---------- Shadow ----------

  // A shadow worked out from a cutout's own shape, for a figure or object
  // standing on the ground at the picture's lowest solid row. Cast lays
  // its silhouette down on the ground away from the light: each point as
  // far from its footing as it's high, times Length, toward Direction,
  // foreshortened by the ground's Perspective, spreading wider with
  // Spread as a near light's does, sharp where it touches and softer and
  // fainter toward its end, with a contact shadow under the footing. Drop
  // is a flat drop shadow. The original's drawn over it
  const SHADOW_ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><rect ${STROKE} x="6.5" y="4.5" width="5" height="12" rx="2.5"/><path ${STROKE} stroke-dasharray="1.5 1.5" d="M11.5 16.5l6.5-3.5M7 16.5l7.5 3h4l-.5-6.5"/></svg>`;
  pictureEffect({
    id: "shadow",
    thumb: { sample: "pawnOnFloor", backdrop: "floor", settings: { direction: 65, length: 120, perspective: 60, softFar: 6, opacity: 75, fade: 25 } },
    name: "Shadow",
    about: "Cast a shadow from a cutout's shape",
    icon: SHADOW_ICON,
    verb: "Add shadow",
    defaults: { mode: "cast", direction: 60, length: 90, perspective: 45, spread: 10, softNear: 1, softFar: 8, opacity: 60, fade: 35, contact: 60, contactSize: 4, ground: 0, distance: 8, angle: 90, blur: 12, color: "#000000", only: false, editable: true },
    presets: [
      ["Afternoon", () => ({})],
      ["Noon", () => ({ length: 30, direction: 40, perspective: 40, softFar: 6, opacity: 60 })],
      ["Evening", () => ({ direction: 75, length: 180, perspective: 40, spread: 20, softFar: 20, opacity: 50, fade: 10 })],
      ["Backlit", () => ({ direction: 180, length: 60, perspective: 45, softFar: 10 })],
      ["Studio", () => ({ direction: -35, length: 60, perspective: 40, softNear: 2, softFar: 22, opacity: 40, fade: 0, contact: 80 })],
      ["Floor", () => ({ length: 0, contact: 85, contactSize: 7 })],
      ["Drop", () => ({ mode: "drop" })],
    ],
    limits: { direction: [-360, 360], length: [0, 1000], perspective: [1, 100], spread: [-90, 400], softNear: [0, 1000], softFar: [0, 1000], opacity: [0, 100], fade: [0, 100], contact: [0, 100], contactSize: [0, 1000], ground: [-1000, 1000], distance: [0, 4000], angle: [-360, 360], blur: [0, 1000] },
    summary: (s) => (s.mode === "drop" ? `Drop · ${s.blur}pt blur` : `Cast · ${s.direction}° · ${s.length}%`),
    panel: (ui) => {
      const { s } = ui;
      const cast = () => s.mode === "cast";
      return [
        ui.section(
          "Shadow",
          ui.segments("mode", "Shadow", [
            ["cast", "Cast", "Laid down on the ground, away from the light"],
            ["drop", "Drop"],
          ]),
          ui.when(
            ui.sliders([
              ["direction", "Direction", "°", () => ({ min: -180, max: 180 })],
              ["length", "Length", "%", () => ({ min: 0, max: 300, middle: 100 })],
              ["perspective", "Perspective", "%", () => ({ min: 1, max: 100 })],
              ["spread", "Spread", "%", () => ({ min: -50, max: 100 })],
            ]),
            cast,
          ),
          ui.when(
            ui.sliders([
              ["distance", "Distance", "pt", (u) => ({ min: 0, max: upTo(u, 0.25, 40), shape: "curve" })],
              ["angle", "Angle", "°", () => ({ min: -180, max: 180 })],
              ["blur", "Blur", "pt", (u) => ({ min: 0, max: upTo(u, 0.25, 40), shape: "curve" })],
            ]),
            () => !cast(),
          ),
        ),
        ui.when(
          ui.section(
            "Softness",
            ui.sliders([
              ["softNear", "At contact", "pt", (u) => ({ min: 0, max: upTo(u, 0.1, 16), shape: "curve", digits: 1 })],
              ["softFar", "At the end", "pt", (u) => ({ min: 0, max: upTo(u, 0.3, 40), shape: "curve" })],
              ["fade", "End opacity", "%", () => ({ min: 0, max: 100 })],
            ]),
          ),
          cast,
        ),
        ui.when(
          ui.section(
            "Contact",
            ui.sliders([
              ["contact", "Darkness", "%", () => ({ min: 0, max: 100 })],
              ["contactSize", "Size", "pt", (u) => ({ min: 0, max: upTo(u, 0.1, 16), shape: "curve", digits: 1 })],
              ["ground", "Ground", "pt", (u) => ({ min: -upTo(u, 0.1, 16), max: upTo(u, 0.1, 16), digits: 1 })],
            ]),
          ),
          cast,
        ),
        ui.section("Look", ui.sliders([["opacity", "Opacity", "%", () => ({ min: 0, max: 100 })]]), ui.color("color", "Color"), ui.check("only", "Shadow only", "Only the shadow, without the layer over it")),
      ];
    },
    process: (image, s, k) => {
      const { width: W, height: H } = image;
      const f = premultiplied(image);
      const alpha = new Float32Array(W * H);
      for (let i = 0; i < W * H; i++) alpha[i] = f[i * 4 + 3];
      const { top, bottom } = solidRows(f, W, H);
      const ground = bottom - s.ground * k;
      const tall = Math.max(1, ground - top);
      const cx = W / 2;
      const cast = s.mode !== "drop";
      const turn = (s.direction * Math.PI) / 180;
      const length = s.length / 100;
      const dx = Math.sin(turn) * length;
      let dy = -Math.cos(turn) * length * (s.perspective / 100);
      if (Math.abs(dy) < 0.004) dy = dy < 0 ? -0.004 : 0.004;
      const spread = s.spread / 100;
      const dropAngle = (s.angle * Math.PI) / 180;
      const [ox, oy] = [Math.cos(dropAngle) * s.distance * k, Math.sin(dropAngle) * s.distance * k];
      const [near, far] = cast ? [s.softNear * k, s.softFar * k] : [s.blur * k, s.blur * k];
      const band = Math.max(1, s.contactSize * k);
      // Where the shadow can reach, and room for its blur
      let [x0, y0, x1, y1] = [0, 0, W, H];
      const reach = (x, y) => {
        x0 = Math.min(x0, x);
        y0 = Math.min(y0, y);
        x1 = Math.max(x1, x);
        y1 = Math.max(y1, y);
      };
      if (cast) {
        for (const y of [top, ground])
          for (const x of [0, W]) {
            const h = ground - y;
            reach(cx + (x - cx) * Math.max(0.05, 1 + (spread * h) / tall) + h * dx, ground + h * dy);
          }
        reach(0, ground + band * 2);
        reach(W, ground + band * 2);
      } else {
        reach(ox, oy);
        reach(W + ox, H + oy);
      }
      const margin = Math.ceil(3 * Math.max(near, far, cast ? band : 0)) + 2;
      [x0, y0, x1, y1] = [Math.floor(x0) - margin, Math.floor(y0) - margin, Math.ceil(x1) + margin, Math.ceil(y1) + margin];
      const [OW, OH] = [x1 - x0, y1 - y0];
      // The shadow's soft, so it's worked out smaller and drawn up
      const q = Math.min(1, 1600 / Math.max(OW, OH));
      const [MW, MH] = [Math.max(1, Math.ceil(OW * q)), Math.max(1, Math.ceil(OH * q))];
      const mask = new Float32Array(MW * MH);
      const heights = new Float32Array(MW * MH);
      const fade = s.fade / 100;
      for (let v = 0; v < MH; v++)
        for (let u = 0; u < MW; u++) {
          const i = v * MW + u;
          const [X, Y] = [x0 + (u + 0.5) / q, y0 + (v + 0.5) / q];
          if (!cast) {
            mask[i] = channelAt(alpha, W, H, X - ox, Y - oy);
            continue;
          }
          const h = (Y - ground) / dy;
          if (h < 0) continue;
          const t = Math.min(1, h / tall);
          heights[i] = t;
          const widen = 1 + spread * (h / tall);
          if (widen <= 0.05) continue;
          mask[i] = channelAt(alpha, W, H, cx + (X - h * dx - cx) / widen, ground - h) * (1 - (1 - fade) * t);
        }
      // Sharper near the footing, softer toward the end: blurred at a few
      // softnesses, each pixel between the two nearest its own
      let shadow;
      if (!cast || Math.abs(far - near) < 0.5) shadow = blurChannel(mask, MW, MH, near * q);
      else {
        const levels = [0, 1, 2, 3].map((j) => blurChannel(mask, MW, MH, (near + ((far - near) * j) / 3) * q));
        shadow = new Float32Array(MW * MH);
        for (let i = 0; i < MW * MH; i++) {
          const at = heights[i] * 3;
          const j = Math.min(2, Math.floor(at));
          const m = at - j;
          shadow[i] = levels[j][i] * (1 - m) + levels[j + 1][i] * m;
        }
      }
      const opacity = s.opacity / 100;
      for (let i = 0; i < MW * MH; i++) shadow[i] = clamp01(shadow[i] * opacity);
      // Where it touches the ground: the footing's own outline, squashed
      // flat and blurred
      if (cast && s.contact > 0) {
        const foot = new Float32Array(W);
        const from = Math.max(0, Math.floor(ground - band));
        for (let y = from; y < Math.min(H, Math.ceil(ground)); y++) for (let x = 0; x < W; x++) foot[x] = Math.max(foot[x], alpha[y * W + x]);
        const contact = new Float32Array(MW * MH);
        for (let v = 0; v < MH; v++) {
          const Y = y0 + (v + 0.5) / q;
          const fall = Math.exp(-(((Y - ground) / (band * 0.5)) ** 2));
          if (fall < 0.002) continue;
          for (let u = 0; u < MW; u++) contact[v * MW + u] = channelAt(foot, W, 1, x0 + (u + 0.5) / q, 0.5) * fall;
        }
        const soft = blurChannel(contact, MW, MH, band * 0.6 * q);
        const amount = s.contact / 100;
        for (let i = 0; i < MW * MH; i++) shadow[i] = 1 - (1 - shadow[i]) * (1 - clamp01(soft[i] * amount * 1.4));
      }
      const color = hexRgb(s.color);
      const out = new Float32Array(OW * OH * 4);
      for (let y = 0; y < OH; y++)
        for (let x = 0; x < OW; x++) {
          const at = (y * OW + x) * 4;
          const a = q === 1 ? shadow[y * MW + x] : channelAt(shadow, MW, MH, (x + 0.5) * q, (y + 0.5) * q);
          let [r, g, b, al] = [color[0] * a, color[1] * a, color[2] * a, a];
          const [sx, sy] = [x + x0, y + y0];
          if (!s.only && sx >= 0 && sy >= 0 && sx < W && sy < H) {
            const from = (sy * W + sx) * 4;
            const keep = 1 - f[from + 3];
            [r, g, b, al] = [f[from] + r * keep, f[from + 1] + g * keep, f[from + 2] + b * keep, f[from + 3] + al * keep];
          }
          out[at] = r;
          out[at + 1] = g;
          out[at + 2] = b;
          out[at + 3] = al;
        }
      return { image: toImage(out, OW, OH), x: x0, y: y0 };
    },
  });

  // ---------- Reflection ----------

  // A cutout's reflection on a floor or on water, worked out from its own
  // picture: mirrored at its lowest solid row and faded and blurred as it
  // goes from the footing. Fresnel makes it fainter toward the viewer, as
  // a real surface's is, and Streaks smear it downward as wet ground and
  // water do. On water, waves seen in perspective, small near the footing
  // and larger toward the viewer, shift it up and down and side to side,
  // their steepest faces breaking it apart and catching glints. Surface
  // lays the floor's or water's own color under it. The original's drawn
  // over it
  const REFLECTION_ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><rect ${STROKE} x="7.5" y="3.5" width="9" height="8" rx="1.5"/><path ${STROKE} d="M4.5 13.5h15"/><path ${STROKE} stroke-dasharray="1.5 1.5" d="M7.5 15.5v3a1.5 1.5 0 0 0 1.5 1.5h6a1.5 1.5 0 0 0 1.5-1.5v-3"/></svg>`;
  // Smooth noise, about 0 to 1, eased between whole points
  const valueNoise = (x, y, seed) => {
    const xi = Math.floor(x);
    const yi = Math.floor(y);
    let fx = x - xi;
    let fy = y - yi;
    fx = fx * fx * (3 - 2 * fx);
    fy = fy * fy * (3 - 2 * fy);
    const a = hash2(xi, yi, seed);
    const b = hash2(xi + 1, yi, seed);
    const c = hash2(xi, yi + 1, seed);
    const d = hash2(xi + 1, yi + 1, seed);
    return a + (b - a) * fx + (c - a) * fy + (a - b - c + d) * fx * fy;
  };
  // A wave surface's height, about −0.5 to 0.5: octaves of noise, each
  // half the size of the last and turned a little from it
  const wavesAt = (x, y, seed) => {
    let v = 0;
    let amp = 1;
    let total = 0;
    for (let o = 0; o < 4; o++) {
      v += (valueNoise(x, y, seed + o * 101) - 0.5) * amp;
      total += amp;
      [x, y] = [x * 1.9 + y * 0.35 + 17.3, y * 1.9 - x * 0.35 + 9.1];
      amp *= 0.5;
    }
    return v / total;
  };
  Object.assign(PIXEL_HELPERS, { valueNoise, wavesAt });
  pictureEffect({
    id: "reflection",
    thumb: { sample: "pawnOnWater", backdrop: "water", settings: { surface: "water", length: 100, opacity: 80, fade: 25, fresnel: 35, blur: 0.5, streak: 3, waves: 3, waveSize: 14, chop: 35, sparkle: 15, tint: "#24506b", tintAmount: 25 } },
    name: "Reflection",
    about: "Reflect a cutout on a floor or water",
    icon: REFLECTION_ICON,
    verb: "Add reflection",
    defaults: { surface: "floor", length: 45, opacity: 40, fade: 100, fresnel: 0, gap: 0, blur: 4, streak: 0, squash: 100, skew: 0, waves: 3, waveSize: 20, perspective: 70, chop: 30, sparkle: 0, seed: 1, tint: "#ffffff", tintAmount: 0, surfaceOpacity: 0, only: false, editable: true },
    presets: [
      ["Glossy", () => ({})],
      ["Mirror", () => ({ length: 100, opacity: 65, blur: 0 })],
      ["Satin", () => ({ length: 35, opacity: 30, blur: 12 })],
      ["Wet ground", () => ({ length: 90, opacity: 50, fade: 60, fresnel: 50, blur: 2, streak: 10, tint: "#2b2f36", tintAmount: 25 })],
      ["Still water", () => ({ surface: "water", length: 100, opacity: 75, fade: 20, fresnel: 45, blur: 0, streak: 1, waves: 1, waveSize: 28, chop: 0, tint: "#1d4e89", tintAmount: 25 })],
      ["Lake", () => ({ surface: "water", length: 100, opacity: 70, fade: 25, fresnel: 50, blur: 0.5, streak: 3, waves: 4, waveSize: 18, chop: 35, sparkle: 15, tint: "#24506b", tintAmount: 35 })],
      ["Choppy", () => ({ surface: "water", length: 100, opacity: 80, fade: 35, fresnel: 35, blur: 1, streak: 5, waves: 9, waveSize: 14, chop: 70, sparkle: 45, tint: "#1b3f5c", tintAmount: 45 })],
      ["Angled", () => ({ squash: 60, skew: 25, length: 55 })],
    ],
    limits: { length: [1, 400], opacity: [0, 100], fade: [0, 100], fresnel: [0, 100], gap: [-1000, 1000], blur: [0, 1000], streak: [0, 1000], squash: [5, 400], skew: [-400, 400], waves: [0, 1000], waveSize: [1, 4000], perspective: [0, 100], chop: [0, 100], sparkle: [0, 100], tintAmount: [0, 100], surfaceOpacity: [0, 100] },
    summary: (s) => `${s.surface === "water" ? "Water" : "Floor"} · ${s.length}% · ${s.opacity}%`,
    panel: (ui) => {
      const { s } = ui;
      return [
        ui.section(
          "Reflection",
          ui.segments("surface", "Surface", [
            ["floor", "Floor", "A glossy floor, polished or worn"],
            ["water", "Water", "Waves that shift and break the reflection"],
          ]),
          ui.sliders([
            ["length", "Length", "%", () => ({ min: 1, max: 100 })],
            ["opacity", "Opacity", "%", () => ({ min: 0, max: 100 })],
            ["fade", "Fade", "%", () => ({ min: 0, max: 100 })],
            ["fresnel", "Fresnel", "%", () => ({ min: 0, max: 100 })],
          ]),
        ),
        ui.when(
          ui.section(
            "Waves",
            ui.sliders([
              ["waves", "Height", "pt", (u) => ({ min: 0, max: upTo(u, 0.08, 12), shape: "curve", digits: 1 })],
              ["waveSize", "Size", "pt", (u) => ({ min: 2, max: upTo(u, 0.3, 48), shape: "curve" })],
              ["perspective", "Perspective", "%", () => ({ min: 0, max: 100 })],
              ["chop", "Broken", "%", () => ({ min: 0, max: 100 })],
              ["sparkle", "Glints", "%", () => ({ min: 0, max: 100 })],
            ]),
            ui.shuffle("seed", "Waves"),
          ),
          () => s.surface === "water",
        ),
        ui.section(
          "Softness",
          ui.sliders([
            ["blur", "Blur", "pt", (u) => ({ min: 0, max: upTo(u, 0.15, 24), shape: "curve", digits: 1 })],
            ["streak", "Streaks", "pt", (u) => ({ min: 0, max: upTo(u, 0.15, 24), shape: "curve", digits: 1 })],
          ]),
        ),
        ui.section(
          "Placement",
          ui.sliders([
            ["gap", "Gap", "pt", (u) => ({ min: -upTo(u, 0.1, 16), max: upTo(u, 0.1, 16), digits: 1 })],
            ["squash", "Squash", "%", () => ({ min: 10, max: 200, middle: 100 })],
            ["skew", "Skew", "%", () => ({ min: -100, max: 100 })],
          ]),
        ),
        ui.section(
          "Color",
          ui.sliders([
            ["tintAmount", "Tint", "%", () => ({ min: 0, max: 100 })],
            ["surfaceOpacity", "Surface", "%", () => ({ min: 0, max: 100 })],
          ]),
          ui.color("tint", "Color"),
          ui.check("only", "Reflection only", "Only the reflection, without the layer over it"),
        ),
      ];
    },
    process: (image, s, k) => {
      const { width: W, height: H } = image;
      const smooth = (a, b, x) => {
        const t = clamp01((x - a) / (b - a));
        return t * t * (3 - 2 * t);
      };
      const f = premultiplied(image);
      const { top, bottom } = solidRows(f, W, H);
      const tall = Math.max(1, bottom - top);
      const water = s.surface === "water";
      const reach = Math.max(1, (tall * s.length) / 100);
      const squash = Math.max(0.05, s.squash / 100);
      const skew = s.skew / 100;
      const depth = reach * squash;
      const blur = s.blur * k;
      const streak = s.streak * k;
      const waves = water ? s.waves * k : 0;
      const margin = Math.ceil(3 * blur + 2 * waves) + 2;
      const start = bottom + s.gap * k;
      // The surface's a pool around the footing, as wide as the cutout and
      // more, as deep as the reflection goes
      const surface = s.surfaceOpacity / 100;
      let [left, right] = [W, 0];
      for (let y = 0; y < H; y++)
        for (let x = 0; x < W; x++)
          if (f[(y * W + x) * 4 + 3] > 0.5) {
            left = Math.min(left, x);
            right = Math.max(right, x + 1);
          }
      if (left >= right) [left, right] = [0, W];
      const cx = (left + right) / 2 + (depth / squash / 2) * skew;
      const rx = (right - left) * 1.1 + depth * 0.6;
      const ry = depth + 3 * streak;
      const x0 = Math.floor(Math.min(0, reach * skew, surface > 0 ? cx - rx : 0)) - margin;
      const x1 = Math.ceil(Math.max(W, W + reach * skew, surface > 0 ? cx + rx : 0)) + margin;
      const y0 = Math.min(0, Math.floor(start) - margin);
      const y1 = Math.max(H, Math.ceil(start + depth + 3 * streak) + margin);
      const [OW, OH] = [x1 - x0, y1 - y0];
      const q = blur > 0.5 || streak > 0.5 || water ? Math.min(1, 1200 / Math.max(OW, OH)) : 1;
      const [MW, MH] = [Math.max(1, Math.ceil(OW * q)), Math.max(1, Math.ceil(OH * q))];
      const tint = hexRgb(s.tint);
      const [tintAmount, opacity, fresnel] = [s.tintAmount / 100, s.opacity / 100, s.fresnel / 100];
      const fadeFrom = 1 - s.fade / 100;
      // Waves in perspective, the horizon `d0` rows above the footing, so
      // they're `waveSize` at the reflection's end and smaller up toward
      // it, wider than deep, as water seen at a low angle is
      const size = Math.max(2, s.waveSize * k);
      const d0 = depth * (0.08 + (1 - s.perspective / 100) * 3);
      const [WIDE, DEEP] = [2.2, 0.45];
      const scaleAt = (d) => (d + d0) / (depth + d0);
      const field = water ? new Float32Array(MW * MH) : null;
      if (water)
        for (let v = 0; v < MH; v++) {
          const d = Math.max(0, y0 + (v + 0.5) / q - start);
          const scale = scaleAt(d);
          const wv = ((depth + d0) / (size * DEEP)) * Math.log(d + d0);
          for (let u = 0; u < MW; u++) field[v * MW + u] = wavesAt((x0 + (u + 0.5) / q - W / 2) / (size * WIDE * scale), wv, s.seed | 0);
        }
      // The waves' slopes, and how steep they usually are, so Height is
      // about how far they move the reflection
      const slopes = water ? new Float32Array(MW * MH * 2) : null;
      let [rms, rmsX] = [1, 1];
      if (water) {
        let [sum, sumX, n] = [0, 0, 0];
        for (let v = 0; v < MH; v++) {
          const d = y0 + (v + 0.5) / q - start;
          if (d < 0) continue;
          const scale = scaleAt(d);
          const [up, down] = [Math.max(0, v - 1), Math.min(MH - 1, v + 1)];
          for (let u = 0; u < MW; u++) {
            const [left, right] = [Math.max(0, u - 1), Math.min(MW - 1, u + 1)];
            const sx = ((field[v * MW + right] - field[v * MW + left]) * q * size * WIDE * scale) / Math.max(1, right - left);
            const sy = ((field[down * MW + u] - field[up * MW + u]) * q * size * DEEP * scale) / Math.max(1, down - up);
            slopes[(v * MW + u) * 2] = sx;
            slopes[(v * MW + u) * 2 + 1] = sy;
            sum += sy * sy;
            sumX += sx * sx;
            n++;
          }
        }
        rms = Math.sqrt(sum / Math.max(1, n)) || 1;
        rmsX = Math.sqrt(sumX / Math.max(1, n)) || 1;
      }
      const chop = s.chop / 100;
      const breakAt = 2.4 - 2 * chop;
      const sparkle = s.sparkle / 100;
      const mirror = new Float32Array(MW * MH * 4);
      const along = new Float32Array(MW * MH);
      const p = new Float32Array(4);
      for (let v = 0; v < MH; v++) {
        const d = y0 + (v + 0.5) / q - start;
        if (d < 0 || d > depth) continue;
        const t = d / depth;
        // Schlick's Fresnel for water, the view going from grazing at the
        // footing to looking down at the near end
        const cos = 0.12 + 0.6 * t;
        const reflectance = 0.02 + 0.98 * (1 - cos) ** 5;
        const fade = opacity * (1 - smooth(fadeFrom, 1, t)) * (1 - fresnel + fresnel * reflectance);
        const scale = scaleAt(d);
        for (let u = 0; u < MW; u++) {
          const i = v * MW + u;
          along[i] = t;
          const X = x0 + (u + 0.5) / q;
          let [dx, dy, keep, glint] = [0, 0, 1, 0];
          if (water) {
            const [sx, sy] = [slopes[i * 2], slopes[i * 2 + 1]];
            dx = (waves * scale * sx * 0.6) / rmsX;
            dy = (waves * scale * sy) / rms;
            const steep = Math.abs(sy) / rms;
            if (chop > 0) keep = 1 - Math.min(1, chop * 2) * smooth(breakAt - 0.35, breakAt + 0.35, steep);
            if (sparkle > 0) glint = sparkle * smooth(2.6 - sparkle, 3.2 - sparkle, -sy / rms);
          }
          const h = Math.abs(d + dy) / squash;
          sampleAt(f, W, H, X - h * skew + dx, bottom - h, p);
          if (!p[3]) continue;
          const w = fade * keep;
          const at = i * 4;
          for (let c = 0; c < 3; c++) {
            const color = p[c] + (tint[c] * p[3] - p[c]) * tintAmount;
            mirror[at + c] = (color + (p[3] - color) * glint) * w;
          }
          mirror[at + 3] = p[3] * w;
        }
      }
      // Blurrier and more streaked the farther from the footing, a channel
      // at a time
      const spread = (c, radius, vertical) => {
        if (!vertical) return blurChannel(c, MW, MH, radius);
        const out = Float32Array.from(c);
        const r = Math.round(radius);
        if (r < 1) return out;
        const tmp = new Float32Array(c.length);
        boxPass(out, tmp, MW, MH, r, false);
        boxPass(tmp, out, MW, MH, r, false);
        return out;
      };
      let reflected = mirror;
      for (const [amount, vertical] of [
        [blur, false],
        [streak, true],
      ]) {
        if (!(amount > 0.5)) continue;
        const next = new Float32Array(MW * MH * 4);
        const channel = new Float32Array(MW * MH);
        for (let c = 0; c < 4; c++) {
          for (let i = 0; i < MW * MH; i++) channel[i] = reflected[i * 4 + c];
          const levels = [0, 0.5, 1].map((m) => spread(channel, amount * m * q, vertical));
          for (let i = 0; i < MW * MH; i++) {
            const at = along[i] * 2;
            const j = Math.min(1, Math.floor(at));
            const m = at - j;
            next[i * 4 + c] = levels[j][i] * (1 - m) + levels[j + 1][i] * m;
          }
        }
        reflected = next;
      }
      // The surface's own color under the reflection, a rounded oblong
      // soft toward its edge
      const out = new Float32Array(OW * OH * 4);
      for (let y = 0; y < OH; y++) {
        const d = y + y0 + 0.5 - start;
        for (let x = 0; x < OW; x++) {
          const at = (y * OW + x) * 4;
          if (q === 1) p.set(reflected.subarray(at, at + 4));
          else sampleAt(reflected, MW, MH, (x + 0.5) * q, (y + 0.5) * q, p);
          if (surface > 0 && d >= 0) {
            const a = surface * (1 - smooth(0.55, 1, Math.sqrt(Math.hypot(((x + x0 + 0.5 - cx) / rx) ** 2, (d / ry) ** 2)))) * (1 - p[3]);
            for (let c = 0; c < 3; c++) p[c] += tint[c] * a;
            p[3] += a;
          }
          const [sx, sy] = [x + x0, y + y0];
          if (!s.only && sx >= 0 && sy >= 0 && sx < W && sy < H) {
            const from = (sy * W + sx) * 4;
            const keep = 1 - f[from + 3];
            for (let c = 0; c < 4; c++) p[c] = f[from + c] + p[c] * keep;
          }
          out.set(p, at);
        }
      }
      return { image: toImage(out, OW, OH), x: x0, y: y0 };
    },
  });

  // ---------- Rim Light ----------

  // A light behind a cutout catching its edges on one side: each edge lit
  // as much as it faces Direction, found by looking a Width toward the
  // light and seeing how much of the cutout's there. Shade darkens the
  // rest as a backlit subject is, and Halo lets the light spill past its
  // edge
  const RIM_ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><circle ${STROKE} cx="11" cy="13" r="6.5"/><path ${STROKE} d="M8.5 7a6.5 6.5 0 0 1 9 4.5"/><path ${STROKE} d="M18 4.5l-1 1.5M20 8l-1.5.5"/></svg>`;
  pictureEffect({
    id: "rimLight",
    thumb: { sample: "pawnOnNight", backdrop: "night", settings: { direction: -40, width: 3.5, softness: 1, color: "#8fdcff", intensity: 200, shade: 55, halo: 70 } },
    name: "Rim Light",
    about: "Light a cutout's edges from behind",
    icon: RIM_ICON,
    verb: "Add light",
    defaults: { direction: -45, width: 4, softness: 2, intensity: 120, shade: 0, halo: 30, color: "#ffe2b0", editable: true },
    presets: [
      ["Golden hour", () => ({})],
      ["Cool edge", () => ({ direction: 60, color: "#8fdcff", intensity: 140 })],
      ["Backlit", () => ({ direction: 0, width: 6, shade: 45, halo: 50, intensity: 150, color: "#fff4e0" })],
      ["Neon", () => ({ direction: 90, width: 3, softness: 1, intensity: 220, halo: 80, color: "#ff4fd8" })],
      ["Dramatic", () => ({ direction: -60, width: 7, shade: 60, intensity: 170 })],
    ],
    limits: { direction: [-360, 360], width: [0, 1000], softness: [0, 1000], intensity: [0, 1000], shade: [0, 100], halo: [0, 100] },
    summary: (s) => `${s.direction}° · ${s.width}pt`,
    panel: (ui) => [
      ui.section(
        "Light",
        ui.sliders([
          ["direction", "Direction", "°", () => ({ min: -180, max: 180 })],
          ["width", "Width", "pt", (u) => ({ min: 0, max: upTo(u, 0.1, 16), shape: "curve", digits: 1 })],
          ["softness", "Softness", "pt", (u) => ({ min: 0, max: upTo(u, 0.1, 16), shape: "curve", digits: 1 })],
          ["intensity", "Intensity", "%", () => ({ min: 0, max: 300, middle: 100 })],
        ]),
        ui.color("color", "Color"),
      ),
      ui.section(
        "Look",
        ui.sliders([
          ["shade", "Shade", "%", () => ({ min: 0, max: 100 })],
          ["halo", "Halo", "%", () => ({ min: 0, max: 100 })],
        ]),
      ),
    ],
    process: (image, s, k) => {
      const { width: W, height: H } = image;
      const f = premultiplied(image);
      const width = Math.max(0.5, s.width * k);
      const soft = s.softness * k;
      const halo = s.halo / 100;
      const pad = halo > 0 && !isOpaque(f) ? Math.ceil(3 * (width + soft)) + 1 : 0;
      const [OW, OH] = [W + 2 * pad, H + 2 * pad];
      const alpha = new Float32Array(W * H);
      for (let i = 0; i < W * H; i++) alpha[i] = f[i * 4 + 3];
      const near = blurChannel(alpha, W, H, Math.max(0.5, width * 0.5));
      // Toward the light: 0° from above, turning clockwise
      const [lx, ly] = [Math.sin((s.direction * Math.PI) / 180), -Math.cos((s.direction * Math.PI) / 180)];
      // How lit each pixel is, in the padded picture, for the halo
      let rim = new Float32Array(OW * OH);
      for (let y = 0; y < H; y++)
        for (let x = 0; x < W; x++) {
          const a = alpha[y * W + x];
          if (!a) continue;
          const toward = channelAt(near, W, H, x + 0.5 + lx * width, y + 0.5 + ly * width);
          rim[(y + pad) * OW + x + pad] = a * clamp01(1 - toward) ** 1.5;
        }
      rim = blurChannel(rim, OW, OH, soft * 0.5);
      const spill = halo > 0 ? blurChannel(rim, OW, OH, width + soft) : null;
      const color = hexRgb(s.color);
      const [intensity, shade] = [s.intensity / 100, s.shade / 100];
      const out = new Float32Array(OW * OH * 4);
      for (let y = 0; y < OH; y++)
        for (let x = 0; x < OW; x++) {
          const i = y * OW + x;
          const [sx, sy] = [x - pad, y - pad];
          const inside = sx >= 0 && sy >= 0 && sx < W && sy < H;
          const from = (sy * W + sx) * 4;
          const a = inside ? f[from + 3] : 0;
          const lit = rim[i] * a * intensity;
          const glow = spill ? spill[i] * halo * intensity : 0;
          const darken = 1 - shade * (1 - Math.min(1, rim[i] * 2));
          const at = i * 4;
          for (let c = 0; c < 3; c++) out[at + c] = (inside ? f[from + c] * darken : 0) + color[c] * (lit + glow * (1 - a));
          out[at + 3] = a + Math.min(1, glow) * (1 - a);
        }
      return { image: toImage(out, OW, OH), x: -pad, y: -pad };
    },
  });

  // ---------- Ligma effects in the right panel ----------

  // Under Figma's own Effects, drawn as they are but apart from them, the
  // Ligma effects on the selected layer, the last applied on top, as Figma
  // lists its own. A row opens its effect to change, the eye hides or shows
  // it, dragging rows by their grip reorders the stack and − removes one.
  // Reordering and removing make the stack again from the original, each
  // effect with its saved settings, under the old one, which stays until
  // the new one's done, so nothing flashes
  const SECTION = "data-ligma-effects-section";
  const OFF = "off";
  const effectOf = (node) => (node ? EFFECTS.find((effect) => effect.of(node)) : null);
  // The source inside an effect's group
  const sourceOf = (group) => group.children.find((child) => child.getSharedPluginData(PLUGIN, ROLE) === "source") ?? null;
  // The stack `node` is in, outermost first: each an effect's group whose
  // source is the next
  const stackOf = (node) => {
    if (!node) return [];
    if (!effectOf(node) && node.parent && effectOf(node.parent)) node = node.parent;
    if (!effectOf(node)) return [];
    while (node.parent && effectOf(node.parent) && sourceOf(node.parent) === node) node = node.parent;
    const stack = [];
    for (let at = node; at && effectOf(at); at = sourceOf(at)) stack.push(at);
    return stack;
  };
  const isOff = (group) => group.getSharedPluginData(PLUGIN, OFF) === "1";
  // Hides an effect, showing what it was made from, or shows it again
  const setOff = (group, off) => {
    group.setSharedPluginData(PLUGIN, OFF, off ? "1" : "");
    const source = sourceOf(group);
    for (const child of group.children) child.visible = child === source ? off || effectOf(group) === EFFECTS[0] : !off;
  };

  // Makes the stack again from `base` with `layers`, outermost first, each
  // { effect, settings, off }, in place of `outer`. `base` is copied, so
  // the old stack shows until the new one's made
  let restacking = false;
  const restack = async (figma, outer, base, layers) => {
    if (restacking || repeating || disintegrating || crushing || filtering) return;
    restacking = true;
    showSection();
    const parent = outer.parent;
    let top = null;
    try {
      top = base.clone();
      parent.insertChild(parent.children.indexOf(outer), top);
      // A copy of a layer in a group, moved out of it, is placed as if the
      // group were at the page's origin, so it's put back where it was
      top.relativeTransform = relativeFor(top.parent, base.absoluteTransform);
      if (top.getSharedPluginData(PLUGIN, ROLE) === "source") top.setSharedPluginData(PLUGIN, ROLE, "");
      top.visible = true;
      for (const { effect, settings, off } of [...layers].reverse()) {
        figma.currentPage.selection = [top];
        await effect.open(figma, false, settings);
        const made = figma.currentPage.selection[0];
        if (!made || made === top || !effectOf(made)) throw new Error("not made");
        top = made;
        if (off) setOff(top, true);
      }
      // Where the old one was, as it was
      top.visible = outer.visible;
      outer.remove();
      figma.currentPage.selection = [top];
      try {
        figma.commitUndo();
      } catch {}
    } catch {
      try {
        if (top && !top.removed && top !== outer) top.remove();
        if (!outer.removed) figma.currentPage.selection = [outer];
      } catch {}
      figma.notify("Couldn't change the effects", { timeout: 2000 });
    }
    restacking = false;
    sectionKey = "";
    showSection();
  };

  // Each effect in `stack` with its saved settings, as `restack` takes them
  const layersOf = (stack) =>
    stack.map((group) => {
      const effect = effectOf(group);
      let settings = {};
      try {
        settings = effect.saved(group);
      } catch {}
      return { group, effect, settings, off: isOff(group) };
    });

  // Once an effect under others is changed, those above are made again
  // from it, as they were made from what it was before
  const restackAbove = (figma, group) => {
    const stack = stackOf(group);
    const at = stack.indexOf(group);
    if (at > 0) restack(figma, stack[0], group, layersOf(stack.slice(0, at)));
  };

  const icon24 = (d, rule = "") => `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><path fill="var(--fpl-icon-color, var(--color-icon))" ${rule} d="${d}"/></svg>`;
  const REMOVE = icon24("M18 11.5a.5.5 0 0 1-.5.5h-11a.5.5 0 0 1 0-1h11a.5.5 0 0 1 .5.5", 'fill-rule="evenodd"');
  const ADD = icon24("M12 6a.5.5 0 0 1 .5.5v5h5a.5.5 0 0 1 0 1h-5v5a.5.5 0 0 1-1 0v-5h-5a.5.5 0 0 1 0-1h5v-5A.5.5 0 0 1 12 6", 'fill-rule="evenodd"');
  // Figma's eye, open and shut
  const EYE = icon24("M12 8c2.6 0 4.7 1.6 5.9 3.7a.6.6 0 0 1 0 .6C16.7 14.4 14.6 16 12 16s-4.7-1.6-5.9-3.7a.6.6 0 0 1 0-.6C7.3 9.6 9.4 8 12 8m0 1c-2.1 0-3.9 1.3-4.9 3 1 1.7 2.8 3 4.9 3s3.9-1.3 4.9-3c-1-1.7-2.8-3-4.9-3m0 1a2 2 0 1 1 0 4 2 2 0 0 1 0-4m0 1a1 1 0 1 0 0 2 1 1 0 0 0 0-2");
  const EYE_SHUT = icon24("M6.15 11.15a.5.5 0 0 1 .7 0C8 12.3 9.8 13 12 13s4-.7 5.15-1.85a.5.5 0 1 1 .7.7l-.66.6.92 1.3a.5.5 0 1 1-.82.58l-.92-1.3q-.86.53-1.86.82l.36 1.5a.5.5 0 1 1-.97.24l-.37-1.54a8 8 0 0 1-3.06 0l-.37 1.54a.5.5 0 1 1-.97-.24l.36-1.5q-1-.29-1.86-.82l-.92 1.3a.5.5 0 1 1-.82-.58l.92-1.3-.66-.6a.5.5 0 0 1 0-.7");
  const GRIP = `<svg width="16" height="16" fill="none" viewBox="0 0 16 16"><path fill="currentColor" d="M6 4.5a1 1 0 1 1-2 0 1 1 0 0 1 2 0m0 3.5a1 1 0 1 1-2 0 1 1 0 0 1 2 0m-1 4.5a1 1 0 1 0 0-2 1 1 0 0 0 0 2M12 4.5a1 1 0 1 1-2 0 1 1 0 0 1 2 0M11 9a1 1 0 1 0 0-2 1 1 0 0 0 0 2m1 2.5a1 1 0 1 1-2 0 1 1 0 0 1 2 0"/></svg>`;

  // As Figma lays out its own: a 32-pixel title row 16 pixels in, then
  // 32-pixel rows of 24-pixel cards with a 1-pixel inset border, the eye
  // showing on hover in place of the card's end, and − on the right
  const sectionStyle = document.createElement("style");
  sectionStyle.textContent = `
    [${SECTION}] { padding: 4px 0 12px; border-bottom: 1px solid var(--color-border); font-family: var(--font-family-default, Inter, sans-serif); font-size: 11px; font-weight: 450; letter-spacing: 0.005em; color: var(--color-text); -webkit-font-smoothing: antialiased; }
    [${SECTION}] .head { display: flex; align-items: center; height: 32px; padding: 0 8px 0 16px; }
    [${SECTION}] h2 { flex: 1; margin: 0; font-size: 11px; font-weight: 550; letter-spacing: 0.01em; line-height: 32px; }
    [${SECTION}] .icon-button { display: grid; place-items: center; width: 24px; height: 24px; padding: 0; border: 0; border-radius: 5px; background: none; color: inherit; }
    [${SECTION}] .icon-button:hover { background: var(--color-bg-hover); }
    [${SECTION}] .row { position: relative; display: flex; align-items: center; height: 32px; padding: 0 8px 0 16px; transition: transform 120ms ease-out; }
    [${SECTION}] .row.dragging { z-index: 1; transition: none; }
    [${SECTION}] .grip { position: absolute; left: 0; top: 8px; display: grid; place-items: center; width: 16px; height: 16px; color: var(--color-icon-tertiary, var(--color-text-tertiary)); opacity: 0; cursor: grab; }
    [${SECTION}] .row:hover .grip, [${SECTION}] .row.dragging .grip { opacity: 1; }
    [${SECTION}] .card { display: flex; align-items: center; width: 184px; height: 24px; padding: 0 8px 0 0; border: 0; border-radius: 5px; background: var(--color-bg); box-shadow: inset 0 0 0 1px var(--color-border); color: inherit; font: inherit; text-align: left; overflow: hidden; }
    [${SECTION}] .card:hover { box-shadow: inset 0 0 0 1px var(--color-border-strong, #555); }
    [${SECTION}] .row.dragging .card { box-shadow: inset 0 0 0 1px var(--color-border-selected, #0d99ff), var(--elevation-200); }
    [${SECTION}] .card .glyph { flex: none; display: grid; place-items: center; width: 24px; height: 24px; }
    [${SECTION}] .card .glyph svg { width: 16px; height: 16px; }
    [${SECTION}] .card .glyph svg [stroke] { stroke: var(--color-icon); stroke-width: 1.5; }
    [${SECTION}] .card .name { flex: none; }
    [${SECTION}] .card .about { flex: 1; min-width: 0; margin-left: 6px; overflow: hidden; color: var(--color-text-secondary); text-overflow: ellipsis; white-space: nowrap; }
    [${SECTION}] .row.off .card :is(.name, .about, .glyph) { opacity: 0.4; }
    [${SECTION}] .eye { display: none; margin-left: 8px; }
    [${SECTION}] .row:hover .card, [${SECTION}] .row.off .card { width: 152px; }
    [${SECTION}] .row:hover .eye, [${SECTION}] .row.off .eye { display: grid; }
    [${SECTION}] .remove { margin-left: auto; }
    [${SECTION}] .rows.busy { opacity: 0.5; pointer-events: none; }
    [${SECTION}] :focus-visible { outline: 1px solid var(--color-border-selected, #0d99ff); outline-offset: -1px; }`;

  let sectionKey = "";
  const showSection = () => {
    const figma = api();
    const anchor = document.getElementById("effects-panel")?.closest(".cachedSubtree");
    let section = document.querySelector(`[${SECTION}]`);
    const [only] = figma?.currentPage.selection ?? [];
    let stack = [];
    try {
      stack = state.on && figma && figma.currentPage.selection.length === 1 ? stackOf(only) : [];
    } catch {}
    if (!stack.length || !anchor) {
      if (!restacking) {
        section?.remove();
        sectionKey = "";
      }
      return;
    }
    if (!sectionStyle.isConnected) document.head.append(sectionStyle);
    const layers = layersOf(stack);
    const key = layers.map(({ group, settings, off }) => group.id + off + JSON.stringify(settings)).join("|") + restacking;
    if (section && section.previousElementSibling === anchor && key === sectionKey) return;
    sectionKey = key;
    section ??= document.createElement("div");
    section.setAttribute(SECTION, "");
    section.innerHTML = "";
    const head = section.appendChild(document.createElement("div"));
    head.className = "head";
    head.innerHTML = `<h2>Ligma Effects</h2>`;
    const add = head.appendChild(Object.assign(document.createElement("button"), { type: "button", className: "icon-button", innerHTML: ADD }));
    add.setAttribute("aria-label", "Add a Ligma effect");
    Object.assign(add.dataset, { tooltip: "Add effect", tooltipType: "text" });
    add.addEventListener("click", () => {
      figma.currentPage.selection = [stack[0]];
      openMenu(add);
    });
    const rows = section.appendChild(document.createElement("div"));
    rows.className = "rows";
    if (restacking) rows.classList.add("busy");
    const base = sourceOf(stack.at(-1));
    layers.forEach((layer, i) => {
      const row = rows.appendChild(document.createElement("div"));
      row.className = layer.off ? "row off" : "row";
      const grip = row.appendChild(Object.assign(document.createElement("span"), { className: "grip", innerHTML: GRIP }));
      grip.setAttribute("aria-hidden", "true");
      const card = row.appendChild(Object.assign(document.createElement("button"), { type: "button", className: "card" }));
      card.innerHTML = `<span class="glyph">${layer.effect.icon}</span><span class="name">${layer.effect.name}</span><span class="about">${layer.effect.summary(layer.settings)}</span>`;
      card.setAttribute("aria-label", `Edit ${layer.effect.name}`);
      card.addEventListener("click", () => {
        figma.currentPage.selection = [layer.group];
        layer.effect.open(figma, true);
      });
      const eye = row.appendChild(Object.assign(document.createElement("button"), { type: "button", className: "icon-button eye", innerHTML: layer.off ? EYE_SHUT : EYE }));
      eye.setAttribute("aria-label", "Toggle visibility");
      Object.assign(eye.dataset, { tooltip: "Toggle visibility", tooltipType: "text" });
      eye.addEventListener("click", () => {
        if (i === 0) {
          // The top one only changes what shows
          setOff(layer.group, !layer.off);
          try {
            figma.commitUndo();
          } catch {}
          sectionKey = "";
          showSection();
        } else {
          // One under others: those above are made again from it
          setOff(layer.group, !layer.off);
          restack(figma, stack[0], layer.group, layers.slice(0, i));
        }
      });
      const remove = row.appendChild(Object.assign(document.createElement("button"), { type: "button", className: "icon-button remove", innerHTML: REMOVE }));
      remove.setAttribute("aria-label", `Remove ${layer.effect.name}`);
      Object.assign(remove.dataset, { tooltip: "Remove", tooltipType: "text" });
      remove.addEventListener("click", () => restack(figma, stack[0], base, layers.filter((_, j) => j !== i)));
      // Dragging a row moves it among the others, and letting go makes the
      // stack again in the new order
      grip.addEventListener("pointerdown", (event) => {
        if (event.button !== 0 || layers.length < 2 || restacking) return;
        event.preventDefault();
        grip.setPointerCapture(event.pointerId);
        row.classList.add("dragging");
        const height = row.offsetHeight;
        const from = event.clientY;
        let to = i;
        const move = (event) => {
          const dy = event.clientY - from;
          row.style.transform = `translateY(${dy}px)`;
          to = Math.max(0, Math.min(layers.length - 1, i + Math.round(dy / height)));
          [...rows.children].forEach((other, j) => {
            if (other === row) return;
            const shift = i < to && j > i && j <= to ? -height : i > to && j < i && j >= to ? height : 0;
            other.style.transform = shift ? `translateY(${shift}px)` : "";
          });
        };
        grip.addEventListener("pointermove", move);
        grip.addEventListener(
          "lostpointercapture",
          () => {
            grip.removeEventListener("pointermove", move);
            row.classList.remove("dragging");
            if (to === i) {
              for (const other of rows.children) other.style.transform = "";
              return;
            }
            // Kept where they were dropped while the stack's made again
            row.style.transform = `translateY(${(to - i) * height}px)`;
            const order = [...layers];
            order.splice(to, 0, order.splice(i, 1)[0]);
            restack(figma, stack[0], base, order);
          },
          { once: true },
        );
      });
    });
    if (section.previousElementSibling !== anchor) anchor.after(section);
  };

  // ---------- Setting ----------

  // Every half second while it's on, so the button and section follow
  // Figma's toolbar and panel. Off, it's all taken away and stops
  let ticking = null;
  const tick = () => {
    if (!state.on) {
      clearInterval(ticking);
      ticking = null;
      button()?.remove();
      closeMenu();
      cancelOpen();
      document.querySelector(`[${SECTION}]`)?.remove();
      style.remove();
      sectionStyle.remove();
      stopCrushers();
      crushers = null;
      return;
    }
    ticking ??= setInterval(tick, 500);
    if (!style.isConnected && document.head) document.head.append(style);
    place();
    try {
      showSection();
    } catch {}
  };
  ticking = setInterval(tick, 500);

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
