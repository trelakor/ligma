//! Select object on this device (Settings > Features > Images): the image
//! bar's Select object in free projects (see ai_actions.rs), which picks
//! out objects in an image as Photoshop's Object Selection does. SAM 2.1
//! Tiny (see models.rs) reads the image once, then finds the object at a
//! point or in a box in a few tens of milliseconds, so the page can show
//! the object under the pointer as it moves
//!
//! The page hands requests over as downloads (see handoff.rs): a line of
//! JSON, followed by the layer as Figma draws it, as a PNG, for "open" and
//! "isolate":
//!
//! - `{"op":"open"}` reads the image, for the requests after it, and gives
//!   its size and the session that names it in those: the job's number
//! - `{"op":"outline","session":<job>,"picks":[…]}` gives the outline of
//!   the picked objects, as SVG path commands in the image's pixels
//! - `{"op":"isolate","session":<job>,"picks":[…]}` gives the image after
//!   the line, larger, with only the picked objects, cropped to them, as a
//!   PNG in base64, and where it was cropped, in fractions of the image
//! - `{"op":"close","session":<job>}` lets the image go
//!
//! A pick is `{"point":[x,y]}` or `{"box":[left,top,right,bottom]}`, in
//! fractions of the image, a stroke of the brush,
//! `{"stroke":[[x,y],…],"radius":r}`, whose radius is a fraction of the
//! image's width, or what's inside a lasso, `{"lasso":[[x,y],…]}`, a
//! closed outline. `"subtract":true` takes it away from the picks before it
//! rather than adds it. `"edge":{"shift":s,"feather":f}` on "outline" and
//! "isolate" moves the edge out by `s` pixels of the image the request is
//! about, or in when it's negative, and softens it over `f` pixels either
//! side. One image is kept at a time

use serde::Deserialize;
use tauri::AppHandle;

#[derive(Deserialize)]
#[cfg_attr(not(birefnet), allow(dead_code))]
struct Request {
    op: String,
    #[serde(default)]
    session: u64,
    #[serde(default)]
    picks: Vec<Pick>,
    #[serde(default)]
    edge: Edge,
}

#[derive(Clone, Deserialize)]
#[cfg_attr(not(birefnet), allow(dead_code))]
struct Pick {
    point: Option<[f32; 2]>,
    #[serde(rename = "box")]
    rect: Option<[f32; 4]>,
    stroke: Option<Vec<[f32; 2]>>,
    #[serde(default)]
    radius: f32,
    lasso: Option<Vec<[f32; 2]>>,
    #[serde(default)]
    subtract: bool,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[cfg_attr(not(birefnet), allow(dead_code))]
struct Edge {
    #[serde(default)]
    shift: f32,
    #[serde(default)]
    feather: f32,
}

/// What `bytes` asks for, done for `job` in tab `tab`: the result as JSON,
/// or why there's none: "cancelled" when the model wasn't downloaded,
/// "closed" when the image was let go, "empty" when nothing is picked and
/// "failed" otherwise
pub fn handle(app: &AppHandle, tab: u32, job: u64, bytes: &[u8]) -> Result<String, &'static str> {
    let split = bytes.iter().position(|&byte| byte == b'\n').unwrap_or(bytes.len());
    let request: Request = serde_json::from_slice(&bytes[..split]).map_err(|_| "failed")?;
    let image = bytes.get(split + 1..).unwrap_or_default();
    #[cfg(birefnet)]
    return sam::handle(app, tab, job, request, image);
    #[cfg(not(birefnet))]
    {
        let _ = (app, tab, job, request, image);
        Err("failed")
    }
}

/// Lets the image and model go, before the model is removed
pub fn unload() {
    #[cfg(birefnet)]
    sam::unload();
}

/// The picks, over an image `width` by `height`, as how far inside them
/// each pixel is: positive inside and negative outside, in any units, as
/// long as they're smooth near the edge
#[cfg_attr(not(birefnet), allow(dead_code))]
struct Field {
    values: Vec<f32>,
    width: usize,
    height: usize,
}

/// Outside anything painted, before anything else is picked
#[cfg_attr(not(birefnet), allow(dead_code))]
const FAR: f32 = -1e4;

#[cfg_attr(not(birefnet), allow(dead_code))]
impl Field {
    fn empty(width: usize, height: usize) -> Self {
        Self { values: vec![FAR; width * height], width, height }
    }

    /// Adds `other` to the picks, or takes it away
    fn combine(&mut self, other: &[f32], subtract: bool) {
        for (value, &other) in self.values.iter_mut().zip(other) {
            *value = if subtract { value.min(-other) } else { value.max(other) };
        }
    }

    /// Adds a stroke of the brush through `points`, in fractions of the
    /// image, `radius` pixels wide either side, or takes it away. Only the
    /// pixels near it change, as nothing farther away is affected
    fn paint(&mut self, points: &[[f32; 2]], radius: f32, subtract: bool) {
        let (width, height) = (self.width, self.height);
        let points: Vec<[f32; 2]> = points.iter().map(|&[x, y]| [x * width as f32, y * height as f32]).collect();
        let segments: Vec<([f32; 2], [f32; 2])> = match points.len() {
            0 => return,
            1 => vec![(points[0], points[0])],
            _ => points.windows(2).map(|pair| (pair[0], pair[1])).collect(),
        };
        // Two pixels more, so the edge's slope is known either side of it
        let reach = radius + 2.0;
        for (a, b) in segments {
            for y in span(a[1], b[1], reach, height) {
                for x in span(a[0], b[0], reach, width) {
                    let inside = radius - from_segment([x as f32 + 0.5, y as f32 + 0.5], a, b);
                    let value = &mut self.values[y * width + x];
                    *value = if subtract { value.min(-inside) } else { value.max(inside) };
                }
            }
        }
    }

    /// Adds what's inside `points`, a closed outline in fractions of the
    /// image, or takes it away. Each row is filled between where it crosses
    /// the outline, and near the outline, how far from it each pixel is
    /// gives the edge's slope
    fn lasso(&mut self, points: &[[f32; 2]], subtract: bool) {
        const NEAR: f32 = 2.0;
        let (width, height) = (self.width, self.height);
        let points: Vec<[f32; 2]> = points.iter().map(|&[x, y]| [x * width as f32, y * height as f32]).collect();
        if points.len() < 3 {
            return;
        }
        let edges: Vec<([f32; 2], [f32; 2])> = (0..points.len()).map(|i| (points[i], points[(i + 1) % points.len()])).collect();
        let lowest = |axis: usize| points.iter().map(|point| point[axis]).fold(f32::INFINITY, f32::min);
        let highest = |axis: usize| points.iter().map(|point| point[axis]).fold(f32::NEG_INFINITY, f32::max);
        let (columns, rows) = (span(lowest(0), highest(0), NEAR, width), span(lowest(1), highest(1), NEAR, height));
        if columns.is_empty() || rows.is_empty() {
            return;
        }
        let across = columns.len();
        let at = |x: usize, y: usize| (y - rows.start) * across + x - columns.start;
        let mut shape = vec![-NEAR; across * rows.len()];
        let mut crossings = Vec::new();
        for y in rows.clone() {
            let middle = y as f32 + 0.5;
            crossings.clear();
            for &(a, b) in &edges {
                if (a[1] <= middle) != (b[1] <= middle) {
                    crossings.push(a[0] + (middle - a[1]) / (b[1] - a[1]) * (b[0] - a[0]));
                }
            }
            crossings.sort_by(f32::total_cmp);
            for &[from, to] in crossings.as_chunks::<2>().0 {
                for x in columns.clone().filter(|&x| (from..to).contains(&(x as f32 + 0.5))) {
                    shape[at(x, y)] = NEAR;
                }
            }
        }
        for &(a, b) in &edges {
            for y in span(a[1], b[1], NEAR, height) {
                for x in span(a[0], b[0], NEAR, width) {
                    let value = &mut shape[at(x, y)];
                    *value = value.signum() * value.abs().min(from_segment([x as f32 + 0.5, y as f32 + 0.5], a, b));
                }
            }
        }
        for y in rows.clone() {
            for x in columns.clone() {
                let (inside, value) = (shape[at(x, y)], &mut self.values[y * width + x]);
                *value = if subtract { value.min(-inside) } else { value.max(inside) };
            }
        }
    }

    /// How far inside the picks each pixel's center is, in pixels, and
    /// negative outside: as far as the nearest pixel on the other side less
    /// half a pixel, and along the edge, to a fraction of a pixel, from the
    /// field's slope there
    fn distances(&self) -> Vec<f32> {
        let (width, height) = (self.width, self.height);
        let inside: Vec<bool> = self.values.iter().map(|&value| value > 0.0).collect();
        let mut distances = squared_distances(&inside, false, width, height);
        let to_inside = squared_distances(&inside, true, width, height);
        for ((distance, &other), &inside) in distances.iter_mut().zip(&to_inside).zip(&inside) {
            *distance = if inside { distance.sqrt() - 0.5 } else { 0.5 - other.sqrt() };
        }
        drop(to_inside);
        let value = |x: usize, y: usize| self.values[y * width + x];
        for y in 0..height {
            for x in 0..width {
                let distance = &mut distances[y * width + x];
                if distance.abs() >= 1.0 {
                    continue;
                }
                let dx = (value((x + 1).min(width - 1), y) - value(x.saturating_sub(1), y)) / ((x + 1).min(width - 1) - x.saturating_sub(1)).max(1) as f32;
                let dy = (value(x, (y + 1).min(height - 1)) - value(x, y.saturating_sub(1))) / ((y + 1).min(height - 1) - y.saturating_sub(1)).max(1) as f32;
                let slope = dx.hypot(dy);
                if slope > 1e-6 {
                    let finer = value(x, y) / slope;
                    if finer.abs() < 1.0 && (finer > 0.0) == (*distance > 0.0) {
                        *distance = finer;
                    }
                }
            }
        }
        distances
    }
}

/// The pixels from `from` to `to` along one side of an image `size`
/// pixels across, and `reach` more either way
#[cfg_attr(not(birefnet), allow(dead_code))]
fn span(from: f32, to: f32, reach: f32, size: usize) -> std::ops::Range<usize> {
    let low = (from.min(to) - reach).floor().max(0.0) as usize;
    let high = ((from.max(to) + reach).ceil().max(0.0) as usize).min(size);
    low.min(high)..high
}

/// How far `point` is from the segment from `a` to `b`
#[cfg_attr(not(birefnet), allow(dead_code))]
fn from_segment(point: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (at, along) = ([point[0] - a[0], point[1] - a[1]], [b[0] - a[0], b[1] - a[1]]);
    let length = along[0] * along[0] + along[1] * along[1];
    let t = if length > 0.0 { ((at[0] * along[0] + at[1] * along[1]) / length).clamp(0.0, 1.0) } else { 0.0 };
    (at[0] - t * along[0]).hypot(at[1] - t * along[1])
}

/// How much of a pixel `distance` inside the picks' edge is kept, with the
/// edge moved and softened as `edge` says: half a pixel either side of it
/// at least, so it's antialiased
#[cfg_attr(not(birefnet), allow(dead_code))]
fn kept(distance: f32, edge: Edge) -> f32 {
    let width = (2.0 * edge.feather).max(1.0);
    let t = ((distance + edge.shift) / width + 0.5).clamp(0.0, 1.0);
    if width > 1.0 {
        t * t * (3.0 - 2.0 * t)
    } else {
        t
    }
}

/// The squared distance from each pixel's center to the nearest one's
/// that's `set` in `mask`, as Felzenszwalb and Huttenlocher find it
#[cfg_attr(not(birefnet), allow(dead_code))]
fn squared_distances(mask: &[bool], set: bool, width: usize, height: usize) -> Vec<f32> {
    const NONE: f32 = 1e20;
    let mut distances: Vec<f32> = mask.iter().map(|&value| if value == set { 0.0 } else { NONE }).collect();
    let longest = width.max(height);
    let (mut line, mut out) = (vec![0f32; longest], vec![0f32; longest]);
    let (mut hulls, mut bounds) = (vec![0usize; longest], vec![0f32; longest + 1]);
    for x in 0..width {
        for y in 0..height {
            line[y] = distances[y * width + x];
        }
        lower_envelope(&line[..height], &mut out, &mut hulls, &mut bounds);
        for y in 0..height {
            distances[y * width + x] = out[y];
        }
    }
    for y in 0..height {
        let row = &mut distances[y * width..(y + 1) * width];
        line[..width].copy_from_slice(row);
        lower_envelope(&line[..width], row, &mut hulls, &mut bounds);
    }
    distances
}

/// The squared distance transform of one line, `f`, into `out`
#[cfg_attr(not(birefnet), allow(dead_code))]
fn lower_envelope(f: &[f32], out: &mut [f32], hulls: &mut [usize], bounds: &mut [f32]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let meet = |q: usize, p: usize| ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * (q as f32 - p as f32));
    let mut k = 0;
    hulls[0] = 0;
    bounds[0] = f32::NEG_INFINITY;
    bounds[1] = f32::INFINITY;
    for q in 1..n {
        let mut s = meet(q, hulls[k]);
        while k > 0 && s <= bounds[k] {
            k -= 1;
            s = meet(q, hulls[k]);
        }
        k += 1;
        hulls[k] = q;
        bounds[k] = s;
        bounds[k + 1] = f32::INFINITY;
    }
    k = 0;
    for (q, out) in out.iter_mut().enumerate().take(n) {
        while bounds[k + 1] < q as f32 {
            k += 1;
        }
        let p = hulls[k];
        let d = q as f32 - p as f32;
        *out = d * d + f[p];
    }
}

/// Blurs `logits`, `side` square, a cell either way: SAM's masks are
/// checkered where it's unsure, from how it scales them up itself
#[cfg_attr(not(birefnet), allow(dead_code))]
fn smooth(logits: &mut [f32], side: usize) {
    let mut across = vec![0f32; logits.len()];
    for y in 0..side {
        for x in 0..side {
            let at = |x: usize| logits[y * side + x];
            across[y * side + x] = (at(x.saturating_sub(1)) + 2.0 * at(x) + at((x + 1).min(side - 1))) / 4.0;
        }
    }
    for y in 0..side {
        for x in 0..side {
            let at = |y: usize| across[y * side + x];
            logits[y * side + x] = (at(y.saturating_sub(1)) + 2.0 * at(y) + at((y + 1).min(side - 1))) / 4.0;
        }
    }
}

/// Takes specks out of the mask in `logits`, `side` square, and fills its
/// pinholes, smaller than `least` cells: the noise SAM leaves where it's
/// unsure
#[cfg_attr(not(birefnet), allow(dead_code))]
fn despeckle(logits: &mut [f32], side: usize, least: usize) {
    let mut seen = vec![false; logits.len()];
    let mut piece = Vec::new();
    for start in 0..logits.len() {
        if seen[start] {
            continue;
        }
        let inside = logits[start] > 0.0;
        seen[start] = true;
        piece.clear();
        piece.push(start);
        let mut edge = false;
        let mut next = 0;
        while next < piece.len() {
            let i = piece[next];
            next += 1;
            let (x, y) = ((i % side) as isize, (i / side) as isize);
            edge |= x == 0 || y == 0 || x as usize == side - 1 || y as usize == side - 1;
            // Pieces touch at corners, holes only at sides
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
                if !inside && dx != 0 && dy != 0 {
                    continue;
                }
                let (x, y) = (x + dx, y + dy);
                if x < 0 || y < 0 || x as usize >= side || y as usize >= side {
                    continue;
                }
                let j = y as usize * side + x as usize;
                if !seen[j] && (logits[j] > 0.0) == inside {
                    seen[j] = true;
                    piece.push(j);
                }
            }
        }
        if piece.len() < least && (inside || !edge) {
            for &i in &piece {
                logits[i] = if inside { -logits[i] } else { (-logits[i]).max(f32::MIN_POSITIVE) };
            }
        }
    }
}

#[cfg(birefnet)]
mod sam {
    use std::collections::HashMap;
    use std::io::Cursor;
    use std::path::Path;
    use std::sync::Mutex;

    use base64::Engine;
    use image::imageops::{self, FilterType};
    use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageReader, RgbaImage};
    use ort::session::builder::AutoDevicePolicy;
    use ort::session::Session;
    use ort::value::TensorRef;
    use tauri::AppHandle;

    use super::{despeckle, kept, smooth, Edge, Field, Pick, Request};
    use crate::models::{Missing, OBJECTS, SAM_DECODER, SAM_ENCODER};

    /// SAM takes the image stretched to a square this size, normalized as
    /// for ImageNet, and gives masks a quarter of it across, as logits
    const SIDE: usize = 1024;
    const MASK: usize = 256;
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];
    /// What the image encoder gives, for the mask decoder
    const EMBEDDINGS: [&str; 3] = ["image_embeddings.0", "image_embeddings.1", "image_embeddings.2"];
    /// Masks found, kept for the picks they were found for, so the outline
    /// of several picks only finds the new one
    const KEPT: usize = 64;
    /// Specks and pinholes smaller than this many of a mask's cells are
    /// left out of it
    const SPECK: usize = MASK * MASK / 4000;

    struct Image {
        session: u64,
        width: usize,
        height: usize,
        /// Where the image shows, in its own pixels
        opaque: Vec<bool>,
        embeddings: Vec<(Vec<i64>, Vec<f32>)>,
        masks: HashMap<String, Vec<f32>>,
    }

    struct Loaded {
        decoder: Session,
        image: Image,
    }

    static LOADED: Mutex<Option<Loaded>> = Mutex::new(None);

    fn failed(error: impl std::fmt::Display) -> &'static str {
        log::warn!("Select object: {error}");
        "failed"
    }

    pub fn unload() {
        *LOADED.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    pub fn handle(app: &AppHandle, tab: u32, job: u64, request: Request, image: &[u8]) -> Result<String, &'static str> {
        match request.op.as_str() {
            "open" => open(app, tab, job, image),
            "close" => {
                let mut loaded = LOADED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if loaded.as_ref().is_some_and(|loaded| loaded.image.session == request.session) {
                    *loaded = None;
                }
                Ok("{}".to_owned())
            }
            op => {
                let mut loaded = LOADED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let loaded = loaded.as_mut().filter(|loaded| loaded.image.session == request.session).ok_or("closed")?;
                match op {
                    "outline" => outline(loaded, &request.picks, request.edge),
                    "isolate" => isolate(loaded, &request.picks, request.edge, image),
                    _ => Err("failed"),
                }
            }
        }
    }

    /// Reads the image in `bytes` for the requests after, as session `job`,
    /// downloading the model first if need be
    fn open(app: &AppHandle, tab: u32, job: u64, bytes: &[u8]) -> Result<String, &'static str> {
        let (_held, folder) = OBJECTS.ensure(app, tab, job).map_err(|missing| match missing {
            Missing::Cancelled => "cancelled",
            Missing::Failed => "failed",
        })?;
        let picture = decode(bytes)?;
        let (width, height) = (picture.width() as usize, picture.height() as usize);
        let plane = SIDE * SIDE;
        let mut input = vec![0f32; 3 * plane];
        let square = imageops::resize(&picture, SIDE as u32, SIDE as u32, FilterType::Triangle);
        for (x, y, pixel) in square.enumerate_pixels() {
            let at = y as usize * SIDE + x as usize;
            for channel in 0..3 {
                input[channel * plane + at] = (pixel[channel] as f32 / 255.0 - MEAN[channel]) / STD[channel];
            }
        }
        let embeddings = {
            // The encoder takes hundreds of MB while it runs, so it's let go
            let mut encoder = session(&folder, SAM_ENCODER.file)?;
            let tensor = TensorRef::from_array_view(([1usize, 3, SIDE, SIDE], &*input)).map_err(failed)?;
            let outputs = encoder.run(ort::inputs![tensor]).map_err(failed)?;
            EMBEDDINGS
                .iter()
                .map(|name| {
                    let (shape, data) = outputs[*name].try_extract_tensor::<f32>().map_err(failed)?;
                    Ok((shape.to_vec(), data.to_vec()))
                })
                .collect::<Result<Vec<_>, &str>>()?
        };
        let image = Image {
            session: job,
            width,
            height,
            opaque: picture.pixels().map(|pixel| pixel[3] > 0).collect(),
            embeddings,
            masks: HashMap::new(),
        };
        let mut loaded = LOADED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let decoder = match loaded.take() {
            Some(Loaded { decoder, .. }) => decoder,
            None => session(&folder, SAM_DECODER.file)?,
        };
        *loaded = Some(Loaded { decoder, image });
        Ok(format!(r#"{{"session":{job},"width":{width},"height":{height}}}"#))
    }

    fn session(folder: &Path, file: &str) -> Result<Session, &'static str> {
        Session::builder(crate::models::environment(folder).map_err(failed)?)
            .map_err(failed)?
            .with_auto_device(AutoDevicePolicy::PreferCPU)
            .map_err(failed)?
            .commit_from_file(folder.join(file))
            .map_err(failed)
    }

    /// The image in `bytes`, turned as its orientation says
    fn decode(bytes: &[u8]) -> Result<RgbaImage, &'static str> {
        let mut decoder = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(failed)?
            .into_decoder()
            .map_err(failed)?;
        let orientation = decoder.orientation().map_err(failed)?;
        let mut image = DynamicImage::from_decoder(decoder).map_err(failed)?;
        image.apply_orientation(orientation);
        Ok(image.into_rgba8())
    }

    /// The picks over an image `width` by `height`, each added to those
    /// before it or taken away from them. SAM's masks are combined in its
    /// logits, as far as they can be, and scaled up together, then strokes
    /// and lassos with them
    fn picked(loaded: &mut Loaded, picks: &[Pick], width: usize, height: usize) -> Result<Field, &'static str> {
        let mut field: Option<Field> = None;
        // Masks since the last stroke, to add to `field`
        let mut masks: Option<Vec<f32>> = None;
        let flush = |field: &mut Option<Field>, masks: &mut Option<Vec<f32>>| {
            let Some(masks) = masks.take() else { return };
            let stretched = stretched(&masks, width, height);
            match field {
                Some(field) => field.combine(&stretched, false),
                None => *field = Some(Field { values: stretched, width, height }),
            }
        };
        for pick in picks {
            if let Some(points) = &pick.stroke {
                flush(&mut field, &mut masks);
                if field.is_none() && pick.subtract {
                    continue;
                }
                field.get_or_insert_with(|| Field::empty(width, height)).paint(points, pick.radius * width as f32, pick.subtract);
                continue;
            }
            if let Some(points) = &pick.lasso {
                flush(&mut field, &mut masks);
                if field.is_none() && pick.subtract {
                    continue;
                }
                field.get_or_insert_with(|| Field::empty(width, height)).lasso(points, pick.subtract);
                continue;
            }
            let mask = mask(loaded, pick)?;
            if pick.subtract && field.is_some() {
                flush(&mut field, &mut masks);
                if let Some(field) = &mut field {
                    field.combine(&stretched(&mask, width, height), true);
                }
                continue;
            }
            masks = Some(match masks {
                None if pick.subtract => continue,
                None => mask,
                Some(mut masks) => {
                    for (logit, &other) in masks.iter_mut().zip(&mask) {
                        *logit = if pick.subtract { logit.min(-other) } else { logit.max(other) };
                    }
                    masks
                }
            });
        }
        flush(&mut field, &mut masks);
        field.ok_or("empty")
    }

    /// The mask SAM finds for `pick`, `MASK` square, in logits, smoothed,
    /// without specks or pinholes. Of the three it gives, from the whole to
    /// a part, the one it scores best
    fn mask(loaded: &mut Loaded, pick: &Pick) -> Result<Vec<f32>, &'static str> {
        let scale = SIDE as f32;
        let key = match (pick.point, pick.rect) {
            (Some([x, y]), _) => format!("{:.0},{:.0}", x * scale, y * scale),
            (None, Some(rect)) => format!("{:.0},{:.0},{:.0},{:.0}", rect[0] * scale, rect[1] * scale, rect[2] * scale, rect[3] * scale),
            (None, None) => return Err("failed"),
        };
        if let Some(mask) = loaded.image.masks.get(&key) {
            return Ok(mask.clone());
        }
        // A box is a box with no points, for which the decoder takes one
        // that's only padding
        let (points, labels, boxes): ([f32; 2], i64, Vec<f32>) = match (pick.point, pick.rect) {
            (Some([x, y]), _) => ([x * scale, y * scale], 1, Vec::new()),
            (None, Some(rect)) => ([0.0; 2], -1, rect.iter().map(|value| value * scale).collect()),
            (None, None) => unreachable!(),
        };
        let labels = [labels];
        let image = &loaded.image;
        let mut inputs = ort::inputs![
            "input_points" => TensorRef::from_array_view(([1usize, 1, 1, 2], &points[..])).map_err(failed)?,
            "input_labels" => TensorRef::from_array_view(([1usize, 1, 1], &labels[..])).map_err(failed)?,
            "input_boxes" => TensorRef::from_array_view(([1usize, boxes.len() / 4, 4], &*boxes)).map_err(failed)?,
        ];
        for (name, (shape, data)) in EMBEDDINGS.iter().zip(&image.embeddings) {
            let shape: Vec<usize> = shape.iter().map(|&size| size as usize).collect();
            inputs.push((
                (*name).into(),
                TensorRef::from_array_view((shape, &data[..])).map_err(failed)?.into(),
            ));
        }
        let outputs = loaded.decoder.run(inputs).map_err(failed)?;
        let (_, scores) = outputs["iou_scores"].try_extract_tensor::<f32>().map_err(failed)?;
        let (shape, masks) = outputs["pred_masks"].try_extract_tensor::<f32>().map_err(failed)?;
        let side = MASK * MASK;
        if shape.last() != Some(&(MASK as i64)) || masks.len() < scores.len() * side || scores.is_empty() {
            return Err(failed("unexpected output"));
        }
        let best = (0..scores.len()).max_by(|&a, &b| scores[a].total_cmp(&scores[b])).unwrap_or(0);
        let mut mask = masks[best * side..(best + 1) * side].to_vec();
        drop(outputs);
        smooth(&mut mask, MASK);
        despeckle(&mut mask, MASK, SPECK);
        let masks = &mut loaded.image.masks;
        if masks.len() >= KEPT {
            masks.clear();
        }
        masks.insert(key, mask.clone());
        Ok(mask)
    }

    /// `logits`, `MASK` square, stretched over an image `width` by `height`,
    /// smoothly, as SAM scales its masks up
    fn stretched(logits: &[f32], width: usize, height: usize) -> Vec<f32> {
        let along = |at: usize, size: usize| {
            let position = ((at as f32 + 0.5) * MASK as f32 / size as f32 - 0.5).clamp(0.0, (MASK - 1) as f32);
            let low = position.floor() as usize;
            (low, (low + 1).min(MASK - 1), position - low as f32)
        };
        let columns: Vec<_> = (0..width).map(|x| along(x, width)).collect();
        let mut out = Vec::with_capacity(width * height);
        for y in 0..height {
            let (top, bottom, down) = along(y, height);
            for &(left, right, across) in &columns {
                let row = |y: usize| logits[y * MASK + left] * (1.0 - across) + logits[y * MASK + right] * across;
                out.push(row(top) * (1.0 - down) + row(bottom) * down);
            }
        }
        out
    }

    /// The outline of the picks over the image, with the edge moved as
    /// `edge` says, where the image shows
    fn outline(loaded: &mut Loaded, picks: &[Pick], edge: Edge) -> Result<String, &'static str> {
        let (width, height) = (loaded.image.width, loaded.image.height);
        let field = picked(loaded, picks, width, height)?;
        let inside: Vec<bool> = if edge.shift == 0.0 {
            field.values.iter().map(|&value| value > 0.0).collect()
        } else {
            field.distances().iter().map(|&distance| distance + edge.shift > 0.0).collect()
        };
        let mask: Vec<bool> = inside.iter().zip(&loaded.image.opaque).map(|(&inside, &opaque)| inside && opaque).collect();
        let d = crate::vectorize::outline(&mask, width, height, 4);
        Ok(serde_json::json!({ "d": d }).to_string())
    }

    /// The image in `bytes`, the layer drawn larger, with only what's
    /// picked, its edge moved and softened as `edge` says, cropped to it.
    /// The edge is antialiased, however large the image
    fn isolate(loaded: &mut Loaded, picks: &[Pick], edge: Edge, bytes: &[u8]) -> Result<String, &'static str> {
        let mut picture = decode(bytes)?;
        let (width, height) = (picture.width() as usize, picture.height() as usize);
        let distances = picked(loaded, picks, width, height)?.distances();
        let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
        for (i, (pixel, &distance)) in picture.pixels_mut().zip(&distances).enumerate() {
            pixel[3] = (pixel[3] as f32 * kept(distance, edge)).round() as u8;
            if pixel[3] > 0 {
                let (x, y) = (i % width, i / width);
                (left, top, right, bottom) = (left.min(x), top.min(y), right.max(x + 1), bottom.max(y + 1));
            }
        }
        if left >= right {
            return Err("empty");
        }
        let cropped = imageops::crop_imm(&picture, left as u32, top as u32, (right - left) as u32, (bottom - top) as u32).to_image();
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(cropped.as_raw(), cropped.width(), cropped.height(), image::ExtendedColorType::Rgba8)
            .map_err(failed)?;
        let (width, height) = (width as f64, height as f64);
        Ok(serde_json::json!({
            "png": base64::engine::general_purpose::STANDARD.encode(png),
            "x": left as f64 / width,
            "y": top as f64 / height,
            "width": (right - left) as f64 / width,
            "height": (bottom - top) as f64 / height,
        })
        .to_string())
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    /// A disc of radius 20 in a 64 square, painted
    fn disc() -> Field {
        let mut field = Field::empty(64, 64);
        field.paint(&[[0.5, 0.5]], 20.0, false);
        field
    }

    #[test]
    fn measures_distances_from_the_edge() {
        let distances = disc().distances();
        let at = |x: usize, y: usize| distances[y * 64 + x];
        // Pixel centers are half a pixel in, so (32, 32) is 19.5 inside,
        // and away from the edge it's to within a pixel
        assert!((at(32, 32) - 19.5).abs() < 1.0, "{}", at(32, 32));
        assert!((at(52, 32) + 0.5).abs() < 0.1, "{}", at(52, 32));
        assert!((at(51, 32) - 0.5).abs() < 0.1, "{}", at(51, 32));
        assert!((at(62, 32) + 10.5).abs() < 0.6, "{}", at(62, 32));
    }

    #[test]
    fn edges_are_hard_unless_feathered() {
        let edge = Edge::default();
        assert_eq!(kept(3.0, edge), 1.0);
        assert_eq!(kept(-3.0, edge), 0.0);
        assert_eq!(kept(0.0, edge), 0.5);
        let shifted = Edge { shift: 2.0, feather: 0.0 };
        assert_eq!(kept(-1.0, shifted), 1.0);
        assert_eq!(kept(-3.0, shifted), 0.0);
        let feathered = Edge { shift: 0.0, feather: 4.0 };
        assert!(kept(-2.0, feathered) > 0.0 && kept(-2.0, feathered) < 0.5);
        assert!(kept(2.0, feathered) > 0.5 && kept(2.0, feathered) < 1.0);
        assert_eq!(kept(5.0, feathered), 1.0);
    }

    #[test]
    fn strokes_add_and_take_away() {
        let mut field = disc();
        field.paint(&[[0.5, 0.0], [0.5, 1.0]], 4.0, true);
        let inside = |field: &Field, x: usize, y: usize| field.values[y * 64 + x] > 0.0;
        assert!(inside(&field, 20, 32) && inside(&field, 44, 32));
        assert!(!inside(&field, 32, 32) && !inside(&field, 30, 20));
        field.paint(&[[0.0, 0.05], [1.0, 0.05]], 1.0, false);
        assert!(inside(&field, 5, 3) && inside(&field, 60, 3) && !inside(&field, 5, 6));
    }

    #[test]
    fn lassos_add_and_take_away() {
        // A triangle, then a square taken out of its middle
        let mut field = Field::empty(64, 64);
        field.lasso(&[[0.1, 0.9], [0.5, 0.1], [0.9, 0.9]], false);
        let inside = |field: &Field, x: usize, y: usize| field.values[y * 64 + x] > 0.0;
        assert!(inside(&field, 32, 40) && inside(&field, 10, 56) && inside(&field, 53, 56));
        assert!(!inside(&field, 10, 20) && !inside(&field, 32, 60) && !inside(&field, 2, 2));
        field.lasso(&[[0.4, 0.5], [0.6, 0.5], [0.6, 0.7], [0.4, 0.7]], true);
        assert!(!inside(&field, 32, 40) && inside(&field, 20, 50));
        // Near the outline, how far in to a fraction of a pixel: the
        // center of (32, 57) is 0.1 above the bottom edge, at 57.6
        let distances = field.distances();
        assert!((distances[57 * 64 + 32] - 0.1).abs() < 0.15, "{}", distances[57 * 64 + 32]);
        // Off the image, it's left as it was
        field.lasso(&[[1.5, 1.5], [2.0, 1.5], [2.0, 2.0]], false);
        assert!(!inside(&field, 63, 63));
    }

    #[test]
    fn despeckles_masks() {
        let side = 16;
        let mut logits = vec![-1.0f32; side * side];
        // A block with a pinhole, and a speck
        for y in 4..12 {
            for x in 4..12 {
                logits[y * side + x] = 1.0;
            }
        }
        logits[7 * side + 7] = -1.0;
        logits[side + 14] = 1.0;
        despeckle(&mut logits, side, 4);
        assert!(logits[7 * side + 7] > 0.0);
        assert!(logits[side + 14] < 0.0);
        assert!(logits[4 * side + 4] > 0.0 && logits[0] < 0.0);
    }
}
