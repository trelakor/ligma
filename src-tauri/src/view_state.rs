//! Keeps where you were in a file across reloads: its page, where the
//! canvas is and its zoom, the toolbar's mode (Design, Draw, Motion or Dev
//! Mode) and the tool. A sleeping tab's webview is closed outright and
//! reloads from its address, as do tabs reopened from the last session,
//! and Figma only goes by that address: the layer or page in its
//! `node-id`, zoomed to fit, with the Move tool
//!
//! `PAGE_JS` keeps them per file in `localStorage`, every second while the
//! tab is shown and as it's hidden, and puts them back once the file has
//! loaded again: in a tab waking from sleep or reopened from the last
//! session, one reloaded, and a file opened without a `node-id`. A link
//! to a layer still goes to that layer
//!
//! The page and canvas go through Figma's console plugin API,
//! `window.figma`. The mode is read from and switched with the toolbar's
//! switch, by its test ids. The tool is read from Figma's Redux store and
//! set with Figma's action for it, both found by text in Figma's modules,
//! as seen on 2026-11-01

/// Runs in Figma tabs. `__LIGMA_RETURNING__` is true in a tab waking from
/// sleep or reopened from the last session
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  const KEY = "ligma-view-state";
  // Files kept, the most recently seen
  const LIMIT = 200;
  const returning = __LIGMA_RETURNING__ || performance.getEntriesByType?.("navigation")[0]?.type === "reload";

  // The open file's key, or its branch's
  const fileKey = () => {
    const match = location.pathname.match(/^\/(?:design|file|board|slides|site|buzz)\/(\w+)(?:\/branch\/(\w+))?/);
    return match ? (match[2] ?? match[1]) : null;
  };
  // Whether the address the page loaded with pointed at a layer or page.
  // Figma adds a `node-id` once the file is open
  const first = { key: fileKey(), linked: new URLSearchParams(location.search).has("node-id") };

  const stored = () => {
    try {
      const all = JSON.parse(localStorage.getItem(KEY));
      return all && typeof all === "object" ? all : {};
    } catch {
      return {};
    }
  };

  const api = () => {
    const figma = window.figma;
    try {
      return figma?.currentPage && figma.viewport ? figma : null;
    } catch {
      return null;
    }
  };
  const ready = () => !!document.querySelector("canvas") && !document.querySelector('[class*="progress_bar--"]') && !!api();
  const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  // Until the file has been drawn twice in a row, for at most a minute
  const loaded = async () => {
    for (let count = 0, tries = 0; tries < 120; tries++) {
      count = ready() ? count + 1 : 0;
      if (count >= 2) return true;
      await wait(500);
    }
    return false;
  };

  // The mode the toolbar's switch shows, or null without one (FigJam,
  // Slides)
  const mode = () =>
    document.querySelector('[data-testid="toolbelt-mode-segmented-control"][data-active-mode]')?.dataset.activeMode ??
    document.querySelector('[data-toolbelt-mode][class*="--selected--"]')?.dataset.toolbeltMode ??
    null;

  // Figma's Redux store, its tools by number and the function its menus
  // and shortcuts run actions with: `function p(e,t){...n.get(e);if(i)
  // {...handler...}else n.reportUnregisteredTrigger(e);...}`. Looked for
  // once, in a file that has loaded
  let internals;
  const figmaInternals = () => {
    if (internals !== undefined) return internals;
    internals = null;
    let require = null;
    try {
      const chunks = globalThis.rspackChunk_figma_web_bundler ?? globalThis.webpackChunk_figma_web_bundler;
      chunks?.push([[Symbol("ligma")], {}, (r) => { require = r; }]);
    } catch {}
    if (!require?.m) return null;
    const exportsOf = (id) => {
      try {
        const exports = require(id);
        return exports ? Object.keys(exports).map((key) => exports[key]) : [];
      } catch {
        return [];
      }
    };
    let storeModule, run, tools;
    for (const id of Object.keys(require.m)) {
      const source = String(require.m[id]);
      if (!storeModule && source.includes("This API is intended only for debugging")) {
        try {
          storeModule = require(id);
        } catch {}
      }
      if (!run && source.includes("reportUnregisteredTrigger")) {
        // Not the class of Figma's action registry, also in modules that
        // call it
        run = exportsOf(id).find((value) => {
          const text = typeof value === "function" ? String(value) : "";
          return text.startsWith("function") && text.includes(".handler") && text.includes("reportUnregisteredTrigger");
        });
      }
      if (!tools && /\.SHAPE_RECTANGLE=\d+\]="SHAPE_RECTANGLE"/.test(source)) {
        tools = exportsOf(id).find((value) => value && typeof value === "object" && value[value.SHAPE_RECTANGLE] === "SHAPE_RECTANGLE");
      }
      if (storeModule && run && tools) break;
    }
    if (!storeModule || !run || !tools) return null;
    return (internals = { store: () => storeModule.globalReduxStore, run, tools });
  };

  // Figma's actions for the tools worth putting back, by Figma's names for
  // the tools. A file opens with the Move tool anyway, and the tools that
  // edit a vector need one being edited
  const ACTIONS = {
    HAND: "hand",
    COMMENTS: "comments",
    FRAME: "frame",
    SECTION: "section",
    SLICE: "slice",
    SHAPE_RECTANGLE: "rectangle",
    SHAPE_ELLIPSE: "ellipse",
    SHAPE_LINE: "line",
    SHAPE_ARROW: "arrow",
    SHAPE_STAR: "star",
    SHAPE_REGULAR_POLYGON: "regular-polygon",
    VECTOR_PEN: "pen",
    VECTOR_PENCIL: "pencil",
    BRUSH: "brush",
    TYPE: "type",
    SCALE: "scale",
    ANNOTATE: "annotate",
    MEASURE: "measure",
    STICKY: "sticky",
    STAMP: "stamp",
    HIGHLIGHTER: "highlighter",
    ERASER: "eraser",
    WASHI_TAPE: "washi-tape",
    LASSO: "lasso",
    MULTISELECT: "multiselect",
    TABLE: "table",
    CODE_BLOCK: "code-block",
    CONNECTOR_ELBOWED: "connector-elbowed",
    CONNECTOR_STRAIGHT: "connector-straight",
    CONNECTOR_CURVED: "connector-curved",
  };
  // The action for the tool in use, or null
  const tool = () => {
    const core = figmaInternals();
    try {
      const name = core?.tools[core.store()?.getState().mirror.appModel.currentTool];
      const action = ACTIONS[name] ?? (name?.startsWith("SHAPE_WHITEBOARD_") ? name.toLowerCase().replaceAll("_", "-") : null);
      return action ? `set-tool-${action}` : null;
    } catch {
      return null;
    }
  };

  // The file this page keeps the state of, once it's been put back
  let key = null;
  let last = null;
  const save = () => {
    const figma = api();
    if (!key || key !== fileKey() || !figma) return;
    let state;
    try {
      const { x, y } = figma.viewport.center;
      state = { page: figma.currentPage.id, x, y, zoom: figma.viewport.zoom, mode: mode(), tool: tool() };
    } catch {
      return;
    }
    if (![state.x, state.y, state.zoom].every(Number.isFinite)) return;
    const json = JSON.stringify(state);
    if (json === last) return;
    last = json;
    const all = stored();
    all[key] = { ...state, at: Date.now() };
    const keys = Object.keys(all);
    if (keys.length > LIMIT) {
      keys.sort((a, b) => (all[b].at ?? 0) - (all[a].at ?? 0)).slice(LIMIT).forEach((old) => delete all[old]);
    }
    try {
      localStorage.setItem(KEY, JSON.stringify(all));
    } catch {}
  };

  const putBack = async (state) => {
    const option = state.mode && mode() && state.mode !== mode() && document.querySelector(`[data-toolbelt-mode="${CSS.escape(state.mode)}"]`);
    if (option && !option.disabled && option.getAttribute("aria-disabled") !== "true") {
      option.click();
      await wait(1000);
      await loaded();
    }
    const figma = api();
    if (!figma) return;
    if (figma.currentPage.id !== state.page) {
      const page = figma.root.children.find((page) => page.id === state.page);
      if (page) await figma.setCurrentPageAsync(page);
    }
    const place = () => {
      figma.viewport.zoom = state.zoom;
      figma.viewport.center = { x: state.x, y: state.y };
    };
    place();
    // Figma may still be zooming to the layer in the address
    await wait(1000);
    const { x, y } = figma.viewport.center;
    if (Math.abs(figma.viewport.zoom - state.zoom) > 1e-6 || Math.hypot(x - state.x, y - state.y) > 1) place();
    if (state.tool && state.tool !== tool()) figmaInternals()?.run(state.tool);
  };

  // Puts back the state of file `current` once it has loaded, then keeps it
  let pending = null;
  const begin = async (current) => {
    key = null;
    last = null;
    pending = current;
    if (!current) return;
    if (!(await loaded())) {
      pending = null;
      return;
    }
    if (pending !== current || fileKey() !== current) return;
    const linked = current === first.key ? first.linked : new URLSearchParams(location.search).has("node-id");
    const state = stored()[current];
    if (state && (returning || !linked)) {
      try {
        await putBack(state);
      } catch {}
    }
    if (pending !== current) return;
    pending = null;
    if (fileKey() === current) key = current;
  };

  // Figma can switch files without loading a page
  setInterval(() => {
    if (document.visibilityState !== "visible") return;
    const current = fileKey();
    if (current !== key && current !== pending) begin(current);
    else save();
  }, 1000);
  document.addEventListener("visibilitychange", () => document.visibilityState === "hidden" && save());
  addEventListener("pagehide", save);
})()"##;

/// `PAGE_JS`, for a Figma tab. `returning` for one waking from sleep or
/// reopened from the last session
pub fn page_js(returning: bool) -> String {
    PAGE_JS.replace("__LIGMA_RETURNING__", if returning { "true" } else { "false" })
}
