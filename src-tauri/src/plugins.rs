//! Development plugins: Figma plugins on this computer, imported from their
//! `manifest.json` like the Figma desktop app's Plugins > Development >
//! Import plugin from manifest…, which figma.com doesn't have.
//!
//! The list is `plugins.json` next to `settings.json`, and Settings >
//! Plugins (`dist/settings.html`) imports, switches on and off, and removes
//! them. Enabled plugins show in Figma's own Actions panel, under Plugins &
//! widgets, like in the desktop app: marked Development, with a Development
//! filter that lists only them and Import from manifest…, and Remove in
//! their … menu. `PAGE_JS` draws them there and reports clicks by opening
//! `SIGNAL_HOST` links, which `on_signal` handles.
//!
//! Figma's web editor offers its plugin API on the page as `window.figma`,
//! for the developer console, in files the user can edit. `RUNNER_JS` runs
//! the plugin's `main` against it, with `__html__` and `__uiFiles__` from
//! its `ui`, so `figma.showUI` and messages to and from the UI work as in
//! the desktop app. Unlike there, the code isn't sandboxed: it runs with
//! the page's own access, which Settings says. The files are read on every
//! run, so a rebuilt plugin runs its new code.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, Url, Webview, Window};
use tauri_plugin_opener::OpenerExt;

const FILE: &str = "plugins.json";

/// An imported plugin, as kept in `plugins.json`.
#[derive(Clone, Deserialize, Serialize)]
struct Entry {
    /// Its `manifest.json`.
    manifest: PathBuf,
    enabled: bool,
}

/// What a manifest says, with paths resolved next to it.
struct Manifest {
    name: String,
    id: Option<String>,
    main: PathBuf,
    ui: Ui,
    /// Editors it runs in: "figma", "figjam", "dev", "slides", "buzz".
    editor_types: Vec<String>,
    /// Its menu's commands as (text, command), submenus flattened into
    /// "Submenu › Command", and None for a separator. Empty if it has no
    /// menu.
    commands: Vec<Option<(String, String)>>,
}

enum Ui {
    None,
    /// One page, the plugin's `__html__`.
    Page(PathBuf),
    /// Named pages, its `__uiFiles__`.
    Pages(Vec<(String, PathBuf)>),
}

/// Reads and checks the manifest at `path`.
fn read_manifest(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("Can't read manifest.json: {error}"))?;
    let json: Value = serde_json::from_str(&text).map_err(|error| format!("manifest.json isn't valid JSON: {error}"))?;
    let dir = path.parent().unwrap_or(Path::new(""));
    let name = json["name"].as_str().filter(|name| !name.trim().is_empty());
    let name = name.ok_or("manifest.json has no name")?.trim().to_owned();
    if json["containsWidget"].as_bool() == Some(true) || json.get("widgetApi").is_some() {
        return Err("Widgets aren't supported, only plugins".into());
    }
    let main = json["main"].as_str().ok_or("manifest.json has no main")?;
    let ui = match &json["ui"] {
        Value::String(page) => Ui::Page(dir.join(page)),
        Value::Object(pages) => Ui::Pages(
            pages
                .iter()
                .filter_map(|(name, page)| Some((name.clone(), dir.join(page.as_str()?))))
                .collect(),
        ),
        _ => Ui::None,
    };
    let editor_types = match json["editorType"].as_array() {
        Some(types) => types.iter().filter_map(|t| t.as_str().map(str::to_owned)).collect(),
        None => vec!["figma".to_owned()],
    };
    let mut commands = Vec::new();
    if let Some(menu) = json["menu"].as_array() {
        flatten_menu(menu, "", &mut commands);
    }
    Ok(Manifest {
        name,
        id: json["id"].as_str().map(str::to_owned),
        main: dir.join(main),
        ui,
        editor_types,
        commands,
    })
}

fn flatten_menu(menu: &[Value], prefix: &str, commands: &mut Vec<Option<(String, String)>>) {
    for item in menu {
        if item["separator"].as_bool() == Some(true) {
            commands.push(None);
            continue;
        }
        let Some(name) = item["name"].as_str() else {
            continue;
        };
        let text = format!("{prefix}{name}");
        if let Some(submenu) = item["menu"].as_array() {
            flatten_menu(submenu, &format!("{text} › "), commands);
        } else if let Some(command) = item["command"].as_str() {
            commands.push(Some((text, command.to_owned())));
        }
    }
}

/// Why a readable manifest's plugin can't run yet, if it can't.
fn problem(manifest: &Manifest) -> Option<String> {
    let file = |path: &Path| path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    if !manifest.main.is_file() {
        return Some(format!("{} not found. Build the plugin first.", file(&manifest.main)));
    }
    let pages = match &manifest.ui {
        Ui::None => vec![],
        Ui::Page(page) => vec![page],
        Ui::Pages(pages) => pages.iter().map(|(_, page)| page).collect(),
    };
    pages.into_iter().find(|page| !page.is_file()).map(|page| format!("{} not found.", file(page)))
}

fn path(app: &AppHandle) -> Option<PathBuf> {
    Some(app.path().app_config_dir().ok()?.join(FILE))
}

fn load(app: &AppHandle) -> Vec<Entry> {
    path(app)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

fn save(app: &AppHandle, entries: &[Entry]) -> Result<(), String> {
    let path = path(app).ok_or("No config folder")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, serde_json::to_string_pretty(entries).unwrap()).map_err(|e| e.to_string())?;
    push(app);
    // Settings, if open, lists them too.
    if let Some(window) = app.get_webview_window(crate::settings::WINDOW) {
        let _ = window.eval("window.showPlugins?.()");
    }
    Ok(())
}

/// Origin of the links `PAGE_JS` opens to report a click.
pub const SIGNAL_HOST: &str = "plugins.ligma.invalid";

/// A secret for this run of Ligma, in `PAGE_JS` and every link it opens, so
/// only that script can run or remove plugins: other frames, like plugin
/// UIs, can open links too but can't read it.
fn token() -> &'static str {
    static TOKEN: OnceLock<String> = OnceLock::new();
    TOKEN.get_or_init(|| {
        use std::hash::{BuildHasher, Hasher};
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_nanos());
        (0..2)
            .map(|round| {
                let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
                hasher.write_u128(nanos + round);
                format!("{:016x}", hasher.finish())
            })
            .collect()
    })
}

/// Initialization script for Figma tabs (see `PAGE_JS`).
pub fn page_js() -> String {
    PAGE_JS.replace("__LIGMA_TOKEN__", token())
}

/// What `PAGE_JS` shows of the enabled plugins.
fn page_list(app: &AppHandle) -> Value {
    let plugins = load(app)
        .into_iter()
        .filter(|entry| entry.enabled)
        .map(|entry| {
            let plugin = describe(&entry);
            let (editor_types, commands) = match read_manifest(&entry.manifest) {
                Ok(manifest) => (manifest.editor_types, manifest.commands),
                Err(_) => (vec!["figma".to_owned()], vec![]),
            };
            serde_json::json!({
                "manifest": plugin.manifest,
                "name": plugin.name,
                "folder": plugin.folder,
                "editorTypes": editor_types,
                "commands": commands,
                "problem": plugin.problem,
            })
        })
        .collect();
    Value::Array(plugins)
}

/// Sends the enabled plugins to Figma tab `webview`.
pub fn push_to(webview: &Webview) {
    let list = page_list(webview.app_handle());
    let _ = webview.eval(format!("window.__ligmaDevPlugins?.set({list})"));
}

/// Sends the enabled plugins to every loaded Figma tab.
fn push(app: &AppHandle) {
    let list = page_list(app);
    for webview in crate::tabs::scriptable_tabs(app) {
        let _ = webview.eval(format!("window.__ligmaDevPlugins?.set({list})"));
    }
}

/// A click in the Actions panel, from `PAGE_JS` in tab `tab`:
/// `/run?manifest=…&command=…`, `/remove?manifest=…` or `/import`.
pub fn on_signal(app: &AppHandle, tab: u32, url: &Url) {
    let param = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    if param("token").as_deref() != Some(token()) {
        return;
    }
    let manifest = param("manifest").map(PathBuf::from);
    match url.path() {
        "/run" => {
            let known = load(app).into_iter().any(|entry| entry.enabled && Some(&entry.manifest) == manifest.as_ref());
            if let (true, Some(manifest)) = (known, manifest) {
                run(app, tab, &manifest, param("command").filter(|command| !command.is_empty()));
            }
        }
        "/remove" => {
            let mut entries = load(app);
            entries.retain(|entry| Some(&entry.manifest) != manifest.as_ref());
            if let Err(error) = save(app, &entries) {
                log::error!("Couldn't remove plugin: {error}");
            }
        }
        "/import" => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let Some(window) = app.get_window("main") else {
                    return;
                };
                if let Err(error) = import(&app, &window).await {
                    crate::prompt::show(&app, "Couldn't import plugin", error, &["OK"], |_| {});
                }
            });
        }
        _ => {}
    }
}

/// The script that runs a plugin, from `RUNNER_JS`, its code and what it's
/// given.
fn script(manifest: &Manifest, command: Option<String>) -> Result<String, String> {
    let read = |path: &Path| {
        std::fs::read_to_string(path).map_err(|error| format!("Can't read {}: {error}", path.display()))
    };
    let code = read(&manifest.main)?;
    let (html, ui_files) = match &manifest.ui {
        Ui::None => (Value::Null, Value::Null),
        Ui::Page(page) => (Value::String(read(page)?), Value::Null),
        Ui::Pages(pages) => {
            let mut files = serde_json::Map::new();
            for (name, page) in pages {
                files.insert(name.clone(), Value::String(read(page)?));
            }
            (Value::Null, Value::Object(files))
        }
    };
    let spec = serde_json::json!({
        "name": manifest.name,
        // Keeps each plugin's `figma.clientStorage` apart.
        "storage": manifest.id.clone().unwrap_or_else(|| manifest.name.clone()),
        "editorTypes": manifest.editor_types,
        "command": command.unwrap_or_default(),
        "html": html,
        "uiFiles": ui_files,
    });
    // The code goes in as written, not through `eval`, which Figma's
    // content security policy may block. A new line ends a last `//`
    // comment.
    Ok(format!(
        "({RUNNER_JS})({spec}, function (figma, __html__, __uiFiles__, setTimeout, setInterval, clearTimeout, clearInterval) {{\n{code}\n}})"
    ))
}

/// Runs the plugin at `manifest` in tab `tab`, with menu command `command`.
fn run(app: &AppHandle, tab: u32, manifest_path: &Path, command: Option<String>) {
    let manifest = match read_manifest(manifest_path) {
        Ok(manifest) => manifest,
        Err(error) => return fail(app, "this plugin", &error),
    };
    if let Some(problem) = problem(&manifest) {
        return fail(app, &manifest.name, &problem);
    }
    let script = match script(&manifest, command) {
        Ok(script) => script,
        Err(error) => return fail(app, &manifest.name, &error),
    };
    let Some(webview) = crate::tabs::scriptable(app, tab) else {
        return;
    };
    let (handle, name) = (app.clone(), manifest.name.clone());
    let result = webview.eval_with_callback(script, move |result| {
        let message = match serde_json::from_str::<Value>(&result).ok().as_ref().and_then(Value::as_str) {
            Some("ok") => return,
            Some("unavailable") => "Plugins run in files you can edit, once they've loaded, and not while another plugin is running.",
            Some("reload") => "Figma's plugin connection in this tab was closed. Reload the tab (right-click it, then Reload) and try again.",
            // A syntax error stops the whole script, so nothing comes back.
            _ => "Its code couldn't be loaded. Check that main in manifest.json points to built JavaScript.",
        };
        fail(&handle, &name, message);
    });
    if let Err(error) = result {
        log::error!("Couldn't run plugin {}: {error}", manifest.name);
    }
}

fn fail(app: &AppHandle, name: &str, message: &str) {
    log::warn!("Plugin {name} didn't run: {message}");
    crate::prompt::show(app, &format!("Couldn't run {name}"), message, &["OK"], |_| {});
}

/// Runs a plugin's code against Figma's console plugin API, `window.figma`,
/// and returns "ok", "unavailable" (no API: the file can't be edited, isn't
/// loaded, or another plugin is running) or "reload" (the API was closed
/// and can't be had again).
///
/// The API lasts until a plugin closes it, and closing the plugin window's
/// X closes it for good, so each run starts on a new one: Figma's own code
/// closes the old one and makes another, as it does after a plugin from its
/// menu ends. Ligma finds that code by text in Figma's modules, and without
/// it falls back on the API as it is.
///
/// The plugin gets the API with `figma.command`, the `run` and `close`
/// events, `closePlugin`, its own `clientStorage` and timers that stop when
/// it closes. `showUI` titles the window with the plugin's name.
const RUNNER_JS: &str = r#"(spec, main) => {
  const findHost = () => {
    const chunks = globalThis.rspackChunk_figma_web_bundler ?? globalThis.webpackChunk_figma_web_bundler;
    let require;
    try {
      chunks?.push([[Symbol("ligma")], {}, (r) => { require = r; }]);
    } catch {}
    if (!require?.m) return null;
    const id = Object.keys(require.m).find((id) => String(require.m[id]).includes("Did not close the previous plugin"));
    if (id == null) return null;
    const fns = Object.values(require(id)).filter((v) => typeof v === "function");
    const text = (fn) => Function.prototype.toString.call(fn);
    const host = {
      isConsole: fns.find((fn) => /^function \w*\(\)\{return \w+\?\.type==="global"\}$/.test(text(fn))),
      close: fns.find((fn) => text(fn).includes("closePluginFunc") && text(fn).includes('"global"===')),
      reset: fns.find((fn) => text(fn).includes("resetGlobalPluginAPI?.(")),
    };
    return host.isConsole && host.close && host.reset ? host : null;
  };
  const host = window.__ligmaPluginHost ??= findHost();
  const alive = (api) => {
    try {
      return api != null && api.currentPage != null;
    } catch {
      return false;
    }
  };
  // A new API, unless a plugin from Figma's menu holds it.
  const renew = () => {
    if (!host?.isConsole()) return;
    host.close();
    host.reset();
  };

  let api = window.figma;
  if (api == null) return "unavailable";
  window.__ligmaClosePlugin?.();
  if (host) {
    renew();
    api = window.figma;
  }
  if (!alive(api)) return host ? "unavailable" : "reload";

  const PRODUCTS = { figma: "Figma Design", figjam: "FigJam", dev: "Dev Mode", slides: "Figma Slides", buzz: "Figma Buzz" };
  if (!spec.editorTypes.includes(api.editorType)) {
    api.notify(`${spec.name} doesn't run in ${PRODUCTS[api.editorType] ?? api.editorType}`, { error: true });
    return "ok";
  }

  const handlers = { run: new Set(), close: new Set() };
  const forwarded = [];
  const timers = new Set();
  let closed = false;
  let watch;
  const close = () => {
    if (closed) return;
    closed = true;
    clearInterval(watch);
    if (window.__ligmaClosePlugin === close) delete window.__ligmaClosePlugin;
    for (const handler of handlers.close) {
      try { handler(); } catch (error) { console.error(error); }
    }
    for (const [type, handler] of forwarded) {
      try { api.off(type, handler); } catch {}
    }
    for (const [id, clear] of timers) clear(id);
    try { api.ui.close(); } catch {}
    renew();
  };
  window.__ligmaClosePlugin = close;
  // Closing the plugin's window closes the API. Its close handlers still
  // run, and its timers stop.
  watch = setInterval(() => { if (!alive(api)) close(); }, 1000);

  const listen = (type, handler, once) => {
    if (type in handlers) {
      const wrapped = once ? (...args) => { handlers[type].delete(wrapped); handler(...args); } : handler;
      wrapped.original = handler;
      handlers[type].add(wrapped);
      return;
    }
    if (once) api.once(type, handler);
    else api.on(type, handler);
    forwarded.push([type, handler]);
  };
  // The console API has no `clientStorage` (it has no plugin ID). Keep
  // each plugin's in figma.com's IndexedDB instead, which clones values
  // like Figma does.
  const storagePrefix = `${spec.storage}:`;
  const storage = (mode, use) =>
    new Promise((resolve, reject) => {
      const open = indexedDB.open("ligma-plugins", 1);
      open.onupgradeneeded = () => open.result.createObjectStore("clientStorage");
      open.onerror = () => reject(open.error);
      open.onsuccess = () => {
        const db = open.result;
        const request = use(db.transaction("clientStorage", mode).objectStore("clientStorage"));
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
        request.transaction.oncomplete = () => db.close();
      };
    });
  const failed = (error) => {
    console.error(error);
    api.notify(`${spec.name}: ${error?.message ?? error}`, { error: true });
  };
  const overrides = {
    command: spec.command,
    parameters: { on() {}, off() {}, once() {} },
    closePlugin(message) {
      if (message) api.notify(String(message));
      close();
    },
    showUI(html, options) {
      return api.showUI(html, { title: spec.name, ...options });
    },
    on: (type, handler) => listen(type, handler, false),
    once: (type, handler) => listen(type, handler, true),
    off(type, handler) {
      if (type in handlers) {
        for (const wrapped of handlers[type]) if (wrapped.original === handler) handlers[type].delete(wrapped);
        return;
      }
      api.off(type, handler);
    },
    clientStorage: {
      getAsync: (key) => storage("readonly", (store) => store.get(storagePrefix + key)),
      setAsync: async (key, value) => {
        await storage("readwrite", (store) => store.put(value, storagePrefix + key));
      },
      deleteAsync: async (key) => {
        await storage("readwrite", (store) => store.delete(storagePrefix + key));
      },
      keysAsync: async () => {
        const keys = await storage("readonly", (store) => store.getAllKeys());
        return keys.filter((key) => String(key).startsWith(storagePrefix)).map((key) => key.slice(storagePrefix.length));
      },
    },
  };
  // Over an empty object: the API's own properties are frozen, and a proxy
  // of it would have to return them unchanged.
  const figma = new Proxy({}, {
    get(_, key) {
      if (Object.hasOwn(overrides, key)) return overrides[key];
      const value = Reflect.get(api, key);
      return typeof value === "function" ? value.bind(api) : value;
    },
    set: (_, key, value) => Reflect.set(api, key, value),
    has: (_, key) => Object.hasOwn(overrides, key) || Reflect.has(api, key),
  });
  const timer = (start, clear) => (...args) => {
    const id = start(...args);
    timers.add([id, clear]);
    return id;
  };

  try {
    main(
      figma,
      spec.html ?? undefined,
      spec.uiFiles ?? undefined,
      timer(window.setTimeout.bind(window), window.clearTimeout.bind(window)),
      timer(window.setInterval.bind(window), window.clearInterval.bind(window)),
      window.clearTimeout.bind(window),
      window.clearInterval.bind(window),
    );
  } catch (error) {
    failed(error);
    close();
    return "ok";
  }
  // Like Figma: `run` comes after the code has set up its handlers.
  window.setTimeout(() => {
    for (const handler of handlers.run) {
      try {
        Promise.resolve(handler({ command: spec.command, parameters: undefined })).catch(failed);
      } catch (error) {
        failed(error);
      }
    }
  });
  return "ok";
}"#;

/// Shows the enabled plugins (sent by `push`) in Figma's Actions panel,
/// under Plugins & widgets, the way the desktop app shows development
/// plugins: each with a Development badge, its commands in a menu when it
/// has some, and Remove in its … menu. The panel's filter gets a
/// Development option that lists only them, with Import from manifest…
///
/// Figma's panel is React's, so this only adds its own nodes next to
/// Figma's and styles them with the class names Figma's rows use (taken
/// from a row on screen when there is one). The Development filter hides
/// Figma's rows and relabels the filter button with CSS, leaving Figma's
/// nodes alone. Clicks go to Ligma as `SIGNAL_HOST` links: through
/// `window.open` (`on_new_window`) on Windows, and on macOS as an iframe
/// load, which WKWebView reports to `on_navigation`.
const PAGE_JS: &str = r##"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  const TOKEN = "__LIGMA_TOKEN__";
  const signal = (action, params = {}) => {
    const url = `https://plugins.ligma.invalid/${action}?` + new URLSearchParams({ token: TOKEN, ...params });
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

  let plugins = [];
  // The Development filter is on.
  let devOnly = false;
  // Figma's own filter: 0 All, 1 Plugins, 2 Widgets.
  let figmaFilter = 0;

  // Figma's class names, as seen on 2026-10-24. Rows take theirs from a
  // Figma row on screen, if there is one, in case these change.
  const CLASSES = {
    row: "cx_flex--tV5oZ cx_alignCenter--1v4xl cx_bRadius5--JCLrp cx_ml8--SwsTN cx_mr8--aRua- cx_p4--Ckaol cx_flexGrow1--IRFtW cx_flexShrink1--HTMk6 cx_flexBasis0--xUinj",
    rowHover: "cx_colorBgHover--S-Hsw",
    column: "x78zum5 xdt5ytf xh8yej3",
    line: "x78zum5 x6s0dn4 x1qughib xh8yej3",
    button: "x78zum5 x6s0dn4 x1md70p1 x1iyjqo2",
    nameWrap: "x163pfp xy13l1i xw5ewwj",
    name: "xuxw1ft xb3r6kr xlyipyv",
    chevron: "xjbqb8w x1bawvfr",
    menu: "base--dropdown--fDh-M _overlayBase--_overlayBase--lIllh pointing--content--5emtR",
    option: "multilevel--option--6Kfme multilevel--optionBase--9MgmU base--_optionBase--elGk- text--ui2FontSize12--AJ7ld text--_fontBase--Qq3Bi",
    optionActive: "multilevel--optionActive--fVOZg multilevel--optionBase--9MgmU base--_optionBase--elGk- text--ui2FontSize12--AJ7ld text--_fontBase--Qq3Bi",
    optionName: "multilevel--name--XnFDS ellipsis--ellipsis--IdJAr",
    separator: "base--separator--F9VEK",
    separatorLine: "base--separatorLine--7cveq",
    filterOption: "base--option--6qoop base--_optionBase--elGk- text--ui2FontSize12--AJ7ld text--_fontBase--Qq3Bi base--checkableOption--onu9O",
    filterHover: "base--optionHover--2xcwm",
    filterLabel: "xb3r6kr xlyipyv x1lliihq",
    check: "base--check--OK8b1",
  };
  const harvest = (list) => {
    const row = list.querySelector('div[data-testid="extension-list-item"]');
    const button = row?.querySelector("button");
    const name = button?.querySelector("div[aria-label]");
    if (!row || !button || !name) return;
    const tokens = row.className.split(/\s+/);
    CLASSES.row = tokens.filter((token) => !token.includes("colorBgHover")).join(" ");
    CLASSES.column = row.firstElementChild?.className || CLASSES.column;
    CLASSES.line = row.firstElementChild?.firstElementChild?.className || CLASSES.line;
    CLASSES.button = button.className;
    CLASSES.nameWrap = name.parentElement.className;
    CLASSES.name = name.className;
  };

  const ICONS = {
    plugin: '<svg class="ligma-icon" width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor"><rect x="1.5" y="1.5" width="13" height="13" rx="3" stroke-dasharray="2 1.6"/><path d="M6.5 6 4.5 8l2 2M9.5 6l2 2-2 2" stroke-linecap="round" stroke-linejoin="round"/></svg>',
    import: '<svg class="ligma-icon" width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round"><path d="M8 2.5v7M5.5 7 8 9.5 10.5 7"/><path d="M3.5 9.5v2.5a1.5 1.5 0 0 0 1.5 1.5h6a1.5 1.5 0 0 0 1.5-1.5V9.5"/></svg>',
    chevron: '<svg width="16" height="16" fill="none" viewBox="0 0 16 16"><path fill="var(--fpl-icon-color, var(--color-icon))" d="M9.768 6.768a.5.5 0 0 1 .707.707l-2.12 2.121a.5.5 0 0 1-.708 0L5.525 7.475a.5.5 0 0 1 .708-.707l1.768 1.767z"/></svg>',
    dots: '<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><path fill="var(--fpl-icon-color, var(--color-icon))" fill-rule="evenodd" d="M7 11.5a1.5 1.5 0 1 1-3 0 1.5 1.5 0 0 1 3 0m6 0a1.5 1.5 0 1 1-3 0 1.5 1.5 0 0 1 3 0m4.5 1.5a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3" clip-rule="evenodd"/></svg>',
    check: '<svg width="24" height="24" fill="none" viewBox="0 0 24 24"><path fill="var(--fpl-icon-color, var(--color-icon))" d="M15.584 7.722a.5.5 0 0 1 .832.555l-5 7.5a.502.502 0 0 1-.77.076l-3-3a.5.5 0 0 1 .708-.707l2.568 2.569z"/></svg>',
  };

  const STYLE = `
    #ligma-dev-plugins { display: flex; flex-direction: column; }
    [class*="list--list"]:has(> #ligma-dev-plugins.ligma-only) > :not(#ligma-dev-plugins) { display: none !important; }
    html.ligma-dev-filter #extensions-filter-dropdown-button [class*="buttonContent"] > div > span:first-child { display: none; }
    html.ligma-dev-filter #extensions-filter-dropdown-button [class*="buttonContent"] > div::before { content: "Development"; white-space: nowrap; }
    html.ligma-dev-filter #extensions-filter-dropdown-menu [role="menuitem"][aria-checked="true"]:not([data-ligma-dev]) > svg { visibility: hidden; }
    /* Where Figma's 20px plugin icons go, with their 2px margin. */
    .ligma-icon { flex: none; box-sizing: border-box; width: 20px; height: 20px; margin: 2px; padding: 2px; color: var(--color-icon-secondary, var(--color-text-secondary)); }
    .ligma-text { min-width: 0; }
    .ligma-name-line { display: flex; align-items: center; gap: 6px; min-width: 0; }
    .ligma-name-line > :first-child { min-width: 0; }
    .ligma-badge { flex: none; padding: 0 4px; border: 1px solid var(--color-border); border-radius: 4px; font-size: 11px; line-height: 14px; color: var(--color-text-secondary); }
    .ligma-path { overflow: hidden; white-space: nowrap; text-overflow: ellipsis; font-size: 11px; line-height: 16px; color: var(--color-text-secondary); }
    .ligma-trail { flex: none; display: flex; align-items: center; }
    .ligma-kind { font-size: 11px; line-height: 16px; color: var(--color-text-tertiary); }
    .ligma-options { display: grid; place-items: center; width: 24px; height: 24px; padding: 0; border: 0; border-radius: 5px; background: none; }
    .ligma-options:hover { background: var(--color-bg-hover, rgba(127, 127, 127, 0.15)); }
    .ligma-row:not(:hover):not(.ligma-open) .ligma-options { display: none; }
    .ligma-row:hover .ligma-kind, .ligma-row.ligma-open .ligma-kind { display: none; }
    .ligma-menu { position: fixed; z-index: 2147483000; max-width: 400px; }
  `;

  const el = (tag, className = "", html = "") => {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (html) node.innerHTML = html;
    return node;
  };
  const rowButton = (icon) => el("button", CLASSES.button, icon);

  // Which plugins run in this file's editor.
  const editorTypes = () =>
    ({ design: ["figma", "dev"], file: ["figma", "dev"], board: ["figjam"], slides: ["slides"], buzz: ["buzz"] })[location.pathname.split("/")[1]] ?? [];

  // Closes the Actions panel, as Figma does when a plugin starts.
  const closeActions = () => {
    const button = [...document.querySelectorAll('[aria-label="Actions"]')].find((button) => button.offsetParent);
    if (document.querySelector('[data-testid="tab-extensions"]')) button?.click();
  };

  // A menu like Figma's (the … and command menus) under `anchor`. `items`
  // are [text, action], or null for a separator.
  let menu = null;
  const closeMenu = () => {
    menu?.remove();
    menu = null;
    for (const node of document.querySelectorAll(".ligma-open")) node.classList.remove("ligma-open");
  };
  const openMenu = (anchor, items, owner, alignEnd = false) => {
    closeMenu();
    menu = el("div", `${CLASSES.menu} ligma-menu`);
    menu.setAttribute("role", "menu");
    const list = el("div");
    for (const item of items) {
      if (!item) {
        list.append(el("div", CLASSES.separator, `<div class="${CLASSES.separatorLine}"></div>`));
        continue;
      }
      const [text, action] = item;
      const option = el("div", CLASSES.option, `<div class="${CLASSES.optionName}"></div>`);
      option.setAttribute("role", "menuitem");
      option.firstChild.textContent = text;
      option.addEventListener("mouseenter", () => (option.className = CLASSES.optionActive));
      option.addEventListener("mouseleave", () => (option.className = CLASSES.option));
      option.addEventListener("click", (e) => {
        e.stopPropagation();
        closeMenu();
        action();
      });
      list.append(option);
    }
    menu.append(list);
    document.body.append(menu);
    const at = anchor.getBoundingClientRect();
    const size = menu.getBoundingClientRect();
    const top = at.bottom + 4 + size.height > innerHeight ? at.top - size.height - 4 : at.bottom + 4;
    menu.style.top = `${Math.max(8, top)}px`;
    const left = alignEnd ? at.right - size.width : at.left;
    menu.style.left = `${Math.max(8, Math.min(left, innerWidth - size.width - 8))}px`;
    owner?.classList.add("ligma-open");
  };
  // A press in the menu mustn't reach Figma or move focus out of its
  // panel, which closes it.
  for (const type of ["pointerdown", "mousedown"]) {
    addEventListener(type, (e) => {
      if (!menu) return;
      if (menu.contains(e.target)) {
        e.stopPropagation();
        e.preventDefault();
      } else if (type === "pointerdown") {
        closeMenu();
      }
    }, true);
  }
  addEventListener("keydown", (e) => {
    if (e.key === "Escape" && menu) {
      e.stopPropagation();
      closeMenu();
    }
  }, true);

  const run = (plugin, command) => {
    signal("run", { manifest: plugin.manifest, command: command ?? "" });
    closeActions();
  };

  const row = (plugin) => {
    const node = el("div", `${CLASSES.row} ligma-row`);
    node.addEventListener("mouseenter", () => node.classList.add(CLASSES.rowHover));
    node.addEventListener("mouseleave", () => node.classList.remove(CLASSES.rowHover));
    const column = el("div", CLASSES.column);
    const line = el("div", CLASSES.line);
    const button = rowButton(ICONS.plugin);
    const text = el("div", `${CLASSES.nameWrap} ligma-text`);
    const nameLine = el("div", "ligma-name-line");
    const name = el("div", CLASSES.name);
    name.textContent = plugin.problem ? `⚠  ${plugin.name}` : plugin.name;
    if (plugin.problem) node.title = plugin.problem;
    nameLine.append(name, Object.assign(el("span", "ligma-badge"), { textContent: "Development" }));
    const commands = plugin.commands ?? [];
    if (commands.length) nameLine.append(el("span", CLASSES.chevron, ICONS.chevron));
    text.append(nameLine);
    if (devOnly) text.append(Object.assign(el("div", "ligma-path"), { textContent: plugin.folder, title: plugin.folder }));
    button.append(text);
    button.addEventListener("click", (e) => {
      e.stopPropagation();
      if (!commands.length) return run(plugin);
      openMenu(text, commands.map((command) => command && [command[0], () => run(plugin, command[1])]), node);
    });
    const trail = el("div", "ligma-trail");
    if (!devOnly) trail.append(Object.assign(el("span", "ligma-kind"), { textContent: "Plugin" }));
    const options = el("button", "ligma-options", ICONS.dots);
    options.setAttribute("aria-label", `More options for ${plugin.name}`);
    options.addEventListener("click", (e) => {
      e.stopPropagation();
      openMenu(options, [["Remove", () => signal("remove", { manifest: plugin.manifest })]], node, true);
    });
    trail.append(options);
    line.append(button, trail);
    column.append(line);
    node.append(column);
    return node;
  };

  const importRow = () => {
    const node = el("div", `${CLASSES.row} ligma-row`);
    node.addEventListener("mouseenter", () => node.classList.add(CLASSES.rowHover));
    node.addEventListener("mouseleave", () => node.classList.remove(CLASSES.rowHover));
    const button = rowButton(ICONS.import);
    const text = el("div", CLASSES.nameWrap);
    text.append(Object.assign(el("div", CLASSES.name), { textContent: "Import from manifest…" }));
    button.append(text);
    button.addEventListener("click", (e) => {
      e.stopPropagation();
      signal("import");
    });
    const line = el("div", CLASSES.line);
    line.append(button);
    node.append(line);
    return node;
  };

  // The Development option in the filter's menu, while it's open.
  const filterOption = () => {
    const items = document.getElementById("extensions-filter-dropdown-menu")?.firstElementChild;
    if (!items) return;
    let option = items.querySelector("[data-ligma-dev]");
    if (!option) {
      option = el("a", CLASSES.filterOption);
      option.dataset.ligmaDev = "";
      option.setAttribute("role", "menuitem");
      option.tabIndex = -1;
      option.addEventListener("mouseenter", () => option.classList.add(CLASSES.filterHover));
      option.addEventListener("mouseleave", () => option.classList.remove(CLASSES.filterHover));
      option.addEventListener("click", (e) => {
        e.preventDefault();
        e.stopPropagation();
        devOnly = true;
        document.getElementById("extensions-filter-dropdown-button")?.click();
        schedule();
      });
      items.append(option);
    }
    if (option.getAttribute("aria-checked") !== String(devOnly) || !option.firstChild) {
      option.setAttribute("aria-checked", String(devOnly));
      option.innerHTML = (devOnly ? ICONS.check : `<span class="${CLASSES.check}"></span>`) + `<div class="${CLASSES.filterLabel}">Development</div>`;
    }
  };
  // Figma's own filter options turn Development off.
  addEventListener("click", (e) => {
    const item = e.target.closest?.('#extensions-filter-dropdown-menu [role="menuitem"]:not([data-ligma-dev])');
    if (!item) return;
    figmaFilter = [...item.parentElement.querySelectorAll('[role="menuitem"]:not([data-ligma-dev])')].indexOf(item);
    devOnly = false;
    schedule();
  }, true);

  let drawn = "";
  const render = () => {
    if (!document.getElementById("ligma-dev-style") && document.head) {
      document.head.append(Object.assign(el("style"), { id: "ligma-dev-style", textContent: STYLE }));
    }
    const tab = document.querySelector('[data-testid="tab-extensions"]');
    const active = tab?.getAttribute("aria-selected") === "true";
    if (!active) {
      devOnly = false;
      figmaFilter = tab ? figmaFilter : 0;
    }
    document.documentElement.classList.toggle("ligma-dev-filter", devOnly);
    filterOption();
    let box = document.getElementById("ligma-dev-plugins");
    if (!active) {
      box?.remove();
      drawn = "";
      return;
    }
    let root = tab;
    while (root && !(root.querySelector("input") && root.querySelector('[class*="list--list"]'))) root = root.parentElement;
    const list = root?.querySelector('[class*="list--list"]');
    if (!list) return;
    const query = (root.querySelector("input")?.value ?? "").trim().toLowerCase();
    const types = editorTypes();
    const shown = devOnly || figmaFilter !== 2
      ? plugins.filter((plugin) => plugin.editorTypes.some((type) => types.includes(type)) && (!query || plugin.name.toLowerCase().includes(query)))
      : [];
    const state = JSON.stringify([devOnly, query, shown]);
    if (box?.parentElement === list && list.firstElementChild === box && state === drawn) return;
    drawn = state;
    harvest(list);
    if (!box) {
      box = el("div");
      box.id = "ligma-dev-plugins";
    }
    box.classList.toggle("ligma-only", devOnly);
    box.replaceChildren(...shown.map(row), ...(devOnly && !query ? [importRow()] : []));
    if (list.firstElementChild !== box) list.prepend(box);
  };
  let scheduled = false;
  const schedule = () => {
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(() => {
      scheduled = false;
      render();
    });
  };
  new MutationObserver((mutations) => {
    // Not for its own changes.
    if (mutations.every((m) => m.target.closest?.("#ligma-dev-plugins, .ligma-menu, [data-ligma-dev]"))) return;
    schedule();
  }).observe(document, { childList: true, subtree: true, attributes: true, attributeFilter: ["aria-selected"] });
  addEventListener("input", schedule, true);

  Object.defineProperty(window, "__ligmaDevPlugins", {
    value: {
      set(list) {
        plugins = list;
        drawn = "";
        schedule();
      },
    },
  });
})();"##;

// Settings > Plugins.

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plugin {
    /// Its `manifest.json`, which identifies it here.
    manifest: String,
    /// The folder it's in, to show.
    folder: String,
    name: String,
    enabled: bool,
    /// Editors it runs in, by name.
    editors: Vec<&'static str>,
    /// Why it can't run, if it can't.
    problem: Option<String>,
}

fn describe(entry: &Entry) -> Plugin {
    let folder = entry.manifest.parent().map_or_else(String::new, |dir| dir.display().to_string());
    let fallback_name = entry
        .manifest
        .parent()
        .and_then(Path::file_name)
        .map_or_else(|| "Plugin".to_owned(), |name| name.to_string_lossy().into_owned());
    let (name, editors, problem) = match read_manifest(&entry.manifest) {
        Ok(manifest) => {
            let editors = manifest
                .editor_types
                .iter()
                .filter_map(|t| match t.as_str() {
                    "figma" => Some("Design"),
                    "figjam" => Some("FigJam"),
                    "dev" => Some("Dev Mode"),
                    "slides" => Some("Slides"),
                    "buzz" => Some("Buzz"),
                    _ => None,
                })
                .collect();
            let problem = problem(&manifest);
            (manifest.name, editors, problem)
        }
        Err(error) => (fallback_name, vec![], Some(error)),
    };
    Plugin {
        manifest: entry.manifest.display().to_string(),
        folder,
        name,
        enabled: entry.enabled,
        editors,
        problem,
    }
}

fn from_settings_window(webview: &Webview) -> bool {
    webview.label() == crate::settings::WINDOW
}

#[tauri::command]
pub async fn get_plugins(app: AppHandle, webview: Webview) -> Vec<Plugin> {
    if !from_settings_window(&webview) {
        return vec![];
    }
    load(&app).iter().map(describe).collect()
}

/// Asks for a plugin's `manifest.json`, over `window`, and adds it,
/// enabled. Ok(false) if the dialog was cancelled.
async fn import(app: &AppHandle, window: &Window) -> Result<bool, String> {
    let picked = rfd::AsyncFileDialog::new()
        .set_parent(window)
        .set_title("Import Plugin from Manifest")
        .add_filter("Plugin manifest", &["json"])
        .pick_file()
        .await;
    let Some(picked) = picked else {
        return Ok(false);
    };
    let manifest = picked.path().to_path_buf();
    if manifest.file_name().is_some_and(|name| name != "manifest.json") {
        return Err("Pick the plugin's manifest.json.".into());
    }
    let name = read_manifest(&manifest)?.name;
    let mut entries = load(app);
    match entries.iter_mut().find(|entry| entry.manifest == manifest) {
        Some(entry) if entry.enabled => return Err(format!("{name} is already imported.")),
        // Imported but switched off: switch it back on.
        Some(entry) => entry.enabled = true,
        None => entries.push(Entry { manifest, enabled: true }),
    }
    save(app, &entries)?;
    Ok(true)
}

/// Asks for a plugin's `manifest.json` and adds it, enabled. Ok(false) if
/// the dialog was cancelled.
#[tauri::command]
pub async fn import_plugin(app: AppHandle, webview: Webview) -> Result<bool, String> {
    if !from_settings_window(&webview) {
        return Err("not allowed".into());
    }
    import(&app, &webview.window()).await
}

#[tauri::command]
pub async fn set_plugin_enabled(app: AppHandle, webview: Webview, manifest: String, enabled: bool) -> Result<(), String> {
    if !from_settings_window(&webview) {
        return Err("not allowed".into());
    }
    let mut entries = load(&app);
    for entry in entries.iter_mut().filter(|entry| entry.manifest == Path::new(&manifest)) {
        entry.enabled = enabled;
    }
    save(&app, &entries)
}

#[tauri::command]
pub async fn remove_plugin(app: AppHandle, webview: Webview, manifest: String) -> Result<(), String> {
    if !from_settings_window(&webview) {
        return Err("not allowed".into());
    }
    let mut entries = load(&app);
    entries.retain(|entry| entry.manifest != Path::new(&manifest));
    save(&app, &entries)
}

/// Shows the plugin's manifest in Explorer or the Finder.
#[tauri::command]
pub async fn reveal_plugin(app: AppHandle, webview: Webview, manifest: String) {
    let known = load(&app).iter().any(|entry| entry.manifest == Path::new(&manifest));
    if from_settings_window(&webview) && known {
        let _ = app.opener().reveal_item_in_dir(&manifest);
    }
}
