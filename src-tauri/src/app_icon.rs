//! The app icon picked in Settings > App Icon (dist/app-icons.js)
//!
//! The settings window draws the picked icon into a PNG and hands it over
//! with `set_app_icon`. It's kept as `app-icon.png` in the app config folder
//! and put back on every launch, so the PNG is the only copy of the icon the
//! app needs. "ligma" (`DEFAULT`) is the icon Ligma ships with
//!
//! On macOS the icon goes in the Dock while Ligma runs
//! (`setApplicationIconImage:`), and on Ligma.app as a custom icon, as
//! Finder's Get Info sets it, so Finder, Launchpad and the Dock show it when
//! Ligma is closed. An update replaces the app and the custom icon with it,
//! so it's set again on the next launch. Without write access to the app,
//! only the Dock changes
//!
//! On Windows the icon goes on Ligma's windows, for the title bar and
//! Alt+Tab, and on its Start menu, desktop and pinned taskbar shortcuts,
//! which Windows 11's taskbar takes the button's icon from (`shortcuts`)

use std::path::PathBuf;

use tauri::ipc::{InvokeBody, Request};
use tauri::{AppHandle, Manager, Webview};

pub const DEFAULT: &str = "ligma";
const FILE: &str = "app-icon.png";

fn path(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_config_dir().ok()?.join(FILE))
}

/// The saved icon, if one other than the default is picked
fn saved(app: &AppHandle, id: &str) -> Option<Vec<u8>> {
    if id == DEFAULT {
        return None;
    }
    std::fs::read(path(app)?).ok()
}

/// Puts the picked icon back, at launch
pub fn init(app: &AppHandle, settings: &crate::settings::Settings) {
    apply(app, saved(app, &settings.app_icon), false);
}

/// From the settings window: the icon's id in the `Ligma-Icon` header and
/// its PNG as the body. No body (or the default's id) goes back to the
/// default
#[tauri::command]
pub fn set_app_icon(app: AppHandle, webview: Webview, request: Request<'_>) -> Result<(), String> {
    if !crate::settings::from_settings_window(&webview) {
        return Err("not allowed".into());
    }
    let id = request.headers().get("ligma-icon").and_then(|id| id.to_str().ok()).unwrap_or(DEFAULT);
    let png = match request.body() {
        InvokeBody::Raw(bytes) if id != DEFAULT && !bytes.is_empty() => Some(bytes.clone()),
        _ => None,
    };
    let id = if png.is_some() { id.to_owned() } else { DEFAULT.to_owned() };
    let file = path(&app).ok_or("no config folder")?;
    match &png {
        Some(png) => {
            std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::write(&file, png).map_err(|e| e.to_string())?;
        }
        None => {
            let _ = std::fs::remove_file(&file);
        }
    }
    let mut settings = crate::settings::current(&app);
    settings.app_icon = id;
    crate::settings::change(&app, settings)?;
    apply(&app, png, true);
    Ok(())
}

#[cfg(target_os = "macos")]
fn apply(app: &AppHandle, png: Option<Vec<u8>>, changed: bool) {
    let _ = app.run_on_main_thread(move || unsafe { macos::apply(png.as_deref(), changed) });
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{c_void, CString};
    use std::path::PathBuf;

    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    /// Ligma.app, when running from one
    fn bundle() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let bundle = exe.parent()?.parent()?.parent()?.to_path_buf();
        (bundle.extension()? == "app").then_some(bundle)
    }

    /// Shows `png` in the Dock, or Ligma's own icon without one, and on the
    /// app when `changed` or when the app's custom icon doesn't match, as
    /// after an update. Main thread only
    pub unsafe fn apply(png: Option<&[u8]>, changed: bool) {
        let image: *mut AnyObject = match png {
            Some(png) => {
                let data: *mut AnyObject =
                    msg_send![class!(NSData), dataWithBytes: png.as_ptr().cast::<c_void>(), length: png.len()];
                let image: *mut AnyObject = msg_send![class!(NSImage), alloc];
                msg_send![image, initWithData: data]
            }
            None => std::ptr::null_mut(),
        };
        if png.is_some() && image.is_null() {
            log::warn!("app icon: not an image");
            return;
        }
        let ns_app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![ns_app, setApplicationIconImage: image];

        if let Some(bundle) = bundle() {
            // Finder keeps a custom icon in this file inside the app
            let has_custom = bundle.join("Icon\r").exists();
            if changed || has_custom != !image.is_null() {
                if let Ok(path) = CString::new(bundle.to_string_lossy().as_bytes()) {
                    let path: *mut AnyObject = msg_send![class!(NSString), stringWithUTF8String: path.as_ptr()];
                    let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
                    let set: bool = msg_send![workspace, setIcon: image, forFile: path, options: 0usize];
                    if !set {
                        log::warn!("app icon: couldn't set it on {}", bundle.display());
                    }
                }
            }
        }
        if !image.is_null() {
            let _: () = msg_send![image, release];
        }
    }
}

#[cfg(windows)]
fn apply(app: &AppHandle, png: Option<Vec<u8>>, _changed: bool) {
    win::set(png.as_deref());
    for window in app.webview_windows().values() {
        apply_to(window);
    }
    if let Ok(dir) = app.path().app_config_dir() {
        std::thread::spawn(move || shortcuts::set(&dir, png.as_deref()));
    }
}

/// Gives a new window the picked icon
#[cfg(windows)]
pub fn apply_to(window: &tauri::WebviewWindow) {
    win::apply_to(window);
}

#[cfg(windows)]
mod win {
    use std::sync::Mutex;

    use image::imageops::FilterType;
    use tauri::image::Image;
    use tauri::Manager;

    #[link(name = "user32")]
    extern "system" {
        fn CreateIcon(instance: isize, width: i32, height: i32, planes: u8, bits: u8, and: *const u8, xor: *const u8) -> isize;
        fn DestroyIcon(icon: isize) -> i32;
        fn SendMessageW(hwnd: isize, message: u32, wparam: usize, lparam: isize) -> isize;
    }
    const WM_SETICON: u32 = 0x0080;
    const ICON_BIG: usize = 1;

    /// The picked icon: the small one for the title bar and Alt+Tab, set
    /// through Tauri, and the big one the taskbar shows, an HICON
    struct Icon {
        small: Image<'static>,
        big: isize,
    }

    static CURRENT: Mutex<Option<Icon>> = Mutex::new(None);

    /// An HICON of `rgba`, `size` square, as tao makes them
    unsafe fn hicon(rgba: &[u8], size: u32) -> isize {
        let bgra: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
        let and: Vec<u8> = rgba.chunks_exact(4).map(|p| p[3].wrapping_sub(u8::MAX)).collect();
        CreateIcon(0, size as i32, size as i32, 1, 32, and.as_ptr(), bgra.as_ptr())
    }

    pub fn set(png: Option<&[u8]>) {
        let icon = png.and_then(|png| image::load_from_memory(png).ok()).map(|picture| {
            let resized = |size| picture.resize_exact(size, size, FilterType::Lanczos3).into_rgba8().into_raw();
            let small = Image::new_owned(resized(32), 32, 32);
            let big = unsafe { hicon(&resized(256), 256) };
            Icon { small, big }
        });
        let old = std::mem::replace(&mut *CURRENT.lock().unwrap(), icon);
        // After the windows have the new one
        if let Some(old) = old {
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(1));
                unsafe { DestroyIcon(old.big) };
            });
        }
    }

    pub fn apply_to(window: &tauri::WebviewWindow) {
        let Ok(hwnd) = window.hwnd() else {
            return;
        };
        let current = CURRENT.lock().unwrap();
        let (small, big) = match &*current {
            Some(icon) => (Some(icon.small.clone()), icon.big),
            None => (window.app_handle().default_window_icon().cloned(), 0),
        };
        if let Some(small) = small {
            let _ = window.set_icon(small);
        }
        unsafe { SendMessageW(hwnd.0 as isize, WM_SETICON, ICON_BIG, big) };
    }
}

/// Windows 11's own taskbar draws Ligma's button with the icon of the
/// shortcut it belongs to (the Start menu one carries Ligma's app ID), not
/// the window's, pinned or not. So the picked icon also goes on Ligma's
/// shortcuts as an .ico, as Properties > Change Icon does. The installer
/// makes the Start menu shortcut again on every update, so it runs on every
/// launch, and only saves shortcuts whose icon is different
#[cfg(windows)]
mod shortcuts {
    use std::hash::{Hash, Hasher};
    use std::path::{Path, PathBuf};

    use image::imageops::FilterType;
    use windows::core::{Interface, GUID, HSTRING, PCWSTR};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READWRITE};
    use windows::Win32::UI::Shell::{
        IShellLinkW, SHChangeNotify, SHGetKnownFolderPath, ShellLink, FOLDERID_CommonPrograms, FOLDERID_Desktop, FOLDERID_Programs, FOLDERID_RoamingAppData,
        KF_FLAG_DEFAULT, SHCNE_ASSOCCHANGED, SHCNE_UPDATEITEM, SHCNF_IDLIST, SHCNF_PATHW,
    };

    const PREFIX: &str = "app-icon-";

    pub fn set(dir: &Path, png: Option<&[u8]>) {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let icon = png.and_then(|png| write(dir, png));
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let mut changed = false;
            for link in links() {
                changed |= update(&link, &exe, icon.as_deref()).unwrap_or(false);
            }
            if changed {
                SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
            }
        }
        // Old icons, once no shortcut points at them
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(PREFIX) && name.ends_with(".ico") && Some(&path) != icon.as_ref() {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    /// The icon as an .ico beside app-icon.png, named after its contents
    /// so Windows' icon cache can't show an older one from the same path
    fn write(dir: &Path, png: &[u8]) -> Option<PathBuf> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        png.hash(&mut hasher);
        let path = dir.join(format!("{PREFIX}{:016x}.ico", hasher.finish()));
        if path.exists() {
            return Some(path);
        }
        std::fs::write(&path, ico(png)?).ok()?;
        Some(path)
    }

    /// An .ico of PNG images, one per size the shell asks for
    fn ico(png: &[u8]) -> Option<Vec<u8>> {
        let picture = image::load_from_memory(png).ok()?;
        let mut images = Vec::new();
        for size in [16, 24, 32, 48, 64, 256] {
            let mut data = Vec::new();
            picture
                .resize_exact(size, size, FilterType::Lanczos3)
                .write_to(&mut std::io::Cursor::new(&mut data), image::ImageFormat::Png)
                .ok()?;
            images.push((size, data));
        }
        let mut out = [0u16, 1, images.len() as u16].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>();
        let mut offset = 6 + 16 * images.len();
        for (size, data) in &images {
            // 0 means 256
            let side = if *size >= 256 { 0 } else { *size as u8 };
            out.extend([side, side, 0, 0]);
            out.extend(1u16.to_le_bytes());
            out.extend(32u16.to_le_bytes());
            out.extend((data.len() as u32).to_le_bytes());
            out.extend((offset as u32).to_le_bytes());
            offset += data.len();
        }
        for (_, data) in images {
            out.extend(data);
        }
        Some(out)
    }

    unsafe fn known(id: &GUID) -> Option<PathBuf> {
        let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let out = path.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(path.0 as _));
        out
    }

    /// Every shortcut that could be Ligma's: the Start menu, the desktop and
    /// the taskbar's pins
    unsafe fn links() -> Vec<PathBuf> {
        let mut found = Vec::new();
        for (id, deep) in [(&FOLDERID_Programs, true), (&FOLDERID_CommonPrograms, true), (&FOLDERID_Desktop, false)] {
            if let Some(dir) = known(id) {
                collect(&dir, deep, &mut found);
            }
        }
        if let Some(roaming) = known(&FOLDERID_RoamingAppData) {
            collect(&roaming.join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar"), false, &mut found);
        }
        found
    }

    fn collect(dir: &Path, deep: bool, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if deep && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                collect(&path, deep, found);
            } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("lnk")) {
                found.push(path);
            }
        }
    }

    /// Points `link` at `icon`, or back at Ligma's own with none, if it
    /// opens `exe`. Whether it was saved
    unsafe fn update(link: &Path, exe: &Path, icon: Option<&Path>) -> windows::core::Result<bool> {
        let shell_link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let file: IPersistFile = shell_link.cast()?;
        let path = HSTRING::from(link.as_os_str());
        file.Load(&path, STGM_READWRITE)?;
        let text = |buffer: &[u16]| String::from_utf16_lossy(&buffer[..buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len())]);
        let mut target = [0u16; 1024];
        shell_link.GetPath(&mut target, std::ptr::null_mut(), 0)?;
        if !text(&target).eq_ignore_ascii_case(&exe.to_string_lossy()) {
            return Ok(false);
        }
        let want = icon.map(|icon| icon.to_string_lossy().into_owned()).unwrap_or_default();
        let mut current = [0u16; 1024];
        let mut index = 0;
        shell_link.GetIconLocation(&mut current, &mut index)?;
        if text(&current).eq_ignore_ascii_case(&want) && index == 0 {
            return Ok(false);
        }
        shell_link.SetIconLocation(&HSTRING::from(want), 0)?;
        file.Save(PCWSTR::null(), true)?;
        SHChangeNotify(SHCNE_UPDATEITEM, SHCNF_PATHW, Some(path.as_ptr() as _), None);
        Ok(true)
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn apply(_app: &AppHandle, _png: Option<Vec<u8>>, _changed: bool) {}
