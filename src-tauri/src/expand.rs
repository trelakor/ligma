//! Expand on this device (Settings > Features > Images): the image bar's
//! Expand in free projects (see ai_actions.rs), which grows an image past
//! its edges, as Photoshop's Generative Expand does, and fills what's new
//! either from the image itself, as its Content-Aware Fill does, or with
//! LaMa, an inpainting model, on this device
//!
//! The page hands a request over as a download (see handoff.rs): a line of
//! JSON, then the layer's fills as Figma draws them, as a PNG:
//!
//! `{"method":"content"|"generative","left":l,"top":t,"right":r,"bottom":b,"sample":[…]}`
//!
//! The sides are how far to grow the image, in fractions of its width
//! (left and right) or height (top and bottom). `sample` is strokes of the
//! brush, `{"points":[[x,y],…],"radius":r,"subtract":false}`, in fractions
//! of the image and its width, painting where Content-aware takes from, or
//! taking it away; without any, it takes from all of the image. The result
//! is `{"png":<base64>}`, the grown image, with the image's own pixels as
//! they were, or "empty" when it doesn't grow
//!
//! - **Content-aware** works as Wexler, Shechtman and Irani's space-time
//!   completion does, coarse to fine: each new pixel's patch is matched to
//!   the image's most alike with PatchMatch (Barnes et al.), and each new
//!   pixel becomes a blend of what the patches over it matched, weighted by
//!   how well, which is matched again, until it settles. Pure Rust, on every
//!   platform, with no download
//! - **Generative** fills it with LaMa (big-lama, Apache 2.0), as ONNX, on
//!   the CPU with ONNX Runtime, where it runs (see models.rs). LaMa takes a
//!   512 pixel square, so its fill is scaled up to the image, and evened out
//!   into the image's own colors along the edge

use base64::Engine;
use serde::Deserialize;
use tauri::AppHandle;

#[derive(Deserialize)]
struct Request {
    method: String,
    #[serde(default)]
    left: f32,
    #[serde(default)]
    top: f32,
    #[serde(default)]
    right: f32,
    #[serde(default)]
    bottom: f32,
    #[serde(default)]
    sample: Vec<Stroke>,
}

#[derive(Deserialize)]
struct Stroke {
    points: Vec<[f32; 2]>,
    radius: f32,
    #[serde(default)]
    subtract: bool,
}

/// The page keeps the grown image under this many pixels
const MAX_PIXELS: u64 = 1 << 24;

/// Why there's no result: the page's job was cancelled, or it failed
#[derive(Debug, PartialEq)]
enum Failure {
    Cancelled,
    Failed,
}

impl From<Failure> for &'static str {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Cancelled => "cancelled",
            Failure::Failed => "failed",
        }
    }
}

/// What `bytes` asks for, done for `job` in tab `tab`: the grown image as
/// JSON, or why there's none: "empty" when it doesn't grow, "cancelled"
/// when the model wasn't downloaded or the page cancelled, and "failed"
/// otherwise
pub fn handle(app: &AppHandle, tab: u32, job: u64, bytes: &[u8]) -> Result<String, &'static str> {
    let split = bytes.iter().position(|&byte| byte == b'\n').unwrap_or(bytes.len());
    let request: Request = serde_json::from_slice(&bytes[..split]).map_err(|_| "failed")?;
    let image = image::load_from_memory_with_format(bytes.get(split + 1..).unwrap_or_default(), image::ImageFormat::Png)
        .map_err(failed)?
        .into_rgba8();
    let mut canvas = Canvas::grown(&image, &request)?;
    let stopped = || crate::handoff::cancelled(tab, job);
    match request.method.as_str() {
        "generative" => generative(app, tab, job, &mut canvas)?,
        _ => {
            let sample = canvas.sample(&request.sample, image.width(), image.height());
            fill(&mut canvas, sample.as_deref(), &stopped).map_err(<&str>::from)?;
        }
    }
    let png = canvas.png()?;
    Ok(format!(r#"{{"png":"{}"}}"#, base64::engine::general_purpose::STANDARD.encode(png)))
}

fn failed(error: impl std::fmt::Display) -> &'static str {
    log::warn!("Expand: {error}");
    "failed"
}

/// Fills `canvas` with the generative model Settings names, once it's
/// downloaded. If it isn't, both models are offered, to download one and
/// make it the one used from then on; with neither chosen, it's cancelled
#[cfg(birefnet)]
fn generative(app: &AppHandle, tab: u32, job: u64, canvas: &mut Canvas) -> Result<(), &'static str> {
    use crate::models::{DIFFUSION, EXPAND};
    let preferred = crate::settings::current(app).generative_model;
    let installed = |name: &str| if name == "lama" { EXPAND.is_installed(app) } else { DIFFUSION.is_installed(app) };
    let (model, chosen) = if installed(&preferred) {
        (preferred, false)
    } else {
        (choose_model(app).ok_or("cancelled")?, true)
    };
    if model == "lama" {
        lama::fill(app, tab, job, canvas, chosen)
    } else {
        diffusion::fill(app, tab, job, canvas, chosen)
    }
}

/// Asks which generative model to download, or use if it's there, and
/// makes it the one Settings names: "lama" or "diffusion", or None
#[cfg(birefnet)]
fn choose_model(app: &AppHandle) -> Option<String> {
    use crate::models::{DIFFUSION, EXPAND};
    use crate::prompt::Choice;
    let place = if cfg!(windows) { "PC" } else { "Mac" };
    let size = |megabytes: u64| if megabytes >= 1000 { format!("{:.1} GB", megabytes as f64 / 1000.0) } else { format!("{megabytes} MB") };
    let choice = |installed: bool, name: &str, detail: String| Choice {
        name: name.to_owned(),
        detail,
        button: if installed { "Use".to_owned() } else { "Download".to_owned() },
    };
    let choices = vec![
        choice(
            DIFFUSION.is_installed(app),
            "Stable Diffusion",
            format!(
                "Draws new things past the edge, from scenery to the rest of a subject. {}, about a minute an image on this {place}",
                size(DIFFUSION.megabytes())
            ),
        ),
        choice(
            EXPAND.is_installed(app),
            "LaMa",
            format!("Carries backgrounds and textures on, without drawing anything new. {}, a few seconds an image", size(EXPAND.megabytes())),
        ),
    ];
    let (send, receive) = std::sync::mpsc::channel();
    crate::prompt::choose(
        app,
        "Choose a Generative Model",
        format!("Generative expand runs on this {place} with a model Ligma downloads once from Hugging Face. You can change it or download the other in Settings."),
        choices,
        move |choice| {
            let _ = send.send(choice);
        },
    );
    let model = ["diffusion", "lama"][receive.recv().ok().flatten()?];
    let mut settings = crate::settings::current(app);
    settings.generative_model = model.to_owned();
    if crate::settings::change(app, settings.clone()).is_ok() {
        crate::settings::show_in_window(app, &settings);
    }
    Some(model.to_owned())
}

#[cfg(not(birefnet))]
fn generative(_: &AppHandle, _: u32, _: u64, _: &mut Canvas) -> Result<(), &'static str> {
    Err("failed")
}

/// The grown image: its pixels, premultiplied, 0 to 255, and which of them
/// are the image's own, the rest to fill
struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<[f32; 4]>,
    known: Vec<bool>,
    /// Where the image is in it
    left: usize,
    top: usize,
}

impl Canvas {
    /// `image` grown as `request` says
    fn grown(image: &image::RgbaImage, request: &Request) -> Result<Self, &'static str> {
        let (width, height) = (image.width() as usize, image.height() as usize);
        let side = |fraction: f32, size: usize| (fraction.max(0.0) * size as f32).round() as usize;
        let (left, right) = (side(request.left, width), side(request.right, width));
        let (top, bottom) = (side(request.top, height), side(request.bottom, height));
        if left + right + top + bottom == 0 || width == 0 || height == 0 {
            return Err("empty");
        }
        let (total_width, total_height) = (width + left + right, height + top + bottom);
        if (total_width as u64) * (total_height as u64) > MAX_PIXELS {
            return Err(failed("too large"));
        }
        let mut canvas = Self {
            width: total_width,
            height: total_height,
            pixels: vec![[0.0; 4]; total_width * total_height],
            known: vec![false; total_width * total_height],
            left,
            top,
        };
        for (x, y, pixel) in image.enumerate_pixels() {
            let at = (y as usize + top) * total_width + x as usize + left;
            canvas.pixels[at] = premultiply(pixel.0);
            canvas.known[at] = true;
        }
        Ok(canvas)
    }

    /// Where Content-aware takes from, as `strokes` paint it over the image,
    /// `width` by `height`, or None for all of it
    fn sample(&self, strokes: &[Stroke], width: u32, height: u32) -> Option<Vec<bool>> {
        if strokes.is_empty() {
            return None;
        }
        let (width, height) = (width as f32, height as f32);
        let mut painted = vec![false; self.width * self.height];
        for stroke in strokes {
            let radius = (stroke.radius * width).max(0.5);
            let points: Vec<[f32; 2]> =
                stroke.points.iter().map(|&[x, y]| [self.left as f32 + x * width, self.top as f32 + y * height]).collect();
            let segments: Vec<([f32; 2], [f32; 2])> = match points.len() {
                0 => continue,
                1 => vec![(points[0], points[0])],
                _ => points.windows(2).map(|pair| (pair[0], pair[1])).collect(),
            };
            for (a, b) in segments {
                let rows = span(a[1], b[1], radius, self.height);
                let columns = span(a[0], b[0], radius, self.width);
                for y in rows {
                    for x in columns.clone() {
                        if from_segment([x as f32 + 0.5, y as f32 + 0.5], a, b) <= radius {
                            painted[y * self.width + x] = !stroke.subtract;
                        }
                    }
                }
            }
        }
        for (painted, &known) in painted.iter_mut().zip(&self.known) {
            *painted &= known;
        }
        painted.iter().any(|&painted| painted).then_some(painted)
    }

    fn png(&self) -> Result<Vec<u8>, &'static str> {
        use image::ImageEncoder;
        let mut raw = Vec::with_capacity(self.pixels.len() * 4);
        for &pixel in &self.pixels {
            raw.extend_from_slice(&unpremultiply(pixel));
        }
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&raw, self.width as u32, self.height as u32, image::ExtendedColorType::Rgba8)
            .map_err(failed)?;
        Ok(png)
    }
}

fn premultiply([r, g, b, a]: [u8; 4]) -> [f32; 4] {
    let alpha = a as f32 / 255.0;
    [r as f32 * alpha, g as f32 * alpha, b as f32 * alpha, a as f32]
}

fn unpremultiply([r, g, b, a]: [f32; 4]) -> [u8; 4] {
    let byte = |value: f32| value.round().clamp(0.0, 255.0) as u8;
    if a < 0.5 {
        return [0, 0, 0, 0];
    }
    let scale = 255.0 / a;
    [byte(r * scale), byte(g * scale), byte(b * scale), byte(a)]
}

/// The pixels from `from` to `to` along one side `size` pixels across, and
/// `reach` more either way
fn span(from: f32, to: f32, reach: f32, size: usize) -> std::ops::Range<usize> {
    let low = (from.min(to) - reach).floor().max(0.0) as usize;
    let high = ((from.max(to) + reach).ceil().max(0.0) as usize).min(size);
    low.min(high)..high
}

/// How far `point` is from the segment from `a` to `b`
fn from_segment(point: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (at, along) = ([point[0] - a[0], point[1] - a[1]], [b[0] - a[0], b[1] - a[1]]);
    let length = along[0] * along[0] + along[1] * along[1];
    let t = if length > 0.0 { ((at[0] * along[0] + at[1] * along[1]) / length).clamp(0.0, 1.0) } else { 0.0 };
    (at[0] - t * along[0]).hypot(at[1] - t * along[1])
}

/// Fills the onion-peel way, ring by ring in from the known pixels: each
/// pixel of `values` that isn't `known` becomes the average of its known or
/// already filled neighbors. Gives how many rings in each pixel is, 0 for
/// known ones
fn diffuse<const N: usize>(values: &mut [[f32; N]], known: &[bool], width: usize, height: usize) -> Vec<u32> {
    let mut depth: Vec<u32> = known.iter().map(|&known| if known { 0 } else { u32::MAX }).collect();
    let neighbors = |at: usize| {
        let (x, y) = ((at % width) as isize, (at / width) as isize);
        (-1isize..=1)
            .flat_map(move |dy| (-1isize..=1).map(move |dx| (x + dx, y + dy)))
            .filter(move |&(nx, ny)| nx >= 0 && ny >= 0 && (nx as usize) < width && (ny as usize) < height && (nx, ny) != (x, y))
            .map(move |(nx, ny)| ny as usize * width + nx as usize)
    };
    let mut ring: Vec<usize> =
        (0..known.len()).filter(|&at| !known[at] && neighbors(at).any(|neighbor| known[neighbor])).collect();
    let mut level = 1;
    while !ring.is_empty() {
        for &at in &ring {
            depth[at] = level;
        }
        let filled: Vec<[f32; N]> = ring
            .iter()
            .map(|&at| {
                let (mut sum, mut count) = ([0f32; N], 0f32);
                for neighbor in neighbors(at).filter(|&neighbor| depth[neighbor] < level) {
                    for (total, value) in sum.iter_mut().zip(values[neighbor]) {
                        *total += value;
                    }
                    count += 1.0;
                }
                sum.map(|total| if count > 0.0 { total / count } else { 0.0 })
            })
            .collect();
        for (&at, value) in ring.iter().zip(filled) {
            values[at] = value;
        }
        let mut next: Vec<usize> = ring.iter().flat_map(|&at| neighbors(at)).filter(|&at| depth[at] == u32::MAX).collect();
        next.sort_unstable();
        next.dedup();
        ring = next;
        level += 1;
    }
    depth
}

/// Patches are 7 pixels square
const RADIUS: isize = 3;
const PATCH: f32 = ((2 * RADIUS + 1) * (2 * RADIUS + 1)) as f32;
/// The coarsest level is no smaller than this across
const COARSEST: usize = 32;
/// How much more a patch one pixel nearer the image's own pixels counts
const CONFIDENCE: f32 = 1.3;
/// Past this many pixels to fill, a level is matched once, near where the
/// level below matched
const LARGE: usize = 1 << 21;

/// One level of the image, coarse to fine
struct Level {
    width: usize,
    height: usize,
    pixels: Vec<[f32; 4]>,
    known: Vec<bool>,
    /// Where it may take from, as painted
    sample: Vec<bool>,
}

impl Level {
    /// Half as large: a pixel is known only where all four are
    fn halved(&self) -> Level {
        let (width, height) = (self.width.div_ceil(2), self.height.div_ceil(2));
        let mut level = Level {
            width,
            height,
            pixels: vec![[0.0; 4]; width * height],
            known: vec![true; width * height],
            sample: vec![false; width * height],
        };
        for y in 0..height {
            for x in 0..width {
                let at = y * width + x;
                let (mut sum, mut count) = ([0f32; 4], 0f32);
                for (fx, fy) in [(2 * x, 2 * y), (2 * x + 1, 2 * y), (2 * x, 2 * y + 1), (2 * x + 1, 2 * y + 1)] {
                    if fx >= self.width || fy >= self.height {
                        continue;
                    }
                    let fine = fy * self.width + fx;
                    level.known[at] &= self.known[fine];
                    level.sample[at] |= self.sample[fine];
                    if self.known[fine] {
                        for (total, value) in sum.iter_mut().zip(self.pixels[fine]) {
                            *total += value;
                        }
                        count += 1.0;
                    }
                }
                if count > 0.0 {
                    level.pixels[at] = sum.map(|total| total / count);
                }
            }
        }
        level
    }

    /// Where patches may be taken from: centers whose whole patch is in
    /// the image, known and painted, or, if there are none, known painted
    /// centers, or known ones
    fn sources(&self) -> Vec<bool> {
        let (width, height) = (self.width, self.height);
        // How many unknown pixels are above and left of each, to count a
        // patch's in constant time
        let mut holes = vec![0u32; (width + 1) * (height + 1)];
        for y in 0..height {
            for x in 0..width {
                holes[(y + 1) * (width + 1) + x + 1] = u32::from(!self.known[y * width + x]) + holes[y * (width + 1) + x + 1]
                    + holes[(y + 1) * (width + 1) + x]
                    - holes[y * (width + 1) + x];
            }
        }
        let radius = RADIUS as usize;
        let mut sources = vec![false; width * height];
        if width > 2 * radius && height > 2 * radius {
            for y in radius..height - radius {
                for x in radius..width - radius {
                    let (x0, y0, x1, y1) = (x - radius, y - radius, x + radius + 1, y + radius + 1);
                    let count = holes[y1 * (width + 1) + x1] + holes[y0 * (width + 1) + x0]
                        - holes[y0 * (width + 1) + x1]
                        - holes[y1 * (width + 1) + x0];
                    sources[y * width + x] = count == 0 && self.sample[y * width + x];
                }
            }
        }
        if !sources.contains(&true) {
            sources = self.known.iter().zip(&self.sample).map(|(&known, &sample)| known && sample).collect();
        }
        if !sources.contains(&true) {
            sources = self.known.clone();
        }
        sources
    }
}

/// A small, fast random number generator, seeded so a fill comes out the
/// same each time
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// From `-reach` to `reach`
    fn within(&mut self, reach: usize) -> isize {
        (self.next() % (2 * reach as u64 + 1)) as isize - reach as isize
    }
}

/// What a level's matching needs to read
struct Matching<'a> {
    width: usize,
    height: usize,
    pixels: &'a [[f32; 4]],
    hole: &'a [bool],
    sources: &'a [bool],
}

impl Matching<'_> {
    /// How unlike the patch at `target` is the one at `source`: the mean
    /// squared difference of the pixels both have in the image, or
    /// infinity once it's past `best`
    fn distance(&self, target: usize, source: usize, best: f32) -> f32 {
        let (width, height) = (self.width as isize, self.height as isize);
        let (tx, ty) = ((target % self.width) as isize, (target / self.width) as isize);
        let (sx, sy) = ((source % self.width) as isize, (source / self.width) as isize);
        let limit = best * PATCH;
        let (mut sum, mut count) = (0f32, 0f32);
        for dy in -RADIUS..=RADIUS {
            let (ty, sy) = (ty + dy, sy + dy);
            if ty < 0 || ty >= height || sy < 0 || sy >= height {
                continue;
            }
            for dx in -RADIUS..=RADIUS {
                let (tx, sx) = (tx + dx, sx + dx);
                if tx < 0 || tx >= width || sx < 0 || sx >= width {
                    continue;
                }
                let (a, b) = (self.pixels[(ty * width + tx) as usize], self.pixels[(sy * width + sx) as usize]);
                sum += (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2) + (a[3] - b[3]).powi(2);
                count += 1.0;
            }
            if sum > limit {
                return f32::INFINITY;
            }
        }
        if count == 0.0 {
            f32::INFINITY
        } else {
            sum / count
        }
    }

    /// One pass of PatchMatch over rows `rows` of the field, whose matches
    /// and costs are `matches` and `costs` for those rows: each pixel to
    /// fill tries its neighbors' matches, moved over by one, then random
    /// ones around its best, from `reach` pixels away down to one.
    /// Backwards, the neighbors are those after it. Neighbors in other rows
    /// are as `before` has them, from before the pass
    #[allow(clippy::too_many_arguments)]
    fn search(
        &self,
        rows: std::ops::Range<usize>,
        matches: &mut [u32],
        costs: &mut [f32],
        before: &[u32],
        backwards: bool,
        reach: usize,
        random: &mut Random,
    ) {
        let width = self.width;
        let first = rows.start * width;
        let step: isize = if backwards { 1 } else { -1 };
        let in_rows = |y: isize| y >= rows.start as isize && y < rows.end as isize;
        let order: Box<dyn Iterator<Item = usize>> =
            if backwards { Box::new((rows.start * width..rows.end * width).rev()) } else { Box::new(rows.start * width..rows.end * width) };
        for at in order {
            if !self.hole[at] {
                continue;
            }
            let (x, y) = ((at % width) as isize, (at / width) as isize);
            let (mut best, mut cost) = (matches[at - first] as usize, costs[at - first]);
            let consider = |candidate: usize, best: &mut usize, cost: &mut f32| {
                if candidate == *best || !self.sources[candidate] {
                    return;
                }
                let distance = self.distance(at, candidate, *cost);
                if distance < *cost {
                    (*best, *cost) = (candidate, distance);
                }
            };
            for (nx, ny) in [(x + step, y), (x, y + step)] {
                if nx < 0 || nx >= width as isize || ny < 0 || ny >= self.height as isize {
                    continue;
                }
                let neighbor = ny as usize * width + nx as usize;
                if !self.hole[neighbor] {
                    continue;
                }
                let matched = if in_rows(ny) { matches[neighbor - first] } else { before[neighbor] } as usize;
                let (mx, my) = ((matched % width) as isize - (nx - x), (matched / width) as isize - (ny - y));
                if mx >= 0 && my >= 0 && (mx as usize) < width && (my as usize) < self.height {
                    consider(my as usize * width + mx as usize, &mut best, &mut cost);
                }
            }
            let mut reach = reach;
            while reach >= 1 {
                let (bx, by) = ((best % width) as isize, (best / width) as isize);
                let cx = (bx + random.within(reach)).clamp(0, width as isize - 1);
                let cy = (by + random.within(reach)).clamp(0, self.height as isize - 1);
                consider(cy as usize * width + cx as usize, &mut best, &mut cost);
                reach /= 2;
            }
            matches[at - first] = best as u32;
            costs[at - first] = cost;
        }
    }
}

/// Splits the rows of an image `height` tall among the threads
fn bands(height: usize) -> Vec<std::ops::Range<usize>> {
    let threads = std::thread::available_parallelism().map_or(4, |count| count.get()).clamp(1, 16);
    let rows = height.div_ceil(threads).max(8);
    (0..height).step_by(rows).map(|start| start..(start + rows).min(height)).collect()
}

/// A pass of PatchMatch over the whole field, a band of rows to a thread
fn search(matching: &Matching, matches: &mut [u32], costs: &mut [f32], pass: u64, reach: usize) {
    let width = matching.width;
    let bands = bands(matching.height);
    let before = matches.to_vec();
    let before = &before;
    std::thread::scope(|scope| {
        let (mut matches, mut costs) = (&mut matches[..], &mut costs[..]);
        for (index, rows) in bands.into_iter().enumerate() {
            let size = rows.len() * width;
            let (band_matches, rest_matches) = std::mem::take(&mut matches).split_at_mut(size);
            let (band_costs, rest_costs) = std::mem::take(&mut costs).split_at_mut(size);
            (matches, costs) = (rest_matches, rest_costs);
            scope.spawn(move || {
                let mut random = Random(0x9E37_79B9_7F4A_7C15 ^ (pass << 32) ^ index as u64 ^ 1);
                matching.search(rows, band_matches, band_costs, before, pass % 2 == 1, reach, &mut random);
            });
        }
    });
}

/// Each match's cost, for the pixels as they are now
fn costs(matching: &Matching, matches: &[u32]) -> Vec<f32> {
    let width = matching.width;
    let mut costs = vec![0f32; matches.len()];
    std::thread::scope(|scope| {
        let mut rest = &mut costs[..];
        for rows in bands(matching.height) {
            let (band, after) = std::mem::take(&mut rest).split_at_mut(rows.len() * width);
            rest = after;
            scope.spawn(move || {
                for (offset, cost) in band.iter_mut().enumerate() {
                    let at = rows.start * width + offset;
                    if matching.hole[at] {
                        *cost = matching.distance(at, matches[at] as usize, f32::INFINITY);
                    }
                }
            });
        }
    });
    costs
}

/// Each pixel to fill becomes the blend of what the patches over it
/// matched, each weighted by how well it matched and by `confidence`, how
/// near the image's own pixels its center is, so what's settled along the
/// edge carries inward, as Wexler et al. weigh it
fn vote(level: &mut Level, hole: &[bool], matches: &[u32], costs: &[f32], confidence: &[f32]) {
    let (width, height) = (level.width, level.height);
    let mut sorted: Vec<f32> = costs.iter().zip(hole).filter(|(cost, &hole)| hole && cost.is_finite()).map(|(&cost, _)| cost).collect();
    if sorted.is_empty() {
        return;
    }
    let spread = {
        let at = sorted.len() * 3 / 4;
        *sorted.select_nth_unstable_by(at, f32::total_cmp).1
    }
    .max(1.0);
    let weights: Vec<f32> = costs
        .iter()
        .zip(confidence)
        .map(|(&cost, &confidence)| if cost.is_finite() { (-cost / (2.0 * spread)).exp().max(1e-6) * confidence } else { 1e-9 })
        .collect();
    let pixels = &level.pixels;
    let mut voted = pixels.clone();
    std::thread::scope(|scope| {
        let mut rest = &mut voted[..];
        for rows in bands(height) {
            let (band, after) = std::mem::take(&mut rest).split_at_mut(rows.len() * width);
            rest = after;
            let weights = &weights;
            scope.spawn(move || {
                for (offset, out) in band.iter_mut().enumerate() {
                    let at = rows.start * width + offset;
                    if !hole[at] {
                        continue;
                    }
                    let (x, y) = ((at % width) as isize, (at / width) as isize);
                    let (mut sum, mut total) = ([0f32; 4], 0f32);
                    for dy in -RADIUS..=RADIUS {
                        let qy = y + dy;
                        if qy < 0 || qy >= height as isize {
                            continue;
                        }
                        for dx in -RADIUS..=RADIUS {
                            let qx = x + dx;
                            if qx < 0 || qx >= width as isize {
                                continue;
                            }
                            let patch = qy as usize * width + qx as usize;
                            if !hole[patch] {
                                continue;
                            }
                            let matched = matches[patch] as usize;
                            let (sx, sy) = ((matched % width) as isize - dx, (matched / width) as isize - dy);
                            if sx < 0 || sy < 0 || sx >= width as isize || sy >= height as isize {
                                continue;
                            }
                            let weight = weights[patch];
                            for (total, value) in sum.iter_mut().zip(pixels[sy as usize * width + sx as usize]) {
                                *total += weight * value;
                            }
                            total += weight;
                        }
                    }
                    if total > 0.0 {
                        *out = sum.map(|value| value / total);
                    }
                }
            });
        }
    });
    level.pixels = voted;
}

/// Fills the coarsest level ring by ring in from the image's own pixels, as
/// Resynthesizer's inside-out fill does: each pixel takes the middle of the
/// patch most like what's around it so far, known or filled before it, so
/// the fill starts out coherent for the rounds after to refine. Every
/// source is tried where there are few, and otherwise its neighbors'
/// matches moved over and a few hundred at random
fn onion(level: &mut Level, hole: &[bool], sources: &[bool], source_list: &[u32], matches: &mut [u32], random: &mut Random) {
    const TRIED: usize = 4096;
    let (width, height) = (level.width, level.height);
    let mut rings = vec![[0f32; 0]; width * height];
    let depth = diffuse(&mut rings, &level.known, width, height);
    let mut order: Vec<usize> = (0..width * height).filter(|&at| hole[at]).collect();
    order.sort_by_key(|&at| depth[at]);
    let mut filled = level.known.clone();
    let mut candidates = Vec::new();
    for at in order {
        let (x, y) = ((at % width) as isize, (at / width) as isize);
        candidates.clear();
        if source_list.len() <= TRIED {
            candidates.extend_from_slice(source_list);
        } else {
            candidates.extend((0..TRIED / 8).map(|_| source_list[(random.next() % source_list.len() as u64) as usize]));
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if (dx, dy) == (0, 0) || nx < 0 || ny < 0 || nx >= width as isize || ny >= height as isize {
                        continue;
                    }
                    let neighbor = ny as usize * width + nx as usize;
                    if !hole[neighbor] || !filled[neighbor] {
                        continue;
                    }
                    let matched = matches[neighbor] as usize;
                    let (mx, my) = ((matched % width) as isize - dx, (matched / width) as isize - dy);
                    if mx >= 0 && my >= 0 && (mx as usize) < width && (my as usize) < height && sources[my as usize * width + mx as usize] {
                        candidates.push((my as usize * width + mx as usize) as u32);
                    }
                }
            }
        }
        let (mut best, mut cost) = (candidates[0], f32::INFINITY);
        for &candidate in &candidates {
            let distance = filled_distance(level, &filled, at, candidate as usize, cost);
            if distance < cost {
                (best, cost) = (candidate, distance);
            }
        }
        matches[at] = best;
        level.pixels[at] = level.pixels[best as usize];
        filled[at] = true;
    }
}

/// How unlike the patch at `target` is the one at `source`, as
/// `Matching::distance`, over only the pixels `filled` so far
fn filled_distance(level: &Level, filled: &[bool], target: usize, source: usize, best: f32) -> f32 {
    let (width, height) = (level.width as isize, level.height as isize);
    let (tx, ty) = ((target % level.width) as isize, (target / level.width) as isize);
    let (sx, sy) = ((source % level.width) as isize, (source / level.width) as isize);
    let (mut sum, mut count) = (0f32, 0f32);
    for dy in -RADIUS..=RADIUS {
        let (ty, sy) = (ty + dy, sy + dy);
        if ty < 0 || ty >= height || sy < 0 || sy >= height {
            continue;
        }
        for dx in -RADIUS..=RADIUS {
            let (tx, sx) = (tx + dx, sx + dx);
            if tx < 0 || tx >= width || sx < 0 || sx >= width {
                continue;
            }
            let target = (ty * width + tx) as usize;
            if !filled[target] {
                continue;
            }
            let (a, b) = (level.pixels[target], level.pixels[(sy * width + sx) as usize]);
            sum += (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2) + (a[3] - b[3]).powi(2);
            count += 1.0;
        }
        if sum > best * PATCH {
            return f32::INFINITY;
        }
    }
    if count == 0.0 {
        f32::INFINITY
    } else {
        sum / count
    }
}

/// Fills what isn't known in `canvas` from what is, taking patches only
/// from where `sample` is set, if given, as Content-aware does. Stops with
/// `Failure::Cancelled` once `stopped`
fn fill(canvas: &mut Canvas, sample: Option<&[bool]>, stopped: &dyn Fn() -> bool) -> Result<(), Failure> {
    if !canvas.known.contains(&true) {
        return Err(Failure::Failed);
    }
    let mut levels = vec![Level {
        width: canvas.width,
        height: canvas.height,
        pixels: std::mem::take(&mut canvas.pixels),
        known: canvas.known.clone(),
        sample: sample.map_or_else(|| canvas.known.clone(), <[bool]>::to_vec),
    }];
    loop {
        let last = levels.last().unwrap();
        if last.width.min(last.height) < 2 * COARSEST {
            break;
        }
        let halved = last.halved();
        // Somewhere to take from, at least
        if !halved.known.contains(&true) {
            break;
        }
        levels.push(halved);
    }

    let mut previous: Option<(usize, Vec<u32>)> = None;
    let count = levels.len();
    for index in (0..count).rev() {
        let coarse_index = count - 1 - index;
        let (head, tail) = levels.split_at_mut(index + 1);
        let level = &mut head[index];
        let coarser = tail.first();
        let (width, height) = (level.width, level.height);
        let hole: Vec<bool> = level.known.iter().map(|&known| !known).collect();
        let sources = level.sources();
        let source_list: Vec<u32> = (0..sources.len()).filter(|&at| sources[at]).map(|at| at as u32).collect();
        let mut random = Random(0x2545_F491_4F6C_DD1D ^ index as u64);
        let any_source = |random: &mut Random| source_list[(random.next() % source_list.len() as u64) as usize];

        let mut matches = vec![0u32; width * height];
        match (coarser, previous.take()) {
            (Some(coarser), Some((coarser_width, coarser_matches))) => {
                // Up from the level below: its matches, twice as far, and its
                // pixels, until they're voted
                for y in 0..height {
                    for x in 0..width {
                        let at = y * width + x;
                        if !hole[at] {
                            continue;
                        }
                        let below = (y / 2).min(coarser.height - 1) * coarser_width + (x / 2).min(coarser_width - 1);
                        level.pixels[at] = coarser.pixels[below];
                        let matched = coarser_matches[below] as usize;
                        let mx = ((matched % coarser_width) * 2 + x % 2).min(width - 1);
                        let my = ((matched / coarser_width) * 2 + y % 2).min(height - 1);
                        let candidate = my * width + mx;
                        matches[at] = if sources[candidate] { candidate as u32 } else { any_source(&mut random) };
                    }
                }
            }
            _ => onion(level, &hole, &sources, &source_list, &mut matches, &mut random),
        }

        let confidence: Vec<f32> = {
            let mut rings = vec![[0f32; 0]; width * height];
            diffuse(&mut rings, &level.known, width, height).into_iter().map(|depth| CONFIDENCE.powi(-(depth.min(200) as i32))).collect()
        };
        let holes = hole.iter().filter(|&&hole| hole).count();
        let large = holes > LARGE;
        let rounds = if large { 1 } else if coarse_index == 0 { 16 } else { (8 - 2 * coarse_index as i32).max(2) as usize };
        let passes = if large { 1 } else if coarse_index <= 1 { 4 } else { 2 };
        let reach = if coarse_index <= 1 { width.max(height) } else { 8 };
        // The pixels voted from the level below first, so they're matched
        // against something like what they'll be
        if coarser.is_some() {
            let matching = Matching { width, height, pixels: &level.pixels, hole: &hole, sources: &sources };
            let costs = costs(&matching, &matches);
            vote(level, &hole, &matches, &costs, &confidence);
        }
        for round in 0..rounds {
            if stopped() {
                return Err(Failure::Cancelled);
            }
            let matching = Matching { width, height, pixels: &level.pixels, hole: &hole, sources: &sources };
            let mut costs = costs(&matching, &matches);
            for pass in 0..passes {
                search(&matching, &mut matches, &mut costs, (round * passes + pass) as u64, reach);
            }
            vote(level, &hole, &matches, &costs, &confidence);
        }
        previous = Some((width, matches));
    }
    let finest = levels.swap_remove(0);
    canvas.pixels = finest.pixels;
    // Rounding aside, the image's own pixels stay as they were
    Ok(())
}

/// Generative: LaMa fills the grown image a 512 pixel square at a time,
/// at a scale where what's new is no deeper than it can see across, then
/// its fill is scaled back up and evened out into the image's own colors
/// along the edge
#[cfg(birefnet)]
mod lama {
    use std::path::Path;
    use std::sync::Arc;

    use image::imageops::{self, FilterType};
    use image::{GrayImage, Luma, Rgb, RgbImage};
    use ort::session::builder::AutoDevicePolicy;
    use ort::session::{RunOptions, Session};
    use ort::value::TensorRef;
    use tauri::AppHandle;

    use super::{diffuse, failed, unpremultiply, Canvas};
    use crate::models::{Missing, EXPAND, LAMA};

    /// LaMa takes a square this size
    const SIDE: usize = 512;
    /// The deepest new pixel is at most this far from the image, in LaMa's
    /// pixels, so each square has the image to go on
    const DEPTH: f32 = 160.0;
    /// Squares overlap, and only their middle, this far in from an edge
    /// inside the image, is kept, where LaMa saw around it
    const STEP: usize = 384;
    const MARGIN: usize = 40;

    pub fn fill(app: &AppHandle, tab: u32, job: u64, canvas: &mut Canvas, chosen: bool) -> Result<(), &'static str> {
        let ensured = if chosen { EXPAND.ensure_chosen(app, tab, job) } else { EXPAND.ensure(app, tab, job) };
        let (_held, folder) = ensured.map_err(|missing| match missing {
            Missing::Cancelled => "cancelled",
            Missing::Failed => "failed",
        })?;
        let options = Arc::new(RunOptions::new().map_err(failed)?);
        let terminate = options.clone();
        crate::handoff::on_cancel(tab, job, move || {
            let _ = terminate.terminate();
        });
        let stopped = || crate::handoff::cancelled(tab, job);
        let progress = |text: &str| crate::handoff::progress(app, tab, job, text);
        run(&folder.join(LAMA.file), Some(&folder), canvas, &options, &stopped, &progress).map_err(|error| {
            if stopped() {
                "cancelled"
            } else {
                failed(error)
            }
        })
    }

    /// Fills `canvas` with the model at `model`, ONNX Runtime from `folder`
    /// where it's downloaded, or as linked in without one, saying how far
    /// it's got with `progress`
    pub fn run(
        model: &Path,
        folder: Option<&Path>,
        canvas: &mut Canvas,
        options: &RunOptions,
        stopped: &dyn Fn() -> bool,
        progress: &dyn Fn(&str),
    ) -> Result<(), String> {
        let environment = crate::models::environment(folder.unwrap_or(Path::new("")))?;
        let mut session = Session::builder(environment)
            .map_err(|error| error.to_string())?
            .with_auto_device(AutoDevicePolicy::PreferCPU)
            .map_err(|error| error.to_string())?
            .commit_from_file(model)
            .map_err(|error| error.to_string())?;
        let plane = SIDE * SIDE;
        squares(canvas, DEPTH, stopped, &mut |input, mask, index, total| {
            if total > 1 {
                progress(&format!("Expanding… {}%", index * 100 / total));
            }
            let image = TensorRef::from_array_view(([1usize, 3, SIDE, SIDE], input)).map_err(|error| error.to_string())?;
            let holes = TensorRef::from_array_view(([1usize, 1, SIDE, SIDE], mask)).map_err(|error| error.to_string())?;
            let outputs = session.run_with_options(ort::inputs!["image" => image, "mask" => holes], options).map_err(|error| error.to_string())?;
            let (_, output) = outputs[0].try_extract_tensor::<f32>().map_err(|error| error.to_string())?;
            if output.len() != 3 * plane {
                return Err("unexpected output".to_owned());
            }
            // 0 to 255, though some exports give 0 to 1
            let range = if output.iter().copied().fold(0f32, f32::max) <= 1.5 { 255.0 } else { 1.0 };
            Ok(output.iter().map(|value| value * range).collect())
        })
    }

    /// What a model is given for a square: the image in it, 0 to 1, channel
    /// by channel, blank where it's new, what's new, as 1, a little more of
    /// it than is, so the edge it blends into is the image's, which square
    /// it is and how many there are at most. It gives the square filled, 0
    /// to 255, channel by channel
    pub(super) type Fill<'a> = dyn FnMut(&[f32], &[f32], usize, usize) -> Result<Vec<f32>, String> + 'a;

    /// Fills `canvas` a 512 pixel square at a time with `fill`, at a scale
    /// where what's new is no deeper than `depth` of the square's pixels,
    /// up to the image's own size, and no smaller than a square. Squares
    /// overlap, those with the most to go on first, and only their middle
    /// is kept where they're inside the image. The fill is scaled back up
    /// and evened out into the image's own colors along the edge
    pub(super) fn squares(canvas: &mut Canvas, depth: f32, stopped: &dyn Fn() -> bool, fill: &mut Fill) -> Result<(), String> {
        let (width, height) = (canvas.width, canvas.height);
        let deepest = {
            let mut rings = vec![[0f32; 0]; width * height];
            let depth = diffuse(&mut rings, &canvas.known, width, height);
            depth.iter().zip(&canvas.known).filter(|(_, &known)| !known).map(|(&depth, _)| depth).max().unwrap_or(1).max(1)
        };
        let scale = (depth / deepest as f32).min(1.0).max(SIDE as f32 / width.min(height) as f32);
        let across = ((width as f32 * scale).ceil() as usize).max(SIDE);
        let down = ((height as f32 * scale).ceil() as usize).max(SIDE);

        let mut picture = RgbImage::new(width as u32, height as u32);
        let mut holes = GrayImage::new(width as u32, height as u32);
        for (at, (&pixel, &known)) in canvas.pixels.iter().zip(&canvas.known).enumerate() {
            let (x, y) = ((at % width) as u32, (at / width) as u32);
            if known {
                let [r, g, b, _] = unpremultiply(pixel);
                picture.put_pixel(x, y, Rgb([r, g, b]));
            } else {
                holes.put_pixel(x, y, Luma([255]));
            }
        }
        let mut work = imageops::resize(&picture, across as u32, down as u32, FilterType::Triangle);
        let small_holes = imageops::resize(&holes, across as u32, down as u32, FilterType::Triangle);
        drop((picture, holes));
        // Known, or filled by a square before: any of it new is new
        let mut filled: Vec<bool> = small_holes.pixels().map(|pixel| pixel[0] == 0).collect();

        let starts = |size: usize| {
            let mut starts: Vec<usize> = (0..size.saturating_sub(SIDE)).step_by(STEP).collect();
            starts.push(size - SIDE);
            starts.dedup();
            starts
        };
        let mut squares: Vec<(usize, usize)> = starts(down)
            .into_iter()
            .flat_map(|y| starts(across).into_iter().map(move |x| (x, y)))
            .filter(|&(x, y)| (y..y + SIDE).any(|row| filled[row * across + x..row * across + x + SIDE].contains(&false)))
            .collect();
        let kept = |(x, y): (usize, usize)| {
            let left = if x > 0 { x + MARGIN } else { 0 };
            let top = if y > 0 { y + MARGIN } else { 0 };
            let right = if x + SIDE < across { x + SIDE - MARGIN } else { across };
            let bottom = if y + SIDE < down { y + SIDE - MARGIN } else { down };
            (left..right, top..bottom)
        };
        let unfilled = |filled: &[bool], square: (usize, usize)| {
            let (columns, rows) = kept(square);
            rows.into_iter().any(|row| filled[row * across + columns.start..row * across + columns.end].contains(&false))
        };

        let plane = SIDE * SIDE;
        let total = squares.len();
        let mut index = 0;
        loop {
            squares.retain(|&square| unfilled(&filled, square));
            // The square with the most to go on first
            let Some(&(x, y)) = squares.iter().max_by_key(|&&(x, y)| {
                (y..y + SIDE).map(|row| filled[row * across + x..row * across + x + SIDE].iter().filter(|&&known| known).count()).sum::<usize>()
            }) else {
                break;
            };
            if stopped() {
                return Err("cancelled".to_owned());
            }
            let mut input = vec![0f32; 3 * plane];
            let mut mask = vec![0f32; plane];
            for row in 0..SIDE {
                for column in 0..SIDE {
                    let (at, from) = (row * SIDE + column, (y + row) * across + x + column);
                    if filled[from] {
                        let pixel = work.get_pixel((x + column) as u32, (y + row) as u32);
                        for channel in 0..3 {
                            input[channel * plane + at] = pixel[channel] as f32 / 255.0;
                        }
                    } else {
                        mask[at] = 1.0;
                    }
                }
            }
            let mask = grown(&mask, 2);
            for (at, &hole) in mask.iter().enumerate() {
                if hole > 0.5 {
                    for channel in 0..3 {
                        input[channel * plane + at] = 0.0;
                    }
                }
            }
            let output = fill(&input, &mask, index, total.max(index + 1))?;
            index += 1;
            let (columns, rows) = kept((x, y));
            for row in rows {
                for column in columns.clone() {
                    let from = row * across + column;
                    if filled[from] {
                        continue;
                    }
                    let at = (row - y) * SIDE + column - x;
                    let pixel = [0, 1, 2].map(|channel| output[channel * plane + at].round().clamp(0.0, 255.0) as u8);
                    work.put_pixel(column as u32, row as u32, Rgb(pixel));
                    filled[from] = true;
                }
            }
        }

        let large = imageops::resize(&work, width as u32, height as u32, FilterType::CatmullRom);
        drop(work);
        blend(canvas, &large, scale);
        Ok(())
    }

    /// `mask` grown by `by` pixels
    pub(super) fn grown(mask: &[f32], by: usize) -> Vec<f32> {
        let mut out = mask.to_vec();
        for y in 0..SIDE {
            for x in 0..SIDE {
                if mask[y * SIDE + x] < 0.5 {
                    continue;
                }
                for ny in y.saturating_sub(by)..(y + by + 1).min(SIDE) {
                    for nx in x.saturating_sub(by)..(x + by + 1).min(SIDE) {
                        out[ny * SIDE + nx] = 1.0;
                    }
                }
            }
        }
        out
    }

    /// The fill LaMa gave, scaled up from `scale`, put where the canvas is
    /// to be filled, with how far its colors are off from the image's along
    /// the edge taken away, less so farther in
    fn blend(canvas: &mut Canvas, large: &RgbImage, scale: f32) {
        let (width, height) = (canvas.width, canvas.height);
        let mut offset = vec![[0f32; 3]; width * height];
        for (at, &known) in canvas.known.iter().enumerate() {
            if known {
                let [r, g, b, _] = unpremultiply(canvas.pixels[at]);
                let model = large.get_pixel((at % width) as u32, (at / width) as u32);
                offset[at] = [r as f32 - model[0] as f32, g as f32 - model[1] as f32, b as f32 - model[2] as f32];
            }
        }
        let depth = diffuse(&mut offset, &canvas.known, width, height);
        let fade = (6.0 / scale).max(6.0);
        for (at, &known) in canvas.known.iter().enumerate() {
            if known {
                continue;
            }
            let model = large.get_pixel((at % width) as u32, (at / width) as u32);
            let kept = (1.0 - depth[at] as f32 / fade).max(0.0);
            let [r, g, b] = [0, 1, 2].map(|channel| (model[channel] as f32 + offset[at][channel] * kept).clamp(0.0, 255.0));
            canvas.pixels[at] = [r, g, b, 255.0];
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Runs LaMa from `LIGMA_LAMA`, a path to lama_fp32.onnx, on
        /// `LIGMA_EXPAND_IMAGE`, grown as `LIGMA_EXPAND_GROW` says, "left,
        /// top,right,bottom" in fractions of it, and writes it next to the
        /// image. `cargo test --release -- --ignored lama`
        #[test]
        #[ignore]
        fn lama_grows_an_image() {
            let (Ok(model), Ok(path)) = (std::env::var("LIGMA_LAMA"), std::env::var("LIGMA_EXPAND_IMAGE")) else {
                return;
            };
            let grow = std::env::var("LIGMA_EXPAND_GROW").unwrap_or_else(|_| "0,0.1,0.35,0.1".to_owned());
            let [left, top, right, bottom]: [f32; 4] = grow.split(',').map(|side| side.trim().parse().unwrap()).collect::<Vec<_>>().try_into().unwrap();
            let mut image = image::open(&path).unwrap().into_rgba8();
            if let Ok(times) = std::env::var("LIGMA_EXPAND_UPSCALE") {
                let times: f32 = times.parse().unwrap();
                let (width, height) = ((image.width() as f32 * times) as u32, (image.height() as f32 * times) as u32);
                image = imageops::resize(&image, width, height, FilterType::CatmullRom);
            }
            let request = super::super::Request { method: "generative".into(), left, top, right, bottom, sample: vec![] };
            let mut canvas = Canvas::grown(&image, &request).unwrap();
            let options = RunOptions::new().unwrap();
            let started = std::time::Instant::now();
            run(Path::new(&model), None, &mut canvas, &options, &|| false, &|text| eprintln!("{text}")).unwrap();
            eprintln!("{}x{} in {:?}", canvas.width, canvas.height, started.elapsed());
            std::fs::write(format!("{path}.generative.png"), canvas.png().unwrap()).unwrap();
        }
    }
}

/// Generative with Stable Diffusion 1.5 Inpainting, which draws what's new
/// rather than carrying the image on: 512 pixel squares of the grown image
/// (see `lama::squares`) are each denoised over `STEPS` steps of DDIM with
/// what's new masked, without a prompt, then decoded
#[cfg(birefnet)]
mod diffusion {
    use std::path::Path;
    use std::sync::Arc;

    use ort::session::builder::AutoDevicePolicy;
    use ort::session::{RunOptions, Session};
    use ort::value::{Tensor, TensorElementType, TensorRef};
    use tauri::AppHandle;

    use super::lama::squares;
    use super::{failed, Canvas};
    use crate::models::{Missing, DIFFUSION, DIFFUSION_DECODER, DIFFUSION_ENCODER, DIFFUSION_UNET};

    /// What the text encoder makes of an empty prompt, 77 tokens of 768, as
    /// half floats, so its 492 MB aren't needed (see `empty_prompt_encodes`)
    const EMPTY_PROMPT: &[u8] = include_bytes!("../assets/empty-prompt.f16");
    const TOKENS: usize = 77;
    const WIDTH: usize = 768;
    const STEPS: usize = 25;
    /// It draws 512 pixel squares, as latents 64 across
    const SIDE: usize = 512;
    const LATENT: usize = 64;
    /// What's new is at most this many of a square's pixels from the image,
    /// as it draws, so it has the image to go on and room to draw
    const DEPTH: f32 = 320.0;
    /// How the VAE's latents are scaled for the UNet
    const LATENT_SCALE: f32 = 0.18215;

    pub fn fill(app: &AppHandle, tab: u32, job: u64, canvas: &mut Canvas, chosen: bool) -> Result<(), &'static str> {
        let ensured = if chosen { DIFFUSION.ensure_chosen(app, tab, job) } else { DIFFUSION.ensure(app, tab, job) };
        let (_held, folder) = ensured.map_err(|missing| match missing {
            Missing::Cancelled => "cancelled",
            Missing::Failed => "failed",
        })?;
        let options = Arc::new(RunOptions::new().map_err(failed)?);
        let terminate = options.clone();
        crate::handoff::on_cancel(tab, job, move || {
            let _ = terminate.terminate();
        });
        let stopped = || crate::handoff::cancelled(tab, job);
        let progress = |text: &str| crate::handoff::progress(app, tab, job, text);
        let files = [DIFFUSION_UNET.file, DIFFUSION_ENCODER.file, DIFFUSION_DECODER.file].map(|file| folder.join(file));
        run([&files[0], &files[1], &files[2]], Some(&folder), canvas, &options, &stopped, &progress).map_err(|error| {
            if stopped() {
                "cancelled"
            } else {
                failed(error)
            }
        })
    }

    fn session(model: &Path, folder: Option<&Path>) -> Result<Session, String> {
        let environment = crate::models::environment(folder.unwrap_or(Path::new("")))?;
        Session::builder(environment)
            .map_err(|error| error.to_string())?
            .with_auto_device(AutoDevicePolicy::PreferCPU)
            .map_err(|error| error.to_string())?
            .commit_from_file(model)
            .map_err(|error| error.to_string())
    }

    /// A half float's value
    fn half(bits: u16) -> f32 {
        let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
        let exponent = (bits >> 10) & 0x1f;
        let fraction = (bits & 0x3ff) as f32;
        sign * match exponent {
            0 => fraction * 2f32.powi(-24),
            31 => f32::INFINITY,
            _ => (1.0 + fraction / 1024.0) * 2f32.powi(exponent as i32 - 15),
        }
    }

    /// Normally distributed numbers, seeded so an image comes out the same
    /// each time
    struct Noise(u64);

    impl Noise {
        fn uniform(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            ((self.0 >> 40) as f32 + 0.5) / (1u64 << 24) as f32
        }

        fn normal(&mut self) -> f32 {
            let (a, b) = (self.uniform(), self.uniform());
            (-2.0 * a.ln()).sqrt() * (std::f32::consts::TAU * b).cos()
        }
    }

    /// The noise schedule SD 1.5 was trained with: how much of the image is
    /// left at each of the 1,000 steps
    fn alphas() -> Vec<f32> {
        let (start, end) = (0.00085f64.sqrt(), 0.012f64.sqrt());
        let mut product = 1.0f64;
        (0..1000)
            .map(|step| {
                let beta = (start + (end - start) * step as f64 / 999.0).powi(2);
                product *= 1.0 - beta;
                product as f32
            })
            .collect()
    }

    /// Fills `canvas` with the UNet, VAE encoder and decoder in `models`,
    /// ONNX Runtime from `folder` where it's downloaded, or as linked in
    /// without one, saying how far it's got with `progress`
    pub fn run(
        models: [&Path; 3],
        folder: Option<&Path>,
        canvas: &mut Canvas,
        options: &RunOptions,
        stopped: &dyn Fn() -> bool,
        progress: &dyn Fn(&str),
    ) -> Result<(), String> {
        let error = |error: ort::Error| error.to_string();
        let prompt: Vec<f32> = EMPTY_PROMPT.as_chunks::<2>().0.iter().map(|&bytes| half(u16::from_le_bytes(bytes))).collect();
        if prompt.len() != TOKENS * WIDTH {
            return Err("unexpected prompt".to_owned());
        }
        progress("Expanding… 0%");
        let mut encoder = session(models[1], folder)?;
        let mut unet = session(models[0], folder)?;
        let mut decoder = session(models[2], folder)?;
        let timestep_type = unet.inputs().iter().find(|input| input.name() == "timestep").and_then(|input| input.dtype().tensor_type());
        let alphas = alphas();
        // DDIM's steps, spaced from the start, offset by one as SD 1.5's are
        let ratio = 1000 / STEPS;
        let steps: Vec<usize> = (0..STEPS).rev().map(|step| step * ratio + 1).collect();
        let (plane, cells) = (SIDE * SIDE, LATENT * LATENT);

        squares(canvas, DEPTH, stopped, &mut |input, mask, index, total| {
            // The image, -1 to 1, blank where it's new
            let masked: Vec<f32> = input.iter().enumerate().map(|(at, &value)| if mask[at % plane] > 0.5 { 0.0 } else { value * 2.0 - 1.0 }).collect();
            // What's new, in the latents' cells: any of a cell new is new
            let mut cells_mask = vec![0f32; cells];
            for (at, &hole) in mask.iter().enumerate() {
                if hole > 0.5 {
                    cells_mask[(at / SIDE / 8) * LATENT + (at % SIDE) / 8] = 1.0;
                }
            }
            let masked_latents: Vec<f32> = {
                let input = TensorRef::from_array_view(([1usize, 3, SIDE, SIDE], &*masked)).map_err(error)?;
                let outputs = encoder.run_with_options(ort::inputs![input], options).map_err(error)?;
                let (_, latents) = outputs[0].try_extract_tensor::<f32>().map_err(error)?;
                if latents.len() != 4 * cells {
                    return Err("unexpected encoder output".to_owned());
                }
                latents.iter().map(|value| value * LATENT_SCALE).collect()
            };
            let mut noise = Noise(0x5DEE_CE66_D1CE_4E5B ^ index as u64);
            let mut latents: Vec<f32> = (0..4 * cells).map(|_| noise.normal()).collect();
            let mut sample = vec![0f32; 9 * cells];
            for (number, &step) in steps.iter().enumerate() {
                if stopped() {
                    return Err("cancelled".to_owned());
                }
                sample[..4 * cells].copy_from_slice(&latents);
                sample[4 * cells..5 * cells].copy_from_slice(&cells_mask);
                sample[5 * cells..].copy_from_slice(&masked_latents);
                let input = TensorRef::from_array_view(([1usize, 9, LATENT, LATENT], &*sample)).map_err(error)?;
                let hidden = TensorRef::from_array_view(([1usize, TOKENS, WIDTH], &*prompt)).map_err(error)?;
                let outputs = match timestep_type {
                    Some(TensorElementType::Float32) => {
                        let timestep = Tensor::from_array(([1usize], vec![step as f32])).map_err(error)?;
                        unet.run_with_options(ort::inputs!["sample" => input, "timestep" => timestep, "encoder_hidden_states" => hidden], options)
                    }
                    _ => {
                        let timestep = Tensor::from_array(([1usize], vec![step as i64])).map_err(error)?;
                        unet.run_with_options(ort::inputs!["sample" => input, "timestep" => timestep, "encoder_hidden_states" => hidden], options)
                    }
                }
                .map_err(error)?;
                let (_, predicted) = outputs[0].try_extract_tensor::<f32>().map_err(error)?;
                if predicted.len() != 4 * cells {
                    return Err("unexpected UNet output".to_owned());
                }
                // DDIM, without noise: the image it predicts, then back to
                // the next step's noise level
                let alpha = alphas[step];
                let previous = step.checked_sub(ratio).map_or(alphas[0], |previous| alphas[previous]);
                for (latent, &noise) in latents.iter_mut().zip(predicted.iter()) {
                    let original = (*latent - (1.0 - alpha).sqrt() * noise) / alpha.sqrt();
                    *latent = previous.sqrt() * original + (1.0 - previous).sqrt() * noise;
                }
                progress(&format!("Expanding… {}%", (index * STEPS + number + 1) * 95 / (total * STEPS)));
            }
            let scaled: Vec<f32> = latents.iter().map(|value| value / LATENT_SCALE).collect();
            let input = TensorRef::from_array_view(([1usize, 4, LATENT, LATENT], &*scaled)).map_err(error)?;
            let outputs = decoder.run_with_options(ort::inputs![input], options).map_err(error)?;
            let (_, image) = outputs[0].try_extract_tensor::<f32>().map_err(error)?;
            if image.len() != 3 * plane {
                return Err("unexpected decoder output".to_owned());
            }
            Ok(image.iter().map(|value| (value + 1.0) * 127.5).collect())
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn halves_read_as_floats() {
            assert_eq!(half(0x3c00), 1.0);
            assert_eq!(half(0xc000), -2.0);
            assert_eq!(half(0x0000), 0.0);
            assert!((half(0x3555) - 0.333).abs() < 1e-3);
        }

        #[test]
        fn the_built_in_prompt_is_whole() {
            assert_eq!(EMPTY_PROMPT.len(), TOKENS * WIDTH * 2);
        }

        /// Runs Stable Diffusion from `LIGMA_DIFFUSION`, a folder with
        /// unet/model.onnx, vae_encoder/model.onnx and
        /// vae_decoder/model.onnx, on `LIGMA_EXPAND_IMAGE`, grown as
        /// `LIGMA_EXPAND_GROW` says, and writes it next to the image.
        /// `cargo test --release -- --ignored diffusion_grows`
        #[test]
        #[ignore]
        fn diffusion_grows_an_image() {
            let (Ok(folder), Ok(path)) = (std::env::var("LIGMA_DIFFUSION"), std::env::var("LIGMA_EXPAND_IMAGE")) else {
                return;
            };
            let folder = Path::new(&folder);
            let grow = std::env::var("LIGMA_EXPAND_GROW").unwrap_or_else(|_| "0,0,0,0.35".to_owned());
            let [left, top, right, bottom]: [f32; 4] =
                grow.split(',').map(|side| side.trim().parse().unwrap()).collect::<Vec<_>>().try_into().unwrap();
            let image = image::open(&path).unwrap().into_rgba8();
            let request = super::super::Request { method: "generative".into(), left, top, right, bottom, sample: vec![] };
            let mut canvas = Canvas::grown(&image, &request).unwrap();
            let options = RunOptions::new().unwrap();
            let files = ["unet/model.onnx", "vae_encoder/model.onnx", "vae_decoder/model.onnx"].map(|file| folder.join(file));
            let started = std::time::Instant::now();
            run([&files[0], &files[1], &files[2]], None, &mut canvas, &options, &|| false, &|text| eprintln!("{text} {:?}", started.elapsed()))
                .unwrap();
            eprintln!("{}x{} in {:?}", canvas.width, canvas.height, started.elapsed());
            std::fs::write(format!("{path}.diffusion.png"), canvas.png().unwrap()).unwrap();
        }

        /// Writes what the text encoder in `LIGMA_TEXT_ENCODER` makes of an
        /// empty prompt to assets/empty-prompt.f16: SD 1.5's tokenizer
        /// gives the start token, then the end token, which pads it too.
        /// `cargo test --release -- --ignored empty_prompt_encodes`
        #[test]
        #[ignore]
        fn empty_prompt_encodes() {
            let Ok(model) = std::env::var("LIGMA_TEXT_ENCODER") else {
                return;
            };
            let mut encoder = session(Path::new(&model), None).unwrap();
            let kind = encoder.inputs()[0].dtype().tensor_type();
            let mut ids = vec![49407i64; TOKENS];
            ids[0] = 49406;
            let outputs = if kind == Some(TensorElementType::Int32) {
                let ids: Vec<i32> = ids.iter().map(|&id| id as i32).collect();
                encoder.run(ort::inputs![Tensor::from_array(([1usize, TOKENS], ids)).unwrap()]).unwrap()
            } else {
                encoder.run(ort::inputs![Tensor::from_array(([1usize, TOKENS], ids)).unwrap()]).unwrap()
            };
            let (_, hidden) = outputs[0].try_extract_tensor::<f32>().unwrap();
            assert_eq!(hidden.len(), TOKENS * WIDTH);
            let mut bytes = Vec::with_capacity(hidden.len() * 2);
            for &value in hidden.iter() {
                bytes.extend_from_slice(&to_half(value).to_le_bytes());
            }
            std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/empty-prompt.f16"), bytes).unwrap();
        }

        /// The nearest half float to `value`, for the values a text encoder
        /// gives
        fn to_half(value: f32) -> u16 {
            let bits = value.to_bits();
            let sign = ((bits >> 16) & 0x8000) as u16;
            let exponent = ((bits >> 23) & 0xff) as i32 - 127 + 15;
            let mantissa = bits & 0x7f_ffff;
            if exponent <= 0 {
                if exponent < -10 {
                    return sign;
                }
                let mantissa = (mantissa | 0x80_0000) >> (1 - exponent);
                return sign | ((mantissa + 0x1000) >> 13) as u16;
            }
            if exponent >= 31 {
                return sign | 0x7c00;
            }
            let rounded = ((exponent as u32) << 10 | (mantissa >> 13)) + ((mantissa >> 12) & 1);
            sign | rounded as u16
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(left: f32, top: f32, right: f32, bottom: f32) -> Request {
        Request { method: "content".into(), left, top, right, bottom, sample: vec![] }
    }

    fn solid(width: u32, height: u32, color: [u8; 4]) -> image::RgbaImage {
        image::RgbaImage::from_pixel(width, height, image::Rgba(color))
    }

    #[test]
    fn grows_by_fractions_of_the_image() {
        let canvas = Canvas::grown(&solid(100, 50, [10, 20, 30, 255]), &request(0.1, 0.2, 0.5, 0.0)).unwrap();
        assert_eq!((canvas.width, canvas.height, canvas.left, canvas.top), (160, 60, 10, 10));
        assert!(canvas.known[10 * 160 + 10]);
        assert!(!canvas.known[0]);
        assert_eq!(canvas.known.iter().filter(|&&known| known).count(), 100 * 50);
    }

    #[test]
    fn not_growing_is_empty() {
        assert_eq!(Canvas::grown(&solid(10, 10, [0; 4]), &request(0.0, 0.0, 0.0, 0.0)).err(), Some("empty"));
        assert_eq!(Canvas::grown(&solid(10, 10, [0; 4]), &request(-1.0, 0.0, 0.01, 0.0)).err(), Some("empty"));
    }

    #[test]
    fn too_large_fails() {
        assert_eq!(Canvas::grown(&solid(4096, 4096, [0; 4]), &request(0.5, 0.0, 0.0, 0.0)).err(), Some("failed"));
    }

    #[test]
    fn strokes_paint_where_to_take_from() {
        let canvas = Canvas::grown(&solid(100, 100, [0, 0, 0, 255]), &request(0.0, 0.0, 0.5, 0.0)).unwrap();
        assert!(canvas.sample(&[], 100, 100).is_none());
        let strokes = [
            Stroke { points: vec![[0.1, 0.5], [0.9, 0.5]], radius: 0.05, subtract: false },
            Stroke { points: vec![[0.5, 0.5]], radius: 0.1, subtract: true },
        ];
        let sample = canvas.sample(&strokes, 100, 100).unwrap();
        assert!(sample[50 * 150 + 20]);
        // Taken away in the middle, and never outside the image
        assert!(!sample[50 * 150 + 50]);
        assert!(!sample[50 * 150 + 120]);
        assert!(!sample[10 * 150 + 20]);
        // Taking away everything leaves all of it to take from
        let none = [Stroke { points: vec![[0.5, 0.5]], radius: 0.1, subtract: true }];
        assert!(canvas.sample(&none, 100, 100).is_none());
    }

    #[test]
    fn transparency_survives_premultiplying() {
        for color in [[255, 128, 0, 255], [10, 200, 30, 128], [0, 0, 0, 0]] {
            assert_eq!(unpremultiply(premultiply(color)), color);
        }
    }

    #[test]
    fn diffusing_fills_from_the_edge_in() {
        let (width, height) = (5, 1);
        let mut values = vec![[10f32], [0.0], [0.0], [0.0], [30.0]];
        let known = [true, false, false, false, true];
        let depth = diffuse(&mut values, &known, width, height);
        assert_eq!(depth, [0, 1, 2, 1, 0]);
        assert_eq!(values[1][0], 10.0);
        assert_eq!(values[3][0], 30.0);
        assert_eq!(values[2][0], 20.0);
    }

    #[test]
    fn a_flat_color_grows_as_itself() {
        let mut canvas = Canvas::grown(&solid(80, 60, [40, 120, 200, 255]), &request(0.25, 0.25, 0.5, 0.25)).unwrap();
        fill(&mut canvas, None, &|| false).unwrap();
        for &pixel in &canvas.pixels {
            assert_eq!(unpremultiply(pixel), [40, 120, 200, 255]);
        }
    }

    /// Vertical stripes, 6 pixels each, grown to the right, carry on as
    /// stripes: each new column is close to one color all the way down
    #[test]
    fn stripes_carry_on() {
        let image = image::RgbaImage::from_fn(96, 96, |x, _| if (x / 6) % 2 == 0 { image::Rgba([230, 40, 40, 255]) } else { image::Rgba([20, 20, 220, 255]) });
        let mut canvas = Canvas::grown(&image, &request(0.0, 0.0, 0.5, 0.0)).unwrap();
        let original = canvas.pixels.clone();
        fill(&mut canvas, None, &|| false).unwrap();
        let width = canvas.width;
        for (at, &known) in canvas.known.iter().enumerate() {
            if known {
                assert_eq!(canvas.pixels[at], original[at]);
            }
        }
        let mut alike = 0;
        for x in 96..144 {
            let column: Vec<[u8; 4]> = (0..96).map(|y| unpremultiply(canvas.pixels[y * width + x])).collect();
            let reds = column.iter().filter(|pixel| pixel[0] > 128 && pixel[2] < 128).count();
            let blues = column.iter().filter(|pixel| pixel[2] > 128 && pixel[0] < 128).count();
            if reds.max(blues) >= 86 {
                alike += 1;
            }
        }
        assert!(alike >= 46, "only {alike} of 48 columns are stripes");
    }

    /// Only the painted half, blue, is taken from, so the fill is blue
    #[test]
    fn takes_only_from_what_is_painted() {
        let image = image::RgbaImage::from_fn(80, 80, |x, _| if x < 40 { image::Rgba([220, 30, 30, 255]) } else { image::Rgba([30, 30, 220, 255]) });
        let mut canvas = Canvas::grown(&image, &request(0.0, 0.0, 0.0, 0.5)).unwrap();
        let strokes = [Stroke { points: vec![[0.75, 0.0], [0.75, 1.0]], radius: 0.25, subtract: false }];
        let sample = canvas.sample(&strokes, 80, 80).unwrap();
        fill(&mut canvas, Some(&sample), &|| false).unwrap();
        let width = canvas.width;
        let blue = (80..120).flat_map(|y| (0..80).map(move |x| (x, y))).filter(|&(x, y)| {
            let [r, _, b, _] = unpremultiply(canvas.pixels[y * width + x]);
            b > r
        });
        assert!(blue.count() > 80 * 40 * 9 / 10);
    }

    #[test]
    fn stops_once_cancelled() {
        let mut canvas = Canvas::grown(&solid(64, 64, [1, 2, 3, 255]), &request(0.5, 0.0, 0.0, 0.0)).unwrap();
        assert_eq!(fill(&mut canvas, None, &|| true), Err(Failure::Cancelled));
    }

    #[test]
    fn transparent_images_grow_transparent() {
        let mut canvas = Canvas::grown(&solid(64, 64, [0, 0, 0, 0]), &request(0.0, 0.0, 0.5, 0.0)).unwrap();
        fill(&mut canvas, None, &|| false).unwrap();
        assert!(canvas.pixels.iter().all(|pixel| pixel[3] < 0.5));
    }

    /// Grows `LIGMA_EXPAND_IMAGE` with Content-aware and writes it next to
    /// it, to look at. `cargo test --release -- --ignored content_aware`
    #[test]
    #[ignore]
    fn content_aware_grows_an_image() {
        let Ok(path) = std::env::var("LIGMA_EXPAND_IMAGE") else {
            return;
        };
        let image = image::open(&path).unwrap().into_rgba8();
        let mut canvas = Canvas::grown(&image, &request(0.0, 0.1, 0.35, 0.1)).unwrap();
        let started = std::time::Instant::now();
        fill(&mut canvas, None, &|| false).unwrap();
        eprintln!("{}x{} in {:?}", canvas.width, canvas.height, started.elapsed());
        std::fs::write(format!("{path}.content.png"), canvas.png().unwrap()).unwrap();
    }
}
