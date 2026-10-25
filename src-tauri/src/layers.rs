//! Layer previews (Settings > Appearance > Layers): the layers panel shows
//! each layer's picture in every mode, as Figma's Draw mode does, not only
//! in Draw mode. Off by default: Figma renders every preview.
//!
//! Draw mode has no layers panel of its own. Figma's layers panel takes its
//! look from a React context, and Draw mode provides another value there:
//! taller rows with previews. Design mode can also use Figma's newer layers
//! panel, which has no previews; Figma picks it when the file is in Design
//! mode and a user flag or experiment says so, and in Design mode the
//! older panel builds its rows another way, which makes no previews.
//! `PAGE_JS` gives the panel Draw mode's rows, and keeps the older panel
//! building them as in Draw mode, in every mode. Tools and the right
//! sidebar stay the mode's own.
//!
//! Figma's modules export through getters that can't be redefined once
//! loaded, so `PAGE_JS` wraps the four modules' factories as their chunks
//! arrive. That means reading every module's source as it loads (about
//! 60 ms over a file's load), so it only does that in pages loaded with
//! the setting on. Turning it off applies right away. Turning it on in a
//! file already open reloads it, when its tab is shown and its edits have
//! been saved.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::settings::Settings;

/// Runs in Figma tabs. `__LIGMA_LAYER_PREVIEWS__` is `{ on, rev }`: the
/// setting when the tab was made. A page loaded later in the same tab may
/// find a newer one in `localStorage`; the higher `rev` wins.
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  const KEY = "ligma-layer-previews";
  let state = __LIGMA_LAYER_PREVIEWS__;
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
  // Figma's modules `install` wraps, as seen on 2026-10-25, by what their
  // source contains.
  const MODULES = {
    // Picks Figma's newer layers panel.
    choice: (source) => source.includes('"exp_layers_panel_v2"') && source.includes("fplTreeGridMigration"),
    // How the older layers panel builds its rows.
    rows: (source) => source.includes("orderedLayersPanelChildren") && source.includes('"0:-2"==='),
    // The layers panel's looks.
    looks: (source) => source.includes("showVisualLayerIcons:!0"),
    // The layers panel.
    panel: (source) => source.includes('label:"ObjectsPanel"'),
  };

  // Wraps them as they arrive (see the top).
  let patched = false;
  const install = () => {
    patched = true;
    let React = null;
    // Draw mode's layers panel look, and the context Figma passes it in.
    let looks = null;
    // Only the parts that make previews: Draw mode also focuses the layers
    // panel on the selection and doesn't bold names, which stay the mode's.
    const PREVIEW_KEYS = ["topLevelObjectRowHeight", "nestedObjectRowHeight", "rowSelectionGap", "insertionLineBoxOffset", "showVisualLayerIcons", "topLevelIconSize", "nestedIconSize"];

    // React, from what a module imports: the module `x` whose `x.useState`
    // and the like it calls.
    const findReact = (source, require) => {
      for (const [, name, id] of source.matchAll(/([\w$]+)=[\w$]+\((\d+)\)/g)) {
        const escaped = name.replace(/\$/g, "\\$");
        if (!new RegExp(`[^\\w$]${escaped}\\.use(State|Effect|Context|Memo|Callback|Ref)\\b`).test(source)) continue;
        try {
          const exports = require(id);
          if (exports?.useSyncExternalStore && exports.forwardRef && exports.createElement) return exports;
        } catch {}
      }
      return null;
    };

    // An export getter that hands out `wrap(value)` instead, made when the
    // value is first read: modules may export a `let` declared after the
    // getters.
    const wrapGetter = (get, wrap) => {
      let from;
      let to;
      return () => {
        const value = get();
        if (value !== from) {
          from = value;
          to = wrap(value);
        }
        return to;
      };
    };

    // A hook among a module's exports, `pattern` its source, that says false
    // instead while on. Figma's is called every time, so hooks keep their
    // order.
    const falseWhileOn = (pattern) => (value) => {
      if (typeof value !== "function" || !React || !pattern.test(String(value))) return value;
      return function () {
        const figma = value.apply(this, arguments);
        return React.useSyncExternalStore(subscribe, getOn) ? false : figma;
      };
    };

    // What to do with each of `MODULES`.
    const PATCHES = [
      {
        // Picks Figma's newer layers panel, which has no previews, in Design
        // mode: `function m(){return!0===x()}` among its exports.
        match: MODULES.choice,
        getter: falseWhileOn(/^function [\w$]*\(\)\{return!0===[\w$]+\(\)\}$/),
      },
      {
        // Has the older layers panel build its rows another way in Design
        // mode, which skips making previews: `function c(){let
        // e=(0,o.Em)(),t=(0,o.E3)()===r.n.Design;return e&&t}`.
        match: MODULES.rows,
        getter: falseWhileOn(/^function [\w$]*\(\)\{let [\w$]+=\(0,[\w$.]+\)\(\),[\w$]+=\(0,[\w$.]+\)\(\)===[\w$.]+\.Design;return [\w$]+&&[\w$]+\}$/),
      },
      {
        // The layers panel's looks: Design mode's, Draw mode's (with
        // `showVisualLayerIcons`) and the context they're passed in.
        match: MODULES.looks,
        loaded: (exports) => {
          const values = Object.keys(exports).map((key) => exports[key]);
          const context = values.find((value) => value?.Provider && value.$$typeof);
          const draw = values.find((value) => value?.showVisualLayerIcons === true);
          if (context && draw) looks = { context, draw };
        },
      },
      {
        // The layers panel: provides Draw mode's look while on.
        match: MODULES.panel,
        getter: (Panel) => {
          if (!Panel?.$$typeof || !React || !looks) return Panel;
          const { context, draw } = looks;
          const Previews = React.forwardRef((props, ref) => {
            const outer = React.useContext(context);
            const on = React.useSyncExternalStore(subscribe, getOn);
            const value = React.useMemo(() => {
              if (!on || !outer) return outer;
              const value = { ...outer };
              for (const key of PREVIEW_KEYS) if (key in draw) value[key] = draw[key];
              return value;
            }, [on, outer]);
            return React.createElement(context.Provider, { value }, React.createElement(Panel, { ...props, ref }));
          });
          Previews.displayName = "LigmaLayerPreviews";
          return Previews;
        },
      },
    ];
    let remaining = PATCHES.length;

    const wrapFactory = (factory, patch, source) =>
      function (module, exports, require) {
        React ??= findReact(source, require);
        const proxied = patch.getter
          ? new Proxy(require, {
              get: (target, key) =>
                key === "d"
                  ? (exports, getters, values) => {
                      const wrapped = {};
                      for (const name of Object.keys(getters ?? {})) wrapped[name] = wrapGetter(getters[name], patch.getter);
                      return target.d(exports, wrapped, values);
                    }
                  : target[key],
            })
          : require;
        const result = factory.call(this, module, exports, proxied);
        try {
          patch.loaded?.(module.exports);
        } catch {}
        return result;
      };

    const patchChunk = (chunk) => {
      const modules = Array.isArray(chunk) ? chunk[1] : null;
      if (!remaining || !modules || typeof modules !== "object") return;
      for (const id of Object.keys(modules)) {
        const factory = modules[id];
        if (typeof factory !== "function") continue;
        const source = Function.prototype.toString.call(factory);
        const patch = PATCHES.find((patch) => !patch.done && patch.match(source));
        if (!patch) continue;
        patch.done = true;
        remaining--;
        modules[id] = wrapFactory(factory, patch, source);
        if (!remaining) return;
      }
    };

    // Figma's chunks arrive through `push` on this array: Array's own until
    // the bundler's runtime loads and sets its own (if it hasn't already).
    // Each push goes through `patchChunk` first.
    const GLOBAL = "rspackChunk_figma_web_bundler";
    const chunks = (window[GLOBAL] ??= []);
    let push = chunks.push;
    Object.defineProperty(chunks, "push", {
      configurable: true,
      get: () => {
        const target = push;
        return function (...items) {
          for (const item of items) patchChunk(item);
          return target.apply(this, items);
        };
      },
      set: (value) => {
        push = value;
      },
    });
  };

  // Some of the modules `install` wraps have arrived, so it's too late to
  // wrap them all.
  const figmaLoaded = () =>
    (window.rspackChunk_figma_web_bundler ?? []).some((chunk) => {
      const modules = Array.isArray(chunk) ? chunk[1] : null;
      return Object.values((modules && typeof modules === "object" && modules) || {}).some((factory) => {
        if (typeof factory !== "function") return false;
        const source = Function.prototype.toString.call(factory);
        return Object.values(MODULES).some((match) => match(source));
      });
    });

  // Turned on after the panel loaded without previews: reload, once the
  // tab is shown and Figma has no edits still saving.
  let reloading = false;
  const reloadWhenReady = () => {
    if (reloading || !state.on) return;
    if (document.visibilityState !== "visible") return;
    if (window.__ligmaCanRest && !window.__ligmaCanRest()) {
      setTimeout(reloadWhenReady, 1000);
      return;
    }
    reloading = true;
    location.reload();
  };

  const apply = (next) => {
    const changed = next.on !== state.on;
    state = next;
    if (!changed) return;
    for (const listener of listeners) listener();
    if (!state.on || patched) return;
    if (figmaLoaded()) reloadWhenReady();
    else install();
  };
  // From Ligma, when the setting changes.
  window.__ligmaLayerPreviews = (next) => {
    try {
      localStorage.setItem(KEY, JSON.stringify(next));
    } catch {}
    apply(next);
  };
  // From another tab, or while this one was frozen and missed it.
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
    if (state.on && !patched && figmaLoaded()) reloadWhenReady();
  });
  if (state.on) install();
})()"##;

/// The setting,
static ON: AtomicBool = AtomicBool::new(false);
/// and when it last changed, for `PAGE_JS` to tell it from an older one in
/// `localStorage`. Launch counts as a change.
static REV: AtomicU64 = AtomicU64::new(0);

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_millis() as u64)
}

fn state() -> String {
    let (on, rev) = (ON.load(Ordering::Relaxed), REV.load(Ordering::Relaxed));
    format!(r#"{{"on":{on},"rev":{rev}}}"#)
}

/// Call at launch, before any tab.
pub fn init(settings: &Settings) {
    ON.store(settings.layer_previews, Ordering::Relaxed);
    REV.store(now(), Ordering::Relaxed);
}

/// `PAGE_JS`, for a new Figma tab.
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_LAYER_PREVIEWS__", &state())
}

/// Sends a changed setting to every loaded tab.
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    if ON.swap(settings.layer_previews, Ordering::Relaxed) == settings.layer_previews {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaLayerPreviews?.({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
