//! Patching Figma's modules as they load, for features that change how a
//! part of Figma behaves (layer previews, the classic home page)
//!
//! Figma's modules export through getters that can't be redefined once
//! loaded, so `PAGE_JS` wraps a module's factory when its chunk arrives,
//! found by what its source contains. That means reading every module's
//! source as it loads (about 60 ms over a file's load), so features only
//! ask for it in pages loaded with them on

/// Runs in Figma tabs, before the features' own scripts. Sets
/// `window.__ligmaModules`:
///
/// - `patch(patches)` wraps, from now on, the first module whose source
///   `patch.match(source)` accepts, for each patch. `patch.getter(value,
///   React)` replaces each of its exports, read as the module's getters
///   are; `patch.loaded(exports, React)` sees them once it has run. `React`
///   is what the module imports, or null
/// - `arrived(match)` says whether a module `match` accepts has already
///   loaded, so it's too late to patch it
pub const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window || window.__ligmaModules) return;
  const GLOBAL = "rspackChunk_figma_web_bundler";
  const patches = [];
  let hooked = false;
  let React = null;

  // React, from what a module imports: the module `x` whose `x.useState`
  // and the like it calls
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
  // getters
  const wrapGetter = (get, wrap) => {
    let from;
    let to;
    return () => {
      const value = get();
      if (value !== from) {
        from = value;
        to = wrap(value, React);
      }
      return to;
    };
  };

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
        patch.loaded?.(module.exports, React);
      } catch {}
      return result;
    };

  const patchChunk = (chunk) => {
    const modules = Array.isArray(chunk) ? chunk[1] : null;
    if (!modules || typeof modules !== "object" || !patches.some((patch) => !patch.done)) return;
    for (const id of Object.keys(modules)) {
      const factory = modules[id];
      if (typeof factory !== "function") continue;
      const source = Function.prototype.toString.call(factory);
      const patch = patches.find((patch) => !patch.done && patch.match(source));
      if (!patch) continue;
      patch.done = true;
      modules[id] = wrapFactory(factory, patch, source);
      if (!patches.some((patch) => !patch.done)) return;
    }
  };

  // Figma's chunks arrive through `push` on this array: Array's own until
  // the bundler's runtime loads and sets its own (if it hasn't already).
  // Each push goes through `patchChunk` first
  const hook = () => {
    hooked = true;
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

  window.__ligmaModules = {
    patch: (more) => {
      patches.push(...more);
      if (!hooked) hook();
    },
    arrived: (match) =>
      (window[GLOBAL] ?? []).some((chunk) => {
        const modules = Array.isArray(chunk) ? chunk[1] : null;
        return Object.values((modules && typeof modules === "object" && modules) || {}).some(
          (factory) => typeof factory === "function" && match(Function.prototype.toString.call(factory)),
        );
      }),
  };
})()"##;
