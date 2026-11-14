//! User settings, from `settings.json` in the app config folder
//! (`%APPDATA%\dev.ligma.client` on Windows,
//! `~/Library/Application Support/dev.ligma.client` on macOS). Written
//! with the defaults on first launch so there's a file to edit
//!
//! The settings window (`dist/settings.html`, Ligma > Settings… on macOS,
//! the gear by the window controls elsewhere)
//! edits the same file and applies each change right away

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, LogicalSize, Manager, PhysicalPosition, Webview, WebviewUrl, WebviewWindowBuilder};

pub const WINDOW: &str = "settings";

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Light or dark, for Figma and Ligma alike. System follows the
    /// system's. Takes the place of Figma's own Theme setting
    pub color_scheme: ColorScheme,
    /// Color theme in light mode, from `dist/themes.js`: "figma" for
    /// Figma's own colors,
    pub light_theme: String,
    /// and in dark mode
    pub dark_theme: String,
    /// App icon, from `dist/app-icons.js`, set by `app_icon::set_app_icon`
    /// with its image. "ligma" is Ligma's own
    pub app_icon: String,
    /// Size of Figma and Ligma's own interface, in percent, like a
    /// browser's zoom. The tab bar grows with it
    pub interface_scale: u32,
    /// Minutes a tab stays hidden before Ligma closes its webview to free
    /// memory. It reloads when selected. On macOS tabs sleep sooner under
    /// memory pressure. 0 keeps every tab loaded
    pub sleep_tabs_after_minutes: u64,
    /// Hidden tabs that stay loaded, most recently used first and Home last.
    /// The rest go to sleep right away. None (null): no limit. Ignored when
    /// sleeping is off
    pub max_hidden_tabs_loaded: Option<usize>,
    /// While a file tab reloads after sleep, show its last image instead of
    /// Figma's loading screen (macOS)
    pub reload_snapshots: bool,
    /// While a game is in front, put every tab to sleep, the ones on screen
    /// too, and give back what memory Ligma can (see game_mode.rs). Tabs on
    /// screen reload once Ligma is in front again
    pub game_mode: bool,
    /// Show Figma's notifications, the ones in its bell, as the system's
    /// own (see notifications.rs)
    pub figma_notifications: bool,
    /// Show the open file, where in it you are and the selected tool on
    /// Discord
    pub discord_rich_presence: bool,
    /// Name the open file on Discord; otherwise it says "Editing a file"
    pub discord_show_file: bool,
    /// Hide every file from Discord, as with `discord_show_file` off, but
    /// those shown from their tab's menu. Otherwise every file shows but
    /// those hidden there
    pub discord_hide_files_by_default: bool,
    /// Show the selected tool's icon on Discord, named when hovered
    pub discord_show_tool: bool,
    /// Under the file's name on Discord, show where in it you are (only
    /// with `discord_show_file`): the page in Figma Design,
    pub discord_show_page: bool,
    /// the page in FigJam,
    pub discord_show_figjam_page: bool,
    /// the slide in Figma Slides, or presenting (the frame of a prototype),
    pub discord_show_slide: bool,
    /// the chat in Figma Make,
    pub discord_show_chat: bool,
    /// the asset in Figma Buzz,
    pub discord_show_asset: bool,
    /// and the webpage in Figma Sites
    pub discord_show_webpage: bool,
    /// Let Discord friends join the open file: invite them from the + menu
    /// in a Discord chat, or accept when they ask to join, and it opens in
    /// their Ligma if they have access. Shows how many are in the file
    pub discord_invites: bool,
    /// Ask in Ligma when a friend asks to join (only with
    /// `discord_invites`). Otherwise requests are left to Discord
    pub discord_join_requests: bool,
    /// Reopen the last session's tabs on launch, asleep until selected
    pub restore_tabs: bool,
    /// Show a tab's last image and name under it while it's hovered
    pub tab_previews: bool,
    /// Split a window between two file tabs, from a tab's menu or by
    /// dragging one over the canvas (see split.rs). Off ends any split
    pub split_view: bool,
    /// Split under one set of Figma's panels and one toolbar, the focused
    /// tab's. Otherwise each tab takes its half of the window whole, panels
    /// and toolbar and all
    pub uniform_split: bool,
    /// Show development and saved plugins in the editor's left rail,
    pub rail_plugins: bool,
    /// and saved widgets
    pub rail_widgets: bool,
    /// Open Figma's Actions menu on its Plugins & widgets tab rather than
    /// All
    pub actions_plugins_tab: bool,
    /// Show each layer's preview in the layers panel in every mode, as in
    /// Draw mode. Figma renders them all, so off by default
    pub layer_previews: bool,
    /// Color each frame in the layers panel, and a tint of it what's in it,
    /// a step darker for each frame inside
    pub frame_colors: bool,
    /// Which colors: "rainbow", or a colorway, "sunset", "ocean", "forest",
    /// "aurora", "candy" or "ember" (see frame_colors.rs)
    pub frame_palette: String,
    /// Show Draw in the mode switch on the right of Figma Design's
    /// toolbar. With one mode left, the toolbar shows no switch,
    pub show_draw_mode: bool,
    /// Design,
    pub show_design_mode: bool,
    /// Motion,
    pub show_motion_mode: bool,
    /// and Dev Mode
    pub show_dev_mode: bool,
    /// Copy and paste the adjustments of image fills, with Cmd+Shift+C in
    /// the image picker and Cmd+Shift+V, or from the picker's menu
    pub adjustments_clipboard: bool,
    /// and with it, show Copy and Paste in the bar Figma shows above the
    /// toolbar for a selected image
    pub adjustments_bar: bool,
    /// In free (Starter) projects, hide the AI actions from that bar:
    /// Select area, Remove background, Edit with prompt and More
    pub hide_free_ai_actions: bool,
    /// and put Ligma's own Remove background in their place, which works
    /// on this device (macOS 14 and later, and Windows, see background.rs)
    pub local_remove_background: bool,
    /// On a Mac with Apple silicon, remove backgrounds with BiRefNet, the
    /// model Windows uses, rather than Apple's Vision
    pub use_background_model: bool,
    /// With `hide_free_ai_actions`, also put Ligma's own Vectorize in the
    /// image bar, which traces the image on this device (see vectorize.rs)
    pub local_vectorize: bool,
    /// With `hide_free_ai_actions`, also put Ligma's own Select object in
    /// the image bar, which picks out objects with a model on this device
    /// (see objects.rs)
    pub local_select_object: bool,
    /// With `hide_free_ai_actions`, also put Ligma's own Spot healing in
    /// the image bar, which fills what's painted over from the image around
    /// it (see heal.rs)
    pub local_heal: bool,
    /// and its Upscale, which makes the image larger with a model on this
    /// device (see upscale.rs)
    pub local_upscale: bool,
    /// Show the image bar's buttons as icons only, their names on hover
    pub image_bar_icons: bool,
    /// Tailwind and React in Figma's right-click menu, under Copy as code,
    /// on every plan
    pub copy_as_code: bool,
    /// Lasso fill in the Creation tools menu, under Pencil (see lasso.rs)
    pub lasso_fill: bool,
    /// Keep Figma's long-standing home page layout on accounts Figma is
    /// trying a newer one on
    pub classic_home: bool,
    /// Ligma's own home page in the Home tab, in place of Figma's file
    /// browser (see new_home.rs). On by default. It was `newHome`, off by
    /// default while experimental; that key is left behind so every
    /// install starts with it on once
    pub ligma_home: bool,
    /// The page Ligma's home page opens on: "home", "recents", "starred",
    /// "shared" or "drafts"
    pub ligma_home_start: String,
    /// Ligma MCP: let AI tools read and edit open files through Figwright's
    /// MCP server, with Ligma in place of its plugin (see mcp_bridge.rs).
    /// It was `mcpBridge`, off by default, with Figma MCP Bridge. That key
    /// is no longer read, so it's on for everyone once
    pub ligma_mcp: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            color_scheme: ColorScheme::System,
            light_theme: "figma".into(),
            dark_theme: "figma".into(),
            app_icon: crate::app_icon::DEFAULT.into(),
            interface_scale: 100,
            sleep_tabs_after_minutes: 30,
            max_hidden_tabs_loaded: None,
            reload_snapshots: false,
            game_mode: true,
            figma_notifications: true,
            discord_rich_presence: true,
            discord_show_file: true,
            discord_hide_files_by_default: false,
            discord_show_tool: true,
            discord_show_page: true,
            discord_show_figjam_page: true,
            discord_show_slide: true,
            discord_show_chat: true,
            discord_show_asset: true,
            discord_show_webpage: true,
            discord_invites: false,
            discord_join_requests: true,
            restore_tabs: false,
            tab_previews: true,
            split_view: true,
            uniform_split: true,
            rail_plugins: true,
            rail_widgets: true,
            actions_plugins_tab: false,
            layer_previews: false,
            frame_colors: false,
            frame_palette: "rainbow".into(),
            show_draw_mode: true,
            show_design_mode: true,
            show_motion_mode: true,
            show_dev_mode: true,
            adjustments_clipboard: true,
            adjustments_bar: true,
            hide_free_ai_actions: true,
            local_remove_background: true,
            use_background_model: false,
            local_vectorize: true,
            local_select_object: true,
            local_heal: true,
            local_upscale: true,
            image_bar_icons: false,
            copy_as_code: true,
            lasso_fill: true,
            classic_home: false,
            ligma_home: true,
            ligma_home_start: "home".into(),
            ligma_mcp: true,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorScheme {
    #[default]
    System,
    Light,
    Dark,
}

/// Interface scales the settings window steps through, in percent, like
/// a browser's zoom levels. Others typed in are kept within the first and
/// last
pub const SCALES: [u32; 11] = [50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200];

/// `interfaceScale` as a zoom factor, 1 at 100%
pub fn scale(settings: &Settings) -> f64 {
    let (min, max) = (SCALES[0], SCALES[SCALES.len() - 1]);
    f64::from(settings.interface_scale.clamp(min, max)) / 100.0
}

/// The settings in effect, for the settings window
pub struct Current(Mutex<Settings>);

/// The settings in effect
pub fn current(app: &AppHandle) -> Settings {
    app.state::<Current>().0.lock().unwrap().clone()
}

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

/// Opens the settings window, or focuses it if it's already open
pub fn open_window(app: &AppHandle) -> tauri::Result<()> {
    open_window_at(app, None)
}

/// Opens the settings window at the page and section with id `section` in
/// settings.html, like `"ai-tools"`, or focuses it there
pub fn open_window_at(app: &AppHandle, section: Option<&str>) -> tauri::Result<()> {
    // The gear button's clicks can arrive together; one window between them
    static OPENING: Mutex<()> = Mutex::new(());
    let _opening = OPENING.lock().unwrap();
    let section = section.map(|section| serde_json::to_string(section).unwrap());
    if let Some(window) = app.get_webview_window(WINDOW) {
        if let Some(section) = &section {
            window.eval(format!("window.showSection?.({section})"))?;
        }
        window.unminimize()?;
        return window.set_focus();
    }
    let size = window_size(app);
    // Shown by `get_settings`, once the page is at the interface scale
    #[allow(unused_mut)]
    let mut builder = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("settings.html".into()))
        .title("Settings")
        .inner_size(size.width, size.height)
        .visible(false)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .center();
    // Read by the page as it starts
    if let Some(section) = &section {
        builder = builder.initialization_script(format!("window.__ligmaOpenSection = {section};"));
    }
    // No flash of another color before the page draws
    if let Some(color) = crate::tabs::theme(app).as_ref().and_then(background) {
        builder = builder.background_color(color);
    }
    // The page draws its own title bar (see settings.html), with the
    // traffic lights in it on macOS and a close button elsewhere
    #[cfg(target_os = "macos")]
    {
        builder = builder.title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true);
    }
    #[cfg(not(target_os = "macos"))]
    {
        builder = builder.decorations(false);
    }
    let window = builder.build()?;
    // The app icon picked in settings, on the taskbar (see app_icon.rs)
    #[cfg(windows)]
    crate::app_icon::apply_to(&window);
    // The size it was built with, and no accent-colored border
    #[cfg(windows)]
    {
        crate::frame::refresh(&window);
        let _ = window.set_size(size);
        crate::frame::match_theme(&window);
        if let Some(webview) = app.get_webview(WINDOW) {
            crate::webview2::add_tab_shortcuts(&webview);
        }
    }
    // Traffic lights centered on the header, like on the tab bar. AppKit
    // only on the main thread
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

/// The settings window's size at the interface scale: fits the longest page
/// (Discord). On a small screen it fits the screen instead, and the page
/// scrolls
fn window_size(app: &AppHandle) -> LogicalSize<f64> {
    let scale = crate::tabs::scale(app);
    let (mut width, mut height) = (680.0 * scale, 600.0 * scale);
    if let Ok(Some(monitor)) = app.primary_monitor() {
        let area = monitor.work_area().size.to_logical::<f64>(monitor.scale_factor());
        width = width.min(area.width - 40.0);
        height = height.min(area.height - 40.0);
    }
    LogicalSize::new(width, height)
}

/// Zooms the settings window, if open, to the interface scale and resizes
/// it around its middle
pub fn apply_scale(app: &AppHandle, scale: f64) {
    let Some(window) = app.get_webview_window(WINDOW) else {
        return;
    };
    let _ = window.set_zoom(scale);
    let (Ok(position), Ok(old), Ok(factor)) = (window.outer_position(), window.outer_size(), window.scale_factor())
    else {
        return;
    };
    let size = window_size(app);
    let _ = window.set_size(size);
    let new = size.to_physical::<f64>(factor);
    let x = position.x as f64 + (old.width as f64 - new.width) / 2.0;
    let y = position.y as f64 + (old.height as f64 - new.height) / 2.0;
    let _ = window.set_position(PhysicalPosition::new(x.round() as i32, y.round().max(0.0) as i32));
}

/// The theme's background as a window color, if it's `#rrggbb`
fn background(theme: &serde_json::Value) -> Option<tauri::window::Color> {
    theme_color(theme, "bg")
}

/// Theme color `key` (see `THEME_JS` in tabs.rs) as a window color, if
/// it's `#rrggbb`
pub fn theme_color(theme: &serde_json::Value, key: &str) -> Option<tauri::window::Color> {
    let hex = theme.get(key)?.as_str()?.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let channel = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    Some(tauri::window::Color(channel(0)?, channel(2)?, channel(4)?, 255))
}

/// Passes the active tab's new theme to the settings window, if open
pub fn apply_theme(app: &AppHandle, theme: &serde_json::Value) {
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.eval(format!("window.applyTheme?.({theme})"));
    }
}

/// True if the settings window has keyboard focus, so Cmd+W closes it
/// rather than a tab
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

/// Closes the settings window, if open, for when the main window goes
pub fn close(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.destroy();
    }
}

/// Only the settings window may read and change settings
pub fn from_settings_window(webview: &Webview) -> bool {
    webview.label() == WINDOW
}

#[tauri::command]
pub async fn get_settings(app: AppHandle, webview: Webview) -> Option<Settings> {
    if !from_settings_window(&webview) {
        return None;
    }
    // Zoomed now: a zoom set before the page loaded doesn't last
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.set_zoom(crate::tabs::scale(&app));
        let _ = window.show();
        let _ = window.set_focus();
    }
    Some(app.state::<Current>().0.lock().unwrap().clone())
}

/// The active tab's Figma theme colors, for the settings window to match
#[tauri::command]
pub async fn get_theme(app: AppHandle, webview: Webview) -> Option<serde_json::Value> {
    from_settings_window(&webview).then(|| crate::tabs::theme(&app)).flatten()
}

/// Saves `settings` from the settings window and applies them
#[tauri::command]
pub async fn set_settings(app: AppHandle, webview: Webview, settings: Settings) -> Result<(), String> {
    if !from_settings_window(&webview) {
        return Err("not allowed".into());
    }
    change(&app, settings)
}

/// Saves `settings` and applies them to the running app
pub fn change(app: &AppHandle, settings: Settings) -> Result<(), String> {
    save(app, &settings)?;
    crate::tabs::apply_settings(app, &settings);
    crate::game_mode::apply_settings(&settings);
    crate::notifications::apply_settings(&settings);
    crate::appearance::apply_settings(app, &settings);
    crate::presence::apply_settings(app, &settings);
    crate::layers::apply_settings(app, &settings);
    crate::frame_colors::apply_settings(app, &settings);
    crate::home::apply_settings(app, &settings);
    crate::new_home::apply_settings(app, &settings);
    crate::toolbar::apply_settings(app, &settings);
    crate::adjustments::apply_settings(app, &settings);
    crate::ai_actions::apply_settings(app, &settings);
    crate::copy_code::apply_settings(app, &settings);
    crate::lasso::apply_settings(app, &settings);
    crate::mcp_bridge::apply_settings(app, &settings);
    let plugins_changed = {
        let current = app.state::<Current>();
        let mut current = current.0.lock().unwrap();
        let shown = |s: &Settings| (s.rail_plugins, s.rail_widgets, s.actions_plugins_tab);
        let changed = shown(&current) != shown(&settings);
        *current = settings;
        changed
    };
    if plugins_changed {
        crate::plugins::push(app);
    }
    Ok(())
}

/// Shows `settings`, changed elsewhere, in the settings window if open
pub fn show_in_window(app: &AppHandle, settings: &Settings) {
    if let Some(window) = app.get_webview_window(WINDOW) {
        let _ = window.eval(format!("window.applySettings?.({})", serde_json::to_string(settings).unwrap()));
    }
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

/// Opens a new GitHub issue for the user to describe a problem
#[tauri::command]
pub async fn report_problem(app: AppHandle, webview: Webview) {
    if from_settings_window(&webview) {
        crate::report::open(&app, None);
    }
}
