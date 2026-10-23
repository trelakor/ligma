//! Auto-update from GitHub Releases. On launch and every 6 hours, Ligma
//! fetches `latest.json` from the latest release (written by
//! `.github/workflows/release.yml`), downloads a newer version in the
//! background and asks to restart. "Later" installs it on quit, so the next
//! launch runs the new version. Packages are verified against `pubkey` in
//! `tauri.conf.json`. The app is replaced in place, so its data (sign-in,
//! settings) stays. Ligma > Check for Updates… does the same on demand.

use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use tauri_plugin_updater::{Update, UpdaterExt};

const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// An update that's downloaded but not installed yet, with its package.
#[derive(Default)]
pub struct Pending(Mutex<Option<(Update, Vec<u8>)>>);

/// Held while checking, so a manual check waits for a background one
/// instead of downloading the same update twice.
static CHECKING: Mutex<()> = Mutex::new(());

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
            if let Ok(true) = tauri::async_runtime::block_on(check(&app)) {
                ask_to_restart(&app);
            }
        }
        std::thread::sleep(CHECK_EVERY);
    });
}

/// Checks right away, for the menu item, and says what it found.
pub fn check_now(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        if cfg!(debug_assertions) {
            tell(&app, "Updates are off in debug builds.".into());
            return;
        }
        match tauri::async_runtime::block_on(check(&app)) {
            Ok(true) => ask_to_restart(&app),
            Ok(false) => {
                let version = app.package_info().version.to_string();
                tell(&app, format!("Ligma {version} is the latest version."));
            }
            Err(error) => tell(&app, format!("Couldn't check for updates: {error}")),
        }
    });
}

/// Downloads a newer version, if there is one. True once one is pending.
async fn check(app: &AppHandle) -> tauri_plugin_updater::Result<bool> {
    let _checking = CHECKING.lock().unwrap();
    if app.state::<Pending>().0.lock().unwrap().is_some() {
        return Ok(true);
    }
    let Some(update) = app.updater()?.check().await? else {
        return Ok(false);
    };
    let bytes = update.download(|_, _| {}, || {}).await?;
    *app.state::<Pending>().0.lock().unwrap() = Some((update, bytes));
    Ok(true)
}

fn ask_to_restart(app: &AppHandle) {
    let Some(version) = app
        .state::<Pending>()
        .0
        .lock()
        .unwrap()
        .as_ref()
        .map(|(update, _)| update.version.clone())
    else {
        return;
    };
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
}

fn tell(app: &AppHandle, message: String) {
    app.dialog().message(message).title("Check for Updates").show(|_| {});
}

/// Installs the downloaded update, if there is one. On macOS this replaces
/// the app bundle; on Windows it runs the installer, which quits Ligma.
pub fn install(app: &AppHandle) {
    let pending = app.state::<Pending>().0.lock().unwrap().take();
    if let Some((update, bytes)) = pending {
        let _ = update.install(bytes);
    }
}
