//! Blurs for Vello's views, on the GPU in compute shaders: what's blurred
//! is drawn by Vello into slots of a texture, which these blur in place, as
//! Skia blurs a layer, for the view to draw from like an image. Wide blurs
//! are blurred smaller, as Skia's are: the slot's averaged down in blocks
//! till the blur's at most 4 pixels, blurred, then scaled back up. Noise
//! and grain are done the same way, over what their layer drew, as draw.rs's
//! shaders do them for Skia

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
    // Blurred, noise over it, or grain
    kind: u32,
    // Noise's kind: multitone, monotone or duotone
    noise: u32,
    density: f32,
    pad0: f32,
    // From a pixel of the slot to the effect's own space
    to_effect: vec4<f32>,
    to_effect_at: vec2<f32>,
    frequency: vec2<f32>,
    center: vec2<f32>,
    pad1: vec2<f32>,
    color: vec4<f32>,
    secondary: vec4<f32>,
    // How far grain moves what's drawn, from the field's middle to its
    // edge, in the slot's pixels
    push: vec4<f32>,
}

@group(0) @binding(0) var<storage, read> blurs: array<Blur>;
@group(0) @binding(1) var<storage, read> tiles: array<vec4<u32>>;
@group(0) @binding(2) var src: texture_2d<f32>;
@group(0) @binding(3) var work_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(4) var slot_out: texture_storage_2d<rgba8unorm, write>;
// Figma's noise octaves: each one's seed and two gradient scales
@group(0) @binding(5) var<storage, read> octaves: array<vec4<f32>>;

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

// Figma's fractal noise, as draw.rs's FBM
fn cell_hash(cell: vec2<f32>) -> f32 {
    var t = dot(cell, vec2(12.9898, 78.233));
    t = t - floor(t / 3.14159) * 3.14159;
    return fract(sin(t * 2.71828) * 438.5453 + 1.73205);
}

fn component(cell: vec2<f32>, k: vec2<f32>, s: f32) -> f32 {
    return 2.0 * (fract(sin(dot(k, cell) * 0.0625) * (s + 312.0)) - 0.5);
}

fn slope(cell: vec2<f32>, octave: vec3<f32>) -> vec2<f32> {
    let a = cell_hash(cell);
    let s1 = octave.x * a * 1.61803 + 18.345;
    let s2 = octave.x * a * 0.61803 + 21.543;
    let gx = component(cell, vec2(12.42, 78.23) * octave.y, s1);
    let gy = component(cell, vec2(-19.98, 31.21) * octave.z, s2);
    return vec2(gx, gy) / length(vec2(gx, gy));
}

fn fade(t: f32) -> f32 {
    return t * t * (3.0 - 2.0 * t);
}

fn gradient_noise(octave: vec3<f32>, p: vec2<f32>) -> f32 {
    let c = floor(p);
    let f = fract(p);
    let u = vec2(fade(f.x), fade(f.y));
    let a = dot(f, slope(c, octave));
    let b = dot(f - vec2(1.0, 0.0), slope(c + vec2(1.0, 0.0), octave));
    let d = dot(f - vec2(0.0, 1.0), slope(c + vec2(0.0, 1.0), octave));
    let e = dot(f - vec2(1.0, 1.0), slope(c + vec2(1.0, 1.0), octave));
    return mix(mix(a, b, u.x), mix(d, e, u.x), u.y);
}

// Three octaves from the `first`, at `q`, from -1.75 to 1.75
fn fbm(first: u32, q: vec2<f32>) -> f32 {
    return gradient_noise(octaves[first].xyz, q) + gradient_noise(octaves[first + 1u].xyz, q * 2.0) / 2.0 + gradient_noise(octaves[first + 2u].xyz, q * 4.0) / 4.0;
}

// From 0 to 1
fn field(first: u32, q: vec2<f32>) -> f32 {
    return (fbm(first, q) + 1.0) / 2.0;
}

fn band(v: f32, upper: bool, density: f32) -> f32 {
    let low = 10.0 * (select(0.25, 0.75, upper) - density / 4.0);
    let high = low + 10.0 * (density / 2.0);
    let x = v * 10.0;
    return step(low, x) * (1.0 - step(high, x));
}

// Figma's noise effect, as draw.rs's NOISE, at `p` in the effect's space
fn noise(b: Blur, p: vec2<f32>) -> vec4<f32> {
    let at = (p - b.center) * b.frequency;
    let red = field(0u, at);
    let green = field(3u, at);
    let blue = field(6u, at);
    if b.noise == 0u {
        let a = band(field(9u, at), false, b.density) * b.color.a;
        return vec4(vec3(2.0 * red - 0.5, 2.0 * green - 0.5, 2.0 * blue - 0.5) * a, a);
    }
    let luma = 0.213 * red + 0.715 * green + 0.072 * blue;
    let low = band(luma, false, b.density);
    if b.noise == 1u {
        let a = low * b.color.a;
        return vec4(b.color.rgb * a, a);
    }
    if low == 1.0 {
        return vec4(b.secondary.rgb * b.secondary.a, b.secondary.a);
    }
    let a = band(luma, true, b.density) * b.color.a;
    return vec4(b.color.rgb * a, a);
}

// Vello's slot at `p`, premultiplied
fn drawn(rect: vec4<i32>, p: vec2<i32>) -> vec4<f32> {
    let c = load(rect, p);
    return vec4(c.rgb * c.a, c.a);
}

// Noise over what the layer drew in its slot, only where it drew, as
// draw.rs's NOISE_OVER, or what it drew moved by grain's two fields, as
// Skia's displacement map moves it, premultiplied into the filtered
// texture
@compute @workgroup_size(16, 16)
fn effect(@builtin(workgroup_id) wid: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let a = at(wid, groups, lid);
    if !a.ok || any(a.p >= a.blur.slot.zw) {
        return;
    }
    let b = a.blur;
    let center = vec2<f32>(a.p) + 0.5;
    let q = mat2x2(b.to_effect.xy, b.to_effect.zw) * center + b.to_effect_at;
    var c: vec4<f32>;
    if b.kind == 1u {
        c = drawn(b.slot, a.p);
        let s = noise(b, q) * c.a;
        c = s + c * (1.0 - s.a);
    } else {
        let at = (q - b.center) * b.frequency;
        // Kept 8 bits a channel, as Skia keeps the field
        let f = round(vec2(fbm(0u, at) / 4.0 + 0.5, fbm(3u, at) / 4.0 + 0.5) * 255.0) / 255.0;
        let source = center + mat2x2(b.push.xy, b.push.zw) * (f - 0.5);
        c = drawn(b.slot, vec2<i32>(floor(source)));
    }
    textureStore(slot_out, b.work.xy + a.p, clamp(c, vec4(0.0), vec4(1.0)));
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
    pub effect: Effect,
}

/// What's done to a slot besides blurring it
#[derive(Clone)]
pub enum Effect {
    Blur,
    /// Figma's noise over it (see draw.rs's `NOISE`): `to_effect` takes
    /// a pixel of the slot, from its corner, to the effect's own space
    Noise { to_effect: [f32; 6], frequency: [f32; 2], center: [f32; 2], kind: u32, density: f32, color: [f32; 4], secondary: [f32; 4] },
    /// It moved by grain's fields, as far as `push` takes them from their
    /// middle (see draw.rs's `TEXTURE`)
    Grain { to_effect: [f32; 6], frequency: [f32; 2], center: [f32; 2], push: [f32; 4] },
}

pub struct Blurrer {
    down: wgpu::ComputePipeline,
    horizontal: wgpu::ComputePipeline,
    vertical: wgpu::ComputePipeline,
    filter: wgpu::ComputePipeline,
    up: wgpu::ComputePipeline,
    /// Two textures blurs are worked on in, kept while big enough
    pub work: Option<(wgpu::Texture, wgpu::Texture, u32, u32)>,
    /// What noise and grain make, on its way back to their slots, kept
    /// while big enough
    pub filtered: Option<(wgpu::Texture, u32, u32)>,
}

/// Blurred at a level where it's at most this wide, as Skia does
const MOST_SIGMA: f32 = 4.0;

/// Blocks a level's pixels are each the average of at most, a slot's
/// level 12 at the most
const LEVELS: u32 = 12;

/// Rows of the work textures blurs take before the next are blurred in a
/// batch of their own, which bounds their memory however many a view has
const WORK_ROWS: u32 = 1024;

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
        Self { down: pipeline("down"), horizontal: pipeline("horizontal"), vertical: pipeline("vertical"), filter: pipeline("effect"), up: pipeline("up"), work: None, filtered: None }
    }

    /// Blurs `jobs` in place in `texture`, Vello's, as the next of
    /// `encoder`'s work, noise and grain with Figma's `octaves`
    pub fn blur(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, texture: &wgpu::Texture, jobs: &[Job], width: u32, octaves: &[f32]) -> Result<(), String> {
        if jobs.is_empty() {
            return Ok(());
        }
        // Each job's level and where it's worked on, shelved: blurs in the
        // work textures, noise and grain in the filtered one
        let mut infos: Vec<u8> = Vec::with_capacity(jobs.len() * 192);
        // Blurs' tiles at their level and in their slot, in batches that
        // each fit the work textures, which are used again for the next
        let mut batches: Vec<(Vec<[u32; 4]>, Vec<[u32; 4]>)> = vec![(Vec::new(), Vec::new())];
        let mut filters: Vec<[u32; 4]> = Vec::new();
        let width = width.max(16);
        let mut shelves = [(0u32, 0u32, 0u32); 2];
        let mut work_rows = 0;
        // Where each of noise and grain is in the filtered texture, and its
        // slot
        let mut filtered: Vec<([u32; 4], [i32; 4])> = Vec::new();
        for (index, job) in jobs.iter().enumerate() {
            let blur = matches!(job.effect, Effect::Blur);
            let sigma = job.sigma.0.max(job.sigma.1);
            let level = if !blur || sigma <= MOST_SIGMA { 0 } else { (sigma / MOST_SIGMA).log2().ceil().clamp(0.0, LEVELS as f32) as u32 };
            let scale = (1u32 << level) as f32;
            let (w, h) = ((job.slot[2] as u32).div_ceil(1 << level), (job.slot[3] as u32).div_ceil(1 << level));
            if w > width {
                return Err("blur wider than its texture".into());
            }
            let (x, y, row) = &mut shelves[usize::from(!blur)];
            if *x + w > width {
                *x = 0;
                *y += *row;
                *row = 0;
            }
            if blur && *y > 0 && *y + h > WORK_ROWS {
                batches.push((Vec::new(), Vec::new()));
                (*x, *y, *row) = (0, 0, 0);
            }
            let work = [*x, *y, w, h];
            *x += w;
            *row = (*row).max(h);
            if blur {
                work_rows = work_rows.max(*y + *row);
            }
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
            let none = ([0.0; 6], [0.0; 2], [0.0; 2]);
            let (kind, noise, density, (to_effect, frequency, center), color, secondary, push) = match &job.effect {
                Effect::Blur => (0u32, 0u32, 0.0f32, none, [0.0; 4], [0.0; 4], [0.0; 4]),
                Effect::Noise { to_effect, frequency, center, kind, density, color, secondary } => (1, *kind, *density, (*to_effect, *frequency, *center), *color, *secondary, [0.0; 4]),
                Effect::Grain { to_effect, frequency, center, push } => (2, 0, 0.0, (*to_effect, *frequency, *center), [0.0; 4], [0.0; 4], *push),
            };
            infos.extend(kind.to_le_bytes());
            infos.extend(noise.to_le_bytes());
            infos.extend(density.to_le_bytes());
            infos.extend(0f32.to_le_bytes());
            for v in to_effect.into_iter().chain(frequency).chain(center).chain([0.0; 2]).chain(color).chain(secondary).chain(push) {
                infos.extend(v.to_le_bytes());
            }
            let tile = |list: &mut Vec<[u32; 4]>, w: u32, h: u32| {
                for ty in 0..h.div_ceil(16) {
                    for tx in 0..w.div_ceil(16) {
                        list.push([index as u32, tx, ty, 0]);
                    }
                }
            };
            if let (true, Some((small, big))) = (blur, batches.last_mut()) {
                tile(small, w, h);
                tile(big, job.slot[2] as u32, job.slot[3] as u32);
            } else {
                tile(&mut filters, w, h);
                filtered.push((work, job.slot));
            }
        }
        let heights = [work_rows, shelves[1].1 + shelves[1].2];
        if heights.iter().any(|&height| height > 8192) {
            return Err("blurs past their textures".into());
        }
        let step = |size: u32| size.div_ceil(256) * 256;
        let make = |label: &str, w: u32, h: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        if batches.iter().any(|(small, _)| !small.is_empty()) && self.work.as_ref().is_none_or(|(_, _, w, h)| *w < width || *h < heights[0]) {
            let (w, h) = (step(width), step(heights[0]));
            let usage = wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING;
            self.work = Some((make("ligma blur work", w, h, wgpu::TextureFormat::Rgba16Float, usage), make("ligma blur work", w, h, wgpu::TextureFormat::Rgba16Float, usage), w, h));
        }
        if !filters.is_empty() && self.filtered.as_ref().is_none_or(|(_, w, h)| *w < width || *h < heights[1]) {
            let (w, h) = (step(width), step(heights[1]));
            self.filtered = Some((make("ligma filtered", w, h, wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC), w, h));
        }
        let buffer = |bytes: &[u8]| {
            use wgpu::util::DeviceExt as _;
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytes, usage: wgpu::BufferUsages::STORAGE })
        };
        let infos = buffer(&infos);
        // Each octave a vec4
        let octaves: Vec<u8> = (0..12).flat_map(|i| (0..4).map(move |j| if j < 3 { octaves.get(i * 3 + j).copied().unwrap_or(0.0) } else { 0.0 })).flat_map(f32::to_le_bytes).collect();
        let octaves = buffer(&octaves);
        let texture_view = texture.create_view(&Default::default());
        let mut pass = |pipeline: &wgpu::ComputePipeline, tiles: &[[u32; 4]], src: &wgpu::TextureView, out: &wgpu::TextureView, slot: bool| {
            if tiles.is_empty() {
                return;
            }
            let list = buffer(&tiles.iter().flatten().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>());
            let out_binding = if slot { 4 } else { 3 };
            let mut entries = vec![
                wgpu::BindGroupEntry { binding: 0, resource: infos.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: list.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(src) },
                wgpu::BindGroupEntry { binding: out_binding, resource: wgpu::BindingResource::TextureView(out) },
            ];
            if std::ptr::eq(pipeline, &self.filter) {
                entries.push(wgpu::BindGroupEntry { binding: 5, resource: octaves.as_entire_binding() });
            }
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &pipeline.get_bind_group_layout(0), entries: &entries });
            let mut compute = encoder.begin_compute_pass(&Default::default());
            compute.set_pipeline(pipeline);
            compute.set_bind_group(0, &group, &[]);
            let count = tiles.len() as u32;
            let x = count.min(65535);
            compute.dispatch_workgroups(x, count.div_ceil(x), 1);
        };
        if let Some((a, b, _, _)) = &self.work {
            let (a_view, b_view) = (a.create_view(&Default::default()), b.create_view(&Default::default()));
            for (small, big) in &batches {
                pass(&self.down, small, &texture_view, &a_view, false);
                pass(&self.horizontal, small, &a_view, &b_view, false);
                pass(&self.vertical, small, &b_view, &a_view, false);
                pass(&self.up, big, &a_view, &texture_view, true);
            }
        }
        // Noise and grain go from their slots to the filtered texture, then
        // back to the slots
        if let (false, Some((out, _, _))) = (filters.is_empty(), &self.filtered) {
            pass(&self.filter, &filters, &texture_view, &out.create_view(&Default::default()), true);
            drop(pass);
            for (work, slot) in &filtered {
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo { texture: out, mip_level: 0, origin: wgpu::Origin3d { x: work[0], y: work[1], z: 0 }, aspect: wgpu::TextureAspect::All },
                    wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d { x: slot[0] as u32, y: slot[1] as u32, z: 0 }, aspect: wgpu::TextureAspect::All },
                    wgpu::Extent3d { width: work[2], height: work[3], depth_or_array_layers: 1 },
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use vello::wgpu::naga;

    #[test]
    fn shader_is_valid() {
        let module = naga::front::wgsl::parse_str(super::SHADER).unwrap_or_else(|error| panic!("{}", error.emit_to_string(super::SHADER)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).unwrap();
    }
}
