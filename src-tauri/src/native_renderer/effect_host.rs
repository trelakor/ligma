//! Runs a file's custom effects natively: each is the file's own code, a
//! module with `setup` and `render` that draws with WebGPU. It runs here in
//! QuickJS, its WebGPU calls (see `effect_host.js`) made on Ligma's own GPU
//! device through wgpu, from the node's pixels as drawn natively to the
//! node's pixels with the effect on. Nothing comes from Figma's renderer
//!
//! Self-contained, so it can be tried on its own

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use rquickjs::{Context, Ctx, Exception, Function, Runtime, TypedArray, Value};
use serde_json::Value as Json;

const SHIM: &str = include_str!("effect_host.js");

/// Ligma's GPU objects for effects, by id
enum Res {
    Buffer(wgpu::Buffer),
    Texture(wgpu::Texture),
    View(wgpu::TextureView),
    Sampler(wgpu::Sampler),
    Module(wgpu::ShaderModule),
    Layout(wgpu::BindGroupLayout),
    PipelineLayout(wgpu::PipelineLayout),
    Render(wgpu::RenderPipeline),
    Compute(wgpu::ComputePipeline),
    Group(wgpu::BindGroup),
    Encoder(wgpu::CommandEncoder),
    Commands(wgpu::CommandBuffer),
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    objects: HashMap<u32, Res>,
    next: u32,
    /// The node each object was made running, let go once it has run
    owners: HashMap<u32, String>,
    owner: Option<String>,
    /// Validation errors since the effect last asked
    errors: Arc<Mutex<Vec<String>>>,
    scopes: Vec<usize>,
    logs: Vec<String>,
    /// Dev tools: time in each op this run
    ops: HashMap<String, (u32, std::time::Duration)>,
}

type Fail = String;

fn fail<T>(message: impl Into<String>) -> Result<T, Fail> {
    Err(message.into())
}

impl Gpu {
    fn add(&mut self, res: Res) -> u32 {
        self.next += 1;
        self.objects.insert(self.next, res);
        if let Some(owner) = &self.owner {
            self.owners.insert(self.next, owner.clone());
        }
        self.next
    }

    /// Lets go of everything made running node `node`
    fn release(&mut self, node: &str) {
        let gone: Vec<u32> = self.owners.iter().filter(|(_, owner)| owner.as_str() == node).map(|(&id, _)| id).collect();
        for id in gone {
            self.owners.remove(&id);
            match self.objects.remove(&id) {
                Some(Res::Texture(texture)) => texture.destroy(),
                Some(Res::Buffer(buffer)) => buffer.destroy(),
                _ => {}
            }
        }
    }

    fn get(&self, value: &Json) -> Result<&Res, Fail> {
        let id = value.get("$").and_then(Json::as_u64).ok_or("expected a GPU object")? as u32;
        self.objects.get(&id).ok_or_else(|| "a GPU object that's gone".into())
    }

    fn buffer(&self, value: &Json) -> Result<&wgpu::Buffer, Fail> {
        match self.get(value)? {
            Res::Buffer(buffer) => Ok(buffer),
            _ => fail("expected a buffer"),
        }
    }

    fn texture(&self, value: &Json) -> Result<&wgpu::Texture, Fail> {
        match self.get(value)? {
            Res::Texture(texture) => Ok(texture),
            _ => fail("expected a texture"),
        }
    }

    fn view(&self, value: &Json) -> Result<wgpu::TextureView, Fail> {
        match self.get(value)? {
            Res::View(view) => Ok(view.clone()),
            Res::Texture(texture) => Ok(texture.create_view(&Default::default())),
            _ => fail("expected a texture view"),
        }
    }

    fn module(&self, value: &Json) -> Result<&wgpu::ShaderModule, Fail> {
        match self.get(value)? {
            Res::Module(module) => Ok(module),
            _ => fail("expected a shader module"),
        }
    }

    fn layout(&self, value: &Json) -> Result<&wgpu::BindGroupLayout, Fail> {
        match self.get(value)? {
            Res::Layout(layout) => Ok(layout),
            _ => fail("expected a bind group layout"),
        }
    }

    fn call(&mut self, op: &str, args: Json, bytes: Option<&[u8]>) -> Result<Json, Fail> {
        let at = std::time::Instant::now();
        let result = self.call_inner(op, args, bytes);
        if super::render::tracing() {
            if !self.ops.contains_key(op) {
                self.ops.insert(op.to_string(), Default::default());
            }
            if let Some((count, took)) = self.ops.get_mut(op) {
                *count += 1;
                *took += at.elapsed();
            }
        }
        result
    }

    fn call_inner(&mut self, op: &str, args: Json, bytes: Option<&[u8]>) -> Result<Json, Fail> {
        if std::env::var_os("LIGMA_EFFECT_OPS").is_some() {
            eprintln!("{op} {}", args.to_string().chars().take(300).collect::<String>());
        }
        let arg = |index: usize| args.get(index).cloned().unwrap_or(Json::Null);
        match op {
            "log" => {
                let parts: Vec<String> = serde_json::from_value(args).unwrap_or_default();
                self.logs.push(parts.join(": "));
                Ok(Json::Null)
            }
            "limits" => Ok(limits_json(&self.device.limits())),
            "features" => {
                let mut names = Vec::new();
                if self.device.features().contains(wgpu::Features::FLOAT32_FILTERABLE) {
                    names.push("float32-filterable");
                }
                Ok(serde_json::json!(names))
            }
            "createBuffer" => {
                let size = num(&args["size"]).ok_or("buffer without a size")? as u64;
                // Written through the queue, never mapped
                let usage = wgpu::BufferUsages::from_bits_truncate(num(&args["usage"]).unwrap_or(0.0) as u32)
                    - wgpu::BufferUsages::MAP_READ
                    - wgpu::BufferUsages::MAP_WRITE
                    | wgpu::BufferUsages::COPY_DST;
                let buffer = self.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: size.div_ceil(4) * 4, usage, mapped_at_creation: false });
                Ok(self.add(Res::Buffer(buffer)).into())
            }
            "createTexture" => {
                let size = extent(&args["size"]);
                let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size,
                    mip_level_count: num(&args["mipLevelCount"]).unwrap_or(1.0) as u32,
                    sample_count: num(&args["sampleCount"]).unwrap_or(1.0) as u32,
                    dimension: match args["dimension"].as_str() {
                        Some("1d") => wgpu::TextureDimension::D1,
                        Some("3d") => wgpu::TextureDimension::D3,
                        _ => wgpu::TextureDimension::D2,
                    },
                    format: format(&args["format"])?,
                    usage: wgpu::TextureUsages::from_bits_truncate(num(&args["usage"]).unwrap_or(0.0) as u32),
                    view_formats: &[],
                });
                Ok(self.add(Res::Texture(texture)).into())
            }
            "createView" => {
                let desc = arg(1);
                let texture = self.texture(&arg(0))?;
                let view = texture.create_view(&wgpu::TextureViewDescriptor {
                    label: None,
                    format: desc.get("format").map(format).transpose()?,
                    dimension: desc.get("dimension").and_then(Json::as_str).map(view_dimension),
                    usage: None,
                    aspect: match desc["aspect"].as_str() {
                        Some("depth-only") => wgpu::TextureAspect::DepthOnly,
                        Some("stencil-only") => wgpu::TextureAspect::StencilOnly,
                        _ => wgpu::TextureAspect::All,
                    },
                    base_mip_level: num(&desc["baseMipLevel"]).unwrap_or(0.0) as u32,
                    mip_level_count: num(&desc["mipLevelCount"]).map(|count| count as u32),
                    base_array_layer: num(&desc["baseArrayLayer"]).unwrap_or(0.0) as u32,
                    array_layer_count: num(&desc["arrayLayerCount"]).map(|count| count as u32),
                });
                Ok(self.add(Res::View(view)).into())
            }
            "createSampler" => {
                let address = |key: &str| match args[key].as_str() {
                    Some("repeat") => wgpu::AddressMode::Repeat,
                    Some("mirror-repeat") => wgpu::AddressMode::MirrorRepeat,
                    _ => wgpu::AddressMode::ClampToEdge,
                };
                let filter = |key: &str| match args[key].as_str() {
                    Some("linear") => wgpu::FilterMode::Linear,
                    _ => wgpu::FilterMode::Nearest,
                };
                let mipmap = match args["mipmapFilter"].as_str() {
                    Some("linear") => wgpu::MipmapFilterMode::Linear,
                    _ => wgpu::MipmapFilterMode::Nearest,
                };
                let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
                    label: None,
                    address_mode_u: address("addressModeU"),
                    address_mode_v: address("addressModeV"),
                    address_mode_w: address("addressModeW"),
                    mag_filter: filter("magFilter"),
                    min_filter: filter("minFilter"),
                    mipmap_filter: mipmap,
                    lod_min_clamp: num(&args["lodMinClamp"]).unwrap_or(0.0) as f32,
                    lod_max_clamp: num(&args["lodMaxClamp"]).unwrap_or(32.0) as f32,
                    compare: args["compare"].as_str().map(compare),
                    anisotropy_clamp: num(&args["maxAnisotropy"]).unwrap_or(1.0) as u16,
                    border_color: None,
                });
                Ok(self.add(Res::Sampler(sampler)).into())
            }
            "createShaderModule" => {
                let code = args["code"].as_str().ok_or("shader without code")?;
                let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(code.into()) });
                Ok(self.add(Res::Module(module)).into())
            }
            "createBindGroupLayout" => {
                let entries = args["entries"].as_array().cloned().unwrap_or_default().iter().map(layout_entry).collect::<Result<Vec<_>, _>>()?;
                let layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &entries });
                Ok(self.add(Res::Layout(layout)).into())
            }
            "createPipelineLayout" => {
                let layouts = args["bindGroupLayouts"].as_array().cloned().unwrap_or_default();
                let layouts: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(|layout| self.layout(layout).map(Some)).collect::<Result<_, _>>()?;
                let layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &layouts, immediate_size: 0 });
                Ok(self.add(Res::PipelineLayout(layout)).into())
            }
            "createRenderPipeline" => {
                let pipeline = self.render_pipeline(&args)?;
                Ok(self.add(Res::Render(pipeline)).into())
            }
            "createComputePipeline" => {
                let layout = self.pipeline_layout(&args["layout"])?;
                let stage = &args["compute"];
                let constants = constants(&stage["constants"]);
                let constants: Vec<(&str, f64)> = constants.iter().map(|(name, value)| (name.as_str(), *value)).collect();
                let pipeline = self.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: None,
                    layout: layout.as_ref(),
                    module: self.module(&stage["module"])?,
                    entry_point: stage["entryPoint"].as_str(),
                    compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, zero_initialize_workgroup_memory: true },
                    cache: None,
                });
                Ok(self.add(Res::Compute(pipeline)).into())
            }
            "getBindGroupLayout" => {
                let index = num(&arg(1)).unwrap_or(0.0) as u32;
                let layout = match self.get(&arg(0))? {
                    Res::Render(pipeline) => pipeline.get_bind_group_layout(index),
                    Res::Compute(pipeline) => pipeline.get_bind_group_layout(index),
                    _ => return fail("expected a pipeline"),
                };
                Ok(self.add(Res::Layout(layout)).into())
            }
            "createBindGroup" => {
                let group = self.bind_group(&args)?;
                Ok(self.add(Res::Group(group)).into())
            }
            "createCommandEncoder" => {
                let encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                Ok(self.add(Res::Encoder(encoder)).into())
            }
            "renderPass" | "computePass" | "copyBufferToBuffer" | "copyBufferToTexture" | "copyTextureToBuffer" | "copyTextureToTexture" | "clearBuffer" => {
                let id = arg(0).get("$").and_then(Json::as_u64).ok_or("expected an encoder")? as u32;
                let Some(Res::Encoder(mut encoder)) = self.objects.remove(&id) else {
                    return fail("expected a command encoder");
                };
                let result = self.encode(op, &args, &mut encoder);
                self.objects.insert(id, Res::Encoder(encoder));
                result.map(|_| Json::Null)
            }
            "finish" => {
                let id = arg(0).get("$").and_then(Json::as_u64).ok_or("expected an encoder")? as u32;
                let Some(Res::Encoder(encoder)) = self.objects.remove(&id) else {
                    return fail("expected a command encoder");
                };
                Ok(self.add(Res::Commands(encoder.finish())).into())
            }
            "submit" => {
                let ids: Vec<u32> = arg(0).as_array().cloned().unwrap_or_default().iter().filter_map(|value| value.get("$").and_then(Json::as_u64)).map(|id| id as u32).collect();
                let mut buffers = Vec::new();
                for id in ids {
                    if let Some(Res::Commands(buffer)) = self.objects.remove(&id) {
                        buffers.push(buffer);
                    }
                }
                self.queue.submit(buffers);
                Ok(Json::Null)
            }
            "writeBuffer" => {
                let offset = num(&arg(1)).unwrap_or(0.0) as u64;
                let data = bytes.ok_or("no data")?;
                let buffer = self.buffer(&arg(0))?;
                // Writes go in steps of 4 bytes
                if data.len() % 4 == 0 {
                    self.queue.write_buffer(buffer, offset, data);
                } else {
                    let mut padded = data.to_vec();
                    padded.resize(data.len().div_ceil(4) * 4, 0);
                    self.queue.write_buffer(buffer, offset, &padded);
                }
                Ok(Json::Null)
            }
            "writeTexture" => {
                let data = bytes.ok_or("no data")?;
                let destination = arg(0);
                let layout = arg(1);
                let texture = self.texture(&destination["texture"])?;
                self.queue.write_texture(
                    copy_texture(texture, &destination),
                    data,
                    wgpu::TexelCopyBufferLayout {
                        offset: num(&layout["offset"]).unwrap_or(0.0) as u64,
                        bytes_per_row: num(&layout["bytesPerRow"]).map(|value| value as u32),
                        rows_per_image: num(&layout["rowsPerImage"]).map(|value| value as u32),
                    },
                    extent(&arg(2)),
                );
                Ok(Json::Null)
            }
            "drop" => {
                if let Some(id) = arg(0).get("$").and_then(Json::as_u64) {
                    match self.objects.remove(&(id as u32)) {
                        Some(Res::Texture(texture)) => texture.destroy(),
                        Some(Res::Buffer(buffer)) => buffer.destroy(),
                        _ => {}
                    }
                }
                Ok(Json::Null)
            }
            "pushErrorScope" => {
                self.scopes.push(self.errors.lock().unwrap().len());
                Ok(Json::Null)
            }
            "popErrorScope" => {
                let from = self.scopes.pop().unwrap_or(0);
                let errors = self.errors.lock().unwrap();
                Ok(errors.get(from).cloned().map(Json::String).unwrap_or(Json::Null))
            }
            _ => fail(format!("{op} isn't supported")),
        }
    }

    fn pipeline_layout(&self, value: &Json) -> Result<Option<wgpu::PipelineLayout>, Fail> {
        if value.is_string() || value.is_null() {
            return Ok(None);
        }
        match self.get(value)? {
            Res::PipelineLayout(layout) => Ok(Some(layout.clone())),
            _ => fail("expected a pipeline layout"),
        }
    }

    fn render_pipeline(&self, args: &Json) -> Result<wgpu::RenderPipeline, Fail> {
        let layout = self.pipeline_layout(&args["layout"])?;
        let vertex = &args["vertex"];
        let buffers: Vec<(u64, wgpu::VertexStepMode, Vec<wgpu::VertexAttribute>)> = vertex["buffers"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|buffer| {
                let attributes = buffer["attributes"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|attribute| {
                        Ok(wgpu::VertexAttribute {
                            format: vertex_format(attribute["format"].as_str().unwrap_or(""))?,
                            offset: num(&attribute["offset"]).unwrap_or(0.0) as u64,
                            shader_location: num(&attribute["shaderLocation"]).unwrap_or(0.0) as u32,
                        })
                    })
                    .collect::<Result<Vec<_>, Fail>>()?;
                let step = if buffer["stepMode"].as_str() == Some("instance") { wgpu::VertexStepMode::Instance } else { wgpu::VertexStepMode::Vertex };
                Ok((num(&buffer["arrayStride"]).unwrap_or(0.0) as u64, step, attributes))
            })
            .collect::<Result<_, Fail>>()?;
        let buffer_layouts: Vec<Option<wgpu::VertexBufferLayout>> =
            buffers.iter().map(|(stride, step, attributes)| Some(wgpu::VertexBufferLayout { array_stride: *stride, step_mode: *step, attributes })).collect();
        let vertex_constants = constants(&vertex["constants"]);
        let vertex_constants: Vec<(&str, f64)> = vertex_constants.iter().map(|(name, value)| (name.as_str(), *value)).collect();
        let fragment = &args["fragment"];
        let targets: Vec<Option<wgpu::ColorTargetState>> = fragment["targets"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|target| {
                if target.is_null() {
                    return Ok(None);
                }
                Ok(Some(wgpu::ColorTargetState {
                    format: format(&target["format"])?,
                    blend: target.get("blend").filter(|blend| !blend.is_null()).map(blend_state).transpose()?,
                    write_mask: wgpu::ColorWrites::from_bits_truncate(num(&target["writeMask"]).unwrap_or(15.0) as u32),
                }))
            })
            .collect::<Result<_, Fail>>()?;
        let fragment_constants = constants(&fragment["constants"]);
        let fragment_constants: Vec<(&str, f64)> = fragment_constants.iter().map(|(name, value)| (name.as_str(), *value)).collect();
        let primitive = &args["primitive"];
        let topology = match primitive["topology"].as_str() {
            Some("point-list") => wgpu::PrimitiveTopology::PointList,
            Some("line-list") => wgpu::PrimitiveTopology::LineList,
            Some("line-strip") => wgpu::PrimitiveTopology::LineStrip,
            Some("triangle-strip") => wgpu::PrimitiveTopology::TriangleStrip,
            _ => wgpu::PrimitiveTopology::TriangleList,
        };
        let depth_stencil = match args.get("depthStencil").filter(|value| !value.is_null()) {
            Some(depth) => Some(wgpu::DepthStencilState {
                format: format(&depth["format"])?,
                depth_write_enabled: depth["depthWriteEnabled"].as_bool(),
                depth_compare: depth["depthCompare"].as_str().map(compare),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            None => None,
        };
        let module = self.module(&vertex["module"])?;
        let fragment_module = if fragment.is_null() { None } else { Some(self.module(&fragment["module"])?) };
        Ok(self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: layout.as_ref(),
            vertex: wgpu::VertexState {
                module,
                entry_point: vertex["entryPoint"].as_str(),
                compilation_options: wgpu::PipelineCompilationOptions { constants: &vertex_constants, zero_initialize_workgroup_memory: true },
                buffers: &buffer_layouts,
            },
            primitive: wgpu::PrimitiveState {
                topology,
                strip_index_format: match primitive["stripIndexFormat"].as_str() {
                    Some("uint16") => Some(wgpu::IndexFormat::Uint16),
                    Some("uint32") => Some(wgpu::IndexFormat::Uint32),
                    _ => None,
                },
                front_face: if primitive["frontFace"].as_str() == Some("cw") { wgpu::FrontFace::Cw } else { wgpu::FrontFace::Ccw },
                cull_mode: match primitive["cullMode"].as_str() {
                    Some("front") => Some(wgpu::Face::Front),
                    Some("back") => Some(wgpu::Face::Back),
                    _ => None,
                },
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil,
            multisample: wgpu::MultisampleState {
                count: num(&args["multisample"]["count"]).unwrap_or(1.0) as u32,
                mask: num(&args["multisample"]["mask"]).map(|mask| mask as u64).unwrap_or(!0),
                alpha_to_coverage_enabled: args["multisample"]["alphaToCoverageEnabled"].as_bool().unwrap_or(false),
            },
            fragment: fragment_module.map(|module| wgpu::FragmentState {
                module,
                entry_point: fragment["entryPoint"].as_str(),
                compilation_options: wgpu::PipelineCompilationOptions { constants: &fragment_constants, zero_initialize_workgroup_memory: true },
                targets: &targets,
            }),
            multiview_mask: None,
            cache: None,
        }))
    }

    fn bind_group(&self, args: &Json) -> Result<wgpu::BindGroup, Fail> {
        let layout = self.layout(&args["layout"])?;
        enum Bound {
            Buffer(wgpu::Buffer, u64, Option<u64>),
            View(wgpu::TextureView),
            Sampler(wgpu::Sampler),
        }
        let mut bound = Vec::new();
        for entry in args["entries"].as_array().cloned().unwrap_or_default() {
            let binding = num(&entry["binding"]).unwrap_or(0.0) as u32;
            let resource = &entry["resource"];
            let thing = if resource.get("$").is_some() {
                match self.get(resource)? {
                    Res::Buffer(buffer) => Bound::Buffer(buffer.clone(), 0, None),
                    Res::Sampler(sampler) => Bound::Sampler(sampler.clone()),
                    Res::View(view) => Bound::View(view.clone()),
                    Res::Texture(texture) => Bound::View(texture.create_view(&Default::default())),
                    _ => return fail("can't bind that"),
                }
            } else if resource.get("buffer").is_some() {
                let buffer = self.buffer(&resource["buffer"])?;
                Bound::Buffer(buffer.clone(), num(&resource["offset"]).unwrap_or(0.0) as u64, num(&resource["size"]).map(|size| size as u64))
            } else {
                return fail("can't bind that");
            };
            bound.push((binding, thing));
        }
        let entries: Vec<wgpu::BindGroupEntry> = bound
            .iter()
            .map(|(binding, thing)| wgpu::BindGroupEntry {
                binding: *binding,
                resource: match thing {
                    Bound::Buffer(buffer, offset, size) => {
                        wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer, offset: *offset, size: size.and_then(wgpu::BufferSize::new) })
                    }
                    Bound::View(view) => wgpu::BindingResource::TextureView(view),
                    Bound::Sampler(sampler) => wgpu::BindingResource::Sampler(sampler),
                },
            })
            .collect();
        Ok(self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout, entries: &entries }))
    }

    fn encode(&self, op: &str, args: &Json, encoder: &mut wgpu::CommandEncoder) -> Result<(), Fail> {
        let arg = |index: usize| args.get(index).cloned().unwrap_or(Json::Null);
        match op {
            "renderPass" => {
                let desc = arg(1);
                let commands = arg(2).as_array().cloned().unwrap_or_default();
                let mut views = Vec::new();
                for attachment in desc["colorAttachments"].as_array().cloned().unwrap_or_default() {
                    if attachment.is_null() {
                        views.push(None);
                        continue;
                    }
                    let view = self.view(&attachment["view"])?;
                    let resolve = attachment.get("resolveTarget").filter(|value| !value.is_null()).map(|value| self.view(value)).transpose()?;
                    views.push(Some((view, resolve, attachment)));
                }
                let colors: Vec<Option<wgpu::RenderPassColorAttachment>> = views
                    .iter()
                    .map(|entry| {
                        entry.as_ref().map(|(view, resolve, attachment)| {
                            let clear = &attachment["clearValue"];
                            let channel = |key: &str, index: usize| clear.get(key).or_else(|| clear.get(index)).and_then(Json::as_f64).unwrap_or(0.0);
                            wgpu::RenderPassColorAttachment {
                                view,
                                depth_slice: None,
                                resolve_target: resolve.as_ref(),
                                ops: wgpu::Operations {
                                    load: if attachment["loadOp"].as_str() == Some("clear") {
                                        wgpu::LoadOp::Clear(wgpu::Color { r: channel("r", 0), g: channel("g", 1), b: channel("b", 2), a: channel("a", 3) })
                                    } else {
                                        wgpu::LoadOp::Load
                                    },
                                    store: if attachment["storeOp"].as_str() == Some("discard") { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store },
                                },
                            }
                        })
                    })
                    .collect();
                let depth_view = desc.get("depthStencilAttachment").filter(|value| !value.is_null()).map(|depth| self.view(&depth["view"])).transpose()?;
                let depth = depth_view.as_ref().map(|view| {
                    let attachment = &desc["depthStencilAttachment"];
                    wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: attachment.get("depthLoadOp").map(|_| wgpu::Operations {
                            load: if attachment["depthLoadOp"].as_str() == Some("clear") {
                                wgpu::LoadOp::Clear(num(&attachment["depthClearValue"]).unwrap_or(1.0) as f32)
                            } else {
                                wgpu::LoadOp::Load
                            },
                            store: if attachment["depthStoreOp"].as_str() == Some("discard") { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store },
                        }),
                        stencil_ops: None,
                    }
                });
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &colors,
                    depth_stencil_attachment: depth,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                for command in &commands {
                    let at = |index: usize| command.get(index).cloned().unwrap_or(Json::Null);
                    let int = |index: usize| num(&at(index)).unwrap_or(0.0);
                    match at(0).as_str().unwrap_or("") {
                        "setPipeline" => match self.get(&at(1))? {
                            Res::Render(pipeline) => pass.set_pipeline(pipeline),
                            _ => return fail("expected a render pipeline"),
                        },
                        "setBindGroup" => {
                            let offsets: Vec<u32> = at(3).as_array().cloned().unwrap_or_default().iter().filter_map(Json::as_f64).map(|offset| offset as u32).collect();
                            match self.get(&at(2)) {
                                Ok(Res::Group(group)) => pass.set_bind_group(int(1) as u32, group, &offsets),
                                _ => pass.set_bind_group(int(1) as u32, None, &[]),
                            }
                        }
                        "setVertexBuffer" => {
                            let buffer = self.buffer(&at(2))?;
                            let offset = int(3) as u64;
                            let slice = match num(&at(4)) {
                                Some(size) => buffer.slice(offset..offset + size as u64),
                                None => buffer.slice(offset..),
                            };
                            pass.set_vertex_buffer(int(1) as u32, slice);
                        }
                        "setIndexBuffer" => {
                            let buffer = self.buffer(&at(1))?;
                            let index = if at(2).as_str() == Some("uint32") { wgpu::IndexFormat::Uint32 } else { wgpu::IndexFormat::Uint16 };
                            let offset = int(3) as u64;
                            let slice = match num(&at(4)) {
                                Some(size) => buffer.slice(offset..offset + size as u64),
                                None => buffer.slice(offset..),
                            };
                            pass.set_index_buffer(slice, index);
                        }
                        "draw" => pass.draw(int(3) as u32..int(3) as u32 + int(1) as u32, int(4) as u32..int(4) as u32 + int(2) as u32),
                        "drawIndexed" => pass.draw_indexed(int(3) as u32..int(3) as u32 + int(1) as u32, int(4) as i32, int(5) as u32..int(5) as u32 + int(2) as u32),
                        "setViewport" => pass.set_viewport(int(1) as f32, int(2) as f32, int(3) as f32, int(4) as f32, int(5) as f32, int(6) as f32),
                        "setScissorRect" => pass.set_scissor_rect(int(1) as u32, int(2) as u32, int(3) as u32, int(4) as u32),
                        "setBlendConstant" => {
                            let color = at(1);
                            pass.set_blend_constant(wgpu::Color {
                                r: num(&color["r"]).unwrap_or(0.0),
                                g: num(&color["g"]).unwrap_or(0.0),
                                b: num(&color["b"]).unwrap_or(0.0),
                                a: num(&color["a"]).unwrap_or(0.0),
                            });
                        }
                        "setStencilReference" => pass.set_stencil_reference(int(1) as u32),
                        other => return fail(format!("{other} in a render pass")),
                    }
                }
                Ok(())
            }
            "computePass" => {
                let commands = arg(2).as_array().cloned().unwrap_or_default();
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
                for command in &commands {
                    let at = |index: usize| command.get(index).cloned().unwrap_or(Json::Null);
                    let int = |index: usize| num(&at(index)).unwrap_or(0.0);
                    match at(0).as_str().unwrap_or("") {
                        "setPipeline" => match self.get(&at(1))? {
                            Res::Compute(pipeline) => pass.set_pipeline(pipeline),
                            _ => return fail("expected a compute pipeline"),
                        },
                        "setBindGroup" => {
                            let offsets: Vec<u32> = at(3).as_array().cloned().unwrap_or_default().iter().filter_map(Json::as_f64).map(|offset| offset as u32).collect();
                            match self.get(&at(2)) {
                                Ok(Res::Group(group)) => pass.set_bind_group(int(1) as u32, group, &offsets),
                                _ => pass.set_bind_group(int(1) as u32, None, &[]),
                            }
                        }
                        "dispatchWorkgroups" => pass.dispatch_workgroups(int(1) as u32, int(2) as u32, int(3) as u32),
                        other => return fail(format!("{other} in a compute pass")),
                    }
                }
                Ok(())
            }
            "copyBufferToBuffer" => {
                let source = self.buffer(&arg(1))?;
                let destination = self.buffer(&arg(3))?;
                let size = num(&arg(5)).map(|size| size as u64);
                encoder.copy_buffer_to_buffer(source, num(&arg(2)).unwrap_or(0.0) as u64, destination, num(&arg(4)).unwrap_or(0.0) as u64, size);
                Ok(())
            }
            "copyBufferToTexture" => {
                let source = arg(1);
                let destination = arg(2);
                let buffer = self.buffer(&source["buffer"])?;
                let texture = self.texture(&destination["texture"])?;
                encoder.copy_buffer_to_texture(copy_buffer(buffer, &source), copy_texture(texture, &destination), extent(&arg(3)));
                Ok(())
            }
            "copyTextureToBuffer" => {
                let source = arg(1);
                let destination = arg(2);
                let texture = self.texture(&source["texture"])?;
                let buffer = self.buffer(&destination["buffer"])?;
                encoder.copy_texture_to_buffer(copy_texture(texture, &source), copy_buffer(buffer, &destination), extent(&arg(3)));
                Ok(())
            }
            "copyTextureToTexture" => {
                let source = arg(1);
                let destination = arg(2);
                let from = self.texture(&source["texture"])?;
                let to = self.texture(&destination["texture"])?;
                encoder.copy_texture_to_texture(copy_texture(from, &source), copy_texture(to, &destination), extent(&arg(3)));
                Ok(())
            }
            "clearBuffer" => {
                let buffer = self.buffer(&arg(1))?;
                encoder.clear_buffer(buffer, num(&arg(2)).unwrap_or(0.0) as u64, num(&arg(3)).map(|size| size as u64));
                Ok(())
            }
            _ => fail(format!("{op} isn't supported")),
        }
    }
}

fn num(value: &Json) -> Option<f64> {
    value.as_f64()
}

fn extent(value: &Json) -> wgpu::Extent3d {
    if let Some(array) = value.as_array() {
        let at = |index: usize, default: f64| array.get(index).and_then(Json::as_f64).unwrap_or(default) as u32;
        return wgpu::Extent3d { width: at(0, 1.0), height: at(1, 1.0), depth_or_array_layers: at(2, 1.0) };
    }
    wgpu::Extent3d {
        width: num(&value["width"]).unwrap_or(1.0) as u32,
        height: num(&value["height"]).unwrap_or(1.0) as u32,
        depth_or_array_layers: num(&value["depthOrArrayLayers"]).unwrap_or(1.0) as u32,
    }
}

fn origin(value: &Json) -> wgpu::Origin3d {
    if let Some(array) = value.as_array() {
        let at = |index: usize| array.get(index).and_then(Json::as_f64).unwrap_or(0.0) as u32;
        return wgpu::Origin3d { x: at(0), y: at(1), z: at(2) };
    }
    wgpu::Origin3d { x: num(&value["x"]).unwrap_or(0.0) as u32, y: num(&value["y"]).unwrap_or(0.0) as u32, z: num(&value["z"]).unwrap_or(0.0) as u32 }
}

fn copy_texture<'a>(texture: &'a wgpu::Texture, info: &Json) -> wgpu::TexelCopyTextureInfo<'a> {
    wgpu::TexelCopyTextureInfo { texture, mip_level: num(&info["mipLevel"]).unwrap_or(0.0) as u32, origin: origin(&info["origin"]), aspect: wgpu::TextureAspect::All }
}

fn copy_buffer<'a>(buffer: &'a wgpu::Buffer, info: &Json) -> wgpu::TexelCopyBufferInfo<'a> {
    wgpu::TexelCopyBufferInfo {
        buffer,
        layout: wgpu::TexelCopyBufferLayout {
            offset: num(&info["offset"]).unwrap_or(0.0) as u64,
            bytes_per_row: num(&info["bytesPerRow"]).map(|value| value as u32),
            rows_per_image: num(&info["rowsPerImage"]).map(|value| value as u32),
        },
    }
}

fn constants(value: &Json) -> Vec<(String, f64)> {
    value.as_object().map(|map| map.iter().filter_map(|(name, value)| Some((name.clone(), value.as_f64()?))).collect()).unwrap_or_default()
}

fn format(value: &Json) -> Result<wgpu::TextureFormat, Fail> {
    use wgpu::TextureFormat as F;
    Ok(match value.as_str().unwrap_or("") {
        "r8unorm" => F::R8Unorm,
        "r8snorm" => F::R8Snorm,
        "r8uint" => F::R8Uint,
        "r8sint" => F::R8Sint,
        "r16uint" => F::R16Uint,
        "r16sint" => F::R16Sint,
        "r16float" => F::R16Float,
        "rg8unorm" => F::Rg8Unorm,
        "rg8snorm" => F::Rg8Snorm,
        "rg8uint" => F::Rg8Uint,
        "rg8sint" => F::Rg8Sint,
        "r32uint" => F::R32Uint,
        "r32sint" => F::R32Sint,
        "r32float" => F::R32Float,
        "rg16uint" => F::Rg16Uint,
        "rg16sint" => F::Rg16Sint,
        "rg16float" => F::Rg16Float,
        "rgba8unorm" => F::Rgba8Unorm,
        "rgba8unorm-srgb" => F::Rgba8UnormSrgb,
        "rgba8snorm" => F::Rgba8Snorm,
        "rgba8uint" => F::Rgba8Uint,
        "rgba8sint" => F::Rgba8Sint,
        "bgra8unorm" => F::Bgra8Unorm,
        "bgra8unorm-srgb" => F::Bgra8UnormSrgb,
        "rgb10a2uint" => F::Rgb10a2Uint,
        "rgb10a2unorm" => F::Rgb10a2Unorm,
        "rg11b10ufloat" => F::Rg11b10Ufloat,
        "rg32uint" => F::Rg32Uint,
        "rg32sint" => F::Rg32Sint,
        "rg32float" => F::Rg32Float,
        "rgba16uint" => F::Rgba16Uint,
        "rgba16sint" => F::Rgba16Sint,
        "rgba16float" => F::Rgba16Float,
        "rgba32uint" => F::Rgba32Uint,
        "rgba32sint" => F::Rgba32Sint,
        "rgba32float" => F::Rgba32Float,
        "depth16unorm" => F::Depth16Unorm,
        "depth24plus" => F::Depth24Plus,
        "depth24plus-stencil8" => F::Depth24PlusStencil8,
        "depth32float" => F::Depth32Float,
        "stencil8" => F::Stencil8,
        other => return fail(format!("texture format {other}")),
    })
}

fn vertex_format(name: &str) -> Result<wgpu::VertexFormat, Fail> {
    use wgpu::VertexFormat as V;
    Ok(match name {
        "uint8x2" => V::Uint8x2,
        "uint8x4" => V::Uint8x4,
        "unorm8x2" => V::Unorm8x2,
        "unorm8x4" => V::Unorm8x4,
        "uint16x2" => V::Uint16x2,
        "uint16x4" => V::Uint16x4,
        "float16x2" => V::Float16x2,
        "float16x4" => V::Float16x4,
        "float32" => V::Float32,
        "float32x2" => V::Float32x2,
        "float32x3" => V::Float32x3,
        "float32x4" => V::Float32x4,
        "uint32" => V::Uint32,
        "uint32x2" => V::Uint32x2,
        "uint32x3" => V::Uint32x3,
        "uint32x4" => V::Uint32x4,
        "sint32" => V::Sint32,
        "sint32x2" => V::Sint32x2,
        "sint32x3" => V::Sint32x3,
        "sint32x4" => V::Sint32x4,
        other => return fail(format!("vertex format {other}")),
    })
}

fn view_dimension(name: &str) -> wgpu::TextureViewDimension {
    match name {
        "1d" => wgpu::TextureViewDimension::D1,
        "2d-array" => wgpu::TextureViewDimension::D2Array,
        "cube" => wgpu::TextureViewDimension::Cube,
        "cube-array" => wgpu::TextureViewDimension::CubeArray,
        "3d" => wgpu::TextureViewDimension::D3,
        _ => wgpu::TextureViewDimension::D2,
    }
}

fn compare(name: &str) -> wgpu::CompareFunction {
    use wgpu::CompareFunction as C;
    match name {
        "never" => C::Never,
        "less" => C::Less,
        "equal" => C::Equal,
        "less-equal" => C::LessEqual,
        "greater" => C::Greater,
        "not-equal" => C::NotEqual,
        "greater-equal" => C::GreaterEqual,
        _ => C::Always,
    }
}

fn blend_state(value: &Json) -> Result<wgpu::BlendState, Fail> {
    let component = |value: &Json| {
        let factor = |key: &str, default: wgpu::BlendFactor| {
            use wgpu::BlendFactor as B;
            match value[key].as_str() {
                Some("zero") => B::Zero,
                Some("one") => B::One,
                Some("src") => B::Src,
                Some("one-minus-src") => B::OneMinusSrc,
                Some("src-alpha") => B::SrcAlpha,
                Some("one-minus-src-alpha") => B::OneMinusSrcAlpha,
                Some("dst") => B::Dst,
                Some("one-minus-dst") => B::OneMinusDst,
                Some("dst-alpha") => B::DstAlpha,
                Some("one-minus-dst-alpha") => B::OneMinusDstAlpha,
                Some("src-alpha-saturated") => B::SrcAlphaSaturated,
                Some("constant") => B::Constant,
                Some("one-minus-constant") => B::OneMinusConstant,
                _ => default,
            }
        };
        wgpu::BlendComponent {
            src_factor: factor("srcFactor", wgpu::BlendFactor::One),
            dst_factor: factor("dstFactor", wgpu::BlendFactor::Zero),
            operation: match value["operation"].as_str() {
                Some("subtract") => wgpu::BlendOperation::Subtract,
                Some("reverse-subtract") => wgpu::BlendOperation::ReverseSubtract,
                Some("min") => wgpu::BlendOperation::Min,
                Some("max") => wgpu::BlendOperation::Max,
                _ => wgpu::BlendOperation::Add,
            },
        }
    };
    Ok(wgpu::BlendState { color: component(&value["color"]), alpha: component(&value["alpha"]) })
}

fn layout_entry(entry: &Json) -> Result<wgpu::BindGroupLayoutEntry, Fail> {
    let ty = if let Some(buffer) = entry.get("buffer") {
        wgpu::BindingType::Buffer {
            ty: match buffer["type"].as_str() {
                Some("storage") => wgpu::BufferBindingType::Storage { read_only: false },
                Some("read-only-storage") => wgpu::BufferBindingType::Storage { read_only: true },
                _ => wgpu::BufferBindingType::Uniform,
            },
            has_dynamic_offset: buffer["hasDynamicOffset"].as_bool().unwrap_or(false),
            min_binding_size: num(&buffer["minBindingSize"]).and_then(|size| wgpu::BufferSize::new(size as u64)),
        }
    } else if let Some(sampler) = entry.get("sampler") {
        wgpu::BindingType::Sampler(match sampler["type"].as_str() {
            Some("non-filtering") => wgpu::SamplerBindingType::NonFiltering,
            Some("comparison") => wgpu::SamplerBindingType::Comparison,
            _ => wgpu::SamplerBindingType::Filtering,
        })
    } else if let Some(texture) = entry.get("texture") {
        wgpu::BindingType::Texture {
            sample_type: match texture["sampleType"].as_str() {
                Some("unfilterable-float") => wgpu::TextureSampleType::Float { filterable: false },
                Some("depth") => wgpu::TextureSampleType::Depth,
                Some("sint") => wgpu::TextureSampleType::Sint,
                Some("uint") => wgpu::TextureSampleType::Uint,
                _ => wgpu::TextureSampleType::Float { filterable: true },
            },
            view_dimension: view_dimension(texture["viewDimension"].as_str().unwrap_or("2d")),
            multisampled: texture["multisampled"].as_bool().unwrap_or(false),
        }
    } else if let Some(storage) = entry.get("storageTexture") {
        wgpu::BindingType::StorageTexture {
            access: match storage["access"].as_str() {
                Some("read-only") => wgpu::StorageTextureAccess::ReadOnly,
                Some("read-write") => wgpu::StorageTextureAccess::ReadWrite,
                _ => wgpu::StorageTextureAccess::WriteOnly,
            },
            format: format(&storage["format"])?,
            view_dimension: view_dimension(storage["viewDimension"].as_str().unwrap_or("2d")),
        }
    } else {
        return fail("a binding of no known kind");
    };
    Ok(wgpu::BindGroupLayoutEntry {
        binding: num(&entry["binding"]).unwrap_or(0.0) as u32,
        visibility: wgpu::ShaderStages::from_bits_truncate(num(&entry["visibility"]).unwrap_or(0.0) as u32),
        ty,
        count: None,
    })
}

fn limits_json(limits: &wgpu::Limits) -> Json {
    serde_json::json!({
        "maxTextureDimension1D": limits.max_texture_dimension_1d,
        "maxTextureDimension2D": limits.max_texture_dimension_2d,
        "maxTextureDimension3D": limits.max_texture_dimension_3d,
        "maxTextureArrayLayers": limits.max_texture_array_layers,
        "maxBindGroups": limits.max_bind_groups,
        "maxBindingsPerBindGroup": limits.max_bindings_per_bind_group,
        "maxSampledTexturesPerShaderStage": limits.max_sampled_textures_per_shader_stage,
        "maxSamplersPerShaderStage": limits.max_samplers_per_shader_stage,
        "maxStorageBuffersPerShaderStage": limits.max_storage_buffers_per_shader_stage,
        "maxStorageTexturesPerShaderStage": limits.max_storage_textures_per_shader_stage,
        "maxUniformBuffersPerShaderStage": limits.max_uniform_buffers_per_shader_stage,
        "maxUniformBufferBindingSize": limits.max_uniform_buffer_binding_size,
        "maxStorageBufferBindingSize": limits.max_storage_buffer_binding_size,
        "maxVertexBuffers": limits.max_vertex_buffers,
        "maxBufferSize": limits.max_buffer_size,
        "maxVertexAttributes": limits.max_vertex_attributes,
        "maxVertexBufferArrayStride": limits.max_vertex_buffer_array_stride,
        "maxColorAttachments": limits.max_color_attachments,
        "maxComputeWorkgroupStorageSize": limits.max_compute_workgroup_storage_size,
        "maxComputeInvocationsPerWorkgroup": limits.max_compute_invocations_per_workgroup,
        "maxComputeWorkgroupSizeX": limits.max_compute_workgroup_size_x,
        "maxComputeWorkgroupSizeY": limits.max_compute_workgroup_size_y,
        "maxComputeWorkgroupSizeZ": limits.max_compute_workgroup_size_z,
        "maxComputeWorkgroupsPerDimension": limits.max_compute_workgroups_per_dimension,
        "minUniformBufferOffsetAlignment": limits.min_uniform_buffer_offset_alignment,
        "minStorageBufferOffsetAlignment": limits.min_storage_buffer_offset_alignment,
    })
}

/// The effects' JS and GPU, on the thread that made it
pub struct Host {
    /// Every adapter found, kept: letting one go while another thread draws
    /// can deadlock NVIDIA's driver
    _adapters: Vec<wgpu::Adapter>,
    runtime: Runtime,
    context: Context,
    gpu: Rc<RefCell<Gpu>>,
}

impl Host {
    /// Starts on the adapter `prefer` names, if it's there
    pub fn new(prefer: Option<&str>) -> Result<Self, String> {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::DX12;
        descriptor.flags = super::vello::instance_flags();
        // Windows' own compiler: left to choose, wgpu takes any dxcompiler.dll
        // on the PATH, which some other app's old one crashes
        descriptor.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
        let instance = wgpu::Instance::new(descriptor);
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::DX12));
        let adapter = prefer
            .and_then(|name| adapters.iter().find(|adapter| name.starts_with(adapter.get_info().name.as_str())))
            .or_else(|| adapters.iter().find(|adapter| adapter.get_info().device_type != wgpu::DeviceType::Cpu))
            .cloned()
            .ok_or("no GPU for effects")?;
        let features = adapter.features() & wgpu::Features::FLOAT32_FILTERABLE;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("ligma effects"),
            required_features: features,
            required_limits: wgpu::Limits { max_non_sampler_bindings: super::vello::BINDINGS, ..Default::default() },
            // Small blocks: by default it sets aside hundreds of MB up front
            memory_hints: wgpu::MemoryHints::Manual { suballocated_device_memory_block_size: (1 << 20)..(16 << 20) },
            ..Default::default()
        }))
        .map_err(|error| error.to_string())?;
        let errors = Arc::new(Mutex::new(Vec::new()));
        let caught = errors.clone();
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            let mut caught = caught.lock().unwrap();
            if caught.len() < 64 {
                caught.push(error.to_string());
            }
        }));
        let gpu = Rc::new(RefCell::new(Gpu { device, queue, objects: HashMap::new(), next: 0, owners: HashMap::new(), owner: None, errors, scopes: Vec::new(), logs: Vec::new(), ops: HashMap::new() }));
        let runtime = Runtime::new().map_err(|error| error.to_string())?;
        runtime.set_memory_limit(512 << 20);
        runtime.set_max_stack_size(4 << 20);
        let context = Context::full(&runtime).map_err(|error| error.to_string())?;
        let shared = gpu.clone();
        context.with(|ctx| -> Result<(), String> {
            let native = Function::new(ctx.clone(), move |ctx: Ctx<'_>, op: String, args: String, bytes: rquickjs::function::Opt<Value<'_>>| -> rquickjs::Result<Option<String>> {
                let args: Json = serde_json::from_str(&args).unwrap_or(Json::Null);
                let typed = bytes.0.and_then(|value| TypedArray::<u8>::from_value(value).ok());
                let raw = typed.as_ref().and_then(|typed| typed.as_raw());
                // Only read during the call, while the array is held
                let slice = raw.map(|raw| unsafe { raw.as_ref() });
                // A panic can't cross back into QuickJS, so it's thrown as an error
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| shared.borrow_mut().call(&op, args, slice)))
                    .unwrap_or_else(|panic| Err(format!("{op}: {}", panic.downcast_ref::<String>().cloned().or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default())));
                match result {
                    Ok(Json::Null) => Ok(None),
                    Ok(other) => Ok(Some(other.to_string())),
                    Err(message) => Err(Exception::throw_message(&ctx, &message)),
                }
            })
            .map_err(|error| error.to_string())?;
            ctx.globals().set("__native", native).map_err(|error| error.to_string())?;
            let shim: rquickjs::Result<Value> = ctx.eval(SHIM);
            shim.map_err(|_| describe(&ctx))?;
            Ok(())
        })?;
        Ok(Self { _adapters: adapters, runtime, context, gpu })
    }

    /// Whether effect `id`'s code is loaded
    pub fn has(&self, id: &str) -> bool {
        self.context.with(|ctx| {
            let has: rquickjs::Result<Function> = ctx.globals().get("__has");
            has.and_then(|has| has.call::<_, bool>((id,))).unwrap_or(false)
        })
    }

    /// Loads effect `id` from its bundled `code`
    pub fn load(&mut self, id: &str, code: &str) -> Result<(), String> {
        self.context.with(|ctx| {
            let load: Function = ctx.globals().get("__load").map_err(|error| error.to_string())?;
            load.call::<_, ()>((id, code)).map_err(|_| describe(&ctx))
        })
    }

    /// Runs node `node`'s `chain` of effects, `(id, params as JSON)`, over
    /// `input`, `width` by `height` premultiplied RGBA pixels drawn at
    /// `scale` pixels a unit, and returns the pixels they draw, the same
    /// size and premultiplied too
    pub fn run(&mut self, node: &str, chain: &[(String, String)], input: &[u8], width: u32, height: u32, scale: f32, time: f64) -> Result<Vec<u8>, String> {
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let began = std::time::Instant::now();
        let (input_id, output_id, output) = {
            let mut gpu = self.gpu.borrow_mut();
            gpu.errors.lock().unwrap().clear();
            gpu.logs.clear();
            let source = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("effect input"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &source, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                input,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(width * 4), rows_per_image: Some(height) },
                size,
            );
            let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("effect output"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let input_id = gpu.add(Res::Texture(source));
            let output_id = gpu.add(Res::Texture(target.clone()));
            (input_id, output_id, target)
        };
        self.gpu.borrow_mut().owner = Some(node.to_string());
        let chain_json: Vec<Json> = chain.iter().map(|(id, params)| serde_json::json!([id, serde_json::from_str::<Json>(params).unwrap_or(Json::Null)])).collect();
        let ran = self.context.with(|ctx| -> Result<(), String> {
            let run: Function = ctx.globals().get("__run").map_err(|error| error.to_string())?;
            let chain = ctx.json_parse(serde_json::to_string(&chain_json).unwrap()).map_err(|error| error.to_string())?;
            let promise: rquickjs::Promise = run.call((node, chain, input_id, output_id, vec![width as f64, height as f64, scale as f64, time])).map_err(|_| describe(&ctx))?;
            ctx.globals().set("__ran", promise).map_err(|error| error.to_string())?;
            Ok(())
        });
        let finished = ran.and_then(|_| {
            // Effects may wait on promises, all settled here
            for _ in 0..10_000 {
                match self.runtime.execute_pending_job() {
                    Ok(true) => continue,
                    Ok(false) => break,
                    Err(_) => return Err("an effect failed".to_string()),
                }
            }
            self.context.with(|ctx| {
                let promise: rquickjs::Promise = ctx.globals().get("__ran").map_err(|error| error.to_string())?;
                match promise.state() {
                    rquickjs::promise::PromiseState::Resolved => Ok(()),
                    rquickjs::promise::PromiseState::Rejected => {
                        let _ = promise.result::<Value>();
                        Err(describe(&ctx))
                    }
                    rquickjs::promise::PromiseState::Pending => Err("an effect never finished".into()),
                }
            })
        });
        let js = began.elapsed();
        // What it ran with is kept no longer than the run: its output is
        // kept instead, and it runs again from setup if it has to
        self.context.with(|ctx| {
            let _ = ctx.globals().remove("__ran");
            if let Ok(forget) = ctx.globals().get::<_, Function>("__forget") {
                let _ = forget.call::<_, ()>((node,));
            }
        });
        let mut gpu = self.gpu.borrow_mut();
        let held = |gpu: &Gpu| gpu.device.generate_allocator_report().map_or((0, 0), |report| (report.total_allocated_bytes >> 20, report.total_reserved_bytes >> 20));
        let during = held(&gpu);
        gpu.owner = None;
        gpu.objects.remove(&input_id);
        gpu.objects.remove(&output_id);
        for line in gpu.logs.drain(..) {
            log::info!("native renderer: effect: {line}");
        }
        let result = finished.and_then(|_| match gpu.errors.lock().unwrap().first() {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        });
        let read_at = std::time::Instant::now();
        let pixels = result.and_then(|_| read_back(&gpu.device, &gpu.queue, &output, width, height));
        let read = read_at.elapsed();
        output.destroy();
        gpu.release(node);
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        let after = held(&gpu);
        let mut ops: Vec<_> = gpu.ops.drain().collect();
        ops.sort_by_key(|(_, (_, took))| std::cmp::Reverse(*took));
        let ops: Vec<String> = ops.iter().take(8).map(|(op, (n, took))| format!("{op}x{n}={:.1}", took.as_secs_f64() * 1000.0)).collect();
        super::render::trace(|| format!("effect memory {node} {width}x{height} during={during:?} after={after:?} objects={} js={:.1} read={:.1} ops={}", gpu.objects.len(), js.as_secs_f64() * 1000.0, read.as_secs_f64() * 1000.0, ops.join(" ")));
        pixels
    }
}

/// What went wrong in the JS just now
fn describe(ctx: &Ctx<'_>) -> String {
    let caught = ctx.catch();
    if let Some(exception) = caught.as_exception() {
        return format!("{} {}", exception.message().unwrap_or_default(), exception.stack().unwrap_or_default());
    }
    if let Some(object) = caught.as_object() {
        if let Ok(message) = object.get::<_, String>("message") {
            return message;
        }
    }
    caught.as_string().and_then(|string| string.to_string().ok()).unwrap_or_else(|| format!("{caught:?}"))
}

fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let row = (width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (row * height) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(height) } },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    let (sender, receiver) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device.poll(wgpu::PollType::wait_indefinitely()).map_err(|error| error.to_string())?;
    receiver.recv().map_err(|error| error.to_string())?.map_err(|error| error.to_string())?;
    let mapped = slice.get_mapped_range().map_err(|error| error.to_string())?;
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        let start = (y * row) as usize;
        pixels.extend_from_slice(&mapped[start..start + (width * 4) as usize]);
    }
    drop(mapped);
    buffer.unmap();
    Ok(pixels)
}

impl Drop for Host {
    fn drop(&mut self) {
        self.gpu.borrow_mut().objects.clear();
    }
}
