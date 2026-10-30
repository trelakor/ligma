//! Remove background on this device (Settings > Features > Images): the
//! image bar's Remove background in free projects, which Ligma puts in
//! place of Figma's (see ai_actions.rs). On macOS 14 and later, Apple's
//! Vision finds the image's subject, as Preview's Remove Background does.
//! On Windows it's BiRefNet (MIT), an ONNX model run on the CPU with ONNX
//! Runtime, which is linked in. Macs with Apple silicon can use BiRefNet
//! instead of Vision, with ONNX Runtime downloaded along with it. The model
//! is 224 MB, so it's downloaded when it's first needed, once asked, or
//! from Settings, which can also remove it. Downloads are checked against
//! their SHA-256
//!
//! The page hands the image over and gets the cut-out back as a PNG in
//! base64 (see handoff.rs)

use serde::Serialize;
use tauri::{AppHandle, Manager, Webview};

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

/// Whether this device can run BiRefNet. ONNX Runtime needs macOS 14, as
/// Vision does
pub fn model_supported() -> bool {
    cfg!(birefnet) && available()
}

enum Failure {
    /// Nothing to keep was found
    NoSubject,
    /// The model wasn't downloaded
    #[cfg_attr(not(birefnet), allow(dead_code))]
    Cancelled,
    Failed,
}

/// The image in `bytes` with its background made transparent, as a PNG
/// in base64, for `job` in tab `tab`, or why there's none: "subject" when
/// there's nothing to keep, "cancelled" when the model wasn't downloaded
/// and "failed" otherwise
pub fn remove(app: &AppHandle, tab: u32, job: u64, bytes: &[u8]) -> Result<String, &'static str> {
    match cut_out(app, tab, job, bytes) {
        Ok(png) => Ok(png),
        Err(Failure::NoSubject) => Err("subject"),
        Err(Failure::Cancelled) => Err("cancelled"),
        Err(Failure::Failed) => {
            log::warn!("Couldn't remove the background of an image");
            Err("failed")
        }
    }
}

fn cut_out(app: &AppHandle, tab: u32, job: u64, bytes: &[u8]) -> Result<String, Failure> {
    #[cfg(target_os = "macos")]
    if !(cfg!(birefnet) && crate::settings::current(app).use_background_model) {
        return vision::remove(bytes);
    }
    #[cfg(birefnet)]
    return birefnet::remove(app, tab, job, bytes);
    #[cfg(not(birefnet))]
    {
        let _ = (app, tab, job);
        Err(Failure::Failed)
    }
}

/// The model as Settings shows it
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelState {
    /// This device can run it
    supported: bool,
    installed: bool,
    /// On disk once installed, otherwise to download
    bytes: u64,
    /// Percent downloaded, while it downloads
    downloading: Option<u8>,
}

fn model_state(app: &AppHandle) -> ModelState {
    #[cfg(birefnet)]
    if model_supported() {
        return birefnet::state(app);
    }
    let _ = app;
    ModelState::default()
}

/// Shows the model's state in the settings window, if open
#[cfg_attr(not(birefnet), allow(dead_code))]
fn show_model_state(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(crate::settings::WINDOW) {
        let state = serde_json::to_string(&model_state(app)).unwrap();
        let _ = window.eval(format!("window.applyBackgroundModel?.({state})"));
    }
}

#[tauri::command]
pub async fn get_background_model(app: AppHandle, webview: Webview) -> Option<ModelState> {
    crate::settings::from_settings_window(&webview).then(|| model_state(&app))
}

/// Downloads the model from Settings
#[tauri::command]
pub async fn install_background_model(app: AppHandle, webview: Webview) {
    if !crate::settings::from_settings_window(&webview) || !model_supported() {
        return;
    }
    #[cfg(birefnet)]
    birefnet::install(&app);
    #[cfg(not(birefnet))]
    let _ = app;
}

/// Stops the model's download
#[tauri::command]
pub async fn cancel_background_model(webview: Webview) {
    if crate::settings::from_settings_window(&webview) {
        #[cfg(birefnet)]
        birefnet::cancel();
    }
}

/// Removes the model from Settings
#[tauri::command]
pub async fn uninstall_background_model(app: AppHandle, webview: Webview) {
    if !crate::settings::from_settings_window(&webview) {
        return;
    }
    #[cfg(birefnet)]
    birefnet::uninstall(&app);
    #[cfg(not(birefnet))]
    let _ = app;
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
#[cfg(birefnet)]
mod birefnet {
    use std::fs::File;
    use std::io::{Cursor, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Mutex, MutexGuard, OnceLock};

    use base64::Engine;
    use image::imageops::{self, FilterType};
    use image::{DynamicImage, GrayImage, ImageDecoder, ImageEncoder, ImageReader, RgbaImage};
    use ort::environment::Environment;
    use ort::session::builder::AutoDevicePolicy;
    use ort::session::Session;
    use ort::value::TensorRef;
    use sha2::{Digest, Sha256};
    use tauri::{AppHandle, Manager};

    use super::{Failure, ModelState};

    /// A file the model needs, downloaded into the models folder as `file`,
    /// or as `entry` taken out of a .tgz. `size` and `sha256` are the
    /// download's
    struct Download {
        url: &'static str,
        sha256: &'static str,
        size: u64,
        file: &'static str,
        entry: Option<&'static str>,
    }

    /// Pinned to one revision
    const MODEL: Download = Download {
        url: "https://huggingface.co/onnx-community/BiRefNet_lite-ONNX/resolve/de15b22ba131738a16dff04aab8bdf8dc32e3ac1/onnx/model.onnx",
        sha256: "5600024376f572a557870a5eb0afb1e5961636bef4e1e22132025467d0f03333",
        size: 224_005_088,
        file: "birefnet-lite.onnx",
        entry: None,
    };

    /// Microsoft's build, which needs macOS 14
    #[cfg(target_os = "macos")]
    const RUNTIME: Download = Download {
        url: "https://github.com/microsoft/onnxruntime/releases/download/v1.31.1/onnxruntime-osx-arm64-1.31.1.tgz",
        sha256: "2e1344a1926ee50eb25b38153cb6992c3567515aef0e677a1a520a1798896d6f",
        size: 36_936_435,
        file: "libonnxruntime.dylib",
        entry: Some("onnxruntime-osx-arm64-1.31.1/lib/libonnxruntime.1.31.1.dylib"),
    };

    #[cfg(windows)]
    const DOWNLOADS: &[&Download] = &[&MODEL];
    #[cfg(target_os = "macos")]
    const DOWNLOADS: &[&Download] = &[&RUNTIME, &MODEL];

    const SIDE: u32 = 1024;
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];

    /// Held while the model is looked for, downloaded, used or removed, so
    /// two images wait for one download and it isn't removed while in use
    static LOCK: Mutex<()> = Mutex::new(());
    /// Percent downloaded, while it downloads
    static DOWNLOADING: Mutex<Option<u8>> = Mutex::new(None);
    /// Settings' Cancel
    static CANCEL: AtomicBool = AtomicBool::new(false);
    /// Jobs waiting on the model, told how far its download has got: tab
    /// and job
    static WATCHERS: Mutex<Vec<(u32, u64)>> = Mutex::new(Vec::new());

    fn lock() -> MutexGuard<'static, ()> {
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn failed(error: impl std::fmt::Display) -> Failure {
        log::warn!("Remove background: {error}");
        Failure::Failed
    }

    fn folder(app: &AppHandle) -> Result<PathBuf, String> {
        Ok(app.path().app_local_data_dir().map_err(|error| error.to_string())?.join("models"))
    }

    fn installed(folder: &Path) -> bool {
        DOWNLOADS.iter().all(|download| folder.join(download.file).is_file())
    }

    pub fn state(app: &AppHandle) -> ModelState {
        let downloading = *DOWNLOADING.lock().unwrap();
        let Ok(folder) = folder(app) else {
            return ModelState { supported: true, downloading, ..Default::default() };
        };
        let installed = installed(&folder);
        let bytes = if installed {
            DOWNLOADS
                .iter()
                .filter_map(|download| std::fs::metadata(folder.join(download.file)).ok())
                .map(|metadata| metadata.len())
                .sum()
        } else {
            DOWNLOADS.iter().map(|download| download.size).sum()
        };
        ModelState { supported: true, installed, bytes, downloading }
    }

    /// While a job waits on the model, it's told how far its download has got
    struct Watching(u32, u64);

    impl Watching {
        fn new(tab: u32, job: u64) -> Self {
            WATCHERS.lock().unwrap().push((tab, job));
            Self(tab, job)
        }
    }

    impl Drop for Watching {
        fn drop(&mut self) {
            WATCHERS.lock().unwrap().retain(|&watcher| watcher != (self.0, self.1));
        }
    }

    fn tell_watchers(app: &AppHandle, text: &str) {
        let watchers = WATCHERS.lock().unwrap().clone();
        for (tab, job) in watchers {
            crate::handoff::progress(app, tab, job, text);
        }
    }

    pub fn remove(app: &AppHandle, tab: u32, job: u64, bytes: &[u8]) -> Result<String, Failure> {
        let _watching = Watching::new(tab, job);
        let _held = lock();
        let folder = folder(app).map_err(failed)?;
        if !installed(&folder) {
            if !ask(app) {
                return Err(Failure::Cancelled);
            }
            match download(app, &folder) {
                Ok(()) => {}
                Err(None) => return Err(Failure::Cancelled),
                Err(Some(error)) => {
                    download_failed(app, &error);
                    return Err(failed(error));
                }
            }
        }
        cut_out(&folder, bytes)
    }

    /// Downloads the model, unless it's there or on its way
    pub fn install(app: &AppHandle) {
        let app = app.clone();
        std::thread::spawn(move || {
            let Ok(_held) = LOCK.try_lock() else {
                return;
            };
            let result = folder(&app).map_err(Some).and_then(|folder| {
                if installed(&folder) {
                    return Ok(());
                }
                download(&app, &folder)
            });
            if let Err(Some(error)) = result {
                log::warn!("Remove background: {error}");
                download_failed(&app, &error);
            }
        });
    }

    pub fn cancel() {
        CANCEL.store(true, Ordering::SeqCst);
    }

    /// Removes the model, once it's not in use
    pub fn uninstall(app: &AppHandle) {
        let app = app.clone();
        std::thread::spawn(move || {
            let _held = lock();
            if let Ok(folder) = folder(&app) {
                for download in DOWNLOADS {
                    let path = folder.join(download.file);
                    if let Err(error) = std::fs::remove_file(&path) {
                        if error.kind() != std::io::ErrorKind::NotFound {
                            log::warn!("Couldn't remove {}: {error}", path.display());
                        }
                    }
                }
            }
            super::show_model_state(&app);
        });
    }

    fn download_failed(app: &AppHandle, error: &str) {
        crate::prompt::show(
            app,
            "Download Failed",
            format!("Couldn't download the background model: {error}"),
            &["OK"],
            |_| {},
        );
    }

    /// Whether the person agrees to download the model
    fn ask(app: &AppHandle) -> bool {
        #[cfg(windows)]
        let message = "Removing backgrounds on this PC needs BiRefNet, a 224 MB model that Ligma downloads once from Hugging Face. You can remove it in Settings.";
        #[cfg(target_os = "macos")]
        let message = "Removing backgrounds with BiRefNet needs the 224 MB model and ONNX Runtime to run it, 261 MB in all, that Ligma downloads once from Hugging Face and GitHub. You can remove them in Settings.";
        let (send, receive) = mpsc::channel();
        crate::prompt::show(app, "Download Background Model", message, &["Cancel", "Download"], move |choice| {
            let _ = send.send(choice == Some(1));
        });
        receive.recv().unwrap_or(false)
    }

    /// Downloads what's missing into `folder`, telling Settings and the
    /// jobs waiting how far it's got. `Err(None)` when cancelled
    fn download(app: &AppHandle, folder: &Path) -> Result<(), Option<String>> {
        CANCEL.store(false, Ordering::SeqCst);
        let missing: Vec<&Download> = DOWNLOADS.iter().copied().filter(|download| !folder.join(download.file).is_file()).collect();
        let total = missing.iter().map(|download| download.size).sum::<u64>().max(1);
        let mut before = 0;
        let mut told = None;
        let mut report = |got: u64| {
            let percent = (got * 100 / total).min(100) as u8;
            if *DOWNLOADING.lock().unwrap() != Some(percent) {
                *DOWNLOADING.lock().unwrap() = Some(percent);
                super::show_model_state(app);
            }
            // In steps of 5%, so Figma's toast isn't redrawn too often
            if told != Some(percent / 5) {
                told = Some(percent / 5);
                tell_watchers(app, &format!("Downloading the background model… {}%", percent / 5 * 5));
            }
        };
        report(0);
        let mut result = std::fs::create_dir_all(folder).map_err(|error| Some(error.to_string()));
        for download in missing {
            if result.is_err() {
                break;
            }
            result = fetch(download, folder, &mut |got| report(before + got));
            before += download.size;
        }
        *DOWNLOADING.lock().unwrap() = None;
        super::show_model_state(app);
        if result.is_ok() {
            tell_watchers(app, "Removing background…");
        }
        result
    }

    /// Downloads `download` into `folder`, through a `.part` file that's
    /// only kept once its hash matches. `Err(None)` when cancelled
    fn fetch(download: &Download, folder: &Path, report: &mut impl FnMut(u64)) -> Result<(), Option<String>> {
        let path = folder.join(download.file);
        let part = folder.join(format!("{}.part", download.file));
        let result = tauri::async_runtime::block_on(async {
            let error = |error: &dyn std::fmt::Display| Some(error.to_string());
            let client = reqwest::Client::builder().tls_backend_native().build().map_err(|e| error(&e))?;
            let mut response = client
                .get(download.url)
                .send()
                .await
                .and_then(|response| response.error_for_status())
                .map_err(|e| error(&e))?;
            let mut file = File::create(&part).map_err(|e| error(&e))?;
            let mut hash = Sha256::new();
            let mut got = 0u64;
            while let Some(chunk) = response.chunk().await.map_err(|e| error(&e))? {
                if CANCEL.load(Ordering::SeqCst) {
                    return Err(None);
                }
                file.write_all(&chunk).map_err(|e| error(&e))?;
                hash.update(&chunk);
                got += chunk.len() as u64;
                report(got.min(download.size));
            }
            file.sync_all().map_err(|e| error(&e))?;
            if format!("{:x}", hash.finalize()) != download.sha256 {
                return Err(Some("the file didn't match".to_owned()));
            }
            Ok(())
        });
        let result = result.and_then(|()| match download.entry {
            Some(entry) => extract(&part, entry, &path).map_err(Some),
            None => std::fs::rename(&part, &path).map_err(|error| Some(error.to_string())),
        });
        let _ = std::fs::remove_file(&part);
        result
    }

    /// Takes `entry` out of the .tgz `archive`, into `path`
    #[cfg(target_os = "macos")]
    fn extract(archive: &Path, entry: &str, path: &Path) -> Result<(), String> {
        let temp = path.with_extension("tmp");
        let result = (|| {
            let archive = File::open(archive).map_err(|error| error.to_string())?;
            let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(archive));
            for file in archive.entries().map_err(|error| error.to_string())? {
                let mut file = file.map_err(|error| error.to_string())?;
                if file.path().map_err(|error| error.to_string())?.as_ref() == Path::new(entry) {
                    file.unpack(&temp).map_err(|error| error.to_string())?;
                    return std::fs::rename(&temp, path).map_err(|error| error.to_string());
                }
            }
            Err("ONNX Runtime wasn't in the download".to_owned())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }

    #[cfg(windows)]
    fn extract(_: &Path, _: &str, _: &Path) -> Result<(), String> {
        Err("nothing to extract on Windows".to_owned())
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

    /// ONNX Runtime, loaded from `folder` on macOS. Once loaded it stays,
    /// even if the files are removed
    fn environment(folder: &Path) -> Result<&'static Environment, Failure> {
        static ENVIRONMENT: OnceLock<Environment> = OnceLock::new();
        if let Some(environment) = ENVIRONMENT.get() {
            return Ok(environment);
        }
        #[cfg(target_os = "macos")]
        let builder = ort::init_from(folder.join(RUNTIME.file)).map_err(failed)?;
        #[cfg(windows)]
        let builder = {
            let _ = folder;
            ort::init()
        };
        let environment = builder.with_name("ligma").build().map_err(failed)?;
        Ok(ENVIRONMENT.get_or_init(|| environment))
    }

    /// The model is loaded for each image and let go after, as it takes
    /// hundreds of MB
    fn cut_out(folder: &Path, bytes: &[u8]) -> Result<String, Failure> {
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

        let mut session = Session::builder(environment(folder)?)
            .map_err(failed)?
            .with_auto_device(AutoDevicePolicy::PreferCPU)
            .map_err(failed)?
            .commit_from_file(folder.join(MODEL.file))
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
