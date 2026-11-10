//! AI tools in Figma files (`mcpBridge`), through Figma MCP Bridge
//! (github.com/gethopp/figma-mcp-bridge) rather than Figma's own MCP
//! server, which a free plan can only call 6 times a month
//!
//! The bridge is an MCP server, `@gethopp/figma-mcp-bridge` on npm, that AI
//! tools start themselves, and a plugin run in each Figma file. Its server
//! listens on `127.0.0.1:1994` and sends the plugin requests over a
//! WebSocket, one connection per file. Ligma takes the plugin's place: it
//! connects each file open in a tab, and runs the plugin's own code
//! (`vendor/figma-mcp-bridge/code.js`) in the tab, without its window,
//! against the plugin API Figma's web editor offers on the page (see
//! plugins.rs). So there's nothing to run in Figma, and no plugin window
//!
//! That API only exists in files the user can edit. Requests are answered
//! in the page, then read back by polling, as `eval` can't wait for a
//! promise. A tab an AI tool has used in the last few minutes isn't frozen
//! or put to sleep (see `CAN_REST_JS` in tabs.rs). `run_script` is refused:
//! the code would run with figma.com's access, not in a plugin sandbox

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager, Webview};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tungstenite::{Message, WebSocket};

/// Version of the vendored plugin code. The MCP server is pinned to it, as
/// its tools have to match
pub const VERSION: &str = include_str!("../vendor/figma-mcp-bridge/VERSION");

const PLUGIN_JS: &str = include_str!("../vendor/figma-mcp-bridge/code.js");

/// The bridge server's port (`FIGMA_BRIDGE_PORT` changes it there, not here)
const PORT: u16 = 1994;

/// How often the open files are matched with connections
const CHECK_EVERY: Duration = Duration::from_secs(2);

/// Wait before connecting again after a failed or lost connection
const RETRY_AFTER: Duration = Duration::from_secs(3);

/// Longest wait for a file still loading before a request
const LOADING_TIMEOUT: Duration = Duration::from_secs(60);

/// Longest wait for a request's answer. The server gives up at 3 minutes
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

/// A file's connection to the bridge server
struct Connection {
    name: String,
    /// Tab the file is in. It may move to another tab showing it
    tab: Arc<Mutex<u32>>,
    connected: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
struct State {
    connections: HashMap<String, Connection>,
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
    state.connections.retain(|key, connection| {
        let file = files.iter().find(|file| &file.key == key);
        // A renamed file connects again under its new name
        let keep = file.is_some_and(|file| file.name == connection.name);
        if keep {
            *connection.tab.lock().unwrap() = file.unwrap().tab;
        } else {
            connection.stop.store(true, Ordering::Relaxed);
        }
        keep
    });
    for file in &files {
        if state.connections.contains_key(&file.key) {
            continue;
        }
        let connection = Connection {
            name: file.name.clone(),
            tab: Arc::new(Mutex::new(file.tab)),
            connected: Arc::default(),
            stop: Arc::default(),
        };
        let (app, key, name) = (app.clone(), file.key.clone(), file.name.clone());
        let (tab, connected, stop) = (connection.tab.clone(), connection.connected.clone(), connection.stop.clone());
        std::thread::spawn(move || keep_connected(&app, &key, &name, &tab, &connected, &stop));
        state.connections.insert(file.key.clone(), connection);
    }
    let status = Status {
        on,
        files: files.len(),
        connected: state.connections.values().filter(|c| c.connected.load(Ordering::Relaxed)).count(),
        version: VERSION,
    };
    if state.status.as_ref() != Some(&status) {
        state.status = Some(status.clone());
        drop(state);
        if let Some(window) = app.get_webview_window(crate::settings::WINDOW) {
            let _ = window.eval(format!("window.showMcpStatus?.({})", serde_json::to_string(&status).unwrap()));
        }
    }
}

/// Keeps file `key` connected to the bridge server until `stop`
fn keep_connected(app: &AppHandle, key: &str, name: &str, tab: &Mutex<u32>, connected: &AtomicBool, stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        if let Some(socket) = connect(key, name) {
            log::info!("MCP bridge: file connected");
            connected.store(true, Ordering::Relaxed);
            serve(app, socket, tab, stop);
            connected.store(false, Ordering::Relaxed);
            log::info!("MCP bridge: file disconnected");
        }
        // In steps, to stop soon after being asked
        let until = Instant::now() + RETRY_AFTER;
        while Instant::now() < until && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// Connects to the server as the plugin does, with the file's key and name
fn connect(key: &str, name: &str) -> Option<WebSocket<TcpStream>> {
    // The server listens on IPv4 unless told otherwise
    let addresses: [SocketAddr; 2] = [(Ipv4Addr::LOCALHOST, PORT).into(), (Ipv6Addr::LOCALHOST, PORT).into()];
    let stream = addresses
        .iter()
        .find_map(|address| TcpStream::connect_timeout(address, Duration::from_secs(1)).ok())?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    stream.set_write_timeout(Some(Duration::from_secs(10))).ok()?;
    stream.set_nodelay(true).ok()?;
    let mut url = tauri::Url::parse(&format!("ws://127.0.0.1:{PORT}/ws")).ok()?;
    url.query_pairs_mut().append_pair("fileKey", key).append_pair("fileName", name);
    match tungstenite::client(url.as_str(), stream) {
        Ok((socket, _)) => Some(socket),
        Err(error) => {
            log::warn!("MCP bridge: handshake failed: {error}");
            None
        }
    }
}

/// Answers the server's requests until the connection ends or `stop`
fn serve(app: &AppHandle, mut socket: WebSocket<TcpStream>, tab: &Mutex<u32>, stop: &AtomicBool) {
    let (send, answers): (Sender<String>, Receiver<String>) = mpsc::channel();
    let pending = Arc::new(AtomicUsize::new(0));
    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = socket.close(None);
            let _ = socket.flush();
            return;
        }
        while let Ok(answer) = answers.try_recv() {
            if socket.send(Message::text(answer)).is_err() {
                return;
            }
        }
        // Answers only come while requests are pending, so the socket is
        // read in short turns only then
        let turn = if pending.load(Ordering::Relaxed) > 0 { 20 } else { 500 };
        let _ = socket.get_ref().set_read_timeout(Some(Duration::from_millis(turn)));
        match socket.read() {
            Ok(Message::Text(text)) => {
                let (app, send, pending) = (app.clone(), send.clone(), pending.clone());
                let tab = *tab.lock().unwrap();
                pending.fetch_add(1, Ordering::Relaxed);
                std::thread::spawn(move || {
                    // Queued while still pending, so it goes out in a short turn
                    let _ = send.send(answer(&app, tab, text.as_str()));
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

/// The plugin's answer to `request`, as the JSON it sends
fn answer(app: &AppHandle, tab: u32, request: &str) -> String {
    let request: Value = serde_json::from_str(request).unwrap_or_default();
    let reply = |error: &str| {
        serde_json::json!({ "type": request["type"], "requestId": request["requestId"], "error": error }).to_string()
    };
    if request["requestId"].as_str().is_none() {
        return reply("Invalid request");
    }
    if request["type"] == "run_script" {
        return reply("run_script isn't available in Ligma: Figma's web editor has no plugin sandbox, so scripts would run with figma.com's access to your account. Use the other tools instead.");
    }
    match run(app, tab, &request) {
        Ok(answer) => answer,
        Err(error) => reply(&error),
    }
}

/// Runs `request` in tab `tab` and waits for the answer
fn run(app: &AppHandle, tab: u32, request: &Value) -> Result<String, String> {
    const ASLEEP: &str = "This file's tab in Ligma is asleep. Select it to wake it.";
    let id = request["requestId"].to_string();
    let start = format!("window.__ligmaMcp?.start({request}) ?? \"missing\"");
    let text = |json: String| serde_json::from_str::<String>(&json).ok();
    let loading_until = Instant::now() + LOADING_TIMEOUT;
    loop {
        let mut started = text(eval(app, tab, &start).ok_or(ASLEEP)?);
        // Set up in the tab on its first request
        if started.as_deref() == Some("missing") {
            started = text(eval(app, tab, &format!("{}\n;{start}", runner())).ok_or(ASLEEP)?);
        }
        match started.as_deref() {
            Some("ok") => break,
            Some("loading") if Instant::now() < loading_until => std::thread::sleep(Duration::from_millis(500)),
            Some("loading") => return Err("This file is still loading in Ligma. Try again once it shows.".into()),
            Some("missing") | None => return Err("The bridge plugin couldn't be loaded in this tab".into()),
            Some(error) => return Err(error.into()),
        }
    }
    // Not `??`: null means the answer isn't there yet
    let take = format!("window.__ligmaMcp ? window.__ligmaMcp.take({id}) : \"missing\"");
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    let mut wait = Duration::from_millis(5);
    while Instant::now() < deadline {
        std::thread::sleep(wait);
        wait = (wait * 2).min(Duration::from_millis(50));
        let result = eval(app, tab, &take).ok_or(ASLEEP)?;
        match serde_json::from_str::<Option<String>>(&result) {
            Ok(Some(missing)) if missing == "missing" => {
                return Err("This file's tab in Ligma reloaded before answering".into())
            }
            Ok(Some(answer)) => return Ok(answer),
            // Null, and on macOS nothing, while there's no answer yet
            _ => continue,
        }
    }
    Err("Request timed out (3 minutes)".into())
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

/// Sets up `window.__ligmaMcp` in a Figma tab, which answers the bridge
/// server's requests with the plugin's own code:
///
/// - `start(request)` hands a request to the plugin, and returns "ok",
///   "loading" while the file loads, or why it can't
/// - `take(requestId)` returns its answer once there is one, else null
/// - `busy()` is true while requests are pending and for a few minutes
///   after, so the tab isn't frozen or put to sleep meanwhile
///
/// The plugin gets `figma` as Figma's console plugin API, `window.figma`,
/// read again for each request, as a plugin run from Figma's menu or from
/// Ligma's development plugins can close it and make a new one. A closed
/// one is made again the way plugins.rs does. The plugin's window, its
/// events and its storage do nothing: Ligma has no use for them
const RUNNER_JS: &str = r#"(() => {
  if (window.__ligmaMcp) return;
  const BUSY_FOR = 5 * 60 * 1000;
  const answers = new Map();
  let api = null;
  let onmessage = null;
  let pending = 0;
  let lastUsed = 0;
  const ui = {
    postMessage(message) {
      if (message && typeof message === "object" && "requestId" in message) answers.set(message.requestId, message);
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
  // On the first request, as the plugin reads the file as it starts
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

  window.__ligmaMcp = {
    start(request) {
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
          return `The bridge plugin couldn't start in this tab: ${error?.message ?? error}`;
        }
      }
      if (typeof onmessage !== "function") return "The bridge plugin didn't start in this tab";
      pending++;
      lastUsed = Date.now();
      const fail = (error) => answers.set(request.requestId, {
        type: request.type,
        requestId: request.requestId,
        error: error instanceof Error ? error.message : String(error),
      });
      const settle = () => {
        pending--;
        lastUsed = Date.now();
      };
      try {
        Promise.resolve(onmessage({ type: "server-request", payload: request }, {})).catch(fail).finally(settle);
      } catch (error) {
        fail(error);
        settle();
      }
      return "ok";
    },
    // As JSON text, like the plugin's window sends it: WebKit hands Ligma
    // other values as Objective-C objects, not all of which make JSON
    take(requestId) {
      if (!answers.has(requestId)) return null;
      const answer = answers.get(requestId);
      answers.delete(requestId);
      try {
        return JSON.stringify(answer);
      } catch (error) {
        return JSON.stringify({ type: answer.type, requestId, error: `The answer couldn't be sent: ${error?.message ?? error}` });
      }
    },
    busy: () => pending > 0 || Date.now() - lastUsed < BUSY_FOR,
  };
})()"#;

/// MCP server configuration for AI tools, pinned to `VERSION`
fn config() -> String {
    // Written out, as serde_json would put "args" before "command"
    format!(
        r#"{{
  "mcpServers": {{
    "figma-bridge": {{
      "command": "npx",
      "args": ["-y", "@gethopp/figma-mcp-bridge@{VERSION}"]
    }}
  }}
}}"#
    )
}

/// Claude Code's command adding the same server
fn claude_command() -> String {
    format!("claude mcp add figma-bridge -- npx -y @gethopp/figma-mcp-bridge@{VERSION}")
}

#[tauri::command]
pub async fn get_mcp_status(app: AppHandle, webview: Webview) -> Option<Status> {
    if !crate::settings::from_settings_window(&webview) {
        return None;
    }
    sync(&app);
    state().lock().unwrap().status.clone()
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
