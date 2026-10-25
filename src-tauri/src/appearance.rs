//! Color scheme and color themes (Settings > Appearance).
//!
//! The color scheme (`colorScheme`: system, light or dark) is the app's
//! theme, which every webview reports to its pages as
//! `prefers-color-scheme`. Figma follows it once its own Theme setting is
//! System, which `PAGE_JS` makes it on every load; Figma keeps that
//! setting in the page's `localStorage` (`global-debug-theme-preference`)
//! and passes changes between its tabs as `storage` events.
//!
//! A color theme (`lightTheme`, `darkTheme`, from `dist/themes.js`)
//! tints the grey `--ramp-*` primitives Figma builds its palette from and
//! points each editor's brand colors at the theme's accent, so every
//! panel, menu and button in Figma takes it, while the colors that stand
//! for a product (Design's blue, FigJam's purple) stay. Ligma's tab bar, menus,
//! prompts and settings window take their colors from the active tab's
//! (`THEME_JS` in tabs.rs), so they follow. The canvas doesn't: Figma
//! copies its colors from `<body>`'s computed style into the editor's
//! renderer, and `PAGE_JS` hands that copy Figma's untinted colors.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Manager, Theme, Webview};

use crate::settings::{ColorScheme, Settings};

const THEMES_JS: &str = include_str!("../../dist/themes.js");

/// Runs in Figma tabs. `__LIGMA_APPEARANCE__` is the `Page` the tab was made
/// with. A page loaded later in the same tab may find a newer one in
/// `localStorage`, from another tab; the higher `rev` wins.
const PAGE_JS: &str = r#"(() => {
  if (!/(^|\.)figma\.com$/.test(location.hostname)) return;
  __LIGMA_THEMES__
  const KEY = "ligma-appearance";
  const FIGMA_THEME = "global-debug-theme-preference";
  const stored = () => {
    try {
      return JSON.parse(localStorage.getItem(KEY));
    } catch {
      return null;
    }
  };
  const style = document.createElement("style");
  style.id = "ligma-appearance";
  let current = null;

  // Figma's Theme setting to System, so Figma follows the color scheme
  // Ligma gives the page. The event tells this page's Figma, which only
  // hears other tabs' changes; at load Figma hasn't read it yet.
  const followScheme = () => {
    try {
      if (localStorage.getItem(FIGMA_THEME) === "system") return;
      localStorage.setItem(FIGMA_THEME, "system");
      const newValue = JSON.stringify(["system", Math.random()]);
      dispatchEvent(new StorageEvent("storage", { key: "ipc:on-theme-update-ipc-key", newValue, storageArea: localStorage }));
    } catch {}
  };

  // Each editor has its own brand color (Design and Home blue, FigJam
  // purple, Dev Mode green, ...), set by a rule for its
  // `[data-preferred-theme][data-editor-theme]`. Found in Figma's
  // stylesheets: the tokens such a rule takes from its brand ramp (that of
  // `--color-bg-menu-selected`), which then come from the accent instead.
  // Not on the theme element Figma wraps around a product's icon (Home's
  // file tiles, the editor switcher), its only child an <svg>, so icons
  // keep their product's color. So do tokens naming a product or a state
  // (danger red).
  const KEEP = /design|figjam|handoff|component|assistive|measure|template|danger|warning|success|desktop|annotation|multiplayer|voting|inspect|timeline|code|prototype|published|-fs|fs[A-Z]/;
  const brandRules = [];
  const scanned = new WeakSet();
  const UNTINTED = ":not([data-ligma-untinted])";
  const selectors = (selector) => {
    const parts = [];
    let depth = 0;
    let start = 0;
    for (let i = 0; i < selector.length; i++) {
      const c = selector[i];
      if (c === "(") depth++;
      else if (c === ")") depth--;
      else if (c === "," && !depth) {
        parts.push(selector.slice(start, i));
        start = i + 1;
      }
    }
    parts.push(selector.slice(start));
    return parts.map((part) => part.trim());
  };
  const NOT_ICON = ":not(:has(> svg:only-child))";
  const brandSelector = (selector) =>
    selectors(selector)
      .map((part) => part + UNTINTED + NOT_ICON)
      .join(", ");
  const scanRule = (rule) => {
    const selector = rule.selectorText;
    if (!selector?.includes("data-preferred-theme")) return false;
    const mode = /data-preferred-theme="?(light|dark)/.exec(selector)?.[1];
    const brand = /--ramp-(?:pale-)?([a-z-]+)-\d+/.exec(rule.style.getPropertyValue("--color-bg-menu-selected"))?.[1];
    if (!mode || !brand) return false;
    const decls = [];
    for (const name of rule.style) {
      if (!name.startsWith("--color-") || KEEP.test(name)) continue;
      const ramp = /^var\(--ramp-(pale-)?([a-z-]+)-(\d+)\)$/.exec(rule.style.getPropertyValue(name).trim());
      if (ramp?.[2] === brand) decls.push(`${name}: var(--ligma-${ramp[1] ? "pale-" : ""}accent-${ramp[3]}) !important;`);
    }
    if (!decls.length) return false;
    brandRules.push({ mode, css: `${brandSelector(selector)} { ${decls.join(" ")} }` });
    return true;
  };
  const scan = () => {
    let found = false;
    const walk = (rules) => {
      for (const rule of rules) {
        if (rule instanceof CSSStyleRule) found = scanRule(rule) || found;
        else if (rule.cssRules) walk(rule.cssRules);
      }
    };
    for (const sheet of document.styleSheets) {
      if (scanned.has(sheet)) continue;
      try {
        walk(sheet.cssRules);
        scanned.add(sheet);
      } catch {}
    }
    if (found) render();
  };

  // The greys and the accent ramps on Figma's theme elements and editors'
  // own inside them, then the brand tokens. Not on `[data-ligma-untinted]`.
  const css = (mode, id) => {
    const vars = Object.entries(ligmaPalette(id, mode)).map(([name, value]) => `${name}: ${value} !important;`);
    if (!vars.length) return "";
    const theme = `[data-preferred-theme="${mode}"]`;
    const brand = brandRules.filter((rule) => rule.mode === mode).map((rule) => rule.css);
    return [`${theme}${UNTINTED}, :where(${theme}) [data-editor-theme]${UNTINTED} { ${vars.join(" ")} }`, ...brand].join("\n");
  };
  const render = () => {
    if (current) style.textContent = css("light", current.light) + "\n" + css("dark", current.dark);
  };

  const apply = (page) => {
    if (!page || (current && page.rev <= current.rev)) return false;
    current = page;
    render();
    followScheme();
    return true;
  };

  // Figma's canvas sync (`updateColorsInFullscreen`) reads every color from
  // <body>'s computed style. Give it a stand-in for <body> with Figma's
  // own colors, so the canvas keeps them.
  let probe = null;
  const computedStyle = window.getComputedStyle;
  window.getComputedStyle = function (element, pseudo) {
    if (element === document.body && style.textContent && /updateColorsInFullscreen/.test(new Error().stack)) {
      probe ??= document.createElement("div");
      probe.hidden = true;
      probe.setAttribute("data-ligma-untinted", "");
      for (const name of ["data-preferred-theme", "data-editor-theme", "data-enhanced-contrast"]) {
        const value = element.getAttribute(name);
        if (value == null) probe.removeAttribute(name);
        else probe.setAttribute(name, value);
      }
      if (probe.parentNode !== element) element.append(probe);
      return computedStyle.call(this, probe, pseudo);
    }
    return computedStyle.call(this, element, pseudo);
  };

  apply([__LIGMA_APPEARANCE__, stored()].sort((a, b) => (b?.rev ?? 0) - (a?.rev ?? 0))[0]);
  const mount = () => {
    if (style.isConnected) return true;
    const parent = document.head ?? document.documentElement;
    parent?.append(style);
    return !!parent;
  };
  if (!mount()) new MutationObserver((_, observer) => mount() && observer.disconnect()).observe(document, { childList: true });
  // Figma's stylesheets, as they arrive.
  document.addEventListener("DOMContentLoaded", scan);
  addEventListener("load", (event) => event.target instanceof HTMLLinkElement && scan(), true);
  addEventListener("load", scan);

  // From Ligma when the settings change, and at each page load.
  Object.defineProperty(window, "__ligmaAppearance", {
    value: (page) => {
      if (!apply(page)) return;
      try {
        localStorage.setItem(KEY, JSON.stringify(page));
      } catch {}
    },
  });
  // A tab that was frozen meanwhile catches up when shown.
  addEventListener("storage", (event) => event.key === KEY && apply(stored()));
  document.addEventListener("visibilitychange", () => apply(stored()));
})();"#;

/// What Figma tabs get of the appearance settings.
#[derive(Clone, PartialEq, Serialize)]
struct Page {
    /// When these settings took effect, in ms since 1970: Ligma's launch
    /// or the last change.
    rev: u64,
    /// Not used by the page, but a change makes it set Figma's Theme
    /// setting to System again, should it have been changed in Figma.
    scheme: ColorScheme,
    light: String,
    dark: String,
}

pub struct Appearance(Mutex<Page>);

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_millis() as u64)
}

fn theme(scheme: ColorScheme) -> Option<Theme> {
    match scheme {
        ColorScheme::System => None,
        ColorScheme::Light => Some(Theme::Light),
        ColorScheme::Dark => Some(Theme::Dark),
    }
}

/// Applies the color scheme to the app. Call at launch, before any window.
pub fn init(app: &AppHandle, settings: &Settings) {
    app.manage(Appearance(Mutex::new(Page::new(settings))));
    app.set_theme(theme(settings.color_scheme));
}

impl Page {
    fn new(settings: &Settings) -> Self {
        Self {
            rev: now(),
            scheme: settings.color_scheme,
            light: settings.light_theme.clone(),
            dark: settings.dark_theme.clone(),
        }
    }
}

fn page(app: &AppHandle) -> Page {
    app.state::<Appearance>().0.lock().unwrap().clone()
}

/// `PAGE_JS`, for a new Figma tab.
pub fn page_js(app: &AppHandle) -> String {
    let page = serde_json::to_string(&page(app)).unwrap();
    PAGE_JS.replace("__LIGMA_THEMES__", THEMES_JS).replace("__LIGMA_APPEARANCE__", &page)
}

/// Sends the themes to a Figma tab's page.
pub fn push_to(webview: &Webview) {
    let page = serde_json::to_string(&page(webview.app_handle())).unwrap();
    let _ = webview.eval(format!("window.__ligmaAppearance?.({page})"));
}

/// Applies changed appearance settings to the app and every loaded tab.
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    app.set_theme(theme(settings.color_scheme));
    let state = app.state::<Appearance>();
    let changed = {
        let mut page = state.0.lock().unwrap();
        let new = Page::new(settings);
        let changed = Page { rev: page.rev, ..new.clone() } != *page;
        if changed {
            *page = new;
        }
        changed
    };
    if !changed {
        return;
    }
    for webview in crate::tabs::scriptable_tabs(app) {
        push_to(&webview);
    }
    // Ligma's own windows take the new colors from the active tab, which
    // has a theme right away but Figma redraws for a new color scheme a
    // moment later. Look again a few times rather than at the next
    // once-a-second check.
    let app = app.clone();
    std::thread::spawn(move || {
        for wait in [0, 40, 80, 120, 200, 300, 500] {
            std::thread::sleep(Duration::from_millis(wait));
            crate::tabs::sync_theme(&app);
        }
    });
}
