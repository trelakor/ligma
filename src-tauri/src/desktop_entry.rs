//! `figma://` links on Linux, where Figma sends Google sign-in back to (see
//! auth.rs) and its "Open in desktop app" links point. There's no Figma
//! desktop app for Linux, so Ligma always handles them: a hidden desktop
//! entry that runs this Ligma (or its AppImage), made the scheme's default
//!
//! A link starts a second Ligma, which `tauri-plugin-single-instance` hands
//! to the running one (`auth::on_args`)

use std::path::PathBuf;
use std::process::{Command, Stdio};

const FILE_NAME: &str = "dev.ligma.client.figma.desktop";
const MIME_TYPE: &str = "x-scheme-handler/figma";

/// Where this Ligma runs from: the AppImage itself rather than its
/// temporary mount. `APPIMAGE` alone could be another AppImage's, passed
/// down to whatever it started
fn executable() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let app_image = std::env::var_os("APPDIR")
        .filter(|dir| exe.starts_with(dir))
        .and_then(|_| std::env::var_os("APPIMAGE"))
        .map(PathBuf::from);
    Some(app_image.unwrap_or(exe))
}

fn applications_dir() -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".local/share")))?;
    Some(data.join("applications"))
}

/// Points `figma://` at this Ligma for this user. Off the main thread: it
/// runs `xdg-mime`
pub fn register() {
    let (Some(exe), Some(dir)) = (executable(), applications_dir()) else {
        return;
    };
    // Quoted for the Exec key, as the desktop entry spec asks
    let exe = exe.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"").replace('`', "\\`").replace('$', "\\$");
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Ligma\nExec=\"{exe}\" %u\nIcon=ligma\nNoDisplay=true\nTerminal=false\nMimeType={MIME_TYPE};\n"
    );
    let path = dir.join(FILE_NAME);
    let changed = std::fs::read_to_string(&path).ok().as_deref() != Some(entry.as_str());
    if changed && (std::fs::create_dir_all(&dir).is_err() || std::fs::write(&path, entry).is_err()) {
        log::warn!("Couldn't write {}", path.display());
        return;
    }
    let current = Command::new("xdg-mime").args(["query", "default", MIME_TYPE]).stderr(Stdio::null()).output();
    if current.is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == FILE_NAME) && !changed {
        return;
    }
    let set = Command::new("xdg-mime").args(["default", FILE_NAME, MIME_TYPE]).status();
    if !set.is_ok_and(|status| status.success()) {
        log::warn!("Couldn't make Ligma handle figma:// links");
    }
    let _ = Command::new("update-desktop-database").arg(&dir).stderr(Stdio::null()).status();
}
