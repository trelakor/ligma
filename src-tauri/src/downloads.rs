//! Figma exports by clicking a `download` link. Without a download handler
//! the webview drops it, so ask where to save, like a browser would.
//!
//! Exporting several images at once, Figma's web app bundles them into one
//! zip. The desktop app saves them to a folder instead, so do the same: ask
//! for a folder, download the zip to a temporary file and unpack it there.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::webview::DownloadEvent;
use tauri::{Url, Webview};

/// Zipped exports on their way: the download's URL, its temporary file and
/// the folder to unpack it into.
static PENDING: Mutex<Vec<(Url, PathBuf, PathBuf)>> = Mutex::new(Vec::new());

pub fn on_download(webview: Webview, event: DownloadEvent) -> bool {
    // These run on the main thread inside the webview's callback, so use
    // synchronous dialogs; the dialog plugin's wait on the main thread.
    match event {
        DownloadEvent::Requested { url, destination } => {
            let window = webview.window();
            let mut dialog = rfd::FileDialog::new().set_parent(&window);
            if let Some(dir) = destination.parent() {
                dialog = dialog.set_directory(dir);
            }
            if is_export_zip(&url, destination) {
                let Some(folder) = dialog.set_title("Export to Folder").pick_folder() else {
                    return false;
                };
                let nanos = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |time| time.as_nanos());
                let temp = std::env::temp_dir().join(format!("ligma-export-{nanos}.zip"));
                PENDING.lock().unwrap().push((url, temp.clone(), folder));
                *destination = temp;
                return true;
            }
            if let Some(name) = destination.file_name() {
                dialog = dialog.set_file_name(name.to_string_lossy());
            }
            match dialog.save_file() {
                Some(path) => {
                    *destination = path;
                    true
                }
                None => false,
            }
        }
        DownloadEvent::Finished { url, success, .. } => {
            let pending = {
                let mut pending = PENDING.lock().unwrap();
                let index = pending.iter().position(|(pending, ..)| *pending == url);
                index.map(|index| pending.remove(index))
            };
            if let Some((_, temp, folder)) = pending {
                let result = if success { unpack(&temp, &folder) } else { Ok(()) };
                let _ = std::fs::remove_file(&temp);
                if let Err(error) = result {
                    rfd::MessageDialog::new()
                        .set_level(rfd::MessageLevel::Error)
                        .set_title("Export failed")
                        .set_description(format!("Couldn't save the images to {}: {error}", folder.display()))
                        .set_parent(&webview.window())
                        .show();
                }
            }
            true
        }
        _ => true,
    }
}

/// Figma builds its zips in the page, so they come from a blob: or data: URL.
fn is_export_zip(url: &Url, destination: &Path) -> bool {
    matches!(url.scheme(), "blob" | "data")
        && destination
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
}

/// Unpacks `zip` into `folder`, never replacing a file already there.
fn unpack(zip: &Path, folder: &Path) -> zip::result::ZipResult<()> {
    let mut archive = zip::ZipArchive::new(File::open(zip)?)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        // Skips folders, and paths that would land outside `folder`.
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

/// `path`, or "name (2).ext", "name (3).ext"… if it's taken.
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
