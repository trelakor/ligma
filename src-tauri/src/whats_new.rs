//! What's new: the release notes in CHANGELOG.md, built into the app. The
//! first launch after an update shows the notes of every version since the
//! one that ran last, in a prompt (prompt.rs), and Settings > What's New
//! lists them all. The version that ran last is kept in `last-version` in
//! the app config folder

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Manager, Webview};

const CHANGELOG: &str = include_str!("../../CHANGELOG.md");
const LAST_VERSION_FILE: &str = "last-version";

/// Whether Ligma had run before this launch, for updates from versions
/// that didn't keep `last-version` yet
static RAN_BEFORE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize)]
pub struct Release {
    pub version: String,
    /// As written in CHANGELOG.md, like 2026-11-14
    pub date: Option<String>,
    /// Markdown list items, each one line
    pub notes: Vec<String>,
}

/// Every release in CHANGELOG.md, newest first
pub fn releases() -> Vec<Release> {
    parse(CHANGELOG)
}

/// Reads the `## <version> - <date>` sections and their list items. A
/// line under an item continues it; one after a blank line starts a new note
fn parse(changelog: &str) -> Vec<Release> {
    let mut releases: Vec<Release> = Vec::new();
    let mut blank = true;
    for line in changelog.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            let (version, date) = match heading.split_once(" - ") {
                Some((version, date)) => (version, Some(date.trim().to_owned())),
                None => (heading, None),
            };
            releases.push(Release { version: version.trim().to_owned(), date, notes: Vec::new() });
            blank = true;
            continue;
        }
        let Some(release) = releases.last_mut() else {
            continue;
        };
        let line = line.trim();
        if line.is_empty() {
            blank = true;
            continue;
        }
        match (line.strip_prefix("- "), release.notes.last_mut()) {
            (Some(item), _) => release.notes.push(item.to_owned()),
            (None, Some(note)) if !blank => {
                note.push(' ');
                note.push_str(line);
            }
            (None, _) => release.notes.push(line.to_owned()),
        }
        blank = false;
    }
    releases
}

/// `major.minor.patch`, ignoring a pre-release or build suffix
fn version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

/// Notes whether Ligma has run before, from its settings file. Called
/// before `settings::load`, which writes the file on first launch
pub fn init(app: &AppHandle) {
    let ran = app.path().app_config_dir().is_ok_and(|dir| dir.join("settings.json").exists());
    RAN_BEFORE.store(ran, Ordering::Relaxed);
}

/// Shows what changed since the version that ran last, if it was older,
/// unless that's switched off in settings. Nothing on first launch
pub fn offer(app: &AppHandle) {
    let Ok(dir) = app.path().app_config_dir() else {
        return;
    };
    let path = dir.join(LAST_VERSION_FILE);
    let current = app.package_info().version.to_string();
    let last = std::fs::read_to_string(&path).ok();
    if last.as_deref().map(str::trim) != Some(current.as_str()) {
        let _ = std::fs::create_dir_all(&dir);
        if let Err(error) = std::fs::write(&path, &current) {
            log::warn!("Couldn't keep the version that ran: {error}");
        }
    }
    let Some(now) = version(&current) else {
        return;
    };
    let new: Vec<Release> = match last.as_deref().map(version) {
        Some(Some(last)) => releases()
            .into_iter()
            .filter(|release| version(&release.version).is_some_and(|v| v > last && v <= now))
            .collect(),
        // Updated from a version that didn't keep `last-version`: only this
        // version's notes, since which one it was is unknown
        None if RAN_BEFORE.load(Ordering::Relaxed) => {
            releases().into_iter().filter(|release| version(&release.version) == Some(now)).collect()
        }
        _ => Vec::new(),
    };
    let new: Vec<Release> = new.into_iter().filter(|release| !release.notes.is_empty()).collect();
    if new.is_empty() || !crate::settings::current(app).whats_new_after_updates {
        return;
    }
    log::info!("Showing what's new since {}", last.as_deref().map(str::trim).unwrap_or("an earlier version"));
    let handle = app.clone();
    crate::prompt::show_releases(
        app,
        &format!("What's new in Ligma {current}"),
        new,
        &["All Updates", "Done"],
        move |choice| {
            if choice == Some(0) {
                if let Err(error) = crate::settings::open_window_at(&handle, Some("whats-new")) {
                    log::error!("Couldn't open What's New in settings: {error}");
                }
            }
        },
    );
}

/// Every release, for Settings > What's New
#[tauri::command]
pub async fn get_releases(webview: Webview) -> Option<Vec<Release>> {
    crate::settings::from_settings_window(&webview).then(releases)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_sections_and_notes() {
        let releases = parse(
            "# Changelog\n\nIntro.\n\n## 0.2.0 - 2026-10-22\n\n- One\n- Two\n  wrapped\n\nAfter a gap\n\n## 0.1.0\n- First\n",
        );
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].version, "0.2.0");
        assert_eq!(releases[0].date.as_deref(), Some("2026-10-22"));
        assert_eq!(releases[0].notes, ["One", "Two wrapped", "After a gap"]);
        assert_eq!(releases[1].date, None);
        assert_eq!(releases[1].notes, ["First"]);
    }

    #[test]
    fn compares_versions() {
        assert!(version("0.10.0") > version("0.9.3"));
        assert_eq!(version("1.2.3-beta.1"), Some((1, 2, 3)));
        assert_eq!(version("nope"), None);
    }

    // The release workflow checks this too; here it fails sooner
    #[test]
    fn changelog_has_this_version() {
        let config: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let current = version(config["version"].as_str().unwrap());
        assert!(releases().iter().any(|release| version(&release.version) == current && !release.notes.is_empty()));
    }
}
