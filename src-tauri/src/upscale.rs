//! Upscale on this device (Settings > Features > Images): the image bar's
//! Upscale in free projects (see ai_actions.rs), which makes an image 2 or
//! 4 times as large with Real-ESRGAN (BSD 3-Clause), the compact general
//! model, realesr-general-x4v3, as ONNX, on the CPU with ONNX Runtime,
//! where it runs (see models.rs). It's 5 MB, downloaded when first needed
//!
//! The page hands a request over as a download (see handoff.rs): a line of
//! JSON, `{"scale":s}`, then the image itself, as Figma keeps it. The model
//! makes it 4 times as large, a tile at a time, and that's scaled down to
//! `s` times, at most 4,096 pixels across, as large as Figma keeps images.
//! Transparency is scaled up smoothly alongside. The result is
//! `{"png":<base64>,"width":w,"height":h}`, or "large" when the image is
//! as large as it can be already

use serde::Deserialize;
use tauri::AppHandle;

#[derive(Deserialize)]
#[cfg_attr(not(birefnet), allow(dead_code))]
struct Request {
    scale: f32,
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
    return esrgan::upscale(app, tab, job, request.scale, image);
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
    use image::imageops::{self, FilterType};
    use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageReader, RgbImage, RgbaImage};
    use ort::session::builder::AutoDevicePolicy;
    use ort::session::{RunOptions, Session};
    use ort::value::TensorRef;
    use tauri::AppHandle;

    use super::factor;
    use crate::models::{Missing, REAL_ESRGAN, UPSCALE};

    /// The model makes images 4 times as large, a tile this size at a time,
    /// each read with this much more of the image around it, so the tiles
    /// meet without seams
    const TIMES: usize = 4;
    const TILE: usize = 192;
    const PAD: usize = 16;

    fn failed(error: impl std::fmt::Display) -> &'static str {
        log::warn!("Upscale: {error}");
        "failed"
    }

    pub fn upscale(app: &AppHandle, tab: u32, job: u64, scale: f32, bytes: &[u8]) -> Result<String, &'static str> {
        let image = decode(bytes)?;
        let factor = factor(image.width(), image.height(), scale).ok_or("large")?;
        let (_held, folder) = UPSCALE.ensure(app, tab, job).map_err(|missing| match missing {
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
        let larger = run(&folder.join(REAL_ESRGAN.file), Some(&folder), &image, factor, &options, &stopped, &progress).map_err(|error| {
            if stopped() {
                "cancelled"
            } else {
                failed(error)
            }
        })?;
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(larger.as_raw(), larger.width(), larger.height(), image::ExtendedColorType::Rgba8)
            .map_err(failed)?;
        Ok(format!(
            r#"{{"png":"{}","width":{},"height":{}}}"#,
            base64::engine::general_purpose::STANDARD.encode(png),
            larger.width(),
            larger.height()
        ))
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
    /// one, saying how far it's got with `progress`
    pub fn run(
        model: &Path,
        folder: Option<&Path>,
        image: &RgbaImage,
        factor: f32,
        options: &RunOptions,
        stopped: &dyn Fn() -> bool,
        progress: &dyn Fn(&str),
    ) -> Result<RgbaImage, String> {
        let error = |error: ort::Error| error.to_string();
        let (width, height) = (image.width() as usize, image.height() as usize);
        let environment = crate::models::environment(folder.unwrap_or(Path::new("")))?;
        let mut session = Session::builder(environment)
            .map_err(|error| error.to_string())?
            .with_auto_device(AutoDevicePolicy::PreferCPU)
            .map_err(|error| error.to_string())?
            .commit_from_file(model)
            .map_err(|error| error.to_string())?;
        // Transparent pixels as they'd show on black, as the model reads color
        let rgb = RgbImage::from_fn(width as u32, height as u32, |x, y| {
            let [r, g, b, a] = image.get_pixel(x, y).0;
            let alpha = a as u16;
            image::Rgb([r, g, b].map(|value| ((value as u16 * alpha + 127) / 255) as u8))
        });
        let (out_width, out_height) = (width * TIMES, height * TIMES);
        let mut out = RgbImage::new(out_width as u32, out_height as u32);
        let tiles: Vec<(usize, usize)> = (0..height).step_by(TILE).flat_map(|y| (0..width).step_by(TILE).map(move |x| (x, y))).collect();
        for (index, &(x, y)) in tiles.iter().enumerate() {
            if stopped() {
                return Err("cancelled".to_owned());
            }
            if tiles.len() > 1 {
                progress(&format!("Upscaling… {}%", index * 100 / tiles.len()));
            }
            // The tile with the image around it, as far as there's image
            let (x0, y0) = (x.saturating_sub(PAD), y.saturating_sub(PAD));
            let (x1, y1) = ((x + TILE + PAD).min(width), (y + TILE + PAD).min(height));
            let (across, down) = (x1 - x0, y1 - y0);
            let plane = across * down;
            let mut input = vec![0f32; 3 * plane];
            for row in 0..down {
                for column in 0..across {
                    let pixel = rgb.get_pixel((x0 + column) as u32, (y0 + row) as u32);
                    for channel in 0..3 {
                        input[channel * plane + row * across + column] = pixel[channel] as f32 / 255.0;
                    }
                }
            }
            let tensor = TensorRef::from_array_view(([1usize, 3, down, across], &*input)).map_err(error)?;
            let outputs = session.run_with_options(ort::inputs![tensor], options).map_err(error)?;
            let (_, output) = outputs[0].try_extract_tensor::<f32>().map_err(error)?;
            let (big_across, big_down) = (across * TIMES, down * TIMES);
            if output.len() != 3 * big_across * big_down {
                return Err("unexpected output".to_owned());
            }
            let big_plane = big_across * big_down;
            // Only the tile itself, not what was read around it
            for row in (y - y0) * TIMES..((y + TILE).min(height) - y0) * TIMES {
                for column in (x - x0) * TIMES..((x + TILE).min(width) - x0) * TIMES {
                    let at = row * big_across + column;
                    let pixel = [0, 1, 2].map(|channel| (output[channel * big_plane + at] * 255.0).round().clamp(0.0, 255.0) as u8);
                    out.put_pixel((x0 * TIMES + column) as u32, (y0 * TIMES + row) as u32, image::Rgb(pixel));
                }
            }
        }
        drop(session);
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
        if target != (out_width as u32, out_height as u32) {
            larger = imageops::resize(&larger, target.0, target.1, FilterType::Lanczos3);
        }
        Ok(larger)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Upscales `LIGMA_UPSCALE_IMAGE` with the model at `LIGMA_UPSCALE`
        /// 4 times and writes it next to the image. `cargo test --release
        /// -- --ignored upscales`
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
            let larger = run(Path::new(&model), None, &image, factor, &options, &|| false, &|_| {}).unwrap();
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
