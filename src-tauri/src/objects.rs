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
//! fractions of the image, with `"subtract":true` to take it away from the
//! picks before it rather than add it. One image is kept at a time

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
}

#[derive(Clone, Deserialize)]
#[cfg_attr(not(birefnet), allow(dead_code))]
struct Pick {
    point: Option<[f32; 2]>,
    #[serde(rename = "box")]
    rect: Option<[f32; 4]>,
    #[serde(default)]
    subtract: bool,
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

    use super::{Pick, Request};
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
                let logits = picked(loaded, &request.picks)?;
                match op {
                    "outline" => Ok(outline(&loaded.image, &logits)),
                    "isolate" => isolate(&logits, image),
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

    /// The picks as one mask, in logits: each added to those before it, or
    /// taken away from them
    fn picked(loaded: &mut Loaded, picks: &[Pick]) -> Result<Vec<f32>, &'static str> {
        let mut logits: Option<Vec<f32>> = None;
        for pick in picks {
            let mask = mask(loaded, pick)?;
            logits = Some(match logits {
                None if pick.subtract => continue,
                None => mask,
                Some(mut logits) => {
                    for (logit, &other) in logits.iter_mut().zip(&mask) {
                        *logit = if pick.subtract { logit.min(-other) } else { logit.max(other) };
                    }
                    logits
                }
            });
        }
        logits.ok_or("empty")
    }

    /// The mask SAM finds for `pick`, `MASK` square, in logits. Of the three
    /// it gives, from the whole to a part, the one it scores best
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
        let mask = masks[best * side..(best + 1) * side].to_vec();
        drop(outputs);
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

    /// The outline of the mask in `logits` over the image, without specks
    fn outline(image: &Image, logits: &[f32]) -> String {
        let (width, height) = (image.width, image.height);
        let mask: Vec<bool> = stretched(logits, width, height).iter().zip(&image.opaque).map(|(&logit, &opaque)| logit > 0.0 && opaque).collect();
        let d = crate::vectorize::outline(&mask, width, height, (width * height / 4000).max(4));
        serde_json::json!({ "d": d }).to_string()
    }

    /// The image in `bytes`, the layer drawn larger, with only what the mask
    /// in `logits` covers, cropped to it. Its edge is antialiased, a pixel or
    /// two wide however large the image
    fn isolate(logits: &[f32], bytes: &[u8]) -> Result<String, &'static str> {
        let mut picture = decode(bytes)?;
        let (width, height) = (picture.width() as usize, picture.height() as usize);
        let sharpness = (width.max(height) as f32 / MASK as f32 / 4.0).max(1.0);
        let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
        for (i, (pixel, &logit)) in picture.pixels_mut().zip(&stretched(logits, width, height)).enumerate() {
            let kept = 1.0 / (1.0 + (-logit * sharpness).exp());
            pixel[3] = (pixel[3] as f32 * kept).round() as u8;
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
