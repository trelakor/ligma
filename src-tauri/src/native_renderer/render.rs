//! The render thread: Skia on D3D12, drawing the mirrored scene for the
//! native renderer's window as Figma's viewport moves. Each frame it composes
//! the view whole from what it's drawn, all of it as of the same frame of
//! Figma's render tree, and hands that to the present thread (see
//! present.rs), which shows it as it is
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
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use std::sync::Arc;

use skia_safe::gpu::d3d::BackendContext;
use skia_safe::RoundOut as _;
use skia_safe::gpu::{self, BackendTexture, DirectContext, Protected, SurfaceOrigin, SyncCpu};
use skia_safe::{
    AlphaType, Color4f, ColorType, Data, FilterMode, Image, ImageInfo, MipmapMode, Paint, Picture,
    PictureRecorder, Rect, SamplingOptions, Surface,
};
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{D3D12CreateDevice, ID3D12CommandQueue, ID3D12Device, ID3D12Fence, D3D12_FENCE_FLAG_NONE};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIAdapter3, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIFactory4, IDXGIFactory6, DXGI_ADAPTER_FLAG, DXGI_ADAPTER_FLAG_SOFTWARE,
    DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
};

use super::draw::{blends_through, blurs_behind, Counts, Encoder, Img, Kit, EFFECT, RENDER, RENDER_LIMIT};
use super::effects::{Job, Kept, Run};
use super::present::{checker_paint, cover, grid_paint, view_at, Device, Fence, Hub, Layer, LayerKind, Patterns, Snapshot, Tex, CHECKER, GRID_COLOR, GRID_LINES};
use super::scene::{Kind, Prepared, Scene};
use super::tree::{Drawer, Frame, Place, Tree};
use super::stats;

/// Figma's viewport, from `view()` in page.js
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq)]
pub struct View {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub zoom: f64,
    /// Figma's canvas in the tab, in CSS pixels: where it starts and its size
    #[serde(default)]
    pub cx: f64,
    #[serde(default)]
    pub cy: f64,
    pub cw: f64,
    pub ch: f64,
    pub dpr: f64,
    pub page: String,
    #[serde(default)]
    pub visible: bool,
    /// Figma's pixel preview: pixels a unit it previews at, 1 or 2, or 0
    #[serde(default)]
    pub preview: f64,
    /// Figma's pixel grid is on
    #[serde(default = "on")]
    pub grid: bool,
    /// Outline mode, as the editor's background, 0xRRGGBB, or -1 when it's
    /// off. Figma's overlay draws the outlines, so its renderer draws only
    /// that, without the page or the pixel grid
    #[serde(default = "off")]
    pub outline: f64,
}

fn on() -> bool {
    true
}

fn off() -> f64 {
    -1.0
}

impl View {
    /// The background outline mode shows, when it's on
    pub fn outline(&self) -> Option<Color4f> {
        (self.outline >= 0.0).then(|| {
            let rgb = self.outline as u32;
            Color4f::new(((rgb >> 16) & 255) as f32 / 255.0, ((rgb >> 8) & 255) as f32 / 255.0, (rgb & 255) as f32 / 255.0, 1.0)
        })
    }

    /// The pixels Figma's canvas has, as the browser lines it up with the
    /// screen's: its edges at the nearest device pixel
    pub fn canvas(&self) -> (u32, u32) {
        let snapped = |at: f64, size: f64| (((at + size) * self.dpr).round() - (at * self.dpr).round()).max(1.0) as u32;
        (snapped(self.cx, self.cw), snapped(self.cy, self.ch))
    }

    /// How much the browser stretches those pixels across the canvas's own
    /// size, which isn't a whole number of them
    pub fn stretch(&self) -> (f64, f64) {
        let (w, h) = self.canvas();
        ((self.cw * self.dpr / w as f64).max(1e-3), (self.ch * self.dpr / h as f64).max(1e-3))
    }

    /// Device pixels a unit in a window `width` pixels wide
    pub fn px(&self, width: u32) -> f32 {
        (width as f64 / self.w) as f32
    }

    /// Pixels a unit the view is drawn at in a window `size`, whether that's
    /// a pixel preview, and the window's size in those pixels. With Figma's
    /// pixel preview on and the view zoomed past it, it's drawn at the
    /// preview's 1 or 2 pixels a unit and magnified, each pixel a square, as
    /// Figma shows it
    pub fn drawn(&self, (width, height): (u32, u32)) -> (f32, bool, i32, i32) {
        let px = self.px(width);
        let preview = self.preview as f32;
        if preview > 0.0 && px > preview {
            let scale = preview / px;
            (preview, true, (width as f32 * scale).ceil() as i32 + 1, (height as f32 * scale).ceil() as i32 + 1)
        } else {
            (px, false, width as i32, height as i32)
        }
    }
}

pub enum Msg {
    /// A page starts streaming in, empty
    Begin(Box<Scene>),
    /// A frame of Figma's render tree, which once it comes is drawn in place
    /// of the scene read through the plugin API
    Frame(Box<Frame>),
    /// Figma's noise octaves, see `scene::Begin`
    Octaves(Vec<f32>),
    Changes(Prepared),
    View(View),
    /// Image `key`, and the width it was asked for at, if it was
    Image(String, Img, Option<f64>),
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
    /// Dev tools: draw views offscreen as a whole view is drawn, without
    /// touching what's drawn or shown, and send back where the time went
    /// (see `Painter::probe`)
    Probe { spec: String, reply: Sender<String> },
    Tint(bool),
    /// The GPU has passed a fence, so what it drew can be shown
    Gpu,
    /// Dev tools: whether units are drawn into images of their own
    Rasters(bool),
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
pub(super) fn tracing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| super::dev() && std::env::var("LIGMA_NATIVE_TRACE").is_ok_and(|value| value == "1"))
}

/// With LIGMA_NATIVE_TRACE_UNITS=1 as well, slow tiles are drawn again a
/// unit at a time to trace what each costs, which holds up drawing
fn tracing_units() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| tracing() && std::env::var("LIGMA_NATIVE_TRACE_UNITS").is_ok_and(|value| value == "1"))
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
    /// Drawn the same alone as over what's drawn before it, so it can be
    /// drawn once into an image of its own (see `Painter::drawn`)
    alone: bool,
    /// Of Figma's render tree, where `slot` is the unit's key
    place: Option<Place>,
}

/// A unit as it's drawn: its picture, or an image of it drawn at the scale
/// wanted and where that goes, in device pixels at that scale
#[derive(Clone)]
enum Drawn {
    Picture(Picture),
    Image(Image, (i32, i32)),
    /// A thumb (see `Painter::thumb`): where it is on its page, in the
    /// page's pixels, the area it covers, in units, and whether it's drawn
    /// with what's under it, so it replaces that
    Paged(usize, Rect, Rect, bool),
    Thumb(Image, Rect, Rect, bool),
}

/// Plays `drawn` onto `canvas`, whose matrix is the identity, at `scale`
/// device pixels a unit, the page's origin at `offset`
/// Milliseconds since the epoch, as the page counts them
pub fn since_epoch_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |since| since.as_secs_f64() * 1000.0)
}

fn play(canvas: &skia_safe::Canvas, drawn: &[Drawn], offset: (f32, f32), scale: f32) {
    for item in drawn {
        match item {
            Drawn::Picture(picture) => {
                canvas.save();
                canvas.translate(offset);
                canvas.scale((scale, scale));
                canvas.draw_picture(picture, None, None);
                canvas.restore();
            }
            Drawn::Image(image, (x, y)) => {
                canvas.draw_image(image, (offset.0 + *x as f32, offset.1 + *y as f32), None);
            }
            Drawn::Thumb(image, from, to, over) => {
                canvas.save();
                canvas.translate(offset);
                canvas.scale((scale, scale));
                let mut paint = Paint::default();
                paint.set_anti_alias(true);
                if *over {
                    paint.set_blend_mode(skia_safe::BlendMode::Src);
                }
                let sampling = SamplingOptions::new(FilterMode::Linear, MipmapMode::None);
                canvas.draw_image_rect_with_sampling_options(image, Some((from, skia_safe::canvas::SrcRectConstraint::Strict)), *to, sampling, &paint);
                canvas.restore();
            }
            Drawn::Paged(..) => {}
        }
    }
}

/// `px` rounded up to a power of two's square root
fn half_octave(px: f32) -> f32 {
    2f32.powf((px.max(1e-6).log2() * 2.0).ceil() / 2.0)
}

/// The node and content the custom effects image `key` is of, for one of
/// the tree's (see `Tree::custom_effect`)
fn tree_effect_of(key: &str) -> Option<(u32, u64)> {
    let (id, content) = key.strip_prefix(EFFECT)?.strip_prefix('t')?.split_once('.')?;
    Some((id.parse().ok()?, content.parse().ok()?))
}

/// Side of the pages thumbs of units are drawn on, in pixels
const THUMB_PAGE: i32 = 2048;
/// Pages of thumbs kept at most, 16 MB each
const THUMB_PAGES: usize = 8;
/// Units wider than this at a thumb's scale are drawn as they are
const THUMB_SIDE: i32 = 1024;
/// Time a draw spends at most making thumbs from pictures, past which
/// thumbs of other powers stand in
const THUMB_TIME: Duration = Duration::from_millis(10);

/// A page of thumbs, filled in rows of thumbs of about the same height
struct ThumbPage {
    surface: Surface,
    /// Each row's top, height and how far it's filled
    rows: Vec<(i32, i32, i32)>,
    bottom: i32,
    used: u64,
    /// Counts the times it was cleared, which the thumbs on it went with
    generation: u64,
}

impl ThumbPage {
    fn place(&mut self, width: i32, height: i32) -> Option<skia_safe::IRect> {
        let height = (height + 7) / 8 * 8;
        for row in &mut self.rows {
            if height <= row.1 && height * 3 >= row.1 * 2 && row.2 + width <= THUMB_PAGE {
                let at = skia_safe::IRect::from_xywh(row.2, row.0, width, height);
                row.2 += width;
                return Some(at);
            }
        }
        if self.bottom + height > THUMB_PAGE || width > THUMB_PAGE {
            return None;
        }
        self.rows.push((self.bottom, height, width));
        let at = skia_safe::IRect::from_xywh(0, self.bottom, width, height);
        self.bottom += height;
        Some(at)
    }
}

/// A unit drawn on a page of thumbs (see `Painter::thumb`)
struct Thumb {
    page: usize,
    generation: u64,
    from: Rect,
    to: Rect,
    /// The picture it was drawn from, and whether that waited on images
    picture: u32,
    picture_level: i32,
    waiting: bool,
    counts: Counts,
    /// Thumbs made so far when it was made
    serial: u64,
    /// For a unit not drawn the same alone, what's under it, which it's
    /// drawn over (see `Painter::under`), else 0
    under: u64,
    /// The unit as it was drawn, see `Painter::mark`
    version: u64,
}

/// An image of a unit (see `Painter::drawn`)
struct Raster {
    image: Image,
    at: (i32, i32),
    /// The picture it was drawn from
    picture: u32,
    used: u64,
}

/// GPU memory an image of a unit takes: D3D12 places textures in 64 KiB
/// pages, so a small one takes a whole page
fn footprint(image: &Image) -> usize {
    ((image.width() * image.height() * 4) as usize).div_ceil(64 << 10) * (64 << 10)
}

/// Widest image a unit is drawn into, past which it's played into each tile
const RASTER_SIDE: i32 = 1024;
/// GPU memory units' images take at most
const RASTER_BUDGET: usize = 32 << 20;
/// Units smaller than this on their longer side are played into each tile
/// rather than drawn into an image: they're rarely in more than one, and
/// far out there are thousands
const RASTER_MIN: i32 = 192;
/// Images of units made at most for a tile or view, so one far out doesn't
/// make hundreds at once
const RASTERS_A_DRAW: u32 = 16;

/// How far around unit `unit` what's drawn before it shows through it, if
/// it does: a blend mode that reaches past it, or a background blur
fn reads_behind(scene: &Scene, unit: &Unit) -> Option<f32> {
    let node = scene.node(unit.slot)?;
    let slots: Vec<usize> = if unit.section && node.kind == Kind::Section {
        // The section's own fill and strokes: its children are units of
        // their own
        let blends = |mode: &Option<String>| mode.as_deref().is_some_and(|mode| mode != "NORMAL" && mode != "PASS_THROUGH");
        let own = blends(&node.blend) || node.visible_fills().chain(node.strokes.iter().filter(|paint| paint.visible)).any(|paint| blends(&paint.blend_mode));
        let blur = node
            .effects
            .iter()
            .filter(|effect| effect.kind == "BACKGROUND_BLUR" || effect.kind == "GLASS")
            .map(|effect| effect.radius as f32 * 1.5)
            .reduce(f32::max);
        return match (own, blur) {
            (false, None) => None,
            (_, blur) => Some(blur.unwrap_or(0.0)),
        };
    } else if unit.section {
        // A mask and the rest of the section's children
        let parent = node.parent.and_then(|parent| scene.node(parent))?;
        parent.children.iter().copied().skip_while(|&child| child != unit.slot).collect()
    } else {
        vec![unit.slot]
    };
    let blends = slots.iter().any(|&slot| blends_through(scene, slot));
    let blur = slots.iter().filter_map(|&slot| blurs_behind(scene, slot)).reduce(f32::max);
    match (blends, blur) {
        (false, None) => None,
        (_, blur) => Some(blur.unwrap_or(0.0)),
    }
}

/// A unit as Vello draws it at a level: a fragment, nothing where it has
/// something only Skia draws, so it's encoded for each view, or why Vello
/// can't draw it. Made again once `stamp` isn't the unit's
struct Encoded {
    stamp: u64,
    made: Result<super::vello::Fragment, String>,
    used: Instant,
}

/// Pixels of Vello's image atlas, 8192 square at most, images and pages
/// may take, leaving room for how they're packed
const ATLAS_MOST: u64 = 8192 * 8192 * 3 / 4;

/// How long fragments no view used are kept
const FRAGMENTS_KEPT: Duration = Duration::from_secs(3);

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

/// Textures kept to use again
const SPARE: usize = 12;

/// The whole view, see `Painter::draw_whole`
struct Whole {
    layer: Layer,
    /// Its corner in device pixels and its zoom, as bits
    at: (i32, i32, u32),
    /// Something in it changed since, past drawing again where
    stale: bool,
    /// Where something in it changed since, in units, to draw again
    patch: Vec<Rect>,
    /// The render queue's fence passes this once the GPU has drawn it
    fence: u64,
    /// Changes with what's drawn in it
    serial: u64,
}

/// After a change, how long the whole view is kept up with every frame, as
/// Figma does, rather than tile by tile
const EDITING: Duration = Duration::from_millis(400);

/// Changed areas past which the whole view is drawn again, rather than each
const PATCHES: usize = 24;
/// Most units drawn again exactly in a patch of the whole view
const PATCH_UNITS: usize = 160;

/// An image on the GPU once, with its mipmaps, and outside Skia's cache:
/// left to the cache, layers drawn in a frame push images out, and each is
/// uploaded and its mipmaps made again next frame. One drawn on the GPU is
/// read back first, so Skia makes its mipmaps on the CPU, as it does
/// correctly, rather than on the GPU (see `draw::sampled`)
fn upload(context: &mut skia_safe::gpu::DirectContext, image: Image) -> Image {
    let _purpose = super::gpu_census::purpose("image");
    let pixels = if image.is_texture_backed() { image.make_raster_image(&mut *context, None) } else { Some(image.clone()) };
    pixels
        .and_then(|pixels| skia_safe::gpu::images::texture_from_image(context, &pixels, skia_safe::gpu::Mipmapped::Yes, skia_safe::gpu::Budgeted::No))
        .unwrap_or(image)
}

/// Images are kept as big as they were ever needed, which drawn with
/// mipmaps shows the same as a smaller copy, till together they pass this
/// many bytes. Then those needed longest ago go, never any in view, down
/// to three quarters of it, so it's seldom
const IMAGES_BUDGET: usize = 384 << 20;
/// Images let go are kept this many pixels on their longest side, so
/// there's something to show where they're drawn till they come again
const PLACEHOLDER: i32 = 256;
/// Bytes of those kept of images nothing near the view draws
const PLACEHOLDERS: usize = 32 << 20;

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
    /// Skia's own GPU cache: images, path masks, scratch layers. Smaller,
    /// the layers effects take over the whole view are made again each
    /// frame it's drawn. Given back once nothing's drawn for a while
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
        // Skia's working set while zoomed out on a big file, past which it
        // makes again the layers and masks it would reuse, thousands a view
        cache_limit: get("LIGMA_NATIVE_CACHE_MB", 768) << 20,
    }
}

/// About how long an exact tile is drawn for at a time, before what else
/// there is to do
const TILE_SLICE: Duration = Duration::from_millis(4);

/// A tile partly drawn (see `Painter::step_tile`)
struct Pending {
    key: TileKey,
    exact: bool,
    draft: bool,
    pictures: Vec<Drawn>,
    names: Vec<String>,
    /// The first of `pictures` not drawn yet, and how many to draw at once
    next: usize,
    group: usize,
    scratch: Surface,
    margin: i32,
    ss: i32,
    /// Drawn with 4 samples a pixel by the GPU rather than supersampled
    msaa: bool,
    offset: (f32, f32),
    counts: Counts,
    timing: bool,
    started: Instant,
    recorded: Duration,
    played: Duration,
    flushed: Duration,
    before: [f64; 6],
    steps: u32,
    /// The longest group of pictures took
    longest: Duration,
}

/// Widest margin a tile is drawn with, in pixels
const MAX_MARGIN: i32 = 256;
/// Zoomed in past this many pixels a unit, exact tiles take 4 samples a
/// pixel from the GPU rather than 16 drawn bigger: 16 would look no
/// different there, but cost 16 times as much, which with big blurs is
/// hundreds of milliseconds a tile
const SUPERSAMPLE_UNTIL: f32 = 2.0;
/// Time to spend per frame drawing tiles not ready yet, so a newly shown
/// part of the page fills in over a few frames rather than stalling one
const BUILD_BUDGET: Duration = Duration::from_millis(10);
/// Frames a quick tile of another zoom is kept after it was last shown
const PASSING: u64 = 30;
/// Picture levels from this on are exact zooms, in steps of 1/EXACT_STEPS
/// of a pixel a unit
const EXACT_LEVEL: i32 = 1 << 24;
/// Levels from this up, below `EXACT_LEVEL`, are drafts of the level as
/// much lower: see `Encoder::draft`
const DRAFT_LEVEL: i32 = 1 << 20;
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
    } else if is_draft(level) {
        2f32.powi(level - DRAFT_LEVEL)
    } else {
        2f32.powi(level)
    }
}

fn is_draft(level: i32) -> bool {
    (DRAFT_LEVEL / 2..EXACT_LEVEL).contains(&level)
}

/// Dev tools: GPU resources made, programs compiled and pipelines made so
/// far, each with the ms they took
fn gpu_counters() -> [f64; 6] {
    use std::sync::atomic::Ordering::Relaxed;
    let (made, made_ms) = super::committed::made();
    let ms = |ns: u64| ns as f64 / 1e6;
    [
        made as f64,
        made_ms,
        super::shader_cache::ASKED.load(Relaxed) as f64,
        ms(super::shader_cache::ASKED_NS.load(Relaxed)),
        super::gpu_census::PIPELINES.load(Relaxed) as f64,
        ms(super::gpu_census::PIPELINES_NS.load(Relaxed) as u64),
    ]
}

/// Units smaller than this many pixels are left out of drafts
const DRAFT_SMALLEST: f32 = 2.0;

/// How long the view stays still before quick tiles are drawn exactly
const REFINE_AFTER: Duration = Duration::from_millis(250);
/// Layers drawing took are given back after this long without a frame
const IDLE: Duration = Duration::from_secs(3);
/// And all else Skia keeps on the GPU after this long, its atlases, masks
/// and images, which the next draw makes again, a hitch of 100 ms or so
const FREE_AFTER: Duration = Duration::from_secs(30);
/// The zoom counts as settled when it hasn't changed for this long
const SETTLE: Duration = Duration::from_millis(120);

/// Tiles of the coarse level kept, 1 MB each. Drawn at a half to a quarter
/// of the view's pixels, they stand in wherever a pan reaches before its
/// tiles are drawn, so it never shows nothing
const COARSE_TILES: usize = 16;

/// Tiles of the levels further out kept, 1 MB each (see `Painter::pyramid`)
const PYRAMID_TILES: usize = 96;
/// Most levels further out
const PYRAMID_LEVELS: usize = 9;

/// How far ahead of a pan tiles are drawn, as time at its speed
const LOOKAHEAD: f64 = 0.3;

/// The coarse level for `px`: a power of two, so it stays put while the
/// zoom changes a little
fn coarse(px: f32) -> f32 {
    2f32.powi(px.log2().floor() as i32 - 1)
}

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
    /// A draft, to show behind the view's tiles while they're drawn. One
    /// the view itself needs is drawn again in full
    draft: bool,
}

impl Drop for Gpu {
    fn drop(&mut self) {
        if let Some((event, stop)) = self.done.take() {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = unsafe { windows::Win32::System::Threading::SetEvent(event) };
        }
    }
}

pub struct Gpu {
    context: DirectContext,
    device: ID3D12Device,
    /// Signalled on Skia's queue after each frame and whole view, so the
    /// present thread shows a frame once the GPU has drawn it, and this one
    /// draws the next once it's done with the last, neither waiting
    queue: ID3D12CommandQueue,
    fence: ID3D12Fence,
    /// The same, for the present thread to know when a frame's drawn
    shared_fence: Arc<Fence>,
    signalled: u64,
    /// Set as the GPU passes each fence signalled, waking the render thread
    /// (see `watch`)
    done: Option<(windows::Win32::Foundation::HANDLE, Arc<std::sync::atomic::AtomicBool>)>,
    /// The window's size in pixels
    size: (u32, u32),
    pub adapter: String,
    memory: Option<IDXGIAdapter3>,
}

/// Dev tools: with LIGMA_NATIVE_D3D_DEBUG set, D3D12 checks everything
/// asked of it, and with it set to 2 what the GPU does too, which is slow.
/// True if it's on
fn debug_layer() -> bool {
    use windows::Win32::Graphics::Direct3D12::{D3D12GetDebugInterface, ID3D12Debug, ID3D12Debug1};
    let Some(level) = std::env::var("LIGMA_NATIVE_D3D_DEBUG").ok().filter(|_| super::dev()) else {
        return false;
    };
    let mut debug: Option<ID3D12Debug> = None;
    if unsafe { D3D12GetDebugInterface(&mut debug) }.is_err() {
        log::warn!("native renderer: D3D12's debug layer isn't installed");
        return false;
    }
    let Some(debug) = debug else { return false };
    unsafe { debug.EnableDebugLayer() };
    let gpu = level == "2" && debug.cast::<ID3D12Debug1>().is_ok_and(|debug| {
        unsafe { debug.SetEnableGPUBasedValidation(true) };
        true
    });
    log::info!("native renderer: D3D12 validation on{}", if gpu { ", with the GPU's" } else { "" });
    true
}

/// Dev tools: what D3D12's debug layer finds goes to the trace as it's
/// found, and what's wrong rather than only slow to the log too
fn report_validation(device: &ID3D12Device) {
    use windows::Win32::Graphics::Direct3D12::{ID3D12InfoQueue, D3D12_MESSAGE, D3D12_MESSAGE_SEVERITY_INFO, D3D12_MESSAGE_SEVERITY_MESSAGE, D3D12_MESSAGE_SEVERITY_WARNING};
    struct Queue(ID3D12InfoQueue);
    unsafe impl Send for Queue {}
    let Ok(queue) = device.cast::<ID3D12InfoQueue>() else { return };
    let queue = Queue(queue);
    std::thread::spawn(move || {
        let queue = queue;
        loop {
            let count = unsafe { queue.0.GetNumStoredMessages() };
            for index in 0..count {
                let mut size = 0usize;
                if unsafe { queue.0.GetMessage(index, None, &mut size) }.is_err() || size == 0 {
                    continue;
                }
                let mut bytes = vec![0u64; size.div_ceil(8)];
                let message = bytes.as_mut_ptr() as *mut D3D12_MESSAGE;
                if unsafe { queue.0.GetMessage(index, Some(message), &mut size) }.is_err() {
                    continue;
                }
                let message = unsafe { &*message };
                if message.Severity == D3D12_MESSAGE_SEVERITY_INFO || message.Severity == D3D12_MESSAGE_SEVERITY_MESSAGE {
                    continue;
                }
                let text = unsafe { std::ffi::CStr::from_ptr(message.pDescription as *const std::ffi::c_char) }.to_string_lossy().into_owned();
                if message.Severity != D3D12_MESSAGE_SEVERITY_WARNING {
                    log::warn!("native renderer: D3D12 {:?}: {text}", message.Severity);
                }
                trace(|| format!("D3D12 {:?} {:?}: {text}", message.Severity, message.ID));
            }
            unsafe { queue.0.ClearStoredMessages() };
            std::thread::sleep(Duration::from_millis(100));
        }
    });
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
        super::shader_cache::install();
        let validate = debug_layer();
        let factory: IDXGIFactory4 = unsafe { CreateDXGIFactory1() }.map_err(|error| format!("DXGI: {error}"))?;
        let (adapter, device) = hardware_adapter(&factory).map_err(|error| format!("adapter: {error}"))?;
        if validate {
            report_validation(&device);
        }
        super::gpu_census::install(&device);
        let desc = unsafe { adapter.GetDesc1() }.map_err(|error| error.to_string())?;
        let name = String::from_utf16_lossy(&desc.Description).trim_end_matches('\0').to_string();
        Ok(Device { factory, adapter, device, name })
    }
}

impl Gpu {
    pub fn new(device: &Device, width: u32, height: u32) -> Result<Self, String> {
        let queue: ID3D12CommandQueue = unsafe { device.device.CreateCommandQueue(&Default::default()) }.map_err(|error| format!("queue: {error}"))?;
        let fence: ID3D12Fence = unsafe { device.device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.map_err(|error| format!("fence: {error}"))?;
        let memory = device.adapter.cast::<IDXGIAdapter3>().ok();
        let backend = BackendContext { adapter: device.adapter.clone(), device: device.device.clone(), queue: queue.clone(), memory_allocator: Some(super::committed::allocator(&device.device, &queue)), protected_context: Protected::No };
        // Room for every program a file needs, as past Skia's 256 they were
        // made again while panning
        let mut options = gpu::ContextOptions::new();
        options.runtime_program_cache_size = 4096;
        // Masks of paths drawn again at the same scale, as tiles are, kept
        options.allow_path_mask_caching = true;
        let mut context = unsafe { gpu::direct_contexts::make_d3d(&backend, &options) }.ok_or("Skia couldn't start on D3D12")?;
        context.set_resource_cache_limit(config().cache_limit);
        let gpu = Self {
            context,
            device: device.device.clone(),
            queue,
            shared_fence: Arc::new(Fence(fence.clone())),
            fence,
            signalled: 0,
            done: None,
            size: (width.max(1), height.max(1)),
            adapter: format!("{} (D3D12)", device.name),
            memory,
        };
        log::info!("native renderer: started on {}, {:?}, {:.0} MB of GPU memory", gpu.adapter, config(), gpu.used() as f64 / 1e6);
        Ok(gpu)
    }

    /// Sends `wake` a `Msg::Gpu` each time the GPU passes a fence, so the
    /// render thread hands over what the GPU drew the moment it's done.
    /// Waiting a millisecond and looking again would be up to 16, as
    /// Windows wakes threads that sleep
    pub fn watch(&mut self, wake: Sender<Msg>, hub: Arc<Hub>) {
        use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
        let Ok(event) = (unsafe { CreateEventW(None, false, false, None) }) else {
            return;
        };
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (raw, stopped) = (event.0 as isize, stop.clone());
        let watching = std::thread::Builder::new().name("native-gpu-done".into()).spawn(move || {
            let event = windows::Win32::Foundation::HANDLE(raw as *mut _);
            loop {
                unsafe { WaitForSingleObject(event, INFINITE) };
                // The present thread shows a snapshot once it's drawn
                hub.poke();
                if stopped.load(std::sync::atomic::Ordering::Relaxed) || wake.send(Msg::Gpu).is_err() {
                    break;
                }
            }
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(event) };
        });
        if watching.is_ok() {
            self.done = Some((event, stop));
        } else {
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(event) };
        }
    }

    /// Signals the fence after all that's submitted so far, returning the
    /// value it will pass
    fn signal(&mut self) -> u64 {
        self.signalled += 1;
        if unsafe { self.queue.Signal(&self.fence, self.signalled) }.is_err() {
            // Without it, waiting is the way to know
            let _ = self.context.flush_submit_and_sync_cpu();
            return 0;
        }
        if let Some((event, _)) = &self.done {
            let _ = unsafe { self.fence.SetEventOnCompletion(self.signalled, *event) };
        }
        self.signalled
    }

    /// The fence value the GPU has passed
    fn passed(&self) -> u64 {
        unsafe { self.fence.GetCompletedValue() }
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

    /// A surface outside Skia's cache, whose memory goes as soon as nothing
    /// holds it, rather than staying in the cache to be used again
    fn own_surface(&mut self, width: i32, height: i32, samples: usize) -> Option<Surface> {
        let info = ImageInfo::new((width, height), ColorType::RGBA8888, AlphaType::Premul, None);
        gpu::surfaces::render_target(&mut self.context, gpu::Budgeted::No, &info, samples, SurfaceOrigin::TopLeft, None, false, None)
    }

    /// Sends what's drawn on `surface` to the GPU, never flushed without
    /// being submitted. Skia's D3D backend keeps the tables telling shaders
    /// which textures to read till it submits, found by the slots of the
    /// textures in them, and a texture let go hands its slot at once to the
    /// next one made: drawn before a submit, that one could be read as the
    /// one let go, another image entirely. Each submit starts the tables
    /// afresh, and within a flush every texture is made before any is read
    fn flush(&mut self, surface: &mut Surface) {
        let _ = self.context.flush_and_submit_surface(surface, None);
    }
}

pub struct Painter {
    gpu: Gpu,
    kit: Kit,
    scene: Scene,
    /// Figma's render tree, once frames of it come
    tree: Option<Tree>,
    units: Vec<Unit>,
    /// By unit, zoom level and whether edges are anti-aliased
    pictures: HashMap<(usize, bool, i32, bool), Recorded>,
    images: HashMap<String, Img>,
    /// Widths images were asked for at
    asked: HashMap<String, f64>,
    /// When each image was last needed near the view
    wanted: HashMap<String, Instant>,
    /// Images that couldn't be had, and when
    failed: HashMap<String, Instant>,
    tiles: HashMap<TileKey, Tile>,
    /// Where tiles are drawn before they're kept, by the margin around the
    /// tile and how many samples a pixel takes along each side
    scratch: Vec<(i32, i32, Surface)>,
    /// A tile partly drawn, to draw the rest of when there's nothing sooner
    pending: Option<Pending>,
    /// Tiles drawn, to time some of them
    made: u64,
    /// Units drawn into images of their own, by unit, picture level, whether
    /// anti-aliased and scale (as bits), and the bytes they take
    rasters: HashMap<(usize, bool, i32, bool, u32), Raster>,
    raster_bytes: usize,
    rasters_off: bool,
    /// Draws the whole view, with LIGMA_NATIVE_VELLO set (see vello.rs),
    /// once it's started
    vello: Option<super::vello::Vello>,
    vello_starting: Option<std::sync::mpsc::Receiver<Result<super::vello::Vello, String>>>,
    /// Images units drew with when Vello last drew them, by unit, as
    /// pictures keep them
    vello_uses: HashMap<(usize, bool), Vec<(String, f64)>>,
    /// Units encoded for Vello, by unit and level, see `Encoded`
    fragments: HashMap<(usize, bool, i32), Encoded>,
    /// Images of units made for the tile or view being drawn
    rasters_made: u32,
    /// Textures to use again once the present thread has let go of them
    spare: Vec<Arc<Tex>>,
    /// What the last snapshot handed over had, its frame, and the fence the
    /// GPU passes once it's drawn it
    published: Option<(u64, u64, u64)>,
    /// Draws snapshots as the present thread does, for dev tools
    patterns: Patterns,
    /// Counts up, for each thing drawn to be told apart from what it was
    serial: u64,
    /// The whole view, drawn while the zoom changes, while things change or
    /// before its tiles are drawn
    whole: Option<Whole>,
    /// The whole view drawn before it, while nothing in it has changed
    /// since: shown under it where the view's moved past it, the same at
    /// another zoom, so the view never waits on the next
    last_whole: Option<Layer>,
    hub: Arc<Hub>,
    /// When the view was last drawn, and whether anything was since
    /// Part of a pixel the view is drawn off the pixel grid by, see `fractional`
    frac: (f32, f32),
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
    /// Dev tools: see `verify_tile`
    verifying: bool,
    verified: u64,
    /// The zooms of the levels further out the view has now
    pyramid: Vec<u32>,
    /// When a frame was last drawn, and whether memory was given back since
    drawn: Instant,
    released: bool,
    freed: bool,
    drew: bool,
    /// Figma's checkerboard, and the pixel grid it draws when zoomed in
    checker: Option<skia_safe::RuntimeEffect>,
    grid: Option<(skia_safe::RuntimeEffect, skia_safe::RuntimeEffect)>,
    view: Option<View>,
    /// The last zoom (pixels per unit) and when it changed
    zoom: (f32, Instant),
    /// When the view last moved
    moved: Instant,
    /// Where the view's center was, how wide it was and when, in units, how
    /// fast it's moving, in units a second, and how fast it's widening, in
    /// doublings a second
    motion: Option<(f64, f64, f64, Instant)>,
    velocity: (f64, f64),
    widening: f64,
    /// How long the whole view last took to draw
    whole_took: Duration,
    /// The last frame of Figma's render tree applied
    seq: u64,
    /// Units drawn small at powers of two, on pages of their own, for the
    /// view while it moves: by unit and power
    thumbs: HashMap<(usize, bool, i32), Thumb>,
    thumb_pages: Vec<ThumbPage>,
    thumbs_made: usize,
    /// Thumbs are being made ahead, and there's no room left for them
    thumbs_ahead: bool,
    thumbs_full: bool,
    thumbs_derived: usize,
    pages_cleared: usize,
    /// The last thumb asked for wasn't made, for want of time
    thumb_later: bool,
    thumb_serial: u64,
    /// Thumbs are made from pictures till then, this draw
    thumb_deadline: Option<Instant>,
    /// Pages of thumbs as they were before this draw drew on them, and the
    /// thumbs made by then
    page_images: Vec<Option<(Image, u64)>>,
    /// The view's corner in device pixels and its zoom, as bits, as last drawn
    at: (i32, i32, u32),
    /// When a frame last changed what's drawn
    edited: Instant,
    /// When Figma sent the latest frame, in milliseconds since the epoch,
    /// until what it changed is handed over, for the trace
    sent: Option<f64>,
    frame: u64,
    tint: bool,
    dirty: bool,
    requests: Sender<Request>,
    /// Nodes' custom effects to run, see effects.rs
    effects: Sender<Job>,
    /// What effects drew, whole, kept on their thread
    kept: Kept,
    /// Effects waiting on images the node draws, and those images, each
    /// run again once one of its own comes
    effects_waiting: Vec<(String, f64, Vec<String>)>,
    /// Where units were drawn without images or effects not here yet, or
    /// with them at another size, by image: drawn again once they come,
    /// though the pictures that drew them may have gone by then
    waiting_areas: HashMap<String, Vec<Rect>>,
    /// Units that drew with each image, missing or at some size, which are
    /// drawn again once it comes
    image_units: HashMap<String, std::collections::HashSet<(usize, bool)>>,
    /// Counts up for a unit when what it draws changes but what it is
    /// doesn't, as when an image it drew comes
    versions: HashMap<(usize, bool), u64>,
    /// Counts up for a unit when an image it drew comes sharper: what's
    /// drawn of it is out of date for tiles, but thumbs, which are drawn
    /// small anyway, show the same thing
    sharpened: HashMap<(usize, bool), u64>,
    version_next: u64,
    /// Changes with units or their versions, so `check` knows when to look
    revision: u64,
    checked_revision: u64,
    /// Each unit as of the last `check`, in the order they're drawn: what
    /// it is, where, and its version
    checked: Vec<Mark>,
    /// Textures let go of, freed once the queue passes each fence value
    graves: Vec<(u64, Vec<windows::Win32::Graphics::Direct3D12::ID3D12Resource>)>,
}

/// A unit as `check` knows it: its slot and section, bounds as bits, a
/// hash of its version and where it's placed (see `Painter::mark`), and
/// when an image it drew last came sharper
type Mark = ((usize, bool), [u32; 4], u64, u64);

impl Painter {
    pub fn new(gpu: Gpu, requests: Sender<Request>, effects: Sender<Job>, kept: Kept, hub: Arc<Hub>) -> Self {
        let vello_starting = std::env::var_os("LIGMA_NATIVE_VELLO").filter(|value| value != "0").map(|_| super::vello::start(&gpu.device));
        Self {
            vello: None,
            vello_starting,
            vello_uses: HashMap::new(),
            fragments: HashMap::new(),
            gpu,
            kit: Kit::new(),
            scene: Scene::default(),
            tree: None,
            units: Vec::new(),
            pictures: HashMap::new(),
            images: HashMap::new(),
            asked: HashMap::new(),
            wanted: HashMap::new(),
            failed: HashMap::new(),
            tiles: HashMap::new(),
            scratch: Vec::new(),
            pending: None,
            made: 0,
            rasters: HashMap::new(),
            raster_bytes: 0,
            rasters_made: 0,
            rasters_off: super::dev() && std::env::var("LIGMA_NATIVE_RASTERS").is_ok_and(|value| value == "0"),
            spare: Vec::new(),
            published: None,
            patterns: Patterns::new(),
            serial: 0,
            whole: None,
            last_whole: None,
            hub,
            drew: false,
            live: None,
            fitted: Instant::now(),
            fresh: false,
            frac: (0.0, 0.0),
            average: skia_safe::RuntimeEffect::make_for_shader(AVERAGE, None)
                .map_err(|error| log::warn!("native renderer: average: {error}"))
                .ok(),
            backdrops: Vec::new(),
            config: config(),
            cleaned: Instant::now(),
            verifying: false,
            verified: 0,
            pyramid: Vec::new(),
            drawn: Instant::now(),
            released: true,
            freed: true,
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
            motion: None,
            velocity: (0.0, 0.0),
            widening: 0.0,
            whole_took: Duration::ZERO,
            seq: 0,
            thumbs: HashMap::new(),
            thumb_pages: Vec::new(),
            thumbs_made: 0,
            thumbs_ahead: false,
            thumbs_full: false,
            thumbs_derived: 0,
            pages_cleared: 0,
            thumb_later: false,
            thumb_serial: 0,
            thumb_deadline: None,
            page_images: Vec::new(),
            at: (0, 0, 0),
            edited: Instant::now().checked_sub(EDITING).unwrap_or_else(Instant::now),
            sent: None,
            frame: 0,
            tint: false,
            dirty: true,
            requests,
            effects,
            effects_waiting: Vec::new(),
            waiting_areas: HashMap::new(),
            image_units: HashMap::new(),
            versions: HashMap::new(),
            sharpened: HashMap::new(),
            version_next: 0,
            revision: 1,
            checked_revision: 0,
            checked: Vec::new(),
            graves: Vec::new(),
            kept,
        }
    }

    /// Frees textures let go of (see `Tex`) once the GPU's done with them:
    /// all asked of it so far is submitted, and freed once the queue passes
    /// a fence signalled after
    fn bury(&mut self) {
        let buried = super::present::buried();
        if !buried.is_empty() {
            let _ = self.gpu.context.flush_and_submit();
            let value = self.gpu.signal();
            self.graves.push((value, buried));
        }
        let passed = self.gpu.passed();
        self.graves.retain(|(value, _)| *value > passed);
    }

    pub fn run(mut self, messages: Receiver<Msg>) {
        super::profile::register();
        // How long until there's more to draw, if there is
        let mut next: Option<Duration> = Some(Duration::ZERO);
        loop {
            self.bury();
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
            // The latest frame is handed over before more is drawn, which
            // takes a while, and again with what's drawn
            self.dirty = false;
            self.publish();
            next = self.fill();
            self.publish();
            if std::mem::take(&mut self.drew) {
                self.drawn = Instant::now();
                self.released = false;
                self.freed = false;
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
                self.trim();
            } else if !self.freed && self.drawn.elapsed() > FREE_AFTER {
                self.freed = true;
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

    /// The levels further out than the coarse one, each a power of two
    /// below the last, over what the view would show zoomed out to it, but
    /// only where there's anything, down to where all there is fits in
    /// view. Zooming out, the present thread scales them to the view while
    /// it's drawn exactly, rather than wait for that. Their tiles, the
    /// furthest level first, as it covers the most and has the fewest, and
    /// each level's area and zoom
    fn pyramid(&self, center: (f32, f32), width: i32, height: i32, cp: f32) -> (Vec<TileKey>, Vec<(Rect, f32)>) {
        let Some(content) = self.content() else { return (Vec::new(), Vec::new()) };
        let mut levels = Vec::new();
        // Powers of two, whatever the zoom, so zooming in a little keeps them
        let mut pk = (cp / 2.0).log2().floor().exp2();
        for _ in 0..PYRAMID_LEVELS {
            // The furthest is all there is, wherever the view is, so no
            // zoom out ever finds nothing drawn
            if content.width() * pk <= width as f32 && content.height() * pk <= height as f32 {
                levels.push((content, pk));
                break;
            }
            let (w, h) = (width as f32 / pk, height as f32 / pk);
            let mut area = Rect::from_xywh(center.0 - w / 2.0, center.1 - h / 2.0, w, h);
            if area.intersect(content) {
                levels.push((area, pk));
            }
            pk /= 2.0;
        }
        levels.reverse();
        let mut keys = Vec::new();
        let mut kept = Vec::new();
        for (area, pk) in levels {
            let (x, y) = ((area.left * pk).floor() as i32, (area.top * pk).floor() as i32);
            let (r, b) = ((area.right * pk).ceil() as i32, (area.bottom * pk).ceil() as i32);
            let level = Self::tiles_over(x, y, r - x - 1, b - y - 1, pk.to_bits(), 0);
            if keys.len() + level.len() > PYRAMID_TILES {
                break;
            }
            keys.extend(level);
            kept.push((area, pk));
        }
        (keys, kept)
    }

    /// Where there's anything drawn, in units
    fn content(&self) -> Option<Rect> {
        let mut units = self.units.iter().map(|unit| unit.bounds);
        let first = units.next()?;
        Some(units.fold(first, |all, bounds| Rect::join2(all, bounds)))
    }

    /// Draws what the view needs next: the whole view while the zoom changes or when most of it is missing,
    /// otherwise the tile it needs most. Tiles in view come first, quickly,
    /// then those around it, so panning finds them ready, then exactly once
    /// the view has been still a moment. Returns how long until there's
    /// more to draw, if there is
    fn fill(&mut self) -> Option<Duration> {
        self.check();
        let view = self.view.clone()?;
        let (px, preview, width, height) = view.drawn(self.gpu.size);
        if !px.is_finite() || px <= 0.0 {
            return None;
        }
        if px != self.zoom.0 {
            self.zoom = (px, Instant::now());
        }
        self.frame += 1;
        let pxb = px.to_bits();
        // A preview's pixels are magnified, so it's drawn from the pixel
        // the window's corner is in
        let snap = |at: f64| if preview { at.floor() } else { at.round() } as i32;
        let ox = snap(view.x * px as f64);
        let oy = snap(view.y * px as f64);
        let in_view = Self::tiles_over(ox, oy, width, height, pxb, 0);
        let mut around: Vec<TileKey> = Self::tiles_over(ox, oy, width, height, pxb, 1).into_iter().filter(|key| !in_view.contains(key)).collect();
        // Where a pan is headed, so its tiles are drawn before it gets there
        let moving = self.moved.elapsed() < Duration::from_millis(150);
        let (ax, ay) = if moving { ((self.velocity.0 * LOOKAHEAD * px as f64) as i32, (self.velocity.1 * LOOKAHEAD * px as f64) as i32) } else { (0, 0) };
        if (ax, ay) != (0, 0) {
            let ahead: Vec<TileKey> = Self::tiles_over(ox + ax, oy + ay, width, height, pxb, 0).into_iter().filter(|key| !in_view.contains(key) && !around.contains(key)).collect();
            around.extend(ahead);
        }
        if in_view.len() + around.len() > self.config.max_tiles {
            around.truncate(self.config.max_tiles.saturating_sub(in_view.len()));
        }
        if self.pending.as_ref().is_some_and(|pending| !in_view.contains(&pending.key) && !around.contains(&pending.key)) {
            self.pending = None;
        }
        // The coarse level over the view, and around it and ahead
        let cp = coarse(px);
        let cpb = cp.to_bits();
        let scale = cp / px;
        let at = |x: i32, w: i32| ((x as f32 * scale).floor() as i32, (w as f32 * scale).ceil() as i32);
        let ((cx, cw), (cy, ch)) = (at(ox, width), at(oy, height));
        let coarse_view = Self::tiles_over(cx, cy, cw, ch, cpb, 0);
        let mut coarse_around: Vec<TileKey> = Self::tiles_over(cx, cy, cw, ch, cpb, 1).into_iter().filter(|key| !coarse_view.contains(key)).collect();
        if (ax, ay) != (0, 0) {
            let (cax, cay) = ((ax as f32 * scale) as i32, (ay as f32 * scale) as i32);
            let ahead: Vec<TileKey> = Self::tiles_over(cx + cax, cy + cay, cw, ch, cpb, 0).into_iter().filter(|key| !coarse_view.contains(key) && !coarse_around.contains(key)).collect();
            coarse_around.extend(ahead);
        }
        coarse_around.truncate(COARSE_TILES.saturating_sub(coarse_view.len()));
        let center = ((ox + width / 2) as f32 / px, (oy + height / 2) as f32 / px);
        let (pyramid, levels) = self.pyramid(center, width, height, cp);
        self.pyramid = levels.iter().map(|(_, pk)| pk.to_bits()).collect();
        let frame = self.frame;
        for key in in_view.iter().chain(&around).chain(&coarse_view).chain(&coarse_around).chain(&pyramid) {
            if let Some(tile) = self.tiles.get_mut(key) {
                tile.used = frame;
            }
        }
        self.at = (ox, oy, pxb);
        let missing = in_view.iter().filter(|key| !self.tiles.contains_key(key)).count();
        let behind = in_view.iter().any(|key| self.tiles.get(key).is_some_and(|tile| tile.stale));
        // While something's changing, what changed is drawn into the whole
        // view at once, each frame, as Figma draws it, and tiles wait
        let editing = EDITING.saturating_sub(self.edited.elapsed());
        // While the zoom changes, Figma changes what it shows of some nodes
        // each frame, which isn't an edit to keep up with exactly
        let settle = SETTLE.saturating_sub(self.zoom.1.elapsed());
        if !editing.is_zero() && settle.is_zero() && (behind || missing > 0 || self.whole.is_some()) {
            let current = self.whole.as_ref().filter(|whole| whole.at == (ox, oy, pxb) && !whole.stale);
            // One drawn at a time, what changes meanwhile drawn together
            // once the GPU's done
            if self.whole_busy() {
                return Some(Duration::from_millis(1));
            }
            match current {
                Some(whole) if whole.patch.is_empty() => return Some(editing),
                Some(_) => self.patch_whole(),
                None => self.draw_whole(ox, oy, width, height, px, true),
            }
            self.dirty = true;
            return Some(Duration::ZERO);
        }
        if self.whole.as_ref().is_some_and(|whole| !whole.patch.is_empty() && whole.at == (ox, oy, pxb) && !whole.stale) {
            if self.whole_busy() {
                return Some(Duration::from_millis(1));
            }
            // What changed that isn't an edit, an image come or the like,
            // is drawn from thumbs, as the view is meanwhile, rather than
            // exactly, which for a big image is hundreds of milliseconds,
            // as tiles are drawn exactly after
            self.draw_whole(ox, oy, width, height, px, true);
            self.dirty = true;
            return Some(Duration::ZERO);
        }
        let whole_here = self.whole.as_ref().is_some_and(|whole| whole.at == (ox, oy, pxb) && !whole.stale && whole.patch.is_empty());
        // While the zoom changes, tiles of it are of no use a moment later,
        // so the whole view is drawn instead, as Figma draws every frame
        if !settle.is_zero() {
            // One drawn at the half octave this zoom's in that still covers
            // the view is scaled to it
            let half = half_octave(px);
            // Where the view will be by when another could be drawn, so the
            // next is ready before the view leaves this one
            let lag = (self.whole_took.as_secs_f64() + 0.02).min(0.1);
            let view_area = self.ahead(Rect::from_xywh(ox as f32 / px, oy as f32 / px, width as f32 / px, height as f32 / px), lag);
            let covered = self.whole.as_ref().is_some_and(|whole| {
                let layer = &whole.layer;
                let drawn = Rect::from_xywh(layer.x as f32 / half, layer.y as f32 / half, layer.tex.width as f32 / half, layer.tex.height as f32 / half);
                layer.px == half && !whole.stale && whole.patch.is_empty() && drawn.left <= view_area.left && drawn.top <= view_area.top && drawn.right >= view_area.right && drawn.bottom >= view_area.bottom
            });
            if !whole_here && !covered {
                if self.whole_busy() {
                    return Some(Duration::from_millis(1));
                }
                self.draw_whole(ox, oy, width, height, px, true);
                self.dirty = true;
                return Some(Duration::ZERO);
            }
            return Some(settle);
        }
        // Frames are composed only of what's up to date, all of the view,
        // so where that isn't drawn yet the whole view's drawn first
        if cover(&self.layers(false), self.content(), &view, self.gpu.size).1 && !self.whole_at(&view) {
            if self.whole_busy() {
                return Some(Duration::from_millis(1));
            }
            self.draw_whole(ox, oy, width, height, px, true);
            self.dirty = true;
            return Some(Duration::ZERO);
        }
        let ready = |tiles: &HashMap<TileKey, Tile>, key: &TileKey| tiles.get(key).is_some_and(|tile| !tile.stale && (!tile.draft || key.px != pxb));
        let mut order: Vec<(TileKey, bool)> = Vec::new();
        // Where tiles are missing, the coarse level first: it covers four
        // or more tiles' worth at once, so a pan finds something drawn
        if missing > 0 && !whole_here {
            order.extend(coarse_view.iter().copied().filter(|key| !self.tiles.contains_key(key)).map(|key| (key, false)));
        }
        order.extend(in_view.iter().copied().filter(|key| !self.tiles.contains_key(key)).map(|key| (key, false)));
        // Then the furthest level, all there is, which zooming out shows
        // till what's nearer is drawn, so none ever waits. Once drawn it's
        // shown, while out of date too, so it's drawn again only when idle
        let furthest = levels.first().map(|(_, pk)| pk.to_bits());
        order.extend(pyramid.iter().copied().filter(|key| Some(key.px) == furthest && !self.tiles.contains_key(key)).map(|key| (key, false)));
        // An out of date tile is drawn again as well as it was, the whole
        // view standing in meanwhile
        order.extend(in_view.iter().filter_map(|key| self.tiles.get(key).filter(|tile| tile.stale || tile.draft).map(|tile| (*key, tile.exact))));
        order.extend(around.iter().copied().filter(|key| !ready(&self.tiles, key)).map(|key| (key, false)));
        order.extend(coarse_view.iter().chain(&coarse_around).copied().filter(|key| !ready(&self.tiles, key) && key.px != pxb).map(|key| (key, false)));
        // And the levels between, so zooming out shows them less blurred
        order.extend(pyramid.iter().copied().filter(|key| !self.tiles.contains_key(key)).map(|key| (key, false)));
        let rough: Vec<TileKey> = in_view.iter().copied().filter(|key| self.tiles.get(key).is_some_and(|tile| !tile.stale && !tile.draft && !tile.exact)).collect();
        // Nor while the page changes, as while a file loads, which would
        // draw them again soon anyway
        let refine = REFINE_AFTER.saturating_sub(self.moved.elapsed().min(self.edited.elapsed()));
        if refine.is_zero() {
            order.extend(rough.iter().map(|&key| (key, true)));
        }
        let mut seen = std::collections::HashSet::new();
        order.retain(|item| seen.insert(*item));
        if let Some(&(key, exact)) = order.first() {
            if let Some(tile) = self.draw_tile(key, exact, key.px != pxb && !self.pyramid.contains(&key.px)) {
                self.put_tile(key, tile);
                self.verify_tile(key);
                self.drew = true;
            }
            self.evict(pxb, cpb);
            self.dirty = true;
            return Some(Duration::ZERO);
        }
        // Supersampled scratch is big, and only drawn on once the view is
        // still, so it goes once that's done
        self.scratch.retain(|(_, ss, _)| *ss == 1);
        // All there: the whole view drawn for the meantime can go
        if missing == 0 && !behind {
            if let Some(whole) = self.whole.take() {
                self.retire_whole(whole);
                self.dirty = true;
            }
        }
        self.update_reason(&view, px, width, height);
        if rough.is_empty() && self.moved.elapsed() > Duration::from_millis(300) {
            if self.prewarm(&view, px, &levels) {
                return Some(Duration::ZERO);
            }
            if let Some(&key) = pyramid.iter().find(|key| !self.tiles.get(key).is_some_and(|tile| !tile.stale)) {
                if let Some(tile) = self.draw_tile(key, false, false) {
                    self.put_tile(key, tile);
                    self.drew = true;
                }
                self.evict(pxb, cpb);
                self.dirty = true;
                return Some(Duration::ZERO);
            }
        }
        (!rough.is_empty()).then_some(refine)
    }

    /// Draws the whole view, (`ox`, `oy`) its corner and `width` by `height`
    /// its size in pixels at `px` a unit, with a margin for background blurs
    /// near its edges to see what they blur, for frames where tiles of the
    /// zoom aren't drawn or up to date
    /// Vello, once it's started and while it works
    fn vello_ready(&mut self) -> bool {
        if let Some(starting) = &self.vello_starting {
            match starting.try_recv() {
                Ok(started) => {
                    self.vello = started.ok();
                    self.vello_starting = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.vello_starting = None,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.vello.as_ref().is_some_and(|vello| vello.broken()) {
            log::warn!("native renderer: Vello stopped after an error, Skia draws the view");
            self.vello = None;
        }
        self.vello.is_some()
    }

    /// The whole view drawn by Vello at the zoom it's at, every path of it
    /// each time, over what `draw_whole` draws. False where it can't be,
    /// for Skia to draw it
    fn draw_whole_vello(&mut self, ox: i32, oy: i32, width: i32, height: i32, px: f32) -> bool {
        let _purpose = super::gpu_census::purpose("whole");
        let started = Instant::now();
        let at = (ox, oy, px.to_bits());
        let lag = (self.whole_took.as_secs_f64() + 0.03).min(0.12) * 2.0;
        let area = self.ahead(Rect::from_xywh(ox as f32 / px, oy as f32 / px, width as f32 / px, height as f32 / px), lag);
        let area = area.with_outset((area.width() / 16.0, area.height() / 16.0));
        let (ox, oy, width, height) = {
            let (left, top) = ((area.left * px).floor() as i32, (area.top * px).floor() as i32);
            let step = |size: i32| (size + 127) / 128 * 128;
            (left, top, step((area.right * px).ceil() as i32 - left), step((area.bottom * px).ceil() as i32 - top))
        };
        let view_area = Rect::from_xywh(ox as f32 / px, oy as f32 / px, width as f32 / px, height as f32 / px);
        let margin = self.reach(view_area, px);
        let reach = view_area.with_outset((margin as f32 / px, margin as f32 / px));
        let (w, h) = (width + margin * 2, height + margin * 2);
        match self.vello_view(ox - margin, oy - margin, w, h, px, reach) {
            Ok((tex, counts, stats)) => {
                let fence = self.gpu.signal();
                trace(|| format!("  whole vello px={px:.3} {stats} {:.1}ms", started.elapsed().as_secs_f64() * 1000.0));
                self.hand_whole(Layer { tex, x: ox - margin, y: oy - margin, px, counts, kind: LayerKind::Whole }, fence, at);
                self.whole_took = started.elapsed();
                self.drew = true;
                true
            }
            Err(why) => {
                trace(|| format!("  whole vello px={px:.3} can't: {why}"));
                false
            }
        }
    }

    /// The `w` by `h` device pixels at `px` whose corner is at (`left`,
    /// `top`), of units over `reach`, drawn by Vello into a texture of the
    /// kind the present thread shows, submitted, and how long that took
    fn vello_view(&mut self, left: i32, top: i32, w: i32, h: i32, px: f32, reach: Rect) -> Result<(Arc<Tex>, Counts, String), String> {
        // Drawn again where it ran out of room in Vello's buffers, which
        // then have more, before it's shown
        for _ in 0..8 {
            let (tex, counts, stats) = self.vello_view_once(left, top, w, h, px, reach)?;
            let vello = self.vello.as_mut().ok_or("no Vello")?;
            let short = vello.check()?;
            if let Some(why) = short {
                trace(|| format!("  vello ran out of room, drawing again: {why}"));
            } else {
                let c = &vello::low_level::vello_encoding::CAPACITY;
                let r = |a: &std::sync::atomic::AtomicU32| a.load(std::sync::atomic::Ordering::Relaxed) >> 10;
                return Ok((
                    tex,
                    counts,
                    format!(
                        "{stats} checked={:.1}ms gpu:{} room(k)=bin {} ptcl {} tile {} segc {} seg {} blend {} lines {}",
                        vello.waited.as_secs_f64() * 1000.0,
                        vello.stages,
                        r(&c.binning),
                        r(&c.ptcl),
                        r(&c.tile),
                        r(&c.seg_counts),
                        r(&c.segments),
                        r(&c.blend),
                        r(&c.lines)
                    ),
                ));
            }
        }
        Err("out of room in Vello's buffers".into())
    }

    fn vello_view_once(&mut self, left: i32, top: i32, w: i32, h: i32, px: f32, reach: Rect) -> Result<(Arc<Tex>, Counts, String), String> {
        let started = Instant::now();
        let Some(mut vello) = self.vello.take() else { return Err("no Vello".into()) };
        let _ = self.gpu.context.flush_and_submit();
        let mut post = skia_safe::Matrix::translate((-left as f32, -top as f32));
        post.pre_scale((px, px), None);
        let mut asked = Vec::new();
        let (mut units, mut counts, mut inexact, mut skia_took, mut kinds) = (0, Counts::default(), 0, Duration::ZERO, String::new());
        let level = super::vello::level(px);
        let stamps: Vec<u64> = self
            .units
            .iter()
            .map(|unit| {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                self.mark(unit).hash(&mut hasher);
                self.sharpened.get(&(unit.slot, unit.section)).hash(&mut hasher);
                if let (Some(tree), Some(place)) = (&self.tree, &unit.place) {
                    tree.content(place.id).hash(&mut hasher);
                }
                hasher.finish()
            })
            .collect();
        let stamped = started.elapsed();
        let (mut put_took, mut append_took) = (Duration::ZERO, Duration::ZERO);
        let mut fragments = std::mem::take(&mut self.fragments);
        // Image fills' textures the view draws, by key
        let mut pictures: Vec<(String, u32)> = Vec::new();
        let (mut reused, mut made) = (0, 0);
        // Blurs, drawn and put in place for the view
        let mut plan = super::vello::Plan::new(w);
        let whole = vello::kurbo::Rect::new(0.0, 0.0, w as f64, h as f64);
        let (scene, failed, fell_back, mut pages) = match self.tree.as_ref() {
            None => (vello::Scene::new(), Some("not a tree".to_string()), 0, Vec::new()),
            Some(tree) => {
                let mut encoder = super::vello::Encoder::new(tree, &self.images, &self.kit, &mut vello, &self.gpu.device, &mut self.gpu.context, w, h);
                let now = Instant::now();
                // Dev tools: every unit encoded for the view
                let fresh = std::env::var_os("LIGMA_NATIVE_VELLO_FRESH").is_some();
                for (unit, &stamp) in self.units.iter().zip(&stamps) {
                    if !Rect::intersects2(&unit.bounds, &reach) || unit.bounds.width().max(unit.bounds.height()) * px < 0.5 {
                        continue;
                    }
                    let Some(place) = &unit.place else {
                        encoder.failed = Some("not a tree".into());
                        break;
                    };
                    let key = (unit.slot, unit.section, level);
                    if fresh {
                        encoder.place(place, &post);
                        let (mut effects, mut scene) = (std::mem::take(&mut encoder.effects), std::mem::take(&mut encoder.scene));
                        if let Err(why) = encoder.put(&mut plan, &mut effects, scene.encoding_mut(), vello::kurbo::Affine::IDENTITY, whole, 0) {
                            encoder.failed = Some(why);
                        }
                        encoder.scene = scene;
                        pictures.extend(encoder.pictures.drain(..).map(|(key, level, _)| (key, level)));
                        asked.push(((unit.slot, unit.section), unit.bounds, std::mem::take(&mut encoder.asked)));
                        units += 1;
                        if encoder.failed.is_some() {
                            break;
                        }
                        continue;
                    }
                    // Kept while it's of the unit as it is, and the textures of
                    // the images it draws are those Vello has
                    let images = &self.images;
                    let current = fragments.get(&key).is_some_and(|encoded| {
                        encoded.stamp == stamp
                            && match &encoded.made {
                                Ok(fragment) => fragment.pictures.iter().all(|(key, level, id)| {
                                    images.get(key).and_then(|img| encoder.vello.picture(key, img, *level)).is_some_and(|data| data.data.id() == *id)
                                }),
                                _ => true,
                            }
                    });
                    let encoded = match fragments.get_mut(&key).filter(|_| current) {
                        Some(encoded) => {
                            reused += 1;
                            encoded
                        }
                        None => {
                            made += 1;
                            let fragment = encoder.fragment(place, (unit.bounds.left, unit.bounds.top), level);
                            let made = match encoder.failed.take() {
                                Some(why) => Err(why),
                                None => Ok(fragment),
                            };
                            fragments.insert(key, Encoded { stamp, made, used: now });
                            fragments.get_mut(&key).unwrap()
                        }
                    };
                    encoded.used = now;
                    match &mut encoded.made {
                        Ok(fragment) => {
                            let at = fragment.at(px, left, top);
                            let timer = Instant::now();
                            // What Skia draws in it, drawn for the view, asks
                            // about images of its own
                            if let Err(why) = encoder.put(&mut plan, &mut fragment.effects, fragment.scene.encoding_mut(), at, whole, 0) {
                                encoder.failed = Some(why);
                            }
                            put_took += timer.elapsed();
                            let timer = Instant::now();
                            encoder.scene.append(&fragment.scene, Some(at));
                            append_took += timer.elapsed();
                            pictures.extend(fragment.pictures.iter().map(|(key, level, _)| (key.clone(), *level)));
                            encoder.counts += fragment.counts;
                            let mut unit_asked = fragment.asked.clone();
                            let mut drawn = std::mem::take(&mut encoder.asked);
                            unit_asked.missing.append(&mut drawn.missing);
                            unit_asked.sharper.append(&mut drawn.sharper);
                            unit_asked.used.append(&mut drawn.used);
                            asked.push(((unit.slot, unit.section), unit.bounds, unit_asked));
                        }
                        Err(why) => encoder.failed = Some(why.clone()),
                    }
                    units += 1;
                    if encoder.failed.is_some() {
                        break;
                    }
                }
                counts += encoder.counts;
                inexact = encoder.inexact;
                skia_took = encoder.skia_took;
                kinds = format!("{:?}", encoder.kinds);
                (std::mem::take(&mut encoder.scene), encoder.failed.take(), encoder.fell_back, encoder.take_pages())
            }
        };
        fragments.retain(|_, encoded| encoded.used.elapsed() < FRAGMENTS_KEPT);
        self.fragments = fragments;
        let encoded = started.elapsed();
        let used_pages = pages.len();
        for page in &mut pages {
            self.gpu.flush(page);
        }
        drop(pages);
        if let Some(why) = failed {
            self.vello = Some(vello);
            return Err(why);
        }
        pictures.sort_unstable();
        pictures.dedup();
        // Past what Vello's atlas holds, it'd draw from where other images are
        let area = vello.atlas_area(&pictures, used_pages);
        if area > ATLAS_MOST {
            self.vello = Some(vello);
            return Err(format!("images need {}M px of Vello's atlas", area >> 20));
        }
        let _ = self.gpu.context.flush_and_submit();
        // Dev tools: Skia's GPU work for the view done first, to time apart
        let skia_gpu = std::env::var_os("LIGMA_NATIVE_VELLO_SYNC").map(|_| {
            let at = Instant::now();
            let _ = self.gpu.context.flush_submit_and_sync_cpu();
            at.elapsed().as_secs_f64() * 1000.0
        });
        let blurs = plan.len();
        let scene_mb = {
            use std::mem::size_of_val as size;
            let e = scene.encoding();
            (size(&e.path_tags[..]) + size(&e.path_data[..]) + size(&e.draw_tags[..]) + size(&e.draw_data[..]) + size(&e.transforms[..]) + size(&e.styles[..])) as f64 / 1048576.0
        };
        let rendered = vello.render(&self.gpu.device, &self.gpu.queue, &scene, w, h, used_pages, &pictures, &plan);
        drop(plan);
        vello.spare = scene;
        vello.keep_pictures(&self.images);
        vello.prune();
        self.vello = Some(vello);
        let out = rendered.map_err(|error| {
            log::warn!("native renderer: Vello: {error}");
            error
        })?;
        let vello_took = started.elapsed() - encoded;
        // What Vello drew, unpremultiplied, copied into a texture of the
        // kind the present thread shows
        let tex = self.tex(w, h).ok_or("texture")?;
        let backend = tex.backend();
        let mut surface = self.target(&tex).ok_or("surface")?;
        let drawn = gpu::images::borrow_texture_from(&mut self.gpu.context, &out.backend(), SurfaceOrigin::TopLeft, ColorType::RGBA8888, AlphaType::Unpremul, None).ok_or("Vello's texture")?;
        let mut paint = Paint::default();
        paint.set_blend_mode(skia_safe::BlendMode::Src);
        let all = Rect::from_wh(w as f32, h as f32);
        surface.canvas().draw_image_rect(&drawn, Some((&all, skia_safe::canvas::SrcRectConstraint::Fast)), all, &paint);
        let image = self.finish(&backend, false);
        drop(surface);
        drop(drawn);
        if image.is_none() {
            return Err("submit".into());
        }
        for (unit, bounds, asked) in asked {
            let super::vello::Asked { missing, sharper, used } = asked;
            let drew: Vec<String> = used.iter().chain(&missing).chain(&sharper).map(|(key, _)| key.clone()).collect();
            let images: Vec<(String, f64)> = missing.iter().chain(&sharper).cloned().collect();
            self.ask(&images);
            let waiting: Vec<String> = missing.into_iter().map(|(key, _)| key).collect();
            self.wait_for(&waiting, bounds);
            self.drew_with(drew, unit);
            let mut uses = used;
            uses.extend(images);
            self.vello_uses.insert(unit, uses);
        }
        let stats = format!(
            "{w}x{h} units={units} reused={reused} made={made} scene={scene_mb:.1}MB sizes={:?} blurs={blurs} skia_gpu={skia_gpu:?} inexact={inexact} pages={used_pages} fell_back={}k px encoded={:.1}ms (stamps {:.1} put {:.1} append {:.1}) skia={:.1}ms vello_cpu={:.1}ms (blurs {:.1}ms) vello={:.1}ms {}",
            super::vello::SIZES.iter().map(|n| n.swap(0, std::sync::atomic::Ordering::Relaxed)).collect::<Vec<_>>(),
            fell_back / 1000,
            encoded.as_secs_f64() * 1000.0,
            stamped.as_secs_f64() * 1000.0,
            put_took.as_secs_f64() * 1000.0,
            append_took.as_secs_f64() * 1000.0,
            skia_took.as_secs_f64() * 1000.0,
            self.vello.as_ref().map_or(0.0, |vello| vello.took.as_secs_f64() * 1000.0),
            self.vello.as_ref().map_or(0.0, |vello| vello.took_blurs.as_secs_f64() * 1000.0),
            vello_took.as_secs_f64() * 1000.0,
            kinds.replace('"', "")
        );
        Ok((tex, counts, stats))
    }

    fn draw_whole(&mut self, ox: i32, oy: i32, width: i32, height: i32, px: f32, quick: bool) {
        if quick && self.tree.is_some() && self.vello_ready() && self.draw_whole_vello(ox, oy, width, height, px) {
            return;
        }
        let _purpose = super::gpu_census::purpose("whole");
        let started = Instant::now();
        let at = (ox, oy, px.to_bits());
        self.rasters_made = 0;
        // Over where the view is headed too, as it's shown a frame or two
        // after it's drawn
        // As far as it'll be till the next is drawn and shown, which is
        // started before the view leaves this
        let lag = (self.whole_took.as_secs_f64() + 0.03).min(0.12) * 2.0;
        let area = self.ahead(Rect::from_xywh(ox as f32 / px, oy as f32 / px, width as f32 / px, height as f32 / px), lag);
        // With a little more, for the view to move into before another's
        // drawn
        let area = if quick { area.with_outset((area.width() / 16.0, area.height() / 16.0)) } else { area };
        // While the zoom changes, drawn at the half octave at or above it,
        // which frames scale to the view: Skia keeps what it
        // works out for a scale, path masks and glyphs, for the next frame
        // at it, and the view's drawn again only once it leaves what's drawn
        let px = if quick && !SETTLE.saturating_sub(self.zoom.1.elapsed()).is_zero() { half_octave(px) } else { px };
        let (ox, oy, width, height) = {
            let (left, top) = ((area.left * px).floor() as i32, (area.top * px).floor() as i32);
            // In steps, so the texture of one is used again for the next
            let step = |size: i32| (size + 127) / 128 * 128;
            (left, top, step((area.right * px).ceil() as i32 - left), step((area.bottom * px).ceil() as i32 - top))
        };
        let view_area = Rect::from_xywh(ox as f32 / px, oy as f32 / px, width as f32 / px, height as f32 / px);
        let margin = self.reach(view_area, px);
        let reach = view_area.with_outset((margin as f32 / px, margin as f32 / px));
        let mut pictures = Vec::new();
        let mut counts = Counts::default();
        let (mut thumbs_took, mut drawn_took, mut slowest) = (Duration::ZERO, Duration::ZERO, (Duration::ZERO, 0));
        if quick {
            self.thumb_deadline = Some(Instant::now() + THUMB_TIME);
        }
        for unit in 0..self.units.len() {
            let bounds = self.units[unit].bounds;
            if !Rect::intersects2(&bounds, &reach) {
                continue;
            }
            if bounds.width().max(bounds.height()) * px < 0.5 {
                counts.skipped += 1;
                continue;
            }
            if quick {
                let at = Instant::now();
                let thumb = self.thumb(unit, px);
                let took = at.elapsed();
                thumbs_took += took;
                if took > slowest.0 {
                    slowest = (took, unit);
                }
                if let Some((drawn, drawn_counts)) = thumb {
                    counts += drawn_counts;
                    pictures.push(drawn);
                    continue;
                }
                // Without its thumb yet, it's drawn as it is
                self.thumb_later = false;
            }
            let at = Instant::now();
            let level = self.level(&self.units[unit], px, false);
            if let Some((drawn, drawn_counts)) = self.drawn(unit, level, true, px, true) {
                counts += drawn_counts;
                pictures.push(drawn);
            }
            let took = at.elapsed();
            drawn_took += took;
            if took > slowest.0 {
                slowest = (took, unit);
            }
        }
        let pictures = self.unpage(pictures);
        let start = Instant::now();
        let before = if tracing() { gpu_counters() } else { [0.0; 6] };
        let Some(tex) = self.tex(width + margin * 2, height + margin * 2) else {
            return;
        };
        let backend = tex.backend();
        let Some(mut surface) = self.target(&tex) else {
            return;
        };
        let recorded = start - started;
        let canvas = surface.canvas();
        canvas.clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
        play(canvas, &pictures, ((margin - ox) as f32, (margin - oy) as f32), px);
        let played = start.elapsed();
        // The GPU draws it while what's next is made ready, and it's shown
        // once the GPU's done, what's shown now till then
        let image = self.finish(&backend, false);
        drop(surface);
        if image.is_none() {
            return;
        }
        let fence = self.gpu.signal();
        let vram = if tracing() { self.gpu.used() >> 20 } else { 0 };
        trace(|| {
            format!(
                "  whole px={px:.3} pictures={} thumbs={} made={} derived={} cleared={} thumbs_took={:.1}ms drawn_took={:.1}ms slowest={:.1}ms:{} margin={margin} recorded={:.1}ms played={:.1}ms {:.1}ms vram={vram} gpu={}",
                pictures.len(),
                pictures.iter().filter(|item| matches!(item, Drawn::Thumb(..))).count(),
                std::mem::take(&mut self.thumbs_made),
                std::mem::take(&mut self.thumbs_derived),
                std::mem::take(&mut self.pages_cleared),
                thumbs_took.as_secs_f64() * 1000.0,
                drawn_took.as_secs_f64() * 1000.0,
                slowest.0.as_secs_f64() * 1000.0,
                self.units.get(slowest.1).map(|unit| format!("{}x{}{}", (unit.bounds.width() * px) as i32, (unit.bounds.height() * px) as i32, if unit.alone { "" } else { "+under" })).unwrap_or_default(),
                recorded.as_secs_f64() * 1000.0,
                played.as_secs_f64() * 1000.0,
                start.elapsed().as_secs_f64() * 1000.0,
                {
                    let after = gpu_counters();
                    let [made, made_ms, programs, programs_ms, ..] = std::array::from_fn::<f64, 6, _>(|i| after[i] - before[i]);
                    let usage = self.gpu.context.resource_cache_usage();
                    let purgeable = self.gpu.context.resource_cache_purgeable_bytes();
                    format!(
                        "cache={}MB/{} purgeable={}MB made={made}/{made_ms:.1} programs={programs}/{programs_ms:.1} {}",
                        usage.resource_bytes >> 20,
                        usage.resource_count,
                        purgeable >> 20,
                        super::committed::take_kinds()
                    )
                }
            )
        });
        self.hand_whole(Layer { tex, x: ox - margin, y: oy - margin, px, counts, kind: LayerKind::Whole }, fence, at);
        self.whole_took = started.elapsed();
        self.drew = true;
    }

    /// The whole view's last drawing still on the GPU
    fn whole_busy(&self) -> bool {
        self.whole.as_ref().is_some_and(|whole| whole.fence > self.gpu.passed())
    }

    /// Makes `layer`, which the GPU draws till `fence` passes, the whole
    /// view of `at`
    fn hand_whole(&mut self, layer: Layer, fence: u64, at: (i32, i32, u32)) {
        self.serial += 1;
        let serial = self.serial;
        let mut older = None;
        let old = match self.whole.as_mut() {
            Some(whole) => {
                if !whole.stale && whole.patch.is_empty() {
                    older = Some(self.last_whole.replace(whole.layer.clone()));
                }
                whole.fence = fence;
                whole.serial = serial;
                whole.at = at;
                whole.stale = false;
                whole.patch.clear();
                Some(std::mem::replace(&mut whole.layer, layer))
            }
            None => {
                self.whole = Some(Whole { layer, at, stale: false, patch: Vec::new(), fence, serial });
                None
            }
        };
        if let Some(Some(older)) = older {
            self.retire(older.tex);
        }
        if let Some(old) = old {
            self.retire(old.tex);
        }
    }

    fn retire_whole(&mut self, whole: Whole) {
        self.retire(whole.layer.tex);
        self.drop_last_whole();
    }

    fn drop_last_whole(&mut self) {
        if let Some(last) = self.last_whole.take() {
            self.retire(last.tex);
        }
    }

    /// Draws again where things changed in the whole view: in place if the
    /// present thread doesn't have it, otherwise onto a copy, as snapshots
    /// handed over never change
    fn patch_whole(&mut self) {
        let _purpose = super::gpu_census::purpose("whole");
        let started = Instant::now();
        let Some(whole) = self.whole.as_mut() else {
            return;
        };
        let areas = std::mem::take(&mut whole.patch);
        let (tex, x, y, px) = (whole.layer.tex.clone(), whole.layer.x, whole.layer.y, whole.layer.px);
        let (width, height) = (tex.width, tex.height);
        if areas.len() > PATCHES {
            whole.stale = true;
            return;
        }
        let texture = skia_safe::IRect::from_xywh(x, y, width, height);
        let mut rects: Vec<skia_safe::IRect> = Vec::new();
        let mut reach: Option<Rect> = None;
        for area in &areas {
            let device: skia_safe::IRect = Rect::from_ltrb(area.left * px, area.top * px, area.right * px, area.bottom * px).round_out();
            let device = device.with_outset((2, 2));
            let Some(device) = skia_safe::IRect::intersect(&device, &texture) else {
                continue;
            };
            rects.push(device);
            reach = Some(match reach {
                Some(mut all) => {
                    all.join(area);
                    all
                }
                None => *area,
            });
        }
        let Some(reach) = reach else {
            return;
        };
        let reach = reach.with_outset((3.0 / px, 3.0 / px));
        let near: Vec<Rect> = areas.iter().map(|area| area.with_outset((3.0 / px, 3.0 / px))).collect();
        // Units under the areas changed, not all between them
        let under: Vec<usize> = (0..self.units.len())
            .filter(|&unit| {
                let bounds = self.units[unit].bounds;
                Rect::intersects2(&bounds, &reach) && bounds.width().max(bounds.height()) * px >= 0.5 && near.iter().any(|area| Rect::intersects2(&bounds, area))
            })
            .collect();
        // So many it'd take a while: the whole view's drawn again from
        // thumbs instead
        if under.len() > PATCH_UNITS {
            if let Some(whole) = self.whole.as_mut() {
                whole.stale = true;
            }
            trace(|| format!("  patch of {} units drawn from thumbs instead", under.len()));
            return;
        }
        // Rasters of units that change each frame would be made each frame
        self.rasters_made = RASTERS_A_DRAW;
        let mut recorded = 0usize;
        let mut pictures: Vec<(Rect, Drawn)> = Vec::new();
        for unit in under {
            let level = self.level(&self.units[unit], px, false);
            if !self.pictures.contains_key(&(self.units[unit].slot, self.units[unit].section, level, true)) {
                recorded += 1;
            }
            if let Some((drawn, _)) = self.drawn(unit, level, true, px, true) {
                pictures.push((self.units[unit].bounds, drawn));
            }
        }
        // Areas mostly over each other are drawn as one. Each is a plain
        // rect clip, which the GPU does with a scissor, where a region of
        // them would take a mask the size of the view each time
        let size = |rect: &skia_safe::IRect| rect.width() as i64 * rect.height() as i64;
        'merge: loop {
            for i in 0..rects.len() {
                for j in i + 1..rects.len() {
                    let (a, b) = (rects[i], rects[j]);
                    let both = skia_safe::IRect::from_ltrb(a.left.min(b.left), a.top.min(b.top), a.right.max(b.right), a.bottom.max(b.bottom));
                    if size(&both) * 4 <= (size(&rects[i]) + size(&rects[j])) * 5 {
                        rects[i] = both;
                        rects.swap_remove(j);
                        continue 'merge;
                    }
                }
            }
            break;
        }
        let passes: Vec<(skia_safe::IRect, Vec<Drawn>)> = rects
            .iter()
            .map(|rect| {
                let area = Rect::from_ltrb(rect.left as f32 / px, rect.top as f32 / px, rect.right as f32 / px, rect.bottom as f32 / px).with_outset((1.0 / px, 1.0 / px));
                (*rect, pictures.iter().filter(|(bounds, _)| Rect::intersects2(bounds, &area)).map(|(_, drawn)| drawn.clone()).collect())
            })
            .collect();
        let record = started.elapsed();
        // This one and the whole view's own
        let in_place = Arc::strong_count(&tex) == 2;
        let target = if in_place { Some(tex.clone()) } else { self.tex(width, height) };
        let surface = target.clone().and_then(|target| self.target(&target));
        let previous = if in_place {
            None
        } else {
            gpu::images::borrow_texture_from(&mut self.gpu.context, &tex.backend(), SurfaceOrigin::TopLeft, ColorType::RGBA8888, AlphaType::Premul, None)
        };
        let surface = surface.filter(|_| in_place || previous.is_some());
        let (Some(target), Some(mut surface)) = (target, surface) else {
            // Left as it was, so drawn again whole
            if let Some(whole) = self.whole.as_mut() {
                whole.stale = true;
            }
            return;
        };
        let backend = target.backend();
        let canvas = surface.canvas();
        if let Some(previous) = &previous {
            let mut copy = Paint::default();
            copy.set_blend_mode(skia_safe::BlendMode::Src);
            canvas.draw_image(previous, (0, 0), Some(&copy));
        }
        for (rect, drawn) in &passes {
            canvas.save();
            canvas.clip_irect(rect.with_offset((-x, -y)), skia_safe::ClipOp::Intersect);
            canvas.clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
            play(canvas, drawn, (-x as f32, -y as f32), px);
            canvas.restore();
        }
        let played = started.elapsed();
        let image = self.finish(&backend, false);
        drop(surface);
        drop(previous);
        let fence = self.gpu.signal();
        self.serial += 1;
        let serial = self.serial;
        let mut replaced = None;
        if let Some(whole) = self.whole.as_mut() {
            whole.fence = fence;
            whole.serial = serial;
            whole.stale |= image.is_none();
            if !in_place {
                replaced = Some(std::mem::replace(&mut whole.layer.tex, target));
            }
        }
        if let Some(replaced) = replaced {
            self.retire(replaced);
        }
        trace(|| {
            format!(
                "  patch areas={} passes={} pictures={} recorded={recorded} {:.1}ms played={:.1}ms {:.1}ms",
                areas.len(),
                passes.len(),
                pictures.len(),
                record.as_secs_f64() * 1000.0,
                played.as_secs_f64() * 1000.0,
                started.elapsed().as_secs_f64() * 1000.0
            )
        });
        self.drew = true;
    }

    /// `area` of the view, with where the view is likely to be `lag`
    /// seconds from now, as far as it moves and widens meanwhile, up to a
    /// third of it each way
    fn ahead(&self, area: Rect, lag: f64) -> Rect {
        if self.moved.elapsed() > Duration::from_millis(150) {
            return area;
        }
        let grow = 2f64.powf((self.widening * lag).clamp(-1.0, 1.0)) as f32;
        let center = area.center();
        let (w, h) = (area.width() * grow, area.height() * grow);
        let (x, y) = (center.x + (self.velocity.0 * lag) as f32, center.y + (self.velocity.1 * lag) as f32);
        let mut next = Rect::from_xywh(x - w / 2.0, y - h / 2.0, w, h);
        next.join(area);
        let most = area.with_outset((area.width() / 3.0, area.height() / 3.0));
        if !next.intersect(most) {
            return area;
        }
        next
    }

    /// A texture `width` by `height`, one kept to use again if there is one
    fn tex(&mut self, width: i32, height: i32) -> Option<Arc<Tex>> {
        let _purpose = super::gpu_census::purpose("tile");
        let free = self.spare.iter().position(|tex| tex.width == width && tex.height == height && Arc::strong_count(tex) == 1);
        if let Some(free) = free {
            std::sync::atomic::fence(std::sync::atomic::Ordering::Acquire);
            return Some(self.spare.swap_remove(free));
        }
        Tex::new(&self.gpu.device, width, height)
    }

    /// Dev tools: `tex`'s pixels
    fn read_tex(&mut self, tex: &Tex) -> Option<Vec<u8>> {
        let (width, height) = (tex.width, tex.height);
        let info = ImageInfo::new((width, height), ColorType::RGBA8888, AlphaType::Premul, None);
        let mut target = self.gpu.surface(width, height, 1)?;
        let image = gpu::images::borrow_texture_from(&mut self.gpu.context, &tex.backend(), SurfaceOrigin::TopLeft, ColorType::RGBA8888, AlphaType::Premul, None)?;
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        target.canvas().clear(Color4f::new(0.0, 0.0, 0.0, 0.0)).draw_image(&image, (0, 0), None).read_pixels(&info, &mut pixels, (width * 4) as usize, (0, 0)).then_some(pixels)
    }

    /// Dev tools, with LIGMA_NATIVE_VERIFY set: an exact tile just drawn
    /// against the same drawn again at once from what's kept, traced and
    /// saved where they differ: with `live`, that tells a tile drawn wrong
    /// from one whose texture changed after
    fn verify_tile(&mut self, key: TileKey) {
        if std::env::var_os("LIGMA_NATIVE_VERIFY").is_none() || self.verifying {
            return;
        }
        let Some(tile) = self.tiles.get(&key).filter(|tile| tile.exact && !tile.stale) else { return };
        let tex = tile.tex.clone();
        let Some(had) = self.read_tex(&tex) else { return };
        self.verifying = true;
        let old = self.tiles.remove(&key);
        let pending = self.pending.take();
        let mut again = None;
        for _ in 0..1000 {
            again = self.draw_tile(key, true, false);
            if again.is_some() {
                break;
            }
        }
        self.pending = pending;
        if let Some(old) = old {
            self.tiles.insert(key, old);
        }
        self.verifying = false;
        let Some(again) = again else { return };
        let fresh = self.read_tex(&again.tex);
        self.drop_tile(again);
        let Some(fresh) = fresh else { return };
        let differ = had.chunks(4).zip(fresh.chunks(4)).filter(|(a, b)| (0..4).any(|c| (a[c] as i32 - b[c] as i32).abs() > 24)).count();
        self.verified += 1;
        let n = self.verified;
        trace(|| format!("verify {n} tile {},{} px={:.3} differ={differ}", key.x, key.y, key.px()));
        if differ > 2000 {
            let dir = std::env::temp_dir().join("ligma-verify");
            let _ = std::fs::create_dir_all(&dir);
            for (name, pixels) in [("had", had), ("again", fresh)] {
                if let Some(image) = image::RgbaImage::from_raw(TILE as u32, TILE as u32, pixels) {
                    let _ = image.save(dir.join(format!("{n}-{}_{}-{name}.png", key.x, key.y)));
                }
            }
        }
    }

    fn put_tile(&mut self, key: TileKey, tile: Tile) {
        if let Some(old) = self.tiles.insert(key, tile) {
            self.drop_tile(old);
        }
    }

    fn drop_tile(&mut self, tile: Tile) {
        self.retire(tile.tex);
    }

    /// Keeps `tex` to use again once the present thread has let go of it
    fn retire(&mut self, tex: Arc<Tex>) {
        self.spare.push(tex);
        // Others than tiles come in sizes rarely the same twice, so only the
        // latest of them are kept
        let mut odd = 0;
        for index in (0..self.spare.len()).rev() {
            let tex = &self.spare[index];
            if (tex.width, tex.height) != (TILE, TILE) {
                odd += 1;
                if odd > 2 && Arc::strong_count(tex) == 1 {
                    self.spare.remove(index);
                }
            }
        }
        if self.spare.len() > SPARE {
            let index = self.spare.iter().position(|tex| Arc::strong_count(tex) == 1).unwrap_or(0);
            self.spare.remove(index);
        }
    }

    fn clear_tiles(&mut self) {
        self.pending = None;
        let tiles: Vec<Tile> = self.tiles.drain().map(|(_, tile)| tile).collect();
        for tile in tiles {
            self.drop_tile(tile);
        }
        if let Some(whole) = self.whole.take() {
            self.retire_whole(whole);
        }
    }

    /// A surface to draw into `tex` through. What's drawn into a shared
    /// texture is submitted on its own (see `Tex`): whatever was read
    /// before, possibly from this texture as it was, is submitted first
    fn target(&mut self, tex: &Tex) -> Option<Surface> {
        let _ = self.gpu.context.flush_and_submit();
        gpu::surfaces::wrap_backend_texture(&mut self.gpu.context, &tex.backend(), SurfaceOrigin::TopLeft, 1, ColorType::RGBA8888, None, None)
    }

    /// Submits what's been drawn into a texture through `backend`, and
    /// returns it as this context's image to read it by
    fn finish(&mut self, backend: &BackendTexture, wait: bool) -> Option<Image> {
        if wait {
            let _ = self.gpu.context.flush_submit_and_sync_cpu();
        } else {
            let _ = self.gpu.context.flush_and_submit();
        }
        gpu::images::borrow_texture_from(&mut self.gpu.context, backend, SurfaceOrigin::TopLeft, ColorType::RGBA8888, AlphaType::Premul, None)
    }

    /// The whole view is drawn for `view` itself, up to date
    fn whole_at(&self, view: &View) -> bool {
        let at = view_at(view, self.gpu.size);
        self.whole.as_ref().is_some_and(|whole| whole.at == at && !whole.stale && whole.patch.is_empty())
    }

    /// What's drawn and up to date: tiles, and the whole view
    /// What's drawn, with tiles being drawn again as they were if `stale`
    fn layers(&self, stale: bool) -> Vec<Layer> {
        let tiles = self.tiles.iter().filter(|(_, tile)| stale || !tile.stale).map(|(key, tile)| Layer {
            tex: tile.tex.clone(),
            x: key.x * TILE,
            y: key.y * TILE,
            px: key.px(),
            counts: tile.counts,
            kind: if tile.stale {
                LayerKind::Stale
            } else if tile.draft {
                LayerKind::Draft
            } else {
                LayerKind::Tile
            },
        });
        let whole = self.whole.as_ref().and_then(|whole| match !whole.stale && whole.patch.is_empty() {
            true => Some(whole.layer.clone()),
            false => stale.then(|| Layer { kind: LayerKind::Stale, ..whole.layer.clone() }),
        });
        tiles.chain(whole).chain(self.last_whole.clone()).collect()
    }

    /// Hands the present thread a snapshot of what's drawn and up to date,
    /// as of the latest frame of Figma's render tree, when that's changed
    fn publish(&mut self) {
        self.check();
        let layers = self.layers(true);
        let background = self.background();
        let (transparent, dark) = match self.tree.as_ref().and_then(Tree::background) {
            Some((_, transparent, dark)) => (transparent, dark),
            None => (self.scene.transparent, self.scene.dark),
        };
        let whole_at = self.whole.as_ref().filter(|whole| !whole.stale && whole.patch.is_empty()).map(|whole| whole.at);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for layer in &layers {
            (Arc::as_ptr(&layer.tex) as usize, layer.x, layer.y, layer.px.to_bits(), layer.kind).hash(&mut hasher);
        }
        self.whole.as_ref().map(|whole| whole.serial).hash(&mut hasher);
        (whole_at, background.as_array().map(f32::to_bits), transparent, dark).hash(&mut hasher);
        let signature = hasher.finish();
        if self.published.is_some_and(|(last, seq, _)| last == signature && seq == self.seq) {
            return;
        }
        // What it has is drawn once the GPU passes this
        let value = match self.published {
            Some((last, _, value)) if last == signature => value,
            _ => self.gpu.signal(),
        };
        self.published = Some((signature, self.seq, value));
        self.serial += 1;
        let snapshot = Snapshot {
            seq: self.seq,
            fence: self.gpu.shared_fence.clone(),
            value,
            serial: self.serial,
            layers,
            content: self.content(),
            whole_at,
            background,
            transparent,
            dark,
            pending: self.dirty,
        };
        *self.hub.snapshot.lock().unwrap() = Some(Arc::new(snapshot));
        self.hub.poke();
        if let Some(sent) = self.sent.take() {
            trace(|| format!("  change handed over {:.1}ms after Figma sent it", since_epoch_ms() - sent));
        }
        if std::mem::take(&mut self.fresh) {
            super::mark(|| "drew new images".into());
        }
        self.update_memory();
    }

    /// The approximate reason of a unit in view, if any, for the overlay
    fn update_reason(&mut self, view: &View, px: f32, width: i32, height: i32) {
        let view_area = Rect::from_xywh(view.x as f32, view.y as f32, width as f32 / px, height as f32 / px);
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
            Msg::Frame(frame) => self.frame(*frame),
            Msg::Octaves(octaves) => {
                if let Some(tree) = &mut self.tree {
                    if tree.octaves != octaves {
                        tree.octaves = octaves;
                        self.pictures.clear();
                        self.thumbs.clear();
                        self.clear_rasters();
                        self.clear_tiles();
                        self.dirty = true;
                    }
                }
            }
            // The plugin API's scene, which the render tree replaces
            Msg::Begin(_) | Msg::Changes(_) if self.tree.is_some() => {}
            Msg::View(_) if self.tree.is_some() => {}
            Msg::Begin(scene) => {
                self.scene = *scene;
                self.pictures.clear();
                self.thumbs.clear();
                self.clear_rasters();
                self.clear_tiles();
                self.kit.forget_patterns(None);
                self.rebuild_units();
                stats().lock().unwrap().nodes = 0;
                self.dirty = true;
            }
            Msg::Changes(changes) => {
                let before: HashMap<usize, Rect> = self.units.iter().map(|unit| (unit.slot, unit.bounds)).collect();
                let (touched, changed) = self.scene.apply(changes);
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
                let changed_units: Vec<(usize, bool)> = self
                    .pictures
                    .iter()
                    .filter(|(&(slot, _, _, _), recorded)| touched.contains(&slot) || stale(recorded))
                    .map(|(&(slot, section, _, _), _)| (slot, section))
                    .chain(touched.iter().flat_map(|&slot| [(slot, false), (slot, true)]))
                    .collect();
                self.pictures.retain(|&(slot, _, _, _), recorded| scene.node(slot).is_some() && !touched.contains(&slot) && !stale(recorded));
                // Figma's renders of nodes that changed are asked for again,
                // and shown until they come
                self.images.retain(|key, img| {
                    let Some(id) = key.strip_prefix(RENDER).or_else(|| key.strip_prefix(EFFECT).map(|id| id.split('>').next().unwrap_or(id))) else {
                        return true;
                    };
                    let Some(&slot) = scene.by_id.get(id) else {
                        self.asked.remove(key);
                        return false;
                    };
                    // Effects only change with their node, Figma's renders
                    // are of the whole unit
                    let changed = if key.starts_with(EFFECT) { scene.changed(id, &changed) } else { touched.contains(&scene.unit_of(slot)) };
                    if changed {
                        img.stale = true;
                        self.asked.remove(key);
                    }
                    true
                });
                self.kept.lock().unwrap().retain(|key| {
                    let id = key.strip_prefix(EFFECT).map_or(key, |id| id.split('>').next().unwrap_or(id));
                    !scene.changed(id, &changed)
                });
                self.kit.forget_patterns(None);
                self.bump(changed_units);
                trace(|| format!("changes touched={} areas={}", touched.len(), areas.len()));
                self.drop_tiles(&areas);
                stats().lock().unwrap().nodes = self.scene.live_nodes();
                self.dirty = true;
            }
            Msg::View(view) => self.set_view(view),
            Msg::Failed(key) => {
                self.asked.remove(&key);
                self.failed.insert(key, Instant::now());
            }
            Msg::Image(key, img, width) => {
                // Fetches run side by side, so one asked for before can come
                // after: it's of a view gone by, and what's asked for since
                // is on its way
                // is on its way. It's still shown till then, unless what's
                // here is as sharp
                let superseded = width.is_some_and(|width| self.asked.get(&key).is_some_and(|&asked| asked != width));
                let sharper = self.images.get(&key).is_none_or(|kept| kept.stale || kept.image.width() < img.image.width());
                if superseded && !sharper {
                    trace(|| format!("image {key} for {width:?} came after one asked since"));
                    return true;
                }
                self.image_came(key, img);
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
            Msg::Probe { spec, reply } => {
                let _ = reply.send(self.probe(&spec));
            }
            Msg::Replay { views, reply } => {
                let _ = reply.send(self.replay(views));
                self.release();
                self.dirty = true;
            }
            Msg::Rasters(on) => {
                self.rasters_off = !on;
                self.clear_rasters();
                self.clear_tiles();
                self.dirty = true;
            }
            Msg::Gpu => {}
            Msg::Tint(on) => {
                self.tint = on;
                self.pictures.clear();
                self.thumbs.clear();
                self.clear_rasters();
                self.clear_tiles();
                self.kit.forget_patterns(None);
                if let Some(tree) = &self.tree {
                    tree.forget_patterns(None);
                }
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

    fn set_view(&mut self, view: View) {
        if self.view.as_ref() == Some(&view) {
            return;
        }
        let now = Instant::now();
        let (cx, cy) = (view.x + view.w / 2.0, view.y + view.h / 2.0);
        match self.motion {
            Some((x, y, w, at)) if now - at < Duration::from_millis(150) && w > 0.0 && view.w > 0.0 => {
                let dt = (now - at).as_secs_f64().max(0.004);
                let (vx, vy) = ((cx - x) / dt, (cy - y) / dt);
                self.velocity = (self.velocity.0 * 0.5 + vx * 0.5, self.velocity.1 * 0.5 + vy * 0.5);
                self.widening = self.widening * 0.5 + (view.w / w).log2() / dt * 0.5;
            }
            _ => {
                self.velocity = (0.0, 0.0);
                self.widening = 0.0;
            }
        }
        self.motion = Some((cx, cy, view.w, now));
        self.view = Some(view);
        self.moved = now;
        self.dirty = true;
    }

    /// Image `key` came: what waited for it is drawn again
    fn image_came(&mut self, key: String, img: Img) {
        self.failed.remove(&key);
        // Asked for twice, by pictures wanting it at sizes that came
        // to the same: what's drawn with it is right as it is
        let same = self.images.get(&key).is_some_and(|kept| {
            !kept.stale && kept.area == img.area && (kept.image.width(), kept.image.height()) == (img.image.width(), img.image.height())
        });
        if same && !key.starts_with(RENDER) && !key.starts_with(EFFECT) {
            self.wake_effects(&key);
            return;
        }
        // All that drew without it, or with it at another size, is drawn
        // again. The same image only sharper leaves thumbs of what drew it
        // as they are
        let sharper = !key.starts_with(RENDER) && !key.starts_with(EFFECT) && self.images.get(&key).is_some_and(|kept| !kept.stale && kept.area == img.area);
        let drew = self.image_units.remove(&key).unwrap_or_default();
        if sharper {
            for unit in drew {
                self.version_next += 1;
                self.sharpened.insert(unit, self.version_next);
                self.revision += 1;
            }
        } else {
            self.bump(drew);
        }
        let waiting: Vec<(usize, bool, i32, bool)> =
            self.pictures.iter().filter(|(_, recorded)| recorded.missing.contains(&key)).map(|(&key, _)| key).collect();
        let mut areas: Vec<Rect> = waiting
            .iter()
            .filter_map(|&(slot, _, _, _)| self.units.iter().find(|unit| unit.slot == slot).map(|unit| unit.bounds))
            .collect();
        for area in self.waiting_areas.remove(&key).unwrap_or_default() {
            if !areas.contains(&area) {
                areas.push(area);
            }
        }
        for picture in waiting {
            self.pictures.remove(&picture);
        }
        // Pictures holding it at another size let that go, drawn
        // again when next needed
        self.pictures.retain(|_, recorded| !recorded.uses.iter().any(|(used, _)| *used == key));
        self.kit.forget_patterns(Some(&key));
        if let Some(tree) = &self.tree {
            tree.forget_patterns(Some(&key));
        }
        let mut img = img;
        img.image = upload(&mut self.gpu.context, img.image);
        // Its copy on the way up is let go once the GPU has it, not
        // with those of every other image that came in meanwhile
        if img.bytes() > 1 << 20 {
            let _ = self.gpu.context.flush_and_submit();
        }
        let vram = if tracing() { self.gpu.used() >> 20 } else { 0 };
        trace(|| format!("image {key} {}x{} areas={} vram={vram}", img.image.width(), img.image.height(), areas.len()));
        self.images.insert(key.clone(), img);
        self.drop_tiles(&areas);
        self.wake_effects(&key);
        self.fresh = true;
        self.update_memory();
        self.dirty = true;
    }

    /// Runs again the effects that waited on image `key`, which may itself
    /// be another node's effects
    fn wake_effects(&mut self, key: &str) {
        if self.effects_waiting.is_empty() {
            return;
        }
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.effects_waiting).into_iter().partition(|(_, _, on)| on.iter().any(|image| image == key));
        self.effects_waiting = waiting;
        for (effect, width, _) in ready {
            self.asked.remove(&effect);
            self.ask(&[(effect, width)]);
        }
    }

    /// A frame of Figma's render tree: the units it changed are drawn
    /// again, the rest kept, as a node that changes is a new node. The view
    /// comes with it, as Figma drew it
    fn frame(&mut self, frame: Frame) {
        let first = self.tree.is_none();
        let tree = self.tree.get_or_insert_with(|| Tree::new(super::rt::Schema::bundled()));
        if tree.octaves.is_empty() && self.scene.noise_octaves.len() >= 36 {
            tree.octaves = self.scene.noise_octaves.clone();
        }
        let start = Instant::now();
        let applied = match tree.apply(&frame) {
            Ok(applied) => applied,
            Err(error) => {
                log::warn!("native renderer: frame {} not applied: {error}", frame.header.seq);
                self.hub.tree_failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return;
            }
        };
        self.hub.tree_failures.store(0, std::sync::atomic::Ordering::Relaxed);
        self.seq = frame.header.seq;
        let applied_ms = start.elapsed().as_secs_f64() * 1000.0;
        // What custom effects of nodes gone drew goes with them
        let gone = |key: &str| tree_effect_of(key).is_some_and(|(id, content)| tree.node(id).is_none() || tree.content(id) != content);
        self.images.retain(|key, _| !gone(key));
        self.kept.lock().unwrap().retain(|key| !gone(key));
        let before: HashMap<usize, Rect> = self.units.iter().map(|unit| (unit.slot, unit.bounds)).collect();
        self.rebuild_units();
        if first {
            self.waiting_areas.clear();
            self.pictures.clear();
            self.thumbs.clear();
            self.clear_rasters();
            self.clear_tiles();
            self.kit.forget_patterns(None);
        } else {
            let now: std::collections::HashSet<usize> = self.units.iter().map(|unit| unit.slot).collect();
            let mut areas: Vec<Rect> = before.iter().filter(|(slot, _)| !now.contains(slot)).map(|(_, bounds)| *bounds).collect();
            areas.extend(self.units.iter().filter(|unit| !before.contains_key(&unit.slot)).map(|unit| unit.bounds));
            self.pictures.retain(|(slot, _, _, _), _| now.contains(slot));
            self.thumbs.retain(|(slot, _, _), _| now.contains(slot));
            self.versions.retain(|(slot, _), _| now.contains(slot));
            self.sharpened.retain(|(slot, _), _| now.contains(slot));
            self.image_units.retain(|_, units| {
                units.retain(|(slot, _)| now.contains(slot));
                !units.is_empty()
            });
            let gone: Vec<_> = self.rasters.keys().filter(|(slot, ..)| !now.contains(slot)).copied().collect();
            for key in gone {
                if let Some(raster) = self.rasters.remove(&key) {
                    self.raster_bytes -= footprint(&raster.image);
                }
            }
            trace(|| format!("frame {} mode={} new={} applied={applied_ms:.1}ms units={} areas={}", frame.header.seq, frame.header.mode, applied.new_nodes.len(), self.units.len(), areas.len()));
            if !areas.is_empty() {
                self.edited = Instant::now();
                self.sent = Some(frame.header.at);
            }
            self.drop_tiles(&areas);
        }
        stats().lock().unwrap().nodes = self.tree.as_ref().map_or(0, |tree| tree.forest.nodes.len());
        self.set_view(frame.view());
        self.dirty = true;
    }

    /// Gives back the layers drawing took, keeping what the next draw would
    /// only make again, as an edit's first draw would wait on that
    fn trim(&mut self) {
        self.fit_images();
        self.spare.retain(|tex| (tex.width, tex.height) == (TILE, TILE) || Arc::strong_count(tex) > 1);
        self.scratch.retain(|(_, ss, _)| *ss == 1);
        let _ = self.gpu.context.flush_and_submit();
        self.gpu.context.purge_unlocked_resources(skia_safe::gpu::PurgeResourceOptions::ScratchResourcesOnly);
        super::committed::trim();
        self.update_memory();
        trace(|| format!("trimmed to {} MB in Skia's cache, {} MB used", self.gpu.context.resource_cache_usage().resource_bytes >> 20, self.gpu.used() >> 20));
    }

    /// Gives back the GPU memory an offscreen render took, whole views drawn
    /// supersampled, which Skia would otherwise keep for reuse
    fn release(&mut self) {
        self.fit_images();
        self.spare.retain(|tex| (tex.width, tex.height) == (TILE, TILE) || Arc::strong_count(tex) > 1);
        // Surfaces drawn on are made again when next needed
        self.live = None;
        self.scratch.clear();
        let _ = self.gpu.context.flush_submit_and_sync_cpu();
        self.gpu.context.free_gpu_resources();
        super::committed::trim();
        self.update_memory();
    }

    /// Keeps images within `IMAGES_BUDGET`. An image is never made smaller
    /// while it's needed, as that draws again all that uses it, blurrier
    /// till it's had bigger again, so it'd flicker
    fn fit_images(&mut self) {
        // As run.rs sizes images it fetches
        let fitted = |(w, h): (u32, u32), width: f64| {
            let side = (w.max(h) as f64 * (width.max(1.0) * 1.25 / w.max(1) as f64)).ceil();
            w.max(h) >> super::run::halvings(w.max(h), side)
        };
        // Pictures of other zooms are quick to record again, and hold on to
        // images at the size those zooms took
        let px = self.zoom.0;
        let current = px.max(1e-6).log2().round() as i32;
        let exact = EXACT_LEVEL + (px * EXACT_STEPS).round() as i32;
        if px > 0.0 {
            self.pictures.retain(|&(_, _, level, _), _| {
                if level >= EXACT_LEVEL {
                    level == exact
                } else if is_draft(level) {
                    (level - DRAFT_LEVEL - current).abs() <= 3
                } else {
                    (level - current).abs() <= 1
                }
            });
        }
        // What's near the view needs its images; elsewhere they can go, as
        // the page's encoded copy is kept, so they come back quickly
        let near: Option<Rect> = self.view.as_ref().map(|view| {
            let area = Rect::from_xywh(view.x as f32, view.y as f32, view.w as f32, view.h as f32);
            area.with_outset((area.width(), area.height()))
        });
        let seen: Option<Rect> = self.view.as_ref().map(|view| Rect::from_xywh(view.x as f32, view.y as f32, view.w as f32, view.h as f32));
        // Units near the view, and whether they're in it
        let near_units: HashMap<(usize, bool), bool> = self
            .units
            .iter()
            .filter(|unit| near.is_none_or(|near| Rect::intersects2(&unit.bounds, &near)))
            .map(|unit| ((unit.slot, unit.section), seen.is_none_or(|seen| Rect::intersects2(&unit.bounds, &seen))))
            .collect();
        // What pictures of this zoom draw them at, those of the zooms either
        // side kept for a while would keep them twice as big
        let mut need: HashMap<String, f64> = HashMap::new();
        let mut in_view: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (&(slot, section, level, _), recorded) in &self.pictures {
            let Some(&shown) = near_units.get(&(slot, section)) else {
                continue;
            };
            let this_zoom = level == current || level == exact;
            for (key, width) in &recorded.uses {
                let entry = need.entry(key.clone()).or_insert(0.0);
                if this_zoom {
                    *entry = entry.max(*width);
                }
                if shown {
                    in_view.insert(key.clone());
                }
            }
        }
        // And what Vello drew them with
        for (unit, uses) in &self.vello_uses {
            let Some(&shown) = near_units.get(unit) else {
                continue;
            };
            for (key, width) in uses {
                let entry = need.entry(key.clone()).or_insert(0.0);
                *entry = entry.max(*width);
                if shown {
                    in_view.insert(key.clone());
                }
            }
        }
        let now = Instant::now();
        for key in need.keys() {
            self.wanted.insert(key.clone(), now);
        }
        let mut total: usize = self.images.values().map(Img::bytes).sum();
        trace(|| format!("images {} MB, {} near the view, {} in it", total >> 20, need.len(), in_view.len()));
        if total > IMAGES_BUDGET {
            // Those not in view, needed longest ago (or never) first, and of
            // those the biggest
            let mut aside: Vec<(Option<Instant>, usize, String)> = self
                .images
                .iter()
                .filter(|(key, img)| !key.starts_with(RENDER) && !in_view.contains(key.as_str()) && (key.starts_with(EFFECT) || img.image.width().max(img.image.height()) > PLACEHOLDER))
                .map(|(key, img)| (self.wanted.get(key).copied(), img.bytes(), key.clone()))
                .collect();
            aside.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
            let mut gone = Vec::new();
            for (_, _, key) in aside {
                if total <= IMAGES_BUDGET * 3 / 4 {
                    break;
                }
                total = self.shrink(&key, total);
                gone.push(key);
            }
            self.forget_images(&gone);
        }
        // Those in view alone past it, as far out with many images each
        // zoomed in on before: each more than twice what it's drawn at is
        // had again at that, the same on screen
        if total > IMAGES_BUDGET {
            let mut smaller: Vec<(usize, String, f64)> = self
                .images
                .iter()
                .filter(|(key, _)| !key.starts_with(RENDER) && !key.starts_with(EFFECT))
                .filter_map(|(key, img)| {
                    let width = *need.get(key.as_str()).filter(|width| **width > 0.0)?;
                    let kept = img.image.width().max(img.image.height()) as f64;
                    let fits = fitted(img.original, width) as f64;
                    (kept > fits * 2.0).then(|| ((img.bytes() as f64 * (1.0 - (fits / kept).powi(2))) as usize, key.clone(), width))
                })
                .collect();
            smaller.sort_by(|a, b| b.0.cmp(&a.0));
            for (saving, key, width) in smaller {
                if total <= IMAGES_BUDGET * 3 / 4 {
                    break;
                }
                total = total.saturating_sub(saving);
                if self.asked.get(&key) != Some(&width) {
                    self.asked.insert(key.clone(), width);
                    self.request(&key, width);
                }
            }
        }
        let images = &self.images;
        self.wanted.retain(|key, _| images.contains_key(key));
        // Small copies of images far off are let go past their own budget,
        // the biggest first
        let mut small: Vec<(String, usize)> = self
            .images
            .iter()
            .filter(|(key, img)| !key.starts_with(RENDER) && !key.starts_with(EFFECT) && !need.contains_key(key.as_str()) && img.image.width().max(img.image.height()) <= PLACEHOLDER)
            .map(|(key, img)| (key.clone(), img.bytes()))
            .collect();
        let mut kept: usize = small.iter().map(|(_, bytes)| bytes).sum();
        if kept > PLACEHOLDERS {
            small.sort_by(|a, b| b.1.cmp(&a.1));
            let mut gone = Vec::new();
            for (key, bytes) in small {
                if kept <= PLACEHOLDERS {
                    break;
                }
                self.images.remove(&key);
                self.asked.remove(&key);
                kept -= bytes;
                gone.push(key);
            }
            self.forget_images(&gone);
        }
    }

    /// Keeps image `key` as a small copy, shown where it's drawn till it's
    /// asked for again at the size needed, rather than nothing. Returns
    /// `total` less what that saves
    fn shrink(&mut self, key: &str, total: usize) -> usize {
        let _purpose = super::gpu_census::purpose("image");
        let Some(img) = self.images.remove(key) else {
            return total;
        };
        self.asked.remove(key);
        let bytes = img.bytes();
        // Effects are run again
        if key.starts_with(EFFECT) {
            return total - bytes;
        }
        let (w, h) = (img.image.width(), img.image.height());
        let scale = PLACEHOLDER as f32 / w.max(h) as f32;
        if scale >= 1.0 {
            self.images.insert(key.to_string(), img);
            return total;
        }
        let (sw, sh) = (((w as f32 * scale).round() as i32).max(1), ((h as f32 * scale).round() as i32).max(1));
        let Some(mut surface) = self.gpu.own_surface(sw, sh, 1) else {
            return total - bytes;
        };
        let canvas = surface.canvas();
        canvas.clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
        let sampling = super::draw::sampled(&img.image, FilterMode::Linear, MipmapMode::Linear);
        let mut paint = Paint::default();
        paint.set_blend_mode(skia_safe::BlendMode::Src);
        canvas.draw_image_rect_with_sampling_options(&img.image, None, Rect::from_wh(sw as f32, sh as f32), sampling, &paint);
        // Kept on the GPU without mipmaps, as reading it back to make them
        // waits on all the GPU has to do, once per image let go, which
        // stalled moving the view (see `draw::sampled`)
        let image = surface.image_snapshot();
        let small = Img { image, original: img.original, area: img.area, stale: img.stale };
        let total = total - bytes + small.bytes();
        self.images.insert(key.to_string(), small);
        total
    }

    /// Pictures hold on to images they drew, so those that drew one of
    /// `keys` go, drawn again with what's kept of it when next needed
    fn forget_images(&mut self, keys: &[String]) {
        if keys.is_empty() {
            return;
        }
        self.pictures.retain(|_, recorded| !recorded.uses.iter().any(|(used, _)| keys.contains(used)));
        for key in keys {
            self.kit.forget_patterns(Some(key));
            if let Some(tree) = &self.tree {
                tree.forget_patterns(Some(key));
            }
        }
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

    /// Unit `unit`'s version, see `versions`
    fn version(&self, slot: usize, section: bool) -> u64 {
        self.versions.get(&(slot, section)).copied().unwrap_or(0)
    }

    /// Unit `unit`'s version, with where it's drawn and what clips it, as
    /// its parents place it: all that makes what it draws, with its slot,
    /// which is what it draws itself
    fn mark(&self, unit: &Unit) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.version(unit.slot, unit.section).hash(&mut hasher);
        if let Some(place) = &unit.place {
            let matrix = |m: &skia_safe::Matrix| (0..9).map(|i| m[i].to_bits()).collect::<Vec<u32>>();
            (place.id, matrix(&place.at)).hash(&mut hasher);
            for (id, at) in &place.clips {
                (id, self.tree.as_ref().map(|tree| tree.content(*id)), matrix(at)).hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    /// What units `keys` draw changed, though they're the same units
    fn bump(&mut self, keys: impl IntoIterator<Item = (usize, bool)>) {
        for key in keys {
            self.version_next += 1;
            self.versions.insert(key, self.version_next);
            self.revision += 1;
        }
    }

    /// Compares every unit with what it was at the last check, and wherever
    /// one came, went, moved, changed, or is drawn in another order, what's
    /// drawn there is out of date: tiles over it are drawn again and the
    /// whole view patched. Everything drawn is drawn from units as they are,
    /// so this finds every change, whatever brought it, and what's out of
    /// date is never shown
    fn check(&mut self) {
        if self.revision == self.checked_revision {
            return;
        }
        self.checked_revision = self.revision;
        let bits = |r: &Rect| [r.left.to_bits(), r.top.to_bits(), r.right.to_bits(), r.bottom.to_bits()];
        let now: Vec<Mark> = self
            .units
            .iter()
            .map(|unit| ((unit.slot, unit.section), bits(&unit.bounds), self.mark(unit), self.sharpened.get(&(unit.slot, unit.section)).copied().unwrap_or(0)))
            .collect();
        if now == self.checked {
            return;
        }
        let rect = |b: &[u32; 4]| Rect::from_ltrb(f32::from_bits(b[0]), f32::from_bits(b[1]), f32::from_bits(b[2]), f32::from_bits(b[3]));
        let before: HashMap<(usize, bool), &Mark> = self.checked.iter().map(|mark| (mark.0, mark)).collect();
        let after: HashMap<(usize, bool), &Mark> = now.iter().map(|mark| (mark.0, mark)).collect();
        let mut areas: Vec<Rect> = Vec::new();
        // Where only images came sharper: what's drawn shows the same, so
        // it's shown till it's drawn again
        let mut sharper: Vec<Rect> = Vec::new();
        for mark in &now {
            match before.get(&mark.0) {
                Some(old) if **old == *mark => {}
                Some(old) if (old.1, old.2) == (mark.1, mark.2) => sharper.push(rect(&mark.1)),
                Some(old) => {
                    areas.push(rect(&old.1));
                    areas.push(rect(&mark.1));
                }
                None => areas.push(rect(&mark.1)),
            }
        }
        for mark in &self.checked {
            if !after.contains_key(&mark.0) {
                areas.push(rect(&mark.1));
            }
        }
        // Of those here before and now, any drawn in another order
        let old_order = self.checked.iter().filter(|mark| after.contains_key(&mark.0));
        let new_order = now.iter().filter(|mark| before.contains_key(&mark.0));
        for (old, new) in old_order.zip(new_order) {
            if old.0 != new.0 {
                areas.push(rect(&old.1));
                areas.push(rect(&new.1));
            }
        }
        self.checked = now;
        areas.retain(|area| area.width() > 0.0 || area.height() > 0.0);
        trace(|| format!("  check found {} areas changed, {} sharper", areas.len(), sharper.len()));
        self.drop_tiles(&areas);
        self.sharpen(&sharper);
    }

    /// Tiles over `areas`, where images came sharper, drawn again, and
    /// shown as they are meanwhile
    fn sharpen(&mut self, areas: &[Rect]) {
        if areas.is_empty() {
            return;
        }
        let over: Vec<TileKey> = self
            .tiles
            .keys()
            .filter(|key| {
                let reach = self.margin(**key) as f32 / key.px();
                let area = key.area().with_outset((reach, reach));
                areas.iter().any(|other| Rect::intersects2(&area, other))
            })
            .copied()
            .collect();
        for key in over {
            if let Some(tile) = self.tiles.get_mut(&key).filter(|tile| !tile.stale) {
                tile.draft = true;
            }
        }
        self.dirty = true;
    }

    fn drop_tiles(&mut self, areas: &[Rect]) {
        if areas.is_empty() {
            return;
        }
        // Background blurs over a change see it as far as they reach
        let mut areas = areas.to_vec();
        for &(bounds, reach) in &self.backdrops {
            let near = bounds.with_outset((reach, reach));
            if areas.iter().any(|area| Rect::intersects2(&near, area)) {
                areas.push(bounds);
            }
        }
        let areas = &areas[..];
        let stale: Vec<TileKey> = self
            .tiles
            .keys()
            .filter(|key| {
                let reach = self.margin(**key) as f32 / key.px();
                let area = key.area().with_outset((reach, reach));
                areas.iter().any(|other| Rect::intersects2(&area, other))
            })
            .copied()
            .collect();
        for key in stale {
            if let Some(tile) = self.tiles.get_mut(&key) {
                tile.stale = true;
            }
        }
        if let Some(pending) = &self.pending {
            let reach = pending.margin as f32 / pending.key.px();
            let area = pending.key.area().with_outset((reach, reach));
            if areas.iter().any(|other| Rect::intersects2(&area, other)) {
                self.pending = None;
            }
        }
        if let Some(whole) = &mut self.whole {
            whole.patch.extend_from_slice(areas);
        }
        self.drop_last_whole();
    }

    fn rebuild_units(&mut self) {
        self.revision += 1;
        if let Some(tree) = &self.tree {
            // Background blurs draw from copies of what's behind them, so
            // nothing reads what tiles have drawn
            self.backdrops.clear();
            let mut units: Vec<Unit> = tree
                .units()
                .into_iter()
                .map(|unit| Unit { slot: unit.key, section: false, bounds: unit.bounds, custom: unit.custom, alone: !unit.blends, place: Some(unit.place) })
                .collect();
            for i in 0..units.len() {
                if !units[i].alone {
                    let area = units[i].bounds;
                    units[i].alone = !units[..i].iter().any(|lower| Rect::intersects2(&lower.bounds, &area));
                }
            }
            self.units = units;
            return;
        }
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
                units.push(Unit { slot, section: true, bounds, custom: false, alone: false, place: None });
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
                            units.push(Unit { slot: child, section: true, bounds: rest, custom: false, alone: false, place: None });
                        }
                        break;
                    }
                    add(scene, child, units);
                }
            } else {
                units.push(Unit { slot, section: false, bounds, custom: false, alone: false, place: None });
            }
        }
        for &root in &self.scene.roots {
            add(&self.scene, root, &mut units);
        }
        for unit in &mut units {
            unit.custom = custom_unit(&self.scene, unit);
        }
        // Over nothing drawn before it, a unit that reads what's behind it
        // reads nothing either way
        for i in 0..units.len() {
            units[i].alone = match reads_behind(&self.scene, &units[i]) {
                None => true,
                Some(reach) => {
                    let area = units[i].bounds.with_outset((reach, reach));
                    !units[..i].iter().any(|lower| Rect::intersects2(&lower.bounds, &area))
                }
            };
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
            let started = Instant::now();
            let slow = |pictures: &HashMap<_, Recorded>| {
                let took = started.elapsed();
                if took > Duration::from_millis(15) {
                    let counts = pictures.get(&key).map(|recorded: &Recorded| recorded.counts).unwrap_or_default();
                    trace(|| format!("  picture slot={slot} section={section} level={level} took {:.1}ms native={} approximate={} {:.0}x{:.0}", took.as_secs_f64() * 1000.0, counts.native, counts.approximate, bounds.width(), bounds.height()));
                }
            };
            if let (Some(tree), Some(place)) = (&self.tree, &self.units[unit].place) {
                let mut recorder = PictureRecorder::new();
                let canvas = recorder.begin_recording(bounds.with_outset((1.0, 1.0)), true);
                let mut drawer = Drawer::new(tree, &self.images, &self.kit, canvas, level_px(level));
                drawer.draft = is_draft(level);
                drawer.tint = self.tint;
                drawer.aa = aa;
                drawer.place(place);
                let (counts, reason) = (drawer.counts, drawer.approximate_reason.take());
                let drew: Vec<String> = drawer.used_images.iter().chain(&drawer.missing_images).chain(&drawer.sharper_images).map(|(key, _)| key.clone()).collect();
                if drawer.draft {
                    drawer.missing_images.clear();
                    drawer.sharper_images.clear();
                }
                let waiting = drawer.missing_images.len();
                let images: Vec<(String, f64)> = drawer.missing_images.drain(..).chain(drawer.sharper_images.drain(..)).collect();
                let mut uses = std::mem::take(&mut drawer.used_images);
                uses.extend(images.iter().cloned());
                drop(drawer);
                let picture = recorder.finish_recording_as_picture(None)?;
                self.ask(&images);
                let missing: Vec<String> = images.into_iter().map(|(key, _)| key).collect();
                self.wait_for(&missing, bounds);
                self.drew_with(drew, (slot, section));
                self.pictures.insert(key, Recorded { picture, counts, reason, missing, waiting, sources: Vec::new(), uses });
                slow(&self.pictures);
                return self.pictures.get(&key);
            }
            let mut recorder = PictureRecorder::new();
            let canvas = recorder.begin_recording(bounds.with_outset((1.0, 1.0)), true);
            let mut encoder = Encoder::new(&self.scene, &self.images, &self.kit, canvas, level_px(level));
            encoder.exact_renders = level >= EXACT_LEVEL;
            encoder.draft = is_draft(level);
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
            let drew: Vec<String> = encoder.used_images.iter().chain(&encoder.missing_images).chain(&encoder.sharper_images).chain(&encoder.exact_images).map(|(key, _)| key.clone()).collect();
            // A draft draws what's here, and waits for nothing
            if encoder.draft {
                encoder.missing_images.clear();
                encoder.sharper_images.clear();
                encoder.exact_images.clear();
            }
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
            let missing: Vec<String> = images.into_iter().chain(exact).map(|(key, _)| key).collect();
            self.wait_for(&missing, bounds);
            self.drew_with(drew, (slot, section));
            self.pictures.insert(key, Recorded { picture, counts, reason, missing, waiting, sources, uses });
            slow(&self.pictures);
        }
        self.pictures.get(&key)
    }

    /// Notes that unit `unit` drew with images `keys`, see `image_units`
    fn drew_with(&mut self, keys: Vec<String>, unit: (usize, bool)) {
        for key in keys {
            self.image_units.entry(key).or_default().insert(unit);
        }
    }

    /// Notes that what's drawn over `bounds` waits on `missing`
    fn wait_for(&mut self, missing: &[String], bounds: Rect) {
        for key in missing {
            let areas = self.waiting_areas.entry(key.clone()).or_default();
            if !areas.contains(&bounds) {
                areas.push(bounds);
            }
        }
    }

    /// Unit `unit` as drawn at `scale` device pixels a unit, with its
    /// picture for `level`: an image of it drawn once for that scale where
    /// that comes out the same, so a unit over several tiles, or tiles and
    /// whole views, is drawn once rather than for each. `on_grid` if it's
    /// drawn with the page's origin on a whole device pixel
    fn drawn(&mut self, unit: usize, level: i32, aa: bool, scale: f32, on_grid: bool) -> Option<(Drawn, Counts)> {
        let recorded = self.picture(unit, level, aa)?;
        let (picture, counts) = (recorded.picture.clone(), recorded.counts);
        let (slot, section, bounds, alone) = (self.units[unit].slot, self.units[unit].section, self.units[unit].bounds, self.units[unit].alone);
        let device: skia_safe::IRect = Rect::from_ltrb(bounds.left * scale, bounds.top * scale, bounds.right * scale, bounds.bottom * scale).round_out();
        let device = device.with_outset((1, 1));
        if self.rasters_off || !on_grid || !alone || device.width() > RASTER_SIDE || device.height() > RASTER_SIDE || device.width().max(device.height()) < RASTER_MIN {
            return Some((Drawn::Picture(picture), counts));
        }
        let key = (slot, section, level, aa, scale.to_bits());
        let frame = self.frame;
        if let Some(raster) = self.rasters.get_mut(&key).filter(|raster| raster.picture == picture.unique_id()) {
            raster.used = frame;
            return Some((Drawn::Image(raster.image.clone(), raster.at), counts));
        }
        if self.rasters_made >= RASTERS_A_DRAW {
            return Some((Drawn::Picture(picture), counts));
        }
        self.rasters_made += 1;
        let _purpose = super::gpu_census::purpose("raster");
        let Some(mut surface) = self.gpu.own_surface(device.width(), device.height(), self.config.samples) else {
            return Some((Drawn::Picture(picture), counts));
        };
        let canvas = surface.canvas();
        canvas.clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
        canvas.translate((-device.left as f32, -device.top as f32));
        canvas.scale((scale, scale));
        canvas.draw_picture(&picture, None, None);
        let image = surface.image_snapshot();
        let bytes = footprint(&image);
        if let Some(old) = self.rasters.insert(key, Raster { image: image.clone(), at: (device.left, device.top), picture: picture.unique_id(), used: frame }) {
            self.raster_bytes -= footprint(&old.image);
        }
        self.raster_bytes += bytes;
        // The least lately used go past the budget
        if self.raster_bytes > RASTER_BUDGET {
            let mut by_use: Vec<_> = self.rasters.iter().map(|(key, raster)| (raster.used, *key)).collect();
            by_use.sort_by_key(|(used, _)| *used);
            for (_, gone) in by_use {
                if self.raster_bytes <= RASTER_BUDGET * 3 / 4 {
                    break;
                }
                if gone == key {
                    continue;
                }
                if let Some(raster) = self.rasters.remove(&gone) {
                    self.raster_bytes -= footprint(&raster.image);
                }
            }
        }
        Some((Drawn::Image(image, (device.left, device.top)), counts))
    }

    /// Unit `unit` as a thumb, for drawing at `scale` while the view moves:
    /// drawn at the power of two at or above `scale` onto a page of thumbs,
    /// and kept while its picture is. Made from a sharper thumb where there
    /// is one, which is quick, or else from its picture while there's time
    /// this draw, or else a thumb of another power stands in. None where it
    /// isn't drawn the same alone, is too big for one, or there's none yet
    fn thumb(&mut self, unit: usize, scale: f32) -> Option<(Drawn, Counts)> {
        if self.rasters_off || !(scale > 0.0) {
            return None;
        }
        let (slot, section, bounds) = (self.units[unit].slot, self.units[unit].section, self.units[unit].bounds);
        let power = scale.log2().ceil() as i32;
        let side = 2f32.powi(power);
        let (width, height) = ((bounds.width() * side).ceil() as i32 + 2, (bounds.height() * side).ceil() as i32 + 2);
        if width.max(height) > THUMB_SIDE || width <= 2 || height <= 2 {
            return None;
        }
        if let Some(found) = self.kept_thumb(unit, power) {
            return Some(found);
        }
        let key = (slot, section, power);
        // From a sharper one drawn before this draw, halved
        for sharper in power + 1..=power + 3 {
            if self.kept_thumb(unit, sharper).is_none() {
                continue;
            }
            let Some(source) = self.thumbs.get(&(slot, section, sharper)) else { continue };
            let (source_page, source_from, serial) = (source.page, source.from, source.serial);
            let (picture, picture_level, waiting, counts, under, version) = (source.picture, source.picture_level, source.waiting, source.counts, source.under, source.version);
            let Some(image) = self.page_image(source_page, serial) else { continue };
            let Some((page, at)) = self.thumb_space(width, height) else { return None };
            let from = Rect::from_xywh(at.left as f32 + 1.0, at.top as f32 + 1.0, bounds.width() * side, bounds.height() * side);
            let canvas = self.thumb_pages[page].surface.canvas();
            canvas.save();
            canvas.clip_rect(Rect::from(at), None, false);
            canvas.clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
            let mut paint = Paint::default();
            paint.set_blend_mode(skia_safe::BlendMode::Src);
            let sampling = SamplingOptions::new(FilterMode::Linear, MipmapMode::None);
            canvas.draw_image_rect_with_sampling_options(&image, Some((&source_from, skia_safe::canvas::SrcRectConstraint::Strict)), from, sampling, &paint);
            canvas.restore();
            let generation = self.thumb_pages[page].generation;
            self.thumb_serial += 1;
            let serial = self.thumb_serial;
            self.thumbs.insert(key, Thumb { page, generation, from, to: bounds, picture, picture_level, waiting, counts, serial, under, version });
            self.thumbs_derived += 1;
            return Some((Drawn::Paged(page, from, bounds, under != 0), counts));
        }
        // From its picture, while there's time
        if self.thumb_deadline.is_some_and(|deadline| Instant::now() > deadline) {
            for other in [power + 1, power - 1, power + 2, power - 2, power + 3, power - 3] {
                if let Some(found) = self.kept_thumb(unit, other) {
                    return Some(found);
                }
            }
            self.thumb_later = true;
            return None;
        }
        // Any picture of it kept will do, the more detailed the better
        let level = self.level(&self.units[unit], side, false);
        let kept = (level..=level + 4).chain((level - 2..level).rev()).find(|&other| self.pictures.contains_key(&(slot, section, other, true)));
        let picture_level = kept.unwrap_or(level);
        // One not drawn the same alone is drawn over the thumbs of what's
        // under it, made before this draw
        let under = if self.units[unit].alone { 0 } else { self.under(unit) };
        let mut backdrop: Vec<(Image, Rect, Rect, bool)> = Vec::new();
        if under != 0 {
            for lower in 0..unit {
                if !Rect::intersects2(&self.units[lower].bounds, &bounds) {
                    continue;
                }
                let lower_key = (self.units[lower].slot, self.units[lower].section, power);
                let found = self.kept_thumb(lower, power).is_some().then(|| self.thumbs.get(&lower_key)).flatten().map(|thumb| (thumb.page, thumb.serial, thumb.from, thumb.to, thumb.under != 0));
                let Some((lower_page, serial, from, to, over)) = found else {
                    // Later, once that has a thumb, unless it can't have one
                    let lower_bounds = self.units[lower].bounds;
                    let (lw, lh) = ((lower_bounds.width() * side).ceil() as i32 + 2, (lower_bounds.height() * side).ceil() as i32 + 2);
                    self.thumb_later = lw.max(lh) <= THUMB_SIDE;
                    return None;
                };
                let Some(image) = self.page_image(lower_page, serial) else {
                    self.thumb_later = true;
                    return None;
                };
                backdrop.push((image, from, to, over));
            }
        }
        let recorded = self.picture(unit, picture_level, true)?;
        let (picture, waiting, counts) = (recorded.picture.clone(), recorded.waiting > 0, recorded.counts);
        let (page, at) = self.thumb_space(width, height)?;
        let _purpose = super::gpu_census::purpose("raster");
        let canvas = self.thumb_pages[page].surface.canvas();
        canvas.save();
        canvas.clip_rect(Rect::from(at), None, false);
        canvas.clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
        canvas.translate((at.left as f32 + 1.0 - bounds.left * side, at.top as f32 + 1.0 - bounds.top * side));
        let sampling = SamplingOptions::new(FilterMode::Linear, MipmapMode::None);
        for (image, from, to, over) in &backdrop {
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            if *over {
                paint.set_blend_mode(skia_safe::BlendMode::Src);
            }
            let to = Rect::from_ltrb(to.left * side, to.top * side, to.right * side, to.bottom * side);
            canvas.draw_image_rect_with_sampling_options(image, Some((from, skia_safe::canvas::SrcRectConstraint::Strict)), to, sampling, &paint);
        }
        canvas.scale((side, side));
        canvas.draw_picture(&picture, None, None);
        canvas.restore();
        // Skia does the work of drawing it once flushed, so where time's
        // short it's flushed now, for the time it takes to count
        if self.thumb_deadline.is_some() {
            self.gpu.flush(&mut self.thumb_pages[page].surface);
        }
        let from = Rect::from_xywh(at.left as f32 + 1.0, at.top as f32 + 1.0, bounds.width() * side, bounds.height() * side);
        let generation = self.thumb_pages[page].generation;
        self.thumb_serial += 1;
        let serial = self.thumb_serial;
        let version = self.mark(&self.units[unit]);
        self.thumbs.insert(key, Thumb { page, generation, from, to: bounds, picture: picture.unique_id(), picture_level, waiting, counts, serial, under, version });
        self.thumbs_made += 1;
        Some((Drawn::Paged(page, from, bounds, under != 0), counts))
    }

    /// What's under unit `unit`, as the units it's drawn over and where,
    /// hashed, never 0
    fn under(&self, unit: usize) -> u64 {
        let bounds = self.units[unit].bounds;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for lower in &self.units[..unit] {
            if Rect::intersects2(&lower.bounds, &bounds) {
                (lower.slot, lower.section, lower.bounds.left.to_bits(), lower.bounds.top.to_bits(), lower.bounds.right.to_bits(), lower.bounds.bottom.to_bits()).hash(&mut hasher);
                self.mark(lower).hash(&mut hasher);
            }
        }
        hasher.finish().max(1)
    }

    /// Unit `unit`'s thumb for `power`, if it's kept and still as the unit
    /// is drawn
    fn kept_thumb(&mut self, unit: usize, power: i32) -> Option<(Drawn, Counts)> {
        let (slot, section, bounds) = (self.units[unit].slot, self.units[unit].section, self.units[unit].bounds);
        let thumb = self.thumbs.get(&(slot, section, power))?;
        let page = self.thumb_pages.get(thumb.page)?;
        if page.generation != thumb.generation || thumb.to != bounds || thumb.version != self.mark(&self.units[unit]) {
            return None;
        }
        if thumb.under != 0 && thumb.under != self.under(unit) {
            return None;
        }
        let fine = match self.pictures.get(&(slot, section, thumb.picture_level, true)) {
            Some(recorded) => recorded.picture.unique_id() == thumb.picture,
            None => !thumb.waiting,
        };
        if !fine {
            return None;
        }
        let drawn = (Drawn::Paged(thumb.page, thumb.from, thumb.to, thumb.under != 0), thumb.counts);
        self.thumb_pages[thumb.page].used = self.frame;
        Some(drawn)
    }

    /// Page `page` as it was before this draw drew on it, if thumb `serial`
    /// is on it: what's drawn on it from it then draws from that, rather
    /// than Skia copying the page each time
    fn page_image(&mut self, page: usize, serial: u64) -> Option<Image> {
        if self.page_images.len() < self.thumb_pages.len() {
            self.page_images.resize(self.thumb_pages.len(), None);
        }
        if self.page_images[page].is_none() {
            let image = self.thumb_pages[page].surface.image_snapshot();
            self.page_images[page] = Some((image, self.thumb_serial));
        }
        let (image, upto) = self.page_images[page].as_ref()?;
        (serial <= *upto).then(|| image.clone())
    }

    /// Makes thumbs ahead, a little at a time, for the view as it is and
    /// zoomed out up to 16 times, so zooming out has them. True if there's
    /// more to make
    fn prewarm(&mut self, view: &View, px: f32, levels: &[(Rect, f32)]) -> bool {
        let start = Instant::now();
        let (cx, cy) = ((view.x + view.w / 2.0) as f32, (view.y + view.h / 2.0) as f32);
        self.thumbs_ahead = true;
        self.thumb_deadline = Some(start + Duration::from_millis(4));
        let mut more = false;
        // The levels further out first, then as the view is and zoomed out
        // a little
        let near = (0..=1).map(|n| {
            let grow = (1 << n) as f32;
            let (w, h) = (view.w as f32 * grow, view.h as f32 * grow);
            (Rect::from_xywh(cx - w / 2.0, cy - h / 2.0, w, h), px / grow)
        });
        let targets: Vec<(Rect, f32)> = levels.iter().copied().chain(near).collect();
        'out: for (area, scale) in targets {
            let power = scale.log2().ceil() as i32;
            let mut near: Vec<(f32, usize)> = (0..self.units.len())
                .filter(|&unit| Rect::intersects2(&self.units[unit].bounds, &area))
                .filter(|&unit| !self.thumbs.contains_key(&(self.units[unit].slot, self.units[unit].section, power)))
                .map(|unit| {
                    let center = self.units[unit].bounds.center();
                    ((center.x - cx).hypot(center.y - cy), unit)
                })
                .collect();
            near.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (_, unit) in near {
                if start.elapsed() > Duration::from_millis(4) {
                    more = true;
                    break 'out;
                }
                if self.thumb(unit, scale).is_none() && self.thumbs_full {
                    break 'out;
                }
            }
        }
        self.thumbs_ahead = false;
        self.thumbs_full = false;
        self.thumb_deadline = None;
        self.thumb_later = false;
        self.page_images.clear();
        // Drawn now, rather than with what's drawn next
        if std::mem::take(&mut self.thumbs_made) + std::mem::take(&mut self.thumbs_derived) > 0 {
            let _ = self.gpu.context.flush_and_submit();
        }
        if more {
            self.drew = true;
        }
        more
    }

    /// Room for a thumb `width` by `height` pixels: on a page with room, a
    /// new page, or else the page least lately used, cleared, unless that's
    /// in use for this draw, or the thumb is made ahead
    fn thumb_space(&mut self, width: i32, height: i32) -> Option<(usize, skia_safe::IRect)> {
        let frame = self.frame;
        for (index, page) in self.thumb_pages.iter_mut().enumerate() {
            if let Some(at) = page.place(width, height) {
                page.used = frame;
                return Some((index, at));
            }
        }
        if self.thumb_pages.len() < THUMB_PAGES {
            let _purpose = super::gpu_census::purpose("raster");
            let mut surface = self.gpu.own_surface(THUMB_PAGE, THUMB_PAGE, 1)?;
            surface.canvas().clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
            self.thumb_pages.push(ThumbPage { surface, rows: Vec::new(), bottom: 0, used: frame, generation: 0 });
            let index = self.thumb_pages.len() - 1;
            return self.thumb_pages[index].place(width, height).map(|at| (index, at));
        }
        if self.thumbs_ahead {
            self.thumbs_full = true;
            return None;
        }
        let (index, page) = self.thumb_pages.iter_mut().enumerate().filter(|(_, page)| page.used != frame).min_by_key(|(_, page)| page.used)?;
        page.rows.clear();
        page.bottom = 0;
        page.generation += 1;
        self.pages_cleared += 1;
        page.used = frame;
        page.surface.canvas().clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
        self.thumbs.retain(|_, thumb| thumb.page != index);
        page.place(width, height).map(|at| (index, at))
    }

    /// Thumbs as they're drawn, once all that's drawn of them for a view is
    fn unpage(&mut self, drawn: Vec<Drawn>) -> Vec<Drawn> {
        self.page_images.clear();
        self.thumb_deadline = None;
        if !drawn.iter().any(|item| matches!(item, Drawn::Paged(..))) {
            return drawn;
        }
        let images: Vec<Image> = self.thumb_pages.iter_mut().map(|page| page.surface.image_snapshot()).collect();
        drawn
            .into_iter()
            .map(|item| match item {
                Drawn::Paged(page, from, to, over) => Drawn::Thumb(images[page].clone(), from, to, over),
                other => other,
            })
            .collect()
    }

    fn clear_rasters(&mut self) {
        self.rasters.clear();
        self.raster_bytes = 0;
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
                self.request(key, *width);
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
                self.request(key, *width);
            }
        }
    }

    /// Asks the page for image `key` about `width` pixels wide, or runs a
    /// node's effects for it
    fn request(&mut self, key: &str, width: f64) {
        if key.starts_with(EFFECT) {
            self.effect(key, width);
        } else {
            let _ = self.requests.send(Request { key: key.to_string(), width });
        }
    }

    /// Runs the custom effects of the node `key` names, as Figma's canvas
    /// does: at a pixel a unit, on the node as drawn without them over its
    /// box in its own space, or for a group, on the child after `>` as drawn
    /// over its box. If that draws images not here yet, it waits for them
    fn effect(&mut self, key: &str, width: f64) {
        let _purpose = super::gpu_census::purpose("effect input");
        let Some(name) = key.strip_prefix(EFFECT) else {
            return;
        };
        if let Some((id, _)) = tree_effect_of(key) {
            return self.tree_effect(key, id, width);
        }
        let (owner, child) = match name.split_once('>') {
            Some((owner, child)) => (owner, Some(child)),
            None => (name, None),
        };
        let Some(&owner_slot) = self.scene.by_id.get(owner) else {
            return;
        };
        let Some(slot) = (match child {
            Some(child) => self.scene.by_id.get(child).copied(),
            None => Some(owner_slot),
        }) else {
            return;
        };
        let (Some(owner_node), Some(node)) = (self.scene.node(owner_slot), self.scene.node(slot)) else {
            return;
        };
        // Drawn before, and not changed since: another size of it
        if self.kept.lock().unwrap().has(key) {
            let _ = self.effects.send(Job::Resize { key: key.to_string(), width });
            return;
        }
        let chain = owner_node.shaders();
        let (w, h) = (node.size.0 as f32, node.size.1 as f32);
        let transform = node.transform;
        let unit = transform.scale_x().hypot(transform.skew_y()).max(transform.skew_x().hypot(transform.scale_y()));
        let Some(inverse) = transform.invert() else {
            return;
        };
        if chain.is_empty() || w <= 0.0 || h <= 0.0 || unit <= 0.0 {
            return;
        }
        // A pixel a unit, unless that's more than a GPU texture takes
        let scale = (RENDER_LIMIT as f32 / w.max(h)).min(1.0);
        let (pw, ph) = ((w * scale).ceil().max(1.0) as i32, (h * scale).ceil().max(1.0) as i32);
        // Its own memory, given back as soon as it's read
        let Some(mut surface) = self.gpu.own_surface(pw, ph, 1) else {
            return;
        };
        let canvas = surface.canvas();
        canvas.clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
        canvas.scale((scale, scale));
        canvas.concat(&inverse);
        let mut encoder = Encoder::new(&self.scene, &self.images, &self.kit, canvas, scale / unit);
        encoder.bare = Some(owner_slot);
        encoder.node(slot);
        let id = owner_node.id.clone() + child.map(|child| format!(">{child}")).unwrap_or_default().as_str();
        let images: Vec<(String, f64)> = encoder.missing_images.drain(..).filter(|(missing, _)| missing != key).collect();
        drop(encoder);
        if !images.is_empty() {
            // Asked for still: run again once one of those images comes,
            // not each time what draws it is recorded or another image comes
            let on: Vec<String> = images.iter().map(|(image, _)| image.clone()).collect();
            self.effects_waiting.retain(|(waiting, _, _)| waiting != key);
            self.effects_waiting.push((key.to_string(), width, on));
            self.ask(&images);
            return;
        }
        let info = ImageInfo::new((pw, ph), ColorType::RGBA8888, AlphaType::Premul, None);
        let mut pixels = vec![0u8; (pw * ph * 4) as usize];
        if !surface.canvas().read_pixels(&info, &mut pixels, (pw * 4) as usize, (0, 0)) {
            return;
        }
        let vram = if tracing() { self.gpu.used() >> 20 } else { 0 };
        trace(|| format!("effect input {key} {pw}x{ph} vram={vram}"));
        let _ = self.effects.send(Job::Run(Run { key: key.to_string(), node: id, chain, pixels, width: pw as u32, height: ph as u32, scale, shown: width }));
    }

    /// Runs the custom effects of render tree node `id`, as Figma's canvas
    /// does: at a pixel a unit, on the node's content as drawn over the box
    /// they cover, in the node's own space
    fn tree_effect(&mut self, key: &str, id: u32, width: f64) {
        let Some(tree) = &self.tree else {
            return;
        };
        if self.kept.lock().unwrap().has(key) {
            let _ = self.effects.send(Job::Resize { key: key.to_string(), width });
            return;
        }
        let Some(effect) = tree.custom_effect(id) else {
            return;
        };
        let Some(inverse) = effect.to.invert() else {
            return;
        };
        let area = effect.area;
        let scale = (RENDER_LIMIT as f32 / area.width().max(area.height())).min(1.0);
        let (pw, ph) = ((area.width() * scale).ceil().max(1.0) as i32, (area.height() * scale).ceil().max(1.0) as i32);
        let Some(mut surface) = self.gpu.own_surface(pw, ph, 1) else {
            return;
        };
        let canvas = surface.canvas();
        canvas.clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
        canvas.scale((scale, scale));
        canvas.translate((-area.left, -area.top));
        canvas.concat(&inverse);
        // The surface's own pixels, which its matrix already maps to
        let mut drawer = Drawer::new(tree, &self.images, &self.kit, canvas, 1.0);
        drawer.draw(effect.child);
        let images: Vec<(String, f64)> = drawer.missing_images.drain(..).filter(|(missing, _)| missing != key).collect();
        drop(drawer);
        if !images.is_empty() {
            // Asked for still: run again once one of those images comes,
            // not each time what draws it is recorded or another image comes
            let on: Vec<String> = images.iter().map(|(image, _)| image.clone()).collect();
            self.effects_waiting.retain(|(waiting, _, _)| waiting != key);
            self.effects_waiting.push((key.to_string(), width, on));
            self.ask(&images);
            return;
        }
        let info = ImageInfo::new((pw, ph), ColorType::RGBA8888, AlphaType::Premul, None);
        let mut pixels = vec![0u8; (pw * ph * 4) as usize];
        if !surface.canvas().read_pixels(&info, &mut pixels, (pw * 4) as usize, (0, 0)) {
            return;
        }
        trace(|| format!("effect input {key} {pw}x{ph}"));
        let run = Run { key: key.to_string(), node: effect.node, chain: effect.chain, pixels, width: pw as u32, height: ph as u32, scale, shown: width };
        let _ = self.effects.send(Job::Run(run));
    }

    /// Draws tile `key`: every unit over it, in order. Exact tiles take
    /// Figma's 16 samples a pixel, others one with anti-aliased edges, which
    /// is many times quicker
    fn draw_tile(&mut self, key: TileKey, exact: bool, draft: bool) -> Option<Tile> {
        let _purpose = super::gpu_census::purpose("drawing tiles");
        let exact = exact && !draft;
        if let Some(pending) = self.pending.take_if(|pending| (pending.key, pending.exact, pending.draft) == (key, exact, draft)) {
            return self.step_tile(pending);
        }
        self.rasters_made = 0;
        let ss = if exact { self.supersample(key.px()) } else { 1 };
        // Exact tiles not supersampled take 4 samples a pixel instead of
        // smoothing each shape's edges alone, which where shapes share an
        // edge lets what's under them show through as a seam
        let msaa = exact && ss == 1 && self.config.samples < 4;
        let samples = if msaa { 4 } else { self.config.samples };
        // Skia takes the samples only for what's drawn anti-aliased
        let aa = ss < 4;
        // Scratch surfaces are told apart by `ss`, multisampled ones as 0
        let kind = if msaa { 0 } else { ss };
        let area = key.area();
        let px = key.px();
        let margin = self.margin(key);
        let reach = area.with_outset((margin as f32 / px, margin as f32 / px));
        let units: Vec<usize> = (0..self.units.len()).filter(|&i| Rect::intersects2(&self.units[i].bounds, &reach)).collect();
        let mut counts = Counts::default();
        let mut pictures: Vec<Drawn> = Vec::new();
        let mut names = Vec::new();
        let on_grid = self.frac == (0.0, 0.0);
        // Timing waits for the GPU, so only some tiles are timed
        self.made += 1;
        let timing = super::dev() && (self.made % 8 == 0 || tracing());
        let start = Instant::now();
        // Thumbs a tile needs are all made: they cost about what playing
        // the unit would, and later tiles and views use them again
        self.thumb_deadline = None;
        for unit in units {
            // Too small to see: left out, as the encoder would
            let bounds = self.units[unit].bounds;
            if bounds.width().max(bounds.height()) * px < if draft { DRAFT_SMALLEST } else { 0.5 } {
                counts.skipped += 1;
                continue;
            }
            // Quick tiles while the view moves draw what thumbs there are
            if !exact {
                if let Some((drawn, drawn_counts)) = self.thumb(unit, px) {
                    counts += drawn_counts;
                    pictures.push(drawn);
                    continue;
                }
                self.thumb_later = false;
            }
            let level = self.level(&self.units[unit], px, exact);
            let level = if draft { DRAFT_LEVEL + level } else { level };
            if let Some((drawn, drawn_counts)) = self.drawn(unit, level, aa, px * ss as f32, on_grid) {
                counts += drawn_counts;
                pictures.push(drawn);
                if tracing_units() {
                    names.push(self.scene.node(self.units[unit].slot).map(|node| format!("{} {} {}", node.id, node.type_name, node.effects.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>().join(","))).unwrap_or_default());
                }
            }
        }
        let pictures = self.unpage(pictures);
        let recorded = start.elapsed();
        let before = if tracing() { gpu_counters() } else { [0.0; 6] };
        let scratch = match self.scratch.iter().position(|(kept, of, _)| (*kept, *of) == (margin, kind)) {
            Some(found) => self.scratch.swap_remove(found).2,
            // Outside Skia's cache, so its memory goes as soon as it does
            None => self.gpu.own_surface((TILE + margin * 2) * ss, (TILE + margin * 2) * ss, samples)?,
        };
        let offset = (((margin - key.x * TILE) * ss) as f32 - self.frac.0 * ss as f32, ((margin - key.y * TILE) * ss) as f32 - self.frac.1 * ss as f32);
        let mut pending = Pending {
            key,
            exact,
            draft,
            pictures,
            names,
            next: 0,
            group: 2,
            scratch,
            margin,
            ss,
            msaa,
            offset,
            counts,
            timing,
            started: start,
            recorded,
            played: Duration::ZERO,
            flushed: Duration::ZERO,
            before,
            steps: 0,
            longest: Duration::ZERO,
        };
        pending.scratch.canvas().clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
        self.step_tile(pending)
    }

    /// Draws more of tile `pending`: a quick one all at once, an exact one
    /// for about `TILE_SLICE`, then the rest later, so the view is never
    /// held up long by it. Returns the tile once it's all drawn
    fn step_tile(&mut self, mut pending: Pending) -> Option<Tile> {
        let began = Instant::now();
        let scale = pending.key.px() * pending.ss as f32;
        let mut groups = 0;
        while pending.next < pending.pictures.len() {
            if !pending.exact {
                let at = Instant::now();
                play(pending.scratch.canvas(), &pending.pictures, pending.offset, scale);
                pending.next = pending.pictures.len();
                pending.played += at.elapsed();
                if pending.timing {
                    let at = Instant::now();
                    self.gpu.flush(&mut pending.scratch);
                    pending.flushed += at.elapsed();
                }
                break;
            }
            if groups > 0 && began.elapsed() >= TILE_SLICE {
                // What's drawn so far goes to the GPU meanwhile
                self.gpu.context.submit(None);
                pending.steps += 1;
                self.pending = Some(pending);
                return None;
            }
            let end = (pending.next + pending.group).min(pending.pictures.len());
            let at = Instant::now();
            play(pending.scratch.canvas(), &pending.pictures[pending.next..end], pending.offset, scale);
            let flushing = Instant::now();
            // Skia does most of the work of drawing here, not as it's told
            // what to draw, so what it's given at once is kept to a slice
            self.gpu.flush(&mut pending.scratch);
            let took = at.elapsed();
            pending.played += flushing - at;
            pending.flushed += flushing.elapsed();
            pending.next = end;
            pending.longest = pending.longest.max(took);
            groups += 1;
            if took > TILE_SLICE {
                pending.group = (pending.group / 2).max(1);
            } else if took < TILE_SLICE / 4 {
                pending.group = (pending.group * 2).min(64);
            }
        }
        let Pending { key, exact, draft, pictures, names, mut scratch, margin, ss, msaa, offset, counts, timing, started, recorded, played, flushed, before, steps, longest, .. } = pending;
        let px = key.px();
        let tail = Instant::now();
        if timing {
            self.gpu.context.submit(Some(SyncCpu::Yes));
        }
        let tex = self.tex(TILE, TILE)?;
        let source = skia_safe::IRect::from_xywh(margin * ss, margin * ss, TILE * ss, TILE * ss);
        let drawn = scratch.image_snapshot();
        let backend = tex.backend();
        let mut target = self.target(&tex)?;
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
        let image = self.finish(&backend, false);
        drop(target);
        let image = image?;
        if timing {
            let gpu = tail.elapsed();
            let mut stats = stats().lock().unwrap();
            let mix = |mean: &mut f64, ms: f64| *mean = if *mean == 0.0 { ms } else { *mean * 0.9 + ms * 0.1 };
            mix(&mut stats.tile_record_ms, recorded.as_secs_f64() * 1000.0);
            mix(&mut stats.tile_play_ms, (played + flushed).as_secs_f64() * 1000.0);
            mix(&mut stats.tile_gpu_ms, gpu.as_secs_f64() * 1000.0);
            stats.tiles_drawn += 1;
            drop(stats);
            let vram = if tracing() { self.gpu.used() >> 20 } else { 0 };
            let wall = started.elapsed();
            trace(|| {
                let ms = |d: Duration| d.as_secs_f64() * 1000.0;
                let after = gpu_counters();
                let [made, made_ms, programs, programs_ms, pipelines, pipelines_ms] = std::array::from_fn(|i| after[i] - before[i]);
                let kinds = super::committed::take_kinds();
                format!(
                    "  tile {},{} px={px:.4} draft={draft} exact={exact} margin={margin} units={} record={:.1} play={:.1} flush={:.1} gpu={:.1} steps={steps} longest={:.1} wall={:.1} vram={vram} made={made}/{made_ms:.1} programs={programs}/{programs_ms:.1} pipelines={pipelines}/{pipelines_ms:.1} kinds: {kinds}",
                    key.x,
                    key.y,
                    pictures.len(),
                    ms(recorded),
                    ms(played),
                    ms(flushed),
                    ms(gpu),
                    ms(longest),
                    ms(wall)
                )
            });
            // What a slow tile spent its time on, each unit drawn alone
            if tracing_units() && flushed + gpu > Duration::from_millis(50) {
                if let Some(mut alone) = self.gpu.surface((TILE + margin * 2) * ss, (TILE + margin * 2) * ss, self.config.samples) {
                    {
                        let canvas = alone.canvas();
                        let at = Instant::now();
                        play(canvas, &pictures, offset, px * ss as f32);
                        let _ = self.gpu.context.flush_and_submit_surface(&mut alone, Some(SyncCpu::Yes));
                        trace(|| format!("    whole tile again {:.1}ms", at.elapsed().as_secs_f64() * 1000.0));
                    }
                    for (picture, name) in pictures.iter().zip(&names) {
                        let canvas = alone.canvas();
                        canvas.clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
                        let at = Instant::now();
                        play(canvas, std::slice::from_ref(picture), offset, px * ss as f32);
                        let _ = self.gpu.context.flush_and_submit_surface(&mut alone, Some(SyncCpu::Yes));
                        let took = at.elapsed().as_secs_f64() * 1000.0;
                        if took > 5.0 {
                            trace(|| format!("    {took:.1}ms {name}"));
                        }
                    }
                }
            }
        }
        // Margins are rare, so their bigger surfaces aren't kept
        if margin == 0 {
            self.scratch.push((margin, if msaa { 0 } else { ss }, scratch));
        }
        Some(Tile { image, tex, used: self.frame, counts, stale: false, exact, draft })
    }

    fn background(&self) -> Color4f {
        if let Some((background, _, dark)) = self.tree.as_ref().and_then(Tree::background) {
            return match background {
                Some(background) => background,
                None if dark => Color4f::new(31.0 / 255.0, 31.0 / 255.0, 31.0 / 255.0, 1.0),
                None => Color4f::new(1.0, 1.0, 1.0, 1.0),
            };
        }
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
        let (transparent, dark) = match self.tree.as_ref().and_then(Tree::background) {
            Some((_, transparent, dark)) => (transparent, dark),
            None => (self.scene.transparent, self.scene.dark),
        };
        if !transparent {
            return None;
        }
        checker_paint(self.checker.as_ref()?, dark, origin, px, zoom)
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
        let size = (target.width() as u32, target.height() as u32);
        if let Some(color) = view.outline() {
            target.canvas().clear(color);
            return (Counts::default(), None, false);
        }
        let (drawn, preview, width, height) = view.drawn(size);
        if !preview {
            return self.compose_at(target, view, budget, settled);
        }
        // A pixel preview: the view drawn at its pixels from the one the
        // corner is in, then magnified, each pixel a square
        let (cx, cy) = ((view.x * drawn as f64).floor(), (view.y * drawn as f64).floor());
        let small = View { x: cx / drawn as f64, y: cy / drawn as f64, w: width as f64 / drawn as f64, h: height as f64 / drawn as f64, preview: 0.0, ..view.clone() };
        let Some(mut surface) = self.gpu.surface(width, height, 1) else {
            return (Counts::default(), None, true);
        };
        let result = self.compose_at(&mut surface, &small, budget, settled);
        let px = view.px(size.0);
        let scale = px / drawn;
        let image = surface.image_snapshot();
        // Its corner lands on a whole pixel, as the view's does
        let at = (small.x * px as f64 - (view.x * px as f64).round()) as f32;
        let at_y = (small.y * px as f64 - (view.y * px as f64).round()) as f32;
        let dest = Rect::from_xywh(at, at_y, width as f32 * scale, height as f32 * scale);
        let canvas = target.canvas();
        canvas.draw_image_rect_with_sampling_options(&image, None, dest, SamplingOptions::new(FilterMode::Nearest, MipmapMode::None), &Paint::default());
        if view.grid {
            self.pixel_grid(canvas, (view.x * px as f64, view.y * px as f64), px, size.0 as i32, size.1 as i32);
        }
        result
    }

    /// `compose` at the window's own pixels
    fn compose_at(&mut self, target: &mut Surface, view: &View, budget: Option<Duration>, settled: bool) -> (Counts, Option<String>, bool) {
        self.check();
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
            if let Some(tile) = self.draw_tile(key, exact, false) {
                self.put_tile(key, tile);
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
        if view.grid {
            self.pixel_grid(canvas, (view.x * px as f64, view.y * px as f64), px, width, height);
        }
        // The approximate reason of a unit in view, if any
        let view_area = Rect::from_xywh(view.x as f32, view.y as f32, width as f32 / px, height as f32 / px);
        let aa = self.supersample(px) < 4;
        let reason = self
            .units
            .iter()
            .filter(|unit| Rect::intersects2(&unit.bounds, &view_area))
            .find_map(|unit| self.pictures.get(&(unit.slot, unit.section, self.level(unit, px, true), aa)).and_then(|recorded| recorded.reason.clone()));
        self.evict(pxb, coarse(px).to_bits());
        (counts, reason, pending || !missing.is_empty())
    }

    /// Keeps the most recently shown tiles. Quick tiles of zooms passed
    /// through go soon, as they only stand in for a moment
    fn evict(&mut self, current: u32, coarse: u32) {
        let frame = self.frame;
        let gone: Vec<TileKey> =
            self.tiles.iter().filter(|(key, tile)| !(tile.exact || key.px == current || key.px == coarse || self.pyramid.contains(&key.px) || frame - tile.used < PASSING)).map(|(key, _)| *key).collect();
        for key in gone {
            if let Some(tile) = self.tiles.remove(&key) {
                self.drop_tile(tile);
            }
        }
        // As do the levels further out
        let pyramid = std::mem::take(&mut self.pyramid);
        let own = |key: &TileKey| key.px == coarse || (key.px != current && pyramid.contains(&key.px));
        let mut kept: Vec<(u64, TileKey)> = self.tiles.iter().filter(|(key, _)| key.px != coarse && own(key)).map(|(key, tile)| (tile.used, *key)).collect();
        if kept.len() > PYRAMID_TILES {
            kept.sort_unstable_by_key(|(used, _)| std::cmp::Reverse(*used));
            for (_, key) in kept.split_off(PYRAMID_TILES) {
                if let Some(tile) = self.tiles.remove(&key) {
                    self.drop_tile(tile);
                }
            }
        }
        // The coarse level has room of its own
        let mut kept: Vec<(u64, TileKey)> = self.tiles.iter().filter(|(key, _)| key.px == coarse).map(|(key, tile)| (tile.used, *key)).collect();
        if kept.len() > COARSE_TILES {
            kept.sort_unstable_by_key(|(used, _)| std::cmp::Reverse(*used));
            for (_, key) in kept.split_off(COARSE_TILES) {
                if let Some(tile) = self.tiles.remove(&key) {
                    self.drop_tile(tile);
                }
            }
        }
        // Scratch layers and images not used for a while
        if self.cleaned.elapsed() > Duration::from_secs(2) {
            self.cleaned = Instant::now();
            self.gpu.context.perform_deferred_cleanup(Duration::from_secs(5), None);
        }
        let max = self.config.max_tiles;
        let fine = self.tiles.keys().filter(|key| !own(key)).count();
        if fine <= max {
            self.pyramid = pyramid;
            return;
        }
        let mut used: Vec<u64> = self.tiles.iter().filter(|(key, _)| !own(key)).map(|(_, tile)| tile.used).collect();
        used.sort_unstable();
        let cutoff = used[fine - max];
        let gone: Vec<TileKey> = self.tiles.iter().filter(|(key, tile)| !own(key) && tile.used < cutoff).map(|(key, _)| *key).collect();
        self.pyramid = pyramid;
        for key in gone {
            if let Some(tile) = self.tiles.remove(&key) {
                self.drop_tile(tile);
            }
        }
        // Pictures of zoom levels far from any tile kept go too
        let mut levels: Vec<i32> = self.tiles.keys().map(|key| key.px().log2().round() as i32).collect();
        if let Some(whole) = &self.whole {
            levels.push(whole.layer.px.log2().round() as i32);
        }
        // Thumbs stay as they are only while the pictures they're of do
        let thumbed: std::collections::HashSet<(usize, bool, i32)> = self.thumbs.iter().map(|(&(slot, section, _), thumb)| (slot, section, thumb.picture_level)).collect();
        let exact: Vec<i32> = self.tiles.keys().map(|key| EXACT_LEVEL + (key.px() * EXACT_STEPS).round() as i32).collect();
        self.pictures.retain(|&(slot, section, level, aa), _| {
            if aa && thumbed.contains(&(slot, section, level)) {
                return true;
            }
            if level >= EXACT_LEVEL {
                exact.contains(&level)
            } else if is_draft(level) {
                levels.iter().any(|&kept| (kept - (level - DRAFT_LEVEL)).abs() <= 1)
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
        // As exact tiles draw it, custom effects run at exactly this scale
        encoder.exact_renders = true;
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
        let exact = std::mem::take(&mut encoder.exact_images);
        drop(encoder);
        let waiting = images.len() + exact.len();
        self.ask(&images);
        self.ask_exact(&exact);
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
            self.thumbs.clear();
            self.clear_rasters();
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
        if id == "*" || id == "*draft" {
            let draft = id == "*draft";
            // Every unit over a 1200 by 800 view there, played as live
            let area = Rect::from_xywh(x, y, 1200.0 / px, 800.0 / px);
            let Some(mut surface) = self.gpu.surface(1200, 800, 1) else {
                return "no surface".into();
            };
            let resources = super::committed::made();
            let programs = super::shader_cache::ASKED.load(std::sync::atomic::Ordering::Relaxed);
            let compiling = super::shader_cache::ASKED_NS.load(std::sync::atomic::Ordering::Relaxed);
            let pipelines = super::gpu_census::PIPELINES.load(std::sync::atomic::Ordering::Relaxed);
            let piping = super::gpu_census::PIPELINES_NS.load(std::sync::atomic::Ordering::Relaxed);
            let mut rows = Vec::new();
            for unit in 0..self.units.len() {
                if !Rect::intersects2(&self.units[unit].bounds, &area) {
                    continue;
                }
                let level = self.level(&self.units[unit], px, false);
                let level = if draft { DRAFT_LEVEL + level } else { level };
                let made = Instant::now();
                let Some(picture) = self.picture(unit, level, true).map(|recorded| recorded.picture.clone()) else { continue };
                let made = made.elapsed().as_secs_f64() * 1000.0;
                let mut best = f64::MAX;
                let mut first = 0.0;
                for _ in 0..3 {
                    let canvas = surface.canvas();
                    canvas.save();
                    canvas.scale((px, px));
                    canvas.translate((-x, -y));
                    let start = Instant::now();
                    canvas.draw_picture(&picture, None, None);
                    canvas.restore();
                    let _ = self.gpu.context.flush_submit_and_sync_cpu();
                    let ms = start.elapsed().as_secs_f64() * 1000.0;
                    if first == 0.0 {
                        first = ms;
                    }
                    best = best.min(ms);
                }
                let name = self.scene.node(self.units[unit].slot).map(|node| format!("{} {}", node.id, node.type_name)).unwrap_or_default();
                rows.push((best, name, picture.approximate_op_count(), first, made));
            }
            rows.sort_by(|a, b| b.3.total_cmp(&a.3));
            let first: f64 = rows.iter().map(|row| row.3).sum();
            let best: f64 = rows.iter().map(|row| row.0).sum();
            let made: f64 = rows.iter().map(|row| row.4).sum();
            let head = format!("first {first:.1}ms, best {best:.1}ms, recorded {made:.1}ms, {} resources made in {:.1}ms\n", super::committed::made().0 - resources.0, super::committed::made().1 - resources.1) + &format!(
                "{} programs compiled in {:.1}ms, {} pipelines made in {:.1}ms
",
                super::shader_cache::ASKED.load(std::sync::atomic::Ordering::Relaxed) - programs,
                (super::shader_cache::ASKED_NS.load(std::sync::atomic::Ordering::Relaxed) - compiling) as f64 / 1e6,
                super::gpu_census::PIPELINES.load(std::sync::atomic::Ordering::Relaxed) - pipelines,
                (super::gpu_census::PIPELINES_NS.load(std::sync::atomic::Ordering::Relaxed) - piping) as f64 / 1e6
            );
            return head + &rows.iter().map(|(ms, name, ops, first, made)| format!("{first:.2}ms first {ms:.2}ms best {made:.2}ms recorded {name} ops={ops}\n")).collect::<String>();
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

    /// What the window shows now, but for what Figma draws over it: the
    /// latest snapshot at the latest view
    fn shown(&mut self) -> Option<(Vec<u8>, u32, u32)> {
        let view = self.view.clone()?;
        let pixels = self.snapshot_at(&view)?;
        Some((pixels, self.gpu.size.0, self.gpu.size.1))
    }

    /// The latest snapshot composed at `view`, as the present thread does,
    /// RGBA
    fn snapshot_at(&mut self, view: &View) -> Option<Vec<u8>> {
        let snapshot = self.hub.snapshot.lock().unwrap().clone()?;
        let size = self.gpu.size;
        let (pieces, _) = cover(&snapshot.layers, snapshot.content, view, size);
        let mut images: Vec<Option<Image>> = vec![None; snapshot.layers.len()];
        for piece in &pieces {
            let backend = snapshot.layers[piece.layer].tex.backend();
            images[piece.layer] = gpu::images::borrow_texture_from(&mut self.gpu.context, &backend, SurfaceOrigin::TopLeft, ColorType::RGBA8888, AlphaType::Premul, None);
        }
        let mut target = self.gpu.surface(size.0 as i32, size.1 as i32, 1)?;
        self.patterns.paint(target.canvas(), &snapshot, &images, &pieces, view, size);
        let info = ImageInfo::new((size.0 as i32, size.1 as i32), ColorType::RGBA8888, AlphaType::Unpremul, None);
        let mut pixels = vec![0u8; (size.0 * size.1 * 4) as usize];
        target.canvas().read_pixels(&info, &mut pixels, (size.0 * 4) as usize, (0, 0)).then_some(pixels)
    }

    /// Each layer of the latest snapshot against the same drawn again from
    /// nothing, traced, and saved where they differ, for finding which holds
    /// something out of date
    fn check_layers(&mut self, view: &View) {
        let Some(snapshot) = self.hub.snapshot.lock().unwrap().clone() else { return };
        let dir = std::env::temp_dir().join("ligma-layers");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let background = self.background();
        // Each layer as shown, drawn again with all but tiles kept as they
        // are, and drawn again from nothing: where only the first differs,
        // its texture changed after it was drawn
        let mut had: Vec<Option<Vec<u8>>> = Vec::new();
        let mut cached: Vec<Option<Vec<u8>>> = Vec::new();
        for pass in 0..2 {
            for (index, layer) in snapshot.layers.iter().enumerate() {
                let (width, height) = (layer.tex.width, layer.tex.height);
                let info = ImageInfo::new((width, height), ColorType::RGBA8888, AlphaType::Premul, None);
                let Some(mut target) = self.gpu.surface(width, height, 1) else {
                    had.push(None);
                    cached.push(None);
                    continue;
                };
                if pass == 0 {
                    let Some(image) = gpu::images::borrow_texture_from(&mut self.gpu.context, &layer.tex.backend(), SurfaceOrigin::TopLeft, ColorType::RGBA8888, AlphaType::Premul, None) else {
                        had.push(None);
                        cached.push(None);
                        continue;
                    };
                    let mut pixels = vec![0u8; (width * height * 4) as usize];
                    let read = target.canvas().clear(background).draw_image(&image, (0, 0), None).read_pixels(&info, &mut pixels, (width * 4) as usize, (0, 0));
                    had.push(read.then_some(pixels));
                    self.clear_tiles();
                } else {
                    self.clear_tiles();
                    self.pictures.clear();
                    self.thumbs.clear();
                    self.clear_rasters();
                }
                let px = layer.px as f64;
                let at = View { x: layer.x as f64 / px, y: layer.y as f64 / px, w: width as f64 / px, h: height as f64 / px, preview: 0.0, ..view.clone() };
                target.canvas().clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
                self.compose_at(&mut target, &at, None, true);
                let mut drawn = vec![0u8; (width * height * 4) as usize];
                let read = target.canvas().read_pixels(&info, &mut drawn, (width * 4) as usize, (0, 0));
                if pass == 0 {
                    cached.push(read.then_some(drawn));
                    continue;
                }
                let (Some(had), Some(cached)) = (&had[index], &cached[index]) else { continue };
                if !read {
                    continue;
                }
                let count = |a: &[u8], b: &[u8]| a.chunks(4).zip(b.chunks(4)).filter(|(a, b)| (0..4).any(|c| (a[c] as i32 - b[c] as i32).abs() > 24)).count();
                let (differ, cached_differ) = (count(had, &drawn), count(cached, &drawn));
                trace(|| format!("layer {index} kind={:?} px={:.3} x={} y={} {}x{} differ={differ} cached={cached_differ}", layer.kind, layer.px, layer.x, layer.y, width, height));
                if differ > 2000 || cached_differ > 2000 {
                    for (name, pixels) in [("had", had), ("cached", cached), ("fresh", &drawn)] {
                        if let Some(image) = image::RgbaImage::from_raw(width as u32, height as u32, pixels.clone()) {
                            let _ = image.save(dir.join(format!("{index}-{name}.png")));
                        }
                    }
                }
            }
        }
    }

    /// What the window shows now, and the same view drawn again from
    /// nothing: any difference is something out of date in what it shows.
    /// Both without what Figma draws over them
    fn live(&mut self) -> Option<(Vec<u8>, Vec<u8>, u32, u32)> {
        let view = self.view.clone()?;
        let (width, height) = (self.gpu.size.0, self.gpu.size.1);
        let shown = self.snapshot_at(&view)?;
        self.check_layers(&view);
        self.clear_tiles();
        self.pictures.clear();
        self.thumbs.clear();
        self.clear_rasters();
        self.kit.forget_patterns(None);
        let mut target = self.gpu.surface(width as i32, height as i32, 1)?;
        self.compose(&mut target, &view, None, true);
        let info = ImageInfo::new((width as i32, height as i32), ColorType::RGBA8888, AlphaType::Unpremul, None);
        let mut fresh = vec![0u8; (width * height * 4) as usize];
        if !target.canvas().read_pixels(&info, &mut fresh, (width * 4) as usize, (0, 0)) {
            return None;
        }
        Some((shown, fresh, width, height))
    }

    /// The nodes under `point`, the topmost at each level
    fn hit(&self, point: (f32, f32)) -> Vec<String> {
        if let Some(tree) = &self.tree {
            return tree.hit(point);
        }
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

    /// Draws each view `spec` gives, `;` between them, each
    /// `px,x,y,width,height[,samples[,thumbs[,alone]]]`: `px` pixels a unit
    /// centered on (`x`, `y`) in units, `width` by `height` pixels, with
    /// `samples` MSAA samples, from thumbs where there are any if `thumbs`
    /// is 1, as while the zoom changes, and the `alone` slowest units timed
    /// each on its own. As JSON, a line a view
    fn probe(&mut self, spec: &str) -> String {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let mut out = Vec::new();
        for one in spec.split(';').map(str::trim).filter(|one| !one.is_empty()) {
            let v: Vec<f64> = one.split(',').filter_map(|value| value.trim().parse().ok()).collect();
            if v.len() < 5 {
                out.push(format!("{{\"error\":\"{one}\"}}"));
                continue;
            }
            let (px, cx, cy, w, h) = (v[0] as f32, v[1] as f32, v[2] as f32, v[3] as i32, v[4] as i32);
            let samples = v.get(5).copied().unwrap_or(1.0) as usize;
            let thumbs = v.get(6).copied().unwrap_or(0.0) > 0.0;
            let alone = v.get(7).copied().unwrap_or(0.0) as usize;
            // What's left out is recorded afresh, and those pictures let go
            // after, never drawn in the window
            let mut skip = v.get(8).copied().unwrap_or(0.0) as u32;
            for (which, at) in [11, 12].into_iter().enumerate() {
                let px = v.get(at).copied().unwrap_or(0.0) as f32;
                super::tree::SMALL[which].store(px.to_bits(), std::sync::atomic::Ordering::Relaxed);
                if px > 0.0 {
                    skip |= 1 << 30;
                }
            }
            let emit = v.get(13).copied().unwrap_or(0.0) > 0.0;
            if emit {
                skip |= 1 << 30;
                super::emit::start();
            }
            if skip != 0 {
                self.pictures.clear();
            }
            super::tree::SKIP.store(skip, std::sync::atomic::Ordering::Relaxed);
            for counted in &super::tree::COUNTED {
                counted.store(0, std::sync::atomic::Ordering::Relaxed);
            }
            let area = Rect::from_xywh(cx - w as f32 / px / 2.0, cy - h as f32 / px / 2.0, w as f32 / px, h as f32 / px);
            let offset = (-(area.left * px).floor(), -(area.top * px).floor());
            let started = Instant::now();
            let before = gpu_counters();
            let _ = super::committed::take_kinds();
            let (mut pictures, mut units, mut thumbed) = (Vec::new(), Vec::new(), 0);
            for unit in 0..self.units.len() {
                let bounds = self.units[unit].bounds;
                if !Rect::intersects2(&bounds, &area) || bounds.width().max(bounds.height()) * px < 0.5 {
                    continue;
                }
                if thumbs {
                    if let Some((drawn, _)) = self.thumb(unit, px) {
                        pictures.push(drawn);
                        units.push(unit);
                        thumbed += 1;
                        continue;
                    }
                }
                let level = self.level(&self.units[unit], px, false);
                if emit {
                    let mut post = skia_safe::Matrix::translate(offset);
                    post.pre_scale((px, px), None);
                    super::emit::unit(post);
                }
                if let Some((drawn, _)) = self.drawn(unit, level, true, px, true) {
                    pictures.push(drawn);
                    units.push(unit);
                }
            }
            let pictures = self.unpage(pictures);
            let recorded = started.elapsed();
            if emit {
                let _ = std::fs::write(std::env::temp_dir().join(format!("ligma-emit-{px}.bin")), super::emit::finish());
            }
            super::tree::SKIP.store(0, std::sync::atomic::Ordering::Relaxed);
            for small in &super::tree::SMALL {
                small.store(0, std::sync::atomic::Ordering::Relaxed);
            }
            let counted: Vec<u64> = super::tree::COUNTED.iter().map(|counted| counted.load(std::sync::atomic::Ordering::Relaxed)).collect();
            // The view's drawing saved, to replay elsewhere
            if v.get(9).copied().unwrap_or(0.0) > 0.0 {
                let mut recorder = PictureRecorder::new();
                play(recorder.begin_recording(Rect::from_wh(w as f32, h as f32), false), &pictures, offset, px);
                if let Some(picture) = recorder.finish_recording_as_picture(None) {
                    let _ = std::fs::write(std::env::temp_dir().join(format!("ligma-view-{px}.skp")), picture.serialize().as_bytes());
                }
            }
            let Some(mut surface) = self.gpu.own_surface(w, h, samples) else {
                out.push(format!("{{\"error\":\"no surface for {one}\"}}"));
                continue;
            };
            let at = Instant::now();
            surface.canvas().clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
            // Each unit drawn by Skia at exactly this zoom, nothing kept
            if v.get(15).copied().unwrap_or(0.0) > 0.0 {
                if let Some(tree) = &self.tree {
                    let canvas = surface.canvas();
                    let mut post = skia_safe::Matrix::translate(offset);
                    post.pre_scale((px, px), None);
                    for &unit in &units {
                        let Some(place) = &self.units[unit].place else { continue };
                        canvas.save();
                        canvas.concat(&post);
                        let mut drawer = Drawer::new(tree, &self.images, &self.kit, canvas, 1.0);
                        drawer.place(place);
                        drop(drawer);
                        canvas.restore();
                    }
                }
            } else {
                play(surface.canvas(), &pictures, offset, px);
            }
            let played = at.elapsed();
            let _ = self.gpu.context.flush_and_submit_surface(&mut surface, Some(SyncCpu::Yes));
            let flushed = at.elapsed() - played;
            let after = gpu_counters();
            // What it drew, to compare
            if v.get(10).copied().unwrap_or(0.0) > 0.0 {
                let image = surface.image_snapshot();
                if let Some(png) = image.encode(Some(&mut self.gpu.context), skia_safe::EncodedImageFormat::PNG, None) {
                    let _ = std::fs::write(std::env::temp_dir().join(format!("ligma-probe-{px}-{skip}-{samples}-{}-{}{}.png", v.get(11).copied().unwrap_or(0.0), v.get(12).copied().unwrap_or(0.0), if v.get(15).copied().unwrap_or(0.0) > 0.0 { "-exact" } else { "" })), png.as_bytes());
                }
            }
            let vello = match v.get(14).copied().unwrap_or(0.0) > 0.0 && self.vello_ready() {
                false => String::new(),
                true => {
                    let at = Instant::now();
                    match self.vello_view(-offset.0 as i32, -offset.1 as i32, w, h, px, area) {
                        Ok((tex, _, stats)) => {
                            let _ = self.gpu.context.flush_submit_and_sync_cpu();
                            let took = at.elapsed();
                            if let Some(pixels) = self.read_tex(&tex) {
                                let info = ImageInfo::new((tex.width, tex.height), ColorType::RGBA8888, AlphaType::Premul, None);
                                let image = skia_safe::images::raster_from_data(&info, skia_safe::Data::new_copy(&pixels), (tex.width * 4) as usize);
                                if let Some(png) = image.and_then(|image| image.encode(None, skia_safe::EncodedImageFormat::PNG, None)) {
                                    let _ = std::fs::write(std::env::temp_dir().join(format!("ligma-vello-{px}.png")), png.as_bytes());
                                }
                            }
                            format!("{stats} all={:.1}ms", took.as_secs_f64() * 1000.0)
                        }
                        Err(why) => format!("can't: {why}"),
                    }
                }
            };
            let kinds = super::committed::take_kinds();
            let usage = self.gpu.context.resource_cache_usage();
            let mut slow = Vec::new();
            if alone > 0 {
                for (picture, &unit) in pictures.iter().zip(&units) {
                    surface.canvas().clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
                    let at = Instant::now();
                    let made = gpu_counters()[0];
                    play(surface.canvas(), std::slice::from_ref(picture), offset, px);
                    let _ = self.gpu.context.flush_and_submit_surface(&mut surface, Some(SyncCpu::Yes));
                    slow.push((at.elapsed(), unit, gpu_counters()[0] - made));
                }
                slow.sort_by(|a, b| b.0.cmp(&a.0));
                slow.truncate(alone);
            }
            drop(pictures);
            if skip != 0 {
                self.pictures.clear();
            }
            let slow: Vec<String> = slow
                .iter()
                .map(|(took, unit, made)| {
                    let u = &self.units[*unit];
                    let id = match (&u.place, &self.tree) {
                        (Some(place), Some(tree)) => tree.census(place.id),
                        _ => self.scene.node(u.slot).map(|node| format!("{} {}", node.id, node.type_name)).unwrap_or_default(),
                    };
                    format!("{{\"ms\":{:.2},\"made\":{made},\"node\":\"{id}\",\"px\":\"{}x{}\",\"alone\":{}}}", ms(*took), (u.bounds.width() * px) as i32, (u.bounds.height() * px) as i32, u.alone)
                })
                .collect();
            out.push(format!(
                "{{\"view\":\"{one}\",\"vello\":\"{vello}\",\"counted\":\"mask layers {} mask clips {} blends {} blurs {} silhouettes {} paths {} points {}\",\"units\":{},\"thumbs\":{thumbed},\"record_ms\":{:.1},\"play_ms\":{:.1},\"gpu_ms\":{:.1},\"made\":{},\"made_ms\":{:.1},\"programs\":{},\"cache_mb\":{},\"kinds\":\"{}\",\"slowest\":[{}]}}",
                counted[0],
                counted[1],
                counted[2],
                counted[3],
                counted[4],
                counted[5],
                counted[6],
                units.len(),
                ms(recorded),
                ms(played),
                ms(flushed),
                after[0] - before[0],
                after[1] - before[1],
                after[2] - before[2],
                usage.resource_bytes >> 20,
                kinds.replace('"', "'"),
                slow.join(",")
            ));
        }
        out.join("\n")
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
        super::draw::sampled(image, FilterMode::Linear, mipmap),
        &paint,
    );
}
