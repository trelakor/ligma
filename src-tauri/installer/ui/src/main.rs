// The Windows installer's window. installer.nsi starts it as
// `ligma-setup-ui.exe <installer hwnd> <version> <first|again> <bundle id> <install dir>`
// and waits for it to set `Local\LigmaSetupReady<hwnd>`, or
// `Local\LigmaSetupFailed<hwnd>` to keep its own wizard. A first install then
// waits on the welcome step for `Local\LigmaSetupGo<hwnd>` (`GoBare` without a
// desktop shortcut) or `Local\LigmaSetupQuit<hwnd>`. Once it installs, the
// installer turns its wizard invisible, and this window reads the wizard's
// progress bar, follows it to the end, and gives the wizard back if the
// install stops on an error. A first install ends on a theme, saved to
// Ligma's settings, and an Open Ligma button. Any other ends by itself, as
// the installer opens Ligma
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() {
    app::run();
}

#[cfg(windows)]
mod app {
    use std::borrow::Cow;
    use std::ffi::c_void;
    use std::path::PathBuf;
    use std::thread;
    use std::time::{Duration, Instant};

    use tao::dpi::{LogicalSize, PhysicalPosition};
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
    use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
    use tao::window::{Window, WindowBuilder};
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, WAIT_OBJECT_0};
    use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenEventW, OpenProcess, SetEvent, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, GetClassNameW, GetDlgItem, GetWindowLongW, GetWindowThreadProcessId,
        IsWindow, SendMessageW, SetForegroundWindow, SetWindowLongW, ShowWindow, GWL_EXSTYLE,
        GWL_STYLE, SW_HIDE, SW_SHOW, WS_DISABLED, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
    };
    use wry::http::Response;
    use wry::{WebContext, WebViewBuilder};

    const INDEX: &str = include_str!("../index.html");
    const THEMES: &str = include_str!("../../../../dist/themes.js");
    const LOGO: &[u8] = include_bytes!("../../../../assets/logo.png");
    const INTER: &[u8] = include_bytes!("../../../../site/fonts/inter.woff2");

    const PBM_GETRANGE: u32 = 0x0407;
    const PBM_GETPOS: u32 = 0x0408;
    const EVENT_MODIFY_STATE: u32 = 0x0002;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const SYNCHRONIZE: u32 = 0x0010_0000;
    const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
    const DWMWCP_ROUND: u32 = 2;

    enum Msg {
        Ready,
        Drag,
        Install { desktop: bool },
        Quit,
        Theme(String),
        Open,
        Progress(f64),
        Done,
        Close,
    }

    struct Setup {
        version: String,
        first: bool,
        bundle: String,
        dir: PathBuf,
    }

    pub fn run() {
        let mut args = std::env::args().skip(1);
        let Some(installer) = args.next().and_then(|a| a.parse::<isize>().ok()) else {
            return;
        };
        let setup = Setup {
            version: args.next().unwrap_or_default(),
            first: args.next().as_deref() == Some("first"),
            bundle: args.next().unwrap_or_default(),
            dir: args.next().map(PathBuf::from).unwrap_or_default(),
        };

        // No WebView2 yet: the installer installs it, with its own wizard
        if wry::webview_version().is_err() {
            return signal(installer, "Failed");
        }

        let event_loop = EventLoopBuilder::<Msg>::with_user_event().build();
        let proxy = event_loop.create_proxy();
        let window = match WindowBuilder::new()
            .with_title("Install Ligma")
            .with_inner_size(LogicalSize::new(440.0, 340.0))
            .with_resizable(false)
            .with_maximizable(false)
            .with_decorations(false)
            .with_undecorated_shadow(true)
            .with_visible(false)
            .build(&event_loop)
        {
            Ok(window) => window,
            Err(_) => return signal(installer, "Failed"),
        };

        let mut context = WebContext::new(Some(temp().join("WebView2")));
        let ipc_proxy = proxy.clone();
        let webview = match WebViewBuilder::new_with_web_context(&mut context)
            .with_custom_protocol("ligma".into(), |_, req| {
                let (body, mime): (&'static [u8], &str) = match req.uri().path() {
                    "/logo.png" => (LOGO, "image/png"),
                    "/inter.woff2" => (INTER, "font/woff2"),
                    "/themes.js" => (THEMES.as_bytes(), "text/javascript"),
                    _ => (INDEX.as_bytes(), "text/html"),
                };
                Response::builder().header("Content-Type", mime).body(Cow::Borrowed(body)).unwrap()
            })
            .with_ipc_handler(move |req| {
                let body = req.body();
                let msg = match body.as_str() {
                    "ready" => Msg::Ready,
                    "drag" => Msg::Drag,
                    "install:desktop" => Msg::Install { desktop: true },
                    "install" => Msg::Install { desktop: false },
                    "quit" => Msg::Quit,
                    "open" => Msg::Open,
                    "close" => Msg::Close,
                    _ => match body.strip_prefix("theme:") {
                        Some(id) => Msg::Theme(id.to_owned()),
                        None => return,
                    },
                };
                let _ = ipc_proxy.send_event(msg);
            })
            .with_url(format!(
                "ligma://localhost/?v={}&first={}",
                setup.version,
                if setup.first { 1 } else { 0 }
            ))
            .build(&window)
        {
            Ok(webview) => webview,
            Err(_) => return signal(installer, "Failed"),
        };

        // Until the install starts the window can close, which cancels it.
        // While it installs it can't. After, it closes as it likes
        let mut installing = false;
        let hwnd = window.hwnd();
        event_loop.run(move |event, _, flow| {
            *flow = ControlFlow::Wait;
            match event {
                Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                    if !installing {
                        if !window.is_visible() || setup.first {
                            signal(installer, "Quit");
                        }
                        *flow = ControlFlow::Exit;
                    }
                }
                // Shown once the page has drawn, then the installer is told
                Event::UserEvent(Msg::Ready) if !window.is_visible() => {
                    center(&window);
                    round_corners(hwnd);
                    window.set_visible(true);
                    window.set_focus();
                    signal(installer, "Ready");
                    // Anything but a first install starts right away
                    if !setup.first {
                        installing = true;
                        let proxy = proxy.clone();
                        thread::spawn(move || watch(installer, proxy));
                    }
                }
                Event::UserEvent(Msg::Install { desktop }) if setup.first && !installing => {
                    installing = true;
                    signal(installer, if desktop { "Go" } else { "GoBare" });
                    let proxy = proxy.clone();
                    thread::spawn(move || watch(installer, proxy));
                }
                Event::UserEvent(Msg::Quit) if !installing => {
                    signal(installer, "Quit");
                    *flow = ControlFlow::Exit;
                }
                Event::UserEvent(Msg::Drag) => {
                    let _ = window.drag_window();
                }
                Event::UserEvent(Msg::Progress(p)) => {
                    let _ = webview.evaluate_script(&format!("progress({p:.4})"));
                }
                Event::UserEvent(Msg::Done) => {
                    installing = false;
                    let _ = webview.evaluate_script("done()");
                }
                Event::UserEvent(Msg::Theme(id)) => {
                    if let Err(error) = save_theme(&setup, &id) {
                        eprintln!("Couldn't save the theme: {error}");
                    }
                }
                Event::UserEvent(Msg::Open) => {
                    let _ = std::process::Command::new(setup.dir.join("ligma.exe")).current_dir(&setup.dir).spawn();
                    *flow = ControlFlow::Exit;
                }
                Event::UserEvent(Msg::Close) => {
                    installing = false;
                    *flow = ControlFlow::Exit;
                }
                _ => {}
            }
        });
    }

    fn temp() -> PathBuf {
        std::env::temp_dir().join("LigmaSetup")
    }

    // Sets the theme for light and dark mode in Ligma's settings.json. On a
    // first run Ligma takes an existing settings file to mean it ran before,
    // so `last-version` goes with it, or What's New would show every release
    fn save_theme(setup: &Setup, id: &str) -> Result<(), String> {
        if !id.chars().all(|c| c.is_ascii_lowercase()) || setup.bundle.is_empty() {
            return Err(format!("not a theme: {id}"));
        }
        let appdata = std::env::var_os("APPDATA").ok_or("no APPDATA")?;
        let dir = PathBuf::from(appdata).join(&setup.bundle);
        let path = dir.join("settings.json");
        let mut settings = match std::fs::read_to_string(&path) {
            Ok(json) => serde_json::from_str::<serde_json::Value>(&json).map_err(|e| e.to_string())?,
            Err(_) => {
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                std::fs::write(dir.join("last-version"), &setup.version).map_err(|e| e.to_string())?;
                serde_json::json!({})
            }
        };
        let object = settings.as_object_mut().ok_or("settings.json isn't an object")?;
        object.insert("lightTheme".into(), id.into());
        object.insert("darkTheme".into(), id.into());
        std::fs::write(&path, serde_json::to_string_pretty(&settings).unwrap()).map_err(|e| e.to_string())
    }

    // On the screen the cursor is on, where the installer was opened from
    fn center(window: &Window) {
        let cursor = window.cursor_position().ok();
        let monitor = window
            .available_monitors()
            .find(|m| {
                let (Some(c), p, s) = (cursor, m.position(), m.size()) else { return false };
                c.x >= p.x as f64
                    && c.y >= p.y as f64
                    && c.x < p.x as f64 + s.width as f64
                    && c.y < p.y as f64 + s.height as f64
            })
            .or_else(|| window.primary_monitor());
        let Some(monitor) = monitor else { return };
        let (p, s, w) = (monitor.position(), monitor.size(), window.outer_size());
        window.set_outer_position(PhysicalPosition::new(
            p.x + (s.width as i32 - w.width as i32) / 2,
            p.y + (s.height as i32 - w.height as i32) / 2,
        ));
    }

    fn round_corners(hwnd: isize) {
        let pref = DWMWCP_ROUND;
        unsafe {
            DwmSetWindowAttribute(
                hwnd as HWND,
                DWMWA_WINDOW_CORNER_PREFERENCE as _,
                &pref as *const u32 as *const c_void,
                4,
            );
        }
    }

    // Sets the installer's `Local\LigmaSetup<what><hwnd>` event
    fn signal(installer: isize, what: &str) {
        let name: Vec<u16> = format!("Local\\LigmaSetup{what}{installer}\0").encode_utf16().collect();
        unsafe {
            let event = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
            if !event.is_null() {
                SetEvent(event);
                CloseHandle(event);
            }
        }
    }

    // Follows the installer until it exits, from when it starts installing
    fn watch(installer: isize, proxy: EventLoopProxy<Msg>) {
        let hwnd = installer as HWND;
        let process = unsafe {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid)
        };
        if process.is_null() {
            let _ = proxy.send_event(Msg::Close);
            return;
        }

        let started = Instant::now();
        let mut hidden = false;
        let mut running = false;
        let mut stopped_since: Option<Instant> = None;
        let mut bar: isize = 0;
        loop {
            thread::sleep(Duration::from_millis(50));
            unsafe {
                if WaitForSingleObject(process, 0) == WAIT_OBJECT_0 {
                    let mut code = 1u32;
                    GetExitCodeProcess(process, &mut code);
                    CloseHandle(process);
                    let _ = proxy.send_event(if code == 0 { Msg::Done } else { Msg::Close });
                    return;
                }
                if IsWindow(hwnd) == 0 {
                    continue;
                }
                let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
                if !hidden {
                    // The installer gave up waiting and kept its own wizard, so this window goes
                    if ex & WS_EX_LAYERED != 0 {
                        hidden = true;
                    } else if started.elapsed() > Duration::from_secs(3) {
                        CloseHandle(process);
                        let _ = proxy.send_event(Msg::Close);
                        return;
                    }
                    continue;
                }

                if bar == 0 || IsWindow(bar as HWND) == 0 {
                    bar = find_progress_bar(hwnd);
                }
                if bar != 0 {
                    let max = SendMessageW(bar as HWND, PBM_GETRANGE, 0, 0);
                    let pos = SendMessageW(bar as HWND, PBM_GETPOS, 0, 0);
                    if max > 0 {
                        let _ = proxy.send_event(Msg::Progress((pos as f64 / max as f64).clamp(0.0, 1.0)));
                    }
                }

                // The wizard's Next button stays disabled while it installs. Enabled again
                // with the installer still open means it stopped on an error, since a
                // finished install closes itself, so the wizard comes back to show it
                let next = GetDlgItem(hwnd, 1);
                let disabled = !next.is_null() && GetWindowLongW(next, GWL_STYLE) as u32 & WS_DISABLED != 0;
                if disabled {
                    running = true;
                    stopped_since = None;
                } else if running && bar != 0 {
                    let since = *stopped_since.get_or_insert_with(Instant::now);
                    if since.elapsed() > Duration::from_millis(1500) {
                        reveal(hwnd, ex);
                        CloseHandle(process);
                        let _ = proxy.send_event(Msg::Close);
                        return;
                    }
                }
            }
        }
    }

    unsafe fn reveal(hwnd: HWND, ex: u32) {
        ShowWindow(hwnd, SW_HIDE);
        SetWindowLongW(hwnd, GWL_EXSTYLE, (ex & !(WS_EX_LAYERED | WS_EX_TOOLWINDOW)) as i32);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }

    fn find_progress_bar(hwnd: HWND) -> isize {
        unsafe extern "system" fn each(child: HWND, found: LPARAM) -> BOOL {
            let mut class = [0u16; 32];
            let len = GetClassNameW(child, class.as_mut_ptr(), class.len() as i32);
            if String::from_utf16_lossy(&class[..len.max(0) as usize]) == "msctls_progress32" {
                *(found as *mut isize) = child as isize;
                return 0;
            }
            1
        }
        let mut found: isize = 0;
        unsafe {
            EnumChildWindows(hwnd, Some(each), &mut found as *mut isize as LPARAM);
        }
        found
    }
}
