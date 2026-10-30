//! Vectorize on this device (Settings > Features > Images): the image bar's
//! Vectorize in free projects, which Ligma adds in place of the one in
//! Figma's More menu (see ai_actions.rs). Figma's sends the layer as a PNG
//! to its servers and gets back an SVG of flat-colored shapes, each a
//! single outline, stacked from the back. This makes the same here:
//!
//! 1. The image is smoothed where it's alike, keeping its edges, so noise
//!    doesn't make ragged shapes
//! 2. Blends, where antialiasing or a soft edge goes from one color to
//!    another, are set aside, to go to whichever side they look most like
//!    at the end, so edges fall halfway through them
//! 3. The rest is split into many small regions of alike pixels, by
//!    Felzenszwalb and Huttenlocher's graph-based segmentation
//! 4. Neighboring regions are merged while they look alike, cheapest
//!    first, by Ward's method in CIELAB: alike colors and small regions go
//!    first, so a soft gradient ends up in bands. What's left too small,
//!    and slivers along edges, go into their nearest neighbor
//! 5. Edges are moved to where the image changes from one region to the
//!    next, as the merging leaves them ragged, and thin bands between two
//!    colors are taken out
//! 6. Each region takes its average color, and regions of nearly the same
//!    color share one
//! 7. Each region is drawn as one filled outline, over the regions it lies
//!    in, which are drawn first, and reaches a little under the regions
//!    drawn over it, so no seams show between them. Its outline is fitted
//!    with cubic curves much as Potrace does (see `curve`), with
//!    visioncortex (MIT or Apache 2.0) finding it in the pixels
//!
//! Transparency is part of each color, so see-through areas, like shadows,
//! keep their opacity, and transparent ones are left out
//!
//! The page hands the PNG over and gets back JSON: the SVG, and how many
//! pixels of each color show, for the recolor bar (see handoff.rs)

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap};
use std::fmt::Write;
use std::io::Cursor;

use image::ImageReader;
use visioncortex::clusters::Cluster;
use visioncortex::{BinaryImage, PathSimplifyMode, PointI32};

/// Beyond this many shapes Figma slows down, so the image is traced again,
/// merging more, `LEVELS` times as much at most
const MAX_PATHS: usize = 2500;
const LEVELS: [f64; 4] = [1.0, 3.0, 9.0, 27.0];

/// How alike pixels must be, in CIELAB units, to start in one region
const ALIKE: f32 = 12.0;

/// How far apart, in CIELAB units, regions may look and still start as one
const DRIFT: f32 = 4.0;

/// How costly a merge may be, by Ward's method, in CIELAB units squared
/// over the image (see `segment`)
const MERGE: f64 = 0.0025;

/// The smallest region kept, as a share of the image
const SMALLEST: f64 = 2.5e-5;

/// Regions under this many pixels across on average, whose colors lie
/// within `BLEND_OFF` CIELAB units of the way between two of their
/// neighbors', are antialiasing (see `dissolve_blends`)
const THIN: f64 = 4.0;
const BLEND_OFF: f64 = 12.0;

/// How close in CIELAB units a sliver is to the neighbor it joins (see
/// `segment`)
const SLIVER: f64 = 16.0;
const SLIVER_WIDTH: f64 = 3.0;

/// How far either side of a pixel, in pixels, and how different in CIELAB
/// units, the colors it may be a blend of are (see `blend_pixels`)
const BLEND_REACH: isize = 5;
const CONTRAST: f32 = 20.0;

/// How many times edges are moved to where the image changes (see
/// `settle`)
const SETTLE: usize = 8;

/// Regions this close in CIELAB units share a color
const SAME: f64 = 5.0;

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
    if width == 0 || height == 0 {
        return Err("empty");
    }
    let pixels: Vec<[u8; 4]> = image.as_raw().as_chunks::<4>().0.iter().map(|&pixel| premultiply(pixel)).collect();
    let looks = smoothed(&pixels.iter().map(look).collect::<Vec<_>>(), width);
    let start = oversegment(&looks, width);
    let blends = blend_pixels(&looks, width);

    let mut traced = None;
    for (i, &coarser) in LEVELS.iter().enumerate() {
        let regions = segment(&pixels, &looks, &start, &blends, width, coarser * fitting.pixel * fitting.pixel);
        let shapes = shapes(&regions, width, height, &fitting);
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

/// The pixel's color times its alpha, and its alpha
fn premultiply([r, g, b, a]: [u8; 4]) -> [u8; 4] {
    let times = |channel: u8| ((u16::from(channel) * u16::from(a) + 127) / 255) as u8;
    [times(r), times(g), times(b), a]
}

/// How a premultiplied pixel looks: its color in CIELAB, faded by its
/// alpha, and its alpha on the same scale as lightness, so transparent
/// pixels are alike whatever their color, and colors compare by how
/// different they look
fn look(&[r, g, b, a]: &[u8; 4]) -> [f32; 4] {
    if a == 0 {
        return [0.0; 4];
    }
    let alpha = f32::from(a) / 255.0;
    let linear = |channel: u8| {
        let value = (f32::from(channel) / 255.0 / alpha).min(1.0);
        if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
    };
    let (r, g, b) = (linear(r), linear(g), linear(b));
    let f = |t: f32| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let x = f((0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047);
    let y = f(0.2126 * r + 0.7152 * g + 0.0722 * b);
    let z = f((0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883);
    [(116.0 * y - 16.0) * alpha, 500.0 * (x - y) * alpha, 200.0 * (y - z) * alpha, 100.0 * alpha]
}

fn apart(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    (0..4).map(|c| (a[c] - b[c]) * (a[c] - b[c])).sum::<f32>().sqrt()
}

/// `looks` smoothed twice by a bilateral filter: each pixel takes the
/// average of those around it, each counting less the farther it is and
/// the more different it looks, so areas even out but edges stay
fn smoothed(looks: &[[f32; 4]], width: usize) -> Vec<[f32; 4]> {
    let height = looks.len() / width;
    let (reach, spread) = (2isize, 1.5f32);
    let mut current = looks.to_vec();
    for _ in 0..2 {
        let source = current.clone();
        for y in 0..height as isize {
            for x in 0..width as isize {
                let own = source[y as usize * width + x as usize];
                let (mut sum, mut total) = ([0f32; 4], 0f32);
                for dy in -reach..=reach {
                    let ny = y + dy;
                    if ny < 0 || ny >= height as isize {
                        continue;
                    }
                    for dx in -reach..=reach {
                        let nx = x + dx;
                        if nx < 0 || nx >= width as isize {
                            continue;
                        }
                        let other = source[ny as usize * width + nx as usize];
                        let near = ((dx * dx + dy * dy) as f32) / (2.0 * spread * spread);
                        let alike = apart(&own, &other).powi(2) / (2.0 * ALIKE * ALIKE);
                        let weight = (-near - alike).exp();
                        for c in 0..4 {
                            sum[c] += other[c] * weight;
                        }
                        total += weight;
                    }
                }
                current[y as usize * width + x as usize] = sum.map(|value| value / total);
            }
        }
    }
    current
}

/// Sets of things, merged by union-find
struct Sets(Vec<u32>);

impl Sets {
    fn new(count: usize) -> Self {
        Self((0..count as u32).collect())
    }

    fn find(&mut self, mut i: u32) -> u32 {
        while self.0[i as usize] != i {
            let parent = self.0[i as usize];
            self.0[i as usize] = self.0[parent as usize];
            i = parent;
        }
        i
    }

    /// Puts `b`'s set into `a`'s, both found
    fn join(&mut self, a: u32, b: u32) {
        self.0[b as usize] = a;
    }
}

/// Each pixel's region, of many small ones of alike pixels, by
/// Felzenszwalb and Huttenlocher's graph-based segmentation: neighbors are
/// joined from the most alike, while they differ less than the regions they
/// join do inside, with `ALIKE` to spare, less for larger regions. Along a
/// soft gradient every step is small, so regions also stop growing once
/// their averages are `DRIFT` apart, leaving its bands to `segment`
fn oversegment(looks: &[[f32; 4]], width: usize) -> Vec<u32> {
    let count = looks.len();
    let mut edges: Vec<(f32, u32, u32)> = Vec::with_capacity(count * 2);
    for i in 0..count {
        if (i + 1) % width != 0 {
            edges.push((apart(&looks[i], &looks[i + 1]), i as u32, i as u32 + 1));
        }
        if i + width < count {
            edges.push((apart(&looks[i], &looks[i + width]), i as u32, (i + width) as u32));
        }
    }
    edges.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    let mut sets = Sets::new(count);
    let mut sizes = vec![1u32; count];
    let mut limits = vec![ALIKE; count];
    let mut sums = looks.to_vec();
    let average = |sum: &[f32; 4], size: u32| sum.map(|value| value / size as f32);
    for (weight, a, b) in edges {
        let (a, b) = (sets.find(a), sets.find(b));
        let (i, j) = (a as usize, b as usize);
        if a == b || weight > limits[i].min(limits[j]) || apart(&average(&sums[i], sizes[i]), &average(&sums[j], sizes[j])) > DRIFT {
            continue;
        }
        sets.join(a, b);
        sizes[i] += sizes[j];
        let other = sums[j];
        for c in 0..4 {
            sums[i][c] += other[c];
        }
        limits[i] = weight + ALIKE / sizes[i] as f32;
    }
    (0..count as u32).map(|i| sets.find(i)).collect()
}

/// Regions of the image, each pixel's, and each region's color, unless
/// it's left out as transparent
struct Regions {
    labels: Vec<u32>,
    colors: Vec<Option<([u8; 3], f64)>>,
}

/// What's added up for a region: its pixels, how they look and their
/// premultiplied colors
#[derive(Clone, Copy, Default)]
struct Sum {
    count: f64,
    look: [f64; 4],
    color: [f64; 4],
}

impl Sum {
    fn add(&mut self, other: &Sum) {
        self.count += other.count;
        for c in 0..4 {
            self.look[c] += other.look[c];
            self.color[c] += other.color[c];
        }
    }

    fn look(&self) -> [f64; 4] {
        self.look.map(|value| value / self.count)
    }

    fn apart(&self, other: &Sum) -> f64 {
        let (a, b) = (self.look(), other.look());
        (0..4).map(|c| (a[c] - b[c]).powi(2)).sum::<f64>()
    }

    /// The cost of merging, by Ward's method: how much it adds to how far
    /// pixels are from their region's average, over all `pixels`
    fn ward(&self, other: &Sum, pixels: f64) -> f64 {
        self.count * other.count / (self.count + other.count) * self.apart(other) / pixels
    }
}

/// The image split from the regions of `start` into shapes to draw:
/// merged by Ward's method while it costs less than `MERGE`, `coarser`
/// times as much, then what's smaller than `SMALLEST`, `coarser` times as
/// large, into the neighbor it looks most like. Then each region takes its
/// average color, those within `SAME` of a larger one take its color, and
/// neighbors of one color merge
fn segment(pixels: &[[u8; 4]], looks: &[[f32; 4]], start: &[u32], blends: &[bool], width: usize, coarser: f64) -> Regions {
    let total = pixels.len() as f64;
    // Blends are left out until the regions are known
    let start: Vec<u32> = start.iter().zip(blends).map(|(&label, &blend)| if blend { u32::MAX } else { label }).collect();
    let mut sums = vec![Sum::default(); pixels.len()];
    for (i, (&label, pixel)) in start.iter().zip(pixels).enumerate() {
        if label == u32::MAX {
            continue;
        }
        let sum = &mut sums[label as usize];
        sum.count += 1.0;
        for c in 0..4 {
            sum.look[c] += f64::from(looks[i][c]);
            sum.color[c] += f64::from(pixel[c]);
        }
    }
    let neighbors = |labels: &dyn Fn(usize) -> u32| {
        let mut pairs = Vec::new();
        for i in 0..pixels.len() {
            let here = labels(i);
            for j in [i + 1, i + width] {
                if j < pixels.len() && (j != i + 1 || j % width != 0) {
                    let there = labels(j);
                    if here != there && here != u32::MAX && there != u32::MAX {
                        pairs.push((here.min(there), here.max(there)));
                    }
                }
            }
        }
        pairs.sort_unstable();
        pairs.dedup();
        pairs
    };

    // Cheapest merge first. A merge changes the costs of the merged
    // region's other merges, which are worked out again when they come up
    let mut sets = Sets::new(pixels.len());
    let mut heap: BinaryHeap<Reverse<(u64, u32, u32)>> =
        neighbors(&|i| start[i]).into_iter().map(|(a, b)| Reverse((sums[a as usize].ward(&sums[b as usize], total).to_bits(), a, b))).collect();
    while let Some(Reverse((cost, a, b))) = heap.pop() {
        let (a, b) = (sets.find(a), sets.find(b));
        if a == b {
            continue;
        }
        let now = sums[a as usize].ward(&sums[b as usize], total);
        if now > f64::from_bits(cost) * (1.0 + 1e-9) {
            heap.push(Reverse((now.to_bits(), a, b)));
            continue;
        }
        if now > MERGE * coarser {
            break;
        }
        let other = sums[b as usize];
        sums[a as usize].add(&other);
        sets.join(a, b);
    }

    // What's still too small goes into the neighbor it looks most like, as
    // do slivers: regions under `SLIVER_WIDTH` pixels across on average whose
    // color is within `SLIVER` of a neighbor's, such as an edge's
    // outermost pixels, tinted a little by its blend
    let smallest = (SMALLEST * coarser * total).max(4.0);
    loop {
        let labels: Vec<u32> = (0..pixels.len()).map(|i| if start[i] == u32::MAX { u32::MAX } else { sets.find(start[i]) }).collect();
        let pairs = neighbors(&|i| labels[i]);
        let mut adjacent: HashMap<u32, Vec<u32>> = HashMap::new();
        for &(a, b) in &pairs {
            adjacent.entry(a).or_default().push(b);
            adjacent.entry(b).or_default().push(a);
        }
        let mut borders: HashMap<u32, u32> = HashMap::new();
        for (a, b) in pairs_of(pixels.len(), width) {
            if labels[a] != labels[b] {
                for label in [labels[a], labels[b]] {
                    if label != u32::MAX {
                        *borders.entry(label).or_default() += 1;
                    }
                }
            }
        }
        // The neighbor `region` looks most like
        fn nearest(sets: &mut Sets, sums: &[Sum], adjacent: &HashMap<u32, Vec<u32>>, region: u32) -> Option<u32> {
            let found: Vec<u32> = adjacent[&region].iter().map(|&other| sets.find(other)).filter(|&other| other != region).collect();
            found.into_iter().min_by(|&x, &y| sums[region as usize].apart(&sums[x as usize]).total_cmp(&sums[region as usize].apart(&sums[y as usize])))
        }
        let sliver = |sets: &mut Sets, sums: &[Sum], region: u32| {
            let sum = sums[region as usize];
            sum.count * 2.0 < f64::from(borders.get(&region).copied().unwrap_or(0)) * SLIVER_WIDTH
                && nearest(sets, sums, &adjacent, region).is_some_and(|other| sum.apart(&sums[other as usize]) < SLIVER * SLIVER)
        };
        let mut small: Vec<u32> = adjacent.keys().copied().filter(|&r| sums[r as usize].count < smallest || sliver(&mut sets, &sums, r)).collect();
        small.sort_unstable_by(|a, b| sums[*a as usize].count.total_cmp(&sums[*b as usize].count));
        small.dedup();
        let mut merged = false;
        for region in small {
            // Merged into since, its neighbors aren't all known
            if sets.find(region) != region || (sums[region as usize].count >= smallest && !sliver(&mut sets, &sums, region)) {
                continue;
            }
            if let Some(into) = nearest(&mut sets, &sums, &adjacent, region) {
                let other = sums[region as usize];
                sums[into as usize].add(&other);
                sets.join(into, region);
                merged = true;
            }
        }
        if !merged {
            break;
        }
    }

    let mut labels: Vec<u32> = (0..pixels.len()).map(|i| if start[i] == u32::MAX { u32::MAX } else { sets.find(start[i]) }).collect();
    assign_blends(&mut labels, looks, &sums, width);
    // Each region's sums, added up again for the regions as they now are
    let tally = |labels: &[u32], sums: &mut [Sum]| {
        for sum in sums.iter_mut() {
            *sum = Sum::default();
        }
        for (i, (&label, pixel)) in labels.iter().zip(pixels).enumerate() {
            let sum = &mut sums[label as usize];
            sum.count += 1.0;
            for c in 0..4 {
                sum.look[c] += f64::from(looks[i][c]);
                sum.color[c] += f64::from(pixel[c]);
            }
        }
    };
    // Settling edges sorts out bands of blends that were in pieces, so
    // they're looked for again after
    let decided = blurred(looks, width);
    for _ in 0..2 {
        if dissolve_blends(&mut labels, looks, &sums, width) {
            tally(&labels, &mut sums);
        }
        settle(&mut labels, &decided, &sums, width);
        tally(&labels, &mut sums);
    }

    // Colors, the largest regions' first, shared by those alike
    let mut regions: Vec<u32> = labels.clone();
    regions.sort_unstable();
    regions.dedup();
    regions.sort_by(|a, b| sums[*b as usize].count.total_cmp(&sums[*a as usize].count));
    let mut palette: Vec<Sum> = Vec::new();
    let mut shared = vec![u32::MAX; pixels.len()];
    for &region in &regions {
        let sum = sums[region as usize];
        let entry = palette.iter().position(|color| color.apart(&sum) < SAME.powi(2));
        shared[region as usize] = match entry {
            Some(entry) => entry as u32,
            None => {
                palette.push(sum);
                palette.len() as u32 - 1
            }
        };
    }

    // Pixels of one color side by side are one region. Pieces too small to
    // draw join the one they touch most
    let entries: Vec<u32> = labels.iter().map(|&label| shared[label as usize]).collect();
    let mut pieces = Sets::new(pixels.len());
    for (a, b) in pairs_of(pixels.len(), width) {
        if entries[a] == entries[b] {
            let (a, b) = (pieces.find(a as u32), pieces.find(b as u32));
            if a != b {
                pieces.join(a, b);
            }
        }
    }
    let roots: Vec<u32> = (0..pixels.len() as u32).map(|i| pieces.find(i)).collect();
    let mut sizes = vec![0u32; pixels.len()];
    for &root in &roots {
        sizes[root as usize] += 1;
    }
    let tiny = |root: u32| f64::from(sizes[root as usize]) < (smallest / 4.0).max(4.0);
    let mut touching: HashMap<(u32, u32), u32> = HashMap::new();
    for (a, b) in pairs_of(pixels.len(), width) {
        let (a, b) = (roots[a], roots[b]);
        if a != b {
            for (from, to) in [(a, b), (b, a)] {
                if tiny(from) {
                    *touching.entry((from, to)).or_default() += 1;
                }
            }
        }
    }
    let mut most: HashMap<u32, (u32, u32)> = HashMap::new();
    for (&(from, to), &count) in &touching {
        let best = most.entry(from).or_insert((to, 0));
        if count > best.1 {
            *best = (to, count);
        }
    }
    for (&from, &(to, _)) in &most {
        let (from, to) = (pieces.find(from), pieces.find(to));
        if from != to {
            pieces.join(to, from);
        }
    }

    let mut index = vec![u32::MAX; pixels.len()];
    let mut colors = Vec::new();
    let labels: Vec<u32> = (0..pixels.len())
        .map(|i| {
            let root = pieces.find(i as u32) as usize;
            if index[root] == u32::MAX {
                index[root] = colors.len() as u32;
                let sum = palette[entries[root] as usize];
                let alpha = sum.color[3] / sum.count;
                colors.push((alpha >= TRANSPARENT).then(|| {
                    let rgb = [0, 1, 2].map(|c| (sum.color[c] / sum.count * 255.0 / alpha).round().clamp(0.0, 255.0) as u8);
                    (rgb, alpha)
                }));
            }
            index[root]
        })
        .collect();
    Regions { labels, colors }
}

/// The pixels not in `mask`, `width` a row, joined to `start` side by
/// side, marked in `seen`
fn unmasked(start: usize, width: usize, mask: &[bool], seen: &mut [bool]) -> Vec<usize> {
    let height = mask.len() / width;
    let (mut stack, mut found) = (vec![start], Vec::new());
    seen[start] = true;
    while let Some(i) = stack.pop() {
        found.push(i);
        let (x, y) = (i % width, i / width);
        for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
            if nx < width && ny < height && !seen[ny * width + nx] && !mask[ny * width + nx] {
                seen[ny * width + nx] = true;
                stack.push(ny * width + nx);
            }
        }
    }
    found
}

/// Which pixels are blends, from antialiasing or a soft edge: those that
/// lie between two clearly different colors up to `BLEND_REACH` pixels
/// either side of them, in a line, within `BLEND_OFF` of the way from one
/// to the other. A thin line of its own color isn't between its sides
fn blend_pixels(looks: &[[f32; 4]], width: usize) -> Vec<bool> {
    let count = looks.len();
    let height = count / width;
    let mut blends = vec![false; count];
    for y in 0..height as isize {
        for x in 0..width as isize {
            let own = looks[y as usize * width + x as usize];
            let at = |x: isize, y: isize| (x >= 0 && y >= 0 && (x as usize) < width && (y as usize) < height).then(|| looks[y as usize * width + x as usize]);
            blends[y as usize * width + x as usize] = [(1, 0), (0, 1), (1, 1), (1, -1)].iter().any(|&(dx, dy)| {
                (1..=BLEND_REACH).any(|k| {
                    let (Some(from), Some(to)) = (at(x - dx * k, y - dy * k), at(x + dx * k, y + dy * k)) else {
                        return false;
                    };
                    let along: [f32; 4] = std::array::from_fn(|c| to[c] - from[c]);
                    let length = along.iter().map(|v| v * v).sum::<f32>();
                    if length < CONTRAST * CONTRAST {
                        return false;
                    }
                    let t = (0..4).map(|c| (own[c] - from[c]) * along[c]).sum::<f32>() / length;
                    let off = (0..4).map(|c| (own[c] - from[c] - t * along[c]).powi(2)).sum::<f32>().sqrt();
                    (0.15..0.85).contains(&t) && f64::from(off) < BLEND_OFF
                })
            });
        }
    }
    blends
}

/// Gives each blend, labeled `u32::MAX`, to the region around it it looks
/// most like, from the edges in, so an edge falls halfway through its blend
fn assign_blends(labels: &mut [u32], looks: &[[f32; 4]], sums: &[Sum], width: usize) {
    let count = labels.len();
    let mut left: Vec<usize> = (0..count).filter(|&i| labels[i] == u32::MAX).collect();
    while !left.is_empty() {
        let mut taken = Vec::new();
        for &i in &left {
            let (x, y) = (i % width, i / width);
            let near = [(x > 0).then(|| i - 1), (x + 1 < width).then(|| i + 1), (y > 0).then(|| i - width), (i + width < count).then(|| i + width)];
            let look = looks[i].map(f64::from);
            let distance = |label: u32| {
                let average = sums[label as usize].look();
                (0..4).map(|c| (average[c] - look[c]).powi(2)).sum::<f64>()
            };
            let best = near.into_iter().flatten().map(|j| labels[j]).filter(|&label| label != u32::MAX).min_by(|&a, &b| distance(a).total_cmp(&distance(b)));
            if let Some(label) = best {
                taken.push((i, label));
            }
        }
        if taken.is_empty() {
            // Nothing but blends: each its own
            for &i in &left {
                labels[i] = i as u32;
            }
            break;
        }
        for &(i, label) in &taken {
            labels[i] = label;
        }
        left.retain(|&i| labels[i] == u32::MAX);
    }
}

/// The pixels side by side in an image `width` wide, of `count` pixels
fn pairs_of(count: usize, width: usize) -> impl Iterator<Item = (usize, usize)> {
    (0..count).flat_map(move |i| [((i + 1) % width != 0).then_some((i, i + 1)), (i + width < count).then_some((i, i + width))].into_iter().flatten())
}

/// `looks` blurred a little, two box blurs over 3 × 3 pixels
fn blurred(looks: &[[f32; 4]], width: usize) -> Vec<[f32; 4]> {
    let height = looks.len() / width;
    let mut current = looks.to_vec();
    for _ in 0..2 {
        for vertical in [false, true] {
            let source = current.clone();
            for y in 0..height {
                for x in 0..width {
                    let (mut sum, mut count) = ([0f32; 4], 0f32);
                    for step in -1isize..=1 {
                        let (nx, ny) = if vertical { (x as isize, y as isize + step) } else { (x as isize + step, y as isize) };
                        if nx >= 0 && ny >= 0 && (nx as usize) < width && (ny as usize) < height {
                            let other = source[ny as usize * width + nx as usize];
                            for c in 0..4 {
                                sum[c] += other[c];
                            }
                            count += 1.0;
                        }
                    }
                    current[y * width + x] = sum.map(|value| value / count);
                }
            }
        }
    }
    current
}

/// Moves the edges between regions to where the image changes from one to
/// the other: each pixel on an edge goes to whichever region around it it
/// looks most like, by `looks`, again and again. Regions are merged a
/// piece at a time, so their edges come out ragged before this
fn settle(labels: &mut [u32], looks: &[[f32; 4]], sums: &[Sum], width: usize) {
    let count = labels.len();
    let height = count / width;
    for _ in 0..SETTLE {
        let source = labels.to_vec();
        let mut changed = false;
        for y in 0..height {
            for x in 0..width {
                let i = y * width + x;
                let own = source[i];
                let mut around = [own; 9];
                let mut found = 0;
                for ny in y.saturating_sub(1)..(y + 2).min(height) {
                    for nx in x.saturating_sub(1)..(x + 2).min(width) {
                        let label = source[ny * width + nx];
                        if !around[..found].contains(&label) {
                            around[found] = label;
                            found += 1;
                        }
                    }
                }
                if found < 2 {
                    continue;
                }
                let look = looks[i].map(f64::from);
                let distance = |label: u32| {
                    let average = sums[label as usize].look();
                    (0..4).map(|c| (average[c] - look[c]).powi(2)).sum::<f64>()
                };
                let mut best = (own, distance(own));
                for &label in &around[..found] {
                    let d = distance(label);
                    if d < best.1 {
                        best = (label, d);
                    }
                }
                if best.0 != own {
                    labels[i] = best.0;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// Takes out the bands antialiasing leaves where two regions meet: regions
/// on average under `THIN` pixels across whose colors lie between those of
/// two regions around them, past any thin regions they touch, as a blend
/// over several pixels splits into a band for each. Each of their pixels
/// goes to the neighbor it looks most like, from the edges in, so the edge
/// falls halfway through the blend. Thin lines of their own color stay.
/// Whether any did
fn dissolve_blends(labels: &mut [u32], looks: &[[f32; 4]], sums: &[Sum], width: usize) -> bool {
    let count = labels.len();
    let mut borders: HashMap<u32, (u32, Vec<u32>)> = HashMap::new();
    for i in 0..count {
        for j in [i + 1, i + width] {
            if j < count && (j != i + 1 || j % width != 0) && labels[i] != labels[j] {
                for (a, b) in [(labels[i], labels[j]), (labels[j], labels[i])] {
                    let entry = borders.entry(a).or_default();
                    entry.0 += 1;
                    entry.1.push(b);
                }
            }
        }
    }
    for (_, neighbors) in borders.values_mut() {
        neighbors.sort_unstable();
        neighbors.dedup();
    }
    let thin = |region: u32| borders.get(&region).is_some_and(|&(border, _)| sums[region as usize].count * 2.0 < f64::from(border) * THIN);
    let between = |region: u32, a: u32, b: u32| {
        let (from, to, color) = (sums[a as usize].look(), sums[b as usize].look(), sums[region as usize].look());
        let along: [f64; 4] = std::array::from_fn(|c| to[c] - from[c]);
        let length = along.iter().map(|v| v * v).sum::<f64>();
        if length == 0.0 {
            return false;
        }
        let t = (0..4).map(|c| (color[c] - from[c]) * along[c]).sum::<f64>() / length;
        let off = (0..4).map(|c| (color[c] - from[c] - t * along[c]).powi(2)).sum::<f64>().sqrt();
        (0.05..0.95).contains(&t) && off < BLEND_OFF
    };
    // Thin regions that touch are taken together, with the wide regions
    // around them
    let thins: Vec<u32> = borders.keys().copied().filter(|&region| thin(region)).collect();
    let index: HashMap<u32, u32> = thins.iter().enumerate().map(|(i, &region)| (region, i as u32)).collect();
    let mut groups = Sets::new(thins.len());
    for (i, region) in thins.iter().enumerate() {
        for other in &borders[region].1 {
            if let Some(&j) = index.get(other) {
                let (a, b) = (groups.find(i as u32), groups.find(j));
                if a != b {
                    groups.join(a, b);
                }
            }
        }
    }
    let mut around: HashMap<u32, Vec<u32>> = HashMap::new();
    for (i, region) in thins.iter().enumerate() {
        let wide = borders[region].1.iter().copied().filter(|other| !index.contains_key(other));
        around.entry(groups.find(i as u32)).or_default().extend(wide);
    }
    let blends: std::collections::HashSet<u32> = thins
        .iter()
        .enumerate()
        .filter(|&(i, &region)| {
            let wide = &around[&groups.find(i as u32)];
            wide.iter().any(|&a| wide.iter().any(|&b| a < b && between(region, a, b)))
        })
        .map(|(_, &region)| region)
        .collect();
    if blends.is_empty() {
        return false;
    }

    let mut left: Vec<usize> = (0..count).filter(|&i| blends.contains(&labels[i])).collect();
    while !left.is_empty() {
        let mut taken = Vec::new();
        for &i in &left {
            let (x, y) = (i % width, i / width);
            let near = [(x > 0).then(|| i - 1), (x + 1 < width).then(|| i + 1), (y > 0).then(|| i - width), (i + width < count).then(|| i + width)];
            let look = looks[i].map(f64::from);
            let distance = |label: u32| {
                let average = sums[label as usize].look();
                (0..4).map(|c| (average[c] - look[c]).powi(2)).sum::<f64>()
            };
            let best = near.into_iter().flatten().map(|j| labels[j]).filter(|label| !blends.contains(label)).min_by(|&a, &b| distance(a).total_cmp(&distance(b)));
            if let Some(label) = best {
                taken.push((i, label));
            }
        }
        // Enclosed by blends alone: left as they are
        if taken.is_empty() {
            break;
        }
        for &(i, label) in &taken {
            labels[i] = label;
        }
        left.retain(|&i| blends.contains(&labels[i]));
    }
    true
}

/// Traced shapes, as SVG paths, how many pixels of each fill show and how
/// many shapes there are
struct Shapes {
    paths: String,
    areas: BTreeMap<String, u64>,
    count: usize,
}

/// `regions` traced, each one filled outline, those that hold others drawn
/// first. A region reaches under its neighbors drawn after it, and fills
/// its holes where they hold only those, but not where it would show:
/// under what's see-through or left out
fn shapes(regions: &Regions, width: usize, height: usize, fitting: &Fitting) -> Shapes {
    let count = regions.colors.len();
    let mut boxes = vec![(usize::MAX, usize::MAX, 0, 0); count];
    let mut areas = vec![0u64; count];
    for (i, &label) in regions.labels.iter().enumerate() {
        let (x, y) = (i % width, i / width);
        let found = &mut boxes[label as usize];
        *found = (found.0.min(x), found.1.min(y), found.2.max(x), found.3.max(y));
        areas[label as usize] += 1;
    }
    // A region that holds another has a larger box, so drawing the largest
    // boxes first draws each region before those it holds
    let box_area = |(left, top, right, bottom): (usize, usize, usize, usize)| (right - left + 1) * (bottom - top + 1);
    let mut order: Vec<usize> = (0..count).filter(|&r| regions.colors[r].is_some()).collect();
    order.sort_by_key(|&r| (Reverse(box_area(boxes[r])), Reverse(areas[r])));
    let mut rank = vec![usize::MAX; count];
    for (position, &r) in order.iter().enumerate() {
        rank[r] = position;
    }
    // Whether region `under` may be covered by `over`: it's drawn after it
    // and opaque
    let covers = |over: usize, under: u32| {
        let under = under as usize;
        rank[under] != usize::MAX && rank[under] > rank[over] && regions.colors[under].is_some_and(|(_, alpha)| alpha >= OPAQUE)
    };
    let reach = (2.0 * fitting.pixel).ceil() as usize;

    let mut shapes = Shapes { paths: String::new(), areas: BTreeMap::new(), count: 0 };
    for &r in &order {
        let (left, top, right, bottom) = boxes[r];
        let (left, top) = (left.saturating_sub(reach), top.saturating_sub(reach));
        let (right, bottom) = ((right + reach).min(width - 1), (bottom + reach).min(height - 1));
        // With a pixel to spare all round, outside the region
        let (w, h) = (right - left + 3, bottom - top + 3);
        let label = |x: usize, y: usize| regions.labels[(top + y - 1) * width + left + x - 1];
        let inside = |x: usize, y: usize| x > 0 && y > 0 && x < w - 1 && y < h - 1;
        let mut mask = vec![false; w * h];
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                mask[y * w + x] = label(x, y) as usize == r;
            }
        }
        // Reaches `reach` pixels under what's drawn over it
        let mut wide = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                wide[y * w + x] = (x.saturating_sub(reach)..=(x + reach).min(w - 1)).any(|nx| mask[y * w + nx]);
            }
        }
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                if !mask[y * w + x] && covers(r, label(x, y)) && (y.saturating_sub(reach)..=(y + reach).min(h - 1)).any(|ny| wide[ny * w + x]) {
                    mask[y * w + x] = true;
                }
            }
        }
        // Reaching under others may leave pieces apart from it
        let first = (0..w * h).find(|&i| mask[i] && inside(i % w, i / w) && label(i % w, i / w) as usize == r);
        let Some(first) = first else {
            continue;
        };
        let negative: Vec<bool> = mask.iter().map(|&set| !set).collect();
        let mut kept = vec![false; w * h];
        unmasked(first, w, &negative, &mut kept);
        for (set, &kept) in mask.iter_mut().zip(&kept) {
            *set &= kept;
        }
        // Holes it may cover are filled
        let mut seen = vec![false; w * h];
        unmasked(0, w, &mask, &mut seen);
        for start in 0..w * h {
            if seen[start] || mask[start] {
                continue;
            }
            let hole = unmasked(start, w, &mask, &mut seen);
            if hole.iter().all(|&i| inside(i % w, i / w) && covers(r, label(i % w, i / w))) {
                for i in hole {
                    mask[i] = true;
                }
            }
        }

        join_corners(&mut mask, w, h);
        let mut image = BinaryImage::new_w_h(w, h);
        for (i, &set) in mask.iter().enumerate() {
            if set {
                image.set_pixel(i % w, i / w, true);
            }
        }
        let mut d = String::new();
        for mut outline in Cluster::image_to_paths(&image, PathSimplifyMode::None) {
            outline.offset(&PointI32 { x: left as i32 - 1, y: top as i32 - 1 });
            curve(&outline.path, fitting, &mut d);
        }
        if d.is_empty() {
            continue;
        }
        let Some((rgb, alpha)) = regions.colors[r] else {
            continue;
        };
        let fill = format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]);
        let _ = write!(shapes.paths, r#"<path d="{d}" fill="{fill}""#);
        if alpha < OPAQUE {
            let _ = write!(shapes.paths, r#" fill-opacity="{:.3}""#, alpha / 255.0);
        }
        shapes.paths.push_str("/>");
        *shapes.areas.entry(fill).or_default() += areas[r];
        shapes.count += 1;
    }
    shapes
}

/// Pixels that touch only at a corner can't be followed round, so they're
/// joined by one more
fn join_corners(mask: &mut [bool], w: usize, h: usize) {
    loop {
        let mut changed = false;
        for y in 0..h - 1 {
            for x in 0..w - 1 {
                let (a, b, c, e) = (y * w + x, y * w + x + 1, (y + 1) * w + x, (y + 1) * w + x + 1);
                if mask[a] && mask[e] && !mask[b] && !mask[c] {
                    mask[b] = true;
                    changed = true;
                } else if mask[b] && mask[c] && !mask[a] && !mask[e] {
                    mask[a] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// The outlines of `mask`, `width` pixels wide, as smooth SVG path
/// commands, fitted as Vectorize fits its shapes, holes and all, for
/// Select object (see objects.rs). Specks under `least` pixels are left out
pub fn outline(mask: &[bool], width: usize, height: usize, least: usize) -> String {
    let mut image = BinaryImage::new_w_h(width, height);
    for (i, _) in mask.iter().enumerate().filter(|(_, &set)| set) {
        image.set_pixel(i % width, i / width, true);
    }
    let fitting = Fitting::new(1.0);
    let mut d = String::new();
    for cluster in image.to_clusters(false).iter().filter(|cluster| cluster.size() >= least) {
        // A pixel of space all round, as in `shapes`
        let (left, top) = (cluster.rect.left, cluster.rect.top);
        let (w, h) = (cluster.rect.width() as usize + 2, cluster.rect.height() as usize + 2);
        let mut mask = vec![false; w * h];
        for point in cluster.iter() {
            mask[(point.y - top + 1) as usize * w + (point.x - left + 1) as usize] = true;
        }
        join_corners(&mut mask, w, h);
        let mut image = BinaryImage::new_w_h(w, h);
        for (i, _) in mask.iter().enumerate().filter(|(_, &set)| set) {
            image.set_pixel(i % w, i / w, true);
        }
        for mut outline in Cluster::image_to_paths(&image, PathSimplifyMode::None) {
            outline.offset(&PointI32 { x: left - 1, y: top - 1 });
            curve(&outline.path, &fitting, &mut d);
        }
    }
    d
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

    fn traced(bytes: &[u8]) -> serde_json::Value {
        serde_json::from_str(&trace(bytes, 1.0).unwrap()).unwrap()
    }

    fn svg(bytes: &[u8]) -> String {
        traced(bytes)["svg"].as_str().unwrap().to_owned()
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
        assert_eq!(svg.matches("<path").count(), 2, "{svg}");
        // The background first, whole, then the circle over it
        let (white, red) = (svg.find(r##"fill="#FFFFFF""##).unwrap(), svg.find(r##"fill="#FF0000""##).unwrap());
        assert!(white < red, "{svg}");
        assert_eq!(svg.matches('M').count(), 2, "{svg}");
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
        let traced = traced(&bytes);
        assert_eq!(traced["areas"]["#0000FF"], 50);
        assert_eq!(traced["areas"]["#FFFF00"], 50);
    }

    #[test]
    fn bands_gradients() {
        let bytes = png(256, 32, |x, _| [x as u8, 128, 255 - x as u8, 255]);
        let svg = svg(&bytes);
        assert!(svg.matches("<path").count() >= 6, "{svg}");
    }

    #[test]
    fn leaves_out_transparency() {
        // A blue ring: its hole stays a hole
        let bytes = png(64, 64, |x, y| {
            let distance = (x as i32 - 32).pow(2) + (y as i32 - 32).pow(2);
            if (100..600).contains(&distance) { [0, 0, 255, 255] } else { [0, 0, 0, 0] }
        });
        let svg = svg(&bytes);
        assert_eq!(svg.matches("<path").count(), 1, "{svg}");
        assert_eq!(svg.matches('M').count(), 2, "{svg}");
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

    #[test]
    fn outlines_each_piece_and_leaves_out_specks() {
        let (width, height) = (64, 32);
        let mask: Vec<bool> = (0..width * height)
            .map(|i| {
                let (x, y) = (i % width, i / width);
                let square = (4..20).contains(&x) && (4..20).contains(&y);
                let other = (30..50).contains(&x) && (8..28).contains(&y);
                let speck = x == 60 && y == 2;
                square || other || speck
            })
            .collect();
        let d = outline(&mask, width, height, 4);
        assert_eq!(d.matches('M').count(), 2, "{d}");
        // Along the edge of the image too
        let full = outline(&[true; 16 * 16], 16, 16, 1);
        assert_eq!(full.matches('M').count(), 1, "{full}");
    }
}
