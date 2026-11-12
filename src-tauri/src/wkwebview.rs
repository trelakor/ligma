//! WKWebView controls for hidden tabs (macOS only)
//!
//! Freezing uses `_suspendPage:`/`_resumePage:`, WebKit SPI (macOS 12+) that
//! moves the page into the back/forward cache: scripts and timers stop. It
//! saves CPU, not memory, which stays allocated so the page resumes
//! instantly. Both are looked up at runtime and skipped if missing
//!
//! WebKit throws an Objective-C exception, which aborts Ligma, if a frozen
//! page is asked to run JavaScript. Callers must not `eval` in a tab while
//! it's frozen or still resuming (`Tab::suspended`). The calls here catch
//! exceptions themselves
//!
//! On macOS 27, WebKit's Screen Time setup races with closing a webview: its
//! KVO observer is removed on the main thread while Screen Time updates it
//! on a private queue, and the exception aborts Ligma (WebKit bug, see
//! https://developer.apple.com/forums/thread/809860). `disable_screen_time`
//! turns that setup off for the whole process, so Screen Time can't block
//! Figma pages inside Ligma, though it still counts the time spent in it
//!
//! wry keeps Cmd shortcuts from child webviews, the tabs, so the menu bar
//! gets them. For Cmd+Z that means Edit > Undo runs WebKit's text undo and
//! Figma never sees the key. `pass_undo_to_pages` gives undo and redo back
//! to WebKit, which lets the page handle them first, as Safari does, and
//! passes them on to the menu bar if it doesn't
//!
//! `clip` shows only parts of a tab, for split view (see split.rs): a layer
//! mask hides the rest, and a patched `hitTest:` lets clicks there through
//! to the tab below

use std::ffi::{c_char, c_void, CStr};
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, Once, OnceLock};

use block2::RcBlock;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use objc2::{class, msg_send, sel};
use tauri::Webview;

/// Replaces `-[WKWebView _installScreenTimeWebpageControllerIfNeeded]`
/// with a no-op. Call before the first webview is created. Skipped if
/// WebKit doesn't have it
pub fn disable_screen_time() {
    unsafe extern "C-unwind" fn install_nothing(_: *mut AnyObject, _: Sel) {}

    let method = AnyClass::get(c"WKWebView")
        .and_then(|class| class.instance_method(sel!(_installScreenTimeWebpageControllerIfNeeded)));
    if let Some(method) = method {
        let imp: unsafe extern "C-unwind" fn(*mut AnyObject, Sel) = install_nothing;
        unsafe { method.set_implementation(std::mem::transmute::<_, Imp>(imp)) };
    }
}

/// IMPs of `performKeyEquivalent:`: wry's override and WebKit's own
static WRY_KEY_EQUIVALENT: OnceLock<Imp> = OnceLock::new();
static WEBKIT_KEY_EQUIVALENT: OnceLock<Imp> = OnceLock::new();

type KeyEquivalent = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> Bool;

/// Cmd+Z or Shift+Cmd+Z, by the character typed so other keyboard layouts
/// match the menu bar
unsafe fn is_undo_or_redo(event: *mut AnyObject) -> bool {
    const SHIFT: usize = 1 << 17;
    const CONTROL: usize = 1 << 18;
    const OPTION: usize = 1 << 19;
    const COMMAND: usize = 1 << 20;
    let flags: usize = msg_send![event, modifierFlags];
    if flags & (COMMAND | OPTION | CONTROL) != COMMAND {
        return false;
    }
    let chars: *mut AnyObject = msg_send![event, charactersIgnoringModifiers];
    if chars.is_null() {
        return false;
    }
    let utf8: *const c_char = msg_send![chars, UTF8String];
    if utf8.is_null() {
        return false;
    }
    match CStr::from_ptr(utf8).to_bytes() {
        b"z" => true,
        b"Z" => flags & SHIFT != 0,
        _ => false,
    }
}

unsafe extern "C-unwind" fn perform_key_equivalent(view: *mut AnyObject, selector: Sel, event: *mut AnyObject) -> Bool {
    let imp = if !event.is_null() && is_undo_or_redo(event) {
        WEBKIT_KEY_EQUIVALENT.get()
    } else {
        WRY_KEY_EQUIVALENT.get()
    };
    match imp {
        Some(imp) => std::mem::transmute::<Imp, KeyEquivalent>(*imp)(view, selector, event),
        None => Bool::NO,
    }
}

/// Lets Figma handle Cmd+Z and Shift+Cmd+Z in tabs. Patches the class of
/// `webview`, so once is enough. Skipped if wry no longer overrides
/// `performKeyEquivalent:`
pub fn pass_undo_to_pages(webview: &Webview) {
    static PATCHED: Once = Once::new();
    let _ = webview.with_webview(|platform| unsafe {
        let view = platform.inner() as *mut AnyObject;
        if view.is_null() {
            return;
        }
        PATCHED.call_once(|| {
            let selector = sel!(performKeyEquivalent:);
            let (Some(wry), Some(webkit)) = (
                (*view).class().instance_method(selector),
                AnyClass::get(c"WKWebView").and_then(|class| class.instance_method(selector)),
            ) else {
                return;
            };
            if wry.implementation() as usize == webkit.implementation() as usize {
                return;
            }
            let _ = WEBKIT_KEY_EQUIVALENT.set(webkit.implementation());
            let _ = WRY_KEY_EQUIVALENT.set(wry.implementation());
            let imp: KeyEquivalent = perform_key_equivalent;
            wry.set_implementation(std::mem::transmute::<KeyEquivalent, Imp>(imp));
        });
    });
}

/// Runs WebKit calls so that an Objective-C exception from them is logged
/// rather than aborting Ligma (Rust can't unwind through one). False if one
/// was thrown
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

/// Wraps `done` in a block WebKit calls once with a BOOL
fn bool_block(done: impl FnOnce(bool) + Send + 'static) -> RcBlock<dyn Fn(Bool)> {
    let done = Mutex::new(Some(done));
    RcBlock::new(move |result: Bool| {
        if let Some(done) = done.lock().unwrap().take() {
            done(result.as_bool());
        }
    })
}

/// Freezes a hidden tab. `done` gets whether it worked, on the main thread.
/// Not called if the SPI is missing
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
/// again, and only then may it `eval`. Not called if WebKit threw
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
/// couldn't take one. `done` runs on the main thread
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

/// NSImage to a JPEG `data:` URL
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

#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: f64,
    y: f64,
}

unsafe impl objc2::Encode for Point {
    const ENCODING: objc2::Encoding = objc2::Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Size {
    width: f64,
    height: f64,
}

unsafe impl objc2::Encode for Size {
    const ENCODING: objc2::Encoding = objc2::Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    origin: Point,
    size: Size,
}

unsafe impl objc2::Encode for Rect {
    const ENCODING: objc2::Encoding = objc2::Encoding::Struct("CGRect", &[Point::ENCODING, Size::ENCODING]);
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPathCreateMutable() -> *mut c_void;
    fn CGPathAddRect(path: *mut c_void, transform: *const c_void, rect: Rect);
    fn CGPathRelease(path: *const c_void);
    fn CGColorCreateSRGB(red: f64, green: f64, blue: f64, alpha: f64) -> *mut c_void;
    fn CGColorRelease(color: *const c_void);
}

/// Paints `color` behind tab `webview`'s page, which is all it shows until
/// the page draws. wry only stops WebKit drawing white there for a
/// background color, which leaves the window's showing through
pub fn set_background(webview: &Webview, color: tauri::window::Color) {
    let _ = webview.with_webview(move |platform| unsafe {
        let view = platform.inner() as *mut AnyObject;
        if view.is_null() {
            return;
        }
        catching("tab background", || {
            let _: () = msg_send![view, setWantsLayer: true];
            let layer: *mut AnyObject = msg_send![view, layer];
            if layer.is_null() {
                return;
            }
            let channel = |value: u8| f64::from(value) / 255.0;
            let cg = CGColorCreateSRGB(channel(color.0), channel(color.1), channel(color.2), 1.0);
            let _: () = msg_send![layer, setBackgroundColor: cg];
            CGColorRelease(cg);
        });
    });
}

extern "C" {
    fn class_addMethod(class: *const AnyClass, name: Sel, imp: Imp, types: *const c_char) -> Bool;
}

/// Each clipped tab's WKWebView and the parts it shows, `[left, top,
/// right, bottom]` in points from its top left
static CLIPS: Mutex<Vec<(usize, Vec<[f64; 4]>)>> = Mutex::new(Vec::new());

/// `hitTest:` of the tabs' class before `patch_hit_test`
static VIEW_HIT_TEST: OnceLock<Imp> = OnceLock::new();

type HitTest = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, Point) -> *mut AnyObject;

/// A clipped tab takes no clicks outside the parts it shows
unsafe extern "C-unwind" fn hit_test(view: *mut AnyObject, selector: Sel, point: Point) -> *mut AnyObject {
    let shown = CLIPS.lock().unwrap().iter().find(|(clipped, _)| *clipped == view as usize).map(|(_, rects)| rects.clone());
    if let Some(rects) = shown {
        // `point` is in the superview's coordinates
        let superview: *mut AnyObject = msg_send![view, superview];
        let local: Point = msg_send![view, convertPoint: point, fromView: superview];
        let flipped: Bool = msg_send![view, isFlipped];
        let bounds: Rect = msg_send![view, bounds];
        let y = if flipped.as_bool() { local.y } else { bounds.size.height - local.y };
        if !rects.iter().any(|r| local.x >= r[0] && local.x < r[2] && y >= r[1] && y < r[3]) {
            return std::ptr::null_mut();
        }
    }
    match VIEW_HIT_TEST.get() {
        Some(imp) => std::mem::transmute::<Imp, HitTest>(*imp)(view, selector, point),
        None => std::ptr::null_mut(),
    }
}

/// Gives the class of tab `view` its own `hitTest:`, once
unsafe fn patch_hit_test(view: *mut AnyObject) {
    static PATCHED: Once = Once::new();
    PATCHED.call_once(|| {
        let class = (*view).class();
        let selector = sel!(hitTest:);
        let Some(inherited) = class.instance_method(selector) else {
            return;
        };
        let _ = VIEW_HIT_TEST.set(inherited.implementation());
        let imp: HitTest = hit_test;
        let imp = std::mem::transmute::<HitTest, Imp>(imp);
        // Added to the tabs' class alone, unless it has its own already
        if !class_addMethod(class, selector, imp, c"@@:{CGPoint=dd}".as_ptr()).as_bool() {
            inherited.set_implementation(imp);
        }
    });
}

/// Shows only `rects` of tab `webview`, `[left, top, right, bottom]` in
/// points from its top left, or all of it with None (see split.rs)
pub fn clip(webview: &Webview, rects: Option<Vec<[f64; 4]>>) {
    let _ = webview.with_webview(move |platform| unsafe {
        let view = platform.inner() as *mut AnyObject;
        if view.is_null() {
            return;
        }
        patch_hit_test(view);
        {
            let mut clips = CLIPS.lock().unwrap();
            clips.retain(|(clipped, _)| *clipped != view as usize);
            if let Some(rects) = &rects {
                clips.push((view as usize, rects.clone()));
            }
        }
        catching("split view mask", || {
            let _: () = msg_send![view, setWantsLayer: true];
            let layer: *mut AnyObject = msg_send![view, layer];
            if layer.is_null() {
                return;
            }
            let Some(rects) = &rects else {
                let none: *mut AnyObject = std::ptr::null_mut();
                let _: () = msg_send![layer, setMask: none];
                return;
            };
            let bounds: Rect = msg_send![view, bounds];
            let flipped: Bool = msg_send![layer, contentsAreFlipped];
            let path = CGPathCreateMutable();
            for r in rects {
                let y = if flipped.as_bool() { r[1] } else { bounds.size.height - r[3] };
                let rect = Rect { origin: Point { x: r[0], y }, size: Size { width: r[2] - r[0], height: r[3] - r[1] } };
                CGPathAddRect(path, std::ptr::null(), rect);
            }
            let mask: *mut AnyObject = msg_send![class!(CAShapeLayer), layer];
            let _: () = msg_send![mask, setFrame: bounds];
            let _: () = msg_send![mask, setPath: path];
            CGPathRelease(path);
            let _: () = msg_send![layer, setMask: mask];
        });
    });
}
