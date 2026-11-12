//! Keeps where you were in a file: its page, where the canvas is and its
//! zoom, and the mode (Design, Draw, Dev Mode). Figma opens a file where
//! its address says, `node-id` for the page, `viewport` for the canvas
//! (`x,y,zoom`: where the canvas origin sits in the canvas area, in
//! pixels, and the zoom) and `m` for the mode, so that's where it's kept
//!
//! `PAGE_JS` writes them into the address as they change, so a tab that
//! sleeps, reloads or is reopened from the last session, which all load
//! from the address, opens right there, with nothing to put back after.
//! It also keeps them per file in `localStorage`, for a file opened
//! without saying where, as from Home: it puts them in the address before
//! Figma reads it. A link to a layer or another page still goes there.
//! Draw mode ignores `viewport`, so there the canvas is moved to it as
//! soon as Figma's API is up, still under its loading screen
//!
//! The page and canvas are read through Figma's console plugin API,
//! `window.figma`

/// Runs in Figma tabs, before Figma's own scripts
pub const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  const KEY = "ligma-view-state";
  // Files kept, the most recently seen
  const LIMIT = 200;
  // How often the address follows the canvas, in ms
  const EVERY = 300;

  // The open file's key, or its branch's. Only Design and FigJam take a
  // viewport from the address
  const fileKey = () => {
    const match = location.pathname.match(/^\/(?:design|file|board)\/(\w+)(?:\/branch\/(\w+))?/);
    return match ? (match[2] ?? match[1]) : null;
  };
  const stored = () => {
    try {
      const all = JSON.parse(localStorage.getItem(KEY));
      return all && typeof all === "object" ? all : {};
    } catch {
      return {};
    }
  };

  // Opened without saying where, or on the page it was left on but not
  // where on it: where it was left
  const opened = fileKey();
  const saved = opened && stored()[opened];
  if (typeof saved?.page === "string" && typeof saved.viewport === "string") {
    const url = new URL(location.href);
    const node = url.searchParams.get("node-id");
    if (!node || (node === saved.page && !url.searchParams.has("viewport"))) {
      url.searchParams.set("node-id", saved.page);
      url.searchParams.set("viewport", saved.viewport);
      if (saved.mode && !url.searchParams.has("m")) url.searchParams.set("m", saved.mode);
      history.replaceState(history.state, "", url);
    }
  }
  const wanted = opened && new URL(location.href).searchParams.get("viewport");

  const api = () => {
    const figma = window.figma;
    try {
      return figma?.currentPage && figma.viewport ? figma : null;
    } catch {
      return null;
    }
  };
  // Drawn, and no longer loading
  const ready = () => !!document.querySelector("canvas") && !document.querySelector('[class*="progress_bar--"]') && !!api();

  // Draw mode drops `viewport` from the address and fits the page in the
  // window instead. So the canvas goes where it asked as soon as Figma's
  // API is up, while its loading screen still covers the canvas, and is
  // held there until the file has been drawn for a moment
  if (wanted) {
    const [x, y, zoom] = wanted.split(",").map(Number);
    if ([x, y, zoom].every(Number.isFinite) && zoom > 0) {
      let drawnSince = 0;
      const started = Date.now();
      const timer = setInterval(() => {
        if (fileKey() !== opened || Date.now() - started > 60000) return clearInterval(timer);
        const figma = api();
        if (!figma) return;
        try {
          const bounds = figma.viewport.bounds;
          if (Math.abs(figma.viewport.zoom - zoom) > 1e-3 || Math.hypot(bounds.x + x / zoom, bounds.y + y / zoom) * zoom > 1) {
            figma.viewport.zoom = zoom;
            const { width, height } = figma.viewport.bounds;
            figma.viewport.center = { x: -x / zoom + width / 2, y: -y / zoom + height / 2 };
          }
        } catch {}
        if (!ready()) drawnSince = 0;
        else if (!drawnSince) drawnSince = Date.now();
        else if (Date.now() - drawnSince > 300) clearInterval(timer);
      }, 16);
    }
  }

  // The file the address follows, once it has been ready for a second:
  // until then Figma may still be placing the canvas
  let key = null;
  let readySince = 0;
  let last = null;
  const keep = () => {
    const current = fileKey();
    if (current !== key) {
      key = current;
      readySince = 0;
    }
    if (!current || !ready()) {
      readySince = 0;
      return;
    }
    readySince ||= Date.now();
    if (Date.now() - readySince < 1000) return;
    const figma = api();
    let page, viewport;
    try {
      const { x, y } = figma.viewport.bounds;
      const zoom = figma.viewport.zoom;
      if (![x, y, zoom].every(Number.isFinite) || zoom <= 0) return;
      page = figma.currentPage.id.replaceAll(":", "-");
      viewport = `${Math.round(-x * zoom)},${Math.round(-y * zoom)},${Number(zoom.toFixed(4))}`;
    } catch {
      return;
    }
    const url = new URL(location.href);
    const mode = url.searchParams.get("m");
    if (url.searchParams.get("node-id") !== page || url.searchParams.get("viewport") !== viewport) {
      url.searchParams.set("node-id", page);
      url.searchParams.set("viewport", viewport);
      history.replaceState(history.state, "", url);
    }
    const state = JSON.stringify([current, page, viewport, mode]);
    if (state === last) return;
    last = state;
    const all = stored();
    all[current] = { page, viewport, mode, at: Date.now() };
    const keys = Object.keys(all);
    if (keys.length > LIMIT) {
      keys.sort((a, b) => (all[b].at ?? 0) - (all[a].at ?? 0)).slice(LIMIT).forEach((old) => delete all[old]);
    }
    try {
      localStorage.setItem(KEY, JSON.stringify(all));
    } catch {}
  };

  setInterval(() => document.visibilityState === "visible" && keep(), EVERY);
  document.addEventListener("visibilitychange", () => document.visibilityState === "hidden" && keep());
  addEventListener("pagehide", keep);
})()"##;
