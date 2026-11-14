//! Upscale on this device (Settings > Images): the image bar's
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
//! large as Figma keeps images. An image that was enlarged before, so
//! it holds less detail than its size, is shrunk back to the size its
//! detail is for first, and enlarged twice where once isn't enough.
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

    /// The model makes images 4 times as large, a tile at most this size at
    /// a time, each read with this much more of the image around it, as far
    /// as its large kernels see, so the tiles meet without seams
    const TIMES: usize = 4;
    const CPU_TILE: usize = 256;
    const PAD: usize = 32;
    /// On a graphics card each model reads tiles of at most this many
    /// pixels, as many as it can before it needs much more of the card's
    /// memory: past these it holds gigabytes more, at 4 times the size, more
    /// than a card shares with Figma, so it slows several times over, and
    /// everything else on screen with it. Smaller, more is read twice, and
    /// Photo's model evens out its features over each tile, so the larger
    /// they are the closer it comes to the whole image made larger at once
    const PHOTO_READ: usize = 704 * 704;
    const ILLUSTRATION_READ: usize = 320 * 320;
    const FAST_READ: usize = 576 * 576;

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
        let (model, file, most) = match quality {
            "fast" => (&UPSCALE_FAST, REAL_ESRGAN.file, FAST_READ),
            "illustration" => (&UPSCALE_ART, REAL_ESRGAN_ART.file, ILLUSTRATION_READ),
            _ => (&UPSCALE, NOMOS.file, PHOTO_READ),
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
        let larger = run(&folder.join(file), Some(&folder), &image, factor, strength, most, &options, &stopped, &progress).map_err(|error| {
            if stopped() {
                "cancelled"
            } else {
                failed(error)
            }
        })?;
        // Encoded quickly: it's large, and Figma compresses it again anyway
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new_with_quality(&mut png, CompressionType::Fast, PngFilter::Adaptive)
            .write_image(larger.as_bytes(), larger.width(), larger.height(), larger.color().into())
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
    /// the CPU instead. With whether it's meant for the graphics card
    fn session(environment: &'static ort::environment::Environment, model: &Path, gpu: bool) -> Result<(Session, bool), String> {
        #[cfg(windows)]
        let gpu = gpu && directml();
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
        builder.commit_from_file(model).map(|session| (session, gpu)).map_err(|error| error.to_string())
    }

    /// Whether DirectML can run the model: Windows' own DirectML.dll is
    /// there and at least 1.15, the version ONNX Runtime is built with, as
    /// older ones, on older Windows, can fail in ways it doesn't catch.
    /// It's loaded from System32 here, as Ligma delay-loads it (see
    /// build.rs), so ONNX Runtime then uses this one rather than any other
    /// DirectML.dll it might find first
    #[cfg(windows)]
    fn directml() -> bool {
        use windows::core::{w, HSTRING};
        use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO};
        use windows::Win32::System::LibraryLoader::LoadLibraryW;
        static USABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *USABLE.get_or_init(|| {
            let Some(root) = std::env::var_os("SystemRoot") else {
                return false;
            };
            let path = HSTRING::from(Path::new(&root).join("System32").join("DirectML.dll").as_os_str());
            // SAFETY: the buffer is as large as Windows asks for, and the
            // fixed version info is read only where it says it is
            let version = unsafe {
                let size = GetFileVersionInfoSizeW(&path, None);
                let mut data = vec![0u8; size as usize];
                let (mut info, mut length) = (std::ptr::null_mut(), 0u32);
                if size == 0
                    || GetFileVersionInfoW(&path, None, size, data.as_mut_ptr().cast()).is_err()
                    || !VerQueryValueW(data.as_ptr().cast(), w!("\\"), &mut info, &mut length).as_bool()
                    || info.is_null()
                    || (length as usize) < size_of::<VS_FIXEDFILEINFO>()
                {
                    None
                } else {
                    let fixed = &*(info as *const VS_FIXEDFILEINFO);
                    Some((fixed.dwFileVersionMS >> 16, fixed.dwFileVersionMS & 0xffff))
                }
            };
            let usable = version.is_some_and(|version| version >= (1, 15)) && unsafe { LoadLibraryW(&path) }.is_ok();
            if !usable {
                log::info!("Upscale: no DirectML 1.15 or later ({version:?}), using the CPU");
            }
            usable
        })
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

    /// Images that read as enlarged this many times or more before are
    /// shrunk back first, at most `MOST_SHRINK` times
    const SHRINK_FROM: f32 = 2.0;
    const MOST_SHRINK: f32 = 4.0;
    /// How sharp an image is is read in squares this many pixels across,
    /// those whose brightness spreads this little left out as there's
    /// nothing in them to read
    const SHARP_TILE: usize = 128;
    const SHARP_SPREAD: f32 = 8.0;

    /// How many times `rgb` reads as enlarged before: how far short of the
    /// finest detail a pixel can hold most of its squares stop, in where
    /// their detail fades to a thousandth of their broad shapes', so a few
    /// blurry ones, like a background out of focus, don't count. 1 when
    /// it's as sharp as its pixels allow
    fn shrink_by(rgb: &RgbImage) -> f32 {
        const T: usize = SHARP_TILE;
        let (width, height) = (rgb.width() as usize, rgb.height() as usize);
        if width < T || height < T {
            return 1.0;
        }
        let luma: Vec<f32> = rgb.pixels().map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32).collect();
        let window: Vec<f32> = (0..T).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / (T - 1) as f32).cos()).collect();
        // At most 17 squares each way, overlapping by half where it's small
        let starts = |size: usize| {
            let step = ((size - T) / 16).max(T / 2);
            (0..=size - T).step_by(step).collect::<Vec<_>>()
        };
        let mut reached = Vec::new();
        let mut re = vec![0f32; T * T];
        let mut im = vec![0f32; T * T];
        for y0 in starts(height) {
            for x0 in starts(width) {
                let (mut sum, mut squares) = (0f32, 0f32);
                for y in y0..y0 + T {
                    for &value in &luma[y * width + x0..y * width + x0 + T] {
                        sum += value;
                        squares += value * value;
                    }
                }
                let mean = sum / (T * T) as f32;
                if (squares / (T * T) as f32 - mean * mean).max(0.0).sqrt() < SHARP_SPREAD {
                    continue;
                }
                for y in 0..T {
                    for x in 0..T {
                        re[y * T + x] = (luma[(y0 + y) * width + x0 + x] - mean) * window[y] * window[x];
                        im[y * T + x] = 0.0;
                    }
                }
                fft_2d(&mut re, &mut im, T);
                // Power by how fine it is, in pixels across a wave: `k` waves
                // in the square, the larger of across and down
                let (mut power, mut count) = ([0f32; T / 2 + 1], [0f32; T / 2 + 1]);
                for ky in 0..T {
                    for kx in 0..T {
                        let fine = |k: usize| k.min(T - k);
                        let k = fine(ky).max(fine(kx));
                        let at = ky * T + kx;
                        power[k] += re[at] * re[at] + im[at] * im[at];
                        count[k] += 1.0;
                    }
                }
                let mean = |from: usize, to: usize| power[from..to].iter().sum::<f32>() / count[from..to].iter().sum::<f32>();
                // Broad shapes, 17 to 32 pixels across, and where detail
                // fades, from 12 pixels across
                let broad = mean(4, 8);
                let fades = (11..T / 2).find(|&k| mean(k, (k + 2).min(T / 2 + 1)) < broad * 1e-3);
                reached.push(fades.map_or(0.5, |k| (k - 1) as f32 / T as f32));
            }
        }
        if reached.len() < 4 {
            return 1.0;
        }
        reached.sort_by(f32::total_cmp);
        // A fifth again, as compression leaves some finer detail behind
        (0.6 / reached[reached.len() * 3 / 4]).clamp(1.0, MOST_SHRINK)
    }

    /// The Fourier transform of the `n` by `n` complex values in `re` and
    /// `im`, in place, `n` a power of 2
    fn fft_2d(re: &mut [f32], im: &mut [f32], n: usize) {
        let mut row_re = vec![0f32; n];
        let mut row_im = vec![0f32; n];
        for pass in 0..2 {
            for line in 0..n {
                let at = |i: usize| if pass == 0 { line * n + i } else { i * n + line };
                for i in 0..n {
                    (row_re[i], row_im[i]) = (re[at(i)], im[at(i)]);
                }
                fft(&mut row_re, &mut row_im);
                for i in 0..n {
                    (re[at(i)], im[at(i)]) = (row_re[i], row_im[i]);
                }
            }
        }
    }

    /// The Fourier transform of `re` and `im`, in place, their length a
    /// power of 2
    fn fft(re: &mut [f32], im: &mut [f32]) {
        let n = re.len();
        let mut j = 0;
        for i in 1..n {
            let mut bit = n >> 1;
            while j & bit != 0 {
                j ^= bit;
                bit >>= 1;
            }
            j |= bit;
            if i < j {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut size = 2;
        while size <= n {
            let angle = -std::f32::consts::TAU / size as f32;
            for start in (0..n).step_by(size) {
                for k in 0..size / 2 {
                    let (sin, cos) = (angle * k as f32).sin_cos();
                    let (a, b) = (start + k, start + k + size / 2);
                    let (tr, ti) = (re[b] * cos - im[b] * sin, re[b] * sin + im[b] * cos);
                    (re[b], im[b]) = (re[a] - tr, im[a] - ti);
                    (re[a], im[a]) = (re[a] + tr, im[a] + ti);
                }
            }
            size <<= 1;
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
    /// one, its detail kept to `strength` of the way (see `settle`), in
    /// tiles that read at most `most` pixels on a graphics card, saying how
    /// far it's got with `progress`
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        model: &Path,
        folder: Option<&Path>,
        image: &RgbaImage,
        factor: f32,
        strength: f32,
        most: usize,
        options: &RunOptions,
        stopped: &dyn Fn() -> bool,
        progress: &dyn Fn(&str),
    ) -> Result<DynamicImage, String> {
        let (width, height) = (image.width() as usize, image.height() as usize);
        let environment = crate::models::environment(folder.unwrap_or(Path::new("")))?;
        // Tests can ask for the CPU, to compare
        let gpu = !(cfg!(test) && std::env::var("LIGMA_UPSCALE_CPU").is_ok());
        let (mut session, mut on_gpu) = session(environment, model, gpu).or_else(|_| session(environment, model, false))?;
        // Transparent pixels as they'd show on black, as the model reads color
        let rgb = RgbImage::from_fn(width as u32, height as u32, |x, y| {
            let [r, g, b, a] = image.get_pixel(x, y).0;
            let alpha = a as u16;
            image::Rgb([r, g, b].map(|value| ((value as u16 * alpha + 127) / 255) as u8))
        });
        let target = ((width as f32 * factor).round() as u32, (height as f32 * factor).round() as u32);
        // An image enlarged before holds no more than a smaller one would,
        // spread out and blocky where it was compressed, which a model only
        // enlarges further. It's read at the size its detail is for instead,
        // so the model sees edges, and enlarged twice where once isn't enough
        let shrink = shrink_by(&rgb);
        let mut work = if shrink >= SHRINK_FROM {
            log::info!("Upscale: enlarged {shrink:.1} times before, shrunk back first");
            let size = |side: usize| ((side as f32 / shrink).round() as u32).max(1);
            imageops::resize(&rgb, size(width), size(height), FilterType::Lanczos3)
        } else {
            rgb.clone()
        };
        let passes = if work.width() * TIMES as u32 >= target.0 && work.height() * TIMES as u32 >= target.1 { 1 } else { 2 };
        let mut out = RgbImage::new(0, 0);
        for pass in 0..passes {
            if pass > 0 {
                let size = |side: u32| side.div_ceil(TIMES as u32);
                work = imageops::resize(&out, size(target.0), size(target.1), FilterType::Lanczos3);
            }
            // The last pass comes out at the size asked for, so what the
            // model makes is never held 4 times as large in full
            let size = if pass + 1 == passes { target } else { (work.width() * TIMES as u32, work.height() * TIMES as u32) };
            let step = |done: usize| progress(&format!("Upscaling… {}%", (pass * 100 + done) / passes));
            out = enlarge(&mut session, environment, model, &mut on_gpu, &work, size, most, options, stopped, &step)?;
        }
        drop(session);
        progress("Finishing…");
        settle(&work, &mut out, strength);
        let (out_width, out_height) = (out.width(), out.height());
        // An opaque image stays so, without its transparency read
        if image.pixels().all(|pixel| pixel[3] == 255) {
            return Ok(DynamicImage::ImageRgb8(out));
        }
        // Transparency scaled up smoothly, and the color back off black
        let alpha = imageops::resize(
            &image::GrayImage::from_fn(width as u32, height as u32, |x, y| image::Luma([image.get_pixel(x, y)[3]])),
            out_width,
            out_height,
            FilterType::CatmullRom,
        );
        let larger = RgbaImage::from_fn(out_width, out_height, |x, y| {
            let [r, g, b] = out.get_pixel(x, y).0;
            let a = alpha.get_pixel(x, y)[0];
            if a == 255 {
                return image::Rgba([r, g, b, 255]);
            }
            if a == 0 {
                return image::Rgba([0, 0, 0, 0]);
            }
            let unmultiply = |value: u8| ((value as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
            image::Rgba([unmultiply(r), unmultiply(g), unmultiply(b), a])
        });
        Ok(DynamicImage::ImageRgba8(larger))
    }

    /// How each pixel along a side `size` long is read from one `big` long,
    /// at most as long, as the average of what it covers: the first pixel
    /// it covers, how much of each from there counts, and where its middle
    /// falls
    struct Span {
        start: usize,
        weights: Vec<f32>,
        middle: f32,
    }

    fn spans(size: usize, big: usize) -> Vec<Span> {
        let scale = big as f32 / size as f32;
        (0..size)
            .map(|i| {
                let (low, high) = (i as f32 * scale, ((i + 1) as f32 * scale).min(big as f32));
                let start = (low.floor() as usize).min(big - 1);
                let end = (high.ceil() as usize).clamp(start + 1, big);
                let covered = (high - low).max(f32::EPSILON);
                let weights = (start..end).map(|p| ((p + 1) as f32).min(high) - (p as f32).max(low)).map(|w| w.max(0.0) / covered).collect();
                Span { start, weights, middle: (low + high) / 2.0 }
            })
            .collect()
    }

    /// `count` tiles as long along a side `size` long: how long, and where
    /// each starts, so they meet rather than the last overlapping the one
    /// before by most of its length
    fn split(size: usize, count: usize) -> (usize, Vec<usize>) {
        let tile = size.div_ceil(count.clamp(1, size));
        let mut starts: Vec<usize> = (0..count).map(|i| (i * tile).min(size - tile)).collect();
        starts.dedup();
        (tile, starts)
    }

    /// How a `width` by `height` image is split into tiles all the same
    /// size, none reading more than `most` pixels with what's around it:
    /// across and down, those that read the fewest in all, so the least is
    /// read twice. At least a pixel's read with what's around it, however
    /// little `most` is
    fn layout(width: usize, height: usize, most: usize) -> ((usize, Vec<usize>), (usize, Vec<usize>)) {
        let most = most.max((1 + 2 * PAD).pow(2));
        let mut best = (usize::MAX, 1, 1);
        for columns in 1..=width {
            let read_across = width.div_ceil(columns) + 2 * PAD;
            // As few rows as fit
            let Some(down) = (most / read_across).checked_sub(2 * PAD).filter(|&down| down > 0) else {
                continue;
            };
            let rows = height.div_ceil(down);
            let read = columns * rows * read_across * (height.div_ceil(rows) + 2 * PAD);
            if read < best.0 {
                best = (read, columns, rows);
            }
        }
        (split(width, best.1), split(height, best.2))
    }

    /// The color every pixel `t` reads has, if they all have the same: the
    /// model would only make that color larger, so it isn't asked
    fn uniform(rgb: &RgbImage, t: &Tile) -> Option<image::Rgb<u8>> {
        let (width, height) = (rgb.width() as isize, rgb.height() as isize);
        let (left, top) = (t.x0.clamp(0, width - 1) as u32, t.y0.clamp(0, height - 1) as u32);
        let (right, bottom) = ((t.x0 + t.read_across as isize).clamp(1, width) as u32, (t.y0 + t.read_down as isize).clamp(1, height) as u32);
        let first = *rgb.get_pixel(left, top);
        (top..bottom).all(|y| (left..right).all(|x| *rgb.get_pixel(x, y) == first)).then_some(first)
    }

    /// `rgb` made `TIMES` as large with `session`, a tile at a time, each
    /// reading at most `most` pixels on a graphics card, and read back at
    /// `size`, at most that large, moved to the CPU, in `on_gpu`, and
    /// started over in its tiles if the graphics card fails, saying how far
    /// it's got, in percent, with `progress`
    #[allow(clippy::too_many_arguments)]
    fn enlarge(
        session: &mut Session,
        environment: &'static ort::environment::Environment,
        model: &Path,
        on_gpu: &mut bool,
        rgb: &RgbImage,
        size: (u32, u32),
        most: usize,
        options: &RunOptions,
        stopped: &dyn Fn() -> bool,
        progress: &dyn Fn(usize),
    ) -> Result<RgbImage, String> {
        let error = |error: ort::Error| error.to_string();
        let (width, height) = (rgb.width() as usize, rgb.height() as usize);
        let mut out = RgbImage::new(size.0, size.1);
        // Each pixel of `out` comes from the tile its middle falls in, as
        // the tiles are read with enough around them for what it covers
        let (columns, rows) = (spans(size.0 as usize, width * TIMES), spans(size.1 as usize, height * TIMES));
        // Tiles on a graphics card are all read the same size, their edge
        // pixels carried on past the image's, so the model is prepared for
        // one size only, and the image is split into as few as fit. On the
        // CPU, where that costs nothing, they're smaller, and those at the
        // edges smaller still, so less is read twice
        let tiles: Vec<Tile> = if *on_gpu {
            let ((across, xs), (down, ys)) = layout(width, height, most);
            ys.into_iter()
                .flat_map(|y| xs.iter().map(move |&x| (x, y)))
                .map(|(x, y)| Tile { x, y, across, down, x0: x as isize - PAD as isize, y0: y as isize - PAD as isize, read_across: across + 2 * PAD, read_down: down + 2 * PAD })
                .collect()
        } else {
            let tile = CPU_TILE;
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
        // Only the pixels whose middle is in a tile itself, not in what was
        // read around it
        let within = |spans: &[Span], from: usize, to: usize| {
            let (from, to) = ((from * TIMES) as f32, (to * TIMES) as f32);
            spans.partition_point(|span| span.middle < from)..spans.partition_point(|span| span.middle < to)
        };
        for (index, t) in tiles.iter().enumerate() {
            if stopped() {
                return Err("cancelled".to_owned());
            }
            progress(index * 100 / tiles.len());
            // Like a plain background, or what's see-through
            if let Some(color) = uniform(rgb, t) {
                for oy in within(&rows, t.y, t.y + t.down) {
                    for ox in within(&columns, t.x, t.x + t.across) {
                        out.put_pixel(ox as u32, oy as u32, color);
                    }
                }
                continue;
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
            let output = match ask(session) {
                Ok(output) => output,
                // The graphics card failed: the CPU from then on, in its
                // own tiles, as the card's are too large for it
                Err(_) if *on_gpu && !stopped() => {
                    log::warn!("Upscale: the graphics card failed, using the CPU");
                    *session = self::session(environment, model, false)?.0;
                    *on_gpu = false;
                    return enlarge(session, environment, model, on_gpu, rgb, size, most, options, stopped, progress);
                }
                Err(error) => return Err(error),
            };
            let (big_across, big_down) = (across * TIMES, down * TIMES);
            if output.len() != 3 * big_across * big_down {
                return Err("unexpected output".to_owned());
            }
            let big_plane = big_across * big_down;
            let (origin_x, origin_y) = (t.x0 * TIMES as isize, t.y0 * TIMES as isize);
            for oy in within(&rows, t.y, t.y + t.down) {
                let row = &rows[oy];
                for ox in within(&columns, t.x, t.x + t.across) {
                    let column = &columns[ox];
                    let mut sum = [0f32; 3];
                    for (j, &down_weight) in row.weights.iter().enumerate() {
                        let y = ((row.start + j) as isize - origin_y).clamp(0, big_down as isize - 1) as usize;
                        for (i, &across_weight) in column.weights.iter().enumerate() {
                            let x = ((column.start + i) as isize - origin_x).clamp(0, big_across as isize - 1) as usize;
                            let (at, weight) = (y * big_across + x, down_weight * across_weight);
                            for (channel, total) in sum.iter_mut().enumerate() {
                                *total += output[channel * big_plane + at] * weight;
                            }
                        }
                    }
                    let pixel = sum.map(|value| (value * 255.0).round().clamp(0.0, 255.0) as u8);
                    out.put_pixel(ox as u32, oy as u32, image::Rgb(pixel));
                }
            }
        }
        Ok(out)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reads_how_far_an_image_was_enlarged() {
            // Detail down to single pixels, a little blurred, as a camera has it
            let mut seed = 1u32;
            let noise = RgbImage::from_fn(256, 256, |_, _| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                image::Rgb([(seed >> 24) as u8; 3])
            });
            let sharp = imageops::blur(&noise, 0.6);
            assert!(shrink_by(&sharp) < SHRINK_FROM);
            let enlarged = imageops::resize(&sharp, 768, 768, FilterType::CatmullRom);
            let shrink = shrink_by(&enlarged);
            assert!((2.4..=MOST_SHRINK).contains(&shrink), "{shrink}");
            // Nothing to read
            assert_eq!(shrink_by(&RgbImage::from_pixel(512, 512, image::Rgb([90, 120, 60]))), 1.0);
        }

        #[test]
        fn fourier_transform() {
            let mut re: Vec<f32> = (0..8).map(|i| (std::f32::consts::TAU * i as f32 / 8.0).cos()).collect();
            let mut im = vec![0f32; 8];
            fft(&mut re, &mut im);
            for (k, (r, i)) in re.iter().zip(&im).enumerate() {
                let expected = if k == 1 || k == 7 { 4.0 } else { 0.0 };
                assert!((r - expected).abs() < 1e-4 && i.abs() < 1e-4, "{k}: {r} {i}");
            }
        }

        #[test]
        fn spans_average_what_they_cover() {
            for (size, big) in [(10, 10), (5, 10), (1366, 4000), (4096, 16384), (3, 7)] {
                let spans = spans(size, big);
                assert_eq!(spans.len(), size);
                for span in &spans {
                    assert!((span.weights.iter().sum::<f32>() - 1.0).abs() < 1e-3, "{size} {big}");
                    assert!(span.start + span.weights.len() <= big);
                    assert!((span.start as f32) <= span.middle && span.middle <= (span.start + span.weights.len()) as f32);
                }
            }
            // The same size reads each pixel as it is
            assert!(spans(8, 8).iter().enumerate().all(|(i, span)| span.start == i && span.weights == [1.0]));
        }

        #[test]
        fn tiles_split_sides_evenly() {
            for (size, count) in [(1, 1), (384, 1), (385, 2), (1100, 3), (4096, 16), (777, 3), (5, 4)] {
                let (tile, starts) = split(size, count);
                assert!(starts.len() <= count, "{size} {count}");
                assert_eq!(starts[0], 0);
                assert_eq!(starts.last().unwrap() + tile, size);
                // Each meets the next, overlapping it by less than one pixel per tile
                assert!(starts.windows(2).all(|pair| pair[1] > pair[0] && pair[1] <= pair[0] + tile && pair[0] + tile - pair[1] < count));
            }
            assert_eq!(split(1100, 3), (367, vec![0, 367, 733]));
        }

        #[test]
        fn tiles_read_no_more_than_they_may() {
            for (width, height, most) in [(960, 643, PHOTO_READ), (782, 1200, ILLUSTRATION_READ), (4096, 64, FAST_READ), (1, 1, PHOTO_READ), (2048, 2048, PHOTO_READ), (300, 200, 0)] {
                let ((across, xs), (down, ys)) = layout(width, height, most);
                assert!((across + 2 * PAD) * (down + 2 * PAD) <= most.max((1 + 2 * PAD).pow(2)), "{width} {height}");
                assert_eq!((xs.last().unwrap() + across, ys.last().unwrap() + down), (width, height));
            }
            // An image that fits is one tile
            assert_eq!(layout(600, 400, PHOTO_READ), ((600, vec![0]), (400, vec![0])));
            // and one that nearly does two, not four
            let ((_, xs), (_, ys)) = layout(960, 643, PHOTO_READ);
            assert_eq!(xs.len() * ys.len(), 2);
        }

        #[test]
        fn plain_tiles_are_found() {
            let mut rgb = RgbImage::from_pixel(100, 100, image::Rgb([10, 20, 30]));
            let tile = Tile { x: 0, y: 0, across: 40, down: 40, x0: -(PAD as isize), y0: -(PAD as isize), read_across: 40 + 2 * PAD, read_down: 40 + 2 * PAD };
            assert_eq!(uniform(&rgb, &tile), Some(image::Rgb([10, 20, 30])));
            // A pixel just within what's read around the tile
            rgb.put_pixel(71, 5, image::Rgb([10, 20, 31]));
            assert_eq!(uniform(&rgb, &tile), None);
        }

        /// Upscales `LIGMA_UPSCALE_IMAGE` with the model at `LIGMA_UPSCALE`
        /// 4 times and writes it next to the image, on the graphics card
        /// unless `LIGMA_UPSCALE_CPU` is set, in tiles that read as much as
        /// a square `LIGMA_UPSCALE_TILE` across. `cargo test --release --
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
            let most = std::env::var("LIGMA_UPSCALE_TILE").ok().and_then(|value| value.parse::<usize>().ok()).map_or(PHOTO_READ, |side| side * side);
            let larger = run(Path::new(&model), None, &image, factor, strength, most, &options, &|| false, &|_| {}).unwrap();
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
