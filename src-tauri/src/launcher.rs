//! The launcher: with Ligma's home page on and `ligmaHomeNewTabSearch`, a
//! new tab starts as the page's search, floating over the current tab as
//! in Arc, and no tab opens until something is picked. A file opens in a
//! new tab, as does a page or project of Ligma's home page. Escape or a
//! click elsewhere hides it, and nothing opens
//!
//! It's a borderless window over the tab's content, made on first use and
//! hidden in between, so it opens at once after that. An image of the tab
//! fills it, blurred, under the search. It loads Ligma's home
//! page as Home does (see new_home.rs), marked `launcher` so the page shows
//! its search alone and asks Ligma, through the same signal links, to open
//! a page in a tab or to hide it

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, LogicalSize, Manager, PhysicalPosition, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

pub const WINDOW: &str = "launcher";

/// Bumped on each hide, so a show still waiting for its image when the
/// launcher is dismissed doesn't bring it back
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Shows the launcher over the focused window's current tab, with an image
/// of the tab behind it, blurred by the page
pub fn show(app: &AppHandle) -> tauri::Result<()> {
    let generation = GENERATION.load(Ordering::SeqCst);
    crate::tab_menu::hide(app);
    crate::preview::hide(app);
    let Some(owner) = crate::tabs::focused_window(app) else {
        return Ok(());
    };
    let window = match app.get_webview_window(WINDOW) {
        Some(window) => window,
        None => build(app, &owner)?,
    };
    let _ = window.set_zoom(crate::tabs::scale(app));
    // Over the tab's content, below the tab bar
    let factor = owner.scale_factor()?;
    let content = crate::tabs::content_size(app, &owner)?;
    let top = owner.inner_position()?.to_logical::<f64>(factor);
    let y = top.y + crate::tabs::bar_height(app);
    let _ = window.set_size(LogicalSize::new(content.width, content.height));
    let _ = window.set_position(PhysicalPosition::new((top.x * factor).round() as i32, (y * factor).round() as i32));
    #[cfg(windows)]
    crate::frame::match_theme(&window);
    // Shown once the image is in, or without one if it doesn't come soon
    let pending: Arc<Mutex<Option<WebviewWindow>>> = Arc::new(Mutex::new(Some(window)));
    let finish = {
        let (app, owner, pending) = (app.clone(), owner.clone(), pending.clone());
        move |image: Option<String>| {
            let Some(window) = pending.lock().unwrap().take() else {
                return;
            };
            if let Some(webview) = app.get_webview(WINDOW) {
                crate::appearance::push_to(&webview);
            }
            // The image goes in while it's hidden, unblurred, so it first
            // shows just as the tab under it does. A page still loading
            // opens its search itself
            let image = serde_json::to_string(&image).unwrap();
            let _ = window.eval(format!("window.__ligmaLaunch?.({image})"));
            let app = app.clone();
            let owner = owner.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(40));
                // On the main thread, in turn with `hide`'s, so a dismissal
                // can't land between the check and the show
                let main = app.clone();
                let _ = main.run_on_main_thread(move || {
                    if GENERATION.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    crate::tab_menu::set_owner(&window, &owner);
                    let _ = window.show();
                    let _ = window.set_focus();
                    // The page, not only its window, takes the keys
                    if let Some(webview) = app.get_webview(WINDOW) {
                        let _ = webview.set_focus();
                    }
                    // Then it blurs the tab and brings the search in
                    let _ = window.eval("window.__ligmaShown?.()");
                });
            });
        }
    };
    let fallback = finish.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        fallback(None);
    });
    crate::tabs::snapshot_active(app, move |image| finish(image));
    Ok(())
}

fn build(app: &AppHandle, owner: &tauri::Window) -> tauri::Result<WebviewWindow> {
    let (nav_app, popup_app) = (app.clone(), app.clone());
    #[allow(unused_mut)]
    let mut builder = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::External(crate::new_home::launcher_url()))
        .title("New Tab")
        .inner_size(880.0, 560.0)
        .visible(false)
        .decorations(false)
        // Square, flush with the tab it covers
        .shadow(false)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .skip_taskbar(true)
        .on_navigation(move |url| on_navigation(&nav_app, url))
        .on_new_window(move |url, _| {
            on_signal_or_open(&popup_app, url);
            NewWindowResponse::Deny
        })
        .initialization_script(crate::new_home::page_js());
    let color = crate::tabs::theme(app).and_then(|theme| crate::settings::theme_color(&theme, "bg"));
    if let Some(color) = color {
        builder = builder.background_color(color);
    }
    #[cfg(windows)]
    {
        builder = builder.owner_raw(owner.hwnd()?);
    }
    #[cfg(not(windows))]
    let _ = owner;
    let window = builder.build()?;
    #[cfg(windows)]
    {
        crate::frame::refresh(&window);
        crate::frame::no_transitions(&window);
    }
    let handle = app.clone();
    let launcher = window.clone();
    window.on_window_event(move |event| {
        // Gone only once another window is in front: on Windows the window
        // loses focus to its own page too, as that takes the keys
        if let WindowEvent::Focused(false) = event {
            let (handle, launcher) = (handle.clone(), launcher.clone());
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(150));
                if launcher.is_visible().unwrap_or(false) && !in_front(&launcher) {
                    hide(&handle, false);
                }
            });
        }
    });
    Ok(window)
}

fn in_front(window: &WebviewWindow) -> bool {
    #[cfg(windows)]
    return crate::frame::is_foreground(window);
    #[cfg(not(windows))]
    return window.is_focused().unwrap_or(false);
}

/// Hides the launcher. With `refocus`, the tab under it takes the keys
/// again, as when Escape closes it
pub fn hide(app: &AppHandle, refocus: bool) {
    // At once, so a show started after this isn't taken for an older one.
    // The hiding goes on the main thread, ordered with the shows there
    GENERATION.fetch_add(1, Ordering::SeqCst);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let Some(window) = handle.get_webview_window(WINDOW) else {
            return;
        };
        if !window.is_visible().unwrap_or(false) {
            return;
        }
        crate::tab_menu::release(&window);
        let _ = window.hide();
        if refocus {
            crate::tabs::refocus(&handle);
        }
    });
}

/// Closes the launcher's window, for when the main window goes
pub fn close(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.destroy();
    }
}

fn on_navigation(app: &AppHandle, url: &Url) -> bool {
    if crate::new_home::is_url(url) || !matches!(url.scheme(), "http" | "https" | "figma") {
        return true;
    }
    // Signals on macOS, and anything else the page would go to, which
    // opens in a tab or the browser instead
    on_signal_or_open(app, url.clone());
    false
}

/// A signal from the page (see `signal` in home.html), or a link it opens:
/// Figma's in a new tab, the rest in the browser. Off the webview's own
/// callback, which is still deciding on the page
fn on_signal_or_open(app: &AppHandle, url: Url) {
    let app = app.clone();
    std::thread::spawn(move || {
        if url.host_str() == Some(crate::new_home::SIGNAL_HOST) {
            match url.path() {
                "/close" => hide(&app, true),
                "/tab" => {
                    let view = url.query_pairs().find(|(key, _)| key == "value" || key == "view").map(|(_, view)| view.into_owned());
                    hide(&app, false);
                    if let Some(url) = view.as_deref().and_then(|view| crate::new_home::new_tab_url(Some(view))) {
                        let _ = crate::tabs::open(&app, url);
                    }
                }
                "/settings" => {
                    hide(&app, false);
                    let _ = crate::settings::open_window(&app);
                }
                _ => {}
            }
            return;
        }
        hide(&app, false);
        if url.scheme() == "figma" {
            crate::auth::on_figma_link(&app, &url);
        } else if crate::tabs::is_figma(&url) {
            let _ = crate::tabs::open(&app, url);
        } else if matches!(url.scheme(), "http" | "https") {
            use tauri_plugin_opener::OpenerExt;
            let _ = app.opener().open_url(url.as_str(), None::<&str>);
        }
    });
}
