//! Logs and problem reports. Ligma logs to `ligma.log` in its log folder.
//! Report opens a new GitHub issue in the browser, filled in with Ligma's
//! version, the system, the problem and the log's last lines, for the user
//! to read and edit before submitting. A report too long for a link is
//! also put on the clipboard to paste. Nothing is sent otherwise, and the
//! log keeps no Figma file names or links. A panic is remembered, so the
//! next launch offers to report it.

use std::path::PathBuf;
use std::sync::OnceLock;

use log::LevelFilter;
use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Manager, Url, Wry};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_log::{RotationStrategy, Target, TargetKind, TimezoneStrategy};
use tauri_plugin_opener::OpenerExt;

const NEW_ISSUE: &str = "https://github.com/trelakor/ligma/issues/new";
const LOG_NAME: &str = "ligma";
const CRASH_FILE: &str = "crashed.txt";
/// The log's last lines in a report.
const LOG_LINES: usize = 60;
/// Windows may cut links past 2083 characters when opening them, so a
/// longer report leaves out older log lines to fit.
const MAX_URL: usize = 2000;

/// Where a panic is remembered, set once the app's folders are known.
static CRASH_PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn log_plugin() -> TauriPlugin<Wry> {
    let mut builder = tauri_plugin_log::Builder::new()
        .clear_targets()
        .target(Target::new(TargetKind::LogDir { file_name: Some(LOG_NAME.into()) }))
        .level(LevelFilter::Info)
        // Says which installer the updater ran, and how.
        .level_for("tauri_plugin_updater", LevelFilter::Debug)
        .max_file_size(1_000_000)
        .rotation_strategy(RotationStrategy::KeepOne)
        .timezone_strategy(TimezoneStrategy::UseLocal);
    if cfg!(debug_assertions) {
        builder = builder.target(Target::new(TargetKind::Stdout));
    }
    builder.build()
}

/// Logs panics, and remembers the last one to offer a report next launch.
pub fn catch_panics(app: &AppHandle) {
    if let Ok(dir) = app.path().app_config_dir() {
        let _ = CRASH_PATH.set(dir.join(CRASH_FILE));
    }
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("{info}");
        if let Some(path) = CRASH_PATH.get() {
            let _ = path.parent().map(std::fs::create_dir_all);
            let _ = std::fs::write(path, info.to_string());
        }
        default(info);
    }));
    log::info!("Ligma {} on {}", app.package_info().version, system());
}

/// Offers to report a panic from the last run, if there was one.
pub fn offer_last_crash(app: &AppHandle) {
    let Some(path) = CRASH_PATH.get() else {
        return;
    };
    let Ok(problem) = std::fs::read_to_string(path) else {
        return;
    };
    let _ = std::fs::remove_file(path);
    offer(
        app,
        "Ligma Ran Into a Problem",
        "Something went wrong in Ligma last time it ran. Reporting it helps fix it.".into(),
        problem,
    );
}

/// An error prompt that offers to report `problem`.
pub fn offer(app: &AppHandle, title: &str, message: String, problem: String) {
    let handle = app.clone();
    crate::prompt::show(app, title, message, &["Close", "Report…"], move |choice| {
        if choice == Some(1) {
            open(&handle, Some(&problem));
        }
    });
}

/// Opens a new GitHub issue in the browser, filled in for `problem`, or for
/// the user to describe one.
pub fn open(app: &AppHandle, problem: Option<&str>) {
    let title = problem
        .and_then(|problem| problem.lines().next())
        .map(|line| line.chars().take(100).collect::<String>())
        .unwrap_or_default();
    let log_path = app.path().app_log_dir().map(|dir| dir.join(format!("{LOG_NAME}.log")));
    let log = log_path
        .as_ref()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    let lines: Vec<&str> = log.lines().collect();
    let all = lines.len().saturating_sub(LOG_LINES);
    // The report from log line `first` on, with the problem cut to `chars`.
    let body = |first: usize, chars: usize, note: &str| {
        let mut body = format!(
            "<!-- What were you doing when it happened? -->\n\n\n{note}**Ligma** {}\n**System** {}\n",
            app.package_info().version,
            system()
        );
        if let Some(problem) = problem {
            let problem: String = problem.chars().take(chars).collect();
            body += &format!("\n**Problem**\n```text\n{}\n```\n", redact(&problem));
        }
        if first < lines.len() {
            body += &format!(
                "\n<details><summary>Log</summary>\n\n```text\n{}\n```\n\n</details>\n",
                redact(&lines[first..].join("\n"))
            );
        }
        if let Ok(path) = &log_path {
            body += &format!("\nFull log: `{}`\n", redact(&path.display().to_string()));
        }
        body
    };
    let url = |body: &str| Url::parse_with_params(NEW_ISSUE, [("title", title.as_str()), ("body", body)]).ok();
    let full = body(all, 2000, "");
    let mut link = url(&full);
    // Too long for a link: the full report goes on the clipboard, and the
    // link keeps what fits.
    if link.as_ref().is_some_and(|link| link.as_str().len() > MAX_URL) {
        let _ = app.clipboard().write_text(full);
        let note = "<!-- The full report, with more of the log, is on your clipboard. Paste it here in place of what's below. -->\n\n";
        let mut first = all;
        loop {
            link = url(&body(first, 300, note));
            if link.as_ref().is_none_or(|link| link.as_str().len() <= MAX_URL) || first >= lines.len() {
                break;
            }
            first += 1;
        }
    }
    if let Some(link) = link {
        log::info!("Opening a problem report");
        let _ = app.opener().open_url(link.as_str(), None::<&str>);
    }
}

/// Hides the user's name in paths and any Figma links.
fn redact(text: &str) -> String {
    let mut text = text.to_string();
    for var in ["USERPROFILE", "HOME"] {
        if let Some(home) = std::env::var_os(var).map(|home| home.to_string_lossy().into_owned()) {
            if home.len() > 3 {
                // Also as `{:?}` prints it, with backslashes doubled.
                text = text.replace(&home.replace('\\', "\\\\"), "~").replace(&home, "~");
            }
        }
    }
    text.split_inclusive(char::is_whitespace)
        .map(|word| {
            if word.contains("figma.com/") {
                let space = &word[word.trim_end().len()..];
                format!("<figma link>{space}")
            } else {
                word.to_string()
            }
        })
        .collect()
}

/// The OS, its version and the processor, like "Windows 11 24H2 (26100) x86_64".
fn system() -> String {
    let arch = std::env::consts::ARCH;
    #[cfg(windows)]
    {
        let key = windows_registry::LOCAL_MACHINE.open(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
        let get = |name: &str| key.as_ref().ok().and_then(|key| key.get_string(name).ok());
        let build = get("CurrentBuild").unwrap_or_default();
        // Windows 11 still says "Windows 10" in ProductName.
        let name = if build.parse::<u32>().is_ok_and(|build| build >= 22000) {
            "Windows 11"
        } else {
            "Windows 10"
        };
        let release = get("DisplayVersion").unwrap_or_default();
        format!("{name} {release} ({build}) {arch}")
    }
    #[cfg(target_os = "macos")]
    {
        let version = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .unwrap_or_default();
        format!("macOS {version} {arch}")
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        format!("{} {arch}", std::env::consts::OS)
    }
}
