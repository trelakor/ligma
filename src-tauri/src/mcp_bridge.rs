//! AI tools in Figma files (`mcpBridge`), through Figwright
//! (github.com/awdr74100/figwright) rather than Figma's own MCP server,
//! which a free plan can only call 6 times a month
//!
//! Figwright is an MCP server, `@figwright/mcp` on npm, that AI tools start
//! themselves, and a plugin run in each Figma file. Its server listens on
//! `127.0.0.1:3055` and sends the plugin tool calls over a WebSocket, one
//! connection per file, as MessagePack. Ligma takes the plugin's place: it
//! connects each file open in a tab, and runs the plugin's own code
//! (`vendor/figwright/code.js`) in the tab, without its window, against
//! the plugin API Figma's web editor offers on the page (see plugins.rs).
//! So there's nothing to run in Figma, and no plugin window
//!
//! That API only exists in files the user can edit. Tool calls are answered
//! in the page, then read back by polling, as `eval` can't wait for a
//! promise. A tab an AI tool has used in the last few minutes isn't frozen
//! or put to sleep (see `CAN_REST_JS` in tabs.rs)
//!
//! The protocol is Figwright's own: `packages/shared/src` for the
//! WebSocket, `packages/plugin/protocol` for the plugin's messages

use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::io::ErrorKind;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Webview};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::CloseFrame;
use tungstenite::{Message, WebSocket};

/// Version of the vendored plugin code. The MCP server is pinned to it, as
/// its tools have to match
pub const VERSION: &str = include_str!("../vendor/figwright/VERSION");

const PLUGIN_JS: &str = include_str!("../vendor/figwright/code.js");

/// Figwright's wire protocol (`PROTOCOL_VERSION`), which the server refuses
/// a plugin over if it differs
const PROTOCOL: &str = "0.1.0";

/// The server's port. Figwright always uses this one
const PORT: u16 = 3055;

/// How often the open files are matched with connections
const CHECK_EVERY: Duration = Duration::from_secs(2);

/// Wait before connecting again after a failed or lost connection
const RETRY_AFTER: Duration = Duration::from_secs(3);

/// Wait before trying again a server that refused Ligma
const REFUSED_RETRY_AFTER: Duration = Duration::from_secs(30);

/// Longest wait for a file still loading before a tool call
const LOADING_TIMEOUT: Duration = Duration::from_secs(60);

/// Longest wait for a tool call's answer. The server gives up sooner
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

static ENABLED: AtomicBool = AtomicBool::new(false);

pub fn init(settings: &crate::settings::Settings) {
    ENABLED.store(settings.mcp_bridge, Ordering::Relaxed);
}

pub fn apply_settings(app: &AppHandle, settings: &crate::settings::Settings) {
    init(settings);
    // Settings shows the change right away rather than at the next check
    sync(app);
}

/// What a connection knows of its file, kept up to date by `sync`
#[derive(Clone, PartialEq)]
struct File {
    name: String,
    /// Tab the file is in. It may move to another tab showing it
    tab: u32,
    /// Whether that tab is on screen, which decides which file AI tools
    /// use when they don't pick one
    shown: bool,
}

/// A file's connection to the MCP server
struct Connection {
    file: Arc<Mutex<File>>,
    connected: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
struct State {
    connections: HashMap<String, Connection>,
    /// Whether the server last turned Ligma away, until one connects
    refused: bool,
    /// What Settings was last sent
    status: Option<Status>,
}

/// What Settings shows under the switch
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    on: bool,
    /// Files open in an editor
    files: usize,
    /// Of those, the ones the MCP server has
    connected: usize,
    version: &'static str,
    /// Whether the server turned Ligma away, as it speaks another version
    /// of Figwright's protocol
    refused: bool,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(Mutex::default)
}

/// Matches connections with the open files every `CHECK_EVERY`, on a
/// thread of its own
pub fn start(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || loop {
        sync(&app);
        std::thread::sleep(CHECK_EVERY);
    });
}

/// Connects files that opened, disconnects ones that closed or slept, and
/// sends Settings the new status
fn sync(app: &AppHandle) {
    let on = ENABLED.load(Ordering::Relaxed);
    let files = if on { crate::tabs::editor_files(app) } else { Vec::new() };
    let mut state = state().lock().unwrap();
    state.connections.retain(|key, connection| match files.iter().find(|file| &file.key == key) {
        Some(file) => {
            *connection.file.lock().unwrap() = File { name: file.name.clone(), tab: file.tab, shown: file.shown };
            true
        }
        None => {
            connection.stop.store(true, Ordering::Relaxed);
            false
        }
    });
    for file in &files {
        if state.connections.contains_key(&file.key) {
            continue;
        }
        let connection = Connection {
            file: Arc::new(Mutex::new(File { name: file.name.clone(), tab: file.tab, shown: file.shown })),
            connected: Arc::default(),
            stop: Arc::default(),
        };
        let (app, file_info) = (app.clone(), connection.file.clone());
        let (connected, stop) = (connection.connected.clone(), connection.stop.clone());
        std::thread::spawn(move || keep_connected(&app, &file_info, &connected, &stop));
        state.connections.insert(file.key.clone(), connection);
    }
    let connected = state.connections.values().filter(|c| c.connected.load(Ordering::Relaxed)).count();
    let status = Status {
        on,
        files: files.len(),
        connected,
        version: VERSION.trim(),
        refused: on && connected == 0 && state.refused,
    };
    if state.status.as_ref() != Some(&status) {
        state.status = Some(status.clone());
        drop(state);
        if let Some(window) = app.get_webview_window(crate::settings::WINDOW) {
            let _ = window.eval(format!("window.showMcpStatus?.({})", serde_json::to_string(&status).unwrap()));
        }
    }
}

/// A random id, for sessions and messages
fn new_id() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as u64;
    let part = |salt: u64| {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u64(nanos ^ salt);
        hasher.finish()
    };
    format!("ligma-{:016x}{:016x}", part(0), part(u64::MAX))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

/// An envelope of `kind` as MessagePack, the way Figwright's
/// `createRequest`, `createResponse`, `createError` and `createEvent` make
/// them
fn envelope(kind: &str, id: &str, session: &str, fields: Value) -> Vec<u8> {
    let mut message = json!({ "v": PROTOCOL, "kind": kind, "id": id, "ts": now_ms(), "sessionId": session });
    if let (Some(message), Value::Object(fields)) = (message.as_object_mut(), fields) {
        message.extend(fields);
    }
    rmp_serde::to_vec(&message).unwrap_or_default()
}

fn decode(bytes: &[u8]) -> Option<Value> {
    rmp_serde::from_slice::<Value>(bytes).ok().filter(Value::is_object)
}

/// Keeps a file connected to the MCP server until `stop`. A dropped
/// connection comes back as the same session, so the server keeps tool
/// calls meant for it
fn keep_connected(app: &AppHandle, file: &Mutex<File>, connected: &AtomicBool, stop: &AtomicBool) {
    let session = new_id();
    while !stop.load(Ordering::Relaxed) {
        let mut wait = RETRY_AFTER;
        match connect(&session) {
            Ok(socket) => {
                log::info!("Figwright: file connected");
                state().lock().unwrap().refused = false;
                connected.store(true, Ordering::Relaxed);
                serve(app, socket, &session, file, stop);
                connected.store(false, Ordering::Relaxed);
                log::info!("Figwright: file disconnected");
            }
            Err(Some(refused)) => {
                log::warn!("Figwright: the MCP server refused Ligma: {refused}");
                state().lock().unwrap().refused = true;
                wait = REFUSED_RETRY_AFTER;
            }
            Err(None) => {}
        }
        // In steps, to stop soon after being asked
        let until = Instant::now() + wait;
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// Connects to the server and says hello as the plugin does. Fails with
/// the server's reason if it refused Ligma, or None if there's no server
fn connect(session: &str) -> Result<WebSocket<TcpStream>, Option<String>> {
    // The server listens on IPv4 unless told otherwise
    let addresses: [SocketAddr; 2] = [(Ipv4Addr::LOCALHOST, PORT).into(), (Ipv6Addr::LOCALHOST, PORT).into()];
    let stream = addresses
        .iter()
        .find_map(|address| TcpStream::connect_timeout(address, Duration::from_secs(1)).ok())
        .ok_or(None)?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).map_err(|_| None)?;
    stream.set_write_timeout(Some(Duration::from_secs(10))).map_err(|_| None)?;
    stream.set_nodelay(true).map_err(|_| None)?;
    // Without an Origin header, which the server takes as a client that
    // isn't a web page
    let (mut socket, _) = tungstenite::client(format!("ws://127.0.0.1:{PORT}/"), stream).map_err(|error| {
        log::warn!("Figwright: handshake failed: {error}");
        None
    })?;
    let hello = new_id();
    let params = json!({ "clientType": "plugin", "clientVersion": VERSION.trim(), "protocolVersion": PROTOCOL });
    socket
        .send(Message::binary(envelope("req", &hello, session, json!({ "method": "$hello", "params": params }))))
        .map_err(|_| None)?;
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        let reply = match socket.read() {
            Ok(Message::Binary(bytes)) => decode(&bytes),
            Ok(_) => continue,
            Err(_) => return Err(None),
        };
        let Some(reply) = reply.filter(|reply| reply["id"] == hello.as_str()) else { continue };
        if reply["kind"] == "res" {
            if let Some(notice) = reply["result"]["skewNotice"].as_str() {
                log::warn!("Figwright: {notice}");
            }
            return Ok(socket);
        }
        let reason = reply["error"]["message"].as_str().unwrap_or("hello rejected").to_string();
        return Err(Some(reason));
    }
    Err(None)
}

/// Answers the server's tool calls until the connection ends or `stop`
fn serve(app: &AppHandle, mut socket: WebSocket<TcpStream>, session: &str, file: &Mutex<File>, stop: &AtomicBool) {
    let (send, answers): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = mpsc::channel();
    let pending = Arc::new(AtomicUsize::new(0));
    let mut announced: Option<(String, bool)> = None;
    loop {
        if stop.load(Ordering::Relaxed) {
            // Normal closure: the file closed, so the server forgets it now
            let _ = socket.close(Some(CloseFrame { code: CloseCode::Normal, reason: "file closed".into() }));
            let _ = socket.flush();
            return;
        }
        // Which file this is, and whether it's on screen, so AI tools that
        // don't pick a file use the one in front. A file in a hidden tab
        // only says its name
        let File { name, tab, shown } = file.lock().unwrap().clone();
        if announced.as_ref() != Some(&(name.clone(), shown)) {
            let params = json!({ "fileName": name, "pageId": "", "pageName": "", "foreground": shown });
            let event = envelope("evt", &new_id(), session, json!({ "method": "$activity", "params": params }));
            if socket.send(Message::binary(event)).is_err() {
                return;
            }
            announced = Some((name, shown));
        }
        while let Ok(answer) = answers.try_recv() {
            if socket.send(Message::binary(answer)).is_err() {
                return;
            }
        }
        // Answers only come while tool calls are pending, so the socket is
        // read in short turns only then
        let turn = if pending.load(Ordering::Relaxed) > 0 { 20 } else { 500 };
        let _ = socket.get_ref().set_read_timeout(Some(Duration::from_millis(turn)));
        match socket.read() {
            Ok(Message::Binary(bytes)) => {
                let Some(request) = decode(&bytes).filter(|request| request["kind"] == "req") else { continue };
                let id = request["id"].as_str().unwrap_or_default().to_string();
                let session = request["sessionId"].as_str().unwrap_or(session).to_string();
                let method = request["method"].as_str().unwrap_or_default().to_string();
                if method == "$ping" {
                    let pong = envelope("res", &id, &session, json!({ "result": { "ok": true } }));
                    if socket.send(Message::binary(pong)).is_err() {
                        return;
                    }
                    continue;
                }
                let (app, send, pending) = (app.clone(), send.clone(), pending.clone());
                pending.fetch_add(1, Ordering::Relaxed);
                std::thread::spawn(move || {
                    let answer = match run(&app, tab, &id, &method, request.get("params")) {
                        Ok(Ok(result)) => envelope("res", &id, &session, json!({ "result": result })),
                        Ok(Err((code, message))) => envelope("err", &id, &session, json!({ "error": { "code": code, "message": message } })),
                        Err(message) => {
                            envelope("err", &id, &session, json!({ "error": { "code": "INTERNAL_ERROR", "message": message } }))
                        }
                    };
                    // Queued while still pending, so it goes out in a short turn
                    let _ = send.send(answer);
                    pending.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(error)) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                // Sends what's queued, like replies to the server's pings
                let _ = socket.flush();
            }
            Err(_) => return,
        }
    }
}

/// The plugin's answer to a tool call: its result, or its error's code and
/// message
type Answer = Result<Value, (String, String)>;

/// Runs tool `method` in tab `tab` and waits for the answer, or says why it
/// couldn't
fn run(app: &AppHandle, tab: u32, id: &str, method: &str, params: Option<&Value>) -> Result<Answer, String> {
    const ASLEEP: &str = "This file's tab in Ligma is asleep. Select it to wake it.";
    let mut params = params.cloned().unwrap_or(Value::Null);
    // The plugin then sends exported images as base64 rather than bytes,
    // which the server reads as well and which go through JSON
    if let Some(params) = params.as_object_mut() {
        params.remove("binary");
    }
    let call = json!({ "id": id, "method": method, "params": params });
    let start = format!("window.__ligmaMcp?.start({call}) ?? \"missing\"");
    let text = |json: String| serde_json::from_str::<String>(&json).ok();
    let loading_until = Instant::now() + LOADING_TIMEOUT;
    loop {
        let mut started = text(eval(app, tab, &start).ok_or(ASLEEP)?);
        // Set up in the tab on its first tool call
        if started.as_deref() == Some("missing") {
            started = text(eval(app, tab, &format!("{}\n;{start}", runner())).ok_or(ASLEEP)?);
        }
        match started.as_deref() {
            Some("ok") => break,
            Some("loading") if Instant::now() < loading_until => std::thread::sleep(Duration::from_millis(500)),
            Some("loading") => return Err("This file is still loading in Ligma. Try again once it shows.".into()),
            Some("missing") | None => return Err("Figwright's plugin couldn't be loaded in this tab".into()),
            Some(error) => return Err(error.into()),
        }
    }
    // Not `??`: null means the answer isn't there yet
    let id = serde_json::to_string(id).unwrap();
    let take = format!("window.__ligmaMcp ? window.__ligmaMcp.take({id}) : \"missing\"");
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let mut wait = Duration::from_millis(5);
    while Instant::now() < deadline {
        std::thread::sleep(wait);
        wait = (wait * 2).min(Duration::from_millis(50));
        let result = eval(app, tab, &take).ok_or(ASLEEP)?;
        let answer = match serde_json::from_str::<Option<String>>(&result) {
            Ok(Some(answer)) if answer == "missing" => return Err("This file's tab in Ligma reloaded before answering".into()),
            Ok(Some(answer)) => answer,
            // Null, and on macOS nothing, while there's no answer yet
            _ => continue,
        };
        let answer: Value = serde_json::from_str(&answer).map_err(|error| format!("The answer couldn't be read: {error}"))?;
        return Ok(if answer["kind"] == "tool-result" {
            Ok(answer.get("result").cloned().unwrap_or(Value::Null))
        } else {
            let field = |key: &str, default: &str| answer[key].as_str().unwrap_or(default).to_string();
            Err((field("code", "INTERNAL_ERROR"), field("message", "The plugin failed")))
        });
    }
    Err("Tool call timed out (3 minutes)".into())
}

/// Result of `script` in tab `tab`, as JSON, or None if the tab can't run
/// JavaScript now
fn eval(app: &AppHandle, tab: u32, script: &str) -> Option<String> {
    let webview: Webview = crate::tabs::scriptable(app, tab)?;
    let (send, receive) = mpsc::channel();
    webview
        .eval_with_callback(script, move |result| {
            let _ = send.send(result);
        })
        .ok()?;
    receive.recv_timeout(Duration::from_secs(30)).ok()
}

/// `RUNNER_JS` with the plugin's code in it
fn runner() -> &'static str {
    static RUNNER: OnceLock<String> = OnceLock::new();
    RUNNER.get_or_init(|| {
        RUNNER_JS
            .replace("__FIND_HOST__", crate::plugins::FIND_HOST_JS)
            .replace("__PLUGIN_JS__", PLUGIN_JS)
    })
}

/// Sets up `window.__ligmaMcp` in a Figma tab, which answers the MCP
/// server's tool calls with the plugin's own code:
///
/// - `start({ id, method, params })` hands a tool call to the plugin, and
///   returns "ok", "loading" while the file loads, or why it can't
/// - `take(id)` returns its answer once there is one, else null
/// - `busy()` is true while tool calls are pending and for a few minutes
///   after, so the tab isn't frozen or put to sleep meanwhile
///
/// The plugin gets `figma` as Figma's console plugin API, `window.figma`,
/// read again for each tool call, as a plugin run from Figma's menu or
/// from Ligma's development plugins can close it and make a new one. A
/// closed one is made again the way plugins.rs does
///
/// The plugin's window would pass tool calls in and answers out, and the
/// plugin sends it a "yield-request" in long reads, to let Figma draw. Its
/// answer comes from a message task rather than a timer, which a hidden
/// tab holds back (see `cooperative.ts` in Figwright). The plugin's
/// events, its storage and the rest of its window do nothing: Ligma has no
/// use for them
const RUNNER_JS: &str = r#"(() => {
  if (window.__ligmaMcp) return;
  const BUSY_FOR = 5 * 60 * 1000;
  const BRIDGE = "@figwright/bridge";
  const YIELD = "@figwright/yield";
  const answers = new Map();
  const waiting = new Set();
  let api = null;
  let onmessage = null;
  let lastUsed = 0;
  const turns = new MessageChannel();
  turns.port1.onmessage = ({ data: id }) => {
    try {
      onmessage?.({ tag: YIELD, kind: "yield-resume", id, background: document.hidden });
    } catch (error) {
      console.error(error);
    }
  };
  const ui = {
    postMessage(message) {
      if (message?.tag === YIELD && message.kind === "yield-request") {
        turns.port2.postMessage(message.id);
      } else if (message?.tag === BRIDGE && waiting.has(message.id)) {
        waiting.delete(message.id);
        lastUsed = Date.now();
        answers.set(message.id, message);
      }
    },
    get onmessage() { return onmessage; },
    set onmessage(handler) { onmessage = handler; },
    show() {}, hide() {}, resize() {}, reposition() {}, close() {},
    on() {}, once() {}, off() {},
  };
  const memory = new Map();
  const overrides = {
    showUI() {},
    ui,
    on() {}, once() {}, off() {},
    closePlugin() {},
    clientStorage: {
      getAsync: async (key) => memory.get(key),
      setAsync: async (key, value) => { memory.set(key, value); },
      deleteAsync: async (key) => { memory.delete(key); },
      keysAsync: async () => [...memory.keys()],
    },
  };
  const figma = new Proxy({}, {
    get(_, key) {
      if (Object.hasOwn(overrides, key)) return overrides[key];
      const value = Reflect.get(api, key);
      return typeof value === "function" ? value.bind(api) : value;
    },
    set: (_, key, value) => Reflect.set(api, key, value),
    has: (_, key) => Object.hasOwn(overrides, key) || Reflect.has(api, key),
  });
  // On the first tool call, as the plugin reads the file as it starts
  let started = false;
  const load = () => {
    started = true;
    (function (figma, __html__) {
__PLUGIN_JS__
    })(figma, "");
  };

  const alive = (candidate) => {
    try {
      return candidate != null && candidate.currentPage != null;
    } catch {
      return false;
    }
  };
  const host = window.__ligmaPluginHost ??= (__FIND_HOST__)();
  const current = () => {
    let candidate = window.figma;
    if (candidate == null) return null;
    if (!alive(candidate) && host?.isConsole()) {
      host.close();
      host.reset();
      candidate = window.figma;
    }
    return alive(candidate) ? candidate : null;
  };

  // The file is drawn and Figma's progress bar gone, as READY_JS in tabs.rs
  const ready = () => !!document.querySelector("canvas") && !document.querySelector('[class*="progress_bar--"]');

  const fail = (id, error) => {
    waiting.delete(id);
    answers.set(id, { tag: BRIDGE, kind: "tool-error", id, code: "INTERNAL_ERROR", message: error instanceof Error ? error.message : String(error) });
  };

  window.__ligmaMcp = {
    start({ id, method, params }) {
      if (!ready()) return "loading";
      const found = current();
      if (found == null) {
        return window.figma == null
          ? "Figma's plugin API isn't available in this file. AI tools can only use files you can edit, once they've loaded."
          : "Figma's plugin API in this tab was closed. Reload the tab in Ligma (right-click it, then Reload) and try again.";
      }
      api = found;
      if (!started) {
        try {
          load();
        } catch (error) {
          console.error(error);
          return `Figwright's plugin couldn't start in this tab: ${error?.message ?? error}`;
        }
      }
      if (typeof onmessage !== "function") return "Figwright's plugin didn't start in this tab";
      waiting.add(id);
      lastUsed = Date.now();
      try {
        onmessage(params == null ? { tag: BRIDGE, kind: "tool-call", id, method } : { tag: BRIDGE, kind: "tool-call", id, method, params });
      } catch (error) {
        fail(id, error);
      }
      return "ok";
    },
    // As JSON text, like the plugin's window sends it: WebKit hands Ligma
    // other values as Objective-C objects, not all of which make JSON
    take(id) {
      if (!answers.has(id)) return null;
      const answer = answers.get(id);
      answers.delete(id);
      try {
        return JSON.stringify(answer);
      } catch (error) {
        return JSON.stringify({ tag: BRIDGE, kind: "tool-error", id, code: "INTERNAL_ERROR", message: `The answer couldn't be sent: ${error?.message ?? error}` });
      }
    },
    busy: () => waiting.size > 0 || Date.now() - lastUsed < BUSY_FOR,
  };
})()"#;

/// MCP server configuration for AI tools, pinned to `VERSION`
fn config() -> String {
    // Written out, as serde_json would put "args" before "command"
    format!(
        r#"{{
  "mcpServers": {{
    "{name}": {{
      "command": "npx",
      "args": ["-y", "{package}@{version}"]
    }}
  }}
}}"#,
        name = crate::mcp_clients::NAME,
        package = crate::mcp_clients::PACKAGE,
        version = VERSION.trim(),
    )
}

/// Claude Code's command adding the same server, for every project
pub fn claude_command() -> String {
    format!(
        "claude mcp add --scope user {} -- npx -y {}@{}",
        crate::mcp_clients::NAME,
        crate::mcp_clients::PACKAGE,
        VERSION.trim()
    )
}

#[tauri::command]
pub async fn get_mcp_status(app: AppHandle, webview: Webview) -> Option<Status> {
    if !crate::settings::from_settings_window(&webview) {
        return None;
    }
    sync(&app);
    state().lock().unwrap().status.clone()
}

/// The MCP server's configuration (`"json"`) or Claude Code's command
/// (`"claude"`), shown in Settings for adding it by hand
#[tauri::command]
pub async fn get_mcp_config(webview: Webview, kind: String) -> Option<String> {
    if !crate::settings::from_settings_window(&webview) {
        return None;
    }
    Some(match kind.as_str() {
        "claude" => claude_command(),
        _ => config(),
    })
}

/// Copies the MCP server's configuration (`"json"`) or Claude Code's
/// command (`"claude"`)
#[tauri::command]
pub async fn copy_mcp_config(app: AppHandle, webview: Webview, kind: String) -> Result<(), String> {
    if !crate::settings::from_settings_window(&webview) {
        return Err("not allowed".into());
    }
    let text = match kind.as_str() {
        "claude" => claude_command(),
        _ => config(),
    };
    app.clipboard().write_text(text).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_round_trip_as_messagepack() {
        let bytes = envelope("err", "a", "s", json!({ "error": { "code": "X", "message": "m" } }));
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded["v"], PROTOCOL);
        assert_eq!(decoded["kind"], "err");
        assert_eq!(decoded["sessionId"], "s");
        assert_eq!(decoded["error"]["code"], "X");
        assert!(decoded["ts"].as_u64().is_some());
    }

    #[test]
    fn ids_differ() {
        assert_ne!(new_id(), new_id());
    }
}
