//! Blurs for Vello's views, on the GPU in compute shaders: what's blurred
//! is drawn by Vello into slots of a texture, which these blur in place, as
//! Skia blurs a layer, for the view to draw from like an image. Wide blurs
//! are blurred smaller, as Skia's are: the slot's averaged down in blocks
//! till the blur's at most 4 pixels, blurred, then scaled back up

use vello::wgpu;

const SHADER: &str = r#"
struct Blur {
    // Its slot in the texture Vello drew into, level 0
    slot: vec4<i32>,
    // Where it's blurred, in the work textures, at its level
    work: vec4<i32>,
    sigma: vec2<f32>,
    radius: vec2<i32>,
    // The area it's clamped to, from the slot's corner, level 0
    clamp: vec4<f32>,
    clamp_sigma: vec2<f32>,
    level: u32,
    clamped: u32,
}

@group(0) @binding(0) var<storage, read> blurs: array<Blur>;
@group(0) @binding(1) var<storage, read> tiles: array<vec4<u32>>;
@group(0) @binding(2) var src: texture_2d<f32>;
@group(0) @binding(3) var work_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(4) var slot_out: texture_storage_2d<rgba8unorm, write>;

struct At {
    blur: Blur,
    p: vec2<i32>,
    ok: bool,
}

fn at(wid: vec3<u32>, groups: vec3<u32>, lid: vec3<u32>) -> At {
    var out: At;
    let index = wid.x + wid.y * groups.x;
    if index >= arrayLength(&tiles) {
        out.ok = false;
        return out;
    }
    let tile = tiles[index];
    out.blur = blurs[tile.x];
    out.p = vec2<i32>(tile.yz * 16u + lid.xy);
    out.ok = true;
    return out;
}

fn load(rect: vec4<i32>, p: vec2<i32>) -> vec4<f32> {
    if any(p < vec2(0)) || any(p >= rect.zw) {
        return vec4(0.0);
    }
    return textureLoad(src, rect.xy + p, 0);
}

// Vello's slot, unpremultiplied, averaged in blocks of its level and
// premultiplied
@compute @workgroup_size(16, 16)
fn down(@builtin(workgroup_id) wid: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let a = at(wid, groups, lid);
    if !a.ok || any(a.p >= a.blur.work.zw) {
        return;
    }
    let size = 1 << a.blur.level;
    var sum = vec4(0.0);
    for (var y = 0; y < size; y += 1) {
        for (var x = 0; x < size; x += 1) {
            let c = load(a.blur.slot, a.p * size + vec2(x, y));
            sum += vec4(c.rgb * c.a, c.a);
        }
    }
    textureStore(work_out, a.blur.work.xy + a.p, sum / f32(size * size));
}

fn gauss(p: vec2<i32>, rect: vec4<i32>, step: vec2<i32>, sigma: f32, radius: i32) -> vec4<f32> {
    var sum = vec4(0.0);
    var weights = 0.0;
    for (var i = -radius; i <= radius; i += 1) {
        let w = exp(-f32(i * i) / (2.0 * sigma * sigma));
        sum += load(rect, p + step * i) * w;
        weights += w;
    }
    return sum / weights;
}

@compute @workgroup_size(16, 16)
fn horizontal(@builtin(workgroup_id) wid: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let a = at(wid, groups, lid);
    if !a.ok || any(a.p >= a.blur.work.zw) {
        return;
    }
    let c = select(load(a.blur.work, a.p), gauss(a.p, a.blur.work, vec2(1, 0), a.blur.sigma.x, a.blur.radius.x), a.blur.radius.x > 0);
    textureStore(work_out, a.blur.work.xy + a.p, c);
}

@compute @workgroup_size(16, 16)
fn vertical(@builtin(workgroup_id) wid: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let a = at(wid, groups, lid);
    if !a.ok || any(a.p >= a.blur.work.zw) {
        return;
    }
    let c = select(load(a.blur.work, a.p), gauss(a.p, a.blur.work, vec2(0, 1), a.blur.sigma.y, a.blur.radius.y), a.blur.radius.y > 0);
    textureStore(work_out, a.blur.work.xy + a.p, c);
}

fn erf(x: f32) -> f32 {
    // Abramowitz and Stegun 7.1.26
    let t = 1.0 / (1.0 + 0.3275911 * abs(x));
    let y = 1.0 - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t * exp(-x * x);
    return sign(x) * y;
}

// How much of a blur of `sigma` at `x` falls between `low` and `high`
fn inside(x: f32, low: f32, high: f32, sigma: f32) -> f32 {
    if sigma <= 0.0 {
        return select(0.0, 1.0, x >= low && x < high);
    }
    let k = 1.0 / (sigma * sqrt(2.0));
    return 0.5 * (erf((high - x) * k) - erf((low - x) * k));
}

// Scaled back up to the slot, bilinear, divided by the blur of the area
// it's clamped to if it is, premultiplied
@compute @workgroup_size(16, 16)
fn up(@builtin(workgroup_id) wid: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let a = at(wid, groups, lid);
    if !a.ok || any(a.p >= a.blur.slot.zw) {
        return;
    }
    var c: vec4<f32>;
    if a.blur.level == 0u {
        c = load(a.blur.work, a.p);
    } else {
        let u = (vec2<f32>(a.p) + 0.5) / f32(1 << a.blur.level) - 0.5;
        let i = vec2<i32>(floor(u));
        let f = u - floor(u);
        let top = mix(load(a.blur.work, i), load(a.blur.work, i + vec2(1, 0)), f.x);
        let bottom = mix(load(a.blur.work, i + vec2(0, 1)), load(a.blur.work, i + vec2(1, 1)), f.x);
        c = mix(top, bottom, f.y);
    }
    if a.blur.clamped != 0u {
        let x = vec2<f32>(a.p) + 0.5;
        let w = inside(x.x, a.blur.clamp.x, a.blur.clamp.z, a.blur.clamp_sigma.x) * inside(x.y, a.blur.clamp.y, a.blur.clamp.w, a.blur.clamp_sigma.y);
        c = select(vec4(0.0), c / w, w > 0.0);
    }
    textureStore(slot_out, a.blur.slot.xy + a.p, clamp(c, vec4(0.0), vec4(1.0)));
}
"#;

/// One blur of a slot
#[derive(Clone)]
pub struct Job {
    /// The slot, in the texture Vello drew into
    pub slot: [i32; 4],
    /// Its blur, in pixels, each way
    pub sigma: (f32, f32),
    /// The area it's clamped to, from the slot's corner
    pub clamp: Option<[f32; 4]>,
}

pub struct Blurrer {
    down: wgpu::ComputePipeline,
    horizontal: wgpu::ComputePipeline,
    vertical: wgpu::ComputePipeline,
    up: wgpu::ComputePipeline,
    /// Two textures blurs are worked on in, kept while big enough
    pub work: Option<(wgpu::Texture, wgpu::Texture, u32, u32)>,
}

/// Blurred at a level where it's at most this wide, as Skia does
const MOST_SIGMA: f32 = 4.0;

/// Blocks a level's pixels are each the average of at most, a slot's
/// level 12 at the most
const LEVELS: u32 = 12;

impl Blurrer {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("ligma blur"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Self { down: pipeline("down"), horizontal: pipeline("horizontal"), vertical: pipeline("vertical"), up: pipeline("up"), work: None }
    }

    /// Blurs `jobs` in place in `texture`, Vello's, as the next of
    /// `encoder`'s work
    pub fn blur(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture, jobs: &[Job], width: u32) -> Result<(), String> {
        if jobs.is_empty() {
            return Ok(());
        }
        // Each job's level and where it's worked on, shelved
        let mut infos: Vec<u8> = Vec::with_capacity(jobs.len() * 80);
        let (mut small, mut big): (Vec<[u32; 4]>, Vec<[u32; 4]>) = (Vec::new(), Vec::new());
        let width = width.max(16);
        let (mut x, mut y, mut row) = (0u32, 0u32, 0u32);
        for (index, job) in jobs.iter().enumerate() {
            let sigma = job.sigma.0.max(job.sigma.1);
            let level = if sigma <= MOST_SIGMA { 0 } else { (sigma / MOST_SIGMA).log2().ceil().clamp(0.0, LEVELS as f32) as u32 };
            let scale = (1u32 << level) as f32;
            let (w, h) = ((job.slot[2] as u32).div_ceil(1 << level), (job.slot[3] as u32).div_ceil(1 << level));
            if w > width {
                return Err("blur wider than its texture".into());
            }
            if x + w > width {
                x = 0;
                y += row;
                row = 0;
            }
            let work = [x, y, w, h];
            x += w;
            row = row.max(h);
            let sigma = (job.sigma.0 / scale, job.sigma.1 / scale);
            let radius = |sigma: f32| if sigma > 0.03 { (3.0 * sigma).ceil() as i32 } else { 0 };
            let clamp = job.clamp.unwrap_or([0.0; 4]);
            for v in job.slot {
                infos.extend(v.to_le_bytes());
            }
            for v in work {
                infos.extend((v as i32).to_le_bytes());
            }
            infos.extend(sigma.0.to_le_bytes());
            infos.extend(sigma.1.to_le_bytes());
            infos.extend(radius(sigma.0).to_le_bytes());
            infos.extend(radius(sigma.1).to_le_bytes());
            for v in clamp {
                infos.extend(v.to_le_bytes());
            }
            infos.extend(job.sigma.0.to_le_bytes());
            infos.extend(job.sigma.1.to_le_bytes());
            infos.extend(level.to_le_bytes());
            infos.extend((job.clamp.is_some() as u32).to_le_bytes());
            for ty in 0..h.div_ceil(16) {
                for tx in 0..w.div_ceil(16) {
                    small.push([index as u32, tx, ty, 0]);
                }
            }
            for ty in 0..(job.slot[3] as u32).div_ceil(16) {
                for tx in 0..(job.slot[2] as u32).div_ceil(16) {
                    big.push([index as u32, tx, ty, 0]);
                }
            }
        }
        let height = y + row;
        if height > 8192 {
            return Err("blurs past their textures".into());
        }
        if self.work.as_ref().is_none_or(|(_, _, w, h)| *w < width || *h < height) {
            let step = |size: u32| size.div_ceil(256) * 256;
            let (w, h) = (step(width), step(height));
            let make = || {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("ligma blur work"),
                    size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
            };
            self.work = Some((make(), make(), w, h));
        }
        let Some((a, b, _, _)) = &self.work else { return Err("no work textures".into()) };
        let buffer = |bytes: &[u8]| {
            use wgpu::util::DeviceExt as _;
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytes, usage: wgpu::BufferUsages::STORAGE })
        };
        let tiles = |list: &[[u32; 4]]| buffer(&list.iter().flatten().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>());
        let (infos, small_tiles, big_tiles) = (buffer(&infos), tiles(&small), tiles(&big));
        let (texture_view, a_view, b_view) = (texture.create_view(&Default::default()), a.create_view(&Default::default()), b.create_view(&Default::default()));
        let mut pass = |pipeline: &wgpu::ComputePipeline, tiles: &wgpu::Buffer, count: usize, src: &wgpu::TextureView, out: &wgpu::TextureView, slot: bool| {
            let out_binding = if slot { 4 } else { 3 };
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: infos.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: tiles.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(src) },
                    wgpu::BindGroupEntry { binding: out_binding, resource: wgpu::BindingResource::TextureView(out) },
                ],
            });
            let mut compute = encoder.begin_compute_pass(&Default::default());
            compute.set_pipeline(pipeline);
            compute.set_bind_group(0, &group, &[]);
            let count = count as u32;
            let x = count.min(65535);
            compute.dispatch_workgroups(x, count.div_ceil(x), 1);
        };
        pass(&self.down, &small_tiles, small.len(), &texture_view, &a_view, false);
        pass(&self.horizontal, &small_tiles, small.len(), &a_view, &b_view, false);
        pass(&self.vertical, &small_tiles, small.len(), &b_view, &a_view, false);
        pass(&self.up, &big_tiles, big.len(), &a_view, &texture_view, true);
        Ok(())
    }
}
