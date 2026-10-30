//! Remove background on this device (Settings > Features > Images): the
//! image bar's Remove background in free projects, which Ligma puts in
//! place of Figma's (see ai_actions.rs). On macOS 14 and later, Apple's
//! Vision finds the image's subject, as Preview's Remove Background does.
//! On Windows it's BiRefNet (MIT), an ONNX model run on the CPU with ONNX
//! Runtime, which is linked in. The model is 224 MB, so it's downloaded
//! the first time it's needed, once asked, and checked against its SHA-256
//!
//! Figma tabs get no IPC, so the page hands the image over as a download
//! named `ligma-background-<job>`, which `on_requested` saves to a
//! temporary file instead of asking where. Once it's saved, the cut-out
//! goes back to the page as a PNG, through
//! `window.__ligmaRemoveBackground.done(job, base64)`, or `failed(job,
//! reason)`. While the model downloads, `progress(job, text)` says how far
//! it's got

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager, Url, Webview};

const PREFIX: &str = "ligma-background-";
/// Larger than Figma accepts, so anything bigger isn't an image of a file
const MAX_BYTES: u64 = 64 << 20;

/// Images on their way: the download's URL, the tab and job it's for and
/// its temporary file
static PENDING: Mutex<Vec<(Url, u32, u64, PathBuf)>> = Mutex::new(Vec::new());

/// Whether this Mac can remove backgrounds
#[cfg(target_os = "macos")]
pub fn available() -> bool {
    vision::available()
}

/// Windows can, once it has the model
#[cfg(windows)]
pub fn available() -> bool {
    true
}

/// An image a tab hands over: saved to a temporary file. False for other
/// downloads
pub fn on_requested(webview: &Webview, url: &Url, destination: &mut PathBuf) -> bool {
    if url.scheme() != "blob" {
        return false;
    }
    // WebView2 may add an extension, like ".txt", or " (1)" to the name
    let Some(job) = destination.file_name().and_then(|name| {
        let rest = name.to_str()?.strip_prefix(PREFIX)?;
        let digits = rest.find(|char: char| !char.is_ascii_digit()).unwrap_or(rest.len());
        rest[..digits].parse::<u64>().ok()
    }) else {
        return false;
    };
    let Some(tab) = webview.label().strip_prefix("tab-").and_then(|id| id.parse().ok()) else {
        return false;
    };
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_nanos());
    let temp = std::env::temp_dir().join(format!("{PREFIX}{nanos}"));
    PENDING.lock().unwrap().push((url.clone(), tab, job, temp.clone()));
    *destination = temp;
    true
}

/// A download `on_requested` took is saved: removes the background off the
/// main thread and sends the result to its tab. False for other downloads
pub fn on_finished(webview: &Webview, url: &Url, success: bool) -> bool {
    let pending = {
        let mut pending = PENDING.lock().unwrap();
        let index = pending.iter().position(|(pending, ..)| pending == url);
        index.map(|index| pending.remove(index))
    };
    let Some((_, tab, job, temp)) = pending else {
        return false;
    };
    let app = webview.app_handle().clone();
    std::thread::spawn(move || {
        let result = if success { remove(&app, tab, job, &temp) } else { Err(Failure::Failed) };
        let _ = std::fs::remove_file(&temp);
        if let Err(Failure::Failed) = result {
            log::warn!("Couldn't remove the background of an image");
        }
        reply(&app, tab, job, result);
    });
    true
}

enum Failure {
    /// Nothing to keep was found
    NoSubject,
    /// The model wasn't downloaded
    #[cfg_attr(not(windows), allow(dead_code))]
    Cancelled,
    Failed,
}

/// The image in `path` with its background made transparent, as a PNG
/// in base64, for `job` in tab `tab`
fn remove(app: &AppHandle, tab: u32, job: u64, path: &Path) -> Result<String, Failure> {
    if std::fs::metadata(path).map_err(|_| Failure::Failed)?.len() > MAX_BYTES {
        return Err(Failure::Failed);
    }
    let bytes = std::fs::read(path).map_err(|_| Failure::Failed)?;
    #[cfg(target_os = "macos")]
    return {
        let _ = (app, tab, job);
        vision::remove(&bytes)
    };
    #[cfg(windows)]
    return birefnet::remove(app, &bytes, |text| progress(app, tab, job, text));
}

/// Tells the page in tab `tab` how far `job` has got
#[cfg_attr(not(windows), allow(dead_code))]
fn progress(app: &AppHandle, tab: u32, job: u64, text: &str) {
    if let Some(webview) = crate::tabs::scriptable(app, tab) {
        let _ = webview.eval(format!("window.__ligmaRemoveBackground?.progress({job}, {text:?})"));
    }
}

/// Sends `result` for `job` to tab `tab`, if it can still run JavaScript.
/// Otherwise the page gives up on it in time
fn reply(app: &AppHandle, tab: u32, job: u64, result: Result<String, Failure>) {
    let Some(webview) = crate::tabs::scriptable(app, tab) else {
        return;
    };
    let script = match result {
        Ok(png) => format!("window.__ligmaRemoveBackground?.done({job}, {png:?})"),
        Err(Failure::NoSubject) => format!("window.__ligmaRemoveBackground?.failed({job}, \"subject\")"),
        Err(Failure::Cancelled) => format!("window.__ligmaRemoveBackground?.failed({job}, \"cancelled\")"),
        Err(Failure::Failed) => format!("window.__ligmaRemoveBackground?.failed({job}, \"failed\")"),
    };
    let _ = webview.eval(script);
}

/// Apple's Vision, through `VNGenerateForegroundInstanceMaskRequest`
/// (macOS 14+). It runs on the GPU: on some Macs loading its model onto the
/// Neural Engine never finishes
#[cfg(target_os = "macos")]
mod vision {
    use std::ffi::{c_char, c_void, CStr};
    use std::panic::AssertUnwindSafe;
    use std::ptr::null_mut;

    use objc2::encode::{Encoding, RefEncode};
    use objc2::rc::{autoreleasepool, Allocated, Retained};
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2::{class, msg_send};

    use super::Failure;

    /// `CVPixelBufferRef` and `CGColorSpaceRef`, typed for objc2's checks
    #[repr(C)]
    struct CVBuffer([u8; 0]);
    unsafe impl RefEncode for CVBuffer {
        const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("__CVBuffer", &[]));
    }
    #[repr(C)]
    struct CGColorSpace([u8; 0]);
    unsafe impl RefEncode for CGColorSpace {
        const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGColorSpace", &[]));
    }

    #[link(name = "Vision", kind = "framework")]
    extern "C" {}

    #[link(name = "CoreImage", kind = "framework")]
    extern "C" {
        static kCIImageApplyOrientationProperty: *mut AnyObject;
        static kCIFormatRGBA8: i32;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        static kCGColorSpaceSRGB: *const c_void;
        fn CGColorSpaceCreateWithName(name: *const c_void) -> *mut CGColorSpace;
        fn CGColorSpaceGetModel(space: *const CGColorSpace) -> i32;
        fn CGColorSpaceRelease(space: *mut CGColorSpace);
    }

    /// `kCGColorSpaceModelRGB`
    const RGB_MODEL: i32 = 1;

    pub fn available() -> bool {
        AnyClass::get(c"VNGenerateForegroundInstanceMaskRequest").is_some()
    }

    pub fn remove(bytes: &[u8]) -> Result<String, Failure> {
        let mut result = Err(Failure::Failed);
        let caught = objc2::exception::catch(AssertUnwindSafe(|| {
            result = autoreleasepool(|_| unsafe { cut_out(bytes) });
        }));
        if let Err(exception) = caught {
            log::warn!("Vision threw {exception:?}");
        }
        result
    }

    unsafe fn cut_out(bytes: &[u8]) -> Result<String, Failure> {
        let (Some(handler_class), Some(request_class)) = (
            AnyClass::get(c"VNImageRequestHandler"),
            AnyClass::get(c"VNGenerateForegroundInstanceMaskRequest"),
        ) else {
            return Err(Failure::Failed);
        };
        let data: *mut AnyObject =
            msg_send![class!(NSData), dataWithBytes: bytes.as_ptr().cast::<c_void>(), length: bytes.len()];
        // Turned as the photo's orientation says, as Figma shows it
        let yes: *mut AnyObject = msg_send![class!(NSNumber), numberWithBool: Bool::YES];
        let options: *mut AnyObject =
            msg_send![class!(NSDictionary), dictionaryWithObject: yes, forKey: kCIImageApplyOrientationProperty];
        let image: *mut AnyObject = msg_send![class!(CIImage), imageWithData: data, options: options];
        if image.is_null() {
            return Err(Failure::Failed);
        }
        let empty: *mut AnyObject = msg_send![class!(NSDictionary), dictionary];
        let handler: Allocated<AnyObject> = msg_send![handler_class, alloc];
        let handler: Option<Retained<AnyObject>> = msg_send![handler, initWithCIImage: image, options: empty];
        let handler = handler.ok_or(Failure::Failed)?;
        let request: Option<Retained<AnyObject>> = msg_send![request_class, new];
        let request = request.ok_or(Failure::Failed)?;
        use_gpu(&request);

        let requests: *mut AnyObject = msg_send![class!(NSArray), arrayWithObject: &*request];
        let mut error: *mut AnyObject = null_mut();
        let done: Bool = msg_send![&*handler, performRequests: requests, error: &mut error];
        if !done.as_bool() {
            return Err(Failure::Failed);
        }
        let results: *mut AnyObject = msg_send![&*request, results];
        let observation: *mut AnyObject = if results.is_null() { null_mut() } else { msg_send![results, firstObject] };
        if observation.is_null() {
            return Err(Failure::NoSubject);
        }
        let instances: *mut AnyObject = msg_send![observation, allInstances];
        let buffer: *mut CVBuffer = msg_send![
            observation,
            generateMaskedImageOfInstances: instances,
            fromRequestHandler: &*handler,
            croppedToInstancesExtent: Bool::NO,
            error: &mut error
        ];
        if buffer.is_null() {
            return Err(Failure::Failed);
        }
        let masked: *mut AnyObject = msg_send![class!(CIImage), imageWithCVPixelBuffer: buffer];
        if masked.is_null() {
            return Err(Failure::Failed);
        }

        // In the photo's own colors, unless they aren't RGB
        let own: *mut CGColorSpace = msg_send![image, colorSpace];
        let created = own.is_null() || CGColorSpaceGetModel(own) != RGB_MODEL;
        let space = if created { CGColorSpaceCreateWithName(kCGColorSpaceSRGB) } else { own };
        let no_options: *mut AnyObject = null_mut();
        let context: *mut AnyObject = msg_send![class!(CIContext), contextWithOptions: no_options];
        let png: *mut AnyObject = msg_send![
            context,
            PNGRepresentationOfImage: masked,
            format: kCIFormatRGBA8,
            colorSpace: space,
            options: empty
        ];
        if created && !space.is_null() {
            CGColorSpaceRelease(space);
        }
        if png.is_null() {
            return Err(Failure::Failed);
        }
        let base64: *mut AnyObject = msg_send![png, base64EncodedStringWithOptions: 0usize];
        let utf8: *const c_char = msg_send![base64, UTF8String];
        if utf8.is_null() {
            return Err(Failure::Failed);
        }
        Ok(CStr::from_ptr(utf8).to_string_lossy().into_owned())
    }

    /// Has every stage of `request` run on the GPU, where it can
    unsafe fn use_gpu(request: &AnyObject) {
        let Some(gpu) = AnyClass::get(c"MLGPUComputeDevice") else {
            return;
        };
        let mut error: *mut AnyObject = null_mut();
        let devices: *mut AnyObject = msg_send![request, supportedComputeStageDevicesAndReturnError: &mut error];
        if devices.is_null() {
            return;
        }
        let stages: *mut AnyObject = msg_send![devices, allKeys];
        let count: usize = msg_send![stages, count];
        for i in 0..count {
            let stage: *mut AnyObject = msg_send![stages, objectAtIndex: i];
            let list: *mut AnyObject = msg_send![devices, objectForKey: stage];
            let len: usize = msg_send![list, count];
            for j in 0..len {
                let device: *mut AnyObject = msg_send![list, objectAtIndex: j];
                let is_gpu: Bool = msg_send![device, isKindOfClass: gpu];
                if is_gpu.as_bool() {
                    let _: () = msg_send![request, setComputeDevice: device, forComputeStage: stage];
                    break;
                }
            }
        }
    }
}

/// BiRefNet-lite (https://github.com/ZhengPeng7/BiRefNet, MIT), as ONNX
/// from https://huggingface.co/onnx-community/BiRefNet_lite-ONNX. It takes
/// a 1024 × 1024 RGB image normalized as for ImageNet and gives the
/// foreground's logits at the same size, which are scaled back to the
/// image as its alpha
#[cfg(windows)]
mod birefnet {
    use std::fs::File;
    use std::io::{Cursor, Write};
    use std::path::{Path, PathBuf};
    use std::sync::{mpsc, Mutex, OnceLock};

    use base64::Engine;
    use image::imageops::{self, FilterType};
    use image::{DynamicImage, GrayImage, ImageDecoder, ImageEncoder, ImageReader, RgbaImage};
    use ort::environment::Environment;
    use ort::session::builder::AutoDevicePolicy;
    use ort::session::Session;
    use ort::value::TensorRef;
    use sha2::{Digest, Sha256};
    use tauri::{AppHandle, Manager};

    use super::Failure;

    /// Pinned to one revision, which `SHA256` is the hash of
    const URL: &str =
        "https://huggingface.co/onnx-community/BiRefNet_lite-ONNX/resolve/de15b22ba131738a16dff04aab8bdf8dc32e3ac1/onnx/model.onnx";
    const SHA256: &str = "5600024376f572a557870a5eb0afb1e5961636bef4e1e22132025467d0f03333";
    const SIZE: u64 = 224_005_088;
    const SIDE: u32 = 1024;
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];

    /// Held while the model is looked for or downloaded, so two images wait
    /// for one download
    static MODEL: Mutex<()> = Mutex::new(());

    fn failed(error: impl std::fmt::Display) -> Failure {
        log::warn!("Remove background: {error}");
        Failure::Failed
    }

    pub fn remove(app: &AppHandle, bytes: &[u8], progress: impl Fn(&str)) -> Result<String, Failure> {
        let model = model(app, &progress)?;
        cut_out(&model, bytes)
    }

    /// The model's file, downloaded first if it isn't there yet
    fn model(app: &AppHandle, progress: &impl Fn(&str)) -> Result<PathBuf, Failure> {
        let _held = MODEL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let path = app.path().app_local_data_dir().map_err(failed)?.join("models").join("birefnet-lite.onnx");
        if path.is_file() {
            return Ok(path);
        }
        if !ask(app) {
            return Err(Failure::Cancelled);
        }
        if let Err(error) = download(&path, progress) {
            crate::prompt::show(
                app,
                "Download Failed",
                format!("Couldn't download the background model: {error}"),
                &["OK"],
                |_| {},
            );
            return Err(failed(error));
        }
        progress("Removing background…");
        Ok(path)
    }

    /// Whether the person agrees to download the model
    fn ask(app: &AppHandle) -> bool {
        let (send, receive) = mpsc::channel();
        crate::prompt::show(
            app,
            "Download Background Model",
            "Removing backgrounds on this PC needs BiRefNet, a 224 MB model that Ligma downloads once from Hugging Face.",
            &["Cancel", "Download"],
            move |choice| {
                let _ = send.send(choice == Some(1));
            },
        );
        receive.recv().unwrap_or(false)
    }

    /// Downloads the model to `path`, through a `.part` file that's only
    /// renamed once its hash matches
    fn download(path: &Path, progress: &impl Fn(&str)) -> Result<(), String> {
        let folder = path.parent().ok_or("no folder for it")?;
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
        let part = path.with_extension("onnx.part");
        let result = tauri::async_runtime::block_on(async {
            let client = reqwest::Client::builder().tls_backend_native().build().map_err(|error| error.to_string())?;
            let mut response = client
                .get(URL)
                .send()
                .await
                .and_then(|response| response.error_for_status())
                .map_err(|error| error.to_string())?;
            let total = response.content_length().unwrap_or(SIZE).max(1);
            let mut file = File::create(&part).map_err(|error| error.to_string())?;
            let mut hash = Sha256::new();
            let mut got = 0u64;
            let mut shown = None;
            while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
                file.write_all(&chunk).map_err(|error| error.to_string())?;
                hash.update(&chunk);
                got += chunk.len() as u64;
                // In steps of 5%, so Figma's toast isn't redrawn too often
                let percent = (got * 100 / total).min(100) / 5 * 5;
                if shown != Some(percent) {
                    shown = Some(percent);
                    progress(&format!("Downloading the background model… {percent}%"));
                }
            }
            file.sync_all().map_err(|error| error.to_string())?;
            if format!("{:x}", hash.finalize()) != SHA256 {
                return Err("the file didn't match".to_owned());
            }
            Ok(())
        });
        match result.and_then(|()| std::fs::rename(&part, path).map_err(|error| error.to_string())) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = std::fs::remove_file(&part);
                Err(error)
            }
        }
    }

    /// The image in `bytes`, turned as its orientation says, as Figma shows
    /// it, and its color profile
    fn decode(bytes: &[u8]) -> Result<(RgbaImage, Option<Vec<u8>>), Failure> {
        let mut decoder = ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(failed)?
            .into_decoder()
            .map_err(failed)?;
        let orientation = decoder.orientation().map_err(failed)?;
        let profile = decoder.icc_profile().ok().flatten();
        let mut image = DynamicImage::from_decoder(decoder).map_err(failed)?;
        image.apply_orientation(orientation);
        Ok((image.into_rgba8(), profile))
    }

    fn environment() -> Result<&'static Environment, Failure> {
        static ENVIRONMENT: OnceLock<Environment> = OnceLock::new();
        if let Some(environment) = ENVIRONMENT.get() {
            return Ok(environment);
        }
        let environment = ort::init().with_name("ligma").build().map_err(failed)?;
        Ok(ENVIRONMENT.get_or_init(|| environment))
    }

    /// The model is loaded for each image and let go after, as it takes
    /// hundreds of MB
    fn cut_out(model: &Path, bytes: &[u8]) -> Result<String, Failure> {
        let (mut image, profile) = decode(bytes)?;
        let (width, height) = image.dimensions();
        let plane = (SIDE * SIDE) as usize;
        let mut input = vec![0f32; 3 * plane];
        let small = imageops::resize(&image, SIDE, SIDE, FilterType::Triangle);
        for (x, y, pixel) in small.enumerate_pixels() {
            let at = (y * SIDE + x) as usize;
            for channel in 0..3 {
                input[channel * plane + at] = (pixel[channel] as f32 / 255.0 - MEAN[channel]) / STD[channel];
            }
        }

        let mut session = Session::builder(environment()?)
            .map_err(failed)?
            .with_auto_device(AutoDevicePolicy::PreferCPU)
            .map_err(failed)?
            .commit_from_file(model)
            .map_err(failed)?;
        let tensor = TensorRef::from_array_view(([1usize, 3, SIDE as usize, SIDE as usize], &*input)).map_err(failed)?;
        let outputs = session.run(ort::inputs![tensor]).map_err(failed)?;
        let (_, logits) = outputs[outputs.len() - 1].try_extract_tensor::<f32>().map_err(failed)?;
        if logits.len() != plane {
            return Err(failed("unexpected output"));
        }
        let mask = GrayImage::from_fn(SIDE, SIDE, |x, y| {
            let logit = logits[(y * SIDE + x) as usize];
            image::Luma([(255.0 / (1.0 + (-logit).exp())).round() as u8])
        });
        drop(outputs);
        drop(session);
        if mask.pixels().all(|pixel| pixel[0] < 128) {
            return Err(Failure::NoSubject);
        }

        let mask = imageops::resize(&mask, width, height, FilterType::Triangle);
        for (pixel, kept) in image.pixels_mut().zip(mask.pixels()) {
            pixel[3] = ((pixel[3] as u16 * kept[0] as u16 + 127) / 255) as u8;
        }
        let mut png = Vec::new();
        let mut encoder = image::codecs::png::PngEncoder::new(&mut png);
        if let Some(profile) = profile {
            let _ = encoder.set_icc_profile(profile);
        }
        encoder
            .write_image(image.as_raw(), width, height, image::ExtendedColorType::Rgba8)
            .map_err(failed)?;
        Ok(base64::engine::general_purpose::STANDARD.encode(png))
    }
}
