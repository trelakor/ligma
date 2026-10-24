//! User settings, from `settings.json` in the app config folder
//! (`%APPDATA%\dev.ligma.client` on Windows,
//! `~/Library/Application Support/dev.ligma.client` on macOS). Written
//! with the defaults on first launch so there's a file to edit.
//!
//! The settings window (`dist/settings.html`, Ligma > Settings… on macOS,
//! the gear by the window controls elsewhere)
//! edits the same file and applies each change right away.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Webview, WebviewUrl, WebviewWindowBuilder};

pub const WINDOW: &str = "settings";

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Size of Figma and Ligma's own interface, in percent, like a
    /// browser's zoom. The tab bar grows with it.
    pub interface_scale: u32,
    /// Minutes a tab stays hidden before Ligma closes its webview to free
    /// memory. It reloads when selected. On macOS tabs sleep sooner under
    /// memory pressure. 0 keeps every tab loaded.
    pub sleep_tabs_after_minutes: u64,
    /// Hidden tabs that stay loaded, most recently used first and Home last.
    /// The rest go to sleep right away. None (null): no limit. Ignored when
    /// sleeping is off.
    pub max_hidden_tabs_loaded: Option<usize>,
    /// While a file tab reloads after sleep, show its last image instead of
    /// Figma's loading screen (macOS).
    pub reload_snapshots: bool,
    /// Show the open file, where in it you are and the selected tool on
    /// Discord.
    pub discord_rich_presence: bool,
    /// Name the open file on Discord; otherwise it says "Editing a file".
    pub discord_show_file: bool,
    /// Show the selected tool's icon on Discord, named when hovered.
    pub discord_show_tool: bool,
    /// Under the file's name on Discord, show where in it you are: the
    /// page in Figma Design,
    pub discord_show_page: bool,
    /// the page in FigJam,
    pub discord_show_figjam_page: bool,
    /// the slide in Figma Slides, or presenting (the frame of a prototype),
    pub discord_show_slide: bool,
    /// the chat in Figma Make,
    pub discord_show_chat: bool,
    /// the asset in Figma Buzz,
    pub discord_show_asset: bool,
    /// and the webpage in Figma Sites.
    pub discord_show_webpage: bool,
    /// Let Discord friends join the open file: invite them from the + menu
    /// in a Discord chat, or accept when they ask to join, and it opens in
    /// their Ligma if they have access. Shows how many are in the file.
    pub discord_invites: bool,
    /// Reopen the last session's tabs on launch, asleep until selected.
    pub restore_tabs: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            interface_scale: 100,
            sleep_tabs_after_minutes: 30,
            max_hidden_tabs_loaded: None,
            reload_snapshots: false,
            discord_rich_presence: true,
            discord_show_file: true,
            discord_show_tool: true,
            discord_show_page: true,
            discord_show_figjam_page: true,
            discord_show_slide: true,
            discord_show_chat: true,
            discord_show_asset: true,
            discord_show_webpage: true,
            discord_invites: false,
            restore_tabs: false,
        }
    }
}

/// Interface scales the settings window steps through, in percent, like
/// a browser's zoom levels. Others typed in are kept within the first and
/// last.
pub const SCALES: [u32; 11] = [50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200];

/// `interfaceScale` as a zoom factor, 1 at 100%.
pub fn scale(settings: &Settings) -> f64 {
    let (min, max) = (SCALES[0], SCALES[SCALES.len() - 1]);
    f64::from(settings.interface_scale.clamp(min, max)) / 100.0
}

/// The settings in effect, for the settings window.
pub struct Current(Mutex<Settings>);

pub fn load(app: &AppHandle) -> Settings {
    let Ok(dir) = app.path().app_config_dir() else {
        return Settings::default();
    };
    let path = dir.join("settings.json");
    let settings = match std::fs::read_to_string(&path) {
        Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
        Err(_) => {
            let settings = Settings::default();
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&path, serde_json::to_string_pretty(&settings).unwrap());
            settings
        }
    };
    app.manage(Current(Mutex::new(settings.clone())));
    settings
}

fn save(app: &AppHandle, settings: &Settings) -> Result<(), String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("settings.json"), serde_json::to_string_pretty(settings).unwrap())
        .map_err(|e| e.to_string())
}

/// Opens the settings window, or focuses it if it's already open.
pub fn open_window(app: &AppHandle) -> tauri::Result<()> {
    // The gear button's clicks can arrive together; one window between them.
    static OPENING: Mutex<()> = Mutex::new(());
    let _opening = OPENING.lock().unwrap();
    if let Some(window) = app.get_webview_window(WINDOW) {
        window.unminimize()?;
        return window.set_focus();
    }
    // Fits the longest page (Discord). On a short screen it fits the
    // screen instead, and the page scrolls.
    let mut height: f64 = 600.0;
    if let Ok(Some(monitor)) = app.primary_monitor() {
        let area = monitor.work_area().size.to_logical::<f64>(monitor.scale_factor());
        height = height.min(area.height - 40.0);
    }
    #[allow(unused_mut)]
    let mut builder = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("settings.html".into()))
        .title("Settings")
        .inner_size(680.0, height)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .center();
    // No flash of another color before the page draws.
    if let Some(color) = crate::tabs::theme(app).as_ref().and_then(background) {
        builder = builder.background_color(color);
    }
    // The page draws its own title bar (see settings.html), with the
    // traffic lights in it on macOS and a close button elsewhere.
    #[cfg(target_os = "macos")]
    {
        builder = builder.title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true);
    }
    #[cfg(not(target_os = "macos"))]
    {
        builder = builder.decorations(false);
    }
    let window = builder.build()?;
    // No accent-colored border.
    #[cfg(windows)]
    crate::frame::match_theme(&window);
    // Traffic lights centered on the header, like on the tab bar. AppKit
    // only on the main thread.
    #[cfg(target_os = "macos")]
    {
        let ns_window = window.clone();
        window.run_on_main_thread(move || {
            if let Ok(ns_window) = ns_window.ns_window() {
                unsafe { crate::titlebar::center_traffic_lights(ns_window.cast()) };
            }
        })?;
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = window;
    Ok(())
}

/// The theme's background as a window color, if it's `#rrggbb`.
fn background(theme: &serde_json::Value) -> Option<tauri::window::Color> {
    theme_color(theme, "bg")
}

/// Theme color `key` (see `THEME_JS` in tabs.rs) as a window color, if
/// it's `#rrggbb`.
pub fn theme_color(theme: &serde_json::Value, key: &str) -> Option<tauri::window::Color> {
    let hex = theme.get(key)?.as_str()?.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    Some(tauri::window::Color(channel(0)?, channel(2)?, channel(4)?, 255))
}

/// Passes the active tab's new theme to the settings window, if open.
pub fn apply_theme(app: &AppHandle, theme: &serde_json::Value) {
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.eval(format!("window.applyTheme?.({theme})"));
    }
}

/// True if the settings window has keyboard focus, so Cmd+W closes it
/// rather than a tab.
pub fn close_if_focused(app: &AppHandle) -> bool {
    let Some(window) = app.get_webview_window(WINDOW) else {
        return false;
    };
    if !window.is_focused().unwrap_or(false) {
        return false;
    }
    let _ = window.close();
    true
}

/// Only the settings window may read and change settings.
fn from_settings_window(webview: &Webview) -> bool {
    webview.label() == WINDOW
}

#[tauri::command]
pub async fn get_settings(app: AppHandle, webview: Webview) -> Option<Settings> {
    from_settings_window(&webview).then(|| app.state::<Current>().0.lock().unwrap().clone())
}

/// The active tab's Figma theme colors, for the settings window to match.
#[tauri::command]
pub async fn get_theme(app: AppHandle, webview: Webview) -> Option<serde_json::Value> {
    from_settings_window(&webview).then(|| crate::tabs::theme(&app)).flatten()
}

/// Saves `settings` and applies them to the running app.
#[tauri::command]
pub async fn set_settings(app: AppHandle, webview: Webview, settings: Settings) -> Result<(), String> {
    if !from_settings_window(&webview) {
        return Err("not allowed".into());
    }
    save(&app, &settings)?;
    crate::tabs::apply_settings(&app, &settings);
    crate::presence::apply_settings(&app, &settings);
    *app.state::<Current>().0.lock().unwrap() = settings;
    Ok(())
}

#[tauri::command]
pub async fn get_version(app: AppHandle, webview: Webview) -> Option<String> {
    from_settings_window(&webview).then(|| app.package_info().version.to_string())
}

#[tauri::command]
pub async fn check_for_updates(app: AppHandle, webview: Webview) {
    if from_settings_window(&webview) {
        crate::updater::check_now(&app);
    }
}

/// Opens a new GitHub issue for the user to describe a problem.
#[tauri::command]
pub async fn report_problem(app: AppHandle, webview: Webview) {
    if from_settings_window(&webview) {
        crate::report::open(&app, None);
    }
}
