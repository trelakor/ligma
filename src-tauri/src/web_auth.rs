//! Google sign-in on macOS through `ASWebAuthenticationSession`, Apple's
//! sheet for signing in on the web from an app (see auth.rs)
//!
//! The sheet shows Figma's grant page with Safari's cookies, so a user
//! signed in to Figma there only approves. Figma then sends the sheet to
//! `figma://app_auth/redeem`, which the session catches itself because
//! it's started with `figma` as its callback scheme: the link goes to Ligma
//! even with the Figma desktop app installed, and Ligma registers nothing

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use block2::RcBlock;
use objc2::runtime::{AnyClass, AnyObject, AnyProtocol, Bool, ClassBuilder, Sel};
use objc2::{class, msg_send, sel};
use tauri::{AppHandle, Url};

#[link(name = "AuthenticationServices", kind = "framework")]
extern "C" {}

/// The running session, released when the next one starts. Kept as an
/// address: AppKit objects aren't `Send`, but this is only touched on the
/// main thread
static SESSION: Mutex<usize> = Mutex::new(0);

/// Counts sessions, so a replaced one's completion (it's cancelled) is
/// ignored
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// `ASWebAuthenticationSessionErrorCodeCanceledLogin`: the user closed the
/// sheet
const CANCELED: isize = 1;

unsafe fn ns_string(text: &str) -> *mut AnyObject {
    let text = std::ffi::CString::new(text).unwrap_or_default();
    msg_send![class!(NSString), stringWithUTF8String: text.as_ptr()]
}

/// The session's presentation context provider: shows the sheet over
/// Ligma's main window
fn anchor_provider() -> *mut AnyObject {
    static PROVIDER: Mutex<usize> = Mutex::new(0);

    unsafe extern "C-unwind" fn anchor(_: *mut AnyObject, _: Sel, _: *mut AnyObject) -> *mut AnyObject {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let window: *mut AnyObject = msg_send![app, mainWindow];
        if window.is_null() {
            msg_send![app, keyWindow]
        } else {
            window
        }
    }

    let mut provider = PROVIDER.lock().unwrap();
    if *provider == 0 {
        let class = AnyClass::get(c"LigmaWebAuthAnchor").unwrap_or_else(|| {
            let mut builder = ClassBuilder::new(c"LigmaWebAuthAnchor", class!(NSObject)).unwrap();
            if let Some(protocol) = AnyProtocol::get(c"ASWebAuthenticationPresentationContextProviding") {
                builder.add_protocol(protocol);
            }
            unsafe {
                builder.add_method(
                    sel!(presentationAnchorForWebAuthenticationSession:),
                    anchor as unsafe extern "C-unwind" fn(_, _, _) -> _,
                );
            }
            builder.register()
        });
        // Kept for the life of Ligma: the session only holds it weakly
        let object: *mut AnyObject = unsafe { msg_send![class, new] };
        *provider = object as usize;
    }
    *provider as *mut AnyObject
}

/// Shows Figma's grant page `url` in a sign-in sheet. The `figma://` link
/// it ends on goes to `auth::on_figma_link`. Closing the sheet cancels the
/// sign-in. Runs on the main thread
pub fn start(app: &AppHandle, url: Url) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || unsafe {
        let app = handle;
        let callback_app = app.clone();
        let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        let completion = RcBlock::new(move |link: *mut AnyObject, error: *mut AnyObject| {
            if GENERATION.load(Ordering::SeqCst) != generation {
                return;
            }
            let link: Option<String> = (!link.is_null()).then(|| {
                let text: *mut AnyObject = msg_send![link, absoluteString];
                let text: *const std::ffi::c_char = msg_send![text, UTF8String];
                std::ffi::CStr::from_ptr(text).to_string_lossy().into_owned()
            });
            let code: isize = if error.is_null() { 0 } else { msg_send![error, code] };
            let app = callback_app.clone();
            // Off the main thread: it may open a tab
            std::thread::spawn(move || match link.as_deref().map(Url::parse) {
                Some(Ok(link)) => crate::auth::on_figma_link(&app, &link),
                _ => crate::auth::cancel(&app, (code != CANCELED).then_some(crate::auth::START_FAILED)),
            });
        });
        let ns_url: *mut AnyObject = msg_send![class!(NSURL), URLWithString: ns_string(url.as_str())];
        let session: *mut AnyObject = msg_send![class!(ASWebAuthenticationSession), alloc];
        let session: *mut AnyObject = msg_send![
            session,
            initWithURL: ns_url,
            callbackURLScheme: ns_string("figma"),
            completionHandler: &*completion
        ];
        if session.is_null() {
            crate::auth::cancel(&app, Some(crate::auth::START_FAILED));
            return;
        }
        let _: () = msg_send![session, setPresentationContextProvider: anchor_provider()];
        // Share Safari's cookies, so a signed-in user only approves
        let _: () = msg_send![session, setPrefersEphemeralWebBrowserSession: Bool::NO];
        let previous = std::mem::replace(&mut *SESSION.lock().unwrap(), session as usize);
        if previous != 0 {
            let previous = previous as *mut AnyObject;
            let _: () = msg_send![previous, cancel];
            let _: () = msg_send![previous, release];
        }
        let started: Bool = msg_send![session, start];
        if !started.as_bool() {
            crate::auth::cancel(&app, Some(crate::auth::START_FAILED));
        }
    });
}
