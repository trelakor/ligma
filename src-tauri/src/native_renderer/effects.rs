//! The thread custom effects run on: each node's effects in order, the
//! file's own code for each, run natively by `effect_host` over the node as
//! drawn without them, and sent back to the render thread as the node's
//! image. What they draw is kept whole, compressed once the thread's idle,
//! so another size of it doesn't run them again

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

/// What nodes' effects drew, whole, until they change
#[derive(Default)]
pub struct Results {
    kept: HashMap<String, Full>,
    bytes: usize,
    clock: u64,
    /// Runs asked for of what's since changed or gone, not to be run
    pub stale: std::collections::HashSet<String>,
}

pub type Kept = Arc<Mutex<Results>>;

struct Full {
    /// Compressed, or left as it is till the thread's idle, as one run
    /// after another while an effect's edited replaces it each time
    data: Vec<u8>,
    raw: bool,
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
                bytes -= full.data.len();
            }
            kept
        });
        self.bytes = bytes;
    }

    fn put(&mut self, key: &str, full: Full) {
        self.bytes += full.data.len();
        if let Some(old) = self.kept.insert(key.to_string(), full) {
            self.bytes -= old.data.len();
        }
        while self.bytes > KEPT_BUDGET && self.kept.len() > 1 {
            let Some(oldest) = self.kept.iter().filter(|(other, _)| *other != key).min_by_key(|(_, full)| full.used).map(|(key, _)| key.clone()) else {
                break;
            };
            if let Some(gone) = self.kept.remove(&oldest) {
                self.bytes -= gone.data.len();
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
    // Code that couldn't be had, and when: not asked for again for a while,
    // as a tab that didn't answer may have crashed and loaded again since
    let mut missing: HashMap<String, (String, Instant)> = HashMap::new();
    loop {
        let first = match jobs.try_recv() {
            Ok(job) => job,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                // Idle: one result compressed at a time, then back to any
                // job that's come meanwhile
                if compress_one(&kept) {
                    continue;
                }
                match jobs.recv() {
                    Ok(job) => job,
                    Err(_) => break,
                }
            }
        };
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
            // Moving a node asks again each frame, and what it was a frame
            // ago would show it going back there once it's stopped
            if matches!(job, Job::Run(_)) && kept.lock().unwrap().stale.remove(&key) {
                let _ = messages.send(Msg::Skipped(key));
                continue;
            }
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

fn one(host: &mut Host, fetch: &Fetch, missing: &mut HashMap<String, (String, Instant)>, kept: &Kept, job: Run) -> Result<Img, String> {
    for (id, _) in &job.chain {
        if host.has(id) {
            continue;
        }
        if let Some((error, _)) = missing.get(id).filter(|(_, at)| at.elapsed() < std::time::Duration::from_secs(10)) {
            return Err(error.clone());
        }
        let start = Instant::now();
        let code = fetch(id).inspect_err(|error| {
            missing.insert(id.clone(), (error.clone(), Instant::now()));
        })?;
        host.load(id, &code)?;
        trace(|| format!("effect {id} loaded {:.0}ms", start.elapsed().as_secs_f64() * 1000.0));
    }
    let chain: Vec<(String, String)> = job.chain.iter().map(|(id, params)| (id.clone(), params.to_string())).collect();
    let start = Instant::now();
    let pixels = host.run(&job.node, &chain, &job.pixels, job.width, job.height, job.scale, 0.0)?;
    let ran = start.elapsed();
    dump(&job, &pixels);
    let img = shrunk(&pixels, job.width, job.height, job.scale, job.shown);
    trace(|| format!("effects {} {}x{} {:.1}ms run={:.1} shown={:.0}", job.key, job.width, job.height, start.elapsed().as_secs_f64() * 1000.0, ran.as_secs_f64() * 1000.0, job.shown));
    let mut results = kept.lock().unwrap();
    results.clock += 1;
    let used = results.clock;
    results.put(&job.key, Full { data: pixels, raw: true, width: job.width, height: job.height, scale: job.scale, used });
    img
}

/// Compresses one result left as it is, if there's one, and whether there was
fn compress_one(kept: &Kept) -> bool {
    // Copied out, so what draws isn't held up while it's compressed
    let (key, pixels, used) = {
        let results = kept.lock().unwrap();
        let Some((key, full)) = results.kept.iter().find(|(_, full)| full.raw) else {
            return false;
        };
        (key.clone(), full.data.clone(), full.used)
    };
    let compressed = lz4_flex::compress_prepend_size(&pixels);
    let mut results = kept.lock().unwrap();
    let Results { kept: all, bytes, .. } = &mut *results;
    // Unless it's been run again or used since
    if let Some(full) = all.get_mut(&key).filter(|full| full.raw && full.used == used) {
        *bytes = *bytes - full.data.len() + compressed.len();
        full.data = compressed;
        full.raw = false;
    }
    true
}

/// What node `key`'s effects drew, as kept, for `width` pixels
fn resize(kept: &Kept, key: &str, width: f64) -> Result<Img, String> {
    let (data, raw, w, h, scale) = {
        let mut results = kept.lock().unwrap();
        results.clock += 1;
        let used = results.clock;
        let full = results.kept.get_mut(key).ok_or("not kept")?;
        full.used = used;
        (full.data.clone(), full.raw, full.width, full.height, full.scale)
    };
    let pixels = if raw { data } else { lz4_flex::decompress_size_prepended(&data).map_err(|error| error.to_string())? };
    shrunk(&pixels, w, h, scale, width)
}

/// `pixels`, `width` by `height` at `scale` pixels a unit, smaller where
/// they show at `shown` pixels wide, filtered on premultiplied colors
fn shrunk(pixels: &[u8], width: u32, height: u32, scale: f32, shown: f64) -> Result<Img, String> {
    if pixels.len() != (width * height * 4) as usize {
        return Err("wrong size".into());
    }
    let longest = width.max(height);
    let (mut kw, mut kh) = (width, height);
    let mut small: Option<Vec<u8>> = None;
    for _ in 0..super::run::halvings(longest, longest as f64 * shown * 1.25 / width.max(1) as f64) {
        if kw == 1 && kh == 1 {
            break;
        }
        let (half, hw, hh) = halve(small.as_deref().unwrap_or(pixels), kw, kh);
        (small, kw, kh) = (Some(half), hw, hh);
    }
    let pixels = small.as_deref().unwrap_or(pixels);
    let info = ImageInfo::new((kw as i32, kh as i32), ColorType::RGBA8888, AlphaType::Premul, None);
    let image = skia_safe::images::raster_from_data(&info, Data::new_copy(pixels), (kw * 4) as usize).ok_or("couldn't make the image")?;
    // A pixel to a pixel at its scale, so the last column and row, which the
    // box only partly covers, reach a little past it
    let area = Rect::from_wh(width as f32 / scale, height as f32 / scale);
    Ok(Img { image, original: (width, height), area: Some(area), stale: false })
}

/// Premultiplied `pixels`, `width` by `height`, half the size, each pixel
/// the mean of those 2 by 2 under it, or of those there are at an odd edge
fn halve(pixels: &[u8], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let (w, h) = (width as usize, height as usize);
    let (hw, hh) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = vec![0u8; hw * hh * 4];
    for y in 0..hh {
        let rows = [y * 2, (y * 2 + 1).min(h - 1)];
        let rows = &rows[..if rows[0] == rows[1] { 1 } else { 2 }];
        for x in 0..hw {
            let columns = [x * 2, (x * 2 + 1).min(w - 1)];
            let columns = &columns[..if columns[0] == columns[1] { 1 } else { 2 }];
            let n = (rows.len() * columns.len()) as u32;
            let mut sum = [0u32; 4];
            for &row in rows {
                for &column in columns {
                    let at = (row * w + column) * 4;
                    for (total, &value) in sum.iter_mut().zip(&pixels[at..at + 4]) {
                        *total += value as u32;
                    }
                }
            }
            let to = (y * hw + x) * 4;
            for (out, total) in out[to..to + 4].iter_mut().zip(sum) {
                *out = ((total + n / 2) / n) as u8;
            }
        }
    }
    (out, hw as u32, hh as u32)
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

#[cfg(test)]
mod tests {
    use super::halve;

    #[test]
    fn halves_odd_sizes_by_what_is_there() {
        // 3 by 1: the first two averaged, the last on its own
        let pixels = [10, 20, 30, 40, 30, 40, 50, 60, 200, 100, 0, 255];
        let (out, w, h) = halve(&pixels, 3, 1);
        assert_eq!((w, h), (2, 1));
        assert_eq!(out, vec![20, 30, 40, 50, 200, 100, 0, 255]);
        let (out, w, h) = halve(&[7; 4 * 5 * 3], 5, 3);
        assert_eq!((w, h), (3, 2));
        assert!(out.iter().all(|&value| value == 7));
    }
}
