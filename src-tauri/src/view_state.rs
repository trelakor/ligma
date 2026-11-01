//! Keeps where you were in a file across reloads: its page, where the
//! canvas is and its zoom, and the toolbar's mode (Design, Draw, Motion or
//! Dev Mode). A sleeping tab's webview is closed outright and reloads from
//! its address, as do tabs reopened from the last session, and Figma only
//! goes by that address: the layer or page in its `node-id`, zoomed to fit
//!
//! `PAGE_JS` keeps them per file in `localStorage`, every second while the
//! tab is shown and as it's hidden, and puts them back once the file has
//! loaded again: in a tab waking from sleep or reopened from the last
//! session, one reloaded, and a file opened without a `node-id`. A link
//! to a layer still goes to that layer. Off with `keepFileView`
//!
//! The page and canvas go through Figma's console plugin API,
//! `window.figma`. The mode is read from and switched with the toolbar's
//! switch, by its test ids

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::settings::Settings;

/// Runs in Figma tabs. `__LIGMA_RETURNING__` is true in a tab waking from
/// sleep or reopened from the last session. `__LIGMA_KEEP_VIEW__` is
/// `{ on, rev }`: the setting when the tab was made. A page loaded later in
/// the same tab may find a newer one in `localStorage`; the higher `rev`
/// wins
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  const KEY = "ligma-view-state";
  const SETTING_KEY = "ligma-keep-view";
  // Files kept, the most recently seen
  const LIMIT = 200;
  const returning = __LIGMA_RETURNING__ || performance.getEntriesByType?.("navigation")[0]?.type === "reload";
  let setting = __LIGMA_KEEP_VIEW__;
  try {
    const stored = JSON.parse(localStorage.getItem(SETTING_KEY));
    if (stored?.rev > setting.rev) setting = stored;
  } catch {}

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
  // Until `check` has held three times in a row, 100ms apart, for at most
  // `limit` ms
  const until = async (check, limit) => {
    for (let count = 0, tries = 0; tries < limit / 100; tries++) {
      count = check() ? count + 1 : 0;
      if (count >= 3) return true;
      await wait(100);
    }
    return false;
  };
  // Until the file has been drawn, for at most a minute
  const loaded = () => until(ready, 60000);

  // The mode the toolbar's switch shows, or null without one (FigJam,
  // Slides)
  const mode = () =>
    document.querySelector('[data-testid="toolbelt-mode-segmented-control"][data-active-mode]')?.dataset.activeMode ??
    document.querySelector('[data-toolbelt-mode][class*="--selected--"]')?.dataset.toolbeltMode ??
    null;

  // The file this page keeps the state of, once it's been put back
  let key = null;
  let last = null;
  const save = () => {
    const figma = api();
    if (!setting.on || !key || key !== fileKey() || !figma) return;
    let state;
    try {
      const { x, y } = figma.viewport.center;
      state = { page: figma.currentPage.id, x, y, zoom: figma.viewport.zoom, mode: mode() };
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
      await until(() => mode() === state.mode, 3000);
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
    // Figma may still be zooming to the layer in the address
    for (let tries = 0; tries < 10; tries++) {
      const { x, y } = figma.viewport.center;
      if (Math.abs(figma.viewport.zoom - state.zoom) > 1e-6 || Math.hypot(x - state.x, y - state.y) > 1) place();
      await wait(100);
    }
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
    if (setting.on && state && (returning || !linked)) {
      try {
        await putBack(state);
      } catch {}
    }
    if (pending !== current) return;
    pending = null;
    if (fileKey() === current) key = current;
  };

  begin(first.key);
  // Figma can switch files without loading a page
  setInterval(() => {
    if (document.visibilityState !== "visible") return;
    const current = fileKey();
    if (current !== key && current !== pending) begin(current);
    else save();
  }, 1000);
  document.addEventListener("visibilitychange", () => document.visibilityState === "hidden" && save());
  addEventListener("pagehide", save);

  // From Ligma, when the setting changes
  window.__ligmaKeepView = (next) => {
    try {
      localStorage.setItem(SETTING_KEY, JSON.stringify(next));
    } catch {}
    setting = next;
  };
  // From another tab
  addEventListener("storage", (event) => {
    if (event.key !== SETTING_KEY) return;
    try {
      const stored = JSON.parse(event.newValue);
      if (stored?.rev > setting.rev) setting = stored;
    } catch {}
  });
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
    ON.store(settings.keep_file_view, Ordering::Relaxed);
    REV.store(now(), Ordering::Relaxed);
}

/// `PAGE_JS`, for a Figma tab. `returning` for one waking from sleep or
/// reopened from the last session
pub fn page_js(returning: bool) -> String {
    PAGE_JS
        .replace("__LIGMA_RETURNING__", if returning { "true" } else { "false" })
        .replace("__LIGMA_KEEP_VIEW__", &state())
}

/// Sends a changed setting to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    if ON.swap(settings.keep_file_view, Ordering::Relaxed) == settings.keep_file_view {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaKeepView?.({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
