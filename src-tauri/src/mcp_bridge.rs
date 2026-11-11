//! Ligma MCP, AI tools in Figma files (`ligmaMcp`), through Figwright
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
//! What AI tools do in a file shows in Figma's right panel, in its MCP
//! section, which Ligma takes over as "Ligma MCP" while the setting is on
//! (`PANEL_JS`). It's on by default, so that section is where people find
//! it
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
    ENABLED.store(settings.ligma_mcp, Ordering::Relaxed);
}

pub fn apply_settings(app: &AppHandle, settings: &crate::settings::Settings) {
    init(settings);
    // Settings shows the change right away rather than at the next check
    sync(app);
    // Figma's own MCP section comes back
    if !settings.ligma_mcp {
        let off = PanelStatus { on: false, connected: false, refused: false, version: VERSION.trim() };
        let script = format!("window.__ligmaMcpLog?.status({})", serde_json::to_string(&off).unwrap());
        for webview in crate::tabs::scriptable_tabs(app) {
            let _ = webview.eval(&script);
        }
    }
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

/// What a tab's MCP section shows (see `PANEL_JS`)
#[derive(Serialize)]
struct PanelStatus {
    on: bool,
    connected: bool,
    refused: bool,
    version: &'static str,
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
    // Each tab's MCP section, every check, so a tab that reloaded is told
    // again. A call that does nothing, so it's cheap
    for connection in state.connections.values() {
        let tab = connection.file.lock().unwrap().tab;
        let panel = PanelStatus {
            on,
            connected: connection.connected.load(Ordering::Relaxed),
            refused: state.refused,
            version: VERSION.trim(),
        };
        if let Some(webview) = crate::tabs::scriptable(app, tab) {
            let _ = webview.eval(format!("window.__ligmaMcpLog?.status({})", serde_json::to_string(&panel).unwrap()));
        }
    }
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
        logged("end", message.id, message);
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

  // For the MCP section's list of calls (see `PANEL_JS`), which mustn't
  // get in the way of the call itself
  const logged = (what, ...args) => {
    try {
      window.__ligmaMcpLog?.[what](...args);
    } catch (error) {
      console.warn(error);
    }
  };

  const fail = (id, error) => {
    waiting.delete(id);
    const answer = { tag: BRIDGE, kind: "tool-error", id, code: "INTERNAL_ERROR", message: error instanceof Error ? error.message : String(error) };
    answers.set(id, answer);
    logged("end", id, answer);
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
      logged("start", id, method, params);
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

/// Runs in Figma tabs. Sets `window.__ligmaMcpLog`, which the runner tells
/// of each tool call as it starts and ends, and Ligma of the setting and
/// the file's connection, with `status`. While the setting is on, it puts
/// its own MCP section in place of Figma's: a copy of Figma's heading,
/// titled "Ligma MCP" so it isn't taken for Figma's own, whose chip says whether the file is connected and whose button opens
/// Settings at AI tools, over the latest calls in this file. The heading
/// collapses it, as Figma's collapsible sections do, and that's
/// remembered. Three calls show, and "Show more" lists them all in a
/// scrolling box no taller than eight rows. Each call opens to show when it
/// ran and its arguments, or why it failed, and selects the layers it
/// named or made. Calls are kept in the tab's `sessionStorage`. Figma only
/// shows its section while nothing is selected, so otherwise Ligma's goes
/// under the selected layers' heading
pub const PANEL_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window || window.__ligmaMcpLog) return;
  // Calls kept per file, rows shown before "Show more", and the most rows
  // shown at once after it, which then scroll
  const KEEP = 50;
  const SHOWN = 3;
  const TALLEST = 8;
  const COLLAPSED = "ligma-mcp-collapsed";
  const ANCHOR = '[data-onboarding-key="design-mode-mcp-agents-panel-callout-target"]';
  const PANEL = '[class*="properties_panel--propertiesPanel--"]';
  const READ = /^(get|search|scan|list|ping|find)(_|$)/;

  // The file's calls, newest first, kept for the tab's life in
  // sessionStorage so a reload doesn't lose them
  const fileKey = () => location.pathname.split("/")[2] ?? "";
  let loadedFor = null;
  let calls = [];
  const load = () => {
    if (loadedFor === fileKey()) return;
    loadedFor = fileKey();
    try {
      calls = JSON.parse(sessionStorage.getItem(`ligma-mcp-log:${loadedFor}`)) ?? [];
    } catch {
      calls = [];
    }
    for (const call of calls) {
      if (call.status !== "pending") continue;
      call.status = "error";
      call.error = "The page reloaded before the call was answered";
    }
  };
  let saving = 0;
  const save = () => {
    clearTimeout(saving);
    saving = setTimeout(() => {
      try {
        sessionStorage.setItem(`ligma-mcp-log:${loadedFor}`, JSON.stringify(calls));
      } catch {}
    }, 300);
  };

  // Layers a call named, as Figwright's panel finds them
  const NODE_ID = /^[A-Za-z]?\d+:\d+(?:;[A-Za-z]?\d+:\d+)*$/;
  const nodeIds = (value, out = new Set(), depth = 0) => {
    if (depth > 6 || out.size >= 50 || value === null || typeof value !== "object") return out;
    if (Array.isArray(value)) {
      for (const item of value) nodeIds(item, out, depth + 1);
      return out;
    }
    for (const [key, child] of Object.entries(value)) {
      if (key === "nodeId" && typeof child === "string" && NODE_ID.test(child)) out.add(child);
      if (key === "nodeIds" && Array.isArray(child)) {
        for (const id of child) if (typeof id === "string" && NODE_ID.test(id)) out.add(id);
      }
      nodeIds(child, out, depth + 1);
    }
    return out;
  };

  // A few of the call's arguments, short enough to read in the panel
  const summarize = (params) => {
    if (params === null || typeof params !== "object") return [];
    return Object.entries(params)
      .filter(([, value]) => value !== undefined)
      .slice(0, 6)
      .map(([key, value]) => {
        let text = typeof value === "string" ? value : JSON.stringify(value) ?? String(value);
        if (text.length > 80) text = `${text.slice(0, 80)}…`;
        return [key, text];
      });
  };

  let status = null;
  const expanded = new Set();
  let showAll = false;
  let collapsed = false;
  try {
    collapsed = localStorage.getItem(COLLAPSED) === "1";
  } catch {}

  window.__ligmaMcpLog = {
    start(id, method, params) {
      load();
      calls.unshift({ id, method, args: summarize(params), nodeIds: [...nodeIds(params)], startedAt: Date.now(), status: "pending" });
      calls.length = Math.min(calls.length, KEEP);
      save();
      render();
    },
    end(id, reply) {
      load();
      const call = calls.find((call) => call.id === id);
      if (!call || call.status !== "pending") return;
      call.endedAt = Date.now();
      if (reply?.kind === "tool-result") {
        call.status = "ok";
        // What a write made or changed, which reads only look at
        if (!READ.test(call.method)) call.nodeIds = [...nodeIds(reply.result, new Set(call.nodeIds))];
      } else {
        call.status = "error";
        // Without the tool's name in front, which the row already shows
        const message = String(reply?.message ?? "The call failed");
        call.error = message.startsWith(`${call.method}: `) ? message.slice(call.method.length + 2) : message;
      }
      save();
      render();
    },
    // Sent by Ligma every couple of seconds: whether the setting is on,
    // and whether this file is connected to the MCP server. Each one also
    // puts the section back if Figma's page dropped it
    status(next) {
      const changed = JSON.stringify(next) !== JSON.stringify(status);
      status = next;
      watch();
      if (changed) render();
    },
  };

  // "create_component" as "Create component"
  const ACRONYMS = { svg: "SVG", pdf: "PDF", id: "ID", ids: "IDs", url: "URL", css: "CSS", api: "API" };
  const title = (method) => {
    const words = method.split("_").filter(Boolean).map((word) => ACRONYMS[word] ?? word);
    if (!words.length) return method;
    return words[0][0].toUpperCase() + words[0].slice(1) + (words.length > 1 ? ` ${words.slice(1).join(" ")}` : "");
  };
  const kind = (method) => {
    if (READ.test(method)) return "read";
    if (/^(create|import|clone|add|combine|duplicate|group|boolean|flatten|repeat)(_|$)/.test(method)) return "create";
    if (/^(delete|remove)(_|$)/.test(method)) return "delete";
    if (/^(export|save)(_|$)/.test(method)) return "export";
    return "edit";
  };

  // Drawn in Figma's UI3 icon grammar: 24 by 24, thin strokes
  const svg = (paths) =>
    `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${paths}</svg>`;
  const ICONS = {
    read: svg('<circle cx="11" cy="11" r="4.5"/><path d="m14.5 14.5 3.5 3.5"/>'),
    create: svg('<path d="M12 6.5v11M6.5 12h11"/>'),
    edit: svg('<path d="m14.5 7 2.5 2.5-8 8H6.5V15z"/>'),
    delete: svg('<path d="M7 8.5h10M10.5 8.5V7h3v1.5M8.5 8.5l.7 9h5.6l.7-9"/>'),
    export: svg('<path d="M12 14.5v-8M9 9.5l3-3 3 3M7 13v4h10v-4"/>'),
    error: svg('<circle cx="12" cy="12" r="5.5"/><path d="M12 9.2v3.3"/><circle cx="12" cy="14.8" r=".4" fill="currentColor"/>'),
    pending: svg('<circle cx="12" cy="12" r="5.5" stroke-opacity=".3"/><path d="M12 6.5a5.5 5.5 0 0 1 5.5 5.5"/>'),
    select: svg('<circle cx="12" cy="12" r="4"/><path d="M12 5v3M12 16v3M5 12h3M16 12h3"/>'),
  };

  const since = (time) => {
    const seconds = Math.max(0, Math.round((Date.now() - time) / 1000));
    if (seconds < 10) return "now";
    if (seconds < 60) return `${seconds}s`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
    if (seconds < 86400) return `${Math.floor(seconds / 3600)}h`;
    return new Date(time).toLocaleDateString(undefined, { month: "short", day: "numeric" });
  };
  const clock = (time) => new Date(time).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit", second: "2-digit" });
  const took = (call) => {
    const ms = call.endedAt - call.startedAt;
    return ms < 1000 ? `${ms}ms` : `${(ms / 1000).toFixed(1)}s`;
  };

  // Selects the layers and zooms to them, going to their page if need be,
  // as Figwright's panel does
  const reveal = async (ids) => {
    const figma = window.figma;
    try {
      const found = await Promise.all(ids.map((id) => figma.getNodeByIdAsync(id).catch(() => null)));
      const nodes = found.filter((node) => node && !node.removed && node.type !== "PAGE" && node.type !== "DOCUMENT");
      const pageOf = (node) => {
        while (node && node.type !== "PAGE") node = node.parent;
        return node;
      };
      const page = nodes.length ? pageOf(nodes[0]) : null;
      if (!page) {
        figma.notify("Those layers aren't in this file anymore");
        return;
      }
      const shown = nodes.filter((node) => pageOf(node) === page);
      if (page !== figma.currentPage) await figma.setCurrentPageAsync(page);
      figma.currentPage.selection = shown;
      figma.viewport.scrollAndZoomIntoView(shown);
      if (figma.viewport.zoom > 4) figma.viewport.zoom = 4;
    } catch (error) {
      console.warn("Ligma: couldn't select the layers", error);
    }
  };

  // Asks Ligma to open Settings at AI tools (see `on_signal`): through
  // `window.open` on Windows, and on macOS as an iframe load, which
  // WKWebView reports to `on_navigation`
  const openSettings = () => {
    const url = "https://mcp.ligma.invalid/settings";
    if (navigator.userAgent.includes("Mac")) {
      const frame = document.createElement("iframe");
      frame.style.display = "none";
      frame.src = url;
      document.documentElement.appendChild(frame);
      setTimeout(() => frame.remove(), 1000);
    } else {
      window.open(url, "_blank");
    }
  };

  const style = document.createElement("style");
  style.textContent = `
    ${ANCHOR}:not([data-ligma-mcp]) { display: none !important; }
    [data-ligma-mcp] .ligma-mcp-list { display: flex; flex-direction: column; padding: 0 8px 8px; }
    [data-ligma-mcp] .ligma-mcp-list[hidden], [data-ligma-mcp][data-collapsed] .ligma-mcp-list { display: none; }
    [data-ligma-mcp] .ligma-mcp-rows { display: flex; flex-direction: column; }
    [data-ligma-mcp] .ligma-mcp-rows[data-all] {
      max-height: ${TALLEST * 26}px; overflow-y: auto; overscroll-behavior: contain;
      scrollbar-width: thin; scrollbar-color: var(--color-border) transparent;
    }
    [data-ligma-mcp] .ligma-mcp-rows[data-below] { mask-image: linear-gradient(to bottom, #000 calc(100% - 24px), transparent); }
    [data-ligma-mcp] .ligma-mcp-heading { cursor: default; }
    [data-ligma-mcp] .ligma-mcp-heading:focus-visible { outline: 1px solid var(--color-border-selected); outline-offset: 1px; border-radius: 4px; }
    [data-ligma-mcp] .ligma-mcp-chevron { opacity: 1; visibility: hidden; transition: transform .12s ease-out; }
    [data-ligma-mcp] .ligma-mcp-heading:is(:hover, :focus-visible) .ligma-mcp-chevron { visibility: visible; }
    [data-ligma-mcp]:not([data-collapsed]) .ligma-mcp-chevron { transform: rotate(90deg); }
    [data-ligma-mcp] .ligma-mcp-call { position: relative; }
    [data-ligma-mcp] button.ligma-mcp-row {
      all: unset; box-sizing: border-box; display: flex; align-items: center; width: 100%; height: 26px;
      padding: 0 8px 0 4px; border-radius: 5px; color: var(--color-text); cursor: default;
    }
    [data-ligma-mcp] .ligma-mcp-row:hover, [data-ligma-mcp] .ligma-mcp-call:has(.ligma-mcp-select:hover) .ligma-mcp-row { background: var(--color-bg-hover); }
    [data-ligma-mcp] :is(.ligma-mcp-row, .ligma-mcp-select, .ligma-mcp-more):focus-visible { outline: 1px solid var(--color-border-selected); outline-offset: -1px; }
    [data-ligma-mcp] .ligma-mcp-icon { flex: none; display: grid; place-items: center; width: 24px; height: 24px; color: var(--color-icon-secondary); }
    [data-ligma-mcp] [data-status="error"] .ligma-mcp-icon { color: var(--color-icon-danger); }
    [data-ligma-mcp] [data-status="pending"] .ligma-mcp-icon svg { animation: ligma-mcp-spin .8s linear infinite; }
    [data-ligma-mcp] .ligma-mcp-name { flex: 1; min-width: 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; padding-left: 4px; }
    [data-ligma-mcp] [data-kind="read"] .ligma-mcp-name { color: var(--color-text-secondary); }
    [data-ligma-mcp] .ligma-mcp-count { color: var(--color-text-tertiary); margin-left: 4px; font-variant-numeric: tabular-nums; }
    [data-ligma-mcp] .ligma-mcp-meta { flex: none; margin-left: 8px; color: var(--color-text-secondary); font-variant-numeric: tabular-nums; }
    [data-ligma-mcp] [data-status="error"] .ligma-mcp-meta { color: var(--color-text-danger); }
    [data-ligma-mcp] button.ligma-mcp-select {
      all: unset; box-sizing: border-box; position: absolute; top: 1px; right: 1px; display: none; place-items: center;
      width: 24px; height: 24px; border-radius: 4px; color: var(--color-icon-secondary);
    }
    [data-ligma-mcp] .ligma-mcp-select:hover { color: var(--color-icon); background: var(--color-bg-pressed); }
    [data-ligma-mcp] .ligma-mcp-call:is(:hover, :focus-within) .ligma-mcp-select { display: grid; }
    [data-ligma-mcp] .ligma-mcp-call:is(:hover, :focus-within):has(.ligma-mcp-select) .ligma-mcp-meta { visibility: hidden; }
    [data-ligma-mcp] .ligma-mcp-details { padding: 2px 8px 6px 32px; color: var(--color-text-secondary); }
    [data-ligma-mcp] .ligma-mcp-details p { margin: 0; overflow-wrap: anywhere; }
    [data-ligma-mcp] .ligma-mcp-error { color: var(--color-text-danger); margin-bottom: 4px !important; }
    [data-ligma-mcp] .ligma-mcp-when { font-variant-numeric: tabular-nums; }
    [data-ligma-mcp] .ligma-mcp-args { display: grid; grid-template-columns: auto minmax(0, 1fr); column-gap: 8px; margin: 4px 0 0; }
    [data-ligma-mcp] .ligma-mcp-args dt { color: var(--color-text-tertiary); }
    [data-ligma-mcp] .ligma-mcp-args dd { margin: 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
    [data-ligma-mcp] button.ligma-mcp-more {
      all: unset; box-sizing: border-box; height: 24px; padding: 0 8px 0 32px; border-radius: 5px; color: var(--color-text-secondary);
    }
    [data-ligma-mcp] .ligma-mcp-more:hover { color: var(--color-text); }
    [data-ligma-mcp] .ligma-mcp-chip {
      display: inline-flex; align-items: center; height: 16px; padding: 0 4px; border-radius: 5px;
      box-shadow: inset 0 0 0 1px var(--color-border); font-weight: 450;
    }
    @keyframes ligma-mcp-spin { to { transform: rotate(360deg); } }
    @media (prefers-reduced-motion: reduce) {
      [data-ligma-mcp] [data-status="pending"] .ligma-mcp-icon svg { animation: none; }
      [data-ligma-mcp] .ligma-mcp-chevron { transition: none; }
    }
  `;

  // Figma's own section, copied as it's first seen so ours looks the same,
  // with its heading, chip and button
  let template = null;
  let root = null;
  let chip = null;
  let chipHolder = null;
  let heading = null;
  let list = null;
  let rowsBox = null;
  const toggle = () => {
    collapsed = !collapsed;
    try {
      localStorage.setItem(COLLAPSED, collapsed ? "1" : "0");
    } catch {}
    render();
  };
  const build = () => {
    if (root || !template) return;
    root = template.cloneNode(true);
    root.removeAttribute("data-onboarding-key");
    root.setAttribute("data-ligma-mcp", "");
    for (const element of root.querySelectorAll("[id]")) element.removeAttribute("id");
    chip = root.querySelector("strong span") ?? root.querySelector("strong");
    chipHolder = root.querySelector("[data-tooltip]:has(strong)");
    if (!chip) {
      // A heading without Figma's chip, as when it shows something else
      const heading = [...root.querySelectorAll("span")].find((span) => span.textContent.trim() === "MCP");
      chipHolder = document.createElement("span");
      chipHolder.style.marginLeft = "4px";
      chip = document.createElement("span");
      chip.className = "ligma-mcp-chip";
      chipHolder.append(chip);
      heading?.after(chipHolder);
    }
    // Named apart from Figma's own MCP, which it takes the place of
    const name = [...root.querySelectorAll("i18n-text, span")].find((node) => node.childElementCount === 0 && node.textContent.trim() === "MCP");
    if (name) name.textContent = "Ligma MCP";
    const button = root.querySelector("button");
    if (button) {
      button.setAttribute("aria-label", "Set up AI tools");
      button.setAttribute("data-tooltip", "Set up AI tools");
      button.addEventListener("click", openSettings);
    }
    // The heading opens and closes the section, with the chevron Figma's
    // collapsible sections show beside it on hover
    heading = root.querySelector('[class*="panel_title--panelTitle"]');
    const chevron = root.querySelector('[class*="collapsible_panel--icon"]');
    if (chevron) {
      chevron.classList.remove(...[...chevron.classList].filter((name) => name.includes("hiddenIcon")));
      chevron.classList.add("ligma-mcp-chevron");
    }
    if (heading) {
      heading.classList.add("ligma-mcp-heading");
      heading.setAttribute("role", "button");
      heading.tabIndex = 0;
      heading.addEventListener("click", toggle);
      heading.addEventListener("keydown", (event) => {
        if (event.key !== "Enter" && event.key !== " ") return;
        event.preventDefault();
        toggle();
      });
    }
    list = element("div", "ligma-mcp-list");
    rowsBox = element("div", "ligma-mcp-rows");
    rowsBox.setAttribute("role", "list");
    rowsBox.setAttribute("aria-label", "AI tool activity in this file");
    rowsBox.addEventListener("scroll", fade, { passive: true });
    list.append(rowsBox);
    root.append(list);
    // Figma acts on Enter and Space itself, as shortcuts
    root.addEventListener("keydown", (event) => {
      if ((event.key === "Enter" || event.key === " ") && event.target.closest('button, [role="button"]')) event.stopPropagation();
    });
    render();
  };

  // Consecutive calls of the same tool that went through share a row
  const groups = () => {
    const out = [];
    for (const call of calls) {
      const last = out.at(-1);
      if (last && call.status === "ok" && last.status === "ok" && last.method === call.method) last.calls.push(call);
      else out.push({ id: call.id, method: call.method, status: call.status, calls: [call] });
    }
    return out;
  };

  const element = (tag, className, text) => {
    const made = document.createElement(tag);
    if (className) made.className = className;
    if (text !== undefined) made.textContent = text;
    return made;
  };

  const row = (group) => {
    const [call] = group.calls;
    const item = element("div", "ligma-mcp-call");
    item.setAttribute("role", "listitem");
    item.dataset.status = group.status;
    item.dataset.kind = kind(group.method);
    const open = expanded.has(group.id);
    const button = element("button", "ligma-mcp-row");
    button.type = "button";
    button.setAttribute("aria-expanded", open);
    const icon = element("span", "ligma-mcp-icon");
    icon.innerHTML = ICONS[group.status === "ok" ? kind(group.method) : group.status];
    const name = element("span", "ligma-mcp-name", title(group.method));
    if (group.calls.length > 1) name.append(element("span", "ligma-mcp-count", `×${group.calls.length}`));
    const meta = element("span", "ligma-mcp-meta", group.status === "error" ? "Failed" : group.status === "pending" ? "" : since(call.endedAt ?? call.startedAt));
    button.append(icon, name, meta);
    button.addEventListener("click", () => {
      if (expanded.has(group.id)) expanded.delete(group.id);
      else expanded.add(group.id);
      render();
    });
    item.append(button);

    const ids = [...new Set(group.calls.flatMap((call) => call.nodeIds ?? []))];
    if (ids.length) {
      const select = element("button", "ligma-mcp-select");
      select.type = "button";
      select.innerHTML = ICONS.select;
      const label = ids.length === 1 ? "Select its layer" : `Select its ${ids.length} layers`;
      select.setAttribute("aria-label", label);
      select.setAttribute("data-tooltip", label);
      select.setAttribute("data-tooltip-type", "text");
      select.addEventListener("click", () => reveal(ids));
      item.append(select);
    }

    if (open) {
      const details = element("div", "ligma-mcp-details");
      if (call.error) details.append(element("p", "ligma-mcp-error", call.error));
      const when =
        group.calls.length > 1
          ? `${group.calls.length} calls, the last at ${clock(call.startedAt)}`
          : call.endedAt
            ? `${clock(call.startedAt)}, took ${took(call)}`
            : `Started at ${clock(call.startedAt)}`;
      details.append(element("p", "ligma-mcp-when", when));
      if (call.args?.length) {
        const args = element("dl", "ligma-mcp-args");
        for (const [key, value] of call.args) {
          const dd = element("dd", "", value);
          dd.title = value;
          args.append(element("dt", "", key), dd);
        }
        details.append(args);
      }
      item.append(details);
    }
    return item;
  };

  const CHIP = {
    connected: ["Connected", (version) => `Figwright ${version}. AI tools can read and edit this file through Ligma.`],
    working: ["Working", () => "An AI tool is using this file"],
    waiting: ["Not connected", () => "Ligma connects this file once an AI tool set up with Figwright starts. Set one up with the button on the right."],
    refused: ["Other version", (version) => `Your AI tool runs another version of Figwright's MCP server. Ligma needs ${version}.`],
  };

  const render = () => {
    if (!root) return;
    load();
    const state = !status?.connected
      ? status?.refused ? "refused" : "waiting"
      : calls.some((call) => call.status === "pending") ? "working" : "connected";
    const [text, tip] = CHIP[state];
    chip.textContent = text;
    chipHolder?.setAttribute("data-tooltip", tip(status?.version ?? ""));
    chipHolder?.setAttribute("data-tooltip-type", "text");
    root.toggleAttribute("data-collapsed", collapsed);
    heading?.setAttribute("aria-expanded", !collapsed);
    const all = groups();
    list.hidden = !all.length;
    const extra = all.length > SHOWN;
    if (!extra) showAll = false;
    // Keeps focus on the button it was on, and the list where it was
    // scrolled to, as the list is redrawn
    const buttons = () => [...list.querySelectorAll("button")];
    const focused = list.contains(document.activeElement) ? buttons().indexOf(document.activeElement) : -1;
    const scrolled = rowsBox.scrollTop;
    rowsBox.toggleAttribute("data-all", showAll);
    rowsBox.replaceChildren(...(showAll ? all : all.slice(0, SHOWN)).map(row));
    list.querySelector(".ligma-mcp-more")?.remove();
    if (extra) {
      const more = element("button", "ligma-mcp-more", showAll ? "Show less" : `Show ${all.length - SHOWN} more`);
      more.type = "button";
      more.setAttribute("aria-expanded", showAll);
      more.addEventListener("click", () => {
        showAll = !showAll;
        rowsBox.scrollTop = 0;
        render();
      });
      list.append(more);
    }
    rowsBox.scrollTop = showAll ? scrolled : 0;
    fade();
    if (focused >= 0) buttons()[focused]?.focus();
  };

  // A fade at the foot of the list while there's more to scroll to
  const fade = () => {
    const below = rowsBox.hasAttribute("data-all") && rowsBox.scrollTop + rowsBox.clientHeight < rowsBox.scrollHeight - 1;
    rowsBox.toggleAttribute("data-below", below);
  };

  // Ours goes where Figma's MCP section is, which shows while nothing is
  // selected, and under the selected layers' heading otherwise, where it's
  // still in view
  const place = () => {
    if (!status?.on) {
      root?.remove();
      style.remove();
      return;
    }
    if (!style.isConnected) (document.head ?? document.documentElement).append(style);
    const anchor = document.querySelector(`${ANCHOR}:not([data-ligma-mcp])`);
    if (anchor && !template) {
      template = anchor.cloneNode(true);
      build();
    }
    if (!root) return;
    if (anchor) {
      if (anchor.nextElementSibling !== root) anchor.after(root);
      return;
    }
    const panel = document.querySelector(PANEL);
    const head = panel && [...panel.children].find((child) => child !== root);
    if (!head) root.remove();
    else if (head.nextElementSibling !== root) head.after(root);
  };

  let observer = null;
  let queued = false;
  let ticking = 0;
  const watch = () => {
    if (status?.on && !observer) {
      observer = new MutationObserver(() => {
        if (queued) return;
        queued = true;
        requestAnimationFrame(() => {
          queued = false;
          place();
        });
      });
      // The document itself: Figma's page replaces what's in it as it loads
      observer.observe(document, { childList: true, subtree: true });
      // Times like "2m" move on
      ticking = setInterval(() => root?.isConnected && !document.hidden && render(), 15000);
    } else if (!status?.on && observer) {
      observer.disconnect();
      observer = null;
      clearInterval(ticking);
    }
    place();
  };
})()"##;

/// Origin of the links `PANEL_JS` opens to ask for Settings
pub const SIGNAL_HOST: &str = "mcp.ligma.invalid";

/// A click on the MCP section's button: opens Settings at AI tools. Not
/// from within the webview's own callback, which is still deciding on the
/// page
pub fn on_signal(app: &AppHandle, url: &tauri::Url) {
    if url.path() != "/settings" {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(error) = crate::settings::open_window_at(&app, Some("ai-tools")) {
            log::warn!("Couldn't open Settings: {error}");
        }
    });
}

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
