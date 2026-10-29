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
//! Hiding the mode a file is in switches it to the first one shown, in the
//! switch's order, or the next if Figma doesn't take it. Only then: Figma
//! puts some accounts in a mode on its own, and would put them back
//!
//! Right-clicking the switch opens a menu of the four modes (see
//! tab_menu.rs), checked when shown. `PAGE_JS` asks for it by loading
//! `SIGNAL_HOST`, which `on_signal` handles, and picking one turns it on
//! or off as in Settings
//!
//! Figma's shortcuts and the Actions menu still switch to hidden modes:
//! they're handled where Ligma can't reach

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Url};

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
  // The switch on screen: its modes, the one the file is in and how it
  // switches, for leaving a mode as it's hidden
  let shown = null;
  // Modes hidden while this page couldn't leave them, for the page it
  // reloads into to leave
  const LEAVE_KEY = "ligma-toolbar-leave";
  let leaving = [];
  try {
    leaving = JSON.parse(sessionStorage.getItem(LEAVE_KEY)) ?? [];
    sessionStorage.removeItem(LEAVE_KEY);
  } catch {}
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
            shown = props;
            React.useEffect(() => () => {
              if (shown === props) shown = null;
            });
            // Once, when the page reloaded for a hidden mode has drawn the
            // file: Figma doesn't take a switch before
            React.useEffect(() => {
              const left = leaving;
              leaving = [];
              if (!left.length) return;
              const ready = () => document.querySelector("canvas") && !document.querySelector('[class*="progress_bar--"]');
              const wait = () => (ready() ? leave(left) : setTimeout(wait, 500));
              wait();
            }, []);
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
  const TO_DESIGN = '[data-tooltip-shortcut-key="enter-design-mode"]';

  // Switches out of the mode the file is in if it's one of `hidden`, to
  // the first one shown. Figma doesn't always take a switch (Motion from
  // Design wants something to animate), so if the file is still in a
  // hidden mode a while later, to the one after it, and so on
  const leave = (hidden, tried = 0) => {
    if (shown) {
      const { modes, activeMode, onModeSwitch } = shown;
      if (!hidden.includes(activeMode) || !Array.isArray(modes) || typeof onModeSwitch !== "function") return;
      const next = modes.filter((item) => !item.disabled && !state.hidden.includes(item.mode))[tried];
      if (!next) return;
      onModeSwitch(next.mode);
      setTimeout(() => leave(hidden, tried + 1), 3000);
      return;
    }
    if (tried) return;
    // The older switch has only the other mode to go to
    const toggle = document.querySelector(TOGGLE);
    if (!toggle) return;
    const inDev = toggle.matches(TO_DESIGN);
    const [from, to] = inDev ? ["handoff", "design"] : ["design", "handoff"];
    if (hidden.includes(from) && !state.hidden.includes(to)) toggle.querySelector('button, [role="switch"], input')?.click();
  };

  // Right-clicking the switch asks Ligma for its menu (see `on_signal`):
  // through `window.open` on Windows, and on macOS as an iframe load,
  // which WKWebView reports to `on_navigation`
  const SWITCH = `[class*="toolbelt_mode_segmented_control--container"], [data-testid="toolbelt-mode-segmented-control"], ${TOGGLE}`;
  addEventListener(
    "contextmenu",
    (event) => {
      if (!event.target.closest?.(SWITCH)) return;
      event.preventDefault();
      event.stopPropagation();
      const url = "https://toolbar.ligma.invalid/menu";
      if (navigator.userAgent.includes("Mac")) {
        const frame = document.createElement("iframe");
        frame.style.display = "none";
        frame.src = url;
        document.documentElement.appendChild(frame);
        setTimeout(() => frame.remove(), 1000);
      } else {
        window.open(url, "_blank");
      }
    },
    true,
  );
  const style = document.createElement("style");
  const restyle = () => {
    const toggles = [];
    if (state.hidden.includes("handoff")) toggles.push(`${TOGGLE}:not(${TO_DESIGN})`);
    if (state.hidden.includes("design")) toggles.push(`${TOGGLE}${TO_DESIGN}`);
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
    try {
      sessionStorage.setItem(LEAVE_KEY, JSON.stringify(leaving));
    } catch {}
    location.reload();
  };

  const apply = (next) => {
    const changed = next.hidden.join(" ") !== state.hidden.join(" ");
    const newly = next.hidden.filter((mode) => !state.hidden.includes(mode));
    state = next;
    if (!changed) return;
    restyle();
    for (const listener of listeners) listener();
    if (newly.length) {
      if (patched || !window.__ligmaModules.arrived(MODULE)) leave(newly);
      else leaving = [...new Set([...leaving, ...newly])].filter((mode) => state.hidden.includes(mode));
    }
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

/// Origin of the links `PAGE_JS` opens to ask for the menu
pub const SIGNAL_HOST: &str = "toolbar.ligma.invalid";

/// The menu's items, in the switch's order: their actions and text
const MODES: [(&str, &str); 4] = [("draw", "Draw"), ("design", "Design"), ("motion", "Motion"), ("dev", "Dev Mode")];

/// The setting that shows mode `action` (see `MODES`)
fn shows<'a>(settings: &'a mut Settings, action: &str) -> Option<&'a mut bool> {
    match action {
        "draw" => Some(&mut settings.show_draw_mode),
        "design" => Some(&mut settings.show_design_mode),
        "motion" => Some(&mut settings.show_motion_mode),
        "dev" => Some(&mut settings.show_dev_mode),
        _ => None,
    }
}

/// A right-click on the switch in tab `tab`: opens the menu of modes. Not
/// from within the webview's own callback, which is still deciding on the
/// page
pub fn on_signal(app: &AppHandle, tab: u32, url: &Url) {
    if url.path() != "/menu" {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let mut settings = crate::settings::current(&app);
        let items = MODES.iter().map(|&item| Some(item)).collect();
        let checked = MODES
            .iter()
            .map(|&(action, _)| action)
            .filter(|action| shows(&mut settings, action).is_some_and(|shown| *shown))
            .collect();
        let _ = crate::tab_menu::show(&app, tab, items, crate::tab_menu::Extras { checked: Some(checked), ..Default::default() }, on_pick);
    });
}

/// Turns the mode picked in the menu on or off
fn on_pick(app: &AppHandle, _tab: u32, action: &str) {
    let mut settings = crate::settings::current(app);
    let Some(shown) = shows(&mut settings, action) else {
        return;
    };
    *shown = !*shown;
    if let Err(error) = crate::settings::change(app, settings.clone()) {
        log::error!("Couldn't save settings: {error}");
        return;
    }
    crate::settings::show_in_window(app, &settings);
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
