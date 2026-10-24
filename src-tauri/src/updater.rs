//! Auto-update from GitHub Releases. On launch and every 6 hours, Ligma
//! fetches `latest.json` from the latest release (written by
//! `.github/workflows/release.yml`), downloads a newer version in the
//! background and asks to restart. "Later" installs it on quit, so the next
//! launch runs the new version. Packages are verified against `pubkey` in
//! `tauri.conf.json`. The app is replaced in place, so its data (sign-in,
//! settings) stays. Ligma > Check for Updates… does the same on demand.
//! If installing fails, Ligma says why and links to the release to install
//! by hand, rather than carrying on as the old version.

use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::{Update, UpdaterExt};

const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

const RELEASES: &str = "https://github.com/trelakor/ligma/releases/latest";

/// Why installing on quit failed, kept in the config folder to say so on
/// the next launch.
const FAILED_FILE: &str = "update-failed.txt";

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
    if let Some(error) = take_quit_failure(app) {
        install_failed(app, error);
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
    // On Windows the installer quits Ligma without the usual exit, so this
    // does its cleanup first.
    let handle = app.clone();
    let updater = app
        .updater_builder()
        .on_before_exit(move || {
            crate::shut_down(&handle);
            handle.cleanup_before_exit();
        })
        .build()?;
    let Some(update) = updater.check().await? else {
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
            if !restart {
                return;
            }
            // On Windows a successful install quits Ligma, and the installer
            // starts the new version.
            match install(&handle) {
                Ok(()) => handle.restart(),
                Err(error) => install_failed(&handle, format!("Couldn't install Ligma {version}: {error}")),
            }
        });
}

/// Says why an update couldn't be installed and offers its download page.
fn install_failed(app: &AppHandle, error: String) {
    let handle = app.clone();
    app.dialog()
        .message(format!(
            "{error}

Download the latest version and run its installer. Your sign-in and settings stay."
        ))
        .title("Update Failed")
        .kind(MessageDialogKind::Error)
        .buttons(MessageDialogButtons::OkCancelCustom("Download".into(), "Close".into()))
        .show(move |download| {
            if download {
                let _ = handle.opener().open_url(RELEASES, None::<&str>);
            }
        });
}

fn tell(app: &AppHandle, message: String) {
    app.dialog().message(message).title("Check for Updates").show(|_| {});
}

/// Installs the downloaded update, if there is one. On macOS this replaces
/// the app bundle; on Windows it runs the installer, which quits Ligma, so it
/// only returns if that failed. A failed update stays pending, so Check for
/// Updates… offers it again.
fn install(app: &AppHandle) -> tauri_plugin_updater::Result<()> {
    let pending = app.state::<Pending>().0.lock().unwrap().take();
    let Some((update, bytes)) = pending else {
        return Ok(());
    };
    let result = update.install(&bytes);
    if result.is_err() {
        *app.state::<Pending>().0.lock().unwrap() = Some((update, bytes));
    }
    result
}

/// Installs an update the user chose to install later. Ligma is quitting
/// and can't show a dialog, so a failure is shown on the next launch.
pub fn install_on_quit(app: &AppHandle) {
    let version = match app.state::<Pending>().0.lock().unwrap().as_ref() {
        Some((update, _)) => update.version.clone(),
        None => return,
    };
    if let Err(error) = install(app) {
        if let Ok(dir) = app.path().app_config_dir() {
            let message = format!("Ligma {version} couldn't be installed when you quit: {error}");
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join(FAILED_FILE), message);
        }
    }
}

/// The reason a quit-time install failed, if one did since the last launch.
fn take_quit_failure(app: &AppHandle) -> Option<String> {
    let path = app.path().app_config_dir().ok()?.join(FAILED_FILE);
    let error = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(path);
    // The update that failed may be the version running now, installed by hand.
    let version = app.package_info().version.to_string();
    (!error.starts_with(&format!("Ligma {version} "))).then_some(error)
}
