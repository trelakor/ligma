// The Windows installer's window. installer.nsi starts it as
// `ligma-setup-ui.exe <installer hwnd> <version>` and waits for it to set the
// `Local\LigmaSetupReady<hwnd>` event, then makes its own wizard invisible, or
// for `Local\LigmaSetupFailed<hwnd>`, then keeps the wizard. From
// then on this window reads the wizard's progress bar, closes when the
// installer exits, and gives the wizard back if the install stops on an error
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

    use tao::dpi::LogicalSize;
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
    use tao::platform::windows::{WindowBuilderExtWindows, WindowExtWindows};
    use tao::window::WindowBuilder;
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
        Progress(f64),
        Done,
        Close,
    }

    pub fn run() {
        let mut args = std::env::args().skip(1);
        let Some(installer) = args.next().and_then(|a| a.parse::<isize>().ok()) else {
            return;
        };
        let version = args.next().unwrap_or_default();

        // No WebView2 yet: the installer installs it, with its own wizard
        if wry::webview_version().is_err() {
            signal(installer, "Failed");
            return;
        }

        let event_loop = EventLoopBuilder::<Msg>::with_user_event().build();
        let proxy = event_loop.create_proxy();
        let window = match WindowBuilder::new()
            .with_title("Installing Ligma")
            .with_inner_size(LogicalSize::new(400.0, 280.0))
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

        let mut context = WebContext::new(Some(data_dir()));
        let ipc_proxy = proxy.clone();
        let hwnd = window.hwnd();
        let webview = match WebViewBuilder::new_with_web_context(&mut context)
            .with_custom_protocol("ligma".into(), |_, req| {
                let (body, mime): (&'static [u8], &str) = match req.uri().path() {
                    "/logo.png" => (LOGO, "image/png"),
                    "/inter.woff2" => (INTER, "font/woff2"),
                    _ => (INDEX.as_bytes(), "text/html"),
                };
                Response::builder()
                    .header("Content-Type", mime)
                    .body(Cow::Borrowed(body))
                    .unwrap()
            })
            .with_ipc_handler(move |req| {
                let msg = match req.body().as_str() {
                    "ready" => Msg::Ready,
                    "drag" => Msg::Drag,
                    "close" => Msg::Close,
                    _ => return,
                };
                let _ = ipc_proxy.send_event(msg);
            })
            .with_url(format!("ligma://localhost/?v={version}"))
            .build(&window)
        {
            Ok(webview) => webview,
            Err(_) => return signal(installer, "Failed"),
        };

        let mut finished = false;
        event_loop.run(move |event, _, flow| {
            *flow = ControlFlow::Wait;
            match event {
                Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                    // The install keeps going without the window, so it only closes once it's over
                    if finished {
                        *flow = ControlFlow::Exit;
                    }
                }
                // Shown once the page has drawn, then the installer is told to hide its wizard
                Event::UserEvent(Msg::Ready) if !window.is_visible() => {
                    center(&window);
                    round_corners(hwnd);
                    window.set_visible(true);
                    window.set_focus();
                    signal(installer, "Ready");
                    let proxy = proxy.clone();
                    thread::spawn(move || watch(installer, proxy));
                }
                Event::UserEvent(Msg::Drag) => {
                    let _ = window.drag_window();
                }
                Event::UserEvent(Msg::Progress(p)) => {
                    let _ = webview.evaluate_script(&format!("progress({p:.4})"));
                }
                Event::UserEvent(Msg::Done) => {
                    finished = true;
                    let _ = webview.evaluate_script("done()");
                }
                Event::UserEvent(Msg::Close) => *flow = ControlFlow::Exit,
                _ => {}
            }
        });
    }

    // On the screen the cursor is on, where the installer was opened from
    fn center(window: &tao::window::Window) {
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
        window.set_outer_position(tao::dpi::PhysicalPosition::new(
            p.x + (s.width as i32 - w.width as i32) / 2,
            p.y + (s.height as i32 - w.height as i32) / 2,
        ));
    }

    fn data_dir() -> PathBuf {
        std::env::temp_dir().join("LigmaSetup").join("WebView2")
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

    // Sets the installer's `Local\LigmaSetupReady<hwnd>` or `Local\LigmaSetupFailed<hwnd>` event
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

    // Follows the installer until it exits. Runs once the window is up
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
