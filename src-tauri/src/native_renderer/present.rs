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
    /// The tab it's drawn from, shown only while that tab is
    pub tab: Option<Arc<str>>,
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
    /// Drawn in place of Figma's canvas, under the tab: the active Figma
    /// tab, and where its webview is in the window, in pixels
    pub origin: Mutex<Option<(String, (i32, i32))>>,
    pub stop: AtomicBool,
    /// Frames of the render tree that couldn't be applied in a row
    pub tree_failures: std::sync::atomic::AtomicU32,
    /// Memory shared with the tab drawn
    pub shared: Mutex<Option<crate::webview2::Shared>>,
    /// Frames of the render tree from the one shown now on
    frames: Mutex<std::collections::VecDeque<FrameView>>,
    /// The frame last shown, for tracing
    pub shown_seq: AtomicU64,
    /// The tab the last frame shown whole was drawn from
    pub whole_tab: Mutex<Option<Arc<str>>>,
    /// To the render thread, for the view shown ahead of Figma's frames
    pub messages: Mutex<Option<std::sync::mpsc::Sender<super::render::Msg>>>,
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
            whole_tab: Mutex::new(None),
            messages: Mutex::new(None),
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
    pub fn frame_view(&self, seq: u64, view: View, overlay: Overlay, edits: bool, made: Option<Made>, sent: f64) {
        let mut frames = self.frames.lock().unwrap();
        // Lost track: those before go
        if frames.back().is_some_and(|last| last.seq >= seq) {
            frames.clear();
        }
        frames.push_back(FrameView { seq, view: view.clone(), overlay, edits, made, came: Instant::now(), sent });
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
            return self.view.lock().unwrap().clone().map(|view| Picked { view, overlay: None, seq, oldest: None, wait: None, came: None, sent: 0.0 }).ok_or(None);
        }
        let mut take = None;
        let mut wait = None;
        let latest = frames.back().map(|frame| frame.view.clone());
        for (i, frame) in frames.iter().enumerate() {
            // One that changes the scene waits for it to be drawn, so what
            // moves with the view moves with it, but not for long, as the
            // view standing still reads as a stutter. Not while the view
            // moves, as Figma sends nodes again as it pans and zooms, and
            // what's drawn moves with the view either way
            let waited = frame.came.elapsed();
            let still = Some(&frame.view) == latest.as_ref() && (i == 0 || frames[i - 1].view == frame.view);
            if frame.edits && still && frame.seq > seq && waited < EDIT_HOLD {
                wait = Some(EDIT_HOLD - waited);
                break;
            }
            take = Some(i);
        }
        let take = take.ok_or(wait)?;
        let oldest = frames.iter().take(take + 1).filter(|frame| frame.seq > shown).find_map(|frame| frame.made);
        frames.drain(..take);
        let frame = frames.front().ok_or(wait)?;
        Ok(Picked { view: frame.view.clone(), overlay: frame.overlay.clone(), seq: frame.seq, oldest, wait, came: Some(frame.came), sent: frame.sent })
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
    /// When Figma sent it, in ms since the epoch
    sent: f64,
}

struct Picked {
    view: View,
    overlay: Overlay,
    seq: u64,
    oldest: Option<Made>,
    /// How long till a frame after it that changes the scene is shown
    /// without waiting for it
    wait: Option<Duration>,
    /// When it came and when Figma sent it, for frames between Figma's
    came: Option<Instant>,
    sent: f64,
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
    let (pieces, hole) = cover_over(layers, layers.len(), content, view, size, &std::collections::HashSet::new());
    (pieces, hole.is_some())
}

/// As `cover`, with the layers from `behind` on used only where those
/// before them leave holes, and out of date ones of textures in `shown`,
/// the last frame's, first of those out of date
fn cover_over(layers: &[Layer], behind: usize, content: Option<Rect>, view: &View, size: (u32, u32), shown: &std::collections::HashSet<usize>) -> (Vec<Piece>, Option<skia_safe::IRect>) {
    if view.outline().is_some() {
        return (Vec::new(), None);
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
    (pieces, (!left.is_empty()).then(|| *left.bounds()))
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
    /// The window frames go to
    window: isize,
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
    /// Where the view's headed between Figma's frames (see `ahead`)
    pan: Pan,
    /// How far ahead of Figma's last frame the frame being drawn is, in
    /// device pixels, which what Figma draws over the scene moves by too
    ahead_px: (f32, f32),
    ahead_scale: f32,
    /// The view the render thread was told the wheel's zoomed to last
    told: Option<View>,
}

/// The view's pan between Figma's frames. Figma sends one on fewer than
/// every refresh while the view moves, 50 to 80 a second on a 100 Hz
/// screen, so frames in between show the view moved on from the last.
/// While a mouse drags the canvas the view is where the pointer puts it:
/// Figma keeps the point under the pointer under it, so the view's corner
/// and the pointer, in device pixels, add up to the same all drag long
/// (`Drag`). Figma takes 10 to 40 ms to show a move, so where the pointer
/// is shows the view as Figma will once it catches up. Otherwise, as with a
/// wheel, it's moved on as it's moving, by about a frame of Figma's. It's
/// never taken back from where it's shown while the view moves, and once
/// it stops it's eased back to where Figma left it
#[derive(Default)]
struct Pan {
    /// The last frame of Figma's seen: its seq
    last: Option<u64>,
    /// Figma's recent frames of the same zoom and canvas: when each was
    /// sent and its view
    recent: std::collections::VecDeque<(f64, View)>,
    /// Units a millisecond, and the milliseconds between Figma's frames
    velocity: Option<(f64, f64, f64)>,
    drag: Option<Drag>,
    /// Figma's last few frames while the wheel turns: when each was sent
    /// and the view's corner in device pixels
    wheeled: std::collections::VecDeque<(f64, (f64, f64))>,
    wheel: Option<Wheel>,
    /// Figma's frame before the last: when it was sent and the view's
    /// corner, in device pixels
    before: Option<(f64, (f64, f64))>,
    /// How far ahead of Figma's last frame the view was shown last, in
    /// units, and since when it's been eased back from there, both from the
    /// view's corner in that frame
    lead: (f64, f64),
    back: Option<((f64, f64), Instant)>,
    base: Option<(f64, f64)>,
    /// The window frames go to, and where Figma's canvas is in it, in
    /// device pixels: left, top, width and height
    canvas: Option<(isize, (i32, i32, i32, i32))>,
    /// When the buttons went down last that a drag was worked out from
    tried: Option<f64>,
    /// Figma's recent frames as the zoom changes: when each was sent and
    /// its view
    zooms: std::collections::VecDeque<(f64, View)>,
    zooming: Option<Zooming>,
    /// How much further the zoom's shown than Figma's last frame, as the
    /// log of the ratio, since when it's been eased back from there, and
    /// the log of the zoom of the frame it's from
    zoom_lead: f64,
    zoom_back: Option<(f64, Instant)>,
    zoom_base: Option<f64>,
    /// Where the zoom's about, in CSS pixels from the canvas's corner
    anchor: (f64, f64),
    /// Figma's last frames: when each was sent and its view
    history: std::collections::VecDeque<(f64, View)>,
    wheel_zoom: Option<WheelZoom>,
}

/// The wheel zooming, turned with Ctrl held. Each turn's shown the moment
/// it's made, as Figma would, from Figma's last frame, as its page can take
/// hundreds of milliseconds to show them while it's busy
struct WheelZoom {
    /// Figma's view before the first turn
    base: View,
    /// The turns since: their `ZOOM_TURNS` serial, when, how far, and the
    /// point the zoom's about, in CSS pixels from the canvas's corner
    turns: Vec<(u64, f64, f64, (f64, f64))>,
    /// How many of them Figma's last frame shows
    shown: usize,
    /// Figma's frames didn't zoom as the wheel did, so they're shown as
    /// they come
    lost: bool,
}

/// The zoom changing as Figma's frames show it: the log of its ratio a
/// millisecond, and the milliseconds between frames
#[derive(Clone, Copy)]
struct Zooming {
    rate: f64,
    gap: f64,
}

/// The wheel panning the view: the view's corner, in device pixels, is
/// `at` plus `per` times how far the wheel's turned, across and down, as
/// Figma took it a moment before each frame
struct Wheel {
    at: (f64, f64),
    per: (f64, f64),
    /// How far what's shown is from where the wheel puts the view, in
    /// device pixels, taken up only as the wheel turns, and where the wheel
    /// put it last
    off: Option<(f64, f64)>,
    exact: Option<(f64, f64)>,
}

/// A drag of the canvas with a button held, the view's corner and the
/// pointer, in device pixels, adding up to `to`
struct Drag {
    to: (f64, f64),
    /// How far what's shown is from where the pointer puts the view, in
    /// device pixels, taken up only as the pointer moves, so the view never
    /// moves on its own
    off: Option<(f64, f64)>,
    /// Pressed with what pans the canvas whatever's under the pointer, the
    /// middle button or the left with space held, so followed from the
    /// start, and when
    sure: Option<f64>,
    /// Figma's frames that moved the view as the pointer would, and those
    /// since in a row that didn't
    agreed: u32,
    missed: u32,
    /// The corner as Figma's last frame had it
    corner: (f64, f64),
    /// Where the pointer was when what's shown was worked out last
    pointer: Option<(f64, f64)>,
    /// What the last frame with the pointer still a while added up to, as
    /// one may yet be a move behind
    rested: Option<(f64, f64)>,
}

/// Figma's frames over this many milliseconds give the view's velocity, as
/// one to the next moves unevenly: Figma takes in input in batches, and
/// sends some frames in twos
const VELOCITY_OVER: f64 = 60.0;

/// Furthest ahead of Figma's last frame the view is moved without a pointer
/// to follow, in milliseconds, and how much of the time between Figma's
/// last two it covers, so the view keeps moving till the next is due
const AHEAD_MOST: f64 = 20.0;
const AHEAD_OF_GAP: f64 = 1.0;
/// How long after Figma's last frame it's held there, through the gaps of
/// 100 ms or so Figma leaves while the view moves, and then how long it's
/// taken back over, in milliseconds
const HOLD_MS: f64 = 120.0;
const BACK_MS: f64 = 60.0;
/// Figma's frames over this many milliseconds give how the zoom changes,
/// and how far, in device pixels, each may be from where the point it's
/// about puts it
const ZOOM_OVER: f64 = 80.0;
const ZOOM_FIT: f64 = 1.5;
/// Figma's least and most zoom
const ZOOM_LEAST: f64 = 0.02;
const ZOOM_MOST: f64 = 256.0;
/// How long the wheel's still before a turn starts another zoom, how long
/// after the last turn Figma's frames show all of it, and how long it's
/// shown without, in milliseconds
const ZOOM_QUIET: f64 = 300.0;
const ZOOM_SETTLED: f64 = 100.0;
const ZOOM_GIVE_UP: f64 = 2000.0;
/// How far Figma's zoom may be from where the wheel put it, as the log of
/// the ratio, to show those turns
const ZOOM_MATCH: f64 = 0.01;
/// Figma's frames kept for the view before the wheel turned
const HISTORY: usize = 32;

/// How long after the pointer moves Figma's frames show it, at most, and
/// how long before, as the page's clock may be a little behind, in
/// milliseconds
const DRAG_LATE: f64 = 100.0;
const DRAG_EARLY: f64 = 15.0;
/// How far, in device pixels, a frame of Figma's may be from where the
/// pointer put the view, a moment before, to agree
const DRAG_NEAR: f64 = 3.0;
/// Frames of Figma's that agree before the view follows the pointer, and
/// that don't before it stops
const DRAG_AGREED: u32 = 2;
const DRAG_MISSED: u32 = 3;
/// How long the pointer's still before Figma's frames show it where it is,
/// in milliseconds
const DRAG_REST: f64 = 80.0;
/// How much of how far the pointer moves, or the wheel, what's shown is
/// brought to where they put the view by
const DRAG_TAKE_UP: f64 = 0.5;
/// How long a drag followed from the start goes with the pointer moving
/// and no frame of Figma's moving the view before it's let go, in ms
const DRAG_UNSURE: f64 = 120.0;

impl Pan {
    /// The view to show for frame `seq` of Figma's, sent at `sent`, in ms
    /// since the epoch, and come `age` ms ago, with how far it's moved on
    /// from `view`, in units, `px` device pixels each
    fn ahead(&mut self, seq: u64, sent: f64, age: f64, view: &View, px: f64) -> Option<(View, (f64, f64))> {
        if self.last != Some(seq) {
            self.last = Some(seq);
            self.took(sent, view, px);
        }
        if let Some(zoomed) = self.wheel_zoom(sent, view) {
            self.lead = (0.0, 0.0);
            self.back = None;
            self.base = None;
            self.zoom_lead = 0.0;
            self.zoom_back = None;
            self.zoom_base = None;
            return zoomed;
        }
        if self.zooming.is_some() || self.zoom_lead != 0.0 || self.zoom_back.is_some() {
            return self.zoom_ahead(age, view);
        }
        self.zoom_base = None;
        // From the corner of the frame it's shown over now, so it shows
        // the same as Figma's frames come
        if let Some(base) = self.base {
            let by = (view.x - base.0, view.y - base.1);
            // Where Figma's frame has gone past what was shown, it's shown
            // as it is, rather than what's shown brought to it after
            let past = |lead: f64, by: f64| if (lead - by) * by < 0.0 { 0.0 } else { lead - by };
            self.lead = (past(self.lead.0, by.0), past(self.lead.1, by.1));
            if let Some((from, _)) = &mut self.back {
                *from = (past(from.0, by.0), past(from.1, by.1));
            }
        }
        self.base = Some((view.x, view.y));
        let lead = match self.target(age, view, px) {
            Some(target) => {
                self.back = None;
                target
            }
            // Eased back from where it was shown, not at once
            None if self.lead != (0.0, 0.0) || self.back.is_some() => {
                let (from, since) = *self.back.get_or_insert((self.lead, Instant::now()));
                let left = 1.0 - since.elapsed().as_secs_f64() * 1000.0 / BACK_MS;
                if left <= 0.0 {
                    self.back = None;
                    (0.0, 0.0)
                } else {
                    (from.0 * left, from.1 * left)
                }
            }
            None => (0.0, 0.0),
        };
        self.lead = lead;
        if lead == (0.0, 0.0) {
            return None;
        }
        let mut moved = view.clone();
        moved.x += lead.0;
        moved.y += lead.1;
        Some((moved, lead))
    }

    /// What's shown was Figma's own frame after all, where moved on from it
    /// would show what isn't drawn yet
    fn not_ahead(&mut self) {
        self.lead = (0.0, 0.0);
        self.back = None;
        self.zoom_lead = 0.0;
        self.zoom_back = None;
        if let Some(drag) = &mut self.drag {
            drag.off = None;
        }
        if let Some(wheel) = &mut self.wheel {
            wheel.off = None;
        }
    }

    /// Whether the view follows the pointer
    fn following(&self) -> bool {
        // Pressed with what pans the canvas, a frame of Figma's moving the
        // view as the pointer would is enough. Not none, as Figma pans
        // no further than its limits, nor at all zoomed out all the way
        self.drag.as_ref().is_some_and(|drag| drag.agreed >= if drag.sure.is_some() { 1 } else { DRAG_AGREED })
    }

    /// Takes in a new frame of Figma's
    fn took(&mut self, sent: f64, view: &View, px: f64) {
        sample_pointer();
        watch_wheel();
        let same = |before: &View| before.zoom == view.zoom && before.w == view.w && before.h == view.h && before.dpr == view.dpr && before.cx == view.cx && before.cy == view.cy && before.cw == view.cw && before.ch == view.ch && before.page == view.page && before.preview == view.preview;
        if self.recent.back().is_some_and(|(_, before)| !same(before)) {
            self.drag = None;
            self.before = None;
            self.wheeled.clear();
            self.wheel = None;
            // What's shown is of another zoom, so it's not eased back from
            self.lead = (0.0, 0.0);
            self.back = None;
            self.base = None;
        }
        if self.recent.back().is_some_and(|(then, before)| !same(before) || sent - then > VELOCITY_OVER) {
            self.recent.clear();
        }
        // Sent with the one before, so of the same input
        if self.recent.back().is_some_and(|(then, _)| sent - then < 1.0) {
            self.recent.pop_back();
        }
        self.recent.push_back((sent, view.clone()));
        while self.recent.front().is_some_and(|(then, _)| sent - then > VELOCITY_OVER) {
            self.recent.pop_front();
        }
        self.velocity = match (self.recent.front(), self.recent.len()) {
            (Some((then, first)), n) if n >= 3 && sent - then >= 15.0 => {
                let span = sent - then;
                let (vx, vy) = ((view.x - first.x) / span, (view.y - first.y) / span);
                (vx != 0.0 || vy != 0.0).then_some((vx, vy, span / (n - 1) as f64))
            }
            _ => None,
        };
        let corner = (view.x * px, view.y * px);
        self.dragged(sent, corner);
        self.before = Some((sent, corner));
        self.wheeled(sent, corner);
        self.zoomed(sent, view);
        self.history.push_back((sent, view.clone()));
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
    }

    /// The view to show for Figma's frame `view`, sent at `sent`, as the
    /// wheel zooms it, none while it doesn't
    fn wheel_zoom(&mut self, sent: f64, view: &View) -> Option<Option<(View, (f64, f64))>> {
        let now = super::render::since_epoch_ms();
        let turns: Vec<(u64, f64, f64, (f64, f64))> = ZOOM_TURNS.lock().unwrap().iter().copied().collect();
        if self.wheel_zoom.is_none() {
            let last = turns.last()?;
            if now - last.1 > ZOOM_QUIET || self.following() {
                return None;
            }
            // From the first turn after the wheel was still a while
            let mut start = turns.len() - 1;
            while start > 0 && turns[start].1 - turns[start - 1].1 <= ZOOM_QUIET {
                start -= 1;
            }
            let first = turns[start].1;
            // Figma's frame before it, which was the view then however
            // long before it was sent
            let Some((_, base)) = self.history.iter().rev().find(|(then, _)| *then <= first) else {
                super::render::trace(|| "wheel zoom not followed: no frame of Figma's before it".to_string());
                return None;
            };
            self.wheel_zoom = Some(WheelZoom { base: base.clone(), turns: Vec::new(), shown: 0, lost: false });
        }
        let step = zoom_step();
        let canvas = self.canvas;
        let zoom = self.wheel_zoom.as_mut().unwrap();
        let after = zoom.turns.last().map(|turn| turn.0);
        let from = turns.iter().rposition(|turn| after.is_some_and(|after| turn.0 <= after)).map_or_else(
            || {
                let mut start = turns.len().saturating_sub(1);
                while start > 0 && turns[start].1 - turns[start - 1].1 <= ZOOM_QUIET {
                    start -= 1;
                }
                start
            },
            |i| i + 1,
        );
        let dpr = view.dpr;
        let same = |other: &View| other.dpr == view.dpr && other.cx == view.cx && other.cy == view.cy && other.cw == view.cw && other.ch == view.ch && other.page == view.page && other.preview == view.preview;
        if !zoom.lost && !same(&zoom.base) {
            zoom.lost = true;
            super::render::trace(|| "wheel zoom not followed: the canvas changed".to_string());
        }
        for &(serial, when, delta, at) in turns.iter().skip(from) {
            if zoom.lost {
                break;
            }
            // Where the pointer was on the canvas, as Figma zooms about it
            let about = canvas.and_then(|(window, canvas)| canvas_point(window, canvas, at)).map(|(x, y)| (x / dpr, y / dpr)).filter(|(x, y)| (0.0..view.cw).contains(x) && (0.0..view.ch).contains(y));
            match about {
                Some(about) => zoom.turns.push((serial, when, delta, about)),
                None => {
                    zoom.lost = true;
                    super::render::trace(|| format!("wheel zoom not followed: turned at {:.0},{:.0}, off the canvas", at.0, at.1));
                }
            }
        }
        // Which of the turns Figma's frame shows, by its zoom
        let zooms = zoom.turns.iter().fold(vec![zoom.base.zoom], |mut zooms, (_, _, delta, _)| {
            let last = *zooms.last().unwrap();
            zooms.push((last * (step * delta).exp()).clamp(ZOOM_LEAST, ZOOM_MOST));
            zooms
        });
        if !zoom.lost {
            let have = view.zoom.ln();
            let (best, off) = (zoom.shown..zooms.len()).map(|i| (i, (zooms[i].ln() - have).abs())).min_by(|a, b| a.1.total_cmp(&b.1)).unwrap_or((zoom.shown, f64::INFINITY));
            if off <= ZOOM_MATCH {
                if best != zoom.shown {
                    let (n, shown) = (zoom.turns.len(), best);
                    super::render::trace(|| format!("wheel zoom: Figma shows {shown} of {n} turns, {:.0}ms after the last", sent - zoom.turns[shown - 1].1));
                }
                zoom.shown = best;
            } else {
                zoom.lost = true;
                let (base, turned) = (zoom.base.zoom, zoom.turns.iter().map(|turn| turn.2).sum::<f64>());
                super::render::trace(|| format!("wheel zoom not followed: Figma's at {:.4} from {base:.4}, {turned} turned at {step:.6} a step", view.zoom));
            }
        }
        let last = zoom.turns.last().map_or(now, |turn| turn.1);
        let quiet = now - last > ZOOM_QUIET;
        // Done: Figma's shown all of it, or doesn't zoom as the wheel did,
        // or hasn't shown it for too long
        if quiet && (zoom.lost || zoom.shown == zoom.turns.len()) || now - last > ZOOM_GIVE_UP {
            // How far a step zooms, from how far Figma zoomed once it had
            // taken all of it in
            let turned: f64 = zoom.turns.iter().map(|turn| turn.2).sum();
            let clamped = |z: f64| z <= ZOOM_LEAST * 1.01 || z >= ZOOM_MOST / 1.01;
            let seen = zooms.iter().any(|z| clamped(*z)) || clamped(view.zoom);
            if sent - last > ZOOM_SETTLED && turned.abs() >= 1.0 && !seen && same(&zoom.base) {
                let found = (view.zoom / zoom.base.zoom).ln() / turned;
                if found > ZOOM_STEP_GUESS / 8.0 && found < ZOOM_STEP_GUESS * 8.0 {
                    let was = ZOOM_STEP.swap(found.to_bits(), Ordering::Relaxed);
                    if was != found.to_bits() {
                        super::render::trace(|| format!("wheel zoom step {found:.6}, was {:.6}", if was == 0 { ZOOM_STEP_GUESS } else { f64::from_bits(was) }));
                    }
                }
            }
            self.wheel_zoom = None;
            return None;
        }
        if zoom.lost {
            return None;
        }
        Some(wheel_zoomed(view, &zoom.turns[zoom.shown..], step))
    }

    /// The wheel's zoom with only the first `n` turns Figma's frame `view`
    /// doesn't show yet, for where what's drawn covers no more
    fn wheel_zoom_fewer(&self, view: &View, n: usize) -> Option<(View, (f64, f64))> {
        let zoom = self.wheel_zoom.as_ref().filter(|zoom| !zoom.lost)?;
        let left = &zoom.turns[zoom.shown..];
        wheel_zoomed(view, &left[..n.min(left.len())], zoom_step())
    }

    /// How many turns of the wheel's zoom Figma's frames don't show yet
    fn wheel_zoom_left(&self) -> usize {
        self.wheel_zoom.as_ref().filter(|zoom| !zoom.lost).map_or(0, |zoom| zoom.turns.len() - zoom.shown)
    }

    /// Takes in a frame of Figma's for the zoom changing
    fn zoomed(&mut self, sent: f64, view: &View) {
        let same = |before: &View| before.dpr == view.dpr && before.cx == view.cx && before.cy == view.cy && before.cw == view.cw && before.ch == view.ch && before.page == view.page && before.preview == view.preview;
        if self.zooms.back().is_some_and(|(then, before)| !same(before) || sent - then > ZOOM_OVER) {
            self.zooms.clear();
        }
        if self.zooms.back().is_some_and(|(then, _)| sent - then < 1.0) {
            self.zooms.pop_back();
        }
        self.zooms.push_back((sent, view.clone()));
        while self.zooms.front().is_some_and(|(then, _)| sent - then > ZOOM_OVER) {
            self.zooms.pop_front();
        }
        // Only since it last turned from zooming in to out or back, as
        // where it was headed before isn't where it's headed now
        let step = |(a, b): (&(f64, View), &(f64, View))| (b.1.zoom - a.1.zoom).signum();
        let last = self.zooms.iter().zip(self.zooms.iter().skip(1)).map(step).filter(|sign| *sign != 0.0).last();
        if let Some(last) = last {
            while self.zooms.len() > 1 && self.zooms.iter().zip(self.zooms.iter().skip(1)).map(step).any(|sign| sign != 0.0 && sign != last) {
                self.zooms.pop_front();
            }
        }
        self.zooming = None;
        let (Some((then, first)), n) = (self.zooms.front(), self.zooms.len()) else { return };
        let span = sent - then;
        if n < 3 || span < 15.0 || first.zoom == view.zoom || self.following() {
            return;
        }
        // Figma zooms about a point that stays where it is: from the first
        // and the last, checked against each between
        let inverse = 1.0 / first.zoom - 1.0 / view.zoom;
        let anchor = ((view.x - first.x) / inverse, (view.y - first.y) / inverse);
        let about = |frame: &View| (frame.x + anchor.0 / frame.zoom, frame.y + anchor.1 / frame.zoom);
        let world = about(view);
        let off = self.zooms.iter().map(|(_, frame)| {
            let at = about(frame);
            ((at.0 - world.0) * frame.zoom * frame.dpr).abs().max(((at.1 - world.1) * frame.zoom * frame.dpr).abs())
        });
        let off = off.fold(0.0, f64::max);
        if off > ZOOM_FIT {
            super::render::trace(|| format!("zoom not followed: {n} frames {off:.1}px off about {:.1},{:.1}", anchor.0, anchor.1));
            return;
        }
        self.anchor = anchor;
        self.zooming = Some(Zooming { rate: (view.zoom / first.zoom).ln() / span, gap: span / (n - 1) as f64 });
    }

    /// The view to show for Figma's frame `view`, come `age` ms ago, as the
    /// zoom changes: moved on as it's changing, or eased back to the frame
    /// once it's stopped, and how far its corner's moved, in units
    fn zoom_ahead(&mut self, age: f64, view: &View) -> Option<(View, (f64, f64))> {
        let log = view.zoom.ln();
        // From the zoom of the frame it's shown over now, as with a pan
        if let Some(base) = self.zoom_base {
            let by = log - base;
            let past = |lead: f64| if (lead - by) * by < 0.0 { 0.0 } else { lead - by };
            self.zoom_lead = past(self.zoom_lead);
            if let Some((from, _)) = &mut self.zoom_back {
                *from = past(*from);
            }
        }
        self.zoom_base = Some(log);
        let lead = match self.zooming.filter(|_| age <= HOLD_MS) {
            Some(zooming) => {
                self.zoom_back = None;
                let most = (zooming.gap * AHEAD_OF_GAP).min(AHEAD_MOST);
                let to = zooming.rate * age.min(most);
                let shown = self.zoom_lead;
                // The wheel's stopped, so it's zoomed no further, only held
                // where it's shown till Figma's frames catch up. Otherwise
                // not back from where it's shown, as Figma's next frame often
                // changes it less than the one before
                if wheel_stopped() {
                    if shown * zooming.rate > 0.0 { shown } else { 0.0 }
                } else if shown * zooming.rate > 0.0 && shown.abs() > to.abs() {
                    shown.abs().min(zooming.rate.abs() * AHEAD_MOST * 2.0).copysign(shown)
                } else {
                    to
                }
            }
            None if self.zoom_lead != 0.0 || self.zoom_back.is_some() => {
                let (from, since) = *self.zoom_back.get_or_insert((self.zoom_lead, Instant::now()));
                let left = 1.0 - since.elapsed().as_secs_f64() * 1000.0 / BACK_MS;
                if left <= 0.0 {
                    self.zoom_back = None;
                    0.0
                } else {
                    from * left
                }
            }
            None => 0.0,
        };
        self.zoom_lead = lead;
        if lead == 0.0 {
            return None;
        }
        let zoom = view.zoom * lead.exp();
        let (ax, ay) = self.anchor;
        let mut moved = view.clone();
        moved.x = view.x + ax / view.zoom - ax / zoom;
        moved.y = view.y + ay / view.zoom - ay / zoom;
        moved.w = view.w * view.zoom / zoom;
        moved.h = view.h * view.zoom / zoom;
        moved.zoom = zoom;
        let by = (moved.x - view.x, moved.y - view.y);
        Some((moved, by))
    }

    /// Takes in a frame of Figma's, with the view's corner at `corner`, for
    /// the wheel panning the view
    fn wheeled(&mut self, sent: f64, corner: (f64, f64)) {
        let turns = wheel_turns();
        if self.following() || turns.last().is_none_or(|(when, ..)| sent - when > WHEEL_OVER) {
            self.wheeled.clear();
            self.wheel = None;
            return;
        }
        self.wheeled.push_back((sent, corner));
        while self.wheeled.len() > WHEEL_FRAMES || self.wheeled.front().is_some_and(|(then, _)| sent - then > WHEEL_OVER) {
            self.wheeled.pop_front();
        }
        let fit = fit_wheel(&self.wheeled, &turns);
        self.wheel = match (fit, self.wheel.take()) {
            (Some((at, per)), Some(was)) => Some(Wheel { at, per, ..was }),
            (Some((at, per)), None) => Some(Wheel { at, per, off: None, exact: None }),
            (None, _) => None,
        };
    }

    /// Takes in a frame of Figma's, with the view's corner at `corner`, for
    /// a drag of the canvas
    fn dragged(&mut self, sent: f64, corner: (f64, f64)) {
        let samples: Vec<(f64, f64, f64, u8)> = {
            let pointer = POINTER.lock().unwrap();
            pointer.iter().copied().collect()
        };
        let held = samples.iter().rev().take_while(|(when, ..)| sent - when < 30.0).any(|(.., held)| held & BUTTONS != 0) || samples.last().is_some_and(|(.., held)| held & BUTTONS != 0);
        if !held {
            self.drag = None;
            return;
        }
        // Where the pointer was at times `from` to `to`, and just before
        let between = |from: f64, to: f64| {
            let start = samples.partition_point(|(when, ..)| *when < from).saturating_sub(1);
            samples[start..].iter().take_while(move |(when, ..)| *when <= to).map(|(_, x, y, _)| (*x, *y))
        };
        // Whether the pointer was still for `DRAG_REST` ms before `at`, and
        // where
        let rested = |at: f64| {
            let mut points = between(at - DRAG_REST, at);
            let first = points.next()?;
            points.all(|point| (point.0 - first.0).abs() <= 1.0 && (point.1 - first.1).abs() <= 1.0).then_some(first)
        };
        match &mut self.drag {
            Some(drag) => {
                let moved = corner != drag.corner;
                drag.corner = corner;
                if moved && drag.sure.is_some() {
                    drag.agreed = drag.agreed.max(1);
                }
                // Figma's frame has the view where the pointer put it at
                // some moment before it was sent
                let agrees = passed(between(sent - DRAG_LATE, sent + DRAG_EARLY), (drag.to.0 - corner.0, drag.to.1 - corner.1));
                if agrees {
                    drag.missed = 0;
                    if moved {
                        drag.agreed += 1;
                    }
                } else {
                    drag.missed += 1;
                    let (to, missed) = (drag.to, drag.missed);
                    super::render::trace(|| format!("drag missed {missed}: corner {:.1},{:.1} to {:.1},{:.1} sent={sent:.1}", corner.0, corner.1, to.0, to.1));
                    if missed >= DRAG_MISSED {
                        self.drag = None;
                        return;
                    }
                }
                // Still a while, so Figma's caught up with the pointer: where
                // it adds up to exactly, once two frames say so
                match rested(sent) {
                    Some(point) => {
                        let to = (corner.0 + point.0, corner.1 + point.1);
                        let again = drag.rested.is_some_and(|was| (was.0 - to.0).abs() <= 0.5 && (was.1 - to.1).abs() <= 0.5);
                        if again && to != drag.to {
                            let was = drag.to;
                            super::render::trace(|| format!("drag rested: to {:.1},{:.1} was {:.1},{:.1}", to.0, to.1, was.0, was.1));
                            // What's shown stays where it is, and comes to
                            // where it should be as the pointer moves
                            if let Some(off) = &mut drag.off {
                                *off = (off.0 + was.0 - to.0, off.1 + was.1 - to.1);
                            }
                            drag.to = to;
                        }
                        drag.rested = Some(to);
                    }
                    None => drag.rested = None,
                }
            }
            None => {
                // The button went down, and the pointer hasn't moved since:
                // the view's corner then and the pointer add up to what
                // they will all drag long, should it drag the canvas
                let pressed = samples.iter().rposition(|(.., held)| held & BUTTONS == 0).map_or(0, |i| i + 1);
                let Some(&(at, x, y, _)) = samples.get(pressed) else { return };
                let still = samples[pressed..].iter().take_while(|(when, ..)| *when <= sent).all(|(_, px, py, _)| (px - x).abs() <= 1.0 && (py - y).abs() <= 1.0);
                // From this frame, or, should Figma have sent none from the
                // button going down till the pointer moved, the one before
                let from = if at <= sent && still { Some(corner) } else { self.before.filter(|(then, _)| *then <= at && pressed > 0).map(|(_, corner)| corner) };
                if let Some(from) = from {
                    // Pressed with what pans the canvas whatever's there, so
                    // followed at once
                    let held = samples[pressed].3;
                    let pans = held & MIDDLE != 0 || held & (LEFT | SPACE) == LEFT | SPACE;
                    let sure = (pans && self.canvas.is_some_and(|(window, canvas)| on_canvas(window, canvas, (x, y)))).then_some(at);
                    let mut drag = Drag { to: (from.0 + x, from.1 + y), off: None, sure, agreed: 0, missed: 0, corner: from, pointer: None, rested: None };
                    if from != corner {
                        // This frame's the first to say whether the canvas is
                        // dragged
                        let agrees = passed(between(sent - DRAG_LATE, sent + DRAG_EARLY), (drag.to.0 - corner.0, drag.to.1 - corner.1));
                        if !agrees {
                            return;
                        }
                        drag.agreed = 1;
                        drag.corner = corner;
                    }
                    let (to, agreed, sure) = (drag.to, drag.agreed, drag.sure.is_some());
                    super::render::trace(|| format!("drag from {:.1},{:.1} pressed at {at:.1} sent={sent:.1}: to {:.1},{:.1} agreed={agreed} sure={sure}", from.0, from.1, to.0, to.1));
                    self.drag = Some(drag);
                }
            }
        }
    }

    /// The middle button's gone down on the canvas, or the left with space
    /// held, which pans it whatever's there: the view's corner and the
    /// pointer then add up to what they will all drag long
    fn pressed(&mut self, view: &View, px: f64) {
        let Some((window, canvas)) = self.canvas else { return };
        let Some((at, x, y, held)) = pointer_pressed() else { return };
        if self.tried == Some(at) {
            return;
        }
        if held & MIDDLE == 0 && held & (LEFT | SPACE) != LEFT | SPACE {
            return;
        }
        // Figma's frame shows the view as it was at the press: its last,
        // with the view still a while by then
        let Some((sent, corner)) = self.before else { return };
        if sent > at || self.velocity.is_some() && at - sent < VELOCITY_OVER || corner != (view.x * px, view.y * px) {
            let moving = self.velocity.is_some();
            super::render::trace(|| format!("drag press not followed: frame sent {:.1} after it, moving={moving}", sent - at));
            self.tried = Some(at);
            return;
        }
        self.tried = Some(at);
        if !on_canvas(window, canvas, (x, y)) {
            super::render::trace(|| "drag press not followed: not on the canvas".to_string());
            return;
        }
        let to = (corner.0 + x, corner.1 + y);
        super::render::trace(|| format!("drag pressed at {at:.1}: to {:.1},{:.1}", to.0, to.1));
        self.drag = Some(Drag { to, off: None, sure: Some(at), agreed: 0, missed: 0, corner, pointer: None, rested: None });
    }

    /// How far ahead of Figma's last frame to show the view now, none once
    /// the view's not moving
    fn target(&mut self, age: f64, view: &View, px: f64) -> Option<(f64, f64)> {
        if self.drag.is_none() {
            self.pressed(view, px);
        }
        if let Some(since) = self.drag.as_ref().filter(|drag| drag.agreed == 0).and_then(|drag| drag.sure) {
            // Followed from the start, but the pointer's gone on moving
            // with Figma moving the view none, so it's not a drag of the
            // canvas after all
            if pointer_moved_since(since).is_some_and(|moved| super::render::since_epoch_ms() - moved > DRAG_UNSURE) {
                super::render::trace(|| "drag unsure: let go".to_string());
                self.drag = None;
            }
        }
        if self.following() {
            // The button's up and Figma's sent nothing since, so the view's
            // stopped where it was
            if age > HOLD_MS && !pointer_held() {
                return None;
            }
            // Where the pointer is, or was as the button went up, as Figma
            // takes in that last move too
            let (x, y) = pointer_now()?;
            let drag = self.drag.as_mut()?;
            // Figma's last frame didn't move the view as the pointer would,
            // as at the edge of where it pans: held where it's shown, rather
            // than gone on past where Figma stops
            if drag.missed > 0 {
                drag.pointer = Some((x, y));
                drag.off = None;
                return Some(self.lead);
            }
            let exact = (drag.to.0 - x, drag.to.1 - y);
            let moved = drag.pointer.map_or((0.0, 0.0), |was| (x - was.0, y - was.1));
            // Brought to where the pointer puts the view only as the pointer
            // moves, by part of that, so the view never moves on its own or
            // back against the pointer
            let take_up = |off: f64, moved: f64| off - off.signum() * off.abs().min(moved.abs() * DRAG_TAKE_UP);
            let off = match drag.off {
                Some(off) => (take_up(off.0, moved.0), take_up(off.1, moved.1)),
                // From where it's shown now, not at once
                None => (view.x * px + self.lead.0 * px - exact.0, view.y * px + self.lead.1 * px - exact.1),
            };
            drag.off = Some(off);
            drag.pointer = Some((x, y));
            let lead = ((exact.0 + off.0) / px - view.x, (exact.1 + off.1) / px - view.y);
            // No further than the view's width, as the pointer jumping that
            // far is something else
            let pixels = (lead.0 * px).hypot(lead.1 * px);
            return (pixels < view.w * px).then_some(lead);
        }
        // A button's held but the view's not following the pointer, or was
        // a moment ago, as a drag that's let go stops where it is: Figma's
        // frames as they come, rather than moved on as if it'll go on
        if pointer_held_within(RELEASED_MS) {
            return None;
        }
        if let Some(wheel) = &mut self.wheel {
            let turns = wheel_turns();
            if age > HOLD_MS && turns.last().is_none_or(|(when, ..)| super::render::since_epoch_ms() - when > WHEEL_OVER) {
                return None;
            }
            let turned = wheel_turned(&turns, f64::INFINITY);
            let exact = (wheel.at.0 + wheel.per.0 * turned.0, wheel.at.1 + wheel.per.1 * turned.1);
            // Where the wheel puts the view, as soon as it turns, as Figma
            // moves it a step at a time too. What's shown is brought there
            // only as the wheel turns, by part of each step, so the view
            // never moves on its own or back against the wheel
            let moved = wheel.exact.map_or((0.0, 0.0), |was| (exact.0 - was.0, exact.1 - was.1));
            let take_up = |off: f64, moved: f64| off - off.signum() * off.abs().min(moved.abs() * DRAG_TAKE_UP);
            let off = match wheel.off {
                Some(off) => (take_up(off.0, moved.0), take_up(off.1, moved.1)),
                // From where it's shown now, not at once
                None => (view.x * px + self.lead.0 * px - exact.0, view.y * px + self.lead.1 * px - exact.1),
            };
            wheel.off = Some(off);
            wheel.exact = Some(exact);
            let shown = (exact.0 + off.0, exact.1 + off.1);
            let lead = (shown.0 / px - view.x, shown.1 / px - view.y);
            let pixels = (lead.0 * px).hypot(lead.1 * px);
            return (pixels < view.w * px).then_some(lead);
        }
        let (vx, vy, dt) = self.velocity?;
        let most = (dt * AHEAD_OF_GAP).min(AHEAD_MOST);
        if age > HOLD_MS {
            return None;
        }
        // The wheel's stopped, so the view's moved on no further, only held
        // where it's shown till Figma's frames catch up, rather than taken
        // back from past where they'll stop
        if wheel_stopped() {
            let hold = |lead: f64, v: f64| if lead * v > 0.0 { lead } else { 0.0 };
            return Some((hold(self.lead.0, vx), hold(self.lead.1, vy)));
        }
        let by = age.min(most);
        // Not back from where it's shown, along where it's headed, as Figma's
        // next frame often moves less than the one before
        let keep = |d: f64, v: f64, shown: f64| if v != 0.0 && shown * v.signum() > d * v.signum() { shown.min(v.abs() * AHEAD_MOST * 2.0) } else { d };
        Some((keep(vx * by, vx, self.lead.0), keep(vy * by, vy, self.lead.1)))
    }

    /// How long till what's shown changes with no new frame of Figma's,
    /// none once it's back where Figma left it
    fn wake(&self, age: f64) -> Option<Duration> {
        // The wheel zooming: each turn and frame wakes it, this for when
        // it's done
        if self.wheel_zoom.is_some() {
            return Some(Duration::from_millis(50));
        }
        // The zoom changing, moved on as it does
        if let Some(zooming) = self.zooming.filter(|_| age <= HOLD_MS) {
            let most = (zooming.gap * AHEAD_OF_GAP).min(AHEAD_MOST);
            return Some(Duration::from_secs_f64(if age <= most { 0.001 } else { (HOLD_MS - age) / 1000.0 }));
        }
        if self.zoom_back.is_some() {
            return Some(Duration::from_millis(1));
        }
        // Following the pointer, as it moves
        if self.back.is_some() || self.following() && (age <= HOLD_MS || pointer_held()) || self.wheel.is_some() && age <= WHEEL_OVER {
            return Some(Duration::from_millis(1));
        }
        // A button's down, which may be a drag of the canvas to follow
        if self.drag.is_none() && pointer_held() {
            return Some(Duration::from_millis(2));
        }
        let (_, _, dt) = self.velocity?;
        let most = (dt * AHEAD_OF_GAP).min(AHEAD_MOST);
        let ms = if age <= most {
            0.0
        } else if age <= HOLD_MS {
            HOLD_MS - age
        } else {
            return None;
        };
        Some(Duration::from_secs_f64(ms / 1000.0))
    }
}

/// The pointer, sampled a millisecond apart: when, in ms since the epoch,
/// where, in screen pixels, and the mouse buttons held, and space
static POINTER: Mutex<std::collections::VecDeque<(f64, f64, f64, u8)>> = Mutex::new(std::collections::VecDeque::new());
const LEFT: u8 = 1;
const MIDDLE: u8 = 2;
const RIGHT: u8 = 4;
const SPACE: u8 = 8;
const BUTTONS: u8 = LEFT | MIDDLE | RIGHT;

/// Samples the pointer from now on, for frames between Figma's
fn sample_pointer() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    let _ = std::thread::Builder::new().name("native-pointer".into()).spawn(|| {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON, VK_SPACE};
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
        loop {
            let mut at = POINT::default();
            if unsafe { GetCursorPos(&mut at) }.is_ok() {
                let held = [(VK_LBUTTON, LEFT), (VK_MBUTTON, MIDDLE), (VK_RBUTTON, RIGHT), (VK_SPACE, SPACE)]
                    .iter()
                    .filter(|(key, _)| unsafe { GetAsyncKeyState(key.0 as i32) } < 0)
                    .fold(0, |held, (_, bit)| held | bit);
                let mut samples = POINTER.lock().unwrap();
                samples.push_back((super::render::since_epoch_ms(), at.x as f64, at.y as f64, held));
                while samples.len() > 1000 {
                    samples.pop_front();
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    });
}

/// When the wheel last turned and the time before, in ms since the epoch,
/// as bits
static WHEELED: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];

/// The wheel's turns, the last 2 s of them: when, in ms since the epoch,
/// and how far it's turned across and down in all by then, shift turning
/// it across as in Figma
static WHEEL: Mutex<std::collections::VecDeque<(f64, f64, f64)>> = Mutex::new(std::collections::VecDeque::new());
static WHEEL_TOTAL: Mutex<(f64, f64)> = Mutex::new((0.0, 0.0));

/// Figma's frames the wheel's worked out from, at most, and over how many
/// milliseconds, the wheel having turned in them
const WHEEL_FRAMES: usize = 10;
const WHEEL_OVER: f64 = 300.0;
/// How long after the wheel turns Figma's frames show it, at most, in
/// milliseconds
const WHEEL_LATE: usize = 80;
/// How far, in device pixels, Figma's frames may be from where the wheel
/// put the view for the view to follow it
const WHEEL_FIT: f64 = 2.0;

/// The wheel's turns of the last 2 s
fn wheel_turns() -> Vec<(f64, f64, f64)> {
    WHEEL.lock().unwrap().iter().copied().collect()
}

/// How far the wheel had turned in all, across and down, by `at`
fn wheel_turned(turns: &[(f64, f64, f64)], at: f64) -> (f64, f64) {
    let i = turns.partition_point(|(when, ..)| *when <= at);
    turns.get(i.saturating_sub(1)).map_or((0.0, 0.0), |turn| (turn.1, turn.2))
}

/// Where the view's corner is, in device pixels, with the wheel not turned
/// at all, and how far it moves a step of the wheel, across and down, if
/// Figma's frames `frames` have the view where the wheel put it, each as
/// the wheel was the same time before it was sent
fn fit_wheel(frames: &std::collections::VecDeque<(f64, (f64, f64))>, turns: &[(f64, f64, f64)]) -> Option<((f64, f64), (f64, f64))> {
    if frames.len() < 3 {
        return None;
    }
    let n = frames.len() as f64;
    let mut best: Option<(f64, (f64, f64), (f64, f64))> = None;
    for late in 0..=WHEEL_LATE {
        let turned: Vec<(f64, f64)> = frames.iter().map(|(sent, _)| wheel_turned(turns, sent - late as f64)).collect();
        let mut error = 0.0;
        let mut fitted = [(0.0, 0.0); 2];
        for (axis, fit) in fitted.iter_mut().enumerate() {
            let xs: Vec<f64> = turned.iter().map(|t| if axis == 0 { t.0 } else { t.1 }).collect();
            let ys: Vec<f64> = frames.iter().map(|(_, c)| if axis == 0 { c.0 } else { c.1 }).collect();
            let (mx, my) = (xs.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
            let var: f64 = xs.iter().map(|x| (x - mx).powi(2)).sum();
            // Not turned this way: the view's not to move this way either
            let k = if var < 1.0 { 0.0 } else { xs.iter().zip(&ys).map(|(x, y)| (x - mx) * (y - my)).sum::<f64>() / var };
            let a = my - k * mx;
            error += xs.iter().zip(&ys).map(|(x, y)| (y - a - k * x).powi(2)).sum::<f64>();
            *fit = (a, k);
        }
        let per = (fitted[0].1, fitted[1].1);
        // Moved, and by a plausible amount a step
        let moves = (per.0.abs() >= 0.05 || per.1.abs() >= 0.05) && per.0.abs() <= 20.0 && per.1.abs() <= 20.0;
        if moves && best.is_none_or(|(least, ..)| error < least) {
            best = Some((error, (fitted[0].0, fitted[1].0), per));
        }
    }
    let (error, at, per) = best?;
    ((error / n).sqrt() <= WHEEL_FIT).then_some((at, per))
}

/// The wheel turned with Ctrl held, zooming, the last 10 s of it: each
/// turn's serial, when it was, in ms since the epoch, how far, and where
/// the pointer was, in screen pixels
static ZOOM_TURNS: Mutex<std::collections::VecDeque<(u64, f64, f64, (f64, f64))>> = Mutex::new(std::collections::VecDeque::new());
/// How much the log of Figma's zoom changes a step of the wheel, as bits,
/// as worked out from the last zoom, 0 before one has been
static ZOOM_STEP: AtomicU64 = AtomicU64::new(0);
/// Till then: Figma zooms by a factor of 2 every 3 notches of 120
const ZOOM_STEP_GUESS: f64 = std::f64::consts::LN_2 / 360.0;
/// The present thread's hub, woken as the wheel turns
static WHEEL_HUB: Mutex<Option<std::sync::Weak<Hub>>> = Mutex::new(None);

fn zoom_step() -> f64 {
    match ZOOM_STEP.load(Ordering::Relaxed) {
        0 => ZOOM_STEP_GUESS,
        bits => f64::from_bits(bits),
    }
}

/// `view` zoomed by `turns` of the wheel, `step` each, and how far its
/// corner moves, in units
fn wheel_zoomed(view: &View, turns: &[(u64, f64, f64, (f64, f64))], step: f64) -> Option<(View, (f64, f64))> {
    if turns.is_empty() {
        return None;
    }
    let mut moved = view.clone();
    for &(_, _, delta, (ax, ay)) in turns {
        let to = (moved.zoom * (step * delta).exp()).clamp(ZOOM_LEAST, ZOOM_MOST);
        moved.x += ax / moved.zoom - ax / to;
        moved.y += ay / moved.zoom - ay / to;
        moved.w *= moved.zoom / to;
        moved.h *= moved.zoom / to;
        moved.zoom = to;
    }
    let by = (moved.x - view.x, moved.y - view.y);
    Some((moved, by))
}

/// Watches the mouse wheel from now on, for when it stops
fn watch_wheel() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    let _ = std::thread::Builder::new().name("native-wheel".into()).spawn(|| unsafe {
        use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VIRTUAL_KEY, VK_CONTROL, VK_SHIFT};
        use windows::Win32::UI::WindowsAndMessaging::{CallNextHookEx, GetMessageW, SetWindowsHookExW, UnhookWindowsHookEx, MSG, MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_MOUSEHWHEEL, WM_MOUSEWHEEL};
        unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
            if code >= 0 && (wparam.0 as u32 == WM_MOUSEWHEEL || wparam.0 as u32 == WM_MOUSEHWHEEL) {
                let now = super::render::since_epoch_ms();
                WHEELED[1].store(WHEELED[0].swap(now.to_bits(), Ordering::Relaxed), Ordering::Relaxed);
                let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
                let delta = ((info.mouseData >> 16) as u16 as i16) as f64;
                let held = |key: VIRTUAL_KEY| unsafe { GetAsyncKeyState(key.0 as i32) } < 0;
                // Ctrl zooms, which isn't a pan
                if held(VK_CONTROL) {
                    if wparam.0 as u32 == WM_MOUSEWHEEL {
                        static SERIAL: AtomicU64 = AtomicU64::new(0);
                        if let Ok(mut turns) = ZOOM_TURNS.try_lock() {
                            turns.push_back((SERIAL.fetch_add(1, Ordering::Relaxed), now, delta, (info.pt.x as f64, info.pt.y as f64)));
                            while turns.front().is_some_and(|turn| now - turn.1 > 10_000.0) {
                                turns.pop_front();
                            }
                        }
                        if let Some(hub) = WHEEL_HUB.try_lock().ok().and_then(|hub| hub.as_ref().and_then(std::sync::Weak::upgrade)) {
                            hub.poke();
                        }
                    }
                } else {
                    let across = wparam.0 as u32 == WM_MOUSEHWHEEL || held(VK_SHIFT);
                    if let (Ok(mut turns), Ok(mut total)) = (WHEEL.try_lock(), WHEEL_TOTAL.try_lock()) {
                        // How far it had turned before, for the frames before
                        if turns.back().is_none_or(|(when, ..)| now - when > 2000.0) {
                            turns.clear();
                            turns.push_back((now - 1.0, total.0, total.1));
                        }
                        if across {
                            total.0 += delta;
                        } else {
                            total.1 += delta;
                        }
                        turns.push_back((now, total.0, total.1));
                        while turns.len() > 2 && turns[1].0 < now - 2000.0 {
                            turns.pop_front();
                        }
                    }
                }
            }
            unsafe { CallNextHookEx(None, code, wparam, lparam) }
        }
        let module = GetModuleHandleW(None).ok();
        let Ok(handle) = SetWindowsHookExW(WH_MOUSE_LL, Some(hook), module.map(|module| module.into()), 0) else { return };
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        let _ = UnhookWindowsHookEx(handle);
    });
}

/// Whether the wheel turned a moment ago but not since it would have again
/// were it still turning: half again as long as between its last two turns
fn wheel_stopped() -> bool {
    let [last, before] = [&WHEELED[0], &WHEELED[1]].map(|at| f64::from_bits(at.load(Ordering::Relaxed)));
    let since = super::render::since_epoch_ms() - last;
    let quiet = if before > 0.0 { ((last - before) * 1.5).clamp(15.0, 120.0) } else { 120.0 };
    last > 0.0 && since > quiet && since < 1000.0
}

/// Whether the pointer, moving along `points` from one to the next,
/// passed within `DRAG_NEAR` of `at`, as moving fast it's tens of pixels
/// from one sample to the next
fn passed(points: impl Iterator<Item = (f64, f64)>, at: (f64, f64)) -> bool {
    let mut before: Option<(f64, f64)> = None;
    for point in points {
        let from = before.unwrap_or(point);
        before = Some(point);
        let (dx, dy) = (point.0 - from.0, point.1 - from.1);
        let length = dx * dx + dy * dy;
        let along = if length > 0.0 { (((at.0 - from.0) * dx + (at.1 - from.1) * dy) / length).clamp(0.0, 1.0) } else { 0.0 };
        let nearest = (from.0 + dx * along, from.1 + dy * along);
        if (nearest.0 - at.0).abs() <= DRAG_NEAR && (nearest.1 - at.1).abs() <= DRAG_NEAR {
            return true;
        }
    }
    false
}

/// Where the pointer is, or was last while a button was held
fn pointer_now() -> Option<(f64, f64)> {
    let samples = POINTER.lock().unwrap();
    samples.iter().rev().find(|(.., held)| held & BUTTONS != 0).map(|(_, x, y, _)| (*x, *y))
}

/// Whether a mouse button is held, or was a moment ago
pub(super) fn pointer_held() -> bool {
    pointer_held_within(30.0)
}

/// How long after a button's let go the view's not moved on as it was
/// moving, in ms, as Figma's last frames of a drag come after
const RELEASED_MS: f64 = 300.0;

/// Whether a mouse button is held, or was in the last `ms` milliseconds
fn pointer_held_within(ms: f64) -> bool {
    let now = super::render::since_epoch_ms();
    POINTER.lock().unwrap().iter().rev().take_while(|(when, ..)| now - when < ms).any(|(.., held)| held & BUTTONS != 0)
}

/// The buttons held now, if they've been since a moment ago, and when they
/// went down, where the pointer was and what was held then, with space
fn pointer_pressed() -> Option<(f64, f64, f64, u8)> {
    let samples = POINTER.lock().unwrap();
    let now = samples.back().filter(|(.., held)| held & BUTTONS != 0)?;
    let pressed = samples.iter().rposition(|(.., held)| held & BUTTONS == 0).map_or(0, |i| i + 1);
    // Pressed a while ago, so samples may be missing from the start of it
    (pressed > 0 && now.0 - samples[pressed].0 < 500.0).then(|| samples[pressed])
}

/// When the pointer first moved more than a few pixels from where it was at
/// `since`, with a button held all along
fn pointer_moved_since(since: f64) -> Option<f64> {
    let samples = POINTER.lock().unwrap();
    let start = samples.partition_point(|(when, ..)| *when < since);
    let &(_, x, y, _) = samples.get(start)?;
    samples.range(start..).find(|(_, px, py, _)| (px - x).abs() > 3.0 || (py - y).abs() > 3.0).map(|(when, ..)| *when)
}

/// Whether `point`, on screen, is on the canvas `canvas` of window `window`,
/// with the window in front and nothing over it there
fn on_canvas(window: isize, (left, top, width, height): (i32, i32, i32, i32), (x, y): (f64, f64)) -> bool {
    use windows::Win32::Foundation::{HWND, POINT};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GetForegroundWindow, WindowFromPoint, GA_ROOT};
    let window = HWND(window as *mut _);
    let mut corner = POINT { x: left, y: top };
    unsafe {
        let root = GetAncestor(window, GA_ROOT);
        if GetForegroundWindow() != root || !ClientToScreen(window, &mut corner).as_bool() {
            return false;
        }
        let point = POINT { x: x as i32, y: y as i32 };
        if GetAncestor(WindowFromPoint(point), GA_ROOT) != root {
            return false;
        }
    }
    let (x, y) = (x - corner.x as f64, y - corner.y as f64);
    x >= 0.0 && y >= 0.0 && x < width as f64 && y < height as f64
}

/// Where `point`, on screen, is from the corner of the canvas `canvas` of
/// window `window`, in pixels, if it's on it
fn canvas_point(window: isize, canvas: (i32, i32, i32, i32), (x, y): (f64, f64)) -> Option<(f64, f64)> {
    use windows::Win32::Foundation::{HWND, POINT};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    if !on_canvas(window, canvas, (x, y)) {
        return None;
    }
    let mut corner = POINT { x: canvas.0, y: canvas.1 };
    unsafe { ClientToScreen(HWND(window as *mut _), &mut corner) }.as_bool().then(|| (x - corner.x as f64, y - corner.y as f64))
}

/// Whether frames between Figma's show the view moved on, off with
/// LIGMA_NATIVE_PREDICT=0
fn predicting() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("LIGMA_NATIVE_PREDICT").map_or(true, |value| value != "0"))
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
        let window = match output {
            Output::Window(hwnd) | Output::Layer(hwnd) => hwnd,
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
            window,
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
            pan: Pan::default(),
            ahead_px: (0.0, 0.0),
            ahead_scale: 1.0,
            told: None,
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
        *WHEEL_HUB.lock().unwrap() = Some(Arc::downgrade(&hub));
        // The snapshot frames are composed of: the latest the GPU has drawn
        let mut current: Option<Arc<Snapshot>> = None;
        // The last snapshot shown whole: while the latest has holes, the
        // view moves on over this, all of it as it was a moment before,
        // rather than standing still till the render thread fills them
        let mut complete: Option<Arc<Snapshot>> = None;
        let mut complete_view: Option<View> = None;
        // The last shown whole of the tabs shown before, and their views,
        // shown the moment one's back till it's drawn again, not nothing
        let mut left: Vec<(Arc<Snapshot>, View, Instant)> = Vec::new();
        // What's shown: the snapshot, the view, what's over it and the size
        let mut shown: Option<(u64, View, Option<u32>, (u32, u32))> = None;
        let mut seen = 0;
        // Whether the swap chain has room for a frame already waited for
        let mut room = false;
        // Since when the view's waited for what it needs drawn
        let mut held: Option<Instant> = None;
        while !hub.stop.load(Ordering::Relaxed) {
            if !room {
                let waiting = Instant::now();
                unsafe { WaitForSingleObjectEx(self.waitable, 250, true) };
                let waited = waiting.elapsed();
                if waited > Duration::from_millis(20) {
                    super::render::trace(|| format!("present waited {:.1}ms for room in the swap chain", waited.as_secs_f64() * 1000.0));
                }
                room = true;
            }
            let latest = hub.snapshot.lock().unwrap().clone();
            if let Some(latest) = latest.filter(|latest| current.as_ref().is_none_or(|current| (current.serial, current.seq) != (latest.serial, latest.seq)) && latest.drawn()) {
                current = Some(latest);
            }
            let picked = current.as_ref().ok_or(None).and_then(|snapshot| hub.pick(snapshot.changes, self.timed).map(|picked| (snapshot.clone(), picked)));
            let (snapshot, Picked { view, overlay, seq, oldest, wait, came, sent }) = match picked {
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
                let showing = hub.origin.lock().unwrap().as_ref().map(|(tab, _)| tab.clone());
                let whole_tab = hub.whole_tab.lock().unwrap().clone();
                let away = showing.as_deref() != whole_tab.as_deref();
                if away && complete.as_ref().is_some_and(|complete| complete.tab == whole_tab) && !left.iter().any(|(kept, ..)| kept.tab == whole_tab) {
                    if let (Some(kept), Some(view)) = (complete.clone(), complete_view.clone()) {
                        left.push((kept, view, Instant::now()));
                        if left.len() > LEFT_TABS {
                            left.remove(0);
                        }
                    }
                }
                left.retain(|(kept, _, at)| at.elapsed() <= LEFT_KEPT && (away || kept.tab != whole_tab));
                let back = left.iter().position(|(kept, ..)| showing.is_some() && kept.tab.as_deref() == showing.as_deref() && snapshot.tab != kept.tab).map(|index| left.remove(index));
                if let Some((kept, kept_view, _)) = back {
                    hub.whole_tab.lock().unwrap().clone_from(&kept.tab);
                    let (pieces, _) = cover_over(&kept.layers, kept.layers.len(), kept.content, &kept_view, self.size, &self.shown);
                    let _ = self.place(placed(&hub, &kept_view));
                    self.frame(&kept, &pieces, &kept_view, None, self.timed);
                    super::hud::tab_drawn();
                    room = false;
                    shown = Some((kept.serial, kept_view.clone(), None, self.size));
                    complete = Some(kept);
                    complete_view = Some(kept_view);
                    continue;
                }
                // Another tab's, not shown over this one
                if snapshot.tab.is_some() && snapshot.tab.as_deref() != showing.as_deref() && snapshot.tab != whole_tab {
                    let _ = self.place(None);
                    seen = hub.wait(seen, Duration::from_millis(100));
                    continue;
                }
                let at = placed(&hub, &view);
                if self.place(at) {
                    shown = None;
                }
                self.pan.canvas = at.map(|(x, y)| (self.window, (x, y, self.size.0 as i32, self.size.1 as i32)));
            }
            // Between Figma's frames, the view moved on as it's moving,
            // where what's drawn covers it there
            let age = came.map_or(f64::INFINITY, |came| came.elapsed().as_secs_f64() * 1000.0);
            let px = view.px(self.size.0) as f64;
            let ahead = (predicting() && came.is_some()).then(|| self.pan.ahead(seq, sent, age, &view, px)).flatten();
            let zoom = view.zoom;
            // The render thread draws for where the wheel's zoomed to, not
            // Figma's frame, which can be hundreds of milliseconds behind
            let wanted = ahead.as_ref().filter(|_| self.pan.wheel_zoom_left() > 0).map(|(moved, _)| moved.clone());
            if wanted != self.told {
                if let Some(messages) = hub.messages.lock().unwrap().as_ref() {
                    let _ = messages.send(super::render::Msg::Ahead(wanted.clone()));
                }
                self.told = wanted;
            }
            let covered = |view: &View| cover_over(&snapshot.layers, snapshot.layers.len(), snapshot.content, view, self.size, &self.shown).1.is_none();
            // The wheel zooming out past what's drawn: as far as it covers
            let ahead = match ahead {
                Some((moved, by)) if !covered(&moved) && self.pan.wheel_zoom_left() > 1 => {
                    let left = self.pan.wheel_zoom_left();
                    (1..left).rev().find_map(|n| self.pan.wheel_zoom_fewer(&view, n).filter(|(fewer, _)| covered(fewer))).or(Some((moved, by)))
                }
                ahead => ahead,
            };
            let (view, by) = match ahead {
                Some((moved, by)) if covered(&moved) => (moved, by),
                Some((moved, by)) => {
                    let hole = cover_over(&snapshot.layers, snapshot.layers.len(), snapshot.content, &moved, self.size, &self.shown).1;
                    let kinds: Vec<String> = snapshot.layers.iter().map(|layer| format!("{:?}@{},{} {}x{} px={:.3}", layer.kind, layer.x, layer.y, layer.tex.width, layer.tex.height, layer.px)).take(6).collect();
                    super::render::trace(|| format!("present not ahead by {:.1},{:.1}: hole {hole:?}, {} layers {kinds:?}", by.0 * px, by.1 * px, snapshot.layers.len()));
                    self.pan.not_ahead();
                    (view, (0.0, 0.0))
                }
                None => (view, (0.0, 0.0)),
            };
            // Zoomed on from Figma's frame too, its device pixels a unit
            // scaled by as much
            let scale = view.zoom / zoom;
            self.ahead_px = ((by.0 * px * scale) as f32, (by.1 * px * scale) as f32);
            self.ahead_scale = scale as f32;
            let wake = if predicting() { self.pan.wake(age) } else { None };
            let now = (snapshot.serial, view.clone(), overlay.as_ref().map(|drawn| drawn.canvas.unique_id()), self.size);
            if shown.as_ref() == Some(&now) {
                // Nothing to show that isn't on screen already, so it's
                // not waiting for anything
                if self.owed.take().is_some() {
                    self.timed = seq;
                }
                drop(snapshot);
                let timeout = wait.into_iter().chain(wake).min().unwrap_or(Duration::from_millis(100));
                seen = hub.wait(seen, timeout.clamp(Duration::from_millis(1), Duration::from_millis(100)));
                continue;
            }
            // All of the view or nothing: what's shown stays till the
            // render thread has drawn what the view needs. Its whole view
            // is drawn for the view itself, so it's what there is
            let (pieces, hole) = cover_over(&snapshot.layers, snapshot.layers.len(), snapshot.content, &view, self.size, &self.shown);
            if hole.is_some() && snapshot.whole_at != Some(view_at(&view, self.size)) {
                stats().lock().unwrap().holes(true);
                // Its holes filled from the last snapshot shown whole, as
                // it was a moment before, so the view never stands still
                // for what's being drawn again
                let merged = complete.as_ref().filter(|before| before.serial != snapshot.serial && before.tab == snapshot.tab).map(|before| {
                    let mut layers = snapshot.layers.clone();
                    layers.extend(before.layers.iter().cloned());
                    let (pieces, merged_hole) = cover_over(&layers, snapshot.layers.len(), snapshot.content, &view, self.size, &self.shown);
                    let holes = merged_hole.is_some();
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
                        tab: snapshot.tab.clone(),
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
                let before = complete.clone().filter(|before| before.serial != snapshot.serial && before.tab == snapshot.tab).and_then(|before| {
                    let (pieces, holes) = cover_over(&before.layers, before.layers.len(), before.content, &view, self.size, &self.shown);
                    let holes = holes.is_some();
                    let then = (before.serial, view.clone(), overlay.as_ref().map(|drawn| drawn.canvas.unique_id()), self.size);
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
                        let hole = hole.map(|at| format!("{},{} {}x{}", at.left, at.top, at.width(), at.height())).unwrap_or_default();
                        format!("present held: seq={seq} px={:.3} drawn={drawn_seq} hole={hole} size={}x{} layers={}", view.px(self.size.0), self.size.0, self.size.1, levels.join(","))
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
            complete_view = Some(view.clone());
            room = false;
            shown = Some(now);
            let mut whole_tab = hub.whole_tab.lock().unwrap();
            if *whole_tab != snapshot.tab {
                whole_tab.clone_from(&snapshot.tab);
                drop(whole_tab);
                // Shown now it's drawn whole for the tab shown, presented
                // again next, as the layer shows nothing till then
                if self.layer.is_some() && self.place(placed(&hub, &view)) {
                    shown = None;
                }
                super::hud::tab_drawn();
            }
            complete = Some(snapshot);
        }
        drop((current, complete));
        let _ = self.context.flush_submit_and_sync_cpu();
        drop(self);
        done();
    }

    /// Moves the layer to `at` in the window, or hides it. True where it's
    /// shown again, which shows nothing till a frame's presented again
    fn place(&mut self, at: Option<(i32, i32)>) -> bool {
        let Some(layer) = &mut self.layer else {
            return false;
        };
        if layer.at == at {
            return false;
        }
        let again = layer.at.is_none() && at.is_some();
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
        again
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
    fn frame(&mut self, snapshot: &Snapshot, pieces: &[Piece], view: &View, overlay: Option<&super::overlay::Drawn>, seq: u64) {
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
            let (sx, sy) = (sx as f32, sy as f32);
            let (ax, ay) = self.ahead_px;
            canvas.save();
            // With the scene, where the view's moved on from Figma's frame
            canvas.translate((-ax, -ay));
            canvas.scale((self.ahead_scale, self.ahead_scale));
            canvas.scale((sx, sy));
            canvas.draw_picture(&overlay.canvas, None, None);
            canvas.restore();
            // The rulers stay put, their marks moved with the scene along
            // each, within it
            let rulers = overlay.rulers.as_ref().map(|picture| (picture, (0.0, 0.0), None));
            let across = overlay.across.as_ref().map(|(picture, within)| (picture, (ax, 0.0), Some(within)));
            let down = overlay.down.as_ref().map(|(picture, within)| (picture, (0.0, ay), Some(within)));
            for (picture, (bx, by), within) in [rulers, across, down].into_iter().flatten() {
                canvas.save();
                canvas.scale((sx, sy));
                if let Some(within) = within {
                    canvas.clip_rect(within, skia_safe::ClipOp::Intersect, true);
                }
                canvas.translate((-bx / sx, -by / sy));
                canvas.draw_picture(picture, None, None);
                canvas.restore();
            }
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
        let called = Instant::now();
        let _ = unsafe { self.swap_chain.Present(1, DXGI_PRESENT(0)) };
        super::monitor::presented(called.elapsed());
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
                "present seq={seq} at={presented:.2} px={px:.3} layers={} encoded={:.1} rendered={:.1} holes=false x={} y={} placed={:?} drawn={} coarse={:.2} coarsest={coarsest:.2} serial={} path={} used={} ahead={:.1},{:.1} pan={}",
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
                used.join(","),
                self.ahead_px.0,
                self.ahead_px.1,
                if self.pan.following() { "pointer" } else if self.pan.wheel.is_some() { "wheel" } else if self.pan.velocity.is_some() { "velocity" } else { "-" }
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

/// How long what a tab showed last is kept once another's shown, for when
/// it's back
const LEFT_KEPT: Duration = Duration::from_secs(60);
/// Tabs whose last frame is kept so
const LEFT_TABS: usize = 2;

/// Where the browser puts Figma's canvas: at the nearest device pixel, a
/// pixel of it to a pixel of the screen. None to hide the layer, while the
/// tab shown isn't the one drawn whole last, as one loading or Figma's own
/// canvas drawing it till Ligma's drawn it, so another file's never shows
fn placed(hub: &Hub, view: &View) -> Option<(i32, i32)> {
    let (tab, (x, y)) = hub.origin.lock().unwrap().clone()?;
    if !view.visible || hub.whole_tab.lock().unwrap().as_deref() != Some(tab.as_str()) {
        return None;
    }
    Some((x + (view.cx * view.dpr).round() as i32, y + (view.cy * view.dpr).round() as i32))
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
