//! Remove background on this device (Settings > Features > Images): the
//! image bar's Remove background in free projects, which Ligma puts in
//! place of Figma's (see ai_actions.rs). On macOS 14 and later, Apple's
//! Vision finds the image's subject, as Preview's Remove Background does;
//! elsewhere there's no button yet
//!
//! Figma tabs get no IPC, so the page hands the image over as a download
//! named `ligma-background-<job>`, which `on_requested` saves to a
//! temporary file instead of asking where. Once it's saved, the cut-out
//! goes back to the page as a PNG, through
//! `window.__ligmaRemoveBackground.done(job, base64)`, or `failed(job,
//! reason)`

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

#[cfg(not(target_os = "macos"))]
pub fn available() -> bool {
    false
}

/// An image a tab hands over: saved to a temporary file. False for other
/// downloads
pub fn on_requested(webview: &Webview, url: &Url, destination: &mut PathBuf) -> bool {
    if url.scheme() != "blob" {
        return false;
    }
    let Some(job) = destination
        .file_name()
        .and_then(|name| name.to_str()?.strip_prefix(PREFIX)?.parse::<u64>().ok())
    else {
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
        let result = if success { remove(&temp) } else { Err(Failure::Failed) };
        let _ = std::fs::remove_file(&temp);
        if let Err(Failure::Failed) = result {
            log::warn!("Couldn't remove the background of an image");
        }
        reply(&app, tab, job, result);
    });
    true
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
enum Failure {
    /// Vision found nothing to keep
    NoSubject,
    Failed,
}

/// The image in `path` with its background made transparent, as a PNG
/// in base64
fn remove(path: &Path) -> Result<String, Failure> {
    if std::fs::metadata(path).map_err(|_| Failure::Failed)?.len() > MAX_BYTES {
        return Err(Failure::Failed);
    }
    let bytes = std::fs::read(path).map_err(|_| Failure::Failed)?;
    #[cfg(target_os = "macos")]
    return vision::remove(&bytes);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = bytes;
        Err(Failure::Failed)
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
