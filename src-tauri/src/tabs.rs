//! Figma-style tabs. The main window's own webview is the local tab bar
//! (`dist/index.html`). Each tab is a child webview below it, and only the
//! active one is visible.
//!
//! Hidden tabs are wound down to save memory, since every loaded Figma file
//! keeps its whole editor in RAM. On hide (Windows) WebView2 is asked to trim
//! memory. After `SUSPEND_AFTER` the page is frozen (on macOS that saves
//! CPU, not memory). After the
//! user's `sleepTabsAfterMinutes` the webview is closed ("asleep") and
//! reloads from its last URL when selected. Only the user's
//! `maxHiddenTabsLoaded` most recently hidden tabs stay loaded, Home counting
//! as the oldest; the rest go to sleep right away. On macOS, which has no trimming, tabs go to
//! sleep sooner while the system is short of memory.
//! Freezing and closing only happen when `CAN_REST_JS` says no work would be
//! lost. With `reloadSnapshots` on macOS, a tab's last image is kept when
//! it's hidden and shown in its place while it reloads after sleep, until
//! `READY_JS` says Figma has drawn the file.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::webview::{NewWindowResponse, WebviewBuilder};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Url, Webview, WebviewUrl, Window};
use tauri_plugin_opener::OpenerExt;

pub const TAB_BAR_HEIGHT: f64 = 38.0;
pub const HOME_URL: &str = "https://www.figma.com/files";
const HOME_ID: u32 = 0;

// WKWebView's default UA has no Safari version token, which can trigger
// Figma's "unsupported browser" notice. Present as current Safari instead.
#[cfg(target_os = "macos")]
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";

// Figma only asks its font agent for local fonts once Local Network Access
// is "granted". WebView2 doesn't enforce that permission (loopback requests
// already go through) but still reports "prompt" and never shows a prompt,
// so Figma would wait forever. Report "prompt" as "granted"; a real "denied"
// is left alone.
#[cfg(windows)]
const LOCAL_NETWORK_ACCESS_JS: &str = r#"(() => {
  const names = new Set(["loopback-network", "local-network", "local-network-access"]);
  const query = Permissions.prototype.query;
  Permissions.prototype.query = async function (descriptor) {
    const status = await query.call(this, descriptor);
    if (names.has(descriptor?.name) && status.state === "prompt") {
      Object.defineProperty(status, "state", { get: () => "granted" });
    }
    return status;
  };
})();"#;

/// Remembers the page's `beforeunload` handlers so `CAN_REST_JS` can ask
/// them, without unloading anything, whether leaving now would lose work.
/// Figma registers one while edits haven't reached its servers.
const BEFOREUNLOAD_JS: &str = r#"(() => {
  const handlers = new Set();
  const add = EventTarget.prototype.addEventListener;
  const remove = EventTarget.prototype.removeEventListener;
  EventTarget.prototype.addEventListener = function (type, listener, options) {
    if (this === window && type === "beforeunload" && listener) handlers.add(listener);
    return add.call(this, type, listener, options);
  };
  EventTarget.prototype.removeEventListener = function (type, listener, options) {
    if (this === window && type === "beforeunload") handlers.delete(listener);
    return remove.call(this, type, listener, options);
  };
  Object.defineProperty(window, "__ligmaCanRest", {
    value: () => {
      if (!navigator.onLine) return false;
      let blocked = false;
      const event = {
        type: "beforeunload",
        target: window,
        currentTarget: window,
        preventDefault() { blocked = true; },
        stopPropagation() {},
        stopImmediatePropagation() {},
        get returnValue() { return ""; },
        set returnValue(value) { if (value) blocked = true; },
      };
      for (const handler of [...handlers, window.onbeforeunload]) {
        if (!handler) continue;
        try {
          const result = typeof handler === "function" ? handler.call(window, event) : handler.handleEvent(event);
          if (typeof result === "string" && result) blocked = true;
        } catch {
          blocked = true;
        }
      }
      return !blocked;
    },
  });
})();"#;

/// True once a reloading file tab shows its file rather than Figma's loading
/// screen: the editor canvas is there and Figma's progress bar
/// (`progress_bar--*` classes) is gone. If Figma renames those classes the
/// snapshot just goes away early.
const READY_JS: &str =
    r#"!!document.querySelector("canvas") && !document.querySelector('[class*="progress_bar--"]')"#;

/// Longest a reloading tab stays behind its snapshot.
const REVEAL_TIMEOUT: Duration = Duration::from_secs(20);

/// True if a hidden tab can be frozen or closed without losing work.
const CAN_REST_JS: &str = "window.__ligmaCanRest?.() ?? false";

/// How long a tab stays hidden before it's frozen (Windows, macOS).
#[cfg(any(windows, target_os = "macos"))]
const SUSPEND_AFTER: Duration = Duration::from_secs(5 * 60);

/// Longest a tab stays hidden before it's put to sleep while macOS warns of
/// memory pressure. Under critical pressure every hidden tab sleeps at once.
#[cfg(target_os = "macos")]
const PRESSURE_SLEEP_AFTER: Duration = Duration::from_secs(10 * 60);

#[derive(Serialize)]
struct Tab {
    id: u32,
    title: String,
    kind: &'static str,
    /// No webview; reloads from `url` when selected.
    asleep: bool,
    #[serde(skip)]
    file_key: Option<String>,
    /// Last known address, to reload an asleep tab.
    #[serde(skip)]
    url: Url,
    /// Image of the tab from when it was last hidden, as a `data:` URL
    /// (macOS), shown while it reloads after sleep.
    #[serde(skip)]
    snapshot: Option<String>,
    /// Reloading behind its snapshot: the webview stays hidden until
    /// `READY_JS` says the file is drawn.
    #[serde(skip)]
    revealing: bool,
    /// When the tab was last hidden. None while it's the active tab.
    #[serde(skip)]
    hidden_since: Option<Instant>,
    /// Frozen, or freezing or resuming (Windows, macOS). Nothing may `eval`
    /// in the page meanwhile: on macOS WebKit would throw and abort Ligma.
    #[serde(skip)]
    suspended: bool,
}

#[derive(Default)]
struct TabState {
    tabs: Vec<Tab>,
    active: u32,
    next_id: u32,
    /// Colors of the active tab's Figma theme, from `THEME_JS`.
    theme: Option<serde_json::Value>,
    /// Hidden time after which a tab is put to sleep. None: never.
    sleep_after: Option<Duration>,
    /// Hidden tabs that may stay loaded; older ones sleep. None: no limit.
    max_hidden: Option<usize>,
    /// A snapshot is showing, so the tab bar's webview fills the window.
    snapshot_shown: bool,
    /// Show a waking tab's snapshot while it reloads (`reloadSnapshots`).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    snapshots: bool,
}

pub struct Tabs(Mutex<TabState>);

impl Tabs {
    pub fn new(settings: &crate::settings::Settings) -> Self {
        let mut state = TabState::default();
        state.configure(settings);
        Self(Mutex::new(state))
    }
}

impl TabState {
    fn configure(&mut self, settings: &crate::settings::Settings) {
        let minutes = settings.sleep_tabs_after_minutes;
        // Sleeping turned off means every tab stays loaded, limit or not.
        self.sleep_after = (minutes > 0).then(|| Duration::from_secs(minutes * 60));
        self.max_hidden = settings.max_hidden_tabs_loaded.filter(|_| minutes > 0);
        self.snapshots = settings.reload_snapshots;
        if !self.snapshots {
            for tab in self.tabs.iter_mut().filter(|tab| !tab.revealing) {
                tab.snapshot = None;
            }
        }
    }
}

/// Applies changed settings. Tabs now past a shorter timeout or a lower
/// limit go to sleep right away.
pub fn apply_settings(app: &AppHandle, settings: &crate::settings::Settings) {
    app.state::<Tabs>().inner().0.lock().unwrap().configure(settings);
    rest_idle(app);
}

/// Reads Figma's theme colors. Figma sets `data-preferred-theme` on <body>
/// and defines its palette as CSS variables there; FigJam and Dev Mode
/// override them, so this follows the active editor too.
const THEME_JS: &str = r#"(() => {
  const el = document.querySelector("[data-preferred-theme]");
  if (!el) return null;
  const style = getComputedStyle(el);
  const v = (name) => style.getPropertyValue(name).trim();
  return {
    bg: v("--color-bg"),
    bgSecondary: v("--color-bg-secondary"),
    bgTertiary: v("--color-bg-tertiary"),
    text: v("--color-text"),
    textSecondary: v("--color-text-secondary"),
    border: v("--color-border"),
  };
})()"#;

fn label(id: u32) -> String {
    format!("tab-{id}")
}

fn is_figma(url: &Url) -> bool {
    url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| host == "figma.com" || host.ends_with(".figma.com"))
}

/// Content Figma serves from its own domains inside its pages, like the
/// `*.figma.site` previews of Make and Sites. On macOS `on_navigation` also
/// sees iframe loads, so these must stay in the page rather than go to the
/// browser.
fn is_figma_content(url: &Url) -> bool {
    url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| host == "figma.site" || host.ends_with(".figma.site"))
}

/// Tab icon for a Figma URL, from the first path segment (`/design/<key>/...`).
fn kind(url: &Url) -> &'static str {
    match url.path_segments().and_then(|mut s| s.next()) {
        Some("design" | "file") => "design",
        Some("board") => "board",
        Some("slides" | "deck") => "slides",
        Some("proto") => "proto",
        Some("make") => "make",
        Some("site") => "site",
        Some("buzz") => "buzz",
        _ => "page",
    }
}

/// Key of the Figma file an editor URL points at, if any.
fn file_key(url: &Url) -> Option<String> {
    if !is_figma(url) || kind(url) == "page" {
        return None;
    }
    url.path_segments()?
        .nth(1)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
}

/// Opens `url` in a new tab, or focuses the tab already showing that file.
pub fn open(app: &AppHandle, url: Url) -> tauri::Result<()> {
    let key = file_key(&url);
    let id = {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        let existing = key.as_ref().and_then(|key| {
            let tab = state.tabs.iter().find(|tab| tab.file_key.as_ref() == Some(key))?;
            Some(tab.id)
        });
        if let Some(id) = existing {
            drop(state);
            return select(app, id);
        }
        let id = state.next_id;
        state.next_id += 1;
        state.tabs.push(Tab {
            id,
            title: String::new(),
            kind: if id == HOME_ID { "home" } else { kind(&url) },
            asleep: false,
            file_key: key,
            url: url.clone(),
            hidden_since: None,
            suspended: false,
            snapshot: None,
            revealing: false,
        });
        id
    };
    create_webview(app, id, url)?;
    select(app, id)
}

/// Loads tab `id`'s webview, for a new tab or one waking up.
fn create_webview(app: &AppHandle, id: u32, url: Url) -> tauri::Result<()> {
    let window = app.get_window("main").expect("main window exists");
    let (nav_app, popup_app, title_app) = (app.clone(), app.clone(), app.clone());
    #[allow(unused_mut)]
    let mut builder = WebviewBuilder::new(label(id), WebviewUrl::External(url))
        // Let Figma receive HTML5 drag-and-drop (image/file imports).
        .disable_drag_drop_handler()
        .on_navigation(move |url| on_navigation(&nav_app, id, url))
        .on_new_window(move |url, _| {
            on_new_window(&popup_app, url);
            NewWindowResponse::Deny
        })
        .on_document_title_changed(move |webview, title| {
            on_title_changed(&title_app, id, &webview, &title)
        })
        .initialization_script(BEFOREUNLOAD_JS);
    #[cfg(target_os = "macos")]
    {
        builder = builder.user_agent(USER_AGENT);
    }
    #[cfg(windows)]
    {
        builder = builder.initialization_script(LOCAL_NETWORK_ACCESS_JS);
    }
    window.add_child(
        builder,
        LogicalPosition::new(0.0, TAB_BAR_HEIGHT),
        content_size(&window)?,
    )?;
    Ok(())
}

/// `open` from a webview callback. Those run on the main thread, and
/// creating a webview there would block on itself.
fn open_later(app: &AppHandle, url: Url) {
    let app = app.clone();
    std::thread::spawn(move || {
        let _ = open(&app, url);
    });
}

fn open_in_browser(app: &AppHandle, url: &Url) {
    let _ = app.opener().open_url(url.as_str(), None::<&str>);
}

fn on_navigation(app: &AppHandle, id: u32, url: &Url) -> bool {
    match url.scheme() {
        "http" | "https" => {}
        // blob:, data:, about: and friends stay in the page.
        _ => return true,
    }
    if is_figma_content(url) {
        return true;
    }
    if !is_figma(url) {
        open_in_browser(app, url);
        return false;
    }
    // Like the desktop app, files opened from Home get their own tab.
    if id == HOME_ID && file_key(url).is_some() {
        open_later(app, url.clone());
        return false;
    }
    true
}

fn on_new_window(app: &AppHandle, url: Url) {
    if is_figma(&url) {
        open_later(app, url);
    } else if matches!(url.scheme(), "http" | "https") {
        open_in_browser(app, &url);
    }
}

fn on_title_changed(app: &AppHandle, id: u32, webview: &Webview, title: &str) {
    let url = webview.url().ok();
    // Figma can switch pages client-side without a navigation, so a file
    // can still end up in Home. Move it to its own tab.
    if id == HOME_ID {
        if let Some(url) = url.as_ref().filter(|url| file_key(url).is_some()) {
            let _ = webview.navigate(HOME_URL.parse().unwrap());
            open_later(app, url.clone());
            return;
        }
    }
    {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        tab.title = title.strip_suffix(" – Figma").unwrap_or(title).to_owned();
        if let Some(url) = url {
            if id != HOME_ID {
                tab.kind = kind(&url);
                tab.file_key = file_key(&url);
            }
            tab.url = url;
        }
    }
    push_state(app);
}

pub fn select(app: &AppHandle, id: u32) -> tauri::Result<()> {
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
    let (previous, wake, resume, snapshot) = {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == id) else {
            return Ok(());
        };
        tab.hidden_since = None;
        // On macOS resuming takes a round trip; `suspended` stays set until
        // the page runs again.
        let resume = if cfg!(target_os = "macos") {
            tab.suspended
        } else {
            std::mem::take(&mut tab.suspended)
        };
        let wake = std::mem::take(&mut tab.asleep).then(|| tab.url.clone());
        // Waking with a snapshot: show it while the page loads.
        tab.revealing |= wake.is_some() && tab.snapshot.is_some() && tab.file_key.is_some();
        if !tab.revealing {
            tab.snapshot = None;
        }
        let snapshot = tab.snapshot.clone();
        let previous = std::mem::replace(&mut state.active, id);
        if previous != id {
            if let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == previous) {
                tab.hidden_since = Some(Instant::now());
            }
        }
        (previous, wake, resume, snapshot)
    };
    show_snapshot(app, snapshot.as_deref());
    if let Some(url) = wake {
        // Fails if the old webview is still closing. Stay asleep, so
        // selecting the tab again retries.
        if create_webview(app, id, url).is_err() {
            with_tab(app, id, |tab| {
                tab.asleep = true;
                tab.revealing = false;
            });
        } else if snapshot.is_some() {
            reveal_when_ready(app, id);
        }
    }
    if let Some(webview) = app.get_webview(&label(id)) {
        if snapshot.is_some() {
            webview.hide()?;
        } else {
            webview.show()?;
            webview.set_focus()?;
        }
        #[cfg(windows)]
        crate::webview2::on_shown(&webview);
        #[cfg(target_os = "macos")]
        if resume {
            let app = app.clone();
            crate::wkwebview::resume(&webview, move || {
                with_tab(&app, id, |tab| tab.suspended = false);
            });
        }
    }
    sync_theme(app);
    if previous != id {
        if let Some(webview) = app.get_webview(&label(previous)) {
            #[cfg(target_os = "macos")]
            take_snapshot(app, previous, &webview);
            webview.hide()?;
            #[cfg(windows)]
            crate::webview2::on_hidden(&webview);
        }
        // A tab past `max_hidden` sleeps now, not at the next timer tick.
        rest_idle(app);
    }
    push_state(app);
    Ok(())
}

/// Shows `snapshot` where tab webviews go, or hides it.
fn show_snapshot(app: &AppHandle, snapshot: Option<&str>) {
    let Some(bar) = app.get_webview("main") else {
        return;
    };
    app.state::<Tabs>().inner().0.lock().unwrap().snapshot_shown = snapshot.is_some();
    let _ = match snapshot {
        Some(url) => bar.eval(format!("window.showSnapshot?.({})", serde_json::to_string(url).unwrap())),
        None => bar.eval("window.hideSnapshot?.()"),
    };
    fit_tab_bar(app);
}

/// Sizes the tab bar's webview to the bar, or to the whole window while it
/// shows a snapshot. Kept off the area under the bar otherwise: tab webviews
/// lie on top of it there, and WebKit would never see the mouse leave the
/// bar, leaving the last hovered tab highlighted.
fn fit_tab_bar(app: &AppHandle) {
    let (Some(window), Some(bar)) = (app.get_window("main"), app.get_webview("main")) else {
        return;
    };
    let (Ok(size), Ok(scale)) = (window.inner_size(), window.scale_factor()) else {
        return;
    };
    let size = size.to_logical::<f64>(scale);
    let full = app.state::<Tabs>().inner().0.lock().unwrap().snapshot_shown;
    let height = if full { size.height } else { TAB_BAR_HEIGHT.min(size.height) };
    let _ = bar.set_size(LogicalSize::new(size.width, height));
}

/// Keeps an image of file tab `id` for when it wakes from sleep. Skipped
/// while frozen (WebKit can't) or reloading (the old image is better).
#[cfg(target_os = "macos")]
fn take_snapshot(app: &AppHandle, id: u32, webview: &Webview) {
    let wanted = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        let tab = state.tabs.iter().find(|tab| tab.id == id);
        state.snapshots && tab.is_some_and(|tab| tab.file_key.is_some() && !tab.suspended && !tab.revealing)
    };
    if !wanted {
        return;
    }
    let app = app.clone();
    crate::wkwebview::snapshot(webview, move |url| {
        if url.is_some() {
            with_tab(&app, id, |tab| tab.snapshot = url);
        }
    });
}

/// Shows reloading tab `id` once `READY_JS` has said the file is drawn
/// twice in a row, or after `REVEAL_TIMEOUT`.
fn reveal_when_ready(app: &AppHandle, id: u32) {
    let app = app.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let mut ready_count = 0;
        loop {
            std::thread::sleep(Duration::from_millis(250));
            if with_tab(&app, id, |tab| tab.revealing) != Some(true) {
                return;
            }
            let Some(webview) = app.get_webview(&label(id)) else {
                return;
            };
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = webview.eval_with_callback(READY_JS, move |json| {
                let _ = tx.send(json == "true");
            });
            let ready = rx.recv_timeout(Duration::from_secs(1)).unwrap_or(false);
            ready_count = if ready { ready_count + 1 } else { 0 };
            if ready_count >= 2 || started.elapsed() >= REVEAL_TIMEOUT {
                break;
            }
        }
        let active = {
            let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
            let active = state.active;
            let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == id) else {
                return;
            };
            tab.revealing = false;
            tab.snapshot = None;
            active == id
        };
        if active {
            if let Some(webview) = app.get_webview(&label(id)) {
                let _ = webview.show();
                let _ = webview.set_focus();
            }
            show_snapshot(&app, None);
        }
    });
}

/// Closes a tab. Home can't be closed.
pub fn close(app: &AppHandle, id: u32) -> tauri::Result<()> {
    if id == HOME_ID {
        return Ok(());
    }
    let next = {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        let Some(index) = state.tabs.iter().position(|tab| tab.id == id) else {
            return Ok(());
        };
        state.tabs.remove(index);
        // Prefer the tab to the right, like the desktop app. Home is always
        // left, so the list is never empty.
        (state.active == id).then(|| state.tabs[index.min(state.tabs.len() - 1)].id)
    };
    if let Some(next) = next {
        select(app, next)?;
    }
    if let Some(webview) = app.get_webview(&label(id)) {
        webview.close()?;
    }
    push_state(app);
    Ok(())
}

pub fn close_active(app: &AppHandle) -> tauri::Result<()> {
    let active = app.state::<Tabs>().inner().0.lock().unwrap().active;
    close(app, active)
}

/// Selects the tab `offset` places from the active one, wrapping around.
pub fn cycle(app: &AppHandle, offset: isize) -> tauri::Result<()> {
    let id = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        let len = state.tabs.len() as isize;
        let index = state.tabs.iter().position(|tab| tab.id == state.active).unwrap_or(0) as isize;
        state.tabs[(index + offset).rem_euclid(len) as usize].id
    };
    select(app, id)
}

pub fn new_tab(app: &AppHandle) -> tauri::Result<()> {
    open(app, HOME_URL.parse().unwrap())
}

/// Title and webview of the active tab, if it shows a Figma file and can
/// run JavaScript (not still resuming).
pub fn active_file(app: &AppHandle) -> Option<(String, Webview)> {
    let (id, title) = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        let tab = state.tabs.iter().find(|tab| tab.id == state.active && !tab.suspended)?;
        tab.file_key.as_ref()?;
        (tab.id, tab.title.clone())
    };
    Some((title, app.get_webview(&label(id))?))
}

/// Runs `f` on tab `id`, if it's still open.
fn with_tab<T>(app: &AppHandle, id: u32, f: impl FnOnce(&mut Tab) -> T) -> Option<T> {
    let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
    state.tabs.iter_mut().find(|tab| tab.id == id).map(f)
}

#[derive(Clone, Copy)]
enum Rest {
    #[cfg(any(windows, target_os = "macos"))]
    Suspend,
    Sleep,
}

/// Hidden time after which a tab is put to sleep (None: never). Shorter on
/// macOS while memory is short, but a user who turned sleeping off still
/// never gets tabs closed.
fn sleep_after(setting: Option<Duration>) -> Option<Duration> {
    #[cfg(target_os = "macos")]
    {
        use crate::pressure::Pressure;
        match crate::pressure::current() {
            Pressure::Normal => {}
            Pressure::Warn => return setting.map(|after| after.min(PRESSURE_SLEEP_AFTER)),
            Pressure::Critical => return setting.map(|_| Duration::ZERO),
        }
    }
    setting
}

/// Freezes or closes tabs that have been hidden long enough. Call
/// periodically, and more often while `under_pressure`.
pub fn rest_idle(app: &AppHandle) {
    let now = Instant::now();
    let due: Vec<(u32, Instant, Rest, bool)> = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        let sleep_after = sleep_after(state.sleep_after);
        // Loaded hidden tabs are ranked most recently hidden first, but Home
        // last: it reloads quickly, so it's the first to sleep. A tab with
        // `max_hidden` tabs ranked above it sleeps whatever its time.
        let loaded: Vec<(u32, Instant)> = state
            .tabs
            .iter()
            .filter(|tab| !tab.asleep)
            .filter_map(|tab| Some((tab.id, tab.hidden_since?)))
            .collect();
        let over_limit = |id: u32, since: Instant| {
            let above = |&&(other, t): &&(u32, Instant)| {
                other != id && (id == HOME_ID || (other != HOME_ID && t > since))
            };
            state.max_hidden.is_some_and(|max| loaded.iter().filter(above).count() >= max)
        };
        state
            .tabs
            .iter()
            .filter_map(|tab| {
                let since = tab.hidden_since.filter(|_| !tab.asleep)?;
                let hidden = now - since;
                if over_limit(tab.id, since) || sleep_after.is_some_and(|after| hidden >= after) {
                    return Some((tab.id, since, Rest::Sleep, tab.suspended));
                }
                #[cfg(any(windows, target_os = "macos"))]
                if !tab.suspended && hidden >= SUSPEND_AFTER {
                    return Some((tab.id, since, Rest::Suspend, false));
                }
                None
            })
            .collect()
    };
    for (id, since, rest, suspended) in due {
        let Some(webview) = app.get_webview(&label(id)) else {
            continue;
        };
        // A frozen page can't answer, and hasn't changed since it last said
        // it could rest.
        if suspended {
            rest_now(app, id, since, rest);
            continue;
        }
        let app = app.clone();
        let _ = webview.eval_with_callback(CAN_REST_JS, move |json| {
            if json == "true" {
                // Not from inside the webview's own callback.
                let app = app.clone();
                std::thread::spawn(move || rest_now(&app, id, since, rest));
            }
        });
    }
}

/// True while macOS reports memory pressure, when `rest_idle` should run
/// more often than usual.
pub fn under_pressure() -> bool {
    #[cfg(target_os = "macos")]
    return crate::pressure::current() != crate::pressure::Pressure::Normal;
    #[cfg(not(target_os = "macos"))]
    false
}

/// Applies `rest` to tab `id`, unless it was shown again after `since`.
fn rest_now(app: &AppHandle, id: u32, since: Instant, rest: Rest) {
    let Some(webview) = app.get_webview(&label(id)) else {
        return;
    };
    let still_hidden = move |tab: &Tab| tab.hidden_since == Some(since) && !tab.asleep;
    match rest {
        #[cfg(any(windows, target_os = "macos"))]
        Rest::Suspend => {
            // Marked before freezing, so nothing evals in the page meanwhile.
            let marked = with_tab(app, id, |tab| {
                let hidden = still_hidden(tab);
                tab.suspended |= hidden;
                hidden
            });
            if marked != Some(true) {
                return;
            }
            let app = app.clone();
            #[cfg(windows)]
            use crate::webview2::try_suspend;
            #[cfg(target_os = "macos")]
            use crate::wkwebview::try_suspend;
            try_suspend(&webview, move |ok| {
                // Once shown again, `select` owns the flag.
                with_tab(&app, id, |tab| {
                    if still_hidden(tab) {
                        tab.suspended = ok;
                    }
                });
            });
        }
        Rest::Sleep => {
            let url = webview.url().ok();
            let slept = with_tab(app, id, |tab| {
                if !still_hidden(tab) {
                    return false;
                }
                tab.asleep = true;
                tab.suspended = false;
                tab.revealing = false;
                if let Some(url) = url {
                    tab.url = url;
                }
                true
            });
            if slept == Some(true) {
                let _ = webview.close();
                push_state(app);
            }
        }
    }
}

/// Matches the tab bar to the active tab's Figma theme. Runs on tab switch
/// and on a timer, since Figma changes theme without any event Ligma sees.
pub fn sync_theme(app: &AppHandle) {
    let active = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        if state.tabs.iter().any(|tab| tab.id == state.active && tab.suspended) {
            return;
        }
        state.active
    };
    let Some(webview) = app.get_webview(&label(active)) else {
        return;
    };
    let app = app.clone();
    let _ = webview.eval_with_callback(THEME_JS, move |json| {
        // null until Figma has rendered its UI.
        let Ok(theme @ serde_json::Value::Object(_)) = serde_json::from_str(&json) else {
            return;
        };
        {
            let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
            if state.active != active || state.theme.as_ref() == Some(&theme) {
                return;
            }
            state.theme = Some(theme);
        }
        push_state(&app);
    });
}

fn content_size(window: &Window) -> tauri::Result<LogicalSize<f64>> {
    let size = window.inner_size()?.to_logical::<f64>(window.scale_factor()?);
    Ok(LogicalSize::new(size.width, (size.height - TAB_BAR_HEIGHT).max(0.0)))
}

/// Refits tab webviews to the window. Call on resize.
pub fn layout(app: &AppHandle) {
    let Some(window) = app.get_window("main") else {
        return;
    };
    let Ok(size) = content_size(&window) else {
        return;
    };
    for webview in window.webviews() {
        if webview.label().starts_with("tab-") {
            let _ = webview.set_size(size);
        }
    }
    fit_tab_bar(app);
    push_state(app);
}

/// Sends the tab list to the tab bar.
fn push_state(app: &AppHandle) {
    #[derive(Serialize)]
    struct View<'a> {
        tabs: &'a [Tab],
        active: u32,
        fullscreen: bool,
        theme: &'a Option<serde_json::Value>,
        platform: &'static str,
    }

    // Not `get_webview_window`: that returns None once the window holds
    // more than one webview.
    let (Some(window), Some(bar)) = (app.get_window("main"), app.get_webview("main")) else {
        return;
    };
    let fullscreen = window.is_fullscreen().unwrap_or(false);
    let json = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        serde_json::to_string(&View {
            tabs: &state.tabs,
            active: state.active,
            fullscreen,
            theme: &state.theme,
            platform: std::env::consts::OS,
        })
        .unwrap()
    };
    let _ = bar.eval(format!("window.render?.({json})"));
}

/// Only the local tab bar may drive tabs. Figma pages have no IPC access
/// anyway (no capability grants it), this is a second fence.
fn from_tab_bar(webview: &Webview) -> bool {
    webview.label() == "main"
}

#[tauri::command]
pub async fn tab_bar_ready(app: AppHandle, webview: Webview) {
    if from_tab_bar(&webview) {
        push_state(&app);
    }
}

#[tauri::command]
pub async fn select_tab(app: AppHandle, webview: Webview, id: u32) {
    if from_tab_bar(&webview) {
        let _ = select(&app, id);
    }
}

#[tauri::command]
pub async fn close_tab(app: AppHandle, webview: Webview, id: u32) {
    if from_tab_bar(&webview) {
        let _ = close(&app, id);
    }
}

#[tauri::command]
pub async fn open_new_tab(app: AppHandle, webview: Webview) {
    if from_tab_bar(&webview) {
        let _ = new_tab(&app);
    }
}

/// Minimize, maximize and close buttons the tab bar draws where the window
/// has no native title bar (Windows, Linux).
#[tauri::command]
pub async fn window_control(webview: Webview, action: String) {
    if !from_tab_bar(&webview) {
        return;
    }
    let window = webview.window();
    let _ = match action.as_str() {
        "minimize" => window.minimize(),
        "maximize" if window.is_maximized().unwrap_or(false) => window.unmaximize(),
        "maximize" => window.maximize(),
        "close" => window.close(),
        _ => Ok(()),
    };
}
