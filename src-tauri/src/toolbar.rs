//! Toolbar modes (Settings > Appearance > Toolbar): hides modes from the
//! switch on the right of Figma Design's toolbar (Draw, Design, Motion and
//! Dev Mode), and from the picker Figma's mode quick toggle shows. With one
//! left, the toolbar shows no switch. The mode you're in always shows, so
//! there's a way back from it
//!
//! Figma's switch is one component, handed the modes it shows, and the
//! picker lists the modes a jotai atom holds. `PAGE_JS` wraps both as their
//! module loads (see `modules.rs`) to leave out the hidden ones, so only in
//! pages loaded with a mode hidden. Changes apply right away in pages it
//! wrapped. Hiding one in a file already open otherwise reloads it, when
//! its tab is shown and its edits have been saved
//!
//! Accounts without Draw get Figma's older switch instead, a single Dev
//! Mode toggle. A stylesheet hides it, with no reload, when the mode it
//! goes to is hidden
//!
//! Figma's shortcuts and the Actions menu still switch to hidden modes:
//! they're handled where Ligma can't reach

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::settings::Settings;

/// Runs in Figma tabs. `__LIGMA_TOOLBAR_MODES__` is `{ hidden, rev }`: the
/// modes hidden when the tab was made, by Figma's names for them. A page
/// loaded later in the same tab may find a newer one in `localStorage`;
/// the higher `rev` wins
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window || !window.__ligmaModules) return;
  const KEY = "ligma-toolbar-modes";
  let state = __LIGMA_TOOLBAR_MODES__;
  try {
    const stored = JSON.parse(localStorage.getItem(KEY));
    if (stored?.rev > state.rev) state = stored;
  } catch {}
  const listeners = new Set();
  const subscribe = (listener) => {
    listeners.add(listener);
    return () => listeners.delete(listener);
  };
  // A string, so React sees the same value until it changes
  const getHidden = () => state.hidden.join(" ");
  // The switch's module, as seen on 2026-10-29: `function f(e){let{modes:
  // t,activeMode:n,onModeSwitch:l,...}=e;...}`, its only export. Each mode
  // is `{ mode, icon, label }`, `mode` one of "illustration" (Draw),
  // "design", "metronome" (Motion) and "handoff" (Dev Mode)
  const MODULE = (source) => source.includes('"toolbelt-mode-segmented-control"');
  // The quick toggle's module: atoms, among them the modes the picker
  // lists, most recently used first (so the one you're in first), derived
  // from the others: `(0,r.eU)(e=>{...return e(a).filter(e=>"metronome"===
  // e?t:...)})`
  const QUICK_TOGGLE = (source) => source.includes('"design","illustration","handoff"') && source.includes("visibleModeCount");

  let patched = false;
  const install = () => {
    patched = true;
    window.__ligmaModules.patch([
      {
        match: MODULE,
        getter: (Switch, React) => {
          if (typeof Switch !== "function" || !React || !String(Switch).includes("activeMode")) return Switch;
          const Modes = (props) => {
            const hidden = React.useSyncExternalStore(subscribe, getHidden).split(" ");
            const modes = Array.isArray(props.modes)
              ? props.modes.filter((item) => item.mode === props.activeMode || !hidden.includes(item.mode))
              : props.modes;
            if (modes === props.modes || modes.length === props.modes.length) return React.createElement(Switch, props);
            if (modes.length < 2) return null;
            return React.createElement(Switch, { ...props, modes });
          };
          Modes.displayName = "LigmaToolbarModes";
          return Modes;
        },
      },
      {
        match: QUICK_TOGGLE,
        loaded: (exports) => {
          const atoms = Object.keys(exports).map((key) => exports[key]);
          const modes = atoms.find((atom) => typeof atom?.read === "function" && String(atom.read).includes('"metronome"==='));
          if (!modes) return;
          // Its flags, the picker's open one among them, so it's worked out
          // again with the hidden modes as they are when it opens
          const flags = atoms.filter((atom) => atom !== modes && atom?.init === false);
          const read = modes.read;
          modes.read = function (get, options) {
            for (const flag of flags) get(flag);
            const figma = read.call(this, get, options);
            return Array.isArray(figma) ? figma.filter((mode, i) => i === 0 || !state.hidden.includes(mode)) : figma;
          };
        },
      },
    ]);
  };

  // Figma's older switch, by the shortcut its tooltip names: to Design
  // when in Dev Mode, to Dev Mode otherwise. The divider before it goes
  // with it
  const TOGGLE = '[class*="mode_switcher_toggle--toggleWrapper"]';
  const style = document.createElement("style");
  const restyle = () => {
    const toggles = [];
    if (state.hidden.includes("handoff")) toggles.push(`${TOGGLE}:not([data-tooltip-shortcut-key="enter-design-mode"])`);
    if (state.hidden.includes("design")) toggles.push(`${TOGGLE}[data-tooltip-shortcut-key="enter-design-mode"]`);
    const hidden = toggles.join(", ");
    style.textContent = hidden
      ? `:is(${hidden}), [class*="mode_toggle_switch_divider--hideIfLast"]:has(+ :is(${hidden})) { display: none !important; }`
      : "";
  };
  restyle();
  const mount = () => {
    if (style.isConnected) return true;
    const parent = document.head ?? document.documentElement;
    parent?.append(style);
    return !!parent;
  };
  if (!mount()) new MutationObserver((_, observer) => mount() && observer.disconnect()).observe(document, { childList: true });

  // Hid one after the switch loaded: reload, once the tab is shown and
  // Figma has no edits still saving
  let reloading = false;
  const reloadWhenReady = () => {
    if (reloading || !state.hidden.length) return;
    if (document.visibilityState !== "visible") return;
    if (window.__ligmaCanRest && !window.__ligmaCanRest()) {
      setTimeout(reloadWhenReady, 1000);
      return;
    }
    reloading = true;
    location.reload();
  };

  const apply = (next) => {
    const changed = next.hidden.join(" ") !== state.hidden.join(" ");
    state = next;
    if (!changed) return;
    restyle();
    for (const listener of listeners) listener();
    if (!state.hidden.length || patched) return;
    if (window.__ligmaModules.arrived(MODULE)) reloadWhenReady();
    else install();
  };
  // From Ligma, when the setting changes
  window.__ligmaToolbarModes = (next) => {
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
    if (state.hidden.length && !patched && window.__ligmaModules.arrived(MODULE)) reloadWhenReady();
  });
  if (state.hidden.length) install();
})()"##;

/// The modes hidden, by Figma's names for them,
static HIDDEN: Mutex<Vec<&str>> = Mutex::new(Vec::new());
/// and when they last changed, for `PAGE_JS` to tell them from older ones
/// in `localStorage`. Launch counts as a change
static REV: AtomicU64 = AtomicU64::new(0);

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_millis() as u64)
}

fn hidden(settings: &Settings) -> Vec<&'static str> {
    [
        (settings.show_draw_mode, "illustration"),
        (settings.show_design_mode, "design"),
        (settings.show_motion_mode, "metronome"),
        (settings.show_dev_mode, "handoff"),
    ]
    .into_iter()
    .filter_map(|(shown, mode)| (!shown).then_some(mode))
    .collect()
}

fn state() -> String {
    let hidden = serde_json::to_string(&*HIDDEN.lock().unwrap()).unwrap();
    let rev = REV.load(Ordering::Relaxed);
    format!(r#"{{"hidden":{hidden},"rev":{rev}}}"#)
}

/// Call at launch, before any tab
pub fn init(settings: &Settings) {
    *HIDDEN.lock().unwrap() = hidden(settings);
    REV.store(now(), Ordering::Relaxed);
}

/// `PAGE_JS`, for a new Figma tab
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_TOOLBAR_MODES__", &state())
}

/// Sends changed settings to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    let hidden = hidden(settings);
    if std::mem::replace(&mut *HIDDEN.lock().unwrap(), hidden.clone()) == hidden {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaToolbarModes?.({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
