//! Color scheme and color themes (Settings > Appearance)
//!
//! The color scheme (`colorScheme`: system, light or dark) is the app's
//! theme, which every webview reports to its pages as
//! `prefers-color-scheme`. Figma follows it once its own Theme setting is
//! System, which `PAGE_JS` makes it on every load; Figma keeps that
//! setting in the page's `localStorage` (`global-debug-theme-preference`)
//! and passes changes between its tabs as `storage` events
//!
//! A color theme (`lightTheme`, `darkTheme`, from `dist/themes.js`)
//! tints the grey `--ramp-*` primitives Figma builds its palette from and
//! points each editor's brand colors at the theme's accent, so every
//! panel, menu and button in Figma takes it, while the colors that stand
//! for a product (Design's blue, FigJam's purple) stay. Ligma's tab bar,
//! menus, prompts and settings window take their colors from the active
//! tab's (`THEME_JS` in tabs.rs), so they follow. Figma copies colors from
//! `<body>`'s computed style into the editor's canvas, for the selection,
//! rulers and labels it draws there; `PAGE_JS` hands it themed ones, with
//! the editor's own color too, but Figma's for new shapes' fills

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Manager, Theme, Webview};

use crate::settings::{ColorScheme, Settings};

const THEMES_JS: &str = include_str!("../../dist/themes.js");

/// Runs in Figma tabs. `__LIGMA_APPEARANCE__` is the `Page` the tab was made
/// with. A page loaded later in the same tab may find a newer one in
/// `localStorage`, from another tab; the higher `rev` wins
const PAGE_JS: &str = r##"(() => {
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
  // hears other tabs' changes; at load Figma hasn't read it yet
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
  // `--color-bg-menu-selected`), which then come from the accent instead
  // Only in the open editor: on <body>, which carries its theme, and on
  // theme elements of the same editor inside it (FigJam's toolbar). Figma
  // draws each product's icon and button (Home's file tiles and its Design,
  // FigJam, Slides buttons) in a theme element of that product, and those
  // keep their product's color. So do tokens naming a product or a state
  // (danger red)
  const KEEP = /design|figjam|handoff|component|assistive|measure|template|danger|warning|success|desktop|annotation|multiplayer|voting|inspect|timeline|code|prototype|published|-fs|fs[A-Z]/;
  // The canvas (see `getComputedStyle` below) takes the theme for what Figma
  // draws over your designs, selection and rulers included: also the open
  // editor's product color (Design's blue selection) and canvas tokens. The
  // fills Figma gives new shapes and frames keep Figma's colors, so nothing
  // themed ends up in a design
  const CANVAS = /design|figjam|handoff|-fs|fs[A-Z]/;
  const CANVAS_KEEP = /component|assistive|measure|template|danger|warning|success|annotation|multiplayer|voting|code|DefaultFill/;
  const DEFAULT_FILL = /DefaultFill/;
  const AREA = /selectedarea/i;
  // Figma's own color for `var(--ramp-<name>-<step>)`, where Ligma tints it
  const figmaColor = (name, step) =>
    name === "grey" ? LIGMA_GREY[LIGMA_STEPS.indexOf(Number(step))] : name === "white" && step === "1000" ? "#ffffff" : null;
  const brandRules = [];
  const canvasRules = [];
  const scanned = new WeakSet();
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
  const brandSelector = (selector) =>
    selectors(selector)
      .flatMap((part) => {
        const editor = /data-editor-theme="?([\w-]+)/.exec(part)?.[1];
        const inside = editor && `body[data-editor-theme="${editor}"]`;
        // `:where(<theme> <editor>)`: an editor's theme element inside a theme
        const nested = /^:where\((\S+) (.+)\)$/.exec(part);
        if (nested) return editor ? [`body${nested[1]}[data-editor-theme="${editor}"] ${nested[2]}`] : [];
        if (!part.startsWith("[")) return [];
        // And menus (see `markMenus`), which come in a theme element of their own
        const menu = `${part}[data-ligma-menu]`;
        return editor ? [`body${part}`, `${inside} ${part}`, menu] : [`body${part}`, menu];
      })
      .join(", ");

  // Figma draws each menu in a theme element of its own, dark in light mode
  // too, so it would take the dark theme's colors there. In light mode the
  // element becomes light; in both, it's marked for the theme's brand
  const MENU = '[role="menu"], [role="listbox"]';
  const markMenus = (root) => {
    const menus = root.matches(MENU) ? [root] : root.querySelectorAll(MENU);
    for (const menu of menus) {
      const scope = menu.closest("[data-preferred-theme]");
      if (!scope || scope === document.body || scope.childElementCount !== 1) continue;
      scope.setAttribute("data-ligma-menu", "");
      if (scope.getAttribute("data-preferred-theme") === "dark" && document.body.getAttribute("data-preferred-theme") === "light") {
        scope.setAttribute("data-preferred-theme", "light");
        scope.style.colorScheme = "light";
      }
    }
  };
  const menuObserver = new MutationObserver((records) => {
    for (const record of records) {
      for (const node of record.addedNodes) if (node.nodeType === Node.ELEMENT_NODE) markMenus(node);
    }
  });

  // Figma's menu and tooltip colors are dark in light mode too. There, each
  // one (`--color-bg-menu`, `--color-text-tooltip-secondary`, ...) takes the
  // matching color of the page instead, but for a highlighted item, which
  // keeps the brand color with white on it
  const POPUP = /-?(menu|tooltip)/;
  const POPUP_KEEP = /^--color-(bg-(menu|tooltip)-selected(-hover|-pressed)?|(text|icon)-(menu|tooltip)-onselected|border-(menu|tooltip)-(on)?selected(-strong)?)$/;
  // A light tooltip on a light panel needs an edge: the shadow of Figma's
  // other popovers, outline and all, in place of its dark tooltips' own
  const menuRules = ['[data-preferred-theme="light"] { --elevation-300-tooltip: var(--elevation-300) !important; }'];
  const scanMenus = (rule, selector) => {
    const decls = [];
    for (const name of rule.style) {
      if (!name.startsWith("--color-") || !POPUP.test(name) || POPUP_KEEP.test(name)) continue;
      const base = name.replace(POPUP, "");
      if (rule.style.getPropertyValue(base)) decls.push(`${name}: var(${base}) !important;`);
    }
    if (!decls.length) return false;
    menuRules.push(`${selector} { ${decls.join(" ")} }`);
    return true;
  };
  const scanRule = (rule) => {
    const selector = rule.selectorText;
    if (!selector?.includes("data-preferred-theme")) return false;
    const mode = /data-preferred-theme="?(light|dark)/.exec(selector)?.[1];
    const menus = mode === "light" && scanMenus(rule, selector);
    const [, paleBrand, brand] = /--ramp-(pale-)?([a-z-]+)-\d+/.exec(rule.style.getPropertyValue("--color-bg-menu-selected")) ?? [];
    if (!mode || !brand) return menus;
    const decls = [];
    const canvas = [];
    for (const name of rule.style) {
      if (!name.startsWith("--color-")) continue;
      const value = rule.style.getPropertyValue(name).trim();
      // The fill of a selection box (dragging across Home's files, or the
      // canvas) is the brand color, see-through, written out rather than
      // taken from the ramp, in long hex or short (Specs')
      const area = AREA.test(name) && /^#(?:[0-9a-f]{6}([0-9a-f]{2})|[0-9a-f]{3}([0-9a-f]))$/i.exec(value);
      if (area) {
        const alpha = Math.round((parseInt(area[1] ?? area[2].repeat(2), 16) / 255) * 100);
        decls.push(`${name}: color-mix(in srgb, var(--ligma-accent-500) ${alpha}%, transparent) !important;`);
        continue;
      }
      const ramp = /^var\(--ramp-(pale-)?([a-z-]+)-(\d+)\)$/.exec(value);
      if (!ramp) continue;
      // An editor branded in a pale ramp (Specs' slate blue) takes the
      // accent itself, else it would stay all but grey in every theme
      const pale = ramp[1] && !paleBrand;
      const accent = `${name}: var(--ligma-${pale ? "pale-" : ""}accent-${ramp[3]}) !important;`;
      if (DEFAULT_FILL.test(name)) {
        const figma = !ramp[1] && figmaColor(ramp[2], ramp[3]);
        if (figma) canvas.push(`${name}: ${figma} !important;`);
      } else if (ramp[2] !== brand) continue;
      else if (!KEEP.test(name)) decls.push(accent);
      else if (CANVAS.test(name) && !CANVAS_KEEP.test(name)) canvas.push(accent);
    }
    const parts = selectors(selector).filter((part) => part.startsWith("["));
    if (canvas.length && parts.length) {
      canvasRules.push({ mode, css: `${parts.map((part) => part + "[data-ligma-canvas]").join(", ")} { ${canvas.join(" ")} }` });
    }
    const brandSelectors = brandSelector(selector);
    if (!decls.length || !brandSelectors) return menus || canvas.length > 0;
    brandRules.push({ mode, css: `${brandSelectors} { ${decls.join(" ")} }` });
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
  // own inside them, then the brand tokens and the canvas's. Light menus
  // and tooltips come after, whatever the theme
  const css = (mode, id) => {
    const vars = Object.entries(ligmaPalette(id, mode)).map(([name, value]) => `${name}: ${value} !important;`);
    if (!vars.length) return "";
    const theme = `[data-preferred-theme="${mode}"]`;
    const own = (rules) => rules.filter((rule) => rule.mode === mode).map((rule) => rule.css);
    return [`${theme}, :where(${theme}) [data-editor-theme] { ${vars.join(" ")} }`, ...own(brandRules), ...own(canvasRules)].join("\n");
  };
  // At an interface scale that puts the panels' edges between device
  // pixels, the editor's canvas and the panels beside it round to them
  // differently, leaving a pixel-wide gap at a panel's edge. Behind it is
  // the canvas's own black background or, past it, the transparent page
  // and the webview's white or black. Both take the panels' color
  const SEAM = "body:has(.gpu-view-content), .gpu-view-content, .gpu-view-content > canvas { background-color: var(--color-bg) !important; }";
  // Home's Design, FigJam and Slides buttons draw their icons, white on the
  // product's color, in the menus' icon color, which light menus above
  // turn dark. They stay white, as in Figma
  const PRODUCT_ICONS = '[data-preferred-theme="light"] :is([data-testid="file-icon"], [data-testid="plus-icon"]) { --color-icon: #ffffff !important; }';
  const render = () => {
    if (current) style.textContent = [css("light", current.light), css("dark", current.dark), ...menuRules, SEAM, PRODUCT_ICONS].join("\n");
  };

  const apply = (page) => {
    if (!page || (current && page.rev <= current.rev)) return false;
    const changed = !!current;
    current = page;
    render();
    followScheme();
    if (changed) redrawCanvas();
    return true;
  };

  // Figma's canvas sync (`updateColorsInFullscreen`) reads every color from
  // <body>'s computed style. Give it a stand-in for <body> instead, with the
  // canvas's colors (`canvasRules`)
  let probe = null;
  // While `redrawCanvas` toggles it: <body>'s `data-enhanced-contrast`
  let contrast = null;
  const computedStyle = window.getComputedStyle;
  window.getComputedStyle = function (element, pseudo) {
    if (element === document.body && style.textContent && /updateColorsInFullscreen/.test(new Error().stack)) {
      probe ??= document.createElement("div");
      probe.hidden = true;
      probe.setAttribute("data-ligma-canvas", "");
      for (const name of ["data-preferred-theme", "data-editor-theme", "data-enhanced-contrast"]) {
        const real = name === "data-enhanced-contrast" && contrast != null;
        const value = real ? (contrast ? "" : null) : element.getAttribute(name);
        if (value == null) probe.removeAttribute(name);
        else probe.setAttribute(name, value);
      }
      if (probe.parentNode !== element) element.append(probe);
      return computedStyle.call(this, probe, pseudo);
    }
    return computedStyle.call(this, element, pseudo);
  };
  // Figma copies the colors into the canvas when <body>'s theme attributes
  // change, so it takes a new theme's once one is toggled and put back,
  // before anything is drawn. The copy keeps the attribute's real value
  const redrawCanvas = () => {
    const body = document.body;
    if (!body || !document.querySelector("canvas") || contrast != null) return;
    const name = "data-enhanced-contrast";
    contrast = body.hasAttribute(name);
    const toggle = (on) => (on ? body.setAttribute(name, "") : body.removeAttribute(name));
    toggle(!contrast);
    // After Figma's observer has seen it
    queueMicrotask(() => {
      toggle(contrast);
      setTimeout(() => {
        contrast = null;
        repaintCanvas();
      });
    });
  };
  // The canvas then has the new colors but draws its rulers again only when
  // something on it changes. A move of the pointer, where it already is,
  // makes it draw everything; a hidden tab gets it once shown
  let pointer = null;
  addEventListener("pointermove", (event) => event.isTrusted && (pointer = [event.clientX, event.clientY]), { capture: true, passive: true });
  let repaintWhenShown = false;
  const repaintCanvas = () => {
    const canvas = document.querySelector("canvas");
    repaintWhenShown = !!canvas && document.hidden;
    if (!canvas || document.hidden) return;
    const box = canvas.getBoundingClientRect();
    // Where the pointer is, if over the canvas; else the rulers' corner
    const over = pointer && pointer[0] >= box.left && pointer[0] < box.right && pointer[1] >= box.top && pointer[1] < box.bottom;
    const [x, y] = over ? pointer : [box.left + 1, box.top + 1];
    for (const type of ["pointermove", "mousemove"]) {
      const Event = type === "pointermove" ? PointerEvent : MouseEvent;
      canvas.dispatchEvent(new Event(type, { bubbles: true, clientX: x, clientY: y }));
    }
  };

  apply([__LIGMA_APPEARANCE__, stored()].sort((a, b) => (b?.rev ?? 0) - (a?.rev ?? 0))[0]);
  const mount = () => {
    if (style.isConnected) return true;
    const parent = document.head ?? document.documentElement;
    parent?.append(style);
    return !!parent;
  };
  if (!mount()) new MutationObserver((_, observer) => mount() && observer.disconnect()).observe(document, { childList: true });
  // Figma's stylesheets, as they arrive
  document.addEventListener("DOMContentLoaded", () => {
    scan();
    menuObserver.observe(document.body, { childList: true, subtree: true });
  });
  addEventListener("load", (event) => event.target instanceof HTMLLinkElement && scan(), true);
  addEventListener("load", scan);

  // From Ligma when the settings change, and at each page load
  Object.defineProperty(window, "__ligmaAppearance", {
    value: (page) => {
      if (!apply(page)) return;
      try {
        localStorage.setItem(KEY, JSON.stringify(page));
      } catch {}
    },
  });
  // A tab that was frozen meanwhile catches up when shown
  addEventListener("storage", (event) => event.key === KEY && apply(stored()));
  document.addEventListener("visibilitychange", () => {
    if (!apply(stored()) && repaintWhenShown) repaintCanvas();
  });
})();"##;

/// What Figma tabs get of the appearance settings
#[derive(Clone, PartialEq, Serialize)]
struct Page {
    /// When these settings took effect, in ms since 1970: Ligma's launch
    /// or the last change
    rev: u64,
    /// Not used by the page, but a change makes it set Figma's Theme
    /// setting to System again, should it have been changed in Figma
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

/// Applies the color scheme to the app. Call at launch, before any window
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

/// `PAGE_JS`, for a new Figma tab
pub fn page_js(app: &AppHandle) -> String {
    let page = serde_json::to_string(&page(app)).unwrap();
    PAGE_JS.replace("__LIGMA_THEMES__", THEMES_JS).replace("__LIGMA_APPEARANCE__", &page)
}

/// Sends the themes to a Figma tab's page
pub fn push_to(webview: &Webview) {
    let page = serde_json::to_string(&page(webview.app_handle())).unwrap();
    let _ = webview.eval(format!("window.__ligmaAppearance?.({page})"));
}

/// Applies changed appearance settings to the app and every loaded tab
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
    // once-a-second check
    let app = app.clone();
    std::thread::spawn(move || {
        for wait in [0, 40, 80, 120, 200, 300, 500] {
            std::thread::sleep(Duration::from_millis(wait));
            crate::tabs::sync_theme(&app);
        }
    });
}
