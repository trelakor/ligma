//! Experimental native renderer (`nativeRenderer`, off by default, Windows
//! only for now): Ligma draws the open file's current page itself, with
//! Skia on D3D12, in a window of its own that follows Figma's viewport. Figma's web app still does everything else. It's a spike to
//! find out whether drawing the canvas natively uses less GPU memory and
//! draws faster than Figma's own renderer, and how close it gets
//!
//! - `page.js` reads the page's scene through the plugin API Figma's editor
//!   offers on the page, `window.figma`, streaming it in nearest the view
//!   first, and its changes, in bulk strings read back in slices, as `eval`
//!   can't wait or carry much at once
//! - `scene.rs` mirrors it: transforms, outlines (`fillGeometry`,
//!   `strokeGeometry`, glyph outlines for text), paints, effects, masks
//! - `draw.rs` draws it with Skia, effects included. What it only gets
//!   close to (shader effects, for one) is counted as approximate
//! - `adjust.rs` applies image adjustments (exposure, contrast and so on)
//! - `render.rs` draws the view in tiles kept on the GPU, 16 samples a pixel
//! - `gpu_memory.rs` reads Windows' GPU memory counters for the overlay
//! - `run.rs` ties it together, and holds the parity harness and benchmark
//!
//! The overlay (`dist/native.html`) shows frame time, GPU memory and what's
//! drawn natively. With `LIGMA_NATIVE_DEV=1` it also runs the parity harness
//! and benchmark, and `LIGMA_NATIVE_AUTORUN=1` runs both once the page has
//! loaded and writes the results to Ligma's log folder.
//! `LIGMA_NATIVE_RENDERER=1` turns it on whatever the setting

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use tauri::{AppHandle, Webview};

use crate::settings::Settings;

#[cfg(windows)]
mod adjust;
#[cfg(windows)]
mod draw;
#[cfg(windows)]
mod gpu_memory;
#[cfg(windows)]
mod render;
#[cfg(windows)]
mod run;
#[cfg(windows)]
mod scene;

/// The native renderer's window, and the overlay webview in it
pub const WINDOW: &str = "native-renderer";
pub const OVERLAY: &str = "native-overlay";

static ON: AtomicBool = AtomicBool::new(false);

fn enabled(settings: &Settings) -> bool {
    settings.native_renderer || std::env::var("LIGMA_NATIVE_RENDERER").is_ok_and(|value| value == "1")
}

/// Whether the parity harness and benchmark are offered
pub fn dev() -> bool {
    std::env::var("LIGMA_NATIVE_DEV").is_ok_and(|value| value == "1") || autorun()
}

pub fn autorun() -> bool {
    std::env::var("LIGMA_NATIVE_AUTORUN").is_ok_and(|value| value == "1")
}

/// Call once the main window is up
pub fn init(app: &AppHandle, settings: &Settings) {
    if enabled(settings) {
        ON.store(true, Ordering::Relaxed);
        start(app);
    }
}

pub fn apply_settings(app: &AppHandle, settings: &Settings) {
    let on = enabled(settings);
    if ON.swap(on, Ordering::Relaxed) == on {
        return;
    }
    if on {
        start(app);
    } else {
        stop(app);
    }
}

#[cfg(windows)]
fn start(app: &AppHandle) {
    run::start(app);
}

#[cfg(windows)]
fn stop(app: &AppHandle) {
    run::stop(app);
}

#[cfg(not(windows))]
fn start(_app: &AppHandle) {
    log::info!("The native renderer is Windows only for now");
}

#[cfg(not(windows))]
fn stop(_app: &AppHandle) {}

/// For the overlay
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub status: String,
    pub adapter: String,
    pub dev: bool,
    pub page: String,
    /// Nodes in the mirrored scene
    pub nodes: usize,
    /// In the last frame: nodes drawn, those of them only drawn close to
    /// Figma's look, and those left out as too small to see
    pub native: u32,
    pub approximate: u32,
    pub skipped: u32,
    /// Why the first approximate node in view is
    pub approximate_reason: Option<String>,
    /// Last frame's encoding, and its encoding and rendering until the GPU
    /// finished, in ms
    pub encode_ms: f64,
    pub frame_ms: f64,
    /// Over the last 120 frames
    pub frame_p50: f64,
    pub frame_p95: f64,
    pub frames: u64,
    /// Parts of the view still being encoded
    pub pending: bool,
    pub zoom: f64,
    pub images: usize,
    pub image_bytes: usize,
    /// Tiles of the view kept on the GPU, and Skia's own GPU cache
    pub tiles: usize,
    pub tile_bytes: usize,
    pub cache_bytes: usize,
    /// Time to draw a tile, mean over the last ones, in ms: recording its
    /// pictures, playing them back, and the GPU's work
    pub tile_record_ms: f64,
    pub tile_play_ms: f64,
    pub tile_gpu_ms: f64,
    pub tiles_drawn: u64,
    /// Dedicated GPU memory of Ligma's own process (the native renderer)
    /// and of WebView2's (Figma), in bytes
    pub vram_ligma: u64,
    pub vram_webview: u64,
    /// The renderer's own use of the GPU's memory, as D3D12 counts it
    pub vram_device: u64,
    /// Reading the page: until its first part was in, in the page, moving
    /// it over, and parsing it
    pub first_batch_ms: f64,
    pub snapshot_ms: f64,
    pub transfer_ms: f64,
    pub parse_ms: f64,
    pub snapshot_bytes: usize,
    /// Where the last parity or benchmark report went
    pub report: Option<String>,
    #[serde(skip)]
    pub recent: std::collections::VecDeque<f64>,
    /// Frame times kept while a benchmark runs
    #[serde(skip)]
    pub log: Option<Vec<f64>>,
}

impl Stats {
    #[allow(clippy::too_many_arguments)]
    pub fn record_frame(&mut self, encode_ms: f64, frame_ms: f64, native: u32, approximate: u32, skipped: u32, zoom: f64, pending: bool) {
        self.encode_ms = encode_ms;
        self.frame_ms = frame_ms;
        self.native = native;
        self.approximate = approximate;
        self.skipped = skipped;
        self.zoom = zoom;
        self.pending = pending;
        self.frames += 1;
        self.recent.push_back(frame_ms);
        if self.recent.len() > 120 {
            self.recent.pop_front();
        }
        let mut sorted: Vec<f64> = self.recent.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        self.frame_p50 = percentile(&sorted, 0.5);
        self.frame_p95 = percentile(&sorted, 0.95);
        if let Some(log) = &mut self.log {
            log.push(frame_ms);
        }
    }
}

/// `p` (0 to 1) of `sorted`
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

pub fn stats() -> &'static Mutex<Stats> {
    static STATS: OnceLock<Mutex<Stats>> = OnceLock::new();
    STATS.get_or_init(|| Mutex::new(Stats { dev: dev(), ..Default::default() }))
}

fn from_overlay(webview: &Webview) -> bool {
    webview.label() == OVERLAY
}

#[tauri::command]
pub async fn get_native_stats(webview: Webview) -> Option<Stats> {
    from_overlay(&webview).then(|| stats().lock().unwrap().clone())
}

/// An overlay button: "tint" or "untint" the nodes only drawn close to
/// Figma's look, read the page again ("resync"), or with `LIGMA_NATIVE_DEV`,
/// run the "parity" harness or the "bench"mark, or "compare:<id>" one node
#[tauri::command]
pub async fn native_action(app: AppHandle, webview: Webview, action: String) -> Result<String, String> {
    if !from_overlay(&webview) {
        return Err("not allowed".into());
    }
    #[cfg(windows)]
    return run::action(&app, &action);
    #[cfg(not(windows))]
    {
        let _ = (app, action);
        Err("Windows only".into())
    }
}
