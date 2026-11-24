// Copyright 2023 the Vello Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

use wgpu::{
    BindGroup, BindGroupLayout, Buffer, BufferUsages, CommandEncoder, CommandEncoderDescriptor,
    ComputePassDescriptor, ComputePipeline, Device, PipelineCache, PipelineCompilationOptions,
    Queue, Texture, TextureAspect, TextureUsages, TextureView, TextureViewDimension,
};

use crate::{
    Error, Result,
    low_level::{BufferProxy, Command, ImageProxy, Recording, ResourceId, ResourceProxy, ShaderId},
    recording::BindType,
};
use vello_shaders::cpu::CpuBinding;

#[cfg(not(target_arch = "wasm32"))]
struct UninitialisedShader {
    wgsl: Cow<'static, str>,
    label: &'static str,
    entries: Vec<wgpu::BindGroupLayoutEntry>,
    shader_id: ShaderId,
}

#[derive(Default)]
pub(crate) struct WgpuEngine {
    shaders: Vec<Shader>,
    pool: ResourcePool,
    bind_map: BindMap,
    downloads: HashMap<ResourceId, Buffer>,
    #[cfg(not(target_arch = "wasm32"))]
    shaders_to_initialise: Option<Vec<UninitialisedShader>>,
    pub(crate) use_cpu: bool,
    /// Overrides from a specific `Image::data`'s [`id`](peniko::Blob::id) to a wgpu `Texture`.
    ///
    /// The `Texture` should have the same size as the `Image`.
    pub(crate) image_overrides: HashMap<u64, wgpu::TexelCopyTextureInfoBase<Texture>>,
    pipeline_cache: Option<PipelineCache>,
    /// Ligma: buffers kept to write big uploads into, mapped again once
    /// the GPU's copied from them, rather than wgpu making new memory
    /// ready for each
    staging: Staging,
    /// Ligma: dev tools, each stage's time on the GPU
    pub(crate) timer: Option<Timer>,
    timer_tried: bool,
    /// Ligma: the last recording's submission
    pub(crate) submitted: Option<wgpu::SubmissionIndex>,
    bind_groups: BindGroups,
}

/// Ligma: bind groups kept by shader and the buffers they bind, made once
/// rather than for each dispatch as the pool hands a render the same buffers
/// each time. None binding an image is kept, and none past a buffer of its
/// being destroyed or `POOL_KEPT` recordings unused, so none holds a buffer
/// that'd otherwise be gone for long
#[derive(Default)]
struct BindGroups {
    kept: HashMap<(usize, Vec<(Buffer, u64, u64)>), (BindGroup, u64)>,
}

/// Ligma: most bind groups kept at once
const BIND_GROUPS_KEPT: usize = 256;

impl BindGroups {
    fn forget(&mut self, buf: &Buffer) {
        self.kept.retain(|(_, bufs), _| bufs.iter().all(|(kept, ..)| kept != buf));
    }
}

/// Dev tools: time `run_recording` took by part, in ns
pub static RUN_PARTS: [std::sync::atomic::AtomicU64; 6] = [const { std::sync::atomic::AtomicU64::new(0) }; 6];
struct Part<'a>(usize, &'a mut std::time::Instant);
impl Drop for Part<'_> {
    fn drop(&mut self) {
        let now = std::time::Instant::now();
        RUN_PARTS[self.0].fetch_add((now - *self.1).as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
        *self.1 = now;
    }
}

pub(crate) struct Timer {
    set: wgpu::QuerySet,
    period: f64,
    labels: Vec<&'static str>,
    pending: Vec<(Buffer, Vec<&'static str>, std::sync::Arc<std::sync::atomic::AtomicBool>, bool)>,
}

impl Timer {
    fn new(device: &Device, queue: &Queue) -> Option<Self> {
        if std::env::var_os("LIGMA_NATIVE_VELLO_TIMES").is_none() || !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }
        let set = device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("ligma.times"), ty: wgpu::QueryType::Timestamp, count: 512 });
        Some(Self { set, period: queue.get_timestamp_period() as f64, labels: Vec::new(), pending: Vec::new() })
    }

    fn pass(&mut self, label: &'static str) -> ComputePassDescriptor<'_> {
        let at = self.labels.len() as u32 * 2;
        if at >= 512 {
            return ComputePassDescriptor::default();
        }
        self.labels.push(label);
        ComputePassDescriptor {
            label: None,
            timestamp_writes: Some(wgpu::ComputePassTimestampWrites { query_set: &self.set, beginning_of_pass_write_index: Some(at), end_of_pass_write_index: Some(at + 1) }),
        }
    }

    fn resolve(&mut self, device: &Device, encoder: &mut CommandEncoder) {
        if self.labels.is_empty() {
            return;
        }
        let size = self.labels.len() as u64 * 16;
        let resolve = device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: BufferUsages::QUERY_RESOLVE | BufferUsages::COPY_SRC, mapped_at_creation: false });
        let read = device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: BufferUsages::MAP_READ | BufferUsages::COPY_DST, mapped_at_creation: false });
        encoder.resolve_query_set(&self.set, 0..self.labels.len() as u32 * 2, &resolve, 0);
        encoder.copy_buffer_to_buffer(&resolve, 0, &read, 0, size);
        self.pending.push((read, std::mem::take(&mut self.labels), Default::default(), false));
    }

    fn submitted(&mut self) {
        for (read, _, mapped, asked) in &mut self.pending {
            if !*asked {
                *asked = true;
                let done = mapped.clone();
                read.slice(..).map_async(wgpu::MapMode::Read, move |result| done.store(result.is_ok(), std::sync::atomic::Ordering::Relaxed));
            }
        }
    }

    /// Each stage's time since last read, in ms, once the GPU's done
    pub(crate) fn read(&mut self) -> String {
        let mut totals: Vec<(&'static str, f64, u32)> = Vec::new();
        let mut each = String::new();
        for (read, labels, mapped, _) in std::mem::take(&mut self.pending) {
            if !mapped.load(std::sync::atomic::Ordering::Relaxed) {
                continue;
            }
            if let Ok(view) = read.slice(..).get_mapped_range() {
                let stamps: &[u64] = bytemuck::cast_slice(&view);
                let first = stamps.first().copied().unwrap_or(0);
                let last = stamps.iter().copied().max().unwrap_or(0);
                let fine = labels.iter().position(|l| l.contains("fine")).map_or(0.0, |index| stamps[index * 2 + 1].saturating_sub(stamps[index * 2]) as f64 * self.period / 1e6);
                each += &format!("{:.1}(fine{fine:.1}),", last.saturating_sub(first) as f64 * self.period / 1e6);
                for (index, label) in labels.iter().enumerate() {
                    let ms = stamps[index * 2 + 1].saturating_sub(stamps[index * 2]) as f64 * self.period / 1e6;
                    match totals.iter_mut().find(|(l, ..)| l == label) {
                        Some(total) => {
                            total.1 += ms;
                            total.2 += 1;
                        }
                        None => totals.push((label, ms, 1)),
                    }
                }
            }
            read.unmap();
        }
        totals.sort_by(|a, b| b.1.total_cmp(&a.1));
        let all: f64 = totals.iter().map(|t| t.1).sum();
        let mut out = format!(" passes={all:.1} renders={each}");
        for (label, ms, count) in totals.iter().take(10) {
            out += &format!(" {label}={ms:.1}/{count}");
        }
        out
    }
}

#[derive(Default)]
struct Staging {
    free: Vec<Buffer>,
    ready: std::sync::Arc<std::sync::Mutex<Vec<Buffer>>>,
    used: Vec<Buffer>,
}

impl Staging {
    /// Whether `upload` would copy `bytes` with the encoder
    fn takes(bytes: &[u8]) -> bool {
        let len = bytes.len() as u64;
        len >= 1 << 18 && len % wgpu::COPY_BUFFER_ALIGNMENT == 0
    }

    /// Writes `bytes` into a mapped buffer copied from into `buf`, false
    /// where it can't
    fn upload(&mut self, device: &Device, encoder: &mut CommandEncoder, buf: &Buffer, bytes: &[u8]) -> bool {
        if !Self::takes(bytes) {
            return false;
        }
        let len = bytes.len() as u64;
        self.free.append(&mut self.ready.lock().unwrap());
        let staging = match self.free.iter().enumerate().filter(|(_, b)| b.size() >= len).min_by_key(|(_, b)| b.size()) {
            Some((index, _)) => self.free.swap_remove(index),
            None => device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ligma.staging"),
                size: len.next_power_of_two(),
                usage: BufferUsages::MAP_WRITE | BufferUsages::COPY_SRC,
                mapped_at_creation: true,
            }),
        };
        match staging.get_mapped_range_mut(..len) {
            Ok(mut view) => view.copy_from_slice(bytes),
            Err(_) => return false,
        }
        staging.unmap();
        encoder.copy_buffer_to_buffer(&staging, 0, buf, 0, len);
        self.used.push(staging);
        true
    }

    /// Maps what was used again once the GPU's done with it, keeping a few
    fn submitted(&mut self) {
        for staging in self.used.drain(..) {
            let ready = self.ready.clone();
            let kept = staging.clone();
            staging.slice(..).map_async(wgpu::MapMode::Write, move |result| {
                if result.is_ok() {
                    ready.lock().unwrap().push(kept);
                }
            });
        }
        self.free.sort_by_key(|b| std::cmp::Reverse(b.size()));
        self.free.truncate(1);
    }
}

enum PipelineState {
    Compute(ComputePipeline),
    #[cfg(feature = "debug_layers")]
    Render(wgpu::RenderPipeline),
}

struct WgpuShader {
    pipeline: PipelineState,
    bind_group_layout: BindGroupLayout,
}

pub(crate) enum CpuShaderType {
    Present(fn(u32, &[CpuBinding<'_>])),
    Missing,
    Skipped,
}

struct CpuShader {
    shader: fn(u32, &[CpuBinding<'_>]),
}

enum ShaderKind<'a> {
    Wgpu(&'a WgpuShader),
    Cpu(&'a CpuShader),
}

struct Shader {
    label: &'static str,
    wgpu: Option<WgpuShader>,
    cpu: Option<CpuShader>,
}

impl Shader {
    fn select(&self) -> ShaderKind<'_> {
        if let Some(cpu) = self.cpu.as_ref() {
            ShaderKind::Cpu(cpu)
        } else if let Some(wgpu) = self.wgpu.as_ref() {
            ShaderKind::Wgpu(wgpu)
        } else {
            panic!("no available shader for {}", self.label)
        }
    }
}

pub(crate) enum ExternalResource<'a> {
    #[expect(unused, reason = "No buffers are accepted as arguments currently")]
    Buffer(BufferProxy, &'a Buffer),
    Image(ImageProxy, &'a TextureView),
}

/// A buffer can exist either on the GPU or on CPU.
enum MaterializedBuffer {
    Gpu(Buffer),
    Cpu(RefCell<Vec<u8>>),
}

struct BindMapBuffer {
    buffer: MaterializedBuffer,
    label: &'static str,
}

#[derive(Default)]
struct BindMap {
    buf_map: HashMap<ResourceId, BindMapBuffer>,
    image_map: HashMap<ResourceId, (Texture, TextureView)>,
    pending_clears: HashSet<ResourceId>,
}

/// Ligma: buffers free to use again, by name and usage, each with the
/// recording it was last used in, and the biggest asked for of each lately,
/// so one buffer of each serves however much is asked for, rather than one
/// of every size it's been
#[derive(Default)]
struct ResourcePool {
    bufs: HashMap<(&'static str, BufferUsages), Vec<(Buffer, u64)>>,
    most: HashMap<(&'static str, BufferUsages), (u64, u64)>,
    recordings: u64,
}

/// Ligma: recordings a free buffer's kept for without being used
const POOL_KEPT: u64 = 6;

/// The transient bind map contains short-lifetime resources.
///
/// In particular, it has resources scoped to a single call of
/// `run_recording()`, including external resources and also buffer
/// uploads.
#[derive(Default)]
struct TransientBindMap<'a> {
    bufs: HashMap<ResourceId, TransientBuf<'a>>,
    // TODO: create transient image type
    images: HashMap<ResourceId, &'a TextureView>,
}

enum TransientBuf<'a> {
    Cpu(&'a [u8]),
    Gpu(&'a Buffer),
}

impl WgpuEngine {
    /// Ligma: gives back the buffers kept to use again, and the persistent
    /// image `atlas`, for when nothing's drawn for a while
    pub(crate) fn trim(&mut self, atlas: Option<ImageProxy>) {
        if let Some((texture, _)) = atlas.and_then(|atlas| self.bind_map.image_map.remove(&atlas.id)) {
            texture.destroy();
        }
        for (buf, _) in self.pool.bufs.drain().flat_map(|(_, bufs)| bufs) {
            buf.destroy();
        }
        self.pool.most.clear();
        self.bind_groups.kept.clear();
        self.staging.free.clear();
        self.staging.ready.lock().unwrap().clear();
    }

    pub fn new(use_cpu: bool, pipeline_cache: Option<PipelineCache>) -> Self {
        Self {
            use_cpu,
            pipeline_cache,
            ..Default::default()
        }
    }

    /// Enable creating any remaining shaders in parallel
    #[cfg(not(target_arch = "wasm32"))]
    pub fn use_parallel_initialisation(&mut self) {
        if self.shaders_to_initialise.is_some() {
            return;
        }
        self.shaders_to_initialise = Some(Vec::new());
    }

    #[cfg(not(target_arch = "wasm32"))]
    /// Initialise (in parallel) any shaders which are yet to be created
    pub fn build_shaders_if_needed(
        &mut self,
        device: &Device,
        num_threads: Option<std::num::NonZeroUsize>,
    ) {
        use std::num::NonZeroUsize;

        if let Some(mut new_shaders) = self.shaders_to_initialise.take() {
            let num_threads = num_threads
                .map(NonZeroUsize::get)
                .unwrap_or_else(|| {
                    // Fallback onto a heuristic. This tries to not to use all threads.
                    // We keep the main thread blocked and not doing much whilst this is running,
                    // so we broadly leave two cores unused at the point of maximum parallelism
                    // (This choice is arbitrary, and could be tuned, although a 'proper' threadpool
                    // should probably be used instead)
                    std::thread::available_parallelism().map_or(2, |it| it.get().max(4) - 2)
                })
                .min(new_shaders.len());
            log::info!("Initialising shader modules in parallel using {num_threads} threads");
            let remainder = new_shaders.split_off(num_threads);
            let (tx, rx) = std::sync::mpsc::channel::<(ShaderId, WgpuShader)>();

            // We expect each initialisation to take much longer than acquiring a lock, so we just
            // use a mutex for our work queue
            let work_queue = std::sync::Mutex::new(remainder.into_iter());
            let work_queue = &work_queue;
            std::thread::scope(|scope| {
                let pipeline_cache = self.pipeline_cache.as_ref();
                let tx = tx;
                new_shaders
                    .into_iter()
                    .map(|it| {
                        let tx = tx.clone();
                        std::thread::Builder::new()
                            .name("Vello shader initialisation worker thread".into())
                            .spawn_scoped(scope, move || {
                                let shader = Self::create_compute_pipeline(
                                    device,
                                    it.label,
                                    it.wgsl,
                                    it.entries,
                                    pipeline_cache,
                                );
                                // We know the rx can only be closed if all the tx references are dropped
                                tx.send((it.shader_id, shader)).unwrap();
                                while let Ok(mut guard) = work_queue.lock() {
                                    if let Some(value) = guard.next() {
                                        drop(guard);
                                        let shader = Self::create_compute_pipeline(
                                            device,
                                            value.label,
                                            value.wgsl,
                                            value.entries,
                                            pipeline_cache,
                                        );
                                        tx.send((value.shader_id, shader)).unwrap();
                                    } else {
                                        break;
                                    }
                                }
                                // Another thread panicked or we finished.
                                // If another thread panicked, we ignore that here and finish our processing
                                drop(tx);
                            })
                            .expect("failed to spawn thread");
                    })
                    .for_each(drop);
                // Drop the initial sender, to mean that there will be no more senders if and only if all other threads have finished
                drop(tx);

                while let Ok((id, value)) = rx.recv() {
                    self.shaders[id.0].wgpu = Some(value);
                }
            });
        }
    }

    /// Add a shader.
    ///
    /// This function is somewhat limited, it doesn't apply a label, only allows one bind group,
    /// doesn't support push constants, and entry point is hardcoded as "main".
    ///
    /// Maybe should do template instantiation here? But shader compilation pipeline feels maybe
    /// a bit separate.
    pub fn add_compute_shader(
        &mut self,
        device: &Device,
        label: &'static str,
        wgsl: Cow<'static, str>,
        layout: &[BindType],
        cpu_shader: CpuShaderType,
    ) -> ShaderId {
        let mut add = |shader| {
            let id = self.shaders.len();
            self.shaders.push(shader);
            ShaderId(id)
        };

        if self.use_cpu {
            match cpu_shader {
                CpuShaderType::Present(shader) => {
                    return add(Shader {
                        wgpu: None,
                        cpu: Some(CpuShader { shader }),
                        label,
                    });
                }
                // This shader is unused in CPU mode, create a dummy shader
                CpuShaderType::Skipped => {
                    return add(Shader {
                        wgpu: None,
                        cpu: None,
                        label,
                    });
                }
                // Create a GPU shader as we don't have a CPU shader
                CpuShaderType::Missing => {}
            }
        }

        let entries = Self::create_bind_group_layout_entries(
            layout.iter().map(|b| (*b, wgpu::ShaderStages::COMPUTE)),
        );
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(uninit) = self.shaders_to_initialise.as_mut() {
            let id = add(Shader {
                label,
                wgpu: None,
                cpu: None,
            });
            uninit.push(UninitialisedShader {
                wgsl,
                label,
                entries,
                shader_id: id,
            });
            return id;
        }
        let wgpu = Self::create_compute_pipeline(
            device,
            label,
            wgsl,
            entries,
            self.pipeline_cache.as_ref(),
        );
        add(Shader {
            wgpu: Some(wgpu),
            cpu: None,
            label,
        })
    }

    #[cfg(feature = "debug_layers")]
    pub fn add_render_shader(
        &mut self,
        device: &Device,
        label: &'static str,
        module: &wgpu::ShaderModule,
        vertex_main: &'static str,
        fragment_main: &'static str,
        topology: wgpu::PrimitiveTopology,
        color_attachment: wgpu::ColorTargetState,
        vertex_buffer: Option<wgpu::VertexBufferLayout<'_>>,
        bind_layout: &[(BindType, wgpu::ShaderStages)],
    ) -> ShaderId {
        let entries = Self::create_bind_group_layout_entries(bind_layout.iter().copied());
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some(vertex_main),
                buffers: &[vertex_buffer],
                compilation_options: PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some(fragment_main),
                targets: &[Some(color_attachment)],
                compilation_options: PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: self.pipeline_cache.as_ref(),
        });
        let id = self.shaders.len();
        self.shaders.push(Shader {
            wgpu: Some(WgpuShader {
                pipeline: PipelineState::Render(pipeline),
                bind_group_layout,
            }),
            cpu: None,
            label,
        });
        ShaderId(id)
    }

    pub fn run_recording(
        &mut self,
        device: &Device,
        queue: &Queue,
        recording: &Recording,
        external_resources: &[ExternalResource<'_>],
        label: &'static str,
        #[cfg(feature = "wgpu-profiler")] profiler: &mut wgpu_profiler::GpuProfiler,
    ) -> Result<()> {
        let mut free_bufs: HashSet<ResourceId> = HashSet::default();
        let mut free_images: HashSet<ResourceId> = HashSet::default();
        let mut transient_map = TransientBindMap::new(external_resources);

        if !self.timer_tried {
            self.timer_tried = true;
            self.timer = Timer::new(device, queue);
        }
        let mut encoder =
            device.create_command_encoder(&CommandEncoderDescriptor { label: Some(label) });
        #[cfg(feature = "wgpu-profiler")]
        let query = profiler.begin_query(label, &mut encoder);
        // Ligma: dispatches share a compute pass until something else needs
        // the encoder, wgpu putting the barriers each needs between them as
        // it does between passes, as `finish` makes and closes two D3D12
        // command lists for each pass. Timed, each has a pass of its own
        let merge = self.timer.is_none() && cfg!(not(feature = "wgpu-profiler"));
        let mut pass: Option<wgpu::ComputePass<'static>> = None;
        let recordings = self.pool.recordings;
        let mut lap = std::time::Instant::now();
        for command in &recording.commands {
            let part = match command {
                Command::Upload(..) | Command::UploadUniform(..) => 0,
                Command::UploadImage(..) | Command::WriteImage(..) => 1,
                Command::Dispatch(..) | Command::DispatchIndirect(..) => 2,
                _ => 3,
            };
            let _part = Part(part, &mut lap);
            match command {
                Command::Upload(buf_proxy, bytes) => {
                    transient_map
                        .bufs
                        .insert(buf_proxy.id, TransientBuf::Cpu(bytes));
                    // TODO: restrict VERTEX usage to "debug_layers" feature?
                    let usage = BufferUsages::COPY_SRC
                        | BufferUsages::COPY_DST
                        | BufferUsages::STORAGE
                        | BufferUsages::VERTEX;
                    let buf = self
                        .pool
                        .get_buf(buf_proxy.size, buf_proxy.name, usage, device);
                    if Staging::takes(bytes) {
                        pass = None;
                    }
                    if !self.staging.upload(device, &mut encoder, &buf, bytes) {
                        queue.write_buffer(&buf, 0, bytes);
                    }
                    self.bind_map.insert_buf(buf_proxy, buf);
                }
                Command::UploadUniform(buf_proxy, bytes) => {
                    transient_map
                        .bufs
                        .insert(buf_proxy.id, TransientBuf::Cpu(bytes));
                    let usage = BufferUsages::UNIFORM | BufferUsages::COPY_DST;
                    // Same consideration as above
                    let buf = self
                        .pool
                        .get_buf(buf_proxy.size, buf_proxy.name, usage, device);
                    queue.write_buffer(&buf, 0, bytes);
                    self.bind_map.insert_buf(buf_proxy, buf);
                }
                Command::UploadImage(image_proxy, bytes) => {
                    let format = image_proxy.format.to_wgpu();
                    let block_size = format
                        .block_copy_size(None)
                        .expect("ImageFormat must have a valid block size");
                    let texture = device.create_texture(&wgpu::TextureDescriptor {
                        label: None,
                        size: wgpu::Extent3d {
                            width: image_proxy.width,
                            height: image_proxy.height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                        format,
                        view_formats: &[],
                    });
                    let texture_view = texture.create_view(&wgpu::TextureViewDescriptor {
                        label: None,
                        dimension: Some(TextureViewDimension::D2),
                        usage: None,
                        aspect: TextureAspect::All,
                        mip_level_count: None,
                        base_mip_level: 0,
                        base_array_layer: 0,
                        array_layer_count: None,
                        format: Some(format),
                    });
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d { x: 0, y: 0, z: 0 },
                            aspect: TextureAspect::All,
                        },
                        bytes,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(image_proxy.width * block_size),
                            rows_per_image: None,
                        },
                        wgpu::Extent3d {
                            width: image_proxy.width,
                            height: image_proxy.height,
                            depth_or_array_layers: 1,
                        },
                    );
                    self.bind_map
                        .insert_image(image_proxy.id, texture, texture_view);
                }
                Command::WriteImage(proxy, [x, y], image) => {
                    let (texture, _) = self.bind_map.get_or_create_image(*proxy, device);
                    let format = proxy.format.to_wgpu();
                    let block_size = format
                        .block_copy_size(None)
                        .expect("ImageFormat must have a valid block size");
                    if let Some(overrider) = self.image_overrides.get(&image.data.id()) {
                        pass = None;
                        encoder.copy_texture_to_texture(
                            wgpu::TexelCopyTextureInfo {
                                texture: &overrider.texture,
                                mip_level: overrider.mip_level,
                                origin: overrider.origin,
                                aspect: overrider.aspect,
                            },
                            wgpu::TexelCopyTextureInfo {
                                texture,
                                mip_level: 0,
                                origin: wgpu::Origin3d { x: *x, y: *y, z: 0 },
                                aspect: TextureAspect::All,
                            },
                            wgpu::Extent3d {
                                width: image.width,
                                height: image.height,
                                depth_or_array_layers: 1,
                            },
                        );
                    } else {
                        if image.data.is_empty() && image.width != 0 && image.height != 0 {
                            panic!(
                                "Tried to draw an invalid empty image (id: {}). \
                                Maybe it was registered to a different renderer, or \
                                unregistered before this render was submitted.",
                                image.data.id()
                            );
                        }
                        queue.write_texture(
                            wgpu::TexelCopyTextureInfo {
                                texture,
                                mip_level: 0,
                                origin: wgpu::Origin3d { x: *x, y: *y, z: 0 },
                                aspect: TextureAspect::All,
                            },
                            image.data.data(),
                            wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(image.width * block_size),
                                rows_per_image: None,
                            },
                            wgpu::Extent3d {
                                width: image.width,
                                height: image.height,
                                depth_or_array_layers: 1,
                            },
                        );
                    }
                }
                Command::Dispatch(shader_id, wg_size, bindings) => {
                    let (x, y, z) = *wg_size;
                    // println!("dispatching {:?} with {} bindings", wg_size, bindings.len());
                    let shader = &self.shaders[shader_id.0];
                    match shader.select() {
                        ShaderKind::Cpu(cpu_shader) => {
                            // The current strategy is to run the CPU shader synchronously. This
                            // works because there is currently the added constraint that data
                            // can only flow from CPU to GPU, not the other way around. If and
                            // when we implement that, we will need to defer the execution. Of
                            // course, we will also need to wire up more async synchronization
                            // mechanisms, as the CPU dispatch can't run until the preceding
                            // command buffer submission completes (and, in WebGPU, the async
                            // mapping operations on the buffers completes).
                            let resources =
                                transient_map.create_cpu_resources(&mut self.bind_map, bindings);
                            (cpu_shader.shader)(x, &resources);
                        }
                        ShaderKind::Wgpu(wgpu_shader) => {
                            // Workaround for https://github.com/linebender/vello/issues/637
                            if x == 0 || y == 0 || z == 0 {
                                continue;
                            }
                            if BindMap::clears(&self.bind_map, bindings) {
                                pass = None;
                            }
                            let bind_group = transient_map.create_bind_group(
                                &mut self.bind_map,
                                &mut self.pool,
                                device,
                                queue,
                                &mut encoder,
                                &wgpu_shader.bind_group_layout,
                                bindings,
                                Some((&mut self.bind_groups, shader_id.0, recordings)),
                            );
                            if pass.is_none() {
                                pass = Some(match &mut self.timer {
                                    Some(timer) => encoder.begin_compute_pass(&timer.pass(shader.label)),
                                    None => encoder.begin_compute_pass(&ComputePassDescriptor::default()),
                                }.forget_lifetime());
                            }
                            let cpass = pass.as_mut().unwrap();
                            #[cfg(feature = "wgpu-profiler")]
                            let query = profiler
                                .begin_query(shader.label, cpass)
                                .with_parent(Some(&query));
                            #[cfg_attr(
                                not(feature = "debug_layers"),
                                expect(
                                    irrefutable_let_patterns,
                                    reason = "Render shaders are only enabled if we have the debug pipeline"
                                )
                            )]
                            let PipelineState::Compute(pipeline) = &wgpu_shader.pipeline else {
                                panic!("cannot issue a dispatch with a render pipeline");
                            };
                            cpass.set_pipeline(pipeline);
                            cpass.set_bind_group(0, &bind_group, &[]);
                            cpass.dispatch_workgroups(x, y, z);
                            #[cfg(feature = "wgpu-profiler")]
                            profiler.end_query(cpass, query);
                            if !merge {
                                pass = None;
                            }
                        }
                    }
                }
                Command::DispatchIndirect(shader_id, proxy, offset, bindings) => {
                    let shader = &self.shaders[shader_id.0];
                    match shader.select() {
                        ShaderKind::Cpu(cpu_shader) => {
                            // Same consideration as above about running the CPU shader synchronously.
                            let n_wg;
                            if let CpuBinding::BufferRW(b) = self.bind_map.get_cpu_buf(proxy.id) {
                                let slice = b.borrow();
                                let indirect: &[u32] = bytemuck::cast_slice(&slice);
                                n_wg = indirect[0];
                            } else {
                                panic!("indirect buffer missing from bind map");
                            }
                            let resources =
                                transient_map.create_cpu_resources(&mut self.bind_map, bindings);
                            (cpu_shader.shader)(n_wg, &resources);
                        }
                        ShaderKind::Wgpu(wgpu_shader) => {
                            if BindMap::clears(&self.bind_map, bindings) {
                                pass = None;
                            }
                            let bind_group = transient_map.create_bind_group(
                                &mut self.bind_map,
                                &mut self.pool,
                                device,
                                queue,
                                &mut encoder,
                                &wgpu_shader.bind_group_layout,
                                bindings,
                                Some((&mut self.bind_groups, shader_id.0, recordings)),
                            );
                            transient_map.materialize_gpu_buf_for_indirect(
                                &mut self.bind_map,
                                &mut self.pool,
                                device,
                                queue,
                                proxy,
                            );
                            if pass.is_none() {
                                pass = Some(match &mut self.timer {
                                    Some(timer) => encoder.begin_compute_pass(&timer.pass(shader.label)),
                                    None => encoder.begin_compute_pass(&ComputePassDescriptor::default()),
                                }.forget_lifetime());
                            }
                            let cpass = pass.as_mut().unwrap();
                            #[cfg(feature = "wgpu-profiler")]
                            let query = profiler
                                .begin_query(shader.label, cpass)
                                .with_parent(Some(&query));
                            #[cfg_attr(
                                not(feature = "debug_layers"),
                                expect(
                                    irrefutable_let_patterns,
                                    reason = "Render shaders are only enabled if we have the debug pipeline"
                                )
                            )]
                            let PipelineState::Compute(pipeline) = &wgpu_shader.pipeline else {
                                panic!("cannot issue a dispatch with a render pipeline");
                            };
                            cpass.set_pipeline(pipeline);
                            cpass.set_bind_group(0, &bind_group, &[]);
                            let buf = self.bind_map.get_gpu_buf(proxy.id).ok_or(
                                Error::UnavailableBufferUsed(proxy.name, "indirect dispatch"),
                            )?;
                            cpass.dispatch_workgroups_indirect(buf, *offset);
                            #[cfg(feature = "wgpu-profiler")]
                            profiler.end_query(cpass, query);
                            if !merge {
                                pass = None;
                            }
                        }
                    }
                }
                #[cfg(feature = "debug_layers")]
                Command::Draw(draw_params) => {
                    let shader = &self.shaders[draw_params.shader_id.0];
                    #[cfg(feature = "wgpu-profiler")]
                    let label = shader.label;
                    let ShaderKind::Wgpu(shader) = shader.select() else {
                        panic!("a render pass does not have a CPU equivalent");
                    };
                    pass = None;
                    let bind_group = transient_map.create_bind_group(
                        &mut self.bind_map,
                        &mut self.pool,
                        device,
                        queue,
                        &mut encoder,
                        &shader.bind_group_layout,
                        &draw_params.resources,
                        None,
                    );
                    let render_target = transient_map
                        .materialize_external_image_for_render_pass(&draw_params.target);
                    let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: render_target,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: match draw_params.clear_color {
                                    Some(c) => wgpu::LoadOp::Clear(wgpu::Color {
                                        r: c[0] as f64,
                                        g: c[1] as f64,
                                        b: c[2] as f64,
                                        a: c[3] as f64,
                                    }),
                                    None => wgpu::LoadOp::Load,
                                },
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        occlusion_query_set: None,
                        timestamp_writes: None,
                        multiview_mask: None,
                    });
                    #[cfg(feature = "wgpu-profiler")]
                    let query = profiler
                        .begin_query(label, &mut rpass)
                        .with_parent(Some(&query));
                    let PipelineState::Render(pipeline) = &shader.pipeline else {
                        panic!("cannot issue a draw with a compute pipeline");
                    };
                    rpass.set_pipeline(pipeline);
                    if let Some(proxy) = draw_params.vertex_buffer {
                        // TODO: need a way to materialize a CPU initialized buffer. For now assume
                        // buffer exists? Also, need to materialize this buffer with vertex usage
                        let buf = self
                            .bind_map
                            .get_gpu_buf(proxy.id)
                            .ok_or(Error::UnavailableBufferUsed(proxy.name, "draw"))?;
                        rpass.set_vertex_buffer(0, buf.slice(..));
                    }
                    rpass.set_bind_group(0, &bind_group, &[]);
                    rpass.draw(0..draw_params.vertex_count, 0..draw_params.instance_count);
                    #[cfg(feature = "wgpu-profiler")]
                    profiler.end_query(&mut rpass, query);
                }
                Command::Download(proxy) => {
                    let src_buf = self
                        .bind_map
                        .get_gpu_buf(proxy.id)
                        .ok_or(Error::UnavailableBufferUsed(proxy.name, "download"))?;
                    let usage = BufferUsages::MAP_READ | BufferUsages::COPY_DST;
                    let buf = self.pool.get_buf(proxy.size, "download", usage, device);
                    pass = None;
                    encoder.copy_buffer_to_buffer(src_buf, 0, &buf, 0, proxy.size);
                    self.downloads.insert(proxy.id, buf);
                }
                Command::Clear(proxy, offset, size) => {
                    if let Some(buf) = self.bind_map.get_buf(*proxy) {
                        match &buf.buffer {
                            MaterializedBuffer::Gpu(b) => {
                                pass = None;
                                encoder.clear_buffer(b, *offset, *size);
                            }
                            MaterializedBuffer::Cpu(b) => {
                                let mut slice = &mut b.borrow_mut()[*offset as usize..];
                                if let Some(size) = size {
                                    slice = &mut slice[..*size as usize];
                                }
                                slice.fill(0);
                            }
                        }
                    } else {
                        self.bind_map.pending_clears.insert(proxy.id);
                    }
                }
                Command::FreeBuffer(proxy) => {
                    free_bufs.insert(proxy.id);
                }
                Command::FreeImage(proxy) => {
                    free_images.insert(proxy.id);
                }
            }
        }
        drop(pass);
        #[cfg(feature = "wgpu-profiler")]
        profiler.end_query(&mut encoder, query);
        // TODO: This only actually needs to happen once per frame, but run_recording happens two or three times
        #[cfg(feature = "wgpu-profiler")]
        profiler.resolve_queries(&mut encoder);
        if let Some(timer) = &mut self.timer {
            timer.resolve(device, &mut encoder);
        }
        let _part = Part(4, &mut lap);
        self.submitted = Some(queue.submit(Some(encoder.finish())));
        drop(_part);
        let _part = Part(5, &mut lap);
        self.staging.submitted();
        if let Some(timer) = &mut self.timer {
            timer.submitted();
        }
        let recordings = self.pool.recordings;
        for id in free_bufs {
            if let Some(buf) = self.bind_map.buf_map.remove(&id)
                && let MaterializedBuffer::Gpu(gpu_buf) = buf.buffer
            {
                let kind = (buf.label, gpu_buf.usage());
                let bufs = self.pool.bufs.entry(kind).or_default();
                bufs.push((gpu_buf, recordings));
                // The biggest of a kind alone: a render takes each kind once,
                // and a smaller one is left over from before it took more,
                // which the biggest serves as well
                if bufs.len() > 1 {
                    bufs.sort_by_key(|(buf, _)| std::cmp::Reverse(buf.size()));
                    for (buf, _) in bufs.drain(1..) {
                        self.bind_groups.forget(&buf);
                        buf.destroy();
                    }
                }
            }
        }
        self.pool.recordings += 1;
        let recordings = self.pool.recordings;
        // Those not used lately, or much bigger than any asked for lately, go
        let most = &self.pool.most;
        for (kind, bufs) in &mut self.pool.bufs {
            let most = most.get(kind).filter(|(_, at)| at + POOL_KEPT > recordings).map_or(0, |(size, _)| *size);
            bufs.retain(|(buf, at)| {
                let keep = at + POOL_KEPT > recordings && buf.size() <= most.saturating_mul(2);
                if !keep {
                    self.bind_groups.forget(buf);
                    buf.destroy();
                }
                keep
            });
        }
        self.pool.bufs.retain(|_, bufs| !bufs.is_empty());
        self.pool.most.retain(|_, (_, at)| *at + POOL_KEPT > recordings);
        self.bind_groups.kept.retain(|_, (_, at)| *at + POOL_KEPT > recordings);
        for id in free_images {
            if let Some((texture, _view)) = self.bind_map.image_map.remove(&id) {
                // TODO: have a pool to avoid needless re-allocation
                texture.destroy();
            }
        }
        Ok(())
    }

    pub fn get_download(&self, buf: BufferProxy) -> Option<&Buffer> {
        self.downloads.get(&buf.id)
    }

    pub fn free_download(&mut self, buf: BufferProxy) {
        self.downloads.remove(&buf.id);
    }

    fn create_bind_group_layout_entries(
        layout: impl Iterator<Item = (BindType, wgpu::ShaderStages)>,
    ) -> Vec<wgpu::BindGroupLayoutEntry> {
        layout
            .enumerate()
            .map(|(i, (bind_type, visibility))| match bind_type {
                BindType::Buffer | BindType::BufReadOnly => wgpu::BindGroupLayoutEntry {
                    binding: i as u32,
                    visibility,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage {
                            read_only: bind_type == BindType::BufReadOnly,
                        },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                BindType::Uniform => wgpu::BindGroupLayoutEntry {
                    binding: i as u32,
                    visibility,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                BindType::Image(format) | BindType::ImageRead(format) => {
                    wgpu::BindGroupLayoutEntry {
                        binding: i as u32,
                        visibility,
                        ty: if bind_type == BindType::ImageRead(format) {
                            wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: TextureViewDimension::D2,
                                multisampled: false,
                            }
                        } else {
                            wgpu::BindingType::StorageTexture {
                                access: wgpu::StorageTextureAccess::WriteOnly,
                                format: format.to_wgpu(),
                                view_dimension: TextureViewDimension::D2,
                            }
                        },
                        count: None,
                    }
                }
            })
            .collect::<Vec<_>>()
    }

    fn create_compute_pipeline(
        device: &Device,
        label: &str,
        wgsl: Cow<'_, str>,
        entries: Vec<wgpu::BindGroupLayoutEntry>,
        cache: Option<&PipelineCache>,
    ) -> WgpuShader {
        // SAFETY: We only call this with trusted shaders (written by Vello developers)
        let shader_module = unsafe {
            device.create_shader_module_trusted(
                wgpu::ShaderModuleDescriptor {
                    label: Some(label),
                    source: wgpu::ShaderSource::Wgsl(wgsl),
                },
                wgpu::ShaderRuntimeChecks::unchecked(),
            )
        };
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &entries,
        });
        let compute_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(&bind_group_layout)],
                immediate_size: 0,
            });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(label),
            layout: Some(&compute_pipeline_layout),
            module: &shader_module,
            entry_point: None,
            compilation_options: PipelineCompilationOptions {
                zero_initialize_workgroup_memory: false,
                ..Default::default()
            },
            cache,
        });
        WgpuShader {
            pipeline: PipelineState::Compute(pipeline),
            bind_group_layout,
        }
    }
}

impl BindMap {
    /// Ligma: whether binding `bindings` clears a buffer with the encoder
    fn clears(&self, bindings: &[ResourceProxy]) -> bool {
        !self.pending_clears.is_empty()
            && bindings.iter().any(|binding| match binding {
                ResourceProxy::Buffer(proxy) | ResourceProxy::BufferRange { proxy, .. } => self.pending_clears.contains(&proxy.id),
                ResourceProxy::Image(_) => false,
            })
    }

    fn insert_buf(&mut self, proxy: &BufferProxy, buffer: Buffer) {
        self.buf_map.insert(
            proxy.id,
            BindMapBuffer {
                buffer: MaterializedBuffer::Gpu(buffer),
                label: proxy.name,
            },
        );
    }

    /// Get a buffer, only if it's on GPU.
    fn get_gpu_buf(&self, id: ResourceId) -> Option<&Buffer> {
        self.buf_map.get(&id).and_then(|b| match &b.buffer {
            MaterializedBuffer::Gpu(b) => Some(b),
            _ => None,
        })
    }

    /// Get a CPU buffer.
    ///
    /// Panics if buffer is not present or is on GPU.
    fn get_cpu_buf(&self, id: ResourceId) -> CpuBinding<'_> {
        match &self.buf_map[&id].buffer {
            MaterializedBuffer::Cpu(b) => CpuBinding::BufferRW(b),
            _ => panic!("getting cpu buffer, but it's on gpu"),
        }
    }

    fn materialize_cpu_buf(&mut self, buf: &BufferProxy) {
        self.buf_map.entry(buf.id).or_insert_with(|| {
            let buffer = MaterializedBuffer::Cpu(RefCell::new(vec![0; buf.size as usize]));
            BindMapBuffer {
                buffer,
                // TODO: do we need to cfg this?
                label: buf.name,
            }
        });
    }

    fn insert_image(&mut self, id: ResourceId, image: Texture, image_view: TextureView) {
        self.image_map.insert(id, (image, image_view));
    }

    fn get_buf(&mut self, proxy: BufferProxy) -> Option<&BindMapBuffer> {
        self.buf_map.get(&proxy.id)
    }

    fn get_or_create_image(
        &mut self,
        proxy: ImageProxy,
        device: &Device,
    ) -> &(Texture, TextureView) {
        match self.image_map.entry(proxy.id) {
            Entry::Occupied(occupied) => occupied.into_mut(),
            Entry::Vacant(vacant) => {
                let format = proxy.format.to_wgpu();
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: proxy.width,
                        height: proxy.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                    format,
                    view_formats: &[],
                });
                let texture_view = texture.create_view(&wgpu::TextureViewDescriptor {
                    label: None,
                    usage: None,
                    dimension: Some(TextureViewDimension::D2),
                    aspect: TextureAspect::All,
                    mip_level_count: None,
                    base_mip_level: 0,
                    base_array_layer: 0,
                    array_layer_count: None,
                    format: Some(proxy.format.to_wgpu()),
                });
                vacant.insert((texture, texture_view))
            }
        }
    }
}

const SIZE_CLASS_BITS: u32 = 2;

impl ResourcePool {
    /// Get a buffer from the pool or create one.
    fn get_buf(
        &mut self,
        size: u64,
        name: &'static str,
        usage: BufferUsages,
        device: &Device,
    ) -> Buffer {
        let rounded_size = Self::size_class(size, SIZE_CLASS_BITS);
        let kind = (name, usage);
        let recordings = self.recordings;
        let most = self.most.entry(kind).or_insert((0, recordings));
        if rounded_size >= most.0 || most.1 + POOL_KEPT <= recordings {
            *most = (rounded_size, recordings);
        }
        if let Some(bufs) = self.bufs.get_mut(&kind) {
            // The smallest that's big enough
            let fits = bufs.iter().enumerate().filter(|(_, (buf, _))| buf.size() >= rounded_size).min_by_key(|(_, (buf, _))| buf.size()).map(|(index, _)| index);
            if let Some(index) = fits {
                return bufs.swap_remove(index).0;
            }
        }
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(name),
            size: rounded_size,
            usage,
            mapped_at_creation: false,
        })
    }

    /// Quantize a size up to the nearest size class.
    fn size_class(x: u64, bits: u32) -> u64 {
        if x > 1 << bits {
            let a = (x - 1).leading_zeros();
            let b = (x - 1) | (((u64::MAX / 2) >> bits) >> a);
            b + 1
        } else {
            1 << bits
        }
    }
}

impl BindMapBuffer {
    // Upload a buffer from CPU to GPU if needed.
    //
    // Note data flow is one way only, from CPU to GPU. Once this method is
    // called, the buffer is no longer materialized on CPU, and cannot be
    // accessed from a CPU shader.
    fn upload_if_needed(
        &mut self,
        proxy: &BufferProxy,
        device: &Device,
        queue: &Queue,
        pool: &mut ResourcePool,
    ) {
        if let MaterializedBuffer::Cpu(cpu_buf) = &self.buffer {
            let usage = BufferUsages::COPY_SRC
                | BufferUsages::COPY_DST
                | BufferUsages::STORAGE
                | BufferUsages::INDIRECT;
            let buf = pool.get_buf(proxy.size, proxy.name, usage, device);
            queue.write_buffer(&buf, 0, &cpu_buf.borrow());
            self.buffer = MaterializedBuffer::Gpu(buf);
        }
    }
}

impl<'a> TransientBindMap<'a> {
    /// Create new transient bind map, seeded from external resources
    fn new(external_resources: &'a [ExternalResource<'_>]) -> Self {
        let mut bufs = HashMap::default();
        let mut images = HashMap::default();
        for resource in external_resources {
            match resource {
                ExternalResource::Buffer(proxy, gpu_buf) => {
                    bufs.insert(proxy.id, TransientBuf::Gpu(gpu_buf));
                }
                ExternalResource::Image(proxy, gpu_image) => {
                    images.insert(proxy.id, *gpu_image);
                }
            }
        }
        TransientBindMap { bufs, images }
    }

    fn materialize_gpu_buf_for_indirect(
        &mut self,
        bind_map: &mut BindMap,
        pool: &mut ResourcePool,
        device: &Device,
        queue: &Queue,
        buf: &BufferProxy,
    ) {
        if !self.bufs.contains_key(&buf.id)
            && let Some(b) = bind_map.buf_map.get_mut(&buf.id)
        {
            b.upload_if_needed(buf, device, queue, pool);
        }
    }

    #[cfg(feature = "debug_layers")]
    fn materialize_external_image_for_render_pass(&mut self, proxy: &ImageProxy) -> &TextureView {
        // TODO: Maybe this should support instantiating a transient texture. Right now all render
        // passes target a `SurfaceTexture`, so supporting external textures is sufficient.
        self.images
            .get(&proxy.id)
            .expect("texture not materialized")
    }

    fn create_bind_group(
        &mut self,
        bind_map: &mut BindMap,
        pool: &mut ResourcePool,
        device: &Device,
        queue: &Queue,
        encoder: &mut CommandEncoder,
        layout: &BindGroupLayout,
        bindings: &[ResourceProxy],
        kept: Option<(&mut BindGroups, usize, u64)>,
    ) -> BindGroup {
        for proxy in bindings {
            match proxy {
                ResourceProxy::Buffer(proxy)
                | ResourceProxy::BufferRange {
                    proxy,
                    offset: _,
                    size: _,
                } => {
                    if self.bufs.contains_key(&proxy.id) {
                        continue;
                    }
                    match bind_map.buf_map.entry(proxy.id) {
                        Entry::Vacant(v) => {
                            // TODO: only some buffers will need indirect & vertex, but does it hurt?
                            let usage = BufferUsages::COPY_SRC
                                | BufferUsages::COPY_DST
                                | BufferUsages::STORAGE
                                | BufferUsages::INDIRECT
                                | BufferUsages::VERTEX;
                            let buf = pool.get_buf(proxy.size, proxy.name, usage, device);
                            if bind_map.pending_clears.remove(&proxy.id) {
                                encoder.clear_buffer(&buf, 0, None);
                            }
                            v.insert(BindMapBuffer {
                                buffer: MaterializedBuffer::Gpu(buf),
                                label: proxy.name,
                            });
                        }
                        Entry::Occupied(mut o) => {
                            o.get_mut().upload_if_needed(proxy, device, queue, pool);
                        }
                    }
                }
                ResourceProxy::Image(proxy) => {
                    if self.images.contains_key(&proxy.id) {
                        continue;
                    }
                    if let Entry::Vacant(v) = bind_map.image_map.entry(proxy.id) {
                        let format = proxy.format.to_wgpu();
                        let texture = device.create_texture(&wgpu::TextureDescriptor {
                            label: None,
                            size: wgpu::Extent3d {
                                width: proxy.width,
                                height: proxy.height,
                                depth_or_array_layers: 1,
                            },
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                            format,
                            view_formats: &[],
                        });
                        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor {
                            label: None,
                            usage: None,
                            dimension: Some(TextureViewDimension::D2),
                            aspect: TextureAspect::All,
                            mip_level_count: None,
                            base_mip_level: 0,
                            base_array_layer: 0,
                            array_layer_count: None,
                            format: Some(proxy.format.to_wgpu()),
                        });
                        v.insert((texture, texture_view));
                    }
                }
            }
        }
        // Ligma: keyed by the buffers bound, the shader's layout and what
        // they're bound as being the same each time it's used
        let mut kept = kept.and_then(|(kept, shader, recording)| {
            let bufs = bindings
                .iter()
                .map(|binding| {
                    let (proxy, offset, size) = match binding {
                        ResourceProxy::Buffer(proxy) => (proxy, 0, 0),
                        ResourceProxy::BufferRange { proxy, offset, size } => (proxy, *offset, *size),
                        ResourceProxy::Image(_) => return None,
                    };
                    match self.bufs.get(&proxy.id) {
                        Some(TransientBuf::Gpu(_)) => None,
                        _ => Some((bind_map.get_gpu_buf(proxy.id)?.clone(), offset, size)),
                    }
                })
                .collect::<Option<Vec<_>>>()?;
            Some((kept, (shader, bufs), recording))
        });
        if let Some((cache, key, recording)) = &mut kept
            && let Some((bind_group, at)) = cache.kept.get_mut(&*key)
        {
            *at = *recording;
            return bind_group.clone();
        }
        let entries = bindings
            .iter()
            .enumerate()
            .map(|(i, proxy)| match proxy {
                ResourceProxy::Buffer(proxy) => {
                    let buf = match self.bufs.get(&proxy.id) {
                        Some(TransientBuf::Gpu(b)) => b,
                        _ => bind_map.get_gpu_buf(proxy.id).unwrap(),
                    };
                    wgpu::BindGroupEntry {
                        binding: i as u32,
                        resource: buf.as_entire_binding(),
                    }
                }
                ResourceProxy::BufferRange {
                    proxy,
                    offset,
                    size,
                } => {
                    let buf = match self.bufs.get(&proxy.id) {
                        Some(TransientBuf::Gpu(b)) => b,
                        _ => bind_map.get_gpu_buf(proxy.id).unwrap(),
                    };
                    wgpu::BindGroupEntry {
                        binding: i as u32,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: buf,
                            offset: *offset,
                            size: core::num::NonZeroU64::new(*size),
                        }),
                    }
                }
                ResourceProxy::Image(proxy) => {
                    let view = self
                        .images
                        .get(&proxy.id)
                        .copied()
                        .or_else(|| bind_map.image_map.get(&proxy.id).map(|v| &v.1))
                        .unwrap();
                    wgpu::BindGroupEntry {
                        binding: i as u32,
                        resource: wgpu::BindingResource::TextureView(view),
                    }
                }
            })
            .collect::<Vec<_>>();
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout,
            entries: &entries,
        });
        if let Some((cache, key, recording)) = kept {
            if cache.kept.len() >= BIND_GROUPS_KEPT {
                cache.kept.clear();
            }
            cache.kept.insert(key, (bind_group.clone(), recording));
        }
        bind_group
    }

    fn create_cpu_resources(
        &self,
        bind_map: &'a mut BindMap,
        bindings: &[ResourceProxy],
    ) -> Vec<CpuBinding<'_>> {
        // First pass is mutable; create buffers as needed
        for resource in bindings {
            match resource {
                ResourceProxy::Buffer(proxy)
                | ResourceProxy::BufferRange {
                    proxy,
                    offset: _,
                    size: _,
                } => match self.bufs.get(&proxy.id) {
                    Some(TransientBuf::Cpu(_)) => (),
                    Some(TransientBuf::Gpu(_)) => panic!("buffer was already materialized on GPU"),
                    _ => bind_map.materialize_cpu_buf(proxy),
                },
                ResourceProxy::Image(_) => todo!(),
            }
        }
        // Second pass takes immutable references
        bindings
            .iter()
            .map(|resource| match resource {
                ResourceProxy::Buffer(buf) => match self.bufs.get(&buf.id) {
                    Some(TransientBuf::Cpu(b)) => CpuBinding::Buffer(b),
                    _ => bind_map.get_cpu_buf(buf.id),
                },
                ResourceProxy::BufferRange { .. } => todo!(),
                ResourceProxy::Image(_) => todo!(),
            })
            .collect()
    }
}
