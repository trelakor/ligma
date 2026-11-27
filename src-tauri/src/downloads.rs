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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::webview::DownloadEvent;
use tauri::{Manager, Url, Webview};

/// An in-page export on its way to its temporary file
struct Pending {
    url: Url,
    /// Unique to this download, so it's how a finished download is matched
    /// where the webview says where it saved it
    temp: PathBuf,
    /// The name the webview suggested
    name: String,
    /// The folder the webview suggested
    dir: Option<PathBuf>,
}

static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());

/// Extensions a webview may put on an export it couldn't name, which say
/// nothing about what's in it
const GENERIC: [&str; 6] = ["txt", "bin", "htm", "html", "xml", "dat"];

/// The extensions `sniff` knows
const KNOWN: [&str; 8] = ["png", "jpg", "jpeg", "svg", "pdf", "gif", "webp", "zip"];

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
                let temp = temp_path();
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
        DownloadEvent::Finished { url, path, success } => {
            if crate::handoff::on_finished(&webview, &url, success) {
                return true;
            }
            let Some(pending) = take_pending(&url, path.as_deref()) else {
                return true;
            };
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
            true
        }
        _ => true,
    }
}

/// A temporary file no other download has
fn temp_path() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_nanos());
    let next = NEXT.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("ligma-export-{nanos}-{next}"))
}

/// The pending export a finished download is for. Windows says where it saved
/// a finished download, which is the export's own temporary file. When it
/// doesn't, after a failure or always on macOS, the URL is all there is, so
/// the oldest export from that URL is taken. Two at once from one blob: or
/// data: URL hold the same bytes, so only their suggested names can mix up
fn take_pending(url: &Url, path: Option<&Path>) -> Option<Pending> {
    let mut pending = PENDING.lock().unwrap();
    let index = path
        .filter(|path| !path.as_os_str().is_empty())
        .and_then(|path| pending.iter().position(|pending| pending.temp == path))
        .or_else(|| pending.iter().position(|pending| pending.url == *url))?;
    Some(pending.remove(index))
}

/// What a zip export holds
enum Contents {
    /// One file, by its index
    One(usize),
    /// Several files
    Several,
    /// Nothing to pull out, or something that can't be read or would land
    /// outside the folder, so the zip is saved as it is
    Archive,
}

fn contents(archive: &mut zip::ZipArchive<File>) -> Contents {
    let mut files = Vec::new();
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            return Contents::Archive;
        };
        if entry.is_dir() {
            continue;
        }
        if !entry.is_file() || entry.enclosed_name().is_none() {
            return Contents::Archive;
        }
        files.push(index);
    }
    match files[..] {
        [] => Contents::Archive,
        [index] => Contents::One(index),
        _ => Contents::Several,
    }
}

/// Asks where to save an in-page export that's in its temporary file
fn save(webview: &Webview, pending: &Pending) -> std::io::Result<()> {
    let kind = sniff(&pending.temp)?;
    let ext = extension(&pending.name);
    // Formats like .docx and .sketch are zips too, so only a zip that's
    // named as one, or named as nothing in particular, is opened up
    let export_zip = kind == Some("zip")
        && ext
            .as_deref()
            .is_none_or(|ext| ext == "zip" || GENERIC.contains(&ext));
    if export_zip {
        if let Ok(mut archive) = zip::ZipArchive::new(File::open(&pending.temp)?) {
            match contents(&mut archive) {
                Contents::One(index) => {
                    let mut entry = archive.by_index(index).map_err(std::io::Error::other)?;
                    let name = entry
                        .enclosed_name()
                        .and_then(|path| {
                            path.file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                        })
                        .unwrap_or_default();
                    let dialog = save_dialog(webview, &name, pending.dir.as_deref());
                    let Some(path) = dialog.save_file() else {
                        return Ok(());
                    };
                    return write(&path, |file| std::io::copy(&mut entry, file).map(|_| ()));
                }
                Contents::Several => {
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
                Contents::Archive => {}
            }
        }
    }
    let name = with_extension(&pending.name, kind);
    let Some(path) = save_dialog(webview, &name, pending.dir.as_deref()).save_file() else {
        return Ok(());
    };
    write(&path, |file| {
        std::io::copy(&mut File::open(&pending.temp)?, file).map(|_| ())
    })
}

/// Writes `path` through a file beside it, so a failed write neither leaves
/// half a file nor spoils one it was replacing. The file beside it is always
/// a new one, so nothing already there is touched
fn write(path: &Path, fill: impl FnOnce(&mut File) -> std::io::Result<()>) -> std::io::Result<()> {
    let (partial, mut file) = (1..=100)
        .find_map(|n| {
            let mut partial = path.as_os_str().to_owned();
            partial.push(format!(".ligma-partial-{n}"));
            let partial = PathBuf::from(partial);
            match File::create_new(&partial) {
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                result => Some(result.map(|file| (partial, file))),
            }
        })
        .unwrap_or_else(|| Err(std::io::ErrorKind::AlreadyExists.into()))?;
    // The file is closed before it's renamed, which Windows needs
    let written = fill(&mut file).and_then(|()| file.sync_all());
    drop(file);
    let result = written.and_then(|()| std::fs::rename(&partial, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
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
/// part of the name, like "v1.5". A zip keeps an extension `sniff` doesn't
/// know, since formats like .docx are zips
fn with_extension(name: &str, kind: Option<&str>) -> String {
    let Some(kind) = kind else {
        return name.to_owned();
    };
    let name = if name.is_empty() { "Export" } else { name };
    match extension(name) {
        Some(ext) if ext == kind || (kind == "jpg" && ext == "jpeg") => name.to_owned(),
        Some(ext) if KNOWN.contains(&ext.as_str()) || GENERIC.contains(&ext.as_str()) => {
            format!("{}.{kind}", &name[..name.len() - ext.len() - 1])
        }
        Some(_) if kind == "zip" => name.to_owned(),
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
        let mut file = File::create_new(&path)?;
        if let Err(error) = std::io::copy(&mut entry, &mut file) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(error.into());
        }
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

#[cfg(test)]
mod tests {
    use super::with_extension;

    #[test]
    fn names_follow_the_contents() {
        assert_eq!(with_extension("Frame 1", Some("png")), "Frame 1.png");
        assert_eq!(with_extension("Frame 1.txt", Some("png")), "Frame 1.png");
        assert_eq!(with_extension("Frame 1.jpeg", Some("jpg")), "Frame 1.jpeg");
        assert_eq!(with_extension("v1.5", Some("png")), "v1.5.png");
        assert_eq!(with_extension("Export", Some("zip")), "Export.zip");
        assert_eq!(with_extension("Report.docx", Some("zip")), "Report.docx");
        assert_eq!(with_extension("", Some("svg")), "Export.svg");
        assert_eq!(with_extension("Notes", None), "Notes");
    }
}
