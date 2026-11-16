//! Raster mode (Settings > Editor > Toolbar): a mode in the switch on the
//! right of Figma Design's toolbar, after Design, that swaps Figma's tools
//! for Photoshop's, painting in pixels. On by default
//!
//! Figma's switch is handed the modes it shows (see toolbar.rs), so
//! `PAGE_JS` adds Raster to them as its module loads (see `modules.rs`).
//! Picking it keeps Figma in Design, hides Figma's tools and shows Ligma's
//! in their place: compact buttons for the layer above, a new layer and
//! the layer below, then Move, the marquees, the lassos, the magic wand,
//! the eyedropper, the brush and pencil, the clone stamp, the eraser and
//! magic eraser, the paint bucket and gradient, blur, sharpen and smudge,
//! dodge, burn and sponge, and the foreground and background colors. Each
//! tool's options show in a bar above the toolbar, where Figma shows its
//! own for a selection. Photoshop's shortcuts pick them
//!
//! A layer is painted on if it has an image fill, the top one: images
//! dropped into the file as well as the layers Raster mode makes,
//! rectangles filled with a transparent image two pixels to the point, as
//! large as their frame, up to 4,096 pixels across, as Figma keeps images.
//! Painting with nothing to paint on adds one. Its pixels are read through
//! Figma's console plugin API, `window.figma`, which Figma only offers in
//! files that can be edited, so the mode is greyed out in others. While
//! painting, a canvas over Figma's shows the layer, and strokes in quick
//! succession replace its image together, as one undo step, so Figma's
//! undo works as it does for anything else. The canvas stays until Figma
//! has drawn the new image sharply, then fades. It follows the layer as
//! it's moved, turned or resized in Figma, goes at once when the layer is
//! undone, changed by someone else, hidden or deleted, and as soon as Figma
//! has the strokes when anything above it changes (Figma's
//! `documentchange`), so it never hides Figma's own. Erasing hides Figma's copy
//! of the layer until the change is in, so what's under it shows
//!
//! The selection is a mask on a layer's pixels, drawn with marching ants,
//! that strokes, fills and erasing stay inside, as in Photoshop. Figma's
//! own keys for its tools are kept out while the mode is on, as no button
//! would show them

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::settings::Settings;

/// Runs in Figma tabs, after `modules::PAGE_JS` and `toolbar::page_js`, so
/// its wrap goes around the toolbar's. `__LIGMA_RASTER_MODE__` is `{ on,
/// rev }`: the setting when the tab was made. A page loaded later in the
/// same tab may find a newer one in `localStorage`; the higher `rev` wins
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window || !window.__ligmaModules) return;
  const KEY = "ligma-raster-mode";
  let state = __LIGMA_RASTER_MODE__;
  try {
    const stored = JSON.parse(localStorage.getItem(KEY));
    if (stored?.rev > state.rev) state = stored;
  } catch {}

  const RASTER = "ligma-raster";
  const ATTR = "data-ligma-raster";
  const SVG = "http://www.w3.org/2000/svg";
  const mac = navigator.userAgent.includes("Mac");
  // Figma keeps images at most this many pixels across
  const LARGEST = 4096;
  // Pixels per point of the layers Raster mode makes
  const DENSITY = 2;

  // The plugin API, if the file can be edited
  const api = () => {
    const figma = window.figma;
    try {
      return figma?.currentPage && figma.editorType === "figma" ? figma : null;
    } catch {
      return null;
    }
  };
  const notify = (message) => {
    try {
      window.figma?.notify(message, { timeout: 2500 });
    } catch {}
  };
  const swallow = (event) => {
    event.preventDefault();
    event.stopImmediatePropagation();
  };
  const typing = (event) => event.target?.closest?.(':is(input, textarea):not([readonly]), [contenteditable]:not([contenteditable="false"])');
  // Figma's canvas, the largest, not one of Raster mode's own
  const mainCanvas = () =>
    [...document.querySelectorAll("canvas")].filter((canvas) => !canvas.closest(`[${ATTR}-overlay]`)).reduce(
      (best, canvas) => (canvas.offsetWidth * canvas.offsetHeight > (best ? best.offsetWidth * best.offsetHeight : 0) ? canvas : best),
      null,
    );
  const clamp = (value, min, max) => Math.min(max, Math.max(min, value));
  const make = (tag, props = {}, ...children) => {
    const node = tag.startsWith("svg:") ? document.createElementNS(SVG, tag.slice(4)) : document.createElement(tag);
    for (const [key, value] of Object.entries(props)) {
      if (value === undefined || value === null || value === false) continue;
      if (key === "class") node.setAttribute("class", value);
      else if (key === "text") node.textContent = value;
      else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
      else node.setAttribute(key, value === true ? "" : value);
    }
    node.append(...children.filter(Boolean));
    return node;
  };
  const canvasOf = (width, height) => Object.assign(document.createElement("canvas"), { width, height });
  const copyOf = (canvas) => {
    const copy = canvasOf(canvas.width, canvas.height);
    copy.getContext("2d").drawImage(canvas, 0, 0);
    return copy;
  };
  const tooltip = (node, text, shortcut = "") => {
    Object.assign(node.dataset, { tooltip: text, tooltipType: "text", tooltipShowAbove: "true", tooltipShortcut: shortcut });
    node.setAttribute("aria-label", text);
    return node;
  };

  // Icons, drawn on Figma's 24 point grid in 1 point strokes
  const STROKE = 'fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round"';
  const icon = (body, size = 24) => `<svg width="${size}" height="${size}" viewBox="0 0 ${size} ${size}" fill="none" aria-hidden="true">${body}</svg>`;
  const ICONS = {
    move: `<path fill="currentColor" d="M4.586 4.586a2 2 0 0 1 2.005-.497l.14.05 13 5.107a2 2 0 0 1 1.267 1.779v.159a2 2 0 0 1-1.26 1.782l-.15.053-5.024 1.545-1.545 5.024a2 2 0 0 1-1.677 1.398l-.158.012a2 2 0 0 1-1.938-1.267l-5.107-13a2 2 0 0 1 .447-2.145m1.78.484a1 1 0 0 0-1.073.223l-.097.112a1 1 0 0 0-.127.96l5.108 13a1 1 0 0 0 .811.628l.158.006a1 1 0 0 0 .859-.558l.058-.147 1.7-5.53 5.531-1.701a1 1 0 0 0 .687-.76l.018-.157a1 1 0 0 0-.492-.9l-.142-.069z"/>`,
    marquee: `<rect ${STROKE} x="5.5" y="5.5" width="13" height="13" rx="1" stroke-dasharray="2 2"/>`,
    ellipse: `<ellipse ${STROKE} cx="12" cy="12" rx="7" ry="6" stroke-dasharray="2 2"/>`,
    lasso: `<path ${STROKE} d="M8.7 15.6C6.5 14.8 5 13.2 5 11.3 5 8.4 8.1 6 12 6s7 2.4 7 5.3-3.1 5.3-7 5.3q-1.2 0-2.3-.3"/><circle ${STROKE} cx="8.5" cy="16.5" r="1.5"/><path ${STROKE} d="M8 18c-.3.9.1 1.6.8 2"/>`,
    polygon: `<path ${STROKE} d="M6 17 5 8.5l7-3.5 7 5.5-4 6.5z" stroke-dasharray="2 2"/><circle fill="currentColor" cx="6" cy="17" r="1.25"/><circle fill="currentColor" cx="5" cy="8.5" r="1.25"/><circle fill="currentColor" cx="12" cy="5" r="1.25"/><circle fill="currentColor" cx="19" cy="10.5" r="1.25"/><circle fill="currentColor" cx="15" cy="17" r="1.25"/>`,
    wand: `<path ${STROKE} d="m5 19 9.5-9.5"/><path ${STROKE} d="m13 11 1.5 1.5"/><path ${STROKE} d="M17 4v2.5M17 10.5V12m2-4h2.5M12.5 8H14m4.5-2.5 1.2-1.2m-5.4 5.4 1.2-1.2m4.2 0 1.2 1.2"/>`,
    eyedropper: `<path ${STROKE} d="M15.8 5.2a2 2 0 0 1 2.9 2.9l-1.8 1.8.8.8-1.2 1.2-4.6-4.6 1.2-1.2.8.8z"/><path ${STROKE} d="m12.7 9.3-6.4 6.4-.8 2.8 0 0 2.8-.8 6.4-6.4"/>`,
    brush: `<path ${STROKE} d="M18.9 5.1a1.3 1.3 0 0 0-1.8 0l-6.6 7.1 1.3 1.3 7.1-6.6a1.3 1.3 0 0 0 0-1.8z"/><path ${STROKE} d="M10.4 13.4c-1.6-.3-3 .8-3.3 2.4-.2 1.2-.7 2.1-1.6 2.7 2.3.7 4.7.2 5.7-1.5.5-.9.4-2.1-.4-2.9z"/>`,
    pencil: `<path ${STROKE} d="m15.3 5.7 3 3-9.6 9.6-3.9.9.9-3.9z"/><path ${STROKE} d="m13.3 7.7 3 3"/><path fill="currentColor" d="m4.8 19.2.5-2.3 1.8 1.8z"/>`,
    clone: `<circle ${STROKE} cx="12" cy="6.5" r="2"/><path ${STROKE} d="M11 8.5v3h2v-3"/><path ${STROKE} d="M7 11.5h10a1.5 1.5 0 0 1 1.5 1.5v2.5h-13V13A1.5 1.5 0 0 1 7 11.5zM6.5 18.5h11"/>`,
    eraser: `<path ${STROKE} d="m13.4 5.4 5.2 5.2a1 1 0 0 1 0 1.4l-6.5 6.5H8.4l-2.9-2.9a1 1 0 0 1 0-1.4z"/><path ${STROKE} d="m9.2 9.6 5.2 5.2M12.5 18.5h6"/>`,
    magicEraser: `<path ${STROKE} d="m11.4 7.4 5.2 5.2a1 1 0 0 1 0 1.4l-4.5 4.5H8.4l-2.9-2.9a1 1 0 0 1 0-1.4z"/><path ${STROKE} d="m8.2 10.6 4.2 4.2"/><path ${STROKE} d="M17.5 3.5v3m-1.5-1.5h3M20 9v1.5m-.75-.75h1.5"/>`,
    bucket: `<path ${STROKE} d="m10.5 5.5 7 7-6 6a1.4 1.4 0 0 1-2 0l-5-5a1.4 1.4 0 0 1 0-2z"/><path ${STROKE} d="M5 12.5h12.5M8.5 7.5l-2-2"/><path ${STROKE} d="M19.5 14.5s1.5 1.9 1.5 2.9a1.5 1.5 0 0 1-3 0c0-1 1.5-2.9 1.5-2.9z"/>`,
    gradient: `<defs><linearGradient id="ligma-raster-gradient" x1="0" x2="1" y1="0" y2="0"><stop offset="0" stop-color="currentColor"/><stop offset="1" stop-color="currentColor" stop-opacity="0"/></linearGradient></defs><rect ${STROKE} x="5.5" y="5.5" width="13" height="13" rx="2"/><rect x="7" y="7" width="10" height="10" rx="1" fill="url(#ligma-raster-gradient)"/>`,
    blur: `<path ${STROKE} d="M12 4.5s5.5 5.8 5.5 9.5a5.5 5.5 0 0 1-11 0c0-3.7 5.5-9.5 5.5-9.5z"/>`,
    sharpen: `<path ${STROKE} d="M12 4.5 17.5 19h-11z"/>`,
    smudge: `<path ${STROKE} d="M10.5 19.5v-4.8l-2.2-2.3a1.2 1.2 0 0 1 1.7-1.7l1.5 1.5V5.7a1.25 1.25 0 0 1 2.5 0v5.1l3.2.7a1.6 1.6 0 0 1 1.2 1.8l-.6 4-.6 2.2"/>`,
    dodge: `<circle ${STROKE} cx="9.5" cy="9.5" r="4"/><path ${STROKE} d="m12.5 12.5 6.5 6.5"/>`,
    burn: `<path ${STROKE} d="M8 19.5v-3.8c0-1-1.5-2.2-1.5-4.2a5.5 5.5 0 0 1 11 0c0 2-1.5 3.2-1.5 4.2v3.8"/><circle ${STROKE} cx="12" cy="11.5" r="2"/>`,
    sponge: `<rect ${STROKE} x="5.5" y="7.5" width="13" height="9" rx="3"/><circle fill="currentColor" cx="9.5" cy="11" r="1"/><circle fill="currentColor" cx="14" cy="10.5" r=".75"/><circle fill="currentColor" cx="12.5" cy="13.5" r=".75"/><circle fill="currentColor" cx="15.5" cy="13.5" r="1"/>`,
    // The mode: a square of pixels
    raster: `<rect ${STROKE} x="5.5" y="5.5" width="13" height="13" rx="2"/><path fill="currentColor" d="M8 8h4v4H8zM12 12h4v4h-4z"/>`,
  };
  const SMALL = {
    up: `<path ${STROKE} d="M8 12.5v-9M4.5 7 8 3.5 11.5 7"/>`,
    down: `<path ${STROKE} d="M8 3.5v9M4.5 9 8 12.5 11.5 9"/>`,
    add: `<path ${STROKE} d="M8 3.5v9M3.5 8h9"/>`,
    chevron: `<path fill="currentColor" d="M13.647 10.647a.5.5 0 0 1 .707.707l-2.5 2.5a.5.5 0 0 1-.707 0l-2.5-2.5a.5.5 0 0 1 .707-.707l2.146 2.146z"/>`,
    check: `<path ${STROKE} d="m4 8.5 2.5 2.5L12 5"/>`,
    swap: `<path ${STROKE} d="M3 7V5.5A2.5 2.5 0 0 1 5.5 3H9M7.5 1.5 9 3 7.5 4.5"/>`,
    picker: `<path ${STROKE} d="M10.6 3.4a1.4 1.4 0 0 1 2 2l-1.2 1.2.6.6-.8.8-3.2-3.2.8-.8.6.6z"/><path ${STROKE} d="M8.4 6.2 4 10.6l-.5 1.9 1.9-.5 4.4-4.4"/>`,
    layer: `<path ${STROKE} d="M8 2.5 13.5 5.5 8 8.5 2.5 5.5z"/><path ${STROKE} d="m2.5 8.5 5.5 3 5.5-3"/>`,
  };

  // Tools, in their groups as the toolbar shows them, with Photoshop's
  // shortcuts: a group's key picks its tool, and with Shift the next
  const TOOLS = {
    move: { label: "Move", key: "V", icon: "move" },
    marquee: { label: "Rectangular marquee", key: "M", icon: "marquee", select: true },
    ellipse: { label: "Elliptical marquee", key: "M", icon: "ellipse", select: true },
    lasso: { label: "Lasso", key: "L", icon: "lasso", select: true },
    polygon: { label: "Polygonal lasso", key: "L", icon: "polygon", select: true },
    wand: { label: "Magic wand", key: "W", icon: "wand", select: true },
    eyedropper: { label: "Eyedropper", key: "I", icon: "eyedropper" },
    brush: { label: "Brush", key: "B", icon: "brush", paints: true },
    pencil: { label: "Pencil", key: "B", icon: "pencil", paints: true },
    clone: { label: "Clone stamp", key: "S", icon: "clone", paints: true },
    eraser: { label: "Eraser", key: "E", icon: "eraser", paints: true },
    magicEraser: { label: "Magic eraser", key: "E", icon: "magicEraser" },
    bucket: { label: "Paint bucket", key: "G", icon: "bucket" },
    gradient: { label: "Gradient", key: "G", icon: "gradient" },
    blur: { label: "Blur", key: "R", icon: "blur", paints: true, direct: true },
    sharpen: { label: "Sharpen", key: "R", icon: "sharpen", paints: true, direct: true },
    smudge: { label: "Smudge", key: "R", icon: "smudge", paints: true, direct: true },
    dodge: { label: "Dodge", key: "O", icon: "dodge", paints: true, direct: true },
    burn: { label: "Burn", key: "O", icon: "burn", paints: true, direct: true },
    sponge: { label: "Sponge", key: "O", icon: "sponge", paints: true, direct: true },
  };
  const GROUPS = [
    ["move"],
    ["marquee", "ellipse"],
    ["lasso", "polygon"],
    ["wand"],
    ["eyedropper"],
    ["brush", "pencil"],
    ["clone"],
    ["eraser", "magicEraser"],
    ["bucket", "gradient"],
    ["blur", "sharpen", "smudge"],
    ["dodge", "burn", "sponge"],
  ];
  const GROUP_NAMES = ["Move", "Marquee tools", "Lasso tools", "Magic wand", "Eyedropper", "Brush tools", "Clone stamp", "Eraser tools", "Fill tools", "Blur tools", "Toning tools"];
  const groupOf = (tool) => GROUPS.find((group) => group.includes(tool));

  // What each tool's options are, and where they start
  const DEFAULTS = {
    marquee: { mode: "new", feather: 0 },
    ellipse: { mode: "new", feather: 0 },
    lasso: { mode: "new", feather: 0 },
    polygon: { mode: "new", feather: 0 },
    wand: { mode: "new", tolerance: 32, contiguous: true, all: false, smooth: true },
    eyedropper: { sample: 1, all: true },
    brush: { size: 24, angle: 0, roundness: 100, hardness: 80, opacity: 100, smoothing: 10, pressure: true },
    pencil: { size: 4, angle: 0, roundness: 100, opacity: 100, smoothing: 0, pressure: false },
    clone: { size: 40, angle: 0, roundness: 100, hardness: 50, opacity: 100, smoothing: 0, aligned: true },
    eraser: { size: 40, angle: 0, roundness: 100, hardness: 80, opacity: 100, smoothing: 0, pressure: true },
    magicEraser: { tolerance: 32, contiguous: true, opacity: 100 },
    bucket: { tolerance: 32, contiguous: true, all: false, opacity: 100 },
    gradient: { shape: "linear", colors: "background", opacity: 100, reverse: false },
    blur: { size: 40, angle: 0, roundness: 100, hardness: 0, strength: 50, smoothing: 0 },
    sharpen: { size: 40, angle: 0, roundness: 100, hardness: 0, strength: 30, smoothing: 0 },
    smudge: { size: 30, angle: 0, roundness: 100, hardness: 0, strength: 50, smoothing: 0 },
    dodge: { size: 60, angle: 0, roundness: 100, hardness: 0, exposure: 30, smoothing: 0 },
    burn: { size: 60, angle: 0, roundness: 100, hardness: 0, exposure: 30, smoothing: 0 },
    sponge: { size: 60, angle: 0, roundness: 100, hardness: 0, flow: 40, smoothing: 0, saturate: false },
  };

  // The tool, its options and the colors, kept between files
  const PREFS = "ligma-raster-prefs";
  const prefs = { tool: "brush", picked: {}, options: {}, foreground: [0, 0, 0], background: [255, 255, 255], recent: [] };
  try {
    const stored = JSON.parse(localStorage.getItem(PREFS));
    if (stored && typeof stored === "object") Object.assign(prefs, stored);
  } catch {}
  if (!TOOLS[prefs.tool]) prefs.tool = "brush";
  for (const [tool, options] of Object.entries(DEFAULTS)) prefs.options[tool] = { ...options, ...(prefs.options[tool] ?? {}) };
  let saveTimer = 0;
  const savePrefs = () => {
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => {
      try {
        localStorage.setItem(PREFS, JSON.stringify(prefs));
      } catch {}
    }, 300);
  };
  const options = () => prefs.options[prefs.tool] ?? {};

  // Whether this page is in Raster mode, and React's view of it
  let active = false;
  const listeners = new Set();
  const subscribe = (listener) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  };
  const snapshot = () => `${state.on}|${active}|${!!api()}`;
  const emit = () => {
    for (const listener of listeners) listener();
  };
  // The switch on screen, for its mode and how it switches
  let shown = null;
  const ACTIVE_KEY = `ligma-raster-active:${location.pathname.split("/")[2] ?? ""}`;

  // Figma's mode switch, as in toolbar.rs, with Raster after Design
  const MODULE = (source) => source.includes('"toolbelt-mode-segmented-control"');
  window.__ligmaModules.patch([
    {
      match: MODULE,
      getter: (Switch, React) => {
        if (typeof Switch !== "function" || !React || !String(Switch).includes("activeMode")) return Switch;
        const icon = React.createElement("svg", {
          width: 24,
          height: 24,
          viewBox: "0 0 24 24",
          fill: "none",
          style: { color: "var(--fpl-icon-color, var(--color-icon))" },
          dangerouslySetInnerHTML: { __html: ICONS.raster },
        });
        const Modes = (props) => {
          const [on, inRaster, editable] = React.useSyncExternalStore(subscribe, snapshot).split("|");
          const modes = props.modes;
          const design = Array.isArray(modes) ? modes.findIndex((item) => item.mode === "design") : -1;
          React.useEffect(() => {
            if (design < 0) return;
            shown = props;
            return () => {
              if (shown === props) shown = null;
            };
          });
          // Figma left Design on its own, by a shortcut or the menu
          React.useEffect(() => {
            if (active && design >= 0 && props.activeMode !== "design") exit();
          }, [props.activeMode]);
          React.useEffect(() => {
            if (design >= 0) resume();
          }, [editable]);
          if (on !== "true" || design < 0) return React.createElement(Switch, props);
          const item = { mode: RASTER, label: "Raster", icon, disabled: editable !== "true" };
          const next = [...modes.slice(0, design + 1), item, ...modes.slice(design + 1)];
          const onModeSwitch = (mode) => {
            if (mode === RASTER) return enter();
            const was = inRaster === "true";
            exit();
            if (mode !== props.activeMode || !was) props.onModeSwitch?.(mode);
          };
          return React.createElement(Switch, { ...props, modes: next, activeMode: inRaster === "true" ? RASTER : props.activeMode, onModeSwitch });
        };
        Modes.displayName = "LigmaRasterMode";
        return Modes;
      },
    },
  ]);

  // Back into Raster mode when the page reloads in it
  let resumed = false;
  const resume = () => {
    if (resumed || active || !state.on || !api()) return;
    resumed = true;
    try {
      if (sessionStorage.getItem(ACTIVE_KEY) === "1") enter();
    } catch {}
  };

  const enter = () => {
    if (!state.on || active) return;
    if (!api()) {
      notify("Raster mode works in files you can edit");
      return;
    }
    if (shown && shown.activeMode !== "design") shown.onModeSwitch?.("design");
    active = true;
    try {
      sessionStorage.setItem(ACTIVE_KEY, "1");
    } catch {}
    document.documentElement.setAttribute(ATTR, "");
    // Figma's own tool goes back to Move, for the Move tool and for the
    // pointer over the canvas
    document.querySelector('[data-testid="Move-tool"]:not([aria-hidden="true"])')?.click();
    emit();
    mount();
    loop();
    watchDocument(true);
  };
  const exit = () => {
    if (!active) return;
    cancelAll();
    save(target);
    active = false;
    try {
      sessionStorage.removeItem(ACTIVE_KEY);
    } catch {}
    document.documentElement.removeAttribute(ATTR);
    closeMenus();
    ui.tools?.remove();
    ui.bar?.remove();
    ui.overlay?.remove();
    setCursor(null);
    selection = null;
    showing = null;
    watchDocument(false);
    emit();
    releaseMemory();
  };

  // ---------------------------------------------------------------------
  // Geometry. Figma's transforms are [[a, c, e], [b, d, f]], DOMMatrix's
  // [a, b, c, d, e, f]

  const matrix = (transform) => new DOMMatrix([transform[0][0], transform[1][0], transform[0][1], transform[1][1], transform[0][2], transform[1][2]]);
  // From the page to the canvas element's own pixels, which are CSS pixels
  const view = () => {
    const figma = api();
    const canvas = mainCanvas();
    if (!figma || !canvas) return null;
    try {
      const { bounds, zoom } = figma.viewport;
      return { canvas, zoom, bounds, toScreen: new DOMMatrix().scale(zoom).translate(-bounds.x, -bounds.y), rect: canvas.getBoundingClientRect() };
    } catch {
      return null;
    }
  };
  const pageAt = (event, v = view()) => (v ? [v.bounds.x + (event.clientX - v.rect.left) / v.zoom, v.bounds.y + (event.clientY - v.rect.top) / v.zoom] : null);
  const apply = (m, [x, y]) => {
    const point = m.transformPoint(new DOMPoint(x, y));
    return [point.x, point.y];
  };

  // ---------------------------------------------------------------------
  // Layers. Any layer with an image fill can be painted on, its top one,
  // as Photoshop paints on a layer: a photo dropped in as well as the
  // layers Raster mode makes, rectangles filled with an image as large as
  // them

  const fillsOf = (node) => {
    try {
      const fills = node?.fills;
      return Array.isArray(fills) ? fills : [];
    } catch {
      return [];
    }
  };
  const imageIndex = (node) => fillsOf(node).findLastIndex((paint) => paint.type === "IMAGE" && paint.imageHash && paint.visible !== false);
  const selected = () => {
    const figma = api();
    try {
      const selection = figma?.currentPage.selection ?? [];
      return selection.length === 1 && !selection[0].removed ? selection[0] : null;
    } catch {
      return null;
    }
  };
  const isRaster = (node) => !!node && !node.removed && "fills" in node && node.width > 0 && node.height > 0 && imageIndex(node) >= 0;
  const rasterNode = () => {
    const node = selected();
    return isRaster(node) ? node : null;
  };

  // The layer being painted on, read into `base`, its pixels as Figma has
  // them, and `live`, what's shown while painting. `hash` is its image as
  // Ligma last read or wrote it, to know when it's changed by an undo or
  // someone else
  let target = null;
  let reading = null;
  const releaseMemory = () => {
    target = null;
    reading = null;
    scratch = null;
    strokeBuffer = null;
  };

  // How the image fill draws in its layer, `[width, height]` in pixels
  // and a draw onto a canvas that size
  const layout = (paint, node, bitmap) => {
    const [w, h, iw, ih] = [node.width, node.height, bitmap.width, bitmap.height];
    const fit = (cover) => {
      const turns = Math.round(((paint.rotation ?? 0) % 360) / 90);
      const quarter = Math.abs(turns) % 2 === 1;
      const [rw, rh] = quarter ? [ih, iw] : [iw, ih];
      const k = (cover ? Math.max : Math.min)(w / rw, h / rh);
      const scale = Math.min(1 / k, LARGEST / Math.max(w, h));
      const size = [Math.max(1, Math.round(w * scale)), Math.max(1, Math.round(h * scale))];
      return [
        size,
        (ctx) => {
          ctx.translate(size[0] / 2, size[1] / 2);
          ctx.rotate((turns * Math.PI) / 2);
          const [dw, dh] = [iw * k * scale, ih * k * scale];
          ctx.drawImage(bitmap, -dw / 2, -dh / 2, dw, dh);
        },
      ];
    };
    if (paint.scaleMode === "FIT") return fit(false);
    if (paint.scaleMode === "TILE") {
      const s = paint.scalingFactor ?? 1;
      const scale = Math.min(1 / s, LARGEST / Math.max(w, h));
      const size = [Math.max(1, Math.round(w * scale)), Math.max(1, Math.round(h * scale))];
      return [
        size,
        (ctx) => {
          const pattern = ctx.createPattern(bitmap, "repeat");
          pattern.setTransform(new DOMMatrix().scale(s * scale));
          ctx.fillStyle = pattern;
          ctx.fillRect(0, 0, size[0], size[1]);
        },
      ];
    }
    if (paint.scaleMode === "CROP" && paint.imageTransform) {
      // The transform takes the layer's unit square into the image's
      const t = matrix(paint.imageTransform);
      const size = [Math.max(1, Math.round(Math.hypot(t.a, t.b) * iw)), Math.max(1, Math.round(Math.hypot(t.c, t.d) * ih))];
      const shrink = Math.min(1, LARGEST / Math.max(...size));
      size[0] = Math.max(1, Math.round(size[0] * shrink));
      size[1] = Math.max(1, Math.round(size[1] * shrink));
      return [
        size,
        (ctx) => {
          ctx.setTransform(new DOMMatrix().scale(size[0], size[1]).multiply(t.inverse()).scale(1 / iw, 1 / ih));
          ctx.drawImage(bitmap, 0, 0);
        },
      ];
    }
    return fit(true);
  };

  const describe = (node, index) => {
    const paint = fillsOf(node)[index];
    return { node, id: node.id, index, paint, hash: paint.imageHash, w: node.width, h: node.height, M: matrix(node.absoluteTransform) };
  };
  // A layer's pixel grid: from its pixels to the page
  const gridOf = (t) => t.M.multiply(new DOMMatrix().scale(t.w / t.W, t.h / t.H));

  // Reads the layer's image, unless it's the one read last
  const read = (node) => {
    const index = imageIndex(node);
    if (index < 0) return Promise.resolve(null);
    const paint = fillsOf(node)[index];
    // Figma keeps sizes as 32-bit floats, so they come back a little off
    const same = (a, b) => Math.abs(a - b) <= Math.max(1e-3, Math.abs(b) * 1e-6);
    const fresh = target && target.id === node.id && target.hash === paint.imageHash && same(target.w, node.width) && same(target.h, node.height);
    if (fresh) {
      Object.assign(target, { node, index, paint, M: matrix(node.absoluteTransform) });
      return Promise.resolve(target);
    }
    if (reading?.id === node.id && reading.hash === paint.imageHash) return reading.promise;
    const figma = api();
    const promise = (async () => {
      const bytes = await figma.getImageByHash(paint.imageHash).getBytesAsync();
      const bitmap = await createImageBitmap(new Blob([bytes]));
      const [[W, H], draw] = layout(paint, node, bitmap);
      const base = canvasOf(W, H);
      const ctx = base.getContext("2d");
      ctx.imageSmoothingQuality = "high";
      draw(ctx);
      bitmap.close?.();
      const next = { ...describe(node, index), W, H, base, live: copyOf(base), pending: 0 };
      if (reading?.promise === promise) reading = null;
      target = next;
      return next;
    })();
    promise.catch(() => {
      if (reading?.promise === promise) reading = null;
    });
    reading = { id: node.id, hash: paint.imageHash, promise };
    return promise;
  };

  // The frame, component or section a new layer goes in at `point`:
  // the innermost under it, shown and unlocked
  const CONTAINERS = new Set(["FRAME", "COMPONENT", "SECTION"]);
  const contains = (node, [x, y]) => {
    const inverse = matrix(node.absoluteTransform).inverse();
    const [u, v] = apply(inverse, [x, y]);
    return u >= 0 && v >= 0 && u <= node.width && v <= node.height;
  };
  const containerAt = (figma, point) => {
    let parent = figma.currentPage;
    for (;;) {
      const next = [...parent.children].reverse().find((node) => CONTAINERS.has(node.type) && node.visible && !node.locked && contains(node, point));
      if (!next) return parent === figma.currentPage ? null : parent;
      parent = next;
    }
  };
  const frameAbove = (node) => {
    for (let at = node?.parent; at && at.type !== "PAGE" && at.type !== "DOCUMENT"; at = at.parent) if (CONTAINERS.has(at.type)) return at;
    return null;
  };

  // Where a new layer goes, as Photoshop adds one above the layer you're
  // on and as large as the canvas: above the selected layer, or at the
  // top of the selected frame, or of the frame under `point`, as large as
  // the frame. A raster layer's new one shares its grid. With no frame,
  // as large as the canvas on screen
  const placement = (figma, point) => {
    const node = selected();
    if (node && isRaster(node) && node.parent) {
      return { parent: node.parent, index: node.parent.children.indexOf(node) + 1, M: matrix(node.absoluteTransform), w: node.width, h: node.height, like: node };
    }
    let parent = null;
    let index = null;
    if (node && CONTAINERS.has(node.type)) parent = node;
    else if (node?.parent && node.parent.type !== "PAGE") {
      parent = node.parent;
      index = parent.children.indexOf(node) + 1;
    } else if (node?.parent?.type === "PAGE") {
      parent = null;
    } else if (point) parent = containerAt(figma, point);
    // Not inside an instance, which takes no new layers
    for (let at = parent; at && at.type !== "PAGE"; at = at.parent) {
      if (at.type === "INSTANCE") {
        parent = at.parent;
        index = null;
      }
    }
    if (parent && (parent.type === "PAGE" || !("appendChild" in parent))) parent = null;
    const frame = parent && (CONTAINERS.has(parent.type) ? parent : frameAbove(parent));
    if (frame) {
      // Where the frame is in the space its new layer is placed in: a
      // group's children are placed in the space of what holds the group
      let space = parent;
      while (space.type === "GROUP" || space.type === "BOOLEAN_OPERATION") space = space.parent;
      const local = matrix(space.absoluteTransform).inverse().multiply(matrix(frame.absoluteTransform));
      return { parent, index, M: matrix(frame.absoluteTransform), w: frame.width, h: frame.height, local };
    }
    const { bounds } = figma.viewport;
    return { parent: figma.currentPage, index: node?.parent?.type === "PAGE" ? figma.currentPage.children.indexOf(node) + 1 : null, M: new DOMMatrix().translate(bounds.x, bounds.y), w: bounds.width, h: bounds.height };
  };
  // A blank layer's pixels for a placement
  const blank = (place) => {
    const scale = Math.min(DENSITY, LARGEST / Math.max(place.w, place.h));
    const [W, H] = [Math.max(1, Math.round(place.w * scale)), Math.max(1, Math.round(place.h * scale))];
    return { node: null, id: null, index: 0, paint: null, hash: null, w: place.w, h: place.h, M: place.M, W, H, base: canvasOf(W, H), live: canvasOf(W, H), pending: 0, place };
  };
  const layerName = (parent) => {
    let most = 0;
    for (const child of parent?.children ?? []) {
      const found = /^Layer (\d+)$/.exec(child.name);
      if (found) most = Math.max(most, +found[1]);
    }
    return `Layer ${most + 1}`;
  };
  // Makes the layer a blank target stands for, filled with `hash`
  const createLayer = (figma, t, hash) => {
    const place = t.place;
    const node = figma.createRectangle();
    const parent = place.parent ?? figma.currentPage;
    if (place.index !== null && place.index !== undefined) parent.insertChild(Math.min(place.index, parent.children.length), node);
    else parent.appendChild(node);
    node.name = layerName(parent);
    if ("layoutMode" in parent && parent.layoutMode && parent.layoutMode !== "NONE") node.layoutPositioning = "ABSOLUTE";
    node.resize(place.w, place.h);
    if (place.like) node.relativeTransform = place.like.relativeTransform;
    else if (parent.type === "PAGE") {
      node.x = place.M.e;
      node.y = place.M.f;
    } else {
      const m = place.local ?? new DOMMatrix();
      node.relativeTransform = [
        [m.a, m.c, m.e],
        [m.b, m.d, m.f],
      ];
    }
    try {
      node.constraints = { horizontal: "SCALE", vertical: "SCALE" };
    } catch {}
    node.fills = [{ type: "IMAGE", imageHash: hash, scaleMode: "FILL" }];
    return node;
  };

  // Hands a layer's pixels to Figma, one change at a time and each its own
  // undo step. The picture is taken now, so painting can go on meanwhile
  let writing = Promise.resolve();
  // Erasing shows what's under the layer, so Figma's own copy of it is
  // hidden until the change is in, shown again with it in the same undo
  // step. Strokes overlap, so it's counted
  const conceal = (t) => {
    if (!t.node) return false;
    try {
      if (!t.hidden) {
        if (!t.node.visible) return false;
        t.node.visible = false;
      }
      t.hidden = (t.hidden ?? 0) + 1;
      return true;
    } catch {
      return false;
    }
  };
  const unconceal = (t) => {
    t.hidden = Math.max(0, (t.hidden ?? 0) - 1);
    try {
      if (!t.hidden && t.node && !t.node.removed) t.node.visible = true;
    } catch {}
  };
  const commit = (t, picture, { reveals = 0, select = false } = {}) => {
    const figma = api();
    if (!figma) return Promise.resolve();
    t.pending++;
    const blob = new Promise((resolve) => picture.toBlob(resolve, "image/png"));
    const done = (writing = writing.then(async () => {
      let left = reveals;
      const uncover = () => {
        for (; left > 0; left--) unconceal(t);
      };
      try {
        const bytes = new Uint8Array(await (await blob).arrayBuffer());
        const image = figma.createImage(bytes);
        if (!t.node) {
          t.node = createLayer(figma, t, image.hash);
          Object.assign(t, { id: t.node.id, index: 0, paint: t.node.fills[0], M: matrix(t.node.absoluteTransform), w: t.node.width, h: t.node.height, place: null });
          if (select) figma.currentPage.selection = [t.node];
        } else {
          if (t.node.removed) return;
          const fills = [...fillsOf(t.node)];
          const at = fills.findLastIndex((paint) => paint.type === "IMAGE" && paint.imageHash === t.hash);
          // Undone or changed meanwhile: left as it is, and read again
          if (at < 0) {
            uncover();
            t.hash = null;
            notify("The layer changed while painting");
            return;
          }
          const { imageTransform, scalingFactor, rotation, ...rest } = fills[at];
          fills[at] = { ...rest, imageHash: image.hash, scaleMode: "FILL" };
          t.node.fills = fills;
          uncover();
          t.paint = fills[at];
          t.index = at;
        }
        t.hash = image.hash;
        figma.commitUndo();
      } catch {
        uncover();
        notify("Couldn't update the layer");
      } finally {
        t.pending--;
      }
    }));
    return done;
  };

  // Strokes in quick succession go to Figma together: each new image is
  // work for Figma to read and draw again, and Ligma's canvas shows the
  // layer meanwhile. A new layer goes at once, so there's one to paint on
  const COMMIT_AFTER = 450;
  // How long Ligma's canvas stays once Figma has the change: Figma reads
  // a new image, fades it in and draws it roughly before it's sharp, the
  // longer the larger it is
  const holdFor = (t) => 2200 + 1800 * Math.min(1, (t.W * t.H) / (LARGEST * LARGEST));
  const schedule = (t, { reveal = false } = {}) => {
    t.unsaved = true;
    if (reveal) t.reveals = (t.reveals ?? 0) + 1;
    clearTimeout(t.timer);
    if (!t.node) return save(t);
    t.timer = setTimeout(() => save(t), COMMIT_AFTER);
    return Promise.resolve();
  };
  const save = (t) => {
    if (!t) return writing;
    clearTimeout(t.timer);
    if (!t.unsaved) return writing;
    t.unsaved = false;
    const reveals = t.reveals ?? 0;
    t.reveals = 0;
    const fresh = !t.node && !t.creating;
    if (fresh) t.creating = true;
    return commit(t, t.base, { reveals, select: fresh }).then(() => {
      t.holdUntil = t.released ? 0 : performance.now() + holdFor(t);
      if (fresh && t.node) target = t;
    });
  };
  // Undo before the strokes have gone to Figma takes them back here
  const discard = (t) => {
    clearTimeout(t.timer);
    t.unsaved = false;
    for (let i = t.reveals ?? 0; i > 0; i--) unconceal(t);
    t.reveals = 0;
    if (target === t) target = null;
    if (showing?.t === t) hideNow();
    if (t.node) read(t.node).catch(() => {});
  };
  // A change made at once, by a fill or the gradient, becomes the layer's
  const adopt = (t, canvas, { reveal = false } = {}) => {
    t.base = canvas;
    t.live = copyOf(canvas);
    t.pixels = null;
    show(t);
    return schedule(t, { reveal });
  };

  // Up and down a layer, among the layer's siblings, and a new one
  const step = (by) => {
    const figma = api();
    if (!figma) return;
    const node = selected();
    if (!node) {
      const children = figma.currentPage.children;
      const top = children.at(by > 0 ? -1 : 0);
      if (top) figma.currentPage.selection = [top];
      return;
    }
    const siblings = node.parent?.children ?? [];
    const next = siblings[siblings.indexOf(node) + by];
    if (next) figma.currentPage.selection = [next];
    else notify(by > 0 ? "This is the top layer" : "This is the bottom layer");
    refreshBar();
  };
  const newLayer = async () => {
    const figma = api();
    if (!figma) return;
    const v = view();
    const point = v ? [v.bounds.x + v.bounds.width / 2, v.bounds.y + v.bounds.height / 2] : null;
    const t = blank(placement(figma, point));
    await commit(t, t.base, { select: true });
    if (t.node) target = t;
    refreshBar();
  };

  // ---------------------------------------------------------------------
  // The overlay on the canvas: the layer while it's painted, marching ants
  // around the selection, the brush and what's being drawn

  const ui = {};
  const overlay = () => {
    const canvas = mainCanvas();
    if (!canvas) return null;
    if (!ui.overlay) {
      const root = make("div", { [`${ATTR}-overlay`]: true });
      const live = root.appendChild(make("div", { class: "live" }));
      const svg = root.appendChild(make("svg:svg", { class: "marks" }));
      const ants = svg.appendChild(make("svg:g", { class: "ants" }));
      const antsUnder = ants.appendChild(make("svg:path", { class: "under" }));
      const antsOver = ants.appendChild(make("svg:path", { class: "over" }));
      const shape = svg.appendChild(make("svg:path", { class: "shape" }));
      const shapeOver = svg.appendChild(make("svg:path", { class: "shape over" }));
      const line = svg.appendChild(make("svg:path", { class: "line" }));
      const string = svg.appendChild(make("svg:path", { class: "string" }));
      const source = svg.appendChild(make("svg:path", { class: "source" }));
      const brushUnder = svg.appendChild(make("svg:ellipse", { class: "brush under" }));
      const brushOver = svg.appendChild(make("svg:ellipse", { class: "brush" }));
      Object.assign(ui, { overlay: root, live, svg, ants, antsUnder, antsOver, shape, shapeOver, line, string, source, brushUnder, brushOver });
    }
    if (ui.overlay.previousElementSibling !== canvas) canvas.after(ui.overlay);
    return ui.overlay;
  };
  // The canvas shown over the layer while it's painted, until Figma has
  // drawn the change sharply. How it's drawn in the file, its opacity and
  // the frame clipping it, is read once, as the plugin API is slow to ask
  // every frame
  let showing = null;
  const show = (t, canvas = t.live) => {
    if (!overlay()) return;
    if (ui.live.firstChild !== canvas) ui.live.replaceChildren(canvas);
    appear();
    let opacity = 1;
    try {
      if (t.node && !t.node.removed) opacity = (t.node.opacity ?? 1) * (t.paint?.opacity ?? 1);
    } catch {}
    const clip = clipOf(t);
    canvas.style.opacity = opacity;
    t.released = false;
    showing = { t, canvas, clip, fading: 0, opacity, at: null };
    place();
  };
  // Shown at once, never faded in: a fade out stopped partway would
  // otherwise come back dim
  const appear = () => {
    if (!ui.live.classList.contains("fading")) return;
    ui.live.style.transition = "none";
    ui.live.classList.remove("fading");
    void ui.live.offsetWidth;
    ui.live.style.transition = "";
  };
  const hideNow = () => {
    ui.live?.replaceChildren();
    showing = null;
  };
  // Keeps the canvas with the layer in Figma, every frame: moved, turned or
  // resized there, or a frame around it moved, it goes with it, its
  // pixels stretched as Figma stretches the image. Undone, redone, changed
  // by someone else, hidden or deleted, it's Figma's to show at once
  const follow = (shown) => {
    const { t } = shown;
    const node = t.node;
    if (!node) return;
    try {
      if (node.removed) return gone(t);
      const at = node.absoluteTransform;
      const key = `${at[0].join()},${at[1].join()},${node.width},${node.height}`;
      if (shown.at !== key) {
        if (shown.at !== null) {
          Object.assign(t, { M: matrix(at), w: node.width, h: node.height });
          shown.clip = clipOf(t);
        }
        shown.at = key;
      }
      if (!t.pending && !t.unsaved && !(drag?.stroke?.t === t)) {
        const paint = fillsOf(node)[imageIndex(node)];
        if (paint?.imageHash !== t.hash) return gone(t);
      }
      if (!node.visible && !t.hidden) return gone(t);
      const opacity = (node.opacity ?? 1) * (t.paint?.opacity ?? 1);
      if (opacity !== shown.opacity) {
        shown.opacity = opacity;
        shown.canvas.style.opacity = opacity;
      }
    } catch {}
  };
  // Changed in Figma: what Ligma has of it is out of date
  const gone = (t) => {
    if (showing?.t === t) hideNow();
    if (target === t && !t.unsaved) target = null;
  };
  // The frame clipping the layer, if any
  const clipOf = (t) => {
    try {
      for (let at = t.node?.parent ?? t.place?.parent; at && at.type !== "PAGE"; at = at.parent) {
        if (at.clipsContent) return { M: matrix(at.absoluteTransform), w: at.width, h: at.height };
      }
    } catch {}
    return null;
  };
  // Where a layer is in the order Figma draws the page, so later is above
  const order = (node) => {
    const path = [];
    for (let at = node; at?.parent; at = at.parent) path.unshift(at.parent.children.indexOf(at));
    return path;
  };
  const later = (a, b) => {
    for (let i = 0; i < Math.min(a.length, b.length); i++) if (a[i] !== b[i]) return a[i] > b[i];
    return a.length > b.length;
  };
  const boxOfNode = (node) => {
    const box = node.absoluteRenderBounds ?? node.absoluteBoundingBox;
    if (box) return [box.x, box.y, box.x + box.width, box.y + box.height];
    if (!node.absoluteTransform) return null;
    const corners = [
      [0, 0],
      [node.width, 0],
      [node.width, node.height],
      [0, node.height],
    ].map((corner) => apply(matrix(node.absoluteTransform), corner));
    const xs = corners.map(([x]) => x);
    const ys = corners.map(([, y]) => y);
    return [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)];
  };
  const overlaps = (a, b) => a && b && a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3];
  // Anything else changed in Figma over the layer, as it's edited or by
  // someone else: the canvas would hide it, so it goes as soon as Figma
  // has the strokes. The layer's own changes are followed every frame
  let watching = null;
  const onDocument = (event) => {
    const t = showing?.t;
    if (!t?.node || t.released) return;
    let mine = null;
    for (const change of event.documentChanges ?? []) {
      if (change.id === t.id || change.type === "STYLE_CREATE" || change.type === "STYLE_DELETE" || change.type === "STYLE_PROPERTY_CHANGE") continue;
      const node = change.node;
      if (!node || node.removed || node.type === "PAGE" || node.type === "DOCUMENT") continue;
      try {
        // A frame around the layer moves it, which it follows
        let around = false;
        for (let at = t.node.parent; at && !around; at = at.parent) around = at.id === node.id;
        if (around) continue;
        mine ??= { order: order(t.node), box: boxOfNode(t.node) };
        if (later(order(node), mine.order) && overlaps(boxOfNode(node), mine.box)) {
          release(t);
          return;
        }
      } catch {}
    }
  };
  const release = (t) => {
    t.released = true;
    t.holdUntil = 0;
    if (t.unsaved) save(t);
    else if (!t.pending && showing?.t === t && drag?.stroke?.t !== t) hideNow();
  };
  const watchDocument = (on) => {
    const figma = api();
    try {
      if (watching) watching.figma.off("documentchange", onDocument);
      watching = null;
      if (on && figma) {
        figma.on("documentchange", onDocument);
        watching = { figma };
      }
    } catch {}
  };

  // Whether the canvas still covers the layer: while it's painted, until
  // Figma has the change, and a moment after. The gradient's preview only
  // while it's dragged
  const needed = ({ t, canvas }) => {
    if (canvas !== t.live) return drag?.live === canvas;
    return drag?.stroke?.t === t || drag?.t === t || t.unsaved || t.pending > 0 || performance.now() < (t.holdUntil ?? 0);
  };
  const place = () => {
    const v = view();
    const root = ui.overlay;
    if (!v || !root) return;
    const box = [`${v.canvas.offsetLeft}px`, `${v.canvas.offsetTop}px`, `${v.canvas.offsetWidth}px`, `${v.canvas.offsetHeight}px`];
    if (root.dataset.box !== box.join()) {
      root.dataset.box = box.join();
      Object.assign(root.style, { left: box[0], top: box[1], width: box[2], height: box[3] });
    }
    if (showing) {
      const now = performance.now();
      if (needed(showing)) {
        if (showing.fading) {
          showing.fading = 0;
          appear();
        }
      } else if (!showing.fading) {
        showing.fading = now;
        ui.live.classList.add("fading");
      } else if (now - showing.fading > 320) hideNow();
    }
    if (showing) follow(showing);
    if (showing) {
      const { t, canvas, clip } = showing;
      // Laid out a pixel square and scaled up to the layer, so it's never
      // the largest canvas on the page, which features take for Figma's
      const transform = v.toScreen.multiply(gridOf(t)).scale(t.W, t.H).toString();
      if (canvas.style.transform !== transform) canvas.style.transform = transform;
      let path = "none";
      if (clip) {
        const m = v.toScreen.multiply(clip.M);
        const corners = [
          [0, 0],
          [clip.w, 0],
          [clip.w, clip.h],
          [0, clip.h],
        ].map((corner) => apply(m, corner));
        path = `polygon(${corners.map(([x, y]) => `${x.toFixed(1)}px ${y.toFixed(1)}px`).join(",")})`;
      }
      if (ui.live.style.clipPath !== path) ui.live.style.clipPath = path;
    }
    if (selection) ui.ants.setAttribute("transform", v.toScreen.multiply(selection.G).toString());
    marks(v);
  };

  // Runs every frame in Raster mode: keeps what's on the canvas with it
  // as it pans and zooms, and the toolbar in place
  let looping = false;
  let lastCheck = 0;
  const loop = () => {
    if (looping) return;
    looping = true;
    const frame = (time) => {
      if (!active) {
        looping = false;
        return;
      }
      if (!ui.tools?.isConnected) mount();
      if (ui.overlay && !ui.overlay.isConnected) ui.overlay = null;
      if (selection || showing || drag || pointer.over) overlay();
      place();
      placeBar();
      if (time - lastCheck > 250) {
        lastCheck = time;
        refreshBar();
        preload();
        settle();
      }
      requestAnimationFrame(frame);
    };
    requestAnimationFrame(frame);
  };

  // Strokes waiting for Figma go once another layer's picked, and the
  // canvas stops covering a layer that's been undone or changed in Figma
  const settle = () => {
    if (target?.unsaved && !drag && target.node && rasterNode()?.id !== target.id) save(target);
    const t = showing?.t;
    if (!t?.node || t.unsaved || t.pending || drag) return;
    try {
      const paint = fillsOf(t.node)[imageIndex(t.node)];
      if (t.node.removed || paint?.imageHash !== t.hash) t.holdUntil = 0;
    } catch {}
  };

  // Reads the selected layer ahead of the first stroke
  const preload = () => {
    if (!TOOLS[prefs.tool].paints && !["bucket", "gradient", "magicEraser", "wand", "eyedropper"].includes(prefs.tool)) return;
    const node = rasterNode();
    if (node && !drag) read(node).catch(() => {});
  };

  // ---------------------------------------------------------------------
  // The selection: a mask on a layer's pixel grid, `G` from its pixels to
  // the page

  let selection = null;
  const setSelection = (next) => {
    selection = next;
    if (selection) selection.version = (selection.version ?? 0) + 1;
    drawAnts();
    refreshBar();
  };
  // The mask over a layer's own grid, alpha 0 to 255
  const maskFor = (t) => {
    if (!selection) return null;
    const key = `${t.W}x${t.H}:${gridOf(t).toString()}:${selection.version}`;
    if (selection.cache?.key === key) return selection.cache;
    const canvas = canvasOf(t.W, t.H);
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    ctx.setTransform(gridOf(t).inverse().multiply(selection.G));
    ctx.drawImage(selection.canvas, 0, 0);
    const alpha = new Uint8ClampedArray(t.W * t.H);
    const { data } = ctx.getImageData(0, 0, t.W, t.H);
    for (let i = 0; i < alpha.length; i++) alpha[i] = data[i * 4 + 3];
    selection.cache = { key, canvas, alpha };
    return selection.cache;
  };
  // The grid a new selection is made on: the layer's, a blank one's, or
  // the canvas on screen
  const selectionGrid = (point) => {
    const node = rasterNode();
    if (node && target?.id === node.id) return { G: gridOf(target), W: target.W, H: target.H };
    const figma = api();
    if (node) {
      const scale = Math.min(DENSITY, LARGEST / Math.max(node.width, node.height));
      const [W, H] = [Math.max(1, Math.round(node.width * scale)), Math.max(1, Math.round(node.height * scale))];
      return { G: matrix(node.absoluteTransform).multiply(new DOMMatrix().scale(node.width / W, node.height / H)), W, H };
    }
    const t = blank(placement(figma, point));
    return { G: gridOf(t), W: t.W, H: t.H };
  };
  // Puts `shape` (a canvas the grid's size, white where picked) into the
  // selection by `mode`: new, add, subtract or intersect
  const combine = (grid, shape, mode) => {
    if (mode === "new" || !selection) {
      if (mode === "subtract" || mode === "intersect") return setSelection(null);
      return setSelection({ ...grid, canvas: shape });
    }
    // Onto the selection's own grid
    const canvas = canvasOf(selection.W, selection.H);
    const ctx = canvas.getContext("2d");
    ctx.drawImage(selection.canvas, 0, 0);
    ctx.globalCompositeOperation = { add: "source-over", subtract: "destination-out", intersect: "destination-in" }[mode];
    ctx.setTransform(selection.G.inverse().multiply(grid.G));
    ctx.drawImage(shape, 0, 0);
    setSelection({ ...selection, canvas, cache: null });
  };
  const empty = (canvas) => {
    const { data } = canvas.getContext("2d", { willReadFrequently: true }).getImageData(0, 0, canvas.width, canvas.height);
    for (let i = 3; i < data.length; i += 4) if (data[i] >= 128) return false;
    return true;
  };
  const selectAll = () => {
    const grid = selectionGrid(null);
    const canvas = canvasOf(grid.W, grid.H);
    const ctx = canvas.getContext("2d");
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, grid.W, grid.H);
    setSelection({ ...grid, canvas });
  };
  const invert = () => {
    if (!selection) return selectAll();
    const canvas = canvasOf(selection.W, selection.H);
    const ctx = canvas.getContext("2d");
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    ctx.globalCompositeOperation = "destination-out";
    ctx.drawImage(selection.canvas, 0, 0);
    setSelection({ ...selection, canvas, cache: null });
  };
  // Marching ants: the edges between picked and unpicked pixels, joined
  // along rows and columns. Large selections are traced at half size
  const drawAnts = () => {
    overlay();
    if (!ui.overlay) return;
    if (!selection) {
      ui.antsUnder.removeAttribute("d");
      ui.antsOver.removeAttribute("d");
      return;
    }
    const { canvas } = selection;
    const step = canvas.width * canvas.height > 4e6 ? 2 : 1;
    const [W, H] = [Math.ceil(canvas.width / step), Math.ceil(canvas.height / step)];
    const small = canvasOf(W, H);
    const ctx = small.getContext("2d", { willReadFrequently: true });
    ctx.drawImage(canvas, 0, 0, W, H);
    const { data } = ctx.getImageData(0, 0, W, H);
    const inside = (x, y) => x >= 0 && y >= 0 && x < W && y < H && data[(y * W + x) * 4 + 3] >= 128;
    const parts = [];
    for (let y = 0; y <= H; y++) {
      let start = -1;
      for (let x = 0; x <= W; x++) {
        const edge = x < W && inside(x, y - 1) !== inside(x, y);
        if (edge && start < 0) start = x;
        if (!edge && start >= 0) {
          parts.push(`M${start * step} ${y * step}H${x * step}`);
          start = -1;
        }
      }
    }
    for (let x = 0; x <= W; x++) {
      let start = -1;
      for (let y = 0; y <= H; y++) {
        const edge = y < H && inside(x - 1, y) !== inside(x, y);
        if (edge && start < 0) start = y;
        if (!edge && start >= 0) {
          parts.push(`M${x * step} ${start * step}V${y * step}`);
          start = -1;
        }
      }
    }
    const d = parts.join("");
    ui.antsUnder.setAttribute("d", d);
    ui.antsOver.setAttribute("d", d);
    place();
  };

  // ---------------------------------------------------------------------
  // Pixels

  const rgb = ([r, g, b]) => `rgb(${r} ${g} ${b})`;
  const hex = ([r, g, b]) => `#${[r, g, b].map((value) => value.toString(16).padStart(2, "0")).join("")}`;
  const parseHex = (text) => {
    const found = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(text.trim());
    if (!found) return null;
    const digits = found[1].length === 3 ? [...found[1]].map((digit) => digit + digit).join("") : found[1];
    return [0, 2, 4].map((at) => parseInt(digits.slice(at, at + 2), 16));
  };
  // A brush tip `size` pixels across, turned `angle` degrees and squashed
  // to `roundness`, as Photoshop's: opaque to `hardness` of its radius
  // and fading out past it, or with a hard, aliased edge
  const tips = new Map();
  // How far a point is from a tip's middle, in its own radii, its long
  // axis along `angle`
  const shaped = (dx, dy, angle, roundness) => {
    const a = (angle * Math.PI) / 180;
    const [c, s] = [Math.cos(a), Math.sin(a)];
    return Math.hypot(c * dx - s * dy, (s * dx + c * dy) / Math.max(0.01, roundness / 100));
  };
  const tip = (size, hardness, aliased, angle = 0, roundness = 100, color = null) => {
    const d = Math.max(1, Math.ceil(size));
    const key = `${d}:${hardness}:${aliased}:${angle}:${roundness}:${color}`;
    if (tips.has(key)) return tips.get(key);
    const canvas = canvasOf(d, d);
    const ctx = canvas.getContext("2d");
    const r = size / 2;
    if (aliased) {
      const image = ctx.createImageData(d, d);
      for (let y = 0; y < d; y++)
        for (let x = 0; x < d; x++) {
          if (shaped(x + 0.5 - d / 2, y + 0.5 - d / 2, angle, roundness) <= Math.max(0.5, r)) image.data[(y * d + x) * 4 + 3] = 255;
        }
      ctx.putImageData(image, 0, 0);
    } else {
      const inner = clamp(hardness / 100, 0, 0.99) * r;
      ctx.translate(d / 2, d / 2);
      ctx.rotate((-angle * Math.PI) / 180);
      ctx.scale(1, Math.max(0.01, roundness / 100));
      const gradient = ctx.createRadialGradient(0, 0, inner, 0, 0, r);
      gradient.addColorStop(0, "#000");
      // Eased, so a soft brush fades smoothly
      for (let i = 1; i <= 8; i++) gradient.addColorStop(i / 8, `rgb(0 0 0 / ${(1 - i / 8) ** 2})`);
      ctx.fillStyle = gradient;
      ctx.beginPath();
      ctx.arc(0, 0, r, 0, Math.PI * 2);
      ctx.fill();
      ctx.setTransform(1, 0, 0, 1, 0, 0);
    }
    // Tinted once, rather than at every dab
    if (color) {
      ctx.globalCompositeOperation = "source-in";
      ctx.fillStyle = color;
      ctx.fillRect(0, 0, d, d);
    }
    if (tips.size > 24) tips.delete(tips.keys().next().value);
    tips.set(key, canvas);
    return canvas;
  };
  // How much of a brush's strength reaches a point `distance` radii from
  // its middle
  const falloff = (distance, r, hardness) => {
    if (distance >= r) return 0;
    const inner = clamp(hardness / 100, 0, 0.99) * r;
    if (distance <= inner) return 1;
    const t = (distance - inner) / (r - inner);
    return (1 - t) ** 2;
  };

  let scratch = null;
  const scratchOf = (width, height) => {
    if (!scratch || scratch.width < width || scratch.height < height) scratch = canvasOf(Math.max(width, scratch?.width ?? 0), Math.max(height, scratch?.height ?? 0));
    const ctx = scratch.getContext("2d");
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.globalCompositeOperation = "source-over";
    ctx.globalAlpha = 1;
    ctx.clearRect(0, 0, width, height);
    return ctx;
  };
  let strokeBuffer = null;

  // Composites the stroke onto the layer over `rect`, through the
  // selection: `live` = `base` with the stroke on top, or taken away
  const composite = (stroke, [x0, y0, x1, y1]) => {
    const { t, opacity, erase } = stroke;
    const live = t.live;
    x0 = Math.max(0, Math.floor(x0));
    y0 = Math.max(0, Math.floor(y0));
    x1 = Math.min(t.W, Math.ceil(x1));
    y1 = Math.min(t.H, Math.ceil(y1));
    if (x1 <= x0 || y1 <= y0) return;
    const [w, h] = [x1 - x0, y1 - y0];
    const ctx = live.getContext("2d");
    // Not with "copy", which would clear the rest of the canvas
    ctx.clearRect(x0, y0, w, h);
    ctx.drawImage(t.base, x0, y0, w, h, x0, y0, w, h);
    const buffer = scratchOf(w, h);
    buffer.drawImage(strokeBuffer, x0, y0, w, h, 0, 0, w, h);
    if (stroke.mask) {
      buffer.globalCompositeOperation = "destination-in";
      buffer.drawImage(stroke.mask.canvas, x0, y0, w, h, 0, 0, w, h);
    }
    ctx.save();
    ctx.globalAlpha = opacity;
    ctx.globalCompositeOperation = erase ? "destination-out" : "source-over";
    ctx.drawImage(scratch, 0, 0, w, h, x0, y0, w, h);
    ctx.restore();
  };

  // ---------------------------------------------------------------------
  // Painting: brush, pencil, eraser and clone stamp lay dabs into a stroke
  // buffer composited over the layer; blur, sharpen, smudge, dodge, burn
  // and sponge change the layer's pixels where they pass

  let drag = null;
  let cloneSource = null;
  const pointer = { over: false, x: 0, y: 0, pressure: 1 };

  const startPaint = (event, page) => {
    const figma = api();
    const tool = prefs.tool;
    const node = rasterNode();
    // A new layer still on its way to Figma takes more strokes meanwhile
    if (!node && target && !target.node && target.place) return { tool, points: [], ready: Promise.resolve(target) };
    // With nothing selected and no frame under the pointer, the raster layer
    // under it on the page, as layers made there are as large as the screen
    if (!node && !selected() && !containerAt(figma, page)) {
      const under = [...figma.currentPage.children].reverse().find((each) => isRaster(each) && each.visible && !each.locked && contains(each, page));
      if (under) {
        figma.currentPage.selection = [under];
        return { tool, points: [], ready: read(under) };
      }
    }
    if (!node && (tool === "brush" || tool === "pencil")) {
      // Painting with no layer to paint on adds one
      const t = blank(placement(figma, page));
      target = t;
      return { tool, t, fresh: true, points: [], ready: Promise.resolve(t) };
    }
    if (!node) {
      notify(`Select an image layer to use the ${TOOLS[tool].label.toLowerCase()} on`);
      return null;
    }
    return { tool, points: [], ready: read(node) };
  };

  // The layer's pixels as data, for the tools that change them in place,
  // kept while only those tools change them
  const pixelsOf_ = (t) => {
    if (!t.pixels) t.pixels = t.live.getContext("2d", { willReadFrequently: true }).getImageData(0, 0, t.W, t.H);
    return t.pixels;
  };

  const beginStroke = (stroke) => {
    const { t, tool } = stroke;
    const o = prefs.options[tool];
    stroke.opacity = (o.opacity ?? 100) / 100;
    stroke.erase = tool === "eraser";
    stroke.mask = maskFor(t);
    stroke.leftover = 0;
    stroke.rect = null;
    stroke.total = null;
    const grid = gridOf(t);
    // Pixels of the layer to a pixel on screen, for smoothing's string
    stroke.perScreen = 1 / Math.max(1e-6, Math.hypot(grid.a, grid.b) * (view()?.zoom ?? 1));
    if (TOOLS[tool].direct) stroke.pixels = pixelsOf_(t);
    else {
      if (tool === "clone" && (!cloneSource || cloneSource.id !== t.id)) {
        notify(`${mac ? "Option" : "Alt"}-click to set where to clone from`);
        return false;
      }
      if (!strokeBuffer || strokeBuffer.width !== t.W || strokeBuffer.height !== t.H) strokeBuffer = canvasOf(t.W, t.H);
      if (tool !== "clone") {
        const color = tool === "eraser" ? "#000" : rgb(prefs.foreground);
        stroke.stamp = tip(o.size, tool === "pencil" ? 100 : o.hardness ?? 100, tool === "pencil", o.angle ?? 0, o.roundness ?? 100, color);
      }
    }
    // Smudging and blurring can thin the layer too
    stroke.reveal = ["eraser", "smudge", "blur"].includes(tool) && conceal(t);
    show(t);
    return true;
  };

  // A point from the pointer. With smoothing, the brush trails the pointer
  // on a string, as Photoshop's does, moving only once it's pulled taut
  const feed = (stroke, page, pressure) => {
    const point = apply(gridOf(stroke.t).inverse(), page);
    stroke.pointer = point;
    const smoothing = (prefs.options[stroke.tool].smoothing ?? 0) / 100;
    if (!stroke.brush || !smoothing) {
      stroke.brush = point;
      strokeTo(stroke, point, pressure);
      return;
    }
    const length = smoothing * 90 * stroke.perScreen;
    const [dx, dy] = [point[0] - stroke.brush[0], point[1] - stroke.brush[1]];
    const distance = Math.hypot(dx, dy);
    if (distance <= length) return;
    const pull = (distance - length) / distance;
    stroke.brush = [stroke.brush[0] + dx * pull, stroke.brush[1] + dy * pull];
    strokeTo(stroke, stroke.brush, pressure);
  };

  // Dabs from the last point to `to`, spaced by the brush's size
  const strokeTo = (stroke, to, pressure) => {
    const { tool } = stroke;
    const o = prefs.options[tool];
    stroke.pressure = pressure;
    const p = o.pressure ? clamp(pressure, 0.05, 1) : 1;
    const size = Math.max(1, o.size * p);
    const spacing = Math.max(tool === "pencil" ? 0.5 : 1, size * Math.max(0.05, (o.roundness ?? 100) / 100) * (TOOLS[tool].direct ? 0.2 : 0.1));
    const from = stroke.last;
    stroke.last = to;
    if (!from) {
      dab(stroke, to, size);
      return;
    }
    const length = Math.hypot(to[0] - from[0], to[1] - from[1]);
    let at = spacing - stroke.leftover;
    while (at <= length) {
      const f = at / length;
      dab(stroke, [from[0] + (to[0] - from[0]) * f, from[1] + (to[1] - from[1]) * f], size);
      at += spacing;
    }
    stroke.leftover = length - (at - spacing);
  };

  const grow = (rect, [x0, y0, x1, y1]) => (rect ? [Math.min(rect[0], x0), Math.min(rect[1], y0), Math.max(rect[2], x1), Math.max(rect[3], y1)] : [x0, y0, x1, y1]);
  // Dabs go into the stroke buffer as they come, and onto the canvas shown
  // once a frame
  const dab = (stroke, [x, y], size) => {
    const { t, tool } = stroke;
    const o = prefs.options[tool];
    const r = size / 2;
    if (TOOLS[tool].direct) return directDab(stroke, x, y, r);
    const ctx = strokeBuffer.getContext("2d");
    let rect;
    if (tool === "pencil") {
      ctx.imageSmoothingEnabled = false;
      const d = Math.max(1, Math.ceil(size));
      const [dx, dy] = [Math.round(x - d / 2), Math.round(y - d / 2)];
      ctx.drawImage(stroke.stamp, dx, dy, d, d);
      ctx.imageSmoothingEnabled = true;
      rect = [dx, dy, dx + d, dy + d];
    } else if (tool === "clone") {
      const brush = tip(size, o.hardness ?? 100, false, o.angle ?? 0, o.roundness ?? 100);
      const [dx, dy] = [x - r, y - r];
      const d = Math.ceil(size);
      const color = scratchOf(d + 2, d + 2);
      if (cloneSource.offset === null || cloneSource.offset === undefined) cloneSource.offset = [cloneSource.at[0] - x, cloneSource.at[1] - y];
      const [ox, oy] = cloneSource.offset;
      color.drawImage(t.base, dx + ox, dy + oy, size, size, 0, 0, size, size);
      color.globalCompositeOperation = "destination-in";
      color.drawImage(brush, 0, 0, size, size);
      stroke.source = [x + ox, y + oy];
      ctx.drawImage(scratch, 0, 0, size, size, dx, dy, size, size);
      rect = [dx - 1, dy - 1, dx + size + 1, dy + size + 1];
    } else {
      ctx.drawImage(stroke.stamp, x - r, y - r, size, size);
      rect = [x - r - 1, y - r - 1, x + r + 1, y + r + 1];
    }
    stroke.rect = grow(stroke.rect, rect);
    stroke.total = grow(stroke.total, rect);
    if (!stroke.frame) stroke.frame = requestAnimationFrame(() => drawStroke(stroke));
  };
  const drawStroke = (stroke) => {
    stroke.frame = 0;
    if (!stroke.rect) return;
    composite(stroke, stroke.rect);
    stroke.rect = null;
  };

  // Changes the pixels under a dab of the blur, sharpen, smudge, dodge,
  // burn or sponge tool, through the selection
  const directDab = (stroke, cx, cy, r) => {
    const { t, tool, pixels } = stroke;
    const o = prefs.options[tool];
    const { data } = pixels;
    const W = t.W;
    const [x0, y0, x1, y1] = [Math.max(0, Math.floor(cx - r)), Math.max(0, Math.floor(cy - r)), Math.min(t.W, Math.ceil(cx + r)), Math.min(t.H, Math.ceil(cy + r))];
    if (x1 <= x0 || y1 <= y0) return;
    const [w, h] = [x1 - x0, y1 - y0];
    const mask = stroke.mask?.alpha;
    const hardness = o.hardness ?? 0;
    // The pixels under the dab before it, for blur and sharpen to read
    const copy = new Float32Array(w * h * 4);
    for (let y = 0; y < h; y++) for (let x = 0; x < w; x++) for (let c = 0; c < 4; c++) copy[(y * w + x) * 4 + c] = data[((y0 + y) * W + x0 + x) * 4 + c];
    let blurred = null;
    if (tool === "blur" || tool === "sharpen") {
      // Two passes of a 3 by 3 box, premultiplied so clear pixels don't
      // darken their neighbours
      const pre = new Float32Array(copy);
      for (let i = 0; i < w * h; i++) for (let c = 0; c < 3; c++) pre[i * 4 + c] *= pre[i * 4 + 3] / 255;
      let from = pre;
      for (let pass = 0; pass < 2; pass++) {
        const to = new Float32Array(from.length);
        for (let y = 0; y < h; y++)
          for (let x = 0; x < w; x++) {
            const sum = [0, 0, 0, 0];
            let n = 0;
            for (let dy = -1; dy <= 1; dy++)
              for (let dx = -1; dx <= 1; dx++) {
                const [sx, sy] = [x + dx, y + dy];
                if (sx < 0 || sy < 0 || sx >= w || sy >= h) continue;
                const at = (sy * w + sx) * 4;
                for (let c = 0; c < 4; c++) sum[c] += from[at + c];
                n++;
              }
            for (let c = 0; c < 4; c++) to[(y * w + x) * 4 + c] = sum[c] / n;
          }
        from = to;
      }
      blurred = from;
      for (let i = 0; i < w * h; i++) {
        const a = blurred[i * 4 + 3];
        for (let c = 0; c < 3; c++) blurred[i * 4 + c] = a > 0 ? (blurred[i * 4 + c] * 255) / a : 0;
      }
    }
    if (tool === "smudge" && !stroke.carried) {
      stroke.carried = { w: Math.ceil(r * 2), data: null };
    }
    const strength = (o.strength ?? o.exposure ?? o.flow ?? 50) / 100;
    const carry = tool === "smudge" ? stroke.carried : null;
    const size = carry?.w ?? 0;
    const next = carry ? new Float32Array(size * size * 4) : null;
    for (let y = 0; y < h; y++)
      for (let x = 0; x < w; x++) {
        const [px, py] = [x0 + x, y0 + y];
        let weight = falloff(shaped(px + 0.5 - cx, py + 0.5 - cy, o.angle ?? 0, o.roundness ?? 100), r, hardness);
        if (mask) weight *= mask[py * W + px] / 255;
        const i = (y * w + x) * 4;
        const at = (py * W + px) * 4;
        // Where this pixel sits in the smudge's carried paint
        const [kx, ky] = carry ? [Math.floor(px - cx + size / 2), Math.floor(py - cy + size / 2)] : [-1, -1];
        const inCarry = !!carry && kx >= 0 && ky >= 0 && kx < size && ky < size;
        const ci = (ky * size + kx) * 4;
        if (weight <= 0) {
          if (inCarry && next) for (let c = 0; c < 4; c++) next[ci + c] = copy[i + c];
          continue;
        }
        let [r0, g0, b0, a0] = [copy[i], copy[i + 1], copy[i + 2], copy[i + 3]];
        let [r1, g1, b1, a1] = [r0, g0, b0, a0];
        if (tool === "blur") {
          [r1, g1, b1, a1] = [blurred[i], blurred[i + 1], blurred[i + 2], blurred[i + 3]];
          weight *= strength;
        } else if (tool === "sharpen") {
          const k = strength * 1.5;
          r1 = r0 + (r0 - blurred[i]) * k;
          g1 = g0 + (g0 - blurred[i + 1]) * k;
          b1 = b0 + (b0 - blurred[i + 2]) * k;
          weight *= 0.35;
        } else if (tool === "smudge") {
          if (carry.data && inCarry) [r1, g1, b1, a1] = [carry.data[ci], carry.data[ci + 1], carry.data[ci + 2], carry.data[ci + 3]];
          weight *= strength;
        } else if (tool === "dodge" || tool === "burn") {
          const e = 1 + strength * 0.25;
          const f = tool === "dodge" ? (v) => 255 * (1 - (1 - v / 255) ** e) : (v) => 255 * (v / 255) ** e;
          [r1, g1, b1] = [f(r0), f(g0), f(b0)];
        } else if (tool === "sponge") {
          const l = 0.299 * r0 + 0.587 * g0 + 0.114 * b0;
          const k = o.saturate ? 1 + strength * 0.15 : 1 - strength * 0.15;
          [r1, g1, b1] = [l + (r0 - l) * k, l + (g0 - l) * k, l + (b0 - l) * k];
        }
        const mix = (from, to) => from + (to - from) * weight;
        const out = [mix(r0, r1), mix(g0, g1), mix(b0, b1), mix(a0, a1)];
        for (let c = 0; c < 4; c++) data[at + c] = out[c];
        if (inCarry && next) for (let c = 0; c < 4; c++) next[ci + c] = out[c];
      }
    if (carry) carry.data = next;
    stroke.dirty = grow(stroke.dirty, [x0, y0, x1, y1]);
    stroke.total = grow(stroke.total, [x0, y0, x1, y1]);
    flushSoon(stroke);
  };
  // The changed pixels onto the canvas shown, once a frame
  const flushSoon = (stroke) => {
    if (stroke.flushing) return;
    stroke.flushing = requestAnimationFrame(() => flush(stroke));
  };
  const flush = (stroke) => {
    stroke.flushing = 0;
    if (!stroke.dirty) return;
    const [x0, y0, x1, y1] = stroke.dirty;
    stroke.dirty = null;
    stroke.t.live.getContext("2d").putImageData(stroke.pixels, 0, 0, x0, y0, x1 - x0, y1 - y0);
  };

  const endStroke = (stroke) => {
    const { t, tool } = stroke;
    // Smoothing catches up with the pointer, so the stroke ends under it
    if (stroke.pointer && stroke.brush && stroke.pointer !== stroke.brush) strokeTo(stroke, stroke.pointer, stroke.pressure ?? 1);
    if (TOOLS[tool].direct) {
      if (stroke.flushing) cancelAnimationFrame(stroke.flushing);
      flush(stroke);
    } else {
      if (stroke.frame) cancelAnimationFrame(stroke.frame);
      drawStroke(stroke);
      t.pixels = null;
    }
    if (tool === "clone" && !prefs.options.clone.aligned && cloneSource) cloneSource.offset = null;
    // The stroke is the layer's now: `base` catches up with what's shown
    // where it went, and the stroke buffer is cleared there
    if (stroke.total) {
      const [x0, y0, x1, y1] = [Math.max(0, Math.floor(stroke.total[0])), Math.max(0, Math.floor(stroke.total[1])), Math.min(t.W, Math.ceil(stroke.total[2])), Math.min(t.H, Math.ceil(stroke.total[3]))];
      if (x1 > x0 && y1 > y0) {
        const ctx = t.base.getContext("2d");
        ctx.clearRect(x0, y0, x1 - x0, y1 - y0);
        ctx.drawImage(t.live, x0, y0, x1 - x0, y1 - y0, x0, y0, x1 - x0, y1 - y0);
        if (!TOOLS[tool].direct) strokeBuffer.getContext("2d").clearRect(x0, y0, x1 - x0, y1 - y0);
      }
    }
    schedule(t, { reveal: stroke.reveal });
    ui.source?.removeAttribute("d");
  };

  // ---------------------------------------------------------------------
  // Fills, the gradient, the magic wand and eraser and the eyedropper

  // What's under the layer and on it, from Figma, as its pixels: the
  // outermost frame around it drawn as a picture. Without one, the layer
  const composited = async (t) => {
    let frame = null;
    for (let at = t.node?.parent; at && at.type !== "PAGE"; at = at.parent) if (CONTAINERS.has(at.type)) frame = at;
    if (!frame) return t.base;
    // Figma draws what it has, so the strokes still waiting go first
    await save(t);
    const turned = Math.abs(frame.absoluteTransform[0][1]) > 1e-6 || Math.abs(frame.absoluteTransform[1][0]) > 1e-6;
    if (turned) return t.base;
    const scale = Math.min(Math.hypot(gridOf(t).a, gridOf(t).b) ** -1, LARGEST / Math.max(frame.width, frame.height));
    const bytes = await frame.exportAsync({ format: "PNG", constraint: { type: "SCALE", value: scale }, useAbsoluteBounds: true });
    const bitmap = await createImageBitmap(new Blob([bytes]));
    const canvas = canvasOf(t.W, t.H);
    const ctx = canvas.getContext("2d");
    const [[, , fx], [, , fy]] = frame.absoluteTransform;
    ctx.setTransform(gridOf(t).inverse().multiply(new DOMMatrix().translate(fx, fy).scale(frame.width / bitmap.width, frame.height / bitmap.height)));
    ctx.drawImage(bitmap, 0, 0);
    bitmap.close?.();
    return canvas;
  };
  const pixelsOf = (canvas) => canvas.getContext("2d", { willReadFrequently: true }).getImageData(0, 0, canvas.width, canvas.height);

  // The pixels like the one at `x`, `y`, within `tolerance` in each
  // channel, touching it unless `contiguous` is off. Clear pixels match by
  // their alpha alone. 255 where they match
  const floodMask = ({ data, width: W, height: H }, x, y, tolerance, contiguous) => {
    const out = new Uint8Array(W * H);
    const seed = (y * W + x) * 4;
    const [r, g, b, a] = [data[seed], data[seed + 1], data[seed + 2], data[seed + 3]];
    const clear = a === 0;
    const like = (i) => {
      const p = i * 4;
      if (clear) return data[p + 3] <= tolerance;
      return (
        Math.abs(data[p] - r) <= tolerance && Math.abs(data[p + 1] - g) <= tolerance && Math.abs(data[p + 2] - b) <= tolerance && Math.abs(data[p + 3] - a) <= tolerance
      );
    };
    if (!contiguous) {
      for (let i = 0; i < W * H; i++) if (like(i)) out[i] = 255;
      return out;
    }
    const stack = [x, y];
    while (stack.length) {
      const sy = stack.pop();
      let sx = stack.pop();
      let i = sy * W + sx;
      if (out[i] || !like(i)) continue;
      while (sx > 0 && !out[i - 1] && like(i - 1)) {
        sx--;
        i--;
      }
      let above = false;
      let below = false;
      for (; sx < W && !out[i] && like(i); sx++, i++) {
        out[i] = 255;
        if (sy > 0) {
          const up = !out[i - W] && like(i - W);
          if (up && !above) stack.push(sx, sy - 1);
          above = up;
        }
        if (sy < H - 1) {
          const down = !out[i + W] && like(i + W);
          if (down && !below) stack.push(sx, sy + 1);
          below = down;
        }
      }
    }
    return out;
  };
  // A mask as a canvas, white where picked, its edge softened a little
  const maskCanvas = (alpha, W, H, smooth) => {
    const raw = canvasOf(W, H);
    const image = new ImageData(W, H);
    for (let i = 0; i < alpha.length; i++) {
      const p = i * 4;
      image.data[p] = image.data[p + 1] = image.data[p + 2] = 255;
      image.data[p + 3] = alpha[i];
    }
    raw.getContext("2d").putImageData(image, 0, 0);
    if (!smooth) return raw;
    const soft = canvasOf(W, H);
    const ctx = soft.getContext("2d");
    ctx.filter = "blur(0.6px)";
    ctx.drawImage(raw, 0, 0);
    ctx.filter = "none";
    ctx.drawImage(raw, 0, 0);
    return soft;
  };
  const pixelAt = (t, page) => {
    const [x, y] = apply(gridOf(t).inverse(), page);
    return [Math.floor(x), Math.floor(y)];
  };
  const inside = (t, [x, y]) => x >= 0 && y >= 0 && x < t.W && y < t.H;

  // The bucket: fills what's like the pixel clicked with the foreground
  // color, through the selection
  const bucket = async (page) => {
    const figma = api();
    let node = rasterNode();
    let t;
    if (!node) t = blank(placement(figma, page));
    else t = await read(node);
    if (!t) return;
    const o = prefs.options.bucket;
    const at = pixelAt(t, page);
    if (!inside(t, at)) return;
    const source = o.all && t.node ? await composited(t) : t.base;
    const region = maskCanvas(floodMask(pixelsOf(source), at[0], at[1], o.tolerance, o.contiguous), t.W, t.H, true);
    const live = canvasOf(t.W, t.H);
    const ctx = live.getContext("2d");
    ctx.drawImage(t.base, 0, 0);
    const paint = canvasOf(t.W, t.H);
    const pctx = paint.getContext("2d");
    pctx.drawImage(region, 0, 0);
    pctx.globalCompositeOperation = "source-in";
    pctx.fillStyle = rgb(prefs.foreground);
    pctx.fillRect(0, 0, t.W, t.H);
    const mask = maskFor(t);
    if (mask) {
      pctx.globalCompositeOperation = "destination-in";
      pctx.drawImage(mask.canvas, 0, 0);
    }
    ctx.globalAlpha = o.opacity / 100;
    ctx.drawImage(paint, 0, 0);
    finishInstant(t, live, !node);
  };
  const finishInstant = (t, live, fresh, reveal = false) => {
    if (fresh) target = t;
    adopt(t, live, { reveal });
  };

  // Fills the selection, or the whole layer, with a color, or clears it
  const fillSelection = async (color) => {
    const node = rasterNode();
    if (!node) return notify("Select an image layer");
    const t = await read(node);
    const live = canvasOf(t.W, t.H);
    const ctx = live.getContext("2d");
    ctx.drawImage(t.base, 0, 0);
    const mask = maskFor(t);
    if (color) {
      const paint = canvasOf(t.W, t.H);
      const pctx = paint.getContext("2d");
      pctx.fillStyle = rgb(color);
      pctx.fillRect(0, 0, t.W, t.H);
      if (mask) {
        pctx.globalCompositeOperation = "destination-in";
        pctx.drawImage(mask.canvas, 0, 0);
      }
      ctx.drawImage(paint, 0, 0);
    } else {
      ctx.globalCompositeOperation = "destination-out";
      if (mask) ctx.drawImage(mask.canvas, 0, 0);
      else ctx.fillRect(0, 0, t.W, t.H);
    }
    finishInstant(t, live, false, !color && conceal(t));
  };

  // The magic eraser: clears what's like the pixel clicked
  const magicErase = async (page) => {
    const node = rasterNode();
    if (!node) return notify("Select an image layer to erase from");
    const t = await read(node);
    const o = prefs.options.magicEraser;
    const at = pixelAt(t, page);
    if (!inside(t, at)) return;
    const region = maskCanvas(floodMask(pixelsOf(t.base), at[0], at[1], o.tolerance, o.contiguous), t.W, t.H, true);
    const mask = maskFor(t);
    if (mask) {
      const rctx = region.getContext("2d");
      rctx.globalCompositeOperation = "destination-in";
      rctx.drawImage(mask.canvas, 0, 0);
    }
    const live = canvasOf(t.W, t.H);
    const ctx = live.getContext("2d");
    ctx.drawImage(t.base, 0, 0);
    ctx.globalAlpha = o.opacity / 100;
    ctx.globalCompositeOperation = "destination-out";
    ctx.drawImage(region, 0, 0);
    finishInstant(t, live, false, conceal(t));
  };

  // The magic wand: picks what's like the pixel clicked
  const wand = async (page, mode) => {
    const node = rasterNode();
    if (!node) return notify("Select an image layer to pick from");
    const t = await read(node);
    const o = prefs.options.wand;
    const at = pixelAt(t, page);
    if (!inside(t, at)) return;
    const source = o.all ? await composited(t) : t.base;
    const shape = maskCanvas(floodMask(pixelsOf(source), at[0], at[1], o.tolerance, o.contiguous), t.W, t.H, o.smooth);
    combine({ G: gridOf(t), W: t.W, H: t.H }, shape, mode);
  };

  // The eyedropper: the color under the pointer, averaged over its sample
  // size, from the layer or from everything drawn
  let sampled = null;
  const sample = async (page, toBackground, again = false) => {
    const figma = api();
    const o = prefs.options.eyedropper;
    let source = null;
    let t = null;
    const node = rasterNode();
    if (o.all) {
      if (!again || !sampled) {
        // The frame under the pointer, drawn as Figma draws it
        const frame = containerAt(figma, page);
        let top = frame;
        for (let at = frame?.parent; at && at.type !== "PAGE"; at = at.parent) if (CONTAINERS.has(at.type)) top = at;
        if (top) {
          const scale = Math.min(DENSITY, LARGEST / Math.max(top.width, top.height));
          const bytes = await top.exportAsync({ format: "PNG", constraint: { type: "SCALE", value: scale }, useAbsoluteBounds: true });
          const bitmap = await createImageBitmap(new Blob([bytes]));
          const canvas = canvasOf(bitmap.width, bitmap.height);
          canvas.getContext("2d").drawImage(bitmap, 0, 0);
          const [[, , fx], [, , fy]] = top.absoluteTransform;
          sampled = { canvas, G: new DOMMatrix().translate(fx, fy).scale(top.width / bitmap.width, top.height / bitmap.height), pixels: pixelsOf(canvas) };
        } else sampled = null;
      }
      source = sampled;
    }
    if (!source && node) {
      t = await read(node);
      source = { G: gridOf(t), pixels: pixelsOf(t.base) };
    }
    if (!source) return;
    const [x, y] = apply(source.G.inverse(), page).map(Math.floor);
    const { data, width, height } = source.pixels;
    const half = Math.floor(o.sample / 2);
    const sum = [0, 0, 0];
    let weight = 0;
    for (let sy = y - half; sy <= y + half; sy++)
      for (let sx = x - half; sx <= x + half; sx++) {
        if (sx < 0 || sy < 0 || sx >= width || sy >= height) continue;
        const p = (sy * width + sx) * 4;
        const a = data[p + 3] / 255;
        for (let c = 0; c < 3; c++) sum[c] += data[p + c] * a;
        weight += a;
      }
    if (!weight) return;
    const color = sum.map((value) => Math.round(value / weight));
    setColor(toBackground ? "background" : "foreground", color, false);
  };

  // The gradient: from where the drag starts to where it ends, in the
  // foreground color to the background color or to clear
  const gradientInto = (stroke) => {
    const { t, from, to } = stroke;
    const o = prefs.options.gradient;
    const [a, b] = [apply(gridOf(t).inverse(), from), apply(gridOf(t).inverse(), to)];
    const ctx = stroke.live.getContext("2d");
    ctx.save();
    ctx.globalCompositeOperation = "copy";
    ctx.drawImage(t.base, 0, 0);
    ctx.restore();
    const paint = scratchOf(t.W, t.H);
    const gradient =
      o.shape === "radial" ? paint.createRadialGradient(a[0], a[1], 0, a[0], a[1], Math.max(1, Math.hypot(b[0] - a[0], b[1] - a[1]))) : paint.createLinearGradient(a[0], a[1], b[0], b[1]);
    let stops = [`rgb(${prefs.foreground.join(" ")} / 1)`, o.colors === "clear" ? `rgb(${prefs.foreground.join(" ")} / 0)` : `rgb(${prefs.background.join(" ")} / 1)`];
    if (o.reverse) stops = stops.reverse();
    gradient.addColorStop(0, stops[0]);
    gradient.addColorStop(1, stops[1]);
    paint.fillStyle = gradient;
    paint.fillRect(0, 0, t.W, t.H);
    if (stroke.mask) {
      paint.globalCompositeOperation = "destination-in";
      paint.drawImage(stroke.mask.canvas, 0, 0);
    }
    ctx.globalAlpha = o.opacity / 100;
    ctx.drawImage(scratch, 0, 0, t.W, t.H, 0, 0, t.W, t.H);
    ctx.globalAlpha = 1;
  };

  // Selections drawn by hand: a rectangle, an ellipse or a loop, in page
  // points, onto a grid
  const shapeMask = (grid, draw, feather) => {
    const canvas = canvasOf(grid.W, grid.H);
    const ctx = canvas.getContext("2d");
    const scale = Math.hypot(grid.G.a, grid.G.b);
    if (feather > 0) ctx.filter = `blur(${feather / 2}px)`;
    ctx.setTransform(grid.G.inverse());
    ctx.fillStyle = "#fff";
    ctx.beginPath();
    draw(ctx, scale);
    ctx.fill();
    return canvas;
  };
  const boxOf = (from, to, square, centered) => {
    let [w, h] = [to[0] - from[0], to[1] - from[1]];
    if (square) {
      const most = Math.max(Math.abs(w), Math.abs(h));
      w = Math.sign(w || 1) * most;
      h = Math.sign(h || 1) * most;
    }
    if (centered) return [from[0] - w, from[1] - h, w * 2, h * 2];
    return [from[0], from[1], w, h];
  };
  const finishSelection = (drag) => {
    const o = prefs.options[drag.tool];
    const grid = selectionGrid(drag.from);
    let shape = null;
    if (drag.tool === "marquee" || drag.tool === "ellipse") {
      const [x, y, w, h] = boxOf(drag.from, drag.to ?? drag.from, drag.square, drag.centered);
      if (Math.abs(w) * drag.zoom < 2 || Math.abs(h) * drag.zoom < 2) {
        // A click outside a selection drops it, as in Photoshop
        if (drag.mode === "new") setSelection(null);
        return;
      }
      shape = shapeMask(
        grid,
        (ctx) => {
          if (drag.tool === "marquee") ctx.rect(x, y, w, h);
          else ctx.ellipse(x + w / 2, y + h / 2, Math.abs(w / 2), Math.abs(h / 2), 0, 0, Math.PI * 2);
        },
        o.feather,
      );
    } else {
      if (drag.points.length < 3) {
        if (drag.mode === "new") setSelection(null);
        return;
      }
      shape = shapeMask(
        grid,
        (ctx) => {
          ctx.moveTo(...drag.points[0]);
          for (const point of drag.points.slice(1)) ctx.lineTo(...point);
          ctx.closePath();
        },
        o.feather,
      );
    }
    if (drag.mode === "new" && empty(shape)) return setSelection(null);
    combine(grid, shape, drag.mode);
  };

  // Shift adds to the selection, Alt takes away and both intersect, or
  // the mode in the options bar
  const selectMode = (event) => {
    const alt = event.altKey;
    if (event.shiftKey && alt) return "intersect";
    if (event.shiftKey) return "add";
    if (alt) return "subtract";
    return prefs.options[prefs.tool]?.mode ?? "new";
  };

  const setPath = (node, d) => {
    if (node.getAttribute("d") !== d) node.setAttribute("d", d);
  };
  // Marks on the canvas: what's being drawn, the brush and the clone
  // stamp's source, in screen pixels
  const marks = (v) => {
    if (!ui.overlay) return;
    const screen = (point) => apply(v.toScreen, point);
    const path = (points, close) => (points.length ? `M${points.map((point) => screen(point).map((n) => n.toFixed(1)).join(" ")).join("L")}${close ? "Z" : ""}` : "");
    let shape = "";
    let line = "";
    if (drag?.kind === "select") {
      if (drag.tool === "marquee" || drag.tool === "ellipse") {
        if (drag.to) {
          const [x, y, w, h] = boxOf(drag.from, drag.to, drag.square, drag.centered);
          if (drag.tool === "marquee")
            shape = path(
              [
                [x, y],
                [x + w, y],
                [x + w, y + h],
                [x, y + h],
              ],
              true,
            );
          else {
            const [cx, cy] = screen([x + w / 2, y + h / 2]);
            const [rx, ry] = [Math.abs((w / 2) * v.zoom), Math.abs((h / 2) * v.zoom)];
            shape = `M${cx - rx} ${cy}a${rx} ${ry} 0 1 0 ${rx * 2} 0a${rx} ${ry} 0 1 0 ${-rx * 2} 0Z`;
          }
        }
      } else shape = path(drag.points, drag.tool === "lasso");
    }
    if (polygon) shape = path([...polygon.points, ...(polygon.hover ? [polygon.hover] : [])], false);
    if (drag?.kind === "gradient" && drag.to) line = path([drag.from, drag.to], false);
    // Smoothing's string, from the brush to the pointer
    const stroke = drag?.kind === "paint" ? drag.stroke : null;
    let string = "";
    if (stroke?.brush && stroke.pointer && stroke.t && (prefs.options[stroke.tool].smoothing ?? 0) > 0) {
      const grid = gridOf(stroke.t);
      string = path([apply(grid, stroke.brush), apply(grid, stroke.pointer)], false);
    }
    setPath(ui.shape, shape);
    setPath(ui.shapeOver, shape);
    setPath(ui.line, line);
    setPath(ui.string, string);
    // The brush, its size and shape. The layer's grid, if it's the one
    // read, or a new layer's
    const o = options();
    const brushy = TOOLS[prefs.tool]?.paints && o.size;
    let radius = 0;
    if (brushy && pointer.over) {
      const t = stroke?.t ?? (target && target.id && target.id === shownId ? target : null);
      const scale = t ? Math.hypot(gridOf(t).a, gridOf(t).b) : 1 / DENSITY;
      radius = (o.size / 2) * scale * v.zoom;
    }
    const [cx, cy] = stroke?.brush && stroke.t && (o.smoothing ?? 0) > 0 ? screen(apply(gridOf(stroke.t), stroke.brush)) : [pointer.x - v.rect.left, pointer.y - v.rect.top];
    for (const ring of [ui.brushUnder, ui.brushOver]) {
      ring.setAttribute("cx", cx.toFixed(1));
      ring.setAttribute("cy", cy.toFixed(1));
      ring.setAttribute("rx", Math.max(0, radius).toFixed(2));
      ring.setAttribute("ry", Math.max(0, (radius * (o.roundness ?? 100)) / 100).toFixed(2));
      ring.setAttribute("transform", `rotate(${-(o.angle ?? 0)} ${cx.toFixed(1)} ${cy.toFixed(1)})`);
      ring.style.display = radius >= 3 ? "" : "none";
    }
    setCursor(cursorFor(radius));
    // The clone stamp's source
    let source = "";
    const t = drag?.t;
    const at = drag?.kind === "paint" && drag.stroke?.source && t ? apply(gridOf(t), drag.stroke.source) : !drag && cloneSource && prefs.tool === "clone" ? cloneSource.page : null;
    if (at) {
      const [x, y] = screen(at);
      source = `M${x - 6} ${y}H${x + 6}M${x} ${y - 6}V${y + 6}M${x + 4} ${y}a4 4 0 1 0 -8 0a4 4 0 1 0 8 0`;
    }
    ui.source.setAttribute("d", source);
  };

  // ---------------------------------------------------------------------
  // The pointer on the canvas

  // Cursors: Photoshop's, drawn white edged
  const cursorSvg = (body, x, y) => {
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none"><g stroke="#fff" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" color="#fff">${body}</g><g color="#000">${body}</g></svg>`;
    return `url("data:image/svg+xml,${encodeURIComponent(svg)}") ${x} ${y}, crosshair`;
  };
  const CURSORS = {
    eyedropper: cursorSvg(ICONS.eyedropper, 5, 19),
    bucket: cursorSvg(ICONS.bucket, 19, 19),
    wand: cursorSvg(ICONS.wand, 15, 9),
    magicEraser: cursorSvg(ICONS.magicEraser, 7, 17),
  };
  const cursorFor = (radius) => {
    if (!pointer.over || prefs.tool === "move" || space) return null;
    if (TOOLS[prefs.tool].paints) return radius >= 3 ? "none" : "crosshair";
    return CURSORS[prefs.tool] ?? "crosshair";
  };
  let cursor = null;
  const CURSOR = `${ATTR}-cursor`;
  const setCursor = (next) => {
    if (next === cursor) return;
    cursor = next;
    const canvas = mainCanvas();
    document.querySelectorAll(`[${CURSOR}]`).forEach((node) => node !== canvas && node.removeAttribute(CURSOR));
    if (!canvas) return;
    if (next) {
      canvas.setAttribute(CURSOR, "");
      canvas.style.setProperty("--ligma-raster-cursor", next);
    } else canvas.removeAttribute(CURSOR);
  };

  let polygon = null;
  let afterDrag = false;
  let space = false;
  const cancelAll = () => {
    if (drag?.kind === "paint") {
      // A stroke stopped short is still kept, as Photoshop keeps it
      drag.stroke.ended = true;
      if (drag.stroke.began) finishPaint(drag.stroke);
    }
    drag = null;
    polygon = null;
  };

  const onCanvas = (event) => event.target === mainCanvas() || !!event.target?.closest?.(`[${ATTR}-overlay]`);
  const POINTER = ["pointerdown", "pointermove", "pointerup", "pointercancel", "mousedown", "mousemove", "mouseup", "click", "dblclick", "contextmenu"];
  for (const type of POINTER) {
    addEventListener(
      type,
      (event) => {
        if (!active) return;
        if (type === "pointerdown" && menuOpen() && !event.target.closest?.(`[${ATTR}-menu], [${ATTR}-picker], [${ATTR}-tip], [${ATTR}-bar] .tip, [${ATTR}-tools] .chevron, [${ATTR}-tools] .swatch`)) closeMenus();
        if (type === "pointermove" || type === "pointerdown") {
          pointer.over = onCanvas(event);
          pointer.x = event.clientX;
          pointer.y = event.clientY;
        }
        if (drag) {
          if (event.pointerId !== undefined && drag.pointer !== undefined && event.pointerId !== drag.pointer) return swallow(event);
          swallow(event);
          if (type === "pointermove") move(event);
          else if (type === "pointerup") up(event);
          else if (type === "pointercancel") cancelAll();
          return;
        }
        if (polygon) {
          if (type === "pointermove") {
            polygon.hover = pageAt(event);
            return;
          }
          if (!onCanvas(event)) return;
          swallow(event);
          const page = pageAt(event);
          if (!page) return;
          if (type === "pointerdown" && event.button === 0) {
            const first = polygon.points[0];
            const close = first && Math.hypot(page[0] - first[0], page[1] - first[1]) * polygon.zoom < 8 && polygon.points.length > 2;
            if (close) closePolygon();
            else polygon.points.push(page);
          } else if (type === "dblclick") closePolygon();
          return;
        }
        if (afterDrag && (type === "click" || type === "mouseup" || type === "dblclick")) return swallow(event);
        // Panning, Figma's menu and the Move tool are Figma's
        if (prefs.tool === "move" || space || !onCanvas(event)) return;
        if (type === "pointermove" || type === "mousemove" || type === "contextmenu" || type === "pointercancel") return;
        if (event.button !== 0) return;
        swallow(event);
        if (type === "pointerdown") down(event);
      },
      true,
    );
  }
  addEventListener(
    "pointerleave",
    () => {
      pointer.over = false;
    },
    true,
  );

  const down = (event) => {
    const figma = api();
    const v = view();
    const page = pageAt(event, v);
    if (!figma || !page) return;
    const tool = prefs.tool;
    const base = { tool, pointer: event.pointerId, from: page, zoom: v.zoom };
    try {
      v.canvas.setPointerCapture(event.pointerId);
    } catch {}
    if (TOOLS[tool].select && tool !== "wand") {
      const mode = selectMode(event);
      if (tool === "polygon") {
        polygon = { tool, mode, points: [page], zoom: v.zoom };
        return;
      }
      drag = { ...base, kind: "select", mode, points: [page], to: null, square: false, centered: false };
      return;
    }
    if (tool === "wand") return void wand(page, selectMode(event)).catch(() => notify("Couldn't read the layer"));
    if (tool === "bucket") return void bucket(page).catch(() => notify("Couldn't fill"));
    if (tool === "magicEraser") return void magicErase(page).catch(() => notify("Couldn't erase"));
    if (tool === "eyedropper") {
      drag = { ...base, kind: "sample", background: event.altKey };
      sample(page, event.altKey).catch(() => {});
      return;
    }
    if (tool === "clone" && event.altKey) {
      const node = rasterNode();
      if (!node) return notify("Select an image layer to clone from");
      read(node)
        .then((t) => {
          cloneSource = { id: t.id, at: apply(gridOf(t).inverse(), page), page, offset: null };
        })
        .catch(() => {});
      return;
    }
    if (tool === "gradient") {
      const node = rasterNode();
      const ready = node ? read(node) : Promise.resolve(blank(placement(figma, page)));
      drag = { ...base, kind: "gradient", to: null, fresh: !node, ready };
      ready
        .then((t) => {
          if (!drag || drag.ready !== ready) return;
          drag.t = t;
          drag.mask = maskFor(t);
          drag.live = copyOf(t.base);
          show(t, drag.live);
          if (drag.to) gradientInto(drag);
        })
        .catch(() => (drag = null));
      return;
    }
    // Brushes
    const stroke = startPaint(event, page);
    if (!stroke) return;
    drag = { ...base, kind: "paint", stroke };
    const pressure = event.pointerType === "pen" ? event.pressure : 1;
    stroke.queue = [[page, pressure]];
    stroke.ready
      .then((t) => {
        if (!t || stroke.cancelled) return;
        stroke.t = t;
        if (!beginStroke(stroke)) {
          stroke.cancelled = true;
          if (drag?.stroke === stroke) drag = null;
          return;
        }
        stroke.began = true;
        for (const [point, p] of stroke.queue) feed(stroke, point, p);
        stroke.queue = [];
        if (stroke.ended) finishPaint(stroke);
      })
      .catch(() => {
        stroke.cancelled = true;
        notify("Couldn't read the layer");
        if (drag?.stroke === stroke) drag = null;
      });
  };

  const move = (event) => {
    const v = view();
    if (!v) return;
    if (drag.kind === "paint") {
      const { stroke } = drag;
      const events = event.getCoalescedEvents?.() ?? [];
      for (const each of events.length ? events : [event]) {
        const page = pageAt(each, v);
        const pressure = each.pointerType === "pen" ? each.pressure : 1;
        if (!stroke.began) stroke.queue.push([page, pressure]);
        else feed(stroke, page, pressure);
      }
      return;
    }
    const page = pageAt(event, v);
    if (drag.kind === "select") {
      drag.to = page;
      drag.square = event.shiftKey && drag.mode !== "add";
      drag.centered = event.altKey && drag.mode !== "subtract";
      if (drag.tool === "lasso") drag.points.push(page);
    } else if (drag.kind === "gradient") {
      drag.to = page;
      if (event.shiftKey) {
        // In steps of 45°
        const [dx, dy] = [page[0] - drag.from[0], page[1] - drag.from[1]];
        const angle = Math.round(Math.atan2(dy, dx) / (Math.PI / 4)) * (Math.PI / 4);
        const length = Math.hypot(dx, dy);
        drag.to = [drag.from[0] + Math.cos(angle) * length, drag.from[1] + Math.sin(angle) * length];
      }
      if (drag.live) gradientInto(drag);
    } else if (drag.kind === "sample") sample(page, drag.background, true).catch(() => {});
  };

  const up = (event) => {
    const done = drag;
    drag = null;
    afterDrag = true;
    setTimeout(() => (afterDrag = false));
    try {
      mainCanvas()?.releasePointerCapture(done.pointer);
    } catch {}
    if (done.kind === "select") finishSelection(done);
    else if (done.kind === "paint") {
      // Still reading, it's finished once it's read
      done.stroke.ended = true;
      if (done.stroke.began) finishPaint(done.stroke);
    } else if (done.kind === "gradient") {
      if (!done.live || !done.to || Math.hypot(done.to[0] - done.from[0], done.to[1] - done.from[1]) * done.zoom < 2) return;
      if (done.fresh) target = done.t;
      adopt(done.t, done.live);
    }
  };
  const finishPaint = (stroke) => {
    if (stroke.finished) return;
    stroke.finished = true;
    endStroke(stroke);
  };
  const closePolygon = () => {
    const done = polygon;
    polygon = null;
    if (done) finishSelection({ ...done, kind: "select" });
  };

  // ---------------------------------------------------------------------
  // Keys, Photoshop's

  const KEYS = {};
  for (const [tool, { key }] of Object.entries(TOOLS)) (KEYS[key] ??= []).push(tool);
  const pick = (tool) => {
    if (!TOOLS[tool]) return;
    if (drag) cancelAll();
    polygon = null;
    prefs.tool = tool;
    // Figma's own Move, for selecting and moving layers
    if (tool === "move") document.querySelector('[data-testid="Move-tool"]:not([aria-hidden="true"])')?.click();
    const group = groupOf(tool);
    prefs.picked[GROUPS.indexOf(group)] = tool;
    savePrefs();
    renderTools();
    renderBar();
    preload();
  };
  const sizeBy = (grow) => {
    const o = options();
    if (o.size === undefined) return;
    const stepBy = o.size < 10 ? 1 : o.size < 50 ? 5 : o.size < 100 ? 10 : 25;
    o.size = clamp(Math.round(grow ? o.size + stepBy : o.size - stepBy), 1, 2500);
    savePrefs();
    renderBar();
  };
  const hardnessBy = (grow) => {
    const o = options();
    if (o.hardness === undefined) return;
    o.hardness = clamp(o.hardness + (grow ? 25 : -25), 0, 100);
    savePrefs();
    renderBar();
  };
  addEventListener(
    "keydown",
    (event) => {
      if (event.key === " ") space = true;
      if (!active) return;
      if (typing(event)) {
        // Typing in Raster mode's own fields stays there
        if (event.target.closest?.(`[${ATTR}-ui]`)) event.stopImmediatePropagation();
        return;
      }
      const mod = mac ? event.metaKey : event.ctrlKey;
      const key = event.key.length === 1 ? event.key.toUpperCase() : event.key;
      const handled = () => swallow(event);
      if (event.key === "Escape") {
        if (polygon || drag || menuOpen()) {
          handled();
          closeMenus();
          cancelAll();
        }
        return;
      }
      if (event.key === "Enter" && polygon) return void (handled(), closePolygon());
      if (event.key === "Backspace" && polygon && !mod) {
        handled();
        polygon.points.pop();
        if (!polygon.points.length) polygon = null;
        return;
      }
      if (mod && !event.altKey) {
        // Strokes not yet in Figma are undone here, and go before a redo
        if (key === "Z" && !event.shiftKey && target?.unsaved) return void (handled(), discard(target));
        if ((key === "Y" || (key === "Z" && event.shiftKey)) && target?.unsaved) save(target);
        if (key === "D" && !event.shiftKey) return void (handled(), setSelection(null));
        if (key === "A" && !event.shiftKey && rasterNode()) return void (handled(), selectAll());
        if (key === "I" && event.shiftKey) return void (handled(), invert());
        if (key === "J" && rasterNode()) return void (handled(), layerVia(event.shiftKey).catch(() => notify("Couldn't make the layer")));
        if (key === "N" && event.shiftKey) return void (handled(), newLayer());
        if ((key === "C" || key === "X") && !event.shiftKey && selection && rasterNode()) return void (handled(), copyPixels(key === "X"));
        if ((event.key === "Backspace" || event.key === "Delete") && rasterNode()) return void (handled(), fillSelection(prefs.background));
        return;
      }
      if (event.altKey && !mod) {
        if (event.code === "BracketRight") return void (handled(), step(1));
        if (event.code === "BracketLeft") return void (handled(), step(-1));
        if ((event.key === "Backspace" || event.key === "Delete") && rasterNode()) return void (handled(), fillSelection(prefs.foreground));
        return;
      }
      if (mod || event.altKey) return;
      if ((event.key === "Backspace" || event.key === "Delete") && selection && rasterNode()) return void (handled(), fillSelection(null));
      if ((event.code === "BracketLeft" || event.code === "BracketRight") && options().size !== undefined) {
        handled();
        return event.shiftKey ? hardnessBy(event.code === "BracketRight") : sizeBy(event.code === "BracketRight");
      }
      const amount = ["opacity", "strength", "exposure", "flow"].find((name) => options()[name] !== undefined);
      if (/^Digit\d$/.test(event.code) && !event.shiftKey && amount) {
        handled();
        const o = options();
        const field = amount;
        const digit = +event.code.slice(5);
        o[field] = digit === 0 ? 100 : digit * 10;
        savePrefs();
        renderBar();
        return;
      }
      if (key === "X" && !event.shiftKey) return void (handled(), swapColors());
      if (key === "D" && !event.shiftKey) return void (handled(), resetColors());
      const tools = KEYS[key];
      if (tools) {
        handled();
        const group = groupOf(tools[0]);
        const current = prefs.picked[GROUPS.indexOf(group)] ?? group[0];
        if (event.shiftKey && tools.includes(prefs.tool)) pick(tools[(tools.indexOf(prefs.tool) + 1) % tools.length]);
        else pick(tools.includes(current) ? current : tools[0]);
        return;
      }
      // Figma's own tools stay out of the way: a key for one would pick it
      // with no button to show it
      if (/^Key[A-Z]$/.test(event.code) && !event.shiftKey && key !== "H") handled();
    },
    true,
  );
  for (const type of ["keyup", "keypress"]) {
    addEventListener(
      type,
      (event) => {
        if (type === "keyup" && event.key === " ") space = false;
        if (active && event.target?.closest?.(`[${ATTR}-ui]`) && typing(event)) event.stopImmediatePropagation();
      },
      true,
    );
  }
  addEventListener("blur", () => (space = false));

  // The selection, or the whole layer, into a new layer above, cut out of
  // this one with Shift
  const layerVia = async (cut) => {
    const figma = api();
    const node = rasterNode();
    let t = await read(node);
    await save(t);
    t = await read(node);
    const mask = maskFor(t);
    const copy = canvasOf(t.W, t.H);
    const ctx = copy.getContext("2d");
    ctx.drawImage(t.base, 0, 0);
    if (mask) {
      ctx.globalCompositeOperation = "destination-in";
      ctx.drawImage(mask.canvas, 0, 0);
    }
    if (cut) {
      const rest = canvasOf(t.W, t.H);
      const rctx = rest.getContext("2d");
      rctx.drawImage(t.base, 0, 0);
      rctx.globalCompositeOperation = "destination-out";
      if (mask) rctx.drawImage(mask.canvas, 0, 0);
      else rctx.fillRect(0, 0, t.W, t.H);
      await adopt(t, rest);
      await save(t);
    }
    const fresh = { ...blank({ parent: node.parent, index: node.parent.children.indexOf(node) + 1, M: t.M, w: t.w, h: t.h, like: node }), W: t.W, H: t.H, base: copy, live: copyOf(copy) };
    await commit(fresh, copy, { select: true });
    if (fresh.node) target = fresh;
    setSelection(null);
  };
  // The selected pixels to the clipboard, as a PNG cropped to them
  const copyPixels = async (cut) => {
    const t = await read(rasterNode());
    const mask = maskFor(t);
    const { alpha } = mask;
    let [x0, y0, x1, y1] = [t.W, t.H, 0, 0];
    for (let y = 0; y < t.H; y++)
      for (let x = 0; x < t.W; x++)
        if (alpha[y * t.W + x]) {
          x0 = Math.min(x0, x);
          y0 = Math.min(y0, y);
          x1 = Math.max(x1, x + 1);
          y1 = Math.max(y1, y + 1);
        }
    if (x1 <= x0) return;
    const out = canvasOf(x1 - x0, y1 - y0);
    const ctx = out.getContext("2d");
    ctx.drawImage(t.base, -x0, -y0);
    ctx.globalCompositeOperation = "destination-in";
    ctx.drawImage(mask.canvas, -x0, -y0);
    try {
      const blob = await new Promise((resolve) => out.toBlob(resolve, "image/png"));
      await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
      if (cut) await fillSelection(null);
    } catch {
      notify("Couldn't copy the pixels");
    }
  };

  // ---------------------------------------------------------------------
  // Colors

  const setColor = (which, color, recent = true) => {
    prefs[which] = color.map((value) => clamp(Math.round(value), 0, 255));
    if (recent) {
      const key = hex(prefs[which]);
      prefs.recent = [key, ...prefs.recent.filter((each) => each !== key)].slice(0, 10);
    }
    savePrefs();
    renderColors();
    if (picker?.which === which) syncPicker();
  };
  const swapColors = () => {
    [prefs.foreground, prefs.background] = [prefs.background, prefs.foreground];
    savePrefs();
    renderColors();
    if (picker) syncPicker();
  };
  const resetColors = () => {
    prefs.foreground = [0, 0, 0];
    prefs.background = [255, 255, 255];
    savePrefs();
    renderColors();
    if (picker) syncPicker();
  };

  // HSV, for the picker
  const toHsv = ([r, g, b]) => {
    [r, g, b] = [r / 255, g / 255, b / 255];
    const max = Math.max(r, g, b);
    const d = max - Math.min(r, g, b);
    let h = 0;
    if (d) h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
    return [((h * 60) + 360) % 360, max ? d / max : 0, max];
  };
  const fromHsv = ([h, s, v]) => {
    const f = (n) => {
      const k = (n + h / 60) % 6;
      return Math.round((v - v * s * Math.max(0, Math.min(k, 4 - k, 1))) * 255);
    };
    return [f(5), f(3), f(1)];
  };

  let picker = null;
  const openPicker = (which, anchor) => {
    if (picker?.which === which) return closeMenus();
    closeMenus();
    const root = make("div", { [`${ATTR}-ui`]: true, [`${ATTR}-picker`]: true, "data-fullscreen-prevent-event-capture": "true", role: "dialog", "aria-label": which === "foreground" ? "Foreground color" : "Background color" });
    const title = root.appendChild(make("div", { class: "title" }, make("span", { text: which === "foreground" ? "Foreground" : "Background" })));
    const area = root.appendChild(make("div", { class: "area" }, make("div", { class: "knob" })));
    const hue = root.appendChild(make("div", { class: "hue" }, make("div", { class: "knob" })));
    const row = root.appendChild(make("div", { class: "row" }));
    const swatch = row.appendChild(make("span", { class: "swatch" }));
    const input = row.appendChild(make("input", { type: "text", spellcheck: "false", "aria-label": "Hex" }));
    if (window.EyeDropper) {
      const dropper = row.appendChild(make("button", { type: "button", class: "icon" }));
      dropper.innerHTML = icon(SMALL.picker, 16);
      tooltip(dropper, "Pick a color from the screen");
      dropper.addEventListener("click", async () => {
        try {
          const { sRGBHex } = await new window.EyeDropper().open();
          const color = parseHex(sRGBHex);
          if (color) setColor(which, color);
        } catch {}
      });
    }
    const recents = root.appendChild(make("div", { class: "recent" }));
    for (const each of prefs.recent) {
      const color = parseHex(each);
      if (!color) continue;
      const button = recents.appendChild(make("button", { type: "button", style: `background:${each}` }));
      tooltip(button, each.toUpperCase());
      button.addEventListener("click", () => setColor(which, color));
    }
    if (!recents.childElementCount) recents.remove();
    const hsv = toHsv(prefs[which]);
    picker = { which, root, area, hue, swatch, input, hsv, title };
    const fromArea = (event) => {
      const rect = area.getBoundingClientRect();
      picker.hsv[1] = clamp((event.clientX - rect.left) / rect.width, 0, 1);
      picker.hsv[2] = 1 - clamp((event.clientY - rect.top) / rect.height, 0, 1);
      setColor(which, fromHsv(picker.hsv), false);
    };
    const fromHue = (event) => {
      const rect = hue.getBoundingClientRect();
      picker.hsv[0] = clamp((event.clientX - rect.left) / rect.width, 0, 0.9999) * 360;
      setColor(which, fromHsv(picker.hsv), false);
    };
    for (const [element, handler] of [
      [area, fromArea],
      [hue, fromHue],
    ]) {
      element.addEventListener("pointerdown", (event) => {
        element.setPointerCapture(event.pointerId);
        handler(event);
        const moveTo = (each) => handler(each);
        const stop = () => {
          element.removeEventListener("pointermove", moveTo);
          setColor(which, prefs[which]);
        };
        element.addEventListener("pointermove", moveTo);
        element.addEventListener("pointerup", stop, { once: true });
      });
    }
    input.addEventListener("change", () => {
      const color = parseHex(input.value);
      if (color) {
        picker.hsv = toHsv(color);
        setColor(which, color);
      } else syncPicker();
    });
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter") input.blur();
      if (event.key === "Escape") closeMenus();
    });
    document.body.append(root);
    const rect = anchor.getBoundingClientRect();
    const box = root.getBoundingClientRect();
    root.style.left = `${clamp(rect.left + rect.width / 2 - box.width / 2, 8, innerWidth - box.width - 8)}px`;
    root.style.top = `${Math.max(8, rect.top - box.height - 12)}px`;
    syncPicker();
  };
  const syncPicker = () => {
    if (!picker) return;
    const color = prefs[picker.which];
    const [h, s, v] = picker.hsv;
    // Grays keep the hue they had
    const now = toHsv(color);
    if (hex(fromHsv(picker.hsv)) !== hex(color)) picker.hsv = [now[1] && now[2] ? now[0] : h, now[2] ? now[1] : s, now[2]];
    picker.area.style.setProperty("--hue", `hsl(${picker.hsv[0]} 100% 50%)`);
    const knob = picker.area.firstChild;
    knob.style.left = `${picker.hsv[1] * 100}%`;
    knob.style.top = `${(1 - picker.hsv[2]) * 100}%`;
    knob.style.background = rgb(color);
    picker.hue.firstChild.style.left = `${(picker.hsv[0] / 360) * 100}%`;
    picker.swatch.style.background = rgb(color);
    if (document.activeElement !== picker.input) picker.input.value = hex(color).slice(1).toUpperCase();
  };

  // ---------------------------------------------------------------------
  // The toolbar: layers, the tools and the colors, in place of Figma's
  // tools

  const button = (label, shortcut, body, size = 24) => {
    const node = make("button", { type: "button", class: "tool", "data-fullscreen-prevent-event-capture": "true" });
    node.innerHTML = icon(body, size);
    return tooltip(node, label, shortcut);
  };
  const modKey = mac ? "⌘" : "Ctrl+";
  const altKey = mac ? "⌥" : "Alt+";
  const mount = () => {
    const carousel = document.querySelector('[class*="toolbelt_left_side--leftSideCarouselContainer"]');
    if (!carousel) return;
    if (!ui.tools) {
      const tools = make("div", { [`${ATTR}-ui`]: true, [`${ATTR}-tools`]: true, role: "group", "aria-label": "Raster tools" });
      // Layers: compact, on the very left
      const layers = tools.appendChild(make("div", { class: "layers", role: "group", "aria-label": "Layers" }));
      const up = layers.appendChild(button("Layer above", `${altKey}]`, SMALL.up, 16));
      const add = layers.appendChild(button("New layer", `${mac ? "⇧⌘" : "Ctrl+Shift+"}N`, SMALL.add, 16));
      const downward = layers.appendChild(button("Layer below", `${altKey}[`, SMALL.down, 16));
      up.addEventListener("click", () => step(1));
      add.addEventListener("click", () => newLayer());
      downward.addEventListener("click", () => step(-1));
      tools.appendChild(make("div", { class: "divider" }));
      const groups = tools.appendChild(make("div", { class: "groups" }));
      ui.groupButtons = GROUPS.map((group, index) => {
        const wrap = groups.appendChild(make("div", { class: "group", role: "group" }));
        const main = wrap.appendChild(make("button", { type: "button", class: "tool", "data-fullscreen-prevent-event-capture": "true", "aria-pressed": "false" }));
        main.addEventListener("click", () => pick(prefs.picked[index] && group.includes(prefs.picked[index]) ? prefs.picked[index] : group[0]));
        let chevron = null;
        if (group.length > 1) {
          chevron = wrap.appendChild(make("button", { type: "button", class: "chevron", "aria-haspopup": "menu", "aria-expanded": "false", "data-fullscreen-prevent-event-capture": "true" }));
          chevron.innerHTML = icon(SMALL.chevron);
          tooltip(chevron, GROUP_NAMES[index]);
          chevron.addEventListener("click", () => openMenu(index, chevron));
        }
        return { wrap, main, chevron, group, index };
      });
      tools.appendChild(make("div", { class: "divider" }));
      // Foreground and background, Photoshop's pair
      const colors = tools.appendChild(make("div", { class: "colors" }));
      const background = colors.appendChild(make("button", { type: "button", class: "swatch background", "data-fullscreen-prevent-event-capture": "true" }));
      const foreground = colors.appendChild(make("button", { type: "button", class: "swatch foreground", "data-fullscreen-prevent-event-capture": "true" }));
      const swap = colors.appendChild(make("button", { type: "button", class: "mini swap", "data-fullscreen-prevent-event-capture": "true" }));
      swap.innerHTML = icon(SMALL.swap, 12);
      const reset = colors.appendChild(make("button", { type: "button", class: "mini reset", "data-fullscreen-prevent-event-capture": "true" }));
      reset.innerHTML = `<span></span><span></span>`;
      tooltip(foreground, "Foreground color");
      tooltip(background, "Background color");
      tooltip(swap, "Swap colors", "X");
      tooltip(reset, "Default colors", "D");
      foreground.addEventListener("click", () => openPicker("foreground", foreground));
      background.addEventListener("click", () => openPicker("background", background));
      swap.addEventListener("click", swapColors);
      reset.addEventListener("click", resetColors);
      Object.assign(ui, { tools, foreground, background });
    }
    if (ui.tools.nextElementSibling !== carousel) carousel.before(ui.tools);
    renderTools();
    renderColors();
    if (!ui.bar) buildBar();
    if (!ui.bar.isConnected) document.body.append(ui.bar);
    renderBar();
  };
  const renderTools = () => {
    if (!ui.groupButtons) return;
    for (const { main, chevron, group, index } of ui.groupButtons) {
      const shownTool = group.includes(prefs.tool) ? prefs.tool : group.includes(prefs.picked[index]) ? prefs.picked[index] : group[0];
      const info = TOOLS[shownTool];
      if (main.dataset.tool !== shownTool) {
        main.dataset.tool = shownTool;
        main.innerHTML = icon(ICONS[info.icon]);
        tooltip(main, info.label, info.key);
      }
      const on = group.includes(prefs.tool);
      main.setAttribute("aria-pressed", on);
      main.classList.toggle("selected", on);
      chevron?.classList.toggle("selected", on);
    }
  };
  const renderColors = () => {
    if (!ui.foreground) return;
    ui.foreground.style.background = rgb(prefs.foreground);
    ui.background.style.background = rgb(prefs.background);
  };

  // A group's tools, Figma's flyout menu
  let menu = null;
  const menuOpen = () => !!(menu || picker || tipPopover);
  const closeMenus = () => {
    if (menu) {
      menu.chevron.setAttribute("aria-expanded", "false");
      menu.root.remove();
      menu = null;
    }
    if (picker) {
      picker.root.remove();
      picker = null;
    }
    if (tipPopover) {
      tipPopover.root.remove();
      tipPopover.anchor.setAttribute("aria-expanded", "false");
      tipPopover = null;
    }
  };
  const openMenu = (index, chevron) => {
    if (menu?.index === index) return closeMenus();
    closeMenus();
    const group = GROUPS[index];
    const root = make("div", { [`${ATTR}-ui`]: true, [`${ATTR}-menu`]: true, "data-fullscreen-prevent-event-capture": "true" });
    const list = root.appendChild(make("ul", { role: "menu", "aria-label": tooltipOf(chevron) }));
    const items = group.map((tool) => {
      const info = TOOLS[tool];
      const item = list.appendChild(make("li", { role: "menuitemradio", tabindex: "-1", "aria-checked": String(prefs.tool === tool || (!group.includes(prefs.tool) && (prefs.picked[index] ?? group[0]) === tool)) }));
      item.innerHTML = `<span class="row"><span class="check">${icon(SMALL.check, 16)}</span><span class="glyph">${icon(ICONS[info.icon])}</span><span class="name"></span><span class="key">${(group.indexOf(tool) ? (mac ? "⇧" : "Shift+") : "") + info.key}</span></span>`;
      item.querySelector(".name").textContent = info.label;
      item.addEventListener("click", () => {
        closeMenus();
        pick(tool);
      });
      item.addEventListener("pointermove", () => item.focus());
      item.addEventListener("keydown", (event) => {
        const at = items.indexOf(item);
        if (event.key === "ArrowDown") items[(at + 1) % items.length].focus();
        else if (event.key === "ArrowUp") items[(at + items.length - 1) % items.length].focus();
        else if (event.key === "Enter" || event.key === " ") item.click();
        else if (event.key === "Escape") {
          closeMenus();
          chevron.focus();
        } else return;
        event.preventDefault();
        event.stopPropagation();
      });
      return item;
    });
    document.body.append(root);
    const rect = chevron.closest(".group").getBoundingClientRect();
    const box = root.getBoundingClientRect();
    root.style.left = `${clamp(rect.left, 8, innerWidth - box.width - 8)}px`;
    root.style.top = `${rect.top - box.height - 14}px`;
    chevron.setAttribute("aria-expanded", "true");
    menu = { index, root, chevron };
    (items.find((item) => item.getAttribute("aria-checked") === "true") ?? items[0]).focus();
  };
  const tooltipOf = (node) => node.dataset.tooltip ?? "";

  // ---------------------------------------------------------------------
  // The options bar, above the toolbar where Figma shows its own for a
  // selection, as Photoshop shows a tool's options

  const SELECT_MODES = [
    ["new", "New selection", `<rect ${STROKE} x="3.5" y="3.5" width="9" height="9" rx="1"/>`],
    ["add", "Add to selection", `<rect ${STROKE} x="2.5" y="2.5" width="7" height="7" rx="1"/><path ${STROKE} d="M9.5 6.5h3a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1h-5a1 1 0 0 1-1-1v-3"/>`],
    ["subtract", "Subtract from selection", `<rect ${STROKE} x="2.5" y="2.5" width="7" height="7" rx="1"/><path ${STROKE} stroke-dasharray="1.5 1.5" d="M9.5 6.5h3a1 1 0 0 1 1 1v5a1 1 0 0 1-1 1h-5a1 1 0 0 1-1-1v-3"/>`],
    ["intersect", "Intersect with selection", `<path ${STROKE} stroke-dasharray="1.5 1.5" d="M2.5 9.5v-6a1 1 0 0 1 1-1h5a1 1 0 0 1 1 1M13.5 6.5v6a1 1 0 0 1-1 1h-5a1 1 0 0 1-1-1"/><rect fill="currentColor" x="6.5" y="6.5" width="3" height="3"/>`],
  ];
  const FIELDS = {
    size: { label: "Size", min: 1, max: 2500, unit: "px", tip: "Brush size, in the layer's pixels", keys: "[ ]" },
    hardness: { label: "Hardness", min: 0, max: 100, unit: "%", tip: "How sharp the brush's edge is", keys: "⇧[ ⇧]" },
    opacity: { label: "Opacity", min: 1, max: 100, unit: "%", tip: "The most a stroke covers", keys: "1–0" },
    strength: { label: "Strength", min: 1, max: 100, unit: "%" },
    exposure: { label: "Exposure", min: 1, max: 100, unit: "%" },
    flow: { label: "Flow", min: 1, max: 100, unit: "%" },
    tolerance: { label: "Tolerance", min: 0, max: 255, unit: "", tip: "How far a color can be from the one clicked" },
    feather: { label: "Feather", min: 0, max: 250, unit: "px", tip: "Softens the selection's edge" },
    smoothing: { label: "Smoothing", min: 0, max: 100, unit: "%", tip: "Steadies strokes: the brush trails the pointer on a string" },
    angle: { label: "Angle", min: -180, max: 180, unit: "°" },
    roundness: { label: "Roundness", min: 1, max: 100, unit: "%" },
  };
  const TOGGLES = {
    pressure: ["Pressure", "Pen pressure sets the size"],
    contiguous: ["Contiguous", "Only colors touching the one clicked"],
    all: ["All layers", "Sample what's drawn, not only this layer"],
    smooth: ["Anti-alias", "Smooths the selection's edge"],
    aligned: ["Aligned", "Each stroke carries on from where the last left off"],
    reverse: ["Reverse", "Swap the gradient's ends"],
  };
  const CHOICES = {
    shape: ["Shape", [["linear", "Linear"], ["radial", "Radial"]]],
    colors: ["Colors", [["background", "To background"], ["clear", "To clear"]]],
    sample: ["Sample", [[1, "Point"], [3, "3 × 3"], [5, "5 × 5"]]],
    saturate: ["Mode", [[false, "Desaturate"], [true, "Saturate"]]],
  };
  const ORDER = ["mode", "size", "tip", "hardness", "opacity", "strength", "exposure", "flow", "smoothing", "tolerance", "feather", "shape", "colors", "sample", "saturate", "pressure", "contiguous", "all", "smooth", "aligned", "reverse"];

  const buildBar = () => {
    ui.bar = make("div", { [`${ATTR}-ui`]: true, [`${ATTR}-bar`]: true, role: "toolbar", "aria-label": "Tool options", "data-fullscreen-prevent-event-capture": "true" });
  };
  let barKey = "";
  // The layer painted on, and the selection, as the bar shows them
  const status = () => {
    const node = selected();
    const raster = isRaster(node);
    let name = "";
    try {
      name = node?.name ?? "";
    } catch {}
    return { node, raster, name };
  };
  let shownId = null;
  const refreshBar = () => {
    if (!ui.bar) return;
    const { node, raster, name } = status();
    shownId = raster ? node.id : null;
    const key = `${prefs.tool}|${node?.id ?? ""}|${raster}|${name}|${!!selection}`;
    if (key !== barKey) renderBar();
  };
  const renderBar = () => {
    if (!ui.bar) return;
    if (ui.bar.contains(document.activeElement) && document.activeElement.tagName === "INPUT") return;
    const { node, raster, name } = status();
    barKey = `${prefs.tool}|${node?.id ?? ""}|${raster}|${name}|${!!selection}`;
    const bar = ui.bar;
    bar.replaceChildren();
    const tool = prefs.tool;
    const o = options();
    // Where it paints
    const layer = bar.appendChild(make("div", { class: "layer" }));
    layer.innerHTML = icon(SMALL.layer, 16);
    const label = layer.appendChild(make("span"));
    if (raster) {
      label.textContent = name;
      tooltip(layer, "Painting on this layer");
    } else {
      label.textContent = node ? `${name}` : "No layer";
      layer.classList.add("muted");
      tooltip(layer, TOOLS[tool].paints || tool === "bucket" || tool === "gradient" ? "Painting adds a new layer" : "Select an image layer");
    }
    if (tool === "move") {
      bar.appendChild(make("div", { class: "divider" }));
      bar.appendChild(make("span", { class: "hint", text: "Select, move and resize layers as in Design" }));
    }
    for (const name of ORDER) {
      if (name === "tip" ? o.angle === undefined : o[name] === undefined) continue;
      // The tip sits with the size, as one
      if (name !== "tip") bar.appendChild(make("div", { class: "divider" }));
      if (name === "tip") {
        const button = bar.appendChild(tipButton(o));
        if (tipPopover?.o === o) tipPopover.anchor = button;
      }
      else if (name === "mode") bar.appendChild(modes(o));
      else if (FIELDS[name]) bar.appendChild(field(o, name));
      else if (TOGGLES[name]) bar.appendChild(toggle(o, name));
      else if (CHOICES[name]) bar.appendChild(choice(o, name));
    }
    if (tool === "clone") {
      bar.appendChild(make("div", { class: "divider" }));
      bar.appendChild(make("span", { class: "hint", text: cloneSource ? "Source set" : `${mac ? "Option" : "Alt"}-click to set the source` }));
    }
    if (selection) {
      bar.appendChild(make("div", { class: "divider" }));
      const invertButton = bar.appendChild(make("button", { type: "button", class: "text", text: "Invert" }));
      tooltip(invertButton, "Invert the selection", `${mac ? "⇧⌘" : "Ctrl+Shift+"}I`);
      invertButton.addEventListener("click", invert);
      const deselect = bar.appendChild(make("button", { type: "button", class: "text", text: "Deselect" }));
      tooltip(deselect, "Deselect", `${modKey}D`);
      deselect.addEventListener("click", () => setSelection(null));
    }
    placeBar();
  };
  const changed = () => {
    savePrefs();
    renderBar();
  };

  // The brush tip's angle and roundness, Photoshop's dial: a circle with
  // the tip's outline in it, an arrow along its long axis to turn it and
  // two dots across it to squash it
  const dial = (o, size) => {
    const svg = make("svg:svg", { width: size, height: size, viewBox: "-50 -50 100 100", class: "dial", "aria-hidden": "true" });
    svg.innerHTML = `<circle class="track" r="44"/><g class="shape"><ellipse class="outline" rx="34" ry="34"/><path class="axis" d="M-34 0H34"/><path class="arrow" d="M34 0l-9-5v10z"/><circle class="dot" cx="0" cy="-34" r="4.5"/><circle class="dot" cx="0" cy="34" r="4.5"/></g>`;
    const sync = () => {
      const ry = (34 * o.roundness) / 100;
      svg.querySelector(".shape").setAttribute("transform", `rotate(${-o.angle})`);
      svg.querySelector(".outline").setAttribute("ry", ry);
      const [top, bottom] = svg.querySelectorAll(".dot");
      top.setAttribute("cy", -ry);
      bottom.setAttribute("cy", ry);
    };
    sync();
    svg.sync = sync;
    return svg;
  };
  const tipButton = (o) => {
    const button = make("button", { type: "button", class: "tip", "aria-haspopup": "dialog", "aria-expanded": String(tipPopover?.o === o) });
    button.append(dial(o, 24));
    tooltip(button, `Brush tip: ${o.angle}°, ${o.roundness}% round`);
    button.addEventListener("click", () => openTip(o, button));
    return button;
  };
  let tipPopover = null;
  const openTip = (o, anchor) => {
    if (tipPopover) return closeMenus();
    closeMenus();
    const root = make("div", { [`${ATTR}-ui`]: true, [`${ATTR}-tip`]: true, "data-fullscreen-prevent-event-capture": "true", role: "dialog", "aria-label": "Brush tip" });
    root.appendChild(make("div", { class: "title" }, make("span", { text: "Brush tip" })));
    const big = root.appendChild(dial(o, 136));
    big.setAttribute("aria-hidden", "false");
    big.setAttribute("role", "img");
    const fields = root.appendChild(make("div", { class: "fields" }));
    const renderFields = () => {
      fields.replaceChildren(field(o, "angle", refresh), field(o, "roundness", refresh));
    };
    // The dial follows the fields, and the bar's button both
    function refresh() {
      big.sync();
      anchor.querySelector(".dial")?.sync?.();
      tooltip(anchor, `Brush tip: ${o.angle}°, ${o.roundness}% round`);
    }
    renderFields();
    // Dragging the dots squashes the tip, anywhere else turns it, as the
    // arrow points to the pointer
    big.addEventListener("pointerdown", (event) => {
      event.preventDefault();
      try {
        big.setPointerCapture(event.pointerId);
      } catch {}
      const rect = big.getBoundingClientRect();
      const local = (each) => {
        const scale = 100 / rect.width;
        return [(each.clientX - rect.left) * scale - 50, (each.clientY - rect.top) * scale - 50];
      };
      const [x, y] = local(event);
      const a = (o.angle * Math.PI) / 180;
      // Across the tip's long axis, where the dots are
      const across = Math.abs(-Math.sin(-a) * x + Math.cos(-a) * y);
      const along = Math.abs(Math.cos(-a) * x + Math.sin(-a) * y);
      const squash = Math.abs(across - (34 * o.roundness) / 100) < 9 && along < 12;
      const moveTo = (each) => {
        const [px, py] = local(each);
        if (squash) {
          const off = Math.abs(-Math.sin(-a) * px + Math.cos(-a) * py);
          o.roundness = clamp(Math.round((off / 34) * 100), 1, 100);
        } else {
          let angle = Math.round((Math.atan2(-py, px) * 180) / Math.PI);
          if (each.shiftKey) angle = Math.round(angle / 15) * 15;
          o.angle = angle > 180 ? angle - 360 : angle;
        }
        savePrefs();
        refresh();
        renderFields();
      };
      moveTo(event);
      big.addEventListener("pointermove", moveTo);
      big.addEventListener("pointerup", () => big.removeEventListener("pointermove", moveTo), { once: true });
    });
    // Double-click puts it back round and upright
    big.addEventListener("dblclick", () => {
      o.angle = 0;
      o.roundness = 100;
      savePrefs();
      refresh();
      renderFields();
    });
    document.body.append(root);
    const rect = anchor.getBoundingClientRect();
    const box = root.getBoundingClientRect();
    root.style.left = `${clamp(rect.left + rect.width / 2 - box.width / 2, 8, innerWidth - box.width - 8)}px`;
    root.style.top = `${Math.max(8, rect.top - box.height - 12)}px`;
    anchor.setAttribute("aria-expanded", "true");
    tipPopover = { root, o, anchor };
  };
  const modes = (o) => {
    const group = make("div", { class: "segments", role: "radiogroup", "aria-label": "Selection mode" });
    for (const [value, label, body] of SELECT_MODES) {
      const item = group.appendChild(make("button", { type: "button", role: "radio", "aria-checked": String(o.mode === value) }));
      item.innerHTML = icon(body, 16);
      tooltip(item, label, { new: "", add: mac ? "⇧" : "Shift", subtract: mac ? "⌥" : "Alt", intersect: mac ? "⇧⌥" : "Shift+Alt" }[value]);
      item.addEventListener("click", () => {
        o.mode = value;
        changed();
      });
    }
    return group;
  };
  // A number, typed, or dragged by its label as Figma's fields are
  const field = (o, name, onChange = null) => {
    const spec = FIELDS[name];
    const wrap = make("div", { class: "field" });
    const label = wrap.appendChild(make("span", { class: "label", text: spec.label }));
    if (spec.tip) tooltip(label, spec.tip, spec.keys ?? "");
    const input = wrap.appendChild(make("input", { type: "text", inputmode: "numeric", value: `${o[name]}`, "aria-label": spec.label, spellcheck: "false" }));
    if (spec.unit) wrap.appendChild(make("span", { class: "unit", text: spec.unit }));
    input.style.width = `${Math.max(2, String(spec.max).length) + 1}ch`;
    const set = (value) => {
      if (!Number.isFinite(value)) return;
      o[name] = clamp(Math.round(value), spec.min, spec.max);
      input.value = o[name];
      savePrefs();
      onChange?.();
    };
    input.addEventListener("change", () => set(parseFloat(input.value)));
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter") input.blur();
      else if (event.key === "Escape") {
        input.value = o[name];
        input.blur();
      } else if (event.key === "ArrowUp" || event.key === "ArrowDown") {
        event.preventDefault();
        set((parseFloat(input.value) || 0) + (event.key === "ArrowUp" ? 1 : -1) * (event.shiftKey ? 10 : 1));
      }
    });
    if (!onChange) input.addEventListener("blur", () => setTimeout(renderBar));
    label.addEventListener("pointerdown", (event) => {
      event.preventDefault();
      label.setPointerCapture(event.pointerId);
      const start = [event.clientX, o[name]];
      const per = (spec.max - spec.min) > 300 ? 1 : 0.5;
      let moved = false;
      const moveTo = (each) => {
        if (Math.abs(each.clientX - start[0]) > 2) moved = true;
        if (moved) set(start[1] + (each.clientX - start[0]) * per * (each.shiftKey ? 10 : 1));
      };
      label.addEventListener("pointermove", moveTo);
      label.addEventListener(
        "pointerup",
        () => {
          label.removeEventListener("pointermove", moveTo);
          if (moved) return;
          input.focus();
          input.select();
        },
        { once: true },
      );
    });
    return wrap;
  };
  const toggle = (o, name) => {
    const [label, tip] = TOGGLES[name];
    const item = make("button", { type: "button", class: "toggle", role: "switch", "aria-checked": String(!!o[name]) }, make("span", { class: "box" }), make("span", { text: label }));
    tooltip(item, tip);
    item.setAttribute("aria-label", label);
    item.addEventListener("click", () => {
      o[name] = !o[name];
      changed();
    });
    return item;
  };
  const choice = (o, name) => {
    const [label, values] = CHOICES[name];
    const group = make("div", { class: "segments text", role: "radiogroup", "aria-label": label });
    for (const [value, text] of values) {
      const item = group.appendChild(make("button", { type: "button", role: "radio", "aria-checked": String(o[name] === value), text }));
      item.addEventListener("click", () => {
        o[name] = value;
        changed();
      });
    }
    return group;
  };
  const placeBar = () => {
    const bar = ui.bar;
    const toolbar = ui.tools?.closest('[role="toolbar"]');
    if (!bar || !toolbar) return;
    const rect = toolbar.getBoundingClientRect();
    if (!rect.width) return;
    const left = `${rect.left + rect.width / 2}px`;
    const bottom = `${innerHeight - rect.top + 8}px`;
    if (bar.style.left !== left) bar.style.left = left;
    if (bar.style.bottom !== bottom) bar.style.bottom = bottom;
  };

  // ---------------------------------------------------------------------
  // Styles

  const style = document.createElement("style");
  style.textContent = `
    html[${ATTR}] [class*="toolbelt_left_side--leftSideCarouselContainer"] { display: none !important; }
    html[${ATTR}] [data-secondary-toolbelt] { display: none !important; }
    html[${ATTR}] canvas[${CURSOR}] { cursor: var(--ligma-raster-cursor) !important; }
    [${ATTR}-tools] { display: flex; align-items: center; height: 48px; padding: 0 8px; gap: 8px; color: var(--color-icon); }
    [${ATTR}-tools] .groups { display: flex; gap: 8px; }
    [${ATTR}-tools] .group, [${ATTR}-tools] .layers { display: flex; gap: 1px; }
    [${ATTR}-tools] button { all: unset; box-sizing: border-box; display: flex; align-items: center; justify-content: center; border-radius: 5px; color: var(--color-icon); }
    [${ATTR}-tools] button:focus-visible { outline: 2px solid var(--color-border-selected, #0d99ff); outline-offset: -2px; }
    [${ATTR}-tools] .tool { width: 32px; height: 32px; }
    [${ATTR}-tools] .tool:hover, [${ATTR}-tools] .chevron:hover { background: var(--color-bg-hover); }
    [${ATTR}-tools] .tool.selected, [${ATTR}-tools] .tool.selected:hover { background: var(--color-bg-brand); color: var(--color-icon-onbrand, #fff); }
    [${ATTR}-tools] .chevron { width: 16px; height: 32px; }
    [${ATTR}-tools] .chevron[aria-expanded="true"] { background: var(--color-bg-hover); }
    [${ATTR}-tools] .layers .tool { width: 22px; color: var(--color-icon-secondary); }
    [${ATTR}-tools] .layers .tool:hover { color: var(--color-icon); }
    [${ATTR}-tools] .divider { width: 1px; align-self: stretch; margin: 0 -1px; background: var(--color-border); }
    [${ATTR}-tools] .colors { position: relative; width: 32px; height: 32px; flex: none; }
    [${ATTR}-tools] .swatch { position: absolute; width: 18px; height: 18px; border-radius: 4px; box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.2), 0 0 0 1px var(--color-bg); }
    [${ATTR}-tools] .swatch.foreground { left: 1px; top: 1px; z-index: 1; }
    [${ATTR}-tools] .swatch.background { right: 1px; bottom: 1px; }
    [${ATTR}-tools] .swatch:hover { box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.2), 0 0 0 1px var(--color-bg), 0 0 0 2px var(--color-icon-secondary); }
    [${ATTR}-tools] .mini { position: absolute; width: 12px; height: 12px; color: var(--color-icon-secondary); border-radius: 3px; }
    [${ATTR}-tools] .mini:hover { color: var(--color-icon); }
    [${ATTR}-tools] .swap { right: -1px; top: -1px; }
    [${ATTR}-tools] .reset { left: 0; bottom: 0; }
    [${ATTR}-tools] .reset span { position: absolute; width: 6px; height: 6px; border-radius: 1.5px; box-shadow: 0 0 0 1px var(--color-bg); }
    [${ATTR}-tools] .reset span:first-child { left: 4px; top: 4px; background: #fff; }
    [${ATTR}-tools] .reset span:last-child { left: 1px; top: 1px; background: #000; z-index: 1; }

    [${ATTR}-menu] { position: fixed; z-index: 100; min-width: 208px; padding: 8px 0; border-radius: 13px; background: var(--color-bg-menu, #1e1e1e); color: var(--color-text-menu, #fff); font: 450 11px/16px Inter, sans-serif; letter-spacing: 0.005em; box-shadow: 0 2px 5px rgb(0 0 0 / 0.35), inset 0 0 0.5px rgb(255 255 255 / 0.35), 0 10px 16px rgb(0 0 0 / 0.35), inset 0 0.5px 0 rgb(255 255 255 / 0.08); }
    [${ATTR}-menu] ul { list-style: none; margin: 0; padding: 0; }
    [${ATTR}-menu] li { padding: 0 8px; outline: none; color: var(--color-text-menu, rgb(255 255 255 / 0.9)); }
    [${ATTR}-menu] .row { display: flex; align-items: center; height: 24px; padding: 4px 8px 4px 4px; box-sizing: border-box; border-radius: 5px; gap: 0; }
    [${ATTR}-menu] li:focus .row, [${ATTR}-menu] li:hover .row { background: var(--color-bg-menu-selected, #0c8ce9); color: #fff; }
    [${ATTR}-menu] .check { width: 16px; height: 16px; display: flex; visibility: hidden; color: inherit; }
    [${ATTR}-menu] li[aria-checked="true"] .check { visibility: visible; }
    [${ATTR}-menu] .glyph { width: 24px; height: 24px; display: flex; margin: -4px 4px -4px 4px; color: inherit; }
    [${ATTR}-menu] .name { flex: 1; }
    [${ATTR}-menu] .key { margin-left: 24px; color: var(--color-text-menu-secondary, rgb(255 255 255 / 0.5)); }
    [${ATTR}-menu] li:focus .key, [${ATTR}-menu] li:hover .key { color: rgb(255 255 255 / 0.7); }

    [${ATTR}-bar] { position: fixed; z-index: 8; transform: translateX(-50%); display: flex; align-items: center; gap: 8px; height: 40px; padding: 0 8px; box-sizing: border-box; border-radius: 13px; background: var(--color-bg); color: var(--color-text); font: 450 11px/16px Inter, sans-serif; letter-spacing: 0.005em; white-space: nowrap; box-shadow: 0 3px 8px rgb(0 0 0 / 0.35), 0 1px 3px rgb(0 0 0 / 0.5), inset 0 1px 0 rgb(255 255 255 / 0.08), inset 0 0 1px rgb(255 255 255 / 0.3); }
    [${ATTR}-bar] .divider { width: 1px; align-self: stretch; background: var(--color-border); }
    [${ATTR}-bar] .layer { display: flex; align-items: center; gap: 4px; max-width: 160px; color: var(--color-text); }
    [${ATTR}-bar] .layer svg { flex: none; color: var(--color-icon-secondary); }
    [${ATTR}-bar] .layer span { overflow: hidden; text-overflow: ellipsis; }
    [${ATTR}-bar] .layer.muted { color: var(--color-text-secondary); }
    [${ATTR}-bar] .hint { color: var(--color-text-secondary); }
    [${ATTR}-bar] button { all: unset; box-sizing: border-box; display: flex; align-items: center; justify-content: center; height: 24px; border-radius: 5px; color: var(--color-text); cursor: default; }
    [${ATTR}-bar] button:focus-visible { outline: 2px solid var(--color-border-selected, #0d99ff); outline-offset: -2px; }
    [${ATTR}-bar] button.text { padding: 0 8px; }
    [${ATTR}-bar] button.text:hover, [${ATTR}-bar] .toggle:hover { background: var(--color-bg-hover); }
    [${ATTR}-bar] .segments { display: flex; padding: 1px; gap: 1px; border-radius: 6px; background: var(--color-bg-secondary); }
    [${ATTR}-bar] .segments button { min-width: 24px; height: 22px; padding: 0 4px; color: var(--color-icon-secondary); }
    [${ATTR}-bar] .segments.text button { padding: 0 8px; color: var(--color-text-secondary); }
    [${ATTR}-bar] .segments button:hover { color: var(--color-text); }
    [${ATTR}-bar] .segments button[aria-checked="true"] { background: var(--color-bg); color: var(--color-text); box-shadow: inset 0 0 0 1px var(--color-border); }
    [${ATTR}-bar] .field { display: flex; align-items: center; height: 24px; padding: 0 6px 0 0; border-radius: 5px; background: var(--color-bg-secondary); }
    [${ATTR}-bar] .field:focus-within { box-shadow: inset 0 0 0 1px var(--color-border-selected, #0d99ff); }
    [${ATTR}-bar] .field .label { padding: 0 6px 0 8px; height: 24px; line-height: 24px; color: var(--color-text-secondary); cursor: ew-resize; user-select: none; }
    [${ATTR}-bar] .field input { all: unset; min-width: 2ch; text-align: right; color: var(--color-text); font-variant-numeric: tabular-nums; }
    [${ATTR}-bar] .field .unit { margin-left: 1px; color: var(--color-text-secondary); }
    [${ATTR}-bar] .toggle { gap: 6px; padding: 0 8px 0 6px; color: var(--color-text-secondary); }
    [${ATTR}-bar] .toggle .box { width: 12px; height: 12px; border-radius: 3px; box-shadow: inset 0 0 0 1px var(--color-icon-secondary); }
    [${ATTR}-bar] .toggle[aria-checked="true"] { color: var(--color-text); }
    [${ATTR}-bar] .toggle[aria-checked="true"] .box { background: var(--color-bg-brand) url("data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="12" height="12" viewBox="0 0 16 16" fill="none"><path d="m4 8.5 2.5 2.5L12 5" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>`)}") center / 12px no-repeat; box-shadow: none; }

    [${ATTR}-bar] .tip { width: 28px; height: 28px; margin: 0 -4px 0 -2px; }
    [${ATTR}-bar] .tip:hover, [${ATTR}-bar] .tip[aria-expanded="true"] { background: var(--color-bg-hover); }
    [${ATTR}-bar] .dial .track, [${ATTR}-tip] .dial .track { fill: none; stroke: var(--color-icon-secondary); stroke-width: 6; }
    [${ATTR}-bar] .dial .outline { fill: var(--color-icon); stroke: none; opacity: 0.9; }
    [${ATTR}-bar] .dial .axis, [${ATTR}-bar] .dial .arrow, [${ATTR}-bar] .dial .dot { display: none; }
    [${ATTR}-tip] { position: fixed; z-index: 100; width: 216px; padding: 0 0 12px; border-radius: 13px; background: var(--color-bg, #2c2c2c); color: var(--color-text); font: 450 11px/16px Inter, sans-serif; box-shadow: 0 2px 5px rgb(0 0 0 / 0.35), inset 0 0 0.5px rgb(255 255 255 / 0.35), 0 10px 16px rgb(0 0 0 / 0.35), inset 0 0.5px 0 rgb(255 255 255 / 0.08); }
    [${ATTR}-tip] .title { display: flex; align-items: center; height: 40px; padding: 0 16px; font-weight: 550; border-bottom: 1px solid var(--color-border); margin-bottom: 12px; }
    [${ATTR}-tip] .dial { display: block; margin: 0 auto; cursor: grab; touch-action: none; }
    [${ATTR}-tip] .dial:active { cursor: grabbing; }
    [${ATTR}-tip] .dial .track { stroke-width: 1; stroke: var(--color-border); fill: var(--color-bg-secondary); }
    [${ATTR}-tip] .dial .outline { fill: none; stroke: var(--color-icon); stroke-width: 1.25; }
    [${ATTR}-tip] .dial .axis { stroke: var(--color-icon-secondary); stroke-width: 1; stroke-dasharray: 2 2; }
    [${ATTR}-tip] .dial .arrow { fill: var(--color-icon); }
    [${ATTR}-tip] .dial .dot { fill: var(--color-bg); stroke: var(--color-icon); stroke-width: 1.25; cursor: ns-resize; }
    [${ATTR}-tip] .fields { display: flex; gap: 8px; margin: 12px 12px 0; }
    [${ATTR}-tip] .field { flex: 1; display: flex; align-items: center; height: 24px; padding: 0 6px 0 0; border-radius: 5px; background: var(--color-bg-secondary); }
    [${ATTR}-tip] .field:focus-within { box-shadow: inset 0 0 0 1px var(--color-border-selected, #0d99ff); }
    [${ATTR}-tip] .field .label { padding: 0 6px 0 8px; color: var(--color-text-secondary); cursor: ew-resize; user-select: none; }
    [${ATTR}-tip] .field input { all: unset; flex: 1; min-width: 0; text-align: right; color: var(--color-text); font-variant-numeric: tabular-nums; }
    [${ATTR}-tip] .field .unit { margin-left: 1px; color: var(--color-text-secondary); }
    [${ATTR}-picker] { position: fixed; z-index: 100; width: 240px; padding: 0 0 12px; border-radius: 13px; background: var(--color-bg, #2c2c2c); color: var(--color-text); font: 450 11px/16px Inter, sans-serif; box-shadow: 0 2px 5px rgb(0 0 0 / 0.35), inset 0 0 0.5px rgb(255 255 255 / 0.35), 0 10px 16px rgb(0 0 0 / 0.35), inset 0 0.5px 0 rgb(255 255 255 / 0.08); }
    [${ATTR}-picker] .title { display: flex; align-items: center; height: 40px; padding: 0 16px; font-weight: 550; border-bottom: 1px solid var(--color-border); margin-bottom: 12px; }
    [${ATTR}-picker] .area { --hue: red; position: relative; height: 160px; margin: 0 12px; border-radius: 4px; background: linear-gradient(to top, #000, transparent), linear-gradient(to right, #fff, var(--hue)); cursor: crosshair; touch-action: none; }
    [${ATTR}-picker] .hue { position: relative; height: 12px; margin: 12px 18px; border-radius: 6px; background: linear-gradient(to right, red, #ff0, lime, cyan, blue, #f0f, red); touch-action: none; }
    [${ATTR}-picker] .hue::before, [${ATTR}-picker] .hue::after { content: none; }
    [${ATTR}-picker] .knob { position: absolute; width: 12px; height: 12px; margin: -6px 0 0 -6px; border-radius: 50%; box-shadow: 0 0 0 2px #fff, 0 0 0 3px rgb(0 0 0 / 0.25), inset 0 0 0 1px rgb(0 0 0 / 0.2); pointer-events: none; }
    [${ATTR}-picker] .hue .knob { top: 50%; background: transparent; }
    [${ATTR}-picker] .row { display: flex; align-items: center; gap: 8px; margin: 0 12px; }
    [${ATTR}-picker] .row .swatch { width: 16px; height: 16px; border-radius: 3px; box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.15); flex: none; }
    [${ATTR}-picker] .row input { all: unset; flex: 1; height: 24px; padding: 0 8px; border-radius: 5px; background: var(--color-bg-secondary); color: var(--color-text); text-transform: uppercase; font-variant-numeric: tabular-nums; }
    [${ATTR}-picker] .row input:focus { box-shadow: inset 0 0 0 1px var(--color-border-selected, #0d99ff); }
    [${ATTR}-picker] button.icon { all: unset; display: flex; align-items: center; justify-content: center; width: 24px; height: 24px; border-radius: 5px; color: var(--color-icon); }
    [${ATTR}-picker] button.icon:hover { background: var(--color-bg-hover); }
    [${ATTR}-picker] .recent { display: flex; flex-wrap: wrap; gap: 6px; margin: 12px 12px 0; }
    [${ATTR}-picker] .recent button { all: unset; width: 16px; height: 16px; border-radius: 3px; box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.15); }
    [${ATTR}-picker] .recent button:hover { box-shadow: inset 0 0 0 1px rgb(0 0 0 / 0.15), 0 0 0 1px var(--color-bg), 0 0 0 2px var(--color-icon-secondary); }

    [${ATTR}-overlay] { position: absolute; overflow: hidden; pointer-events: none; }
    [${ATTR}-overlay] .live { position: absolute; inset: 0; transition: opacity 0.3s ease-in-out; }
    [${ATTR}-overlay] .live.fading { opacity: 0; }
    [${ATTR}-overlay] .live canvas { position: absolute; left: 0; top: 0; width: 1px; height: 1px; transform-origin: 0 0; background: transparent !important; pointer-events: none !important; }
    [${ATTR}-overlay], [${ATTR}-overlay] * { pointer-events: none !important; }
    [${ATTR}-overlay] .marks { position: absolute; inset: 0; width: 100%; height: 100%; overflow: visible; }
    [${ATTR}-overlay] .ants path { fill: none; stroke-width: 1px; vector-effect: non-scaling-stroke; shape-rendering: crispEdges; }
    [${ATTR}-overlay] .ants .under { stroke: #fff; }
    [${ATTR}-overlay] .ants .over { stroke: #000; stroke-dasharray: 4 4; animation: ligma-raster-ants 0.5s linear infinite; }
    [${ATTR}-overlay] .shape { fill: none; stroke: #fff; stroke-width: 1px; }
    [${ATTR}-overlay] .shape.over { stroke: #000; stroke-dasharray: 4 4; animation: ligma-raster-ants 0.5s linear infinite; }
    [${ATTR}-overlay] .line { fill: none; stroke: var(--color-border-selected, #0d99ff); stroke-width: 1.5px; }
    [${ATTR}-overlay] .string { fill: none; stroke: var(--color-border-selected, #0d99ff); stroke-width: 1px; stroke-linecap: round; opacity: 0.8; }
    [${ATTR}-overlay] .source { fill: none; stroke: #fff; stroke-width: 1px; filter: drop-shadow(0 0 1px #000); }
    [${ATTR}-overlay] .brush { fill: none; stroke: #fff; stroke-width: 1px; }
    [${ATTR}-overlay] .brush.under { stroke: rgb(0 0 0 / 0.6); stroke-width: 3px; }
    @keyframes ligma-raster-ants { to { stroke-dashoffset: -8; } }
    @media (prefers-reduced-motion: reduce) { [${ATTR}-overlay] .over { animation: none !important; } }
  `;
  const mountStyle = () => {
    if (style.isConnected) return true;
    const parent = document.head ?? document.documentElement;
    parent?.append(style);
    return !!parent;
  };
  if (!mountStyle()) new MutationObserver((_, observer) => mountStyle() && observer.disconnect()).observe(document, { childList: true });

  // From Ligma, when the setting changes
  const applyState = (next) => {
    state = next;
    if (!state.on) exit();
    emit();
  };
  window.__ligmaRasterMode = (next) => {
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
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") sync();
    else save(target);
  });
  // The plugin API arrives once the file has loaded, and goes in files
  // that can't be edited
  let editable = false;
  setInterval(() => {
    const now = !!api();
    if (now === editable) return;
    editable = now;
    emit();
    if (!now) exit();
  }, 1000);
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
    ON.store(settings.show_raster_mode, Ordering::Relaxed);
    REV.store(now(), Ordering::Relaxed);
}

/// `PAGE_JS`, for a new Figma tab
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_RASTER_MODE__", &state())
}

/// Sends a changed setting to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    if ON.swap(settings.show_raster_mode, Ordering::Relaxed) == settings.show_raster_mode {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaRasterMode?.({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
