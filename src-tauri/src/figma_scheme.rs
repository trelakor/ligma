//! `figma://` links on Windows, where Figma sends Google sign-in back to
//! (see auth.rs) and its "Open in desktop app" links point
//!
//! Without the Figma desktop app nothing handles `figma://`, so Ligma
//! registers itself (`claim_if_free`, at launch) and opens those links in
//! its tabs. When Figma desktop owns the scheme, Ligma only borrows it while
//! a sign-in waits (`borrow`): Figma's entry is kept under Ligma's own key
//! and put back (`restore`) once the link arrives, the sign-in expires,
//! Ligma quits, or at the next launch after a crash. If Figma registered
//! itself again meanwhile (it does on every update), its new entry stays
//!
//! A link starts a second Ligma, which `tauri-plugin-single-instance` hands
//! to the running one (`on_args`)

use tauri::{AppHandle, Url};
use windows_registry::CURRENT_USER;

const SCHEME_KEY: &str = r"Software\Classes\figma";
const COMMAND_KEY: &str = r"Software\Classes\figma\shell\open\command";

/// Where a borrowed scheme's own command is kept. Empty: there was no
/// per-user entry, so restoring deletes Ligma's
const BACKUP_KEY: &str = r"Software\dev.ligma.client";
const BACKUP_VALUE: &str = "BorrowedFigmaCommand";

fn our_command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\" \"%1\"", exe.display()))
}

/// The command `figma://` runs for this user, per-user entry first
fn current_command() -> Option<String> {
    let read = |root: &windows_registry::Key| root.open(COMMAND_KEY).and_then(|key| key.get_string(""));
    read(CURRENT_USER).or_else(|_| read(windows_registry::LOCAL_MACHINE)).ok()
}

/// The program a command starts: its first, usually quoted, word
fn program(command: &str) -> &str {
    match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => command.split_whitespace().next().unwrap_or(command),
    }
}

/// True if `command` starts a Ligma, this copy or another
fn is_ligma(command: &str) -> bool {
    std::path::Path::new(program(command))
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("ligma.exe"))
}

/// Points `figma://` at this Ligma for this user
fn register(command: &str) -> windows_registry::Result<()> {
    let key = CURRENT_USER.create(SCHEME_KEY)?;
    key.set_string("", "URL:figma")?;
    key.set_string("URL Protocol", "")?;
    CURRENT_USER.create(COMMAND_KEY)?.set_string("", command)
}

/// Registers Ligma for `figma://` if no installed app handles it, or keeps
/// a Ligma registration pointing at this copy. Call at launch, after
/// `restore`
pub fn claim_if_free() {
    let Some(ours) = our_command() else {
        return;
    };
    match current_command() {
        Some(command) if command == ours => {}
        Some(command) if !is_ligma(&command) && std::path::Path::new(program(&command)).is_file() => {}
        _ => {
            let _ = register(&ours);
        }
    }
}

/// Points `figma://` at Ligma until `restore`, keeping the current entry
pub fn borrow() {
    let Some(ours) = our_command() else {
        return;
    };
    match current_command() {
        Some(command) if command == ours => return,
        // Another copy of Ligma: nothing to give back
        Some(command) if is_ligma(&command) => {
            let _ = register(&ours);
            return;
        }
        _ => {}
    }
    let Ok(backup) = CURRENT_USER.create(BACKUP_KEY) else {
        return;
    };
    // A backup from an earlier borrow is Figma's real entry; keep it
    if backup.get_string(BACKUP_VALUE).is_err() {
        let own = CURRENT_USER.open(COMMAND_KEY).and_then(|key| key.get_string(""));
        if backup.set_string(BACKUP_VALUE, own.unwrap_or_default()).is_err() {
            return;
        }
    }
    let _ = register(&ours);
}

/// Gives a borrowed `figma://` back, unless its owner registered again
pub fn restore() {
    let Ok(backup) = CURRENT_USER.open(BACKUP_KEY) else {
        return;
    };
    let Ok(saved) = backup.get_string(BACKUP_VALUE) else {
        return;
    };
    if CURRENT_USER
        .open(COMMAND_KEY)
        .and_then(|key| key.get_string(""))
        .is_ok_and(|command| is_ligma(&command))
    {
        let _ = if saved.is_empty() {
            CURRENT_USER.remove_tree(SCHEME_KEY)
        } else {
            CURRENT_USER.create(COMMAND_KEY).and_then(|key| key.set_string("", &saved))
        };
    }
    let _ = CURRENT_USER
        .options()
        .write()
        .open(BACKUP_KEY)
        .and_then(|key| key.remove_value(BACKUP_VALUE));
}

/// Handles the `figma://` links among a Ligma's command line arguments
pub fn on_args(app: &AppHandle, args: impl IntoIterator<Item = String>) {
    for arg in args {
        if let Some(url) = Url::parse(&arg).ok().filter(|url| url.scheme() == "figma") {
            crate::auth::on_figma_link(app, &url);
        }
    }
}
