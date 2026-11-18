//! The present thread: puts Figma's view in the window every frame, from
//! the latest snapshot of what the render thread has drawn, so the window
//! follows the view the moment it moves however long drawing takes
//!
//! The render thread draws tiles and the whole view into textures of their
//! own (`Tex`), made outside Skia so its caches never hand them to anything
//! else, and hands over snapshots of them (`Snapshot`): only what's up to
//! date, all of it as of one frame of Figma's render tree, never changed
//! once handed over. This thread has a Skia context of its own on the same
//! device and a queue of its own, and composes each frame of one snapshot
//! the GPU has finished drawing, at the latest view that changes nothing in
//! the scene since, with what Figma draws over the canvas in that frame.
//! Where the snapshot doesn't cover all of the view it shows nothing new
//! till the render thread has drawn more, so what's in the window is always
//! all of the view, as of one moment of the page

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use skia_safe::gpu::d3d::{BackendContext, TextureResourceInfo};
use skia_safe::gpu::{self, BackendRenderTarget, BackendTexture, DirectContext, Protected, SurfaceOrigin, SyncCpu};
use skia_safe::{AlphaType, Color4f, ColorType, Data, FilterMode, Image, Matrix, MipmapMode, Paint, Rect, SamplingOptions, Surface, TileMode};
use windows::core::Interface;
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Graphics::Direct3D12::{
    ID3D12CommandQueue, ID3D12Device, ID3D12Fence, ID3D12Resource, D3D12_COMMAND_QUEUE_DESC, D3D12_FENCE_FLAG_NONE, D3D12_COMMAND_QUEUE_PRIORITY_HIGH, D3D12_HEAP_FLAG_NONE,
    D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_TEXTURE2D,
    D3D12_RESOURCE_FLAGS, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, D3D12_HEAP_FLAG_SHARED, D3D12_RESOURCE_STATE_COMMON,
    D3D12_TEXTURE_LAYOUT_UNKNOWN,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC, DXGI_STANDARD_MULTISAMPLE_QUALITY_PATTERN,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice2, IDCompositionDesktopDevice, IDCompositionTarget, IDCompositionVisual2, DCOMPOSITION_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_IGNORE, DXGI_ALPHA_MODE_UNSPECIFIED};
use windows::Win32::Graphics::Dxgi::{
    IDXGIAdapter1, IDXGIFactory2, IDXGIFactory4, IDXGISwapChain1, IDXGISwapChain2, IDXGISwapChain3, DXGI_PRESENT, DXGI_SCALING_NONE, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::System::Threading::WaitForSingleObjectEx;

use super::draw::Counts;
use super::overlay::Overlay;
use super::render::View;
use super::stats;

/// The GPU both threads draw with
#[derive(Clone)]
pub struct Device {
    pub factory: IDXGIFactory4,
    pub adapter: IDXGIAdapter1,
    pub device: ID3D12Device,
    pub name: String,
}

// D3D12 devices and DXGI factories are free-threaded
unsafe impl Send for Device {}

/// A texture the render thread draws a tile or a view into and this thread
/// shows. Two Skia contexts on two queues use it, each knowing nothing of
/// what the other did to it, so it allows simultaneous access: D3D12 itself
/// returns it to the common state once each submission that used it is
/// done, and promotes it from there to whatever the next needs. So it's in
/// the common state whenever a context wraps it, as each is told, whatever
/// either did before. Its render target memory is also never compressed,
/// which reading it without the transitions that would resolve that could
/// otherwise show what was in that memory before. It's made with the sample
/// quality Skia gives what it makes itself, which the stencils it makes for
/// it have to match. Once neither thread holds it, it's freed once the GPU
/// is done with it too (see `buried`)
pub struct Tex {
    resource: std::mem::ManuallyDrop<ID3D12Resource>,
    pub width: i32,
    pub height: i32,
}

/// Textures let go of, for the render thread to free once its queue has
/// passed all it was asked to do with them
struct Graves(Vec<ID3D12Resource>);

unsafe impl Send for Graves {}

static GRAVES: std::sync::Mutex<Graves> = std::sync::Mutex::new(Graves(Vec::new()));

/// Textures let go of since last asked, see `Tex`
pub fn buried() -> Vec<ID3D12Resource> {
    std::mem::take(&mut GRAVES.lock().unwrap().0)
}

/// The render and present threads running
static DRAWING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn drawing() {
    DRAWING.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// One of them ending, its GPU done with all it asked: the last frees the
/// textures let go of, as no thread's left to
pub fn done() {
    if DRAWING.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
        drop(buried());
    }
}

impl Drop for Tex {
    fn drop(&mut self) {
        let resource = unsafe { std::mem::ManuallyDrop::take(&mut self.resource) };
        GRAVES.lock().unwrap().0.push(resource);
    }
}

unsafe impl Send for Tex {}
unsafe impl Sync for Tex {}

impl Tex {
    pub fn new(device: &ID3D12Device, width: i32, height: i32) -> Option<Arc<Tex>> {
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: width.max(1) as u64,
            Height: height.max(1) as u32,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: DXGI_STANDARD_MULTISAMPLE_QUALITY_PATTERN },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAGS(D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET.0 | D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS.0),
        };
        let mut resource: Option<ID3D12Resource> = None;
        unsafe { device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut resource) }
            .map_err(|error| log::warn!("native renderer: texture: {error}"))
            .ok()?;
        let resource = resource?;
        let _ = unsafe { resource.SetName(windows::core::w!("Ligma shared texture")) };
        Some(Arc::new(Tex { resource: std::mem::ManuallyDrop::new(resource), width, height }))
    }

    /// One another device can open too, through a shared handle, and that
    /// compute shaders can write
    pub fn new_shared(device: &ID3D12Device, width: i32, height: i32) -> Option<Arc<Tex>> {
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Alignment: 0,
            Width: width.max(1) as u64,
            Height: height.max(1) as u32,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAGS(
                D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET.0 | D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS.0 | D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0,
            ),
        };
        let mut resource: Option<ID3D12Resource> = None;
        unsafe { device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_SHARED, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut resource) }
            .map_err(|error| log::warn!("native renderer: shared texture: {error}"))
            .ok()?;
        let resource = resource?;
        let _ = unsafe { resource.SetName(windows::core::w!("Ligma Vello texture")) };
        Some(Arc::new(Tex { resource: std::mem::ManuallyDrop::new(resource), width, height }))
    }

    pub fn resource(&self) -> &ID3D12Resource {
        &self.resource
    }

    /// For a Skia context to wrap, in the common state it's always in
    /// between submissions
    pub fn backend(&self) -> BackendTexture {
        let info = TextureResourceInfo {
            resource: (*self.resource).clone(),
            alloc: None,
            resource_state: D3D12_RESOURCE_STATE_COMMON,
            format: DXGI_FORMAT_R8G8B8A8_UNORM,
            sample_count: 1,
            level_count: 1,
            sample_quality_pattern: DXGI_STANDARD_MULTISAMPLE_QUALITY_PATTERN,
            protected: Protected::No,
        };
        gpu::backend_textures::make_d3d((self.width, self.height), &info, "ligma")
    }
}

/// A fence on the render thread's queue
pub struct Fence(pub ID3D12Fence);

unsafe impl Send for Fence {}
unsafe impl Sync for Fence {}

/// What a layer is
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LayerKind {
    Tile,
    /// A tile drawn quickly at less detail, to stand in for a moment
    Draft,
    /// All of the view and around it, drawn while the zoom changes, while
    /// things change, or before its tiles are drawn
    Whole,
    /// A tile or the whole view as it was before something in it changed,
    /// being drawn again: what was shown there a moment ago, so preferred
    /// to anything up to date but much blurrier, which would show the
    /// view go soft and sharp again
    Stale,
    /// A tile out of date since before it was last in view: the page as it
    /// was some while ago, shown only where nothing up to date is
    Old,
}

/// Something drawn at `px` device pixels a unit, its top left corner at
/// (`x`, `y`) device pixels at that zoom, and `phase` of a pixel further
#[derive(Clone)]
pub struct Layer {
    pub tex: Arc<Tex>,
    pub x: i32,
    pub y: i32,
    pub phase: (f32, f32),
    pub px: f32,
    pub counts: Counts,
    pub kind: LayerKind,
    /// When it was drawn, later ones higher (see `Painter::serial`)
    pub drawn: u64,
}

/// Longest a view waits for what it needs drawn before it's shown with
/// what there is
const HOLD_MOST: Duration = Duration::from_millis(50);

/// Longest a frame that changes the scene waits for the change to be drawn,
/// so Figma's overlay never shows ahead of what it's drawn over, unless the
/// render thread's that far behind
const EDIT_HOLD: Duration = Duration::from_millis(250);

/// What the render thread has drawn, every layer of it up to date as of
/// frame `seq` of Figma's render tree, so whatever's composed of it at any
/// view is all of one moment of the page. Nothing in it changes once it's
/// handed over: what's drawn again is drawn into textures of its own
pub struct Snapshot {
    pub seq: u64,
    /// The latest frame whose changes are drawn wherever the view is: one
    /// after it that changes the scene isn't shown over this
    pub changes: u64,
    /// The GPU has drawn all of it once `fence` passes `value`
    pub fence: Arc<Fence>,
    pub value: u64,
    /// Tells it from the one before
    pub serial: u64,
    pub layers: Vec<Layer>,
    /// Where there's anything drawn, in units: past it the view shows only
    /// the canvas, so needs no layer
    pub content: Option<Rect>,
    /// The view the whole view is drawn for, if it's up to date: its
    /// corner and zoom, see `view_at`
    pub whole_at: Option<(i32, i32, u32)>,
    pub background: Color4f,
    /// Figma's checkerboard, and its darker kind, for a hidden background
    pub transparent: bool,
    pub dark: bool,
    /// The render thread has more to draw for the view
    pub pending: bool,
}

impl Snapshot {
    /// Up to date as of frame `seq` once the GPU has drawn it
    pub fn drawn(&self) -> bool {
        (unsafe { self.fence.0.GetCompletedValue() }) >= self.value
    }
}

/// What the threads share
pub struct Hub {
    pub view: Mutex<Option<View>>,
    /// The latest snapshot handed over
    pub snapshot: Mutex<Option<Arc<Snapshot>>>,
    /// The window's size in pixels
    pub size: Mutex<(u32, u32)>,
    /// Drawn in place of Figma's canvas, under the tab: where the tab's
    /// webview is in the window, in pixels, and whether the tab is shown
    pub origin: Mutex<Option<(i32, i32)>>,
    pub stop: AtomicBool,
    /// Frames of the render tree that couldn't be applied in a row
    pub tree_failures: std::sync::atomic::AtomicU32,
    /// Memory shared with the tab drawn
    pub shared: Mutex<Option<crate::webview2::Shared>>,
    /// Frames of the render tree from the one shown now on
    frames: Mutex<std::collections::VecDeque<FrameView>>,
    /// The frame last shown, for tracing
    pub shown_seq: AtomicU64,
    changes: Mutex<u64>,
    changed: Condvar,
}

impl Hub {
    pub fn new(size: (u32, u32)) -> Self {
        Hub {
            view: Mutex::new(None),
            snapshot: Mutex::new(None),
            size: Mutex::new(size),
            origin: Mutex::new(None),
            stop: AtomicBool::new(false),
            tree_failures: std::sync::atomic::AtomicU32::new(0),
            shared: Mutex::new(None),
            frames: Mutex::new(std::collections::VecDeque::new()),
            shown_seq: AtomicU64::new(0),
            changes: Mutex::new(0),
            changed: Condvar::new(),
        }
    }

    /// Wakes the present thread for something new to show
    pub fn poke(&self) {
        *self.changes.lock().unwrap() += 1;
        self.changed.notify_all();
    }

    pub fn set_view(&self, view: View) {
        self.frames.lock().unwrap().clear();
        *self.view.lock().unwrap() = Some(view);
        self.poke();
    }

    /// Frame `seq` of the render tree: its view, what Figma draws over the
    /// canvas in it, whether it `edits` the scene, and how long it took
    pub fn frame_view(&self, seq: u64, view: View, overlay: Overlay, edits: bool, made: Option<Made>) {
        let mut frames = self.frames.lock().unwrap();
        // Lost track: those before go
        if frames.back().is_some_and(|last| last.seq >= seq) {
            frames.clear();
        }
        frames.push_back(FrameView { seq, view: view.clone(), overlay, edits, made, came: Instant::now() });
        while frames.len() > 256 {
            frames.pop_front();
        }
        drop(frames);
        *self.view.lock().unwrap() = Some(view);
        self.poke();
    }

    /// The latest frame that can be shown over a snapshot as of frame
    /// `seq`: one that changes nothing in the scene since, so its view and
    /// what Figma draws over it go with the scene as it is in the snapshot.
    /// None while the next is an edit not drawn yet, to show nothing new
    /// till it is
    /// The latest frame to show with what's drawn up to frame `seq`, and
    /// the oldest frame after `shown` up to it, which it's shown in place
    /// of. Else how long till one changing the scene is shown without it
    fn pick(&self, seq: u64, shown: u64) -> Result<Picked, Option<Duration>> {
        let mut frames = self.frames.lock().unwrap();
        if frames.is_empty() {
            return self.view.lock().unwrap().clone().map(|view| Picked { view, overlay: None, seq, oldest: None, wait: None }).ok_or(None);
        }
        let mut take = None;
        let mut wait = None;
        for (i, frame) in frames.iter().enumerate() {
            // One that changes the scene waits for it to be drawn, so what
            // moves with the view moves with it, but not for long, as the
            // view standing still reads as a stutter
            let waited = frame.came.elapsed();
            if frame.edits && frame.seq > seq && waited < EDIT_HOLD {
                wait = Some(EDIT_HOLD - waited);
                break;
            }
            take = Some(i);
        }
        let take = take.ok_or(wait)?;
        let oldest = frames.iter().take(take + 1).filter(|frame| frame.seq > shown).find_map(|frame| frame.made);
        frames.drain(..take);
        let frame = frames.front().ok_or(wait)?;
        Ok(Picked { view: frame.view.clone(), overlay: frame.overlay.clone(), seq: frame.seq, oldest, wait })
    }

    /// Waits until poked after `seen`, or `timeout`, and returns the count
    fn wait(&self, seen: u64, timeout: Duration) -> u64 {
        let changes = self.changes.lock().unwrap();
        let (changes, _) = self.changed.wait_timeout_while(changes, timeout, |changes| *changes == seen).unwrap();
        *changes
    }
}

struct FrameView {
    seq: u64,
    view: View,
    overlay: Overlay,
    edits: bool,
    made: Option<Made>,
    came: Instant,
}

struct Picked {
    view: View,
    overlay: Overlay,
    seq: u64,
    oldest: Option<Made>,
    /// How long till a frame after it that changes the scene is shown
    /// without waiting for it
    wait: Option<Duration>,
}

/// How long Figma took to make a frame: when it handed it over, in ms since
/// the epoch, and how long before that what it answers came, the input or
/// else the start of the page's frame
#[derive(Clone, Copy)]
pub struct Made {
    pub at: f64,
    pub figma: f32,
}

/// The pixel `at`, in device pixels, is in, and how far into it, which is
/// a fifth of one or so, as Figma's view moves by CSS pixels. To a
/// hundredth, as `at`'s worked out from a zoom in single precision
pub fn phase(at: f64) -> (i32, f32) {
    let pixel = (at + 5e-3).floor();
    (pixel as i32, (((at - pixel) * 100.0).round() / 100.0).max(0.0) as f32)
}

/// The view's corner in device pixels at the zoom it's drawn at in a window
/// `size`, and that zoom, as bits
pub fn view_at(view: &View, size: (u32, u32)) -> (i32, i32, u32) {
    let (px, _, _, _) = view.drawn(size);
    // From the pixel the window's corner is in, as Figma draws from a
    // fraction of one, and a preview's pixels are magnified
    (phase(view.x * px as f64).0, phase(view.y * px as f64).0, px.to_bits())
}

/// Part of a frame: layer `layer` drawn at `dest` in it, where `clip` is
pub struct Piece {
    pub layer: usize,
    pub dest: Rect,
    pub clip: skia_safe::Region,
    /// Sampled nearest, as it lands on whole pixels or is a pixel preview's
    /// squares
    pub square: bool,
}

/// What a frame of `view` in a window `size` is composed of: `layers`
/// nearest what Figma shows first, the tiles of its zoom, then the whole
/// view drawn at it, then the rest, scaled, each where those before don't
/// cover. And whether that leaves some of the view out
pub fn cover(layers: &[Layer], content: Option<Rect>, view: &View, size: (u32, u32)) -> (Vec<Piece>, bool) {
    cover_over(layers, layers.len(), content, view, size, &std::collections::HashSet::new())
}

/// As `cover`, with the layers from `behind` on used only where those
/// before them leave holes, and out of date ones of textures in `shown`,
/// the last frame's, first of those out of date
fn cover_over(layers: &[Layer], behind: usize, content: Option<Rect>, view: &View, size: (u32, u32), shown: &std::collections::HashSet<usize>) -> (Vec<Piece>, bool) {
    if view.outline().is_some() {
        return (Vec::new(), false);
    }
    let px = view.px(size.0);
    let (drawn, preview, _, _) = view.drawn(size);
    let (ox, oy) = (view.x * px as f64, view.y * px as f64);
    let (rx, ry) = (ox.round() as i32, oy.round() as i32);
    // Figma draws from where the view is, which can be a fraction of a
    // pixel, and the browser stretches its canvas to the pixels it's
    // snapped to, so only where neither happens do tiles of this zoom land
    // on whole pixels
    let (sx, sy) = view.stretch(size);
    let ((fx, px_x), (fy, px_y)) = (phase(ox), phase(oy));
    let at_phase = |layer: &Layer| (layer.phase.0 - px_x).abs() < 2e-3 && (layer.phase.1 - px_y).abs() < 2e-3;
    // One coarser than the view is scaled up, blurring it, so one as much
    // finer comes first
    let distance = |at: f32| {
        let octaves = (at / drawn).log2();
        if octaves < 0.0 { -octaves * 2.0 } else { octaves }
    };
    let mut ranked: Vec<(f32, usize)> = layers
        .iter()
        .enumerate()
        .map(|(i, layer)| {
            // One drawn from the fraction of a pixel the view is at is
            // sharper there than one moved to it
            let off = if !preview && layer.px == drawn && !at_phase(layer) { 1.1 } else { 0.0 };
            let rank = match (layer.kind, layer.px == drawn) {
                (LayerKind::Tile, true) => off,
                (LayerKind::Whole, true) => off + 1.0,
                (LayerKind::Draft, true) => off + 1.2,
                (LayerKind::Whole, false) => 1.5 + distance(layer.px),
                // One drawn from where the view is stays till the next is,
                // over tiles moved to it, which sample what's fine in it,
                // as custom effects draw, otherwise, so it'd change back
                // and forth as each change came
                (LayerKind::Stale, true) if !preview && at_phase(layer) && (px_x, px_y) != (0.0, 0.0) => 1.05,
                // Shown only till what changed is drawn again, so where it
                // lands on the grid doesn't matter. What the last frame
                // showed first: another, older or never shown, would show
                // the view other than it just was for a frame
                (LayerKind::Stale, _) => (if shown.contains(&(Arc::as_ptr(&layer.tex) as usize)) { 1.3 } else { 1.6 }) + distance(layer.px),
                // Under all up to date, however coarse
                (LayerKind::Old, _) => 100.0 + distance(layer.px),
                _ => 2.0 + distance(layer.px),
            };
            (if i >= behind { rank + 1000.0 } else { rank }, i)
        })
        .collect();
    // Of those ranked the same, the one drawn last first: a tile out of date
    // since before the whole view was drawn would show it as it was before
    // whatever came since, an image that came showing and going again
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0).then(layers[b.1].drawn.cmp(&layers[a.1].drawn)));
    let mut left = skia_safe::Region::new();
    left.set_rect(skia_safe::IRect::from_wh(size.0 as i32, size.1 as i32));
    if let Some(content) = content {
        let at = Rect::from_ltrb(
            ((content.left as f64 * px as f64 - ox) * sx) as f32,
            ((content.top as f64 * px as f64 - oy) * sy) as f32,
            ((content.right as f64 * px as f64 - ox) * sx) as f32,
            ((content.bottom as f64 * px as f64 - oy) * sy) as f32,
        );
        let at = skia_safe::IRect::from_ltrb(at.left.floor() as i32 - 1, at.top.floor() as i32 - 1, at.right.ceil() as i32 + 1, at.bottom.ceil() as i32 + 1);
        left.op_rect(at, skia_safe::region::RegionOp::Intersect);
    }
    let place = |layer: &Layer| {
        let same = layer.px == px;
        let unstretched = sx == 1.0 && sy == 1.0;
        // A preview's pixels stay squares however the browser stretches its
        // canvas, which at a display scale like 120% is by a pixel or two
        // and would otherwise blur them all
        let square = (same && unstretched && at_phase(layer)) || (preview && layer.px == drawn);
        let dest = if same && square {
            Rect::from_xywh((layer.x - fx) as f32, (layer.y - fy) as f32, layer.tex.width as f32, layer.tex.height as f32)
        } else {
            let scale = (px / layer.px) as f64;
            // A preview's squares are placed from the whole pixel the view
            // lands on, as Figma places them
            let (ox, oy) = if preview && layer.px == drawn { (rx as f64, ry as f64) } else { (ox, oy) };
            // Each edge from where it is on the grid, so neighbors' meet
            // exactly, as one's right edge worked out from its width can
            // round to another pixel than the next one's left, and leave a
            // column of neither
            let (lx, ly) = (layer.phase.0 as f64, layer.phase.1 as f64);
            let edge = |at: i32, phase: f64, o: f64, stretch: f64| (((at as f64 + phase) * scale - o) * stretch) as f32;
            Rect::from_ltrb(
                edge(layer.x, lx, ox, sx),
                edge(layer.y, ly, oy, sy),
                edge(layer.x + layer.tex.width, lx, ox, sx),
                edge(layer.y + layer.tex.height, ly, oy, sy),
            )
        };
        (dest, square)
    };
    let mut pieces = Vec::new();
    // Each pixel goes to the first layer over its center, then any left,
    // as where layers of other zooms or phases meet, to the first over any
    // of it, which `paint` fills with its edge
    for outward in [false, true] {
        for &(_, i) in &ranked {
            if left.is_empty() {
                break;
            }
            let (dest, square) = place(&layers[i]);
            let covers = if outward { skia_safe::IRect::from_ltrb(dest.left.floor() as i32, dest.top.floor() as i32, dest.right.ceil() as i32, dest.bottom.ceil() as i32) } else { dest.round() };
            let mut clip = skia_safe::Region::new();
            clip.set_rect(covers);
            if !clip.op_region(&left, skia_safe::region::RegionOp::Intersect) {
                continue;
            }
            left.op_rect(covers, skia_safe::region::RegionOp::Difference);
            pieces.push(Piece { layer: i, dest, clip, square });
        }
    }
    (pieces, !left.is_empty())
}

/// What draws Figma's checkerboard and pixel grid
pub struct Patterns {
    checker: Option<skia_safe::RuntimeEffect>,
    grid: Option<(skia_safe::RuntimeEffect, skia_safe::RuntimeEffect)>,
}

impl Patterns {
    pub fn new() -> Self {
        Patterns {
            checker: skia_safe::RuntimeEffect::make_for_shader(CHECKER, None).map_err(|error| log::warn!("native renderer: checker: {error}")).ok(),
            grid: skia_safe::RuntimeEffect::make_for_shader(GRID_LINES, None)
                .and_then(|lines| skia_safe::RuntimeEffect::make_for_blender(GRID_COLOR, None).map(|color| (lines, color)))
                .map_err(|error| log::warn!("native renderer: grid: {error}"))
                .ok(),
        }
    }

    /// Draws a frame of `view` onto `canvas`, `size` pixels: the page's
    /// background, `pieces` of `snapshot`, `images` its layers' as the
    /// canvas's context's images, and the pixel grid. Returns what the
    /// layers of this zoom drew
    pub fn paint(&self, canvas: &skia_safe::Canvas, snapshot: &Snapshot, images: &[Option<Image>], pieces: &[Piece], view: &View, size: (u32, u32)) -> Counts {
        let px = view.px(size.0);
        // Figma places its checkerboard and pixel grid from where the view
        // is, unrounded
        let origin = (view.x * px as f64, view.y * px as f64);
        // Outline mode shows the editor's background alone, Figma's overlay
        // drawing the outlines
        let outline = view.outline();
        canvas.clear(outline.unwrap_or(snapshot.background));
        if outline.is_some() {
            return Counts::default();
        }
        // What Figma draws in its canvas, as the browser stretches it
        let (sx, sy) = view.stretch(size);
        if snapshot.transparent {
            if let Some(paint) = self.checker.as_ref().and_then(|checker| checker_paint(checker, snapshot.dark, origin, px, view.zoom)) {
                canvas.save();
                canvas.scale((sx as f32, sy as f32));
                canvas.draw_paint(&paint);
                canvas.restore();
            }
        }
        let (drawn, ..) = view.drawn(size);
        let mut counts = Counts::default();
        for piece in pieces {
            let Some(image) = images.get(piece.layer).and_then(Option::as_ref) else {
                continue;
            };
            let layer = &snapshot.layers[piece.layer];
            // A layer can be in two pieces
            let first = !pieces.iter().take_while(|other| !std::ptr::eq(*other, piece)).any(|other| other.layer == piece.layer);
            if first && layer.px == drawn && !matches!(layer.kind, LayerKind::Draft | LayerKind::Stale | LayerKind::Old) {
                counts += layer.counts;
            }
            // Every pixel of its clip filled, rect by rect, the image's edge
            // carried on past it, as a pixel whose center is on the edge of
            // where it's drawn is left out of it, and showed through a
            // column of the background between tiles
            let filter = if piece.square { FilterMode::Nearest } else { FilterMode::Linear };
            let to_dest = Matrix::rect_2_rect(Rect::from_wh(image.width() as f32, image.height() as f32), piece.dest, None).unwrap_or_default();
            let Some(shader) = image.to_shader((TileMode::Clamp, TileMode::Clamp), SamplingOptions::new(filter, MipmapMode::None), &to_dest) else {
                continue;
            };
            let mut paint = Paint::default();
            paint.set_shader(shader);
            for rect in skia_safe::region::Iterator::new(&piece.clip) {
                canvas.draw_irect(rect, &paint);
            }
        }
        if view.grid {
            if let Some(paint) = self.grid.as_ref().and_then(|grid| grid_paint(grid, origin, px)) {
                canvas.save();
                canvas.scale((sx as f32, sy as f32));
                canvas.draw_rect(Rect::from_wh((size.0 as f64 / sx) as f32, (size.1 as f64 / sy) as f32), &paint);
                canvas.restore();
            }
        }
        counts
    }
}

/// Where frames go: a window of their own, or a layer of the main window's
/// under its webviews, where the tab shows it through its canvas
pub enum Output {
    Window(isize),
    Layer(isize),
}

/// The layer under the webviews, and where it is
struct Composed {
    device: IDCompositionDesktopDevice,
    _target: IDCompositionTarget,
    visual: IDCompositionVisual2,
    /// Where it is
    at: Option<(i32, i32)>,
}

pub struct Screen {
    context: DirectContext,
    layer: Option<Composed>,
    swap_chain: IDXGISwapChain3,
    /// Signaled when the swap chain takes another frame, so each frame is
    /// drawn as late as it can be, with the view as it is then
    waitable: HANDLE,
    buffers: Vec<(Surface, BackendRenderTarget)>,
    size: (u32, u32),
    /// The render thread's textures as this context's images
    images: std::collections::HashMap<usize, (Arc<Tex>, Image)>,
    patterns: Patterns,
    queue: ID3D12CommandQueue,
    /// Passes each frame's number once the GPU has drawn it
    fence: ID3D12Fence,
    frames: u64,
    /// Textures frames read, held till the GPU's drawn them, so the render
    /// thread draws into none the GPU's still reading here
    reading: std::collections::VecDeque<(u64, Vec<Arc<Tex>>)>,
    /// The textures the last frame showed
    shown: std::collections::HashSet<usize>,
    /// For Show stats: the oldest frame of Figma's not shown yet, as those
    /// passed over while the render thread was behind count too, the last
    /// one shown, and when
    owed: Option<Made>,
    timed: u64,
    timed_at: f64,
}

impl Screen {
    pub fn new(device: &Device, output: Output, width: u32, height: u32) -> Result<Self, String> {
        let queue_desc = D3D12_COMMAND_QUEUE_DESC { Priority: D3D12_COMMAND_QUEUE_PRIORITY_HIGH.0, ..Default::default() };
        let queue = unsafe { device.device.CreateCommandQueue(&queue_desc) }.map_err(|error| format!("queue: {error}"))?;
        let fence: ID3D12Fence = unsafe { device.device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }.map_err(|error| format!("fence: {error}"))?;
        let backend = BackendContext {
            adapter: device.adapter.clone(),
            device: device.device.clone(),
            memory_allocator: Some(super::committed::allocator(&device.device, &queue)),
            queue: queue.clone(),
            protected_context: Protected::No,
        };
        let context = unsafe { gpu::direct_contexts::make_d3d(&backend, None) }.ok_or("Skia couldn't start on D3D12")?;
        let desc = |composition: bool| DXGI_SWAP_CHAIN_DESC1 {
            Width: width.max(1),
            Height: height.max(1),
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Scaling: if composition { DXGI_SCALING_STRETCH } else { DXGI_SCALING_NONE },
            AlphaMode: if composition { DXGI_ALPHA_MODE_IGNORE } else { DXGI_ALPHA_MODE_UNSPECIFIED },
            Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
            ..Default::default()
        };
        let (swap_chain, layer): (IDXGISwapChain3, Option<Composed>) = match output {
            Output::Window(hwnd) => {
                let chain: IDXGISwapChain1 = unsafe { device.factory.CreateSwapChainForHwnd(&backend.queue, HWND(hwnd as *mut _), &desc(false), None, None) }
                    .map_err(|error| format!("swap chain: {error}"))?;
                (chain.cast().map_err(|error| format!("swap chain: {error}"))?, None)
            }
            Output::Layer(hwnd) => {
                let factory: IDXGIFactory2 = device.factory.cast().map_err(|error| error.to_string())?;
                let chain: IDXGISwapChain1 = unsafe { factory.CreateSwapChainForComposition(&backend.queue, &desc(true), None) }
                    .map_err(|error| format!("swap chain: {error}"))?;
                // Behind the window's webviews, not over them
                let layer = unsafe {
                    let composition: IDCompositionDesktopDevice = DCompositionCreateDevice2(None).map_err(|error| format!("composition: {error}"))?;
                    let target = composition.CreateTargetForHwnd(HWND(hwnd as *mut _), false).map_err(|error| format!("composition target: {error}"))?;
                    let visual = composition.CreateVisual().map_err(|error| format!("composition visual: {error}"))?;
                    target.SetRoot(&visual).map_err(|error| format!("composition root: {error}"))?;
                    composition.Commit().map_err(|error| format!("composition: {error}"))?;
                    Composed { device: composition, _target: target, visual, at: None }
                };
                let chain3: IDXGISwapChain3 = chain.cast().map_err(|error| format!("swap chain: {error}"))?;
                unsafe { layer.visual.SetContent(&chain3) }.map_err(|error| format!("composition content: {error}"))?;
                (chain3, Some(layer))
            }
        };
        let waitable = unsafe {
            let chain: IDXGISwapChain2 = swap_chain.cast().map_err(|error| error.to_string())?;
            let _ = chain.SetMaximumFrameLatency(1);
            chain.GetFrameLatencyWaitableObject()
        };
        let mut screen = Screen {
            context,
            layer,
            swap_chain,
            waitable,
            buffers: Vec::new(),
            size: (width.max(1), height.max(1)),
            images: std::collections::HashMap::new(),
            patterns: Patterns::new(),
            shown: std::collections::HashSet::new(),
            queue,
            fence,
            frames: 0,
            reading: std::collections::VecDeque::new(),
            owed: None,
            timed: 0,
            timed_at: 0.0,
        };
        screen.wrap_buffers()?;
        screen.warm();
        Ok(screen)
    }

    /// Draws once what frames draw, so the GPU programs for it are made now
    /// rather than in the middle of the first frames that need them
    fn warm(&mut self) {
        let info = skia_safe::ImageInfo::new((64, 64), ColorType::RGBA8888, AlphaType::Premul, None);
        let Some(mut target) = gpu::surfaces::render_target(&mut self.context, gpu::Budgeted::Yes, &info, 1, SurfaceOrigin::TopLeft, None, false, None) else {
            return;
        };
        let Some(mut source) = gpu::surfaces::render_target(&mut self.context, gpu::Budgeted::Yes, &info, 1, SurfaceOrigin::TopLeft, None, false, None) else {
            return;
        };
        source.canvas().clear(Color4f::new(0.5, 0.5, 0.5, 0.5));
        let image = source.image_snapshot();
        let canvas = target.canvas();
        let mut clip = skia_safe::Region::new();
        clip.set_rect(skia_safe::IRect::from_wh(40, 40));
        clip.op_rect(skia_safe::IRect::from_xywh(10, 10, 10, 10), skia_safe::region::RegionOp::Difference);
        for filter in [FilterMode::Nearest, FilterMode::Linear] {
            for clipped in [false, true] {
                canvas.save();
                if clipped {
                    if let Some(rect) = std::iter::Iterator::next(&mut skia_safe::region::Iterator::new(&clip)) {
                        canvas.clip_irect(rect, skia_safe::ClipOp::Intersect);
                    }
                }
                canvas.draw_image_rect_with_sampling_options(&image, None, Rect::from_xywh(0.5, 0.5, 50.0, 50.0), SamplingOptions::new(filter, MipmapMode::None), &Paint::default());
                canvas.restore();
            }
        }
        canvas.draw_image(&image, (0, 0), Some(&copy()));
        if let Some(paint) = self.patterns.checker.as_ref().and_then(|checker| checker_paint(checker, false, (0.0, 0.0), 1.0, 1.0)) {
            canvas.draw_paint(&paint);
        }
        if let Some(paint) = self.patterns.grid.as_ref().and_then(|grid| grid_paint(grid, (0.0, 0.0), 10.0)) {
            canvas.draw_paint(&paint);
        }
        let _ = self.context.flush_submit_and_sync_cpu();
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
                    sample_quality_pattern: 0,
                    protected: Protected::No,
                },
            );
            let surface = gpu::surfaces::wrap_backend_render_target(&mut self.context, &target, SurfaceOrigin::TopLeft, ColorType::RGBA8888, None, None)
                .ok_or("couldn't wrap the swap chain")?;
            self.buffers.push((surface, target));
        }
        Ok(())
    }

    fn resize(&mut self, (width, height): (u32, u32)) -> Result<(), String> {
        self.buffers.clear();
        let _ = self.context.flush_submit_and_sync_cpu();
        // Skia may still hold the old buffers in its cache
        self.context.free_gpu_resources();
        self.size = (width.max(1), height.max(1));
        unsafe {
            self.swap_chain.ResizeBuffers(0, self.size.0, self.size.1, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0))
        }
        .map_err(|error| format!("resize: {error}"))?;
        self.wrap_buffers()
    }

    pub fn run(mut self, hub: Arc<Hub>) {
        drawing();
        // The snapshot frames are composed of: the latest the GPU has drawn
        let mut current: Option<Arc<Snapshot>> = None;
        // The last snapshot shown whole: while the latest has holes, the
        // view moves on over this, all of it as it was a moment before,
        // rather than standing still till the render thread fills them
        let mut complete: Option<Arc<Snapshot>> = None;
        // What's shown: the snapshot, the view, what's over it and the size
        let mut shown: Option<(u64, View, Option<u32>, (u32, u32))> = None;
        let mut seen = 0;
        // Whether the swap chain has room for a frame already waited for
        let mut room = false;
        // Since when the view's waited for what it needs drawn
        let mut held: Option<Instant> = None;
        while !hub.stop.load(Ordering::Relaxed) {
            if !room {
                unsafe { WaitForSingleObjectEx(self.waitable, 250, true) };
                room = true;
            }
            let latest = hub.snapshot.lock().unwrap().clone();
            if let Some(latest) = latest.filter(|latest| current.as_ref().is_none_or(|current| (current.serial, current.seq) != (latest.serial, latest.seq)) && latest.drawn()) {
                current = Some(latest);
            }
            let picked = current.as_ref().ok_or(None).and_then(|snapshot| hub.pick(snapshot.changes, self.timed).map(|picked| (snapshot.clone(), picked)));
            let (snapshot, Picked { view, overlay, seq, oldest, wait }) = match picked {
                Ok(picked) => picked,
                Err(wait) => {
                    // The render thread pokes once there's more, and the GPU
                    // has drawn it
                    seen = hub.wait(seen, wait.unwrap_or(Duration::from_millis(100)).clamp(Duration::from_millis(1), Duration::from_millis(100)));
                    continue;
                }
            };
            if let Some(oldest) = oldest.filter(|oldest| self.owed.is_none_or(|owed| oldest.at < owed.at)) {
                self.owed = Some(oldest);
            }
            let size = match &self.layer {
                Some(_) => view.canvas(),
                None => *hub.size.lock().unwrap(),
            };
            if size != self.size && size.0 > 0 && size.1 > 0 {
                if let Err(error) = self.resize(size) {
                    log::warn!("native renderer: {error}");
                }
                shown = None;
            }
            if self.layer.is_some() {
                // Where the browser puts Figma's canvas: at the nearest
                // device pixel, a pixel of it to a pixel of the screen
                let at = (*hub.origin.lock().unwrap()).filter(|_| view.visible).map(|(x, y)| (x + (view.cx * view.dpr).round() as i32, y + (view.cy * view.dpr).round() as i32));
                self.place(at);
            }
            let now = (snapshot.serial, view.clone(), overlay.as_ref().map(|picture| picture.unique_id()), self.size);
            if shown.as_ref() == Some(&now) {
                // Nothing to show that isn't on screen already, so it's
                // not waiting for anything
                if self.owed.take().is_some() {
                    self.timed = seq;
                }
                drop(snapshot);
                seen = hub.wait(seen, wait.unwrap_or(Duration::from_millis(100)).clamp(Duration::from_millis(1), Duration::from_millis(100)));
                continue;
            }
            // All of the view or nothing: what's shown stays till the
            // render thread has drawn what the view needs. Its whole view
            // is drawn for the view itself, so it's what there is
            let (pieces, holes) = cover_over(&snapshot.layers, snapshot.layers.len(), snapshot.content, &view, self.size, &self.shown);
            if holes && snapshot.whole_at != Some(view_at(&view, self.size)) {
                stats().lock().unwrap().holes(true);
                // Its holes filled from the last snapshot shown whole, as
                // it was a moment before, so the view never stands still
                // for what's being drawn again
                let merged = complete.as_ref().filter(|before| before.serial != snapshot.serial).map(|before| {
                    let mut layers = snapshot.layers.clone();
                    layers.extend(before.layers.iter().cloned());
                    let (pieces, holes) = cover_over(&layers, snapshot.layers.len(), snapshot.content, &view, self.size, &self.shown);
                    let merged = Snapshot {
                        seq: snapshot.seq,
                        changes: snapshot.changes,
                        fence: snapshot.fence.clone(),
                        value: snapshot.value,
                        serial: snapshot.serial,
                        layers,
                        content: snapshot.content,
                        whole_at: snapshot.whole_at,
                        background: snapshot.background,
                        transparent: snapshot.transparent,
                        dark: snapshot.dark,
                        pending: snapshot.pending,
                    };
                    (merged, pieces, holes)
                });
                // Waited for too long, it's shown with what there is and
                // the background where there's nothing yet, filled in as
                // it's drawn, as a view standing still reads as frozen
                let waited = held.get_or_insert_with(Instant::now).elapsed();
                let late = waited >= HOLD_MOST;
                let (merged, partial) = match merged {
                    Some((merged, pieces, false)) => (Some((merged, pieces)), None),
                    // Something of it, as nothing at all would hide Figma's
                    // own canvas, before the first frame's drawn
                    Some((merged, pieces, true)) => (None, (late && !pieces.is_empty()).then_some((Some(merged), pieces))),
                    None => (None, (late && !pieces.is_empty()).then_some((None, pieces))),
                };
                if let Some((partial, pieces)) = partial {
                    hub.shown_seq.store(seq, Ordering::Relaxed);
                    PATH.store(3, Ordering::Relaxed);
                    self.frame(partial.as_ref().unwrap_or(&snapshot), &pieces, &view, overlay.as_ref(), seq);
                    room = false;
                    shown = Some(now);
                    continue;
                }
                if let Some((merged, pieces)) = merged {
                    held = None;
                    hub.shown_seq.store(seq, Ordering::Relaxed);
                    PATH.store(1, Ordering::Relaxed);
                    self.frame(&merged, &pieces, &view, overlay.as_ref(), seq);
                    room = false;
                    shown = Some(now);
                    continue;
                }
                let before = complete.clone().filter(|before| before.serial != snapshot.serial).and_then(|before| {
                    let (pieces, holes) = cover_over(&before.layers, before.layers.len(), before.content, &view, self.size, &self.shown);
                    let then = (before.serial, view.clone(), overlay.as_ref().map(|picture| picture.unique_id()), self.size);
                    (!holes && shown.as_ref() != Some(&then)).then_some((before, pieces, then))
                });
                let drawn_seq = snapshot.seq;
                let snapshot_levels = {
                    let mut counted: Vec<(f32, LayerKind, usize)> = Vec::new();
                    for layer in &snapshot.layers {
                        match counted.iter_mut().find(|(px, kind, _)| *px == layer.px && *kind == layer.kind) {
                            Some(entry) => entry.2 += 1,
                            None => counted.push((layer.px, layer.kind, 1)),
                        }
                    }
                    counted
                };
                drop(snapshot);
                let Some((before, pieces, then)) = before else {
                    super::render::trace(|| {
                        let mut levels: Vec<String> = snapshot_levels.iter().map(|(px, kind, n)| format!("{px:.4}{kind:?}x{n}")).collect();
                        levels.sort();
                        format!("present held: seq={seq} px={:.3} drawn={drawn_seq} layers={}", view.px(self.size.0), levels.join(","))
                    });
                    let left = HOLD_MOST.saturating_sub(held.map_or(Duration::ZERO, |at| at.elapsed()));
                    seen = hub.wait(seen, left.max(Duration::from_millis(1)).min(Duration::from_millis(100)));
                    continue;
                };
                hub.shown_seq.store(seq, Ordering::Relaxed);
                PATH.store(2, Ordering::Relaxed);
                self.frame(&before, &pieces, &view, overlay.as_ref(), seq);
                room = false;
                shown = Some(then);
                continue;
            }
            held = None;
            hub.shown_seq.store(seq, Ordering::Relaxed);
            PATH.store(0, Ordering::Relaxed);
            self.frame(&snapshot, &pieces, &view, overlay.as_ref(), seq);
            room = false;
            shown = Some(now);
            complete = Some(snapshot);
        }
        drop((current, complete));
        let _ = self.context.flush_submit_and_sync_cpu();
        drop(self);
        done();
    }

    /// Moves the layer to `at` in the window, or hides it
    fn place(&mut self, at: Option<(i32, i32)>) {
        let Some(layer) = &mut self.layer else {
            return;
        };
        if layer.at == at {
            return;
        }
        layer.at = at;
        unsafe {
            match at {
                Some((x, y)) => {
                    let _ = layer.visual.SetOffsetX2(x as f32);
                    let _ = layer.visual.SetOffsetY2(y as f32);
                    let _ = layer.visual.SetBitmapInterpolationMode(DCOMPOSITION_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR);
                    let _ = layer.visual.SetContent(&self.swap_chain);
                }
                None => {
                    let _ = layer.visual.SetContent(None::<&windows::core::IUnknown>);
                }
            }
            let _ = layer.device.Commit();
        }
    }

    fn image(&mut self, tex: &Arc<Tex>) -> Option<Image> {
        let key = Arc::as_ptr(tex) as usize;
        if let Some((_, image)) = self.images.get(&key) {
            return Some(image.clone());
        }
        let image = gpu::images::borrow_texture_from(&mut self.context, &tex.backend(), SurfaceOrigin::TopLeft, ColorType::RGBA8888, AlphaType::Premul, None)?;
        self.images.insert(key, (tex.clone(), image.clone()));
        Some(image)
    }

    /// Shows `pieces` of `snapshot` at `view`, and what Figma draws over it
    /// in frame `seq`
    fn frame(&mut self, snapshot: &Snapshot, pieces: &[Piece], view: &View, overlay: Option<&skia_safe::Picture>, seq: u64) {
        let start = Instant::now();
        let index = unsafe { self.swap_chain.GetCurrentBackBufferIndex() } as usize;
        if index >= self.buffers.len() {
            return;
        }
        let mut images: Vec<Option<Image>> = vec![None; snapshot.layers.len()];
        for piece in pieces {
            images[piece.layer] = self.image(&snapshot.layers[piece.layer].tex);
        }
        let size = self.size;
        let (surface, _) = &mut self.buffers[index];
        let canvas = surface.canvas();
        let counts = self.patterns.paint(canvas, snapshot, &images, pieces, view, size);
        record(&mut self.context, &self.patterns, snapshot, &images, pieces, view, size, seq);
        if let Some(overlay) = overlay {
            let (sx, sy) = view.stretch(size);
            canvas.save();
            canvas.scale((sx as f32, sy as f32));
            canvas.draw_picture(overlay, None, None);
            canvas.restore();
        }
        let encoded = start.elapsed();
        // Left ready to present, and what it reads held till the GPU's done
        if let Err(error) = self.context.flush_surface_with_access(surface, skia_safe::surface::BackendSurfaceAccess::Present, &gpu::FlushInfo::default()) {
            log::warn!("native renderer: {error:?}");
        }
        self.context.submit(SyncCpu::No);
        drop(images);
        self.frames += 1;
        let read: Vec<Arc<Tex>> = pieces.iter().map(|piece| snapshot.layers[piece.layer].tex.clone()).collect();
        self.shown = read.iter().map(|tex| Arc::as_ptr(tex) as usize).collect();
        if unsafe { self.queue.Signal(&self.fence, self.frames) }.is_err() {
            // Without the fence, waited for instead
            self.context.submit(SyncCpu::Yes);
        } else {
            self.reading.push_back((self.frames, read));
        }
        let done = unsafe { self.fence.GetCompletedValue() };
        while self.reading.front().is_some_and(|(frame, _)| *frame <= done) {
            self.reading.pop_front();
        }
        let rendered = start.elapsed();
        let _ = unsafe { self.swap_chain.Present(1, DXGI_PRESENT(0)) };
        let presented = super::render::since_epoch_ms();
        // Textures of snapshots before, let go of
        let kept: std::collections::HashSet<usize> = snapshot.layers.iter().map(|layer| Arc::as_ptr(&layer.tex) as usize).collect();
        self.images.retain(|key, _| kept.contains(key));
        let px = view.px(size.0);
        super::render::trace(|| {
            // How much of the view's from layers drawn coarser than it, and
            // how coarse the coarsest
            let area = (size.0 * size.1).max(1) as f32;
            let coarse: f32 = pieces.iter().filter(|piece| snapshot.layers[piece.layer].px < px * 0.99).flat_map(|piece| skia_safe::region::Iterator::new(&piece.clip).map(|rect| (rect.width() * rect.height()) as f32).collect::<Vec<_>>()).sum();
            // Which layers it's from, by how sharp and how much of the view
            let mut used: Vec<(f32, LayerKind, f32)> = Vec::new();
            for piece in pieces {
                let layer = &snapshot.layers[piece.layer];
                let shown: f32 = skia_safe::region::Iterator::new(&piece.clip).map(|rect| (rect.width() * rect.height()) as f32).sum();
                match used.iter_mut().find(|(px, kind, _)| *px == layer.px && *kind == layer.kind) {
                    Some(entry) => entry.2 += shown,
                    None => used.push((layer.px, layer.kind, shown)),
                }
            }
            let used: Vec<String> = used.iter().filter(|(_, _, shown)| *shown > 0.0).map(|(layer_px, kind, shown)| format!("{:.2}{kind:?}{:.0}%", layer_px / px, shown / area * 100.0)).collect();
            let coarsest = pieces.iter().filter(|piece| !piece.clip.is_empty()).map(|piece| snapshot.layers[piece.layer].px / px).fold(1.0f32, f32::min);
            format!(
                "present seq={seq} at={presented:.2} px={px:.3} layers={} encoded={:.1} rendered={:.1} holes=false x={} y={} placed={:?} drawn={} coarse={:.2} coarsest={coarsest:.2} serial={} path={} used={}",
                pieces.len(),
                encoded.as_secs_f64() * 1000.0,
                rendered.as_secs_f64() * 1000.0,
                (view.x * px as f64).round(),
                (view.y * px as f64).round(),
                self.layer.as_ref().and_then(|layer| layer.at),
                snapshot.seq,
                (coarse / area).min(1.0),
                snapshot.serial,
                PATH.load(Ordering::Relaxed),
                used.join(",")
            )
        });
        // Once a frame of Figma's, not again as what's drawn under it gets
        // sharper: from when the oldest frame not shown till now was sent.
        // While it was waiting at the last one shown, the time since counts
        // to the frame rate however long, as that's a stutter, not the view
        // at rest
        if let Some(owed) = self.owed.take().filter(|_| seq != self.timed) {
            let late = (presented - owed.at).max(0.0);
            if late > 50.0 {
                super::render::trace(|| format!("shown late: seq={seq} {late:.1}ms after Figma sent the oldest frame waiting"));
            }
            super::hud::presented(owed.figma, late as f32, owed.at < self.timed_at);
            self.timed = seq;
            self.timed_at = presented;
        }
        let mut stats = stats().lock().unwrap();
        stats.holes(false);
        stats.record_frame(encoded.as_secs_f64() * 1000.0, rendered.as_secs_f64() * 1000.0, counts.native, counts.approximate, counts.skipped, view.zoom, snapshot.pending);
    }
}

/// How the frame being shown was put together, for `record`: the latest
/// snapshot, it merged with the last whole one, or the last whole one
static PATH: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Dev tools: with LIGMA_NATIVE_RECORD set to a folder, each frame shown is
/// saved there a quarter the size, as raw RGBA named by its order, snapshot
/// and time, for finding flicker too quick to see in captures
#[allow(clippy::too_many_arguments)]
fn record(context: &mut DirectContext, patterns: &Patterns, snapshot: &Snapshot, images: &[Option<Image>], pieces: &[Piece], view: &View, size: (u32, u32), seq: u64) {
    static DIR: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();
    static FRAME: AtomicU64 = AtomicU64::new(0);
    let Some(dir) = DIR.get_or_init(|| std::env::var_os("LIGMA_NATIVE_RECORD").filter(|_| super::dev()).map(std::path::PathBuf::from)) else {
        return;
    };
    let (w, h) = (size.0.div_ceil(4), size.1.div_ceil(4));
    let info = skia_safe::ImageInfo::new((w as i32, h as i32), ColorType::RGBA8888, AlphaType::Premul, None);
    let Some(mut small) = gpu::surfaces::render_target(context, gpu::Budgeted::No, &info, 1, SurfaceOrigin::TopLeft, None, false, None) else {
        return;
    };
    small.canvas().scale((0.25, 0.25));
    patterns.paint(small.canvas(), snapshot, images, pieces, view, size);
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    if small.canvas().read_pixels(&info, &mut pixels, (w * 4) as usize, (0, 0)) {
        let n = FRAME.fetch_add(1, Ordering::Relaxed);
        let _ = std::fs::create_dir_all(dir);
        let path = ["new", "merged", "held"][PATH.load(Ordering::Relaxed) as usize % 3];
        super::render::trace(|| {
            let pieces: Vec<String> = pieces
                .iter()
                .map(|piece| {
                    let layer = &snapshot.layers[piece.layer];
                    let b = piece.clip.bounds();
                    format!("{:?}@{:.4}#{} {}x{}+{}+{}", layer.kind, layer.px, layer.drawn, b.width(), b.height(), b.left, b.top)
                })
                .collect();
            format!("recorded {n} {path} serial={} pieces={}", snapshot.serial, pieces.join(" "))
        });
        let _ = std::fs::write(dir.join(format!("{n:05}_{seq}_{:.0}_{w}x{h}_{path}_s{}_l{}_p{}.rgba", super::render::since_epoch_ms(), snapshot.serial, snapshot.layers.len(), pieces.len())), pixels);
    }
}

/// Paints over what's there rather than on it
fn copy() -> Paint {
    let mut paint = Paint::default();
    paint.set_blend_mode(skia_safe::BlendMode::Src);
    paint
}

pub(super) fn checker_paint(effect: &skia_safe::RuntimeEffect, dark: bool, origin: (f64, f64), px: f32, zoom: f64) -> Option<Paint> {
    let cell = 8.0 / 2f64.powf(zoom.max(1e-6).log2().round()) as f32;
    let (a, b) = if dark { (0.12f32, 0.17f32) } else { (1.0, 0.95) };
    // Within one repeat, which keeps it exact in 32 bits far out
    let period = 2.0 * cell as f64 * px as f64;
    let origin = (origin.0.rem_euclid(period) as f32, origin.1.rem_euclid(period) as f32);
    let shader = effect.make_shader(uniforms(&[origin.0, origin.1, px, cell, a, b]), &[], None)?;
    let mut paint = Paint::default();
    paint.set_shader(shader);
    Some(paint)
}

pub(super) fn grid_paint((lines, color): &(skia_safe::RuntimeEffect, skia_safe::RuntimeEffect), origin: (f64, f64), px: f32) -> Option<Paint> {
    if px < 8.0 {
        return None;
    }
    let origin = (origin.0.rem_euclid(px as f64) as f32, origin.1.rem_euclid(px as f64) as f32);
    let shader = lines.make_shader(uniforms(&[origin.0, origin.1, px]), &[], None)?;
    let blender = color.make_blender(Data::new_empty(), None)?;
    let mut paint = Paint::default();
    paint.set_shader(shader);
    paint.set_blender(blender);
    Some(paint)
}

/// Packs floats as a runtime effect's uniforms
pub(super) fn uniforms(values: &[f32]) -> Data {
    let bytes: Vec<u8> = values.iter().flat_map(|value| value.to_le_bytes()).collect();
    Data::new_copy(&bytes)
}

/// The checkerboard at device pixel `p`: canvas point `(p + origin) / px`,
/// squares `cell` units wide, `a` where the cell's x and y differ in parity
pub(super) const CHECKER: &str = "
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
pub(super) const GRID_LINES: &str = "
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
pub(super) const GRID_COLOR: &str = "
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
