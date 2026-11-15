//! Ligma adjustments (Settings > Images > Adjustments): a stack of
//! Photoshop's and Camera Raw's adjustments for image fills and strokes,
//! which Figma's picker doesn't have, in a section of its own under its
//! seven sliders, marked as Ligma's. On by default
//!
//! The section lists only what's been added, from its + menu, grouped as
//! Photoshop's Adjustments menu: Levels and Curves; Presence (Texture,
//! Clarity, Dehaze) and Sharpen; Vibrance, Hue/Saturation, Color Balance,
//! Black & White, Photo Filter, Channel Mixer, Color Lookup and Selective
//! Color; Vignette and Invert. Each can be hidden or removed, and one at a
//! time shows its controls. The image is always made in that same order
//!
//! Figma can't run code on an image as it draws it, so the image is made
//! again with them, on the GPU, and put in the paint in place of the
//! original. The original is kept as a hidden paint right below it, so the
//! layer stays a plain image fill anyone can edit, with or without Ligma.
//! Which image each one was made from, and with what, is kept on the layer
//! (shared plugin data `ligma`/`color`), keyed by the made image's hash,
//! so the controls open where they were left and are always applied to the
//! original, never to an image already adjusted
//!
//! A change shows at once, drawn over the picker's thumbnail and, where
//! Ligma can draw the layer exactly as Figma would, over the canvas. The
//! file follows: while a control moves, an image 768 pixels across a few
//! times a second, so others in the file see it move, then the full size
//! one once it's let go. Pictures are made in a worker, with WebGL on an
//! OffscreenCanvas, so the page's thread stays free for Figma; where it
//! can't start, the page makes them. Each new image is loaded on a tiny
//! layer before it's swapped in, so the fill never shows empty. The hidden
//! original is put below once the picker closes, as adding a paint while
//! it's open would move the picker to another one
//!
//! Copy and paste adjustments (adjustments.rs) carry these too, through
//! `window.__ligmaColor`

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::AppHandle;

use crate::settings::Settings;

/// Runs in Figma tabs (color_adjustments.js). `__LIGMA_COLOR__` is `{ on,
/// rev }`: the setting when the tab was made. A page loaded later in the
/// same tab may find a newer one in `localStorage`; the higher `rev` wins
const PAGE_JS: &str = include_str!("color_adjustments.js");

/// The setting,
static ON: AtomicBool = AtomicBool::new(true);
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
    ON.store(settings.color_adjustments, Ordering::Relaxed);
    REV.store(now(), Ordering::Relaxed);
}

/// `PAGE_JS`, for a new Figma tab
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_COLOR__", &state())
}

/// Sends a changed setting to every loaded tab
pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    if ON.swap(settings.color_adjustments, Ordering::Relaxed) == settings.color_adjustments {
        return;
    }
    REV.store(now(), Ordering::Relaxed);
    let script = format!("window.__ligmaColor?.set({})", state());
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(&script);
    }
}
