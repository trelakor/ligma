//! WKWebView controls for hidden tabs (macOS only).
//!
//! Freezing uses `_suspendPage:`/`_resumePage:`, WebKit SPI (macOS 12+) that
//! moves the page into the back/forward cache: scripts and timers stop. It
//! saves CPU, not memory, which stays allocated so the page resumes
//! instantly. Both are looked up at runtime and skipped if missing.
//!
//! WebKit throws an Objective-C exception, which aborts Ligma, if a frozen
//! page is asked to run JavaScript. Callers must not `eval` in a tab while
//! it's frozen or still resuming (`Tab::suspended`). The calls here catch
//! exceptions themselves.
//!
//! On macOS 27, WebKit's Screen Time setup races with closing a webview: its
//! KVO observer is removed on the main thread while Screen Time updates it
//! on a private queue, and the exception aborts Ligma (WebKit bug, see
//! https://developer.apple.com/forums/thread/809860). `disable_screen_time`
//! turns that setup off for the whole process, so Screen Time can't block
//! Figma pages inside Ligma, though it still counts the time spent in it.

use std::ffi::{c_char, c_void, CStr};
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{class, msg_send, sel};
use tauri::Webview;

/// Replaces `-[WKWebView _installScreenTimeWebpageControllerIfNeeded]`
/// with a no-op. Call before the first webview is created. Skipped if
/// WebKit doesn't have it.
pub fn disable_screen_time() {
    unsafe extern "C-unwind" fn install_nothing(_: *mut AnyObject, _: Sel) {}

    let method = AnyClass::get(c"WKWebView")
        .and_then(|class| class.instance_method(sel!(_installScreenTimeWebpageControllerIfNeeded)));
    if let Some(method) = method {
        let imp: unsafe extern "C-unwind" fn(*mut AnyObject, Sel) = install_nothing;
        unsafe { method.set_implementation(std::mem::transmute::<_, Imp>(imp)) };
    }
}

/// Runs WebKit calls so that an Objective-C exception from them is logged
/// rather than aborting Ligma (Rust can't unwind through one). False if one
/// was thrown.
unsafe fn catching(what: &str, f: impl FnOnce()) -> bool {
    match objc2::exception::catch(AssertUnwindSafe(f)) {
        Ok(()) => true,
        Err(exception) => {
            eprintln!("ligma: {what} threw {exception:?}");
            false
        }
    }
}

unsafe fn responds(view: *mut AnyObject, selector: Sel) -> bool {
    !view.is_null() && msg_send![view, respondsToSelector: selector]
}

/// Wraps `done` in a block WebKit calls once with a BOOL.
fn bool_block(done: impl FnOnce(bool) + Send + 'static) -> RcBlock<dyn Fn(Bool)> {
    let done = Mutex::new(Some(done));
    RcBlock::new(move |result: Bool| {
        if let Some(done) = done.lock().unwrap().take() {
            done(result.as_bool());
        }
    })
}

/// Freezes a hidden tab. `done` gets whether it worked, on the main thread.
/// Not called if the SPI is missing.
pub fn try_suspend(webview: &Webview, done: impl FnOnce(bool) + Send + 'static) {
    let _ = webview.with_webview(move |platform| unsafe {
        let view = platform.inner() as *mut AnyObject;
        if responds(view, sel!(_suspendPage:)) {
            let block = bool_block(done);
            catching("_suspendPage:", || {
                let _: () = msg_send![view, _suspendPage: &*block];
            });
        }
    });
}

/// Undoes `try_suspend`. `done` runs on the main thread once the page runs
/// again, and only then may it `eval`. Not called if WebKit threw.
pub fn resume(webview: &Webview, done: impl FnOnce() + Send + 'static) {
    let _ = webview.with_webview(move |platform| unsafe {
        let view = platform.inner() as *mut AnyObject;
        if responds(view, sel!(_resumePage:)) {
            let block = bool_block(move |_| done());
            catching("_resumePage:", || {
                let _: () = msg_send![view, _resumePage: &*block];
            });
        } else {
            done();
        }
    });
}

/// Image of what the tab shows, as a JPEG `data:` URL, or None if WebKit
/// couldn't take one. `done` runs on the main thread.
pub fn snapshot(webview: &Webview, done: impl FnOnce(Option<String>) + Send + 'static) {
    let _ = webview.with_webview(move |platform| unsafe {
        let view = platform.inner() as *mut AnyObject;
        if view.is_null() {
            return;
        }
        let done = Arc::new(Mutex::new(Some(done)));
        let block_done = done.clone();
        let block = RcBlock::new(move |image: *mut AnyObject, _error: *mut AnyObject| {
            if let Some(done) = block_done.lock().unwrap().take() {
                let mut url = None;
                catching("snapshot encoding", || url = jpeg_data_url(image));
                done(url);
            }
        });
        let no_config: *mut AnyObject = std::ptr::null_mut();
        let taken = catching("takeSnapshotWithConfiguration:", || {
            let _: () = msg_send![view, takeSnapshotWithConfiguration: no_config, completionHandler: &*block];
        });
        if !taken {
            if let Some(done) = done.lock().unwrap().take() {
                done(None);
            }
        }
    });
}

/// NSImage to a JPEG `data:` URL.
unsafe fn jpeg_data_url(image: *mut AnyObject) -> Option<String> {
    if image.is_null() {
        return None;
    }
    let no_rect: *mut c_void = std::ptr::null_mut();
    let no_object: *mut AnyObject = std::ptr::null_mut();
    let cg_image: *mut c_void =
        msg_send![image, CGImageForProposedRect: no_rect, context: no_object, hints: no_object];
    if cg_image.is_null() {
        return None;
    }
    let rep: Allocated<AnyObject> = msg_send![class!(NSBitmapImageRep), alloc];
    let rep: Option<Retained<AnyObject>> = msg_send![rep, initWithCGImage: cg_image];
    let rep = rep?;
    let key: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: c"NSImageCompressionFactor".as_ptr()];
    let quality: *mut AnyObject = msg_send![class!(NSNumber), numberWithDouble: 0.6f64];
    let properties: *mut AnyObject = msg_send![class!(NSDictionary), dictionaryWithObject: quality, forKey: key];
    // NSBitmapImageFileTypeJPEG
    let data: *mut AnyObject = msg_send![&*rep, representationUsingType: 3usize, properties: properties];
    if data.is_null() {
        return None;
    }
    let base64: *mut AnyObject = msg_send![data, base64EncodedStringWithOptions: 0usize];
    let utf8: *const c_char = msg_send![base64, UTF8String];
    if utf8.is_null() {
        return None;
    }
    Some(format!("data:image/jpeg;base64,{}", CStr::from_ptr(utf8).to_str().ok()?))
}
