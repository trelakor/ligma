//! Remove background on this device (Settings > Features > Images): the
//! image bar's Remove background in free projects, which Ligma puts in
//! place of Figma's (see ai_actions.rs). On macOS 14 and later, Apple's
//! Vision finds the image's subject, as Preview's Remove Background does.
//! On Windows it's BiRefNet (MIT), an ONNX model run on the CPU with ONNX
//! Runtime. Macs with Apple silicon can use BiRefNet instead of Vision. The
//! model is 224 MB, so it's downloaded when it's first needed (see
//! models.rs)
//!
//! The page hands the image over and gets the cut-out back as a PNG in
//! base64 (see handoff.rs)

use tauri::AppHandle;

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

/// BiRefNet-lite (see models.rs). It takes a 1024 × 1024 RGB image
/// normalized as for ImageNet and gives the foreground's logits at the same
/// size, which are scaled back to the image as its alpha
#[cfg(birefnet)]
mod birefnet {
    use std::io::Cursor;
    use std::path::Path;

    use base64::Engine;
    use image::imageops::{self, FilterType};
    use image::{DynamicImage, GrayImage, ImageDecoder, ImageEncoder, ImageReader, RgbaImage};
    use ort::session::builder::AutoDevicePolicy;
    use ort::session::Session;
    use ort::value::TensorRef;
    use tauri::AppHandle;

    use super::Failure;
    use crate::models::{Missing, BACKGROUND, BIREFNET};

    const SIDE: u32 = 1024;
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];

    fn failed(error: impl std::fmt::Display) -> Failure {
        log::warn!("Remove background: {error}");
        Failure::Failed
    }

    pub fn remove(app: &AppHandle, tab: u32, job: u64, bytes: &[u8]) -> Result<String, Failure> {
        let (_held, folder) = BACKGROUND.ensure(app, tab, job).map_err(|missing| match missing {
            Missing::Cancelled => Failure::Cancelled,
            Missing::Failed => Failure::Failed,
        })?;
        cut_out(&folder, bytes)
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

        let mut session = Session::builder(crate::models::environment(folder).map_err(failed)?)
            .map_err(failed)?
            .with_auto_device(AutoDevicePolicy::PreferCPU)
            .map_err(failed)?
            .commit_from_file(folder.join(BIREFNET.file))
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
