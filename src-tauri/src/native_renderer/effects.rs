//! The thread custom effects run on: each node's effects in order, the
//! file's own code for each, run natively by `effect_host` over the node as
//! drawn without them, and sent back to the render thread as the node's
//! image

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Instant;

use skia_safe::{AlphaType, ColorType, Data, ImageInfo, Rect};

use super::draw::Img;
use super::effect_host::Host;
use super::render::{trace, Msg};

/// A node's effects to run
pub struct Job {
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
}

/// Effect code by name, from the page
pub type Fetch = Box<dyn Fn(&str) -> Result<String, String> + Send>;

/// Runs jobs from `jobs` as they come, on the GPU named `adapter`, and sends
/// each result, or that it failed, to `messages`. Returns once its GPU is
/// set up, which has to be before anything else draws: finding GPUs while
/// Skia draws can deadlock NVIDIA's driver
pub fn spawn(jobs: Receiver<Job>, messages: Sender<Msg>, adapter: Option<String>, fetch: Fetch) {
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
                    run(host, jobs, messages, fetch);
                }
                Err(error) => {
                    log::warn!("native renderer: effects: {error}");
                    while let Ok(job) = jobs.recv() {
                        let _ = messages.send(Msg::Failed(job.key));
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

fn run(mut host: Host, jobs: Receiver<Job>, messages: Sender<Msg>, fetch: Fetch) {
    // Code that couldn't be had, not asked for again
    let mut missing: HashMap<String, String> = HashMap::new();
    while let Ok(first) = jobs.recv() {
        // Only the latest for each node, when several came meanwhile
        let mut latest: Vec<Job> = vec![first];
        while let Ok(job) = jobs.try_recv() {
            latest.retain(|kept| kept.key != job.key);
            latest.push(job);
        }
        for job in latest {
            let key = job.key.clone();
            match one(&mut host, &fetch, &mut missing, job) {
                Ok(img) => {
                    let _ = messages.send(Msg::Image(key, img));
                }
                Err(error) => {
                    log::warn!("native renderer: effects of {key}: {error}");
                    let _ = messages.send(Msg::Failed(key));
                }
            }
        }
    }
}

fn one(host: &mut Host, fetch: &Fetch, missing: &mut HashMap<String, String>, job: Job) -> Result<Img, String> {
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
    let info = ImageInfo::new((job.width as i32, job.height as i32), ColorType::RGBA8888, AlphaType::Premul, None);
    let image = skia_safe::images::raster_from_data(&info, Data::new_copy(&pixels), (job.width * 4) as usize).ok_or("couldn't make the image")?;
    // A pixel to a pixel at its scale, so the last column and row, which the
    // box only partly covers, reach a little past it
    let area = Rect::from_wh(job.width as f32 / job.scale, job.height as f32 / job.scale);
    Ok(Img { image, original: (job.width, job.height), area: Some(area), stale: false })
}

/// Dev tools: with LIGMA_NATIVE_EFFECT_DUMP set to a folder, each run's
/// input and output go there as raw premultiplied RGBA, and its chain as JSON
fn dump(job: &Job, output: &[u8]) {
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
