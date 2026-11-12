//! The render thread: Skia on D3D12, drawing the mirrored scene for the
//! native renderer's window as Figma's viewport moves. It hands what it draws
//! to the present thread (see present.rs), which shows it, so however long
//! drawing takes the window follows the view every frame
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

use std::sync::Arc;

use skia_safe::gpu::d3d::BackendContext;
use skia_safe::gpu::{self, BackendTexture, DirectContext, Protected, SurfaceOrigin, SyncCpu};
use skia_safe::{
    AlphaType, Color4f, ColorType, Data, FilterMode, Image, ImageInfo, MipmapMode, Paint, Picture,
    PictureRecorder, Rect, SamplingOptions, Surface,
};
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{D3D12CreateDevice, ID3D12Device};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIAdapter3, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIFactory4, IDXGIFactory6, DXGI_ADAPTER_FLAG, DXGI_ADAPTER_FLAG_SOFTWARE,
    DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
};

use super::draw::{Counts, Encoder, Img, Kit, RENDER};
use super::present::{checker_paint, grid_paint, Device, Hub, Layer, Tex, CHECKER, GRID_COLOR, GRID_LINES};
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
    /// Image `key` couldn't be had, so it's asked for again later
    Failed(String),
    Resize(u32, u32),
    /// Draw node `id` alone, offscreen, over `bounds` at `width` by
    /// `height`, and send back its pixels, RGBA with straight alpha. With
    /// `backdrop`, over everything drawn before it
    Offscreen { id: String, bounds: Rect, width: u32, height: u32, backdrop: bool, reply: Sender<Option<Offscreen>> },
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
    /// What the window shows right now, as it would be drawn
    Shown(Sender<Option<(Vec<u8>, u32, u32)>>),
    /// See `Renderer::profile`
    Profile { id: String, x: f32, y: f32, px: f32, ss: i32, reply: Sender<String> },
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

/// In a dev build with LIGMA_NATIVE_TRACE=1, frame and tile timings are
/// written to ligma-trace.txt in the temp folder
fn tracing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| super::dev() && std::env::var("LIGMA_NATIVE_TRACE").is_ok_and(|value| value == "1"))
}

pub(super) fn trace(line: impl FnOnce() -> String) {
    if tracing() {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let at = START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0;
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("ligma-trace.txt")) {
            let _ = writeln!(file, "{at:9.1} {}", line());
        }
    }
}

/// In a dev build with LIGMA_NATIVE_FRAC=1, the view is drawn where it is
/// to a fraction of a pixel, as Figma draws it, rather than snapped to whole
/// pixels, so audits compare how things are drawn, not where. Tiles then
/// can't be reused once the view moves by part of a pixel
fn fractional() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| super::dev() && std::env::var("LIGMA_NATIVE_FRAC").is_ok_and(|value| value == "1"))
}

struct Unit {
    slot: usize,
    /// A section's own drawing, without what's in it, or, for a mask in a
    /// section, the section's children from it on, which it masks
    section: bool,
    bounds: Rect,
    /// Has a node in it drawn as Figma renders it, which its exact tiles
    /// want at their exact zoom
    custom: bool,
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
    /// Images it drew, and how wide each needs keeping for it
    uses: Vec<(String, f64)>,
}

const TILE: i32 = 512;

/// Textures kept to use again once the present thread is done with them
const SPARE: usize = 12;

/// The whole view, see `Painter::draw_whole`
struct Whole {
    layer: Layer,
    /// Its corner in device pixels and its zoom, as bits
    at: (i32, i32, u32),
    /// Something in it changed since
    stale: bool,
}

/// GPU memory used past which, once idle, image textures are packed again.
/// Windows counts it in steps of 256 MiB, so past one step is past 300 MB
const COMPACT_ABOVE: u64 = 300_000_000;

/// An image on the GPU once, with its mipmaps, and outside Skia's cache:
/// left to the cache, layers drawn in a frame push images out, and each is
/// uploaded and its mipmaps made again next frame
fn upload(context: &mut skia_safe::gpu::DirectContext, image: Image) -> Image {
    skia_safe::gpu::images::texture_from_image(context, &image, skia_safe::gpu::Mipmapped::Yes, skia_safe::gpu::Budgeted::No).unwrap_or(image)
}

/// Images past this many bytes go once nothing drawn uses them
const IMAGE_BUDGET: usize = 64 << 20;

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
        max_tiles: get("LIGMA_NATIVE_TILES", 40),
        cache_limit: get("LIGMA_NATIVE_CACHE_MB", 96) << 20,
    }
}

/// Widest margin a tile is drawn with, in pixels
const MAX_MARGIN: i32 = 256;
/// Zoomed in past this many pixels a unit, exact tiles take one sample a
/// pixel with smoothed edges: 16 samples would look no different there,
/// but cost 16 times as much, which with big blurs is hundreds of
/// milliseconds a tile
const SUPERSAMPLE_UNTIL: f32 = 2.0;
/// Time to spend per frame drawing tiles not ready yet, so a newly shown
/// part of the page fills in over a few frames rather than stalling one
const BUILD_BUDGET: Duration = Duration::from_millis(10);
/// Frames a quick tile of another zoom is kept after it was last shown
const PASSING: u64 = 30;
/// Picture levels from this on are exact zooms, in steps of 1/EXACT_STEPS
/// of a pixel a unit
const EXACT_LEVEL: i32 = 1 << 24;
const EXACT_STEPS: f32 = 4096.0;

/// How far a render's scale may be from the one wanted, as a share, before
/// Figma is asked for another. Each takes Figma's whole renderer a while, so
/// asking at every zoom passed through would slow Figma right down
const EXACT_TOLERANCE: f32 = 0.25;

/// Whether unit `unit` draws a node as Figma renders it
fn custom_unit(scene: &Scene, unit: &Unit) -> bool {
    fn any(scene: &Scene, slot: usize) -> bool {
        scene.node(slot).is_some_and(|node| !node.hidden && (node.custom() || node.children.iter().any(|&child| any(scene, child))))
    }
    let Some(node) = scene.node(unit.slot) else {
        return false;
    };
    if !unit.section {
        return any(scene, unit.slot);
    }
    if node.kind == Kind::Section {
        return node.custom();
    }
    // A mask and the section's children after it
    let Some(parent) = node.parent.and_then(|parent| scene.node(parent)) else {
        return any(scene, unit.slot);
    };
    parent.children.iter().skip_while(|&&child| child != unit.slot).any(|&child| any(scene, child))
}

/// Pixels per unit of picture level `level`
fn level_px(level: i32) -> f32 {
    if level >= EXACT_LEVEL {
        (level - EXACT_LEVEL) as f32 / EXACT_STEPS
    } else {
        2f32.powi(level)
    }
}

/// How long the view stays still before quick tiles are drawn exactly
const REFINE_AFTER: Duration = Duration::from_millis(250);
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
    /// As this context's image of `tex`
    image: Image,
    tex: Arc<Tex>,
    used: u64,
    counts: Counts,
    /// Out of date, shown until it's drawn again
    stale: bool,
    /// Drawn at full quality, sampled as Figma samples, rather than quickly
    /// to fill the view while it moves
    exact: bool,
}

pub struct Gpu {
    context: DirectContext,
    device: ID3D12Device,
    /// The window's size in pixels
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

impl Device {
    pub fn open() -> Result<Self, String> {
        let factory: IDXGIFactory4 = unsafe { CreateDXGIFactory1() }.map_err(|error| format!("DXGI: {error}"))?;
        let (adapter, device) = hardware_adapter(&factory).map_err(|error| format!("adapter: {error}"))?;
        let desc = unsafe { adapter.GetDesc1() }.map_err(|error| error.to_string())?;
        let name = String::from_utf16_lossy(&desc.Description).trim_end_matches('\0').to_string();
        Ok(Device { factory, adapter, device, name })
    }
}

impl Gpu {
    pub fn new(device: &Device, width: u32, height: u32) -> Result<Self, String> {
        let queue = unsafe { device.device.CreateCommandQueue(&Default::default()) }.map_err(|error| format!("queue: {error}"))?;
        let memory = device.adapter.cast::<IDXGIAdapter3>().ok();
        let backend = BackendContext { adapter: device.adapter.clone(), device: device.device.clone(), queue, memory_allocator: None, protected_context: Protected::No };
        let mut context = unsafe { gpu::direct_contexts::make_d3d(&backend, None) }.ok_or("Skia couldn't start on D3D12")?;
        context.set_resource_cache_limit(config().cache_limit);
        let gpu = Self {
            context,
            device: device.device.clone(),
            size: (width.max(1), height.max(1)),
            adapter: format!("{} (D3D12)", device.name),
            memory,
        };
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

    fn resize(&mut self, width: u32, height: u32) {
        self.size = (width.max(1), height.max(1));
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
    /// By unit, zoom level and whether edges are anti-aliased
    pictures: HashMap<(usize, bool, i32, bool), Recorded>,
    images: HashMap<String, Img>,
    /// Widths images were asked for at
    asked: HashMap<String, f64>,
    /// Images that couldn't be had, and when
    failed: HashMap<String, Instant>,
    tiles: HashMap<TileKey, Tile>,
    /// Where tiles are drawn before they're kept, by the margin around the
    /// tile and how many samples a pixel takes along each side
    scratch: Vec<(i32, i32, Surface)>,
    /// Tiles drawn, to time some of them
    made: u64,
    /// Textures handed over that the present thread may still show, used
    /// again once it's done with them
    spare: Vec<Arc<Tex>>,
    /// The whole view drawn for the present thread while the zoom changes or
    /// before its tiles are drawn
    whole: Option<Whole>,
    /// What a texture just drawn is read into once, which leaves it ready to
    /// be read
    touch: Option<Surface>,
    hub: Arc<Hub>,
    /// When the view was last drawn, and whether anything was since
    /// Part of a pixel the view is drawn off the pixel grid by, see `fractional`
    frac: (f32, f32),
    /// GPU memory used after images were last packed again
    compacted: u64,
    /// An image came since the last frame, for the dev timeline
    fresh: bool,
    /// When images were last fitted to what's drawn
    fitted: Instant,
    /// Where the view is drawn live with a margin, and the view and
    /// pictures it shows
    live: Option<(Surface, Option<(i32, i32, u32, i32, Vec<u32>)>)>,
    /// Averages each 4 by 4 block of samples into a pixel
    average: Option<skia_safe::RuntimeEffect>,
    /// Background blurs, by their absolute bounds and how far they reach
    backdrops: Vec<(Rect, f32)>,
    config: Config,
    cleaned: Instant,
    /// When a frame was last drawn, and whether memory was given back since
    drawn: Instant,
    released: bool,
    drew: bool,
    /// Figma's checkerboard, and the pixel grid it draws when zoomed in
    checker: Option<skia_safe::RuntimeEffect>,
    grid: Option<(skia_safe::RuntimeEffect, skia_safe::RuntimeEffect)>,
    view: Option<View>,
    /// The last zoom (pixels per unit) and when it changed
    zoom: (f32, Instant),
    /// When the view last moved
    moved: Instant,
    frame: u64,
    tint: bool,
    dirty: bool,
    requests: Sender<Request>,
}

impl Painter {
    pub fn new(gpu: Gpu, requests: Sender<Request>, hub: Arc<Hub>) -> Self {
        Self {
            gpu,
            kit: Kit::new(),
            scene: Scene::default(),
            units: Vec::new(),
            pictures: HashMap::new(),
            images: HashMap::new(),
            asked: HashMap::new(),
            failed: HashMap::new(),
            tiles: HashMap::new(),
            scratch: Vec::new(),
            made: 0,
            spare: Vec::new(),
            whole: None,
            touch: None,
            hub,
            drew: false,
            live: None,
            fitted: Instant::now(),
            compacted: 0,
            fresh: false,
            frac: (0.0, 0.0),
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
            moved: Instant::now(),
            frame: 0,
            tint: false,
            dirty: true,
            requests,
        }
    }

    pub fn run(mut self, messages: Receiver<Msg>) {
        // How long until there's more to draw, if there is
        let mut next: Option<Duration> = Some(Duration::ZERO);
        loop {
            let wait = if self.dirty { Duration::ZERO } else { next.unwrap_or(Duration::from_millis(100)).min(Duration::from_millis(100)) };
            let first = if wait.is_zero() {
                match messages.try_recv() {
                    Ok(message) => Some(message),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(_) => return,
                }
            } else {
                match messages.recv_timeout(wait) {
                    Ok(message) => Some(message),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            };
            let mut message = first;
            while let Some(one) = message {
                if !self.handle(one) {
                    return;
                }
                message = messages.try_recv().ok();
            }
            let changed = std::mem::take(&mut self.dirty);
            next = self.fill();
            if changed || self.dirty {
                self.publish();
            }
            if std::mem::take(&mut self.drew) {
                self.drawn = Instant::now();
                self.released = false;
                // Images sized for where the view was go while it moves on,
                // not only once it stops
                if self.zoom.1.elapsed() >= SETTLE && self.fitted.elapsed() > Duration::from_secs(1) {
                    self.fitted = Instant::now();
                    self.fit_images();
                }
            } else if !self.released && self.drawn.elapsed() > IDLE {
                // Layers and scratch from drawing tiles stay with the GPU
                // driver for reuse until given back
                self.released = true;
                self.release();
            }
            self.dirty = next == Some(Duration::ZERO);
        }
    }

    /// The tiles of `px` (as bits) over the window's `width` by `height`
    /// pixels at (`ox`, `oy`), and `ring` tiles around, nearest the middle
    /// first
    fn tiles_over(ox: i32, oy: i32, width: i32, height: i32, px: u32, ring: i32) -> Vec<TileKey> {
        let mut keys = Vec::new();
        for y in oy.div_euclid(TILE) - ring..=(oy + height).div_euclid(TILE) + ring {
            for x in ox.div_euclid(TILE) - ring..=(ox + width).div_euclid(TILE) + ring {
                keys.push(TileKey { px, x, y });
            }
        }
        let (mx, my) = ((ox + width / 2) as f32 / TILE as f32, (oy + height / 2) as f32 / TILE as f32);
        keys.sort_by(|a, b| {
            let da = (a.x as f32 + 0.5 - mx).powi(2) + (a.y as f32 + 0.5 - my).powi(2);
            let db = (b.x as f32 + 0.5 - mx).powi(2) + (b.y as f32 + 0.5 - my).powi(2);
            da.total_cmp(&db)
        });
        keys
    }

    /// Draws what the view needs next, for the present thread to show: the
    /// whole view while the zoom changes or when most of it is missing,
    /// otherwise the tile it needs most. Tiles in view come first, quickly,
    /// then those around it, so panning finds them ready, then exactly once
    /// the view has been still a moment. Returns how long until there's
    /// more to draw, if there is
    fn fill(&mut self) -> Option<Duration> {
        let view = self.view.clone()?;
        let (width, height) = (self.gpu.size.0 as i32, self.gpu.size.1 as i32);
        let px = (width as f64 / view.w) as f32;
        if !px.is_finite() || px <= 0.0 {
            return None;
        }
        if px != self.zoom.0 {
            self.zoom = (px, Instant::now());
        }
        self.frame += 1;
        let pxb = px.to_bits();
        let ox = (view.x * px as f64).round() as i32;
        let oy = (view.y * px as f64).round() as i32;
        let in_view = Self::tiles_over(ox, oy, width, height, pxb, 0);
        let mut around: Vec<TileKey> = Self::tiles_over(ox, oy, width, height, pxb, 1).into_iter().filter(|key| !in_view.contains(key)).collect();
        if in_view.len() + around.len() > self.config.max_tiles {
            around.clear();
        }
        let frame = self.frame;
        for key in in_view.iter().chain(&around) {
            if let Some(tile) = self.tiles.get_mut(key) {
                tile.used = frame;
            }
        }
        let whole_here = self.whole.as_ref().is_some_and(|whole| whole.at == (ox, oy, pxb) && !whole.stale);
        let missing = in_view.iter().filter(|key| !self.tiles.contains_key(key)).count();
        // While the zoom changes, tiles of it are of no use a moment later,
        // so the whole view is drawn instead, as Figma draws every frame
        let settle = SETTLE.saturating_sub(self.zoom.1.elapsed());
        if !settle.is_zero() || missing > 4 {
            if !whole_here {
                self.draw_whole(ox, oy, px);
                self.dirty = true;
                return Some(Duration::ZERO);
            }
            if !settle.is_zero() {
                return Some(settle);
            }
        }
        let mut order: Vec<(TileKey, bool)> = in_view.iter().copied().filter(|key| !self.tiles.contains_key(key)).map(|key| (key, false)).collect();
        // An out of date tile still shows, so it's drawn again as well as
        // it was, rather than quickly, which would blur it a while
        order.extend(in_view.iter().filter_map(|key| self.tiles.get(key).filter(|tile| tile.stale).map(|tile| (*key, tile.exact))));
        order.extend(around.iter().copied().filter(|key| self.tiles.get(key).is_none_or(|tile| tile.stale)).map(|key| (key, false)));
        let rough: Vec<TileKey> = in_view.iter().copied().filter(|key| self.tiles.get(key).is_some_and(|tile| !tile.stale && !tile.exact)).collect();
        let refine = REFINE_AFTER.saturating_sub(self.moved.elapsed());
        if refine.is_zero() {
            order.extend(rough.iter().map(|&key| (key, true)));
        }
        if let Some(&(key, exact)) = order.first() {
            if let Some(tile) = self.draw_tile(key, exact) {
                if let Some(old) = self.tiles.insert(key, tile) {
                    self.retire(old.tex);
                }
                self.drew = true;
            }
            self.evict(pxb);
            self.dirty = true;
            return Some(Duration::ZERO);
        }
        // All there: the whole view drawn for the meantime can go
        if missing == 0 {
            if let Some(whole) = self.whole.take() {
                self.retire(whole.layer.tex);
                self.dirty = true;
            }
        }
        self.update_reason(&view, px);
        (!rough.is_empty()).then_some(refine)
    }

    /// Draws the whole view, (`ox`, `oy`) its corner in device pixels at
    /// `px` a unit, with a margin for background blurs near its edges to
    /// see what they blur, for the present thread to show where tiles of the
    /// zoom aren't drawn yet
    fn draw_whole(&mut self, ox: i32, oy: i32, px: f32) {
        let (width, height) = (self.gpu.size.0 as i32, self.gpu.size.1 as i32);
        let view_area = Rect::from_xywh(ox as f32 / px, oy as f32 / px, width as f32 / px, height as f32 / px);
        let margin = self.reach(view_area, px);
        let reach = view_area.with_outset((margin as f32 / px, margin as f32 / px));
        let mut pictures = Vec::new();
        let mut counts = Counts::default();
        for unit in 0..self.units.len() {
            let bounds = self.units[unit].bounds;
            if !Rect::intersects2(&bounds, &reach) {
                continue;
            }
            if bounds.width().max(bounds.height()) * px < 0.5 {
                counts.skipped += 1;
                continue;
            }
            let level = self.level(&self.units[unit], px, false);
            if let Some(recorded) = self.picture(unit, level, true) {
                counts += recorded.counts;
                pictures.push(recorded.picture.clone());
            }
        }
        let start = Instant::now();
        let Some(tex) = self.tex(width + margin * 2, height + margin * 2) else {
            return;
        };
        let backend = tex.backend();
        let Some(mut surface) = gpu::surfaces::wrap_backend_texture(&mut self.gpu.context, &backend, SurfaceOrigin::TopLeft, 1, ColorType::RGBA8888, None, None) else {
            return;
        };
        let canvas = surface.canvas();
        canvas.clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
        canvas.translate(((margin - ox) as f32, (margin - oy) as f32));
        canvas.scale((px, px));
        for picture in &pictures {
            canvas.draw_picture(picture, None, None);
        }
        let image = self.finish(&backend);
        drop(surface);
        if image.is_none() {
            return;
        }
        if let Some(old) = self.whole.take() {
            self.retire(old.layer.tex);
        }
        trace(|| format!("  whole px={px:.3} pictures={} margin={margin} {:.1}ms", pictures.len(), start.elapsed().as_secs_f64() * 1000.0));
        self.whole = Some(Whole { layer: Layer { tex, x: ox - margin, y: oy - margin, px, counts }, at: (ox, oy, px.to_bits()), stale: false });
        self.drew = true;
    }

    /// A texture `width` by `height` to hand over, one the present thread is
    /// done with if there is one
    fn tex(&mut self, width: i32, height: i32) -> Option<Arc<Tex>> {
        let free = self.spare.iter().position(|tex| tex.width == width && tex.height == height && Arc::strong_count(tex) == 1);
        if let Some(free) = free {
            std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
            return Some(self.spare.swap_remove(free));
        }
        Tex::new(&self.gpu.device, width, height)
    }

    /// Keeps `tex` to use again once the present thread is done with it
    fn retire(&mut self, tex: Arc<Tex>) {
        self.spare.push(tex);
        if self.spare.len() > SPARE {
            let index = self.spare.iter().position(|tex| Arc::strong_count(tex) == 1).unwrap_or(0);
            self.spare.remove(index);
        }
    }

    fn clear_tiles(&mut self) {
        let tiles: Vec<Tile> = self.tiles.drain().map(|(_, tile)| tile).collect();
        for tile in tiles {
            self.retire(tile.tex);
        }
        if let Some(whole) = self.whole.take() {
            self.retire(whole.layer.tex);
        }
    }

    /// Leaves a texture just drawn into through `backend` ready to be read,
    /// and returns it as this context's image
    fn finish(&mut self, backend: &BackendTexture) -> Option<Image> {
        // Drawn first, as Skia wouldn't know to wait for it when it's read
        // through another of its wrappers
        let _ = self.gpu.context.flush_submit_and_sync_cpu();
        let image = gpu::images::borrow_texture_from(&mut self.gpu.context, backend, SurfaceOrigin::TopLeft, ColorType::RGBA8888, AlphaType::Premul, None)?;
        if self.touch.is_none() {
            self.touch = self.gpu.surface(1, 1, 1);
        }
        let touch = self.touch.as_mut()?;
        let one = Rect::from_wh(1.0, 1.0);
        touch.canvas().draw_image_rect(&image, Some((&one, skia_safe::canvas::SrcRectConstraint::Strict)), one, &Paint::default());
        let _ = self.gpu.context.flush_submit_and_sync_cpu();
        Some(image)
    }

    /// Hands the present thread what's drawn
    fn publish(&mut self) {
        let background = self.background();
        let mut shown = self.hub.shown.lock().unwrap();
        shown.generation += 1;
        shown.tiles = self
            .tiles
            .iter()
            .map(|(key, tile)| Layer { tex: tile.tex.clone(), x: key.x * TILE, y: key.y * TILE, px: key.px(), counts: tile.counts })
            .collect();
        shown.live = self.whole.as_ref().map(|whole| whole.layer.clone());
        shown.background = Some(background);
        shown.transparent = self.scene.transparent;
        shown.dark = self.scene.dark;
        shown.pending = self.dirty;
        drop(shown);
        self.hub.poke();
        if std::mem::take(&mut self.fresh) {
            super::mark(|| "drew new images".into());
        }
        self.update_memory();
    }

    /// The approximate reason of a unit in view, if any, for the overlay
    fn update_reason(&mut self, view: &View, px: f32) {
        let (width, height) = (self.gpu.size.0 as f32, self.gpu.size.1 as f32);
        let view_area = Rect::from_xywh(view.x as f32, view.y as f32, width / px, height / px);
        let aa = self.supersample(px) < 4;
        let reason = self
            .units
            .iter()
            .filter(|unit| Rect::intersects2(&unit.bounds, &view_area))
            .find_map(|unit| self.pictures.get(&(unit.slot, unit.section, self.level(unit, px, true), aa)).and_then(|recorded| recorded.reason.clone()));
        stats().lock().unwrap().approximate_reason = reason;
    }

    /// False to stop
    fn handle(&mut self, message: Msg) -> bool {
        match message {
            Msg::Begin(scene) => {
                self.scene = *scene;
                self.pictures.clear();
                self.clear_tiles();
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
                    if self.pictures.iter().any(|(&(slot, _, _, _), recorded)| slot == unit.slot && stale(recorded)) {
                        areas.push(unit.bounds);
                    }
                }
                self.pictures.retain(|&(slot, _, _, _), recorded| scene.node(slot).is_some() && !touched.contains(&slot) && !stale(recorded));
                // Figma's renders of nodes that changed are asked for again,
                // and shown until they come
                self.images.retain(|key, img| {
                    let Some(id) = key.strip_prefix(RENDER) else {
                        return true;
                    };
                    let Some(&slot) = scene.by_id.get(id) else {
                        self.asked.remove(key);
                        return false;
                    };
                    if touched.contains(&scene.unit_of(slot)) {
                        img.stale = true;
                        self.asked.remove(key);
                    }
                    true
                });
                self.kit.forget_patterns(None);
                trace(|| format!("changes touched={} areas={}", touched.len(), areas.len()));
                self.drop_tiles(&areas);
                stats().lock().unwrap().nodes = self.scene.live_nodes();
                self.dirty = true;
            }
            Msg::View(view) => {
                if self.view.as_ref() != Some(&view) {
                    self.view = Some(view);
                    self.moved = Instant::now();
                    self.dirty = true;
                }
            }
            Msg::Failed(key) => {
                self.asked.remove(&key);
                self.failed.insert(key, Instant::now());
            }
            Msg::Image(key, img) => {
                self.failed.remove(&key);
                // Asked for twice, by pictures wanting it at sizes that came
                // to the same: what's drawn with it is right as it is
                let same = self.images.get(&key).is_some_and(|kept| {
                    !kept.stale && kept.area == img.area && (kept.image.width(), kept.image.height()) == (img.image.width(), img.image.height())
                });
                if same && !key.starts_with(RENDER) {
                    return true;
                }
                let waiting: Vec<(usize, bool, i32, bool)> =
                    self.pictures.iter().filter(|(_, recorded)| recorded.missing.contains(&key)).map(|(&key, _)| key).collect();
                let areas: Vec<Rect> = waiting
                    .iter()
                    .filter_map(|&(slot, _, _, _)| self.units.iter().find(|unit| unit.slot == slot).map(|unit| unit.bounds))
                    .collect();
                for picture in waiting {
                    self.pictures.remove(&picture);
                }
                // Pictures holding it at another size let that go, drawn
                // again when next needed
                self.pictures.retain(|_, recorded| !recorded.uses.iter().any(|(used, _)| *used == key));
                self.kit.forget_patterns(Some(&key));
                let mut img = img;
                img.image = upload(&mut self.gpu.context, img.image);
                trace(|| format!("image {key} {}x{} areas={}", img.image.width(), img.image.height(), areas.len()));
                self.images.insert(key, img);
                self.drop_tiles(&areas);
                self.fresh = true;
                self.update_memory();
                self.dirty = true;
            }
            Msg::Resize(width, height) => {
                if (width, height) != self.gpu.size && width > 0 && height > 0 {
                    self.scratch.clear();
                    self.gpu.resize(width, height);
                    self.dirty = true;
                }
            }
            Msg::Offscreen { id, bounds, width, height, backdrop, reply } => {
                let _ = reply.send(self.offscreen(&id, bounds, width, height, backdrop));
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
                self.clear_tiles();
                self.kit.forget_patterns(None);
                self.dirty = true;
            }
            Msg::Render { view, width, height, reply } => {
                let _ = reply.send(self.render_view(&view, width, height));
                self.release();
                self.dirty = true;
            }
            Msg::Shown(reply) => {
                let _ = reply.send(self.shown());
            }
            Msg::Profile { id, x, y, px, ss, reply } => {
                let _ = reply.send(self.profile(&id, x, y, px, ss));
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
        self.fit_images();
        // Surfaces drawn on are made again when next needed
        self.live = None;
        self.scratch.clear();
        self.touch = None;
        self.compact();
        let _ = self.gpu.context.flush_submit_and_sync_cpu();
        self.gpu.context.free_gpu_resources();
        self.update_memory();
    }

    /// Keeps images about as big as what's drawn from them needs: ones kept
    /// much bigger are decoded again smaller, from the bytes kept, and past
    /// `IMAGE_BUDGET` the biggest no picture uses are let go
    fn fit_images(&mut self) {
        // As run.rs sizes images it fetches
        let fitted = |(w, h): (u32, u32), width: f64| {
            let side = (w.max(h) as f64 * (width.max(1.0) * 1.25 / w.max(1) as f64)).ceil();
            w.max(h) >> super::run::halvings(w.max(h), side)
        };
        // Pictures of other zooms are quick to record again, and hold on to
        // images at the size those zooms took
        let px = self.zoom.0;
        if px > 0.0 {
            let current = px.log2().round() as i32;
            let exact = EXACT_LEVEL + (px * EXACT_STEPS).round() as i32;
            self.pictures.retain(|&(_, _, level, _), _| if level >= EXACT_LEVEL { level == exact } else { (level - current).abs() <= 1 });
        }
        // What's near the view needs its images; elsewhere they can go, as
        // the page's encoded copy is kept, so they come back quickly
        let near: Option<Rect> = self.view.as_ref().map(|view| {
            let area = Rect::from_xywh(view.x as f32, view.y as f32, view.w as f32, view.h as f32);
            area.with_outset((area.width(), area.height()))
        });
        let near_units: std::collections::HashSet<(usize, bool)> = self
            .units
            .iter()
            .filter(|unit| near.is_none_or(|near| Rect::intersects2(&unit.bounds, &near)))
            .map(|unit| (unit.slot, unit.section))
            .collect();
        let mut need: HashMap<&str, f64> = HashMap::new();
        for (&(slot, section, _, _), recorded) in &self.pictures {
            if !near_units.contains(&(slot, section)) {
                continue;
            }
            for (key, width) in &recorded.uses {
                let entry = need.entry(key.as_str()).or_insert(0.0);
                *entry = entry.max(*width);
            }
        }
        let mut smaller = Vec::new();
        let mut unused = Vec::new();
        for (key, img) in &self.images {
            if key.starts_with(RENDER) {
                continue;
            }
            let kept = img.image.width().max(img.image.height()) as f64;
            match need.get(key.as_str()) {
                // Smaller where what the page would send for it now is
                Some(&width) if kept > fitted(img.original, width) as f64 => smaller.push((key.clone(), width)),
                Some(_) => {}
                None => unused.push((key.clone(), img.bytes())),
            }
        }
        for (key, width) in smaller {
            self.asked.insert(key.clone(), width);
            let _ = self.requests.send(Request { key, width });
        }
        let mut total: usize = self.images.values().map(Img::bytes).sum();
        unused.sort_by(|a, b| b.1.cmp(&a.1));
        let mut gone = Vec::new();
        for (key, bytes) in unused {
            if total <= IMAGE_BUDGET {
                break;
            }
            self.images.remove(&key);
            self.asked.remove(&key);
            total -= bytes;
            gone.push(key);
        }
        // Pictures hold on to what they drew, so those that drew one go too
        if !gone.is_empty() {
            self.pictures.retain(|_, recorded| !recorded.uses.iter().any(|(used, _)| gone.contains(used)));
            for key in &gone {
                self.kit.forget_patterns(Some(key));
            }
        }
    }

    /// Image textures, uploaded at all sorts of times, end up spread over
    /// many of the driver's memory blocks, each kept from being given back
    /// by the few in it. Once idle with more than a block in use, they're
    /// copied off the GPU and uploaded again together, which fits them in
    /// few blocks and frees the rest
    fn compact(&mut self) {
        let before = self.gpu.used();
        // Nor again while no more is used than after the last time, when it
        // didn't help: what's in use then isn't images
        if before < COMPACT_ABOVE || before <= self.compacted {
            return;
        }
        let mut rasters = Vec::new();
        for (key, img) in &self.images {
            if let Some(raster) = img.image.make_raster_image(&mut self.gpu.context, None) {
                rasters.push((key.clone(), raster));
            }
        }
        // Pictures hold the textures too
        self.pictures.clear();
        self.kit.forget_patterns(None);
        for (key, raster) in rasters {
            if let Some(img) = self.images.get_mut(&key) {
                img.image = raster;
            }
        }
        let _ = self.gpu.context.flush_submit_and_sync_cpu();
        self.gpu.context.free_gpu_resources();
        for img in self.images.values_mut() {
            img.image = upload(&mut self.gpu.context, img.image.clone());
        }
        let _ = self.gpu.context.flush_submit_and_sync_cpu();
        self.compacted = self.gpu.used();
        log::info!("native renderer: GPU memory compacted, {} MB to {} MB", before / 1_000_000, self.compacted / 1_000_000);
        self.dirty = true;
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
        if let Some(whole) = &mut self.whole {
            whole.stale = true;
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
                units.push(Unit { slot, section: true, bounds, custom: false });
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
                            units.push(Unit { slot: child, section: true, bounds: rest, custom: false });
                        }
                        break;
                    }
                    add(scene, child, units);
                }
            } else {
                units.push(Unit { slot, section: false, bounds, custom: false });
            }
        }
        for &root in &self.scene.roots {
            add(&self.scene, root, &mut units);
        }
        for unit in &mut units {
            unit.custom = custom_unit(&self.scene, unit);
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
        self.reach(key.area(), key.px())
    }

    /// Pixels at `px` a unit that background blurs over `area` reach out
    fn reach(&self, area: Rect, px: f32) -> i32 {
        let reach: f32 = self.backdrops.iter().filter(|(bounds, reach)| Rect::intersects2(bounds, &area.with_outset((*reach, *reach)))).map(|(_, reach)| reach).sum();
        if reach <= 0.0 {
            return 0;
        }
        // In steps of 32, so tiles share scratch surfaces
        (((reach * px).ceil() as i32 + 31) / 32 * 32).min(MAX_MARGIN)
    }

    /// Samples a side exact tiles of `px` pixels a unit take
    fn supersample(&self, px: f32) -> i32 {
        if px > SUPERSAMPLE_UNTIL { 1 } else { self.config.supersample }
    }

    /// The zoom level unit `unit` is recorded at for tiles of `px` pixels a
    /// unit: the nearest power of two, or for exact tiles `px` itself where
    /// it matters: custom effects work in whole pixels, and without 16
    /// samples a pixel images average their texels under each pixel, which
    /// needs to know just where it falls
    fn level(&self, unit: &Unit, px: f32, exact: bool) -> i32 {
        if exact && (unit.custom || px > SUPERSAMPLE_UNTIL) {
            EXACT_LEVEL + (px * EXACT_STEPS).round().min((i32::MAX - EXACT_LEVEL) as f32) as i32
        } else {
            px.log2().round().clamp(-12.0, 8.0) as i32
        }
    }

    /// Unit `unit`'s picture for zoom `level`, recorded if it isn't yet,
    /// with anti-aliased edges if `aa`
    fn picture(&mut self, unit: usize, level: i32, aa: bool) -> Option<&Recorded> {
        let (slot, section, bounds) = (self.units[unit].slot, self.units[unit].section, self.units[unit].bounds);
        let key = (slot, section, level, aa);
        if !self.pictures.contains_key(&key) {
            let mut recorder = PictureRecorder::new();
            let canvas = recorder.begin_recording(bounds.with_outset((1.0, 1.0)), true);
            let mut encoder = Encoder::new(&self.scene, &self.images, &self.kit, canvas, level_px(level));
            encoder.exact_renders = level >= EXACT_LEVEL;
            encoder.tint = self.tint;
            encoder.aa = aa;
            if section && self.scene.node(slot).is_some_and(|node| node.kind == Kind::Section) {
                encoder.section(slot);
            } else if section {
                encoder.masked(slot);
            } else {
                encoder.node(slot);
            }
            let (counts, reason) = (encoder.counts, encoder.approximate_reason.take());
            let waiting = encoder.missing_images.len() + encoder.exact_images.len();
            let images: Vec<(String, f64)> = encoder.missing_images.drain(..).chain(encoder.sharper_images.drain(..)).collect();
            let exact = std::mem::take(&mut encoder.exact_images);
            let sources = std::mem::take(&mut encoder.sources);
            let mut uses = std::mem::take(&mut encoder.used_images);
            uses.extend(encoder.missing_images.iter().cloned());
            drop(encoder);
            let picture = recorder.finish_recording_as_picture(None)?;
            self.ask(&images);
            self.ask_exact(&exact);
            let missing = images.into_iter().chain(exact).map(|(key, _)| key).collect();
            self.pictures.insert(key, Recorded { picture, counts, reason, missing, waiting, sources, uses });
        }
        self.pictures.get(&key)
    }

    /// Asks for renders at exactly the width given, unless that's what was
    /// asked for last
    fn ask_exact(&mut self, renders: &[(String, f64)]) {
        for (key, width) in renders {
            if self.failed.get(key).is_some_and(|at| at.elapsed() < Duration::from_secs(10)) {
                continue;
            }
            let asked = self.asked.get(key).copied().unwrap_or(0.0);
            if (width / asked - 1.0).abs() > EXACT_TOLERANCE as f64 {
                self.asked.insert(key.clone(), *width);
                let _ = self.requests.send(Request { key: key.clone(), width: *width });
            }
        }
    }

    fn ask(&mut self, images: &[(String, f64)]) {
        for (key, width) in images {
            if self.failed.get(key).is_some_and(|at| at.elapsed() < Duration::from_secs(10)) {
                continue;
            }
            let asked = self.asked.get(key).copied().unwrap_or(0.0);
            if *width > asked * 1.05 {
                self.asked.insert(key.clone(), *width);
                let _ = self.requests.send(Request { key: key.clone(), width: *width });
            }
        }
    }

    /// Draws tile `key`: every unit over it, in order. Exact tiles take
    /// Figma's 16 samples a pixel, others one with anti-aliased edges, which
    /// is many times quicker
    fn draw_tile(&mut self, key: TileKey, exact: bool) -> Option<Tile> {
        let ss = if exact { self.supersample(key.px()) } else { 1 };
        let aa = ss < 4;
        let area = key.area();
        let px = key.px();
        let margin = self.margin(key);
        let reach = area.with_outset((margin as f32 / px, margin as f32 / px));
        let units: Vec<usize> = (0..self.units.len()).filter(|&i| Rect::intersects2(&self.units[i].bounds, &reach)).collect();
        let mut counts = Counts::default();
        let mut pictures = Vec::new();
        let mut names = Vec::new();
        // Timing waits for the GPU, so only some tiles are timed
        self.made += 1;
        let timing = super::dev() && (self.made % 8 == 0 || tracing());
        let start = Instant::now();
        for unit in units {
            // Too small to see: left out, as the encoder would
            let bounds = self.units[unit].bounds;
            if bounds.width().max(bounds.height()) * px < 0.5 {
                counts.skipped += 1;
                continue;
            }
            let level = self.level(&self.units[unit], px, exact);
            if let Some(recorded) = self.picture(unit, level, aa) {
                counts += recorded.counts;
                pictures.push(recorded.picture.clone());
                if tracing() {
                    names.push(self.scene.node(self.units[unit].slot).map(|node| format!("{} {} {}", node.id, node.type_name, node.effects.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>().join(","))).unwrap_or_default());
                }
            }
        }
        let recorded = start.elapsed();
        let tex = self.tex(TILE, TILE)?;
        let found = match self.scratch.iter().position(|(kept, samples, _)| (*kept, *samples) == (margin, ss)) {
            Some(found) => found,
            None => {
                let side = (TILE + margin * 2) * ss;
                let surface = self.gpu.surface(side, side, self.config.samples)?;
                self.scratch.push((margin, ss, surface));
                self.scratch.len() - 1
            }
        };
        let (_, _, scratch) = &mut self.scratch[found];
        let canvas = scratch.canvas();
        canvas.clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
        canvas.save();
        canvas.translate((((margin - key.x * TILE) * ss) as f32 - self.frac.0 * ss as f32, ((margin - key.y * TILE) * ss) as f32 - self.frac.1 * ss as f32));
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
        let drawn = scratch.image_snapshot();
        let backend = tex.backend();
        let mut target =
            gpu::surfaces::wrap_backend_texture(&mut self.gpu.context, &backend, SurfaceOrigin::TopLeft, 1, ColorType::RGBA8888, None, None)?;
        if ss == 1 {
            let mut paint = Paint::default();
            paint.set_blend_mode(skia_safe::BlendMode::Src);
            let whole = Rect::from_iwh(TILE, TILE);
            let from = Rect::from(source);
            target.canvas().draw_image_rect(&drawn, Some((&from, skia_safe::canvas::SrcRectConstraint::Strict)), whole, &paint);
        } else {
            shrink(&mut target, &drawn, source, self.average.as_ref());
        }
        drop(drawn);
        let image = self.finish(&backend);
        drop(target);
        let image = image?;
        // Margins are rare, so their bigger surfaces aren't kept
        if margin != 0 {
            self.scratch.remove(found);
        }
        if timing {
            let gpu = start.elapsed();
            let mut stats = stats().lock().unwrap();
            let mix = |mean: &mut f64, ms: f64| *mean = if *mean == 0.0 { ms } else { *mean * 0.9 + ms * 0.1 };
            mix(&mut stats.tile_record_ms, recorded.as_secs_f64() * 1000.0);
            mix(&mut stats.tile_play_ms, (played - recorded).as_secs_f64() * 1000.0);
            mix(&mut stats.tile_gpu_ms, (gpu - played).as_secs_f64() * 1000.0);
            stats.tiles_drawn += 1;
            drop(stats);
            trace(|| {
                let ms = |d: Duration| d.as_secs_f64() * 1000.0;
                format!("  tile {},{} exact={exact} margin={margin} units={} record={:.1} play={:.1} gpu={:.1}", key.x, key.y, pictures.len(), ms(recorded), ms(played - recorded), ms(gpu - played))
            });
            // What a slow tile spent its time on, each unit drawn alone
            if tracing() && (gpu - played) > Duration::from_millis(50) {
                if let Some(mut alone) = self.gpu.surface((TILE + margin * 2) * ss, (TILE + margin * 2) * ss, self.config.samples) {
                    {
                        let canvas = alone.canvas();
                        canvas.save();
                        canvas.translate((((margin - key.x * TILE) * ss) as f32, ((margin - key.y * TILE) * ss) as f32));
                        canvas.scale((px * ss as f32, px * ss as f32));
                        let at = Instant::now();
                        for picture in &pictures {
                            canvas.draw_picture(picture, None, None);
                        }
                        canvas.restore();
                        let _ = self.gpu.context.flush_and_submit_surface(&mut alone, Some(SyncCpu::Yes));
                        trace(|| format!("    whole tile again {:.1}ms", at.elapsed().as_secs_f64() * 1000.0));
                    }
                    for (picture, name) in pictures.iter().zip(&names) {
                        let canvas = alone.canvas();
                        canvas.clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
                        canvas.save();
                        canvas.translate((((margin - key.x * TILE) * ss) as f32, ((margin - key.y * TILE) * ss) as f32));
                        canvas.scale((px * ss as f32, px * ss as f32));
                        let at = Instant::now();
                        canvas.draw_picture(picture, None, None);
                        canvas.restore();
                        let _ = self.gpu.context.flush_and_submit_surface(&mut alone, Some(SyncCpu::Yes));
                        let took = at.elapsed().as_secs_f64() * 1000.0;
                        if took > 5.0 {
                            trace(|| format!("    {took:.1}ms {name}"));
                        }
                    }
                }
            }
        }
        Some(Tile { image, tex, used: self.frame, counts, stale: false, exact })
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
        checker_paint(self.checker.as_ref()?, self.scene.dark, origin, px, zoom)
    }

    /// Figma's pixel grid, see `present::grid_paint`
    fn pixel_grid(&self, canvas: &skia_safe::Canvas, origin: (f64, f64), px: f32, width: i32, height: i32) {
        if let Some(paint) = self.grid.as_ref().and_then(|grid| grid_paint(grid, origin, px)) {
            canvas.draw_rect(Rect::from_iwh(width, height), &paint);
        }
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
        let frac = if fractional() {
            ((view.x * px as f64 - ox as f64) as f32, (view.y * px as f64 - oy as f64) as f32)
        } else {
            (0.0, 0.0)
        };
        if (frac.0 - self.frac.0).abs() > 0.01 || (frac.1 - self.frac.1).abs() > 0.01 {
            self.clear_tiles();
            self.frac = frac;
        }
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
        // Renders off screen want the finished look straight away
        let at_once = budget.is_none();
        // Missing tiles first, then out of date ones, which show meanwhile,
        // drawn quickly so the view fills even while it moves. Once it has
        // been still a moment, quick tiles are drawn again exactly, one a
        // frame, as each takes a while
        // While the zoom changes, tiles of it are of no use a frame later, so
        // the view is drawn straight from the pictures instead
        let zooming = !at_once && !settled;
        let mut order: Vec<(TileKey, bool)> =
            needed.iter().copied().filter(|key| !zooming && !self.tiles.contains_key(key)).map(|key| (key, at_once)).collect();
        // An out of date tile still shows, so it's drawn again as well as
        // it was, rather than quickly, which would blur it a while
        order.extend(
            needed.iter().copied().filter_map(|key| self.tiles.get(&key).filter(|tile| !zooming && tile.stale).map(|tile| (key, at_once || tile.exact))),
        );
        let rough: Vec<TileKey> =
            needed.iter().copied().filter(|key| self.tiles.get(key).is_some_and(|tile| !tile.stale && !tile.exact)).collect();
        if !rough.is_empty() {
            pending = true;
            if settled && self.moved.elapsed() >= REFINE_AFTER {
                order.push((rough[0], true));
            }
        }
        let mut behind = false;
        // Exact tiles take the GPU a while, so one a frame at most
        let mut exact_drawn = false;
        for (key, exact) in order {
            if budget.is_some_and(|budget| start.elapsed() >= budget) || (exact && !at_once && (behind || exact_drawn)) {
                pending = true;
                behind = true;
                continue;
            }
            if let Some(tile) = self.draw_tile(key, exact) {
                if let Some(old) = self.tiles.insert(key, tile) {
                    self.retire(old.tex);
                }
            }
            exact_drawn |= exact && !at_once;
            // The budget is of time the GPU takes too, as that's most of a
            // tile's cost, so it waits for each before drawing another
            if !at_once {
                let _ = self.gpu.context.flush_submit_and_sync_cpu();
            }
        }
        let checker = self.checker((view.x * px as f64, view.y * px as f64), px, view.zoom);
        let canvas = target.canvas();
        canvas.clear(self.background());
        if let Some(checker) = &checker {
            canvas.draw_paint(checker);
        }
        // Where this zoom's tiles aren't ready, the pictures are played
        // straight onto the view at this zoom, as Figma draws every frame,
        // rather than stretching tiles of another zoom, which blurs
        let missing: Vec<TileKey> = needed.iter().copied().filter(|key| !self.tiles.contains_key(key)).collect();
        let live_start = Instant::now();
        let mut live = Counts::default();
        if !missing.is_empty() {
            let mut holes = skia_safe::Region::new();
            for key in &missing {
                holes.op_rect(skia_safe::IRect::from_xywh(key.x * TILE - ox, key.y * TILE - oy, TILE, TILE), skia_safe::region::RegionOp::Union);
            }
            let view_area = Rect::from_xywh(view.x as f32, view.y as f32, width as f32 / px, height as f32 / px);
            // Background blurs read what's drawn around them, so the whole
            // view is drawn, with a margin near one, and then the holes taken
            // from it. Drawn in the holes alone, a blur would see nothing
            // past their edges and show a seam there
            let margin = self.reach(view_area, px);
            let reach = view_area.with_outset((margin as f32 / px, margin as f32 / px));
            let mut pictures = Vec::new();
            for unit in 0..self.units.len() {
                let bounds = self.units[unit].bounds;
                if !Rect::intersects2(&bounds, &reach) || bounds.width().max(bounds.height()) * px < 0.5 {
                    continue;
                }
                let level = self.level(&self.units[unit], px, false);
                if let Some(recorded) = self.picture(unit, level, true) {
                    live += recorded.counts;
                    pictures.push(recorded.picture.clone());
                }
            }
            let play = |canvas: &skia_safe::Canvas, x: i32, y: i32| {
                canvas.translate((x as f32 - frac.0, y as f32 - frac.1));
                canvas.scale((px, px));
                for picture in &pictures {
                    canvas.draw_picture(picture, None, None);
                }
            };
            // Drawn whole, with a margin, onto a surface kept while the view
            // and what's in it stay the same, as they do while tiles fill in
            let size = (width + margin * 2, height + margin * 2);
            let drawn = (ox, oy, pxb, margin, pictures.iter().map(|picture| picture.unique_id()).collect::<Vec<u32>>());
            if self.live.as_ref().is_none_or(|(surface, _)| (surface.width(), surface.height()) != size) {
                self.live = self.gpu.surface(size.0, size.1, self.config.samples).map(|surface| (surface, None));
            }
            if let Some((surface, shows)) = self.live.as_mut() {
                if shows.as_ref() != Some(&drawn) {
                    let live_canvas = surface.canvas();
                    live_canvas.clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
                    live_canvas.save();
                    play(live_canvas, margin - ox, margin - oy);
                    live_canvas.restore();
                    *shows = Some(drawn);
                }
                let image = surface.image_snapshot();
                canvas.save();
                canvas.clip_region(&holes, skia_safe::ClipOp::Intersect);
                canvas.draw_image(&image, (-margin as f32, -margin as f32), None);
                canvas.restore();
            }
            trace(|| format!("  live holes={} pictures={} margin={margin} {:.1}ms", missing.len(), pictures.len(), live_start.elapsed().as_secs_f64() * 1000.0));
        }
        let mut counts = live;
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
        let aa = self.supersample(px) < 4;
        let reason = self
            .units
            .iter()
            .filter(|unit| Rect::intersects2(&unit.bounds, &view_area))
            .find_map(|unit| self.pictures.get(&(unit.slot, unit.section, self.level(unit, px, true), aa)).and_then(|recorded| recorded.reason.clone()));
        self.evict(pxb);
        (counts, reason, pending || !missing.is_empty())
    }

    /// Keeps the most recently shown tiles. Quick tiles of zooms passed
    /// through go soon, as they only stand in for a moment
    fn evict(&mut self, current: u32) {
        let frame = self.frame;
        let gone: Vec<TileKey> = self.tiles.iter().filter(|(key, tile)| !(tile.exact || key.px == current || frame - tile.used < PASSING)).map(|(key, _)| *key).collect();
        for key in gone {
            if let Some(tile) = self.tiles.remove(&key) {
                self.retire(tile.tex);
            }
        }
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
        let gone: Vec<TileKey> = self.tiles.iter().filter(|(_, tile)| tile.used < cutoff).map(|(key, _)| *key).collect();
        for key in gone {
            if let Some(tile) = self.tiles.remove(&key) {
                self.retire(tile.tex);
            }
        }
        // Pictures of zoom levels far from any tile kept go too
        let levels: Vec<i32> = self.tiles.keys().map(|key| key.px().log2().round() as i32).collect();
        let exact: Vec<i32> = self.tiles.keys().map(|key| EXACT_LEVEL + (key.px() * EXACT_STEPS).round() as i32).collect();
        self.pictures.retain(|&(_, _, level, _), _| {
            if level >= EXACT_LEVEL {
                exact.contains(&level)
            } else {
                levels.iter().any(|&kept| (kept - level).abs() <= 1)
            }
        });
    }

    fn offscreen(&mut self, id: &str, bounds: Rect, width: u32, height: u32, backdrop: bool) -> Option<Offscreen> {
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
        if backdrop {
            encoder.stop_after = Some(slot);
            for &root in &self.scene.roots {
                if self.scene.node(root).and_then(|node| node.bounds).is_none_or(|reach| Rect::intersects2(&reach, &bounds)) {
                    encoder.node(root);
                }
            }
        } else {
            encoder.node(slot);
        }
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
        let aa = self.supersample(px) < 4;
        let area = Rect::from_xywh(view.x as f32, view.y as f32, view.w as f32, view.h as f32);
        let waiting = self
            .units
            .iter()
            .filter(|unit| Rect::intersects2(&unit.bounds, &area))
            .filter_map(|unit| self.pictures.get(&(unit.slot, unit.section, self.level(unit, px, true), aa)))
            .map(|recorded| recorded.waiting)
            .sum();
        let info = ImageInfo::new((width as i32, height as i32), ColorType::RGBA8888, AlphaType::Unpremul, None);
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        if !target.canvas().read_pixels(&info, &mut pixels, (width * 4) as usize, (0, 0)) {
            return None;
        }
        Some((pixels, waiting))
    }

    /// Dev tools: how long node `id` and each node under it take to draw on
    /// a tile at (`x`, `y`) at `px` pixels a unit with `ss` samples a side;
    /// or for `*`, each unit over a view there; `#images`, the images kept;
    /// `#compact`, GPU memory used as tiles, pictures and images are let go
    fn profile(&mut self, id: &str, x: f32, y: f32, px: f32, ss: i32) -> String {
        if id == "#compact" {
            let before = self.gpu.used();
            self.clear_tiles();
            self.pictures.clear();
            self.kit.forget_patterns(None);
            self.live = None;
            self.scratch.clear();
            let _ = self.gpu.context.flush_submit_and_sync_cpu();
            self.gpu.context.free_gpu_resources();
            let without_tiles = self.gpu.used();
            self.images.clear();
            self.asked.clear();
            let _ = self.gpu.context.flush_submit_and_sync_cpu();
            self.gpu.context.free_gpu_resources();
            return format!("{} MB, without tiles and pictures {} MB, without images {} MB", before / 1_000_000, without_tiles / 1_000_000, self.gpu.used() / 1_000_000);
        }
        if id == "#images" {
            let mut rows: Vec<_> = self.images.iter().map(|(key, img)| (img.bytes(), format!("{} {}x{} of {:?} {:?}", key, img.image.width(), img.image.height(), img.original, img.area))).collect();
            rows.sort_by(|a, b| b.0.cmp(&a.0));
            return rows.iter().map(|(bytes, row)| format!("{:.1}MB {row}\n", *bytes as f64 / 1e6)).collect();
        }
        if id == "*" {
            // Every unit over a 1200 by 800 view there, played as live
            let area = Rect::from_xywh(x, y, 1200.0 / px, 800.0 / px);
            let Some(mut surface) = self.gpu.surface(1200, 800, 1) else {
                return "no surface".into();
            };
            let mut rows = Vec::new();
            for unit in 0..self.units.len() {
                if !Rect::intersects2(&self.units[unit].bounds, &area) {
                    continue;
                }
                let level = self.level(&self.units[unit], px, false);
                let Some(picture) = self.picture(unit, level, true).map(|recorded| recorded.picture.clone()) else { continue };
                let mut best = f64::MAX;
                for _ in 0..3 {
                    let canvas = surface.canvas();
                    canvas.save();
                    canvas.scale((px, px));
                    canvas.translate((-x, -y));
                    let start = Instant::now();
                    canvas.draw_picture(&picture, None, None);
                    canvas.restore();
                    let _ = self.gpu.context.flush_submit_and_sync_cpu();
                    best = best.min(start.elapsed().as_secs_f64() * 1000.0);
                }
                let name = self.scene.node(self.units[unit].slot).map(|node| format!("{} {}", node.id, node.type_name)).unwrap_or_default();
                rows.push((best, name, picture.approximate_op_count()));
            }
            rows.sort_by(|a, b| b.0.total_cmp(&a.0));
            return rows.iter().map(|(ms, name, ops)| format!("{ms:.2}ms {name} ops={ops}\n")).collect();
        }
        let Some(&root) = self.scene.by_id.get(id) else {
            return "no such node".into();
        };
        let side = TILE * ss;
        let Some(mut surface) = self.gpu.surface(side, side, self.config.samples) else {
            return "no surface".into();
        };
        let mut out = String::new();
        let mut stack = vec![(root, 0usize)];
        while let Some((slot, depth)) = stack.pop() {
            let mut ms = f64::MAX;
            for _ in 0..3 {
                let canvas = surface.canvas();
                canvas.clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
                let _ = self.gpu.context.flush_submit_and_sync_cpu();
                let canvas = surface.canvas();
                canvas.save();
                canvas.scale((px * ss as f32, px * ss as f32));
                canvas.translate((-x, -y));
                let start = Instant::now();
                {
                    let mut encoder = Encoder::new(&self.scene, &self.images, &self.kit, canvas, px);
                    encoder.aa = ss < 4;
                    encoder.node(slot);
                }
                canvas.restore();
                let _ = self.gpu.context.flush_submit_and_sync_cpu();
                ms = ms.min(start.elapsed().as_secs_f64() * 1000.0);
            }
            let Some(node) = self.scene.node(slot) else { continue };
            let effects: Vec<String> = node.effects.iter().map(|e| format!("{}:{}", e.kind, e.radius)).collect();
            out += &format!(
                "{}{} {} {:.1}ms {:?} mask={:?} blend={:?} {} fills={} outlines={} hidden={} bounds={:?}\n",
                "  ".repeat(depth),
                node.id,
                node.type_name,
                ms,
                node.size,
                node.mask,
                node.blend,
                effects.join(","),
                node.visible_fills().count(),
                node.fill_geometry.len(),
                node.hidden,
                node.bounds
            );
            if ms > 0.5 && depth < 12 {
                for &child in node.children.iter().rev() {
                    stack.push((child, depth + 1));
                }
            }
        }
        out
    }

    fn shown(&mut self) -> Option<(Vec<u8>, u32, u32)> {
        let view = self.view.clone()?;
        let (width, height) = (self.gpu.size.0, self.gpu.size.1);
        let mut target = self.gpu.surface(width as i32, height as i32, 1)?;
        let settled = self.zoom.1.elapsed() >= SETTLE;
        self.compose(&mut target, &view, Some(Duration::ZERO), settled);
        let info = ImageInfo::new((width as i32, height as i32), ColorType::RGBA8888, AlphaType::Unpremul, None);
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        target.canvas().read_pixels(&info, &mut pixels, (width * 4) as usize, (0, 0)).then_some((pixels, width, height))
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
        self.clear_tiles();
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
        self.clear_tiles();
        let cold = time(self, &mut target);
        let warm = time(self, &mut target);
        cold.into_iter().zip(warm).collect()
    }
}

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
