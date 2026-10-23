//! User settings, from `settings.json` in the app config folder
//! (`%APPDATA%\dev.ligma.client` on Windows,
//! `~/Library/Application Support/dev.ligma.client` on macOS). Written
//! with the defaults on first launch so there's a file to edit.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[derive(Deserialize, Serialize)]
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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sleep_tabs_after_minutes: 30,
            max_hidden_tabs_loaded: None,
            reload_snapshots: false,
            discord_rich_presence: true,
        }
    }
}

pub fn load(app: &AppHandle) -> Settings {
    let Ok(dir) = app.path().app_config_dir() else {
        return Settings::default();
    };
    let path = dir.join("settings.json");
    match std::fs::read_to_string(&path) {
        Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
        Err(_) => {
            let settings = Settings::default();
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&path, serde_json::to_string_pretty(&settings).unwrap());
            settings
        }
    }
}
