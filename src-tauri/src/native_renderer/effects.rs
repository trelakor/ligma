//! The thread custom effects run on: each node's effects in order, the
//! file's own code for each, run natively by `effect_host` over the node as
//! drawn without them, and sent back to the render thread as the node's
//! image. What they draw is kept whole, compressed, so another size of it
//! doesn't run them again

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use skia_safe::{AlphaType, ColorType, Data, ImageInfo, Rect};

use super::draw::Img;
use super::effect_host::Host;
use super::render::{trace, Msg};

pub enum Job {
    Run(Run),
    /// What node `key`'s effects drew, kept, at a size for `width` pixels
    Resize { key: String, width: f64 },
}

impl Job {
    fn key(&self) -> &str {
        match self {
            Job::Run(run) => &run.key,
            Job::Resize { key, .. } => key,
        }
    }
}

/// What nodes' effects drew, whole, compressed, until they change
#[derive(Default)]
pub struct Results {
    kept: HashMap<String, Full>,
    bytes: usize,
    clock: u64,
}

pub type Kept = Arc<Mutex<Results>>;

struct Full {
    compressed: Vec<u8>,
    width: u32,
    height: u32,
    scale: f32,
    used: u64,
}

/// Compressed results kept, past which the longest unused go, to be run
/// again if needed
const KEPT_BUDGET: usize = 48 << 20;

impl Results {
    pub fn has(&self, key: &str) -> bool {
        self.kept.contains_key(key)
    }

    /// Drops what's kept for keys `keep` says no to
    pub fn retain(&mut self, mut keep: impl FnMut(&str) -> bool) {
        let mut bytes = self.bytes;
        self.kept.retain(|key, full| {
            let kept = keep(key);
            if !kept {
                bytes -= full.compressed.len();
            }
            kept
        });
        self.bytes = bytes;
    }

    fn put(&mut self, key: &str, full: Full) {
        self.bytes += full.compressed.len();
        if let Some(old) = self.kept.insert(key.to_string(), full) {
            self.bytes -= old.compressed.len();
        }
        while self.bytes > KEPT_BUDGET && self.kept.len() > 1 {
            let Some(oldest) = self.kept.iter().filter(|(other, _)| *other != key).min_by_key(|(_, full)| full.used).map(|(key, _)| key.clone()) else {
                break;
            };
            if let Some(gone) = self.kept.remove(&oldest) {
                self.bytes -= gone.compressed.len();
            }
        }
    }
}

/// A node's effects to run
pub struct Run {
    pub key: String,
    pub node: String,
    /// Each effect's code by name, and its settings
    pub chain: Vec<(String, serde_json::Value)>,
    /// The node as drawn without its effects, premultiplied RGBA
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Pixels a unit of the node's own space
    pub scale: f32,
    /// Pixels wide it's shown at, which its result is kept about the size of
    pub shown: f64,
}

/// Effect code by name, from the page
pub type Fetch = Box<dyn Fn(&str) -> Result<String, String> + Send>;

/// Runs jobs from `jobs` as they come, on the GPU named `adapter`, and sends
/// each result, or that it failed, to `messages`. Returns once its GPU is
/// set up, which has to be before anything else draws: finding GPUs while
/// Skia draws can deadlock NVIDIA's driver
pub fn spawn(jobs: Receiver<Job>, messages: Sender<Msg>, adapter: Option<String>, fetch: Fetch, kept: Kept) {
    let (ready, set_up) = std::sync::mpsc::channel();
    let started = std::thread::Builder::new()
        .name("native-effects".into())
        // QuickJS and the shader compiler both go deep
        .stack_size(16 << 20)
        .spawn(move || {
            super::shader_cache::install();
            let start = Instant::now();
            let host = Host::new(adapter.as_deref());
            let _ = ready.send(());
            match host {
                Ok(host) => {
                    trace(|| format!("effects set up {:.0}ms", start.elapsed().as_secs_f64() * 1000.0));
                    run(host, jobs, messages, fetch, kept);
                }
                Err(error) => {
                    log::warn!("native renderer: effects: {error}");
                    while let Ok(job) = jobs.recv() {
                        let _ = messages.send(Msg::Failed(job.key().to_string()));
                    }
                }
            }
        });
    match started {
        Ok(_) => {
            let _ = set_up.recv_timeout(std::time::Duration::from_secs(10));
        }
        Err(error) => log::warn!("native renderer: effects thread: {error}"),
    }
}

fn run(mut host: Host, jobs: Receiver<Job>, messages: Sender<Msg>, fetch: Fetch, kept: Kept) {
    // Code that couldn't be had, not asked for again
    let mut missing: HashMap<String, String> = HashMap::new();
    while let Ok(first) = jobs.recv() {
        // Only the latest for each node, when several came meanwhile
        let mut latest: Vec<Job> = vec![first];
        while let Ok(job) = jobs.try_recv() {
            // A run isn't put off for a resize, which needs it first
            if matches!(job, Job::Resize { .. }) && latest.iter().any(|other| matches!(other, Job::Run(_)) && other.key() == job.key()) {
                continue;
            }
            latest.retain(|other| other.key() != job.key());
            latest.push(job);
        }
        for job in latest {
            let key = job.key().to_string();
            let done = match job {
                Job::Run(run) => one(&mut host, &fetch, &mut missing, &kept, run),
                Job::Resize { key, width } => resize(&kept, &key, width),
            };
            match done {
                Ok(img) => {
                    let _ = messages.send(Msg::Image(key, img, None));
                }
                Err(error) => {
                    log::warn!("native renderer: effects of {key}: {error}");
                    let _ = messages.send(Msg::Failed(key));
                }
            }
        }
    }
}

fn one(host: &mut Host, fetch: &Fetch, missing: &mut HashMap<String, String>, kept: &Kept, job: Run) -> Result<Img, String> {
    for (id, _) in &job.chain {
        if host.has(id) {
            continue;
        }
        if let Some(error) = missing.get(id) {
            return Err(error.clone());
        }
        let start = Instant::now();
        let code = fetch(id).inspect_err(|error| {
            missing.insert(id.clone(), error.clone());
        })?;
        host.load(id, &code)?;
        trace(|| format!("effect {id} loaded {:.0}ms", start.elapsed().as_secs_f64() * 1000.0));
    }
    let chain: Vec<(String, String)> = job.chain.iter().map(|(id, params)| (id.clone(), params.to_string())).collect();
    let start = Instant::now();
    let pixels = host.run(&job.node, &chain, &job.pixels, job.width, job.height, job.scale, 0.0)?;
    dump(&job, &pixels);
    trace(|| format!("effects {} {}x{} {:.1}ms", job.key, job.width, job.height, start.elapsed().as_secs_f64() * 1000.0));
    let img = shrunk(&pixels, job.width, job.height, job.scale, job.shown);
    let compressed = lz4_flex::compress_prepend_size(&pixels);
    let mut results = kept.lock().unwrap();
    results.clock += 1;
    let used = results.clock;
    results.put(&job.key, Full { compressed, width: job.width, height: job.height, scale: job.scale, used });
    img
}

/// What node `key`'s effects drew, as kept, for `width` pixels
fn resize(kept: &Kept, key: &str, width: f64) -> Result<Img, String> {
    let (compressed, w, h, scale) = {
        let mut results = kept.lock().unwrap();
        results.clock += 1;
        let used = results.clock;
        let full = results.kept.get_mut(key).ok_or("not kept")?;
        full.used = used;
        (full.compressed.clone(), full.width, full.height, full.scale)
    };
    let pixels = lz4_flex::decompress_size_prepended(&compressed).map_err(|error| error.to_string())?;
    shrunk(&pixels, w, h, scale, width)
}

/// `pixels`, `width` by `height` at `scale` pixels a unit, smaller where
/// they show at `shown` pixels wide, filtered on premultiplied colors
fn shrunk(pixels: &[u8], width: u32, height: u32, scale: f32, shown: f64) -> Result<Img, String> {
    let longest = width.max(height);
    let keep = longest >> super::run::halvings(longest, longest as f64 * shown * 1.25 / width.max(1) as f64);
    let (kw, kh) = ((width * keep).div_ceil(longest).clamp(1, width), (height * keep).div_ceil(longest).clamp(1, height));
    let small;
    let pixels = if (kw, kh) == (width, height) {
        pixels
    } else {
        let full = image::RgbaImage::from_raw(width, height, pixels.to_vec()).ok_or("wrong size")?;
        small = image::imageops::resize(&full, kw, kh, image::imageops::FilterType::Triangle).into_raw();
        &small
    };
    let info = ImageInfo::new((kw as i32, kh as i32), ColorType::RGBA8888, AlphaType::Premul, None);
    let image = skia_safe::images::raster_from_data(&info, Data::new_copy(pixels), (kw * 4) as usize).ok_or("couldn't make the image")?;
    // A pixel to a pixel at its scale, so the last column and row, which the
    // box only partly covers, reach a little past it
    let area = Rect::from_wh(width as f32 / scale, height as f32 / scale);
    Ok(Img { image, original: (width, height), area: Some(area), stale: false })
}

/// Dev tools: with LIGMA_NATIVE_EFFECT_DUMP set to a folder, each run's
/// input and output go there as raw premultiplied RGBA, and its chain as JSON
fn dump(job: &Run, output: &[u8]) {
    let Some(dir) = std::env::var_os("LIGMA_NATIVE_EFFECT_DUMP").filter(|_| super::dev()) else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let name = job.key.replace([':', '>', ';'], "_");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(format!("{name}.in.rgba")), &job.pixels);
    let _ = std::fs::write(dir.join(format!("{name}.out.rgba")), output);
    let chain: Vec<_> = job.chain.iter().map(|(id, params)| serde_json::json!([id, params])).collect();
    let _ = std::fs::write(dir.join(format!("{name}.json")), serde_json::json!({ "width": job.width, "height": job.height, "scale": job.scale, "chain": chain }).to_string());
}
