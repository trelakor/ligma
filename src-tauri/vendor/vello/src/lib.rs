// Copyright 2022 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Vello is a compute-centric 2D graphics rendering engine written in Rust,
//! using [`wgpu`].
//! It efficiently draws large 2D scenes with interactive or near-interactive
//! performance.
//!
//! ![image](https://github.com/linebender/vello/assets/8573618/cc2b742e-2135-4b70-8051-c49aeddb5d19)
//!
//!
//! ## Motivation
//!
//! Vello is meant to fill the same place in the graphics stack as other vector graphics renderers like [Skia](https://skia.org/), [Cairo](https://www.cairographics.org/), and its predecessor project [Piet](https://www.cairographics.org/).
//! On a basic level, that means it provides tools to render shapes, images, gradients, texts, etc, using a PostScript-inspired API, the same that powers SVG files and [the browser `<canvas>` element](https://developer.mozilla.org/en-US/docs/Web/API/CanvasRenderingContext2D).
//!
//! Vello's selling point is that it gets better performance than other renderers by better leveraging the GPU.
//! In traditional PostScript-style renderers, some steps of the render process like sorting and clipping either need to be handled in the CPU or done through the use of intermediary textures.
//! Vello avoids this by using prefix-scan algorithms to parallelize work that usually needs to happen in sequence, so that work can be offloaded to the GPU with minimal use of temporary buffers.
//!
//! This means that Vello needs a GPU with support for compute shaders to run.
//!
//!
//! ## Getting started
//!
//! Vello is meant to be integrated deep in UI render stacks.
//! While drawing in a Vello [`Scene`] is easy, actually rendering that scene to a surface setting up a wgpu context, which is a non-trivial task.
//!
//! To use Vello as the renderer for your PDF reader / GUI toolkit / etc, your code will have to look roughly like this:
//!
//! ```ignore
//! // Initialize wgpu and get handles
//! let (width, height) = ...;
//! let device: wgpu::Device = ...;
//! let queue: wgpu::Queue = ...;
//! let mut renderer = Renderer::new(
//!    &device,
//!    RendererOptions {
//!       use_cpu: false,
//!       antialiasing_support: vello::AaSupport::all(),
//!       num_init_threads: NonZeroUsize::new(1),
//!    },
//! ).expect("Failed to create renderer");
//!
//! // Create scene and draw stuff in it
//! let mut scene = vello::Scene::new();
//! scene.fill(
//!    vello::peniko::Fill::NonZero,
//!    vello::Affine::IDENTITY,
//!    vello::Color::from_rgb8(242, 140, 168),
//!    None,
//!    &vello::Circle::new((420.0, 200.0), 120.0),
//! );
//!
//! // Draw more stuff
//! scene.push_layer(...);
//! scene.fill(...);
//! scene.stroke(...);
//! scene.pop_layer(...);
//!
//! let texture = device.create_texture(&...);
//! // Render to a wgpu Texture
//! renderer
//!    .render_to_texture(
//!       &device,
//!       &queue,
//!       &scene,
//!       &texture,
//!       &vello::RenderParams {
//!          base_color: palette::css::BLACK, // Background color
//!          width,
//!          height,
//!          antialiasing_method: AaConfig::Msaa16,
//!       },
//!    )
//!    .expect("Failed to render to a texture");
//! // Do things with surface texture, such as blitting it to the Surface using
//! // wgpu::util::TextureBlitter.
//! ```
//!
//! See the [`research/examples/`](https://github.com/linebender/vello/tree/main/research/examples) folder to see how that code integrates with frameworks like winit.

// LINEBENDER LINT SET - lib.rs - v2
// See https://linebender.org/wiki/canonical-lints/
// These lints aren't included in Cargo.toml because they
// shouldn't apply to examples and tests
#![warn(unused_crate_dependencies)]
#![warn(clippy::print_stdout, clippy::print_stderr)]
// Targeting e.g. 32-bit means structs containing usize can give false positives for 64-bit.
#![cfg_attr(target_pointer_width = "64", warn(clippy::trivially_copy_pass_by_ref))]
// END LINEBENDER LINT SET
#![cfg_attr(docsrs, feature(doc_cfg))]
// The following lints are part of the Linebender standard set,
// but resolving them has been deferred for now.
// Feel free to send a PR that solves one or more of these.
// Need to allow instead of expect until Rust 1.83 https://github.com/rust-lang/rust/pull/130025
#![allow(missing_docs, reason = "We have many as-yet undocumented items.")]
#![expect(
    missing_debug_implementations,
    clippy::cast_possible_truncation,
    clippy::missing_assert_message,
    reason = "Deferred"
)]
#![allow(
    clippy::todo,
    unreachable_pub,
    unnameable_types,
    reason = "Deferred, only apply in some feature sets so not expect"
)]

mod debug;
mod recording;
mod render;
mod scene;
mod shaders;

#[cfg(feature = "wgpu")]
pub mod util;
#[cfg(feature = "wgpu")]
mod wgpu_engine;
#[cfg(feature = "wgpu")]
pub use wgpu_engine::RUN_PARTS;

pub mod low_level {
    //! Utilities which can be used to create an alternative Vello renderer to [`Renderer`][crate::Renderer].
    //!
    //! These APIs have not been carefully designed, and might not be powerful enough for this use case.

    pub use crate::debug::DebugLayers;
    pub use crate::recording::{
        BindType, BufferProxy, Command, ImageFormat, ImageProxy, Recording, ResourceId,
        ResourceProxy, ShaderId,
    };
    pub use crate::render::Render;
    pub use crate::shaders::FullShaders;

    pub use vello_encoding;
}
/// Styling and composition primitives.
pub use peniko;
/// 2D geometry, with a focus on curves.
pub use peniko::kurbo;

#[cfg(feature = "wgpu")]
use peniko::ImageData;
#[cfg(feature = "wgpu")]
pub use wgpu;

pub use scene::{DrawGlyphs, Scene};
pub use vello_encoding::{FontEmbolden, Glyph, NormalizedCoord};

use low_level::ShaderId;
#[cfg(feature = "wgpu")]
use low_level::{FullShaders, Recording, Render};
use thiserror::Error;

#[cfg(feature = "wgpu")]
use debug::DebugLayers;
#[cfg(feature = "wgpu")]
use vello_encoding::{BumpAllocators, Resolver};
#[cfg(feature = "wgpu")]
use wgpu_engine::{ExternalResource, WgpuEngine};

#[cfg(feature = "wgpu")]
use std::{num::NonZeroUsize, sync::atomic::AtomicBool};
#[cfg(feature = "wgpu")]
use wgpu::{Device, Queue, TextureView};
#[cfg(all(feature = "wgpu", feature = "wgpu-profiler"))]
use wgpu_profiler::{GpuProfiler, GpuProfilerSettings};

/// Represents the anti-aliasing method to use during a render pass.
///
/// Can be configured for a render operation by setting [`RenderParams::antialiasing_method`].
/// Each value of this can only be used if the corresponding field on [`AaSupport`] was used.
///
/// This can be converted into an `AaSupport` using [`Iterator::collect`],
/// as `AaSupport` implements `FromIterator`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum AaConfig {
    /// Area anti-aliasing, where the alpha value for a pixel is computed from integrating
    /// the winding number over its square area.
    ///
    /// This technique produces very accurate values when the shape has winding number of 0 or 1
    /// everywhere, but can result in conflation artifacts otherwise.
    /// It generally has better performance than the multi-sampling methods.
    ///
    /// Can only be used if [enabled][AaSupport::area] for the `Renderer`.
    Area,
    /// Ligma: 4x multisampling, where GPUs' standard pattern has them
    Msaa4,
    /// 8x Multisampling
    ///
    /// Can only be used if [enabled][AaSupport::msaa8] for the `Renderer`.
    Msaa8,
    /// 16x Multisampling
    ///
    /// Can only be used if [enabled][AaSupport::msaa16] for the `Renderer`.
    Msaa16,
}

/// Represents the set of anti-aliasing configurations to enable during pipeline creation.
///
/// This is configured at `Renderer` creation time ([`Renderer::new`]) by setting
/// [`RendererOptions::antialiasing_support`].
///
/// This can be created from a set of `AaConfig` using [`Iterator::collect`],
/// as `AaSupport` implements `FromIterator`.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct AaSupport {
    /// Support [`AaConfig::Area`].
    pub area: bool,
    /// Support [`AaConfig::Msaa4`].
    pub msaa4: bool,
    /// Support [`AaConfig::Msaa8`].
    pub msaa8: bool,
    /// Support [`AaConfig::Msaa16`].
    pub msaa16: bool,
}

impl AaSupport {
    /// Support every anti-aliasing method.
    ///
    /// This might increase startup time, as more shader variations must be compiled.
    pub fn all() -> Self {
        Self {
            area: true,
            msaa4: true,
            msaa8: true,
            msaa16: true,
        }
    }

    /// Support only [`AaConfig::Area`].
    ///
    /// This should be the default choice for most users.
    pub fn area_only() -> Self {
        Self {
            area: true,
            msaa4: false,
            msaa8: false,
            msaa16: false,
        }
    }
}

impl FromIterator<AaConfig> for AaSupport {
    fn from_iter<T: IntoIterator<Item = AaConfig>>(iter: T) -> Self {
        let mut result = Self {
            area: false,
            msaa4: false,
            msaa8: false,
            msaa16: false,
        };
        for config in iter {
            match config {
                AaConfig::Area => result.area = true,
                AaConfig::Msaa4 => result.msaa4 = true,
                AaConfig::Msaa8 => result.msaa8 = true,
                AaConfig::Msaa16 => result.msaa16 = true,
            }
        }
        result
    }
}

/// Errors that can occur in Vello.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum Error {
    /// There is no available device with the features required by Vello.
    #[cfg(feature = "wgpu")]
    #[error("Couldn't find suitable device")]
    NoCompatibleDevice,
    /// Failed to create surface.
    /// See [`wgpu::CreateSurfaceError`] for more information.
    #[cfg(feature = "wgpu")]
    #[error("Couldn't create wgpu surface")]
    WgpuCreateSurfaceError(#[from] wgpu::CreateSurfaceError),
    /// Surface doesn't support the required texture formats.
    /// Make sure that you have a surface which provides one of
    /// [`TextureFormat::Rgba8Unorm`][wgpu::TextureFormat::Rgba8Unorm]
    /// or [`TextureFormat::Bgra8Unorm`][wgpu::TextureFormat::Bgra8Unorm] as texture formats.
    // TODO: Why does this restriction exist?
    #[cfg(feature = "wgpu")]
    #[error("Couldn't find `Rgba8Unorm` or `Bgra8Unorm` texture formats for surface")]
    UnsupportedSurfaceFormat,

    /// Used a buffer inside a recording while it was not available.
    /// Check if you have created it and not freed before its last usage.
    #[cfg(feature = "wgpu")]
    #[error("Buffer '{0}' is not available but used for {1}")]
    UnavailableBufferUsed(&'static str, &'static str),
    /// Failed to async map a buffer.
    /// See [`wgpu::BufferAsyncError`] for more information.
    #[cfg(feature = "wgpu")]
    #[error("Failed to async map a buffer")]
    BufferAsyncError(#[from] wgpu::BufferAsyncError),
    /// Failed to download an internal buffer for debug visualization.
    #[cfg(feature = "wgpu")]
    #[cfg(feature = "debug_layers")]
    #[error("Failed to download internal buffer '{0}' for visualization")]
    DownloadError(&'static str),

    #[cfg(feature = "wgpu")]
    #[error("wgpu Error from scope")]
    WgpuErrorFromScope(#[from] wgpu::Error),

    /// Ligma: the scene needs more of a buffer than the GPU can give it
    #[cfg(feature = "wgpu")]
    #[error("Ran out of room in the GPU's buffers")]
    OutOfRoom,

    /// Ligma: what was to be drawn again isn't kept any more
    #[cfg(feature = "wgpu")]
    #[error("The scene to draw again isn't kept")]
    NotRetained,

    /// Failed to create [`GpuProfiler`].
    /// See [`wgpu_profiler::CreationError`] for more information.
    #[cfg(feature = "wgpu-profiler")]
    #[error("Couldn't create wgpu profiler")]
    #[doc(hidden)] // End-users of Vello should not have `wgpu-profiler` enabled.
    ProfilerCreationError(#[from] wgpu_profiler::CreationError),

    /// Failed to compile the shaders.
    #[cfg(feature = "hot_reload")]
    #[error("Failed to compile shaders:\n{0}")]
    #[doc(hidden)] // End-users of Vello should not have `hot_reload` enabled.
    ShaderCompilation(#[from] vello_shaders::compile::ErrorVec),
}

#[cfg_attr(
    not(feature = "wgpu"),
    expect(dead_code, reason = "this can be unused when wgpu feature is not used")
)]
pub(crate) type Result<T, E = Error> = std::result::Result<T, E>;

/// Renders a scene into a texture or surface.
///
/// Currently, each renderer only supports a single surface format, if it
/// supports drawing to surfaces at all.
/// This is an assumption which is known to be limiting, and is planned to change.
#[cfg(feature = "wgpu")]
pub struct Renderer {
    #[cfg_attr(
        not(feature = "hot_reload"),
        expect(
            dead_code,
            reason = "Options are only used to reinitialise on a hot reload"
        )
    )]
    options: RendererOptions,
    engine: WgpuEngine,
    resolver: Resolver,
    image_atlas: Option<recording::ImageProxy>,
    /// Ligma: the scene last drawn, kept on the GPU to draw again
    retained: Option<render::Retained>,
    shaders: FullShaders,
    #[cfg(feature = "debug_layers")]
    debug: debug::DebugRenderer,
    #[cfg(feature = "wgpu-profiler")]
    #[doc(hidden)] // End-users of Vello should not have `wgpu-profiler` enabled.
    /// The profiler used with events for this renderer. This is *not* treated as public API.
    pub profiler: GpuProfiler,
    #[cfg(feature = "wgpu-profiler")]
    #[doc(hidden)] // End-users of Vello should not have `wgpu-profiler` enabled.
    /// The results from profiling. This is *not* treated as public API.
    pub profile_result: Option<Vec<wgpu_profiler::GpuTimerQueryResult>>,
    /// Ligma: renders' allocation counts being read back, and whether
    /// they've mapped
    pending: Vec<(wgpu::Buffer, std::sync::Arc<std::sync::Mutex<Option<bool>>>, Option<wgpu::SubmissionIndex>, bool, [u32; 7], u32)>,
    /// Ligma: of the renders `check` last checked, in the order rendered,
    /// which ran out of room
    pub ran_out: Vec<bool>,
    /// Ligma: how long the last render took to record and to run, in ms
    pub timing: (f64, f64),
}
// This is not `Send` (or `Sync`) on WebAssembly as the
// underlying wgpu types are not. This can be enabled with the
// `fragile-send-sync-non-atomic-wasm` feature in wgpu.
// See https://github.com/gfx-rs/wgpu/discussions/4127 for
// further discussion of this topic.
#[cfg(all(feature = "wgpu", not(target_arch = "wasm32")))]
static_assertions::assert_impl_all!(Renderer: Send);

/// Parameters used in a single render that are configurable by the client.
///
/// These are used in [`Renderer::render_to_texture`].
pub struct RenderParams {
    /// The background color applied to the target. This value is only applicable to the full
    /// pipeline.
    pub base_color: peniko::Color,

    /// Dimensions of the rasterization target
    pub width: u32,
    pub height: u32,

    /// The anti-aliasing algorithm. The selected algorithm must have been initialized while
    /// constructing the `Renderer`.
    pub antialiasing_method: AaConfig,

    /// Ligma: where the scene is put in the target, after its own
    /// transforms, as the coefficients of an affine
    pub view: [f32; 6],
    /// Ligma: kept on the GPU as this, where not 0, to be drawn again with
    /// `reuse`
    pub retain: u64,
    /// Ligma: sized from room of its own, so a render taking far more
    /// than others leaves theirs as it is, see `vello_encoding::ONE_OFF`
    pub one_off: bool,
    /// Ligma: the scene kept as `retain` drawn again where `view` puts it,
    /// the scene passed being left alone
    pub reuse: bool,
}

#[cfg(feature = "wgpu")]
/// Options which are set at renderer creation time, used in [`Renderer::new`].
pub struct RendererOptions {
    /// If true, run all stages up to fine rasterization on the CPU.
    ///
    /// This is not a recommended configuration as it is expected to have poor performance,
    /// but it can be useful for debugging.
    // TODO: Consider evolving this so that the CPU stages can be configured dynamically via
    // `RenderParams`.
    pub use_cpu: bool,

    /// Represents the enabled set of AA configurations. This will be used to determine which
    /// pipeline permutations should be compiled at startup.
    ///
    /// By default this will be all modes, to support the widest range of.
    /// It is recommended that most users configure this.
    pub antialiasing_support: AaSupport,

    /// How many threads to use for initialisation of shaders.
    ///
    /// Use `Some(1)` to use a single thread. This is recommended when on macOS
    /// (see <https://github.com/bevyengine/bevy/pull/10812#discussion_r1496138004>)
    ///
    /// Set to `None` to use a heuristic which will use many but not all threads
    ///
    /// Has no effect on WebAssembly
    ///
    /// Will default to `None` on most platforms, `Some(1)` on macOS.
    pub num_init_threads: Option<NonZeroUsize>,

    /// The pipeline cache to use when creating the shaders.
    ///
    /// For much more discussion of expected usage patterns, see the documentation on that type.
    pub pipeline_cache: Option<wgpu::PipelineCache>,
}

#[cfg(feature = "wgpu")]
impl Default for RendererOptions {
    fn default() -> Self {
        Self {
            use_cpu: false,
            antialiasing_support: AaSupport::all(),
            #[cfg(target_os = "macos")]
            num_init_threads: NonZeroUsize::new(1),
            #[cfg(not(target_os = "macos"))]
            num_init_threads: None,
            pipeline_cache: None,
        }
    }
}

#[cfg(feature = "wgpu")]
struct RenderResult {
    bump: Option<BumpAllocators>,
    #[cfg(feature = "debug_layers")]
    captured: Option<render::CapturedBuffers>,
}

#[cfg(feature = "wgpu")]
impl Renderer {
    /// Creates a new renderer for the specified device.
    pub fn new(device: &Device, options: RendererOptions) -> Result<Self> {
        let mut engine = WgpuEngine::new(options.use_cpu, options.pipeline_cache.clone());
        // If we are running in parallel (i.e. the number of threads is not 1)
        if options.num_init_threads != NonZeroUsize::new(1) {
            #[cfg(not(target_arch = "wasm32"))]
            engine.use_parallel_initialisation();
        }
        let shaders = shaders::full_shaders(device, &mut engine, &options)?;
        #[cfg(not(target_arch = "wasm32"))]
        engine.build_shaders_if_needed(device, options.num_init_threads);
        #[cfg(feature = "debug_layers")]
        let debug = debug::DebugRenderer::new(device, wgpu::TextureFormat::Rgba8Unorm, &mut engine);

        Ok(Self {
            options,
            engine,
            resolver: Resolver::new(),
            image_atlas: None,
            retained: None,
            shaders,
            #[cfg(feature = "debug_layers")]
            debug,
            #[cfg(feature = "wgpu-profiler")]
            profiler: GpuProfiler::new(device, GpuProfilerSettings::default())?,
            #[cfg(feature = "wgpu-profiler")]
            profile_result: None,
            pending: Vec::new(),
            ran_out: Vec::new(),
            timing: (0.0, 0.0),
        })
    }

    /// Renders a scene to the target texture.
    ///
    /// The texture is assumed to be of the specified dimensions and have been created with
    /// the [`wgpu::TextureFormat::Rgba8Unorm`] format and the [`wgpu::TextureUsages::STORAGE_BINDING`]
    /// flag set.
    ///
    /// If you want to render Vello content to a surface (such as in a UI toolkit), you have two options:
    /// 1) Render to an intermediate texture, which is the same size as the surface.
    ///    You would then use [`TextureBlitter`][wgpu::util::TextureBlitter] to blit the rendered result from
    ///    that texture to the surface.
    ///    This pattern is supported by the [`util`] module.
    /// 2) Call `render_to_texture` directly on the [`SurfaceTexture`][wgpu::SurfaceTexture]'s texture, if
    ///    it has the right usages. This should generally be avoided, as some GPUs assume that you will not
    ///    be rendering to the surface using a compute pipeline, and optimise accordingly.
    pub fn render_to_texture(
        &mut self,
        device: &Device,
        queue: &Queue,
        scene: &Scene,
        texture: &TextureView,
        params: &RenderParams,
    ) -> Result<()> {
        // Ligma: what each buffer coarse allocates from took is read back
        // once the GPU's done, see `check`
        let at = std::time::Instant::now();
        if params.reuse && !self.retained.as_ref().is_some_and(|kept| kept.id == params.retain && params.retain != 0) {
            return Err(Error::NotRetained);
        }
        let mut render = Render::new();
        vello_encoding::ENCODING_ONE_OFF.store(params.one_off, std::sync::atomic::Ordering::Relaxed);
        let mut recording = render.render_encoding_coarse(
            scene.encoding(),
            &mut self.resolver,
            &self.shaders,
            &mut self.image_atlas,
            &mut self.retained,
            params,
            true,
        );
        vello_encoding::ENCODING_ONE_OFF.store(false, std::sync::atomic::Ordering::Relaxed);
        // The room this render's buffers have, which others change before
        // it's checked
        let rooms = if params.one_off { &vello_encoding::ONE_OFF } else { &vello_encoding::CAPACITY }.all().map(|room| room.load(std::sync::atomic::Ordering::Relaxed));
        let least = vello_encoding::LEAST.load(std::sync::atomic::Ordering::Relaxed);
        let target = render.out_image();
        let bump_buf = render.bump_buf();
        render.record_fine(&self.shaders, &mut recording);
        let external_resources = [ExternalResource::Image(target, texture)];
        let recorded = at.elapsed();
        self.engine.run_recording(
            device,
            queue,
            &recording,
            &external_resources,
            "render_to_texture",
            #[cfg(feature = "wgpu-profiler")]
            &mut self.profiler,
        )?;
        for command in recording.commands {
            if let recording::Command::Upload(proxy, data) = command
                && proxy.name == "vello.scene"
            {
                *render::SPARE.lock().unwrap() = data;
            }
        }
        if let Some(buf) = self.engine.get_download(bump_buf) {
            let buf = buf.clone();
            let mapped = std::sync::Arc::new(std::sync::Mutex::new(None));
            let done = mapped.clone();
            buf.slice(..).map_async(wgpu::MapMode::Read, move |result| {
                *done.lock().unwrap() = Some(result.is_ok());
            });
            self.pending.push((buf, mapped, self.engine.submitted.clone(), params.one_off, rooms, least));
        }
        self.engine.free_download(bump_buf);
        self.timing = (recorded.as_secs_f64() * 1000.0, (at.elapsed() - recorded).as_secs_f64() * 1000.0);
        // N.B. This is horrible; this integration of wgpu-profiler really needs some work...
        #[cfg(feature = "wgpu-profiler")]
        {
            self.profiler.end_frame().unwrap();
            if let Some(result) = self
                .profiler
                .process_finished_frame(queue.get_timestamp_period())
            {
                self.profile_result = Some(result);
            }
        }

        Ok(())
    }

    /// Ligma: once the GPU's done with what's been rendered, sizes the
    /// buffers coarse allocates from to what it took, and false if any of
    /// it ran out of room, drawing what didn't fit wrong, to render again,
    /// with what ran out, of the first `renders` of those not yet checked,
    /// waiting for no others
    pub fn check(&mut self, device: &Device, renders: usize) -> Result<Option<String>> {
        let renders = renders.min(self.pending.len());
        if renders == 0 {
            return Ok(None);
        }
        let submission_index = self.pending[renders - 1].2.clone();
        let _ = device.poll(wgpu::PollType::Wait { submission_index, timeout: None });
        let mut short = None;
        self.ran_out.clear();
        for (buf, mapped, _, one_off, rooms, least) in self.pending.drain(..renders).collect::<Vec<_>>() {
            if *mapped.lock().unwrap() != Some(true) {
                return Err(Error::OutOfRoom);
            }
            let bump: BumpAllocators = {
                let view = buf.slice(..).get_mapped_range().map_err(|_| Error::OutOfRoom)?;
                bytemuck::pod_read_unaligned(&view)
            };
            buf.unmap();
            let fits = fit(&bump, one_off, rooms, least);
            self.ran_out.push(fits.is_some());
            if let Some(why) = fits {
                short = Some(why);
            }
        }
        Ok(short)
    }

    /// Ligma: gives back the GPU memory kept between renders, for when
    /// nothing's drawn for a while
    pub fn trim(&mut self) {
        self.retained = None;
        self.engine.trim(self.image_atlas.take());
        self.resolver.trim();
    }

    /// Ligma: how many renders haven't been checked
    pub fn unchecked(&self) -> usize {
        self.pending.len()
    }

    /// Ligma: dev tools, each stage's GPU time since asked last, once
    /// checked
    pub fn times(&mut self) -> String {
        let t = *LAST_TOOK.lock().unwrap();
        let took = format!(" took=binning {} ptcl {} tile {} seg_counts {} segments {} blend {} lines {}", t[0], t[1], t[2], t[3], t[4], t[5], t[6]);
        self.engine.timer.as_mut().map(|timer| timer.read()).unwrap_or_default() + &took
    }

    /// Overwrite `image` with `texture`.
    ///
    /// Most users should prefer [`register_texture`](Self::register_texture), which
    /// ergonomically encapulates these requirements.
    ///
    /// `texture` must have the [`wgpu::TextureFormat::Rgba8Unorm`] format and
    /// the [`wgpu::TextureUsages::COPY_SRC`] flag set. The `Rgba8UnormSrgb` format
    /// might also be supported, but this is not tested.
    ///
    /// The given `Texture`'s data will be copied into the slot in Vello's image
    /// atlas where the image would be placed each frame.
    /// This has the effect that wherever you request the image to be drawn (e.g.
    /// through [`Scene::draw_image`]), the data from the texture will be used instead.
    ///
    /// Correct behaviour is not guaranteed if the texture does not have the same
    /// dimensions as the image, nor if an image which uses the same [blob] but different
    /// dimensions would be rendered.
    ///
    /// [blob]: peniko::ImageData::data
    pub fn override_image(
        &mut self,
        image: &ImageData,
        texture: Option<wgpu::TexelCopyTextureInfoBase<wgpu::Texture>>,
    ) -> Option<wgpu::TexelCopyTextureInfoBase<wgpu::Texture>> {
        self.resolver.mark_image_dirty(image);
        match texture {
            Some(texture) => self.engine.image_overrides.insert(image.data.id(), texture),
            None => self.engine.image_overrides.remove(&image.data.id()),
        }
    }

    /// Marks `image` as dirty in the atlas cache, so its override texture will be recopied on
    /// the next render that uses it.
    ///
    /// Call this for any `image` whose existing override texture has changed since the last render.
    /// Otherwise, stale image data from the atlas may be used.
    pub fn mark_override_image_dirty(&mut self, image: &ImageData) {
        self.resolver.mark_image_dirty(image);
    }

    /// Register a [`wgpu::Texture`] with Vello, to allow drawing GPU-resident data.
    ///
    /// The returned `Image` can be used in [`Scene`]s (only those rendered with this `Renderer`)
    /// Rendering Scenes which use this `Image` with other `Renderer`s will panic.
    ///  
    /// `texture` must have the [`wgpu::TextureFormat::Rgba8Unorm`] format and
    /// the [`wgpu::TextureUsages::COPY_SRC`] flag set. This is because the data will
    /// be copied into Vello's image atlas at the start of each frame.
    /// The `Rgba8UnormSrgb` format might also be supported, but this is not tested.
    /// The texture is assumed to have unpremultiplied alpha.
    ///
    /// This is a utility wrapper around [`override_image`](Self::override_image).
    /// For greater control, use that method.
    ///
    /// If the texture is no longer active then it should be unregistered using [`unregister_texture`](Self::unregister_texture)
    pub fn register_texture(&mut self, texture: wgpu::Texture) -> ImageData {
        // Create a fake, empty blob which will be used to back the returned image
        // This image data will never be read by Vello, due to being added to
        // image_overrides, below.
        let fake_blob = peniko::Blob::new(std::sync::Arc::new(&[]));

        let image = ImageData {
            data: fake_blob,
            format: peniko::ImageFormat::Rgba8,
            alpha_type: peniko::ImageAlphaType::Alpha,
            width: texture.width(),
            height: texture.height(),
        };

        // For this utility API, we take the full texture and use the base layer and mip level
        let texture_base = wgpu::TexelCopyTextureInfoBase {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        };

        // We overwrite any attempt to use the fake blob, instead reading from the texture.
        self.override_image(&image, Some(texture_base));

        image
    }

    /// Unregister a [`wgpu::Texture`] that was registered with [`register_texture`](Self::register_texture).
    pub fn unregister_texture(&mut self, handle: ImageData) {
        self.override_image(&handle, None);
    }

    /// Reload the shaders. This should only be used during `vello` development
    #[cfg(feature = "hot_reload")]
    #[doc(hidden)] // End-users of Vello should not have `hot_reload` enabled.
    pub async fn reload_shaders(&mut self, device: &Device) -> Result<(), Error> {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut engine = WgpuEngine::new(self.options.use_cpu, self.options.pipeline_cache.clone());
        // We choose not to initialise these shaders in parallel, to ensure the error scope works correctly
        let shaders = shaders::full_shaders(device, &mut engine, &self.options)?;
        #[cfg(feature = "debug_layers")]
        let debug = debug::DebugRenderer::new(device, wgpu::TextureFormat::Rgba8Unorm, &mut engine);
        let error = scope.pop().await;
        if let Some(error) = error {
            return Err(error.into());
        }
        self.engine = engine;
        self.image_atlas = None;
        self.retained = None;
        self.shaders = shaders;
        #[cfg(feature = "debug_layers")]
        {
            self.debug = debug;
        }
        Ok(())
    }

    /// Renders a scene to the target texture using an async pipeline.
    ///
    /// Almost all consumers should prefer [`Self::render_to_texture`].
    ///
    /// The return value is the value of the `BumpAllocators` in this rendering, which is currently used
    /// for debug output.
    ///
    /// This return type is not stable, and will likely be changed when a more principled way to access
    /// relevant statistics is implemented
    #[cfg_attr(docsrs, doc(hidden))]
    #[deprecated(
        note = "render_to_texture should be preferred, as the _async version has no stability guarantees"
    )]
    pub async fn render_to_texture_async(
        &mut self,
        device: &Device,
        queue: &Queue,
        scene: &Scene,
        texture: &TextureView,
        params: &RenderParams,
        debug_layers: DebugLayers,
    ) -> Result<Option<BumpAllocators>> {
        if cfg!(not(feature = "debug_layers")) && !debug_layers.is_empty() {
            static HAS_WARNED: AtomicBool = AtomicBool::new(false);
            if !HAS_WARNED.swap(true, std::sync::atomic::Ordering::Release) {
                log::warn!(
                    "Requested debug layers {debug_layers:?} but `debug_layers` feature is not enabled"
                );
            }
        }

        let result = self
            .render_to_texture_async_internal(device, queue, scene, texture, params)
            .await?;

        #[cfg(feature = "debug_layers")]
        {
            let mut recording = Recording::default();
            let target_proxy = recording::ImageProxy::new(
                params.width,
                params.height,
                recording::ImageFormat::Rgba8,
            );
            if let Some(captured) = result.captured {
                let bump = result.bump.as_ref().unwrap();
                // TODO: We could avoid this download if `DebugLayers::VALIDATION` is unset.
                let downloads = DebugDownloads::map(&self.engine, &captured, bump).await?;
                self.debug.render(
                    &mut recording,
                    target_proxy,
                    &captured,
                    bump,
                    params,
                    &downloads,
                    debug_layers,
                );

                // TODO: this sucks. better to release everything in a helper
                // TODO: it would be much better to have a way to safely destroy a buffer.
                self.engine.free_download(captured.lines);
                captured.release_buffers(&mut recording);
            }
            let external_resources = [ExternalResource::Image(target_proxy, texture)];
            self.engine.run_recording(
                device,
                queue,
                &recording,
                &external_resources,
                "render_to_texture_async debug layers",
                #[cfg(feature = "wgpu-profiler")]
                &mut self.profiler,
            )?;
        }

        #[cfg(feature = "wgpu-profiler")]
        {
            self.profiler.end_frame().unwrap();
            if let Some(result) = self
                .profiler
                .process_finished_frame(queue.get_timestamp_period())
            {
                self.profile_result = Some(result);
            }
        }

        Ok(result.bump)
    }

    async fn render_to_texture_async_internal(
        &mut self,
        device: &Device,
        queue: &Queue,
        scene: &Scene,
        texture: &TextureView,
        params: &RenderParams,
    ) -> Result<RenderResult> {
        let mut render = Render::new();
        let encoding = scene.encoding();
        // TODO: turn this on; the download feature interacts with CPU dispatch.
        // Currently this is always enabled when the `debug_layers` setting is enabled as the bump
        // counts are used for debug visualiation.
        let robust = cfg!(feature = "debug_layers");
        let recording = render.render_encoding_coarse(
            encoding,
            &mut self.resolver,
            &self.shaders,
            &mut self.image_atlas,
            &mut self.retained,
            params,
            robust,
        );
        let target = render.out_image();
        let bump_buf = render.bump_buf();
        #[cfg(feature = "debug_layers")]
        let captured = render.take_captured_buffers();
        self.engine.run_recording(
            device,
            queue,
            &recording,
            &[],
            "t_async_coarse",
            #[cfg(feature = "wgpu-profiler")]
            &mut self.profiler,
        )?;

        let mut bump: Option<BumpAllocators> = None;
        if let Some(bump_buf) = self.engine.get_download(bump_buf) {
            let buf_slice = bump_buf.slice(..);
            let (sender, receiver) = futures_intrusive::channel::shared::oneshot_channel();
            buf_slice.map_async(wgpu::MapMode::Read, move |v| sender.send(v).unwrap());
            receiver.receive().await.expect("channel was closed")?;
            let mapped = buf_slice.get_mapped_range().unwrap();
            bump = Some(bytemuck::pod_read_unaligned(&mapped));
        }
        // TODO: apply logic to determine whether we need to rerun coarse, and also
        // allocate the blend stack as needed.
        self.engine.free_download(bump_buf);
        // Maybe clear to reuse allocation?
        let mut recording = Recording::default();
        render.record_fine(&self.shaders, &mut recording);
        let external_resources = [ExternalResource::Image(target, texture)];
        self.engine.run_recording(
            device,
            queue,
            &recording,
            &external_resources,
            "t_async_fine",
            #[cfg(feature = "wgpu-profiler")]
            &mut self.profiler,
        )?;
        Ok(RenderResult {
            bump,
            #[cfg(feature = "debug_layers")]
            captured,
        })
    }
}
/// Ligma: the most each buffer's taken lately, and when, which is
/// forgotten after `PEAK_KEPT`, and what it took the last time its stage
/// ran to the end
#[cfg(feature = "wgpu")]
static TAKEN: std::sync::Mutex<[(u32, Option<std::time::Instant>, u32); 7]> = std::sync::Mutex::new([(0, None, 0); 7]);
/// Ligma: the same of one-off renders
#[cfg(feature = "wgpu")]
static TAKEN_ONE_OFF: std::sync::Mutex<[(u32, Option<std::time::Instant>, u32); 7]> = std::sync::Mutex::new([(0, None, 0); 7]);

/// Ligma: the buffers' room back to what it starts with, as after an error,
/// so what led to it isn't carried over
#[cfg(feature = "wgpu")]
pub fn reset_room() {
    vello_encoding::CAPACITY.reset();
    vello_encoding::ONE_OFF.reset();
    for crossings in &vello_encoding::CROSSINGS {
        crossings.store(1f32.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }
    for words in &PTCL_PER_TILE {
        words.store(PTCL_PER_TILE_AT_FIRST.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }
    *TAKEN.lock().unwrap() = [(0, None, 0); 7];
    *TAKEN_ONE_OFF.lock().unwrap() = [(0, None, 0); 7];
    *WHOLE_TOOK.lock().unwrap() = ([0; 7], [0; 7]);
}

/// Ligma: how long buffers keep room for the most they took, so zooming
/// back out doesn't run out
#[cfg(feature = "wgpu")]
const PEAK_KEPT: std::time::Duration = std::time::Duration::from_secs(1);

/// Ligma: sizes the buffers coarse allocates from to what renders take,
/// with room to spare, and what ran out where this one did
#[cfg(feature = "wgpu")]
/// `x` rounded up to a power of two, or a quarter, half or three quarters
/// past one
fn size_class(x: u32) -> u32 {
    let step = (x.checked_next_power_of_two().unwrap_or(1 << 31) / 8).max(1);
    x.div_ceil(step).saturating_mul(step)
}

/// Ligma: command list words renders took for each tile, where both ran
/// to the end, the most lately, coming down slowly, as what the command
/// lists of one whose tiles ran out will take is known only from that
static PTCL_PER_TILE: [std::sync::atomic::AtomicU32; 2] = [std::sync::atomic::AtomicU32::new(PTCL_PER_TILE_AT_FIRST.to_bits()), std::sync::atomic::AtomicU32::new(PTCL_PER_TILE_AT_FIRST.to_bits())];

/// Ligma: about what views took
const PTCL_PER_TILE_AT_FIRST: f32 = 6.0;

/// Ligma: what the last render checked took of each buffer
pub static LAST_TOOK: std::sync::Mutex<[u32; 7]> = std::sync::Mutex::new([0; 7]);

/// Ligma: what the last whole view drawn exactly took of each buffer, which
/// a part of it, or some of what's in it, drawn again at its zoom takes no
/// more of, and what the parts of one drawn in parts took so far
#[cfg(feature = "wgpu")]
static WHOLE_TOOK: std::sync::Mutex<([u32; 7], [u32; 7])> = std::sync::Mutex::new(([0; 7], [0; 7]));

/// Ligma: what the render checked last took, as the whole view's or, with
/// `parts`, what its parts took, see `took_part`
#[cfg(feature = "wgpu")]
pub fn took_whole(parts: bool) {
    let mut whole = WHOLE_TOOK.lock().unwrap();
    whole.0 = if parts { whole.1 } else { *LAST_TOOK.lock().unwrap() };
}

/// Ligma: what the render checked last took, as one more part of a whole
/// view drawn in parts, its first where `first`. Lines are those of the
/// paths it draws, which most parts draw many of, so the most of a part
/// rather than of all of them
#[cfg(feature = "wgpu")]
pub fn took_part(first: bool) {
    let took = *LAST_TOOK.lock().unwrap();
    let mut whole = WHOLE_TOOK.lock().unwrap();
    for (index, (parts, took)) in whole.1.iter_mut().zip(took).enumerate() {
        *parts = match (first, index) {
            (true, _) => took,
            (false, 6) => (*parts).max(took),
            (false, _) => parts.saturating_add(took),
        };
    }
}

/// Ligma: room for what the whole view took and a quarter more, as `fit`
/// sizes it, in the room renders flagged `one_off` take, for what's drawn
/// again of it as something's edited, which otherwise ran out after its
/// room shrank to what tiles drawn meanwhile took and was drawn again two or
/// three times. Held as if taken, so `fit` gives it back once it's not
/// drawn for `PEAK_KEPT`
#[cfg(feature = "wgpu")]
pub fn room_for_whole(one_off: bool) {
    use std::sync::atomic::Ordering::Relaxed;
    let need = WHOLE_TOOK.lock().unwrap().0;
    let room = if one_off { vello_encoding::ONE_OFF.all() } else { vello_encoding::CAPACITY.all() };
    let mut taken = if one_off { TAKEN_ONE_OFF.lock().unwrap() } else { TAKEN.lock().unwrap() };
    for (index, ((room, need), taken)) in room.into_iter().zip(need).zip(taken.iter_mut()).enumerate() {
        if need == 0 {
            continue;
        }
        let cap = vello_encoding::most(index);
        let floor = if index == 1 { 1 << 19 } else { 1 << 18 };
        room.fetch_max(size_class(need.saturating_add(need / 4).max(floor)).min(cap), Relaxed);
        if need >= taken.0 || taken.1.is_none_or(|at| at.elapsed() > PEAK_KEPT) {
            *taken = (need.min(cap), Some(std::time::Instant::now()), taken.2);
        }
    }
}

fn fit(bump: &BumpAllocators, one_off: bool, had: [u32; 7], least: u32) -> Option<String> {
    use std::sync::atomic::Ordering::Relaxed;
    let room = if one_off { vello_encoding::ONE_OFF.all() } else { vello_encoding::CAPACITY.all() };
    let took = [bump.binning, bump.ptcl, bump.tile, bump.seg_counts, bump.segments, bump.blend, bump.lines];
    *LAST_TOOK.lock().unwrap() = took;
    let names = ["binning", "ptcl", "tile", "seg_counts", "segments", "blend", "lines"];
    // Tiles each segment crossed, where all the lines were there to count,
    // the most lately, coming down slowly, of scenes big enough to run out
    if least >= 1 << 16 && took[6] <= had[6] {
        let crossed = vello_encoding::CROSSINGS[one_off as usize].load(Relaxed);
        let (was, now) = (f32::from_bits(crossed), took[3] as f32 / least as f32);
        let next = if now > was { now } else { was * 0.75 + now * 0.25 };
        vello_encoding::CROSSINGS[one_off as usize].store(next.clamp(0.5, 4.0).to_bits(), Relaxed);
    }
    // The stage of each, in the order the GPU fills them: those past the
    // first that ran out never ran to the end, so what they took isn't what
    // they'd take
    let stage = [1, 4, 2, 3, 5, 4, 0];
    let (first, grow) = took.iter().zip(had).zip(stage).filter(|((took, have), _)| **took > *have).fold((usize::MAX, 1.25f64), |(first, grow), ((took, have), stage)| {
        (first.min(stage), grow.max(*took as f64 / have.max(1) as f64 * 1.25))
    });
    let ran = |index: usize| stage[index] <= first;
    if ran(1) && took[2] >= 1 << 16 {
        let (was, now) = (f32::from_bits(PTCL_PER_TILE[one_off as usize].load(Relaxed)), took[1] as f32 / took[2] as f32);
        let next = if now > was { now } else { was * 0.75 + now * 0.25 };
        PTCL_PER_TILE[one_off as usize].store(next.clamp(1.0, 32.0).to_bits(), Relaxed);
    }
    let mut short = (bump.failed != 0).then(|| format!("failed={:#x}", bump.failed));
    let mut too_big = false;
    let mut taken = if one_off { TAKEN_ONE_OFF.lock().unwrap() } else { TAKEN.lock().unwrap() };
    for (index, ((((room, took), have), taken), name)) in room.into_iter().zip(took).zip(had).zip(taken.iter_mut()).zip(names).enumerate() {
        if took >= taken.0 || taken.1.is_none_or(|at| at.elapsed() > PEAK_KEPT) {
            *taken = (took, Some(std::time::Instant::now()), taken.2);
        }
        if ran(index) {
            taken.2 = took;
        }
        let most = taken.0;
        let cap = vello_encoding::most(index);
        // A quarter more than it's taken lately, in the pool's size classes
        // so its buffers are used again, the command lists' 2MB at least,
        // which views often take more than
        let floor = if name == "ptcl" { 1 << 19 } else { 1 << 18 };
        let want = size_class(most.saturating_add(most / 4).max(floor)).min(cap);
        if took > have || took > have - have / 8 || (bump.failed == 0 && want < have - have / 4) {
            room.store(want.max(took.min(cap)), Relaxed);
        }
        if took > have {
            *short.get_or_insert_with(String::new) += &format!(" {name} {took}>{have}");
        }
        too_big |= took > cap;
    }
    if bump.failed != 0 {
        // Each segment counted is one written, and each line crosses as many
        // tiles as lines did lately, at least one
        let crossed = f32::from_bits(vello_encoding::CROSSINGS[one_off as usize].load(Relaxed));
        let per_tile = f32::from_bits(PTCL_PER_TILE[one_off as usize].load(Relaxed));
        let need = |index: usize| match index {
            1 if ran(2) => Some((took[2] as f32 * per_tile) as u32),
            4 if ran(3) => Some(took[3]),
            3 if ran(6) => Some(took[6].max((least as f32 * crossed) as u32)),
            _ => None,
        };
        for (index, ((room, (took, have)), stage)) in room.into_iter().zip(took.into_iter().zip(had)).zip(stage).enumerate() {
            if stage > first && took <= have {
                // Grown from what it took when it last ran to the end, not
                // from the room it has, so renders that run out again before
                // it's drawn don't grow it again and again
                let want = match need(index) {
                    Some(need) => need.saturating_add(need / 4) as f64,
                    None => taken[index].2.max(1 << 16).min(have.max(1)) as f64 * grow.min(8.0),
                };
                let want = size_class(want.min(u32::MAX as f64) as u32).min(vello_encoding::most(index));
                room.store(want.max(room.load(Relaxed)), Relaxed);
                // As if taken, so renders checked after that took less
                // don't give it back before it's drawn again
                if want > taken[index].0 {
                    taken[index] = (want, Some(std::time::Instant::now()), taken[index].2);
                }
            }
        }
    }
    // More than any buffer can hold, so drawing it again won't fit either
    if too_big {
        short = Some(format!("too big for Vello's buffers:{}", short.unwrap_or_default()));
    }
    short
}

#[cfg(all(feature = "debug_layers", feature = "wgpu"))]
pub(crate) struct DebugDownloads<'a> {
    pub lines: wgpu::BufferSlice<'a>,
}

#[cfg(all(feature = "debug_layers", feature = "wgpu"))]
impl<'a> DebugDownloads<'a> {
    pub async fn map(
        engine: &'a WgpuEngine,
        captured: &render::CapturedBuffers,
        bump: &BumpAllocators,
    ) -> Result<DebugDownloads<'a>> {
        use vello_encoding::LineSoup;

        let Some(lines_buf) = engine.get_download(captured.lines) else {
            return Err(Error::DownloadError("linesoup"));
        };

        let lines = lines_buf.slice(..bump.lines as u64 * size_of::<LineSoup>() as u64);
        let (sender, receiver) = futures_intrusive::channel::shared::oneshot_channel();
        lines.map_async(wgpu::MapMode::Read, move |v| sender.send(v).unwrap());
        receiver.receive().await.expect("channel was closed")?;
        Ok(Self { lines })
    }
}
