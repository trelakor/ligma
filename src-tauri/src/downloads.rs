//! Figma exports by clicking a `download` link. Without a download handler
//! the webview drops it, so ask where to save, like a browser would
//!
//! Exports built in the page (blob: and data: URLs, from Figma or a plugin)
//! are saved to a temporary file first and only asked about once they're
//! there, so the dialog can go by what was exported rather than the name
//! the webview guessed. That fixes the extension when the name lost it, and
//! a zip holding one file saves just that file. A zip of several, which
//! Figma's web app makes when exporting several images, is unpacked into a
//! folder like the desktop app does

use std::ffi::OsStr;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::webview::DownloadEvent;
use tauri::{Manager, Url, Webview};

/// An in-page export on its way to its temporary file
struct Pending {
    url: Url,
    temp: PathBuf,
    /// The name the webview suggested
    name: String,
    /// The folder the webview suggested
    dir: Option<PathBuf>,
}

static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());

pub fn on_download(webview: Webview, event: DownloadEvent) -> bool {
    // These run on the main thread inside the webview's callback, so use
    // synchronous dialogs; the dialog plugin's wait on the main thread
    match event {
        DownloadEvent::Requested { url, destination } => {
            if crate::handoff::on_requested(&webview, &url, destination) {
                return true;
            }
            let name = destination
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let dir = destination.parent().map(Path::to_owned);
            if matches!(url.scheme(), "blob" | "data") {
                let nanos = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |time| time.as_nanos());
                let temp = std::env::temp_dir().join(format!("ligma-export-{nanos}"));
                PENDING.lock().unwrap().push(Pending {
                    url,
                    temp: temp.clone(),
                    name,
                    dir,
                });
                *destination = temp;
                return true;
            }
            match save_dialog(&webview, &name, dir.as_deref()).save_file() {
                Some(path) => {
                    *destination = path;
                    true
                }
                None => false,
            }
        }
        DownloadEvent::Finished { url, success, .. } => {
            if crate::handoff::on_finished(&webview, &url, success) {
                return true;
            }
            let pending = {
                let mut pending = PENDING.lock().unwrap();
                let index = pending.iter().position(|pending| pending.url == url);
                index.map(|index| pending.remove(index))
            };
            if let Some(pending) = pending {
                let result = if success {
                    save(&webview, &pending)
                } else {
                    Ok(())
                };
                let _ = std::fs::remove_file(&pending.temp);
                if let Err(error) = result {
                    crate::prompt::show(
                        webview.app_handle(),
                        "Export Failed",
                        format!("Couldn't save the export: {error}"),
                        &["OK"],
                        |_| {},
                    );
                }
            }
            true
        }
        _ => true,
    }
}

/// Asks where to save an in-page export that's in its temporary file
fn save(webview: &Webview, pending: &Pending) -> std::io::Result<()> {
    let kind = sniff(&pending.temp)?;
    if kind == Some("zip") && extension(&pending.name).is_some_and(|ext| ext == "zip") {
        if let Ok(mut archive) = zip::ZipArchive::new(File::open(&pending.temp)?) {
            let files: Vec<usize> = (0..archive.len())
                .filter(|&index| {
                    archive
                        .by_index(index)
                        .is_ok_and(|entry| entry.is_file() && entry.enclosed_name().is_some())
                })
                .collect();
            if let [index] = files[..] {
                let mut entry = archive.by_index(index).map_err(std::io::Error::other)?;
                let name = entry
                    .enclosed_name()
                    .and_then(|path| {
                        path.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                    })
                    .unwrap_or_default();
                let Some(path) = save_dialog(webview, &name, pending.dir.as_deref()).save_file()
                else {
                    return Ok(());
                };
                std::io::copy(&mut entry, &mut File::create(path)?)?;
                return Ok(());
            }
            if files.len() > 1 {
                let mut dialog = rfd::FileDialog::new()
                    .set_parent(&webview.window())
                    .set_title("Export to Folder");
                if let Some(dir) = &pending.dir {
                    dialog = dialog.set_directory(dir);
                }
                let Some(folder) = dialog.pick_folder() else {
                    return Ok(());
                };
                return unpack(&mut archive, &folder).map_err(std::io::Error::other);
            }
        }
    }
    let name = with_extension(&pending.name, kind);
    let Some(path) = save_dialog(webview, &name, pending.dir.as_deref()).save_file() else {
        return Ok(());
    };
    std::fs::copy(&pending.temp, path).map(|_| ())
}

/// A save dialog for `name`, filtered to its type so Windows adds the
/// extension back if it's left off
fn save_dialog(webview: &Webview, name: &str, dir: Option<&Path>) -> rfd::FileDialog {
    let mut dialog = rfd::FileDialog::new().set_parent(&webview.window());
    if let Some(dir) = dir {
        dialog = dialog.set_directory(dir);
    }
    if !name.is_empty() {
        dialog = dialog.set_file_name(name);
    }
    if let Some(ext) = extension(name) {
        let label = match ext.as_str() {
            "png" => "PNG Image".to_owned(),
            "jpg" | "jpeg" => "JPEG Image".to_owned(),
            "svg" => "SVG Image".to_owned(),
            "pdf" => "PDF Document".to_owned(),
            "gif" => "GIF Image".to_owned(),
            "webp" => "WebP Image".to_owned(),
            "zip" => "Zip Archive".to_owned(),
            other => format!("{} File", other.to_uppercase()),
        };
        dialog = dialog
            .add_filter(label, &[ext.as_str()])
            .add_filter("All Files", &["*"]);
    }
    dialog
}

/// `name`'s extension, lowercased
fn extension(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(OsStr::to_str)
        .filter(|ext| !ext.is_empty() && !ext.contains(' '))
        .map(str::to_ascii_lowercase)
}

/// `name` with the extension for `kind`. A wrong one the webview may have
/// added, like ".txt", is replaced, anything else after a dot is kept as
/// part of the name, like "v1.5"
fn with_extension(name: &str, kind: Option<&str>) -> String {
    let Some(kind) = kind else {
        return name.to_owned();
    };
    let name = if name.is_empty() { "Export" } else { name };
    match extension(name) {
        Some(ext) if ext == kind || (kind == "jpg" && ext == "jpeg") => name.to_owned(),
        Some(ext)
            if matches!(
                ext.as_str(),
                "png"
                    | "jpg"
                    | "jpeg"
                    | "svg"
                    | "pdf"
                    | "gif"
                    | "webp"
                    | "zip"
                    | "txt"
                    | "bin"
                    | "htm"
                    | "html"
                    | "xml"
            ) =>
        {
            format!("{}.{kind}", &name[..name.len() - ext.len() - 1])
        }
        _ => format!("{name}.{kind}"),
    }
}

/// What the file holds, as its usual extension, going by its first bytes
fn sniff(path: &Path) -> std::io::Result<Option<&'static str>> {
    let mut head = Vec::with_capacity(1024);
    File::open(path)?.take(1024).read_to_end(&mut head)?;
    let kind = if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if head.starts_with(b"\xff\xd8\xff") {
        Some("jpg")
    } else if head.starts_with(b"%PDF") {
        Some("pdf")
    } else if head.starts_with(b"GIF8") {
        Some("gif")
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("webp")
    } else if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        Some("zip")
    } else if String::from_utf8_lossy(&head).contains("<svg") {
        Some("svg")
    } else {
        None
    };
    Ok(kind)
}

/// Unpacks `archive` into `folder`, never replacing a file already there
fn unpack(archive: &mut zip::ZipArchive<File>, folder: &Path) -> zip::result::ZipResult<()> {
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        // Skips folders, and paths that would land outside `folder`
        let Some(name) = entry.enclosed_name().filter(|_| entry.is_file()) else {
            continue;
        };
        let path = free_path(&folder.join(name));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::io::copy(&mut entry, &mut File::create_new(&path)?)?;
    }
    Ok(())
}

/// `path`, or "name (2).ext", "name (3).ext"… if it's taken
fn free_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_owned();
    }
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| path.with_file_name(format!("{stem} ({n}){ext}")))
        .find(|path| !path.exists())
        .unwrap()
}
