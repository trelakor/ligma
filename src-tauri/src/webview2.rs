//! WebView2 memory controls for hidden tabs, and a fix for Figma's plugin
//! downloads (Windows only).

use tauri::Webview;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2_19, ICoreWebView2_22, ICoreWebView2_3, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
};
use webview2_com::TrySuspendCompletedHandler;
use windows_core::{w, Interface};

/// Takes Tauri's request filters off a Figma tab, which never loads
/// Tauri's own `tauri:`, `ipc:` or `asset:` URLs: Figma pages get no IPC.
///
/// While a webview has any such filter, WebView2 follows a redirect from
/// the page's origin to another without an `Origin` header. Figma loads a
/// community plugin's code that way, from `/community/plugin/<id>/code`
/// to a signed copy on `s3-alpha-sig.figma.com`, which then answers
/// without `Access-Control-Allow-Origin`; the read fails and Figma says "An
/// error occurred while loading the plugin environment".
pub fn drop_request_filters(webview: &Webview) {
    let _ = webview.with_webview(|platform| unsafe {
        let Ok(core) = platform.controller().CoreWebView2().and_then(|core| core.cast::<ICoreWebView2_22>()) else {
            return;
        };
        // As wry adds them (`custom_protocol_workaround`). Missing ones
        // (no `asset:` here) just fail.
        for uri in [w!("http://tauri.*"), w!("http://ipc.*"), w!("http://asset.*")] {
            let _ = core.RemoveWebResourceRequestedFilterWithRequestSourceKinds(
                uri,
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
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
