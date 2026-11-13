//! The present thread: puts Figma's view in the window every frame, from
//! the tiles the render thread has drawn so far, so the window follows
//! Figma's view the moment it moves however long a tile takes to draw
//!
//! The render thread draws tiles into textures of their own (`Tex`), made
//! outside Skia so its caches never hand them to anything else, and hands
//! them over (`Shown`). This thread has a Skia context of its own on the
//! same device and a queue of its own, ahead of the render thread's, and
//! draws them where the view has them now: the tiles of the view's zoom
//! first, then the whole view the render thread draws while the zoom
//! changes, then tiles of other zooms, scaled, where neither has anything
//! yet

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use skia_safe::gpu::d3d::{BackendContext, TextureResourceInfo};
use skia_safe::gpu::{self, BackendRenderTarget, BackendTexture, DirectContext, Protected, SurfaceOrigin, SyncCpu};
use skia_safe::{
    AlphaType, Color4f, ColorType, Data, FilterMode, Image, MipmapMode, Paint, Rect, SamplingOptions, Surface,
};
use windows::core::Interface;
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::Graphics::Direct3D12::{
    ID3D12Device, ID3D12Resource, D3D12_COMMAND_QUEUE_DESC, D3D12_COMMAND_QUEUE_PRIORITY_HIGH, D3D12_HEAP_FLAG_NONE,
    D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_TEXTURE2D,
    D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
    D3D12_TEXTURE_LAYOUT_UNKNOWN,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC, DXGI_STANDARD_MULTISAMPLE_QUALITY_PATTERN,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice2, IDCompositionDesktopDevice, IDCompositionTarget, IDCompositionVisual2, DCOMPOSITION_BITMAP_INTERPOLATION_MODE_LINEAR,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_ALPHA_MODE_IGNORE, DXGI_ALPHA_MODE_UNSPECIFIED};
use windows::Win32::Graphics::Dxgi::{
    IDXGIAdapter1, IDXGIFactory2, IDXGIFactory4, IDXGISwapChain1, IDXGISwapChain2, IDXGISwapChain3, DXGI_PRESENT, DXGI_SCALING_NONE, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT, DXGI_SWAP_EFFECT_FLIP_DISCARD,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::System::Threading::WaitForSingleObjectEx;

use super::draw::Counts;
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
/// shows. Whenever it isn't being drawn into it's left ready to be read
/// (`D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE`), which every Skia context
/// that wraps it is told, so none has to change its state but to draw into
/// it. It's freed once neither thread holds it
pub struct Tex {
    pub resource: ID3D12Resource,
    pub width: i32,
    pub height: i32,
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
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
        };
        let mut resource: Option<ID3D12Resource> = None;
        unsafe { device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE, None, &mut resource) }
            .map_err(|error| log::warn!("native renderer: texture: {error}"))
            .ok()?;
        Some(Arc::new(Tex { resource: resource?, width, height }))
    }

    /// For a Skia context to wrap, ready to be read
    pub fn backend(&self) -> BackendTexture {
        let info = TextureResourceInfo {
            resource: self.resource.clone(),
            alloc: None,
            resource_state: D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            format: DXGI_FORMAT_R8G8B8A8_UNORM,
            sample_count: 1,
            level_count: 1,
            sample_quality_pattern: DXGI_STANDARD_MULTISAMPLE_QUALITY_PATTERN,
            protected: Protected::No,
        };
        gpu::backend_textures::make_d3d((self.width, self.height), &info, "ligma")
    }
}

/// Something drawn at `px` device pixels a unit, its top left corner at
/// (`x`, `y`) device pixels at that zoom
#[derive(Clone)]
pub struct Layer {
    pub tex: Arc<Tex>,
    pub x: i32,
    pub y: i32,
    pub px: f32,
    pub counts: Counts,
    /// An out of date tile, shown only where the whole view, drawn since,
    /// isn't
    pub behind: bool,
}

/// What the render thread has drawn for the view
#[derive(Clone, Default)]
pub struct Shown {
    /// Changes whenever anything else does
    pub generation: u64,
    pub tiles: Vec<Layer>,
    /// The whole view, drawn while the zoom changes or before its tiles are
    pub live: Option<Layer>,
    pub background: Option<Color4f>,
    /// Figma's checkerboard, and its darker kind, for a hidden background
    pub transparent: bool,
    pub dark: bool,
    /// The render thread has more to draw for the view
    pub pending: bool,
}

/// What the threads share
pub struct Hub {
    pub view: Mutex<Option<View>>,
    pub shown: Mutex<Shown>,
    /// The window's size in pixels
    pub size: Mutex<(u32, u32)>,
    /// Drawn in place of Figma's canvas, under the tab: where the tab's
    /// webview is in the window, in pixels, and whether the tab is shown
    pub origin: Mutex<Option<(i32, i32)>>,
    pub stop: AtomicBool,
    /// Frames of Figma's render tree that couldn't be applied in a row
    pub tree_failures: std::sync::atomic::AtomicU32,
    /// Memory shared with the tab drawn, to tell it the last frame drawn
    pub shared: Mutex<Option<crate::webview2::Shared>>,
    /// Views of frames of the render tree not shown yet, with when Figma's
    /// worker drew its overlay for each, the first the one shown now
    frames: Mutex<std::collections::VecDeque<FrameView>>,
    /// When Figma's worker last said it drew its overlay
    overlay_at: Mutex<Option<Instant>>,
    /// The frame whose view was last shown, for tracing
    pub shown_seq: AtomicU64,
    changes: Mutex<u64>,
    changed: Condvar,
}

impl Hub {
    pub fn new(size: (u32, u32)) -> Self {
        Hub {
            view: Mutex::new(None),
            shown: Mutex::new(Shown::default()),
            size: Mutex::new(size),
            origin: Mutex::new(None),
            stop: AtomicBool::new(false),
            tree_failures: std::sync::atomic::AtomicU32::new(0),
            shared: Mutex::new(None),
            frames: Mutex::new(std::collections::VecDeque::new()),
            overlay_at: Mutex::new(None),
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

    /// The view of frame `seq` of the render tree, shown once Figma's
    /// worker has drawn its overlay for it, so the two show together
    pub fn frame_view(&self, seq: u64, view: View) {
        let mut frames = self.frames.lock().unwrap();
        // Lost track: shown at once
        if frames.back().is_some_and(|last| last.seq >= seq) {
            frames.clear();
        }
        frames.push_back(FrameView { seq, view: view.clone(), came: Instant::now(), drawn: None });
        while frames.len() > 64 {
            frames.pop_front();
        }
        drop(frames);
        *self.view.lock().unwrap() = Some(view);
        self.poke();
    }

    /// Figma's worker drew its overlay for frames up to `seq`
    pub fn overlay_drawn(&self, seq: u64) {
        let now = Instant::now();
        *self.overlay_at.lock().unwrap() = Some(now);
        for frame in self.frames.lock().unwrap().iter_mut().filter(|frame| frame.seq <= seq) {
            frame.drawn.get_or_insert(now);
        }
        self.poke();
    }

    /// The view to show now: the latest whose overlay Figma's worker has
    /// drawn, `OVERLAY_LATENCY` ago, or that's waited `OVERLAY_HOLD`. With
    /// how long until the next is due, if one waits
    fn shown_view(&self) -> (Option<View>, Option<Duration>) {
        let mut frames = self.frames.lock().unwrap();
        if frames.is_empty() {
            return (self.view.lock().unwrap().clone(), None);
        }
        let now = Instant::now();
        // The worker isn't saying, so views aren't held for it
        let heard = self.overlay_at.lock().unwrap().is_some_and(|at| now - at < Duration::from_secs(1));
        let latency = overlay_latency();
        let due = |frame: &FrameView| {
            let Some(latency) = latency.filter(|_| heard) else {
                return frame.came;
            };
            let drawn = frame.drawn.map_or(frame.came + OVERLAY_HOLD, |drawn| drawn + latency);
            drawn.min(frame.came + OVERLAY_HOLD)
        };
        let mut next = None;
        while frames.len() > 1 {
            let at = due(&frames[1]);
            if at > now {
                next = Some(at - now);
                break;
            }
            frames.pop_front();
        }
        self.shown_seq.store(frames.front().map_or(0, |frame| frame.seq), Ordering::Relaxed);
        (frames.front().map(|frame| frame.view.clone()), next)
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
    came: Instant,
    drawn: Option<Instant>,
}

/// Longest a frame's view waits for Figma's worker to draw its overlay
const OVERLAY_HOLD: Duration = Duration::from_millis(50);

/// From when Figma's worker draws its overlay to when it's on screen,
/// past what it takes this thread to show a frame, in microseconds, or
/// negative for views not to wait for it (`set_overlay_latency`)
static OVERLAY_LATENCY: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

fn overlay_latency() -> Option<Duration> {
    let micros = OVERLAY_LATENCY.load(Ordering::Relaxed);
    (micros >= 0).then(|| Duration::from_micros(micros as u64))
}

/// For the dev build, to tune it
pub fn set_overlay_latency(ms: f64) {
    OVERLAY_LATENCY.store((ms * 1000.0) as i64, Ordering::Relaxed);
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
    /// Where it is, and how much it's stretched, in hundred thousandths
    at: Option<((i32, i32), (i32, i32))>,
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
    images: HashMap<usize, (Arc<Tex>, BackendTexture, Image)>,
    checker: Option<skia_safe::RuntimeEffect>,
    grid: Option<(skia_safe::RuntimeEffect, skia_safe::RuntimeEffect)>,
}

impl Screen {
    pub fn new(device: &Device, output: Output, width: u32, height: u32) -> Result<Self, String> {
        let queue_desc = D3D12_COMMAND_QUEUE_DESC { Priority: D3D12_COMMAND_QUEUE_PRIORITY_HIGH.0, ..Default::default() };
        let queue = unsafe { device.device.CreateCommandQueue(&queue_desc) }.map_err(|error| format!("queue: {error}"))?;
        let backend = BackendContext {
            adapter: device.adapter.clone(),
            device: device.device.clone(),
            queue,
            memory_allocator: Some(super::committed::allocator(&device.device)),
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
            images: HashMap::new(),
            checker: skia_safe::RuntimeEffect::make_for_shader(CHECKER, None).map_err(|error| log::warn!("native renderer: checker: {error}")).ok(),
            grid: skia_safe::RuntimeEffect::make_for_shader(GRID_LINES, None)
                .and_then(|lines| skia_safe::RuntimeEffect::make_for_blender(GRID_COLOR, None).map(|color| (lines, color)))
                .map_err(|error| log::warn!("native renderer: grid: {error}"))
                .ok(),
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
                    canvas.clip_region(&clip, skia_safe::ClipOp::Intersect);
                }
                canvas.draw_image_rect_with_sampling_options(&image, None, Rect::from_xywh(0.5, 0.5, 50.0, 50.0), SamplingOptions::new(filter, MipmapMode::None), &Paint::default());
                canvas.restore();
            }
        }
        let shown = Shown { transparent: true, ..Default::default() };
        if let Some(paint) = self.checker(&shown, (0.0, 0.0), 1.0, 1.0) {
            canvas.draw_paint(&paint);
        }
        if let Some(paint) = self.pixel_grid((0.0, 0.0), 10.0) {
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
        let mut shown: Option<(View, u64, (u32, u32))> = None;
        let mut seen = 0;
        // Whether the swap chain has room for a frame already waited for
        let mut room = false;
        while !hub.stop.load(Ordering::Relaxed) {
            if !room {
                unsafe { WaitForSingleObjectEx(self.waitable, 250, true) };
                room = true;
            }
            let (view, due) = hub.shown_view();
            let size = match (&self.layer, &view) {
                (Some(_), Some(view)) => view.canvas(),
                _ => *hub.size.lock().unwrap(),
            };
            if size != self.size && size.0 > 0 && size.1 > 0 {
                if let Err(error) = self.resize(size) {
                    log::warn!("native renderer: {error}");
                }
                shown = None;
            }
            let what = hub.shown.lock().unwrap().clone();
            if self.layer.is_some() {
                // Where the browser would put Figma's canvas: at the nearest
                // device pixel, stretched to its size
                let at = (*hub.origin.lock().unwrap()).zip(view.as_ref().filter(|view| view.visible)).map(|((x, y), view)| {
                    let (sx, sy) = view.stretch();
                    let at = (x + (view.cx * view.dpr).round() as i32, y + (view.cy * view.dpr).round() as i32);
                    (at, ((sx * 1e5).round() as i32, (sy * 1e5).round() as i32))
                });
                self.place(at);
            }
            let Some(view) = view else {
                seen = hub.wait(seen, Duration::from_millis(100));
                continue;
            };
            let now = (view.clone(), what.generation, self.size);
            if shown.as_ref() == Some(&now) {
                seen = hub.wait(seen, due.unwrap_or(Duration::from_millis(100)).min(Duration::from_millis(100)));
                continue;
            }
            self.frame(&view, &what, hub.shown_seq.load(Ordering::Relaxed));
            room = false;
            shown = Some(now);
        }
    }

    /// Moves the layer to `at` in the window, or hides it
    fn place(&mut self, at: Option<((i32, i32), (i32, i32))>) {
        let Some(layer) = &mut self.layer else {
            return;
        };
        if layer.at == at {
            return;
        }
        layer.at = at;
        unsafe {
            match at {
                Some(((x, y), (sx, sy))) => {
                    let _ = layer.visual.SetOffsetX2(0.0);
                    let _ = layer.visual.SetOffsetY2(0.0);
                    let matrix = windows_numerics::Matrix3x2 { M11: sx as f32 / 1e5, M12: 0.0, M21: 0.0, M22: sy as f32 / 1e5, M31: x as f32, M32: y as f32 };
                    let _ = layer.visual.SetTransform2(&matrix);
                    let _ = layer.visual.SetBitmapInterpolationMode(DCOMPOSITION_BITMAP_INTERPOLATION_MODE_LINEAR);
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
        if let Some((_, _, image)) = self.images.get(&key) {
            return Some(image.clone());
        }
        let backend = tex.backend();
        let image = gpu::images::borrow_texture_from(&mut self.context, &backend, SurfaceOrigin::TopLeft, ColorType::RGBA8888, AlphaType::Premul, None)?;
        self.images.insert(key, (tex.clone(), backend, image.clone()));
        Some(image)
    }

    fn frame(&mut self, view: &View, what: &Shown, seq: u64) {
        let start = Instant::now();
        let index = unsafe { self.swap_chain.GetCurrentBackBufferIndex() } as usize;
        if index >= self.buffers.len() {
            return;
        }
        let (width, height) = (self.size.0 as i32, self.size.1 as i32);
        let px = view.px(self.size.0);
        // What the render thread draws the view at, a pixel preview's
        // pixels magnified
        let (drawn, preview, _, _) = view.drawn(self.size);
        let (ox, oy) = (view.x * px as f64, view.y * px as f64);
        // Tiles of this zoom land on whole pixels, as Figma's do
        let (rx, ry) = (ox.round() as i32, oy.round() as i32);
        // Nearest to what Figma shows first: tiles of this zoom, then the
        // whole view drawn at it, then the rest, nearest zoom first
        let mut layers: Vec<(f32, &Layer)> = what
            .tiles
            .iter()
            .map(|layer| (if layer.px == drawn && !layer.behind { 0.0 } else { 2.0 + (layer.px / drawn).log2().abs() }, layer))
            .chain(what.live.iter().map(|layer| (if layer.px == drawn { 1.0 } else { 1.5 + (layer.px / drawn).log2().abs() }, layer)))
            .collect();
        layers.sort_by(|a, b| a.0.total_cmp(&b.0));
        let window = skia_safe::IRect::from_wh(width, height);
        let mut left = skia_safe::Region::new();
        left.set_rect(window);
        let mut counts = Counts::default();
        let mut draws: Vec<(Image, Rect, skia_safe::Region, bool)> = Vec::new();
        for (rank, layer) in layers {
            if left.is_empty() {
                break;
            }
            let same = layer.px == px;
            let square = same || (preview && layer.px == drawn);
            let dest = if same {
                Rect::from_xywh((layer.x - rx) as f32, (layer.y - ry) as f32, layer.tex.width as f32, layer.tex.height as f32)
            } else {
                let scale = (px / layer.px) as f64;
                // A preview's squares are placed from the whole pixel the
                // view lands on, as Figma places them
                let (ox, oy) = if square { (rx as f64, ry as f64) } else { (ox, oy) };
                Rect::from_xywh(
                    (layer.x as f64 * scale - ox) as f32,
                    (layer.y as f64 * scale - oy) as f32,
                    (layer.tex.width as f64 * scale) as f32,
                    (layer.tex.height as f64 * scale) as f32,
                )
            };
            let covers = dest.round();
            let mut here = skia_safe::Region::new();
            here.set_rect(covers);
            if !here.op_region(&left, skia_safe::region::RegionOp::Intersect) {
                continue;
            }
            let Some(image) = self.image(&layer.tex) else {
                continue;
            };
            if rank < 2.0 {
                counts += layer.counts;
            }
            left.op_rect(covers, skia_safe::region::RegionOp::Difference);
            draws.push((image, dest, here, square));
        }
        // Outline mode shows the editor's background alone, Figma's overlay
        // drawing the outlines
        let outline = view.outline();
        if outline.is_some() {
            draws.clear();
            left.set_empty();
        }
        let checker = if outline.is_some() { None } else { self.checker(what, (ox, oy), px, view.zoom) };
        let grid = if view.grid && outline.is_none() { self.pixel_grid((ox, oy), px) } else { None };
        let (surface, _) = &mut self.buffers[index];
        let canvas = surface.canvas();
        canvas.clear(outline.or(what.background).unwrap_or(Color4f::new(1.0, 1.0, 1.0, 1.0)));
        if let Some(checker) = &checker {
            canvas.draw_paint(checker);
        }
        for (image, dest, clip, square) in &draws {
            canvas.save();
            canvas.clip_region(clip, skia_safe::ClipOp::Intersect);
            let sampling = if *square { SamplingOptions::new(FilterMode::Nearest, MipmapMode::None) } else { SamplingOptions::new(FilterMode::Linear, MipmapMode::None) };
            canvas.draw_image_rect_with_sampling_options(image, None, *dest, sampling, &Paint::default());
            canvas.restore();
        }
        if let Some(grid) = &grid {
            canvas.draw_rect(Rect::from_iwh(width, height), grid);
        }
        let encoded = start.elapsed();
        if let Err(error) = self.context.flush_and_submit_surface(surface, Some(SyncCpu::Yes)) {
            log::warn!("native renderer: {error:?}");
        }
        let rendered = start.elapsed();
        let _ = unsafe { self.swap_chain.Present(1, DXGI_PRESENT(0)) };
        let presented = super::render::since_epoch_ms();
        // Textures the render thread let go of, now the GPU is done with them
        let kept: std::collections::HashSet<usize> =
            what.tiles.iter().chain(what.live.iter()).map(|layer| Arc::as_ptr(&layer.tex) as usize).collect();
        self.images.retain(|key, _| kept.contains(key));
        super::render::trace(|| {
            format!("present seq={seq} at={presented:.2} px={px:.3} layers={} encoded={:.1} rendered={:.1} holes={}", draws.len(), encoded.as_secs_f64() * 1000.0, rendered.as_secs_f64() * 1000.0, !left.is_empty())
        });
        let mut stats = stats().lock().unwrap();
        stats.holes(!left.is_empty());
        stats.record_frame(
            encoded.as_secs_f64() * 1000.0,
            rendered.as_secs_f64() * 1000.0,
            counts.native,
            counts.approximate,
            counts.skipped,
            view.zoom,
            what.pending || !left.is_empty(),
        );
    }

    /// Figma's checkerboard for a page with its background hidden. It's
    /// anchored to the canvas, its squares 8 units wide at 100% and halved or
    /// doubled at each power of two of zoom, so they stay 5.7 to 11.3 points
    /// `origin` is the view's corner in device pixels, unrounded, as Figma
    /// places the pattern
    fn checker(&self, what: &Shown, origin: (f64, f64), px: f32, zoom: f64) -> Option<Paint> {
        if !what.transparent {
            return None;
        }
        checker_paint(self.checker.as_ref()?, what.dark, origin, px, zoom)
    }

    /// Figma's pixel grid, lines a device pixel wide at each unit once a unit
    /// is 8 device pixels or more, which lighten dark colors and darken light
    /// ones
    fn pixel_grid(&self, origin: (f64, f64), px: f32) -> Option<Paint> {
        grid_paint(self.grid.as_ref()?, origin, px)
    }
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
