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
//! recolors the `--ramp-*` primitives Figma builds its palette from, so
//! every panel, menu and button in Figma takes it. Ligma's tab bar, menus,
//! prompts and settings window take their colors from the active tab's
//! (`THEME_JS` in tabs.rs), so they follow. The canvas doesn't: Figma
//! copies its colors from `<body>`'s computed style into the editor's
//! renderer, and `PAGE_JS` hands that copy Figma's untinted colors.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

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

  // Overrides Figma's ramps where it sets them: on its theme element and on
  // editors' own theme elements inside it. Not on `[data-ligma-untinted]`.
  const css = (mode, id) => {
    const vars = Object.entries(ligmaPalette(id, mode)).map(([name, value]) => `${name}: ${value} !important;`);
    if (!vars.length) return "";
    const theme = `[data-preferred-theme="${mode}"]`;
    return `${theme}:not([data-ligma-untinted]), :where(${theme}) [data-editor-theme]:not([data-ligma-untinted]) { ${vars.join(" ")} }`;
  };

  const apply = (page) => {
    if (!page || (current && page.rev <= current.rev)) return false;
    current = page;
    style.textContent = css("light", page.light) + "\n" + css("dark", page.dark);
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
    if changed {
        for webview in crate::tabs::scriptable_tabs(app) {
            push_to(&webview);
        }
    }
}
