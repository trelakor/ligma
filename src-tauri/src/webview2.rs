//! WebView2 memory controls for hidden tabs, the tab shortcuts, and a fix
//! for Figma's plugin downloads (Windows only).

use tauri::{AppHandle, Manager, Webview};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2_19, ICoreWebView2_22, ICoreWebView2_3, COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN,
    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
    COREWEBVIEW2_PHYSICAL_KEY_STATUS, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
    COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL,
};
use webview2_com::{AcceleratorKeyPressedEventHandler, TrySuspendCompletedHandler};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VIRTUAL_KEY, VK_CONTROL, VK_MENU, VK_SHIFT, VK_T, VK_TAB, VK_W,
};
use windows_core::{w, Interface};

/// The tab shortcuts macOS has in its menu bar, which Windows doesn't have:
/// `Ctrl+T` new tab, `Ctrl+W` close tab (or the settings window, when that
/// has focus), `Ctrl+Tab` / `Ctrl+Shift+Tab` switch tabs. Added to every
/// webview that can have keyboard focus, before the page sees the keys;
/// figma.com leaves them to the browser anyway.
pub fn add_tab_shortcuts(webview: &Webview) {
    let app = webview.app_handle().clone();
    let _ = webview.with_webview(move |platform| unsafe {
        let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut kind = Default::default();
            let mut key = 0;
            let mut status = COREWEBVIEW2_PHYSICAL_KEY_STATUS::default();
            args.KeyEventKind(&mut kind)?;
            args.VirtualKey(&mut key)?;
            args.PhysicalKeyStatus(&mut status)?;
            let down = |key: VIRTUAL_KEY| GetKeyState(key.0 as i32) < 0;
            if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN || !down(VK_CONTROL) || down(VK_MENU) {
                return Ok(());
            }
            let shift = down(VK_SHIFT);
            let action: fn(&AppHandle) -> tauri::Result<()> = match VIRTUAL_KEY(key as u16) {
                VK_T if !shift => crate::tabs::new_tab,
                VK_W if !shift => |app| match crate::settings::close_if_focused(app) {
                    true => Ok(()),
                    false => crate::tabs::close_active(app),
                },
                VK_TAB if shift => |app| crate::tabs::cycle(app, -1),
                VK_TAB => |app| crate::tabs::cycle(app, 1),
                _ => return Ok(()),
            };
            args.SetHandled(true)?;
            // Held keys repeat, as menu shortcuts do on macOS, except
            // closing, which would take every tab with it.
            if status.WasKeyDown.as_bool() && key as u16 == VK_W.0 {
                return Ok(());
            }
            // Off the main thread: opening or waking a tab creates a
            // webview, which would block on itself here.
            let app = app.clone();
            std::thread::spawn(move || {
                let _ = action(&app);
            });
            Ok(())
        }));
        let mut token = 0;
        let _ = platform.controller().add_AcceleratorKeyPressed(&handler, &mut token);
    });
}

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
