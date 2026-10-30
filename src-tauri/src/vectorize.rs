//! Vectorize on this device (Settings > Features > Images): the image bar's
//! Vectorize in free projects, which Ligma adds in place of the one in
//! Figma's More menu (see ai_actions.rs). Figma's sends the layer as a PNG
//! to its servers and gets back an SVG of flat-colored shapes stacked from
//! the back. This traces it the same way here: visioncortex (MIT or Apache
//! 2.0), the library behind VTracer, clusters pixels by color,
//! hierarchically, into stacked shapes, and their outlines are fitted with
//! cubic curves much as Potrace does (see `curve`)
//!
//! First the image is reduced to a palette, by median cut, so that:
//! - soft gradients become bands instead of one shape, as with Figma's
//! - the colors come from the inside of areas, not from the blends
//!   antialiasing makes along edges, so a logo keeps its few colors and an
//!   edge falls halfway through its blend. The palette stops growing once
//!   it's close enough to the image
//! - transparency is part of a color, so see-through areas, like shadows,
//!   keep their opacity
//!
//! Then each pixel takes the color most of its neighbors have, which
//! takes out specks and small steps along edges before they're traced
//!
//! The page hands the PNG over and gets back JSON: the SVG, and how many
//! pixels of each color show, for the recolor bar (see handoff.rs)

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write;
use std::io::Cursor;

use image::ImageReader;
use visioncortex::color_clusters::{KeyingAction, Runner, RunnerConfig, HIERARCHICAL_MAX};
use visioncortex::{Color, ColorImage, CompoundPathElement, PathSimplifyMode, PointI32};

/// Beyond this many shapes Figma slows down, so the image is traced again
/// with coarser settings, up to the last of `LEVELS`
const MAX_PATHS: usize = 2500;

/// How finely to trace, from finest: how many colors the palette may have
/// and the smallest shape's side in pixels
const LEVELS: [(usize, usize); 4] = [(32, 4), (24, 6), (16, 8), (8, 12)];

/// A pixel whose neighbors differ from it by more than this in a channel is
/// on an edge, and left out of the palette
const EDGE: u8 = 24;

/// The palette stops growing once each of its colors stands for pixels off
/// from it by less than this, on average
const CLOSE_ENOUGH: f64 = 7.0;

/// A color between two others that cover this many times as many pixels
/// as it, within this distance of the way from one to the other, is a
/// blend of them (see `without_blends`)
const BLEND_RATIO: usize = 4;
const BLEND_DISTANCE: f64 = 24.0;

/// Colors with less alpha than this are left out
const TRANSPARENT: f64 = 32.0;

/// Colors with more alpha than this are opaque
const OPAQUE: f64 = 243.0;

/// The PNG in `bytes` traced, as JSON: `svg`, the shapes stacked from the
/// back in an SVG the size of the image, and `areas`, how many pixels of
/// each fill show. `pixel` is how wide a pixel of the original image is in
/// the PNG, at least 1: an image drawn larger than it is has steps that
/// wide along its edges, which are smoothed as such
pub fn trace(bytes: &[u8], pixel: f64) -> Result<String, &'static str> {
    let fitting = Fitting::new(pixel);
    let failed = |error: &dyn std::fmt::Display| {
        log::warn!("Vectorize: {error}");
        "failed"
    };
    let image = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| failed(&error))?
        .decode()
        .map_err(|error| failed(&error))?
        .into_rgba8();
    let (width, height) = (image.width() as usize, image.height() as usize);
    let pixels: Vec<[u8; 4]> = image.as_raw().as_chunks::<4>().0.iter().map(|&pixel| premultiply(pixel)).collect();

    let mut traced = None;
    for (i, &(colors, speckle)) in LEVELS.iter().enumerate() {
        let palette = without_blends(palette(&pixels, width, colors), &pixels);
        let mut labels: Vec<u8> = pixels.iter().map(|pixel| nearest(&palette, pixel)).collect();
        smooth(&mut labels, width, palette.len());
        let speckle = (speckle as f64 * fitting.pixel).round() as usize;
        let shapes = shapes(&palette, &labels, width, height, speckle, &fitting);
        let done = shapes.count <= MAX_PATHS || i == LEVELS.len() - 1;
        traced = Some(shapes);
        if done {
            break;
        }
    }
    let Some(Shapes { paths, areas, count }) = traced else {
        return Err("empty");
    };
    if count == 0 {
        return Err("empty");
    }
    let svg = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">{paths}</svg>"#);
    Ok(serde_json::json!({ "svg": svg, "areas": areas }).to_string())
}

/// The pixel's color times its alpha, and its alpha, so that colors at
/// any alpha compare by how they look
fn premultiply([r, g, b, a]: [u8; 4]) -> [u8; 4] {
    let times = |channel: u8| ((u16::from(channel) * u16::from(a) + 127) / 255) as u8;
    [times(r), times(g), times(b), a]
}

/// A palette color, premultiplied as `premultiply` does
type Entry = [f64; 4];

/// The palette for `pixels`, `width` pixels a row: at most `colors`, by
/// median cut. The box of colors that strays furthest from its average is
/// split along its widest channel at its median pixel, until every box is
/// within `CLOSE_ENOUGH` of its average or there are `colors` boxes. Pixels
/// are binned at 4 bits a channel. Only pixels inside areas count, unless
/// there are hardly any
fn palette(pixels: &[[u8; 4]], width: usize, colors: usize) -> Vec<Entry> {
    let bin = |pixel: &[u8; 4]| pixel.iter().fold(0, |bin, &channel| bin << 4 | usize::from(channel >> 4));
    let close = |a: &[u8; 4], b: &[u8; 4]| (0..4).all(|c| a[c].abs_diff(b[c]) <= EDGE);
    let inside = |i: usize| {
        let (x, y) = (i % width, i / width);
        (x == 0 || close(&pixels[i], &pixels[i - 1]))
            && (x + 1 == width || close(&pixels[i], &pixels[i + 1]))
            && (y == 0 || close(&pixels[i], &pixels[i - width]))
            && (i + width >= pixels.len() || close(&pixels[i], &pixels[i + width]))
    };
    let mut counted: Vec<&[u8; 4]> = pixels.iter().enumerate().filter(|&(i, _)| inside(i)).map(|(_, pixel)| pixel).collect();
    if counted.len() < pixels.len() / 100 {
        counted = pixels.iter().collect();
    }
    let mut counts = vec![0f64; 1 << 16];
    let mut sums = vec![[0f64; 4]; 1 << 16];
    let mut squares = vec![[0f64; 4]; 1 << 16];
    for pixel in counted {
        let i = bin(pixel);
        counts[i] += 1.0;
        for c in 0..4 {
            let value = f64::from(pixel[c]);
            sums[i][c] += value;
            squares[i][c] += value * value;
        }
    }

    // A box's pixel count, sums and squares, per channel
    let stats = |bins: &[usize]| {
        let mut total = (0.0, [0.0; 4], [0.0; 4]);
        for &i in bins {
            total.0 += counts[i];
            for c in 0..4 {
                total.1[c] += sums[i][c];
                total.2[c] += squares[i][c];
            }
        }
        total
    };
    let variance = |(count, sum, square): (f64, [f64; 4], [f64; 4]), c: usize| (square[c] - sum[c] * sum[c] / count) / count;
    let spread = |bins: &[usize]| (0..4).map(|c| variance(stats(bins), c).max(0.0)).sum::<f64>();
    let mut boxes: Vec<Vec<usize>> = vec![(0..1 << 16).filter(|&i| counts[i] > 0.0).collect()];
    while boxes.len() < colors {
        let Some((index, worst)) = boxes
            .iter()
            .enumerate()
            .filter(|(_, bins)| bins.len() > 1)
            .map(|(index, bins)| (index, spread(bins)))
            .max_by(|a, b| (a.1 * stats(&boxes[a.0]).0).total_cmp(&(b.1 * stats(&boxes[b.0]).0)))
        else {
            break;
        };
        if worst.sqrt() < CLOSE_ENOUGH && boxes.iter().all(|bins| spread(bins).sqrt() < CLOSE_ENOUGH) {
            break;
        }
        let mut bins = boxes.swap_remove(index);
        let total = stats(&bins);
        let channel = (0..4).max_by(|&a, &b| variance(total, a).total_cmp(&variance(total, b))).unwrap_or(0);
        bins.sort_by(|&a, &b| (sums[a][channel] / counts[a]).total_cmp(&(sums[b][channel] / counts[b])));
        let mut seen = 0.0;
        let cut = bins
            .iter()
            .position(|&i| {
                seen += counts[i];
                seen > total.0 / 2.0
            })
            .unwrap_or(0)
            .clamp(1, bins.len() - 1);
        let upper = bins.split_off(cut);
        boxes.push(bins);
        boxes.push(upper);
    }
    boxes
        .iter()
        .map(|bins| {
            let (count, sum, _) = stats(bins);
            sum.map(|value| value / count)
        })
        .collect()
}

/// `palette` without the colors antialiasing makes where two others meet:
/// a color that lies between two others, on the way from one to the
/// other, and is closest to far fewer of `pixels` than either. Its pixels
/// go to the nearer of the two. Bands of a gradient lie between their
/// neighbors too, but cover about as much
fn without_blends(palette: Vec<Entry>, pixels: &[[u8; 4]]) -> Vec<Entry> {
    let mut counts = vec![0usize; palette.len()];
    for pixel in pixels {
        counts[usize::from(nearest(&palette, pixel))] += 1;
    }
    let mut kept: Vec<usize> = (0..palette.len()).filter(|&e| counts[e] > 0).collect();
    let mut order = kept.clone();
    order.sort_by_key(|&e| counts[e]);
    for e in order {
        let common: Vec<usize> = kept.iter().copied().filter(|&other| other != e && counts[other] >= counts[e] * BLEND_RATIO).collect();
        let ends = common.iter().flat_map(|&a| common.iter().map(move |&b| (a, b))).find(|&(a, b)| {
            let (from, to, color) = (palette[a], palette[b], palette[e]);
            let along: [f64; 4] = std::array::from_fn(|c| to[c] - from[c]);
            let length = along.iter().map(|v| v * v).sum::<f64>();
            if a == b || length == 0.0 {
                return false;
            }
            let t = (0..4).map(|c| (color[c] - from[c]) * along[c]).sum::<f64>() / length;
            let off = (0..4).map(|c| (color[c] - from[c] - t * along[c]).powi(2)).sum::<f64>().sqrt();
            (0.05..0.95).contains(&t) && off < BLEND_DISTANCE
        });
        if let Some((a, b)) = ends {
            let distance = |other: usize| (0..4).map(|c| (palette[other][c] - palette[e][c]).powi(2)).sum::<f64>();
            let closer = if distance(a) <= distance(b) { a } else { b };
            counts[closer] += counts[e];
            kept.retain(|&other| other != e);
        }
    }
    kept.into_iter().map(|e| palette[e]).collect()
}

/// Which color of `palette` is closest to `pixel`
fn nearest(palette: &[Entry], pixel: &[u8; 4]) -> u8 {
    let distance = |entry: &Entry| (0..4).map(|c| (entry[c] - f64::from(pixel[c])).powi(2)).sum::<f64>();
    (0..palette.len()).min_by(|&a, &b| distance(&palette[a]).total_cmp(&distance(&palette[b]))).unwrap_or(0) as u8
}

/// Gives each pixel the color most of the 3 × 3 pixels around it have,
/// keeping its own on a tie, so edges lose the steps and specks that would
/// make traced outlines wobble
fn smooth(labels: &mut [u8], width: usize, colors: usize) {
    let height = labels.len() / width;
    let source = labels.to_vec();
    let mut counts = vec![0u8; colors];
    for y in 0..height {
        for x in 0..width {
            counts.fill(0);
            for ny in y.saturating_sub(1)..(y + 2).min(height) {
                for nx in x.saturating_sub(1)..(x + 2).min(width) {
                    counts[usize::from(source[ny * width + nx])] += 1;
                }
            }
            let own = source[y * width + x];
            let mut best = own;
            for (label, &count) in counts.iter().enumerate() {
                if count > counts[usize::from(best)] {
                    best = label as u8;
                }
            }
            labels[y * width + x] = best;
        }
    }
}

/// Traced shapes, as SVG paths, how many pixels of each fill show and how
/// many shapes there are
struct Shapes {
    paths: String,
    areas: BTreeMap<String, u64>,
    count: usize,
}

/// The image of `labels`, colors of `palette` for `width` × `height`
/// pixels, traced, leaving out shapes smaller than `speckle` pixels a side
fn shapes(palette: &[Entry], labels: &[u8], width: usize, height: usize, speckle: usize, fitting: &Fitting) -> Shapes {
    // Each color as the tracer sees it, without alpha, made unique, or a
    // key it leaves out when it's mostly transparent
    let mut taken = HashSet::new();
    let mut colors: Vec<Option<[u8; 3]>> = Vec::with_capacity(palette.len());
    for entry in palette {
        if entry[3] < TRANSPARENT {
            colors.push(None);
            continue;
        }
        let mut rgb = [0, 1, 2].map(|c| (entry[c] * 255.0 / entry[3]).round().clamp(0.0, 255.0) as u8);
        while !taken.insert(rgb) {
            rgb[2] = rgb[2].wrapping_add(1);
        }
        colors.push(Some(rgb));
    }
    let key = (0u32..1 << 24)
        .map(|rgb| [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8])
        .find(|rgb| !taken.contains(rgb))
        .unwrap_or_default();
    let has_key = colors.iter().any(Option::is_none);
    let pixels = labels
        .iter()
        .flat_map(|&label| {
            let [r, g, b] = colors[usize::from(label)].unwrap_or(key);
            [r, g, b, 255]
        })
        .collect();

    let clusters = Runner::new(
        RunnerConfig {
            diagonal: false,
            hierarchical: HIERARCHICAL_MAX,
            batch_size: 25600,
            good_min_area: speckle * speckle,
            good_max_area: width * height,
            is_same_color_a: 0,
            is_same_color_b: 0,
            deepen_diff: 16,
            hollow_neighbours: 1,
            key_color: if has_key { Color::new(key[0], key[1], key[2]) } else { Color::default() },
            keying_action: KeyingAction::Discard,
        },
        ColorImage { pixels, width, height },
    )
    .run();
    let view = clusters.view();
    let mut shapes = Shapes { paths: String::new(), areas: BTreeMap::new(), count: 0 };
    for &index in view.clusters_output.iter().rev() {
        let cluster = view.get_cluster(index);
        let path = cluster.to_compound_path(&view, false, PathSimplifyMode::None, 0.0, 0.0, 1, 0.0);
        let mut d = String::new();
        for element in &path.paths {
            if let CompoundPathElement::PathI32(outline) = element {
                curve(&outline.path, fitting, &mut d);
            }
        }
        if d.is_empty() {
            continue;
        }
        // Shapes that took in specks of other colors are off a little, so
        // each takes the palette color nearest its own
        let own = cluster.residue_color();
        let distance = |rgb: &[u8; 3]| [own.r, own.g, own.b].iter().zip(rgb).map(|(&a, &b)| (i32::from(a) - i32::from(b)).pow(2)).sum::<i32>();
        let Some((label, rgb)) = colors.iter().enumerate().filter_map(|(label, rgb)| Some((label, (*rgb)?))).min_by_key(|(_, rgb)| distance(rgb)) else {
            continue;
        };
        let fill = format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]);
        let _ = write!(shapes.paths, r#"<path d="{d}" fill="{fill}""#);
        let alpha = palette[label][3];
        if alpha < OPAQUE {
            let _ = write!(shapes.paths, r#" fill-opacity="{:.3}""#, alpha / 255.0);
        }
        shapes.paths.push_str("/>");
        *shapes.areas.entry(fill).or_default() += u64::from(cluster.residue_sum.counter);
        shapes.count += 1;
    }
    shapes
}

/// An outline turning sharper than this, in degrees, has a corner there
const CORNER_ANGLE: f64 = 140.0;

/// How outlines are fitted, in pixels of the original image (see `trace`)
struct Fitting {
    pixel: f64,
    /// How far either way along an outline a corner is judged
    reach: usize,
    /// How far an edge may stray from straight and still be straightened
    /// (see `straighten`)
    straight: f64,
    /// How many times each stretch between corners is smoothed, after
    /// it's straightened, enough to even out steps a pixel wide
    smoothing: usize,
    /// How far curves may stray from the smoothed outline
    tolerance: f64,
}

impl Fitting {
    fn new(pixel: f64) -> Self {
        let pixel = if pixel.is_finite() { pixel.clamp(1.0, 8.0) } else { 1.0 };
        Self {
            pixel,
            reach: (4.0 * pixel).round() as usize,
            straight: 1.0,
            smoothing: (4.0 * pixel * pixel).round() as usize,
            tolerance: 0.8 * pixel,
        }
    }
}

type Point = [f64; 2];

fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn scale(a: Point, by: f64) -> Point {
    [a[0] * by, a[1] * by]
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn length(a: Point) -> f64 {
    dot(a, a).sqrt()
}

fn unit(a: Point) -> Point {
    let length = length(a);
    if length > 0.0 {
        scale(a, 1.0 / length)
    } else {
        a
    }
}

/// The closed outline through the pixel corners `outline` as smooth SVG
/// path commands, added to `d`, much as Potrace does it: corners are found
/// first, then each stretch between them is straightened where it's
/// straight but for the steps of the pixels, smoothed, and fitted with as
/// few cubic curves as stay within its tolerance
fn curve(outline: &[PointI32], fitting: &Fitting, d: &mut String) {
    let mut corners: Vec<Point> = outline.iter().map(|point| [f64::from(point.x), f64::from(point.y)]).collect();
    if corners.len() > 1 && corners.first() == corners.last() {
        corners.pop();
    }
    if corners.len() < 3 {
        return;
    }
    corners.push(corners[0]);
    let mut points = resample(&corners);
    points.pop();
    let n = points.len();
    if n < 3 {
        return;
    }

    // How far the outline turns at each point, in degrees, between the
    // points `reach` away either way, and the corners: the points where it
    // turns most within `apart` either way, and far enough
    let reach = fitting.reach.min((n - 1) / 4).max(1);
    let turns: Vec<f64> = (0..n)
        .map(|i| {
            let (before, after) = (sub(points[i], points[(i + n - reach) % n]), sub(points[(i + reach) % n], points[i]));
            let cos = dot(before, after) / (length(before) * length(after)).max(f64::EPSILON);
            cos.clamp(-1.0, 1.0).acos().to_degrees()
        })
        .collect();
    let apart = (reach / 2).max(2);
    let corner_at: Vec<usize> = (0..n)
        .filter(|&i| {
            turns[i] > 180.0 - CORNER_ANGLE && (1..=apart).all(|k| turns[i] >= turns[(i + k) % n] && turns[i] > turns[(i + n - k) % n])
        })
        .collect();

    d.push('M');
    if corner_at.is_empty() {
        // Round all the way: smoothed as a loop, from and back to its
        // first point, along the same tangent
        let mut loop_points = points.clone();
        loop_points.push(points[0]);
        let mut ring = resample(&straighten(&loop_points, fitting.straight));
        ring.pop();
        let n = ring.len();
        if n < 3 {
            d.pop();
            return;
        }
        for _ in 0..fitting.smoothing {
            ring = (0..n).map(|i| scale(add(add(ring[(i + n - 1) % n], scale(ring[i], 2.0)), ring[(i + 1) % n]), 0.25)).collect();
        }
        let tangent = unit(sub(ring[1], ring[n - 1]));
        ring.push(ring[0]);
        let _ = write!(d, "{:.2} {:.2}", ring[0][0], ring[0][1]);
        fit(&ring, tangent, scale(tangent, -1.0), fitting.tolerance, d);
    } else {
        let first = points[corner_at[0]];
        let _ = write!(d, "{:.2} {:.2}", first[0], first[1]);
        for (k, &start) in corner_at.iter().enumerate() {
            let end = corner_at[(k + 1) % corner_at.len()];
            let count = match (end + n - start) % n {
                0 => n,
                count => count,
            };
            let raw: Vec<Point> = (0..=count).map(|j| points[(start + j) % n]).collect();
            // Straight from corner to corner but for the steps of the
            // original pixels: a line
            if straighten(&raw, fitting.pixel).len() == 2 {
                let end = raw[raw.len() - 1];
                let _ = write!(d, "L{:.2} {:.2}", end[0], end[1]);
                continue;
            }
            let mut stretch = resample(&straighten(&raw, fitting.straight));
            let last = stretch.len() - 1;
            for _ in 0..fitting.smoothing {
                stretch = (0..=last)
                    .map(|i| if i == 0 || i == last { stretch[i] } else { scale(add(add(stretch[i - 1], scale(stretch[i], 2.0)), stretch[i + 1]), 0.25) })
                    .collect();
            }
            let (ahead, behind) = (stretch[2.min(last)], stretch[last.saturating_sub(2)]);
            fit(&stretch, unit(sub(ahead, stretch[0])), unit(sub(behind, stretch[last])), fitting.tolerance, d);
        }
    }
    d.push('Z');
}

/// The polyline through `points` with a point about every pixel along it
fn resample(points: &[Point]) -> Vec<Point> {
    let mut resampled = Vec::with_capacity(points.len());
    for pair in points.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let steps = length(sub(to, from)).round().max(1.0) as usize;
        resampled.extend((0..steps).map(|step| add(from, scale(sub(to, from), step as f64 / steps as f64))));
    }
    resampled.extend(points.last());
    resampled
}

/// `points` with the points dropped that lie within `straight` of a line
/// through those kept, by Douglas and Peucker's algorithm, so a staircase
/// of pixels along a slanted edge becomes the line it stands for
fn straighten(points: &[Point], straight: f64) -> Vec<Point> {
    let last = points.len() - 1;
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[last] = true;
    let mut stack = vec![(0, last)];
    while let Some((from, to)) = stack.pop() {
        let along = sub(points[to], points[from]);
        let span = length(along);
        let off = |point: Point| {
            let to_point = sub(point, points[from]);
            if span < f64::EPSILON {
                length(to_point)
            } else {
                (to_point[0] * along[1] - to_point[1] * along[0]).abs() / span
            }
        };
        let farthest = (from + 1..to).max_by(|&a, &b| off(points[a]).total_cmp(&off(points[b])));
        if let Some(i) = farthest.filter(|&i| off(points[i]) > straight) {
            keep[i] = true;
            stack.push((from, i));
            stack.push((i, to));
        }
    }
    points.iter().zip(keep).filter(|(_, keep)| *keep).map(|(&point, _)| point).collect()
}

/// Cubic curves through `points`, leaving and arriving along the unit
/// tangents given, added to `d` as SVG `C` commands, by Philip J.
/// Schneider's algorithm (Graphics Gems, 1990): one curve is fitted by
/// least squares and split where it strays most, until it's within
/// `tolerance`
fn fit(points: &[Point], start: Point, end: Point, tolerance: f64, d: &mut String) {
    let last = points.len() - 1;
    let write = |d: &mut String, [_, a, b, c]: [Point; 4]| {
        let _ = write!(d, "C{:.2} {:.2} {:.2} {:.2} {:.2} {:.2}", a[0], a[1], b[0], b[1], c[0], c[1]);
    };
    if last < 2 {
        let third = length(sub(points[last], points[0])) / 3.0;
        write(d, [points[0], add(points[0], scale(start, third)), add(points[last], scale(end, third)), points[last]]);
        return;
    }
    // Each point's place along the curve, by distance
    let mut t = vec![0.0; points.len()];
    for i in 1..=last {
        t[i] = t[i - 1] + length(sub(points[i], points[i - 1]));
    }
    let total = t[last].max(f64::EPSILON);
    t.iter_mut().for_each(|value| *value /= total);

    let mut worst = last / 2;
    for _ in 0..4 {
        let bezier = least_squares(points, &t, start, end);
        let (error, at) = stray(points, &t, &bezier);
        if error <= tolerance * tolerance {
            write(d, bezier);
            return;
        }
        worst = at;
        // Near enough to be worth moving each point's place to where the
        // curve passes closest, by a step of Newton's method, and trying
        // again
        if error > 16.0 * tolerance * tolerance {
            break;
        }
        for i in 0..=last {
            t[i] = closer(&bezier, points[i], t[i]);
        }
    }
    let split = worst.clamp(1, last - 1);
    let middle = unit(sub(points[split - 1], points[split + 1]));
    fit(&points[..=split], start, middle, tolerance, d);
    fit(&points[split..], scale(middle, -1.0), end, tolerance, d);
}

fn at(bezier: &[Point; 4], t: f64) -> Point {
    let u = 1.0 - t;
    let [a, b, c, e] = *bezier;
    add(add(scale(a, u * u * u), scale(b, 3.0 * u * u * t)), add(scale(c, 3.0 * u * t * t), scale(e, t * t * t)))
}

/// The cubic from the first to the last of `points`, along the tangents,
/// with the handle lengths that best fit the points at places `t`
fn least_squares(points: &[Point], t: &[f64], start: Point, end: Point) -> [Point; 4] {
    let (first, last) = (points[0], points[points.len() - 1]);
    let (mut c00, mut c01, mut c11, mut x0, mut x1) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (&point, &t) in points.iter().zip(t) {
        let u = 1.0 - t;
        let (b0, b1, b2, b3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        let (a1, a2) = (scale(start, b1), scale(end, b2));
        c00 += dot(a1, a1);
        c01 += dot(a1, a2);
        c11 += dot(a2, a2);
        let rest = sub(point, add(scale(first, b0 + b1), scale(last, b2 + b3)));
        x0 += dot(a1, rest);
        x1 += dot(a2, rest);
    }
    let determinant = c00 * c11 - c01 * c01;
    let chord = length(sub(last, first));
    let (mut alpha, mut beta) = if determinant.abs() > 1e-12 {
        ((x0 * c11 - x1 * c01) / determinant, (c00 * x1 - c01 * x0) / determinant)
    } else {
        (chord / 3.0, chord / 3.0)
    };
    // Handles that are tiny or turn back make loops: a third of the way
    // instead
    if alpha < chord * 1e-3 || beta < chord * 1e-3 {
        (alpha, beta) = (chord / 3.0, chord / 3.0);
    }
    [first, add(first, scale(start, alpha)), add(last, scale(end, beta)), last]
}

/// How far, squared, the farthest of `points` is from `bezier` at its
/// place, and which point that is
fn stray(points: &[Point], t: &[f64], bezier: &[Point; 4]) -> (f64, usize) {
    let mut worst = (0.0, points.len() / 2);
    for (i, (&point, &t)) in points.iter().zip(t).enumerate() {
        let off = sub(at(bezier, t), point);
        if dot(off, off) > worst.0 {
            worst = (dot(off, off), i);
        }
    }
    worst
}

/// `t` moved toward the place on `bezier` nearest `point`, by a step of
/// Newton's method
fn closer(bezier: &[Point; 4], point: Point, t: f64) -> f64 {
    let [a, b, c, e] = *bezier;
    let first = [scale(sub(b, a), 3.0), scale(sub(c, b), 3.0), scale(sub(e, c), 3.0)];
    let second = [scale(sub(first[1], first[0]), 2.0), scale(sub(first[2], first[1]), 2.0)];
    let u = 1.0 - t;
    let d1 = add(add(scale(first[0], u * u), scale(first[1], 2.0 * u * t)), scale(first[2], t * t));
    let d2 = add(scale(second[0], u), scale(second[1], t));
    let off = sub(at(bezier, t), point);
    let denominator = dot(d1, d1) + dot(off, d2);
    if denominator.abs() < 1e-12 {
        t
    } else {
        (t - dot(off, d1) / denominator).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(width, height, |x, y| image::Rgba(pixel(x, y)));
        let mut bytes = Vec::new();
        image.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        bytes
    }

    fn svg(bytes: &[u8]) -> String {
        let traced: serde_json::Value = serde_json::from_str(&trace(bytes, 1.0).unwrap()).unwrap();
        traced["svg"].as_str().unwrap().to_owned()
    }

    #[test]
    fn traces_shapes_in_their_colors() {
        let bytes = png(64, 64, |x, y| {
            if (x as i32 - 32).pow(2) + (y as i32 - 32).pow(2) < 400 {
                [255, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            }
        });
        let svg = svg(&bytes);
        assert!(svg.starts_with(r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64""#));
        assert!(svg.contains(r##"fill="#FF0000""##), "{svg}");
        assert!(svg.contains(r##"fill="#FFFFFF""##), "{svg}");
    }

    #[test]
    fn leaves_out_edge_blends() {
        // A red square on white, with a soft edge
        let bytes = png(64, 64, |x, _| match x {
            0..30 => [255, 0, 0, 255],
            30..34 => [255, (x - 29) as u8 * 51, (x - 29) as u8 * 51, 255],
            _ => [255, 255, 255, 255],
        });
        let svg = svg(&bytes);
        assert_eq!(svg.matches("<path").count(), 2, "{svg}");
    }

    #[test]
    fn reports_areas() {
        let bytes = png(10, 10, |x, _| if x < 5 { [0, 0, 255, 255] } else { [255, 255, 0, 255] });
        let traced: serde_json::Value = serde_json::from_str(&trace(&bytes, 1.0).unwrap()).unwrap();
        assert_eq!(traced["areas"]["#0000FF"], 50);
        assert_eq!(traced["areas"]["#FFFF00"], 50);
    }

    #[test]
    fn bands_gradients() {
        let bytes = png(256, 32, |x, _| [x as u8, 128, 255 - x as u8, 255]);
        let svg = svg(&bytes);
        assert!(svg.matches("<path").count() >= 8, "{svg}");
    }

    #[test]
    fn leaves_out_transparency() {
        let bytes = png(64, 64, |x, y| if (16..48).contains(&x) && (16..48).contains(&y) { [0, 0, 255, 255] } else { [0, 0, 0, 0] });
        let svg = svg(&bytes);
        assert_eq!(svg.matches("<path").count(), 1, "{svg}");
        assert!(svg.contains(r##"fill="#0000FF""##), "{svg}");
    }

    #[test]
    fn keeps_partial_transparency() {
        let bytes = png(64, 64, |x, y| if (16..48).contains(&x) && (16..48).contains(&y) { [0, 0, 0, 64] } else { [0, 0, 0, 0] });
        let svg = svg(&bytes);
        assert!(svg.contains(r#"fill-opacity="0.251""#), "{svg}");
    }

    #[test]
    fn fully_transparent_is_empty() {
        let bytes = png(16, 16, |_, _| [0, 0, 0, 0]);
        assert_eq!(trace(&bytes, 1.0), Err("empty"));
    }
}
