//! Figma-style tabs. The main window's own webview is the local tab bar
//! (`dist/index.html`). Each tab is a child webview below it, and only the
//! active one is visible.

use std::sync::Mutex;

use serde::Serialize;
use tauri::webview::{NewWindowResponse, WebviewBuilder};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Url, Webview, WebviewUrl, Window};
use tauri_plugin_opener::OpenerExt;

pub const TAB_BAR_HEIGHT: f64 = 40.0;
pub const HOME_URL: &str = "https://www.figma.com/files";
const HOME_ID: u32 = 0;

// WKWebView's default UA has no Safari version token, which can trigger
// Figma's "unsupported browser" notice. Present as current Safari instead.
#[cfg(target_os = "macos")]
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";

#[derive(Serialize)]
struct Tab {
    id: u32,
    title: String,
    kind: &'static str,
    #[serde(skip)]
    file_key: Option<String>,
}

#[derive(Default)]
struct TabState {
    tabs: Vec<Tab>,
    active: u32,
    next_id: u32,
    /// Colors of the active tab's Figma theme, from `THEME_JS`.
    theme: Option<serde_json::Value>,
}

#[derive(Default)]
pub struct Tabs(Mutex<TabState>);

/// Reads Figma's theme colors. Figma sets `data-preferred-theme` on <body>
/// and defines its palette as CSS variables there; FigJam and Dev Mode
/// override them, so this follows the active editor too.
const THEME_JS: &str = r#"(() => {
  const el = document.querySelector("[data-preferred-theme]");
  if (!el) return null;
  const style = getComputedStyle(el);
  const v = (name) => style.getPropertyValue(name).trim();
  return {
    bg: v("--color-bg"),
    bgSecondary: v("--color-bg-secondary"),
    bgTertiary: v("--color-bg-tertiary"),
    text: v("--color-text"),
    textSecondary: v("--color-text-secondary"),
    border: v("--color-border"),
  };
})()"#;

fn label(id: u32) -> String {
    format!("tab-{id}")
}

fn is_figma(url: &Url) -> bool {
    url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| host == "figma.com" || host.ends_with(".figma.com"))
}

/// Tab icon for a Figma URL, from the first path segment (`/design/<key>/...`).
fn kind(url: &Url) -> &'static str {
    match url.path_segments().and_then(|mut s| s.next()) {
        Some("design" | "file") => "design",
        Some("board") => "board",
        Some("slides" | "deck") => "slides",
        Some("proto") => "proto",
        Some("make") => "make",
        Some("site") => "site",
        Some("buzz") => "buzz",
        _ => "page",
    }
}

/// Key of the Figma file an editor URL points at, if any.
fn file_key(url: &Url) -> Option<String> {
    if !is_figma(url) || kind(url) == "page" {
        return None;
    }
    url.path_segments()?
        .nth(1)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
}

/// Opens `url` in a new tab, or focuses the tab already showing that file.
pub fn open(app: &AppHandle, url: Url) -> tauri::Result<()> {
    let key = file_key(&url);
    let id = {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        let existing = key.as_ref().and_then(|key| {
            let tab = state.tabs.iter().find(|tab| tab.file_key.as_ref() == Some(key))?;
            Some(tab.id)
        });
        if let Some(id) = existing {
            drop(state);
            return select(app, id);
        }
        let id = state.next_id;
        state.next_id += 1;
        state.tabs.push(Tab {
            id,
            title: String::new(),
            kind: if id == HOME_ID { "home" } else { kind(&url) },
            file_key: key,
        });
        id
    };

    let window = app.get_window("main").expect("main window exists");
    let (nav_app, popup_app, title_app) = (app.clone(), app.clone(), app.clone());
    #[allow(unused_mut)]
    let mut builder = WebviewBuilder::new(label(id), WebviewUrl::External(url))
        // Let Figma receive HTML5 drag-and-drop (image/file imports).
        .disable_drag_drop_handler()
        .on_navigation(move |url| on_navigation(&nav_app, id, url))
        .on_new_window(move |url, _| {
            on_new_window(&popup_app, url);
            NewWindowResponse::Deny
        })
        .on_document_title_changed(move |webview, title| {
            on_title_changed(&title_app, id, &webview, &title)
        });
    #[cfg(target_os = "macos")]
    {
        builder = builder.user_agent(USER_AGENT);
    }
    window.add_child(
        builder,
        LogicalPosition::new(0.0, TAB_BAR_HEIGHT),
        content_size(&window)?,
    )?;
    select(app, id)
}

/// `open` from a webview callback. Those run on the main thread, and
/// creating a webview there would block on itself.
fn open_later(app: &AppHandle, url: Url) {
    let app = app.clone();
    std::thread::spawn(move || {
        let _ = open(&app, url);
    });
}

fn open_in_browser(app: &AppHandle, url: &Url) {
    let _ = app.opener().open_url(url.as_str(), None::<&str>);
}

fn on_navigation(app: &AppHandle, id: u32, url: &Url) -> bool {
    match url.scheme() {
        "http" | "https" => {}
        // blob:, data:, about: and friends stay in the page.
        _ => return true,
    }
    if !is_figma(url) {
        open_in_browser(app, url);
        return false;
    }
    // Like the desktop app, files opened from Home get their own tab.
    if id == HOME_ID && file_key(url).is_some() {
        open_later(app, url.clone());
        return false;
    }
    true
}

fn on_new_window(app: &AppHandle, url: Url) {
    if is_figma(&url) {
        open_later(app, url);
    } else if matches!(url.scheme(), "http" | "https") {
        open_in_browser(app, &url);
    }
}

fn on_title_changed(app: &AppHandle, id: u32, webview: &Webview, title: &str) {
    let url = webview.url().ok();
    // Figma can switch pages client-side without a navigation, so a file
    // can still end up in Home. Move it to its own tab.
    if id == HOME_ID {
        if let Some(url) = url.as_ref().filter(|url| file_key(url).is_some()) {
            let _ = webview.navigate(HOME_URL.parse().unwrap());
            open_later(app, url.clone());
            return;
        }
    }
    {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        tab.title = title.strip_suffix(" – Figma").unwrap_or(title).to_owned();
        if let Some(url) = url.filter(|_| id != HOME_ID) {
            tab.kind = kind(&url);
            tab.file_key = file_key(&url);
        }
    }
    push_state(app);
}

pub fn select(app: &AppHandle, id: u32) -> tauri::Result<()> {
    let previous = {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        if !state.tabs.iter().any(|tab| tab.id == id) {
            return Ok(());
        }
        std::mem::replace(&mut state.active, id)
    };
    if let Some(webview) = app.get_webview(&label(id)) {
        webview.show()?;
        webview.set_focus()?;
    }
    sync_theme(app);
    if previous != id {
        if let Some(webview) = app.get_webview(&label(previous)) {
            webview.hide()?;
        }
    }
    push_state(app);
    Ok(())
}

/// Closes a tab. Home can't be closed.
pub fn close(app: &AppHandle, id: u32) -> tauri::Result<()> {
    if id == HOME_ID {
        return Ok(());
    }
    let next = {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        let Some(index) = state.tabs.iter().position(|tab| tab.id == id) else {
            return Ok(());
        };
        state.tabs.remove(index);
        // Prefer the tab to the right, like the desktop app. Home is always
        // left, so the list is never empty.
        (state.active == id).then(|| state.tabs[index.min(state.tabs.len() - 1)].id)
    };
    if let Some(next) = next {
        select(app, next)?;
    }
    if let Some(webview) = app.get_webview(&label(id)) {
        webview.close()?;
    }
    push_state(app);
    Ok(())
}

pub fn close_active(app: &AppHandle) -> tauri::Result<()> {
    let active = app.state::<Tabs>().inner().0.lock().unwrap().active;
    close(app, active)
}

/// Selects the tab `offset` places from the active one, wrapping around.
pub fn cycle(app: &AppHandle, offset: isize) -> tauri::Result<()> {
    let id = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        let len = state.tabs.len() as isize;
        let index = state.tabs.iter().position(|tab| tab.id == state.active).unwrap_or(0) as isize;
        state.tabs[(index + offset).rem_euclid(len) as usize].id
    };
    select(app, id)
}

pub fn new_tab(app: &AppHandle) -> tauri::Result<()> {
    open(app, HOME_URL.parse().unwrap())
}

/// Matches the tab bar to the active tab's Figma theme. Runs on tab switch
/// and on a timer, since Figma changes theme without any event Ligma sees.
pub fn sync_theme(app: &AppHandle) {
    let active = app.state::<Tabs>().inner().0.lock().unwrap().active;
    let Some(webview) = app.get_webview(&label(active)) else {
        return;
    };
    let app = app.clone();
    let _ = webview.eval_with_callback(THEME_JS, move |json| {
        // null until Figma has rendered its UI.
        let Ok(theme @ serde_json::Value::Object(_)) = serde_json::from_str(&json) else {
            return;
        };
        {
            let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
            if state.active != active || state.theme.as_ref() == Some(&theme) {
                return;
            }
            state.theme = Some(theme);
        }
        push_state(&app);
    });
}

fn content_size(window: &Window) -> tauri::Result<LogicalSize<f64>> {
    let size = window.inner_size()?.to_logical::<f64>(window.scale_factor()?);
    Ok(LogicalSize::new(size.width, (size.height - TAB_BAR_HEIGHT).max(0.0)))
}

/// Refits tab webviews to the window. Call on resize.
pub fn layout(app: &AppHandle) {
    let Some(window) = app.get_window("main") else {
        return;
    };
    let Ok(size) = content_size(&window) else {
        return;
    };
    for webview in window.webviews() {
        if webview.label().starts_with("tab-") {
            let _ = webview.set_size(size);
        }
    }
    push_state(app);
}

/// Sends the tab list to the tab bar.
fn push_state(app: &AppHandle) {
    #[derive(Serialize)]
    struct View<'a> {
        tabs: &'a [Tab],
        active: u32,
        fullscreen: bool,
        theme: &'a Option<serde_json::Value>,
        platform: &'static str,
    }

    // Not `get_webview_window`: that returns None once the window holds
    // more than one webview.
    let (Some(window), Some(bar)) = (app.get_window("main"), app.get_webview("main")) else {
        return;
    };
    let fullscreen = window.is_fullscreen().unwrap_or(false);
    let json = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        serde_json::to_string(&View {
            tabs: &state.tabs,
            active: state.active,
            fullscreen,
            theme: &state.theme,
            platform: std::env::consts::OS,
        })
        .unwrap()
    };
    let _ = bar.eval(format!("window.render?.({json})"));
}

/// Only the local tab bar may drive tabs. Figma pages have no IPC access
/// anyway (no capability grants it), this is a second fence.
fn from_tab_bar(webview: &Webview) -> bool {
    webview.label() == "main"
}

#[tauri::command]
pub async fn tab_bar_ready(app: AppHandle, webview: Webview) {
    if from_tab_bar(&webview) {
        push_state(&app);
    }
}

#[tauri::command]
pub async fn select_tab(app: AppHandle, webview: Webview, id: u32) {
    if from_tab_bar(&webview) {
        let _ = select(&app, id);
    }
}

#[tauri::command]
pub async fn close_tab(app: AppHandle, webview: Webview, id: u32) {
    if from_tab_bar(&webview) {
        let _ = close(&app, id);
    }
}

#[tauri::command]
pub async fn open_new_tab(app: AppHandle, webview: Webview) {
    if from_tab_bar(&webview) {
        let _ = new_tab(&app);
    }
}

/// Minimize, maximize and close buttons the tab bar draws where the window
/// has no native title bar (Windows, Linux).
#[tauri::command]
pub async fn window_control(webview: Webview, action: String) {
    if !from_tab_bar(&webview) {
        return;
    }
    let window = webview.window();
    let _ = match action.as_str() {
        "minimize" => window.minimize(),
        "maximize" if window.is_maximized().unwrap_or(false) => window.unmaximize(),
        "maximize" => window.maximize(),
        "close" => window.close(),
        _ => Ok(()),
    };
}
