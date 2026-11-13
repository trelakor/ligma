//! Upscale on this device (Settings > Features > Images): the image bar's
//! Upscale in free projects (see ai_actions.rs), which makes an image 2 or
//! 4 times as large: Photo with 4xNomosWebPhoto_RealPLKSR (Philip Hofmann,
//! CC BY 4.0), trained on real photos to keep their texture, grain and
//! noise rather than smooth them away, Illustration with Real-ESRGAN's
//! x4plus anime 6B (BSD 3-Clause), trained on drawings, so lines and flat
//! color stay clean where a photo model would make up texture, or Fast with
//! Real-ESRGAN's compact general model, realesr-general-x4v3 (BSD
//! 3-Clause), several times as quick and smoother. They run as ONNX with
//! ONNX Runtime where it runs (see models.rs), on the graphics card where
//! there's one to use, with DirectML on Windows and Core ML on Macs, and
//! otherwise, or if that fails, on the CPU. Each is downloaded when first
//! needed
//!
//! The page hands a request over as a download (see handoff.rs): a line of
//! JSON, `{"scale":s,"quality":"photo"|"illustration"|"fast"}`, then the image itself,
//! as Figma keeps it. The model makes it 4 times as large, a tile at a
//! time, each the same size, so a graphics card prepares the model once,
//! and that's scaled down to `s` times, at most 4,096 pixels across, as
//! large as Figma keeps images.
//! Transparency is scaled up smoothly alongside. The result is
//! `{"png":<base64>,"width":w,"height":h}`, or "large" when the image is
//! as large as it can be already

use serde::Deserialize;
use tauri::AppHandle;

#[derive(Deserialize)]
#[cfg_attr(not(birefnet), allow(dead_code))]
struct Request {
    scale: f32,
    #[serde(default)]
    quality: String,
}

/// Figma keeps images at most this many pixels across
const LARGEST: f32 = 4096.0;

/// How many times as large `width` by `height` can be made, up to `scale`,
/// or None if it can't be made larger
#[cfg_attr(not(birefnet), allow(dead_code))]
fn factor(width: u32, height: u32, scale: f32) -> Option<f32> {
    let factor = scale.clamp(1.0, 4.0).min(LARGEST / width.max(height).max(1) as f32);
    (factor > 1.01).then_some(factor)
}

/// What `bytes` asks for, done for `job` in tab `tab`: the image made
/// larger, as JSON, or why there's none: "large" when it can't be larger,
/// "cancelled" when the model wasn't downloaded or the page cancelled, and
/// "failed" otherwise
pub fn handle(app: &AppHandle, tab: u32, job: u64, bytes: &[u8]) -> Result<String, &'static str> {
    let split = bytes.iter().position(|&byte| byte == b'\n').unwrap_or(bytes.len());
    let request: Request = serde_json::from_slice(&bytes[..split]).map_err(|_| "failed")?;
    let image = bytes.get(split + 1..).unwrap_or_default();
    #[cfg(birefnet)]
    return esrgan::upscale(app, tab, job, request.scale, &request.quality, image);
    #[cfg(not(birefnet))]
    {
        let _ = (app, tab, job, request, image);
        Err("failed")
    }
}

#[cfg(birefnet)]
mod esrgan {
    use std::io::Cursor;
    use std::path::Path;
    use std::sync::Arc;

    use base64::Engine;
    use image::codecs::png::{CompressionType, FilterType as PngFilter};
    use image::imageops::{self, FilterType};
    use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageReader, RgbImage, RgbaImage};
    use ort::session::builder::AutoDevicePolicy;
    use ort::session::{RunOptions, Session};
    use ort::value::TensorRef;
    use tauri::AppHandle;

    use super::factor;
    use crate::models::{Missing, NOMOS, REAL_ESRGAN, REAL_ESRGAN_ART, UPSCALE, UPSCALE_ART, UPSCALE_FAST};

    /// The model makes images 4 times as large, a tile this size at a time,
    /// each read with this much more of the image around it, as far as its
    /// large kernels see, so the tiles meet without seams
    const TIMES: usize = 4;
    const GPU_TILE: usize = 512;
    const CPU_TILE: usize = 256;
    const PAD: usize = 32;

    /// A tile, `across` by `down` at `x` and `y`, read as `read_across` by
    /// `read_down` from `x0` and `y0`, which may be past the image's edge
    struct Tile {
        x: usize,
        y: usize,
        across: usize,
        down: usize,
        x0: isize,
        y0: isize,
        read_across: usize,
        read_down: usize,
    }

    fn failed(error: impl std::fmt::Display) -> &'static str {
        log::warn!("Upscale: {error}");
        "failed"
    }

    pub fn upscale(app: &AppHandle, tab: u32, job: u64, scale: f32, quality: &str, bytes: &[u8]) -> Result<String, &'static str> {
        let image = decode(bytes)?;
        let factor = factor(image.width(), image.height(), scale).ok_or("large")?;
        let (model, file) = match quality {
            "fast" => (&UPSCALE_FAST, REAL_ESRGAN.file),
            "illustration" => (&UPSCALE_ART, REAL_ESRGAN_ART.file),
            _ => (&UPSCALE, NOMOS.file),
        };
        // Photo makes up texture most readily, so it's kept to most of its
        // way, the rest a smooth enlargement
        let strength = if quality == "fast" || quality == "illustration" { 1.0 } else { PHOTO_STRENGTH };
        let (_held, folder) = model.ensure(app, tab, job).map_err(|missing| match missing {
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
        let larger = run(&folder.join(file), Some(&folder), &image, factor, strength, &options, &stopped, &progress).map_err(|error| {
            if stopped() {
                "cancelled"
            } else {
                failed(error)
            }
        })?;
        // Encoded quickly: it's large, and Figma compresses it again anyway
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new_with_quality(&mut png, CompressionType::Fast, PngFilter::Adaptive)
            .write_image(larger.as_raw(), larger.width(), larger.height(), image::ExtendedColorType::Rgba8)
            .map_err(failed)?;
        Ok(format!(
            r#"{{"png":"{}","width":{},"height":{}}}"#,
            base64::engine::general_purpose::STANDARD.encode(png),
            larger.width(),
            larger.height()
        ))
    }

    /// A session for the model at `model`, on the graphics card if `gpu`
    /// and there's one to use: DirectML on Windows, picking the fastest
    /// card, and Core ML on Macs. Where it can't, ONNX Runtime runs it on
    /// the CPU instead
    fn session(environment: &'static ort::environment::Environment, model: &Path, gpu: bool) -> Result<Session, String> {
        let builder = Session::builder(environment).map_err(|error| error.to_string())?;
        let mut builder = if gpu {
            #[cfg(windows)]
            let providers = [ort::ep::DirectML::default().with_performance_preference(ort::ep::directml::PerformancePreference::HighPerformance).build()];
            #[cfg(target_os = "macos")]
            let providers = [ort::ep::CoreML::default()
                .with_compute_units(ort::ep::coreml::ComputeUnits::All)
                .with_model_format(ort::ep::coreml::ModelFormat::MLProgram)
                .build()];
            builder
                // DirectML runs nodes one at a time, in buffers it lays out itself
                .with_memory_pattern(false)
                .map_err(|error| error.to_string())?
                .with_parallel_execution(false)
                .map_err(|error| error.to_string())?
                .with_execution_providers(providers)
                .map_err(|error| error.to_string())?
        } else {
            builder.with_auto_device(AutoDevicePolicy::PreferCPU).map_err(|error| error.to_string())?
        };
        builder.commit_from_file(model).map_err(|error| error.to_string())
    }

    /// How much of its way Photo's model is kept, the rest a smooth
    /// enlargement, as it makes up texture most readily
    const PHOTO_STRENGTH: f32 = 0.85;
    /// Where the original is this flat, in how far its brightness spreads
    /// around each pixel, only `FLAT_KEEP` of the model is kept, and where
    /// it's `DETAILED` or more, all of it
    const FLAT: f32 = 3.0;
    const DETAILED: f32 = 12.0;
    const FLAT_KEEP: f32 = 0.15;

    /// Evens `out`, `src` made 4 times as large by a model, toward a smooth
    /// enlargement of `src` where `src` is flat: there a model makes up
    /// texture from compression and noise, like a pattern on a green
    /// screen or wax on skin. Where `src` has detail, the model's stays.
    /// All of it is kept to `strength` of the way
    fn settle(src: &RgbImage, out: &mut RgbImage, strength: f32) {
        let (width, height) = (src.width() as usize, src.height() as usize);
        let smooth = imageops::resize(src, out.width(), out.height(), FilterType::CatmullRom);
        // How far brightness spreads in the 5 by 5 pixels around each
        let luma: Vec<f32> = src.pixels().map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32).collect();
        let mut keep = vec![0f32; width * height];
        for y in 0..height {
            for x in 0..width {
                let (mut sum, mut squares, mut count) = (0f32, 0f32, 0f32);
                for ny in y.saturating_sub(2)..(y + 3).min(height) {
                    for nx in x.saturating_sub(2)..(x + 3).min(width) {
                        let value = luma[ny * width + nx];
                        sum += value;
                        squares += value * value;
                        count += 1.0;
                    }
                }
                let mean = sum / count;
                let spread = (squares / count - mean * mean).max(0.0).sqrt();
                let t = ((spread - FLAT) / (DETAILED - FLAT)).clamp(0.0, 1.0);
                keep[y * width + x] = strength * (FLAT_KEEP + (1.0 - FLAT_KEEP) * t * t * (3.0 - 2.0 * t));
            }
        }
        // Read smoothly between the original's pixels
        let at = |x: f32, y: f32| {
            let (x, y) = (x.clamp(0.0, (width - 1) as f32), y.clamp(0.0, (height - 1) as f32));
            let (x0, y0) = (x.floor() as usize, y.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
            let (fx, fy) = (x - x0 as f32, y - y0 as f32);
            let top = keep[y0 * width + x0] * (1.0 - fx) + keep[y0 * width + x1] * fx;
            let bottom = keep[y1 * width + x0] * (1.0 - fx) + keep[y1 * width + x1] * fx;
            top * (1.0 - fy) + bottom * fy
        };
        let (scale_x, scale_y) = (width as f32 / out.width() as f32, height as f32 / out.height() as f32);
        for (x, y, pixel) in out.enumerate_pixels_mut() {
            let k = at((x as f32 + 0.5) * scale_x - 0.5, (y as f32 + 0.5) * scale_y - 0.5);
            let base = smooth.get_pixel(x, y);
            for channel in 0..3 {
                let (model, base) = (pixel[channel] as f32, base[channel] as f32);
                pixel[channel] = (base + (model - base) * k).round().clamp(0.0, 255.0) as u8;
            }
        }
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

    /// `image` made `factor` times as large with the model at `model`, ONNX
    /// Runtime from `folder` where it's downloaded, or as linked in without
    /// one, its detail kept to `strength` of the way (see `settle`), saying
    /// how far it's got with `progress`
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        model: &Path,
        folder: Option<&Path>,
        image: &RgbaImage,
        factor: f32,
        strength: f32,
        options: &RunOptions,
        stopped: &dyn Fn() -> bool,
        progress: &dyn Fn(&str),
    ) -> Result<RgbaImage, String> {
        let error = |error: ort::Error| error.to_string();
        let (width, height) = (image.width() as usize, image.height() as usize);
        let environment = crate::models::environment(folder.unwrap_or(Path::new("")))?;
        // Tests can ask for the CPU, to compare
        let gpu = !(cfg!(test) && std::env::var("LIGMA_UPSCALE_CPU").is_ok());
        let mut session = session(environment, model, gpu).or_else(|_| session(environment, model, false))?;
        let mut on_gpu = gpu;
        // Transparent pixels as they'd show on black, as the model reads color
        let rgb = RgbImage::from_fn(width as u32, height as u32, |x, y| {
            let [r, g, b, a] = image.get_pixel(x, y).0;
            let alpha = a as u16;
            image::Rgb([r, g, b].map(|value| ((value as u16 * alpha + 127) / 255) as u8))
        });
        let (out_width, out_height) = (width * TIMES, height * TIMES);
        let mut out = RgbImage::new(out_width as u32, out_height as u32);
        // Tiles on a graphics card are all read the same size, their edge
        // pixels carried on past the image's, so the model is prepared for
        // one size only. On the CPU, where that costs nothing, they're
        // smaller, and those at the edges smaller still, so less is read
        // twice
        let tile = if gpu { GPU_TILE } else { CPU_TILE };
        let tiles: Vec<Tile> = if gpu {
            let (across, down) = (tile.min(width), tile.min(height));
            let starts = |size: usize, tile: usize| {
                let mut starts: Vec<usize> = (0..size.saturating_sub(tile)).step_by(tile).collect();
                starts.push(size - tile);
                starts.dedup();
                starts
            };
            starts(height, down)
                .into_iter()
                .flat_map(|y| starts(width, across).into_iter().map(move |x| (x, y)))
                .map(|(x, y)| Tile { x, y, across, down, x0: x as isize - PAD as isize, y0: y as isize - PAD as isize, read_across: across + 2 * PAD, read_down: down + 2 * PAD })
                .collect()
        } else {
            (0..height)
                .step_by(tile)
                .flat_map(|y| (0..width).step_by(tile).map(move |x| (x, y)))
                .map(|(x, y)| {
                    let (x0, y0) = (x.saturating_sub(PAD), y.saturating_sub(PAD));
                    let (x1, y1) = ((x + tile + PAD).min(width), (y + tile + PAD).min(height));
                    Tile {
                        x,
                        y,
                        across: tile.min(width - x),
                        down: tile.min(height - y),
                        x0: x0 as isize,
                        y0: y0 as isize,
                        read_across: x1 - x0,
                        read_down: y1 - y0,
                    }
                })
                .collect()
        };
        for (index, t) in tiles.iter().enumerate() {
            if stopped() {
                return Err("cancelled".to_owned());
            }
            if tiles.len() > 1 {
                progress(&format!("Upscaling… {}%", index * 100 / tiles.len()));
            }
            let (across, down) = (t.read_across, t.read_down);
            let plane = across * down;
            let mut input = vec![0f32; 3 * plane];
            for row in 0..down {
                let source_y = (t.y0 + row as isize).clamp(0, height as isize - 1) as u32;
                for column in 0..across {
                    let source_x = (t.x0 + column as isize).clamp(0, width as isize - 1) as u32;
                    let pixel = rgb.get_pixel(source_x, source_y);
                    for channel in 0..3 {
                        input[channel * plane + row * across + column] = pixel[channel] as f32 / 255.0;
                    }
                }
            }
            let ask = |session: &mut Session| -> Result<Vec<f32>, String> {
                let tensor = TensorRef::from_array_view(([1usize, 3, down, across], &*input)).map_err(error)?;
                let outputs = session.run_with_options(ort::inputs![tensor], options).map_err(error)?;
                let (_, output) = outputs[0].try_extract_tensor::<f32>().map_err(error)?;
                Ok(output.to_vec())
            };
            let output = match ask(&mut session) {
                Ok(output) => output,
                // The graphics card failed: the CPU from then on
                Err(_) if on_gpu && !stopped() => {
                    log::warn!("Upscale: the graphics card failed, using the CPU");
                    session = self::session(environment, model, false)?;
                    on_gpu = false;
                    ask(&mut session)?
                }
                Err(error) => return Err(error),
            };
            let (big_across, big_down) = (across * TIMES, down * TIMES);
            if output.len() != 3 * big_across * big_down {
                return Err("unexpected output".to_owned());
            }
            let big_plane = big_across * big_down;
            // Only the tile itself, not what was read around it
            let (inner_x, inner_y) = ((t.x as isize - t.x0) as usize * TIMES, (t.y as isize - t.y0) as usize * TIMES);
            for row in 0..t.down * TIMES {
                for column in 0..t.across * TIMES {
                    let at = (inner_y + row) * big_across + inner_x + column;
                    let pixel = [0, 1, 2].map(|channel| (output[channel * big_plane + at] * 255.0).round().clamp(0.0, 255.0) as u8);
                    out.put_pixel((t.x * TIMES + column) as u32, (t.y * TIMES + row) as u32, image::Rgb(pixel));
                }
            }
        }
        drop(session);
        progress("Finishing…");
        settle(&rgb, &mut out, strength);
        // Transparency scaled up smoothly, and the color back off black
        let alpha = imageops::resize(
            &image::GrayImage::from_fn(width as u32, height as u32, |x, y| image::Luma([image.get_pixel(x, y)[3]])),
            out_width as u32,
            out_height as u32,
            FilterType::CatmullRom,
        );
        let opaque = image.pixels().all(|pixel| pixel[3] == 255);
        let mut larger = RgbaImage::from_fn(out_width as u32, out_height as u32, |x, y| {
            let [r, g, b] = out.get_pixel(x, y).0;
            let a = if opaque { 255 } else { alpha.get_pixel(x, y)[0] };
            if a == 255 {
                return image::Rgba([r, g, b, 255]);
            }
            if a == 0 {
                return image::Rgba([0, 0, 0, 0]);
            }
            let unmultiply = |value: u8| ((value as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
            image::Rgba([unmultiply(r), unmultiply(g), unmultiply(b), a])
        });
        drop(out);
        let target = ((width as f32 * factor).round() as u32, (height as f32 * factor).round() as u32);
        if target == ((width * 2) as u32, (height * 2) as u32) {
            larger = halved(&larger);
        } else if target != (out_width as u32, out_height as u32) {
            larger = imageops::resize(&larger, target.0, target.1, FilterType::Triangle);
        }
        Ok(larger)
    }

    /// `image` half as large, each pixel the average of four, weighed by
    /// how opaque they are
    fn halved(image: &RgbaImage) -> RgbaImage {
        RgbaImage::from_fn(image.width() / 2, image.height() / 2, |x, y| {
            let (mut sum, mut alpha) = ([0u32; 3], 0u32);
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let [r, g, b, a] = image.get_pixel(2 * x + dx, 2 * y + dy).0;
                for (total, value) in sum.iter_mut().zip([r, g, b]) {
                    *total += value as u32 * a as u32;
                }
                alpha += a as u32;
            }
            if alpha == 0 {
                return image::Rgba([0, 0, 0, 0]);
            }
            let [r, g, b] = sum.map(|total| ((total + alpha / 2) / alpha) as u8);
            image::Rgba([r, g, b, ((alpha + 2) / 4) as u8])
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Upscales `LIGMA_UPSCALE_IMAGE` with the model at `LIGMA_UPSCALE`
        /// 4 times and writes it next to the image, on the graphics card
        /// unless `LIGMA_UPSCALE_CPU` is set. `cargo test --release --
        /// --ignored upscales`
        #[test]
        #[ignore]
        fn upscales_an_image() {
            let (Ok(model), Ok(path)) = (std::env::var("LIGMA_UPSCALE"), std::env::var("LIGMA_UPSCALE_IMAGE")) else {
                return;
            };
            let image = decode(&std::fs::read(&path).unwrap()).unwrap();
            let options = RunOptions::new().unwrap();
            let started = std::time::Instant::now();
            let factor = factor(image.width(), image.height(), 4.0).unwrap();
            let strength = std::env::var("LIGMA_UPSCALE_STRENGTH").ok().and_then(|value| value.parse().ok()).unwrap_or(PHOTO_STRENGTH);
            let larger = run(Path::new(&model), None, &image, factor, strength, &options, &|| false, &|_| {}).unwrap();
            eprintln!("{}x{} in {:?}", larger.width(), larger.height(), started.elapsed());
            larger.save(format!("{path}.upscaled.png")).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grows_up_to_what_figma_keeps() {
        assert_eq!(factor(1000, 500, 2.0), Some(2.0));
        assert_eq!(factor(1000, 500, 4.0), Some(4.0));
        assert_eq!(factor(2048, 1000, 4.0), Some(2.0));
        assert_eq!(factor(3000, 3000, 2.0), Some(4096.0 / 3000.0));
        assert_eq!(factor(4096, 100, 2.0), None);
    }
}
