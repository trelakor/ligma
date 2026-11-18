//! Draws the whole view with Vello, which fills paths on the GPU in compute
//! shaders, all of the view in a few passes however many paths and layers
//! it has, rather than a draw and often a render pass each as Skia does. On
//! a device of its own on the same GPU, sharing textures and fences with
//! Skia's. What Vello can't draw exactly, images, blurs, shadows and
//! effects, Skia draws into an atlas at the same zoom first, which Vello
//! places where it goes, under the same clips, masks and layers

use std::collections::HashMap;
use std::sync::Arc;

use skia_safe::RoundOut as _;
use skia_safe::{Color4f, IRect, Matrix, Path, PathFillType, PathVerb, Point, Rect};
use vello::kurbo::{self, Affine, BezPath};
use vello::peniko::{self, color::AlphaColor, color::DynamicColor, color::Srgb, BlendMode, ColorStop, Compose, Extend, Fill, ImageBrush, ImageData, ImageQuality, ImageSampler, Mix};
use vello::low_level::vello_encoding::{Encoding, Patch, Transform};
use vello::wgpu;
use windows::core::Interface;
use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE, LUID};
use windows::Win32::Graphics::Direct3D12::{ID3D12CommandQueue, ID3D12Device, ID3D12Fence, ID3D12Resource, D3D12_FENCE_FLAG_SHARED};

use super::draw::{self, Counts, Img, Kit};
use super::present::Tex;
use super::rt::{self, MaskOpType, Node, Op, PaintType};
use super::tree::{self, Drawer, Place, Tree};
use super::vello_blur::{Blurrer, Job};

type Dx12 = wgpu::hal::api::Dx12;

/// Most pages of the atlas, each the size of the view, as much as Skia
/// draws in one
const PAGES: usize = 4;

/// How long a page of the atlas no view has needed is kept
const PAGE_KEPT: std::time::Duration = std::time::Duration::from_secs(3);

/// A D3D12 object made on Skia's device, opened on Vello's
struct Shared<T> {
    skia: T,
    vello: T,
}

unsafe impl<T> Send for Shared<T> {}

/// A texture of Skia's that Vello reads or writes
struct Target {
    tex: Arc<Tex>,
    raw: ID3D12Resource,
}

unsafe impl Send for Target {}

/// A page of the atlas Skia draws what Vello can't into
struct Page {
    target: Target,
    used: std::time::Instant,
}

/// What shaders may bind at once, see `Vello::new`
pub const BINDINGS: u32 = 1 << 16;

/// The most bytes any one of Vello's buffers a render allocates from takes
const MOST_BUFFER: u64 = 384 << 20;

pub struct Vello {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: vello::Renderer,
    /// Skia signals it once the atlas is drawn, Vello waits for it
    atlas_drawn: Shared<ID3D12Fence>,
    /// Vello signals it once the view's drawn, Skia waits for it
    view_drawn: Shared<ID3D12Fence>,
    signalled: u64,
    /// How long Vello took to hand the last view to the GPU
    pub took: std::time::Duration,
    /// Dev tools: of which drawing blurs, and waiting to know buffers had
    /// room
    pub took_blurs: std::time::Duration,
    pub waited: std::time::Duration,
    /// Dev tools: each stage's GPU time, where it's synced after each
    pub stages: String,
    /// The last view's scene, cleared, so its memory's used again rather
    /// than new memory made ready each frame
    pub spare: vello::Scene,
    /// How many renders of Vello's the last view took
    pub renders: usize,
    out: Option<Target>,
    pages: Vec<Page>,
    /// What Skia drew onto the atlas for earlier views, drawn again from
    /// here where it would be the same, and the views drawn
    pieces: HashMap<PieceKey, Piece>,
    piece_bytes: usize,
    views: u64,
    /// Vello's copies of paths, by the path they're of

    /// Set if wgpu reports an error, after which Vello isn't used
    broken: Arc<std::sync::atomic::AtomicBool>,
    /// Image fills' textures Vello has, by key and mipmap level
    pictures: HashMap<(String, u32), Held>,
    barriers: Barriers,
    skia: SendDevice,
    blurrer: Option<Blurrer>,
    /// The textures blurs' sources are drawn into, by how deep in other
    /// blurs they are, and when each was last used
    blurred: Vec<Option<(wgpu::Texture, std::time::Instant)>>,
}

/// An image fill's texture, Skia's, which Vello copies into its atlas
struct Held {
    data: ImageData,
    /// Skia's, whose state it tracks
    backend: skia_safe::gpu::BackendTexture,
    resource: ID3D12Resource,
    /// The Skia image's, so a new one for the key makes a new picture
    id: u32,
}

/// Barriers on Skia's queue, from command allocators each used again once
/// the GPU's past the fence value it was last sent with
struct Barriers {
    allocators: Vec<(windows::Win32::Graphics::Direct3D12::ID3D12CommandAllocator, u64)>,
    list: Option<windows::Win32::Graphics::Direct3D12::ID3D12GraphicsCommandList>,
}

unsafe impl Send for Held {}
unsafe impl Send for Barriers {}

struct SendDevice(ID3D12Device);

unsafe impl Send for SendDevice {}

/// Starts Vello on its own thread, as making its shaders takes a moment
pub fn start(device: &ID3D12Device) -> std::sync::mpsc::Receiver<Result<Vello, String>> {
    let (send, receive) = std::sync::mpsc::channel();
    let device = SendDevice(device.clone());
    let _ = std::thread::Builder::new().name("native-vello-start".into()).stack_size(64 << 20).spawn(move || {
        let device = device;
        let started = std::time::Instant::now();
        let made = Vello::new(&device.0);
        match &made {
            Ok(_) => {
                log::info!("native renderer: Vello started in {:.0}ms", started.elapsed().as_secs_f64() * 1000.0);
                super::render::trace(|| format!("Vello started in {:.0}ms", started.elapsed().as_secs_f64() * 1000.0));
            }
            Err(error) => {
                log::warn!("native renderer: Vello didn't start: {error}");
                super::render::trace(|| format!("Vello didn't start: {error}"));
            }
        }
        let _ = send.send(made);
    });
    receive
}

fn shared_handle(device: &ID3D12Device, object: &windows::core::IUnknown) -> Result<HANDLE, String> {
    let child: windows::Win32::Graphics::Direct3D12::ID3D12DeviceChild = object.cast().map_err(|error| error.to_string())?;
    unsafe { device.CreateSharedHandle(&child, None, GENERIC_ALL.0, windows::core::PCWSTR::null()) }.map_err(|error| format!("shared handle: {error}"))
}

fn open<T: Interface>(device: &ID3D12Device, handle: HANDLE) -> Result<T, String> {
    let mut out: Option<T> = None;
    let opened = unsafe { device.OpenSharedHandle(handle, &mut out) };
    let _ = unsafe { CloseHandle(handle) };
    opened.map_err(|error| format!("open shared: {error}"))?;
    out.ok_or_else(|| "open shared: nothing".to_string())
}

impl Vello {
    fn new(skia: &ID3D12Device) -> Result<Self, String> {
        let luid: LUID = unsafe { skia.GetAdapterLuid() };
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::DX12;
        desc.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::StaticDxc;
        let instance = wgpu::Instance::new(desc);
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::DX12));
        let adapter = adapters
            .into_iter()
            .find(|adapter| {
                let Some(hal) = (unsafe { adapter.as_hal::<Dx12>() }) else { return false };
                let Ok(desc) = (unsafe { hal.as_raw().GetDesc1() }) else { return false };
                desc.AdapterLuid.LowPart == luid.LowPart && desc.AdapterLuid.HighPart == luid.HighPart
            })
            .ok_or("no wgpu adapter for Skia's GPU")?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("ligma vello"),
            // Dev tools: timing what's drawn on the GPU
            required_features: adapter.features()
                & (wgpu::Features::CLEAR_TEXTURE
                    | if std::env::var_os("LIGMA_NATIVE_VELLO_TIMES").is_some() { wgpu::Features::TIMESTAMP_QUERY } else { wgpu::Features::empty() }),
            // Its table of what shaders bind, kept in GPU memory, is as big
            // as this says, 32 MB at the most there could be, where a render
            // takes a few hundred
            required_limits: wgpu::Limits { max_non_sampler_bindings: BINDINGS, ..adapter.limits() },
            // Small blocks of GPU memory, rather than ones far bigger than
            // what's in them, and memory of its own for anything bigger,
            // given back the moment it's let go
            memory_hints: wgpu::MemoryHints::Manual { suballocated_device_memory_block_size: (4 << 20)..(4 << 20) },
            ..Default::default()
        }))
        .map_err(|error| format!("device: {error}"))?;
        // A buffer past what the device takes is an error that stops every
        // render after, and the pool makes buffers up to a quarter bigger
        // than asked. A render needing more than this is drawn in tiles
        let limits = device.limits();
        let most = limits.max_buffer_size.min(limits.max_storage_buffer_binding_size as u64) / 5 * 4;
        // Dev tools: less, to try views too big for it
        let most_buffer = std::env::var("LIGMA_NATIVE_VELLO_MOST_MB").ok().and_then(|mb| mb.parse::<u64>().ok()).map_or(MOST_BUFFER, |mb| mb << 20);
        vello::low_level::vello_encoding::MOST_BYTES.store(most.min(most_buffer) as u32, std::sync::atomic::Ordering::Relaxed);
        // Started again after an error, not with the room that led to it
        vello::reset_room();
        let broken = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = broken.clone();
        device.on_uncaptured_error(Arc::new(move |error| {
            log::warn!("native renderer: Vello: {error}");
            super::render::trace(|| format!("Vello error: {error}"));
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }));
        let renderer = vello::Renderer::new(
            &device,
            vello::RendererOptions { antialiasing_support: vello::AaSupport { area: std::env::var_os("LIGMA_NATIVE_VELLO_AREA").is_some(), msaa4: std::env::var_os("LIGMA_NATIVE_VELLO_MSAA4").is_some(), msaa8: false, msaa16: true }, ..Default::default() },
        )
        .map_err(|error| format!("renderer: {error}"))?;
        let raw = Self::raw_device(&device)?;
        let fence = |name: &str| -> Result<Shared<ID3D12Fence>, String> {
            let skia_fence: ID3D12Fence = unsafe { skia.CreateFence(0, D3D12_FENCE_FLAG_SHARED) }.map_err(|error| format!("{name}: {error}"))?;
            let handle = shared_handle(skia, &skia_fence.cast().map_err(|error| error.to_string())?)?;
            Ok(Shared { skia: skia_fence, vello: open(&raw, handle)? })
        };
        Ok(Self {
            atlas_drawn: fence("atlas fence")?,
            view_drawn: fence("view fence")?,
            device,
            queue,
            renderer,
            signalled: 0,
            took: std::time::Duration::ZERO,
            took_blurs: std::time::Duration::ZERO,
            waited: std::time::Duration::ZERO,
            stages: String::new(),
            spare: vello::Scene::new(),
            renders: 0,
            out: None,
            pages: Vec::new(),
            pieces: HashMap::new(),
            piece_bytes: 0,
            views: 0,

            broken,
            pictures: HashMap::new(),
            barriers: Barriers { allocators: Vec::new(), list: None },
            skia: SendDevice(skia.clone()),
            blurrer: None,
            blurred: Vec::new(),
        })
    }

    fn raw_device(device: &wgpu::Device) -> Result<ID3D12Device, String> {
        let hal = unsafe { device.as_hal::<Dx12>() }.ok_or("not D3D12")?;
        Ok(hal.raw_device().clone())
    }

    pub fn broken(&self) -> bool {
        self.broken.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// A texture of Skia's of at least `width` by `height` that Vello can
    /// also use, kept in `slot` and made again only when it's too small
    fn target(skia: &ID3D12Device, vello: &wgpu::Device, slot: &mut Option<Target>, width: i32, height: i32) -> Result<Arc<Tex>, String> {
        let _span = super::span::span("vello target");
        if let Some(target) = slot.as_ref().filter(|target| target.tex.width >= width && target.tex.height >= height) {
            return Ok(target.tex.clone());
        }
        let target = Self::make(skia, vello, width, height)?;
        let tex = target.tex.clone();
        *slot = Some(target);
        Ok(tex)
    }

    fn make(skia: &ID3D12Device, vello: &wgpu::Device, width: i32, height: i32) -> Result<Target, String> {
        // In steps, so a slightly bigger view doesn't make another
        let step = |size: i32| (size + 255) / 256 * 256;
        let (width, height) = (step(width), step(height));
        let tex = Tex::new_shared(skia, width, height).ok_or("shared texture")?;
        let handle = shared_handle(skia, &tex.resource().cast().map_err(|error| error.to_string())?)?;
        let raw = open(&Self::raw_device(vello)?, handle)?;
        Ok(Target { tex, raw })
    }

    /// `target`'s texture as wgpu's, as it is between submissions
    fn wrap(&self, target: &Target, usage: wgpu::TextureUsages) -> wgpu::Texture {
        let size = wgpu::Extent3d { width: target.tex.width as u32, height: target.tex.height as u32, depth_or_array_layers: 1 };
        let hal = unsafe { <Dx12 as wgpu::hal::Api>::Device::texture_from_raw(target.raw.clone(), wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureDimension::D2, size, 1, 1) };
        let desc = wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage,
            view_formats: &[],
        };
        unsafe { self.device.create_texture_from_hal::<Dx12>(hal, &desc, wgpu::TextureUses::UNINITIALIZED) }
    }

    /// Page `index` of the atlas, for Skia to draw into, at least `width` by
    /// `height`
    fn page(&mut self, skia: &ID3D12Device, index: usize, width: i32, height: i32) -> Result<Arc<Tex>, String> {
        if self.pages.first().is_some_and(|page| page.target.tex.width < width || page.target.tex.height < height) {
            self.pages.clear();
        }
        while self.pages.len() <= index {
            let target = Self::make(skia, &self.device, width, height)?;
            self.pages.push(Page { target, used: std::time::Instant::now() });
        }
        let page = &mut self.pages[index];
        page.used = std::time::Instant::now();
        Ok(page.target.tex.clone())
    }

    /// Draws `scene` over `width` by `height` pixels into the texture it
    /// returns, unpremultiplied, once Skia's queue has drawn the atlas.
    /// Skia's queue waits for it before what's submitted after
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        skia: &ID3D12Device,
        skia_queue: &ID3D12CommandQueue,
        scene: &vello::Scene,
        width: i32,
        height: i32,
        pages: usize,
        pictures: &[(String, u32)],
        plan: &Plan,
        kept: Kept,
    ) -> Result<Arc<Tex>, String> {
        let _span = super::span::span("vello submit");
        let out = Self::target(skia, &self.device, &mut self.out, width, height)?;
        self.signalled += 1;
        let value = self.signalled;
        self.ready(skia_queue, pictures, value)?;
        unsafe { skia_queue.Signal(&self.atlas_drawn.skia, value) }.map_err(|error| format!("signal: {error}"))?;
        let out_texture = self.wrap(self.out.as_ref().ok_or("no target")?, wgpu::TextureUsages::STORAGE_BINDING);
        let textures: Vec<wgpu::Texture> = (0..pages.min(self.pages.len())).map(|index| self.wrap(&self.pages[index].target, wgpu::TextureUsages::COPY_SRC)).collect();
        for (image, page, x, y) in &plan.skia {
            let Some(texture) = textures.get(*page) else { continue };
            let origin = wgpu::Origin3d { x: *x as u32, y: *y as u32, z: 0 };
            let base = wgpu::TexelCopyTextureInfoBase { texture: texture.clone(), mip_level: 0, origin, aspect: wgpu::TextureAspect::All };
            self.renderer.override_image(image, Some(base));
        }
        // Pages no view has needed for a while go
        while self.pages.len() > pages && self.pages.last().is_some_and(|page| page.used.elapsed() > PAGE_KEPT) {
            self.pages.pop();
        }
        {
            let hal = unsafe { self.queue.as_hal::<Dx12>() }.ok_or("not D3D12")?;
            hal.add_wait_fence(self.atlas_drawn.vello.clone(), value);
        }
        let view = out_texture.create_view(&Default::default());
        let params = vello::RenderParams {
            base_color: AlphaColor::<Srgb>::new([0.0, 0.0, 0.0, 0.0]),
            width: width as u32,
            height: height as u32,
            antialiasing_method: if std::env::var_os("LIGMA_NATIVE_VELLO_AREA").is_some() {
                vello::AaConfig::Area
            } else if std::env::var_os("LIGMA_NATIVE_VELLO_MSAA4").is_some() {
                vello::AaConfig::Msaa4
            } else {
                // 16 samples a pixel, as Figma draws
                vello::AaConfig::Msaa16
            },
            view: kept.view,
            retain: kept.id,
            reuse: kept.again,
            one_off: kept.one_off,
        };
        let at = std::time::Instant::now();
        let dropped = vello::low_level::vello_encoding::DROPPED_IMAGES.load(std::sync::atomic::Ordering::Relaxed);
        let unchecked = self.renderer.unchecked();
        self.stages.clear();
        self.sync("start");
        let blurred = self.blur(plan, params.antialiasing_method);
        self.took_blurs = at.elapsed();
        let rendered = blurred.and_then(|()| self.renderer.render_to_texture(&self.device, &self.queue, scene, &view, &params).map_err(|error| format!("render: {error}")));
        self.stages += &format!(" record={:.1} run={:.1}", self.renderer.timing.0, self.renderer.timing.1);
        self.sync("main");
        self.renders = self.renderer.unchecked() - unchecked;
        for depth in &plan.depths {
            for blur in &depth.blurs {
                self.renderer.override_image(&blur.image, None);
            }
        }
        for (image, ..) in &plan.skia {
            self.renderer.override_image(image, None);
        }
        self.took = at.elapsed();
        let left_out = vello::low_level::vello_encoding::DROPPED_IMAGES.load(std::sync::atomic::Ordering::Relaxed) != dropped;
        // Signalled after all Vello submitted, on a submission of its own
        {
            let hal = unsafe { self.queue.as_hal::<Dx12>() }.ok_or("not D3D12")?;
            hal.add_signal_fence(self.view_drawn.vello.clone(), value);
        }
        self.queue.submit([self.device.create_command_encoder(&Default::default()).finish()]);
        let _ = self.device.poll(wgpu::PollType::Poll);
        unsafe { skia_queue.Wait(&self.view_drawn.skia, value) }.map_err(|error| format!("wait: {error}"))?;
        rendered?;
        // Drawn without an image its atlas had no room for
        if left_out {
            return Err("images past Vello's atlas".into());
        }
        Ok(out)
    }

    /// Once the GPU's drawn what's been rendered, what ran out of room in
    /// Vello's buffers if any of it did, which are made bigger for it to be
    /// rendered again
    /// of the first `renders` not yet checked, waiting for those alone
    pub fn check(&mut self, renders: usize) -> Result<Option<String>, String> {
        let _span = super::span::span("vello wait for GPU");
        let at = std::time::Instant::now();
        let fits = self.renderer.check(&self.device, renders).map_err(|error| format!("check: {error}"));
        self.waited = at.elapsed();
        self.stages += &self.renderer.times();
        // Dev tools: what Vello's memory holds
        if let Some(report) = std::env::var_os("LIGMA_NATIVE_VELLO_MEMORY").and_then(|_| self.device.generate_allocator_report()) {
            let mut named: HashMap<String, (u64, u32)> = HashMap::new();
            for allocation in &report.allocations {
                let entry = named.entry(allocation.name.clone()).or_default();
                entry.0 += allocation.size;
                entry.1 += 1;
            }
            let mut named: Vec<_> = named.into_iter().collect();
            named.sort_by_key(|(_, (size, _))| std::cmp::Reverse(*size));
            self.stages += &format!(" memory={}/{}MB", report.total_allocated_bytes >> 20, report.total_reserved_bytes >> 20);
            for (name, (size, count)) in named.iter().take(12) {
                self.stages += &format!(" [{name} {}MB x{count}]", size >> 20);
            }
        }
        fits
    }

    /// Dev tools: waits for the GPU, noting how long it took
    fn sync(&mut self, stage: &str) {
        if std::env::var_os("LIGMA_NATIVE_VELLO_SYNC").is_none() {
            return;
        }
        let at = std::time::Instant::now();
        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        self.stages += &format!(" {stage}={:.1}", at.elapsed().as_secs_f64() * 1000.0);
    }

    /// Draws the sources of `plan`'s blurs and blurs them, the deepest in
    /// other blurs first, each standing in as an image where it's drawn
    fn blur(&mut self, plan: &Plan, antialiasing_method: vello::AaConfig) -> Result<(), String> {
        let _span = super::span::span("vello blurs");
        for (index, depth) in plan.depths.iter().enumerate().rev() {
            if depth.blurs.is_empty() {
                continue;
            }
            let (width, height) = (depth.used.max(1) as u32, (depth.shelf.y + depth.shelf.row).max(1) as u32);
            while self.blurred.len() <= index {
                self.blurred.push(None);
            }
            let fits = self.blurred[index].as_ref().is_some_and(|(texture, _)| texture.width() >= width && texture.height() >= height);
            if !fits {
                let step = |size: u32| size.div_ceil(256) * 256;
                let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("ligma blurred"),
                    size: wgpu::Extent3d { width: step(width), height: step(height), depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                self.blurred[index] = Some((texture, std::time::Instant::now()));
            }
            let Some((texture, used)) = self.blurred[index].as_mut() else { return Err("no blur texture".into()) };
            *used = std::time::Instant::now();
            let texture = texture.clone();
            for blur in &depth.blurs {
                let origin = wgpu::Origin3d { x: blur.job.slot[0] as u32, y: blur.job.slot[1] as u32, z: 0 };
                let base = wgpu::TexelCopyTextureInfoBase { texture: texture.clone(), mip_level: 0, origin, aspect: wgpu::TextureAspect::All };
                self.renderer.override_image(&blur.image, Some(base));
            }
            let view = texture.create_view(&Default::default());
            let params = vello::RenderParams { base_color: AlphaColor::<Srgb>::new([0.0, 0.0, 0.0, 0.0]), width, height, antialiasing_method, view: IDENTITY, retain: 0, reuse: false, one_off: false };
            self.renderer.render_to_texture(&self.device, &self.queue, &depth.scene, &view, &params).map_err(|error| format!("blur's source: {error}"))?;
            self.sync("sources");
            let blurrer = self.blurrer.get_or_insert_with(|| Blurrer::new(&self.device));
            let mut encoder = self.device.create_command_encoder(&Default::default());
            let jobs: Vec<Job> = depth.blurs.iter().map(|blur| blur.job.clone()).collect();
            blurrer.blur(&self.device, &mut encoder, &texture, &jobs, width)?;
            self.queue.submit([encoder.finish()]);
            self.sync("blurs");
        }
        // Textures for blurs no view has needed for a while go
        for slot in self.blurred.iter_mut().skip(plan.depths.len()) {
            if slot.as_ref().is_some_and(|(_, used)| used.elapsed() > PAGE_KEPT) {
                *slot = None;
            }
        }
        Ok(())
    }

    /// Mipmap level `level` of `img`, image `key`, as Vello draws it, its
    /// texture Skia's. None where Vello can't copy it, or the image has no
    /// such level
    pub fn picture(&mut self, key: &str, img: &Img, level: u32) -> Option<ImageData> {
        let _span = super::span::span("vello picture");
        // Dev tools: images as Skia draws them
        if std::env::var_os("LIGMA_NATIVE_VELLO_SKIA_IMAGES").is_some() {
            return None;
        }
        let id = img.image.unique_id();
        let at = (key.to_string(), level);
        if let Some(picture) = self.pictures.get(&at).filter(|picture| picture.id == id) {
            return Some(picture.data.clone());
        }
        if let Some(old) = self.pictures.remove(&at) {
            self.renderer.override_image(&old.data, None);
        }
        let (backend, origin) = skia_safe::gpu::images::get_backend_texture_from_image(&img.image, false)?;
        let info = backend.d3d_texture_resource_info()?;
        if level >= info.level_count.max(1) {
            return None;
        }
        let (full_width, full_height) = (backend.width() as u32, backend.height() as u32);
        let (width, height) = ((full_width >> level).max(1), (full_height >> level).max(1));
        // As Vello's atlas takes them
        if origin != skia_safe::gpu::SurfaceOrigin::TopLeft
            || info.format != windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM
            || info.sample_count > 1
            || width.max(height) > 8192
        {
            return None;
        }
        let size = wgpu::Extent3d { width: full_width, height: full_height, depth_or_array_layers: 1 };
        let levels = info.level_count.max(1);
        let hal = unsafe { <Dx12 as wgpu::hal::Api>::Device::texture_from_raw(info.resource.clone(), wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureDimension::D2, size, levels, 1) };
        let desc = wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        };
        // Always in the copy source state when Vello's GPU work starts, see
        // `ready`
        let texture = unsafe { self.device.create_texture_from_hal::<Dx12>(hal, &desc, wgpu::TextureUses::COPY_SRC) };
        let data = ImageData {
            data: peniko::Blob::new(Arc::new([0u8; 0])),
            format: peniko::ImageFormat::Rgba8,
            alpha_type: peniko::ImageAlphaType::AlphaPremultiplied,
            width,
            height,
        };
        let base = wgpu::TexelCopyTextureInfoBase { texture, mip_level: level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All };
        self.renderer.override_image(&data, Some(base));
        self.pictures.insert(at, Held { data: data.clone(), backend, resource: info.resource, id });
        Some(data)
    }

    /// Pixels pictures `keys` take in Vello's atlas, with its pages
    pub fn atlas_area(&self, keys: &[(String, u32)], pages: usize) -> u64 {
        let pictures: u64 = keys.iter().filter_map(|key| self.pictures.get(key)).map(|picture| (picture.data.width as u64 + 2) * (picture.data.height as u64 + 2)).sum();
        let pages: u64 = self.pages.iter().take(pages).map(|page| page.target.tex.width as u64 * page.target.tex.height as u64).sum();
        pictures + pages
    }

    /// Gives back the GPU memory kept for drawing the next view, as when
    /// nothing's drawn for a while: the next view takes a few milliseconds
    /// more, to make it again
    pub fn trim(&mut self) {
        let _span = super::span::span("vello trim");
        self.pieces.clear();
        self.piece_bytes = 0;
        self.renderer.trim();
        self.out = None;
        self.pages.clear();
        self.blurred.clear();
        if let Some(blurrer) = &mut self.blurrer {
            blurrer.work = None;
        }
        self.spare = vello::Scene::new();
        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        // Dev tools: what's left
        if let Some(report) = std::env::var_os("LIGMA_NATIVE_VELLO_MEMORY").and_then(|_| self.device.generate_allocator_report()) {
            let names: Vec<String> = report.allocations.iter().map(|allocation| format!("{} {}MB", allocation.name, allocation.size >> 20)).collect();
            super::render::trace(|| format!("vello trimmed to {}/{}MB in {} blocks: {names:?}", report.total_allocated_bytes >> 20, report.total_reserved_bytes >> 20, report.blocks.len()));
        }
    }

    fn keep_piece(&mut self, key: PieceKey, piece: Piece) {
        self.piece_bytes += piece_bytes(&piece);
        if let Some(old) = self.pieces.insert(key, piece) {
            self.piece_bytes -= piece_bytes(&old);
        }
    }

    /// A view begins: pieces no view's had for a while go, and the longest
    /// unused past what's kept
    fn begin_view(&mut self) {
        let _span = super::span::span("vello begin view");
        self.views += 1;
        let views = self.views;
        self.pieces.retain(|_, piece| piece.view + PIECE_VIEWS >= views);
        self.piece_bytes = self.pieces.values().map(piece_bytes).sum();
        if self.piece_bytes > PIECES_KEPT {
            let mut by_age: Vec<(u64, PieceKey)> = self.pieces.iter().map(|(key, piece)| (piece.view, key.clone())).collect();
            by_age.sort_unstable_by_key(|(view, _)| *view);
            for (_, key) in by_age {
                if self.piece_bytes <= PIECES_KEPT {
                    break;
                }
                if let Some(gone) = self.pieces.remove(&key) {
                    self.piece_bytes -= piece_bytes(&gone);
                }
            }
        }
    }

    /// Lets go of pictures of images Skia no longer has as they were
    pub fn keep_pictures(&mut self, images: &HashMap<String, Img>) {
        let gone: Vec<(String, u32)> =
            self.pictures.iter().filter(|((key, _), picture)| images.get(key).is_none_or(|img| img.image.unique_id() != picture.id)).map(|(key, _)| key.clone()).collect();
        for key in gone {
            if let Some(picture) = self.pictures.remove(&key) {
                self.renderer.override_image(&picture.data, None);
            }
        }
    }

    /// Puts the textures of pictures `keys` in the copy source state on
    /// Skia's queue, where Vello's copies take them, and has Skia know
    fn ready(&mut self, skia_queue: &ID3D12CommandQueue, keys: &[(String, u32)], value: u64) -> Result<(), String> {
        let _span = super::span::span("vello ready");
        use windows::Win32::Graphics::Direct3D12::*;
        let mut barriers = Vec::new();
        for key in keys {
            let Some(picture) = self.pictures.get_mut(key) else { continue };
            let Some(info) = picture.backend.d3d_texture_resource_info() else { continue };
            if info.resource_state != D3D12_RESOURCE_STATE_COPY_SOURCE {
                barriers.push(unsafe { super::committed::transition(&picture.resource, info.resource_state, D3D12_RESOURCE_STATE_COPY_SOURCE) });
                picture.backend.set_d3d_resource_state(D3D12_RESOURCE_STATE_COPY_SOURCE.0);
            }
        }
        if barriers.is_empty() {
            return Ok(());
        }
        let done = unsafe { self.atlas_drawn.skia.GetCompletedValue() };
        let allocator = match self.barriers.allocators.iter_mut().find(|(_, sent)| *sent <= done) {
            Some((allocator, sent)) => {
                unsafe { allocator.Reset() }.map_err(|error| format!("allocator: {error}"))?;
                *sent = value;
                allocator.clone()
            }
            None => {
                let allocator: ID3D12CommandAllocator = unsafe { self.skia.0.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT) }.map_err(|error| format!("allocator: {error}"))?;
                self.barriers.allocators.push((allocator.clone(), value));
                allocator
            }
        };
        let list = match &self.barriers.list {
            Some(list) => {
                unsafe { list.Reset(&allocator, None) }.map_err(|error| format!("list: {error}"))?;
                list.clone()
            }
            None => {
                let list: ID3D12GraphicsCommandList = unsafe { self.skia.0.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None) }.map_err(|error| format!("list: {error}"))?;
                self.barriers.list = Some(list.clone());
                list
            }
        };
        unsafe {
            list.ResourceBarrier(&barriers);
            list.Close().map_err(|error| format!("list: {error}"))?;
            skia_queue.ExecuteCommandLists(&[Some(list.cast::<ID3D12CommandList>().map_err(|error| error.to_string())?)]);
        }
        // The barriers hold no references of their own
        for barrier in barriers {
            let transition = std::mem::ManuallyDrop::into_inner(unsafe { barrier.Anonymous.Transition });
            std::mem::forget(transition.pResource);
        }
        Ok(())
    }

    /// What Vello takes of `img`'s texture, where it can draw it: its
    /// mipmap levels and size
    pub fn drawable(img: &Img) -> Option<Drawable> {
        if std::env::var_os("LIGMA_NATIVE_VELLO_SKIA_IMAGES").is_some() {
            return None;
        }
        let (backend, origin) = skia_safe::gpu::images::get_backend_texture_from_image(&img.image, false)?;
        let info = backend.d3d_texture_resource_info()?;
        if origin != skia_safe::gpu::SurfaceOrigin::TopLeft || info.format != windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM || info.sample_count > 1 {
            return None;
        }
        Some(Drawable { id: img.image.unique_id(), levels: info.level_count.max(1), width: backend.width() as u32, height: backend.height() as u32 })
    }
}

/// Vello's copies of the tree's paths, made once, shared by the threads
/// encoding units. Held weakly, as Skia's are (see `Tree`)
#[derive(Default)]
pub struct Paths(std::sync::Mutex<HashMap<usize, (std::sync::Weak<rt::Path>, Arc<BezPath>)>>);

impl Paths {
    /// Vello's copy of `path`
    pub fn path(&self, tree: &Tree, path: &Arc<rt::Path>) -> Arc<BezPath> {
        let key = Arc::as_ptr(path) as usize;
        if let Some((_, made)) = self.0.lock().unwrap().get(&key) {
            return made.clone();
        }
        let made = Arc::new(bez(&tree.skia_path(path)));
        self.0.lock().unwrap().insert(key, (Arc::downgrade(path), made.clone()));
        made
    }

    /// Lets go of copies of paths the tree no longer has
    pub fn prune(&self) {
        self.0.lock().unwrap().retain(|_, (path, _)| path.strong_count() > 0);
    }
}

/// An image's texture as Vello can take it (see `Vello::drawable`)
#[derive(Clone, Copy)]
pub struct Drawable {
    id: u32,
    levels: u32,
    width: u32,
    height: u32,
}

/// Skia's `path` as Vello's
pub fn bez(path: &Path) -> BezPath {
    let mut out = BezPath::new();
    let p = |point: Point| kurbo::Point::new(point.x as f64, point.y as f64);
    let mut quads = [Point::default(); 9];
    for rec in path.iter() {
        let points = rec.points();
        match rec.verb() {
            PathVerb::Move => out.move_to(p(points[0])),
            PathVerb::Line => out.line_to(p(points[1])),
            PathVerb::Quad => out.quad_to(p(points[1]), p(points[2])),
            PathVerb::Conic => match Path::convert_conic_to_quads(points[0], points[1], points[2], rec.conic_weight(), &mut quads, 2) {
                Some(count) => {
                    for quad in 0..count {
                        out.quad_to(p(quads[1 + quad * 2]), p(quads[2 + quad * 2]));
                    }
                }
                None => out.line_to(p(points[2])),
            },
            PathVerb::Cubic => out.curve_to(p(points[1]), p(points[2]), p(points[3])),
            PathVerb::Close => out.close_path(),
        }
    }
    out
}

pub fn affine(m: &Matrix) -> Affine {
    Affine::new([m.scale_x() as f64, m.skew_y() as f64, m.skew_x() as f64, m.scale_y() as f64, m.translate_x() as f64, m.translate_y() as f64])
}

fn fill_of(path: &Path) -> Fill {
    match path.fill_type() {
        PathFillType::EvenOdd | PathFillType::InverseEvenOdd => Fill::EvenOdd,
        _ => Fill::NonZero,
    }
}

fn color(c: Color4f) -> AlphaColor<Srgb> {
    AlphaColor::new([c.r, c.g, c.b, c.a])
}

/// Source over, for a layer that starts as a copy of what's under it. Ligma's
/// Vello shaders take this compose mode, otherwise unused, for that
const OVER_COPY: BlendMode = BlendMode { mix: Mix::Normal, compose: Compose::SrcAtop };

/// `mode` as Vello's, where Ligma's shaders take Figma's soft light for the
/// W3C's, and compose modes otherwise unused for its linear burn and dodge
fn mix(mode: Option<rt::BlendMode>) -> Option<BlendMode> {
    use rt::BlendMode::*;
    let mix = match mode {
        Option::None | Some(None) | Some(PassThrough) | Some(Normal) | Some(Unknown(_)) => return Option::None,
        Some(LinearBurn) => return Some(BlendMode { mix: Mix::Normal, compose: Compose::Xor }),
        Some(LinearDodge) => return Some(BlendMode { mix: Mix::Normal, compose: Compose::DestAtop }),
        Some(SoftLight) => Mix::SoftLight,
        Some(Darken) => Mix::Darken,
        Some(Multiply) => Mix::Multiply,
        Some(ColorBurn) => Mix::ColorBurn,
        Some(Lighten) => Mix::Lighten,
        Some(Screen) => Mix::Screen,
        Some(ColorDodge) => Mix::ColorDodge,
        Some(Overlay) => Mix::Overlay,
        Some(HardLight) => Mix::HardLight,
        Some(Difference) => Mix::Difference,
        Some(Exclusion) => Mix::Exclusion,
        Some(Hue) => Mix::Hue,
        Some(Saturation) => Mix::Saturation,
        Some(Color) => Mix::Color,
        Some(Luminosity) => Mix::Luminosity,
    };
    Some(BlendMode::new(mix, Compose::SrcOver))
}

/// Places rectangles on a page of the atlas in rows
struct Shelf {
    width: i32,
    height: i32,
    x: i32,
    y: i32,
    row: i32,
}

impl Shelf {
    fn place(&mut self, width: i32, height: i32) -> Option<(i32, i32)> {
        if width > self.width || height > self.height {
            return None;
        }
        if self.x + width > self.width {
            self.x = 0;
            self.y += self.row;
            self.row = 0;
        }
        if self.y + height > self.height {
            return None;
        }
        let at = (self.x, self.y);
        self.x += width + 1;
        self.row = self.row.max(height + 1);
        Some(at)
    }
}

/// What drawing a unit asked about images, as a drawer's lists
#[derive(Default, Clone)]
pub struct Asked {
    pub missing: Vec<(String, f64)>,
    pub sharper: Vec<(String, f64)>,
    pub used: Vec<(String, f64)>,
}

/// A unit encoded once for every zoom from `2^level` up to twice that,
/// placed in a view by `Fragment::at`
pub struct Fragment {
    pub scene: vello::Scene,
    pub level: i32,
    /// The unit's corner, in units, which the scene's origin is
    pub origin: (f32, f32),
    pub asked: Asked,
    pub counts: Counts,
    /// Image fills' textures it draws, by key, mipmap level and Vello's id
    /// for them
    pub pictures: Vec<(String, u32, u64)>,
    /// Its blurs, which each view draws and puts in place
    pub effects: Vec<Effect>,
}

impl Fragment {
    /// What places it in a view at `px` whose corner is at (`left`, `top`)
    /// in device pixels
    pub fn at(&self, px: f32, left: f64, top: f64) -> Affine {
        let ratio = px as f64 / 2f64.powi(self.level);
        Affine::translate((self.origin.0 as f64 * px as f64 - left, self.origin.1 as f64 * px as f64 - top)) * Affine::scale(ratio)
    }
}

/// What a node covers, as paths in device pixels
enum Cover {
    /// Where any of them is
    Any(Vec<Path>),
    /// Where all of them are, each a path or where it isn't
    All(Vec<(Path, bool)>),
}

/// Where `shape` isn't, inside `device`, as one path: `device` and
/// `shape` filled even-odd, where `shape`'s filled even-odd or is one
/// convex contour, so covers nothing twice. Skia's simplifying it to that
/// otherwise moves its edges
fn outside(shape: &Path, device: IRect) -> Option<Path> {
    let simple = shape.fill_type() == PathFillType::EvenOdd || (shape.is_convex() && shape.iter().filter(|v| v.verb() == skia_safe::PathVerb::Move).count() <= 1);
    if !simple {
        return None;
    }
    let mut out = skia_safe::PathBuilder::new();
    out.add_path(shape, None);
    out.add_rect(Rect::from(device), None, None);
    let mut out = out.detach();
    out.set_fill_type(PathFillType::EvenOdd);
    Some(out)
}

/// `shape` drawn by `transform`, as lines no further than `tolerance` from
/// it, where it's small enough that's fewer than it has, or as the cells
/// of a grid `cells` to a pixel it covers, filled by `style`, where that's
/// fewer still
fn simplified(shape: &impl kurbo::Shape, transform: Affine, tolerance: f64, style: Fill, cells: f64) -> Option<BezPath> {
    // Half the tolerance flattening, half leaving points out
    let half = tolerance / 2.0;
    // Its control points' bounds, which hold it, and how many parts it has
    let mut had = 0;
    let (mut low, mut high) = (kurbo::Point::new(f64::MAX, f64::MAX), kurbo::Point::new(f64::MIN, f64::MIN));
    for element in shape.path_elements(0.1) {
        had += 1;
        element_points(&(transform * element), |point| {
            (low.x, low.y, high.x, high.y) = (low.x.min(point.x), low.y.min(point.y), high.x.max(point.x), high.y.max(point.y));
        });
    }
    if had == 0 || (high.x - low.x).max(high.y - low.y) > 64.0 {
        return None;
    }
    SCRATCH.with_borrow_mut(|(line, keep, kept, spans)| {
        let mut out = BezPath::new();
        let mut close = |line: &mut Vec<kurbo::Point>, out: &mut BezPath| {
            if line.len() >= 3 {
                let first = line[0];
                if line.last() != Some(&first) {
                    line.push(first);
                }
                fewer(line, half, keep, kept, spans);
                if keep.len() >= 4 {
                    out.move_to(line[keep[0]]);
                    for &index in &keep[1..keep.len() - 1] {
                        out.line_to(line[index]);
                    }
                    out.close_path();
                }
            }
            line.clear();
        };
        line.clear();
        let mut last = kurbo::Point::ZERO;
        for element in shape.path_elements(0.1) {
            let element = transform * element;
            match element {
                kurbo::PathEl::MoveTo(p) => {
                    close(line, &mut out);
                    line.push(p);
                    last = p;
                }
                kurbo::PathEl::LineTo(p) => {
                    line.push(p);
                    last = p;
                }
                kurbo::PathEl::QuadTo(c, p) => {
                    // Mostly far under a pixel, so as near as its chord
                    if !flat(last, &[c], p, half) {
                        kurbo::flatten([kurbo::PathEl::MoveTo(last), element], half, |part| {
                            if let kurbo::PathEl::LineTo(q) = part {
                                line.push(q);
                            }
                        });
                    }
                    line.push(p);
                    last = p;
                }
                kurbo::PathEl::CurveTo(c1, c2, p) => {
                    if !flat(last, &[c1, c2], p, half) {
                        kurbo::flatten([kurbo::PathEl::MoveTo(last), element], half, |part| {
                            if let kurbo::PathEl::LineTo(q) = part {
                                line.push(q);
                            }
                        });
                    }
                    line.push(p);
                    last = p;
                }
                kurbo::PathEl::ClosePath => close(line, &mut out),
            }
        }
        close(line, &mut out);
        // Small with many contours, as text far out is, each takes lines
        // however small, which what covers the grid's cells doesn't
        let out = match out.elements().len() > RUNS_FROM && (high.x - low.x).max(high.y - low.y) <= RUNS_MOST {
            true => runs(&out, style, cells).filter(|runs| runs.elements().len() < out.elements().len()).unwrap_or(out),
            false => out,
        };
        (out.elements().len() < had).then_some(out)
    })
}

/// Lines past which a small shape's drawn as the cells it covers
const RUNS_FROM: usize = 96;

/// Pixels across a shape's at most to be drawn as the cells it covers
const RUNS_MOST: f64 = 8.0;

/// What `lines`, closed contours of lines alone, covers filled by `style`,
/// as rectangles of the cells of a grid `cells` to a pixel whose centres
/// it covers, as Vello's samples take them, those of rows alike merged
fn runs(lines: &BezPath, style: Fill, cells: f64) -> Option<BezPath> {
    let bounds = kurbo::Shape::bounding_box(lines);
    let (x0, y0) = ((bounds.x0 * cells).floor() / cells, (bounds.y0 * cells).floor() / cells);
    let rows = ((bounds.y1 - y0) * cells).ceil().max(1.0) as usize;
    let columns = ((bounds.x1 - x0) * cells).ceil().max(1.0) as i32;
    if rows > 256 || columns > 256 {
        return None;
    }
    // Where each edge crosses each row's centre, and which way
    let mut crossings: Vec<Vec<(f64, i32)>> = vec![Vec::new(); rows];
    let mut edge = |a: kurbo::Point, b: kurbo::Point| {
        if a.y == b.y {
            return;
        }
        let (low, high, way) = if a.y < b.y { (a, b, 1) } else { (b, a, -1) };
        let first = (((low.y - y0) * cells - 0.5).ceil().max(0.0)) as usize;
        let last = (((high.y - y0) * cells - 0.5).ceil().max(0.0) as usize).min(rows);
        for row in first..last {
            let y = y0 + (row as f64 + 0.5) / cells;
            crossings[row].push((low.x + (y - low.y) / (high.y - low.y) * (high.x - low.x), way));
        }
    };
    let (mut start, mut last) = (kurbo::Point::ZERO, kurbo::Point::ZERO);
    for element in lines.elements() {
        match *element {
            kurbo::PathEl::MoveTo(p) => {
                edge(last, start);
                (start, last) = (p, p);
            }
            kurbo::PathEl::LineTo(p) => {
                edge(last, p);
                last = p;
            }
            kurbo::PathEl::ClosePath => {
                edge(last, start);
                last = start;
            }
            _ => return None,
        }
    }
    edge(last, start);
    // Each row's covered cells, as runs from one to before another
    let cell = |x: f64| (((x - x0) * cells - 0.5).ceil() as i32).clamp(0, columns);
    let mut out = BezPath::new();
    let mut rect = |from: i32, to: i32, top: usize, bottom: usize| {
        let (left, right) = (x0 + from as f64 / cells, x0 + to as f64 / cells);
        let (top, bottom) = (y0 + top as f64 / cells, y0 + bottom as f64 / cells);
        out.move_to((left, top));
        out.line_to((right, top));
        out.line_to((right, bottom));
        out.line_to((left, bottom));
        out.close_path();
    };
    let (mut open, mut since): (Vec<(i32, i32)>, usize) = (Vec::new(), 0);
    let mut spans = Vec::new();
    for (row, list) in crossings.iter_mut().enumerate() {
        list.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        spans.clear();
        let mut winding = 0;
        for pair in list.windows(2) {
            winding += pair[0].1;
            let inside = match style {
                Fill::NonZero => winding != 0,
                Fill::EvenOdd => winding % 2 != 0,
            };
            let (from, to) = (cell(pair[0].0), cell(pair[1].0));
            if inside && to > from {
                match spans.last_mut() {
                    Some((_, end)) if *end == from => *end = to,
                    _ => spans.push((from, to)),
                }
            }
        }
        if spans != open {
            for &(from, to) in &open {
                rect(from, to, since, row);
            }
            open.clone_from(&spans);
            since = row;
        }
    }
    for &(from, to) in &open {
        rect(from, to, since, rows);
    }
    Some(out)
}

thread_local! {
    /// What `simplified` works in, kept for the next
    static SCRATCH: std::cell::RefCell<(Vec<kurbo::Point>, Vec<usize>, Vec<bool>, Vec<(usize, usize)>)> = const { std::cell::RefCell::new((Vec::new(), Vec::new(), Vec::new(), Vec::new())) };
}

fn element_points(element: &kurbo::PathEl, mut f: impl FnMut(kurbo::Point)) {
    match *element {
        kurbo::PathEl::MoveTo(p) | kurbo::PathEl::LineTo(p) => f(p),
        kurbo::PathEl::QuadTo(a, b) => {
            f(a);
            f(b);
        }
        kurbo::PathEl::CurveTo(a, b, c) => {
            f(a);
            f(b);
            f(c);
        }
        kurbo::PathEl::ClosePath => {}
    }
}

/// Whether the curve from `from` to `to` by `controls` is no further than
/// `tolerance` from its chord, its controls being so, alongside it
fn flat(from: kurbo::Point, controls: &[kurbo::Point], to: kurbo::Point, tolerance: f64) -> bool {
    let d = to - from;
    let length2 = d.hypot2();
    controls.iter().all(|&c| {
        let off = c - from;
        if length2 <= 1e-18 {
            return off.hypot2() <= tolerance * tolerance;
        }
        let cross = off.x * d.y - off.y * d.x;
        let along = off.x * d.x + off.y * d.y;
        let length = length2.sqrt();
        cross * cross <= tolerance * tolerance * length2 && along >= -tolerance * length && along <= length2 + tolerance * length
    })
}

/// The indices of the points of the closed `line` that keep it within
/// `tolerance` of itself, by Douglas and Peucker's way
fn fewer(line: &[kurbo::Point], tolerance: f64, keep: &mut Vec<usize>, kept: &mut Vec<bool>, spans: &mut Vec<(usize, usize)>) {
    keep.clear();
    let last = line.len() - 1;
    // A closed line splits at the point furthest from its start first
    let far = (1..last).max_by(|&a, &b| (line[a] - line[0]).hypot2().total_cmp(&(line[b] - line[0]).hypot2())).unwrap_or(0);
    kept.clear();
    kept.resize(line.len(), false);
    kept[0] = true;
    kept[last] = true;
    if (line[far] - line[0]).hypot() <= tolerance {
        return;
    }
    kept[far] = true;
    spans.clear();
    spans.extend([(0, far), (far, last)]);
    while let Some((a, b)) = spans.pop() {
        if b <= a + 1 {
            continue;
        }
        let (p, q) = (line[a], line[b]);
        let d = q - p;
        let length = d.hypot();
        let (mut worst, mut at) = (0.0, a);
        for (index, &point) in line.iter().enumerate().take(b).skip(a + 1) {
            let off = point - p;
            let distance = if length > 0.0 { (off.x * d.y - off.y * d.x).abs() / length } else { off.hypot() };
            if distance > worst {
                worst = distance;
                at = index;
            }
        }
        if worst > tolerance {
            kept[at] = true;
            spans.push((a, at));
            spans.push((at, b));
        }
    }
    keep.extend(kept.iter().enumerate().filter(|(_, &k)| k).map(|(index, _)| index));
}

    /// Dev tools: what node `id` is, a few levels down
fn shape_of(tree: &Tree, id: u32, depth: u32) -> String {
        let Some(node) = tree.node(id) else { return "?".into() };
        let mut out = format!("{:?}", node.op);
        if let Some(geometry) = &node.geometry {
            out += &format!("[{}]", geometry.paints.iter().map(|p| format!("{:?}{:?}", p.kind.unwrap_or(PaintType::Solid), p.color.map(|c| (c.a * 100.0) as i32))).collect::<Vec<_>>().join(","));
        }
        if !node.fill_geometries.is_empty() || !node.stroke_geometries.is_empty() {
            out += &format!("[fills {} strokes {} {:?}]", node.fill_geometries.iter().map(|g| g.paints.len()).sum::<usize>(), node.stroke_geometries.iter().map(|g| g.paints.len()).sum::<usize>(), node.stroke_mode);
        }
        if depth < 3 {
            let kids: Vec<u32> = node.children.iter().copied().chain(node.child).collect();
            if !kids.is_empty() {
                out += &format!("({})", kids.iter().take(4).map(|&k| shape_of(tree, k, depth + 1)).collect::<Vec<_>>().join(" "));
                if kids.len() > 4 {
                    out += &format!("+{}", kids.len() - 4);
                }
            }
        }
        out
    }

/// Dev tools: fills and their segments by size in device pixels, under
/// 1/4, 1, 4, 16, 64, 256 and over
/// 1/4, 1, 4, 16, 64, 256 and over, then layers pushed by what for
/// What leaves a scene where it is
const IDENTITY: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// How a render keeps its scene on the GPU, or draws the one kept again
#[derive(Clone, Copy)]
pub struct Kept {
    /// What it's kept as, or 0 for it not to be
    pub id: u64,
    /// The kept scene drawn again, rather than the one passed
    pub again: bool,
    /// Where the scene's put in the target
    pub view: [f32; 6],
    /// Takes far more of Vello's buffers than the view, see
    /// `vello::RenderParams::one_off`
    pub one_off: bool,
}

impl Kept {
    /// Neither kept nor drawn again
    pub const NOT: Kept = Kept { id: 0, again: false, view: IDENTITY, one_off: false };

    /// Kept as `id`, its corner put at (`x`, `y`) in the target
    pub fn at(id: u64, again: bool, x: f32, y: f32) -> Kept {
        Kept { id, again, view: [1.0, 0.0, 0.0, 1.0, x, y], one_off: false }
    }
}

/// Dev switches read while encoding, read once as there are many encoded
struct Switches {
    sizes: bool,
    nolayers: bool,
    whole: bool,
    shapes: bool,
    tolerance: Option<f64>,
}

fn switches() -> &'static Switches {
    static ON: std::sync::OnceLock<Switches> = std::sync::OnceLock::new();
    ON.get_or_init(|| Switches {
        sizes: std::env::var_os("LIGMA_NATIVE_VELLO_SIZES").is_some(),
        nolayers: std::env::var_os("LIGMA_NATIVE_VELLO_NOLAYERS").is_some(),
        whole: std::env::var_os("LIGMA_NATIVE_VELLO_WHOLE").is_none(),
        shapes: std::env::var_os("LIGMA_NATIVE_VELLO_SHAPES").is_some(),
        tolerance: std::env::var("LIGMA_NATIVE_VELLO_TOL").ok().and_then(|t| t.parse().ok()),
    })
}

/// Dev tools: layers and clips encoded, by the line that made them
pub static LAYERS: std::sync::Mutex<Option<HashMap<u32, u64>>> = std::sync::Mutex::new(None);

#[track_caller]
fn count_layer() {
    if switches().sizes {
        *LAYERS.lock().unwrap().get_or_insert_with(HashMap::new).entry(std::panic::Location::caller().line()).or_default() += 1;
    }
}

pub static SIZES: [std::sync::atomic::AtomicU64; 20] = [const { std::sync::atomic::AtomicU64::new(0) }; 20];
pub static TIMES: [std::sync::atomic::AtomicU64; 12] = [const { std::sync::atomic::AtomicU64::new(0) }; 12];
pub struct Timer(usize, std::time::Instant);
impl Drop for Timer {
    fn drop(&mut self) {
        TIMES[self.0].fetch_add(self.1.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    }
}
pub fn timer(i: usize) -> Timer {
    Timer(i, std::time::Instant::now())
}

/// The level of fragments drawn at `px`
pub fn level(px: f32) -> i32 {
    px.log2().floor().clamp(-16.0, 16.0) as i32
}

/// What's drawn in a scene for each view, in the scene's space: an image
/// of the view's pixels, which the view puts in place by changing the image
/// and its transform in the scene
pub struct Effect {
    kind: Kind,
    /// Where it shows
    bounds: kurbo::Rect,
    /// Its image's transform and patch in the scene's encoding
    transform: usize,
    patch: usize,
}

enum Kind {
    /// A blur of `source`, a scene of its own, which each view draws at the
    /// zoom it's at and blurs
    Blur {
        source: vello::Scene,
        /// Blurs and what Skia draws in the source
        effects: Vec<Effect>,
        /// Where its source is
        reach: kurbo::Rect,
        sigma: (f64, f64),
        /// The area it's clamped to, if it is
        clamp: Option<kurbo::Rect>,
        /// Turns the frame it's blurred in, along whose axes the blur is,
        /// to the scene's space, which `reach`, `clamp`, `sigma` and
        /// `area` are in
        turn: Affine,
        /// Where it shows
        area: kurbo::Rect,
    },
    /// An image fill, `width` device pixels wide in the scene's space, which
    /// each view has drawn from the mipmap level for the zoom it's at
    Image {
        key: String,
        width: f32,
    },
    /// What Skia draws, with `m` from what's drawn to the scene's space,
    /// which each view has it draw onto the atlas at the zoom it's at
    Skia {
        name: &'static str,
        /// Which of what Skia draws it is, unique to it
        id: u64,
        m: Matrix,
        cast: Option<Color4f>,
        draw: SkiaDraw,
    },
}

/// What Skia draws for a view
type SkiaDraw = Box<dyn for<'d> Fn(&mut Drawer<'d>) + Send>;

/// How many times smaller a blur of `sigma` pixels has its source drawn:
/// half what the blur averages it down by (see vello_blur.rs)
fn shrink(sigma: f64) -> i32 {
    let level = if sigma <= 4.0 { 0 } else { (sigma / 4.0).log2().ceil().clamp(0.0, 12.0) as i32 };
    1 << (level - 1).max(0)
}

/// Most blurs inside each other, each a texture
const BLUR_DEPTH: usize = 4;

/// A blur placed in a view, and what stands for it in scenes
struct Placed {
    job: Job,
    image: ImageData,
}

/// Blurs at one depth inside others
struct Depth {
    scene: vello::Scene,
    shelf: Shelf,
    blurs: Vec<Placed>,
    /// How wide what's placed reaches
    used: i32,
}

impl Depth {
    /// Room for `width` by `height`, in rows the view's width, or wider for
    /// one that is
    fn room(&mut self, width: i32, height: i32, soft: i32) -> Option<(i32, i32)> {
        let shelf = &mut self.shelf;
        if shelf.x > 0 && shelf.x + width > soft {
            shelf.x = 0;
            shelf.y += shelf.row;
            shelf.row = 0;
        }
        if shelf.y + height > 8192 {
            return None;
        }
        let at = (shelf.x, shelf.y);
        shelf.x += width + 1;
        shelf.row = shelf.row.max(height + 1);
        self.used = self.used.max(at.0 + width);
        Some(at)
    }
}

/// What a view draws for scenes' effects: blurs' sources, each depth's
/// drawn into a texture of their own, and what Skia drew onto the atlas
pub struct Plan {
    width: i32,
    /// Times blurs' sources are drawn at half again, where they wouldn't
    /// fit their textures otherwise
    pub coarser: u32,
    depths: Vec<Depth>,
    /// What stands for what Skia drew, by page of the atlas and where
    skia: Vec<(ImageData, usize, i32, i32)>,
}

/// What Skia drew for one of `Kind::Skia`, placed in a view by `placed`,
/// its coefficients to a 1024th, as a slot of `width` by `height`
#[derive(Clone, PartialEq, Eq, Hash)]
struct PieceKey {
    id: u64,
    placed: [i64; 6],
    width: i32,
    height: i32,
}

impl PieceKey {
    fn new(id: u64, placed: Affine, width: i32, height: i32) -> Self {
        Self { id, placed: placed.as_coeffs().map(|c| (c * 1024.0).round() as i64), width, height }
    }
}

/// What Skia drew onto the atlas, as it was drawn: its pixels, its
/// counts, the images it drew with and which copies of them, and the last
/// view it was in
struct Piece {
    image: skia_safe::Image,
    counts: Counts,
    used: Vec<(String, f64)>,
    images: Vec<(String, u32)>,
    view: u64,
}

fn piece_bytes(piece: &Piece) -> usize {
    piece.image.width() as usize * piece.image.height() as usize * 4
}

/// Bytes of what Skia drew kept for later views
const PIECES_KEPT: usize = 96 << 20;
/// Views a piece no view's had is kept for
const PIECE_VIEWS: u64 = 8;

/// A transparent pixel, which effects not in a view stand in as
fn nothing() -> ImageData {
    static NOTHING: std::sync::OnceLock<ImageData> = std::sync::OnceLock::new();
    NOTHING
        .get_or_init(|| ImageData {
            data: peniko::Blob::new(Arc::new([0u8; 4])),
            format: peniko::ImageFormat::Rgba8,
            alpha_type: peniko::ImageAlphaType::AlphaPremultiplied,
            width: 1,
            height: 1,
        })
        .clone()
}

/// What stands for a `width` by `height` part of a texture in scenes
fn slot_image(width: i32, height: i32) -> ImageData {
    ImageData {
        data: peniko::Blob::new(Arc::new([0u8; 0])),
        format: peniko::ImageFormat::Rgba8,
        alpha_type: peniko::ImageAlphaType::AlphaPremultiplied,
        width: width as u32,
        height: height as u32,
    }
}

fn to_matrix(t: Affine) -> Matrix {
    let c = t.as_coeffs();
    Matrix::new_all(c[0] as f32, c[2] as f32, c[4] as f32, c[1] as f32, c[3] as f32, c[5] as f32, 0.0, 0.0, 1.0)
}

impl Plan {
    /// Effects in a view `width` wide
    pub fn new(width: i32) -> Self {
        Self { width: width.clamp(256, 8192), coarser: 0, depths: Vec::new(), skia: Vec::new() }
    }

    /// How tall the tallest of its blurs' textures is
    pub fn blur_rows(&self) -> i32 {
        self.depths.iter().map(|depth| depth.shelf.y + depth.shelf.row).max().unwrap_or(0)
    }

    /// How many blurs it has
    pub fn len(&self) -> usize {
        self.depths.iter().map(|depth| depth.blurs.len()).sum()
    }

    /// Has `effect` draw `image`, with `transform` from its pixels to the
    /// scene's space
    fn patch_image(encoding: &mut Encoding, effect: &Effect, image: ImageData) {
        if let Some(Patch::Image { image: was, draw_data_offset }) = encoding.resources.patches.get_mut(effect.patch) {
            encoding.draw_data[*draw_data_offset + 1] = (image.width << 16) | (image.height & 0xFFFF);
            *was = image;
        }
    }

    fn patch(encoding: &mut Encoding, effect: &Effect, image: ImageData, transform: Affine) {
        encoding.transforms[effect.transform] = Transform::from_kurbo(&transform);
        if let Some(Patch::Image { image: was, draw_data_offset }) = encoding.resources.patches.get_mut(effect.patch) {
            encoding.draw_data[*draw_data_offset + 1] = (image.width << 16) | (image.height & 0xFFFF);
            *was = image;
        }
    }
}

/// Opaque black, as silhouettes only masks take are drawn
const BLACK: Color4f = Color4f::new(0.0, 0.0, 0.0, 1.0);

/// Builds a Vello scene of units of the tree, in device pixels of the view
pub struct Encoder<'a> {
    pub tree: &'a Tree,
    pub images: &'a HashMap<String, Img>,
    /// What only a view's drawn with, which threads encoding units don't
    /// have
    kit: Option<&'a Kit>,
    pub vello: Option<&'a mut Vello>,
    pub scene: vello::Scene,
    /// Levels coarser than the view needs images are drawn at, so they fit
    /// Vello's atlas where there are too many at the size the view needs
    pub coarser: u32,
    skia: Option<&'a ID3D12Device>,
    context: Option<&'a mut skia_safe::gpu::DirectContext>,
    paths: &'a Paths,
    /// The images Vello can draw, by key
    drawable: &'a HashMap<String, Drawable>,
    /// Skia's surfaces over the pages of the atlas used
    pages: Vec<skia_safe::Surface>,
    shelf: Shelf,
    /// The view's pixels
    view: IRect,
    m: Matrix,
    pub counts: Counts,
    pub asked: Asked,
    /// Images views have drawn, at the levels they drew them
    pub drawn: Vec<(String, u32)>,
    /// And how wide they showed, in device pixels
    pub shown: Vec<(String, f32)>,
    /// Why the view can't be drawn this way, if it can't
    pub failed: Option<String>,
    /// Pixels Skia drew into the atlas
    pub fell_back: i64,
    /// Dev tools: masks drawn as if what's in them didn't blend
    pub inexact: usize,
    /// Time Skia took drawing onto the atlas
    pub skia_took: std::time::Duration,
    /// Dev tools: what Skia drew, how many and their pixels
    pub kinds: std::collections::BTreeMap<&'static str, (u32, i64, u32)>,
    /// Cuts each thing drawn, in device pixels (see `soften`)
    soft: Option<Path>,
    /// The soft clip to put back as each layer's popped
    /// The clip before each layer or clip, and whether Vello has a layer
    /// for it
    saved: Vec<(Option<Path>, bool)>,
    depth: u8,
    /// Encoding a fragment, for any view: nothing's cut by the view
    retained: bool,
    /// The view's pixels a unit, which images fragments draw are asked for
    /// as shown at, not as big as the fragment's ever shown
    pub px: Option<f32>,
    /// How much bigger than encoded the fragment being encoded shows
    shows: f32,
    /// Image fills' textures drawn, by key, mipmap level and Vello's id
    /// for them
    pub pictures: Vec<(String, u32, u64)>,
    /// Drawing what casts a silhouette, in this color: every paint solid
    /// wherever it's at all, as Figma casts shadows
    cast: Option<Color4f>,
    /// Whether only the silhouette's alpha is taken, not its color
    alpha_only: bool,
    /// Blurs in the scene, to be put in place
    pub effects: Vec<Effect>,
}

impl<'a> Encoder<'a> {
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tree: &'a Tree,
        images: &'a HashMap<String, Img>,
        kit: &'a Kit,
        vello: &'a mut Vello,
        paths: &'a Paths,
        drawable: &'a HashMap<String, Drawable>,
        skia: &'a ID3D12Device,
        context: &'a mut skia_safe::gpu::DirectContext,
        width: i32,
        height: i32,
    ) -> Self {
        vello.begin_view();
        let scene = {
            let mut scene = std::mem::take(&mut vello.spare);
            scene.reset();
            scene
        };
        let mut encoder = Self::worker(tree, images, paths, drawable, width, height);
        (encoder.scene, encoder.kit, encoder.vello, encoder.skia, encoder.context) = (scene, Some(kit), Some(vello), Some(skia), Some(context));
        encoder
    }

    /// One for a thread encoding units, which can't draw views
    pub fn worker(tree: &'a Tree, images: &'a HashMap<String, Img>, paths: &'a Paths, drawable: &'a HashMap<String, Drawable>, width: i32, height: i32) -> Self {
        Self {
            tree,
            images,
            kit: None,
            scene: vello::Scene::new(),
            coarser: 0,
            vello: None,
            skia: None,
            context: None,
            paths,
            drawable,
            pages: Vec::new(),
            shelf: Shelf { width: 0, height: 0, x: 0, y: 0, row: 0 },
            view: IRect::from_wh(width, height),
            m: Matrix::default(),
            counts: Counts::default(),
            asked: Asked::default(),
            failed: None,
            fell_back: 0,
            soft: None,
            saved: Vec::new(),
            inexact: 0,
            skia_took: std::time::Duration::ZERO,
            kinds: Default::default(),
            depth: 0,
            retained: false,
            px: None,
            shows: 2.0,
            pictures: Vec::new(),
            drawn: Vec::new(),
            shown: Vec::new(),
            cast: None,
            alpha_only: false,
            effects: Vec::new(),
        }
    }

    /// Skia's surfaces over the pages of the atlas used, to submit
    pub fn take_pages(&mut self) -> Vec<skia_safe::Surface> {
        std::mem::take(&mut self.pages)
    }

    /// Room on the atlas for `width` by `height`, on a page and where on it
    fn room(&mut self, width: i32, height: i32) -> Option<(usize, i32, i32)> {
        loop {
            if let Some((x, y)) = self.shelf.place(width, height) {
                return Some((self.pages.len() - 1, x, y));
            }
            if self.pages.len() >= PAGES {
                return None;
            }
            let (Some(vello), Some(skia)) = (self.vello.as_deref_mut(), self.skia) else { return None };
            let tex = vello.page(skia, self.pages.len(), self.view.width(), self.view.height()).map_err(|error| log::warn!("native renderer: Vello's atlas: {error}")).ok()?;
            let mut surface = skia_safe::gpu::surfaces::wrap_backend_texture(
                self.context.as_deref_mut()?,
                &tex.backend(),
                skia_safe::gpu::SurfaceOrigin::TopLeft,
                1,
                skia_safe::ColorType::RGBA8888,
                None,
                None,
            )?;
            surface.canvas().clear(Color4f::new(0.0, 0.0, 0.0, 0.0));
            self.pages.push(surface);
            self.shelf = Shelf { width: tex.width, height: tex.height, x: 0, y: 0, row: 0 };
        }
    }

    fn fail(&mut self, why: impl Into<String>) {
        if self.failed.is_none() {
            self.failed = Some(why.into());
        }
    }

    fn scale(&self) -> f32 {
        let m = &self.m;
        m.scale_x().hypot(m.skew_y()).max(m.skew_x().hypot(m.scale_y()))
    }

    /// `bounds`, in the current space, as whole device pixels inside the view
    fn device(&self, bounds: Rect) -> Option<IRect> {
        let device: IRect = self.m.map_rect(bounds).0.round_out();
        let device = device.with_outset((1, 1));
        if self.retained {
            return (!device.is_empty()).then_some(device);
        }
        IRect::intersect(&device, &self.view)
    }

    /// A layer over `bounds`, which only bound what's in it, so are whole
    /// pixels, not an edge that's smoothed. Under a soft clip, the layer's
    /// cut by that too, as what's in it is drawn alone
    #[track_caller]
    fn push(&mut self, mode: BlendMode, alpha: f32, bounds: Rect) -> bool {
        count_layer();
        let Some(device) = self.device(bounds) else { return false };
        if switches().nolayers {
            self.saved.push((self.soft.take(), false));
            return true;
        }
        SIZES[14].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SIZES[if mode == BlendMode::default() { 18 } else { 19 }].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        match self.soft.take() {
            Some(soft) => {
                // A mask's DestIn layer only clears inside its own bounds, so
                // what's outside them is cut here
                let rect = Path::rect(Rect::from(device), None);
                let shape = Path::op(&soft, &rect, skia_safe::PathOp::Intersect).unwrap_or(rect);
                self.scene.push_layer(fill_of(&shape), mode, alpha, Affine::IDENTITY, &bez(&shape));
                self.saved.push((Some(soft), true));
            }
            None => {
                let rect = kurbo::Rect::new(device.left as f64, device.top as f64, device.right as f64, device.bottom as f64);
                self.scene.push_layer(Fill::NonZero, mode, alpha, Affine::IDENTITY, &rect);
                self.saved.push((None, true));
            }
        }
        true
    }

    /// Clipped to `device`, a path in device pixels, and any soft clip
    /// Clipped to where `shape`, in device pixels, isn't inside `device`,
    /// where there's no soft clip. False where there is
    #[track_caller]
    fn clip_outside(&mut self, shape: &Path, device: IRect) -> bool {
        count_layer();
        if self.soft.is_some() {
            return false;
        }
        let around = kurbo::Rect::new(device.left as f64, device.top as f64, device.right as f64, device.bottom as f64);
        self.scene.push_clip_layer_outside(fill_of(shape), Affine::IDENTITY, &bez(shape), around);
        SIZES[15].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.saved.push((None, true));
        true
    }

    #[track_caller]
    fn clip(&mut self, device: Path) {
        count_layer();
        let _t = timer(10);
        let saved = self.soft.take();
        let shape = match &saved {
            Some(soft) => Path::op(soft, &device, skia_safe::PathOp::Intersect).unwrap_or(device),
            None => device,
        };
        if switches().nolayers {
            self.saved.push((saved, false));
            return;
        }
        self.scene.push_clip_layer(fill_of(&shape), Affine::IDENTITY, &bez(&shape));
        SIZES[15].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.saved.push((saved, true));
    }

    fn pop(&mut self) {
        let Some((soft, layer)) = self.saved.pop() else { return };
        if layer {
            self.scene.pop_layer();
        }
        self.soft = soft;
    }

    /// What's drawn from now on is cut by `device` too, a path in device
    /// pixels, each thing on its own, as a clip of Vello's would draw what's
    /// in it alone, where what blends wouldn't reach what's under it.
    /// Returns the soft clip before, to put back
    fn soften(&mut self, device: Path) -> Option<Path> {
        let was = self.soft.clone();
        self.soft = Some(match &was {
            Some(soft) => Path::op(soft, &device, skia_safe::PathOp::Intersect).unwrap_or(device),
            None => device,
        });
        was
    }

    /// A fill, cut by the soft clip if there is one
    fn fill<'b>(&mut self, style: Fill, transform: Affine, brush: impl Into<peniko::BrushRef<'b>>, brush_transform: Option<Affine>, shape: &impl kurbo::Shape) {
        // Small, it's drawn with fewer lines no further than this from it,
        // in device pixels for fragments' largest zoom
        let tolerance = switches().tolerance.unwrap_or(if self.retained { 1.0 / 128.0 } else { 1.0 / 64.0 });
        let env = timer(4);
        let whole = switches().whole;
        drop(env);
        if whole {
            let simple = timer(2);
            // Cells a quarter of a pixel as it's shown, as Vello's samples are
            let cells = if self.retained { 8.0 } else { 4.0 };
            let lines = simplified(shape, transform, tolerance, style, cells);
            drop(simple);
            if let Some(lines) = lines {
                // What the brush is drawn by stays as it was
                let brush: peniko::BrushRef<'b> = brush.into();
                let brush_transform = match brush {
                    peniko::BrushRef::Solid(_) => None,
                    _ => Some(transform * brush_transform.unwrap_or(Affine::IDENTITY)),
                };
                return self.fill_shape(style, Affine::IDENTITY, brush, brush_transform, &lines);
            }
        }
        self.fill_shape(style, transform, brush, brush_transform, shape)
    }

    fn fill_shape<'b>(&mut self, style: Fill, transform: Affine, brush: impl Into<peniko::BrushRef<'b>>, brush_transform: Option<Affine>, shape: &impl kurbo::Shape) {
        let _t = timer(3);
        if switches().sizes {
            let b = transform.transform_rect_bbox(shape.bounding_box());
            let size = b.width().max(b.height());
            let bucket = [0.25, 1.0, 4.0, 16.0, 64.0, 256.0].iter().position(|&edge| size < edge).unwrap_or(6);
            let segments = shape.path_elements(0.1).count() as u64;
            SIZES[bucket * 2].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            SIZES[bucket * 2 + 1].fetch_add(segments, std::sync::atomic::Ordering::Relaxed);
        }
        match &self.soft {
            Some(soft) => {
                SIZES[16].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                self.scene.push_clip_layer(fill_of(soft), Affine::IDENTITY, &bez(soft));
                self.scene.fill(style, transform, brush, brush_transform, shape);
                self.scene.pop_layer();
            }
            None => self.scene.fill(style, transform, brush, brush_transform, shape),
        }
    }

    /// Unit `place`, whose corner is at `origin`, as a fragment of `level`
    pub fn fragment(&mut self, place: &Place, origin: (f32, f32), level: i32) -> Fragment {
        let scale = 2f32.powi(level);
        self.shows = self.px.map_or(2.0, |px| (px / scale).clamp(1.0, 2.0));
        let mut post = Matrix::scale((scale, scale));
        post.pre_translate((-origin.0, -origin.1));
        let scene = std::mem::replace(&mut self.scene, vello::Scene::new());
        let (asked, counts, pictures) = (std::mem::take(&mut self.asked), std::mem::take(&mut self.counts), std::mem::take(&mut self.pictures));
        let effects = std::mem::take(&mut self.effects);
        self.retained = true;
        self.place(place, &post);
        self.retained = false;
        let fragment = Fragment {
            scene: std::mem::replace(&mut self.scene, scene),
            level,
            origin,
            asked: std::mem::replace(&mut self.asked, asked),
            counts: std::mem::replace(&mut self.counts, counts),
            pictures: std::mem::replace(&mut self.pictures, pictures),
            effects: std::mem::replace(&mut self.effects, effects),
        };
        fragment
    }

    /// Unit `place`, drawn with `post` after its own matrix
    pub fn place(&mut self, place: &Place, post: &Matrix) {
        self.m = *post;
        let soft = self.tree.info(place.id).blends;
        let was = self.soft.clone();
        let mut clips = 0;
        for (fill, at) in &place.clips {
            if let Some(outline) = self.tree.outline(*fill) {
                let device = outline.with_transform(&Matrix::concat(post, at));
                if soft {
                    self.soften(device);
                } else {
                    self.clip(device);
                    clips += 1;
                }
            }
        }
        self.m = Matrix::concat(post, &place.at);
        self.draw(place.id);
        for _ in 0..clips {
            self.pop();
        }
        self.soft = was;
    }

    fn draw(&mut self, id: u32) {
        if self.depth >= tree::DEPTH || self.failed.is_some() {
            return;
        }
        let _all = (self.depth == 0).then(|| timer(0));
        let clone = timer(1);
        let Some(node) = self.tree.node(id).cloned() else { return };
        drop(clone);
        let Some(bounds) = self.tree.info(id).bounds else { return };
        // A fragment's shown up to twice as big as it's encoded
        let least = if self.retained { 0.25 } else { 0.5 };
        if bounds.width().max(bounds.height()) * self.scale() < least {
            self.counts.skipped += 1;
            return;
        }
        if !self.retained && !Rect::intersects2(&self.m.map_rect(bounds).0, &Rect::from(self.view)) {
            return;
        }
        self.depth += 1;
        self.op(&node, id, bounds);
        self.depth -= 1;
    }

    fn with(&mut self, transform: Option<&rt::Transform>, f: impl FnOnce(&mut Self)) {
        let was = self.m;
        if let Some(transform) = transform {
            self.m = Matrix::concat(&self.m, &rt::to_matrix(transform));
        }
        f(self);
        self.m = was;
    }

    fn op(&mut self, node: &Node, id: u32, bounds: Rect) {
        let ext = node.ext();
        match node.op {
            Op::Group => {
                for &child in &node.children {
                    self.draw(child);
                }
            }
            Op::StaticTransformBinding => {
                let Some(child) = node.child else { return };
                self.with(Some(&node.transform.unwrap_or(rt::IDENTITY)), |this| this.draw(child));
            }
            Op::Fill => {
                self.counts.native += 1;
                let Some(geometry) = &node.geometry else { return };
                let Some(path) = &geometry.path else { return };
                let (skia, bez) = (self.tree.skia_path(path), self.paths.path(self.tree, path));
                self.with(Some(&node.transform.unwrap_or(rt::IDENTITY)), |this| {
                    for paint in &geometry.paints {
                        this.paint(node, paint, &skia, &bez);
                    }
                });
            }
            Op::FillStroke | Op::ScatterBrushFill | Op::ScatterBrushFillStroke => {
                self.counts.native += 1;
                let instances: Vec<Matrix> =
                    if ext.instance_transforms.is_empty() { vec![Matrix::default()] } else { ext.instance_transforms.iter().map(rt::to_matrix).collect() };
                for instance in instances {
                    self.with(Some(&node.transform.unwrap_or(rt::IDENTITY)), |this| {
                        this.m = Matrix::concat(&this.m, &instance);
                        this.fill_stroke(node);
                    });
                }
            }
            Op::FillGlyphs => {
                self.counts.native += 1;
                let Some(glyphs) = &ext.painted_glyph_geometry else { return };
                let glyph_timer = timer(6);
                let mut outline = skia_safe::PathBuilder::new();
                for glyph in &glyphs.glyphs {
                    let Some(path) = &glyph.path else { continue };
                    let [x, y] = glyph.position.unwrap_or([0.0, 0.0]);
                    let size = glyph.font_size.unwrap_or(1.0);
                    let mut m = Matrix::translate((x, y));
                    m.pre_scale((size, -size), None);
                    m.pre_rotate(glyph.rotation.unwrap_or(0.0).to_degrees(), None);
                    outline.add_path(&self.tree.skia_path(path).with_transform(&m), None);
                }
                let outline = outline.detach();
                let shape = bez(&outline);
                drop(glyph_timer);
                self.with(Some(&node.transform.unwrap_or(rt::IDENTITY)), |this| {
                    for paint in &glyphs.paints {
                        this.paint(node, paint, &outline, &shape);
                    }
                });
            }
            Op::Blend => {
                let Some(child) = node.child else { return };
                // A silhouette is solid however see-through what casts it
                if self.cast.is_some() {
                    return self.draw(child);
                }
                let opacity = node.opacity.unwrap_or(1.0).clamp(0.0, 1.0);
                if opacity <= 0.0 {
                    return;
                }
                let isolate = !tree::pass_through(node.blend_mode) || self.tree.info(child).blends;
                if opacity >= 0.999 && !tree::blends(node.blend_mode) && !isolate {
                    self.draw(child);
                    return;
                }
                SIZES[6].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if mix(node.blend_mode).is_none() && !isolate && tree::single_draw(self.tree, child) {
                    SIZES[7].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                if mix(node.blend_mode).is_some() {
                    SIZES[8].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                if self.push(mix(node.blend_mode).unwrap_or_default(), opacity, bounds) {
                    self.draw(child);
                    self.pop();
                }
            }
            Op::Mask => self.mask(node, id, bounds),
            Op::Blur => self.blur(node, id, bounds),
            Op::HardMaskWithColor => {
                let Some(alpha) = node.alpha_node else { return };
                if self.cast.is_some() {
                    return self.draw(alpha);
                }
                let shade = draw::color(tree::rgba(node.color.unwrap_or_default()), 1.0);
                // A silhouette drawn at once comes out the same colored as
                // it's drawn, as a text's shadow is, without a layer each
                if tree::single_draw(self.tree, alpha) {
                    self.cast = Some(shade);
                    self.draw(alpha);
                    self.cast = None;
                    return;
                }
                SIZES[10].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if !self.push(BlendMode::default(), 1.0, bounds) {
                    return;
                }
                self.cast = Some(BLACK);
                self.alpha_only = true;
                self.draw(alpha);
                (self.cast, self.alpha_only) = (None, false);
                if self.push(BlendMode::new(Mix::Normal, Compose::SrcIn), 1.0, bounds) {
                    let rect = kurbo::Rect::new(bounds.left as f64, bounds.top as f64, bounds.right as f64, bounds.bottom as f64);
                    self.fill(Fill::NonZero, affine(&self.m), color(shade), None, &rect);
                    self.pop();
                }
                self.pop();
            }
            // What makes noise or grain, which silhouettes don't take
            Op::Noise | Op::Grain if self.cast.is_some() => {
                if let Some(child) = node.child {
                    self.draw(child);
                }
            }
            Op::CustomEffect if self.cast.is_none() && self.custom_effect(id) => {}
            // Glass, 3D layers and effects that haven't run, drawn as
            // what's in them, as Skia draws them
            Op::CustomEffect | Op::CustomEffectFill | Op::Glass | Op::SpatialPlane | Op::SpatialTransform | Op::SpatialComposition => {
                self.counts.approximate += 1;
                let ext = node.ext();
                let children: Vec<u32> = node
                    .child
                    .into_iter()
                    .chain(node.children.iter().copied())
                    .chain(ext.spatial_plane_content_id)
                    .chain(ext.spatial_transform_child_id)
                    .chain(ext.spatial_composition_child_ids.iter().copied())
                    .collect();
                for child in children {
                    self.draw(child);
                }
            }
            // Noise and grain, as Skia draws them, which take what's in
            // them in a layer of their own, so what blends in them stays in
            // them
            Op::Noise | Op::Grain => self.fall_back(op_name(node.op), false, bounds, Box::new(move |drawer| drawer.draw(id))),
            _ => {
                for child in node.child.into_iter().chain(node.children.iter().copied()) {
                    self.draw(child);
                }
            }
        }
    }

    fn fill_stroke(&mut self, node: &Node) {
        let fills = |this: &mut Self| {
            for geometry in &node.fill_geometries {
                let Some(path) = &geometry.path else { continue };
                let (skia, bez) = (this.tree.skia_path(path), this.paths.path(this.tree, path));
                for paint in &geometry.paints {
                    this.paint(node, paint, &skia, &bez);
                }
            }
        };
        let stroked = node.stroke_geometries.iter().any(|g| !g.paints.is_empty());
        // An outside stroke under fills that hide all of what's inside it
        // needs no clip, whose edge would meet the fill's and leave a seam
        // of what's beneath where both cover part of a pixel
        if stroked && node.stroke_mode == Some(rt::StrokeMask::Outside) && self.fills_hide(node) {
            for geometry in &node.stroke_geometries {
                let Some(path) = &geometry.path else { continue };
                let (skia, bez) = (self.tree.skia_path(path), self.paths.path(self.tree, path));
                for paint in &geometry.paints {
                    self.paint(node, paint, &skia, &bez);
                }
            }
            return fills(self);
        }
        fills(self);
        if !stroked {
            return;
        }
        let outline_timer = timer(7);
        let outlines: Vec<Path> = node.fill_geometries.iter().filter_map(|g| g.path.as_ref()).map(|path| self.tree.skia_path(path)).collect();
        let outline = match outlines.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            [first, rest @ ..] => Some(rest.iter().fold(first.clone(), |all, path| all.op(path, skia_safe::PathOp::Union).unwrap_or(all))),
        };
        drop(outline_timer);
        let strokes = |this: &mut Self| {
            for geometry in &node.stroke_geometries {
                let Some(path) = &geometry.path else { continue };
                let (skia, bez) = (this.tree.skia_path(path), this.paths.path(this.tree, path));
                for paint in &geometry.paints {
                    this.paint(node, paint, &skia, &bez);
                }
            }
        };
        match (node.stroke_mode, outline) {
            (Some(rt::StrokeMask::Inside), Some(outline)) => {
                let device = outline.with_transform(&self.m);
                if self.cast.is_none() && node.stroke_geometries.iter().any(|g| g.paints.iter().any(|paint| tree::blends(paint.blend_mode))) {
                    let was = self.soften(device);
                    strokes(self);
                    self.soft = was;
                } else {
                    self.clip(device);
                    strokes(self);
                    self.pop();
                }
            }
            (Some(rt::StrokeMask::Outside), Some(outline)) => {
                // Kept where the outline isn't, as a clip to its outside
                let bounds = node.stroke_geometries.iter().filter_map(|g| g.path.as_ref()).map(|path| self.tree.skia_path(path).bounds().with_outset((1.0, 1.0))).fold(Rect::new_empty(), |all, b| {
                    let mut all = all;
                    all.join(b);
                    all
                });
                SIZES[12].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let blends = self.cast.is_none() && node.stroke_geometries.iter().any(|g| g.paints.iter().any(|paint| tree::blends(paint.blend_mode)));
                if let (false, Some(device)) = (blends, self.device(bounds)) {
                    if let Some(n) = self.push_clips(vec![(outline.with_transform(&self.m), true)], device) {
                        SIZES[13].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        strokes(self);
                        for _ in 0..n {
                            self.pop();
                        }
                        return;
                    }
                }
                if !self.push(BlendMode::default(), 1.0, bounds) {
                    return;
                }
                strokes(self);
                if self.push(BlendMode::new(Mix::Normal, Compose::DestOut), 1.0, bounds) {
                    let shape = bez(&outline);
                    self.fill(fill_of(&outline), affine(&self.m), AlphaColor::<Srgb>::new([0.0, 0.0, 0.0, 1.0]), None, &shape);
                    self.pop();
                }
                self.pop();
            }
            _ => strokes(self),
        }
    }

    /// Whether `node`'s fills hide anything drawn under them inside its
    /// outline: each of its shapes has a paint that's opaque all over, or
    /// solid in a silhouette drawn opaque, and its strokes don't blend
    fn fills_hide(&mut self, node: &Node) -> bool {
        if node.fill_geometries.is_empty() {
            return false;
        }
        match self.cast {
            Some(cast) if cast.a >= 1.0 => {}
            Some(_) => return false,
            None if node.stroke_geometries.iter().any(|g| g.paints.iter().any(|paint| tree::blends(paint.blend_mode))) => return false,
            None => {}
        }
        let cast = self.cast.is_some();
        node.fill_geometries.iter().all(|g| {
            g.path.is_some()
                && g.paints.iter().any(|paint| {
                    let kind = paint.kind.unwrap_or(PaintType::Solid);
                    let opacity = paint.opacity.unwrap_or(1.0).clamp(0.0, 1.0);
                    if cast {
                        return self.silhouette(node, paint, kind, opacity) == Some(true);
                    }
                    kind == PaintType::Solid && mix(paint.blend_mode).is_none() && !tree::blends(paint.blend_mode) && tree::rgba(paint.color.unwrap_or_default()).a * opacity >= 1.0
                })
        })
    }

    /// Custom effect node `id` as its effects drew it, as tree.rs draws
    /// it: what they drew covers the box they ran over. Till they've run
    /// again, what they drew for it before stands in. False where they
    /// haven't drawn it
    fn custom_effect(&mut self, id: u32) -> bool {
        let Some(effect) = self.tree.custom_effect(id) else { return false };
        let m = Matrix::concat(&self.m, &effect.to);
        let shows = if self.retained { self.shows } else { 1.0 };
        let scale = m.scale_x().hypot(m.skew_y()).max(m.skew_x().hypot(m.scale_y())) * shows;
        let shown = (effect.area.width().max(effect.area.height()) * scale) as f64 * ASKED;
        let own = self.images.get(&effect.key);
        let stand_in_key = if own.is_none() && !self.images.contains_key(&effect.stand_in) { effect.last_drawn.clone() } else { effect.stand_in.clone() };
        let stand_in = if own.is_none() { self.images.get(&stand_in_key) } else { None };
        let Some((key, img)) = own.map(|img| (effect.key.clone(), img)).or(stand_in.map(|img| (stand_in_key.clone(), img))) else {
            self.asked.missing.push((effect.key.clone(), shown));
            return false;
        };
        let Some(area) = img.area else { return false };
        let drawable = self.drawable.get(&key).filter(|drawable| drawable.id == img.image.unique_id());
        if drawable.is_none_or(|d| (d.width.max(d.height)) > 8192) {
            return false;
        }
        if stand_in.is_some() {
            self.asked.missing.push((effect.key.clone(), shown));
            self.asked.used.push((stand_in_key, shown));
        } else if img.stale {
            self.asked.missing.push((effect.key.clone(), shown));
        } else {
            self.asked.used.push((effect.key.clone(), shown));
            // Kept about as big as it shows, in steps of halves of what the
            // effects drew, as the effects thread sizes it
            let (fw, fh) = img.original;
            let longest = fw.max(fh);
            let fits = longest >> super::run::halvings(longest, longest as f64 * shown * 1.25 / fw.max(1) as f64);
            if (img.image.width().max(img.image.height()) as u32) < fits {
                self.asked.sharper.push((effect.key.clone(), shown));
            }
        }
        let at = Rect::from_xywh(effect.area.left, effect.area.top, area.width(), area.height());
        let rect = kurbo::Rect::new(at.left as f64, at.top as f64, at.right as f64, at.bottom as f64);
        let local = Affine::new([rect.width(), 0.0, 0.0, rect.height(), rect.x0, rect.y0]);
        let brush = ImageBrush { image: nothing(), sampler: ImageSampler { x_extend: Extend::Pad, y_extend: Extend::Pad, quality: ImageQuality::High, alpha: 1.0 } };
        // How wide the image shows, in device pixels as it's encoded
        let width = m.scale_x().hypot(m.skew_y()) * at.width();
        let was = std::mem::replace(&mut self.m, m);
        let patches = self.scene.encoding().resources.patches.len();
        self.pictures.push((key.clone(), 0, img.image.unique_id() as u64));
        self.fill(Fill::NonZero, affine(&self.m), &peniko::Brush::Image(brush), Some(local), &rect);
        self.image_drawn(patches, key, width, &Path::rect(at, None));
        self.m = was;
        self.counts.native += 1;
        true
    }

    /// One paint over `path`, in the node's own coordinates
    fn paint(&mut self, node: &Node, paint: &rt::Paint, skia: &Path, path: &BezPath) {
        let kind = paint.kind.unwrap_or(PaintType::Solid);
        let opacity = paint.opacity.unwrap_or(1.0).clamp(0.0, 1.0);
        if let Some(cast) = self.cast {
            return match self.silhouette(node, paint, kind, opacity) {
                Some(true) => self.fill(fill_of(skia), affine(&self.m), color(cast), None, path),
                Some(false) => {}
                None if kind == PaintType::Image && self.image_silhouette(node, paint, skia, path, cast) => {}
                None => {
                    let bounds = skia.bounds().with_outset((1.0, 1.0));
                    let (node, paint, skia) = (node.clone(), paint.clone(), skia.clone());
                    self.fall_back(paint_name(kind), false, bounds, Box::new(move |drawer| drawer.paint(&node, &paint, &skia)))
                }
            };
        }
        if kind == PaintType::GradientDiamond && self.diamond(node, paint, skia, path) {
            return;
        }
        let mut image = None;
        let brush: Option<(peniko::Brush, Option<Affine>)> = match kind {
            PaintType::Image => match self.image(paint, node.paint_size.unwrap_or([1.0, 1.0]), opacity) {
                Picture::Drawn(brush, at, key, width) => {
                    image = Some((key, width));
                    Some((peniko::Brush::Image(brush), Some(at)))
                }
                Picture::Nothing => return,
                Picture::Skia => None,
            },
            PaintType::Solid => Some((peniko::Brush::Solid(color(draw::color(tree::rgba(paint.color.unwrap_or_default()), opacity))), None)),
            PaintType::GradientLinear | PaintType::GradientRadial => gradient(paint, kind, node.paint_size.unwrap_or([1.0, 1.0])),
            _ => None,
        };
        let Some((brush, brush_transform)) = brush else {
            // Images, patterns and the rest, as Skia draws them
            let bounds = skia.bounds().with_outset((1.0, 1.0));
            // Skia draws it alone and it's blended here as it's put in place
            let blended = match mix(paint.blend_mode) {
                Some(mode) => self.push(mode, 1.0, bounds),
                None => false,
            };
            let blends = !blended && tree::blends(paint.blend_mode);
            let (node, mut paint, skia) = (node.clone(), paint.clone(), skia.clone());
            if blended {
                paint.blend_mode = Some(rt::BlendMode::Normal);
            }
            self.fall_back(paint_name(kind), blends, bounds, Box::new(move |drawer| drawer.paint(&node, &paint, &skia)));
            if blended {
                self.pop();
            }
            return;
        };
        if mix(paint.blend_mode).is_some() {
            SIZES[11].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let blended = match mix(paint.blend_mode) {
            Some(mode) => self.push(mode, 1.0, skia.bounds().with_outset((1.0, 1.0))),
            None => false,
        };
        let patches = self.scene.encoding().resources.patches.len();
        self.fill(fill_of(skia), affine(&self.m), &brush, brush_transform, path);
        if let Some((key, width)) = image {
            self.image_drawn(patches, key, width, skia);
        }
        if blended {
            self.pop();
        }
    }

    /// A diamond gradient over `path`, as draw.rs's shader draws it: its
    /// ramp at twice the distance from the middle across and down added
    /// up, which in each quarter around the middle is a linear gradient,
    /// each cut to its quarter and added to the rest in a layer of their
    /// own, so where they meet what each covers of a pixel adds up to it
    /// all, without a seam. False where Skia's to draw it
    fn diamond(&mut self, node: &Node, paint: &rt::Paint, skia: &Path, path: &BezPath) -> bool {
        let Some((peniko::Brush::Gradient(ramp), Some(local))) = gradient(paint, PaintType::GradientLinear, node.paint_size.unwrap_or([1.0, 1.0])) else { return false };
        if local.determinant().abs() < 1e-12 {
            return false;
        }
        let bounds = skia.bounds().with_outset((1.0, 1.0));
        // Where the path is in the gradient's own space, which its quarters
        // are cut to
        let area = local.inverse().transform_rect_bbox(kurbo::Rect::new(bounds.left as f64, bounds.top as f64, bounds.right as f64, bounds.bottom as f64));
        let blended = match mix(paint.blend_mode) {
            Some(mode) => self.push(mode, 1.0, bounds),
            None => false,
        };
        if self.push(BlendMode::default(), 1.0, bounds) {
            let to_device = affine(&self.m) * local;
            for (sx, sy) in [(1.0, 1.0), (-1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)] {
                let quarter = kurbo::Rect::new(
                    if sx > 0.0 { 0.5 } else { area.x0.min(0.5) },
                    if sy > 0.0 { 0.5 } else { area.y0.min(0.5) },
                    if sx > 0.0 { area.x1.max(0.5) } else { 0.5 },
                    if sy > 0.0 { area.y1.max(0.5) } else { 0.5 },
                );
                if quarter.width() <= 0.0 || quarter.height() <= 0.0 {
                    continue;
                }
                let mut brush = ramp.clone();
                brush.kind = peniko::GradientKind::Linear(peniko::LinearGradientPosition::new((0.5, 0.5), (0.5 + 0.25 * sx, 0.5 + 0.25 * sy)));
                self.scene.push_layer(Fill::NonZero, BlendMode::new(Mix::Normal, Compose::Plus), 1.0, to_device, &quarter);
                self.fill(fill_of(skia), affine(&self.m), &peniko::Brush::Gradient(brush), Some(local), path);
                self.scene.pop_layer();
            }
            self.pop();
        }
        if blended {
            self.pop();
        }
        true
    }

    /// An image just drawn over `skia`, whose patches were `patches`
    /// before, with its level picked for each view (see `Kind::Image`):
    /// where its sampling's encoded, if it was drawn
    fn image_drawn(&mut self, patches: usize, key: String, width: f32, skia: &Path) -> Option<usize> {
        let encoding = self.scene.encoding_mut();
        if encoding.resources.patches.len() <= patches {
            return None;
        }
        let patch = encoding.resources.patches.len() - 1;
        let Some(Patch::Image { draw_data_offset, .. }) = encoding.resources.patches.last() else { return None };
        let at = *draw_data_offset + 2;
        encoding.draw_data[at] |= 1 << 16;
        let bounds = self.m.map_rect(skia.bounds()).0;
        let bounds = kurbo::Rect::new(bounds.left as f64, bounds.top as f64, bounds.right as f64, bounds.bottom as f64);
        self.effects.push(Effect { kind: Kind::Image { key, width }, bounds, transform: usize::MAX, patch });
        Some(at)
    }

    /// Image fill `paint`'s silhouette in `cast`, as Skia casts it: each of
    /// its texels that isn't clear solid, then sampled as the image is. False
    /// where Vello can't draw the image
    fn image_silhouette(&mut self, node: &Node, paint: &rt::Paint, skia: &Path, path: &BezPath, cast: Color4f) -> bool {
        let (brush, key, width) = match self.image(paint, node.paint_size.unwrap_or([1.0, 1.0]), 1.0) {
            Picture::Drawn(brush, at, key, width) => ((peniko::Brush::Image(brush), at), key, width),
            Picture::Nothing => return true,
            Picture::Skia => return false,
        };
        let bounds = skia.bounds().with_outset((1.0, 1.0));
        SIZES[9].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Where only its alpha's taken, it's drawn white without a layer
        let tinted = !self.alpha_only;
        if tinted && !self.push(BlendMode::default(), 1.0, bounds) {
            return true;
        }
        let patches = self.scene.encoding().resources.patches.len();
        self.fill(fill_of(skia), affine(&self.m), &brush.0, Some(brush.1), path);
        if let Some(at) = self.image_drawn(patches, key, width, skia) {
            // Ligma's shaders take the quality otherwise unused for it
            self.scene.encoding_mut().draw_data[at] |= 3 << 12;
        }
        if !tinted {
            return true;
        }
        if self.push(BlendMode::new(Mix::Normal, Compose::SrcIn), 1.0, bounds) {
            self.fill(fill_of(skia), affine(&self.m), color(cast), None, path);
            self.pop();
        }
        self.pop();
        true
    }

    /// What node `id` covers, drawn solid, or as a silhouette where
    /// `casting`, as paths in device pixels: one solid path, a fill and a
    /// stroke, or a stroke kept inside or outside its outline. None where
    /// it's more
    fn cover(&mut self, id: u32, casting: bool) -> Option<Cover> {
        let _t = timer(5);
        let tree = self.tree;
        let mut m = self.m;
        let mut id = id;
        for _ in 0..8 {
            let node = tree.node(id)?;
            match node.op {
                Op::StaticTransformBinding => {
                    m = Matrix::concat(&m, &rt::to_matrix(&node.transform?));
                    id = node.child?;
                }
                Op::Group if node.children.len() == 1 => id = node.children[0],
                Op::Blend if casting || (node.opacity.unwrap_or(1.0) >= 0.999 && !tree::blends(node.blend_mode) && tree::pass_through(node.blend_mode)) => id = node.child?,
                Op::Fill | Op::FillStroke => break,
                _ => return None,
            }
        }
        let node = tree.node(id)?;
        // Too small to be drawn, it covers nothing
        let bounds = tree.info(id).bounds?;
        let scale = m.scale_x().hypot(m.skew_y()).max(m.skew_x().hypot(m.scale_y()));
        if bounds.width().max(bounds.height()) * scale < if self.retained { 0.25 } else { 0.5 } {
            return Some(Cover::Any(Vec::new()));
        }
        let at = Matrix::concat(&m, &rt::to_matrix(&node.transform.unwrap_or(rt::IDENTITY)));
        // Whether paints draw their path solid, or none of it
        let mut drawn = |paints: &[std::sync::Arc<rt::Paint>]| -> Option<bool> {
            let mut any = false;
            for paint in paints {
                let kind = paint.kind.unwrap_or(PaintType::Solid);
                let opacity = paint.opacity.unwrap_or(1.0).clamp(0.0, 1.0);
                let solid = if casting {
                    self.silhouette(node, paint, kind, opacity)?
                } else {
                    if kind != PaintType::Solid || tree::blends(paint.blend_mode) {
                        return None;
                    }
                    let alpha = tree::rgba(paint.color.unwrap_or_default()).a * opacity;
                    if alpha >= 0.999 {
                        true
                    } else if alpha * 255.0 < 0.5 {
                        false
                    } else {
                        return None;
                    }
                };
                any |= solid;
            }
            Some(any)
        };
        let device = |path: &std::sync::Arc<rt::Path>| tree.skia_path(path).with_transform(&at);
        match node.op {
            Op::Fill => {
                let geometry = node.geometry.as_ref()?;
                let path = geometry.path.as_ref()?;
                Some(Cover::Any(if drawn(&geometry.paints)? { vec![device(path)] } else { Vec::new() }))
            }
            Op::FillStroke if node.ext().instance_transforms.is_empty() => {
                let mut fills = Vec::new();
                for geometry in &node.fill_geometries {
                    if drawn(&geometry.paints)? {
                        fills.push(geometry.path.as_ref()?);
                    }
                }
                let mut strokes = Vec::new();
                for geometry in &node.stroke_geometries {
                    if drawn(&geometry.paints)? {
                        strokes.push(geometry.path.as_ref()?);
                    }
                }
                let outlines: Vec<&std::sync::Arc<rt::Path>> = node.fill_geometries.iter().filter_map(|g| g.path.as_ref()).collect();
                let kept = node.stroke_mode.filter(|_| !outlines.is_empty());
                match (fills.as_slice(), strokes.as_slice(), kept, outlines.as_slice()) {
                    (_, [], ..) => Some(Cover::Any(fills.iter().map(|path| device(path)).collect())),
                    ([], [stroke], None | Some(rt::StrokeMask::Unknown(_)), _) => Some(Cover::Any(vec![device(stroke)])),
                    ([], [stroke], Some(rt::StrokeMask::Inside), [outline]) => Some(Cover::All(vec![(device(stroke), false), (device(outline), false)])),
                    ([], [stroke], Some(rt::StrokeMask::Outside), [outline]) => Some(Cover::All(vec![(device(stroke), false), (device(outline), true)])),
                    // The fill's the outline, which the stroke's kept inside
                    // of or not
                    ([fill], [_], Some(rt::StrokeMask::Inside), [_]) => Some(Cover::Any(vec![device(fill)])),
                    ([fill], [stroke], None | Some(rt::StrokeMask::Unknown(_) | rt::StrokeMask::Outside), [_]) => Some(Cover::Any(vec![device(fill), device(stroke)])),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Clips to all of `clips`, each a path or where it isn't inside
    /// `device`, and how many there are to pop. None where that's not to
    /// be had, with nothing pushed
    fn push_clips(&mut self, clips: Vec<(Path, bool)>, device: IRect) -> Option<usize> {
        if self.soft.is_some() && clips.iter().any(|(path, outside)| *outside && self::outside(path, device).is_none()) {
            return None;
        }
        let n = clips.len();
        for (path, outside) in clips {
            if !outside {
                self.clip(path);
            } else if !self.clip_outside(&path, device) {
                self.clip(self::outside(&path, device)?);
            }
        }
        Some(n)
    }

    /// Whether `paint` casts a silhouette over all its path, or none of it,
    /// or None where only Skia knows
    fn silhouette(&mut self, node: &Node, paint: &rt::Paint, kind: PaintType, opacity: f32) -> Option<bool> {
        // Pixels any of it covers are solid, those it doesn't, clear
        let seen = |alpha: f32| alpha * opacity * 255.0 >= 0.5;
        match kind {
            PaintType::Solid => Some(seen(tree::rgba(paint.color.unwrap_or_default()).a)),
            PaintType::GradientLinear | PaintType::GradientRadial | PaintType::GradientAngular | PaintType::GradientDiamond => {
                let stops = &paint.gradient.as_ref()?.stops;
                if stops.iter().all(|stop| seen(tree::rgba(stop.color).a)) {
                    Some(true)
                } else if stops.iter().all(|stop| !seen(tree::rgba(stop.color).a)) {
                    Some(false)
                } else {
                    None
                }
            }
            PaintType::Image => {
                let Some(key) = tree::image_key(paint) else { return Some(false) };
                let Some(img) = self.images.get(&key) else { return Some(false) };
                let size = node.paint_size.unwrap_or([1.0, 1.0]);
                let (w, h) = (size[0].max(1e-3), size[1].max(1e-3));
                let (to_node, tile) = tree::image_layout(paint, w, h, img)?;
                let covered = to_node.map_rect(Rect::from_wh(img.original.0 as f32, img.original.1 as f32)).0;
                let covers = tile == skia_safe::TileMode::Repeat || skia_safe::Contains::contains(&covered, Rect::from_wh(w, h));
                (img.image.is_opaque() && covers).then_some(true)
            }
            _ => None,
        }
    }

    /// Image fill `paint` of a node `size` big, as `Drawer::image` draws it,
    /// with what it asked about the image
    fn image(&mut self, paint: &rt::Paint, size: [f32; 2], opacity: f32) -> Picture {
        let _t = timer(9);
        let Some(key) = tree::image_key(paint) else { return Picture::Nothing };
        let (w, h) = (size[0].max(1e-3), size[1].max(1e-3));
        // A fragment's shown up to twice as big as it's encoded, asked
        // for as it shows now, and for bigger as it's drawn bigger (see
        // `Kind::Image`)
        let shows = if self.retained { self.shows } else { 1.0 };
        let node_scale = self.m.scale_x().hypot(self.m.skew_y()) * shows;
        let Some(img) = self.images.get(&key) else {
            if !self.asked.missing.iter().any(|(missing, _)| *missing == key) {
                self.asked.missing.push((key, (w.max(h) * node_scale * ASKED as f32) as f64));
            }
            return Picture::Nothing;
        };
        let Some((to_node, tile)) = tree::image_layout(paint, w, h, img) else { return Picture::Nothing };
        let (iw, ih) = (img.original.0 as f32, img.original.1 as f32);
        let shown = to_node.scale_x().hypot(to_node.skew_y()) * node_scale * iw;
        let kept = img.image.width() as f32;
        self.asked.used.push((key.clone(), (shown * ASKED as f32) as f64));
        if kept < shown * ASKED as f32 && iw > kept && kept < 4096.0 {
            self.asked.sharper.push((key.clone(), (shown * ASKED as f32) as f64));
        }
        // Up to 4 texels a pixel where it's shown biggest, which Figma's
        // sampling takes as they are. More, and a mipmap level with 2 to 4
        // of its texels a pixel there, as each of its texels is the average
        // of those under it, so Vello's atlas holds no more than that
        let texels = kept / shown.max(1e-6);
        let level = if texels <= 4.0 { 0 } else { (texels / 2.0).log2().floor().clamp(0.0, 12.0) as u32 };
        let drawable = self.drawable.get(&key).filter(|drawable| drawable.id == img.image.unique_id());
        let fits = |level: u32| drawable.is_some_and(|d| level < d.levels && (d.width >> level).max(d.height >> level).max(1) <= 8192);
        let Some(level) = (0..=level).rev().find(|&level| fits(level)) else {
            return Picture::Skia;
        };
        // Each view puts in the texture of the level it draws (see `put`)
        let data = nothing();
        let mut local = to_node;
        local.pre_scale((iw, ih), None);
        let width = shown / shows;
        // Ligma's shaders take reflect for transparent outside the image,
        // and high quality for Figma's sampling
        let extend = if tile == skia_safe::TileMode::Repeat { Extend::Repeat } else { Extend::Reflect };
        self.pictures.push((key.clone(), level, img.image.unique_id() as u64));
        let brush = ImageBrush { image: data, sampler: ImageSampler { x_extend: extend, y_extend: extend, quality: ImageQuality::High, alpha: opacity } };
        Picture::Drawn(brush, affine(&local), key, width)
    }

    fn mask(&mut self, node: &Node, id: u32, bounds: Rect) {
        let _t = timer(11);
        let (Some(alpha), Some(color)) = (node.alpha_node, node.color_node) else {
            return;
        };
        let op = node.mask_op.unwrap_or(MaskOpType::Normal);
        let ext = node.ext();
        match op {
            MaskOpType::KnockoutShadow => {
                SIZES[0].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if switches().shapes && SIZES[0].load(std::sync::atomic::Ordering::Relaxed) % 40 == 1 {
                    super::render::trace(|| format!("  knockout alpha {}", shape_of(self.tree, alpha, 0)));
                }
                // Where the shape's paths, the shadow's kept where none of
                // them is
                let blends = self.cast.is_none() && self.tree.info(color).blends;
                if let (false, Some(Cover::Any(paths)), Some(device)) = (blends, self.cover(alpha, true), self.device(bounds)) {
                    if let Some(n) = self.push_clips(paths.into_iter().map(|path| (path, true)).collect(), device) {
                        SIZES[1].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        self.draw(color);
                        for _ in 0..n {
                            self.pop();
                        }
                        return;
                    }
                }
                if !self.push(BlendMode::default(), 1.0, bounds) {
                    return;
                }
                self.draw(color);
                if self.push(BlendMode::new(Mix::Normal, Compose::DestOut), 1.0, bounds) {
                    self.casting(alpha);
                    self.pop();
                }
                self.pop();
                return;
            }
            MaskOpType::InnerShadow => {
                SIZES[2].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if self.tree.clip_of(alpha).is_some() {
                    SIZES[3].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                // The shadow's color where the shape, moved and blurred,
                // isn't, kept inside the shape
                if !self.push(BlendMode::default(), 1.0, bounds) {
                    return;
                }
                let fill = match self.cast {
                    Some(_) => BLACK,
                    None => draw::color(tree::rgba(ext.inner_shadow_fill.unwrap_or_default()), 1.0),
                };
                let pixel = 1.0 / self.scale().max(1e-6);
                let outset = bounds.with_outset((pixel, pixel));
                let rect = kurbo::Rect::new(outset.left as f64, outset.top as f64, outset.right as f64, outset.bottom as f64);
                self.fill(Fill::NonZero, affine(&self.m), self::color(fill), None, &rect);
                if self.push(BlendMode::new(Mix::Normal, Compose::DestOut), 1.0, bounds) {
                    self.draw(color);
                    self.pop();
                }
                if self.push(BlendMode::new(Mix::Normal, Compose::DestIn), 1.0, bounds) {
                    if ext.inner_shadow_alpha_requires_hard_mask {
                        self.casting(alpha);
                    } else {
                        self.draw(alpha);
                    }
                    self.pop();
                }
                self.pop();
                return;
            }
            _ => {}
        }
        let blends = self.cast.is_none() && self.tree.info(color).blends;
        // A clip cuts what's under it once, as one, as Figma masks, where
        // what's in it doesn't blend with what's under it. A background
        // blur's is a clip whatever's in it, as Skia's, so then each thing's
        // cut on its own
        if (!blends || op == MaskOpType::BackgroundBlur) && matches!(op, MaskOpType::Normal | MaskOpType::Pattern | MaskOpType::BackgroundBlur) {
            if let Some((fill, at)) = self.tree.clip_of(alpha) {
                if let Some(outline) = self.tree.outline(fill) {
                    let device = outline.with_transform(&Matrix::concat(&self.m, &at));
                    if blends {
                        let was = self.soften(device);
                        self.draw(color);
                        self.soft = was;
                    } else {
                        self.clip(device);
                        self.draw(color);
                        self.pop();
                    }
                    return;
                }
            }
        }
        // A mask of one solid path, or one stroke kept inside or outside its
        // outline, is clips to those, which Vello passes over where a tile's
        // all inside or outside them
        if !blends && matches!(op, MaskOpType::Normal | MaskOpType::Pattern) {
            let clips = match self.cover(alpha, self.cast.is_some()) {
                Some(Cover::All(clips)) => Some(clips),
                Some(Cover::Any(paths)) if paths.len() <= 1 => Some(paths.into_iter().map(|path| (path, false)).collect()),
                _ => None,
            };
            if let (Some(clips), Some(device)) = (clips, self.device(bounds)) {
                if let Some(n) = self.push_clips(clips, device) {
                    SIZES[3].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    // Where it covers nothing, nothing's drawn
                    if n == 0 {
                        return;
                    }
                    self.draw(color);
                    for _ in 0..n {
                        self.pop();
                    }
                    return;
                }
            }
        }
        // What's masked blends with what's under the mask, as masks don't
        // isolate it in Figma: it's drawn over a copy of that, then the copy
        // is kept only where the mask is
        SIZES[4].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if switches().shapes && SIZES[4].load(std::sync::atomic::Ordering::Relaxed) % 60 == 1 {
            super::render::trace(|| format!("  mask {:?} alpha {} color {}", op, shape_of(self.tree, alpha, 0), shape_of(self.tree, color, 2)));
        }
        if tree::single_draw(self.tree, alpha) {
            SIZES[5].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if !self.push(if blends { OVER_COPY } else { BlendMode::default() }, 1.0, bounds) {
            return;
        }
        self.draw(color);
        let kept = if op == MaskOpType::Luminance { self.push_luminance(bounds) } else { self.push(BlendMode::new(Mix::Normal, Compose::DestIn), 1.0, bounds) };
        if kept {
            // What a background blur shows is opaque, so it shows wherever
            // its shape is at all
            if op == MaskOpType::BackgroundBlur {
                self.casting(alpha);
            } else {
                self.draw(alpha);
            }
            self.pop();
        }
        self.pop();
        let _ = id;
    }

    /// A layer over `bounds` whose luminance, as Figma takes it, masks
    /// what's under it in the layer it's in
    fn push_luminance(&mut self, bounds: Rect) -> bool {
        let Some(device) = self.device(bounds) else { return false };
        let rect = kurbo::Rect::new(device.left as f64, device.top as f64, device.right as f64, device.bottom as f64);
        self.scene.push_luminance_mask_layer(Fill::NonZero, 1.0, Affine::IDENTITY, &rect);
        SIZES[17].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.saved.push((self.soft.take(), true));
        true
    }

    /// `id` drawn as what casts a silhouette, where only its shape matters
    fn casting(&mut self, id: u32) {
        let (was, alpha_only) = (self.cast, self.alpha_only);
        self.cast = Some(was.unwrap_or(BLACK));
        self.alpha_only = true;
        self.draw(id);
        (self.cast, self.alpha_only) = (was, alpha_only);
    }

    /// A blur of `node`'s child: its source drawn in a scene of its own,
    /// and an image in its place, which each view draws, blurs and puts in
    /// place
    fn blur(&mut self, node: &Node, id: u32, bounds: Rect) {
        let Some(child) = node.child else { return };
        let ext = node.ext();
        let m = self.m;
        // Clamped, it's to the node's box, which its frame has to be square
        // to
        if m.has_perspective() || (ext.clamp.is_some() && !blur_frame(&m).3) {
            return self.fall_back("blur", false, bounds, Box::new(move |drawer| drawer.draw(id)));
        }
        if ext.blur_op_type == Some(rt::BlurOpType::Progressive) {
            return self.progressive(node, child, bounds);
        }
        let sigma = draw::tent_sigma(node.radius.filter(|radius| radius.is_finite()).unwrap_or(0.0).max(0.0) as f64);
        self.blur_of(child, sigma, ext.clamp.map(|clamp| clamp.to_skia()), bounds);
    }

    /// A progressive blur, more from its start to its end, as Skia draws it
    /// (see tree.rs): blurs of radii evenly apart, each shown about where
    /// the radius is its own, added up
    fn progressive(&mut self, node: &Node, child: u32, bounds: Rect) {
        let ext = node.ext();
        let space = tree::Drawer::effect_space(node);
        let point = |offset: Option<rt::Vector>, default: [f32; 2]| {
            // Figma sends them in the space the layer's drawn in already
            let p = match offset.filter(|p| p[0].is_finite() && p[1].is_finite()) {
                Some([x, y]) => skia_safe::Point::new(x, y),
                None => space.map_point((default[0], default[1])),
            };
            (p.x as f64, p.y as f64)
        };
        let (p0, p1) = (point(ext.start_offset, [0.0, 0.0]), point(ext.end_offset, [0.0, 1.0]));
        let finite = |value: Option<f32>| value.filter(|value| value.is_finite()).unwrap_or(0.0).max(0.0);
        let (start, end) = (finite(ext.start_radius), finite(node.radius));
        let span = (end - start).abs() * self.scale();
        let levels = ((span / 6.0).ceil() as usize + 1).clamp(2, 8);
        let rect = kurbo::Rect::new(bounds.left as f64, bounds.top as f64, bounds.right as f64, bounds.bottom as f64);
        if !self.push(BlendMode::default(), 1.0, bounds) {
            return;
        }
        for level in 0..levels {
            let at = level as f32 / (levels - 1) as f32;
            let radius = start + (end - start) * at;
            let step = 1.0 / (levels - 1) as f32;
            let stops: Vec<ColorStop> = [(at - step, 0.0), (at, 1.0), (at + step, 0.0)]
                .into_iter()
                .filter(|(position, _)| (-1e-6..=1.0 + 1e-6).contains(position))
                .map(|(position, alpha)| ColorStop { offset: position.clamp(0.0, 1.0), color: DynamicColor::from_alpha_color(AlphaColor::<Srgb>::new([0.0, 0.0, 0.0, alpha])) })
                .collect();
            if !self.push(BlendMode::new(Mix::Normal, Compose::Plus), 1.0, bounds) {
                continue;
            }
            self.blur_of(child, draw::tent_sigma(radius as f64), None, bounds);
            if self.push(BlendMode::new(Mix::Normal, Compose::DestIn), 1.0, bounds) {
                let ramp = peniko::Gradient::new_linear(p0, p1).with_extend(Extend::Pad).with_stops(stops.as_slice());
                self.fill(Fill::NonZero, affine(&self.m), &peniko::Brush::Gradient(ramp), None, &rect);
                self.pop();
            }
            self.pop();
        }
        self.pop();
    }

    /// `child` blurred by `sigma`, in the current space, clamped to `clamp`
    /// if it is
    fn blur_of(&mut self, child: u32, sigma: f32, clamp: Option<Rect>, bounds: Rect) {
        let m = self.m;
        if sigma <= 0.0 {
            return self.draw(child);
        }
        let Some(reach) = self.tree.info(child).bounds else { return };
        let (turn, sx, sy, _) = blur_frame(&m);
        let in_frame = Matrix::concat(&to_matrix(turn.inverse()), &m);
        let krect = |rect: Rect| {
            let rect = m.map_rect(rect).0;
            kurbo::Rect::new(rect.left as f64, rect.top as f64, rect.right as f64, rect.bottom as f64)
        };
        let frect = |rect: Rect| {
            let rect = in_frame.map_rect(rect).0;
            kurbo::Rect::new(rect.left as f64, rect.top as f64, rect.right as f64, rect.bottom as f64)
        };
        // Drawn for one view, what's just outside it is blurred into it too
        let view = self.view;
        if !self.retained {
            let spread = (3.0 * sigma * sx.max(sy)).ceil() as i32 + 2;
            self.view = view.with_outset((spread, spread));
        }
        let scene = std::mem::replace(&mut self.scene, vello::Scene::new());
        let effects = std::mem::take(&mut self.effects);
        let soft = self.soft.take();
        self.draw(child);
        self.soft = soft;
        self.view = view;
        let source = std::mem::replace(&mut self.scene, scene);
        let nested = std::mem::replace(&mut self.effects, effects);
        let shown = krect(bounds);
        let Some((transform, patch)) = self.stand_in(shown) else { return };
        self.effects.push(Effect {
            kind: Kind::Blur {
                source,
                effects: nested,
                reach: frect(reach),
                sigma: ((sigma * sx) as f64, (sigma * sy) as f64),
                clamp: clamp.map(frect),
                turn,
                area: frect(bounds),
            },
            bounds: shown,
            transform,
            patch,
        });
    }

    /// An image that stands in for an effect over `rect`, in the scene's
    /// space, which views put in place: its transform and patch in the
    /// scene's encoding
    fn stand_in(&mut self, rect: kurbo::Rect) -> Option<(usize, usize)> {
        let brush = ImageBrush { image: nothing(), sampler: ImageSampler { x_extend: Extend::Reflect, y_extend: Extend::Reflect, quality: ImageQuality::Low, alpha: 1.0 } };
        // Its own transform, which a view sets, unlike any other
        self.fill_shape(Fill::NonZero, Affine::IDENTITY, &brush, Some(Affine::scale(0.123456789)), &rect);
        let encoding = self.scene.encoding();
        if !matches!(encoding.resources.patches.last(), Some(Patch::Image { .. })) {
            self.fail("effect's image");
            return None;
        }
        Some((encoding.transforms.len() - 1, encoding.resources.patches.len() - 1))
    }

    /// What Skia draws with `draw`, over `bounds` in the current space,
    /// which each view has it draw onto the atlas at the zoom it's at
    fn fall_back(&mut self, kind: &'static str, blends: bool, bounds: Rect, draw: SkiaDraw) {
        let _t = timer(8);
        // Drawn alone, it can't mix with what's under it
        if blends {
            return self.fail(format!("blends in what Skia draws: {kind}"));
        }
        let device = self.m.map_rect(bounds).0;
        // Pixels more each way, where it's smoothed at the edges, as what's
        // drawn there is transparent
        let pixel = 2.0;
        let rect = kurbo::Rect::new((device.left - pixel) as f64, (device.top - pixel) as f64, (device.right + pixel) as f64, (device.bottom + pixel) as f64);
        let Some((transform, patch)) = self.stand_in(rect) else { return };
        static IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.effects.push(Effect { kind: Kind::Skia { name: kind, id, m: self.m, cast: self.cast, draw }, bounds: rect, transform, patch });
    }

    /// Puts `effects`, of a scene with `encoding` that `t` places over
    /// `view` of a texture, in place, `depth` blurs deep
    pub fn put(&mut self, plan: &mut Plan, effects: &mut [Effect], encoding: &mut Encoding, t: Affine, view: kurbo::Rect, depth: usize) -> Result<(), String> {
        let c = t.as_coeffs();
        let scale = c[0].hypot(c[1]);
        for effect in effects {
            match &mut effect.kind {
                Kind::Blur { source, effects, reach, sigma, clamp, turn, area } => {
                    let sigma = (sigma.0 * scale, sigma.1 * scale);
                    let margin = (3.0 * sigma.0.max(sigma.1)).ceil() + 2.0;
                    // Blurred in its frame as `t` turns it, from which `tf`
                    // only scales and moves
                    let k = scale.max(1e-12);
                    let frame = Affine::new([c[0] / k, c[1] / k, c[2] / k, c[3] / k, 0.0, 0.0]) * *turn;
                    let tf = frame.inverse() * t * *turn;
                    let view_f = frame.inverse().transform_rect_bbox(view);
                    let shown = tf.transform_rect_bbox(*area).intersect(view_f).intersect(tf.transform_rect_bbox(*reach).inflate(margin, margin));
                    if shown.width() <= 0.0 || shown.height() <= 0.0 {
                        Plan::patch(encoding, effect, nothing(), Affine::IDENTITY);
                        continue;
                    }
                    let region = shown.inflate(margin, margin);
                    let (left, top) = (region.x0.floor() as i32, region.y0.floor() as i32);
                    // A wide blur's source is drawn smaller, which the blur
                    // would average it down to anyway, as Skia does, by
                    // half as much, for each of its pixels to be of 16
                    // samples still
                    let shrink = shrink(sigma.0.max(sigma.1)) << plan.coarser;
                    let (width, height) = ((region.x1.ceil() as i32 - left + shrink - 1) / shrink, (region.y1.ceil() as i32 - top + shrink - 1) / shrink);
                    let f = 1.0 / shrink as f64;
                    let sigma = (sigma.0 * f, sigma.1 * f);
                    if depth >= BLUR_DEPTH {
                        return Err("blurs too deep in each other".into());
                    }
                    if width > 8192 || height > 8192 {
                        return Err("blur bigger than its texture".into());
                    }
                    while plan.depths.len() <= depth {
                        plan.depths.push(Depth { scene: vello::Scene::new(), shelf: Shelf { width: 8192, height: 8192, x: 0, y: 0, row: 0 }, blurs: Vec::new(), used: 0 });
                    }
                    let soft = plan.width;
                    let (x, y) = plan.depths[depth].room(width, height, soft).ok_or("blurs past their texture")?;
                    let in_slot = Affine::translate((x as f64, y as f64)) * Affine::scale(f) * Affine::translate((-left as f64, -top as f64)) * tf;
                    let to_slot = in_slot * turn.inverse();
                    let slot = kurbo::Rect::new(x as f64, y as f64, (x + width) as f64, (y + height) as f64);
                    self.put(plan, effects, source.encoding_mut(), to_slot, slot, depth + 1)?;
                    let scene = &mut plan.depths[depth].scene;
                    scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &slot);
                    if let Some(clamp) = clamp {
                        scene.push_clip_layer(Fill::NonZero, in_slot, clamp);
                    }
                    scene.append(source, Some(to_slot));
                    if clamp.is_some() {
                        scene.pop_layer();
                    }
                    scene.pop_layer();
                    let image = slot_image(width, height);
                    let clamp = clamp.map(|clamp| {
                        let clamp = in_slot.transform_rect_bbox(clamp);
                        [(clamp.x0 - x as f64) as f32, (clamp.y0 - y as f64) as f32, (clamp.x1 - x as f64) as f32, (clamp.y1 - y as f64) as f32]
                    });
                    let job = Job { slot: [x, y, width, height], sigma: (sigma.0 as f32, sigma.1 as f32), clamp };
                    let back = *turn * tf.inverse() * Affine::translate((left as f64, top as f64)) * Affine::scale(shrink as f64);
                    Plan::patch(encoding, effect, image.clone(), back);
                    // Drawn bigger, it's sampled smoothly, as Skia scales a
                    // blur back up
                    if let Some(Patch::Image { draw_data_offset, .. }) = encoding.resources.patches.get(effect.patch) {
                        let at = *draw_data_offset + 2;
                        let quality = if shrink > 1 { ImageQuality::Medium } else { ImageQuality::Low };
                        encoding.draw_data[at] = (encoding.draw_data[at] & !(3 << 12)) | (quality as u32) << 12;
                    }
                    plan.depths[depth].blurs.push(Placed { job, image });
                }
                Kind::Image { key, width } => {
                    let shown = t.transform_rect_bbox(effect.bounds).intersect(view);
                    // Out of view, it takes no room in Vello's atlas
                    if shown.width() <= 0.0 || shown.height() <= 0.0 {
                        Plan::patch_image(encoding, effect, nothing());
                        continue;
                    }
                    let Some(img) = self.images.get(key.as_str()) else { return Err("image gone".into()) };
                    // As `image` picks it, for how wide it's shown here
                    let texels = img.image.width() as f32 / (*width * scale as f32).max(1e-6);
                    // A fragment drawn bigger than when it was encoded
                    let wants = *width * scale as f32 * ASKED as f32;
                    if (img.image.width() as f32) < wants && (img.original.0 as f32) > img.image.width() as f32 && img.image.width() < 4096 && !self.asked.sharper.iter().any(|(sharper, _)| sharper == key) {
                        self.asked.sharper.push((key.clone(), wants as f64));
                    }
                    let level = if texels <= 4.0 { 0 } else { (texels / 2.0).log2().floor().clamp(0.0, 12.0) as u32 };
                    let level = (level + self.coarser).min(12);
                    let vello = self.vello.as_deref_mut().ok_or("no Vello")?;
                    let Some((data, level)) = (0..=level).rev().find_map(|level| vello.picture(key, img, level).map(|data| (data, level))) else {
                        return Err("image Vello can't draw".into());
                    };
                    self.drawn.push((key.clone(), level));
                    self.shown.push((key.clone(), *width * scale as f32));
                    Plan::patch_image(encoding, effect, data);
                }
                Kind::Skia { name, id, m, cast, draw } => {
                    let whole = t.transform_rect_bbox(effect.bounds);
                    let shown = whole.intersect(view);
                    if shown.width() <= 0.0 || shown.height() <= 0.0 {
                        Plan::patch(encoding, effect, nothing(), Affine::IDENTITY);
                        continue;
                    }
                    let (left, top) = (shown.x0.floor() as i32, shown.y0.floor() as i32);
                    let (width, height) = (shown.x1.ceil() as i32 - left, shown.y1.ceil() as i32 - top);
                    let (page, x, y) = self.room(width, height).ok_or("atlas full")?;
                    // Drawn whole, where it's placed the same to the pixel, it's
                    // what an earlier view had Skia draw, unless an image it
                    // drew with has come since
                    let placed = Affine::translate((-(left as f64), -(top as f64))) * t;
                    let key = (shown == whole).then(|| PieceKey::new(*id, placed, width, height));
                    let images = self.images;
                    let views = self.vello.as_deref().map_or(0, |vello| vello.views);
                    let reused = key.as_ref().and_then(|key| {
                        let vello = self.vello.as_deref_mut()?;
                        let piece = vello.pieces.get_mut(key)?;
                        if !piece.images.iter().all(|(key, id)| images.get(key).is_some_and(|img| img.image.unique_id() == *id)) {
                            return None;
                        }
                        piece.view = views;
                        Some((piece.image.clone(), piece.counts, piece.used.clone()))
                    });
                    if let Some((image, counts, mut used)) = reused {
                        let canvas = self.pages[page].canvas();
                        canvas.draw_image(&image, (x as f32, y as f32), None);
                        self.counts += counts;
                        self.asked.used.append(&mut used);
                        let entry = self.kinds.entry(name).or_default();
                        entry.0 += 1;
                        let image = slot_image(width, height);
                        Plan::patch(encoding, effect, image.clone(), t.inverse() * Affine::translate((left as f64, top as f64)));
                        plan.skia.push((image, page, x, y));
                        continue;
                    }
                    self.fell_back += width as i64 * height as i64;
                    let entry = self.kinds.entry(name).or_default();
                    entry.0 += 1;
                    entry.1 += width as i64 * height as i64;
                    let (tree, images, kit) = (self.tree, self.images, self.kit.ok_or("no Kit")?);
                    let canvas = self.pages[page].canvas();
                    canvas.save();
                    canvas.clip_rect(Rect::from_xywh(x as f32, y as f32, width as f32, height as f32), None, false);
                    canvas.translate(((x - left) as f32, (y - top) as f32));
                    canvas.concat(&to_matrix(t));
                    canvas.concat(m);
                    let at = std::time::Instant::now();
                    let mut drawer = Drawer::new(tree, images, kit, canvas, 1.0);
                    if let Some(cast) = cast {
                        drawer.casting(Some(*cast));
                    }
                    draw(&mut drawer);
                    self.skia_took += at.elapsed();
                    if let Some(entry) = self.kinds.get_mut(name) {
                        entry.2 += at.elapsed().as_micros() as u32;
                    }
                    let counts = drawer.counts;
                    let mut asked = Asked {
                        missing: std::mem::take(&mut drawer.missing_images),
                        sharper: std::mem::take(&mut drawer.sharper_images),
                        used: std::mem::take(&mut drawer.used_images),
                    };
                    drop(drawer);
                    canvas.restore();
                    if let Some(key) = key.filter(|_| asked.missing.is_empty() && asked.sharper.is_empty()) {
                        let image = self.pages[page].image_snapshot_with_bounds(IRect::from_xywh(x, y, width, height));
                        if let (Some(image), Some(vello)) = (image, self.vello.as_deref_mut()) {
                            let images = asked.used.iter().filter_map(|(key, _)| self.images.get(key).map(|img| (key.clone(), img.image.unique_id()))).collect();
                            vello.keep_piece(key, Piece { image, counts, used: asked.used.clone(), images, view: views });
                        }
                    }
                    self.counts += counts;
                    self.asked.missing.append(&mut asked.missing);
                    self.asked.sharper.append(&mut asked.sharper);
                    self.asked.used.append(&mut asked.used);
                    let image = slot_image(width, height);
                    Plan::patch(encoding, effect, image.clone(), t.inverse() * Affine::translate((left as f64, top as f64)));
                    plan.skia.push((image, page, x, y));
                }
            }
        }
        Ok(())
    }
}

/// How much wider than it shows an image is asked for: as wide as the
/// mipmap level Figma's sampling draws from (see `Encoder::image`)
pub const ASKED: f64 = 1.42;

/// An image fill as Vello draws it, or not
enum Picture {
    /// Its brush, placed from 0 to 1 across the image, with the level
    /// encoded for now, and the image's key and how wide it's shown, in
    /// device pixels (see `Kind::Image`)
    Drawn(ImageBrush, Affine, String, f32),
    /// Nothing, as there's no image yet or it has no layout
    Nothing,
    /// As Skia draws it
    Skia,
}

/// `paint`'s gradient as Vello's, and how it's placed, where Vello draws it
/// as Skia does
fn gradient(paint: &rt::Paint, kind: PaintType, size: [f32; 2]) -> Option<(peniko::Brush, Option<Affine>)> {
    let (w, h) = (size[0].max(1e-3), size[1].max(1e-3));
    let t = rt::to_matrix(&paint.transform.unwrap_or(rt::IDENTITY));
    let mut local = Matrix::scale((w, h));
    local.pre_concat(&t.invert()?);
    let ramp = paint.gradient.as_ref()?;
    if ramp.stops.len() < 2 || ramp.stops.windows(2).any(|pair| pair[1].position < pair[0].position) {
        return None;
    }
    let opacity = paint.opacity.unwrap_or(1.0).clamp(0.0, 1.0);
    let stops: Vec<ColorStop> = ramp
        .stops
        .iter()
        .map(|stop| ColorStop { offset: stop.position.clamp(0.0, 1.0), color: DynamicColor::from_alpha_color(color(draw::color(tree::rgba(stop.color), opacity))) })
        .collect();
    let extend = if ramp.kind == Some(rt::GradientType::Wrap) { Extend::Repeat } else { Extend::Pad };
    let mut gradient = match kind {
        PaintType::GradientLinear => peniko::Gradient::new_linear((0.0, 0.5), (1.0, 0.5)),
        _ => peniko::Gradient::new_radial((0.5, 0.5), 0.5),
    }
    .with_extend(extend)
    .with_stops(stops.as_slice());
    gradient.interpolation_alpha_space = peniko::InterpolationAlphaSpace::Unpremultiplied;
    (gradient.interpolation_cs, gradient.hue_direction) = interpolation(paint);
    Some((peniko::Brush::Gradient(gradient), Some(affine(&local))))
}

/// The color space a gradient mixes its colors in, as tree.rs's
/// `interpolation` gives Skia
fn interpolation(paint: &rt::Paint) -> (peniko::color::ColorSpaceTag, peniko::color::HueDirection) {
    use peniko::color::{ColorSpaceTag, HueDirection};
    let space = match paint.interpolation_color_space {
        Some(rt::InterpolationColorSpace::Oklab) => ColorSpaceTag::Oklab,
        Some(rt::InterpolationColorSpace::Oklch) => ColorSpaceTag::Oklch,
        Some(rt::InterpolationColorSpace::Hsl) => ColorSpaceTag::Hsl,
        Some(rt::InterpolationColorSpace::SrgbLinear) => ColorSpaceTag::LinearSrgb,
        _ => return (ColorSpaceTag::Srgb, HueDirection::Shorter),
    };
    let hue = match paint.interpolation_hue_method {
        Some(rt::InterpolationHueMethod::Longer) => HueDirection::Longer,
        Some(rt::InterpolationHueMethod::Increasing) => HueDirection::Increasing,
        Some(rt::InterpolationHueMethod::Decreasing) => HueDirection::Decreasing,
        _ => HueDirection::Shorter,
    };
    (space, hue)
}

/// The frame a blur through `m` is blurred in: what turns it to `m`'s
/// space, along whose axes a blur round in the node's own space is
/// stretched, and by how much each way. Its axes are the node's where `m`
/// only scales and turns, so its box is square to them, which the last
/// says
fn blur_frame(m: &Matrix) -> (Affine, f32, f32, bool) {
    let (a, b, c, d) = (m.scale_x() as f64, m.skew_y() as f64, m.skew_x() as f64, m.scale_y() as f64);
    let size = (a * a + b * b + c * c + d * d).max(1e-24);
    let angle = if b.abs() < 1e-9 && c.abs() < 1e-9 {
        0.0
    } else if (a * b + c * d).abs() < 1e-6 * size && (a * a + c * c - b * b - d * d).abs() < 1e-6 * size {
        b.atan2(a)
    } else {
        // Where it stretches most and least, square to each other
        0.5 * (2.0 * (a * b + c * d)).atan2(a * a + c * c - b * b - d * d)
    };
    let (sin, cos) = angle.sin_cos();
    let row = [(cos * a + sin * b, cos * c + sin * d), (cos * b - sin * a, cos * d - sin * c)];
    let square = (row[0].1.abs() + row[1].0.abs()).min(row[0].0.abs() + row[1].1.abs()) < 1e-4 * size.sqrt();
    let turn = if angle == 0.0 { Affine::IDENTITY } else { Affine::rotate(angle) };
    (turn, row[0].0.hypot(row[0].1) as f32, row[1].0.hypot(row[1].1) as f32, square)
}

fn op_name(op: Op) -> &'static str {
    match op {
        Op::Blur => "blur",
        Op::HardMaskWithColor => "silhouette",
        Op::Noise => "noise",
        Op::Grain => "grain",
        _ => "effect",
    }
}

fn paint_name(kind: PaintType) -> &'static str {
    match kind {
        PaintType::Image => "image",
        PaintType::Pattern => "pattern",
        PaintType::GradientAngular => "angular",
        PaintType::GradientDiamond => "diamond",
        PaintType::GradientLinear | PaintType::GradientRadial => "gradient space",
        _ => "other paint",
    }
}
