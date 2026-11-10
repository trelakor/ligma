//! Game mode (`gameMode`): while a game is in front, every tab goes to
//! sleep, the ones on screen too, and Ligma gives back what memory it can,
//! so the game gets it. Tabs on screen reload once Ligma is in front again,
//! the others when selected
//!
//! The app in front is a game when its executable is in a game library
//! (Steam, Epic, GOG and others), or on Windows when it runs in exclusive
//! full screen or Windows knows it as a game (`GameConfigStore`, the list
//! Windows' own Game Mode goes by), or on macOS when its bundle's category
//! is a game one (what macOS' own Game Mode goes by). Game mode lasts while
//! that game runs and Ligma isn't in front, so switching to another app
//! mid-game doesn't wake anything. A tab with work that leaving would lose,
//! or that's offline, stays loaded (see `tabs::sleep_all`)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::AppHandle;

static ENABLED: AtomicBool = AtomicBool::new(true);

/// How often the app in front is checked
const CHECK_EVERY: Duration = Duration::from_secs(1);

/// How often tabs that couldn't sleep yet are asked again, in checks
const RETRY_EVERY: u32 = 5;

/// Parts of a path, lowercase, that put an executable in a game library,
/// and parts that make it the library's launcher instead
const LIBRARIES: [&str; 8] = [
    "/steamapps/common/",
    "/epic games/",
    "/gog galaxy/games/",
    "/gog games/",
    "/xboxgames/",
    "/riot games/",
    "/ea games/",
    "/ubisoft game launcher/games/",
];
const LAUNCHERS: [&str; 2] = ["/epic games/launcher/", "/riot games/riot client/"];

pub fn init(settings: &crate::settings::Settings) {
    ENABLED.store(settings.game_mode, Ordering::Relaxed);
}

pub fn apply_settings(settings: &crate::settings::Settings) {
    init(settings);
}

/// The app in front
struct Front {
    pid: u32,
    /// None if the process can't be looked into
    exe: Option<PathBuf>,
    /// Shows it's a game, whatever its path
    game: bool,
}

/// The game game mode is on for
struct Game {
    pid: u32,
    exe: Option<PathBuf>,
}

/// Checks the app in front every `CHECK_EVERY`, on a thread of its own
pub fn start(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || watch(&app));
}

fn watch(app: &AppHandle) {
    let own = std::process::id();
    let mut game: Option<Game> = None;
    // Checks game mode has been on for
    let mut checks = 0u32;
    // Tabs on screen to reload once Ligma is in front
    let mut wake = false;
    let mut known: HashMap<PathBuf, bool> = HashMap::new();
    loop {
        std::thread::sleep(CHECK_EVERY);
        let front = platform::front(app);
        let ligma = front.as_ref().is_some_and(|front| front.pid == own);
        let enabled = ENABLED.load(Ordering::Relaxed);
        if let Some(current) = &game {
            if !enabled || ligma || !running(current) {
                log::info!("Game mode off");
                game = None;
                wake = true;
            }
        }
        if game.is_none() && enabled && !ligma {
            if let Some(front) = front.filter(|front| is_game(front, &mut known)) {
                log::info!("Game mode on for {}", front.exe.as_deref().unwrap_or(Path::new("?")).display());
                game = Some(Game { pid: front.pid, exe: front.exe });
                checks = 0;
                wake = false;
            }
        }
        if game.is_some() {
            if checks % RETRY_EVERY == 0 {
                crate::tabs::sleep_all(app);
            }
            // Once the webviews have closed
            if checks == 2 {
                platform::trim();
            }
            checks += 1;
        }
        if wake && ligma {
            wake = false;
            crate::tabs::wake_shown(app);
        }
    }
}

fn is_game(front: &Front, known: &mut HashMap<PathBuf, bool>) -> bool {
    if front.game {
        return true;
    }
    let Some(exe) = &front.exe else {
        return false;
    };
    *known.entry(exe.clone()).or_insert_with(|| in_library(exe) || platform::known_game(exe))
}

fn in_library(exe: &Path) -> bool {
    let path = exe.to_string_lossy().replace('\\', "/").to_lowercase();
    LIBRARIES.iter().any(|part| path.contains(part)) && !LAUNCHERS.iter().any(|part| path.contains(part))
}

/// True while `game` still runs: its process is there and still runs the
/// same executable, in case its ID was reused
fn running(game: &Game) -> bool {
    match &game.exe {
        Some(exe) => platform::exe_path(game.pid).as_ref() == Some(exe),
        // Can't tell, so it lasts until Ligma is in front
        None => true,
    }
}

#[cfg(windows)]
mod platform {
    use std::os::windows::ffi::OsStringExt;
    use std::path::{Path, PathBuf};

    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::ProcessStatus::K32EmptyWorkingSet;
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_RUNNING_D3D_FULL_SCREEN};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    use windows_core::PWSTR;

    pub(super) fn front(_: &tauri::AppHandle) -> Option<super::Front> {
        let mut pid = 0;
        unsafe {
            let window = GetForegroundWindow();
            if window.is_invalid() {
                return None;
            }
            GetWindowThreadProcessId(window, Some(&mut pid));
        }
        if pid == 0 {
            return None;
        }
        // Set for the app in front only
        let full_screen = unsafe { SHQueryUserNotificationState() } == Ok(QUNS_RUNNING_D3D_FULL_SCREEN);
        Some(super::Front { pid, exe: exe_path(pid), game: full_screen })
    }

    pub(super) fn exe_path(pid: u32) -> Option<PathBuf> {
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buffer = [0u16; 1024];
            let mut len = buffer.len() as u32;
            let read = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len);
            let _ = CloseHandle(process);
            read.ok()?;
            Some(std::ffi::OsString::from_wide(&buffer[..len as usize]).into())
        }
    }

    /// True if Windows knows `exe` as a game: Game Bar keeps the games it
    /// has seen under `GameConfigStore\Children`
    pub(super) fn known_game(exe: &Path) -> bool {
        let Ok(children) = windows_registry::CURRENT_USER.open(r"System\GameConfigStore\Children") else {
            return false;
        };
        let Ok(keys) = children.keys() else {
            return false;
        };
        let exe = exe.to_string_lossy().to_lowercase();
        keys.filter_map(|name| children.open(name).ok())
            .filter_map(|child| child.get_string("MatchedExeFullPath").ok())
            .any(|path| path.to_lowercase() == exe)
    }

    /// Gives back the memory Ligma's own process isn't using
    pub(super) fn trim() {
        unsafe {
            let _ = K32EmptyWorkingSet(GetCurrentProcess());
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::{c_char, c_void, CStr};
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    extern "C" {
        /// libmalloc: frees what the process's malloc zones hold unused
        fn malloc_zone_pressure_relief(zone: *mut c_void, goal: usize) -> usize;
    }

    pub(super) fn front(app: &tauri::AppHandle) -> Option<super::Front> {
        // AppKit keeps the app in front up to date on the main thread
        let (send, receive) = std::sync::mpsc::channel();
        app.run_on_main_thread(move || {
            let _ = send.send(objc2::rc::autoreleasepool(|_| unsafe { frontmost() }));
        })
        .ok()?;
        let (pid, game) = receive.recv_timeout(Duration::from_secs(1)).ok()??;
        Some(super::Front { pid, exe: exe_path(pid), game })
    }

    /// The app in front's process ID, and whether its bundle's category is
    /// a game one (`public.app-category.games`, `…action-games` and so on)
    unsafe fn frontmost() -> Option<(u32, bool)> {
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        let app: *mut AnyObject = msg_send![workspace, frontmostApplication];
        if app.is_null() {
            return None;
        }
        let pid: i32 = msg_send![app, processIdentifier];
        let url: *mut AnyObject = msg_send![app, bundleURL];
        let mut game = false;
        if !url.is_null() {
            let bundle: *mut AnyObject = msg_send![class!(NSBundle), bundleWithURL: url];
            if !bundle.is_null() {
                let key: *mut AnyObject =
                    msg_send![class!(NSString), stringWithUTF8String: c"LSApplicationCategoryType".as_ptr()];
                let category: *mut AnyObject = msg_send![bundle, objectForInfoDictionaryKey: key];
                let is_string: bool = !category.is_null() && msg_send![category, isKindOfClass: class!(NSString)];
                if is_string {
                    let utf8: *const c_char = msg_send![category, UTF8String];
                    if !utf8.is_null() {
                        let category = CStr::from_ptr(utf8).to_string_lossy();
                        game = category.starts_with("public.app-category.") && category.ends_with("games");
                    }
                }
            }
        }
        Some((u32::try_from(pid).ok()?, game))
    }

    pub(super) fn exe_path(pid: u32) -> Option<PathBuf> {
        let mut buffer = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let len = unsafe { libc::proc_pidpath(pid as i32, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
        (len > 0).then(|| std::ffi::OsStr::from_bytes(&buffer[..len as usize]).into())
    }

    /// macOS has no list of games besides the bundle category
    pub(super) fn known_game(_: &Path) -> bool {
        false
    }

    /// Gives back the memory Ligma's own process isn't using
    pub(super) fn trim() {
        unsafe {
            malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use std::path::{Path, PathBuf};

    pub(super) fn front(_: &tauri::AppHandle) -> Option<super::Front> {
        None
    }

    pub(super) fn exe_path(_: u32) -> Option<PathBuf> {
        None
    }

    pub(super) fn known_game(_: &Path) -> bool {
        false
    }

    pub(super) fn trim() {}
}
