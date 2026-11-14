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
    out: Option<Target>,
    pages: Vec<Page>,
    /// Vello's copies of paths, by the path they're of
    paths: HashMap<usize, (Arc<rt::Path>, Arc<BezPath>)>,
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
            Ok(_) => log::info!("native renderer: Vello started in {:.0}ms", started.elapsed().as_secs_f64() * 1000.0),
            Err(error) => log::warn!("native renderer: Vello didn't start: {error}"),
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
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .map_err(|error| format!("device: {error}"))?;
        let broken = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = broken.clone();
        device.on_uncaptured_error(Arc::new(move |error| {
            log::warn!("native renderer: Vello: {error}");
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }));
        let renderer = vello::Renderer::new(
            &device,
            vello::RendererOptions { antialiasing_support: vello::AaSupport { area: std::env::var_os("LIGMA_NATIVE_VELLO_AREA").is_some(), msaa4: true, msaa8: false, msaa16: std::env::var_os("LIGMA_NATIVE_VELLO_MSAA16").is_some() }, ..Default::default() },
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
            out: None,
            pages: Vec::new(),
            paths: HashMap::new(),
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
    ) -> Result<Arc<Tex>, String> {
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
            } else if std::env::var_os("LIGMA_NATIVE_VELLO_MSAA16").is_some() {
                vello::AaConfig::Msaa16
            } else {
                vello::AaConfig::Msaa4
            },
        };
        let at = std::time::Instant::now();
        let dropped = vello::low_level::vello_encoding::DROPPED_IMAGES.load(std::sync::atomic::Ordering::Relaxed);
        self.stages.clear();
        self.sync("start");
        let blurred = self.blur(plan, params.antialiasing_method);
        self.took_blurs = at.elapsed();
        let rendered = blurred.and_then(|()| self.renderer.render_to_texture(&self.device, &self.queue, scene, &view, &params).map_err(|error| format!("render: {error}")));
        self.stages += &format!(" record={:.1} run={:.1}", self.renderer.timing.0, self.renderer.timing.1);
        self.sync("main");
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
    pub fn check(&mut self) -> Result<Option<String>, String> {
        let at = std::time::Instant::now();
        let fits = self.renderer.check(&self.device).map_err(|error| format!("check: {error}"));
        self.waited = at.elapsed();
        self.stages += &self.renderer.times();
        // Dev tools: what Vello's memory holds
        if let Some(report) = self.device.generate_allocator_report() {
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
            let params = vello::RenderParams { base_color: AlphaColor::<Srgb>::new([0.0, 0.0, 0.0, 0.0]), width, height, antialiasing_method };
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

    /// Vello's copy of `path`, made once
    pub fn path(&mut self, tree: &Tree, path: &Arc<rt::Path>) -> Arc<BezPath> {
        let key = Arc::as_ptr(path) as usize;
        if let Some((_, made)) = self.paths.get(&key) {
            return made.clone();
        }
        let made = Arc::new(bez(&tree.skia_path(path)));
        self.paths.insert(key, (path.clone(), made.clone()));
        made
    }

    /// Lets go of copies of paths the tree no longer has
    pub fn prune(&mut self) {
        self.paths.retain(|_, (path, _)| Arc::strong_count(path) > 1);
    }
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
    pub fn at(&self, px: f32, left: i32, top: i32) -> Affine {
        let ratio = px as f64 / 2f64.powi(self.level);
        Affine::translate((self.origin.0 as f64 * px as f64 - left as f64, self.origin.1 as f64 * px as f64 - top as f64)) * Affine::scale(ratio)
    }
}

/// `shape` drawn by `transform`, as lines no further than `tolerance` from
/// it, where it's small enough that's fewer than it has
fn simplified(shape: &impl kurbo::Shape, transform: Affine, tolerance: f64) -> Option<BezPath> {
    let bounds = transform.transform_rect_bbox(shape.bounding_box());
    if bounds.width().max(bounds.height()) > 64.0 {
        return None;
    }
    let had = shape.path_elements(0.1).count();
    let mut out = BezPath::new();
    let mut line: Vec<kurbo::Point> = Vec::new();
    let mut keep = Vec::new();
    // Half the tolerance flattening, half leaving points out
    let mut close = |line: &mut Vec<kurbo::Point>, out: &mut BezPath| {
        if line.len() >= 3 {
            let first = line[0];
            if line.last() != Some(&first) {
                line.push(first);
            }
            fewer(line, tolerance / 2.0, &mut keep);
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
    kurbo::flatten(shape.path_elements(0.1).map(|element| transform * element), tolerance / 2.0, |element| match element {
        kurbo::PathEl::MoveTo(p) => {
            close(&mut line, &mut out);
            line.push(p);
        }
        kurbo::PathEl::LineTo(p) => line.push(p),
        _ => close(&mut line, &mut out),
    });
    close(&mut line, &mut out);
    (out.elements().len() < had).then_some(out)
}

/// The indices of the points of the closed `line` that keep it within
/// `tolerance` of itself, by Douglas and Peucker's way
fn fewer(line: &[kurbo::Point], tolerance: f64, keep: &mut Vec<usize>) {
    keep.clear();
    let last = line.len() - 1;
    // A closed line splits at the point furthest from its start first
    let far = (1..last).max_by(|&a, &b| (line[a] - line[0]).hypot2().total_cmp(&(line[b] - line[0]).hypot2())).unwrap_or(0);
    let mut kept = vec![false; line.len()];
    kept[0] = true;
    kept[last] = true;
    if (line[far] - line[0]).hypot() <= tolerance {
        return;
    }
    kept[far] = true;
    let mut spans = vec![(0, far), (far, last)];
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

/// Dev tools: fills and their segments by size in device pixels, under
/// 1/4, 1, 4, 16, 64, 256 and over
pub static SIZES: [std::sync::atomic::AtomicU64; 14] = [const { std::sync::atomic::AtomicU64::new(0) }; 14];

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
    },
    /// What Skia draws, with `m` from what's drawn to the scene's space,
    /// which each view has it draw onto the atlas at the zoom it's at
    Skia {
        name: &'static str,
        m: Matrix,
        cast: Option<Color4f>,
        draw: SkiaDraw,
    },
}

/// What Skia draws for a view
type SkiaDraw = Box<dyn for<'d> Fn(&mut Drawer<'d>) + Send>;

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
    depths: Vec<Depth>,
    /// What stands for what Skia drew, by page of the atlas and where
    skia: Vec<(ImageData, usize, i32, i32)>,
}

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
        Self { width: width.clamp(256, 8192), depths: Vec::new(), skia: Vec::new() }
    }

    /// How many blurs it has
    pub fn len(&self) -> usize {
        self.depths.iter().map(|depth| depth.blurs.len()).sum()
    }

    /// Has `effect` draw `image`, with `transform` from its pixels to the
    /// scene's space
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
    pub kit: &'a Kit,
    pub vello: &'a mut Vello,
    pub scene: vello::Scene,
    skia: &'a ID3D12Device,
    context: &'a mut skia_safe::gpu::DirectContext,
    /// Skia's surfaces over the pages of the atlas used
    pages: Vec<skia_safe::Surface>,
    shelf: Shelf,
    /// The view's pixels
    view: IRect,
    m: Matrix,
    pub counts: Counts,
    pub asked: Asked,
    /// Why the view can't be drawn this way, if it can't
    pub failed: Option<String>,
    /// Pixels Skia drew into the atlas
    pub fell_back: i64,
    /// Dev tools: masks drawn as if what's in them didn't blend
    pub inexact: usize,
    /// Time Skia took drawing onto the atlas
    pub skia_took: std::time::Duration,
    /// Dev tools: what Skia drew, how many and their pixels
    pub kinds: std::collections::BTreeMap<&'static str, (u32, i64)>,
    /// Cuts each thing drawn, in device pixels (see `soften`)
    soft: Option<Path>,
    /// The soft clip to put back as each layer's popped
    saved: Vec<Option<Path>>,
    depth: u8,
    /// Encoding a fragment, for any view: nothing's cut by the view
    retained: bool,
    /// Image fills' textures drawn, by key, mipmap level and Vello's id
    /// for them
    pub pictures: Vec<(String, u32, u64)>,
    /// Drawing what casts a silhouette, in this color: every paint solid
    /// wherever it's at all, as Figma casts shadows
    cast: Option<Color4f>,
    /// Blurs in the scene, to be put in place
    pub effects: Vec<Effect>,
}

impl<'a> Encoder<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tree: &'a Tree,
        images: &'a HashMap<String, Img>,
        kit: &'a Kit,
        vello: &'a mut Vello,
        skia: &'a ID3D12Device,
        context: &'a mut skia_safe::gpu::DirectContext,
        width: i32,
        height: i32,
    ) -> Self {
        Self {
            tree,
            images,
            kit,
            scene: {
                let mut scene = std::mem::take(&mut vello.spare);
                scene.reset();
                scene
            },
            vello,
            skia,
            context,
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
            pictures: Vec::new(),
            cast: None,
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
            let tex = self.vello.page(self.skia, self.pages.len(), self.view.width(), self.view.height()).map_err(|error| log::warn!("native renderer: Vello's atlas: {error}")).ok()?;
            let mut surface = skia_safe::gpu::surfaces::wrap_backend_texture(
                self.context,
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
    /// cut by that instead, as what's in it is drawn alone
    fn push(&mut self, mode: BlendMode, alpha: f32, bounds: Rect) -> bool {
        let Some(device) = self.device(bounds) else { return false };
        match self.soft.take() {
            Some(soft) => {
                self.scene.push_layer(fill_of(&soft), mode, alpha, Affine::IDENTITY, &bez(&soft));
                self.saved.push(Some(soft));
            }
            None => {
                let rect = kurbo::Rect::new(device.left as f64, device.top as f64, device.right as f64, device.bottom as f64);
                self.scene.push_layer(Fill::NonZero, mode, alpha, Affine::IDENTITY, &rect);
                self.saved.push(None);
            }
        }
        true
    }

    /// Clipped to `device`, a path in device pixels, and any soft clip
    fn clip(&mut self, device: Path) {
        let saved = self.soft.take();
        let shape = match &saved {
            Some(soft) => Path::op(soft, &device, skia_safe::PathOp::Intersect).unwrap_or(device),
            None => device,
        };
        self.scene.push_clip_layer(fill_of(&shape), Affine::IDENTITY, &bez(&shape));
        self.saved.push(saved);
    }

    fn pop(&mut self) {
        self.scene.pop_layer();
        self.soft = self.saved.pop().flatten();
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
        let tolerance = if self.retained { 1.0 / 128.0 } else { 1.0 / 64.0 };
        if std::env::var_os("LIGMA_NATIVE_VELLO_WHOLE").is_none() {
            if let Some(lines) = simplified(shape, transform, tolerance) {
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
        if std::env::var_os("LIGMA_NATIVE_VELLO_SIZES").is_some() {
            let b = transform.transform_rect_bbox(shape.bounding_box());
            let size = b.width().max(b.height());
            let bucket = [0.25, 1.0, 4.0, 16.0, 64.0, 256.0].iter().position(|&edge| size < edge).unwrap_or(6);
            let segments = shape.path_elements(0.1).count() as u64;
            SIZES[bucket * 2].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            SIZES[bucket * 2 + 1].fetch_add(segments, std::sync::atomic::Ordering::Relaxed);
        }
        match &self.soft {
            Some(soft) => {
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
        let Some(node) = self.tree.node(id).cloned() else { return };
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
                let (skia, bez) = (self.tree.skia_path(path), self.vello.path(self.tree, path));
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
                if !self.push(BlendMode::default(), 1.0, bounds) {
                    return;
                }
                self.cast = Some(BLACK);
                self.draw(alpha);
                self.cast = None;
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
            // Noise, grain and effects, as Skia draws them
            Op::Noise
            | Op::Grain
            | Op::CustomEffect
            | Op::CustomEffectFill
            | Op::Glass
            | Op::SpatialPlane
            | Op::SpatialTransform
            | Op::SpatialComposition => self.fall_back(op_name(node.op), self.tree.info(id).blends, bounds, Box::new(move |drawer| drawer.draw(id))),
            _ => {
                for child in node.child.into_iter().chain(node.children.iter().copied()) {
                    self.draw(child);
                }
            }
        }
    }

    fn fill_stroke(&mut self, node: &Node) {
        for geometry in &node.fill_geometries {
            let Some(path) = &geometry.path else { continue };
            let (skia, bez) = (self.tree.skia_path(path), self.vello.path(self.tree, path));
            for paint in &geometry.paints {
                self.paint(node, paint, &skia, &bez);
            }
        }
        if node.stroke_geometries.iter().all(|g| g.paints.is_empty()) {
            return;
        }
        let outlines: Vec<Path> = node.fill_geometries.iter().filter_map(|g| g.path.as_ref()).map(|path| self.tree.skia_path(path)).collect();
        let outline = match outlines.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            [first, rest @ ..] => Some(rest.iter().fold(first.clone(), |all, path| all.op(path, skia_safe::PathOp::Union).unwrap_or(all))),
        };
        let strokes = |this: &mut Self| {
            for geometry in &node.stroke_geometries {
                let Some(path) = &geometry.path else { continue };
                let (skia, bez) = (this.tree.skia_path(path), this.vello.path(this.tree, path));
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

    /// One paint over `path`, in the node's own coordinates
    fn paint(&mut self, node: &Node, paint: &rt::Paint, skia: &Path, path: &BezPath) {
        let kind = paint.kind.unwrap_or(PaintType::Solid);
        let opacity = paint.opacity.unwrap_or(1.0).clamp(0.0, 1.0);
        if let Some(cast) = self.cast {
            return match self.silhouette(node, paint, kind, opacity) {
                Some(true) => self.fill(fill_of(skia), affine(&self.m), color(cast), None, path),
                Some(false) => {}
                None => {
                    let bounds = skia.bounds().with_outset((1.0, 1.0));
                    let (node, paint, skia) = (node.clone(), paint.clone(), skia.clone());
                    self.fall_back(paint_name(kind), false, bounds, Box::new(move |drawer| drawer.paint(&node, &paint, &skia)))
                }
            };
        }
        let brush: Option<(peniko::Brush, Option<Affine>)> = match kind {
            PaintType::Image => match self.image(paint, node.paint_size.unwrap_or([1.0, 1.0]), opacity) {
                Picture::Drawn(brush, at) => Some((peniko::Brush::Image(brush), Some(at))),
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
            let (node, paint, skia) = (node.clone(), paint.clone(), skia.clone());
            return self.fall_back(paint_name(kind), tree::blends(paint.blend_mode), bounds, Box::new(move |drawer| drawer.paint(&node, &paint, &skia)));
        };
        let blended = match mix(paint.blend_mode) {
            Some(mode) => self.push(mode, 1.0, skia.bounds().with_outset((1.0, 1.0))),
            None => false,
        };
        self.fill(fill_of(skia), affine(&self.m), &brush, brush_transform, path);
        if blended {
            self.pop();
        }
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
        let Some(key) = tree::image_key(paint) else { return Picture::Nothing };
        let (w, h) = (size[0].max(1e-3), size[1].max(1e-3));
        // A fragment's shown up to twice as big as it's encoded
        let node_scale = self.m.scale_x().hypot(self.m.skew_y()) * if self.retained { 2.0 } else { 1.0 };
        let Some(img) = self.images.get(&key) else {
            if !self.asked.missing.iter().any(|(missing, _)| *missing == key) {
                self.asked.missing.push((key, (w.max(h) * node_scale * 1.42) as f64));
            }
            return Picture::Nothing;
        };
        let Some((to_node, tile)) = tree::image_layout(paint, w, h, img) else { return Picture::Nothing };
        let (iw, ih) = (img.original.0 as f32, img.original.1 as f32);
        let shown = to_node.scale_x().hypot(to_node.skew_y()) * node_scale * iw;
        let kept = img.image.width() as f32;
        self.asked.used.push((key.clone(), (shown * 1.42) as f64));
        if kept < shown * 1.42 && iw > kept && kept < 4096.0 {
            self.asked.sharper.push((key.clone(), (shown * 1.42) as f64));
        }
        // Up to 4 texels a pixel where it's shown biggest, which Figma's
        // sampling takes as they are. More, and a mipmap level with 2 to 4
        // of its texels a pixel there, as each of its texels is the average
        // of those under it, so Vello's atlas holds no more than that
        let texels = kept / shown.max(1e-6);
        let level = if texels <= 4.0 { 0 } else { (texels / 2.0).log2().floor().clamp(0.0, 12.0) as u32 };
        let Some((data, level)) = (0..=level).rev().find_map(|level| self.vello.picture(&key, img, level).map(|data| (data, level))) else {
            return Picture::Skia;
        };
        let mut local = to_node;
        local.pre_scale((iw / data.width as f32, ih / data.height as f32), None);
        // Ligma's shaders take reflect for transparent outside the image,
        // and high quality for Figma's sampling
        let extend = if tile == skia_safe::TileMode::Repeat { Extend::Repeat } else { Extend::Reflect };
        self.pictures.push((key, level, data.data.id()));
        let brush = ImageBrush { image: data, sampler: ImageSampler { x_extend: extend, y_extend: extend, quality: ImageQuality::High, alpha: opacity } };
        Picture::Drawn(brush, affine(&local))
    }

    fn mask(&mut self, node: &Node, id: u32, bounds: Rect) {
        let (Some(alpha), Some(color)) = (node.alpha_node, node.color_node) else {
            return;
        };
        let op = node.mask_op.unwrap_or(MaskOpType::Normal);
        let ext = node.ext();
        match op {
            MaskOpType::KnockoutShadow => {
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
        // What's masked blends with what's under the mask, as masks don't
        // isolate it in Figma: it's drawn over a copy of that, then the copy
        // is kept only where the mask is
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
        self.saved.push(self.soft.take());
        true
    }

    /// `id` drawn as what casts a silhouette, where only its shape matters
    fn casting(&mut self, id: u32) {
        let was = self.cast;
        self.cast = Some(was.unwrap_or(BLACK));
        self.draw(id);
        self.cast = was;
    }

    /// A blur of `node`'s child: its source drawn in a scene of its own,
    /// and an image in its place, which each view draws, blurs and puts in
    /// place
    fn blur(&mut self, node: &Node, id: u32, bounds: Rect) {
        let Some(child) = node.child else { return };
        let ext = node.ext();
        let m = self.m;
        let sigma = draw::tent_sigma(node.radius.filter(|radius| radius.is_finite()).unwrap_or(0.0).max(0.0) as f64);
        if ext.blur_op_type == Some(rt::BlurOpType::Progressive) || m.has_perspective() || m.skew_x().abs() > 1e-6 || m.skew_y().abs() > 1e-6 {
            return self.fall_back("blur", false, bounds, Box::new(move |drawer| drawer.draw(id)));
        }
        if sigma <= 0.0 {
            return self.draw(child);
        }
        let Some(reach) = self.tree.info(child).bounds else { return };
        let (sx, sy) = (m.scale_x().abs(), m.scale_y().abs());
        let krect = |rect: Rect| {
            let rect = m.map_rect(rect).0;
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
                reach: krect(reach),
                sigma: ((sigma * sx) as f64, (sigma * sy) as f64),
                clamp: ext.clamp.map(|clamp| krect(clamp.to_skia())),
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
        // Drawn alone, it can't mix with what's under it
        if blends {
            return self.fail("blends in what Skia draws");
        }
        let device = self.m.map_rect(bounds).0;
        // Pixels more each way, where it's smoothed at the edges, as what's
        // drawn there is transparent
        let pixel = 2.0;
        let rect = kurbo::Rect::new((device.left - pixel) as f64, (device.top - pixel) as f64, (device.right + pixel) as f64, (device.bottom + pixel) as f64);
        let Some((transform, patch)) = self.stand_in(rect) else { return };
        self.effects.push(Effect { kind: Kind::Skia { name: kind, m: self.m, cast: self.cast, draw }, bounds: rect, transform, patch });
    }

    /// Puts `effects`, of a scene with `encoding` that `t` places over
    /// `view` of a texture, in place, `depth` blurs deep
    pub fn put(&mut self, plan: &mut Plan, effects: &mut [Effect], encoding: &mut Encoding, t: Affine, view: kurbo::Rect, depth: usize) -> Result<(), String> {
        let c = t.as_coeffs();
        let scale = c[0].hypot(c[1]);
        for effect in effects {
            match &mut effect.kind {
                Kind::Blur { source, effects, reach, sigma, clamp } => {
                    let sigma = (sigma.0 * scale, sigma.1 * scale);
                    let margin = (3.0 * sigma.0.max(sigma.1)).ceil() + 2.0;
                    let shown = t.transform_rect_bbox(effect.bounds).intersect(view).intersect(t.transform_rect_bbox(*reach).inflate(margin, margin));
                    if shown.width() <= 0.0 || shown.height() <= 0.0 {
                        Plan::patch(encoding, effect, nothing(), Affine::IDENTITY);
                        continue;
                    }
                    let region = shown.inflate(margin, margin);
                    let (left, top) = (region.x0.floor() as i32, region.y0.floor() as i32);
                    let (width, height) = (region.x1.ceil() as i32 - left, region.y1.ceil() as i32 - top);
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
                    let to_slot = Affine::translate(((x - left) as f64, (y - top) as f64)) * t;
                    let slot = kurbo::Rect::new(x as f64, y as f64, (x + width) as f64, (y + height) as f64);
                    self.put(plan, effects, source.encoding_mut(), to_slot, slot, depth + 1)?;
                    let scene = &mut plan.depths[depth].scene;
                    scene.push_clip_layer(Fill::NonZero, Affine::IDENTITY, &slot);
                    if let Some(clamp) = clamp {
                        scene.push_clip_layer(Fill::NonZero, to_slot, clamp);
                    }
                    scene.append(source, Some(to_slot));
                    if clamp.is_some() {
                        scene.pop_layer();
                    }
                    scene.pop_layer();
                    let image = slot_image(width, height);
                    let clamp = clamp.map(|clamp| {
                        let clamp = to_slot.transform_rect_bbox(clamp);
                        [(clamp.x0 - x as f64) as f32, (clamp.y0 - y as f64) as f32, (clamp.x1 - x as f64) as f32, (clamp.y1 - y as f64) as f32]
                    });
                    let job = Job { slot: [x, y, width, height], sigma: (sigma.0 as f32, sigma.1 as f32), clamp };
                    Plan::patch(encoding, effect, image.clone(), t.inverse() * Affine::translate((left as f64, top as f64)));
                    plan.depths[depth].blurs.push(Placed { job, image });
                }
                Kind::Skia { name, m, cast, draw } => {
                    let shown = t.transform_rect_bbox(effect.bounds).intersect(view);
                    if shown.width() <= 0.0 || shown.height() <= 0.0 {
                        Plan::patch(encoding, effect, nothing(), Affine::IDENTITY);
                        continue;
                    }
                    let (left, top) = (shown.x0.floor() as i32, shown.y0.floor() as i32);
                    let (width, height) = (shown.x1.ceil() as i32 - left, shown.y1.ceil() as i32 - top);
                    let (page, x, y) = self.room(width, height).ok_or("atlas full")?;
                    self.fell_back += width as i64 * height as i64;
                    let entry = self.kinds.entry(name).or_default();
                    entry.0 += 1;
                    entry.1 += width as i64 * height as i64;
                    let (tree, images, kit) = (self.tree, self.images, self.kit);
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
                    let counts = drawer.counts;
                    let mut asked = Asked {
                        missing: std::mem::take(&mut drawer.missing_images),
                        sharper: std::mem::take(&mut drawer.sharper_images),
                        used: std::mem::take(&mut drawer.used_images),
                    };
                    drop(drawer);
                    canvas.restore();
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

/// An image fill as Vello draws it, or not
enum Picture {
    Drawn(ImageBrush, Affine),
    /// Nothing, as there's no image yet or it has no layout
    Nothing,
    /// As Skia draws it
    Skia,
}

/// `paint`'s gradient as Vello's, and how it's placed, where Vello draws it
/// as Skia does
fn gradient(paint: &rt::Paint, kind: PaintType, size: [f32; 2]) -> Option<(peniko::Brush, Option<Affine>)> {
    if paint.interpolation_color_space.is_some_and(|space| !matches!(space, rt::InterpolationColorSpace::Default)) {
        return None;
    }
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
    Some((peniko::Brush::Gradient(gradient), Some(affine(&local))))
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
