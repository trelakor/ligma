//! Image adjustments (Settings > Appearance > Images): copies the seven
//! sliders of an image fill (Exposure, Contrast, Saturation, Temperature,
//! Tint, Highlights and Shadows) and pastes them onto every image or video
//! fill in the selection, leaving the images themselves alone. Figma's
//! Copy/Paste Properties brings the whole fill, image and all
//!
//! With the image picker open, Cmd+Shift+C (Ctrl+Shift+C) copies its
//! adjustments, unless a text field has focus: Figma's Copy as PNG does
//! nothing then, and Cmd+C goes to Ligma's Edit menu before the page sees
//! it. Cmd+Shift+V
//! (Ctrl+Shift+V) pastes them while they're the last thing copied in
//! Ligma; once anything else is copied or cut, it's Figma's Paste over
//! Selection again. A button left of the picker's Rotate 90º, and
//! right-clicking its sliders or an image fill's row in the Fill section,
//! open a menu with Copy, Paste and Reset (see tab_menu.rs). Copy reads
//! the fill whose picker is open or row was clicked. `PAGE_JS` asks for
//! the menu by loading `SIGNAL_HOST`, which `on_signal` handles
//!
//! What's copied is kept in `localStorage`, so it carries to every tab.
//! Fills are read and written through Figma's console plugin API,
//! `window.figma`, so files that can't be edited are left alone, and each
//! paste or reset is one step for Figma's undo
//!
//! The picker, its buttons and the Fill section's rows are found by
//! Figma's test ids and class names as seen on 2026-10-29. The sliders
//! also by seven of them in one box, or by Figma's English labels

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Url};

use crate::settings::Settings;

/// Runs in Figma tabs. `__LIGMA_IMAGE_ADJUSTMENTS__` is `{ on, rev }`: the
/// setting when the tab was made. A page loaded later in the same tab may
/// find a newer one in `localStorage`; the higher `rev` wins
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  const KEY = "ligma-image-adjustments";
  let state = __LIGMA_IMAGE_ADJUSTMENTS__;
  try {
    const stored = JSON.parse(localStorage.getItem(KEY));
    if (stored?.rev > state.rev) state = stored;
  } catch {}
  const MAC = navigator.userAgent.includes("Mac");
  // Figma's names for the sliders, in the picker's order
  const FILTERS = ["exposure", "contrast", "saturation", "temperature", "tint", "highlights", "shadows"];
  const LABELS = ["Exposure", "Contrast", "Saturation", "Temperature", "Tint", "Highlights", "Shadows"];

  // What was copied, for every tab: `{ filters, armed }`, armed until
  // something else is copied, so Cmd+Shift+V pastes it
  const COPIED = "ligma-image-adjustments-copied";
  const copied = () => {
    try {
      return JSON.parse(localStorage.getItem(COPIED));
    } catch {
      return null;
    }
  };
  const keep = (value) => {
    try {
      localStorage.setItem(COPIED, JSON.stringify(value));
    } catch {}
  };
  const disarm = () => {
    const value = copied();
    if (value?.armed) keep({ ...value, armed: false });
  };

  // The plugin API, if the file can be edited
  const api = () => {
    const figma = window.figma;
    try {
      return figma?.currentPage && figma.editorType === "figma" ? figma : null;
    } catch {
      return null;
    }
  };
  const adjustable = (paint) => paint?.type === "IMAGE" || paint?.type === "VIDEO";
  // Text with mixed fills has none to adjust
  const fillsOf = (node) => {
    try {
      return Array.isArray(node.fills) ? node.fills : [];
    } catch {
      return [];
    }
  };
  const plural = (count) => `${count} image${count === 1 ? "" : "s"}`;

  // From the first selected layer with an image: its fill at `index`, the
  // one whose picker is open or row was right-clicked, or else its topmost
  // image fill shown. False with none
  const copy = (index) => {
    const figma = api();
    if (!figma) return false;
    for (const node of figma.currentPage.selection) {
      const fills = fillsOf(node);
      const paint =
        (adjustable(fills[index]) ? fills[index] : null) ??
        fills.findLast((paint) => adjustable(paint) && paint.visible !== false) ??
        fills.findLast(adjustable);
      if (!paint) continue;
      keep({ filters: Object.fromEntries(FILTERS.map((key) => [key, paint.filters?.[key] ?? 0])), armed: true });
      figma.notify("Adjustments copied");
      return true;
    }
    return false;
  };

  // Sets `filters` on every image and video fill of the selected layers,
  // as one undo step
  const adjust = (filters, done) => {
    const figma = api();
    if (!figma) return;
    let images = 0;
    let skipped = 0;
    for (const node of figma.currentPage.selection) {
      const fills = fillsOf(node);
      if (!fills.some(adjustable)) continue;
      try {
        node.fills = fills.map((paint) => (adjustable(paint) ? { ...paint, filters: { ...filters } } : paint));
        images++;
      } catch {
        skipped++;
      }
    }
    if (!images && !skipped) {
      figma.notify("No images selected");
      return;
    }
    try {
      figma.commitUndo();
    } catch {}
    figma.notify(`${done} ${plural(images)}${skipped ? `, ${skipped} skipped` : ""}`);
  };
  const paste = () => {
    const filters = copied()?.filters;
    if (filters) adjust(filters, "Adjustments pasted to");
  };
  const reset = () => adjust(Object.fromEntries(FILTERS.map((key) => [key, 0])), "Adjustments reset on");

  const visible = (node) => node.getClientRects().length > 0;
  // The nearest box around `start` that `holds`, short of the whole page
  const around = (start, holds) => {
    for (let node = start.parentElement; node && node !== document.body; node = node.parentElement) {
      if (holds(node)) return node.getBoundingClientRect().height <= innerHeight * 0.8 ? node : null;
    }
    return null;
  };
  // The image picker's sliders on screen: the smallest box around their
  // labels (`image_settings_modal--colorAdjustLabel`), around seven
  // sliders, or around the first and last of Figma's English labels
  const LABEL = '[class*="image_settings_modal--colorAdjustLabel"]';
  const SLIDER = '[role="slider"], input[type="range"]';
  const adjustments = () => {
    const label = [...document.querySelectorAll(LABEL)].find(visible);
    const box = label && around(label, (node) => node.querySelectorAll(LABEL).length >= FILTERS.length);
    if (box) return box;
    for (const slider of document.querySelectorAll(SLIDER)) {
      if (!visible(slider)) continue;
      const box = around(slider, (node) => node.querySelectorAll(SLIDER).length >= FILTERS.length);
      if (box) return box;
    }
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let text = walker.nextNode(); text; text = walker.nextNode()) {
      const label = text.parentElement;
      if (text.data.trim() !== LABELS[0] || !label || !visible(label)) continue;
      const box = around(label, (node) => node.textContent.includes(LABELS[LABELS.length - 1]));
      if (box) return box;
    }
    return null;
  };

  // The Fill section's rows, the one whose picker is open marked
  // `paintPanelValueSelected`. Each paint's index in the layer's fills is
  // the last number of its `data-onboarding-key`, `paint-panel-row-paint-1-1`
  const ROW = '[data-testid="paint-panel-color-value-container"]';
  const rowIndex = (row) => {
    const match = /-(\d+)$/.exec(row?.dataset.onboardingKey ?? "");
    return match ? Number(match[1]) : undefined;
  };
  const openIndex = () => rowIndex(document.querySelector(`${ROW}[class*="paintPanelValueSelected--"]`));

  // Asks Ligma for the menu (see `on_signal`): through `window.open` on
  // Windows, and on macOS as an iframe load, which WKWebView reports to
  // `on_navigation`. Copy then reads the fill at `index`
  let menuIndex;
  const openMenu = (index) => {
    menuIndex = index;
    const value = copied();
    const url = `https://adjustments.ligma.invalid/menu?${new URLSearchParams({ copied: value ? 1 : 0, armed: value?.armed ? 1 : 0 })}`;
    if (MAC) {
      const frame = document.createElement("iframe");
      frame.style.display = "none";
      frame.src = url;
      document.documentElement.appendChild(frame);
      setTimeout(() => frame.remove(), 1000);
    } else {
      window.open(url, "_blank");
    }
  };

  const editing = (node) =>
    node?.isContentEditable ||
    node?.tagName === "TEXTAREA" ||
    (node?.tagName === "INPUT" && !["range", "checkbox", "radio", "button"].includes(node.type));
  // The letter pressed, on any keyboard layout
  const letter = (event) => (/^[a-z]$/i.test(event.key) ? event.key.toLowerCase() : event.code.replace(/^Key/, "").toLowerCase());
  const stop = (event) => {
    event.preventDefault();
    event.stopImmediatePropagation();
  };

  addEventListener(
    "keydown",
    (event) => {
      const mod = MAC ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
      if (!mod || event.altKey) return;
      const key = letter(event);
      if (key === "c" && event.shiftKey) {
        if (state.on && !editing(document.activeElement) && adjustments() && copy(openIndex())) stop(event);
        else disarm();
      } else if ((key === "c" || key === "x") && !event.shiftKey) {
        disarm();
      } else if (key === "v" && event.shiftKey && state.on && copied()?.armed && api()) {
        stop(event);
        paste();
      }
    },
    true,
  );
  // Copied through a menu instead
  addEventListener("copy", disarm, true);
  addEventListener("cut", disarm, true);

  addEventListener(
    "contextmenu",
    (event) => {
      const figma = state.on && api();
      if (!figma) return;
      if (adjustments()?.contains(event.target)) {
        stop(event);
        openMenu(openIndex());
        return;
      }
      // An image or video fill's row
      const index = rowIndex(event.target.closest?.(ROW));
      if (index === undefined || !figma.currentPage.selection.some((node) => adjustable(fillsOf(node)[index]))) return;
      stop(event);
      openMenu(index);
    },
    true,
  );

  // A button left of the image picker's Rotate 90º, in its look, that opens
  // the menu
  const PICKER = '[data-testid="paint-picker-v2-modal"]';
  const BUTTONS = '[class*="image_settings_modal--settingsButtons"]';
  const STROKE = 'stroke="var(--fpl-icon-color, var(--color-icon))"';
  const ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><path ${STROKE} d="M6 9h6.5m3 0H18M6 15h2.5m3 0H18"/><circle ${STROKE} cx="14" cy="9" r="1.5"/><circle ${STROKE} cx="10" cy="15" r="1.5"/></svg>`;
  const addButton = () => {
    const buttons = document.querySelector(PICKER)?.querySelector(BUTTONS);
    const ours = document.querySelector("[data-ligma-adjustments]");
    if (ours && (!state.on || !buttons?.contains(ours))) ours.parentElement.remove();
    if (!state.on || !buttons || buttons.querySelector("[data-ligma-adjustments]")) return;
    const rotate = buttons.querySelector("button");
    if (!rotate) return;
    const button = Object.assign(document.createElement("button"), { type: "button", className: rotate.className });
    button.dataset.ligmaAdjustments = "";
    button.dataset.tooltip = "Adjustments";
    button.dataset.tooltipType = "text";
    button.setAttribute("aria-label", "Adjustments");
    const icon = rotate.querySelector("span")?.cloneNode(false) ?? document.createElement("span");
    icon.setAttribute("aria-hidden", "true");
    icon.innerHTML = ICON;
    button.append(icon);
    button.addEventListener("click", (event) => {
      stop(event);
      if (api()) openMenu(openIndex());
    });
    const wrap = document.createElement("span");
    wrap.append(button);
    buttons.prepend(wrap);
  };
  let queued = false;
  new MutationObserver(() => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      addButton();
    });
  }).observe(document.documentElement, { childList: true, subtree: true });

  // From Ligma: a pick from the menu, and the setting when it changes
  window.__ligmaImageAdjustments = {
    run: (action) => {
      if (!state.on) return;
      if (action === "copy" && !copy(menuIndex)) api()?.notify("No images selected");
      else if (action === "paste") paste();
      else if (action === "reset") reset();
    },
    set: (next) => {
      try {
        localStorage.setItem(KEY, JSON.stringify(next));
      } catch {}
      state = next;
      addButton();
    },
  };
  // From another tab, or while this one was frozen and missed it
  const sync = () => {
    try {
      const stored = JSON.parse(localStorage.getItem(KEY));
      if (stored?.rev > state.rev) {
        state = stored;
        addButton();
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
    ON.store(settings.adjustments_clipboard, Ordering::Relaxed);
    REV.store(now(), Ordering::Relaxed);
}

/// `PAGE_JS`, for a new Figma tab
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_IMAGE_ADJUSTMENTS__", &state())
}

/// Origin of the links `PAGE_JS` opens to ask for the menu
pub const SIGNAL_HOST: &str = "adjustments.ligma.invalid";

#[cfg(target_os = "macos")]
const SHORTCUTS: [(&str, &str); 2] = [("copy", "⇧⌘C"), ("paste", "⇧⌘V")];
#[cfg(not(target_os = "macos"))]
const SHORTCUTS: [(&str, &str); 2] = [("copy", "Ctrl+Shift+C"), ("paste", "Ctrl+Shift+V")];

/// The picker's button, or a right-click on its sliders or an image
/// fill's row, in tab `tab`: opens the menu, with Paste only once something is copied, and its shortcut only
/// while it's what Cmd+Shift+V pastes. Not from within the webview's own
/// callback, which is still deciding on the page
pub fn on_signal(app: &AppHandle, tab: u32, url: &Url) {
    if url.path() != "/menu" {
        return;
    }
    let flag = |name: &str| url.query_pairs().any(|(key, value)| key == name && value == "1");
    let (copied, armed) = (flag("copied"), flag("armed"));
    let app = app.clone();
    std::thread::spawn(move || {
        let items = vec![
            Some(("copy", "Copy Adjustments")),
            Some(("paste", "Paste Adjustments")),
            None,
            Some(("reset", "Reset Adjustments")),
        ];
        let extras = crate::tab_menu::Extras {
            disabled: if copied { vec![] } else { vec!["paste"] },
            shortcuts: SHORTCUTS.into_iter().filter(|&(action, _)| action == "copy" || armed).collect(),
            ..Default::default()
        };
        let _ = crate::tab_menu::show(&app, tab, items, extras, on_pick);
    });
}

/// Runs the item picked in the menu in the tab it was opened for
fn on_pick(app: &AppHandle, tab: u32, action: &str) {
    if let Some(webview) = crate::tabs::scriptable(app, tab) {
        let _ = webview.eval(format!("window.__ligmaImageAdjustments?.run({action:?})"));
    }
}

/// Sends a changed setting to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    if ON.swap(settings.adjustments_clipboard, Ordering::Relaxed) == settings.adjustments_clipboard {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaImageAdjustments?.set({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
