//! Settings' Show stats: frames and GPU memory of whichever renderer draws
//! Figma's canvas, Ligma's or Figma's own, kept for each, so after switching
//! the one used before shows beside the one in use. A panel in the Figma
//! tab draws them (stats_panel.js), and times Figma's own frames there

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Webview};

use super::gpu_memory::GpuMemory;
use super::stats;

/// Frames in the frame graph
const FRAMES: usize = 200;
/// Memory samples, one a tick: the last minute
const SAMPLES: usize = 240;
/// How often everything is handed to the panel, memory sampled with it
const TICK: Duration = Duration::from_millis(250);
/// How often frames Ligma's renderer presented since are, for the panel's
/// graphs to move with them, as often as the panel's drawn. Each is a
/// script run on the page's main thread, which Figma's frames need
const FRAME_TICK: Duration = Duration::from_millis(33);

const PANEL_JS: &str = include_str!("stats_panel.js");

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    /// How long each frame took to make, in ms: Figma's part, from the
    /// input it answers or the start of the page's frame to handing it over,
    /// and the renderer's, from then till it's on screen. Waiting for input
    /// isn't in it, so input that comes late doesn't make frames look slow
    frames: VecDeque<(f32, f32)>,
    /// The renderer and the page, in MB. Figma's renderer is in its page,
    /// so it's what the page takes above what it took while Ligma's
    /// renderer drew, or below 0 before it ever did
    memory: VecDeque<(f32, f32)>,
    /// Ligma's renderer's draw times and counts
    numbers: Option<Value>,
    /// Frames ever recorded
    #[serde(skip)]
    total: u64,
}

impl Record {
    fn frame(&mut self, made: (f32, f32)) {
        self.total += 1;
        self.frames.push_back(made);
        if self.frames.len() > FRAMES {
            self.frames.pop_front();
        }
    }
}

/// Ligma's renderer, then Figma's
static RECORDS: Mutex<[Option<Record>; 2]> = Mutex::new([None, None]);
static RUNNING: AtomicBool = AtomicBool::new(false);

/// A frame of Figma's Ligma's renderer presented, and how long Figma and
/// the renderer took to make it, in ms
pub fn presented(figma: f32, ligma: f32) {
    if !super::show_stats() {
        return;
    }
    RECORDS.lock().unwrap()[0].get_or_insert_with(Record::default).frame((figma, ligma));
}

/// Shows the panel while Show stats is on, from when it's turned on
pub fn follow(app: &AppHandle) {
    if super::show_stats() && !RUNNING.swap(true, Ordering::Relaxed) {
        let app = app.clone();
        let _ = std::thread::Builder::new().name("native-stats".into()).spawn(move || run(app));
    }
}

fn run(app: AppHandle) {
    let memory = GpuMemory::new();
    let mut shown: Option<Webview> = None;
    let mut tab: Option<Webview> = None;
    let mut ticked: Option<Instant> = None;
    // Ligma's frames the panel has
    let mut sent = 0;
    while super::show_stats() {
        let ligma = super::ON.load(Ordering::Relaxed);
        if ticked.is_some_and(|at| at.elapsed() < TICK) {
            // Only the frames presented since
            let frames: Vec<(f32, f32)> = {
                let records = RECORDS.lock().unwrap();
                let record = records[0].as_ref().filter(|_| ligma);
                let new = record.map_or(0, |record| (record.total - sent).min(record.frames.len() as u64) as usize);
                let frames = record.map(|record| record.frames.iter().skip(record.frames.len() - new).copied().collect()).unwrap_or_default();
                if let Some(record) = record {
                    sent = record.total;
                }
                frames
            };
            if let (false, Some(tab)) = (frames.is_empty(), &tab) {
                let _ = tab.eval(format!("window.__ligmaStats?.tick({})", json!({ "active": "ligma", "frames": frames })));
            }
            std::thread::sleep(FRAME_TICK);
            continue;
        }
        ticked = Some(Instant::now());
        let active = usize::from(!ligma);
        let sample = memory.as_ref().map(GpuMemory::sample);
        tab = crate::tabs::active_file(&app).filter(|file| file.editor == "design").map(|file| file.webview);
        // Figma's renderer draws in the browser's GPU process with the rest
        // of the page, so it's what the page counts it made (see gpu_count.js)
        let figma = if ligma { None } else { tab.as_ref().and_then(|tab| super::run::eval_value(tab, "window.__ligmaGpuBytes ?? null")).and_then(|value| value.as_f64()) };
        let model = {
            let mut records = RECORDS.lock().unwrap();
            let record = records[active].get_or_insert_with(Record::default);
            if let Some(sample) = &sample {
                let (own, page) = (sample.ligma as f32 / 1e6, sample.webview as f32 / 1e6);
                record.memory.push_back(match figma {
                    _ if ligma => (own, page),
                    Some(figma) => {
                        let figma = (figma as f32 / 1e6).clamp(0.0, page);
                        (figma, page - figma)
                    }
                    None => (-1.0, page),
                });
                if record.memory.len() > SAMPLES {
                    record.memory.pop_front();
                }
            }
            if ligma {
                let stats = stats().lock().unwrap();
                record.numbers = Some(json!({
                    "presentMs": stats.frame_ms,
                    "tilesMs": stats.vello_tiles_ms.max(stats.tile_record_ms + stats.tile_play_ms + stats.tile_gpu_ms),
                    "wholeMs": stats.whole_ms,
                    "drawn": stats.native,
                    "approximate": stats.approximate,
                    "tiles": stats.tiles,
                    "zoom": stats.zoom,
                }));
            }
            sent = records[0].as_ref().map_or(0, |record| record.total);
            json!({ "active": if ligma { "ligma" } else { "figma" }, "records": &*records })
        };
        // The panel moves with the active tab
        if let Some(before) = shown.take_if(|before| tab.as_ref().is_none_or(|tab| tab.label() != before.label())) {
            let _ = before.eval("window.__ligmaStats?.remove()");
        }
        if let Some(tab) = tab.clone() {
            let script = format!("(window.__ligmaStats || {PANEL_JS}).tick({model})");
            if let Some(Value::Array(frames)) = eval(&tab, &script) {
                if !ligma {
                    let mut records = RECORDS.lock().unwrap();
                    let record = records[1].get_or_insert_with(Record::default);
                    for frame in frames {
                        if let Some(ms) = frame.as_f64() {
                            record.frame((ms as f32, 0.0));
                        }
                    }
                }
            }
            shown = Some(tab);
        }
        std::thread::sleep(FRAME_TICK);
    }
    for tab in crate::tabs::scriptable_tabs(&app) {
        let _ = tab.eval("window.__ligmaStats?.remove()");
    }
    RUNNING.store(false, Ordering::Relaxed);
    // Turned on again as it stopped
    follow(&app);
}

fn eval(webview: &Webview, script: &str) -> Option<Value> {
    let (send, receive) = mpsc::channel();
    webview
        .eval_with_callback(script, move |result| {
            let _ = send.send(result);
        })
        .ok()?;
    serde_json::from_str(&receive.recv_timeout(Duration::from_secs(1)).ok()?).ok()
}
