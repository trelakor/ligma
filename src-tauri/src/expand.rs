//! Expand on this device (Settings > Features > Images): the image bar's
//! Expand in free projects (see ai_actions.rs), which grows an image past
//! its edges and fills what's new from the image itself, as Photoshop's
//! Content-Aware Fill does
//!
//! The page hands a request over as a download (see handoff.rs): a line of
//! JSON, then the layer's fills as Figma draws them, as a PNG:
//!
//! `{"left":l,"top":t,"right":r,"bottom":b,"sample":[…]}`
//!
//! The sides are how far to grow the image, in fractions of its width
//! (left and right) or height (top and bottom). `sample` is strokes of the
//! brush, `{"points":[[x,y],…],"radius":r,"subtract":false}`, in fractions
//! of the image and its width, painting where the fill takes from, or
//! taking it away; without any, it takes from all of the image. The result
//! is `{"png":<base64>}`, the grown image, with the image's own pixels as
//! they were, or "empty" when it doesn't grow
//!
//! It works as Wexler, Shechtman and Irani's space-time completion does,
//! coarse to fine: each new pixel's patch is matched to the image's most
//! alike with PatchMatch (Barnes et al.), and each new pixel becomes a blend
//! of what the patches over it matched, weighted by how well, which is
//! matched again, until it settles. Pure Rust, on every platform, with no
//! download

use base64::Engine;
use serde::Deserialize;
use tauri::AppHandle;

#[derive(Deserialize)]
struct Request {
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
/// when the page cancelled, and "failed" otherwise
pub fn handle(app: &AppHandle, tab: u32, job: u64, bytes: &[u8]) -> Result<String, &'static str> {
    let split = bytes.iter().position(|&byte| byte == b'\n').unwrap_or(bytes.len());
    let request: Request = serde_json::from_slice(&bytes[..split]).map_err(|_| "failed")?;
    let png = bytes.get(split + 1..).unwrap_or_default();
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png).map_err(failed)?.into_rgba8();
    let mut canvas = Canvas::grown(&image, &request)?;
    let stopped = || crate::handoff::cancelled(tab, job);
    let sample = if request.sample.is_empty() {
        background(app, tab, job, png).and_then(|cut| canvas.background(&cut))
    } else {
        canvas.sample(&request.sample, image.width(), image.height())
    };
    if stopped() {
        return Err("cancelled");
    }
    fill(&mut canvas, sample.as_deref(), &stopped).map_err(<&str>::from)?;
    let png = canvas.png()?;
    Ok(format!(r#"{{"png":"{}"}}"#, base64::engine::general_purpose::STANDARD.encode(png)))
}

/// The image cut out from its background, as Remove background does, if
/// that can be done without asking to download a model first
fn background(app: &AppHandle, tab: u32, job: u64, png: &[u8]) -> Option<image::RgbaImage> {
    if !crate::background::ready(app) {
        return None;
    }
    let cut = crate::background::remove(app, tab, job, png).ok()?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(cut).ok()?;
    Some(image::load_from_memory(&bytes).ok()?.into_rgba8())
}

fn failed(error: impl std::fmt::Display) -> &'static str {
    log::warn!("Expand: {error}");
    "failed"
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

    /// Where the fill takes from, as `strokes` paint it over the image,
    /// `width` by `height`, or None for all of it
    fn sample(&self, strokes: &[Stroke], width: u32, height: u32) -> Option<Vec<bool>> {
        if strokes.is_empty() {
            return None;
        }
        let mut painted = painted(strokes, (width, height), (self.left, self.top), (self.width, self.height));
        for (painted, &known) in painted.iter_mut().zip(&self.known) {
            *painted &= known;
        }
        painted.iter().any(|&painted| painted).then_some(painted)
    }

    /// Where to take from without anything painted: the image's
    /// background, all but its subject, as `cut`, the image cut out, shows
    /// it, and a little around it. None when that leaves too little
    fn background(&self, cut: &image::RgbaImage) -> Option<Vec<bool>> {
        let (width, height) = (cut.width() as usize, cut.height() as usize);
        let subject: Vec<bool> = cut.pixels().map(|pixel| pixel[3] >= SUBJECT).collect();
        // Far enough from the subject that no patch takes any of it
        let reach = (RADIUS as usize + 1).max(width.max(height) / 100);
        let near = grown_by(&subject, width, height, reach);
        let mut sample = vec![false; self.width * self.height];
        let mut count = 0;
        for y in 0..height.min(self.height - self.top) {
            for x in 0..width.min(self.width - self.left) {
                if !near[y * width + x] {
                    sample[(y + self.top) * self.width + x + self.left] = true;
                    count += 1;
                }
            }
        }
        (count as f32 >= (width * height) as f32 * LEAST_BACKGROUND).then_some(sample)
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

/// A pixel of a cut-out at least this opaque is the subject's
const SUBJECT: u8 = 64;
/// With less of the image than this left as background, all of it is
/// taken from
const LEAST_BACKGROUND: f32 = 0.12;

/// `mask`, `width` by `height`, grown by `by` pixels every way, along the
/// rows and then the columns
fn grown_by(mask: &[bool], width: usize, height: usize, by: usize) -> Vec<bool> {
    let line = |get: &dyn Fn(usize) -> bool, length: usize, set: &mut dyn FnMut(usize, bool)| {
        let mut last: Option<usize> = None;
        let mut next = (0..length).find(|&at| get(at));
        for at in 0..length {
            if get(at) {
                last = Some(at);
                next = (at + 1..length).find(|&after| get(after));
            }
            set(at, last.is_some_and(|last| at - last <= by) || next.is_some_and(|next| next - at <= by));
        }
    };
    let mut across = vec![false; mask.len()];
    for y in 0..height {
        line(&|x| mask[y * width + x], width, &mut |x, near| across[y * width + x] = near);
    }
    let mut out = vec![false; mask.len()];
    for x in 0..width {
        line(&|y| across[y * width + x], height, &mut |y, near| out[y * width + x] = near);
    }
    out
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

/// Where `strokes`, in fractions of an image `image` across and down, paint
/// over a canvas `canvas` across and down, with the image at `at` in it,
/// each stroke adding to what's painted, or taking away
fn painted(strokes: &[Stroke], image: (u32, u32), at: (usize, usize), canvas: (usize, usize)) -> Vec<bool> {
    let (width, height) = (image.0 as f32, image.1 as f32);
    let mut painted = vec![false; canvas.0 * canvas.1];
    for stroke in strokes {
        let radius = (stroke.radius * width).max(0.5);
        let points: Vec<[f32; 2]> = stroke.points.iter().map(|&[x, y]| [at.0 as f32 + x * width, at.1 as f32 + y * height]).collect();
        let segments: Vec<([f32; 2], [f32; 2])> = match points.len() {
            0 => continue,
            1 => vec![(points[0], points[0])],
            _ => points.windows(2).map(|pair| (pair[0], pair[1])).collect(),
        };
        for (a, b) in segments {
            for y in span(a[1], b[1], radius, canvas.1) {
                for x in span(a[0], b[0], radius, canvas.0) {
                    if from_segment([x as f32 + 0.5, y as f32 + 0.5], a, b) <= radius {
                        painted[y * canvas.0 + x] = !stroke.subtract;
                    }
                }
            }
        }
    }
    painted
}

#[derive(Deserialize)]
struct Healing {
    op: String,
    #[serde(default)]
    session: u64,
    #[serde(default)]
    strokes: Vec<Stroke>,
}

/// The image Spot healing has open, and the session that names it: one at a
/// time
static HEALING: std::sync::Mutex<Option<(u64, image::RgbaImage)>> = std::sync::Mutex::new(None);

/// Spot healing (see ai_actions.rs), for `job` in tab `tab`, which keeps
/// the image while it's open, so each stroke only says where it paints and
/// gets back only what changed. A request is a line of JSON, followed by
/// the layer's fills as Figma draws them, as a PNG, for "open":
///
/// - `{"op":"open"}` keeps the image, as session `job`, and gives
///   `{"session":<job>}`
/// - `{"op":"heal","session":s,"strokes":[…]}` fills what the strokes, as
///   Expand's `sample` has them, paint over from the image around it, as
///   Photoshop's Spot Healing Brush does, and gives the square of the
///   image that changed, `{"png":<base64>,"x":x,"y":y}`, or "empty" when
///   nothing is painted, or "closed" when the image isn't open any more.
///   Only a square around what's painted is filled, with the image around
///   it to go on, so it's quick on any image
/// - `{"op":"close","session":s}` lets the image go
pub fn heal(tab: u32, job: u64, bytes: &[u8]) -> Result<String, &'static str> {
    let split = bytes.iter().position(|&byte| byte == b'\n').unwrap_or(bytes.len());
    let request: Healing = serde_json::from_slice(&bytes[..split]).map_err(|_| "failed")?;
    let mut open = HEALING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    match request.op.as_str() {
        "open" => {
            let png = bytes.get(split + 1..).unwrap_or_default();
            let image = image::load_from_memory(png).map_err(failed)?.into_rgba8();
            *open = Some((job, image));
            Ok(format!(r#"{{"session":{job}}}"#))
        }
        "close" => {
            if open.as_ref().is_some_and(|(session, _)| *session == request.session) {
                *open = None;
            }
            Ok("{}".to_owned())
        }
        "heal" => {
            let (_, image) = open.as_mut().filter(|(session, _)| *session == request.session).ok_or("closed")?;
            let stopped = || crate::handoff::cancelled(tab, job);
            let (x, y, patch) = heal_strokes(image, &request.strokes, &stopped)?;
            let mut out = Vec::new();
            {
                use image::ImageEncoder;
                image::codecs::png::PngEncoder::new(&mut out)
                    .write_image(patch.as_raw(), patch.width(), patch.height(), image::ExtendedColorType::Rgba8)
                    .map_err(failed)?;
            }
            Ok(format!(r#"{{"png":"{}","x":{x},"y":{y}}}"#, base64::engine::general_purpose::STANDARD.encode(out)))
        }
        _ => Err("failed"),
    }
}

/// Fills what `strokes` paint over in `image` from the image around it, in
/// place, and gives the square of it that changed, and where it is
fn heal_strokes(image: &mut image::RgbaImage, strokes: &[Stroke], stopped: &dyn Fn() -> bool) -> Result<(usize, usize, image::RgbaImage), &'static str> {
    let (width, height) = (image.width() as usize, image.height() as usize);
    let hole = painted(strokes, (image.width(), image.height()), (0, 0), (width, height));
    let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
    for (at, _) in hole.iter().enumerate().filter(|(_, &hole)| hole) {
        let (x, y) = (at % width, at / width);
        (left, top, right, bottom) = (left.min(x), top.min(y), right.max(x + 1), bottom.max(y + 1));
    }
    if right <= left {
        return Err("empty");
    }
    // As much of the image again around it, and at least `AROUND`
    let margin = (right - left).max(bottom - top).max(AROUND);
    let (x0, y0) = (left.saturating_sub(margin), top.saturating_sub(margin));
    let (x1, y1) = ((right + margin).min(width), (bottom + margin).min(height));
    let (across, down) = (x1 - x0, y1 - y0);
    let mut canvas = Canvas {
        width: across,
        height: down,
        pixels: vec![[0.0; 4]; across * down],
        known: vec![true; across * down],
        left: 0,
        top: 0,
    };
    for y in 0..down {
        for x in 0..across {
            let at = y * across + x;
            canvas.pixels[at] = premultiply(image.get_pixel((x0 + x) as u32, (y0 + y) as u32).0);
            canvas.known[at] = !hole[(y0 + y) * width + x0 + x];
        }
    }
    if !canvas.known.contains(&true) {
        return Err("failed");
    }
    if !heal_spots(&mut canvas, stopped)? {
        fill_with(&mut canvas, None, false, stopped).map_err(<&str>::from)?;
    }
    // Only the square around what's painted comes back, and it's as small
    // as the hole and a few pixels around it, as nothing else changed
    let (px0, py0) = (left.saturating_sub(2), top.saturating_sub(2));
    let (px1, py1) = ((right + 2).min(width), (bottom + 2).min(height));
    for y in 0..down {
        for x in 0..across {
            if !canvas.known[y * across + x] {
                image.put_pixel((x0 + x) as u32, (y0 + y) as u32, image::Rgba(unpremultiply(canvas.pixels[y * across + x])));
            }
        }
    }
    let patch = image::imageops::crop_imm(image, px0 as u32, py0 as u32, (px1 - px0) as u32, (py1 - py0) as u32).to_image();
    Ok((px0, py0, patch))
}

/// Heals each spot painted over in `canvas`, as Photoshop's Spot Healing
/// Brush does: the place nearby whose surroundings best match the spot's
/// own is copied over it whole, so its texture, grain and noise come with
/// it as they are, and then its tone is evened out into the spot's
/// surroundings, so it matches the light around it. False when a spot has
/// nowhere to copy from, as when it's most of what's read
fn heal_spots(canvas: &mut Canvas, stopped: &dyn Fn() -> bool) -> Result<bool, &'static str> {
    let (width, height) = (canvas.width, canvas.height);
    let mut hole: Vec<bool> = canvas.known.iter().map(|&known| !known).collect();
    // The pieces to heal one at a time: each spot, what's painted over and
    // touches as one, or, for a long stroke, `PIECE`-pixel squares of it,
    // as a brush heals a dab at a time
    let mut seen = vec![false; hole.len()];
    let mut pieces: Vec<Vec<usize>> = Vec::new();
    for start in 0..hole.len() {
        if !hole[start] || seen[start] {
            continue;
        }
        let mut spot = vec![start];
        seen[start] = true;
        let mut next = 0;
        while next < spot.len() {
            let at = spot[next];
            next += 1;
            let (x, y) = ((at % width) as isize, (at / width) as isize);
            for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= width as isize || ny >= height as isize {
                    continue;
                }
                let near = ny as usize * width + nx as usize;
                if hole[near] && !seen[near] {
                    seen[near] = true;
                    spot.push(near);
                }
            }
        }
        let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
        for &at in &spot {
            let (x, y) = (at % width, at / width);
            (left, top, right, bottom) = (left.min(x), top.min(y), right.max(x), bottom.max(y));
        }
        if right - left < 2 * PIECE && bottom - top < 2 * PIECE {
            pieces.push(spot);
            continue;
        }
        let mut squares: std::collections::BTreeMap<(usize, usize), Vec<usize>> = std::collections::BTreeMap::new();
        for at in spot {
            squares.entry(((at / width) / PIECE, (at % width) / PIECE)).or_default().push(at);
        }
        pieces.extend(squares.into_values());
    }
    // Running counts of what's still to heal, to find a place clear of it
    // at once
    let counts = |hole: &[bool]| {
        let mut holes = vec![0u32; (width + 1) * (height + 1)];
        for y in 0..height {
            for x in 0..width {
                holes[(y + 1) * (width + 1) + x + 1] =
                    u32::from(hole[y * width + x]) + holes[y * (width + 1) + x + 1] + holes[(y + 1) * (width + 1) + x] - holes[y * (width + 1) + x];
            }
        }
        holes
    };
    let mut holes = counts(&hole);
    while !pieces.is_empty() {
        if stopped() {
            return Err("cancelled");
        }
        // The piece with the most of the image already around it next
        let around = |piece: &[usize]| {
            piece
                .iter()
                .filter(|&&at| {
                    let (x, y) = (at % width, at / width);
                    [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)]
                        .iter()
                        .any(|&(nx, ny)| nx < width && ny < height && !hole[ny * width + nx])
                })
                .count()
        };
        let index = (0..pieces.len()).max_by_key(|&i| around(&pieces[i])).unwrap_or(0);
        let piece = pieces.swap_remove(index);
        let Some((ox, oy)) = best_offset(canvas, &hole, &holes, &piece, stopped) else {
            if stopped() {
                return Err("cancelled");
            }
            return Ok(false);
        };
        copy_into(canvas, &hole, &piece, (ox, oy));
        for &at in &piece {
            hole[at] = false;
        }
        holes = counts(&hole);
    }
    Ok(true)
}

/// Long strokes are healed in squares this many pixels across
const PIECE: usize = 40;

/// Copies what's `offset` away over `piece`, with how far the copy's
/// surroundings are off from the piece's carried in from its edge,
/// smoothly, so it matches the light around it
fn copy_into(canvas: &mut Canvas, hole: &[bool], piece: &[usize], (ox, oy): (isize, isize)) {
    let (width, height) = (canvas.width, canvas.height);
    let source = |at: usize| ((at / width) as isize + oy) as usize * width + ((at % width) as isize + ox) as usize;
    let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
    for &at in piece {
        let (x, y) = (at % width, at / width);
        (left, top, right, bottom) = (left.min(x), top.min(y), right.max(x), bottom.max(y));
    }
    let (x0, y0) = (left.saturating_sub(RING), top.saturating_sub(RING));
    let (x1, y1) = ((right + RING + 1).min(width), (bottom + RING + 1).min(height));
    let (across, down) = (x1 - x0, y1 - y0);
    let mut in_piece = vec![false; across * down];
    for &at in piece {
        in_piece[(at / width - y0) * across + at % width - x0] = true;
    }
    // Known only where both it and its copy are the image's own, or healed
    let mut window = vec![[0f32; 4]; across * down];
    let mut known = vec![false; across * down];
    for i in 0..across * down {
        if in_piece[i] {
            continue;
        }
        let (x, y) = (x0 + i % across, y0 + i / across);
        let at = y * width + x;
        let (sx, sy) = (x as isize + ox, y as isize + oy);
        if hole[at] || sx < 0 || sy < 0 || sx >= width as isize || sy >= height as isize || hole[source(at)] {
            continue;
        }
        let (a, b) = (canvas.pixels[at], canvas.pixels[source(at)]);
        window[i] = [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]];
        known[i] = true;
    }
    if known.contains(&true) {
        diffuse(&mut window, &known, across, down);
        relax(&mut window, &known, across, down);
    }
    for &at in piece {
        let offset = window[(at / width - y0) * across + at % width - x0];
        let copied = canvas.pixels[source(at)];
        let alpha = (copied[3] + offset[3]).clamp(0.0, 255.0);
        canvas.pixels[at] = [
            (copied[0] + offset[0]).clamp(0.0, alpha),
            (copied[1] + offset[1]).clamp(0.0, alpha),
            (copied[2] + offset[2]).clamp(0.0, alpha),
            alpha,
        ];
    }
}

/// Smooths what `diffuse` filled in, where `known` isn't set, into a
/// membrane, each pixel the average of the four around it, as Poisson
/// blending with no texture of its own would, so the tone carried in from
/// the edge has no ridges
fn relax(values: &mut [[f32; 4]], known: &[bool], width: usize, height: usize) {
    let rounds = 2 * width.max(height);
    for _ in 0..rounds {
        for parity in 0..2 {
            for y in 0..height {
                for x in 0..width {
                    let at = y * width + x;
                    if known[at] || (x + y) % 2 != parity {
                        continue;
                    }
                    let (mut sum, mut count) = ([0f32; 4], 0f32);
                    for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
                        if nx < width && ny < height {
                            for (total, value) in sum.iter_mut().zip(values[ny * width + nx]) {
                                *total += value;
                            }
                            count += 1.0;
                        }
                    }
                    values[at] = sum.map(|total| total / count);
                }
            }
        }
    }
}

/// How far the edge around a spot is compared, in pixels
const RING: usize = 4;
/// How much more a place costs to copy from for each pixel farther from
/// the spot it is, so a spot takes from close by where that's as good
const FARTHER: f32 = 0.08;

/// The shift from `spot` to the place to copy it from: where the spot and
/// the ring around it all land on the image's own pixels, and the ring
/// there looks most like the spot's own ring, nearer places counting for a
/// little more. Places are tried coarsely, then finer around the best. None
/// when there's no such place, or once `stopped`
fn best_offset(canvas: &Canvas, hole: &[bool], holes: &[u32], spot: &[usize], stopped: &dyn Fn() -> bool) -> Option<(isize, isize)> {
    let (width, height) = (canvas.width, canvas.height);
    let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
    for &at in spot {
        let (x, y) = (at % width, at / width);
        (left, top, right, bottom) = (left.min(x), top.min(y), right.max(x), bottom.max(y));
    }
    // The ring: the image's own pixels within `RING` of the spot, found by
    // growing the spot, at most `RING_SAMPLES` of them
    let (x0, y0) = (left.saturating_sub(RING), top.saturating_sub(RING));
    let (x1, y1) = ((right + RING + 1).min(width), (bottom + RING + 1).min(height));
    let (across, down) = (x1 - x0, y1 - y0);
    let mut mask = vec![false; across * down];
    for &at in spot {
        mask[(at / width - y0) * across + at % width - x0] = true;
    }
    let near = grown_by(&mask, across, down, RING);
    let mut ring: Vec<(isize, isize)> = (0..across * down)
        .filter(|&i| near[i] && !hole[(y0 + i / across) * width + x0 + i % across])
        .map(|i| ((x0 + i % across) as isize, (y0 + i / across) as isize))
        .collect();
    if ring.is_empty() {
        return None;
    }
    if ring.len() > RING_SAMPLES {
        let step = ring.len().div_ceil(RING_SAMPLES);
        ring = ring.into_iter().step_by(step).collect();
    }
    let (w, h) = (width as isize, height as isize);
    let (bx0, by0, bx1, by1) = (x0 as isize, y0 as isize, x1 as isize, y1 as isize);
    // Whether the spot's box and ring, shifted, lands on none of what's
    // painted, from running counts of it
    let clear = |ox: isize, oy: isize| -> bool {
        let (sx0, sy0, sx1, sy1) = (bx0 + ox, by0 + oy, bx1 + ox, by1 + oy);
        if sx0 < 0 || sy0 < 0 || sx1 > w || sy1 > h {
            return false;
        }
        let at = |x: isize, y: isize| holes[y as usize * (width + 1) + x as usize];
        at(sx1, sy1) + at(sx0, sy0) == at(sx0, sy1) + at(sx1, sy0)
    };
    let cost = |ox: isize, oy: isize, limit: f32| -> f32 {
        let distance_squared = (ox * ox + oy * oy) as f32;
        let penalty = FARTHER * distance_squared / 16.0;
        if penalty >= limit || !clear(ox, oy) {
            return f32::INFINITY;
        }
        let budget = (limit - penalty) * ring.len() as f32;
        let mut sum = 0f32;
        for (count, &(x, y)) in ring.iter().enumerate() {
            let (a, b) = (canvas.pixels[(y * w + x) as usize], canvas.pixels[((y + oy) * w + x + ox) as usize]);
            sum += (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2) + (a[3] - b[3]).powi(2);
            if count % 32 == 31 && sum >= budget {
                return f32::INFINITY;
            }
        }
        sum / ring.len() as f32 + penalty
    };
    // Coarsely over everywhere, a step a 24th of the spot's size, then
    // finer and finer around the best
    let mut step = ((across.max(down) / 24).max(2)) as isize;
    let mut best: Option<((isize, isize), f32)> = None;
    let mut tried = 0usize;
    let mut oy = -h;
    while oy < h {
        let mut ox = -w;
        while ox < w {
            tried += 1;
            if tried.is_multiple_of(4096) && stopped() {
                return None;
            }
            let limit = best.map_or(f32::INFINITY, |(_, cost)| cost);
            let found = cost(ox, oy, limit);
            if found < limit {
                best = Some(((ox, oy), found));
            }
            ox += step;
        }
        oy += step;
    }
    while step > 1 {
        let ((cx, cy), _) = best?;
        let half = step;
        step = (step / 2).max(1);
        let mut oy = cy - half;
        while oy <= cy + half {
            let mut ox = cx - half;
            while ox <= cx + half {
                let limit = best.map_or(f32::INFINITY, |(_, cost)| cost);
                let found = cost(ox, oy, limit);
                if found < limit {
                    best = Some(((ox, oy), found));
                }
                ox += step;
            }
            oy += step;
        }
    }
    best.map(|(offset, _)| offset)
}

/// The ring is compared at most this many of its pixels
const RING_SAMPLES: usize = 600;


/// Spot healing reads at least this many pixels around what's painted
const AROUND: usize = 48;

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
    /// How much more a source costs in each `CELL` square, for how much of
    /// the fill already comes from around it (see `overuse`)
    overused: &'a [f32],
}

impl Matching<'_> {
    /// How unlike the patch at `target` is the one at `source`: the mean
    /// squared difference of the pixels both have in the image, and how
    /// overused the source is, or infinity once it's past `best`
    fn distance(&self, target: usize, source: usize, best: f32) -> f32 {
        let (width, height) = (self.width as isize, self.height as isize);
        let (tx, ty) = ((target % self.width) as isize, (target / self.width) as isize);
        let (sx, sy) = ((source % self.width) as isize, (source / self.width) as isize);
        let overused = self.overused[(sy as usize / CELL) * self.width.div_ceil(CELL) + sx as usize / CELL];
        if overused >= best {
            return f32::INFINITY;
        }
        let limit = (best - overused) * PATCH;
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
            sum / count + overused
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

/// Sources are counted in squares this many pixels across
const CELL: usize = 8;
/// How much a source costs for each time over its share the fill already
/// takes from around it, in the units patches are compared in
const OVERUSE: f32 = 250.0;

/// How much more each `CELL` square of sources costs for being used more
/// than its share by `matches`, so the fill takes from all over the image
/// rather than repeating the same part of it, as Kaspar et al.'s
/// self-tuning texture optimization does
fn overuse(matches: &[u32], hole: &[bool], sources: &[bool], width: usize, height: usize) -> Vec<f32> {
    let across = width.div_ceil(CELL);
    let mut used = vec![0f32; across * height.div_ceil(CELL)];
    let mut offered = vec![0f32; used.len()];
    for (at, &source) in sources.iter().enumerate() {
        if source {
            offered[(at / width / CELL) * across + (at % width) / CELL] += 1.0;
        }
    }
    let mut holes = 0f32;
    for (at, &hole) in hole.iter().enumerate() {
        if hole {
            let matched = matches[at] as usize;
            used[(matched / width / CELL) * across + (matched % width) / CELL] += 1.0;
            holes += 1.0;
        }
    }
    let share = holes / offered.iter().sum::<f32>().max(1.0);
    used.iter().zip(&offered).map(|(&used, &offered)| if offered > 0.0 { OVERUSE * (used / (offered * share).max(1.0) - 1.0).max(0.0) } else { 0.0 }).collect()
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
/// from where `sample` is set, if given. Stops with
/// `Failure::Cancelled` once `stopped`
fn fill(canvas: &mut Canvas, sample: Option<&[bool]>, stopped: &dyn Fn() -> bool) -> Result<Vec<u32>, Failure> {
    fill_with(canvas, sample, true, stopped)
}

/// As `fill`, and only with `diverse`, parts of the image already taken
/// from a lot cost more to take from again (see `overuse`): right for a
/// large fill, wrong for a small one, which should take from what's like it
/// right around it
fn fill_with(canvas: &mut Canvas, sample: Option<&[bool]>, diverse: bool, stopped: &dyn Fn() -> bool) -> Result<Vec<u32>, Failure> {
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
            let overused = if diverse { overuse(&matches, &hole, &sources, width, height) } else { vec![0.0; width.div_ceil(CELL) * height.div_ceil(CELL)] };
            let matching = Matching { width, height, pixels: &level.pixels, hole: &hole, sources: &sources, overused: &overused };
            let costs = costs(&matching, &matches);
            vote(level, &hole, &matches, &costs, &confidence);
        }
        for round in 0..rounds {
            if stopped() {
                return Err(Failure::Cancelled);
            }
            let overused = if diverse { overuse(&matches, &hole, &sources, width, height) } else { vec![0.0; width.div_ceil(CELL) * height.div_ceil(CELL)] };
            let matching = Matching { width, height, pixels: &level.pixels, hole: &hole, sources: &sources, overused: &overused };
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
    Ok(previous.map(|(_, matches)| matches).unwrap_or_default())
}


#[cfg(test)]
mod tests {
    use super::*;

    fn request(left: f32, top: f32, right: f32, bottom: f32) -> Request {
        Request { left, top, right, bottom, sample: vec![] }
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

    /// What a Spot healing request gives back, as JSON
    fn ask_heal(header: &str, image: Option<&image::RgbaImage>) -> Result<serde_json::Value, &'static str> {
        let mut png = Vec::new();
        if let Some(image) = image {
            image.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        }
        let request = [header.as_bytes(), b"\n", &png].concat();
        heal(0, 7, &request).map(|reply| serde_json::from_str(&reply).unwrap())
    }

    /// A spot painted over on a flat color heals to that color, and only a
    /// little more than it comes back. Spot healing's tests share its one
    /// open image, so they're one test
    #[test]
    fn heals_a_spot() {
        let mut image = image::RgbaImage::from_pixel(200, 120, image::Rgba([30, 140, 90, 255]));
        for y in 50..60 {
            for x in 90..100 {
                image.put_pixel(x, y, image::Rgba([250, 10, 10, 255]));
            }
        }
        assert_eq!(ask_heal(r#"{"op":"open"}"#, Some(&image)).unwrap()["session"], 7);
        assert_eq!(ask_heal(r#"{"op":"heal","session":7,"strokes":[]}"#, None).err(), Some("empty"));
        let reply = ask_heal(r#"{"op":"heal","session":7,"strokes":[{"points":[[0.475,0.458]],"radius":0.05}]}"#, None).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD.decode(reply["png"].as_str().unwrap()).unwrap();
        let patch = image::load_from_memory(&bytes).unwrap().into_rgba8();
        assert!(patch.width() < 30 && patch.height() < 30);
        assert!(patch.pixels().all(|pixel| pixel.0 == [30, 140, 90, 255]));
        let (x, y) = (reply["x"].as_u64().unwrap(), reply["y"].as_u64().unwrap());
        assert!(x <= 90 && y <= 50 && x + patch.width() as u64 >= 100 && y + patch.height() as u64 >= 60);
        ask_heal(r#"{"op":"close","session":7}"#, None).unwrap();
        assert_eq!(ask_heal(r#"{"op":"heal","session":7,"strokes":[{"points":[[0.5,0.5]],"radius":0.05}]}"#, None).err(), Some("closed"));
    }

    /// Heals `LIGMA_HEAL_IMAGE` where `LIGMA_HEAL_STROKES`, Spot healing's
    /// strokes, paint, and writes it next to the image. `cargo test
    /// --release -- --ignored heals_an_image`
    #[test]
    #[ignore]
    fn heals_an_image() {
        let (Ok(path), Ok(strokes)) = (std::env::var("LIGMA_HEAL_IMAGE"), std::env::var("LIGMA_HEAL_STROKES")) else {
            return;
        };
        let mut image = image::open(&path).unwrap().into_rgba8();
        let strokes: Vec<Stroke> = serde_json::from_str(&strokes).unwrap();
        let started = std::time::Instant::now();
        heal_strokes(&mut image, &strokes, &|| false).unwrap();
        eprintln!("healed in {:?}", started.elapsed());
        image.save(format!("{path}.healed.png")).unwrap();
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

    /// Grows `LIGMA_EXPAND_IMAGE` as `LIGMA_EXPAND_GROW` says, "left,top,
    /// right,bottom" in fractions of it, and writes it next to it, to look
    /// at. `cargo test --release -- --ignored content_aware`
    #[test]
    #[ignore]
    fn content_aware_grows_an_image() {
        let Ok(path) = std::env::var("LIGMA_EXPAND_IMAGE") else {
            return;
        };
        let grow = std::env::var("LIGMA_EXPAND_GROW").unwrap_or_else(|_| "0,0.1,0.35,0.1".to_owned());
        let [left, top, right, bottom]: [f32; 4] = grow.split(',').map(|side| side.trim().parse().unwrap()).collect::<Vec<_>>().try_into().unwrap();
        let image = image::open(&path).unwrap().into_rgba8();
        let mut canvas = Canvas::grown(&image, &request(left, top, right, bottom)).unwrap();
        // The subject left out, as `LIGMA_EXPAND_SUBJECT`, a cut-out, shows it
        let sample = std::env::var("LIGMA_EXPAND_SUBJECT").ok().and_then(|cut| canvas.background(&image::open(cut).unwrap().into_rgba8()));
        let started = std::time::Instant::now();
        fill(&mut canvas, sample.as_deref(), &|| false).unwrap();
        eprintln!("{}x{} in {:?}", canvas.width, canvas.height, started.elapsed());
        std::fs::write(format!("{path}.content.png"), canvas.png().unwrap()).unwrap();
    }
}
