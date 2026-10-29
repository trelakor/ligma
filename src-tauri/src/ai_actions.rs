//! AI actions in free projects (Settings > Appearance > Images): in files
//! from a free (Starter) project, the bar Figma shows above the toolbar
//! for a selected image keeps only Crop, and Ligma's Copy and Paste
//! adjustments (see adjustments.rs). Select area, Remove background, Edit
//! with prompt and the More menu, all AI actions, are hidden with their
//! divider. On by default
//!
//! A file's plan is `INITIAL_OPTIONS.plan_tier` ("starter" when free) while
//! the page is still on the file it loaded with, and otherwise the "Free"
//! badge Figma shows by the file's name. The bar is told by Crop's icon,
//! not by its text. Names as seen on 2026-10-29

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::settings::Settings;

/// Runs in Figma tabs. `__LIGMA_FREE_AI_ACTIONS__` is `{ on, rev }`: the
/// setting when the tab was made. A page loaded later in the same tab may
/// find a newer one in `localStorage`; the higher `rev` wins
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  const KEY = "ligma-free-ai-actions";
  let state = __LIGMA_FREE_AI_ACTIONS__;
  try {
    const stored = JSON.parse(localStorage.getItem(KEY));
    if (stored?.rev > state.rev) state = stored;
  } catch {}

  const BADGE = '[class*="paid_status_badge_base--badge"]';
  const fileKey = () => /^\/(?:design|file|board|slides|proto)\/([^/]+)/.exec(location.pathname)?.[1];
  const free = () => {
    const file = window.INITIAL_OPTIONS?.editing_file;
    if (file?.key && file.key === fileKey()) return window.INITIAL_OPTIONS.plan_tier === "starter";
    return /^free$/i.test(document.querySelector(BADGE)?.textContent.trim() ?? "");
  };

  // The image bar's Crop, by its icon. Everything else in the bar but
  // Ligma's buttons is an AI action, its divider or the More menu
  const BAR = '[data-secondary-toolbelt="true"] [class*="secondary_toolbelt--secondaryToolbeltContainer"]';
  const CROP = 'path[d^="M8.5 5a.5.5 0 0 1 .5.5V15h6V9"]';
  const HIDDEN = "data-ligma-ai-hidden";
  const tidy = () => {
    const hide = state.on && free();
    if (!hide) {
      for (const node of document.querySelectorAll(`[${HIDDEN}]`)) node.removeAttribute(HIDDEN);
      return;
    }
    for (const bar of document.querySelectorAll(BAR)) {
      const crop = bar.querySelector(CROP)?.closest("button");
      if (!crop || crop.parentElement !== bar) continue;
      for (const child of bar.children) {
        if (child === crop || child.matches("[data-ligma-adjustments-bar], [data-editor-theme]")) continue;
        if (!child.hasAttribute(HIDDEN)) child.setAttribute(HIDDEN, "");
      }
    }
  };

  const style = document.createElement("style");
  style.textContent = `[${HIDDEN}] { display: none !important; }`;
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
      tidy();
    });
  }).observe(document.documentElement, { childList: true, subtree: true });

  // From Ligma, when the setting changes
  window.__ligmaFreeAiActions = (next) => {
    try {
      localStorage.setItem(KEY, JSON.stringify(next));
    } catch {}
    state = next;
    tidy();
  };
  // From another tab, or while this one was frozen and missed it
  const sync = () => {
    try {
      const stored = JSON.parse(localStorage.getItem(KEY));
      if (stored?.rev > state.rev) {
        state = stored;
        tidy();
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
    ON.store(settings.hide_free_ai_actions, Ordering::Relaxed);
    REV.store(now(), Ordering::Relaxed);
}

/// `PAGE_JS`, for a new Figma tab
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_FREE_AI_ACTIONS__", &state())
}

/// Sends a changed setting to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    if ON.swap(settings.hide_free_ai_actions, Ordering::Relaxed) == settings.hide_free_ai_actions {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaFreeAiActions?.({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
