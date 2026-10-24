//! Development plugins: Figma plugins on this computer, imported from their
//! `manifest.json` like the Figma desktop app's Plugins > Development >
//! Import plugin from manifest…, which figma.com doesn't have.
//!
//! The list is `plugins.json` next to `settings.json`, and Settings >
//! Plugins (`dist/settings.html`) imports, switches on and off, and removes
//! them. Enabled plugins are run from the tab bar's Plugins button, in the
//! file tab it's on.
//!
//! Figma's web editor offers its plugin API on the page as `window.figma`,
//! for the developer console, in files the user can edit. `RUNNER_JS` runs
//! the plugin's `main` against it, with `__html__` and `__uiFiles__` from
//! its `ui`, so `figma.showUI` and messages to and from the UI work as in
//! the desktop app. Unlike there, the code isn't sandboxed: it runs with
//! the page's own access, which Settings says. The files are read on every
//! run, so a rebuilt plugin runs its new code.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, Webview};
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
    /// "Submenu › Command". Empty if it has no menu.
    commands: Vec<(String, String)>,
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

fn flatten_menu(menu: &[Value], prefix: &str, commands: &mut Vec<(String, String)>) {
    for item in menu {
        let Some(name) = item["name"].as_str() else {
            continue;
        };
        let text = format!("{prefix}{name}");
        if let Some(submenu) = item["menu"].as_array() {
            flatten_menu(submenu, &format!("{text} › "), commands);
        } else if let Some(command) = item["command"].as_str() {
            commands.push((text, command.to_owned()));
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
    *ANY_ENABLED.lock().unwrap() = Some(entries.iter().any(|entry| entry.enabled));
    // The tab bar shows its Plugins button only with a plugin enabled.
    crate::tabs::refresh_bar(app);
    Ok(())
}

/// Whether a plugin is switched on, known since the last change.
static ANY_ENABLED: Mutex<Option<bool>> = Mutex::new(None);

/// True if a plugin is switched on, for the tab bar's Plugins button.
pub fn any_enabled(app: &AppHandle) -> bool {
    *ANY_ENABLED
        .lock()
        .unwrap()
        .get_or_insert_with(|| load(app).iter().any(|entry| entry.enabled))
}

/// Plugin editor types that run in a tab of `kind` (see tabs.rs).
fn editor_types(kind: &str) -> &'static [&'static str] {
    match kind {
        "design" => &["figma", "dev"],
        "board" => &["figjam"],
        "slides" => &["slides"],
        "buzz" => &["buzz"],
        _ => &[],
    }
}

/// Plugins menu items: (manifest, command).
static MENU: Mutex<Vec<(PathBuf, Option<String>)>> = Mutex::new(Vec::new());

/// Opens the Plugins menu for tab `tab`, showing a file of `kind`: the
/// enabled plugins that run in its editor, with their commands.
pub fn show_menu(app: &AppHandle, tab: u32, kind: &str) -> tauri::Result<()> {
    let types = editor_types(kind);
    let mut items: Vec<crate::tab_menu::Item> = Vec::new();
    let mut targets = Vec::new();
    for entry in load(app).into_iter().filter(|entry| entry.enabled) {
        let Ok(manifest) = read_manifest(&entry.manifest) else {
            continue;
        };
        if !manifest.editor_types.iter().any(|t| types.contains(&t.as_str())) {
            continue;
        }
        if manifest.commands.is_empty() {
            items.push(Some((format!("plugins:{}", targets.len()), manifest.name.clone())));
            targets.push((entry.manifest.clone(), None));
        }
        for (text, command) in &manifest.commands {
            items.push(Some((format!("plugins:{}", targets.len()), format!("{} › {text}", manifest.name))));
            targets.push((entry.manifest.clone(), Some(command.clone())));
        }
    }
    if items.is_empty() {
        let product = match kind {
            "board" => "FigJam",
            "slides" => "Figma Slides",
            "buzz" => "Figma Buzz",
            _ => "Figma Design",
        };
        items.push(Some((String::new(), format!("No plugins for {product}"))));
    }
    items.push(None);
    items.push(Some(("manage-plugins".into(), "Manage Plugins…".into())));
    *MENU.lock().unwrap() = targets;
    crate::tab_menu::show(app, tab, items)
}

/// Runs a Plugins menu item in tab `tab`, given its action without the
/// `plugins:` prefix.
pub fn on_menu(app: &AppHandle, tab: u32, action: &str) {
    let target = action.parse::<usize>().ok().and_then(|index| MENU.lock().unwrap().get(index).cloned());
    if let Some((manifest, command)) = target {
        run(app, tab, &manifest, command);
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

/// Asks for a plugin's `manifest.json` and adds it, enabled. Ok(false) if
/// the dialog was cancelled.
#[tauri::command]
pub async fn import_plugin(app: AppHandle, webview: Webview) -> Result<bool, String> {
    if !from_settings_window(&webview) {
        return Err("not allowed".into());
    }
    let window = webview.window();
    let picked = rfd::AsyncFileDialog::new()
        .set_parent(&window)
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
    let mut entries = load(&app);
    if entries.iter().any(|entry| entry.manifest == manifest) {
        return Err(format!("{name} is already imported."));
    }
    entries.push(Entry { manifest, enabled: true });
    save(&app, &entries)?;
    Ok(true)
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
