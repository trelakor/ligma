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

    /// How much the browser stretches Figma's canvas, whose pixels are its
    /// size times the device's ratio, rounded, to the device pixels its
    /// edges are snapped to, where `size` is those
    pub fn stretch(&self, size: (u32, u32)) -> (f64, f64) {
        if size != self.canvas() {
            return (1.0, 1.0);
        }
        let pixels = |at: f64| (at * self.dpr).round().max(1.0);
        (size.0 as f64 / pixels(self.cw), size.1 as f64 / pixels(self.ch))
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
    /// Tab `0` is no longer parked, so its render tree isn't kept
    Unpark(Arc<str>),
    /// Figma's noise octaves, see `scene::Begin`
    Octaves(Vec<f32>),
    Changes(Prepared),
    View(View),
    /// Image `key`, and the width it was asked for at, if it was
    Image(String, Img, Option<f64>),
    /// Image `key` couldn't be had, so it's asked for again later
    Failed(String),
    /// Effects `key` weren't run, as a later run for the same node came
    /// first, so they're asked for again if they're still needed
    Skipped(String),
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
/// as given in pixels, and where it's drawn, in units, where that's known
pub struct Request {
    pub key: String,
    pub width: f64,
    pub at: Option<Rect>,
}

/// Images asked for and not yet taken to be fetched. Those nearest the
/// middle of the view go first, out to its edges and then past them, as
/// that's where the eye is, and of those as near the widest, as what's
/// biggest on screen is what the view is waiting on most. An image asked
/// for again before it's taken is fetched once, at the larger width, as
/// near as it's drawn
#[derive(Default)]
pub struct Requests {
    queue: std::sync::Mutex<Vec<Request>>,
    added: std::sync::Condvar,
    /// The view, in units, as of the last frame
    view: std::sync::Mutex<Option<Rect>>,
}

impl Requests {
    pub fn send(&self, request: Request) {
        let view = *self.view.lock().unwrap();
        let mut queue = self.queue.lock().unwrap();
        match queue.iter_mut().find(|queued| queued.key == request.key) {
            Some(queued) => {
                queued.width = queued.width.max(request.width);
                if rank(request.at, view) < rank(queued.at, view) {
                    queued.at = request.at;
                }
            }
            None => queue.push(request),
        }
        self.added.notify_one();
    }

    pub fn set_view(&self, view: Rect) {
        *self.view.lock().unwrap() = Some(view);
    }

    /// The one nearest the middle of the view, waiting up to `timeout` for
    /// one
    pub fn take(&self, timeout: Duration) -> Option<Request> {
        let view = *self.view.lock().unwrap();
        let mut queue = self.queue.lock().unwrap();
        if queue.is_empty() {
            queue = self.added.wait_timeout(queue, timeout).unwrap().0;
        }
        // In steps of a tenth of the view, so of those about as near, the
        // widest
        let key = |request: &Request| ((rank(request.at, view) * 10.0).floor() as i64, std::cmp::Reverse(request.width.max(0.0) as i64));
        let first = (0..queue.len()).min_by_key(|&i| (key(&queue[i]), i))?;
        Some(queue.remove(first))
    }
}

/// How far `at` is from the middle of `view`, in halves of the view's
/// diagonal, so within the view is about 1 or less. Where it's drawn isn't
/// known, as for an image an effect runs on, just past the view's edge
fn rank(at: Option<Rect>, view: Option<Rect>) -> f32 {
    let (Some(at), Some(view)) = (at, view) else { return 1.0 };
    let center = view.center();
    let dx = (at.left - center.x).max(center.x - at.right).max(0.0);
    let dy = (at.top - center.y).max(center.y - at.bottom).max(0.0);
    dx.hypot(dy) / (view.width().hypot(view.height()) / 2.0).max(1e-6)
}

/// In a dev build with LIGMA_NATIVE_TRACE=1, frame and tile timings are
/// written to ligma-trace.txt in the temp folder
pub(super) fn tracing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| super::dev() && std::env::var("LIGMA_NATIVE_TRACE").is_ok_and(|value| value == "1"))
}

/// With LIGMA_NATIVE_TRACE_UNITS=1 as well, slow tiles are drawn again a
/// unit at a time to trace what each costs, which holds up drawing
/// Dev tools: the units drawn with the most paths, traced for each Vello
/// render
fn sizing() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("LIGMA_NATIVE_VELLO_SIZES").is_some())
}

fn tracing_units() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| tracing() && std::env::var("LIGMA_NATIVE_TRACE_UNITS").is_ok_and(|value| value == "1"))
}

/// Lines are written on a thread of their own, so tracing doesn't hold up
/// the thread tracing, and lines from different threads stay whole
pub(super) fn trace(line: impl FnOnce() -> String) {
    if tracing() {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        static LINES: std::sync::OnceLock<Option<std::sync::mpsc::Sender<String>>> = std::sync::OnceLock::new();
        let at = START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0;
        let lines = LINES.get_or_init(|| {
            let (send, lines) = std::sync::mpsc::channel::<String>();
            std::thread::Builder::new()
                .name("native-trace".into())
                .spawn(move || {
                    use std::io::Write;
                    let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("ligma-trace.txt")) else { return };
                    let mut file = std::io::BufWriter::new(file);
                    while let Ok(line) = lines.recv() {
                        let _ = writeln!(file, "{line}");
                        while let Ok(line) = lines.try_recv() {
                            let _ = writeln!(file, "{line}");
                        }
                        let _ = file.flush();
                    }
                })
                .ok()
                .map(|_| send)
        });
        if let Some(lines) = lines {
            let _ = lines.send(format!("{at:9.1} {}", line()));
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

/// Where what custom effects drew is kept once Figma sends their node again
/// as another, shown till they've run on the new one: by Figma's id of the
/// node they're on and what they run on, as a group's run on each child
/// alone, all with the group's id, and moving a child doesn't change it
pub(super) fn stand_in(node: &str, content: u64) -> String {
    format!("{EFFECT}g{node}.{content}")
}

/// Where the last of what custom effects drew for node `node` over `area`
/// is kept, shown where Figma sends it again with what they run on changed
/// too, as it does over and over as a file loads, till they've run on that
pub(super) fn last_drawn(node: &str, area: Rect) -> String {
    format!("{EFFECT}n{node}.{:.0}x{:.0}", area.width(), area.height())
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
    /// Boxed, so looking over when each was used, as tens of thousands
    /// are, reads little memory
    made: Box<Result<super::vello::Fragment, String>>,
    used: Instant,
}

/// Pixels of Vello's image atlas, 8192 square at most, images and pages
/// may take, leaving room for how they're packed
const ATLAS_MOST: u64 = 8192 * 8192 * 3 / 4;
/// Most levels coarser images are drawn at to fit Vello's atlas, each a
/// quarter of the room
const COARSEST_IMAGES: u32 = 4;

/// How long views are drawn in tiles after one was too big for Vello's
/// buffers
const TOO_BIG_FOR: Duration = Duration::from_secs(3);

/// A pass of the render loop past this is traced, step by step
const SLOW_PASS: Duration = Duration::from_millis(30);

/// Steps of a slow pass shorter than this aren't traced
const SLOW_STEP: Duration = Duration::from_millis(3);

/// Most times blurs' sources are drawn at half again to fit their textures
const COARSEST_BLURS: u32 = 3;

/// Most times Vello's started again after errors in `VELLO_RESTARTS_IN`,
/// past which Skia draws the view
const VELLO_RESTARTS: usize = 3;
const VELLO_RESTARTS_IN: Duration = Duration::from_secs(300);

/// How long the view stays still before tiles to stand in where Vello falls
/// behind are drawn
const VELLO_STILL: Duration = Duration::from_secs(1);

/// How long the view stays put before Vello draws it at once
const PAN_STILL: Duration = Duration::from_millis(300);

/// How long images Vello drew over the view count as in it
const IN_VIEW: Duration = Duration::from_secs(2);

/// How long a node's effects running holds up its next run
const EFFECT_PATIENCE: Duration = Duration::from_secs(5);

/// Tiles a side Vello draws at most at once
const VELLO_BLOCK: i32 = 1;
/// Tiles a row Vello draws at most at once, while they draw few units
const BATCH_TILES: usize = 4;
/// Units the tiles Vello draws at once draw at most, past the first's
const BATCH_UNITS: usize = 400;
/// Tiles kept drawn by Vello, 1 MB each, past those the view needs
const VELLO_TILES: usize = 64;
/// Why a tile isn't drawn yet: its units are encoded over several passes
const UNFINISHED: &str = "units encoded in part";
/// How long a pass encodes units for tiles at most, past which those made
/// are kept and the tile's drawn in a later pass, so a level with thousands
/// of units new to it, as one far out is, never holds frames back
const ENCODE_SLICE: Duration = Duration::from_millis(4);
/// Most bytes of path data a unit's encoded at two or more levels finer
/// than a tile's has to stand in for it
const STAND_IN_MOST: usize = 256 << 10;

/// The level Vello draws tiles at while the zoom goes out: the power of two
/// nearest it
fn level_near(px: f32) -> f32 {
    2f32.powf(px.max(1e-6).log2().round())
}

/// The level Vello draws tiles at while the zoom goes in: the power of two
/// at or past it, which frames only ever scale down
fn level_up(px: f32) -> f32 {
    2f32.powf(px.max(1e-6).log2().ceil())
}

/// How long fragments no view used are kept
const FRAGMENTS_KEPT: Duration = Duration::from_secs(3);

/// Time a view drawn while the zoom changes spends at most encoding units
/// at its level, past which those encoded at a level near it stand in
const ENCODE_ZOOMING: Duration = Duration::from_millis(6);
/// The same once it's settled, as the one before stays shown meanwhile
const ENCODE_SETTLED: Duration = Duration::from_millis(30);

/// Whether `encoded` is of the unit as it is, `stamp`, with the textures of
/// the images it draws those Vello has. One Vello can't draw is only of
/// use at its own level, to fall back to Skia
fn usable(encoded: &Encoded, stamp: u64, own: bool, images: &HashMap<String, Img>) -> bool {
    encoded.stamp == stamp
        && match &*encoded.made {
            Ok(fragment) => fragment.pictures.iter().all(|(key, _, id)| images.get(key).is_some_and(|img| img.image.unique_id() as u64 == *id)),
            Err(_) => own,
        }
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

/// Textures kept to use again
const SPARE: usize = 12;

/// Free tiles' textures kept to use again past the GPU budget
const SPARE_PRESSED: usize = 4;

/// Tiles' textures made ahead, for when none's free to use again
const MADE_AHEAD: usize = 2;

/// Tiles' textures made ahead and those let go of freed on a thread of its
/// own, as each took the render thread a millisecond
struct Maker {
    ready: Arc<std::sync::Mutex<Vec<Arc<Tex>>>>,
    work: Sender<Making>,
}

enum Making {
    Tiles,
    Free(Vec<windows::Win32::Graphics::Direct3D12::ID3D12Resource>),
}

// D3D12 devices and resources are free-threaded
unsafe impl Send for Making {}

struct SendDevice(ID3D12Device);

unsafe impl Send for SendDevice {}

impl Maker {
    fn new(device: &ID3D12Device) -> Option<Self> {
        let ready: Arc<std::sync::Mutex<Vec<Arc<Tex>>>> = Arc::default();
        let (work, works) = std::sync::mpsc::channel::<Making>();
        let (made, device) = (ready.clone(), SendDevice(device.clone()));
        std::thread::Builder::new()
            .name("ligma textures".into())
            .spawn(move || {
                let device = device;
                while let Ok(making) = works.recv() {
                    match making {
                        Making::Tiles => {
                            while made.lock().unwrap().len() < MADE_AHEAD {
                                let Some(tex) = Tex::new(&device.0, TILE, TILE) else { break };
                                made.lock().unwrap().push(tex);
                            }
                        }
                        Making::Free(resources) => drop(resources),
                    }
                }
            })
            .ok()?;
        Some(Self { ready, work })
    }

    /// A tile's texture made ahead, if there's one, and another made
    fn take(&self) -> Option<Arc<Tex>> {
        let tex = self.ready.lock().unwrap().pop();
        let _ = self.work.send(Making::Tiles);
        tex
    }
}

/// The whole view, see `Painter::draw_whole`
/// A whole view Vello's GPU work's been submitted for, not yet checked
struct Unchecked {
    layer: Layer,
    fence: u64,
    at: (i32, i32, u32),
    renders: usize,
    started: Instant,
    stats: String,
    rough: bool,
    /// What changed since it was drawn, as for a whole view shown
    patch: Vec<Rect>,
    /// The frame it's drawn as of
    seq: u64,
}

/// The view Vello draws exactly once it's still, a part the size of a
/// tile a pass, so the GPU's never held long, nor Vello's buffers made big
/// for all of it at once, which dense views took a second of the GPU for
struct Chunked {
    at: (i32, i32, u32),
    phase: (f32, f32),
    tex: Arc<Tex>,
    /// The parts drawn so far of `parts`, each its corner and size in the
    /// view, the middle first
    next: usize,
    parts: Vec<(i32, i32, i32, i32)>,
    counts: Counts,
    started: Instant,
    seq: u64,
}

/// A scene Vello keeps on the GPU
struct KeptScene {
    id: u64,
    /// The zoom, as bits, and its corner and size in device pixels
    px: u32,
    left: i32,
    top: i32,
    width: i32,
    height: i32,
    /// When it was built
    built: Instant,
}

/// Longest an image that couldn't be had waits to be asked for again
const FAILED_MOST: Duration = Duration::from_secs(30);

/// How long since an image last came in before the furthest level's tiles
/// are drawn again for what changed
const IMAGES_QUIET: Duration = Duration::from_secs(1);
/// How often, while images keep coming in, the furthest level's tiles are
/// drawn again for what changed anyway, as zooming out shows them, and as
/// they were before images came they'd show the file as it was opening
const FAR_EVERY: Duration = Duration::from_secs(2);

/// What changes in a kept scene other than edits, images coming in and the
/// like, builds it again no sooner than this after it was built, as they
/// come many at once
const KEPT_CHANGES: Duration = Duration::from_millis(100);

/// How far past the view a kept scene reaches on each side, of the view's
/// size, so a pan stays in it a while
const KEPT_MARGIN: f32 = 0.25;

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
    /// Drawn by Vello with some units encoded at other levels, to draw
    /// again exactly
    rough: bool,
    /// Drawn by Vello, all of it at its zoom
    vello: bool,
    /// The frame it's drawn as of, everything changed since in `patch`
    seq: u64,
    /// Parts of it drawn again since by Vello, to draw it all again at rest
    patched: bool,
}

/// After a change, how long the whole view is kept up with every frame, as
/// Figma does, rather than tile by tile
const EDITING: Duration = Duration::from_millis(400);

/// While something's edited, what's under the units it changes in the whole
/// view, drawn once and kept by Vello, so each frame only those units and
/// what's over them are drawn again, over it, where they change. The whole
/// view drawn again each frame took the GPU 10 milliseconds and more, most
/// of it what's under, longer than Figma's frames come apart while
/// something's dragged
struct Strata {
    at: (i32, i32, u32),
    phase: (f32, f32),
    /// What it's of, none for no units, and the images it drew sharper
    under: Option<u64>,
    sharper: u64,
    built: Instant,
}

/// How often what `Strata` keeps is drawn again for images under what's
/// edited coming sharper, as they come several times as an edit starts,
/// drawn again each time at first. Exact once the edit's done
const STRATA_SHARPER: Duration = Duration::from_millis(300);

/// Changed areas past which the whole view is drawn again, rather than each
const PATCHES: usize = 24;
/// Most units drawn again exactly in a patch of the whole view
const PATCH_UNITS: usize = 160;
/// Vello patches of the whole view are on a grid of this many pixels from
/// its corner, as blurs drawn smaller and back come out a little otherwise
/// from elsewhere
const PATCH_GRID: i32 = 64;

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

/// `img` `halvings` times half as big, its mipmap level that far down
/// copied on the GPU. Not yet submitted. Without mipmaps of its own, as
/// Skia making those on the GPU leaves textures it records wrongly (see
/// `draw::sampled`): it's kept as big as the view needs, which takes it
/// as it is, and had again should the view need it smaller still
fn halved(gpu: &mut Gpu, img: &Img, halvings: u32) -> Option<Img> {
    let (w, h) = ((img.image.width() >> halvings).max(1), (img.image.height() >> halvings).max(1));
    let mut surface = gpu.own_surface(w, h, 0)?;
    let mut paint = Paint::default();
    paint.set_blend_mode(skia_safe::BlendMode::Src);
    let level = SamplingOptions::new(FilterMode::Linear, MipmapMode::Nearest);
    surface.canvas().draw_image_rect_with_sampling_options(&img.image, None, Rect::from_iwh(w, h), level, &paint);
    let image = surface.image_snapshot();
    Some(Img { image, original: img.original, area: img.area, stale: img.stale })
}

/// Time a pass of the render thread spends at most making images smaller
/// on the GPU, the rest left for the passes after
const MAKE_SMALLER_A_PASS: Duration = Duration::from_millis(4);

/// Time a pass of the render thread spends at most taking in images that
/// came, past the first, the rest left for the passes after
const IMAGES_A_PASS: Duration = Duration::from_millis(6);

/// Images are kept as big as they were ever needed, which drawn with
/// mipmaps shows the same as a smaller copy, till together they pass this
/// many bytes. Then those needed longest ago go, never any in view, down
/// to three quarters of it, so it's seldom
const IMAGES_BUDGET: usize = 128 << 20;

/// What the process's GPU memory may grow to past where it is at rest,
/// before what's kept only in case it's needed again is let go and no more
/// is kept, so moving the view takes about what resting does
const HEADROOM: u64 = 48 << 20;
/// How long Skia keeps what it could use again but hasn't while drawing
const UNUSED: Duration = Duration::from_millis(250);
/// How often images past the budget are let go while the view moves
const FIT_SOON: Duration = Duration::from_millis(250);
/// Images let go are kept this many pixels on their longest side, so
/// there's something to show where they're drawn till they come again
const PLACEHOLDER: i32 = 256;
/// Bytes of those kept of images nothing near the view draws
const PLACEHOLDERS: usize = 32 << 20;
/// How long an image stays as big as it was last needed near the view.
/// Made smaller sooner, panning back and forth over a file of many images
/// had them again at one size and the other every few seconds, each time
/// fetched, decoded and uploaded again and what draws them drawn again,
/// blurrier then sharper
const NEEDED_FOR: Duration = Duration::from_secs(10);

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

/// Tiles Vello's drawn and copied out, put once it's checked it had room
/// for them, which waits for the GPU. Checked after the next are handed to
/// it, so the GPU draws them while the CPU encodes those
struct DrawnTiles {
    /// Each tile's texture, whether a change since made it out of date,
    /// and whether it was in view then
    tiles: Vec<(TileKey, Arc<Tex>, bool, bool)>,
    first: TileKey,
    /// Vello's renders unchecked, these last
    renders: usize,
    exact: bool,
    counts: Counts,
    stats: String,
    started: Instant,
    submitted: Duration,
    span: (i32, i32, i32, i32),
}

struct Tile {
    /// As this context's image of `tex`
    image: Image,
    tex: Arc<Tex>,
    used: u64,
    counts: Counts,
    /// Out of date, shown until it's drawn again
    stale: bool,
    /// Out of date while in view and in it since, so what it shows is what
    /// was just there. One out of date off the view shows the page as it
    /// was some time before, as without images that came since, so it's
    /// shown only where nothing up to date is, as one a pan comes to
    seen: bool,
    /// Drawn at full quality, sampled as Figma samples, rather than quickly
    /// to fill the view while it moves
    exact: bool,
    /// A draft, to show behind the view's tiles while they're drawn. One
    /// the view itself needs is drawn again in full
    draft: bool,
    /// Drawn by Vello, not Skia
    vello: bool,
    /// When it was drawn, see `Layer::drawn`
    drawn: u64,
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
        trace(|| format!("started on {}, {:?}, {:.0} MB of GPU memory", gpu.adapter, config(), gpu.used() as f64 / 1e6));
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

    /// GPU memory this process uses, and what the OS gives it before it
    /// starts moving memory out, for the trace
    fn used_and_budget(&self) -> (u64, u64) {
        let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        match &self.memory {
            Some(adapter) if unsafe { adapter.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info) }.is_ok() => (info.CurrentUsage, info.Budget),
            _ => (0, 0),
        }
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
        let _span = super::span::span("skia flush");
        let _ = self.context.flush_and_submit_surface(surface, None);
    }
}

pub struct Painter {
    gpu: Gpu,
    kit: Kit,
    scene: Scene,
    /// Figma's render tree, once frames of it come
    tree: Option<Tree>,
    /// Figma's noise octaves, which can come before the tree does
    octaves: Vec<f32>,
    /// How far into its pixel the view is, as Figma draws from there
    phase: (f32, f32),
    units: Vec<Unit>,
    /// By unit, zoom level and whether edges are anti-aliased
    pictures: HashMap<(usize, bool, i32, bool), Recorded>,
    images: HashMap<String, Img>,
    /// Widths images were asked for at
    asked: HashMap<String, f64>,
    /// When each image was last needed near the view
    wanted: HashMap<String, Instant>,
    /// The most each image was needed at near the view lately, and when,
    /// which it's kept at least as big as for `NEEDED_FOR`
    needed: HashMap<String, (f64, Instant)>,
    /// When each image last came bigger than it was kept, which isn't made
    /// smaller again for `NEEDED_FOR`, or what asks for it and what makes
    /// it smaller could take turns
    grown: HashMap<String, Instant>,
    /// Images that couldn't be had, when, and how long till they're
    /// asked for again: a second, then twice as long each time, as one can
    /// fail only till the page is ready
    failed: HashMap<String, (Instant, Duration)>,
    tiles: HashMap<TileKey, Tile>,
    /// Tiles drawn again for not matching the whole view, and how often,
    /// see `verify_tiles`
    mismatched: HashMap<TileKey, u32>,
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
    /// Draws the whole view (see vello.rs) once it's started, unless
    /// LIGMA_NATIVE_VELLO=0 leaves it to Skia
    vello: Option<super::vello::Vello>,
    /// Vello's copies of the tree's paths
    vello_paths: super::vello::Paths,
    /// Levels coarser images are drawn at in Vello's views, so they fit its
    /// atlas, and how much of it the last view took
    image_coarser: u32,
    atlas_used: u64,
    /// Times blurs' sources are drawn at half again, where they wouldn't
    /// fit their textures otherwise, and how tall those were last
    blur_coarser: u32,
    blur_rows: i32,
    /// Where blurs are drawn from while a patch of the whole view is drawn,
    /// see `Plan::blur_view`
    blur_view: Option<vello::kurbo::Rect>,
    /// The units an edit going on changed, by slot
    changing: std::collections::HashSet<usize>,
    strata: Option<Strata>,
    /// The first and last units by index Vello draws, while a part of
    /// what's in the view is drawn
    only: Option<(usize, usize)>,
    /// Where in what's kept under the units an edit changes Vello draws
    /// over, see `Strata`
    under_at: Option<(u32, u32)>,
    /// When Vello was started again after errors, to give up on it after
    /// a few close together
    vello_restarts: Vec<Instant>,
    /// When a view was last too big for Vello's buffers, so for a while
    /// tiles are drawn rather than it drawn again
    vello_too_big: Option<Instant>,
    /// Tiles Vello couldn't draw, which Skia draws
    vello_cant: std::collections::HashSet<TileKey>,
    /// The scene Vello keeps on the GPU, drawn again for each view inside
    /// it at its zoom
    scene_kept: Option<KeptScene>,
    /// Something in the kept scene changed since it was built
    kept_stale: bool,
    /// What the last scene kept was kept as
    kept_serial: u64,
    /// The kept scene last drawn and where
    kept_drawn: Option<(u64, (i32, i32, u32))>,
    /// When an image last came in
    image_came_at: Instant,
    /// When the furthest level's tiles were last drawn again for what
    /// changed
    far_redrawn: Instant,
    vello_starting: Option<std::sync::mpsc::Receiver<Result<super::vello::Vello, String>>>,
    /// Images units drew with when Vello last drew them, and where, and at
    /// what pixels a unit, by unit, as pictures keep them
    vello_uses: HashMap<(usize, bool), (f32, Vec<(String, f64, Rect)>)>,
    /// How wide Vello showed each image it drew since images were last
    /// fitted, the widest
    vello_shown: HashMap<String, f32>,
    /// Images Vello drew over the view, how wide, and when last. Fragments
    /// drawn again as they were don't say what they use otherwise, and
    /// what the view shows would be let go and had again over and over
    vello_in_view: HashMap<String, (f32, Instant)>,
    /// Units encoded for Vello, by unit and level, see `Encoded`
    fragments: HashMap<(usize, bool, i32), Encoded>,
    /// When fragments unused for `FRAGMENTS_KEPT` were last let go
    swept: Instant,
    /// Images of units made for the tile or view being drawn
    rasters_made: u32,
    /// How long the tile being drawn encodes units at most this pass
    slice: Option<Duration>,
    /// Tiles Vello drew last, not yet checked
    drawn_tiles: Option<DrawnTiles>,
    /// Set while tiles are rendered, which leave those drawn before them
    /// to check after
    pipelined: bool,
    /// Textures to use again once the present thread has let go of them
    spare: Vec<Arc<Tex>>,
    /// What the last snapshot handed over had, its frame, and the fence the
    /// GPU passes once it's drawn it
    published: Option<(u64, u64, u64)>,
    /// The latest frame whose changes are drawn all over the view, as of
    /// the last snapshot handed over
    changes_drawn: u64,
    /// Images come past what a pass takes in of them, see `IMAGES_A_PASS`
    images_later: std::collections::VecDeque<Msg>,
    /// The latest frame that changed the scene
    edited_seq: u64,
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
    /// The GPU memory the process may take, once it's been at rest
    budget: Option<u64>,
    /// Past `budget`, so nothing's kept that isn't needed
    pressed: bool,
    /// Dev tools: see `verify_tile`
    verifying: bool,
    verified: u64,
    /// The zooms of the levels further out the view has now
    pyramid: Vec<u32>,
    /// Vello's tiles of the levels between the view and the furthest, as
    /// of the view at rest last
    between: Vec<TileKey>,
    /// When a frame was last drawn, and whether memory was given back since
    drawn: Instant,
    released: bool,
    /// Whether images were fitted again once what moving needed was past
    refitted: bool,
    freed: bool,
    drew: bool,
    /// Figma's checkerboard, and the pixel grid it draws when zoomed in
    checker: Option<skia_safe::RuntimeEffect>,
    grid: Option<(skia_safe::RuntimeEffect, skia_safe::RuntimeEffect)>,
    view: Option<View>,
    /// The last zoom (pixels per unit) and when it changed
    zoom: (f32, Instant),
    /// When the zoom changed before its last change
    zoom_was: Instant,
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
    /// The whole view Vello drew last, handed over once Vello's checked it
    /// had room for it, meanwhile drawing the next
    unchecked: Option<Unchecked>,
    chunked: Option<Chunked>,
    /// The last frame of Figma's render tree applied
    seq: u64,
    /// The tab the render tree is of
    tab: Option<Arc<str>>,
    /// The last whole scene taken in, summed, while nothing's changed since
    whole_sum: Option<(usize, u64)>,
    /// The render tree of the tab drawn before, kept while it's parked
    parked: Option<Parked>,
    /// Whether the tab's tree is kept as another's drawn, till it's unparked
    park: bool,
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
    /// Frames taken in whose units aren't made yet, see `settle_units`:
    /// how many, the latest's number and when the first was sent
    units_due: Option<(u32, u64, f64)>,
    frame: u64,
    tint: bool,
    dirty: bool,
    requests: Arc<Requests>,
    /// Nodes' custom effects to run, see effects.rs
    effects: Sender<Job>,
    /// What effects drew, whole, kept on their thread
    kept: Kept,
    /// Effects waiting on images the node draws, and those images, each
    /// run again once one of its own comes
    effects_waiting: Vec<(String, f64, Vec<String>)>,
    /// Nodes whose effects are running, by Figma's id, with the key of the
    /// run and since when, and the key and width each is asked for again at
    /// once it's done. Each edit to an effect is another key, and drawing
    /// its input for each would mostly be for runs the next edit passes over
    effects_running: HashMap<String, (String, Instant)>,
    effects_again: HashMap<String, (String, f64)>,
    /// Where each effect asked for keeps what it drew once its node is
    /// sent again as another (see `stand_in`)
    effect_nodes: HashMap<String, String>,
    /// The same for the last each node's effects drew (see `last_drawn`),
    /// kept for runs whose node has gone since, which can still come
    effect_last: HashMap<String, String>,
    /// The images each effect's node draws, and how wide, kept while the
    /// effect is, as it runs again on them whenever the node changes
    effect_images: HashMap<String, Vec<(String, f64)>>,
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
    /// Units whose version changed since the last check, which is all that
    /// changed unless `units_rebuilt`, and where each unit's mark is in
    /// `checked`
    touched: Vec<(usize, bool)>,
    units_rebuilt: bool,
    checked_at: HashMap<(usize, bool), usize>,
    /// Textures let go of, freed once the queue passes each fence value
    graves: Vec<(u64, Vec<windows::Win32::Graphics::Direct3D12::ID3D12Resource>)>,
    maker: Option<Maker>,
}

/// A unit as `check` knows it: its slot and section, bounds as bits, a
/// hash of its version and where it's placed (see `Painter::mark`), and
/// when an image it drew last came sharper
type Mark = ((usize, bool), [u32; 4], u64, u64);

impl Painter {
    pub fn new(gpu: Gpu, requests: Arc<Requests>, effects: Sender<Job>, kept: Kept, hub: Arc<Hub>) -> Self {
        let vello_starting = (std::env::var_os("LIGMA_NATIVE_VELLO").as_deref() != Some(std::ffi::OsStr::new("0"))).then(|| super::vello::start(&gpu.device));
        Self {
            vello: None,
            vello_paths: Default::default(),
            image_coarser: 0,
            atlas_used: 0,
            blur_coarser: 0,
            blur_view: None,
            changing: Default::default(),
            strata: None,
            only: None,
            under_at: None,
            blur_rows: 0,
            vello_restarts: Vec::new(),
            vello_too_big: None,
            vello_cant: Default::default(),
            scene_kept: None,
            kept_stale: false,
            kept_serial: 0,
            kept_drawn: None,
            image_came_at: Instant::now(),
            far_redrawn: Instant::now(),
            vello_starting,
            vello_uses: HashMap::new(),
            vello_shown: HashMap::new(),
            vello_in_view: HashMap::new(),
            fragments: HashMap::new(),
            swept: Instant::now(),
            gpu,
            kit: Kit::new(),
            scene: Scene::default(),
            tree: None,
            octaves: Vec::new(),
            phase: (0.0, 0.0),
            units: Vec::new(),
            pictures: HashMap::new(),
            images: HashMap::new(),
            asked: HashMap::new(),
            wanted: HashMap::new(),
            needed: HashMap::new(),
            grown: HashMap::new(),
            failed: HashMap::new(),
            tiles: HashMap::new(),
            mismatched: HashMap::new(),
            scratch: Vec::new(),
            pending: None,
            made: 0,
            rasters: HashMap::new(),
            raster_bytes: 0,
            rasters_made: 0,
            slice: None,
            drawn_tiles: None,
            pipelined: false,
            rasters_off: super::dev() && std::env::var("LIGMA_NATIVE_RASTERS").is_ok_and(|value| value == "0"),
            spare: Vec::new(),
            published: None,
            changes_drawn: 0,
            images_later: std::collections::VecDeque::new(),
            edited_seq: 0,
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
            budget: None,
            pressed: false,
            verifying: false,
            verified: 0,
            pyramid: Vec::new(),
            between: Vec::new(),
            drawn: Instant::now(),
            released: true,
            refitted: true,
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
            zoom_was: Instant::now(),
            moved: Instant::now(),
            motion: None,
            velocity: (0.0, 0.0),
            widening: 0.0,
            whole_took: Duration::ZERO,
            unchecked: None,
            chunked: None,
            seq: 0,
            tab: None,
            whole_sum: None,
            parked: None,
            park: true,
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
            units_due: None,
            frame: 0,
            tint: false,
            dirty: true,
            requests,
            effects,
            effects_waiting: Vec::new(),
            effects_running: HashMap::new(),
            effects_again: HashMap::new(),
            effect_nodes: HashMap::new(),
            effect_last: HashMap::new(),
            effect_images: HashMap::new(),
            waiting_areas: HashMap::new(),
            image_units: HashMap::new(),
            versions: HashMap::new(),
            sharpened: HashMap::new(),
            version_next: 0,
            revision: 1,
            checked_revision: 0,
            checked: Vec::new(),
            touched: Vec::new(),
            units_rebuilt: true,
            checked_at: HashMap::new(),
            graves: Vec::new(),
            maker: None,
            kept,
        }
    }

    /// Frees textures let go of (see `Tex`) once the GPU's done with them:
    /// all asked of it so far is submitted, and freed once the queue passes
    /// a fence signalled after
    fn bury(&mut self) {
        let _span = super::span::span("bury");
        let buried = super::present::buried();
        if !buried.is_empty() {
            let _ = self.gpu.context.flush_and_submit();
            let value = self.gpu.signal();
            self.graves.push((value, buried));
        }
        let passed = self.gpu.passed();
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.graves).into_iter().partition(|(value, _)| *value <= passed);
        self.graves = kept;
        let gone: Vec<_> = gone.into_iter().flat_map(|(_, resources)| resources).collect();
        if !gone.is_empty() {
            match &self.maker {
                Some(maker) => {
                    let _ = maker.work.send(Making::Free(gone));
                }
                None => drop(gone),
            }
        }
    }

    pub fn run(mut self, messages: Receiver<Msg>) {
        super::present::drawing();
        self.serve(messages);
        let _ = self.gpu.context.flush_submit_and_sync_cpu();
        drop(self);
        super::present::done();
    }

    fn serve(&mut self, messages: Receiver<Msg>) {
        super::profile::register();
        super::monitor::start();
        // How long until there's more to draw, if there is
        let mut next: Option<Duration> = Some(Duration::ZERO);
        // Time busy, passes and the slowest this second, while tracing
        let mut second = (Duration::ZERO, 0u32, Duration::ZERO, Instant::now());
        loop {
            if tracing() {
                super::span::begin();
            }
            self.bury();
            let waiting = Instant::now();
            let wait = if self.dirty || !self.images_later.is_empty() { Duration::ZERO } else { next.unwrap_or(Duration::from_millis(100)).min(Duration::from_millis(100)) };
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
            let idle = waiting.elapsed();
            let mut message = first;
            {
                let _span = super::span::span("messages");
                // Images are taken in a few milliseconds' worth a pass, as
                // a page's worth coming at once would hold up all else
                let mut on_images = Duration::ZERO;
                while on_images < IMAGES_A_PASS {
                    let Some(image) = self.images_later.pop_front() else { break };
                    let started = Instant::now();
                    self.handle(image);
                    on_images += started.elapsed();
                }
                while let Some(one) = message {
                    // What else comes is handled with the units as of the
                    // frames before it, but for the GPU waking it, the view,
                    // and images, which only mark where they're drawn
                    if !matches!(one, Msg::Frame(..) | Msg::Gpu | Msg::View(..) | Msg::Image(..)) {
                        self.settle_units();
                    }
                    if matches!(one, Msg::Image(..)) {
                        if on_images >= IMAGES_A_PASS || !self.images_later.is_empty() {
                            self.images_later.push_back(one);
                        } else {
                            let started = Instant::now();
                            self.handle(one);
                            on_images += started.elapsed();
                        }
                    } else if !self.handle(one) {
                        return;
                    }
                    message = messages.try_recv().ok();
                }
                self.settle_units();
            }
            // The latest frame is handed over before more is drawn, which
            // takes a while, and again with what's drawn
            self.dirty = false;
            self.publish();
            next = self.fill();
            // With nothing more to draw at once, the whole view Vello drew
            // last is shown once it's drawn
            if next != Some(Duration::ZERO) && self.unchecked.is_some() {
                self.settle_unchecked();
            }
            if next != Some(Duration::ZERO) {
                self.settle_tiles();
            }
            self.publish();
            if std::mem::take(&mut self.drew) {
                self.drawn = Instant::now();
                self.released = false;
                self.refitted = false;
                self.freed = false;
                // Images sized for where the view was go while it moves on,
                // not only once it stops
                if self.zoom.1.elapsed() >= SETTLE && self.fitted.elapsed() > Duration::from_secs(1) {
                    self.fitted = Instant::now();
                    self.fit_images(false);
                } else if self.fitted.elapsed() > FIT_SOON && self.images.values().map(Img::bytes).sum::<usize>() > self.images_budget() {
                    // Past the budget as more come into view, those it's
                    // left behind go at once, zooming too, not a second on
                    self.fitted = Instant::now();
                    self.fit_images(true);
                }
            } else if !self.released && self.drawn.elapsed() > IDLE {
                // Layers and scratch from drawing tiles stay with the GPU
                // driver for reuse until given back
                self.released = true;
                self.trim();
            } else if !self.refitted && self.drawn.elapsed() > NEEDED_FOR {
                // Images kept bigger for where the view was moving are made
                // as small as the view needs once that's past
                self.refitted = true;
                self.fit_images(false);
            } else if !self.freed && self.drawn.elapsed() > FREE_AFTER {
                self.freed = true;
                self.release();
            }
            // Where the time went while frames waited on this thread
            if let Some((took, steps)) = super::span::end(SLOW_STEP, idle) {
                second.0 += took;
                second.1 += 1;
                second.2 = second.2.max(took);
                if took > SLOW_PASS {
                    let (used, budget) = self.gpu.used_and_budget();
                    trace(|| format!("slow render pass {:.0}ms, gpu {} of {} MB: {steps}", took.as_secs_f64() * 1000.0, used >> 20, budget >> 20));
                }
                // How busy this thread was, a line a second
                if second.3.elapsed() >= Duration::from_secs(1) {
                    let (used, budget) = self.gpu.used_and_budget();
                    trace(|| {
                        format!(
                            "render thread busy {:.0}% over {:.0}ms, {} passes, slowest {:.0}ms, gpu {} of {} MB",
                            second.0.as_secs_f64() * 100.0 / second.3.elapsed().as_secs_f64(),
                            second.3.elapsed().as_secs_f64() * 1000.0,
                            second.1,
                            second.2.as_secs_f64() * 1000.0,
                            used >> 20,
                            budget >> 20
                        )
                    });
                    trace(|| self.holds());
                    second = (Duration::ZERO, 0, Duration::ZERO, Instant::now());
                }
            }
            self.dirty = next == Some(Duration::ZERO);
        }
    }

    /// Dev tools: what it holds, for the trace, once a second
    fn holds(&self) -> String {
        let mb = |bytes: usize| bytes as f64 / (1024.0 * 1024.0);
        let cache = self.gpu.context.resource_cache_usage();
        let stale = self.tiles.values().filter(|tile| tile.stale).count();
        let whole = self.whole.as_ref().map_or_else(|| "none".to_string(), |whole| format!("{}x{} px={:.3}{}", whole.layer.tex.width, whole.layer.tex.height, whole.layer.px, if whole.stale { " stale" } else { "" }));
        format!(
            "render holds: units={} nodes={} images={} {:.0}MB originals={} {:.0}MB tiles={} ({stale} stale) {:.0}MB rasters={} {:.0}MB pictures={} thumbs={} whole={whole} skia cache={:.0}MB of {} resources asked={} images queued={} effects waiting={} running={} | {}",
            self.units.len(),
            self.tree.as_ref().map_or(0, |tree| tree.forest.nodes.len()),
            self.images.len(),
            mb(self.images.values().map(Img::bytes).sum()),
            super::run::ENCODED_HELD.0.load(std::sync::atomic::Ordering::Relaxed),
            mb(super::run::ENCODED_HELD.1.load(std::sync::atomic::Ordering::Relaxed)),
            self.tiles.len(),
            mb(self.tiles.len() * (TILE * TILE * 4) as usize),
            self.rasters.len(),
            mb(self.raster_bytes),
            self.pictures.len(),
            self.thumbs.len(),
            mb(cache.resource_bytes),
            cache.resource_count,
            self.asked.len(),
            self.images_later.len(),
            self.effects_waiting.len(),
            self.effects_running.len(),
            self.vello.as_ref().map_or_else(|| "no vello".to_string(), |vello| vello.holds()),
        ) + &self.image_holds()
    }

    /// Dev tools: what images take, by kind and by size, and the biggest,
    /// for the trace
    fn image_holds(&self) -> String {
        let mb = |bytes: usize| bytes as f64 / (1024.0 * 1024.0);
        let mut kinds = [(0usize, 0usize); 3];
        let mut sizes = [(0usize, 0usize); 5];
        let mut biggest: Vec<(usize, String)> = Vec::new();
        for (key, img) in &self.images {
            let bytes = img.bytes();
            let kind = if key.starts_with(super::draw::RENDER) { 1 } else if key.starts_with(super::draw::EFFECT) { 2 } else { 0 };
            kinds[kind].0 += 1;
            kinds[kind].1 += bytes;
            let side = img.image.width().max(img.image.height());
            let size = match side {
                ..=256 => 0,
                ..=512 => 1,
                ..=1024 => 2,
                ..=2048 => 3,
                _ => 4,
            };
            sizes[size].0 += 1;
            sizes[size].1 += bytes;
            let needed = self.needed.get(key).map_or(0.0, |(width, _)| *width);
            let asked = self.asked.get(key).copied().unwrap_or(0.0);
            let vello = self.vello_in_view.get(key).map_or(0.0, |(width, _)| *width as f64);
            biggest.push((bytes, format!("{}x{} of {}x{} {} needed={needed:.0} asked={asked:.0} vello={vello:.0}", img.image.width(), img.image.height(), img.original.0, img.original.1, key.get(..12).unwrap_or(key))));
        }
        biggest.sort_by(|a, b| b.0.cmp(&a.0));
        let biggest: Vec<String> = biggest.iter().take(5).map(|(bytes, what)| format!("{:.0}MB {what}", mb(*bytes))).collect();
        format!(
            " | images fills={} {:.0}MB renders={} {:.0}MB effects={} {:.0}MB, by side <=256 {}/{:.0}MB <=512 {}/{:.0}MB <=1k {}/{:.0}MB <=2k {}/{:.0}MB >2k {}/{:.0}MB, biggest {}",
            kinds[0].0,
            mb(kinds[0].1),
            kinds[1].0,
            mb(kinds[1].1),
            kinds[2].0,
            mb(kinds[2].1),
            sizes[0].0,
            mb(sizes[0].1),
            sizes[1].0,
            mb(sizes[1].1),
            sizes[2].0,
            mb(sizes[2].1),
            sizes[3].0,
            mb(sizes[3].1),
            sizes[4].0,
            mb(sizes[4].1),
            biggest.join(", ")
        )
    }

    /// The tiles frames of the view show now, as the present thread
    /// composes them
    fn shown_tiles(&self) -> std::collections::HashSet<TileKey> {
        let Some(view) = self.view.as_ref() else { return std::collections::HashSet::new() };
        // By texture, as `layers` leaves some tiles out
        let keys: HashMap<usize, TileKey> = self.tiles.iter().map(|(key, tile)| (Arc::as_ptr(&tile.tex) as usize, *key)).collect();
        let layers = self.layers(true);
        let (pieces, _) = cover(&layers, self.content(), view, self.gpu.size);
        pieces.iter().filter_map(|piece| keys.get(&(Arc::as_ptr(&layers[piece.layer].tex) as usize)).copied()).collect()
    }

    /// The view's area in units and its zoom
    fn view_area(&self) -> Option<(Rect, f32)> {
        let view = self.view.as_ref()?;
        let (px, _, width, height) = view.drawn(self.gpu.size);
        Some((Rect::from_xywh(view.x as f32, view.y as f32, width as f32 / px, height as f32 / px), px))
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
        // The furthest is all there is, wherever the view is and however
        // far in it's zoomed, so no zoom out ever finds nothing drawn
        let fits = (width as f32 / content.width()).min(height as f32 / content.height()).log2().floor().exp2();
        // Powers of two, whatever the zoom, so zooming in a little keeps them
        let mut pk = (cp / 2.0).log2().floor().exp2();
        for _ in 0..PYRAMID_LEVELS - 1 {
            if pk <= fits {
                break;
            }
            let (w, h) = (width as f32 / pk, height as f32 / pk);
            let mut area = Rect::from_xywh(center.0 - w / 2.0, center.1 - h / 2.0, w, h);
            if area.intersect(content) {
                levels.push((area, pk));
            }
            pk /= 2.0;
        }
        levels.push((content, fits));
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
        let _span = super::span::span("fill");
        self.check();
        self.govern();
        // Till the file's first frame comes there's nothing to draw, not
        // even its background, and Figma's own canvas shows meanwhile
        self.tree.as_ref()?;
        // While Vello starts, a second or so, what's shown stays, as Skia
        // drawing the view meanwhile can take seconds on a big file
        if !self.vello_ready() && self.vello_starting.is_some() {
            return Some(Duration::from_millis(16));
        }
        let view = self.view.clone()?;
        let (px, preview, width, height) = view.drawn(self.gpu.size);
        if !px.is_finite() || px <= 0.0 {
            return None;
        }
        if px != self.zoom.0 {
            self.zoom_was = self.zoom.1;
            self.zoom = (px, Instant::now());
        }
        self.frame += 1;
        let pxb = px.to_bits();
        // From the pixel the window's corner is in, as Figma draws from a
        // fraction of one, and a preview's pixels are magnified. Figma's
        // canvas can be a pixel wider than where the browser shows it
        let (width, height) = if preview { (width, height) } else { (width + 2, height + 2) };
        let ((ox, phase_x), (oy, phase_y)) = (super::present::phase(view.x * px as f64), super::present::phase(view.y * px as f64));
        let phase = if preview { (0.0, 0.0) } else { (phase_x, phase_y) };
        self.phase = phase;
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
        let view_area = Rect::from_xywh(ox as f32 / px, oy as f32 / px, width as f32 / px, height as f32 / px);
        for (key, tile) in &mut self.tiles {
            if tile.seen && !Rect::intersects2(&view_area, &key.area()) {
                tile.seen = false;
                self.dirty = true;
            }
        }
        self.at = (ox, oy, pxb);
        // The view Vello drew here last is shown rather than drawn again
        if self.unchecked.as_ref().is_some_and(|unchecked| unchecked.at == (ox, oy, pxb)) {
            self.settle_unchecked();
        }
        let missing = in_view.iter().filter(|key| !self.tiles.contains_key(key)).count();
        let behind = in_view.iter().any(|key| self.tiles.get(key).is_some_and(|tile| tile.stale));
        // While something's changing, what changed is drawn into the whole
        // view at once, each frame, as Figma draws it, and tiles wait
        let editing = EDITING.saturating_sub(self.edited.elapsed());
        // While the zoom changes, Figma changes what it shows of some nodes
        // each frame, which isn't an edit to keep up with exactly
        let settle = SETTLE.saturating_sub(self.zoom.1.elapsed());
        // Dev tools: Vello draws the view from the scene it keeps, edits and
        // all, so one way of drawing shows what's there. Each frame draws it
        // all again, which takes the GPU longer than tiles do for now
        if self.vello_ready() && self.tree.is_some() && settle.is_zero() && std::env::var_os("LIGMA_NATIVE_VELLO_KEPT").is_some() {
            if let Some(next) = self.fill_kept(px, (ox, oy, width, height)) {
                return next;
            }
        }
        if editing.is_zero() {
            self.drop_strata();
            self.changing.clear();
        }
        if !editing.is_zero() && settle.is_zero() && (behind || missing > 0 || self.whole.is_some()) {
            let current = self.whole.as_ref().filter(|whole| whole.at == (ox, oy, pxb) && !whole.stale && !whole.rough).map(|whole| (whole.patch.is_empty(), whole.vello));
            // One drawn at a time, what changes meanwhile drawn together
            // once the GPU's done
            if self.whole_busy() {
                return Some(Duration::from_millis(1));
            }
            match current {
                Some((true, _)) => return Some(editing),
                // Drawn again by what drew it, as Skia draws images and
                // effects a little otherwise, which would show where it
                // patched it till Vello drew the view again
                Some((_, true)) if self.vello_ready() => {
                    let phase = self.phase;
                    let view = (ox, oy, width, height);
                    if self.fill_strata(px, view, phase, false) || self.patch_whole_vello() || self.fill_strata(px, view, phase, true) {
                        return Some(Duration::ZERO);
                    }
                    if let Some(next) = self.fill_once(px, (ox, oy, width, height), phase) {
                        self.dirty = true;
                        return next;
                    }
                    self.patch_whole();
                }
                Some(_) => self.patch_whole(),
                None => self.draw_whole(ox, oy, width, height, px, true),
            }
            self.dirty = true;
            return Some(Duration::ZERO);
        }
        if self.moved.elapsed() >= PAN_STILL && self.whole.as_ref().is_some_and(|whole| !whole.patch.is_empty() && whole.at == (ox, oy, pxb) && !whole.stale) {
            if self.whole_busy() {
                return Some(Duration::from_millis(1));
            }
            // What changed that isn't an edit, an image come or the like,
            // is drawn from thumbs, as the view is meanwhile, rather than
            // exactly, which for a big image is hundreds of milliseconds,
            // as tiles are drawn exactly after. Vello draws it from where
            // the view is, as it would next, rather than off it, softer, for
            // a few frames
            if settle.is_zero() && self.vello_ready() {
                if let Some(next) = self.fill_once(px, (ox, oy, width, height), phase) {
                    return next;
                }
            }
            self.draw_whole(ox, oy, width, height, px, true);
            self.dirty = true;
            return Some(Duration::ZERO);
        }
        // Once the zoom settles and the view's still, Vello draws the view
        // at once, exactly, so it's sharp in one pass rather than tile by
        // tile, whose tiles are drawn behind it for pans. While it pans,
        // tiles are shown, which a frame only moves
        // Not with a button held, as the view's about to be dragged again,
        // and drawing all of it can take the GPU hundreds of milliseconds
        let still = self.moved.elapsed() >= PAN_STILL && !super::present::pointer_held();
        let sharp = in_view.iter().all(|key| self.tiles.get(key).is_some_and(|tile| !tile.stale && tile.exact));
        let whole_at = self.whole.as_ref().is_some_and(|whole| whole.at == (ox, oy, pxb) && whole.layer.phase == phase && !whole.stale && whole.patch.is_empty() && !whole.rough && whole.vello && !whole.patched);
        let unchecked_at = self.unchecked.as_ref().is_some_and(|unchecked| unchecked.at == (ox, oy, pxb) && unchecked.layer.phase == phase);
        // Tiles land on whole pixels, so where Figma draws the view from a
        // fraction of one, it's drawn from there too once it's still
        let between = phase != (0.0, 0.0);
        // Drawn a part at a time once tiles show it all exactly, as they
        // draw what's missing sooner, and would be drawn twice
        if self.chunked.as_ref().is_some_and(|chunked| chunked.at != (ox, oy, pxb) || chunked.phase != phase || self.edited.elapsed() < chunked.started.elapsed() || !still) {
            if let Some(chunked) = self.chunked.take() {
                self.retire(chunked.tex);
            }
        }
        if self.vello_ready() && self.tree.is_some() && settle.is_zero() && still && sharp && between && !whole_at && !unchecked_at && std::env::var_os("LIGMA_NATIVE_VELLO_WHOLES").is_none() {
            if let Some(next) = self.fill_chunked(px, (ox, oy, width, height), phase) {
                return next;
            }
        }
        // Vello draws tiles, of a level at or past the zoom while it changes,
        // which frames scale till the zoom leaves them, and exactly at the
        // zoom once it's settled
        if self.vello_ready() && self.tree.is_some() && std::env::var_os("LIGMA_NATIVE_VELLO_WHOLES").is_none() {
            let furthest = levels.first().map(|(_, pk)| pk.to_bits());
            // A zoom changing once, as a jump to somewhere does, is drawn
            // at once at the zoom it's at, not first a level up, which took
            // a tenth of a second the view then waited on
            let zooming = !settle.is_zero() && self.zoom.1.duration_since(self.zoom_was) < SETTLE;
            let next = self.fill_vello(px, (ox, oy, width, height), furthest, &pyramid, zooming);
            return if next.is_none() && !settle.is_zero() { Some(settle) } else { next };
        }
        let whole_here = self.whole.as_ref().is_some_and(|whole| whole.at == (ox, oy, pxb) && !whole.stale && whole.patch.is_empty() && !whole.rough);
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
        let rough: Vec<TileKey> = in_view.iter().copied().filter(|key| self.tiles.get(key).is_some_and(|tile| !tile.stale && !tile.draft && !tile.exact)).collect();
        // A view Vello drew with units of other levels standing in, shown
        // till exact tiles cover it, is drawn again exactly first
        let rough_whole = self.whole.as_ref().is_some_and(|whole| whole.rough && whole.at == (ox, oy, pxb) && !whole.stale && whole.patch.is_empty());
        if rough_whole && (missing > 0 || behind || !rough.is_empty()) && self.vello_ready() {
            if self.whole_busy() {
                return Some(Duration::from_millis(1));
            }
            self.draw_whole(ox, oy, width, height, px, true);
            self.dirty = true;
            return Some(Duration::ZERO);
        }
        // A view Vello drew is as exact as tiles, and takes less memory, so
        // it's kept rather than tiles drawn, which close up can take seconds, the furthest level, all there is, which stands in where zooming
        // out passes what Vello's drawn, so the view never waits for it
        if whole_here && self.whole.as_ref().is_some_and(|whole| whole.vello) {
            let furthest = levels.first().map(|(_, pk)| pk.to_bits());
            let gone: Vec<TileKey> = self.tiles.keys().filter(|key| Some(key.px) != furthest).copied().collect();
            for key in &gone {
                if let Some(tile) = self.tiles.remove(key) {
                    self.drop_tile(tile);
                }
            }
            if !gone.is_empty() {
                trace(|| format!("  tiles dropped as Vello drew the whole view: {}", gone.len()));
            }
            self.dirty |= !gone.is_empty();
            let missing = pyramid.iter().copied().find(|key| Some(key.px) == furthest && !self.tiles.get(key).is_some_and(|tile| !tile.stale));
            if let Some(key) = missing {
                // Drawn once the view's been still a while, as a tile of it
                // takes tens of milliseconds no frame can be drawn in. Vello
                // draws it, as Skia records every unit of a big file for it
                let still = VELLO_STILL.saturating_sub(self.moved.elapsed().min(self.edited.elapsed()));
                if !still.is_zero() {
                    return Some(still);
                }
                self.vello_or_skia(&[key], false);
                self.dirty = true;
                return Some(Duration::ZERO);
            }
            self.update_reason(&view, px, width, height);
            return None;
        }
        let ready = |tiles: &HashMap<TileKey, Tile>, key: &TileKey| tiles.get(key).is_some_and(|tile| !tile.stale && (!tile.draft || key.px != pxb));
        let mut order: Vec<(TileKey, bool)> = Vec::new();
        // Where tiles are missing, the coarse level first: it covers four
        // or more tiles' worth at once, so a pan finds something drawn
        if missing > 0 && !whole_here {
            order.extend(coarse_view.iter().copied().filter(|key| !self.tiles.contains_key(key)).map(|key| (key, false)));
        }
        order.extend(in_view.iter().copied().filter(|key| !self.tiles.contains_key(key)).map(|key| (key, false)));
        // Then the view's tiles exactly, before anything else, once it's
        // still, nor while the page changes, as while a file loads, which
        // would draw them again soon anyway
        let refine = REFINE_AFTER.saturating_sub(self.moved.elapsed().min(self.edited.elapsed()));
        if refine.is_zero() {
            order.extend(rough.iter().map(|&key| (key, true)));
        }
        // Then the furthest level, all there is, which zooming out shows
        // till what's nearer is drawn, so none ever waits. Once drawn it's
        // shown, while out of date too, so it's drawn again only when idle
        let furthest = levels.first().map(|(_, pk)| pk.to_bits());
        // Vello draws the whole view each frame, wherever it goes, so tiles
        // only stand in where it falls behind, which the furthest level
        // does, drawn once the view's been still a while, as a tile of it
        // is hundreds of milliseconds no frame can be drawn in
        let vello = self.vello.is_some();
        let still = VELLO_STILL.saturating_sub(self.moved.elapsed().min(self.edited.elapsed()));
        if !vello || still.is_zero() {
            order.extend(pyramid.iter().copied().filter(|key| Some(key.px) == furthest && !self.tiles.contains_key(key)).map(|key| (key, false)));
        }
        // An out of date tile is drawn again as well as it was, the whole
        // view standing in meanwhile
        order.extend(in_view.iter().filter_map(|key| self.tiles.get(key).filter(|tile| tile.stale || tile.draft).map(|tile| (*key, tile.exact))));
        if !vello {
            order.extend(around.iter().copied().filter(|key| !ready(&self.tiles, key)).map(|key| (key, false)));
            order.extend(coarse_view.iter().chain(&coarse_around).copied().filter(|key| !ready(&self.tiles, key) && key.px != pxb).map(|key| (key, false)));
            // And the levels between, so zooming out shows them less blurred
            order.extend(pyramid.iter().copied().filter(|key| !self.tiles.contains_key(key)).map(|key| (key, false)));
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
        if vello {
            let waiting = pyramid.iter().any(|key| Some(key.px) == furthest && !self.tiles.contains_key(key));
            return match (rough.is_empty(), waiting) {
                (false, _) => Some(refine),
                (true, true) => Some(still),
                (true, false) => None,
            };
        }
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
            // What it holds goes with it, so the GPU has room for it again
            self.vello = None;
            self.release();
            self.vello_restarts.retain(|at| at.elapsed() < VELLO_RESTARTS_IN);
            if self.vello_restarts.len() < VELLO_RESTARTS {
                self.vello_restarts.push(Instant::now());
                log::warn!("native renderer: Vello stopped after an error, starting it again");
                trace(|| "Vello stopped after an error, starting it again".into());
                self.vello_starting = Some(super::vello::start(&self.gpu.device));
            } else {
                log::warn!("native renderer: Vello stopped after an error, Skia draws the view");
                trace(|| "Vello stopped after an error, Skia draws the view".into());
            }
        }
        self.vello.is_some()
    }

    /// The whole view drawn by Vello at the zoom it's at, every path of it
    /// each time, over what `draw_whole` draws. False where it can't be,
    /// for Skia to draw it
    fn draw_whole_vello(&mut self, ox: i32, oy: i32, width: i32, height: i32, px: f32) -> bool {
        let _span = super::span::span("draw whole vello");
        if self.too_big() {
            return false;
        }
        // The last, once the GPU's drawn it, is shown before this is encoded
        if self.unchecked.as_ref().is_some_and(|unchecked| self.gpu.passed() >= unchecked.fence) {
            self.settle_shown();
        }
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
        // Blurs draw what's behind them in scenes of their own, so none is
        // drawn around the view for them, as Skia's need
        let (margin, reach) = (0, view_area);
        let (w, h) = (width, height);
        let budget = if SETTLE.saturating_sub(self.zoom.1.elapsed()).is_zero() { ENCODE_SETTLED } else { ENCODE_ZOOMING };
        // Drawn while the GPU draws the last, which is checked after
        match self.vello_view_once(ox - margin, oy - margin, w, h, px, reach, Some(budget)) {
            Ok((tex, counts, stats, rough)) => {
                let fence = self.gpu.signal();
                let renders = self.vello.as_ref().map_or(0, |vello| vello.renders);
                let last = self.unchecked.replace(Unchecked { layer: Layer { tex, x: ox - margin, y: oy - margin, phase: (0.0, 0.0), px, counts, kind: LayerKind::Whole, drawn: 0 }, fence, at, renders, started, stats, rough, patch: Vec::new(), seq: self.seq });
                if let Some(last) = last {
                    self.settle(last);
                }
                self.drew = true;
                true
            }
            Err(why) => {
                trace(|| format!("  whole vello px={px:.3} can't: {why}"));
                // What was drawn before is checked and shown, and Skia
                // draws this
                self.settle_unchecked();
                false
            }
        }
    }

    /// Vello draws the view from the scene it keeps on the GPU, built again
    /// only where the view leaves it, its zoom changes or something in it
    /// does. None where Vello can't, for tiles to be drawn instead
    /// The view drawn by Vello at once, exactly, shown once it's checked.
    /// None where Vello can't, for tiles to be drawn instead
    fn fill_once(&mut self, px: f32, (ox, oy, width, height): (i32, i32, i32, i32), phase: (f32, f32)) -> Option<Option<Duration>> {
        let _span = super::span::span("fill once");
        if self.too_big() {
            return None;
        }
        let started = Instant::now();
        let reach = Rect::from_xywh(ox as f32 / px, oy as f32 / px, width as f32 / px, height as f32 / px);
        self.settle_shown();
        match self.vello_render(ox, oy, phase, width, height, px, reach, None, true, None) {
            Ok((tex, counts, stats, _)) => {
                let fence = self.gpu.signal();
                let renders = self.vello.as_ref().map_or(0, |vello| vello.renders);
                let at = (ox, oy, px.to_bits());
                let last = self.unchecked.replace(Unchecked { layer: Layer { tex, x: ox, y: oy, phase, px, counts, kind: LayerKind::Whole, drawn: 0 }, fence, at, renders, started, stats, rough: false, patch: Vec::new(), seq: self.seq });
                if let Some(last) = last {
                    self.settle(last);
                }
                self.drew = true;
                Some(Some(Duration::ZERO))
            }
            Err(why) => {
                trace(|| format!("  once vello px={px:.3} can't: {why}"));
                None
            }
        }
    }

    /// A part more of the view Vello draws exactly at the fraction of a
    /// pixel it's at, see `Chunked`, shown once it's all drawn. None where
    /// Vello can't, for tiles to stand
    fn fill_chunked(&mut self, px: f32, (ox, oy, width, height): (i32, i32, i32, i32), phase: (f32, f32)) -> Option<Option<Duration>> {
        let _span = super::span::span("fill chunked");
        if self.too_big() {
            return None;
        }
        let at = (ox, oy, px.to_bits());
        if self.chunked.is_none() {
            let tex = self.tex(width, height)?;
            let mut parts = Vec::new();
            for y in (0..height).step_by(TILE as usize) {
                for x in (0..width).step_by(TILE as usize) {
                    parts.push((x, y, TILE.min(width - x), TILE.min(height - y)));
                }
            }
            let (cx, cy) = (width / 2, height / 2);
            parts.sort_by_key(|&(x, y, w, h)| (x + w / 2 - cx).pow(2) + (y + h / 2 - cy).pow(2));
            self.chunked = Some(Chunked { at, phase, tex, next: 0, parts, counts: Counts::default(), started: Instant::now(), seq: self.seq });
        }
        let (x, y, w, h) = {
            let chunked = self.chunked.as_ref()?;
            chunked.parts[chunked.next]
        };
        let reach = Rect::from_xywh((ox + x) as f32 / px, (oy + y) as f32 / px, w as f32 / px, h as f32 / px);
        let started = Instant::now();
        self.slice = Some(ENCODE_SLICE);
        let mut drawn = None;
        for _ in 0..8 {
            let rendered = self.vello_render(ox + x, oy + y, phase, w, h, px, reach, None, false, None);
            let (out, counts) = match rendered {
                Ok((out, counts, ..)) => (out, counts),
                Err(why) if why == UNFINISHED => {
                    self.slice = None;
                    return Some(Some(Duration::ZERO));
                }
                Err(why) => {
                    self.slice = None;
                    trace(|| format!("  chunked vello px={px:.3} can't: {why}"));
                    if let Some(chunked) = self.chunked.take() {
                        self.retire(chunked.tex);
                    }
                    return None;
                }
            };
            let vello = self.vello.as_mut()?;
            match vello.check(usize::MAX) {
                Ok(None) => {
                    drawn = Some((out, counts));
                    break;
                }
                Ok(Some(why)) if !why.starts_with("too big") => trace(|| format!("  vello ran out of room, drawing again: {why}")),
                _ => break,
            }
        }
        self.slice = None;
        let Some((out, counts)) = drawn else {
            if let Some(chunked) = self.chunked.take() {
                self.retire(chunked.tex);
            }
            return None;
        };
        let target = self.chunked.as_ref()?.tex.clone();
        let image = gpu::images::borrow_texture_from(&mut self.gpu.context, &out.backend(), SurfaceOrigin::TopLeft, ColorType::RGBA8888, AlphaType::Unpremul, None)?;
        let mut surface = self.target(&target)?;
        let mut paint = Paint::default();
        paint.set_blend_mode(skia_safe::BlendMode::Src);
        let from = Rect::from_wh(w as f32, h as f32);
        surface.canvas().draw_image_rect(&image, Some((&from, skia_safe::canvas::SrcRectConstraint::Fast)), Rect::from_xywh(x as f32, y as f32, w as f32, h as f32), &paint);
        drop(surface);
        drop(image);
        self.finish(&target.backend(), false)?;
        let chunked = self.chunked.as_mut()?;
        chunked.counts += counts;
        chunked.next += 1;
        trace(|| format!("  chunked vello px={px:.3} part {} of {} at {x},{y} {w}x{h} {:.1}ms", chunked.next, chunked.parts.len(), started.elapsed().as_secs_f64() * 1000.0));
        if chunked.next == chunked.parts.len() {
            let chunked = self.chunked.take()?;
            let fence = self.gpu.signal();
            let layer = Layer { tex: chunked.tex, x: ox, y: oy, phase, px, counts: chunked.counts, kind: LayerKind::Whole, drawn: 0 };
            trace(|| format!("  whole vello chunked px={px:.3} {width}x{height} in {} parts {:.1}ms", chunked.parts.len(), chunked.started.elapsed().as_secs_f64() * 1000.0));
            self.hand_whole(layer, fence, at, false, true, chunked.seq);
            self.drew = true;
        }
        self.dirty = true;
        Some(Some(Duration::ZERO))
    }

    fn fill_kept(&mut self, px: f32, (ox, oy, width, height): (i32, i32, i32, i32)) -> Option<Option<Duration>> {
        let _span = super::span::span("fill kept");
        let pxb = px.to_bits();
        let at = (ox, oy, pxb);
        let inside = self.scene_kept.as_ref().filter(|kept| kept.px == pxb && (!self.kept_stale || (kept.built.elapsed() < KEPT_CHANGES && self.edited.elapsed() >= EDITING)) && ox >= kept.left && oy >= kept.top && ox + width <= kept.left + kept.width && oy + height <= kept.top + kept.height);
        if inside.is_some_and(|kept| self.kept_drawn == Some((kept.id, at))) {
            // Built again once it's been kept a while
            return Some(self.kept_stale.then(|| KEPT_CHANGES.saturating_sub(self.scene_kept.as_ref().map_or(Duration::ZERO, |kept| kept.built.elapsed()))));
        }
        let started = Instant::now();
        let inside = inside.map(|kept| (kept.id, kept.left, kept.top));
        // A kept scene used though out of date leaves what changed since out
        let behind = match (inside.is_some() && self.kept_stale, self.whole.as_mut()) {
            (true, Some(whole)) => whole.patch.clone(),
            (_, Some(whole)) => std::mem::take(&mut whole.patch),
            _ => Vec::new(),
        };
        let drawn = match inside {
            Some(kept) => {
                let (id, x, y) = (kept.0, kept.1 - ox, kept.2 - oy);
                self.vello_again(id, x, y, width, height)
            }
            None => {
                let why = match &self.scene_kept {
                    None => "none",
                    Some(kept) if kept.px != pxb => "zoom",
                    Some(_) if self.kept_stale => "changed",
                    Some(_) => "outside",
                };
                trace(|| format!("  kept vello built again: {why}"));
                self.scene_kept = None;
                self.kept_stale = false;
                let (mx, my) = ((width as f32 * KEPT_MARGIN) as i32, (height as f32 * KEPT_MARGIN) as i32);
                let (left, top, w, h) = (ox - mx, oy - my, width + 2 * mx, height + 2 * my);
                let reach = Rect::from_xywh(left as f32 / px, top as f32 / px, w as f32 / px, h as f32 / px);
                self.kept_serial += 1;
                let id = self.kept_serial;
                let kept = super::vello::Kept::at(id, false, (left - ox) as f32, (top - oy) as f32);
                self.settle_unchecked();
                let drawn = self.vello_render(left, top, (0.0, 0.0), w, h, px, reach, None, true, Some((kept, width, height))).map(|(tex, counts, stats, _)| (tex, counts, stats));
                if drawn.is_ok() {
                    self.scene_kept = Some(KeptScene { id, px: pxb, left, top, width: w, height: h, built: Instant::now() });
                }
                drawn
            }
        };
        match drawn {
            Ok((tex, counts, stats)) => {
                let fence = self.gpu.signal();
                let renders = self.vello.as_ref().map_or(0, |vello| vello.renders);
                self.kept_drawn = self.scene_kept.as_ref().map(|kept| (kept.id, at));
                let last = self.unchecked.replace(Unchecked { layer: Layer { tex, x: ox, y: oy, phase: (0.0, 0.0), px, counts, kind: LayerKind::Whole, drawn: 0 }, fence, at, renders, started, stats, rough: false, patch: behind, seq: self.seq });
                if let Some(last) = last {
                    self.settle(last);
                }
                self.drew = true;
                Some(Some(Duration::ZERO))
            }
            // What was kept is gone: built again next frame
            Err(why) if inside.is_some() => {
                trace(|| format!("  kept vello px={px:.3} again can't: {why}"));
                self.scene_kept = None;
                Some(Some(Duration::ZERO))
            }
            Err(why) => {
                trace(|| format!("  kept vello px={px:.3} can't: {why}"));
                self.scene_kept = None;
                None
            }
        }
    }

    /// The scene Vello keeps as `id` drawn again, its corner at (`x`, `y`)
    /// in a `w` by `h` view
    fn vello_again(&mut self, id: u64, x: i32, y: i32, w: i32, h: i32) -> Result<(Arc<Tex>, Counts, String), String> {
        let _span = super::span::span("vello again");
        let started = Instant::now();
        let Some(mut vello) = self.vello.take() else { return Err("no Vello".into()) };
        let plan = super::vello::Plan::new();
        let kept = super::vello::Kept::at(id, true, x as f32, y as f32);
        let rendered = vello.render(&self.gpu.device, &self.gpu.queue, &vello::Scene::new(), w, h, 0, &[], &plan, kept);
        self.vello = Some(vello);
        let out = rendered?;
        let cpu = started.elapsed();
        let tex = self.vello_copy(&out, w, h)?;
        Ok((tex, Counts::default(), format!("{w}x{h} again at {x},{y} cpu={:.2}ms copied={:.2}ms", cpu.as_secs_f64() * 1000.0, started.elapsed().as_secs_f64() * 1000.0)))
    }

    /// What Vello drew into `out`, unpremultiplied, copied into a texture
    /// of the kind the present thread shows
    fn vello_copy(&mut self, out: &Arc<Tex>, w: i32, h: i32) -> Result<Arc<Tex>, String> {
        let _span = super::span::span("vello copy");
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
        Ok(tex)
    }

    /// What `fill` does with Vello: the view's tiles at the level it's
    /// drawn at, then those of the level the zoom's headed to, then the
    /// furthest level, which stands in wherever the others don't reach
    fn fill_vello(&mut self, px: f32, (ox, oy, width, height): (i32, i32, i32, i32), furthest: Option<u32>, pyramid: &[TileKey], zooming: bool) -> Option<Duration> {
        let _span = super::span::span("fill vello");
        let frame = self.frame;
        let area = Rect::from_xywh(ox as f32 / px, oy as f32 / px, width as f32 / px, height as f32 / px);
        let over = |level: f32, area: Rect| {
            let (left, top) = ((area.left * level).floor() as i32, (area.top * level).floor() as i32);
            let (right, bottom) = ((area.right * level).ceil() as i32, (area.bottom * level).ceil() as i32);
            Self::tiles_over(left, top, (right - left - 1).max(0), (bottom - top - 1).max(0), level.to_bits(), 0)
        };
        // Zooming out, the level nearest, as the one at or past it would be
        // up to four times the view's tiles, drawn to be shown at a quarter
        // of their size, and the zoom's left it by the time they're drawn
        let out = self.widening > 0.0;
        let level_for = |px: f32| if out { level_near(px) } else { level_up(px) };
        let level = if zooming { level_for(px) } else { px };
        let here = over(level, area);
        // Where the zoom will be in a moment, at the level it'll be drawn at
        let ahead = if zooming {
            let lag = 0.25;
            let grow = 2f64.powf((self.widening * lag).clamp(-2.0, 2.0)) as f32;
            let next = level_for(px / grow);
            let center = area.center();
            let (w, h) = (area.width() * grow, area.height() * grow);
            let (x, y) = (center.x + (self.velocity.0 * lag) as f32, center.y + (self.velocity.1 * lag) as f32);
            // Past the furthest level, which is always there, a zoom out
            // shows nothing more
            let past = furthest.is_some_and(|furthest| next < f32::from_bits(furthest));
            if next != level && !past { over(next, Rect::from_xywh(x - w / 2.0, y - h / 2.0, w, h)) } else { Vec::new() }
        } else if self.moved.elapsed() < Duration::from_millis(150) {
            // Where a pan's headed
            let lag = 0.25;
            let (dx, dy) = ((self.velocity.0 * lag) as f32, (self.velocity.1 * lag) as f32);
            over(level, Rect::from_xywh(area.left + dx, area.top + dy, area.width(), area.height())).into_iter().filter(|key| !here.contains(key)).collect()
        } else {
            Vec::new()
        };
        let far: Vec<TileKey> = pyramid.iter().copied().filter(|key| Some(key.px) == furthest).collect();
        // Once it's still, the levels between the view and the furthest,
        // each a power of two further out, over what the view shows zoomed
        // out to it, but what the one before covers. Zooming out then finds
        // a level at most an octave off wherever it goes, not the furthest,
        // many further. They take the room tiles around the view and of
        // places left took, the nearest first, as many as `VELLO_TILES` has
        if !zooming {
            let mut found = Vec::new();
            let content = self.content();
            let mut inner = area;
            let mut next = (px.log2() - 1e-4).floor().exp2();
            let room = VELLO_TILES.saturating_sub(here.len() + far.len());
            // First the level a zoom in draws first, over as much of the view
            // around its centre as half the room holds, nearest the centre
            // first, so it starts sharp rather than from what's at rest
            // scaled up, drawn while the zoom's past it
            let up = level_up(px);
            if up > px * 1.001 {
                let center = area.center();
                let mut part = 1.0;
                let mut tiles = over(up, area);
                while tiles.len() > room / 2 && part > 0.25 {
                    part *= 0.85;
                    let (w, h) = (area.width() * part, area.height() * part);
                    tiles = over(up, Rect::from_xywh(center.x - w / 2.0, center.y - h / 2.0, w, h));
                }
                if tiles.len() <= room / 2 {
                    let off = |key: &TileKey| {
                        let at = key.area();
                        (at.center_x() - center.x).hypot(at.center_y() - center.y)
                    };
                    tiles.sort_by(|a, b| off(a).total_cmp(&off(b)));
                    found.extend(tiles);
                }
            }
            while furthest.is_none_or(|furthest| next > f32::from_bits(furthest)) {
                let (w, h) = (area.width() * px / next, area.height() * px / next);
                let center = area.center();
                let outer = Rect::from_xywh(center.x - w / 2.0, center.y - h / 2.0, w, h);
                let mut reach = outer;
                if !content.is_some_and(|content| reach.intersect(content)) {
                    break;
                }
                let inside = |tile: Rect| tile.left >= inner.left && tile.top >= inner.top && tile.right <= inner.right && tile.bottom <= inner.bottom;
                let tiles: Vec<TileKey> = over(next, reach).into_iter().filter(|key| !inside(key.area())).collect();
                if found.len() + tiles.len() > room {
                    break;
                }
                found.extend(tiles);
                inner = outer;
                next /= 2.0;
            }
            self.between = found;
        }
        let levels = self.between.clone();
        for key in here.iter().chain(&ahead).chain(&far).chain(&levels) {
            if let Some(tile) = self.tiles.get_mut(key) {
                tile.used = frame;
            }
        }
        // Once it's still, only exact tiles will do, as one drawn while it
        // zoomed can be of the same level
        let ready = |tiles: &HashMap<TileKey, Tile>, key: &TileKey| tiles.get(key).is_some_and(|tile| !tile.stale && (zooming || tile.exact || Some(key.px) == furthest));
        let missing: Vec<TileKey> = here.iter().copied().filter(|key| !ready(&self.tiles, key)).collect();
        // Drawn from the fraction of a pixel the view is at, it's sharper
        // than tiles moved there
        let between = self.phase != (0.0, 0.0) && self.whole.as_ref().is_some_and(|whole| whole.at == (ox, oy, px.to_bits()) && whole.layer.phase == self.phase);
        if missing.is_empty() {
            // All of the view's there, so what stood in for it goes
            if let Some(whole) = self.whole.take_if(|_| !between) {
                self.retire_whole(whole);
                self.dirty = true;
            }
            if self.last_whole.is_some() {
                self.drop_last_whole();
                self.dirty = true;
            }
        }
        // What the view needs, and what's drawn ahead of where the zoom's
        // headed and for levels between, which goes first when there's no
        // room, after what's not needed
        let keep: Vec<TileKey> = here.iter().chain(&far).copied().collect();
        let spare: Vec<TileKey> = ahead.iter().chain(&levels).copied().collect();
        // Tiles frames show stay till what's drawn for the view covers them,
        // so what's ahead waits while there's no room, rather than drawn to
        // go again
        let full = self.tiles.len() >= VELLO_TILES;
        let ahead_missing: Vec<TileKey> = if full { Vec::new() } else { ahead.iter().copied().filter(|key| !ready(&self.tiles, key)).collect() };
        let between_missing: Vec<TileKey> = if full || zooming || self.moved.elapsed() < PAN_STILL { Vec::new() } else { levels.iter().copied().filter(|key| !ready(&self.tiles, key)).collect() };
        let between_waits = !zooming && levels.iter().any(|key| !ready(&self.tiles, key));
        let images_quiet = self.image_came_at.elapsed() >= IMAGES_QUIET || self.far_redrawn.elapsed() >= FAR_EVERY;
        // The furthest level changes as the zoom goes out, past which the
        // one there is stands in till it's still
        let far_missing: Vec<TileKey> = match zooming && self.tiles.keys().any(|key| key.px <= far.first().map_or(0, |key| key.px)) {
            true => Vec::new(),
            // One there that's out of date waits till images stop coming
            // in, as a file opens, and the view's still, as each takes long
            // and stands in only while the zoom goes out
            false => far.iter().copied().filter(|key| !ready(&self.tiles, key) && (!self.tiles.contains_key(key) || (images_quiet && self.moved.elapsed() >= VELLO_STILL))).collect(),
        };
        // The furthest drawn while the zoom changed is drawn again exactly
        // once it's been still a while, as is one out of date, as units of
        // other levels standing in for its own can look wrong
        let far_rough: Vec<TileKey> = match zooming || !images_quiet || self.moved.elapsed() < VELLO_STILL {
            true => Vec::new(),
            false => far.iter().copied().filter(|key| self.tiles.get(key).is_some_and(|tile| !tile.exact)).collect(),
        };
        // While the view moves, tiles it doesn't show yet take the GPU at
        // most half the time, as the browser drawing Figma's frames shares
        // it, and Figma sends none while it waits
        let moving = self.moved.elapsed() < VELLO_STILL;
        // While the zoom changes, what it shows first, then the level it's
        // headed to. The furthest is drawn exactly at once when it's still,
        // as standing in saves it little there, where the GPU's time is
        // most of it
        let todo = match zooming {
            true => [(missing, false, false), (ahead_missing, false, true), (far_missing, false, true), (Vec::new(), false, true), (Vec::new(), false, true)],
            false => [(missing, true, false), (ahead_missing, true, true), (far_missing, !moving, true), (between_missing, true, true), (far_rough, true, true)],
        };
        for (keys, exact, later) in todo {
            if keys.is_empty() {
                continue;
            }
            if keys.iter().any(|key| Some(key.px) == furthest && self.tiles.contains_key(key)) {
                self.far_redrawn = Instant::now();
            }
            let started = Instant::now();
            self.vello_or_skia(&keys, exact);
            self.evict_vello(&keep, &spare, furthest);
            if later && moving {
                return Some(started.elapsed());
            }
            self.dirty = true;
            return Some(Duration::ZERO);
        }
        self.evict_vello(&keep, &spare, furthest);
        if zooming {
            return Some(SETTLE.saturating_sub(self.zoom.1.elapsed()));
        }
        if between_waits {
            return Some(PAN_STILL.saturating_sub(self.moved.elapsed()).max(Duration::from_millis(1)));
        }
        // Back once images stop coming in, for those waiting on that
        let rough = far.iter().any(|key| self.tiles.get(key).is_some_and(|tile| !tile.exact));
        if far.iter().any(|key| !ready(&self.tiles, key)) || rough {
            let quiet = IMAGES_QUIET.saturating_sub(self.image_came_at.elapsed()).min(FAR_EVERY.saturating_sub(self.far_redrawn.elapsed()));
            return Some(quiet.max(VELLO_STILL.saturating_sub(self.moved.elapsed())).max(Duration::from_millis(1)));
        }
        let view = self.view.clone()?;
        self.update_reason(&view, px, width, height);
        None
    }

    /// Draws the first of `keys` and those near it with Vello, or Skia where
    /// Vello can't
    fn vello_or_skia(&mut self, keys: &[TileKey], exact: bool) {
        let _span = super::span::span("vello or skia");
        // Those drawn and not yet checked are put once checked
        let drawing = |key: &TileKey| self.drawn_tiles.as_ref().is_some_and(|drawn| drawn.tiles.iter().any(|(at, ..)| at == key));
        let keys: Vec<TileKey> = keys.iter().copied().filter(|key| !drawing(key)).collect();
        if keys.is_empty() {
            self.settle_tiles();
            return;
        }
        let vello: Vec<TileKey> = keys.iter().copied().filter(|key| !self.vello_cant.contains(key)).collect();
        if !vello.is_empty() {
            self.slice = Some(ENCODE_SLICE);
            let drawn = self.vello_tiles(&vello, exact);
            self.slice = None;
            if let Err((key, why)) = drawn {
                if why == UNFINISHED {
                    trace(|| format!("  vello tile {},{} px={:.4} encoded in part", key.x, key.y, key.px()));
                    return;
                }
                self.vello_cannot(key, &why);
            }
            return;
        }
        self.settle_tiles();
        let key = keys[0];
        if let Some(tile) = self.draw_tile(key, exact, false) {
            self.put_tile(key, tile);
            self.drew = true;
        }
    }

    /// Draws the tiles of `keys` in the block of `VELLO_BLOCK` tiles a side
    /// the first is in with Vello, at once, all of a level
    fn vello_tiles(&mut self, keys: &[TileKey], mut exact: bool) -> Result<(), (TileKey, String)> {
        let _span = super::span::span("vello tiles");
        let first = keys[0];
        let _purpose = super::gpu_census::purpose("drawing tiles");
        let started = Instant::now();
        let block = std::env::var("LIGMA_NATIVE_VELLO_BLOCK").ok().and_then(|n| n.parse().ok()).unwrap_or(VELLO_BLOCK).max(1);
        let corner = |key: &TileKey| (key.x.div_euclid(block), key.y.div_euclid(block));
        let batch_span = super::span::span("batch");
        let mine: Vec<TileKey> = if block > 1 { keys.iter().copied().filter(|key| key.px == first.px && corner(key) == corner(&first)).collect() } else { self.batch(first, keys) };
        drop(batch_span);
        let (x0, y0) = (mine.iter().map(|key| key.x).min().unwrap_or(first.x), mine.iter().map(|key| key.y).min().unwrap_or(first.y));
        let (x1, y1) = (mine.iter().map(|key| key.x).max().unwrap_or(first.x), mine.iter().map(|key| key.y).max().unwrap_or(first.y));
        let (left, top, w, h) = (x0 * TILE, y0 * TILE, (x1 - x0 + 1) * TILE, (y1 - y0 + 1) * TILE);
        let px = first.px();
        let reach = Rect::from_xywh(left as f32 / px, top as f32 / px, w as f32 / px, h as f32 / px);
        self.settle_unchecked();
        let fail = |why: String| (first, why);
        // Tiles to scale while the zoom changes have units encoded for
        // other levels stand in, drawn at this one's scale, as one unit can
        // take tens of milliseconds to encode, which the next level doesn't
        // wait for. Tiles once it's still are exact
        let budget = (!exact).then_some(Duration::ZERO);
        self.pipelined = true;
        let rendered = self.vello_render(left, top, (0.0, 0.0), w, h, px, reach, budget, false, None);
        self.pipelined = false;
        let drawn = rendered.and_then(|(out, counts, stats, rough)| {
            // With nothing standing in, it's as drawing them exactly would
            // be, which then isn't done again
            exact |= !rough;
            let _span = super::span::span("copy out");
            let image = gpu::images::borrow_texture_from(&mut self.gpu.context, &out.backend(), SurfaceOrigin::TopLeft, ColorType::RGBA8888, AlphaType::Unpremul, None).ok_or("Vello's texture")?;
            let mut paint = Paint::default();
            paint.set_blend_mode(skia_safe::BlendMode::Src);
            // Copied out each into a tile of its own, submitted at once
            let mut tiles = Vec::new();
            for &key in &mine {
                let tex = self.tex(TILE, TILE).ok_or("texture")?;
                let mut surface = self.target(&tex).ok_or("surface")?;
                let from = Rect::from_xywh(((key.x - x0) * TILE) as f32, ((key.y - y0) * TILE) as f32, TILE as f32, TILE as f32);
                surface.canvas().draw_image_rect(&image, Some((&from, skia_safe::canvas::SrcRectConstraint::Fast)), Rect::from_wh(TILE as f32, TILE as f32), &paint);
                drop(surface);
                tiles.push((key, tex, false, false));
            }
            let _ = self.gpu.context.flush_and_submit();
            Ok(DrawnTiles { tiles, first, renders: 0, exact, counts, stats, started, submitted: started.elapsed(), span: (x0, y0, x1 - x0 + 1, y1 - y0 + 1) })
        });
        // Those drawn before are checked now these are with the GPU, which
        // leaves these the renders unchecked
        self.settle_tiles();
        let mut drawn = drawn.map_err(fail)?;
        drawn.renders = self.vello.as_ref().map_or(0, |vello| vello.unchecked());
        self.drawn_tiles = Some(drawn);
        Ok(())
    }

    /// Puts the tiles Vello drew last once it's checked it had room for
    /// them, waiting for the GPU to draw them
    fn settle_tiles(&mut self) {
        let Some(drawn) = self.drawn_tiles.take() else { return };
        let _span = super::span::span("settle tiles");
        let at = Instant::now();
        let checked = match self.vello.as_mut() {
            Some(vello) => vello.check(drawn.renders).map(|short| (short, vello.waited, std::mem::take(&mut vello.stages))),
            None => Err("no Vello".into()),
        };
        let (waited, stages) = match checked {
            Ok((None, waited, stages)) => (waited, stages),
            failed => {
                for (_, tex, ..) in drawn.tiles {
                    self.retire(tex);
                }
                match failed {
                    // Drawn again with the room it needs where it's still
                    // wanted
                    Ok((Some(why), ..)) if !why.starts_with("too big") => trace(|| format!("  vello ran out of room, drawing again: {why}")),
                    Ok((Some(why), ..)) | Err(why) => self.vello_cannot(drawn.first, &why),
                    Ok((None, ..)) => {}
                }
                return;
            }
        };
        let exact = drawn.exact;
        for (key, tex, stale, seen) in drawn.tiles {
            let Some(image) = gpu::images::borrow_texture_from(&mut self.gpu.context, &tex.backend(), SurfaceOrigin::TopLeft, ColorType::RGBA8888, AlphaType::Premul, None) else {
                self.retire(tex);
                continue;
            };
            self.put_tile(key, Tile { image, tex, used: self.frame, counts: drawn.counts, stale, seen, exact, draft: false, vello: true, drawn: 0 });
        }
        self.drew = true;
        self.dirty = true;
        super::stats().lock().unwrap().vello_tiles_ms = drawn.started.elapsed().as_secs_f64() * 1000.0;
        trace(|| {
            let ms = |d: Duration| d.as_secs_f64() * 1000.0;
            let (x0, y0, w, h) = drawn.span;
            let px = drawn.first.px();
            format!(
                "  vello tiles px={px:.4} at {x0},{y0} {w}x{h} exact={exact} rendered={:.1}ms submitted={:.1} held={:.1} waited={:.1} {} gpu:{stages}",
                ms(drawn.started.elapsed()),
                ms(drawn.submitted),
                ms(at.duration_since(drawn.started).saturating_sub(drawn.submitted)),
                ms(waited),
                drawn.stats
            )
        });
    }

    /// Notes Vello can't draw the tile at `key`, which Skia then draws
    fn vello_cannot(&mut self, key: TileKey, why: &str) {
        trace(|| format!("  vello tile {},{} px={:.4} can't: {why}", key.x, key.y, key.px()));
        if self.vello_cant.len() > 4096 {
            self.vello_cant.clear();
        }
        self.vello_cant.insert(key);
    }

    /// `first`, and the tiles of `keys` beside it in its row while together
    /// they draw few units, as each render takes a millisecond or two however
    /// little it draws, and a zoom's tiles mostly draw little. A unit over
    /// several is drawn once for them all. Not those drawing images the
    /// others don't, as Vello's atlas would grow to hold all of them at once
    fn batch(&self, first: TileKey, keys: &[TileKey]) -> Vec<TileKey> {
        let px = first.px();
        let (top, bottom) = ((first.y * TILE) as f32 / px, ((first.y + 1) * TILE) as f32 / px);
        let mut held = [0usize; 2 * BATCH_TILES - 1];
        let reach = BATCH_TILES as i32 - 1;
        let column = |at: f32| (at * px / TILE as f32).floor() as i32 - first.x;
        let mut in_row = Vec::new();
        for unit in &self.units {
            let b = &unit.bounds;
            if b.bottom <= top || b.top >= bottom || b.width().max(b.height()) * px < 0.5 {
                continue;
            }
            let (from, to) = (column(b.left).max(-reach), column(b.right).min(reach));
            if from > to {
                continue;
            }
            for x in from..=to {
                held[(x + reach) as usize] += 1;
            }
            in_row.push((from, to, unit));
        }
        // Too many alone, which dense tiles mostly are, it's drawn alone
        // without looking up what draws images
        if held[reach as usize] > BATCH_UNITS {
            return vec![first];
        }
        // The images each tile draws, where units drew them last, or the
        // unit for those not drawn yet the tree says draw images
        let mut images: [Vec<u64>; 2 * BATCH_TILES - 1] = Default::default();
        let hash = |what: &dyn Fn(&mut std::collections::hash_map::DefaultHasher)| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            what(&mut hasher);
            hasher.finish()
        };
        for &(from, to, unit) in &in_row {
            if (from..=to).all(|x| held[(x + reach) as usize] > BATCH_UNITS) {
                continue;
            }
            match self.vello_uses.get(&(unit.slot, unit.section)) {
                Some((_, uses)) => {
                    for (key, _, at) in uses {
                        if at.bottom <= top || at.top >= bottom {
                            continue;
                        }
                        let id = hash(&|hasher| key.hash(hasher));
                        for x in column(at.left).max(from)..=column(at.right).min(to) {
                            images[(x + reach) as usize].push(id);
                        }
                    }
                }
                None if unit.custom || unit.place.as_ref().is_none_or(|place| self.tree.as_ref().is_none_or(|tree| tree.draws_images(place.id))) => {
                    let id = hash(&|hasher| (unit.slot, unit.section).hash(hasher));
                    for x in from..=to {
                        images[(x + reach) as usize].push(id);
                    }
                }
                None => {}
            }
        }
        let (mut left, mut right) = (0, 0);
        let mut total = held[reach as usize];
        let mut drawing = images[reach as usize].clone();
        // Units over `x` not over the tiles already taken
        let more = |x: i32, left: i32, right: i32| in_row.iter().filter(|(from, to, _)| (*from..=*to).contains(&x) && (*to < left || *from > right)).count();
        while right - left + 1 < BATCH_TILES as i32 {
            let next = [right + 1, left - 1].into_iter().find(|&x| {
                let at = (x + reach) as usize;
                x.abs() <= reach
                    && keys.contains(&TileKey { x: first.x + x, ..first })
                    && held[at] <= BATCH_UNITS
                    && (drawing.is_empty() || images[at].iter().all(|id| drawing.contains(id)))
                    && total + more(x, left, right) <= BATCH_UNITS
            });
            let Some(x) = next else { break };
            total += more(x, left, right);
            drawing.extend_from_slice(&images[(x + reach) as usize]);
            if x > right {
                right = x;
            } else {
                left = x;
            }
        }
        (left..=right).map(|x| TileKey { x: first.x + x, ..first }).collect()
    }

    /// Tiles past `VELLO_TILES` go, those shown longest ago first and those
    /// of `later` after, never those frames show, those of `keep` or the
    /// furthest level
    fn evict_vello(&mut self, keep: &[TileKey], later: &[TileKey], furthest: Option<u32>) {
        let _span = super::span::span("evict vello");
        // What Skia makes drawing what Vello can't, often textures of a
        // pixel or two, each taking 64 KiB though Skia counts it as a few
        // bytes, so its cache's budget never lets them go. And images let
        // go, and what they were uploaded through, which Skia keeps to use
        // again though few are ever the same size, so moving the view
        // would pile them up
        if self.cleaned.elapsed() > UNUSED {
            self.cleaned = Instant::now();
            self.gpu.context.perform_deferred_cleanup(UNUSED, Some(skia_safe::gpu::PurgeResourceOptions::ScratchResourcesOnly));
            self.gpu.context.perform_deferred_cleanup(Duration::from_secs(1), None);
            super::committed::tidy();
        }
        // Past the budget, what's not shown goes. What frames show now
        // stays, however far from the zoom, till what's drawn for the view
        // covers it, as what's under it is a level far blurrier and the
        // view would go soft where it was clear a moment before. Over the
        // budget by at most that while a level's drawn
        let shown_tiles = self.shown_tiles();
        let shown = |key: &TileKey| shown_tiles.contains(key);
        let extra = self.tiles.len().saturating_sub(VELLO_TILES);
        let out_of_view = if self.pressed { self.tiles.keys().filter(|key| Some(key.px) != furthest && !keep.contains(key) && !later.contains(key) && !shown(key)).count() } else { 0 };
        let extra = extra.max(out_of_view);
        if extra == 0 {
            return;
        }
        // Longest unused first, then those ahead
        let mut old: Vec<(bool, u64, TileKey)> = self.tiles.iter().filter(|(key, _)| Some(key.px) != furthest && !keep.contains(key) && !shown(key)).map(|(key, tile)| (later.contains(key), tile.used, *key)).collect();
        old.sort_unstable_by_key(|(ahead, used, _)| (*ahead, *used));
        trace(|| format!("  tiles evicted: {} of {}, {} shown kept", extra.min(old.len()), self.tiles.len(), shown_tiles.len()));
        for (_, _, key) in old.into_iter().take(extra) {
            if let Some(tile) = self.tiles.remove(&key) {
                self.drop_tile(tile);
            }
        }
    }

    /// As `settle_unchecked`, and shows it before more is drawn, which
    /// would hold it back by as long as that takes to encode
    fn settle_shown(&mut self) {
        if self.unchecked.is_some() {
            self.settle_unchecked();
            self.publish();
        }
    }

    /// Hands over the whole view Vello drew last once it's checked it had
    /// room for it, waiting for the GPU to draw it
    fn settle_unchecked(&mut self) {
        if let Some(last) = self.unchecked.take() {
            self.settle(last);
        }
    }

    fn settle(&mut self, last: Unchecked) {
        let _span = super::span::span("settle");
        let Some(vello) = self.vello.as_mut() else { return };
        let short = vello.check(last.renders);
        let waited = vello.waited;
        let stages = std::mem::take(&mut vello.stages);
        match short {
            Ok(None) => {
                trace(|| format!("  whole vello px={:.3} {} checked={:.1}ms {:.1}ms gpu:{stages}", last.layer.px, last.stats, waited.as_secs_f64() * 1000.0, last.started.elapsed().as_secs_f64() * 1000.0));
                self.whole_took = last.started.elapsed();
                stats().lock().unwrap().whole_ms = self.whole_took.as_secs_f64() * 1000.0;
                self.hand_whole(last.layer, last.fence, last.at, last.rough, true, last.seq);
                // What changed as the GPU drew it isn't in it, as when an
                // object's moved, which would show it where it was
                if let Some(whole) = self.whole.as_mut() {
                    whole.patch = last.patch;
                }
                // At rest alone, as it waits for the GPU
                if !last.rough && self.moved.elapsed() >= PAN_STILL && self.edited.elapsed() >= EDITING {
                    self.verify_tiles();
                }
            }
            // Never shown, and drawn again with the room it needs where
            // it's still wanted
            Ok(Some(why)) => {
                trace(|| format!("  vello ran out of room, drawing again: {why}"));
                if why.starts_with("too big") {
                    self.vello_too_big = Some(Instant::now());
                }
                self.retire(last.layer.tex);
                self.kept_drawn = None;
                self.dirty = true;
            }
            Err(why) => {
                trace(|| format!("  whole vello can't check: {why}"));
                self.retire(last.layer.tex);
                self.kept_drawn = None;
                self.dirty = true;
            }
        }
    }

    /// The `w` by `h` device pixels at `px` whose corner is at (`left`,
    /// `top`), of units over `reach`, drawn by Vello into a texture of the
    /// kind the present thread shows, submitted, and how long that took
    fn vello_view(&mut self, left: i32, top: i32, w: i32, h: i32, px: f32, reach: Rect) -> Result<(Arc<Tex>, Counts, String), String> {
        let _span = super::span::span("vello view");
        self.settle_unchecked();
        // Drawn again where it ran out of room in Vello's buffers, which
        // then have more, before it's shown
        for _ in 0..8 {
            let (tex, counts, stats, _) = self.vello_view_once(left, top, w, h, px, reach, None)?;
            let vello = self.vello.as_mut().ok_or("no Vello")?;
            let short = vello.check(usize::MAX)?;
            if let Some(why) = short {
                trace(|| format!("  vello ran out of room, drawing again: {why}"));
                if why.starts_with("too big") {
                    self.vello_too_big = Some(Instant::now());
                    return Err(why);
                }
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

    fn vello_view_once(&mut self, left: i32, top: i32, w: i32, h: i32, px: f32, reach: Rect, budget: Option<Duration>) -> Result<(Arc<Tex>, Counts, String, bool), String> {
        let _span = super::span::span("vello view once");
        self.vello_render(left, top, (0.0, 0.0), w, h, px, reach, budget, true, None)
    }

    /// As `vello_view_once`, handing back Vello's own target, unpremultiplied
    /// and drawn into again by its next render, where `copy` isn't set
    #[allow(clippy::too_many_arguments)]
    /// The view from (`left`, `top`) and `phase` of a pixel further, in
    /// device pixels, `w` by `h`
    #[allow(clippy::too_many_arguments)]
    /// Images too many for Vello's atlas at the size the view needs are
    /// drawn a level coarser at a time till they fit, rather than Skia
    /// drawing the view, which on big files takes seconds. Finer again once
    /// they'd fit with room to spare
    #[allow(clippy::too_many_arguments)]
    fn vello_render(&mut self, left: i32, top: i32, phase: (f32, f32), w: i32, h: i32, px: f32, reach: Rect, budget: Option<Duration>, copy: bool, kept: Option<(super::vello::Kept, i32, i32)>) -> Result<(Arc<Tex>, Counts, String, bool), String> {
        // Vello's renders are checked in order, so tiles drawn before are
        // checked first
        if !self.pipelined {
            self.settle_tiles();
        }
        loop {
            match self.vello_render_at(left, top, phase, w, h, px, reach, budget, copy, kept) {
                Err(why) if (why.starts_with("images need") || why == "images past Vello's atlas") && self.image_coarser < COARSEST_IMAGES => {
                    self.image_coarser += 1;
                    trace(|| format!("  images a level coarser, {} in all, as {why}", self.image_coarser));
                }
                Err(why) if (why == "blurs past their texture" || why == "blur bigger than its texture" || why == "blurs past their textures") && self.blur_coarser < COARSEST_BLURS => {
                    self.blur_coarser += 1;
                    trace(|| format!("  blurs a level coarser, {} in all, as {why}", self.blur_coarser));
                }
                done => {
                    if done.is_ok() && self.image_coarser > 0 && self.atlas_used * 4 < ATLAS_MOST / 2 {
                        self.image_coarser -= 1;
                    }
                    // Back a level once they'd fit at it with room to spare
                    if done.is_ok() && self.blur_coarser > 0 && self.blur_rows < 8192 / 8 {
                        self.blur_coarser -= 1;
                    }
                    return done;
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn vello_render_at(&mut self, left: i32, top: i32, phase: (f32, f32), w: i32, h: i32, px: f32, reach: Rect, budget: Option<Duration>, copy: bool, kept: Option<(super::vello::Kept, i32, i32)>) -> Result<(Arc<Tex>, Counts, String, bool), String> {
        let _span = super::span::span("vello render at");
        let started = Instant::now();
        let Some(mut vello) = self.vello.take() else { return Err("no Vello".into()) };
        let _ = self.gpu.context.flush_and_submit();
        let only = self.only;
        let (from_x, from_y) = (left as f64 + phase.0 as f64, top as f64 + phase.1 as f64);
        let mut post = skia_safe::Matrix::translate((-from_x as f32, -from_y as f32));
        post.pre_scale((px, px), None);
        let mut asked = Vec::new();
        let (mut units, mut counts, mut inexact, mut skia_took, mut kinds) = (0, Counts::default(), 0, Duration::ZERO, String::new());
        let level = super::vello::level(px);
        // Of those it draws alone, as hashing them all takes a while
        let stamps: Vec<u64> = self
            .units
            .iter()
            .map(|unit| {
                if !Rect::intersects2(&unit.bounds, &reach) {
                    return 0;
                }
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
        let mut widths: Vec<(String, f32)> = Vec::new();
        let (mut reused, mut made_count) = (0, 0);
        let (mut encoding, mut rough) = (Duration::ZERO, false);
        let mut under_area = 0;
        // Blurs, drawn and put in place for the view
        let mut plan = super::vello::Plan::new();
        plan.coarser = self.blur_coarser;
        plan.blur_view = self.blur_view;
        let whole = vello::kurbo::Rect::new(0.0, 0.0, w as f64, h as f64);
        let encoding_span = super::span::span("encode scene");
        let made_span = super::span::span("make fragments");
        let (scene, failed, fell_back, mut pages) = match self.tree.as_ref() {
            None => (vello::Scene::new(), Some("not a tree".to_string()), 0, Vec::new()),
            Some(tree) => {
                let now = Instant::now();
                // Dev tools: every unit encoded for the view
                let fresh = std::env::var_os("LIGMA_NATIVE_VELLO_FRESH").is_some();
                let (images, units_all) = (&self.images, &self.units);
                // The units in view, by index, with the fragment each draws,
                // where there's one of use yet
                let mut shown: Vec<(usize, Option<(usize, bool, i32)>)> = Vec::new();
                // Dev tools: the units drawn with the most paths
                let mut biggest: Vec<(u32, u32, u32, Rect)> = Vec::new();
                // Those to encode, by where they're shown, with what stands
                // in past the time to encode: one of a level near, more
                // detailed first, or else one a level coarser, which has less
                // to it, and whether that's encoded already
                let mut jobs: Vec<(usize, Option<(usize, bool, i32)>, bool)> = Vec::new();
                let mut whole_tree = true;
                let finding = super::span::span("find fragments");
                for (index, (unit, &stamp)) in units_all.iter().zip(&stamps).enumerate() {
                    if !Rect::intersects2(&unit.bounds, &reach) || unit.bounds.width().max(unit.bounds.height()) * px < 0.5 || only.is_some_and(|(first, last)| index < first || index > last) {
                        continue;
                    }
                    if unit.place.is_none() {
                        whole_tree = false;
                        break;
                    }
                    let key = (unit.slot, unit.section, level);
                    if !fresh && fragments.get(&key).is_some_and(|encoded| usable(encoded, stamp, true, images)) {
                        shown.push((index, Some(key)));
                        continue;
                    }
                    if !fresh {
                        let near = budget.and_then(|_| {
                            // Those levels finer than the next only where
                            // they draw little, as zooming out past them
                            // drew far tiles from tens of megabytes of what
                            // a level coarser would make in a few
                            [(level + 1, true), (level + 2, false), (level + 3, false), (level - 1, true), (level - 2, true)]
                                .into_iter()
                                .map(|(other, any)| ((unit.slot, unit.section, other), any))
                                .find(|(other, any)| {
                                    fragments.get(other).is_some_and(|encoded| {
                                        usable(encoded, stamp, false, images) && (*any || matches!(&*encoded.made, Ok(fragment) if fragment.scene.encoding().path_data.len() <= STAND_IN_MOST))
                                    })
                                })
                                .map(|(other, _)| other)
                        });
                        let short = fragments.get(&(unit.slot, unit.section, level - 1)).is_some_and(|encoded| usable(encoded, stamp, true, images));
                        jobs.push((shown.len(), near, short));
                    }
                    shown.push((index, None));
                }
                // Standing in from the start, those with a fragment to stand
                // in need nothing encoded, nor threads to encode it, which
                // took a millisecond or more to start for each tile
                drop(finding);
                type Done = Vec<(usize, (usize, bool, i32), bool, Option<Result<super::vello::Fragment, String>>)>;
                let mut quick: Done = Vec::new();
                if budget.is_some_and(|budget| budget.is_zero()) {
                    jobs.retain(|&(at, near, short)| {
                        let unit = &units_all[shown[at].0];
                        let key = match (near, short) {
                            (Some(near), _) => near,
                            (None, true) => (unit.slot, unit.section, level - 1),
                            (None, false) => return true,
                        };
                        quick.push((at, key, true, None));
                        false
                    });
                }
                let drawable: HashMap<String, super::vello::Drawable> = if jobs.is_empty() && !fresh {
                    HashMap::new()
                } else {
                    images.iter().filter_map(|(key, img)| super::vello::Vello::drawable(img).map(|drawable| (key.clone(), drawable))).collect()
                };
                // Encoded by threads at once, each taking the next unit until
                // there are none
                let (paths, begun, next, slice) = (&self.vello_paths, Instant::now(), std::sync::atomic::AtomicUsize::new(0), self.slice);
                let threads = std::env::var("LIGMA_NATIVE_VELLO_THREADS").ok().and_then(|n| n.parse().ok()).unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()).saturating_sub(1).clamp(1, super::pool::MOST)).min(jobs.len());
                let mut worked: Vec<(Done, i64, usize)> = (|| {
                    let (jobs, shown, drawable, next) = (&jobs, &shown, &drawable, &next);
                    let work = move |behind: bool| {
                                // Behind every other thread, the browser's
                                // above all, whose frames Figma's wait on: at
                                // the same priority they halved Figma's frame
                                // rate as new things came into view
                                if behind {
                                    unsafe {
                                        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_LOWEST};
                                        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_LOWEST);
                                    }
                                }
                                let mut encoder = super::vello::Encoder::worker(tree, images, paths, drawable, w, h);
                                encoder.px = Some(px);
                                let mut done = Vec::new();
                                while let Some(&(at, near, short)) = jobs.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) {
                                    if slice.is_some_and(|slice| begun.elapsed() >= slice) {
                                        break;
                                    }
                                    let unit = &units_all[shown[at].0];
                                    let Some(place) = &unit.place else { continue };
                                    let over = budget.is_some_and(|budget| begun.elapsed() >= budget);
                                    let key = match (over, near) {
                                        (false, _) => (unit.slot, unit.section, level),
                                        (true, Some(near)) => near,
                                        (true, None) => (unit.slot, unit.section, level - 1),
                                    };
                                    let made = (!over || (near.is_none() && !short)).then(|| {
                                        let fragment = encoder.fragment(place, (unit.bounds.left, unit.bounds.top), key.2);
                                        match encoder.failed.take() {
                                            Some(why) => Err(why),
                                            None => Ok(fragment),
                                        }
                                    });
                                    done.push((at, key, over, made));
                                }
                                (done, encoder.fell_back, encoder.inexact)
                    };
                    // One unit, as an edit changes, is encoded here at once,
                    // rather than on a thread started for it, behind others
                    if threads == 1 && jobs.len() == 1 {
                        return vec![work(false)];
                    }
                    super::pool::run(threads, &move || work(true))
                })();
                encoding = begun.elapsed();
                drop(made_span);
                let under = self.under_at.map(|at| (vello.under(), at));
                let mut encoder = super::vello::Encoder::new(tree, images, &self.kit, &mut vello, paths, &drawable, &self.gpu.device, &mut self.gpu.context, w, h);
                encoder.coarser = self.image_coarser;
                match under {
                    Some((Some(image), (x, y))) => {
                        under_area = image.width as u64 * image.height as u64;
                        super::vello::put_under(&mut encoder.scene, &image, x, y, w as u32, h as u32);
                    }
                    Some((None, _)) => encoder.failed = Some("nothing kept under".into()),
                    None => {}
                }
                if !whole_tree {
                    encoder.failed = Some("not a tree".into());
                }
                reused += shown.len() - jobs.len();
                let finished: usize = worked.iter().map(|(done, ..)| done.len()).sum();
                // Those made are kept for the pass that draws it
                if finished < jobs.len() {
                    encoder.failed = Some(UNFINISHED.into());
                }
                worked.push((quick, 0, 0));
                for (done, fell_back, inexact) in worked {
                    encoder.fell_back += fell_back;
                    encoder.inexact += inexact;
                    for (at, key, was_rough, made) in done {
                        rough |= was_rough;
                        match made {
                            Some(made) => {
                                made_count += 1;
                                fragments.insert(key, Encoded { stamp: stamps[shown[at].0], made: Box::new(made), used: now });
                            }
                            None => reused += 1,
                        }
                        shown[at].1 = Some(key);
                    }
                }
                let _placing = super::span::span("place fragments");
                for &(index, key) in &shown {
                    if encoder.failed.is_some() {
                        break;
                    }
                    let unit = &units_all[index];
                    let Some(place) = &unit.place else { break };
                    let Some(key) = key else {
                        if !fresh {
                            continue;
                        }
                        encoder.place(place, &post);
                        let (mut effects, mut scene) = (std::mem::take(&mut encoder.effects), std::mem::take(&mut encoder.scene));
                        if let Err(why) = encoder.put(&mut plan, &mut effects, scene.encoding_mut(), vello::kurbo::Affine::IDENTITY, whole, 0) {
                            encoder.failed = Some(why);
                        }
                        encoder.scene = scene;
                        encoder.pictures.clear();
                        pictures.extend(encoder.drawn.drain(..));
                        widths.extend(encoder.shown.drain(..));
                        asked.push(((unit.slot, unit.section), unit.bounds, std::mem::take(&mut encoder.asked)));
                        units += 1;
                        continue;
                    };
                    let Some(encoded) = fragments.get_mut(&key) else { continue };
                    encoded.used = now;
                    match &mut *encoded.made {
                        Ok(fragment) => {
                            let at = fragment.at(px, from_x, from_y);
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
                            if sizing() {
                                biggest.push((fragment.scene.encoding().path_data.len() as u32, place.id, tree.info(place.id).count, unit.bounds));
                            }
                            pictures.extend(encoder.drawn.drain(..));
                            widths.extend(encoder.shown.drain(..));
                            encoder.counts += fragment.counts;
                            let mut unit_asked = fragment.asked.clone();
                            let mut drawn = std::mem::take(&mut encoder.asked);
                            unit_asked.missing.append(&mut drawn.missing);
                            unit_asked.sharper.append(&mut drawn.sharper);
                            unit_asked.used.append(&mut drawn.used);
                            unit_asked.places.append(&mut drawn.places);
                            asked.push(((unit.slot, unit.section), unit.bounds, unit_asked));
                        }
                        Err(why) => encoder.failed = Some(why.clone()),
                    }
                    units += 1;
                }
                if !biggest.is_empty() {
                    biggest.sort_by_key(|(paths, ..)| std::cmp::Reverse(*paths));
                    trace(|| format!("  biggest units: {}", biggest.iter().take(6).map(|(paths, id, count, b)| format!("{id} words={paths} nodes={count} at {:.0},{:.0} {:.0}x{:.0}", b.left, b.top, b.width(), b.height())).collect::<Vec<_>>().join(", ")));
                    let total: u64 = biggest.iter().map(|b| b.0 as u64).sum();
                    trace(|| format!("  biggest of {} words: {}", total, biggest.iter().take(4).map(|b| super::vello::shape_of(tree, b.1, 0).chars().take(400).collect::<String>()).collect::<Vec<_>>().join(" || ")));
                }
                counts += encoder.counts;
                inexact = encoder.inexact;
                skia_took = encoder.skia_took;
                kinds = format!("{:?}", encoder.kinds);
                (std::mem::take(&mut encoder.scene), encoder.failed.take(), encoder.fell_back, encoder.take_pages())
            }
        };
        // Let go of on a thread of their own, as a zoom leaves thousands
        // unused at once, which took hundreds of milliseconds to free
        let let_go = super::span::span("let go of fragments");
        // A few times a second, as looking over tens of thousands took a
        // millisecond or more of each tile
        if self.swept.elapsed() >= FRAGMENTS_KEPT / 8 {
            self.swept = Instant::now();
            let since = self.swept.checked_sub(FRAGMENTS_KEPT);
            let gone: Vec<Encoded> = fragments.extract_if(|_, encoded| since.is_some_and(|since| encoded.used <= since)).map(|(_, encoded)| encoded).collect();
            if gone.len() > 64 {
                std::thread::spawn(move || drop(gone));
            }
        }
        drop(let_go);
        self.fragments = fragments;
        drop(encoding_span);
        let encoded = started.elapsed();
        let mut steps = Vec::new();
        let mut step = Instant::now();
        let mut lap = |steps: &mut Vec<f64>| {
            steps.push((step.elapsed().as_secs_f64() * 10000.0).round() / 10.0);
            step = Instant::now();
        };
        let used_pages = pages.len();
        for page in &mut pages {
            self.gpu.flush(page);
        }
        drop(pages);
        lap(&mut steps);
        if let Some(why) = failed {
            self.vello = Some(vello);
            return Err(why);
        }
        pictures.sort_unstable();
        pictures.dedup();
        // Past what Vello's atlas holds, it'd draw from where other images are
        let area = vello.atlas_area(&pictures, used_pages) + under_area;
        self.atlas_used = area;
        if area > ATLAS_MOST {
            self.vello = Some(vello);
            return Err(format!("images need {}M px of Vello's atlas", area >> 20));
        }
        lap(&mut steps);
        {
            let _span = super::span::span("skia submit");
            let _ = self.gpu.context.flush_and_submit();
        }
        lap(&mut steps);
        // Dev tools: Skia's GPU work for the view done first, to time apart
        let skia_gpu = std::env::var_os("LIGMA_NATIVE_VELLO_SYNC").map(|_| {
            let at = Instant::now();
            let _ = self.gpu.context.flush_submit_and_sync_cpu();
            at.elapsed().as_secs_f64() * 1000.0
        });
        let blurs = plan.len();
        self.blur_rows = plan.blur_rows();
        let (paths, clips) = (scene.encoding().n_paths, scene.encoding().n_clips);
        let scene_mb = {
            use std::mem::size_of_val as size;
            let e = scene.encoding();
            (size(&e.path_tags[..]) + size(&e.path_data[..]) + size(&e.draw_tags[..]) + size(&e.draw_data[..]) + size(&e.transforms[..]) + size(&e.styles[..])) as f64 / 1048576.0
        };
        // A scene kept is drawn into a target of the view's size
        let (kept, tw, th) = kept.unwrap_or((super::vello::Kept::NOT, w, h));
        // Tiles further out than the coarse level take several times what
        // the view does, all there is drawn small
        let kept = super::vello::Kept { one_off: px < coarse(self.zoom.0), ..kept };
        let rendered = vello.render(&self.gpu.device, &self.gpu.queue, &scene, tw, th, used_pages, &pictures, &plan, kept);
        // Coarse tiles under the view draw most of the file, small
        let near_zoom = (px.max(1e-6) / self.zoom.0.max(1e-6)).log2().abs() < 1.0;
        let seen = self.view.as_ref().map(|view| Rect::from_xywh(view.x as f32, view.y as f32, view.w as f32, view.h as f32)).filter(|_| near_zoom);
        let now = Instant::now();
        for (key, width) in widths {
            let widest = self.vello_shown.entry(key).or_insert(0.0);
            *widest = widest.max(width);
        }
        lap(&mut steps);
        drop(plan);
        vello.spare = scene;
        {
            let _span = super::span::span("keep pictures");
            vello.keep_pictures(&self.images);
            self.vello_paths.prune();
        }
        lap(&mut steps);
        self.vello = Some(vello);
        let out = rendered.map_err(|error| {
            log::warn!("native renderer: Vello: {error}");
            error
        })?;
        let vello_took = started.elapsed() - encoded;
        // What Vello drew, unpremultiplied, copied into a texture of the
        // kind the present thread shows
        let tex = if !copy { out } else { self.vello_copy(&out, tw, th)? };
        let asked_span = super::span::span("units asked");
        for (unit, bounds, asked) in asked {
            let super::vello::Asked { missing, sharper, used, places } = asked;
            let drew: Vec<String> = used.iter().chain(&missing).chain(&sharper).map(|(key, _)| key.clone()).collect();
            let images: Vec<(String, f64)> = missing.iter().chain(&sharper).cloned().collect();
            // Where each image's drawn in the unit, each place apart, as an
            // image used across a frame would span it: the whole unit for
            // those drawn without a place, as effects
            let mut placed: HashMap<&str, Vec<Rect>> = HashMap::new();
            for (key, at) in &places {
                let all = placed.entry(key.as_str()).or_default();
                if !all.contains(at) {
                    all.push(*at);
                }
            }
            let centre = seen.map(|seen| (seen.center_x(), seen.center_y()));
            let nearest = |key: &str| {
                let all = placed.get(key).map_or(&[][..], |all| &all[..]);
                let off = |at: &Rect| centre.map_or(0.0, |(x, y)| (at.center_x() - x).hypot(at.center_y() - y));
                all.iter().copied().min_by(|a, b| off(a).total_cmp(&off(b))).unwrap_or(bounds)
            };
            for image in &images {
                self.ask(std::slice::from_ref(image), Some(nearest(&image.0)));
            }
            let waiting: Vec<String> = missing.into_iter().map(|(key, _)| key).collect();
            self.wait_for(&waiting, bounds);
            self.drew_with(drew, unit);
            let mut widest: HashMap<String, f64> = HashMap::new();
            for (key, width) in used.into_iter().chain(images) {
                let entry = widest.entry(key).or_insert(0.0);
                *entry = entry.max(width);
            }
            let uses: Vec<(String, f64, Rect)> = widest
                .into_iter()
                .flat_map(|(key, width)| {
                    let all = placed.get(key.as_str()).cloned().unwrap_or_else(|| vec![bounds]);
                    all.into_iter().map(move |at| (key.clone(), width, at))
                })
                .collect();
            // Figma sends a node again as another while a file loads, and
            // till the new one's drawn nothing alive uses what the old drew
            // with, a stand-in for its custom effects among them
            // As wide as at the view's zoom, as tiles a level up draw it
            // twice as wide
            let to_view = self.zoom.0 as f64 / px.max(1e-6) as f64;
            for (key, width, at) in &uses {
                if seen.is_some_and(|seen| Rect::intersects2(at, &seen)) {
                    let width = &(width * to_view);
                    let (widest, when) = self.vello_in_view.entry(key.clone()).or_insert((0.0, now));
                    *widest = if when.elapsed() < IN_VIEW { widest.max(*width as f32) } else { *width as f32 };
                    *when = now;
                }
            }
            // What the zoom nearest the view's drew it with is what it
            // needs: tiles far out leave things out, and would have what's
            // in view let go and had again over and over
            let off = |at: f32| (at.max(1e-6) / self.zoom.0.max(1e-6)).log2().abs();
            if self.vello_uses.get(&unit).is_none_or(|(was, _)| off(px) <= off(*was) + 0.01) {
                self.vello_uses.insert(unit, (px, uses));
            }
        }
        drop(asked_span);
        let stats = format!(
            "{w}x{h} units={units} reused={reused} made={made_count} rough={rough} encoding={encoding:.1?} scene={scene_mb:.1}MB paths={} clips={} kinds={:?} sizes={:?} times={:?} blurs={blurs} skia_gpu={skia_gpu:?} inexact={inexact} pages={used_pages} fell_back={}k px encoded={:.1}ms (stamps {:.1} put {:.1} append {:.1}) skia={:.1}ms vello_cpu={:.1}ms (blurs {:.1}ms) vello={:.1}ms steps={steps:?} atlas={}k {}",
            paths,
            clips,
            super::vello::KINDS.iter().map(|n| n.swap(0, std::sync::atomic::Ordering::Relaxed)).collect::<Vec<_>>(),
            {
                let mut layers: Vec<(u32, u64)> = super::vello::LAYERS.lock().unwrap().take().unwrap_or_default().into_iter().collect();
                layers.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
                (super::vello::SIZES.iter().map(|n| n.swap(0, std::sync::atomic::Ordering::Relaxed)).collect::<Vec<_>>(), layers)
            },
            super::vello::TIMES.iter().map(|n| (n.swap(0, std::sync::atomic::Ordering::Relaxed) / 100_000) as f64 / 10.0).collect::<Vec<_>>(),
            fell_back / 1000,
            encoded.as_secs_f64() * 1000.0,
            stamped.as_secs_f64() * 1000.0,
            put_took.as_secs_f64() * 1000.0,
            append_took.as_secs_f64() * 1000.0,
            skia_took.as_secs_f64() * 1000.0,
            self.vello.as_ref().map_or(0.0, |vello| vello.took.as_secs_f64() * 1000.0),
            self.vello.as_ref().map_or(0.0, |vello| vello.took_blurs.as_secs_f64() * 1000.0),
            vello_took.as_secs_f64() * 1000.0,
            area >> 10,
            kinds.replace('"', "")
        );
        Ok((tex, counts, stats, rough))
    }

    fn draw_whole(&mut self, ox: i32, oy: i32, width: i32, height: i32, px: f32, quick: bool) {
        let _span = super::span::span("draw whole");
        if quick && self.tree.is_some() && self.vello_ready() && self.draw_whole_vello(ox, oy, width, height, px) {
            return;
        }
        // What Vello drew before is shown before what Skia draws now
        self.settle_unchecked();
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
        self.hand_whole(Layer { tex, x: ox - margin, y: oy - margin, phase: (0.0, 0.0), px, counts, kind: LayerKind::Whole, drawn: 0 }, fence, at, false, false, self.seq);
        self.whole_took = started.elapsed();
        stats().lock().unwrap().whole_ms = self.whole_took.as_secs_f64() * 1000.0;
        self.drew = true;
    }

    /// The whole view's last drawing still on the GPU
    /// Whether a view was too big for Vello's buffers a moment ago
    fn too_big(&self) -> bool {
        self.vello_too_big.is_some_and(|at| at.elapsed() < TOO_BIG_FOR)
    }

    fn whole_busy(&self) -> bool {
        self.whole.as_ref().is_some_and(|whole| whole.fence > self.gpu.passed())
    }

    /// Makes `layer`, which the GPU draws till `fence` passes, the whole
    /// view of `at`
    fn hand_whole(&mut self, mut layer: Layer, fence: u64, at: (i32, i32, u32), rough: bool, vello: bool, seq: u64) {
        let _span = super::span::span("hand whole");
        self.serial += 1;
        let serial = self.serial;
        layer.drawn = serial;
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
                (whole.rough, whole.vello, whole.seq, whole.patched) = (rough, vello, seq, false);
                Some(std::mem::replace(&mut whole.layer, layer))
            }
            None => {
                self.whole = Some(Whole { layer, at, stale: false, patch: Vec::new(), fence, serial, rough, vello, seq, patched: false });
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

    /// Tiles of the whole view's zoom under it, compared with it: drawn
    /// afresh at rest, it's what they should show. One that came out
    /// otherwise, as where its images went missing unnoticed, would show
    /// that each time the view moves, so is drawn again, and traced with
    /// how it was drawn
    fn verify_tiles(&mut self) {
        let _span = super::span::span("verify tiles");
        // Compared at half size, each pixel the average of 4, block by
        // block, so the fraction of a pixel the whole view's drawn at
        // makes no difference
        const BLOCK: usize = 16;
        const DIFFERENT: u32 = 24;
        let Some(whole) = self.whole.as_ref().filter(|whole| !whole.stale && whole.patch.is_empty() && !whole.rough) else {
            return;
        };
        let (tex, wx, wy, pxb) = (whole.layer.tex.clone(), whole.layer.x, whole.layer.y, whole.layer.px.to_bits());
        let (w, h) = (tex.width / 2, tex.height / 2);
        let under = |key: &TileKey| key.x * TILE < wx + tex.width && (key.x + 1) * TILE > wx && key.y * TILE < wy + tex.height && (key.y + 1) * TILE > wy;
        let keys: Vec<TileKey> = self.tiles.iter().filter(|(key, tile)| key.px == pxb && !tile.stale && !tile.draft && under(key)).map(|(key, _)| *key).collect();
        if keys.is_empty() || w < BLOCK as i32 || h < BLOCK as i32 {
            return;
        }
        let started = Instant::now();
        let Some(whole_image) = gpu::images::borrow_texture_from(&mut self.gpu.context, &tex.backend(), SurfaceOrigin::TopLeft, ColorType::RGBA8888, AlphaType::Premul, None) else {
            return;
        };
        let Some(mut surface) = self.gpu.own_surface(w, h, 1) else {
            return;
        };
        let info = ImageInfo::new((w, h), ColorType::RGBA8888, AlphaType::Premul, None);
        let sampling = SamplingOptions::new(FilterMode::Linear, MipmapMode::None);
        let mut paint = Paint::default();
        paint.set_blend_mode(skia_safe::BlendMode::Src);
        let read = |surface: &mut Surface| {
            let mut pixels = vec![0u8; (w * h * 4) as usize];
            surface.read_pixels(&info, &mut pixels, (w * 4) as usize, (0, 0)).then_some(pixels)
        };
        surface.canvas().clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
        surface.canvas().draw_image_rect_with_sampling_options(&whole_image, None, Rect::from_wh(w as f32, h as f32), sampling, &paint);
        let Some(expected) = read(&mut surface) else { return };
        surface.canvas().clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
        for key in &keys {
            let at = Rect::from_xywh((key.x * TILE - wx) as f32 / 2.0, (key.y * TILE - wy) as f32 / 2.0, (TILE / 2) as f32, (TILE / 2) as f32);
            surface.canvas().draw_image_rect_with_sampling_options(&self.tiles[key].image, None, at, sampling, &paint);
        }
        let Some(drawn) = read(&mut surface) else { return };
        let (w, h) = (w as usize, h as usize);
        let mut redraw = Vec::new();
        for key in keys {
            // Blocks wholly inside both the tile and the view
            let (left, top) = ((key.x * TILE - wx) / 2, (key.y * TILE - wy) / 2);
            let (x0, y0) = (left.max(0) as usize, top.max(0) as usize);
            let (x1, y1) = (((left + TILE / 2) as usize).min(w), ((top + TILE / 2) as usize).min(h));
            let mut worst = (0, 0, 0);
            for by in (y0..y1.saturating_sub(BLOCK - 1)).step_by(BLOCK) {
                for bx in (x0..x1.saturating_sub(BLOCK - 1)).step_by(BLOCK) {
                    let mut sum = 0u32;
                    for y in by..by + BLOCK {
                        let row = (y * w + bx) * 4;
                        for i in row..row + BLOCK * 4 {
                            sum += expected[i].abs_diff(drawn[i]) as u32;
                        }
                    }
                    let mean = sum / (BLOCK * BLOCK * 4) as u32;
                    if mean > worst.0 {
                        worst = (mean, bx * 2, by * 2);
                    }
                }
            }
            if worst.0 > DIFFERENT {
                redraw.push((key, worst));
            }
        }
        for (key, (mean, x, y)) in redraw {
            let times = self.mismatched.entry(key).or_default();
            *times += 1;
            let times = *times;
            let Some(tile) = self.tiles.get_mut(&key) else { continue };
            trace(|| {
                format!(
                    "  tile {},{} px={:.4} differs from the whole view by {mean} at {x},{y} of it, drawn by {} exact={}, {}",
                    key.x,
                    key.y,
                    key.px(),
                    if tile.vello { "Vello" } else { "Skia" },
                    tile.exact,
                    if times > 2 { "left as it is, as drawing it again didn't help" } else { "drawn again" }
                )
            });
            // Drawn again twice and still otherwise, it's how the two draw
            // differently, not what was missing, so is left
            if times <= 2 {
                tile.stale = true;
                self.dirty = true;
            }
        }
        trace(|| format!("  verified tiles {:.1}ms", started.elapsed().as_secs_f64() * 1000.0));
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
        let _span = super::span::span("patch whole");
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
            let level = self.level(&self.units[unit], px, self.units[unit].custom);
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
            whole.layer.drawn = serial;
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

    fn drop_strata(&mut self) {
        if self.strata.take().is_some() {
            if let Some(vello) = self.vello.as_mut() {
                vello.drop_under();
            }
        }
    }

    /// `areas`, in units, as rects of the whole view at (`x`, `y`) and `px`,
    /// `width` by `height`, snapped to `PATCH_GRID` and those mostly over
    /// each other made one
    fn patch_rects(areas: &[Rect], (x, y, px): (i32, i32, f32), width: i32, height: i32) -> Vec<skia_safe::IRect> {
        let snap = |at: i32, down: bool| if down { at.div_euclid(PATCH_GRID) * PATCH_GRID } else { (at + PATCH_GRID - 1).div_euclid(PATCH_GRID) * PATCH_GRID };
        let mut rects: Vec<skia_safe::IRect> = Vec::new();
        for area in areas {
            let device: skia_safe::IRect = Rect::from_ltrb(area.left * px - x as f32, area.top * px - y as f32, area.right * px - x as f32, area.bottom * px - y as f32).round_out();
            let device = device.with_outset((3, 3));
            let snapped = skia_safe::IRect::from_ltrb(snap(device.left, true), snap(device.top, true), snap(device.right, false), snap(device.bottom, false));
            if let Some(rect) = skia_safe::IRect::intersect(&snapped, &skia_safe::IRect::from_wh(width, height)) {
                rects.push(rect);
            }
        }
        let size = |rect: &skia_safe::IRect| rect.width() as i64 * rect.height() as i64;
        'merge: loop {
            for i in 0..rects.len() {
                for j in i + 1..rects.len() {
                    let (a, b) = (rects[i], rects[j]);
                    let both = skia_safe::IRect::from_ltrb(a.left.min(b.left), a.top.min(b.top), a.right.max(b.right), a.bottom.max(b.bottom));
                    if size(&both) * 4 <= (size(&a) + size(&b)) * 5 {
                        rects[i] = both;
                        rects.swap_remove(j);
                        continue 'merge;
                    }
                }
            }
            break;
        }
        rects
    }

    /// The units an edit changes and those over them drawn again where they
    /// change in the whole view, over what's under them, kept from when
    /// they started changing (see `Strata`). False where it can't, or,
    /// unless `build`, where what's under them would be drawn first
    fn fill_strata(&mut self, px: f32, (ox, oy, width, height): (i32, i32, i32, i32), phase: (f32, f32), build: bool) -> bool {
        if self.changing.is_empty() || self.tree.is_none() || (!build && self.strata.is_none()) || self.too_big() {
            return false;
        }
        let at = (ox, oy, px.to_bits());
        let fits = self.whole.as_ref().is_some_and(|whole| {
            whole.vello && !whole.stale && !whole.rough && whole.at == at && whole.layer.phase == phase && whole.layer.tex.width == width && whole.layer.tex.height == height
        });
        if !fits {
            return false;
        }
        let _span = super::span::span("fill strata");
        let started = Instant::now();
        let reach = Rect::from_xywh(ox as f32 / px, oy as f32 / px, width as f32 / px, height as f32 / px);
        let shown = |unit: &Unit| unit.place.is_some() && Rect::intersects2(&unit.bounds, &reach);
        let mut range: Option<(usize, usize)> = None;
        for (index, unit) in self.units.iter().enumerate() {
            if shown(unit) && self.changing.contains(&unit.slot) {
                range = Some(range.map_or((index, index), |(first, _)| (first, index)));
            }
        }
        let Some((first, last)) = range else { return false };
        let (under, sharper) = {
            let (mut hasher, mut sharper) = (std::collections::hash_map::DefaultHasher::new(), std::collections::hash_map::DefaultHasher::new());
            let mut any = false;
            for unit in self.units[..first].iter().filter(|unit| shown(unit)) {
                any = true;
                self.mark(unit).hash(&mut hasher);
                self.sharpened.get(&(unit.slot, unit.section)).hash(&mut sharper);
            }
            (any.then(|| hasher.finish()), sharper.finish())
        };
        let kept = self.strata.as_ref().is_some_and(|strata| {
            strata.at == at && strata.phase == phase && strata.under == under && (strata.sharper == sharper || strata.built.elapsed() < STRATA_SHARPER)
        });
        if !kept && !build {
            return false;
        }
        self.settle_shown();
        let mut built = Duration::ZERO;
        if !kept {
            self.drop_strata();
            if under.is_some() {
                let timer = Instant::now();
                self.only = Some((0, first - 1));
                let mut done = false;
                for _ in 0..8 {
                    self.blur_view = Some(vello::kurbo::Rect::new(0.0, 0.0, width as f64, height as f64));
                    let rendered = self.vello_render(ox, oy, phase, width, height, px, reach.with_outset((2.0 / px, 2.0 / px)), None, false, None);
                    self.blur_view = None;
                    if rendered.is_err() {
                        break;
                    }
                    let Some(vello) = self.vello.as_mut() else { break };
                    match vello.check(usize::MAX) {
                        Ok(None) => {
                            done = vello.keep_under(&self.gpu.device, width, height).map_err(|why| trace(|| format!("  strata can't keep what's under: {why}"))).is_ok();
                            break;
                        }
                        Ok(Some(why)) if !why.starts_with("too big") => trace(|| format!("  vello ran out of room, drawing again: {why}")),
                        _ => break,
                    }
                }
                self.only = None;
                if !done {
                    return false;
                }
                built = timer.elapsed();
            }
            self.strata = Some(Strata { at, phase, under, sharper, built: Instant::now() });
        }
        // Where they were and are
        let mut areas = self.whole.as_ref().map_or_else(Vec::new, |whole| whole.patch.clone());
        areas.extend(self.units[first..=last].iter().filter(|unit| shown(unit)).map(|unit| unit.bounds));
        let rects = Self::patch_rects(&areas, (ox, oy, px), width, height);
        let over = (first, under.is_some());
        let done = self.patch_whole_rects(&rects, Some(over), started);
        trace(|| format!("  strata units={first}..={last} of {} built={:.1}ms {}", self.units.len(), built.as_secs_f64() * 1000.0, if done { "drawn" } else { "failed" }));
        done
    }

    /// What changed in the whole view Vello drew, drawn again by Vello alone
    /// over a copy of it, rather than all of it again, so an edit shows in
    /// a few milliseconds. False where it's better drawn again whole
    fn patch_whole_vello(&mut self) -> bool {
        let started = Instant::now();
        self.settle_shown();
        let Some(whole) = self.whole.as_ref().filter(|whole| whole.vello && !whole.stale && !whole.rough && !whole.patch.is_empty() && whole.patch.len() <= PATCHES) else {
            return false;
        };
        let (width, height) = (whole.layer.tex.width, whole.layer.tex.height);
        let rects = Self::patch_rects(&whole.patch, (whole.layer.x, whole.layer.y, whole.layer.px), width, height);
        // Past half of it, drawing it all again takes about as long
        if rects.iter().map(|rect| rect.width() as i64 * rect.height() as i64).sum::<i64>() * 2 > width as i64 * height as i64 {
            return false;
        }
        self.patch_whole_rects(&rects, None, started)
    }

    /// `rects` of the whole view drawn again by Vello, over a copy of it or
    /// in place. With `over`, only the units from the first, and over what
    /// `Strata` kept under them where they have it
    fn patch_whole_rects(&mut self, rects: &[skia_safe::IRect], over: Option<(usize, bool)>, started: Instant) -> bool {
        let _span = super::span::span("patch whole vello");
        let _purpose = super::gpu_census::purpose("whole");
        let Some(whole) = self.whole.as_ref() else { return false };
        let (tex, x, y, px, phase) = (whole.layer.tex.clone(), whole.layer.x, whole.layer.y, whole.layer.px, whole.layer.phase);
        let (width, height) = (tex.width, tex.height);
        let in_place = Arc::strong_count(&tex) == 2;
        let Some(target) = (if in_place { Some(tex.clone()) } else { self.tex(width, height) }) else {
            return false;
        };
        let previous = if in_place {
            None
        } else {
            match gpu::images::borrow_texture_from(&mut self.gpu.context, &tex.backend(), SurfaceOrigin::TopLeft, ColorType::RGBA8888, AlphaType::Premul, None) {
                Some(previous) => Some(previous),
                None => return false,
            }
        };
        let Some(mut surface) = self.target(&target) else {
            return false;
        };
        let backend = target.backend();
        let mut copy = Paint::default();
        copy.set_blend_mode(skia_safe::BlendMode::Src);
        if let Some(previous) = &previous {
            surface.canvas().draw_image(previous, (0, 0), Some(&copy));
        }
        let (mut waited, mut stats) = (Duration::ZERO, String::new());
        for rect in rects {
            let reach = Rect::from_xywh((x + rect.left) as f32 / px, (y + rect.top) as f32 / px, rect.width() as f32 / px, rect.height() as f32 / px).with_outset((2.0 / px, 2.0 / px));
            if let Some((first, under)) = over {
                self.only = Some((first, self.units.len().saturating_sub(1)));
                self.under_at = under.then_some((rect.left as u32, rect.top as u32));
            }
            let mut drawn = None;
            for _ in 0..8 {
                // Blurs are drawn from all of the whole view, the same for
                // each patch, so drawn once
                self.blur_view = Some(vello::kurbo::Rect::new(-rect.left as f64, -rect.top as f64, (width - rect.left) as f64, (height - rect.top) as f64));
                let rendered = self.vello_render(x + rect.left, y + rect.top, phase, rect.width(), rect.height(), px, reach, None, false, None);
                self.blur_view = None;
                let Ok((out, _, made, _)) = rendered else {
                    break;
                };
                let Some(vello) = self.vello.as_mut() else { break };
                let checked = vello.check(usize::MAX);
                waited += vello.waited;
                let stages = std::mem::take(&mut vello.stages);
                match checked {
                    Ok(None) => {
                        stats = format!("{made} gpu:{stages}");
                        drawn = Some(out);
                        break;
                    }
                    Ok(Some(why)) if !why.starts_with("too big") => trace(|| format!("  vello ran out of room, drawing again: {why}")),
                    _ => break,
                }
            }
            self.only = None;
            self.under_at = None;
            let image = drawn.and_then(|out| gpu::images::borrow_texture_from(&mut self.gpu.context, &out.backend(), SurfaceOrigin::TopLeft, ColorType::RGBA8888, AlphaType::Unpremul, None));
            let Some(image) = image else {
                // What's drawn into it so far is never shown
                drop(surface);
                let _ = self.gpu.context.flush_and_submit();
                if !in_place {
                    self.retire(target);
                } else if let Some(whole) = self.whole.as_mut() {
                    whole.stale = true;
                }
                return false;
            };
            let from = Rect::from_wh(rect.width() as f32, rect.height() as f32);
            surface.canvas().draw_image_rect(&image, Some((&from, skia_safe::canvas::SrcRectConstraint::Fast)), Rect::from(*rect), &copy);
            // Drawn from Vello's target before its next render draws into it
            let _ = self.gpu.context.flush_and_submit();
        }
        let image = self.finish(&backend, false);
        drop(surface);
        drop(previous);
        let fence = self.gpu.signal();
        self.serial += 1;
        let (serial, seq) = (self.serial, self.seq);
        let mut replaced = None;
        if let Some(whole) = self.whole.as_mut() {
            whole.fence = fence;
            whole.serial = serial;
            whole.layer.drawn = serial;
            whole.stale |= image.is_none();
            whole.patch.clear();
            whole.seq = seq;
            // Drawn again whole at rest, as where parts meet each is drawn
            // over the other, not as one
            whole.patched = true;
            if !in_place {
                replaced = Some(std::mem::replace(&mut whole.layer.tex, target));
            }
        }
        if let Some(replaced) = replaced {
            self.retire(replaced);
        }
        trace(|| {
            let rects: Vec<String> = rects.iter().map(|rect| format!("{}x{}", rect.width(), rect.height())).collect();
            format!("  patch vello rects={rects:?} over={over:?} waited={:.1}ms {:.1}ms {stats}", waited.as_secs_f64() * 1000.0, started.elapsed().as_secs_f64() * 1000.0)
        });
        self.drew = true;
        true
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
        if (width, height) == (TILE, TILE) {
            if self.maker.is_none() {
                self.maker = Maker::new(&self.gpu.device);
            }
            if let Some(tex) = self.maker.as_ref().and_then(Maker::take) {
                return Some(tex);
            }
        }
        Tex::new(&self.gpu.device, width, height)
    }

    /// Dev tools: `tex`'s pixels
    fn read_tex(&mut self, tex: &Tex) -> Option<Vec<u8>> {
        let _span = super::span::span("read tex");
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
        let _span = super::span::span("verify tile");
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

    fn put_tile(&mut self, key: TileKey, mut tile: Tile) {
        self.serial += 1;
        tile.drawn = self.serial;
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

    /// Lets go of what's drawn of the tab drawn before, for another's
    fn leave_tab(&mut self) {
        self.tree = None;
        self.whole_sum = None;
        self.clear_tiles();
        if self.last_whole.is_some() {
            self.drop_last_whole();
        }
        if let Some(last) = self.unchecked.take() {
            if let Some(vello) = self.vello.as_mut() {
                let _ = vello.check(last.renders);
            }
            self.retire(last.layer.tex);
        }
        if let Some(chunked) = self.chunked.take() {
            self.retire(chunked.tex);
        }
        self.scene_kept = None;
        self.kept_drawn = None;
        self.fragments.clear();
        self.between.clear();
        self.sharpened.clear();
        self.versions.clear();
        self.image_units.clear();
        self.vello_uses.clear();
    }

    fn clear_tiles(&mut self) {
        let _span = super::span::span("clear tiles");
        self.pending = None;
        if let Some(drawn) = self.drawn_tiles.take() {
            if let Some(vello) = self.vello.as_mut() {
                let _ = vello.check(drawn.renders);
            }
            for (_, tex, ..) in drawn.tiles {
                self.retire(tex);
            }
        }
        let tiles: Vec<Tile> = self.tiles.drain().map(|(_, tile)| tile).collect();
        for tile in tiles {
            self.drop_tile(tile);
        }
        if let Some(whole) = self.whole.take() {
            self.retire_whole(whole);
        }
        self.drop_strata();
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
        self.whole.as_ref().is_some_and(|whole| whole.at == at && !whole.stale && whole.patch.is_empty() && !whole.rough)
    }

    /// What's drawn and up to date: tiles, and the whole view
    /// What's drawn, with tiles being drawn again as they were if `stale`
    fn layers(&self, stale: bool) -> Vec<Layer> {
        // A tile out of date since before the whole view was drawn is older
        // than it wherever they meet. Preferred anywhere it would show the
        // view as it was seconds ago, so it's under all else, there only for
        // what a pan uncovers that nothing else has, as the furthest level
        // is, which waits out a file opening and the view moving before
        // it's drawn again
        let whole_area = self.whole.as_ref().map(|whole| {
            let layer = &whole.layer;
            (Rect::from_xywh(layer.x as f32 / layer.px, layer.y as f32 / layer.px, layer.tex.width as f32 / layer.px, layer.tex.height as f32 / layer.px), layer.drawn, layer.px, whole.rough)
        });
        let older = |key: &TileKey, tile: &Tile| whole_area.is_some_and(|(area, drawn, _, _)| tile.drawn < drawn && Rect::intersects2(&area, &key.area()));
        // One drawn before it at its zoom and wholly inside it is no newer
        // where it's up to date, and may not be where it's not marked so,
        // as for what effects drew while it was drawn
        let under = |key: &TileKey, tile: &Tile| {
            whole_area.is_some_and(|(area, drawn, px, rough)| {
                let tile_area = key.area();
                !rough && tile.drawn < drawn && key.px() == px && area.left <= tile_area.left && area.top <= tile_area.top && area.right >= tile_area.right && area.bottom >= tile_area.bottom
            })
        };
        let tiles = self.tiles.iter().filter(|(key, tile)| !under(key, tile) && (!tile.stale || stale)).map(|(key, tile)| Layer {
            tex: tile.tex.clone(),
            x: key.x * TILE,
            y: key.y * TILE,
            phase: (0.0, 0.0),
            px: key.px(),
            counts: tile.counts,
            drawn: tile.drawn,
            kind: if tile.stale && tile.seen && !older(key, tile) {
                LayerKind::Stale
            } else if tile.stale {
                LayerKind::Old
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
        let _span = super::span::span("publish");
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
            (Arc::as_ptr(&layer.tex) as usize, layer.x, layer.y, layer.px.to_bits(), layer.kind, layer.drawn).hash(&mut hasher);
        }
        self.whole.as_ref().map(|whole| whole.serial).hash(&mut hasher);
        (whole_at, background.as_array().map(f32::to_bits), transparent, dark).hash(&mut hasher);
        let signature = hasher.finish();
        // Where what changed is drawn only as it was before, or not yet,
        // the frames that changed it wait, overlay and all: they're drawn
        // as of the frame the whole view's drawn as of, where it covers the
        // view, and of the latest where everything over it is up to date
        let before = if self.changes_drawn > self.seq { 0 } else { self.changes_drawn };
        let changes = if self.edited_seq <= before {
            // No change since that's waiting
            self.seq
        } else {
            let covers = |layers: &[Layer]| self.view.as_ref().is_none_or(|view| !super::present::cover(layers, self.content(), view, self.gpu.size).1);
            let mut fresh = self.layers(false);
            if self.last_whole.is_some() {
                fresh.pop();
            }
            match covers(&fresh) {
                true => self.seq,
                false => before.max(self.whole.as_ref().filter(|whole| !whole.rough && covers(std::slice::from_ref(&whole.layer))).map_or(0, |whole| whole.seq)),
            }
        };
        if self.published.is_some_and(|(last, seq, _)| last == signature && seq == self.seq) && changes == self.changes_drawn {
            return;
        }
        self.changes_drawn = changes;
        // What it has is drawn once the GPU passes this
        let value = match self.published {
            Some((last, _, value)) if last == signature => value,
            _ => self.gpu.signal(),
        };
        // Only what's drawn tells snapshots apart, so one that's the same
        // as of a later frame isn't shown again
        if self.published.is_none_or(|(last, _, _)| last != signature) {
            self.serial += 1;
        }
        self.published = Some((signature, self.seq, value));
        let snapshot = Snapshot {
            seq: self.seq,
            changes,
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
            tab: self.tab.clone(),
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
            Msg::Unpark(tab) => {
                trace(|| format!("{tab} unparked"));
                self.parked.take_if(|parked| parked.tab == tab);
                if self.tab.as_ref() == Some(&tab) {
                    self.park = false;
                }
            }
            Msg::Octaves(octaves) => {
                self.octaves = octaves.clone();
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
                self.tab.clone_from(&self.scene.tab);
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
                self.kept.lock().unwrap().stale.remove(&key);
                self.effects_done(&key);
                let wait = self.failed.get(&key).map_or(Duration::from_secs(1), |(_, wait)| (*wait * 2).min(FAILED_MOST));
                self.failed.insert(key, (Instant::now(), wait));
            }
            Msg::Skipped(key) => {
                self.asked.remove(&key);
                self.effects_done(&key);
            }
            Msg::Image(key, img, width) => {
                self.effects_done(&key);
                // Fetches run side by side, so one asked for before can come
                // after: it's of a view gone by, and what's asked for since
                // is on its way. It's still shown till then, unless what's
                // here is as sharp
                self.kept.lock().unwrap().stale.remove(&key);
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
        let _span = super::span::span("set view");
        if self.view.as_ref() == Some(&view) {
            return;
        }
        if self.view.as_ref().is_none_or(|was| (was.cw, was.ch, was.dpr) != (view.cw, view.ch, view.dpr)) {
            trace(|| format!("view's canvas {}x{} at {},{} dpr {} zoom {} shows {:.0}x{:.0}", view.cw, view.ch, view.cx, view.cy, view.dpr, view.zoom, view.w, view.h));
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
        self.requests.set_view(Rect::from_xywh(view.x as f32, view.y as f32, view.w as f32, view.h as f32));
        self.view = Some(view);
        self.moved = now;
        self.dirty = true;
    }

    /// Image `key` came: what waited for it is drawn again
    fn image_came(&mut self, key: String, img: Img) {
        self.take_image(key, img, true);
    }

    /// Image `key` in place of what's kept of it, `upload`ed first if it
    /// came from the CPU
    fn take_image(&mut self, key: String, img: Img, upload: bool) {
        let _span = super::span::span("image came");
        // Sizes asked for one after another come in any order: only the
        // last asked for is kept, or one asked for before could follow it
        if !key.starts_with(RENDER) && !key.starts_with(EFFECT) {
            if let Some(&width) = self.asked.get(&key) {
                let side = img.image.width().max(img.image.height()) as u32;
                if side != super::run::fitted(img.original, width) && self.images.contains_key(&key) {
                    trace(|| format!("image {key} {side} came after a size asked for since"));
                    return;
                }
            }
        }
        self.image_came_at = Instant::now();
        if self.images.get(&key).is_some_and(|kept| kept.image.width() < img.image.width()) {
            self.grown.insert(key.clone(), Instant::now());
        }
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
                self.touched.push(unit);
                self.revision += 1;
            }
        } else {
            self.bump(drew);
        }
        let pictures_span = super::span::span("pictures with it");
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
        drop(pictures_span);
        let mut img = img;
        if upload {
            let _span = super::span::span("upload image");
            img.image = self::upload(&mut self.gpu.context, img.image);
            // Its copy on the way up is let go once the GPU has it, not
            // with those of every other image that came in meanwhile
            if img.bytes() > 1 << 20 {
                let _ = self.gpu.context.flush_and_submit();
            }
        }
        let vram = if tracing() { self.gpu.used() >> 20 } else { 0 };
        trace(|| format!("image {key} {}x{} areas={} vram={vram}", img.image.width(), img.image.height(), areas.len()));
        // What a node's effects drew stands in for them once Figma sends it
        // again as another, which it can before they come
        if let Some(stand_in) = self.effect_nodes.get(&key) {
            self.images.insert(stand_in.clone(), Img { stale: true, ..img.clone() });
        }
        let live = tree_effect_of(&key).is_some_and(|(id, content)| self.tree.as_ref().is_some_and(|tree| tree.node(id).is_some() && tree.content(id) == content));
        if let Some(last) = if live { self.effect_last.get(&key).cloned() } else { self.effect_last.remove(&key) } {
            self.images.insert(last, Img { stale: true, ..img.clone() });
        }
        self.images.insert(key.clone(), img);
        {
            let _span = super::span::span("drop tiles");
            self.drop_tiles(&areas);
        }
        self.wake_effects(&key);
        self.fresh = true;
        self.update_memory();
        self.dirty = true;
    }

    /// The run of effects `key` done, and the node's run since asked for
    /// asked for again
    fn effects_done(&mut self, key: &str) {
        let _span = super::span::span("effects done");
        if !key.starts_with(EFFECT) || self.effects_running.is_empty() {
            return;
        }
        let Some(node) = self.effects_running.iter().find(|(_, (running, _))| running == key).map(|(node, _)| node.clone()) else {
            return;
        };
        self.effects_running.remove(&node);
        if let Some((again, width)) = self.effects_again.remove(&node) {
            self.asked.remove(&again);
            self.ask(&[(again, width)], None);
        }
    }

    /// Runs again the effects that waited on image `key`, which may itself
    /// be another node's effects
    fn wake_effects(&mut self, key: &str) {
        let _span = super::span::span("wake effects");
        if self.effects_waiting.is_empty() {
            return;
        }
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.effects_waiting).into_iter().partition(|(_, _, on)| on.iter().any(|image| image == key));
        self.effects_waiting = waiting;
        for (effect, width, _) in ready {
            // Run again on what came, not only resized as it was
            self.kept.lock().unwrap().retain(|kept| kept != effect);
            self.asked.remove(&effect);
            self.ask(&[(effect, width)], None);
        }
    }

    /// A frame of Figma's render tree: the units it changed are drawn
    /// again, the rest kept, as a node that changes is a new node. The view
    /// comes with it, as Figma drew it
    fn frame(&mut self, frame: Frame) {
        let _span = super::span::span("apply frame");
        if self.tab.as_deref() != Some(frame.tab.as_str()) {
            self.settle_units();
        }
        // Another tab's render tree: nothing drawn of the one before is kept,
        // as its units' slots would be taken for others, but its tree is
        // while it's parked (see `run::Ctx::parked`), to carry on from as
        // it's drawn again. What's shown of it stays till this one's drawn,
        // hidden while this tab is shown
        let mut restored = false;
        if self.tab.as_deref() != Some(frame.tab.as_str()) {
            let back = self.parked.take_if(|parked| *parked.tab == *frame.tab);
            if let Some(tree) = self.tree.take() {
                if let Some(tab) = self.tab.clone().filter(|_| self.park) {
                    trace(|| format!("{tab}'s tree kept"));
                    self.parked = Some(Parked { tab, tree, whole_sum: self.whole_sum });
                }
                self.leave_tab();
            }
            self.park = true;
            self.tab = Some(frame.tab.as_str().into());
            if let Some(back) = back {
                trace(|| format!("frame {} carries on from {}'s kept tree", frame.header.seq, back.tab));
                self.tree = Some(back.tree);
                self.whole_sum = back.whole_sum;
                restored = true;
            }
        }
        // A whole scene sent again as it was, as Figma does for a new worker
        // of its own, isn't taken in again, a third of a second of a big one
        let whole_sum = (frame.header.mode == 0).then(|| forest_sum(frame.forest(), frame.header.root));
        let again = whole_sum.is_some() && whole_sum == self.whole_sum && self.tree.is_some();
        if again && !restored {
            trace(|| format!("frame {} mode=0 the same as the last whole scene", frame.header.seq));
            self.seq = frame.header.seq;
            self.set_view(frame.view());
            self.dirty = true;
            return;
        }
        let first = restored || self.tree.is_none();
        let tree = self.tree.get_or_insert_with(|| Tree::new(super::rt::Schema::bundled()));
        if tree.octaves.is_empty() {
            if self.octaves.len() >= 36 {
                tree.octaves = self.octaves.clone();
            } else if self.scene.noise_octaves.len() >= 36 {
                tree.octaves = self.scene.noise_octaves.clone();
            }
        }
        let start = Instant::now();
        let root = tree.root;
        let applied = match if again { Ok(Default::default()) } else { tree.apply(&frame) } {
            Ok(applied) => applied,
            Err(error) => {
                log::warn!("native renderer: frame {} not applied: {error}", frame.header.seq);
                self.hub.tree_failures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return;
            }
        };
        self.hub.tree_failures.store(0, std::sync::atomic::Ordering::Relaxed);
        self.whole_sum = whole_sum.or(self.whole_sum.filter(|_| !frame.edits()));
        self.seq = frame.header.seq;
        if frame.edits() {
            self.edited_seq = self.seq;
        }
        let applied_ms = start.elapsed().as_secs_f64() * 1000.0;
        // Nothing of the tree changed, as for a frame that only moves
        // Figma's overlay, so its units are as they were
        let same = !first && frame.header.mode != 0 && applied.new_nodes.is_empty() && frame.header.released.is_empty() && tree.root == root;
        // What custom effects of nodes gone drew goes with them. That of
        // nodes that changed, as they do over and over as a file loads, is
        // kept for what they are now, shown till their effects have run
        // again, rather than the node without them meanwhile
        let gone = |key: &str| tree_effect_of(key).is_some_and(|(id, content)| tree.node(id).is_none() || tree.content(id) != content);
        let changed: Vec<(String, Option<String>)> = self
            .images
            .keys()
            .filter_map(|key| {
                let (id, content) = tree_effect_of(key)?;
                if tree.node(id).is_none() {
                    return Some((key.clone(), None));
                }
                let now = tree.content(id);
                if now == content {
                    return None;
                }
                // Carried to what's under its id now only if that's the same
                // node, as Figma gives an id to another node at times
                let same = tree.custom_effect(id).is_some_and(|effect| self.effect_nodes.get(key).is_none_or(|was| *was == effect.stand_in));
                Some((key.clone(), same.then(|| format!("{EFFECT}t{id}.{now}"))))
            })
            .collect();
        // Nodes Figma sends again as others, as it does as a file loads,
        // show what their effects drew before till they've run again too
        for (key, now) in changed {
            let Some(mut img) = self.images.remove(&key) else { continue };
            if self.asked.remove(&key).is_some() {
                self.kept.lock().unwrap().stale.insert(key.clone());
            }
            img.stale = true;
            let node = self.effect_nodes.remove(&key);
            let last = self.effect_last.remove(&key);
            let images = self.effect_images.remove(&key);
            match (now, node) {
                (Some(now), node) => {
                    if let Some(node) = node {
                        self.effect_nodes.insert(now.clone(), node);
                    }
                    if let Some(last) = last {
                        self.effect_last.insert(now.clone(), last);
                    }
                    if let Some(images) = images {
                        self.effect_images.insert(now.clone(), images);
                    }
                    self.images.entry(now).or_insert(img);
                }
                (None, Some(node)) => {
                    self.images.insert(node, img);
                }
                (None, None) => {}
            }
        }
        {
            let mut kept = self.kept.lock().unwrap();
            kept.retain(|key| !gone(key));
            let asked: Vec<String> = self.asked.keys().filter(|key| gone(key)).cloned().collect();
            for key in asked {
                self.asked.remove(&key);
                kept.stale.insert(key);
            }
        }
        self.effect_images.retain(|key, _| !gone(key));
        // Those of runs whose node has gone are kept a while for what they
        // draw, if they come
        if self.effect_last.len() > 512 {
            self.effect_last.retain(|key, _| !gone(key));
        }
        if same {
            trace(|| format!("frame {} mode={} new=0 applied={applied_ms:.1}ms units={} areas=0", frame.header.seq, frame.header.mode, self.units.len()));
            self.set_view(frame.view());
            self.dirty = true;
            return;
        }
        if first {
            self.units_due = None;
            self.rebuild_units();
            self.waiting_areas.clear();
            self.pictures.clear();
            self.thumbs.clear();
            self.clear_rasters();
            self.clear_tiles();
            self.kit.forget_patterns(None);
        } else {
            trace(|| format!("frame {} mode={} new={} applied={applied_ms:.1}ms", frame.header.seq, frame.header.mode, applied.new_nodes.len()));
            let (count, at) = self.units_due.map_or((0, frame.header.at), |(count, _, at)| (count, at));
            self.units_due = Some((count + 1, frame.header.seq, at));
        }
        stats().lock().unwrap().nodes = self.tree.as_ref().map_or(0, |tree| tree.forest.nodes.len());
        self.set_view(frame.view());
        self.dirty = true;
    }

    /// The units of the frames taken in since they were last made, made
    /// again once for all of them, with what changed. Made for each, as it
    /// takes milliseconds in a big file, frames came faster than they were
    /// taken in while something moved, and fell further and further behind
    fn settle_units(&mut self) {
        let Some((count, seq, at)) = self.units_due.take() else { return };
        let _span = super::span::span("settle units");
        let before: HashMap<usize, Rect> = self.units.iter().map(|unit| (unit.slot, unit.bounds)).collect();
        self.rebuild_units();
        {
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
            trace(|| format!("units of frame {seq} and {} before units={} areas={}", count - 1, self.units.len(), areas.len()));
            // An edit is drawn into the whole view at once, each frame. Not
            // so units Figma sends as the view pans to them, or what changes
            // out of view, whose tiles are drawn again as any are
            let seen = self.view.as_ref().is_none_or(|view| {
                let view = Rect::from_xywh(view.x as f32, view.y as f32, view.w as f32, view.h as f32);
                areas.iter().any(|area| Rect::intersects2(area, &view))
            });
            let panning = self.moved.elapsed() < PAN_STILL;
            if !areas.is_empty() && seen && !panning {
                if self.edited.elapsed() >= EDITING {
                    self.changing.clear();
                }
                self.changing.extend(self.units.iter().filter(|unit| !before.contains_key(&unit.slot)).map(|unit| unit.slot));
                self.edited = Instant::now();
                self.sent = Some(at);
            }
            self.drop_tiles(&areas);
        }
    }

    /// Gives back the layers drawing took, keeping what the next draw would
    /// only make again, as an edit's first draw would wait on that
    fn trim(&mut self) {
        let _span = super::span::span("trim");
        self.fit_images(false);
        // The whole view before this one only stands in while zooming, and
        // what's shown lets go of textures dropped here once it's done
        if self.last_whole.is_some() {
            self.drop_last_whole();
            self.publish();
        }
        self.spare.retain(|tex| (tex.width, tex.height) == (TILE, TILE));
        self.scratch.retain(|(_, ss, _)| *ss == 1);
        // Tiles show the view while Vello's memory is made again
        self.settle_tiles();
        if let Some(vello) = self.vello.as_mut() {
            vello.trim();
        }
        let _ = self.gpu.context.flush_and_submit();
        self.gpu.context.purge_unlocked_resources(skia_safe::gpu::PurgeResourceOptions::AllResources);
        super::committed::trim();
        self.update_memory();
        self.budget = Some(self.gpu.used() + HEADROOM);
        trace(|| format!("trimmed to {} MB in Skia's cache, {} MB used", self.gpu.context.resource_cache_usage().resource_bytes >> 20, self.gpu.used() >> 20));
    }

    /// Images take as much past the process's budget as within it: fewer
    /// has those that come back into view as it moves had again, over and
    /// over, and all that drew them drawn again each time, which takes
    /// more of the GPU, and its memory, than keeping them
    fn images_budget(&self) -> usize {
        IMAGES_BUDGET
    }

    /// Holds the process's GPU memory to what it takes at rest and a little
    /// more: past that, what's kept only in case it's needed again goes,
    /// Skia's spares, textures kept to use again, tiles out of view and
    /// images far from it, and none of it's kept till it's back under
    fn govern(&mut self) {
        let _span = super::span::span("govern");
        let Some(budget) = self.budget else { return };
        let was = self.pressed;
        // At rest it takes what it takes, and the budget's set again once
        // it's let go of what moving took; letting go of what's in use
        // then would only have it made again, over and over
        let moving = self.moved.elapsed() < VELLO_STILL || !SETTLE.saturating_sub(self.zoom.1.elapsed()).is_zero();
        self.pressed = moving && self.gpu.used() > budget;
        if !self.pressed {
            return;
        }
        if !was {
            trace(|| format!("past the GPU budget of {} MB", budget >> 20));
        }
        // But for a few tiles' worth, which the next tiles drawn take at
        // once, rather than each making a texture again, a millisecond each
        let mut kept = 0;
        self.spare.retain(|tex| {
            let tile = Arc::strong_count(tex) == 1 && (tex.width, tex.height) == (TILE, TILE) && kept < SPARE_PRESSED;
            kept += usize::from(tile);
            tile || Arc::strong_count(tex) > 1
        });
        if self.cleaned.elapsed() > Duration::from_millis(100) {
            self.cleaned = Instant::now();
            let _ = self.gpu.context.flush_and_submit();
            self.gpu.context.purge_unlocked_resources(skia_safe::gpu::PurgeResourceOptions::ScratchResourcesOnly);
            self.gpu.context.perform_deferred_cleanup(UNUSED, None);
            super::committed::tidy();
        }
        if self.fitted.elapsed() > FIT_SOON && self.images.values().map(Img::bytes).sum::<usize>() > self.images_budget() {
            self.fitted = Instant::now();
            self.fit_images(true);
        }
    }

    /// Gives back the GPU memory an offscreen render took, whole views drawn
    /// supersampled, which Skia would otherwise keep for reuse
    fn release(&mut self) {
        let _span = super::span::span("release");
        self.fit_images(false);
        self.settle_tiles();
        if let Some(vello) = self.vello.as_mut() {
            vello.trim();
        }
        self.spare.retain(|tex| (tex.width, tex.height) == (TILE, TILE) || Arc::strong_count(tex) > 1);
        if let Some(maker) = &self.maker {
            maker.ready.lock().unwrap().clear();
        }
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
    /// till it's had bigger again, so it'd flicker. `soon`, only those
    /// nowhere near the view go
    fn fit_images(&mut self, soon: bool) {
        let _span = super::span::span("fit images");
        let budget = self.images_budget();
        // As run.rs sizes images it fetches
        let fitted = super::run::fitted;
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
        // Where tiles at this zoom are drawn as the view moves: a tile
        // around it, and where it's headed. Further out, only coarse tiles.
        // Tiles are on a grid, so those around reach up to two past its edge
        let fine: Option<Rect> = self.view.as_ref().map(|view| {
            let margin = 2.0 * TILE as f64 / px.max(1e-6) as f64;
            let moving = self.moved.elapsed() < Duration::from_millis(150);
            let (vx, vy) = if moving { (self.velocity.0 * LOOKAHEAD * 2.0, self.velocity.1 * LOOKAHEAD * 2.0) } else { (0.0, 0.0) };
            let grow = if moving { (2f64.powf((self.widening * LOOKAHEAD * 2.0).clamp(0.0, 2.0)) - 1.0) / 2.0 } else { 0.0 };
            let (gx, gy) = (view.w * grow + margin, view.h * grow + margin);
            Rect::from_ltrb((view.x - gx + vx.min(0.0)) as f32, (view.y - gy + vy.min(0.0)) as f32, (view.x + view.w + gx + vx.max(0.0)) as f32, (view.y + view.h + gy + vy.max(0.0)) as f32)
        });
        // Units near the view, whether they're in it, and whether tiles of
        // this zoom draw them
        let near_units: HashMap<(usize, bool), (bool, bool)> = self
            .units
            .iter()
            .filter(|unit| near.is_none_or(|near| Rect::intersects2(&unit.bounds, &near)))
            .map(|unit| {
                let shown = seen.is_none_or(|seen| Rect::intersects2(&unit.bounds, &seen));
                ((unit.slot, unit.section), (shown, shown || fine.is_none_or(|fine| Rect::intersects2(&unit.bounds, &fine))))
            })
            .collect();
        // What pictures of this zoom draw them at, those of the zooms either
        // side kept for a while would keep them twice as big
        let mut need: HashMap<String, f64> = HashMap::new();
        let mut in_view: std::collections::HashSet<String> = std::collections::HashSet::new();
        // Of those, what tiles of this zoom draw, and the most coarse tiles
        // draw the others at
        let mut fine_use: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut coarse_need: HashMap<String, f64> = HashMap::new();
        for (&(slot, section, level, _), recorded) in &self.pictures {
            let Some(&(shown, close)) = near_units.get(&(slot, section)) else {
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
                // Those of this zoom further out are recorded again, at
                // what's had then, if the view comes back
                if close && (this_zoom || is_draft(level)) {
                    fine_use.insert(key.clone());
                } else if !this_zoom && !is_draft(level) {
                    let entry = coarse_need.entry(key.clone()).or_insert(0.0);
                    *entry = entry.max(*width);
                }
            }
        }
        // And what Vello drew them with
        let coarse_px = coarse(px.max(1e-6));
        for (unit, (drawn_px, uses)) in &self.vello_uses {
            if !near_units.contains_key(unit) {
                continue;
            }
            for (key, width, at) in uses {
                // Each image where it is, as a frame of many is one unit
                let shown = seen.is_none_or(|seen| Rect::intersects2(at, &seen));
                let close = shown || fine.is_none_or(|fine| Rect::intersects2(at, &fine));
                // At this zoom, whichever level last drew it
                let entry = need.entry(key.clone()).or_insert(0.0);
                *entry = entry.max(width * (px / drawn_px.max(1e-6)) as f64);
                if shown {
                    in_view.insert(key.clone());
                }
                if close {
                    fine_use.insert(key.clone());
                } else {
                    let entry = coarse_need.entry(key.clone()).or_insert(0.0);
                    *entry = entry.max(width * (coarse_px / drawn_px.max(1e-6)).min(1.0) as f64);
                }
            }
        }
        // And the images effects run on, wherever those effects are needed,
        // as they run again on them each time their node changes
        self.vello_in_view.retain(|_, (_, at)| at.elapsed() < IN_VIEW);
        for (key, (width, _)) in &self.vello_in_view {
            let entry = need.entry(key.clone()).or_insert(0.0);
            *entry = entry.max(*width as f64);
            in_view.insert(key.clone());
        }
        for (effect, images) in &self.effect_images {
            if !need.contains_key(effect) {
                continue;
            }
            let (shown, close) = (in_view.contains(effect), fine_use.contains(effect));
            for (key, width) in images {
                let entry = need.entry(key.clone()).or_insert(0.0);
                *entry = entry.max(*width);
                if shown {
                    in_view.insert(key.clone());
                }
                if close {
                    fine_use.insert(key.clone());
                }
            }
        }
        let now = Instant::now();
        for key in need.keys() {
            self.wanted.insert(key.clone(), now);
        }
        // Those that couldn't be had are asked for again once they've
        // waited, as what drew without them doesn't ask again
        let retry: Vec<(String, f64)> = self
            .failed
            .iter()
            .filter(|(key, (at, wait))| at.elapsed() >= *wait && !self.asked.contains_key(key.as_str()))
            .filter_map(|(key, _)| need.get(key).filter(|width| **width > 0.0).map(|width| (key.clone(), *width)))
            .collect();
        for (key, width) in retry {
            self.asked.insert(key.clone(), width);
            self.request(&key, width, None);
        }
        for (key, &width) in &need {
            if !in_view.contains(key) && !fine_use.contains(key) {
                continue;
            }
            let entry = self.needed.entry(key.clone()).or_insert((width, now));
            if width >= entry.0 || entry.1.elapsed() >= NEEDED_FOR {
                *entry = (width, now);
            }
        }
        self.needed.retain(|_, (_, at)| at.elapsed() < NEEDED_FOR);
        self.grown.retain(|_, at| at.elapsed() < NEEDED_FOR);
        let needed: HashMap<String, f64> = self.needed.iter().map(|(key, (width, _))| (key.clone(), *width)).collect();
        let needed = |key: &str| needed.get(key).copied().unwrap_or(0.0);
        let mut total: usize = self.images.values().map(Img::bytes).sum();
        trace(|| format!("images {} MB, {} near the view, {} in it{}", total >> 20, need.len(), in_view.len(), if soon { ", soon" } else { "" }));
        if total > budget {
            // Those not in view, needed longest ago (or never) first, and of
            // those the biggest
            let can_go = |key: &String, img: &Img| !key.starts_with(RENDER) && !in_view.contains(key.as_str()) && !((soon || key.starts_with(EFFECT)) && need.contains_key(key.as_str())) && needed(key) == 0.0 && img.image.width().max(img.image.height()) > PLACEHOLDER;
            // What custom effects drew is kept under more than one key: a
            // texture kept under keys still in use stays as it is, and one
            // let go is let go under each, or making it smaller under one
            // would save nothing
            let in_use: std::collections::HashSet<u32> = self.images.iter().filter(|(key, img)| !can_go(key, img)).map(|(_, img)| img.image.unique_id()).collect();
            let mut seen = std::collections::HashSet::new();
            let mut aside: Vec<(Option<Instant>, usize, String, u32)> = self
                .images
                .iter()
                .filter(|(key, img)| can_go(key, img) && !in_use.contains(&img.image.unique_id()) && seen.insert(img.image.unique_id()))
                .map(|(key, img)| (self.wanted.get(key).copied(), img.bytes(), key.clone(), img.image.unique_id()))
                .collect();
            aside.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
            let mut gone = Vec::new();
            for (_, _, key, id) in aside {
                if total <= budget * 3 / 4 {
                    break;
                }
                total = self.shrink(&key, total);
                // The same small copy under its other keys
                if let Some(small) = self.images.get(&key).cloned() {
                    for (other, img) in self.images.iter_mut().filter(|(other, img)| **other != key && img.image.unique_id() == id) {
                        total = total.saturating_sub(img.bytes()) + small.bytes();
                        *img = Img { stale: img.stale, ..small.clone() };
                        self.asked.remove(other.as_str());
                        gone.push(other.clone());
                    }
                }
                gone.push(key);
            }
            self.forget_images(&gone);
        }
        // Those Vello drew far bigger than it showed them are had again at
        // what it showed them at, with a step to spare for zooming in, and
        // swapped in once here. Vello draws from a mipmap level of them as
        // big, so they look the same
        let shown = std::mem::take(&mut self.vello_shown);
        let mut halve: Vec<(String, f64)> = Vec::new();
        for (key, width) in shown {
            let Some(img) = self.images.get(&key) else { continue };
            if key.starts_with(RENDER) || key.starts_with(EFFECT) || img.stale {
                continue;
            }
            // Nor smaller than units near the view need at this zoom: views
            // drawn since may be only coarse tiles around it, which show it
            // small, while it's in view bigger, and it'd be had again and
            // again at each size
            let asked = (width as f64 * super::vello::ASKED).max(need.get(&key).copied().unwrap_or(0.0)).max(needed(&key));
            let kept = img.image.width().max(img.image.height()) as f64;
            let fits = fitted(img.original, asked) as f64;
            if kept > fits * 2.0 && self.asked.get(&key) != Some(&asked) {
                total = total.saturating_sub((img.bytes() as f64 * (1.0 - (fits / kept).powi(2))) as usize);
                halve.push((key, asked));
            }
        }
        // Moving, those only coarse tiles draw are had again at the size
        // they draw them, swapped in once here, so nothing shows blurrier
        if soon && total > budget {
            let mut smaller: Vec<(usize, String, f64)> = self
                .images
                .iter()
                .filter(|(key, _)| !key.starts_with(RENDER) && !key.starts_with(EFFECT) && !in_view.contains(key.as_str()) && !fine_use.contains(key.as_str()))
                .filter_map(|(key, img)| {
                    let width = coarse_need.get(key.as_str()).filter(|width| **width > 0.0)?.max(needed(key));
                    let kept = img.image.width().max(img.image.height()) as f64;
                    let fits = fitted(img.original, width) as f64;
                    (kept > fits).then(|| ((img.bytes() as f64 * (1.0 - (fits / kept).powi(2))) as usize, key.clone(), width))
                })
                .collect();
            smaller.sort_by(|a, b| b.0.cmp(&a.0));
            for (saving, key, width) in smaller {
                if total <= budget * 3 / 4 {
                    break;
                }
                total = total.saturating_sub(saving);
                halve.push((key, width));
            }
        }
        // Those in view alone past it, as far out with many images each
        // zoomed in on before: each more than twice what it's drawn at is
        // had again at that, the same on screen
        if total > budget {
            let mut smaller: Vec<(usize, String, f64)> = self
                .images
                .iter()
                .filter(|(key, _)| !key.starts_with(RENDER) && !key.starts_with(EFFECT))
                .filter_map(|(key, img)| {
                    let width = need.get(key.as_str()).filter(|width| **width > 0.0)?.max(needed(key));
                    let kept = img.image.width().max(img.image.height()) as f64;
                    let fits = fitted(img.original, width) as f64;
                    (kept > fits * 2.0).then(|| ((img.bytes() as f64 * (1.0 - (fits / kept).powi(2))) as usize, key.clone(), width))
                })
                .collect();
            smaller.sort_by(|a, b| b.0.cmp(&a.0));
            for (saving, key, width) in smaller {
                if total <= budget * 3 / 4 {
                    break;
                }
                total = total.saturating_sub(saving);
                halve.push((key, width));
            }
        }
        self.make_smaller(halve);
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
        let _span = super::span::span("shrink");
        let _purpose = super::gpu_census::purpose("image");
        let Some(img) = self.images.remove(key) else {
            return total;
        };
        self.asked.remove(key);
        let bytes = img.bytes();
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

    /// Has images `smaller` kept at what each width takes. Where that's a
    /// mipmap level of what's kept, the level is copied on the GPU, so it's
    /// at once and draws as it did, rather than fetched, decoded and
    /// uploaded again behind the images still loading. Others are asked for
    /// again
    fn make_smaller(&mut self, smaller: Vec<(String, f64)>) {
        let _span = super::span::span("make smaller");
        let _purpose = super::gpu_census::purpose("image");
        let started = Instant::now();
        let mut made = 0;
        // What they're made from, kept till the GPU has read them (see
        // `Gpu::flush`)
        let mut from: Vec<Image> = Vec::new();
        for (key, width) in smaller {
            // A few milliseconds' worth a pass, the rest at the next
            if started.elapsed() >= MAKE_SMALLER_A_PASS {
                break;
            }
            if self.asked.get(&key) == Some(&width) || self.grown.get(&key).is_some_and(|at| at.elapsed() < NEEDED_FOR) {
                continue;
            }
            let Some(img) = self.images.get(&key) else { continue };
            let kept = img.image.width().max(img.image.height()) as u32;
            let fits = super::run::fitted(img.original, width).max(1);
            let halvings = (kept / fits).max(1).ilog2();
            let level = img.image.has_mipmaps() && halvings > 0 && kept >> halvings == fits;
            self.asked.insert(key.clone(), width);
            match level.then(|| halved(&mut self.gpu, img, halvings)).flatten() {
                Some(small) => {
                    from.push(img.image.clone());
                    made += 1;
                    self.take_image(key, small, false);
                }
                None => self.request(&key, width, None),
            }
        }
        if made > 0 {
            let _ = self.gpu.context.flush_and_submit();
            trace(|| format!("made {made} images smaller on the GPU in {:.1}ms", started.elapsed().as_secs_f64() * 1000.0));
        }
        drop(from);
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
        let _span = super::span::span("update memory");
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
        // A unit of the tree is keyed by what its nodes draw and where, not
        // their ids, as Figma sends what's over an edit again under new ids
        // each frame, the same otherwise
        if unit.place.is_some() {
            unit.slot.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// What units `keys` draw changed, though they're the same units
    fn bump(&mut self, keys: impl IntoIterator<Item = (usize, bool)>) {
        for key in keys {
            self.version_next += 1;
            self.versions.insert(key, self.version_next);
            self.touched.push(key);
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
        let _span = super::span::span("check changes");
        if self.revision == self.checked_revision {
            return;
        }
        self.checked_revision = self.revision;
        let bits = |r: &Rect| [r.left.to_bits(), r.top.to_bits(), r.right.to_bits(), r.bottom.to_bits()];
        let rect = |b: &[u32; 4]| Rect::from_ltrb(f32::from_bits(b[0]), f32::from_bits(b[1]), f32::from_bits(b[2]), f32::from_bits(b[3]));
        // The units the same, only those whose version changed are looked
        // at, as images coming in had all of tens of thousands marked again
        // each, a millisecond a pass
        let touched = std::mem::take(&mut self.touched);
        if !std::mem::take(&mut self.units_rebuilt) && self.checked.len() == self.units.len() {
            let (mut areas, mut sharper) = (Vec::new(), Vec::new());
            for key in touched {
                let Some(&at) = self.checked_at.get(&key) else { continue };
                let unit = &self.units[at];
                let mark = ((unit.slot, unit.section), bits(&unit.bounds), self.mark(unit), self.sharpened.get(&(unit.slot, unit.section)).copied().unwrap_or(0));
                let old = &self.checked[at];
                if *old == mark {
                    continue;
                }
                if (old.1, old.2) == (mark.1, mark.2) {
                    sharper.push(rect(&mark.1));
                } else {
                    areas.push(rect(&old.1));
                    areas.push(rect(&mark.1));
                }
                self.checked[at] = mark;
            }
            areas.retain(|area: &Rect| area.width() > 0.0 || area.height() > 0.0);
            trace(|| format!("  check found {} areas changed, {} sharper", areas.len(), sharper.len()));
            self.drop_tiles(&areas);
            self.sharpen(&sharper);
            return;
        }
        let now: Vec<Mark> = self
            .units
            .iter()
            .map(|unit| ((unit.slot, unit.section), bits(&unit.bounds), self.mark(unit), self.sharpened.get(&(unit.slot, unit.section)).copied().unwrap_or(0)))
            .collect();
        self.checked_at = now.iter().enumerate().map(|(at, mark)| (mark.0, at)).collect();
        if now == self.checked {
            return;
        }
        let mut areas: Vec<Rect> = Vec::new();
        // Where only images came sharper: what's drawn shows the same, so
        // it's shown till it's drawn again
        let mut sharper: Vec<Rect> = Vec::new();
        // The same units in the same order, as when images come, compared
        // in place, as maps of tens of thousands took milliseconds each
        if now.len() == self.checked.len() && now.iter().zip(&self.checked).all(|(new, old)| new.0 == old.0) {
            for (old, mark) in self.checked.iter().zip(&now) {
                if old == mark {
                    continue;
                }
                if (old.1, old.2) == (mark.1, mark.2) {
                    sharper.push(rect(&mark.1));
                } else {
                    areas.push(rect(&old.1));
                    areas.push(rect(&mark.1));
                }
            }
            self.checked = now;
            areas.retain(|area| area.width() > 0.0 || area.height() > 0.0);
            trace(|| format!("  check found {} areas changed, {} sharper", areas.len(), sharper.len()));
            self.drop_tiles(&areas);
            self.sharpen(&sharper);
            return;
        }
        let before: HashMap<(usize, bool), &Mark> = self.checked.iter().map(|mark| (mark.0, mark)).collect();
        let after: HashMap<(usize, bool), &Mark> = now.iter().map(|mark| (mark.0, mark)).collect();
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
        let _span = super::span::span("sharpen");
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
        let _span = super::span::span("drop tiles");
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
        let view = self.view_area().map(|(view, _)| view);
        for key in stale {
            if let Some(tile) = self.tiles.get_mut(&key) {
                tile.seen = (tile.seen || !tile.stale) && view.is_some_and(|view| Rect::intersects2(&view, &key.area()));
                tile.stale = true;
            }
        }
        let hit: Vec<TileKey> = self
            .drawn_tiles
            .iter()
            .flat_map(|drawn| drawn.tiles.iter().map(|(key, ..)| *key))
            .filter(|key| {
                let reach = self.margin(*key) as f32 / key.px();
                let area = key.area().with_outset((reach, reach));
                areas.iter().any(|other| Rect::intersects2(&area, other))
            })
            .collect();
        if let Some(drawn) = self.drawn_tiles.as_mut() {
            for (key, _, stale, seen) in &mut drawn.tiles {
                if hit.contains(key) {
                    *seen = (*seen || !*stale) && view.is_some_and(|view| Rect::intersects2(&view, &key.area()));
                    *stale = true;
                }
            }
        }
        if let Some(pending) = &self.pending {
            let reach = pending.margin as f32 / pending.key.px();
            let area = pending.key.area().with_outset((reach, reach));
            if areas.iter().any(|other| Rect::intersects2(&area, other)) {
                self.pending = None;
            }
        }
        // Only what changed where they show, as one hidden for a change
        // off it would show tiles meanwhile, a fraction of a pixel off
        let over = |layer: &Layer| {
            let at = Rect::from_xywh(layer.x as f32 / layer.px, layer.y as f32 / layer.px, layer.tex.width as f32 / layer.px, layer.tex.height as f32 / layer.px);
            areas.iter().filter(move |area| Rect::intersects2(&at, area)).copied()
        };
        if let Some(whole) = &mut self.whole {
            let inside: Vec<Rect> = over(&whole.layer).collect();
            whole.patch.extend(inside);
        }
        if let Some(unchecked) = &mut self.unchecked {
            let inside: Vec<Rect> = over(&unchecked.layer).collect();
            unchecked.patch.extend(inside);
        }
        if let Some(kept) = &self.scene_kept {
            let px = f32::from_bits(kept.px);
            let area = Rect::from_xywh(kept.left as f32 / px, kept.top as f32 / px, kept.width as f32 / px, kept.height as f32 / px);
            if areas.iter().any(|other| Rect::intersects2(&area, other)) {
                self.kept_stale = true;
            }
        }
        self.drop_last_whole();
    }

    fn rebuild_units(&mut self) {
        let _span = super::span::span("rebuild units");
        self.revision += 1;
        self.units_rebuilt = true;
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
        let _span = super::span::span("record picture");
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
                self.ask(&images, Some(bounds));
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
            self.ask(&images, Some(bounds));
            self.ask_exact(&exact, Some(bounds));
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
        let _span = super::span::span("wait for");
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
        let _span = super::span::span("thumb");
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
        let _span = super::span::span("page image");
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
        let _span = super::span::span("prewarm");
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
        let _span = super::span::span("unpage");
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
    fn ask_exact(&mut self, renders: &[(String, f64)], at: Option<Rect>) {
        for (key, width) in renders {
            if self.failed.get(key).is_some_and(|(at, wait)| at.elapsed() < *wait) {
                continue;
            }
            let asked = self.asked.get(key).copied().unwrap_or(0.0);
            if (width / asked - 1.0).abs() > EXACT_TOLERANCE as f64 {
                self.asked.insert(key.clone(), *width);
                self.request(key, *width, at);
            }
        }
    }

    fn ask(&mut self, images: &[(String, f64)], at: Option<Rect>) {
        for (key, width) in images {
            if self.failed.get(key).is_some_and(|(at, wait)| at.elapsed() < *wait) {
                continue;
            }
            let asked = self.asked.get(key).copied().unwrap_or(0.0);
            if *width > asked * 1.05 {
                self.asked.insert(key.clone(), *width);
                self.request(key, *width, at);
            }
        }
    }

    /// Asks the page for image `key` about `width` pixels wide, or runs a
    /// node's effects for it
    fn request(&mut self, key: &str, width: f64, at: Option<Rect>) {
        if key.starts_with(EFFECT) {
            self.effect(key, width);
        } else {
            self.requests.send(Request { key: key.to_string(), width, at });
        }
    }

    /// Runs the custom effects of the node `key` names, as Figma's canvas
    /// does: at a pixel a unit, on the node as drawn without them over its
    /// box in its own space, or for a group, on the child after `>` as drawn
    /// over its box. If that draws images not here yet, it waits for them
    fn effect(&mut self, key: &str, width: f64) {
        let _span = super::span::span("effect input");
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
            self.ask(&images, None);
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
        let _span = super::span::span("effect input");
        let began = Instant::now();
        let Some(tree) = &self.tree else {
            return;
        };
        let Some(effect) = tree.custom_effect(id) else {
            return;
        };
        // A run that never answers holds up no more than a while
        let running = self.effects_running.get(&effect.node).is_some_and(|(running, at)| running != key && at.elapsed() < EFFECT_PATIENCE);
        if running && !self.kept.lock().unwrap().has(key) {
            // Asked for again once it's done, so not asked for meanwhile
            self.effects_again.insert(effect.node.clone(), (key.to_string(), width));
            self.asked.remove(key);
            return;
        }
        self.effect_nodes.insert(key.to_string(), effect.stand_in.clone());
        self.effect_last.insert(key.to_string(), effect.last_drawn.clone());
        if self.kept.lock().unwrap().has(key) {
            let _ = self.effects.send(Job::Resize { key: key.to_string(), width });
            return;
        }
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
        let sharper: Vec<(String, f64)> = drawer.sharper_images.drain(..).filter(|(sharper, _)| sharper != key).collect();
        let used: Vec<(String, f64)> = drawer.used_images.drain(..).filter(|(used, _)| used != key).collect();
        drop(drawer);
        self.effect_images.insert(key.to_string(), used);
        if !images.is_empty() {
            // Asked for still: run again once one of those images comes,
            // not each time what draws it is recorded or another image comes
            let on: Vec<String> = images.iter().map(|(image, _)| image.clone()).collect();
            self.effects_waiting.retain(|(waiting, _, _)| waiting != key);
            self.effects_waiting.push((key.to_string(), width, on));
            self.ask(&images, None);
            return;
        }
        let info = ImageInfo::new((pw, ph), ColorType::RGBA8888, AlphaType::Premul, None);
        let mut pixels = vec![0u8; (pw * ph * 4) as usize];
        if !surface.canvas().read_pixels(&info, &mut pixels, (pw * 4) as usize, (0, 0)) {
            return;
        }
        // Run now on the smaller copies here, and again once sharper ones
        // come, rather than keeping what an image's thumbnail drew
        if !sharper.is_empty() {
            let on: Vec<String> = sharper.iter().map(|(image, _)| image.clone()).collect();
            self.effects_waiting.retain(|(waiting, _, _)| waiting != key);
            self.effects_waiting.push((key.to_string(), width, on));
            self.ask(&sharper, None);
        }
        trace(|| format!("effect input {key} {pw}x{ph} took={:.1}", began.elapsed().as_secs_f64() * 1000.0));
        self.effects_running.insert(effect.node.clone(), (key.to_string(), Instant::now()));
        self.effects_again.remove(&effect.node);
        let run = Run { key: key.to_string(), node: effect.node, chain: effect.chain, pixels, width: pw as u32, height: ph as u32, scale, shown: width };
        let _ = self.effects.send(Job::Run(run));
    }

    /// Draws tile `key`: every unit over it, in order. Exact tiles take
    /// Figma's 16 samples a pixel, others one with anti-aliased edges, which
    /// is many times quicker
    fn draw_tile(&mut self, key: TileKey, exact: bool, draft: bool) -> Option<Tile> {
        let _span = super::span::span("draw tile");
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
        let _span = super::span::span("step tile");
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
        Some(Tile { image, tex, used: self.frame, counts, stale: false, seen: false, exact, draft, vello: false, drawn: 0 })
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
        let _span = super::span::span("compose at");
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
        let _span = super::span::span("evict");
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
        let _span = super::span::span("offscreen");
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
        self.ask(&images, Some(bounds));
        self.ask_exact(&exact, Some(bounds));
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
        let _span = super::span::span("render view");
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
            let frames = v.get(16).copied().unwrap_or(0.0) as usize;
            let vello = match v.get(14).copied().unwrap_or(0.0) > 0.0 && self.vello_ready() {
                false => String::new(),
                // Frames zooming in a little each, each drawn while the GPU
                // draws the last, as the window draws them
                true if frames > 0 => {
                    self.settle_unchecked();
                    let at = Instant::now();
                    let (mut last, mut short, mut failed): (Option<(Arc<Tex>, usize)>, usize, Option<String>) = (None, 0, None);
                    let mut seen = String::new();
                    for frame in 0..frames {
                        let px = px * (1.0 + frame as f32 * 0.002);
                        let (left, top) = ((cx * px - w as f32 / 2.0).floor() as i32, (cy * px - h as f32 / 2.0).floor() as i32);
                        match self.vello_view_once(left, top, w, h, px, area, None) {
                            Ok((tex, _, stats, _)) => {
                                seen = stats;
                                let renders = self.vello.as_ref().map_or(0, |vello| vello.renders);
                                if let Some((tex, renders)) = last.replace((tex, renders)) {
                                    if self.vello.as_mut().map(|vello| vello.check(renders)).is_some_and(|checked| !matches!(checked, Ok(None))) {
                                        short += 1;
                                    }
                                    self.retire(tex);
                                }
                            }
                            Err(why) => {
                                failed = Some(why);
                                break;
                            }
                        }
                    }
                    if let Some((tex, renders)) = last.take() {
                        if self.vello.as_mut().map(|vello| vello.check(renders)).is_some_and(|checked| !matches!(checked, Ok(None))) {
                            short += 1;
                        }
                        self.retire(tex);
                    }
                    if let Some(vello) = self.vello.as_ref() {
                        seen += " gpu ";
                        seen += &vello.stages;
                    }
                    let _ = self.gpu.context.flush_submit_and_sync_cpu();
                    match failed {
                        Some(why) => format!("can't: {why}"),
                        None => format!("pipelined {frames} frames {:.1}ms each, {short} short, last {seen}", at.elapsed().as_secs_f64() * 1000.0 / frames as f64),
                    }
                }
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

/// A parked tab's render tree, see `Painter::parked`
struct Parked {
    tab: Arc<str>,
    tree: Tree,
    whole_sum: Option<(usize, u64)>,
}

/// A whole scene's forest and root, summed, a word at a time, as hashing it
/// as bytes took tens of milliseconds
fn forest_sum(forest: &[u8], root: u32) -> (usize, u64) {
    let (mut a, mut b) = (0x9e37_79b9_7f4a_7c15u64 ^ root as u64, 0xc2b2_ae3d_27d4_eb4fu64);
    let mut words = forest.chunks_exact(16);
    for pair in &mut words {
        let (x, y) = (u64::from_le_bytes(pair[..8].try_into().unwrap()), u64::from_le_bytes(pair[8..].try_into().unwrap()));
        a = (a ^ x).wrapping_mul(0x9fb2_1c65_1e98_df25).rotate_left(29);
        b = (b ^ y).wrapping_mul(0xd6e8_feb8_6659_fd93).rotate_left(31);
    }
    for (index, &byte) in words.remainder().iter().enumerate() {
        a = (a ^ ((byte as u64) << (index % 8 * 8))).wrapping_mul(0x9fb2_1c65_1e98_df25);
    }
    (forest.len(), a ^ b.rotate_left(17))
}
