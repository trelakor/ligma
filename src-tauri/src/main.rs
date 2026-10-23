// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod downloads;
mod presence;
#[cfg(target_os = "macos")]
mod pressure;
mod settings;
mod tabs;
#[cfg(target_os = "macos")]
mod titlebar;
mod updater;
#[cfg(windows)]
mod webview2;
#[cfg(target_os = "macos")]
mod wkwebview;

use std::process::Child;
use std::sync::Mutex;

use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

/// Font agent process started by Ligma, killed again on exit.
struct FontAgent(Mutex<Option<Child>>);

/// Where the Figma desktop app and Figma's font installer put the agent.
#[cfg(target_os = "macos")]
fn font_agent_path() -> Option<std::path::PathBuf> {
    Some(
        std::path::PathBuf::from(std::env::var_os("HOME")?)
            .join("Library/Application Support/Figma/FigmaAgent.app/Contents/MacOS/figma_agent"),
    )
}

#[cfg(windows)]
fn font_agent_path() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("FigmaAgent/figma_agent.exe"))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn font_agent_path() -> Option<std::path::PathBuf> {
    None
}

/// Starts Figma's local font helper if it isn't already running. figma.com
/// asks it (on http://127.0.0.1:44950 or https://figmadaemon.com:44960) for
/// the user's installed fonts. It normally runs only alongside the Figma
/// desktop app or Figma's font installer.
fn start_font_agent() -> Option<Child> {
    use std::net::{SocketAddr, TcpStream};
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let addr = SocketAddr::from(([127, 0, 0, 1], 44950));
    if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
        return None;
    }
    let exe = font_agent_path().filter(|exe| exe.is_file())?;
    let mut command = Command::new(exe);
    command.stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.spawn().ok()
}

/// macOS menu bar. The app menu opens the settings window. File and Window
/// hold the tab shortcuts; Cmd+W closes the tab, not the window (or the
/// settings window, when that has focus). Other platforms have no menu bar: on Windows it would
/// show as a strip inside the window.
#[cfg(target_os = "macos")]
fn build_menu(app: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};

    let name = app.package_info().name.clone();
    Menu::with_items(
        app,
        &[
            &Submenu::with_items(
                app,
                &name,
                true,
                &[
                    &PredefinedMenuItem::about(app, None, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &MenuItem::with_id(app, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::services(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::hide(app, None)?,
                    &PredefinedMenuItem::hide_others(app, None)?,
                    &PredefinedMenuItem::show_all(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::quit(app, None)?,
                ],
            )?,
            &Submenu::with_items(
                app,
                "File",
                true,
                &[
                    &MenuItem::with_id(app, "new-tab", "New Tab", true, Some("CmdOrCtrl+T"))?,
                    &MenuItem::with_id(app, "close-tab", "Close Tab", true, Some("CmdOrCtrl+W"))?,
                ],
            )?,
            &Submenu::with_items(
                app,
                "Edit",
                true,
                &[
                    &PredefinedMenuItem::undo(app, None)?,
                    &PredefinedMenuItem::redo(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::cut(app, None)?,
                    &PredefinedMenuItem::copy(app, None)?,
                    &PredefinedMenuItem::paste(app, None)?,
                    &PredefinedMenuItem::select_all(app, None)?,
                ],
            )?,
            &Submenu::with_items(
                app,
                "Window",
                true,
                &[
                    &PredefinedMenuItem::minimize(app, None)?,
                    &PredefinedMenuItem::maximize(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &MenuItem::with_id(app, "next-tab", "Show Next Tab", true, Some("Ctrl+Tab"))?,
                    &MenuItem::with_id(
                        app,
                        "previous-tab",
                        "Show Previous Tab",
                        true,
                        Some("Ctrl+Shift+Tab"),
                    )?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::fullscreen(app, None)?,
                ],
            )?,
        ],
    )
}

fn main() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.menu(build_menu);

    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .on_menu_event(|app, event| {
            let _ = match event.id().as_ref() {
                "new-tab" => tabs::new_tab(app),
                "settings" => settings::open_window(app),
                "close-tab" if settings::close_if_focused(app) => Ok(()),
                "close-tab" => tabs::close_active(app),
                "next-tab" => tabs::cycle(app, 1),
                "previous-tab" => tabs::cycle(app, -1),
                _ => Ok(()),
            };
        })
        .invoke_handler(tauri::generate_handler![
            tabs::tab_bar_ready,
            tabs::select_tab,
            tabs::close_tab,
            tabs::open_new_tab,
            tabs::window_control,
            settings::get_settings,
            settings::set_settings,
        ])
        .setup(|app| {
            app.manage(FontAgent(Mutex::new(start_font_agent())));
            let settings = settings::load(app.handle());
            app.manage(tabs::Tabs::new(&settings));
            app.manage(presence::start(&settings));
            updater::start(app.handle());

            // The window's own webview is the tab bar. Figma tabs are added
            // as child webviews below it.
            #[allow(unused_mut)]
            let mut builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Ligma")
                .inner_size(1440.0, 900.0)
                .min_inner_size(800.0, 600.0)
                .background_color(tauri::window::Color(44, 44, 44, 255));
            #[cfg(target_os = "macos")]
            {
                builder = builder.title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true);
            }
            // Elsewhere the tab bar draws its own window controls.
            #[cfg(not(target_os = "macos"))]
            {
                builder = builder.decorations(false);
            }
            let window = builder.build()?;
            // Traffic lights centered on the tab bar (see titlebar.rs).
            #[cfg(target_os = "macos")]
            unsafe {
                titlebar::center_traffic_lights(window.ns_window()?.cast());
            }
            // `tabs::layout` sizes the tab bar's webview from here on.
            if let Some(bar) = app.get_webview("main") {
                bar.set_auto_resize(false)?;
            }

            let handle = app.handle().clone();
            window.on_window_event(move |event| {
                if let WindowEvent::Resized(_) = event {
                    tabs::layout(&handle);
                }
            });

            tabs::layout(app.handle());
            tabs::open(app.handle(), tabs::HOME_URL.parse()?)?;

            let handle = app.handle().clone();
            std::thread::spawn(move || {
                for second in 1u64.. {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    tabs::sync_theme(&handle);
                    presence::sync(&handle);
                    if second % 30 == 0 || (second % 5 == 0 && tabs::under_pressure()) {
                        tabs::rest_idle(&handle);
                    }
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building Ligma")
        .run(|app, event| {
            if let RunEvent::Exit = event {
                if let Some(mut agent) = app.state::<FontAgent>().0.lock().unwrap().take() {
                    let _ = agent.kill();
                }
                // An update the user chose to install later.
                updater::install(app);
            }
        });
}
