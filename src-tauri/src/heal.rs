//! Spot healing on this device (Settings > Features > Images): the image
//! bar's Heal in free projects (see ai_actions.rs), which paints out spots,
//! blemishes, stray objects or text in an image, as Photoshop's Spot
//! Healing Brush does. Pure Rust, on every platform, with no download

use base64::Engine;
use serde::Deserialize;

/// A stroke of the brush, `{"points":[[x,y],…],"radius":r,"subtract":false}`,
/// in fractions of the image and its width, painting over what to heal, or
/// taking it away
#[derive(Deserialize)]
struct Stroke {
    points: Vec<[f32; 2]>,
    radius: f32,
    #[serde(default)]
    subtract: bool,
}

fn failed(error: impl std::fmt::Display) -> &'static str {
    log::warn!("Spot healing: {error}");
    "failed"
}

/// The image around what's healed: its pixels, premultiplied, 0 to 255,
/// and which of them are the image's own, the rest to heal
struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<[f32; 4]>,
    known: Vec<bool>,
}

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
/// each stroke adding to what's painted, or taking away. Stops early once
/// `stopped`, which the caller checks after
fn painted(strokes: &[Stroke], image: (u32, u32), at: (usize, usize), canvas: (usize, usize), stopped: &dyn Fn() -> bool) -> Vec<bool> {
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
            if stopped() {
                return painted;
            }
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
    /// "scrub" to even it out from what's around it rather than copy
    #[serde(default)]
    mode: String,
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
/// - `{"op":"heal","session":s,"strokes":[…],"mode":"heal"|"scrub"}`
///   fills what the strokes paint over from the image around it, as
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
            log::info!("Spot healing: opened {}x{}", image.width(), image.height());
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
            let started = std::time::Instant::now();
            let scrub = request.mode == "scrub";
            let (x, y, patch) = heal_strokes(image, &request.strokes, scrub, &stopped).inspect_err(|reason| {
                if *reason != "empty" && *reason != "cancelled" {
                    log::warn!("Spot healing: {reason} after {:?}", started.elapsed());
                }
            })?;
            log::info!("Spot healing{}: {}x{} patch in {:?}", if scrub { " (scrub)" } else { "" }, patch.width(), patch.height(), started.elapsed());
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
fn heal_strokes(image: &mut image::RgbaImage, strokes: &[Stroke], scrub: bool, stopped: &dyn Fn() -> bool) -> Result<(usize, usize, image::RgbaImage), &'static str> {
    let (width, height) = (image.width() as usize, image.height() as usize);
    let hole = painted(strokes, (image.width(), image.height()), (0, 0), (width, height), stopped);
    if stopped() {
        return Err("cancelled");
    }
    // A pixel more all round, so the soft edge of what's painted over, its
    // anti-aliasing or glow, goes with it rather than spread into the heal
    let hole = grown_by(&hole, width, height, EDGE);
    let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
    for (at, _) in hole.iter().enumerate().filter(|(_, &hole)| hole) {
        let (x, y) = (at % width, at / width);
        (left, top, right, bottom) = (left.min(x), top.min(y), right.max(x + 1), bottom.max(y + 1));
    }
    if right <= left {
        return Err("empty");
    }
    // As much of the image again around it, and at least `AROUND`
    let margin = (right - left).max(bottom - top).clamp(AROUND, MOST_AROUND);
    let (x0, y0) = (left.saturating_sub(margin), top.saturating_sub(margin));
    let (x1, y1) = ((right + margin).min(width), (bottom + margin).min(height));
    let (across, down) = (x1 - x0, y1 - y0);
    let mut canvas = Canvas {
        width: across,
        height: down,
        pixels: vec![[0.0; 4]; across * down],
        known: vec![true; across * down],
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
    if scrub {
        scrub_spots(&mut canvas, stopped)?;
    } else {
        heal_spots(&mut canvas, stopped)?;
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
/// surroundings, so it matches the light around it. A piece with nowhere
/// to copy from is evened out from what's around it instead
fn heal_spots(canvas: &mut Canvas, stopped: &dyn Fn() -> bool) -> Result<(), &'static str> {
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
    // How much of the image is already around each piece, and the box it
    // lies in, so only the pieces next to one just healed are counted again
    let around = |hole: &[bool], piece: &[usize]| {
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
    let bounds = |piece: &[usize]| {
        piece.iter().fold((width, height, 0, 0), |(left, top, right, bottom), &at| {
            let (x, y) = (at % width, at / width);
            (left.min(x), top.min(y), right.max(x), bottom.max(y))
        })
    };
    let mut boxes: Vec<(usize, usize, usize, usize)> = pieces.iter().map(|piece| bounds(piece)).collect();
    let started = std::time::Instant::now();
    let mut recounted = started;
    let mut hints: Vec<(isize, isize)> = Vec::new();
    let mut scores: Vec<usize> = pieces.iter().map(|piece| around(&hole, piece)).collect();
    while !pieces.is_empty() {
        if stopped() {
            return Err("cancelled");
        }
        // The piece with the most of the image already around it next
        let index = (0..pieces.len()).max_by_key(|&i| scores[i]).unwrap_or(0);
        let piece = pieces.swap_remove(index);
        let healed = boxes.swap_remove(index);
        scores.swap_remove(index);
        // Nowhere clear of what's still to heal: what's been healed can
        // be copied from too, and failing that, the piece is evened out
        // from what's around it
        let quick = started.elapsed() > BUDGET;
        let mut offset = best_offset(canvas, &hole, &holes, &piece, &hints, quick, stopped);
        // Counted again at most once in a while, as that's the whole image
        if offset.is_none() && !stopped() && recounted.elapsed() > RECOUNT {
            holes = counts(&hole);
            recounted = std::time::Instant::now();
            offset = best_offset(canvas, &hole, &holes, &piece, &hints, quick, stopped);
        }
        if stopped() {
            return Err("cancelled");
        }
        match offset {
            Some(offset) => {
                copy_into(canvas, &hole, &piece, offset);
                hints.retain(|&hint| hint != offset);
                hints.insert(0, offset);
                hints.truncate(HINTS);
            }
            None => smooth_into(canvas, &hole, &piece),
        }
        for &at in &piece {
            hole[at] = false;
        }
        let (left, top, right, bottom) = healed;
        for (i, &(l, t, r, b)) in boxes.iter().enumerate() {
            if l <= right + 1 && left <= r + 1 && t <= bottom + 1 && top <= b + 1 {
                scores[i] = around(&hole, &pieces[i]);
            }
        }
    }
    Ok(())
}

/// Scrub: evens each spot painted over in `canvas` out from what's around
/// it, whole, as one smooth membrane, so text or a mark on a smooth
/// surface comes off cleanly, without anything beside it copied in
fn scrub_spots(canvas: &mut Canvas, stopped: &dyn Fn() -> bool) -> Result<(), &'static str> {
    let (width, height) = (canvas.width, canvas.height);
    let mut hole: Vec<bool> = canvas.known.iter().map(|&known| !known).collect();
    let mut seen = vec![false; hole.len()];
    for start in 0..hole.len() {
        if !hole[start] || seen[start] {
            continue;
        }
        if stopped() {
            return Err("cancelled");
        }
        let mut spot = vec![start];
        seen[start] = true;
        let mut next = 0;
        while next < spot.len() {
            let at = spot[next];
            next += 1;
            let (x, y) = (at % width, at / width);
            for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
                if nx < width && ny < height && hole[ny * width + nx] && !seen[ny * width + nx] {
                    seen[ny * width + nx] = true;
                    spot.push(ny * width + nx);
                }
            }
        }
        smooth_into(canvas, &hole, &spot);
        for &at in &spot {
            hole[at] = false;
        }
    }
    Ok(())
}

/// Fills `piece` evenly from what's around it, for a piece with nowhere to
/// copy from
fn smooth_into(canvas: &mut Canvas, hole: &[bool], piece: &[usize]) {
    let width = canvas.width;
    let (mut left, mut top, mut right, mut bottom) = (width, canvas.height, 0, 0);
    for &at in piece {
        let (x, y) = (at % width, at / width);
        (left, top, right, bottom) = (left.min(x), top.min(y), right.max(x), bottom.max(y));
    }
    let (x0, y0) = (left.saturating_sub(RING), top.saturating_sub(RING));
    let (x1, y1) = ((right + RING + 1).min(width), (bottom + RING + 1).min(canvas.height));
    let (across, down) = (x1 - x0, y1 - y0);
    let mut window: Vec<[f32; 4]> = (0..across * down).map(|i| canvas.pixels[(y0 + i / across) * width + x0 + i % across]).collect();
    let known: Vec<bool> = (0..across * down).map(|i| !hole[(y0 + i / across) * width + x0 + i % across]).collect();
    if !known.contains(&true) {
        return;
    }
    let depth = diffuse(&mut window, &known, across, down);
    relax(&mut window, &known, across, down, deepest(&depth));
    for &at in piece {
        canvas.pixels[at] = window[(at / width - y0) * across + at % width - x0];
    }
}

/// Long strokes are healed in squares this many pixels across
const PIECE: usize = 40;
/// Where the last this many pieces copied from is tried first
const HINTS: usize = 6;
/// What's left to heal is counted again at most this often
const RECOUNT: std::time::Duration = std::time::Duration::from_millis(100);

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
    // Where the surroundings differ from the copy's far more than most
    // do, it's something left of what's removed, or an edge the copy
    // hasn't, and carrying that in would smear it over the spot
    let far = |value: &[f32; 4]| value.iter().map(|v| v * v).sum::<f32>().sqrt();
    let mut distances: Vec<f32> = (0..across * down).filter(|&i| known[i]).map(|i| far(&window[i])).collect();
    if !distances.is_empty() {
        let middle = distances.len() / 2;
        let typical = *distances.select_nth_unstable_by(middle, f32::total_cmp).1;
        let most = OUTLIER * typical + OUTLIER_FLOOR;
        let kept = (0..across * down).filter(|&i| known[i] && far(&window[i]) <= most).count();
        // Unless that leaves too little to go on
        if kept * 4 >= distances.len() {
            for i in 0..across * down {
                if known[i] && far(&window[i]) > most {
                    known[i] = false;
                }
            }
        }
    }
    if known.contains(&true) {
        let depth = diffuse(&mut window, &known, across, down);
        relax(&mut window, &known, across, down, deepest(&depth));
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

/// Around a spot, where the surroundings differ from the copy's more than
/// this many times as far as typically, and `OUTLIER_FLOOR` more, out of
/// 255, the difference isn't carried in
const OUTLIER: f32 = 3.0;
const OUTLIER_FLOOR: f32 = 24.0;

/// Smooths what `diffuse` filled in, where `known` isn't set, into a
/// membrane, each pixel the average of the four around it, as Poisson
/// blending with no texture of its own would, so the tone carried in from
/// the edge has no ridges
/// `depth` is how far the farthest of it is from what's known, as
/// `diffuse` gives it: a few rounds for each pixel of that settle it.
/// Deep, it's settled at half the size first, a quarter of the pixels
/// in half the rounds, and then only finished here
fn relax<const N: usize>(values: &mut [[f32; N]], known: &[bool], width: usize, height: usize, depth: u32) {
    let mut rounds = (RELAX * depth as usize + 4).min(2 * width.max(height));
    if depth > COARSE_FROM && width >= 4 && height >= 4 {
        let (across, down) = (width.div_ceil(2), height.div_ceil(2));
        // Each 2 by 2 block: known where any of it is, as the average of
        // that, and otherwise the average of what it's been filled with
        let mut coarse = vec![[0f32; N]; across * down];
        let mut coarse_known = vec![false; across * down];
        for cy in 0..down {
            for cx in 0..across {
                let (mut all, mut all_count, mut sure, mut sure_count) = ([0f32; N], 0f32, [0f32; N], 0f32);
                for y in 2 * cy..(2 * cy + 2).min(height) {
                    for x in 2 * cx..(2 * cx + 2).min(width) {
                        let value = values[y * width + x];
                        for channel in 0..N {
                            all[channel] += value[channel];
                        }
                        all_count += 1.0;
                        if known[y * width + x] {
                            for channel in 0..N {
                                sure[channel] += value[channel];
                            }
                            sure_count += 1.0;
                        }
                    }
                }
                let at = cy * across + cx;
                coarse_known[at] = sure_count > 0.0;
                coarse[at] = if sure_count > 0.0 { sure.map(|total| total / sure_count) } else { all.map(|total| total / all_count) };
            }
        }
        relax(&mut coarse, &coarse_known, across, down, depth.div_ceil(2));
        for y in 0..height {
            for x in 0..width {
                if !known[y * width + x] {
                    values[y * width + x] = coarse[(y / 2) * across + x / 2];
                }
            }
        }
        rounds = rounds.min(FINISH);
    }
    // Only what isn't known, split into the two checkerboard colors
    let mut free: [Vec<(usize, [usize; 4], f32)>; 2] = [Vec::new(), Vec::new()];
    for y in 0..height {
        for x in 0..width {
            let at = y * width + x;
            if known[at] {
                continue;
            }
            let mut near = [at; 4];
            let mut count = 0;
            for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
                if nx < width && ny < height {
                    near[count] = ny * width + nx;
                    count += 1;
                }
            }
            free[(x + y) % 2].push((at, near, count as f32));
        }
    }
    for _ in 0..rounds {
        for color in &free {
            for &(at, near, count) in color {
                let mut sum = [0f32; N];
                for &neighbor in &near[..count as usize] {
                    for (total, value) in sum.iter_mut().zip(values[neighbor]) {
                        *total += value;
                    }
                }
                // Past the average, as over-relaxation does, which settles
                // in far fewer rounds
                let old = values[at];
                for (channel, total) in sum.iter().enumerate() {
                    values[at][channel] = old[channel] + OVER * (total / count - old[channel]);
                }
            }
        }
    }
}

/// The farthest of `depth`, as `diffuse` gives it, from what's known
fn deepest(depth: &[u32]) -> u32 {
    depth.iter().copied().filter(|&depth| depth != u32::MAX).max().unwrap_or(0)
}

/// `relax` runs this many rounds for each pixel the unknown reaches
const RELAX: usize = 3;
/// `relax` settles what's deeper than this at half the size first, and
/// then finishes it in this many rounds
const COARSE_FROM: u32 = 6;
const FINISH: usize = 10;
/// How far past the average `relax` moves each pixel each round
const OVER: f32 = 1.8;

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
#[allow(clippy::too_many_arguments)]
fn best_offset(
    canvas: &Canvas,
    hole: &[bool],
    holes: &[u32],
    spot: &[usize],
    hints: &[(isize, isize)],
    quick: bool,
    stopped: &dyn Fn() -> bool,
) -> Option<(isize, isize)> {
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
    // What the spot would be, evened out from its surroundings, and how
    // much those vary from pixel to pixel: a place whose copy strays from
    // that by much more, like a stripe or an edge where it's flat around
    // the spot, costs more, so in a tight spot it doesn't bring in what's
    // beside it. Texture as fine as the surroundings' costs nothing
    let mut guess: Vec<[f32; 4]> = (0..across * down).map(|i| canvas.pixels[(y0 + i / across) * width + x0 + i % across]).collect();
    let known: Vec<bool> = (0..across * down).map(|i| !hole[(y0 + i / across) * width + x0 + i % across]).collect();
    let depth = diffuse(&mut guess, &known, across, down);
    relax(&mut guess, &known, across, down, deepest(&depth));
    let mut inside: Vec<(isize, isize, [f32; 4])> = spot
        .iter()
        .map(|&at| {
            let (x, y) = (at % width, at / width);
            (x as isize, y as isize, guess[(y - y0) * across + x - x0])
        })
        .collect();
    if inside.len() > INSIDE_SAMPLES {
        let step = inside.len().div_ceil(INSIDE_SAMPLES);
        inside = inside.into_iter().step_by(step).collect();
    }
    let grain = {
        let (mut sum, mut count) = (0f32, 0f32);
        for &(x, y) in &ring {
            let (x, y) = (x as usize, y as usize);
            if x == 0 || y == 0 || x + 1 >= width || y + 1 >= height {
                continue;
            }
            let neighbours = [(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)];
            if neighbours.iter().any(|&(nx, ny)| hole[ny * width + nx]) {
                continue;
            }
            let pixel = canvas.pixels[y * width + x];
            for channel in 0..4 {
                let mean = neighbours.iter().map(|&(nx, ny)| canvas.pixels[ny * width + nx][channel]).sum::<f32>() / 4.0;
                sum += (pixel[channel] - mean).powi(2);
            }
            count += 1.0;
        }
        if count > 0.0 { (sum / count).sqrt() } else { 0.0 }
    };
    let allowed = GRAIN * grain + GRAIN_FLOOR;
    let (w, h) = (width as isize, height as isize);
    // How far a copy from `ox` and `oy` strays, on average, or infinity
    // once that's past `limit`
    let strays = |ox: isize, oy: isize, limit: f32| -> f32 {
        let budget = limit * inside.len() as f32;
        let mut sum = 0f32;
        for (count, &(x, y, expected)) in inside.iter().enumerate() {
            let b = canvas.pixels[((y + oy) * w + x + ox) as usize];
            let off = ((expected[0] - b[0]).powi(2) + (expected[1] - b[1]).powi(2) + (expected[2] - b[2]).powi(2) + (expected[3] - b[3]).powi(2)).sqrt();
            sum += (off - allowed).max(0.0).powi(2);
            if count % 32 == 31 && sum >= budget {
                return f32::INFINITY;
            }
        }
        sum / inside.len().max(1) as f32
    };
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
        let around = sum / ring.len() as f32 + penalty;
        around + strays(ox, oy, limit - around)
    };
    // Nearest places first, a step a 24th of the spot's size apart, as far
    // as `REACH` spots' sizes away: once one fits well, places farther off
    // cost more than it for being far, so the search stops there. Then
    // finer and finer around the best
    let mut step = ((across.max(down) / 24).max(2)) as isize;
    // Pressed for time, only close by
    let reach = if quick { QUICK_REACH } else { (across.max(down) * REACH).max(MIN_REACH) } as isize;
    let offsets = nearest_first(step, reach);
    // Where the pieces healed last copied from first, as the next piece
    // along a stroke most often fits there too, so far fewer places are
    // tried after
    let mut best: Option<((isize, isize), f32)> = None;
    for &(ox, oy) in hints {
        let limit = best.map_or(f32::INFINITY, |(_, cost)| cost);
        let found = cost(ox, oy, limit);
        if found < limit {
            best = Some(((ox, oy), found));
        }
    }
    for (tried, &(ox, oy)) in offsets.iter().enumerate() {
        if tried.is_multiple_of(4096) && stopped() {
            return None;
        }
        let limit = best.map_or(f32::INFINITY, |(_, cost)| cost);
        if FARTHER * (ox * ox + oy * oy) as f32 / 16.0 >= limit {
            break;
        }
        let found = cost(ox, oy, limit);
        if found < limit {
            best = Some(((ox, oy), found));
        }
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

/// Places `step` apart, as far as `reach` each way, nearest first, kept
/// for the next piece, which most often asks for the same
fn nearest_first(step: isize, reach: isize) -> std::rc::Rc<Vec<(isize, isize)>> {
    type Kept = Option<((isize, isize), std::rc::Rc<Vec<(isize, isize)>>)>;
    thread_local! {
        static KEPT: std::cell::RefCell<Kept> = const { std::cell::RefCell::new(None) };
    }
    KEPT.with(|kept| {
        if let Some((key, offsets)) = &*kept.borrow() {
            if *key == (step, reach) {
                return offsets.clone();
            }
        }
        let mut offsets: Vec<(isize, isize)> = Vec::new();
        let mut oy = -reach;
        while oy <= reach {
            let mut ox = -reach;
            while ox <= reach {
                offsets.push((ox, oy));
                ox += step;
            }
            oy += step;
        }
        offsets.sort_by_key(|&(ox, oy)| ox * ox + oy * oy);
        let offsets = std::rc::Rc::new(offsets);
        *kept.borrow_mut() = Some(((step, reach), offsets.clone()));
        offsets
    })
}

/// A spot is copied from at most this many times its size away, and at
/// least `MIN_REACH` pixels, or, once a heal has taken `BUDGET`, only
/// `QUICK_REACH`, so a long scribble still comes back quickly
const REACH: usize = 4;
const MIN_REACH: usize = 96;
const QUICK_REACH: usize = 48;
const BUDGET: std::time::Duration = std::time::Duration::from_millis(800);
/// The ring is compared at most this many of its pixels
const RING_SAMPLES: usize = 600;
/// What's copied is compared to what the spot would be, evened out, at
/// most this many of its pixels, its straying costing only past this many
/// times as far as the surroundings vary from pixel to pixel, and
/// `GRAIN_FLOOR` more, out of 255
const INSIDE_SAMPLES: usize = 300;
const GRAIN: f32 = 3.0;
const GRAIN_FLOOR: f32 = 16.0;


/// What's painted over is grown by this many pixels
const EDGE: usize = 1;

/// Spot healing reads at least this many pixels around what's painted,
/// and at most `MOST_AROUND`
const AROUND: usize = 48;
const MOST_AROUND: usize = 384;

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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn scrubs_a_tight_spot_from_what_surrounds_it() {
        // A small white panel on black, a dark mark in it, and nowhere
        // white clear of the mark to copy from
        let mut image = image::RgbaImage::from_pixel(64, 64, image::Rgba([10, 10, 10, 255]));
        for y in 10..32 {
            for x in 10..32 {
                let mark = (17..25).contains(&x) && (17..25).contains(&y);
                image.put_pixel(x, y, image::Rgba(if mark { [40, 40, 40, 255] } else { [240, 240, 240, 255] }));
            }
        }
        let strokes: Vec<Stroke> = serde_json::from_str(r#"[{"points":[[0.328,0.328]],"radius":0.09}]"#).unwrap();
        heal_strokes(&mut image, &strokes, true, &|| false).unwrap();
        for (x, y) in [(21, 21), (18, 18), (24, 24)] {
            assert!(image.get_pixel(x, y)[0] > 200, "{x}, {y}: {:?}", image.get_pixel(x, y));
        }
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
        heal_strokes(&mut image, &strokes, std::env::var("LIGMA_HEAL_SCRUB").is_ok(), &|| false).unwrap();
        eprintln!("healed in {:?}", started.elapsed());
        image.save(format!("{path}.healed.png")).unwrap();
    }
}
