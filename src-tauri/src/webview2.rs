//! WebView2 memory controls for hidden tabs, and a fix for Figma's plugin
//! downloads (Windows only).

use tauri::Webview;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2_19, ICoreWebView2_3, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
};
use webview2_com::{TrySuspendCompletedHandler, WebResourceRequestedEventHandler};
use windows_core::{w, Interface, BOOL};

/// Lets Figma download a community plugin's code. Figma asks for
/// `/community/plugin/<id>/code`, which redirects to a signed copy on
/// `s3-alpha-sig.figma.com`, and WebView2 follows that redirect without an
/// `Origin` header. The server then answers without
/// `Access-Control-Allow-Origin`, the read fails, and Figma says "An error
/// occurred while loading the plugin environment". So give those requests
/// the origin the page would have sent.
pub fn send_origin_to_plugin_downloads(webview: &Webview) {
    let _ = webview.with_webview(|platform| unsafe {
        let Ok(core) = platform.controller().CoreWebView2() else {
            return;
        };
        let handler = WebResourceRequestedEventHandler::create(Box::new(|_, args| {
            let Some(args) = args else {
                return Ok(());
            };
            let headers = args.Request()?.Headers()?;
            let mut has_origin = BOOL::default();
            headers.Contains(w!("Origin"), &mut has_origin)?;
            if !has_origin.as_bool() {
                headers.SetHeader(w!("Origin"), w!("https://www.figma.com"))?;
            }
            Ok(())
        }));
        let mut token = 0;
        if core.add_WebResourceRequested(&handler, &mut token).is_ok() {
            let _ = core.AddWebResourceRequestedFilter(
                w!("https://s3-alpha-sig.figma.com/plugins/*"),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
            );
        }
    });
}

/// Asks a hidden tab to give back memory: WebView2 drops caches and trims
/// what it can without stopping the page. Runtimes older than 1.0.1774
/// don't have the API and are left alone.
pub fn on_hidden(webview: &Webview) {
    set_memory_target(webview, true);
}

/// Undoes `on_hidden` and `try_suspend` for a tab that's shown again.
pub fn on_shown(webview: &Webview) {
    set_memory_target(webview, false);
    let _ = webview.with_webview(|platform| unsafe {
        if let Ok(core) = platform.controller().CoreWebView2().and_then(|core| core.cast::<ICoreWebView2_3>()) {
            let _ = core.Resume();
        }
    });
}

fn set_memory_target(webview: &Webview, low: bool) {
    let level = if low {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
    } else {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
    };
    let _ = webview.with_webview(move |platform| unsafe {
        if let Ok(core) = platform.controller().CoreWebView2().and_then(|core| core.cast::<ICoreWebView2_19>()) {
            let _ = core.SetMemoryUsageTargetLevel(level);
        }
    });
}

/// Freezes a hidden tab: scripts and timers stop and WebView2 trims its
/// memory further. Only works while the webview is hidden; showing it
/// resumes the page. `done` gets whether it worked, on the main thread.
pub fn try_suspend(webview: &Webview, done: impl FnOnce(bool) + Send + 'static) {
    let _ = webview.with_webview(move |platform| unsafe {
        let Ok(core) = platform.controller().CoreWebView2().and_then(|core| core.cast::<ICoreWebView2_3>()) else {
            return;
        };
        let handler = TrySuspendCompletedHandler::create(Box::new(move |result, suspended| {
            done(result.is_ok() && suspended);
            Ok(())
        }));
        let _ = core.TrySuspend(&handler);
    });
}
