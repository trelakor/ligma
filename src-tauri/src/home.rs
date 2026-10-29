//! Classic home (Settings > Features > Home): Figma's file browser keeps
//! its long-standing layout for every account. Off by default
//!
//! Figma is trying a newer layout on some accounts (the `exp_fb_plan_scoped_ia`
//! experiment, as seen on 2026-10-25): the account menu names a plan
//! instead of you, the sidebar shows one plan's drafts and folders at a
//! time, and lists filter by plan. Everything in it asks one hook whether
//! the account is in it. `PAGE_JS` has that hook say no while on
//!
//! `PAGE_JS` wraps the hook's module as it loads (see `modules.rs`), so
//! only in pages loaded with the setting on. Turning it off applies right
//! away. Turning it on reloads the file browser, when its tab is shown;
//! open files pick it up when next loaded

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::settings::Settings;

/// Runs in Figma tabs. `__LIGMA_CLASSIC_HOME__` is `{ on, rev }`: the
/// setting when the tab was made. A page loaded later in the same tab may
/// find a newer one in `localStorage`; the higher `rev` wins
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window || !window.__ligmaModules) return;
  const KEY = "ligma-classic-home";
  let state = __LIGMA_CLASSIC_HOME__;
  try {
    const stored = JSON.parse(localStorage.getItem(KEY));
    if (stored?.rev > state.rev) state = stored;
  } catch {}
  const listeners = new Set();
  const subscribe = (listener) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  };
  const getOn = () => state.on;
  // The hook's module: `function o(){let{getConfig:e}=(0,a.I7)
  // ("exp_fb_plan_scoped_ia");return(0,n.useCallback)(()=>...||e().get(
  // "is_plan_scoped",!1),[e])}`. It hands out a function that says whether
  // the account has the newer layout
  const MODULE = (source) => source.includes('"exp_fb_plan_scoped_ia"') && source.includes('"is_plan_scoped"');
  const never = () => false;

  let patched = false;
  const install = () => {
    patched = true;
    window.__ligmaModules.patch([
      {
        match: MODULE,
        // Figma's hook is still called every time, so hooks keep their order
        getter: (value, React) => {
          if (typeof value !== "function" || !React || !String(value).includes('"is_plan_scoped"')) return value;
          return function () {
            const figma = value.apply(this, arguments);
            const on = React.useSyncExternalStore(subscribe, getOn);
            return on ? never : figma;
          };
        },
      },
    ]);
  };

  // The file browser, which shows the layout, as opposed to a file
  const isHome = () => location.pathname === "/files" || location.pathname.startsWith("/files/");

  // Turned on after the file browser loaded with the newer layout: reload
  // it once its tab is shown
  let reloading = false;
  const reloadWhenReady = () => {
    if (reloading || !state.on || !isHome() || document.visibilityState !== "visible") return;
    reloading = true;
    location.reload();
  };

  const apply = (next) => {
    const changed = next.on !== state.on;
    state = next;
    if (!changed) return;
    for (const listener of listeners) listener();
    if (!state.on || patched) return;
    if (window.__ligmaModules.arrived(MODULE)) reloadWhenReady();
    else install();
  };
  // From Ligma, when the setting changes
  window.__ligmaClassicHome = (next) => {
    try {
      localStorage.setItem(KEY, JSON.stringify(next));
    } catch {}
    apply(next);
  };
  // From another tab, or while this one was frozen and missed it
  const sync = () => {
    try {
      const stored = JSON.parse(localStorage.getItem(KEY));
      if (stored?.rev > state.rev) apply(stored);
    } catch {}
  };
  addEventListener("storage", (event) => event.key === KEY && sync());
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState !== "visible") return;
    sync();
    if (state.on && !patched && window.__ligmaModules.arrived(MODULE)) reloadWhenReady();
  });
  if (state.on) install();
})()"##;

/// The setting,
static ON: AtomicBool = AtomicBool::new(false);
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
    ON.store(settings.classic_home, Ordering::Relaxed);
    REV.store(now(), Ordering::Relaxed);
}

/// `PAGE_JS`, for a new Figma tab
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_CLASSIC_HOME__", &state())
}

/// Sends a changed setting to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    if ON.swap(settings.classic_home, Ordering::Relaxed) == settings.classic_home {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaClassicHome?.({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
