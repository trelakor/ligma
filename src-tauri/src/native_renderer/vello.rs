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
use vello::wgpu;
use windows::core::Interface;
use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE, LUID};
use windows::Win32::Graphics::Direct3D12::{ID3D12CommandQueue, ID3D12Device, ID3D12Fence, ID3D12Resource, D3D12_FENCE_FLAG_SHARED};

use super::draw::{self, Counts, Img, Kit};
use super::present::Tex;
use super::rt::{self, MaskOpType, Node, Op, PaintType};
use super::tree::{self, Drawer, Place, Tree};

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
    /// What stands for it in scenes
    image: ImageData,
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
    out: Option<Target>,
    pages: Vec<Page>,
    /// Vello's copies of paths, by the path they're of
    paths: HashMap<usize, (Arc<rt::Path>, Arc<BezPath>)>,
    /// Set if wgpu reports an error, after which Vello isn't used
    broken: Arc<std::sync::atomic::AtomicBool>,
}

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
            required_features: adapter.features() & wgpu::Features::CLEAR_TEXTURE,
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
            vello::RendererOptions { antialiasing_support: vello::AaSupport { area: false, msaa8: false, msaa16: true }, ..Default::default() },
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
            out: None,
            pages: Vec::new(),
            paths: HashMap::new(),
            broken,
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
    /// `height`, and what stands for it in scenes
    fn page(&mut self, skia: &ID3D12Device, index: usize, width: i32, height: i32) -> Result<(Arc<Tex>, ImageData), String> {
        if self.pages.first().is_some_and(|page| page.target.tex.width < width || page.target.tex.height < height) {
            self.pages.clear();
        }
        while self.pages.len() <= index {
            let target = Self::make(skia, &self.device, width, height)?;
            let image = ImageData {
                data: peniko::Blob::new(Arc::new([0u8; 0])),
                format: peniko::ImageFormat::Rgba8,
                alpha_type: peniko::ImageAlphaType::AlphaPremultiplied,
                width: target.tex.width as u32,
                height: target.tex.height as u32,
            };
            self.pages.push(Page { target, image, used: std::time::Instant::now() });
        }
        let page = &mut self.pages[index];
        page.used = std::time::Instant::now();
        Ok((page.target.tex.clone(), page.image.clone()))
    }

    /// Draws `scene` over `width` by `height` pixels into the texture it
    /// returns, unpremultiplied, once Skia's queue has drawn the atlas.
    /// Skia's queue waits for it before what's submitted after
    pub fn render(&mut self, skia: &ID3D12Device, skia_queue: &ID3D12CommandQueue, scene: &vello::Scene, width: i32, height: i32, pages: usize) -> Result<Arc<Tex>, String> {
        let out = Self::target(skia, &self.device, &mut self.out, width, height)?;
        self.signalled += 1;
        let value = self.signalled;
        unsafe { skia_queue.Signal(&self.atlas_drawn.skia, value) }.map_err(|error| format!("signal: {error}"))?;
        let out_texture = self.wrap(self.out.as_ref().ok_or("no target")?, wgpu::TextureUsages::STORAGE_BINDING);
        for index in 0..pages.min(self.pages.len()) {
            let texture = self.wrap(&self.pages[index].target, wgpu::TextureUsages::COPY_SRC);
            let base = wgpu::TexelCopyTextureInfoBase { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All };
            self.renderer.override_image(&self.pages[index].image, Some(base));
        }
        // Pages no view has needed for a while go
        while self.pages.len() > pages && self.pages.last().is_some_and(|page| page.used.elapsed() > PAGE_KEPT) {
            if let Some(page) = self.pages.pop() {
                self.renderer.override_image(&page.image, None);
            }
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
            antialiasing_method: vello::AaConfig::Msaa16,
        };
        let at = std::time::Instant::now();
        self.renderer.render_to_texture(&self.device, &self.queue, scene, &view, &params).map_err(|error| format!("render: {error}"))?;
        self.took = at.elapsed();
        // Signalled after all Vello submitted, on a submission of its own
        {
            let hal = unsafe { self.queue.as_hal::<Dx12>() }.ok_or("not D3D12")?;
            hal.add_signal_fence(self.view_drawn.vello.clone(), value);
        }
        self.queue.submit([self.device.create_command_encoder(&Default::default()).finish()]);
        let _ = self.device.poll(wgpu::PollType::Poll);
        unsafe { skia_queue.Wait(&self.view_drawn.skia, value) }.map_err(|error| format!("wait: {error}"))?;
        Ok(out)
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

fn mix(mode: Option<rt::BlendMode>) -> Result<Option<Mix>, &'static str> {
    use rt::BlendMode::*;
    Ok(Some(match mode {
        Option::None | Some(None) | Some(PassThrough) | Some(Normal) => return Ok(Option::None),
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
        // Figma's own soft light and the linear ones, which Skia draws with
        // blenders of ours
        // Dev tools: drawn close to them
        Some(SoftLight) if std::env::var_os("LIGMA_NATIVE_VELLO_INEXACT").is_some() => Mix::SoftLight,
        Some(LinearBurn) if std::env::var_os("LIGMA_NATIVE_VELLO_INEXACT").is_some() => Mix::Multiply,
        Some(LinearDodge) if std::env::var_os("LIGMA_NATIVE_VELLO_INEXACT").is_some() => Mix::Screen,
        _ => return Err("blend mode"),
    }))
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
#[derive(Default)]
pub struct Asked {
    pub missing: Vec<(String, f64)>,
    pub sharper: Vec<(String, f64)>,
    pub used: Vec<(String, f64)>,
}

/// Builds a Vello scene of units of the tree, in device pixels of the view
pub struct Encoder<'a> {
    pub tree: &'a Tree,
    pub images: &'a HashMap<String, Img>,
    pub kit: &'a Kit,
    pub vello: &'a mut Vello,
    pub scene: vello::Scene,
    skia: &'a ID3D12Device,
    context: &'a mut skia_safe::gpu::DirectContext,
    /// Skia's surfaces over the pages of the atlas used, and what stands
    /// for them in the scene
    pages: Vec<(skia_safe::Surface, ImageData)>,
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
            vello,
            scene: vello::Scene::new(),
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
        }
    }

    /// Skia's surfaces over the pages of the atlas used, to submit
    pub fn take_pages(&mut self) -> Vec<skia_safe::Surface> {
        self.pages.drain(..).map(|(surface, _)| surface).collect()
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
            let (tex, image) = self.vello.page(self.skia, self.pages.len(), self.view.width(), self.view.height()).map_err(|error| log::warn!("native renderer: Vello's atlas: {error}")).ok()?;
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
            self.pages.push((surface, image));
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
        match &self.soft {
            Some(soft) => {
                self.scene.push_clip_layer(fill_of(soft), Affine::IDENTITY, &bez(soft));
                self.scene.fill(style, transform, brush, brush_transform, shape);
                self.scene.pop_layer();
            }
            None => self.scene.fill(style, transform, brush, brush_transform, shape),
        }
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
        if bounds.width().max(bounds.height()) * self.scale() < 0.5 {
            self.counts.skipped += 1;
            return;
        }
        if !Rect::intersects2(&self.m.map_rect(bounds).0, &Rect::from(self.view)) {
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
                let opacity = node.opacity.unwrap_or(1.0).clamp(0.0, 1.0);
                if opacity <= 0.0 {
                    return;
                }
                let isolate = !tree::pass_through(node.blend_mode) || self.tree.info(child).blends;
                if opacity >= 0.999 && !tree::blends(node.blend_mode) && !isolate {
                    self.draw(child);
                    return;
                }
                let mix = match mix(node.blend_mode) {
                    Ok(mix) => mix.unwrap_or(Mix::Normal),
                    Err(why) => return self.fail(why),
                };
                if self.push(BlendMode::new(mix, Compose::SrcOver), opacity, bounds) {
                    self.draw(child);
                    self.pop();
                }
            }
            Op::Mask => self.mask(node, id, bounds),
            // Blurs, silhouettes, noise, grain and effects, as Skia draws them
            Op::Blur
            | Op::HardMaskWithColor
            | Op::Noise
            | Op::Grain
            | Op::CustomEffect
            | Op::CustomEffectFill
            | Op::Glass
            | Op::SpatialPlane
            | Op::SpatialTransform
            | Op::SpatialComposition => self.fall_back(op_name(node.op), self.tree.info(id).blends, bounds, |drawer| drawer.draw(id)),
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
                if node.stroke_geometries.iter().any(|g| g.paints.iter().any(|paint| tree::blends(paint.blend_mode))) {
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
        let brush: Option<(peniko::Brush, Option<Affine>)> = match kind {
            PaintType::Solid => Some((peniko::Brush::Solid(color(draw::color(tree::rgba(paint.color.unwrap_or_default()), opacity))), None)),
            PaintType::GradientLinear | PaintType::GradientRadial => gradient(paint, kind, node.paint_size.unwrap_or([1.0, 1.0])),
            _ => None,
        };
        let Some((brush, brush_transform)) = brush else {
            // Images, patterns and the rest, as Skia draws them
            let bounds = skia.bounds().with_outset((1.0, 1.0));
            return self.fall_back(paint_name(kind), tree::blends(paint.blend_mode), bounds, |drawer| drawer.paint(node, paint, skia));
        };
        let mix = match mix(paint.blend_mode) {
            Ok(mix) => mix,
            Err(why) => return self.fail(why),
        };
        let blended = match mix {
            Some(mix) => self.push(BlendMode::new(mix, Compose::SrcOver), 1.0, skia.bounds().with_outset((1.0, 1.0))),
            None => false,
        };
        self.fill(fill_of(skia), affine(&self.m), &brush, brush_transform, path);
        if blended {
            self.pop();
        }
    }

    fn mask(&mut self, node: &Node, id: u32, bounds: Rect) {
        let (Some(alpha), Some(color)) = (node.alpha_node, node.color_node) else {
            return;
        };
        let op = node.mask_op.unwrap_or(MaskOpType::Normal);
        // A background blur's blurred backdrop comes in the tree, as what
        // it masks
        if !matches!(op, MaskOpType::Normal | MaskOpType::Pattern | MaskOpType::BackgroundBlur) {
            return self.fall_back("mask kind", self.tree.info(id).blends, bounds, |drawer| drawer.draw(id));
        }
        let blends = self.tree.info(color).blends;
        // A clip cuts what's under it once, as one, as Figma masks. What's
        // masked mixes with what's under the mask in Figma, which in a clip
        // of Vello's it wouldn't, so then each thing's cut on its own
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
        if blends {
            // Dev tools: drawn as if it didn't blend with what's under it,
            // counted
            if std::env::var_os("LIGMA_NATIVE_VELLO_INEXACT").is_none() {
                return self.fail(format!("mask over blends, of {}", self.tree.census(alpha)));
            }
            self.inexact += 1;
        }
        // Wherever its shape is at all, which takes Skia's silhouettes
        if op == MaskOpType::BackgroundBlur {
            return self.fall_back("background blur", false, bounds, |drawer| drawer.draw(id));
        }
        if !self.push(BlendMode::default(), 1.0, bounds) {
            return;
        }
        self.draw(color);
        if self.push(BlendMode::new(Mix::Normal, Compose::DestIn), 1.0, bounds) {
            self.draw(alpha);
            self.pop();
        }
        self.pop();
    }

    /// What Skia draws with `draw`, over `bounds` in the current space, put
    /// on the atlas at the zoom it's shown at and placed from there
    fn fall_back(&mut self, kind: &'static str, blends: bool, bounds: Rect, draw: impl FnOnce(&mut Drawer)) {
        // Drawn alone, it can't mix with what's under it
        if blends {
            return self.fail("blends in what Skia draws");
        }
        let Some(device) = self.device(bounds) else { return };
        let Some((page, x, y)) = self.room(device.width(), device.height()) else {
            return self.fail("atlas full");
        };
        self.fell_back += device.width() as i64 * device.height() as i64;
        let entry = self.kinds.entry(kind).or_default();
        entry.0 += 1;
        entry.1 += device.width() as i64 * device.height() as i64;
        let (tree, images, kit) = (self.tree, self.images, self.kit);
        let canvas = self.pages[page].0.canvas();
        canvas.save();
        canvas.clip_rect(Rect::from_xywh(x as f32, y as f32, device.width() as f32, device.height() as f32), None, false);
        canvas.translate(((x - device.left) as f32, (y - device.top) as f32));
        canvas.concat(&self.m);
        let at = std::time::Instant::now();
        let mut drawer = Drawer::new(tree, images, kit, canvas, 1.0);
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
        let brush = ImageBrush {
            image: self.pages[page].1.clone(),
            sampler: ImageSampler { x_extend: Extend::Pad, y_extend: Extend::Pad, quality: ImageQuality::Low, alpha: 1.0 },
        };
        let rect = kurbo::Rect::new(device.left as f64, device.top as f64, device.right as f64, device.bottom as f64);
        let at = Affine::translate(((device.left - x) as f64, (device.top - y) as f64));
        self.fill(Fill::NonZero, Affine::IDENTITY, &brush, Some(at), &rect);
    }
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
