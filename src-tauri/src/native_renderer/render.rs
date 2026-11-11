//! The render thread: Skia on D3D12, drawing the mirrored scene into the
//! native renderer's window as Figma's viewport moves
//!
//! The page is cut into units, its top-level layers and each layer right in
//! a section, and each unit is recorded once as a Skia picture for a zoom
//! level (a power of two), which sets its level of detail. The view is drawn
//! in tiles of 512 pixels at its exact zoom, each the pictures over it
//! played back 4 times bigger each way without anti-aliasing and averaged
//! down, 16 samples a pixel as Figma does, so shapes that share an edge
//! show no seam. MSAA would do the same, but D3D12 keeps hundreds of MB
//! for it. Tiles are kept, so panning only draws the tiles coming into view. While the zoom
//! changes, the tiles of the last zoom are shown scaled until it settles
//! and new ones are drawn, much as Figma does. Changes and images that come
//! in redraw only the tiles they touch

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use skia_safe::gpu::d3d::{BackendContext, TextureResourceInfo};
use skia_safe::gpu::{self, BackendRenderTarget, DirectContext, Protected, SurfaceOrigin, SyncCpu};
use skia_safe::{
    AlphaType, Color4f, ColorType, Data, FilterMode, Image, ImageInfo, MipmapMode, Paint, Picture,
    PictureRecorder, Rect, SamplingOptions, Surface,
};
use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{D3D12CreateDevice, ID3D12Device, D3D12_RESOURCE_STATE_COMMON};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC, DXGI_STANDARD_MULTISAMPLE_QUALITY_PATTERN};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIAdapter3, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIFactory4, IDXGIFactory6, IDXGISwapChain3, DXGI_ADAPTER_FLAG, DXGI_ADAPTER_FLAG_SOFTWARE,
    DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE, DXGI_PRESENT, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};

use super::draw::{Counts, Encoder, Img, Kit};
use super::scene::{Kind, Prepared, Scene};
use super::stats;

/// Figma's viewport, from `view()` in page.js
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq)]
pub struct View {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub zoom: f64,
    pub cw: f64,
    pub ch: f64,
    pub dpr: f64,
    pub page: String,
    #[serde(default)]
    pub visible: bool,
}

pub enum Msg {
    /// A page starts streaming in, empty
    Begin(Box<Scene>),
    Changes(Prepared),
    View(View),
    Image(String, Img),
    Resize(u32, u32),
    /// Draw node `id` alone, offscreen, over `bounds` at `width` by
    /// `height`, and send back its pixels, RGBA with straight alpha
    Offscreen { id: String, bounds: Rect, width: u32, height: u32, reply: Sender<Option<Offscreen>> },
    /// Draw each view as the window would, offscreen at the size of
    /// Figma's canvas, and send back each frame's time in ms, cold (no
    /// tiles kept) and then warm
    Replay { views: Vec<View>, reply: Sender<Vec<(f64, f64)>> },
    Tint(bool),
    /// Draw `view` whole, `width` by `height` pixels, and send back its
    /// pixels, RGBA, and how many images it's still waiting for
    Render { view: View, width: u32, height: u32, reply: Sender<Option<(Vec<u8>, usize)>> },
    /// The window's view as it shows now, from the tiles it has, then drawn
    /// again from nothing: any difference is a tile left out of date. Sends
    /// back both, RGBA, and the size
    Live(Sender<Option<(Vec<u8>, Vec<u8>, u32, u32)>>),
    /// For each absolute point, the ids of the nodes under it, from the
    /// top-level one down to the deepest
    HitTest { points: Vec<(f32, f32)>, reply: Sender<Vec<Vec<String>>> },
    /// Frees what Skia keeps that nothing uses, and sends back the GPU
    /// memory used before, after purging, and after freeing everything
    Purge(Sender<[u64; 3]>),
    Quit,
}

pub struct Offscreen {
    /// RGBA, straight alpha
    pub pixels: Vec<u8>,
    /// Why something in it is only drawn close to Figma's look
    pub approximate_reason: Option<String>,
    /// Images not here yet, asked for
    pub waiting: usize,
}

/// What the render thread asks the page for: an image fill, about as wide
/// as given in pixels
pub struct Request {
    pub key: String,
    pub width: f64,
}

struct Unit {
    slot: usize,
    /// A section's own drawing, without what's in it, or, for a mask in a
    /// section, the section's children from it on, which it masks
    section: bool,
    bounds: Rect,
}

struct Recorded {
    picture: Picture,
    counts: Counts,
    reason: Option<String>,
    missing: Vec<String>,
    /// Images it has nothing to draw for yet
    waiting: usize,
    /// Nodes its pattern fills drew from, and whether each was there yet
    sources: Vec<(String, bool)>,
}

const TILE: i32 = 512;

/// Settings that trade memory for quality, which can be set by environment
/// variable in a dev build to measure them
#[derive(Clone, Copy, Debug)]
struct Config {
    /// Samples per pixel when drawing tiles
    samples: usize,
    /// Tiles are drawn this many times bigger each way and scaled down,
    /// so shapes that share an edge show no seam
    supersample: i32,
    /// Tiles kept, 1 MB each
    max_tiles: usize,
    /// Skia's own GPU cache: images, path masks, scratch layers
    cache_limit: usize,
}

fn config() -> Config {
    let get = |name: &str, default: usize| {
        if !super::dev() {
            return default;
        }
        std::env::var(name).ok().and_then(|value| value.parse().ok()).unwrap_or(default)
    };
    Config {
        samples: get("LIGMA_NATIVE_SAMPLES", 1),
        supersample: get("LIGMA_NATIVE_SUPERSAMPLE", 4).clamp(1, 4) as i32,
        max_tiles: get("LIGMA_NATIVE_TILES", 64),
        cache_limit: get("LIGMA_NATIVE_CACHE_MB", 96) << 20,
    }
}

/// Widest margin a tile is drawn with, in pixels
const MAX_MARGIN: i32 = 256;
/// Time to spend per frame drawing tiles not ready yet, so a newly shown
/// part of the page fills in over a few frames rather than stalling one
const BUILD_BUDGET: Duration = Duration::from_millis(10);
/// Memory drawing took is given back after this long without a frame
const IDLE: Duration = Duration::from_secs(3);
/// The zoom counts as settled when it hasn't changed for this long
const SETTLE: Duration = Duration::from_millis(120);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct TileKey {
    /// Pixels per unit, as f32 bits
    px: u32,
    x: i32,
    y: i32,
}

impl TileKey {
    fn px(&self) -> f32 {
        f32::from_bits(self.px)
    }

    /// The absolute area it covers
    fn area(&self) -> Rect {
        let px = self.px();
        Rect::from_ltrb(
            (self.x * TILE) as f32 / px,
            (self.y * TILE) as f32 / px,
            ((self.x + 1) * TILE) as f32 / px,
            ((self.y + 1) * TILE) as f32 / px,
        )
    }
}

struct Tile {
    image: Image,
    used: u64,
    counts: Counts,
    /// Out of date, shown until it's drawn again
    stale: bool,
}

pub struct Gpu {
    context: DirectContext,
    swap_chain: IDXGISwapChain3,
    /// The swap chain's buffers as Skia surfaces. The render targets are
    /// kept with them, as Skia doesn't own them
    buffers: Vec<(Surface, BackendRenderTarget)>,
    size: (u32, u32),
    pub adapter: String,
    memory: Option<IDXGIAdapter3>,
}

fn hardware_adapter(factory: &IDXGIFactory4) -> windows::core::Result<(IDXGIAdapter1, ID3D12Device)> {
    // The fastest GPU first, as on laptops with two
    let preferred = factory.cast::<IDXGIFactory6>().ok();
    for i in 0.. {
        let adapter: IDXGIAdapter1 = match &preferred {
            Some(factory) => unsafe { factory.EnumAdapterByGpuPreference(i, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE) }?,
            None => unsafe { factory.EnumAdapters1(i) }?,
        };
        let desc = unsafe { adapter.GetDesc1() }?;
        if DXGI_ADAPTER_FLAG(desc.Flags as i32) & DXGI_ADAPTER_FLAG_SOFTWARE != DXGI_ADAPTER_FLAG(0) {
            continue;
        }
        let mut device: Option<ID3D12Device> = None;
        if unsafe { D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device) }.is_ok() {
            if let Some(device) = device {
                return Ok((adapter, device));
            }
        }
    }
    unreachable!()
}

impl Gpu {
    pub fn new(hwnd: isize, width: u32, height: u32) -> Result<Self, String> {
        let hwnd = HWND(hwnd as *mut _);
        let factory: IDXGIFactory4 = unsafe { CreateDXGIFactory1() }.map_err(|error| format!("DXGI: {error}"))?;
        let (adapter, device) = hardware_adapter(&factory).map_err(|error| format!("adapter: {error}"))?;
        let desc = unsafe { adapter.GetDesc1() }.map_err(|error| error.to_string())?;
        let name = String::from_utf16_lossy(&desc.Description).trim_end_matches('\0').to_string();
        let queue = unsafe { device.CreateCommandQueue(&Default::default()) }.map_err(|error| format!("queue: {error}"))?;
        let memory = adapter.cast::<IDXGIAdapter3>().ok();
        let backend = BackendContext { adapter, device, queue, memory_allocator: None, protected_context: Protected::No };
        let mut context = unsafe { gpu::direct_contexts::make_d3d(&backend, None) }.ok_or("Skia couldn't start on D3D12")?;
        context.set_resource_cache_limit(config().cache_limit);
        let swap_chain: IDXGISwapChain3 = unsafe {
            factory.CreateSwapChainForHwnd(
                &backend.queue,
                hwnd,
                &DXGI_SWAP_CHAIN_DESC1 {
                    Width: width.max(1),
                    Height: height.max(1),
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                    BufferCount: 2,
                    SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    ..Default::default()
                },
                None,
                None,
            )
        }
        .and_then(|chain| chain.cast())
        .map_err(|error| format!("swap chain: {error}"))?;
        let mut gpu =
            Self { context, swap_chain, buffers: Vec::new(), size: (width.max(1), height.max(1)), adapter: format!("{name} (D3D12)"), memory };
        gpu.wrap_buffers()?;
        log::info!("native renderer: started on {}, {:?}, {:.0} MB of GPU memory", gpu.adapter, config(), gpu.used() as f64 / 1e6);
        Ok(gpu)
    }

    /// GPU memory this process uses, as D3D12 counts it
    fn used(&self) -> u64 {
        let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        match &self.memory {
            Some(adapter) if unsafe { adapter.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info) }.is_ok() => info.CurrentUsage,
            _ => 0,
        }
    }

    fn wrap_buffers(&mut self) -> Result<(), String> {
        let (width, height) = self.size;
        self.buffers.clear();
        for i in 0..2 {
            let resource = unsafe { self.swap_chain.GetBuffer(i) }.map_err(|error| format!("buffer: {error}"))?;
            let target = gpu::backend_render_targets::make_d3d(
                (width as i32, height as i32),
                &TextureResourceInfo {
                    resource,
                    alloc: None,
                    resource_state: D3D12_RESOURCE_STATE_COMMON,
                    format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    sample_count: 1,
                    level_count: 0,
                    sample_quality_pattern: DXGI_STANDARD_MULTISAMPLE_QUALITY_PATTERN,
                    protected: Protected::No,
                },
            );
            let surface = gpu::surfaces::wrap_backend_render_target(&mut self.context, &target, SurfaceOrigin::TopLeft, ColorType::RGBA8888, None, None)
                .ok_or("couldn't wrap the swap chain")?;
            self.buffers.push((surface, target));
        }
        Ok(())
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        self.buffers.clear();
        let _ = self.context.flush_submit_and_sync_cpu();
        // Skia may still hold the old buffers in its cache
        self.context.free_gpu_resources();
        self.size = (width.max(1), height.max(1));
        unsafe { self.swap_chain.ResizeBuffers(0, self.size.0, self.size.1, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0)) }
            .map_err(|error| format!("resize: {error}"))?;
        self.wrap_buffers()
    }

    /// An offscreen surface, with `samples` per pixel
    fn surface(&mut self, width: i32, height: i32, samples: usize) -> Option<Surface> {
        let info = ImageInfo::new((width, height), ColorType::RGBA8888, AlphaType::Premul, None);
        gpu::surfaces::render_target(&mut self.context, gpu::Budgeted::Yes, &info, samples, SurfaceOrigin::TopLeft, None, false, None)
    }
}

pub struct Painter {
    gpu: Gpu,
    kit: Kit,
    scene: Scene,
    units: Vec<Unit>,
    /// By unit and zoom level
    pictures: HashMap<(usize, bool, i32), Recorded>,
    images: HashMap<String, Img>,
    /// Widths images were asked for at
    asked: HashMap<String, f64>,
    tiles: HashMap<TileKey, Tile>,
    /// Where tiles are drawn, supersampled, before they're kept, and the margin
    /// around the tile it has
    scratch: Option<(i32, Surface)>,
    /// Where supersampled tiles are scaled down to
    down: Option<Surface>,
    /// Averages each 4 by 4 block of samples into a pixel
    average: Option<skia_safe::RuntimeEffect>,
    /// Background blurs, by their absolute bounds and how far they reach
    backdrops: Vec<(Rect, f32)>,
    config: Config,
    cleaned: Instant,
    /// When a frame was last drawn, and whether memory was given back since
    drawn: Instant,
    released: bool,
    /// Figma's checkerboard, and the pixel grid it draws when zoomed in
    checker: Option<skia_safe::RuntimeEffect>,
    grid: Option<(skia_safe::RuntimeEffect, skia_safe::RuntimeEffect)>,
    view: Option<View>,
    /// The last zoom (pixels per unit) and when it changed
    zoom: (f32, Instant),
    frame: u64,
    tint: bool,
    dirty: bool,
    requests: Sender<Request>,
}

impl Painter {
    pub fn new(gpu: Gpu, requests: Sender<Request>) -> Self {
        Self {
            gpu,
            kit: Kit::new(),
            scene: Scene::default(),
            units: Vec::new(),
            pictures: HashMap::new(),
            images: HashMap::new(),
            asked: HashMap::new(),
            tiles: HashMap::new(),
            scratch: None,
            down: None,
            average: skia_safe::RuntimeEffect::make_for_shader(AVERAGE, None)
                .map_err(|error| log::warn!("native renderer: average: {error}"))
                .ok(),
            backdrops: Vec::new(),
            config: config(),
            cleaned: Instant::now(),
            drawn: Instant::now(),
            released: true,
            checker: skia_safe::RuntimeEffect::make_for_shader(CHECKER, None)
                .map_err(|error| log::warn!("native renderer: checker: {error}"))
                .ok(),
            grid: skia_safe::RuntimeEffect::make_for_shader(GRID_LINES, None)
                .and_then(|lines| skia_safe::RuntimeEffect::make_for_blender(GRID_COLOR, None).map(|color| (lines, color)))
                .map_err(|error| log::warn!("native renderer: grid: {error}"))
                .ok(),
            view: None,
            zoom: (0.0, Instant::now()),
            frame: 0,
            tint: false,
            dirty: true,
            requests,
        }
    }

    pub fn run(mut self, messages: Receiver<Msg>) {
        loop {
            let first = if self.dirty {
                match messages.try_recv() {
                    Ok(message) => Some(message),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(_) => return,
                }
            } else {
                match messages.recv_timeout(Duration::from_millis(100)) {
                    Ok(message) => Some(message),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            };
            let mut next = first;
            while let Some(message) = next {
                if !self.handle(message) {
                    return;
                }
                next = messages.try_recv().ok();
            }
            // Once the zoom settles, the tiles at its exact scale
            if !self.dirty && self.zoom.1.elapsed() < SETTLE * 3 && self.zoom.1.elapsed() >= SETTLE {
                self.dirty = true;
            }
            if self.dirty {
                self.dirty = false;
                self.present();
                self.drawn = Instant::now();
                self.released = false;
            } else if !self.released && self.drawn.elapsed() > IDLE {
                // Layers and scratch from drawing tiles stay with the GPU
                // driver for reuse until given back
                self.released = true;
                self.release();
            }
        }
    }

    /// False to stop
    fn handle(&mut self, message: Msg) -> bool {
        match message {
            Msg::Begin(scene) => {
                self.scene = *scene;
                self.pictures.clear();
                self.tiles.clear();
                self.kit.forget_patterns(None);
                self.rebuild_units();
                stats().lock().unwrap().nodes = 0;
                self.dirty = true;
            }
            Msg::Changes(changes) => {
                let before: HashMap<usize, Rect> = self.units.iter().map(|unit| (unit.slot, unit.bounds)).collect();
                let touched = self.scene.apply(changes);
                self.rebuild_units();
                let mut areas: Vec<Rect> = touched.iter().filter_map(|slot| before.get(slot).copied()).collect();
                areas.extend(self.units.iter().filter(|unit| touched.contains(&unit.slot)).map(|unit| unit.bounds));
                let scene = &self.scene;
                // Patterns whose source came, went or changed are out of
                // date wherever they're used
                let stale = |recorded: &Recorded| {
                    recorded.sources.iter().any(|(id, there)| match scene.by_id.get(id) {
                        Some(&source) => !there || touched.contains(&scene.unit_of(source)),
                        None => *there,
                    })
                };
                for unit in &self.units {
                    if self.pictures.iter().any(|(&(slot, _, _), recorded)| slot == unit.slot && stale(recorded)) {
                        areas.push(unit.bounds);
                    }
                }
                self.pictures.retain(|&(slot, _, _), recorded| scene.node(slot).is_some() && !touched.contains(&slot) && !stale(recorded));
                self.kit.forget_patterns(None);
                self.drop_tiles(&areas);
                stats().lock().unwrap().nodes = self.scene.live_nodes();
                self.dirty = true;
            }
            Msg::View(view) => {
                if self.view.as_ref() != Some(&view) {
                    self.view = Some(view);
                    self.dirty = true;
                }
            }
            Msg::Image(key, img) => {
                let waiting: Vec<(usize, bool, i32)> =
                    self.pictures.iter().filter(|(_, recorded)| recorded.missing.contains(&key)).map(|(&key, _)| key).collect();
                let areas: Vec<Rect> = waiting
                    .iter()
                    .filter_map(|&(slot, _, _)| self.units.iter().find(|unit| unit.slot == slot).map(|unit| unit.bounds))
                    .collect();
                for picture in waiting {
                    self.pictures.remove(&picture);
                }
                self.kit.forget_patterns(Some(&key));
                self.images.insert(key, img);
                self.drop_tiles(&areas);
                self.update_memory();
                self.dirty = true;
            }
            Msg::Resize(width, height) => {
                if (width, height) != self.gpu.size && width > 0 && height > 0 {
                    self.scratch = None;
                    if let Err(error) = self.gpu.resize(width, height) {
                        log::warn!("native renderer: {error}");
                    }
                    self.dirty = true;
                }
            }
            Msg::Offscreen { id, bounds, width, height, reply } => {
                let _ = reply.send(self.offscreen(&id, bounds, width, height));
                self.release();
            }
            Msg::Replay { views, reply } => {
                let _ = reply.send(self.replay(views));
                self.release();
                self.dirty = true;
            }
            Msg::Tint(on) => {
                self.tint = on;
                self.pictures.clear();
                self.tiles.clear();
                self.kit.forget_patterns(None);
                self.dirty = true;
            }
            Msg::Render { view, width, height, reply } => {
                let _ = reply.send(self.render_view(&view, width, height));
                self.release();
                self.dirty = true;
            }
            Msg::Live(reply) => {
                let _ = reply.send(self.live());
                self.release();
                self.dirty = true;
            }
            Msg::HitTest { points, reply } => {
                let _ = reply.send(points.iter().map(|&point| self.hit(point)).collect());
            }
            Msg::Purge(reply) => {
                let before = self.gpu.used();
                let _ = self.gpu.context.flush_submit_and_sync_cpu();
                self.gpu.context.purge_unlocked_resources(skia_safe::gpu::PurgeResourceOptions::AllResources);
                let purged = self.gpu.used();
                self.gpu.context.free_gpu_resources();
                let freed = self.gpu.used();
                let _ = reply.send([before, purged, freed]);
                self.dirty = true;
            }
            Msg::Quit => return false,
        }
        true
    }

    /// Gives back the GPU memory an offscreen render took, whole views drawn
    /// supersampled, which Skia would otherwise keep for reuse
    fn release(&mut self) {
        let _ = self.gpu.context.flush_submit_and_sync_cpu();
        self.gpu.context.free_gpu_resources();
        self.update_memory();
    }

    fn update_memory(&mut self) {
        let mut stats = stats().lock().unwrap();
        stats.images = self.images.len();
        stats.image_bytes = self.images.values().map(Img::bytes).sum();
        stats.tiles = self.tiles.len();
        stats.tile_bytes = self.tiles.len() * (TILE * TILE * 4) as usize;
        stats.cache_bytes = self.gpu.context.resource_cache_usage().resource_bytes;
        stats.vram_device = self.gpu.used();
    }

    fn drop_tiles(&mut self, areas: &[Rect]) {
        if areas.is_empty() {
            return;
        }
        for (key, tile) in self.tiles.iter_mut() {
            let area = key.area();
            if areas.iter().any(|other| Rect::intersects2(&area, other)) {
                tile.stale = true;
            }
        }
    }

    fn rebuild_units(&mut self) {
        let mut units = Vec::new();
        fn add(scene: &Scene, slot: usize, units: &mut Vec<Unit>) {
            let Some(node) = scene.node(slot) else {
                return;
            };
            if node.hidden {
                return;
            }
            let Some(bounds) = node.bounds else {
                return;
            };
            if node.kind == Kind::Section {
                units.push(Unit { slot, section: true, bounds });
                for (i, &child) in node.children.iter().enumerate() {
                    let mask = scene.node(child).filter(|child| !child.hidden && child.mask.is_some());
                    if mask.is_some() {
                        // A mask covers every layer after it, so they're
                        // drawn together
                        let rest = node.children[i..]
                            .iter()
                            .filter_map(|&later| scene.node(later).filter(|later| !later.hidden).and_then(|later| later.bounds))
                            .reduce(|a, b| Rect::join2(a, b));
                        if let Some(rest) = rest {
                            units.push(Unit { slot: child, section: true, bounds: rest });
                        }
                        break;
                    }
                    add(scene, child, units);
                }
            } else {
                units.push(Unit { slot, section: false, bounds });
            }
        }
        for &root in &self.scene.roots {
            add(&self.scene, root, &mut units);
        }
        self.units = units;
        let mut backdrops = Vec::new();
        fn blurs(scene: &Scene, slot: usize, out: &mut Vec<(Rect, f32)>) {
            let Some(node) = scene.node(slot).filter(|node| !node.hidden) else {
                return;
            };
            if let Some(bounds) = node.bounds {
                for effect in node.effects.iter().filter(|effect| effect.kind == "BACKGROUND_BLUR" && effect.radius > 0.0) {
                    // Three sigmas, sigma being half the radius
                    out.push((bounds, effect.radius as f32 * 1.5));
                }
            }
            for &child in &node.children {
                blurs(scene, child, out);
            }
        }
        for &root in &self.scene.roots {
            blurs(&self.scene, root, &mut backdrops);
        }
        self.backdrops = backdrops;
    }

    /// Pixels to draw around tile `key` so background blurs in it see what
    /// they blur: each blur over it reaches that far, and blurs over blurs
    /// add up
    fn margin(&self, key: TileKey) -> i32 {
        let area = key.area();
        let reach: f32 = self.backdrops.iter().filter(|(bounds, reach)| Rect::intersects2(bounds, &area.with_outset((*reach, *reach)))).map(|(_, reach)| reach).sum();
        if reach <= 0.0 {
            return 0;
        }
        // In steps of 32, so tiles share scratch surfaces
        (((reach * key.px()).ceil() as i32 + 31) / 32 * 32).min(MAX_MARGIN)
    }

    /// Unit `unit`'s picture for zoom `level`, recorded if it isn't yet
    fn picture(&mut self, unit: usize, level: i32) -> Option<&Recorded> {
        let (slot, section, bounds) = (self.units[unit].slot, self.units[unit].section, self.units[unit].bounds);
        let key = (slot, section, level);
        if !self.pictures.contains_key(&key) {
            let mut recorder = PictureRecorder::new();
            let canvas = recorder.begin_recording(bounds.with_outset((1.0, 1.0)), true);
            let mut encoder = Encoder::new(&self.scene, &self.images, &self.kit, canvas, 2f32.powi(level));
            encoder.tint = self.tint;
            encoder.aa = self.config.supersample < 4;
            if section && self.scene.node(slot).is_some_and(|node| node.kind == Kind::Section) {
                encoder.section(slot);
            } else if section {
                encoder.masked(slot);
            } else {
                encoder.node(slot);
            }
            let (counts, reason) = (encoder.counts, encoder.approximate_reason.take());
            let waiting = encoder.missing_images.len();
            let images: Vec<(String, f64)> = encoder.missing_images.drain(..).chain(encoder.sharper_images.drain(..)).collect();
            let sources = std::mem::take(&mut encoder.sources);
            drop(encoder);
            let picture = recorder.finish_recording_as_picture(None)?;
            self.ask(&images);
            let missing = images.into_iter().map(|(key, _)| key).collect();
            self.pictures.insert(key, Recorded { picture, counts, reason, missing, waiting, sources });
        }
        self.pictures.get(&key)
    }

    fn ask(&mut self, images: &[(String, f64)]) {
        for (key, width) in images {
            let asked = self.asked.get(key).copied().unwrap_or(0.0);
            if *width > asked * 1.9 {
                self.asked.insert(key.clone(), *width);
                let _ = self.requests.send(Request { key: key.clone(), width: *width });
            }
        }
    }

    /// Draws tile `key`: every unit over it, in order
    fn draw_tile(&mut self, key: TileKey) -> Option<Tile> {
        let area = key.area();
        let px = key.px();
        let level = px.log2().round().clamp(-12.0, 8.0) as i32;
        let margin = self.margin(key);
        let reach = area.with_outset((margin as f32 / px, margin as f32 / px));
        let units: Vec<usize> = (0..self.units.len()).filter(|&i| Rect::intersects2(&self.units[i].bounds, &reach)).collect();
        let mut counts = Counts::default();
        let mut pictures = Vec::new();
        let timing = super::dev();
        let start = Instant::now();
        for unit in units {
            // Too small to see: left out, as the encoder would
            let bounds = self.units[unit].bounds;
            if bounds.width().max(bounds.height()) * px < 0.5 {
                counts.skipped += 1;
                continue;
            }
            if let Some(recorded) = self.picture(unit, level) {
                counts += recorded.counts;
                pictures.push(recorded.picture.clone());
            }
        }
        let recorded = start.elapsed();
        let ss = self.config.supersample;
        if self.scratch.as_ref().is_none_or(|(kept, _)| *kept != margin) {
            let side = (TILE + margin * 2) * ss;
            self.scratch = Some((margin, self.gpu.surface(side, side, self.config.samples)?));
        }
        let (_, scratch) = self.scratch.as_mut()?;
        let canvas = scratch.canvas();
        canvas.clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
        canvas.save();
        canvas.translate((((margin - key.x * TILE) * ss) as f32, ((margin - key.y * TILE) * ss) as f32));
        canvas.scale((px * ss as f32, px * ss as f32));
        for picture in &pictures {
            canvas.draw_picture(picture, None, None);
        }
        canvas.restore();
        let played = start.elapsed();
        if timing {
            let _ = self.gpu.context.flush_and_submit_surface(scratch, Some(SyncCpu::Yes));
        }
        let source = skia_safe::IRect::from_xywh(margin * ss, margin * ss, TILE * ss, TILE * ss);
        let image = if ss == 1 {
            if margin == 0 {
                scratch.image_snapshot()
            } else {
                scratch.image_snapshot_with_bounds(source)?
            }
        } else {
            let drawn = scratch.image_snapshot();
            if self.down.is_none() {
                self.down = self.gpu.surface(TILE, TILE, 1);
            }
            let down = self.down.as_mut()?;
            shrink(down, &drawn, source, self.average.as_ref());
            down.image_snapshot()
        };
        // Margins are rare, so their bigger surface isn't kept
        if margin != 0 {
            self.scratch = None;
        }
        if timing {
            let gpu = start.elapsed();
            let mut stats = stats().lock().unwrap();
            let mix = |mean: &mut f64, ms: f64| *mean = if *mean == 0.0 { ms } else { *mean * 0.9 + ms * 0.1 };
            mix(&mut stats.tile_record_ms, recorded.as_secs_f64() * 1000.0);
            mix(&mut stats.tile_play_ms, (played - recorded).as_secs_f64() * 1000.0);
            mix(&mut stats.tile_gpu_ms, (gpu - played).as_secs_f64() * 1000.0);
            stats.tiles_drawn += 1;
        }
        Some(Tile { image, used: self.frame, counts, stale: false })
    }

    fn background(&self) -> Color4f {
        match self.scene.background {
            Some(bg) if !self.scene.transparent => Color4f::new(bg.r, bg.g, bg.b, 1.0),
            _ if self.scene.dark => Color4f::new(31.0 / 255.0, 31.0 / 255.0, 31.0 / 255.0, 1.0),
            _ => Color4f::new(1.0, 1.0, 1.0, 1.0),
        }
    }

    /// Figma's checkerboard for a page with its background hidden. It's
    /// anchored to the canvas, its squares 8 units wide at 100% and halved or
    /// doubled at each power of two of zoom, so they stay 5.7 to 11.3 points
    /// `origin` is the view's corner in device pixels, unrounded, as Figma
    /// places the pattern
    fn checker(&self, origin: (f64, f64), px: f32, zoom: f64) -> Option<Paint> {
        if !self.scene.transparent {
            return None;
        }
        let cell = 8.0 / 2f64.powf(zoom.max(1e-6).log2().round()) as f32;
        let (a, b) = if self.scene.dark { (0.12f32, 0.17f32) } else { (1.0, 0.95) };
        // Within one repeat, which keeps it exact in 32 bits far out
        let period = 2.0 * cell as f64 * px as f64;
        let origin = (origin.0.rem_euclid(period) as f32, origin.1.rem_euclid(period) as f32);
        let shader = self.checker.as_ref()?.make_shader(uniforms(&[origin.0, origin.1, px, cell, a, b]), &[], None)?;
        let mut paint = Paint::default();
        paint.set_shader(shader);
        Some(paint)
    }

    /// Figma's pixel grid, lines a device pixel wide at each unit once a unit
    /// is 8 device pixels or more, which lighten dark colors and darken light
    /// ones
    fn pixel_grid(&self, canvas: &skia_safe::Canvas, origin: (f64, f64), px: f32, width: i32, height: i32) {
        if px < 8.0 {
            return;
        }
        let origin = (origin.0.rem_euclid(px as f64) as f32, origin.1.rem_euclid(px as f64) as f32);
        let Some((lines, color)) = &self.grid else {
            return;
        };
        let Some(shader) = lines.make_shader(uniforms(&[origin.0, origin.1, px]), &[], None) else {
            return;
        };
        let Some(blender) = color.make_blender(Data::new_empty(), None) else {
            return;
        };
        let mut paint = Paint::default();
        paint.set_shader(shader);
        paint.set_blender(blender);
        canvas.draw_rect(Rect::from_iwh(width, height), &paint);
    }

    /// Draws `view` onto `target`, `width` by `height` pixels, drawing the
    /// tiles it's missing within `budget`. Returns what's drawn and whether
    /// tiles are still missing
    fn compose(&mut self, target: &mut Surface, view: &View, budget: Option<Duration>, settled: bool) -> (Counts, Option<String>, bool) {
        self.frame += 1;
        let (width, height) = (target.width(), target.height());
        let px = (width as f64 / view.w) as f32;
        // Tiles land on whole pixels, as Figma's do, which keeps them sharp
        let ox = (view.x * px as f64).round() as i32;
        let oy = (view.y * px as f64).round() as i32;
        let pxb = px.to_bits();
        let mut needed: Vec<TileKey> = Vec::new();
        for y in oy.div_euclid(TILE)..=(oy + height).div_euclid(TILE) {
            for x in ox.div_euclid(TILE)..=(ox + width).div_euclid(TILE) {
                needed.push(TileKey { px: pxb, x, y });
            }
        }
        // Nearest the middle first
        let (mx, my) = ((ox + width / 2) as f32 / TILE as f32, (oy + height / 2) as f32 / TILE as f32);
        needed.sort_by(|a, b| {
            let da = (a.x as f32 + 0.5 - mx).powi(2) + (a.y as f32 + 0.5 - my).powi(2);
            let db = (b.x as f32 + 0.5 - mx).powi(2) + (b.y as f32 + 0.5 - my).powi(2);
            da.total_cmp(&db)
        });
        let start = Instant::now();
        let mut pending = false;
        // Missing tiles first, then out of date ones, which show meanwhile
        let mut order: Vec<TileKey> = needed.iter().copied().filter(|key| !self.tiles.contains_key(key)).collect();
        order.extend(needed.iter().copied().filter(|key| self.tiles.get(key).is_some_and(|tile| tile.stale)));
        for key in order {
            // While zooming, older tiles stand in, scaled, if there are any
            let covered = || self.tiles.keys().any(|other| other.px != pxb && Rect::intersects2(&other.area(), &key.area()));
            let stale = self.tiles.contains_key(&key);
            if !settled && (stale || covered()) {
                pending = true;
                continue;
            }
            if budget.is_some_and(|budget| start.elapsed() >= budget) {
                pending = true;
                continue;
            }
            if let Some(tile) = self.draw_tile(key) {
                self.tiles.insert(key, tile);
            }
        }
        let checker = self.checker((view.x * px as f64, view.y * px as f64), px, view.zoom);
        let canvas = target.canvas();
        canvas.clear(self.background());
        if let Some(checker) = &checker {
            canvas.draw_paint(checker);
        }
        // Older tiles where this zoom's aren't ready, oldest zoom first
        let missing: Vec<TileKey> = needed.iter().copied().filter(|key| !self.tiles.contains_key(key)).collect();
        if !missing.is_empty() {
            let view_area = Rect::from_xywh(view.x as f32, view.y as f32, width as f32 / px, height as f32 / px);
            let mut older: Vec<(&TileKey, &Tile)> =
                self.tiles.iter().filter(|(key, _)| key.px != pxb && Rect::intersects2(&key.area(), &view_area)).collect();
            older.sort_by_key(|(_, tile)| tile.used);
            canvas.save();
            let mut holes = skia_safe::PathBuilder::new();
            for key in &missing {
                let rect = Rect::from_xywh((key.x * TILE - ox) as f32, (key.y * TILE - oy) as f32, TILE as f32, TILE as f32);
                holes.add_rect(rect, None, None);
            }
            canvas.clip_path(&holes.detach(), skia_safe::ClipOp::Intersect, false);
            let sampling = SamplingOptions::new(FilterMode::Linear, MipmapMode::None);
            for (key, tile) in older {
                let area = key.area();
                let dst = Rect::from_ltrb(
                    (area.left - view.x as f32) * px,
                    (area.top - view.y as f32) * px,
                    (area.right - view.x as f32) * px,
                    (area.bottom - view.y as f32) * px,
                );
                canvas.draw_image_rect_with_sampling_options(&tile.image, None, dst, sampling, &Paint::default());
            }
            canvas.restore();
        }
        let mut counts = Counts::default();
        let frame = self.frame;
        for key in &needed {
            if let Some(tile) = self.tiles.get_mut(key) {
                tile.used = frame;
                counts += tile.counts;
                let at = ((key.x * TILE - ox) as f32, (key.y * TILE - oy) as f32);
                canvas.draw_image(&tile.image, at, None);
            }
        }
        self.pixel_grid(canvas, (view.x * px as f64, view.y * px as f64), px, width, height);
        // The approximate reason of a unit in view, if any
        let view_area = Rect::from_xywh(view.x as f32, view.y as f32, width as f32 / px, height as f32 / px);
        let level = px.log2().round().clamp(-12.0, 8.0) as i32;
        let reason = self
            .units
            .iter()
            .filter(|unit| Rect::intersects2(&unit.bounds, &view_area))
            .find_map(|unit| self.pictures.get(&(unit.slot, unit.section, level)).and_then(|recorded| recorded.reason.clone()));
        self.evict();
        (counts, reason, pending || !missing.is_empty())
    }

    /// Keeps the most recently shown tiles
    fn evict(&mut self) {
        // Scratch layers and images not used for a while
        if self.cleaned.elapsed() > Duration::from_secs(2) {
            self.cleaned = Instant::now();
            self.gpu.context.perform_deferred_cleanup(Duration::from_secs(5), None);
        }
        let max = self.config.max_tiles;
        if self.tiles.len() <= max {
            return;
        }
        let mut used: Vec<u64> = self.tiles.values().map(|tile| tile.used).collect();
        used.sort_unstable();
        let cutoff = used[self.tiles.len() - max];
        self.tiles.retain(|_, tile| tile.used >= cutoff);
        // Pictures of zoom levels far from any tile kept go too
        let levels: Vec<i32> = self.tiles.keys().map(|key| key.px().log2().round() as i32).collect();
        self.pictures.retain(|&(_, _, level), _| levels.iter().any(|&kept| (kept - level).abs() <= 1));
    }

    fn present(&mut self) {
        let Some(view) = self.view.clone() else {
            return;
        };
        let start = Instant::now();
        let px = (self.gpu.size.0 as f64 / view.w) as f32;
        if px != self.zoom.0 {
            self.zoom = (px, Instant::now());
        }
        let settled = self.zoom.1.elapsed() >= SETTLE;
        let index = unsafe { self.gpu.swap_chain.GetCurrentBackBufferIndex() } as usize;
        let Some((mut surface, target)) = (index < self.gpu.buffers.len()).then(|| self.gpu.buffers.swap_remove(index)) else {
            return;
        };
        let (counts, reason, pending) = self.compose(&mut surface, &view, Some(BUILD_BUDGET), settled);
        let encoded = start.elapsed();
        if let Err(error) = self.gpu.context.flush_and_submit_surface(&mut surface, Some(SyncCpu::Yes)) {
            log::warn!("native renderer: {error:?}");
        }
        let rendered = start.elapsed();
        self.gpu.buffers.insert(index, (surface, target));
        let _ = unsafe { self.gpu.swap_chain.Present(1, DXGI_PRESENT(0)) };
        if pending {
            self.dirty = true;
        }
        self.update_memory();
        let mut stats = stats().lock().unwrap();
        stats.approximate_reason = reason;
        stats.record_frame(
            encoded.as_secs_f64() * 1000.0,
            rendered.as_secs_f64() * 1000.0,
            counts.native,
            counts.approximate,
            counts.skipped,
            view.zoom,
            pending,
        );
    }

    fn offscreen(&mut self, id: &str, bounds: Rect, width: u32, height: u32) -> Option<Offscreen> {
        let slot = *self.scene.by_id.get(id)?;
        let px = width as f32 / bounds.width().max(1e-6);
        // As tiles are drawn: supersampled, then scaled down
        let ss = self.config.supersample;
        let mut big = self.gpu.surface(width as i32 * ss, height as i32 * ss, self.config.samples)?;
        let canvas = big.canvas();
        canvas.clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
        canvas.scale((px * ss as f32, px * ss as f32));
        canvas.translate((-bounds.left, -bounds.top));
        let mut encoder = Encoder::new(&self.scene, &self.images, &self.kit, canvas, px);
        encoder.aa = ss < 4;
        encoder.node(slot);
        let reason = encoder.approximate_reason.take();
        let images: Vec<(String, f64)> = encoder.missing_images.drain(..).chain(encoder.sharper_images.drain(..)).collect();
        drop(encoder);
        let waiting = images.len();
        self.ask(&images);
        let drawn = big.image_snapshot();
        let mut surface = self.gpu.surface(width as i32, height as i32, 1)?;
        shrink(&mut surface, &drawn, skia_safe::IRect::from_wh(width as i32 * ss, height as i32 * ss), self.average.as_ref());
        let info = ImageInfo::new((width as i32, height as i32), ColorType::RGBA8888, AlphaType::Unpremul, None);
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        if !surface.canvas().read_pixels(&info, &mut pixels, (width * 4) as usize, (0, 0)) {
            return None;
        }
        Some(Offscreen { pixels, approximate_reason: reason, waiting })
    }

    fn render_view(&mut self, view: &View, width: u32, height: u32) -> Option<(Vec<u8>, usize)> {
        let mut target = self.gpu.surface(width as i32, height as i32, 1)?;
        self.compose(&mut target, view, None, true);
        let px = (width as f64 / view.w) as f32;
        let level = px.log2().round().clamp(-12.0, 8.0) as i32;
        let area = Rect::from_xywh(view.x as f32, view.y as f32, view.w as f32, view.h as f32);
        let waiting = self
            .units
            .iter()
            .filter(|unit| Rect::intersects2(&unit.bounds, &area))
            .filter_map(|unit| self.pictures.get(&(unit.slot, unit.section, level)))
            .map(|recorded| recorded.waiting)
            .sum();
        let info = ImageInfo::new((width as i32, height as i32), ColorType::RGBA8888, AlphaType::Unpremul, None);
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        if !target.canvas().read_pixels(&info, &mut pixels, (width * 4) as usize, (0, 0)) {
            return None;
        }
        Some((pixels, waiting))
    }

    fn live(&mut self) -> Option<(Vec<u8>, Vec<u8>, u32, u32)> {
        let view = self.view.clone()?;
        let (width, height) = (self.gpu.size.0, self.gpu.size.1);
        let read = |target: &mut Surface| {
            let info = ImageInfo::new((width as i32, height as i32), ColorType::RGBA8888, AlphaType::Unpremul, None);
            let mut pixels = vec![0u8; (width * height * 4) as usize];
            target.canvas().read_pixels(&info, &mut pixels, (width * 4) as usize, (0, 0)).then_some(pixels)
        };
        let mut target = self.gpu.surface(width as i32, height as i32, 1)?;
        self.compose(&mut target, &view, Some(Duration::ZERO), false);
        let shown = read(&mut target)?;
        self.tiles.clear();
        self.pictures.clear();
        self.kit.forget_patterns(None);
        self.compose(&mut target, &view, None, true);
        let fresh = read(&mut target)?;
        Some((shown, fresh, width, height))
    }

    /// The nodes under `point`, the topmost at each level
    fn hit(&self, point: (f32, f32)) -> Vec<String> {
        let mut path = Vec::new();
        let mut level: Vec<usize> = self.scene.roots.clone();
        loop {
            let found = level.iter().rev().copied().find(|&slot| {
                self.scene.node(slot).is_some_and(|node| !node.hidden && node.bounds.is_some_and(|bounds| { let (x, y) = point; x >= bounds.left && x <= bounds.right && y >= bounds.top && y <= bounds.bottom }))
            });
            let Some(slot) = found else {
                break;
            };
            let node = self.scene.node(slot).unwrap();
            path.push(node.id.clone());
            level = node.children.clone();
        }
        path
    }

    fn replay(&mut self, views: Vec<View>) -> Vec<(f64, f64)> {
        let Some(first) = views.first() else {
            return Vec::new();
        };
        let width = (first.cw * first.dpr).round().max(1.0) as i32;
        let height = (first.ch * first.dpr).round().max(1.0) as i32;
        let Some(mut target) = self.gpu.surface(width, height, 1) else {
            return Vec::new();
        };
        let time = |painter: &mut Self, target: &mut Surface| -> Vec<f64> {
            let mut last_px = 0.0;
            views
                .iter()
                .map(|view| {
                    let px = (width as f64 / view.w) as f32;
                    // As in the window: tiles at a new zoom only once it stops
                    let settled = px == last_px;
                    last_px = px;
                    let start = Instant::now();
                    painter.compose(target, view, Some(BUILD_BUDGET), settled);
                    let _ = painter.gpu.context.flush_and_submit_surface(target, Some(SyncCpu::Yes));
                    start.elapsed().as_secs_f64() * 1000.0
                })
                .collect()
        };
        self.tiles.clear();
        let cold = time(self, &mut target);
        let warm = time(self, &mut target);
        cold.into_iter().zip(warm).collect()
    }
}

/// Packs floats as a runtime effect's uniforms
fn uniforms(values: &[f32]) -> Data {
    let bytes: Vec<u8> = values.iter().flat_map(|value| value.to_le_bytes()).collect();
    Data::new_copy(&bytes)
}

/// The checkerboard at device pixel `p`: canvas point `(p + origin) / px`,
/// squares `cell` units wide, `a` where the cell's x and y differ in parity
const CHECKER: &str = "
uniform float2 origin;
uniform float px;
uniform float cell;
uniform float a;
uniform float b;
half4 main(float2 p) {
    float2 q = fract((p + origin) / px / (2.0 * cell)) - 0.5;
    half v = half(q.x * q.y < 0.0 ? a : b);
    return half4(v, v, v, 1.0);
}
";

/// Marks the device pixels on the pixel grid: the first after each unit
const GRID_LINES: &str = "
uniform float2 origin;
uniform float px;
half4 main(float2 p) {
    float2 f = fract((p + origin) / px);
    return (f.x < 1.0 / px || f.y < 1.0 / px) ? half4(1) : half4(0);
}
";

/// On the grid, a color's perceived lightness `l` (the root of its weighted
/// squares) goes to `l - 0.05` when light, and dark colors are mixed toward
/// white until it's `0.8 l + 0.15`
const GRID_COLOR: &str = "
half4 main(half4 src, half4 dst) {
    if (src.a == 0.0) {
        return dst;
    }
    float3 c = float3(dst.rgb);
    float3 w = float3(0.299, 0.587, 0.114);
    float k = dot(c * c, w);
    float l = sqrt(k);
    if (l > 0.5) {
        return half4(half3(c * (l - 0.05) / l), dst.a);
    }
    float target = l * 0.8 + 0.15;
    // |c + t (1 - c)|^2 weighted = target^2, solved for t
    float3 d = 1.0 - c;
    float qa = dot(d * d, w);
    float qb = 2.0 * dot(c * d, w);
    float qc = k - target * target;
    float t = qa > 0.0 ? (-qb + sqrt(max(qb * qb - 4.0 * qa * qc, 0.0))) / (2.0 * qa) : 0.0;
    return half4(half3(c + clamp(t, 0.0, 1.0) * d), dst.a);
}
";

/// Each pixel, the average of the 4 by 4 samples drawn for it
const AVERAGE: &str = "
uniform shader image;
uniform float2 origin;
half4 main(float2 p) {
    float2 base = origin + floor(p) * 4.0 + 0.5;
    float4 sum = float4(0);
    for (int y = 0; y < 4; y++) {
        for (int x = 0; x < 4; x++) {
            sum += float4(image.eval(base + float2(float(x), float(y))));
        }
    }
    return half4(sum / 16.0);
}
";

/// Draws `source` of `image` over all of `target`, scaled down: exactly
/// averaged when it's 4 times bigger, otherwise filtered
fn shrink(target: &mut Surface, image: &Image, source: skia_safe::IRect, average: Option<&skia_safe::RuntimeEffect>) {
    let (width, height) = (target.width(), target.height());
    let canvas = target.canvas();
    canvas.clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
    let mut paint = Paint::default();
    paint.set_blend_mode(skia_safe::BlendMode::Src);
    if let (Some(average), true) = (average, source.width() == width * 4) {
        let samples = image.to_shader(None, SamplingOptions::new(FilterMode::Nearest, MipmapMode::None), None);
        let mut uniforms = Vec::with_capacity(8);
        for value in [source.left as f32, source.top as f32] {
            uniforms.extend_from_slice(&value.to_le_bytes());
        }
        if let Some(shader) = samples.and_then(|samples| average.make_shader(Data::new_copy(&uniforms), &[samples.into()], None)) {
            paint.set_shader(shader);
            canvas.draw_rect(Rect::from_iwh(width, height), &paint);
            return;
        }
    }
    let mipmap = if source.width() > width * 2 { MipmapMode::Linear } else { MipmapMode::None };
    canvas.draw_image_rect_with_sampling_options(
        image,
        Some((&Rect::from(source), skia_safe::canvas::SrcRectConstraint::Strict)),
        Rect::from_iwh(width, height),
        SamplingOptions::new(FilterMode::Linear, mipmap),
        &paint,
    );
}
