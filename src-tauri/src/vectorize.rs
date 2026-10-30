//! Vectorize on this device (Settings > Features > Images): the image bar's
//! Vectorize in free projects, which Ligma adds in place of the one in
//! Figma's More menu (see ai_actions.rs). Figma's sends the layer as a PNG
//! to its servers and gets back an SVG of flat-colored shapes stacked from
//! the back. This traces it the same way here, with visioncortex (MIT or
//! Apache 2.0), the library behind VTracer: pixels are clustered by color,
//! hierarchically, and each cluster's outline is fitted with splines.
//! Neighboring pixels of a soft gradient are always alike, so it would
//! become one shape: the image is first reduced to a palette, by median
//! cut, which turns gradients into bands, as Figma's does
//!
//! The page hands the PNG over and gets the SVG back (see handoff.rs)

use std::fmt::Write;
use std::io::Cursor;

use image::ImageReader;
use visioncortex::color_clusters::{KeyingAction, Runner, RunnerConfig, HIERARCHICAL_MAX};
use visioncortex::{Color, ColorImage, PathSimplifyMode, PointF64};

/// Beyond this many shapes Figma slows down, so the image is traced again
/// with coarser settings, up to the last of `LEVELS`
const MAX_PATHS: usize = 2500;

/// Pixels this transparent or more are left out
const TRANSPARENT: u8 = 8;

/// How finely to trace, from finest: how many colors to reduce the image
/// to and the smallest shape's side in pixels
const LEVELS: [(usize, usize); 4] = [(PALETTE, 4), (24, 6), (16, 8), (8, 12)];
const PALETTE: usize = 32;

/// The PNG in `bytes` traced into an SVG the size of the image, its shapes
/// stacked from the back
pub fn trace(bytes: &[u8]) -> Result<String, &'static str> {
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
    let (width, height) = image.dimensions();
    let pixels = image.into_raw();

    let mut svg = String::new();
    for (i, &(colors, speckle)) in LEVELS.iter().enumerate() {
        let mut pixels = pixels.clone();
        let palette = quantize(&mut pixels, colors);
        let key = key_transparent(&mut pixels);
        let image = ColorImage { pixels, width: width as usize, height: height as usize };
        let (paths, count) = shapes(image, key, speckle, &palette);
        svg = paths;
        if count <= MAX_PATHS || i == LEVELS.len() - 1 {
            break;
        }
    }
    if svg.is_empty() {
        return Err("empty");
    }
    Ok(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">{svg}</svg>"#
    ))
}

/// Paints the transparent pixels in a color the image doesn't have, which
/// the tracer then leaves out, and returns it. Black with no alpha, which
/// the tracer takes as no key, when there are none
fn key_transparent(pixels: &mut [u8]) -> Color {
    if !pixels.as_chunks::<4>().0.iter().any(|pixel| pixel[3] < TRANSPARENT) {
        return Color::default();
    }
    let mut used = vec![0u64; (1 << 24) / 64];
    for pixel in pixels.as_chunks::<4>().0.iter().filter(|pixel| pixel[3] >= TRANSPARENT) {
        let rgb = (pixel[0] as usize) << 16 | (pixel[1] as usize) << 8 | pixel[2] as usize;
        used[rgb / 64] |= 1 << (rgb % 64);
    }
    // Every color can't be used: the image would have 16 million pixels
    // with none transparent
    let Some(free) = (0..1 << 24).find(|rgb| used[rgb / 64] & 1 << (rgb % 64) == 0) else {
        return Color::default();
    };
    let key = Color::new((free >> 16) as u8, (free >> 8) as u8, free as u8);
    for pixel in pixels.as_chunks_mut::<4>().0.iter_mut().filter(|pixel| pixel[3] < TRANSPARENT) {
        pixel.copy_from_slice(&[key.r, key.g, key.b, key.a]);
    }
    key
}

/// Reduces the colors of the pixels that aren't transparent to at most
/// `colors`, by median cut: the color space is split in two along its
/// widest channel, at the median pixel, again and again, the box with the
/// most pixels spread the widest first. Each box's pixels take their
/// average color, and those colors are returned. Colors are binned at 5
/// bits a channel
fn quantize(pixels: &mut [u8], colors: usize) -> Vec<[u8; 3]> {
    let bin = |pixel: &[u8]| (pixel[0] as usize >> 3) << 10 | (pixel[1] as usize >> 3) << 5 | pixel[2] as usize >> 3;
    let mut counts = vec![0u64; 1 << 15];
    let mut sums = vec![[0u64; 3]; 1 << 15];
    for pixel in pixels.as_chunks::<4>().0.iter().filter(|pixel| pixel[3] >= TRANSPARENT) {
        let i = bin(pixel);
        counts[i] += 1;
        for channel in 0..3 {
            sums[i][channel] += u64::from(pixel[channel]);
        }
    }
    let channel = |i: usize, channel: usize| (i >> (10 - 5 * channel)) & 31;
    let spread = |bins: &[usize]| {
        (0..3)
            .map(|c| {
                let (min, max) = bins.iter().fold((31, 0), |(min, max), &i| (min.min(channel(i, c)), max.max(channel(i, c))));
                (max.saturating_sub(min), c)
            })
            .max()
            .unwrap_or((0, 0))
    };
    let mut boxes: Vec<Vec<usize>> = vec![(0..1 << 15).filter(|&i| counts[i] > 0).collect()];
    while boxes.len() < colors {
        let Some((index, _)) = boxes
            .iter()
            .enumerate()
            .filter(|(_, bins)| bins.len() > 1)
            .map(|(index, bins)| (index, spread(bins).0 as u64 * bins.iter().map(|&i| counts[i]).sum::<u64>()))
            .max_by_key(|&(_, priority)| priority)
        else {
            break;
        };
        let mut bins = boxes.swap_remove(index);
        let (_, c) = spread(&bins);
        bins.sort_unstable_by_key(|&i| channel(i, c));
        let half = bins.iter().map(|&i| counts[i]).sum::<u64>() / 2;
        let mut seen = 0;
        let cut = bins
            .iter()
            .position(|&i| {
                seen += counts[i];
                seen > half
            })
            .unwrap_or(0)
            .clamp(1, bins.len() - 1);
        let upper = bins.split_off(cut);
        boxes.push(bins);
        boxes.push(upper);
    }
    let mut colors = vec![[0u8; 3]; 1 << 15];
    let mut palette = Vec::with_capacity(boxes.len());
    for bins in &boxes {
        let count = bins.iter().map(|&i| counts[i]).sum::<u64>().max(1);
        let mean: [u8; 3] = std::array::from_fn(|c| (bins.iter().map(|&i| sums[i][c]).sum::<u64>() / count) as u8);
        for &i in bins {
            colors[i] = mean;
        }
        palette.push(mean);
    }
    for pixel in pixels.as_chunks_mut::<4>().0.iter_mut().filter(|pixel| pixel[3] >= TRANSPARENT) {
        let color = colors[bin(pixel)];
        pixel[..3].copy_from_slice(&color);
    }
    palette
}

/// The image's shapes as SVG paths, leaving out those smaller than
/// `speckle` pixels a side, and how many. Shapes that merged with similar
/// neighbors take the closest color of `palette`, so the image keeps that
/// many colors at most
fn shapes(image: ColorImage, key: Color, speckle: usize, palette: &[[u8; 3]]) -> (String, usize) {
    let area = image.width * image.height;
    let clusters = Runner::new(
        RunnerConfig {
            diagonal: false,
            hierarchical: HIERARCHICAL_MAX,
            batch_size: 25600,
            good_min_area: speckle * speckle,
            good_max_area: area,
            is_same_color_a: 2,
            is_same_color_b: 1,
            deepen_diff: 16,
            hollow_neighbours: 1,
            key_color: key,
            keying_action: KeyingAction::Discard,
        },
        image,
    )
    .run();
    let view = clusters.view();
    let mut svg = String::new();
    let mut count = 0;
    for &index in view.clusters_output.iter().rev() {
        let cluster = view.get_cluster(index);
        let path = cluster.to_compound_path(
            &view,
            false,
            PathSimplifyMode::Spline,
            60f64.to_radians(),
            4.0,
            10,
            45f64.to_radians(),
        );
        let (d, offset) = path.to_svg_string(true, PointF64::default(), Some(2));
        if d.is_empty() {
            continue;
        }
        let mut color = cluster.residue_color();
        let own = [color.r, color.g, color.b];
        let distance = |entry: &&[u8; 3]| (0..3).map(|c| (i32::from(entry[c]) - i32::from(own[c])).pow(2)).sum::<i32>();
        if let Some(&[r, g, b]) = palette.iter().min_by_key(distance) {
            (color.r, color.g, color.b) = (r, g, b);
        }
        let _ = write!(svg, r#"<path d="{d}" transform="translate({},{})" fill="{}""#, offset.x, offset.y, color.to_hex_string());
        // Mostly see-through, like a shadow
        if color.a < 243 {
            let _ = write!(svg, r#" fill-opacity="{:.3}""#, f32::from(color.a) / 255.0);
        }
        svg.push_str("/>");
        count += 1;
    }
    (svg, count)
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

    #[test]
    fn traces_shapes_in_their_colors() {
        let bytes = png(64, 64, |x, y| {
            if (x as i32 - 32).pow(2) + (y as i32 - 32).pow(2) < 400 {
                [255, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            }
        });
        let svg = trace(&bytes).unwrap();
        assert!(svg.starts_with(r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64""#));
        assert!(svg.contains(r##"fill="#FF0000""##), "{svg}");
        assert!(svg.contains(r##"fill="#FFFFFF""##), "{svg}");
    }

    #[test]
    fn bands_gradients() {
        let bytes = png(256, 32, |x, _| [x as u8, 128, 255 - x as u8, 255]);
        let svg = trace(&bytes).unwrap();
        assert!(svg.matches("<path").count() >= 8, "{svg}");
    }

    #[test]
    fn leaves_out_transparency() {
        let bytes = png(64, 64, |x, y| if (16..48).contains(&x) && (16..48).contains(&y) { [0, 0, 255, 255] } else { [0, 0, 0, 0] });
        let svg = trace(&bytes).unwrap();
        assert_eq!(svg.matches("<path").count(), 1, "{svg}");
        assert!(svg.contains(r##"fill="#0000FF""##), "{svg}");
    }

    #[test]
    fn keeps_partial_transparency() {
        let bytes = png(64, 64, |x, y| if (16..48).contains(&x) && (16..48).contains(&y) { [0, 0, 0, 64] } else { [0, 0, 0, 0] });
        let svg = trace(&bytes).unwrap();
        assert!(svg.contains(r#"fill-opacity="0.251""#), "{svg}");
    }

    #[test]
    fn fully_transparent_is_empty() {
        let bytes = png(16, 16, |_, _| [0, 0, 0, 0]);
        assert_eq!(trace(&bytes), Err("empty"));
    }
}
