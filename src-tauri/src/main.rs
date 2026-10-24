// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod auth;
mod discord_ipc;
mod downloads;
#[cfg(windows)]
mod figma_scheme;
#[cfg(windows)]
mod frame;
mod presence;
#[cfg(target_os = "macos")]
mod pressure;
mod prompt;
mod report;
mod settings;
mod tab_menu;
mod tabs;
#[cfg(target_os = "macos")]
mod titlebar;
mod updater;
#[cfg(windows)]
mod webview2;
#[cfg(target_os = "macos")]
mod web_auth;
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

/// macOS menu bar. The app menu opens the settings window and checks for
/// updates. File and Window
/// hold the tab shortcuts, and Help reports a problem; Cmd+W closes the tab, not the window (or the
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
                    &MenuItem::with_id(app, "check-for-updates", "Check for Updates…", true, None::<&str>)?,
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
            &Submenu::with_items(
                app,
                "Help",
                true,
                &[&MenuItem::with_id(app, "report-problem", "Report a Problem…", true, None::<&str>)?],
            )?,
        ],
    )
}

/// Undoes what Ligma started or borrowed, before it quits. Also run when an
/// update's installer quits Ligma.
fn shut_down(app: &tauri::AppHandle) {
    if let Some(mut agent) = app.state::<FontAgent>().0.lock().unwrap().take() {
        let _ = agent.kill();
    }
    #[cfg(windows)]
    figma_scheme::restore();
}

fn main() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.menu(build_menu);

    builder
        // First, so a second launch exits before anything else starts. It
        // brings this window forward, and on Windows the `figma://` link
        // that started it to `figma_scheme`.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _| {
            if let Some(window) = app.get_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            // Off the main thread: a link may open a tab.
            #[cfg(windows)]
            {
                let app = app.clone();
                std::thread::spawn(move || figma_scheme::on_args(&app, _args));
            }
        }))
        // Ligma opens links from Rust. The plugin's own handler would catch
        // `target="_blank"` clicks in Figma tabs, which have no IPC, and drop
        // them (Add account, for one).
        .plugin(report::log_plugin())
        .plugin(tauri_plugin_opener::Builder::new().open_js_links_on_click(false).build())
        // Only the main window. Settings has a fixed size, which grows with
        // new options, and the plugin would show a new tab menu or prompt
        // right away, before it's placed.
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_filter(|label| label == "main")
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .on_menu_event(|app, event| {
            let _ = match event.id().as_ref() {
                "new-tab" => tabs::new_tab(app),
                "settings" => settings::open_window(app),
                "check-for-updates" => {
                    updater::check_now(app);
                    Ok(())
                }
                "report-problem" => {
                    report::open(app, None);
                    Ok(())
                }
                "close-tab" if settings::close_if_focused(app) => Ok(()),
                "close-tab" => tabs::close_active(app),
                "next-tab" => tabs::cycle(app, 1),
                "previous-tab" => tabs::cycle(app, -1),
                id if id.starts_with("tab-menu:") => tabs::on_menu(app, &id["tab-menu:".len()..]),
                _ => Ok(()),
            };
        })
        .invoke_handler(tauri::generate_handler![
            tabs::tab_bar_ready,
            tabs::select_tab,
            tabs::close_tab,
            tabs::open_new_tab,
            tabs::tab_menu,
            tabs::move_tab_to,
            tabs::window_control,
            settings::get_settings,
            settings::get_theme,
            settings::set_settings,
            settings::get_version,
            settings::check_for_updates,
            settings::report_problem,
            tab_menu::get_menu,
            tab_menu::menu_ready,
            tab_menu::menu_pick,
            prompt::get_prompt,
            prompt::prompt_ready,
            prompt::prompt_answer,
        ])
        .setup(|app| {
            report::catch_panics(app.handle());
            #[cfg(target_os = "macos")]
            wkwebview::disable_screen_time();
            app.manage(FontAgent(Mutex::new(start_font_agent())));
            app.manage(auth::Pending::default());
            // A sign-in cut short by a crash may still have Figma's
            // `figma://` borrowed.
            #[cfg(windows)]
            {
                figma_scheme::restore();
                figma_scheme::claim_if_free();
            }
            let settings = settings::load(app.handle());
            app.manage(tabs::Tabs::new(&settings));
            app.manage(presence::start(app.handle(), &settings));
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
            window.on_window_event(move |event| match event {
                WindowEvent::Resized(_) => tabs::layout(&handle),
                // The hidden tab menu window, or a prompt, would keep the
                // app running.
                WindowEvent::Destroyed => {
                    tab_menu::close(&handle);
                    prompt::close_all(&handle);
                }
                _ => {}
            });

            tabs::apply_scale(app.handle());
            tabs::restore(app.handle(), &settings)?;
            // Once there's a window to show it over.
            report::offer_last_crash(app.handle());
            updater::offer_last_failure(app.handle());
            // Started by a `figma://` link.
            #[cfg(windows)]
            {
                let app = app.handle().clone();
                std::thread::spawn(move || figma_scheme::on_args(&app, std::env::args().skip(1)));
            }

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
                shut_down(app);
                updater::install_on_quit(app);
            }
        });
}
