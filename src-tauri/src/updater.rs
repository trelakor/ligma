//! Auto-update from GitHub Releases. On launch and every 6 hours, Ligma
//! fetches `latest.json` from the latest release (written by
//! `.github/workflows/release.yml`), downloads a newer version in the
//! background and asks to restart. "Later" installs it on quit, so the next
//! launch runs the new version. Packages are verified against `pubkey` in
//! `tauri.conf.json`. The app is replaced in place, so its data (sign-in,
//! settings) stays. Ligma > Check for Updates… does the same on demand.
//! If installing fails, Ligma says why, links to the release to install by
//! hand and offers a report (report.rs), rather than carrying on as the old
//! version. Each step is logged. On Windows, Ligma winds down (hiding its
//! windows) before it starts the installer; if Windows then won't start it
//! (an antivirus or Smart App Control can refuse an unsigned installer),
//! Ligma restarts itself to say so rather than staying open unseen

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::{Update, UpdaterExt};

const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

const RELEASES: &str = "https://github.com/trelakor/ligma/releases/latest";

/// Why installing failed, kept in the config folder to say so on the next
/// launch: on quit, or when Ligma restarted after Windows wouldn't start the
/// installer
const FAILED_FILE: &str = "update-failed.txt";

/// Ligma has wound down for the installer (`on_before_exit`): its windows
/// are hidden and Tauri can't be used any more, so a failure from here on
/// is shown after a restart
static WOUND_DOWN: AtomicBool = AtomicBool::new(false);

/// Why the last update failed to install, kept by `start` for
/// `offer_last_failure` to show once the main window is up
static LAST_FAILURE: Mutex<Option<String>> = Mutex::new(None);

/// An update that's downloaded but not installed yet, with its package
#[derive(Default)]
pub struct Pending(Mutex<Option<(Update, Vec<u8>)>>);

/// Held while checking, so a manual check waits for a background one
/// instead of downloading the same update twice
static CHECKING: Mutex<()> = Mutex::new(());

pub fn start(app: &AppHandle) {
    app.manage(Pending::default());
    // Debug builds would replace themselves with the release build
    if cfg!(debug_assertions) {
        return;
    }
    // After a failure, don't offer the same update again straight away
    let mut skip_check = false;
    if let Some(error) = take_failure(app) {
        *LAST_FAILURE.lock().unwrap() = Some(error);
        skip_check = true;
    }
    let app = app.clone();
    std::thread::spawn(move || loop {
        // Once one is downloaded, wait for it to be installed
        if !std::mem::take(&mut skip_check) && app.state::<Pending>().0.lock().unwrap().is_none() {
            // Offline or no release reachable: try again next time
            match tauri::async_runtime::block_on(check(&app)) {
                Ok(true) => ask_to_restart(&app),
                Ok(false) => {}
                Err(error) => log::warn!("Couldn't check for updates: {error}"),
            }
        }
        std::thread::sleep(CHECK_EVERY);
    });
}

/// Says why the last update failed to install, if it did. Call once the
/// main window is up, so the prompt is centered over it
pub fn offer_last_failure(app: &AppHandle) {
    if let Some(error) = LAST_FAILURE.lock().unwrap().take() {
        install_failed(app, error);
    }
}

/// Checks right away, for the menu item, and says what it found
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
            Err(error) => {
                let error = format!("Couldn't check for updates: {error}");
                log::warn!("{error}");
                crate::report::offer(&app, "Check for Updates", error.clone(), error);
            }
        }
    });
}

/// Downloads a newer version, if there is one. True once one is pending
async fn check(app: &AppHandle) -> tauri_plugin_updater::Result<bool> {
    let _checking = CHECKING.lock().unwrap();
    if app.state::<Pending>().0.lock().unwrap().is_some() {
        return Ok(true);
    }
    // On Windows the installer quits Ligma without the usual exit, so this
    // does its cleanup first
    let handle = app.clone();
    let updater = app
        .updater_builder()
        .on_before_exit(move || {
            WOUND_DOWN.store(true, Ordering::SeqCst);
            crate::shut_down(&handle);
            handle.cleanup_before_exit();
        })
        .build()?;
    let Some(update) = updater.check().await? else {
        return Ok(false);
    };
    log::info!("Downloading Ligma {}", update.version);
    let bytes = update.download(|_, _| {}, || {}).await?;
    log::info!("Downloaded Ligma {}", update.version);
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
    crate::prompt::show(
        app,
        "Update Available",
        format!("Ligma {version} is ready. Restart now to use it, or it will be installed when you quit."),
        &["Later", "Restart Now"],
        move |choice| {
            if choice != Some(1) {
                return;
            }
            // On Windows a successful install quits Ligma, and the installer
            // starts the new version
            match install(&handle) {
                Ok(()) => relaunch(&handle),
                Err(error) => {
                    let error = format!("Couldn't install Ligma {version}: {error}");
                    if WOUND_DOWN.load(Ordering::SeqCst) {
                        // The windows are hidden and Tauri is shut down:
                        // say so from a fresh start instead
                        log::error!("{error}");
                        remember_failure(&handle, &error);
                        handle.state::<Pending>().0.lock().unwrap().take();
                        handle.restart();
                    }
                    install_failed(&handle, error);
                }
            }
        },
    );
}

/// Quits and starts the new version. On macOS, Tauri's restart opens the
/// app again with `open -n` while this one still runs, and the Dock gives
/// the new one a tile of its own beside the one kept in the Dock. Instead a
/// detached shell waits for Ligma to quit, then opens the app the usual
/// way, into its Dock tile
fn relaunch(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    if let Some(bundle) = bundle_path() {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        let spawned = Command::new("/bin/sh")
            .arg("-c")
            .arg(r#"while kill -0 "$1" 2>/dev/null; do sleep 0.1; done; exec /usr/bin/open "$2""#)
            .arg("sh")
            .arg(std::process::id().to_string())
            .arg(&bundle)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn();
        match spawned {
            Ok(_) => {
                log::info!("Restarting into {}", bundle.display());
                app.exit(0);
                return;
            }
            Err(error) => log::error!("Couldn't restart through LaunchServices: {error}"),
        }
    }
    app.restart();
}

/// The `.app` bundle Ligma runs from, if it runs from one
#[cfg(target_os = "macos")]
fn bundle_path() -> Option<std::path::PathBuf> {
    let exe = tauri::utils::platform::current_exe().ok()?;
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.ends_with("MacOS") && contents.ends_with("Contents") && bundle.extension()? == "app")
        .then(|| bundle.to_path_buf())
}

/// Says why an update couldn't be installed, and offers its download page
/// or a report
fn install_failed(app: &AppHandle, error: String) {
    log::error!("{error}");
    let handle = app.clone();
    crate::prompt::show(
        app,
        "Update Failed",
        format!("{error}\n\nDownload the latest version and run its installer. Your sign-in and settings stay."),
        &["Close", "Report…", "Download"],
        move |choice| match choice {
            Some(2) => {
                let _ = handle.opener().open_url(RELEASES, None::<&str>);
            }
            Some(1) => crate::report::open(&handle, Some(&error)),
            _ => {}
        },
    );
}

fn tell(app: &AppHandle, message: String) {
    crate::prompt::show(app, "Check for Updates", message, &["OK"], |_| {});
}

/// Installs the downloaded update, if there is one. On macOS this replaces
/// the app bundle; on Windows it runs the installer, which quits Ligma, so it
/// only returns if that failed. A failed update stays pending, so Check for
/// Updates… offers it again
fn install(app: &AppHandle) -> tauri_plugin_updater::Result<()> {
    let pending = app.state::<Pending>().0.lock().unwrap().take();
    let Some((update, bytes)) = pending else {
        return Ok(());
    };
    log::info!("Installing Ligma {}", update.version);
    let result = update.install(&bytes);
    if result.is_err() {
        *app.state::<Pending>().0.lock().unwrap() = Some((update, bytes));
    }
    result
}

/// Installs an update the user chose to install later. Ligma is quitting
/// and can't show a dialog, so a failure is shown on the next launch
pub fn install_on_quit(app: &AppHandle) {
    let version = match app.state::<Pending>().0.lock().unwrap().as_ref() {
        Some((update, _)) => update.version.clone(),
        None => return,
    };
    if let Err(error) = install(app) {
        let message = format!("Ligma {version} couldn't be installed when you quit: {error}");
        log::error!("{message}");
        remember_failure(app, &message);
    }
}

/// Keeps `message` in `FAILED_FILE`, for the next launch to show
fn remember_failure(app: &AppHandle, message: &str) {
    if let Ok(dir) = app.path().app_config_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(FAILED_FILE), message);
    }
}

/// Why an update failed to install, if one did since the last launch
fn take_failure(app: &AppHandle) -> Option<String> {
    let path = app.path().app_config_dir().ok()?.join(FAILED_FILE);
    let error = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(path);
    // The update that failed may be the version running now, installed by hand
    let version = app.package_info().version.to_string();
    let this_version = [" ", ":"].iter().any(|end| error.contains(&format!("Ligma {version}{end}")));
    (!this_version).then_some(error)
}
