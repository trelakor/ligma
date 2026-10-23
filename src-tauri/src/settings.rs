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

const WINDOW: &str = "settings";

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
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
    /// Show the open file and selected tool on Discord.
    pub discord_rich_presence: bool,
    /// Reopen the last session's tabs on launch, asleep until selected.
    pub restore_tabs: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sleep_tabs_after_minutes: 30,
            max_hidden_tabs_loaded: None,
            reload_snapshots: false,
            discord_rich_presence: true,
            restore_tabs: false,
        }
    }
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
    WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("settings.html".into()))
        .title("Settings")
        .inner_size(520.0, 480.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .center()
        .build()?;
    Ok(())
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
