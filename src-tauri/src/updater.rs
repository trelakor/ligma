//! Auto-update from GitHub Releases. On launch and every 6 hours, Ligma
//! fetches `latest.json` from the latest release (written by
//! `.github/workflows/release.yml`), downloads a newer version in the
//! background and asks to restart. "Later" installs it on quit, so the next
//! launch runs the new version. Packages are verified against `pubkey` in
//! `tauri.conf.json`. The app is replaced in place, so its data (sign-in,
//! settings) stays.

use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use tauri_plugin_updater::{Update, UpdaterExt};

const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// An update that's downloaded but not installed yet, with its package.
#[derive(Default)]
pub struct Pending(Mutex<Option<(Update, Vec<u8>)>>);

pub fn start(app: &AppHandle) {
    app.manage(Pending::default());
    // Debug builds would replace themselves with the release build.
    if cfg!(debug_assertions) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || loop {
        // Once one is downloaded, wait for it to be installed.
        if app.state::<Pending>().0.lock().unwrap().is_none() {
            // Offline or no release reachable: try again next time.
            let _ = tauri::async_runtime::block_on(check(&app));
        }
        std::thread::sleep(CHECK_EVERY);
    });
}

async fn check(app: &AppHandle) -> tauri_plugin_updater::Result<()> {
    let Some(update) = app.updater()?.check().await? else {
        return Ok(());
    };
    let bytes = update.download(|_, _| {}, || {}).await?;
    let version = update.version.clone();
    *app.state::<Pending>().0.lock().unwrap() = Some((update, bytes));

    let handle = app.clone();
    app.dialog()
        .message(format!(
            "Ligma {version} is ready. Restart now to use it, or it will be installed when you quit."
        ))
        .title("Update Available")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Restart Now".into(),
            "Later".into(),
        ))
        .show(move |restart| {
            if restart {
                install(&handle);
                handle.restart();
            }
        });
    Ok(())
}

/// Installs the downloaded update, if there is one. On macOS this replaces
/// the app bundle; on Windows it runs the installer, which quits Ligma.
pub fn install(app: &AppHandle) {
    let pending = app.state::<Pending>().0.lock().unwrap().take();
    if let Some((update, bytes)) = pending {
        let _ = update.install(bytes);
    }
}
