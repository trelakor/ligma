//! Image adjustments (Settings > Features > Images): copies the seven
//! sliders of an image fill or stroke (Exposure, Contrast, Saturation,
//! Temperature, Tint, Highlights and Shadows) and pastes them onto the
//! images of the selected layers, leaving the images themselves
//! alone. Figma's Copy/Paste Properties brings the whole fill, image and
//! all. Video fills have no sliders
//!
//! With the image picker open, Cmd+Shift+C (Ctrl+Shift+C) copies its
//! adjustments, unless a text field has focus: Figma's Copy as PNG does
//! nothing then, and Cmd+C goes to Ligma's Edit menu before the page sees
//! it. Cmd+Shift+V (Ctrl+Shift+V) pastes them while they're the last thing
//! copied in Ligma, or while an image's row in the Fill or Stroke section
//! is highlighted; otherwise it's Figma's Paste over Selection. Paste and
//! Reset change one image in each selected layer: the one whose picker is
//! open or row is highlighted or was clicked, else its top image fill
//!
//! Copy adjustments and Paste adjustments also show after Crop in the bar
//! above the toolbar for a selected image, Paste once something is copied.
//! A button left of the picker's Rotate 90º, and right-clicking its
//! sliders or an image's row, open a menu with Copy, Paste and Reset (see
//! tab_menu.rs). Copy reads the paint whose picker is open or row was
//! clicked. `PAGE_JS` asks for the menu by loading `SIGNAL_HOST`, which
//! `on_signal` handles
//!
//! What's copied is kept in `localStorage`, so it carries to every tab.
//! Paints are read and written through Figma's console plugin API,
//! `window.figma`, so files that can't be edited are left alone, and each
//! paste or reset is one step for Figma's undo
//!
//! The picker, its buttons and the Fill and Stroke sections' rows are found
//! by Figma's test ids and class names as seen on 2026-10-29. The sliders
//! also by seven of them in one box, or by Figma's English labels

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Url};

use crate::settings::Settings;

/// Runs in Figma tabs. `__LIGMA_IMAGE_ADJUSTMENTS__` is `{ on, bar, rev }`:
/// the settings when the tab was made. A page loaded later in the same tab may
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
    window.__ligmaImageAdjustments?.decorate();
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
  // Video fills have no sliders
  const adjustable = (paint) => paint?.type === "IMAGE";
  // Text with mixed fills has none to adjust
  const paintsOf = (node, list) => {
    try {
      return Array.isArray(node[list]) ? node[list] : [];
    } catch {
      return [];
    }
  };
  const plural = (count) => `${count} image${count === 1 ? "" : "s"}`;
  // One toast at a time, so quick copies and pastes don't queue behind
  // each other
  let toast;
  const notify = (figma, message) => {
    try {
      toast?.cancel();
    } catch {}
    toast = figma.notify(message, { timeout: 2000 });
  };

  // From the first selected layer with an image: the paint `at`, `{ list,
  // index }`, whose picker is open or row was clicked, or else its topmost
  // image fill shown. False with none
  const copy = (at) => {
    const figma = api();
    if (!figma) return false;
    for (const node of figma.currentPage.selection) {
      const fills = paintsOf(node, "fills");
      const chosen = at && paintsOf(node, at.list)[at.index];
      const paint =
        (adjustable(chosen) ? chosen : null) ??
        fills.findLast((paint) => adjustable(paint) && paint.visible !== false) ??
        fills.findLast(adjustable);
      if (!paint) continue;
      keep({ filters: Object.fromEntries(FILTERS.map((key) => [key, paint.filters?.[key] ?? 0])), armed: true });
      notify(figma, "Adjustments copied");
      return true;
    }
    return false;
  };

  // Sets `filters` on one image of each selected layer, as one undo step:
  // the paint `at`, `{ list, index }`, whose picker is open or row is
  // highlighted or was clicked, or else, and in layers without it, the
  // layer's topmost image fill shown
  const adjust = (filters, done, at) => {
    const figma = api();
    if (!figma) return;
    let images = 0;
    let skipped = 0;
    for (const node of figma.currentPage.selection) {
      let list = "fills";
      let index;
      if (at && adjustable(paintsOf(node, at.list)[at.index])) ({ list, index } = at);
      else {
        const fills = paintsOf(node, "fills");
        index = fills.findLastIndex((paint) => adjustable(paint) && paint.visible !== false);
        if (index < 0) index = fills.findLastIndex(adjustable);
        if (index < 0) continue;
      }
      try {
        node[list] = paintsOf(node, list).map((paint, i) => (i === index ? { ...paint, filters: { ...filters } } : paint));
        images++;
      } catch {
        skipped++;
      }
    }
    if (!images && !skipped) {
      notify(figma, "No images selected");
      return;
    }
    try {
      figma.commitUndo();
    } catch {}
    notify(figma, `${done} ${plural(images)}${skipped ? `, ${skipped} skipped` : ""}`);
  };
  const paste = (at) => {
    const filters = copied()?.filters;
    if (filters) adjust(filters, "Adjustments pasted to", at);
  };
  const reset = (at) => adjust(Object.fromEntries(FILTERS.map((key) => [key, 0])), "Adjustments reset on", at);

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

  // The Fill and Stroke sections' rows: the one whose picker is open marked
  // `paintPanelValueSelected`, a highlighted one in a row that's
  // `rowSelected` or `aria-selected`, or has focus.
  // Each names its section and paint in its `data-onboarding-key`,
  // `paint-panel-row-paint-<section>-<index>`. Fill comes first on the
  // page, so its number is the first row's
  const ROW = '[data-testid="paint-panel-color-value-container"]';
  const KEY_PATTERN = /-(\d+)-(\d+)$/;
  const rowPaint = (row) => {
    const match = KEY_PATTERN.exec(row?.dataset.onboardingKey ?? "");
    if (!match) return null;
    const first = KEY_PATTERN.exec(document.querySelector(ROW)?.dataset.onboardingKey ?? "");
    const at = { list: first && first[1] !== match[1] ? "strokes" : "fills", index: Number(match[2]) };
    // A layer with strokes but no fills: Stroke comes first
    const nodes = api()?.currentPage.selection ?? [];
    const other = at.list === "fills" ? "strokes" : "fills";
    const has = (list) => nodes.some((node) => adjustable(paintsOf(node, list)[at.index]));
    if (!has(at.list) && has(other)) at.list = other;
    return at;
  };
  const isImage = (at) => !!at && (api()?.currentPage.selection ?? []).some((node) => adjustable(paintsOf(node, at.list)[at.index]));
  const openPaint = () => rowPaint(document.querySelector(`${ROW}[class*="paintPanelValueSelected--"]`));
  const highlighted = () => {
    const row = [...document.querySelectorAll(ROW)].find((row) => {
      const line = row.closest('[role="row"]') ?? row;
      return /rowSelected--/.test(line.className) || line.getAttribute("aria-selected") === "true" || line.contains(document.activeElement);
    });
    return rowPaint(row);
  };

  // Asks Ligma for the menu (see `on_signal`): through `window.open` on
  // Windows, and on macOS as an iframe load, which WKWebView reports to
  // `on_navigation`. Copy then reads the fill at `index`
  let menuPaint;
  const openMenu = (at) => {
    menuPaint = at;
    const value = copied();
    const params = { copied: value ? 1 : 0, armed: value?.armed ? 1 : 0, open: adjustments() ? 1 : 0 };
    const url = `https://adjustments.ligma.invalid/menu?${new URLSearchParams(params)}`;
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
        if (state.on && !editing(document.activeElement) && adjustments() && copy(openPaint())) stop(event);
        else disarm();
      } else if ((key === "c" || key === "x") && !event.shiftKey) {
        disarm();
      } else if (key === "v" && event.shiftKey && state.on && api() && copied()) {
        const at = openPaint() ?? highlighted();
        if (!copied().armed && !isImage(at)) return;
        stop(event);
        paste(isImage(at) ? at : null);
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
        openMenu(openPaint());
        return;
      }
      // An image's row
      const at = rowPaint(event.target.closest?.(ROW));
      if (!isImage(at)) return;
      stop(event);
      openMenu(at);
    },
    true,
  );

  // A button left of the image picker's Rotate 90º, in its look, that opens
  // the menu. Not for videos, which share the picker but have no sliders
  const PICKER = '[data-testid="paint-picker-v2-modal"]';
  const BUTTONS = '[class*="image_settings_modal--settingsButtons"]';
  const STROKE = 'stroke="var(--fpl-icon-color, var(--color-icon))"';
  const ICON = `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><path ${STROKE} d="M6 9h6.5m3 0H18M6 15h2.5m3 0H18"/><circle ${STROKE} cx="14" cy="9" r="1.5"/><circle ${STROKE} cx="10" cy="15" r="1.5"/></svg>`;
  const addButton = () => {
    const picker = document.querySelector(PICKER);
    const buttons = picker?.querySelector('fieldset[value="IMAGE"]') && picker.querySelector(BUTTONS);
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
      if (api()) openMenu(openPaint());
    });
    const wrap = document.createElement("span");
    wrap.append(button);
    buttons.prepend(wrap);
  };

  // Copy adjustments and Paste adjustments after Crop in the bar Figma shows
  // above the toolbar for a selected image, in its buttons' look. Paste
  // only once something is copied. Both read and change the image whose
  // row is highlighted in the Fill or Stroke section, or else each layer's
  // top image fill. Only with the setting for them, `bar`
  const BAR = '[data-secondary-toolbelt="true"] [class*="secondary_toolbelt--secondaryToolbeltContainer"]';
  const BAR_ICONS = {
    copy: `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><rect ${STROKE} x="9.5" y="9.5" width="9" height="9" rx="1.5"/><path ${STROKE} d="M14.5 9.5V7A1.5 1.5 0 0 0 13 5.5H7A1.5 1.5 0 0 0 5.5 7v6A1.5 1.5 0 0 0 7 14.5h2.5"/></svg>`,
    paste: `<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><path ${STROKE} d="M9.5 6.5H8A1.5 1.5 0 0 0 6.5 8v10A1.5 1.5 0 0 0 8 19.5h8a1.5 1.5 0 0 0 1.5-1.5V8A1.5 1.5 0 0 0 16 6.5h-1.5"/><rect ${STROKE} x="9.5" y="5" width="5" height="3" rx="1"/></svg>`,
  };
  const BAR_LABELS = { copy: "Copy adjustments", paste: "Paste adjustments" };
  const addBarButtons = () => {
    const bar = document.querySelector(BAR);
    for (const ours of document.querySelectorAll("[data-ligma-adjustments-bar]")) {
      if (ours.parentElement !== bar) ours.remove();
    }
    if (!bar) return;
    const figma = state.on && state.bar !== false && api();
    const image = !!figma && figma.currentPage.selection.some((node) => paintsOf(node, "fills").some(adjustable));
    const show = { copy: image, paste: image && !!copied() };
    const crop = bar.querySelector("button:not([data-ligma-adjustments-bar])");
    let after = crop;
    for (const action of ["copy", "paste"]) {
      let button = bar.querySelector(`[data-ligma-adjustments-bar="${action}"]`);
      if (!show[action] || !crop) {
        button?.remove();
        continue;
      }
      if (!button) {
        button = Object.assign(document.createElement("button"), { type: "button", className: crop.className });
        button.dataset.ligmaAdjustmentsBar = action;
        const label = crop.querySelector("span")?.cloneNode(false) ?? document.createElement("span");
        label.textContent = BAR_LABELS[action];
        button.innerHTML = BAR_ICONS[action];
        button.append(label);
        // The row highlighted before the click takes focus from it
        let at = null;
        button.addEventListener("pointerdown", () => {
          const row = openPaint() ?? highlighted();
          at = isImage(row) ? row : null;
        });
        button.addEventListener("click", (event) => {
          stop(event);
          if (action === "copy") copy(at);
          else paste(at);
          at = null;
        });
      }
      if (after.nextElementSibling !== button) after.after(button);
      after = button;
    }
  };
  const decorate = () => {
    addButton();
    addBarButtons();
  };

  // `document`, not its <html>: WebView2 runs this before there is one
  let queued = false;
  new MutationObserver(() => {
    if (queued) return;
    queued = true;
    requestAnimationFrame(() => {
      queued = false;
      decorate();
    });
  }).observe(document, { childList: true, subtree: true });

  // From Ligma: a pick from the menu, and the setting when it changes
  window.__ligmaImageAdjustments = {
    decorate: () => decorate(),
    run: (action) => {
      if (!state.on) return;
      const figma = api();
      if (action === "copy" && !copy(menuPaint) && figma) notify(figma, "No images selected");
      else if (action === "paste") paste(menuPaint);
      else if (action === "reset") reset(menuPaint);
    },
    set: (next) => {
      try {
        localStorage.setItem(KEY, JSON.stringify(next));
      } catch {}
      state = next;
      decorate();
    },
  };
  // From another tab, or while this one was frozen and missed it
  const sync = () => {
    try {
      const stored = JSON.parse(localStorage.getItem(KEY));
      if (stored?.rev > state.rev) {
        state = stored;
        decorate();
      }
    } catch {}
  };
  addEventListener("storage", (event) => {
    if (event.key === KEY) sync();
    // Copied in another tab
    if (event.key === COPIED) decorate();
  });
  document.addEventListener("visibilitychange", () => document.visibilityState === "visible" && sync());
})()"##;

/// The settings: copy and paste,
static ON: AtomicBool = AtomicBool::new(true);
/// the image bar's buttons,
static BAR: AtomicBool = AtomicBool::new(true);
/// and when they last changed, for `PAGE_JS` to tell it from older ones in
/// `localStorage`. Launch counts as a change
static REV: AtomicU64 = AtomicU64::new(0);

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_millis() as u64)
}

fn state() -> String {
    let (on, bar) = (ON.load(Ordering::Relaxed), BAR.load(Ordering::Relaxed));
    format!(r#"{{"on":{on},"bar":{bar},"rev":{}}}"#, REV.load(Ordering::Relaxed))
}

/// Call at launch, before any tab
pub fn init(settings: &Settings) {
    ON.store(settings.adjustments_clipboard, Ordering::Relaxed);
    BAR.store(settings.adjustments_bar, Ordering::Relaxed);
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
/// fill's row, in tab `tab`: opens the menu, with Paste only once
/// something is copied. Each shortcut shows only while it does what the
/// item does: Copy's while the picker is open, Paste's while it's what
/// Cmd+Shift+V pastes. Not from within the webview's own callback, which
/// is still deciding on the page
pub fn on_signal(app: &AppHandle, tab: u32, url: &Url) {
    if url.path() != "/menu" {
        return;
    }
    let flag = |name: &str| url.query_pairs().any(|(key, value)| key == name && value == "1");
    let (copied, armed, open) = (flag("copied"), flag("armed"), flag("open"));
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
            shortcuts: SHORTCUTS.into_iter().filter(|&(action, _)| if action == "copy" { open } else { armed }).collect(),
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

/// Sends changed settings to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    let on = ON.swap(settings.adjustments_clipboard, Ordering::Relaxed) != settings.adjustments_clipboard;
    let bar = BAR.swap(settings.adjustments_bar, Ordering::Relaxed) != settings.adjustments_bar;
    if !on && !bar {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaImageAdjustments?.set({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
