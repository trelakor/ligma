//! Quick connect for Figwright (see mcp_bridge.rs): finds the AI tools
//! installed on this computer and adds Figwright's MCP server to their MCP
//! settings, or removes it, from a switch in Settings
//!
//! Each tool keeps its MCP servers in a file of its own. Ligma only changes
//! its `figwright` entry, and the `figma-bridge` one earlier versions added
//! for Figma MCP Bridge, and keeps everything else as it was, including
//! the order of keys. A file it can't read is left alone, with an error.
//! Claude Code's file is written by Claude Code all the time, so Ligma only
//! reads it, and Settings shows the command that adds the server instead

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Map, Value};
use tauri::Webview;

use crate::mcp_bridge::VERSION;

/// The server's name in each tool
pub const NAME: &str = "figwright";

pub const PACKAGE: &str = "@figwright/mcp";

/// Figma MCP Bridge's entry, which Ligma added before it used Figwright.
/// Update replaces it, and the switch removes it
const OLD_NAME: &str = "figma-bridge";

#[derive(Clone, Copy)]
enum Format {
    /// `{ "mcpServers": { NAME: { "command", "args" } } }`, like Claude
    /// Desktop's
    McpServers,
    /// VS Code's `{ "servers": { NAME: { "type": "stdio", ... } } }`
    VsCode,
    /// Codex's config.toml, `[mcp_servers.NAME]`
    Toml,
    /// Claude Code: read only, added with its command
    ClaudeCode,
}

struct Client {
    id: &'static str,
    name: &'static str,
    format: Format,
    /// Where its settings file is, if it's installed
    find: fn() -> Option<PathBuf>,
}

const CLIENTS: &[Client] = &[
    Client { id: "claude-desktop", name: "Claude Desktop", format: Format::McpServers, find: claude_desktop },
    Client { id: "claude-code", name: "Claude Code", format: Format::ClaudeCode, find: claude_code },
    Client { id: "codex", name: "Codex", format: Format::Toml, find: codex },
    Client { id: "cursor", name: "Cursor", format: Format::McpServers, find: cursor },
    Client { id: "vscode", name: "Visual Studio Code", format: Format::VsCode, find: vscode },
    Client { id: "windsurf", name: "Windsurf", format: Format::McpServers, find: windsurf },
    Client { id: "gemini", name: "Gemini CLI", format: Format::McpServers, find: gemini },
    Client { id: "antigravity", name: "Antigravity", format: Format::McpServers, find: antigravity },
];

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// `file` in `dir`, if the tool made `dir`
fn in_dir(dir: PathBuf, file: &str) -> Option<PathBuf> {
    dir.is_dir().then(|| dir.join(file))
}

/// Folders an app keeps its settings in: %APPDATA% on Windows, Application
/// Support on macOS
fn app_data(name: &str) -> Option<PathBuf> {
    if cfg!(windows) {
        Some(PathBuf::from(std::env::var_os("APPDATA")?).join(name))
    } else {
        Some(home()?.join("Library/Application Support").join(name))
    }
}

fn claude_desktop() -> Option<PathBuf> {
    const FILE: &str = "claude_desktop_config.json";
    let mut dirs = Vec::new();
    // The Microsoft Store app keeps its settings under its package, and
    // newer versions of it in Claude-Data
    if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|_| cfg!(windows)).map(PathBuf::from) {
        dirs.extend(subdirs(&local.join("Claude-Data"), "claude_"));
        dirs.extend(subdirs(&local.join("Packages"), "Claude_").into_iter().map(|dir| dir.join("LocalCache").join("Roaming").join("Claude")));
    }
    dirs.extend(app_data("Claude"));
    dirs.iter()
        .find(|dir| dir.join(FILE).is_file())
        .or_else(|| dirs.iter().find(|dir| dir.is_dir()))
        .map(|dir| dir.join(FILE))
}

/// Folders in `dir` whose names start with `prefix`, ignoring case
fn subdirs(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().to_lowercase().starts_with(&prefix.to_lowercase()))
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect()
}

fn claude_code() -> Option<PathBuf> {
    let home = home()?;
    let file = home.join(".claude.json");
    (file.is_file() || home.join(".claude").is_dir()).then_some(file)
}

fn codex() -> Option<PathBuf> {
    let dir = std::env::var_os("CODEX_HOME").map(PathBuf::from).or_else(|| Some(home()?.join(".codex")))?;
    in_dir(dir, "config.toml")
}

fn cursor() -> Option<PathBuf> {
    in_dir(home()?.join(".cursor"), "mcp.json")
}

fn vscode() -> Option<PathBuf> {
    in_dir(app_data("Code")?.join("User"), "mcp.json")
}

fn windsurf() -> Option<PathBuf> {
    in_dir(home()?.join(".codeium").join("windsurf"), "mcp_config.json")
}

fn gemini() -> Option<PathBuf> {
    // Antigravity makes ~/.gemini too, so only Gemini CLI's own files count
    let dir = home()?.join(".gemini");
    ["settings.json", "oauth_creds.json", "google_accounts.json"]
        .iter()
        .any(|file| dir.join(file).is_file())
        .then(|| dir.join("settings.json"))
}

fn antigravity() -> Option<PathBuf> {
    in_dir(home()?.join(".gemini").join("antigravity"), "mcp_config.json")
}

/// The server's command and arguments, pinned to `VERSION`
fn command() -> (&'static str, Vec<String>) {
    ("npx", vec!["-y".into(), format!("{PACKAGE}@{}", VERSION.trim())])
}

fn entry(format: Format) -> Value {
    let (command, args) = command();
    match format {
        Format::VsCode => json!({ "type": "stdio", "command": command, "args": args }),
        _ => json!({ "command": command, "args": args }),
    }
}

/// What Settings shows for a tool
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientState {
    id: &'static str,
    name: &'static str,
    /// "off", "on", "outdated" (an entry for another version of the
    /// server, or for Figma MCP Bridge) or "manual" (Claude Code, which
    /// only shows its command)
    state: &'static str,
    /// Whether Claude Code has the server, as it can't be switched here
    added: bool,
    /// Whether it still has Figma MCP Bridge's entry
    old: bool,
    path: String,
    /// Why its settings couldn't be read
    error: Option<String>,
}

fn servers_key(format: Format) -> &'static str {
    match format {
        Format::VsCode => "servers",
        _ => "mcpServers",
    }
}

/// The file's JSON, or an empty object if there's no file yet
fn read_json(path: &Path) -> Result<Value, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(json!({})),
        Err(error) => return Err(format!("Couldn't read {}: {error}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    match serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) {
        Ok(value) if value.is_object() => Ok(value),
        _ => Err(format!("{} isn't JSON Ligma can read, so it was left as it is", path.display())),
    }
}

/// The entry named `name` in the file, if it has one
fn current(client: &Client, path: &Path, name: &str) -> Result<Option<String>, String> {
    if let Format::Toml = client.format {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        return Ok(toml_section(&text, name).map(|(start, end)| text[start..end].to_string()));
    }
    let config = read_json(path)?;
    Ok(config.get(servers_key(client.format)).and_then(|servers| servers.get(name)).map(Value::to_string))
}

fn state(client: &Client, path: &Path) -> ClientState {
    let mut state = ClientState {
        id: client.id,
        name: client.name,
        state: "off",
        added: false,
        old: false,
        path: path.display().to_string(),
        error: None,
    };
    match current(client, path, OLD_NAME) {
        Err(error) => state.error = Some(error),
        Ok(found) => state.old = found.is_some(),
    }
    match current(client, path, NAME) {
        Err(error) => state.error = Some(error),
        Ok(None) if state.old => state.state = "outdated",
        Ok(None) => {}
        Ok(Some(found)) => {
            let latest = match client.format {
                Format::Toml => found.contains(&format!("{PACKAGE}@{}\"", VERSION.trim())),
                _ => serde_json::from_str::<Value>(&found).is_ok_and(|found| found == entry(client.format)),
            };
            state.added = true;
            state.state = if latest && !state.old { "on" } else { "outdated" };
        }
    }
    if let Format::ClaudeCode = client.format {
        state.state = "manual";
    }
    state
}

/// The AI tools found on this computer
#[tauri::command]
pub async fn get_mcp_clients(webview: Webview) -> Vec<ClientState> {
    if !crate::settings::from_settings_window(&webview) {
        return Vec::new();
    }
    CLIENTS.iter().filter_map(|client| Some(state(client, &(client.find)()?))).collect()
}

/// Adds the server to a tool's MCP settings (`on`), or updates it to this
/// version, or removes it
#[tauri::command]
pub async fn set_mcp_client(webview: Webview, id: String, on: bool) -> Result<ClientState, String> {
    if !crate::settings::from_settings_window(&webview) {
        return Err("not allowed".into());
    }
    let client = CLIENTS.iter().find(|client| client.id == id).ok_or("No such AI tool")?;
    let path = (client.find)().ok_or_else(|| format!("{} isn't installed", client.name))?;
    // Nothing to remove, and no file made for it
    if !on && current(client, &path, NAME)?.is_none() && current(client, &path, OLD_NAME)?.is_none() {
        return Ok(state(client, &path));
    }
    let text = match client.format {
        Format::ClaudeCode => return Err("Add it with the command instead".into()),
        Format::Toml => set_toml(&std::fs::read_to_string(&path).unwrap_or_default(), on),
        format => set_json(&path, format, on)?,
    };
    write(&path, &text)?;
    log::info!("{} {NAME} for {}", if on { "added" } else { "removed" }, client.name);
    Ok(state(client, &path))
}

fn set_json(path: &Path, format: Format, on: bool) -> Result<String, String> {
    let original = std::fs::read_to_string(path).unwrap_or_default();
    let mut config = read_json(path)?;
    let root = config.as_object_mut().unwrap();
    let key = servers_key(format);
    if on {
        let servers = root.entry(key).or_insert_with(|| Value::Object(Map::new()));
        if !servers.is_object() {
            return Err(format!("\"{key}\" in {} isn't an object, so it was left as it is", path.display()));
        }
        let servers = servers.as_object_mut().unwrap();
        // In the old entry's place, if there is one
        match servers.keys().position(|name| name == OLD_NAME) {
            Some(index) if !servers.contains_key(NAME) => {
                servers.shift_insert(index, NAME.into(), entry(format));
            }
            _ => {
                servers.insert(NAME.into(), entry(format));
            }
        }
        servers.shift_remove(OLD_NAME);
    } else if let Some(servers) = root.get_mut(key).and_then(Value::as_object_mut) {
        servers.shift_remove(NAME);
        servers.shift_remove(OLD_NAME);
    }
    // In the file's own indentation
    let indent = original
        .lines()
        .skip(1)
        .find(|line| !line.trim().is_empty())
        .map(|line| &line[..line.len() - line.trim_start().len()])
        .filter(|indent| !indent.is_empty())
        .unwrap_or("  ");
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    serde::Serialize::serialize(&config, &mut serializer).map_err(|error| error.to_string())?;
    let mut text = String::from_utf8(out).map_err(|error| error.to_string())?;
    if original.ends_with('\n') || original.is_empty() {
        text.push('\n');
    }
    Ok(text)
}

/// The byte range of `[mcp_servers.<name>]` in config.toml, with its own
/// subtables like `[mcp_servers.<name>.env]`, up to the next other table
fn toml_section(text: &str, name: &str) -> Option<(usize, usize)> {
    let is_ours = |header: &str| {
        let header = header.trim().trim_start_matches('[').trim_end_matches(']').trim();
        let header = header.replace(&format!("\"{name}\""), name);
        header == format!("mcp_servers.{name}") || header.starts_with(&format!("mcp_servers.{name}."))
    };
    let mut start = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            match (start, is_ours(trimmed.split('#').next().unwrap_or(""))) {
                (None, true) => start = Some(offset),
                (Some(start), false) => return Some((start, offset)),
                _ => {}
            }
        }
        offset += line.len();
    }
    start.map(|start| (start, text.len()))
}

fn set_toml(text: &str, on: bool) -> String {
    let mut text = text.to_string();
    for name in [NAME, OLD_NAME] {
        if let Some((start, end)) = toml_section(&text, name) {
            text.replace_range(start..end, "");
        }
    }
    if on {
        // Codex starts the command itself, and on Windows npx is a script
        // only a shell runs
        let (command, args) = command();
        let (command, args) = if cfg!(windows) {
            ("cmd", [vec!["/c".to_string(), command.to_string()], args].concat())
        } else {
            (command, args)
        };
        let args = args.iter().map(|arg| format!("\"{arg}\"")).collect::<Vec<_>>().join(", ");
        let trimmed = text.trim_end().len();
        text.truncate(trimmed);
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&format!("[mcp_servers.{NAME}]\ncommand = \"{command}\"\nargs = [{args}]\n"));
    }
    text
}

/// Writes the file whole or not at all
fn write(path: &Path, text: &str) -> Result<(), String> {
    let fail = |error: std::io::Error| format!("Couldn't save {}: {error}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    let temp = path.with_extension("ligma-tmp");
    std::fs::write(&temp, text).map_err(fail)?;
    std::fs::rename(&temp, path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        fail(error)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_adds_and_removes_only_its_section() {
        let before = "model = \"o3\"\n\n[mcp_servers.other]\ncommand = \"x\"\n";
        let added = set_toml(before, true);
        assert!(added.starts_with(before.trim_end()));
        assert!(toml_section(&added, NAME).is_some());
        let again = set_toml(&added, true);
        assert_eq!(again.matches("[mcp_servers.figwright]").count(), 1);
        let removed = set_toml(&format!("{added}[mcp_servers.figwright.env]\nA = \"1\"\n\n[profiles.x]\nmodel = \"y\"\n"), false);
        assert!(removed.contains("[mcp_servers.other]") && removed.contains("[profiles.x]"));
        assert!(!removed.contains("figwright"));
    }

    #[test]
    fn toml_replaces_figma_mcp_bridge() {
        let before = "[mcp_servers.figma-bridge]\ncommand = \"npx\"\n\n[mcp_servers.other]\ncommand = \"x\"\n";
        let added = set_toml(before, true);
        assert!(!added.contains("figma-bridge"));
        assert!(added.contains("[mcp_servers.other]") && added.contains("[mcp_servers.figwright]"));
        assert!(!set_toml(before, false).contains("figma-bridge"));
    }

    #[test]
    fn json_replaces_figma_mcp_bridge_in_place() {
        let path = std::env::temp_dir().join(format!("ligma-mcp-test-{}.json", std::process::id()));
        std::fs::write(&path, r#"{"mcpServers":{"a":{},"figma-bridge":{"command":"npx"},"b":{}}}"#).unwrap();
        let text = set_json(&path, Format::McpServers, true).unwrap();
        let _ = std::fs::remove_file(&path);
        let config: Value = serde_json::from_str(&text).unwrap();
        let names: Vec<&String> = config["mcpServers"].as_object().unwrap().keys().collect();
        assert_eq!(names, ["a", "figwright", "b"]);
    }
}
