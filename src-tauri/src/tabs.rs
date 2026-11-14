//! Figma-style tabs. The main window's own webview is the local tab bar
//! (`dist/index.html`). Each tab is a child webview below it, and only the
//! active one is visible
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
//! `READY_JS` says Figma has drawn the file. With `tabPreviews` the image
//! is kept too, for the card hovering a tab shows (see preview.rs)
//!
//! Tabs can be dragged out of the bar into a window of their own, or into
//! another window's bar. Each window has its own tab bar and active tab;
//! Home stays in the main window. A moved tab's webview is reparented, so
//! its page carries on where it was. A window other than the main one
//! closes with its last tab
//!
//! Two file tabs of a window can share its canvas in a split view (see
//! split.rs). Selecting either keeps the split; selecting another tab ends
//! it

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::webview::{NewWindowResponse, PageLoadEvent, WebviewBuilder};
use tauri::window::Color;
use tauri::{
    AppHandle, LogicalPosition, LogicalSize, Manager, PhysicalPosition, Theme, Url, Webview, WebviewUrl, WebviewWindowBuilder,
    Window, WindowEvent,
};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_opener::OpenerExt;

/// At 100% interface scale; it grows with the scale
pub const TAB_BAR_HEIGHT: f64 = 38.0;
pub const HOME_URL: &str = "https://www.figma.com/files";
pub const HOME_ID: u32 = 0;
/// The window Ligma opens with, which holds Home. Its own webview, like
/// every window's, is its tab bar and has its label
pub const MAIN: &str = "main";
/// Label prefix of the windows tabs are moved into
const WINDOW_PREFIX: &str = "window-";

// WKWebView's default UA has no Safari version token, which can trigger
// Figma's "unsupported browser" notice. Present as current Safari instead
#[cfg(target_os = "macos")]
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Safari/605.1.15";

// Figma only asks its font agent for local fonts once Local Network Access
// is "granted". WebView2 doesn't enforce that permission (loopback requests
// already go through) but still reports "prompt" and never shows a prompt,
// so Figma would wait forever. Report "prompt" as "granted"; a real "denied"
// is left alone
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

/// Outside Safari, Figma moves a file name's leading emoji out of the page
/// title and into the favicon (`#custom-emoji-favicon`). Put it back, so
/// the tab bar can show it like on macOS
#[cfg(windows)]
const TITLE_EMOJI_JS: &str = r#"(() => {
  const title = Object.getOwnPropertyDescriptor(Document.prototype, "title");
  Object.defineProperty(Document.prototype, "title", {
    ...title,
    set(value) {
      const href = document.getElementById("custom-emoji-favicon")?.getAttribute("href") ?? "";
      const emoji = /<text[^>]*>(.+?)<\/text>/s.exec(decodeURIComponent(href))?.[1];
      title.set.call(this, emoji && !value.startsWith(emoji) ? `${emoji} ${value}` : value);
    },
  });
})();"#;

/// Figma's "Open in new tab" makes a `target="_blank"` link and clicks it
/// from script, which WKWebView drops without asking `on_new_window`. Send
/// those links to `on_navigation` instead, as a hidden iframe load of
/// `NEW_TAB_HOST`, which macOS reports there. `NEW_WINDOW_JS` opens tabs
/// the same way, through `__ligmaOpenTab`
#[cfg(target_os = "macos")]
const NEW_TAB_JS: &str = r#"(() => {
  const openTab = (url) => {
    const frame = document.createElement("iframe");
    frame.style.display = "none";
    frame.src = "https://open.ligma.invalid/?url=" + encodeURIComponent(url);
    document.documentElement.appendChild(frame);
    setTimeout(() => frame.remove(), 1000);
  };
  Object.defineProperty(window, "__ligmaOpenTab", { value: openTab });
  const click = HTMLAnchorElement.prototype.click;
  HTMLAnchorElement.prototype.click = function () {
    if (this.target !== "_blank" || this.hasAttribute("download")) return click.call(this);
    openTab(this.href);
  };
})()"#;

/// `wkwebview::pass_undo_to_pages` gives Cmd+Z to the page first, and
/// WebKit's fallback to Edit > Undo when the page leaves it alone never
/// arrives in a child webview. So in a text field, such as a layer being
/// renamed, undo the text here once Figma's listeners have passed on it
#[cfg(target_os = "macos")]
const TEXT_UNDO_JS: &str = r#"(() => {
  const isText = (node) =>
    node instanceof HTMLTextAreaElement ||
    (node instanceof HTMLInputElement && !["button", "checkbox", "color", "file", "radio", "range", "reset", "submit"].includes(node.type)) ||
    node?.isContentEditable;
  addEventListener("keydown", (e) => {
    if (e.key.toLowerCase() !== "z" || !e.metaKey || e.ctrlKey || e.altKey || !isText(e.target)) return;
    // Added while the event is still capturing, so it runs after every other listener
    addEventListener("keydown", (late) => {
      if (late !== e || e.defaultPrevented) return;
      e.preventDefault();
      document.execCommand(e.shiftKey ? "redo" : "undo");
    }, { once: true });
  }, true);
})()"#;

/// Ligma denies every new window and opens Figma URLs as tabs instead, so
/// `window.open` returns null. Figma then presents a file in place of its
/// editor (Design), or loses the presentation altogether (Slides, which
/// opens a blank window and sets its URL once it has one). Hand Figma a
/// stand-in window instead, which sends any URL it's later given to a new
/// tab: through `__ligmaOpenTab` on macOS, and on Windows through
/// `window.open` again, which reaches `on_new_window`
const NEW_WINDOW_JS: &str = r#"(() => {
  const open = window.open;
  const openTab = (url) => {
    if (window.__ligmaOpenTab) window.__ligmaOpenTab(url);
    else open.call(window, url, "_blank");
  };
  const standIn = () => {
    let closed = false;
    const go = (url) => {
      if (closed || url == null || String(url) === "" || String(url) === "about:blank") return;
      try {
        openTab(new URL(String(url), location.href).href);
      } catch {}
    };
    const loc = { assign: go, replace: go, reload() {}, toString: () => "about:blank" };
    Object.defineProperty(loc, "href", { get: () => "about:blank", set: go });
    const doc = { open: () => doc, write() {}, writeln() {}, close() {}, title: "" };
    return {
      get closed() { return closed; },
      close() { closed = true; },
      focus() {},
      blur() {},
      postMessage() {},
      opener: null,
      document: doc,
      get location() { return loc; },
      set location(url) { go(url); },
    };
  };
  window.open = function (url, target, ...rest) {
    if (["_self", "_parent", "_top"].includes(target)) return open.call(this, url, target, ...rest);
    if (url == null || String(url) === "" || String(url) === "about:blank") return standIn();
    return open.call(this, url, target, ...rest) ?? standIn();
  };
})();"#;
#[cfg(target_os = "macos")]
const NEW_TAB_HOST: &str = "open.ligma.invalid";

/// Takes "Back to files" and "Open in desktop app" out of the main menu (the
/// Figma logo's, in every editor), and "Open links in desktop app" out of
/// its Preferences: Home has a tab of its own, and Ligma is the desktop app
/// (see `DESKTOP_APP_JS`). A menu group left empty goes with them, separator and
/// all. Figma's menus are React's, so the items are hidden, not removed.
/// Figma spaces a group's first and last items from its separators; a
/// hidden first or last item hands that space to its visible neighbor
const MAIN_MENU_JS: &str = r#"(() => {
  if (location.hostname !== "www.figma.com" || window.top !== window) return;
  const HIDE = new Set(["Back to files", "Open in desktop app", "Open links in desktop app"]);
  const ITEM = '[role="menuitem"], [role="menuitemcheckbox"]';
  const style = document.createElement("style");
  style.textContent = `
    [data-ligma-hidden] { display: none !important; }
    [role="menu"] > li:has([data-ligma-hidden]):not(:has(:is(${ITEM}):not([data-ligma-hidden]))) { display: none !important; }
  `;
  const visible = (item, step) => {
    let next = item[step];
    while (next?.hasAttribute("data-ligma-hidden")) next = next[step];
    return next;
  };
  const handOver = (item, computed, step, side) => {
    const next = visible(item, step);
    if (!next) return;
    for (const box of ["margin", "padding"]) {
      const value = computed.getPropertyValue(`${box}-${side}`);
      if (parseFloat(value)) next.style.setProperty(`${box}-${side}`, value);
    }
  };
  const hide = (root) => {
    // An item, its label as it comes in, or a menu
    const items = [root.closest(ITEM), ...root.querySelectorAll(ITEM)];
    for (const item of items) {
      if (!item || item.hasAttribute("data-ligma-hidden")) continue;
      if (!item.closest('[role="menu"]') || !HIDE.has(item.textContent.trim())) continue;
      // Before it's hidden, while its spacing still shows
      const computed = getComputedStyle(item);
      if (!visible(item, "previousElementSibling")) handOver(item, computed, "nextElementSibling", "top");
      if (!visible(item, "nextElementSibling")) handOver(item, computed, "previousElementSibling", "bottom");
      item.setAttribute("data-ligma-hidden", "");
    }
  };
  new MutationObserver((records) => {
    for (const record of records) {
      for (const node of record.addedNodes) if (node.nodeType === Node.ELEMENT_NODE) hide(node);
    }
  }).observe(document, { childList: true, subtree: true });
  const mount = () => (document.head ? (document.head.append(style), true) : false);
  if (!mount()) document.addEventListener("DOMContentLoaded", mount, { once: true });
})();"#;

/// Keeps every link in Ligma, even with Figma's "Open links in desktop app"
/// preference on. Figma opens links in the desktop app through its font
/// agent (`/figma/desktop/open-url` on 127.0.0.1:44950 or
/// figmadaemon.com:44960), which Ligma starts for fonts. Fail its
/// `/figma/desktop/` requests, as with no desktop app, and Figma stays on
/// the page instead. Its fonts requests are left alone
const DESKTOP_APP_JS: &str = r#"(() => {
  const blocked = (url) => {
    try {
      const { host, pathname } = new URL(String(url), location.href);
      return (host === "127.0.0.1:44950" || host === "figmadaemon.com:44960") && pathname.startsWith("/figma/desktop/");
    } catch {
      return false;
    }
  };
  const fetch = window.fetch;
  window.fetch = function (input, init) {
    const url = input instanceof Request ? input.url : input;
    if (blocked(url)) return Promise.reject(new TypeError("Failed to fetch"));
    return fetch.call(this, input, init);
  };
  const open = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (method, url, ...rest) {
    // An unresolvable host fails the request the way a missing agent would
    return open.call(this, method, blocked(url) ? "https://agent.ligma.invalid/" : url, ...rest);
  };
})();"#;

/// Remembers the page's `beforeunload` handlers so `CAN_REST_JS` can ask
/// them, without unloading anything, whether leaving now would lose work.
/// Figma registers one while edits haven't reached its servers
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
/// snapshot just goes away early
const READY_JS: &str =
    r#"!!document.querySelector("canvas") && !document.querySelector('[class*="progress_bar--"]')"#;

/// Longest a reloading tab stays behind its snapshot
const REVEAL_TIMEOUT: Duration = Duration::from_secs(20);

/// True if a hidden tab can be frozen or closed without losing work, and
/// no AI tool has used its file lately (see mcp_bridge.rs)
const CAN_REST_JS: &str = "(window.__ligmaCanRest?.() ?? false) && !window.__ligmaMcp?.busy()";

/// How long a tab stays hidden before it's frozen (Windows, macOS)
#[cfg(any(windows, target_os = "macos"))]
const SUSPEND_AFTER: Duration = Duration::from_secs(5 * 60);

/// Longest a tab stays hidden before it's put to sleep while macOS warns of
/// memory pressure. Under critical pressure every hidden tab sleeps at once
#[cfg(target_os = "macos")]
const PRESSURE_SLEEP_AFTER: Duration = Duration::from_secs(10 * 60);

#[derive(Serialize)]
struct Tab {
    id: u32,
    title: String,
    kind: &'static str,
    /// No webview; reloads from `url` when selected
    asleep: bool,
    /// Waking from sleep or reloading, until its page or file is drawn. Its
    /// bar shows a spinner, and its title stays as it was meanwhile
    loading: bool,
    /// Kept left of the other tabs, after Home, and shown as its icon only
    pinned: bool,
    #[serde(skip)]
    file_key: Option<String>,
    /// Last known address, to reload an asleep tab
    #[serde(skip)]
    url: Url,
    /// Image of the tab from when it was last on screen, as a `data:` URL:
    /// shown while it reloads after sleep (macOS), and in its preview
    #[serde(skip)]
    snapshot: Option<String>,
    /// Part of `snapshot` that's open canvas, without Figma's panels and
    /// toolbars (see `CROP_JS`), for the preview to show
    #[serde(skip)]
    crop: Option<[f64; 4]>,
    /// Reloading behind its snapshot: the webview stays hidden until
    /// `READY_JS` says the file is drawn
    #[serde(skip)]
    revealing: bool,
    /// When the tab was last hidden. None while it's the active tab
    #[serde(skip)]
    hidden_since: Option<Instant>,
    /// Frozen, or freezing or resuming (Windows, macOS). Nothing may `eval`
    /// in the page meanwhile: on macOS WebKit would throw and abort Ligma
    #[serde(skip)]
    suspended: bool,
    /// Label of the window it's in
    #[serde(skip)]
    window: String,
}

#[derive(Default)]
struct TabState {
    /// Every window's tabs, each window's in the order its bar shows them
    tabs: Vec<Tab>,
    /// Each window's active tab, by window label
    actives: HashMap<String, u32>,
    /// Each window's tab that was active before its active one
    previous: HashMap<String, u32>,
    /// Each window's split view, if any: its tabs in the first and second
    /// half (see split.rs)
    splits: HashMap<String, [u32; 2]>,
    /// The share of the canvas each split view's first half takes, as last
    /// dragged. A half otherwise
    split_ratios: HashMap<String, f64>,
    /// The window last focused, where new tabs open and the tab shortcuts
    /// act
    focused: String,
    next_id: u32,
    /// Number of the next window tabs are moved into
    next_window: u32,
    /// Colors of the active tab's Figma theme, from `THEME_JS`
    theme: Option<serde_json::Value>,
    /// Hidden time after which a tab is put to sleep. None: never
    sleep_after: Option<Duration>,
    /// Hidden tabs that may stay loaded; older ones sleep. None: no limit
    max_hidden: Option<usize>,
    /// Windows showing a snapshot, whose tab bar's webview fills them
    snapshot_shown: Vec<String>,
    /// Show a waking tab's snapshot while it reloads (`reloadSnapshots`)
    snapshots: bool,
    /// Preview a hovered tab (`tabPreviews`), with its snapshot
    previews: bool,
    /// When the active tab's image was last taken for `capture_active`
    last_capture: Option<Instant>,
    /// Keep the tabs in `SESSION_FILE` for the next launch (`restoreTabs`)
    restore: bool,
    /// Windows may be split (`splitView`),
    split_view: bool,
    /// under one set of Figma's panels (`uniformSplit`)
    uniform_split: bool,
    /// Zoom of Figma tabs and Ligma's own pages (`interfaceScale`), 1 at 100%
    scale: f64,
    /// What was last written there, to skip writing it again
    saved_session: Option<String>,
}

/// Tabs saved for the next launch, in the app data folder
const SESSION_FILE: &str = "session.json";

/// The tabs as `SESSION_FILE` keeps them. Home comes first
#[derive(Deserialize, Serialize)]
struct Session {
    tabs: Vec<SavedTab>,
    /// Index of the active tab
    active: usize,
}

#[derive(Deserialize, Serialize)]
struct SavedTab {
    url: String,
    title: String,
    #[serde(default)]
    pinned: bool,
}

pub struct Tabs(Mutex<TabState>);

impl Tabs {
    pub fn new(settings: &crate::settings::Settings) -> Self {
        let mut state = TabState { focused: MAIN.to_owned(), next_window: 1, ..Default::default() };
        state.configure(settings);
        Self(Mutex::new(state))
    }
}

impl TabState {
    fn configure(&mut self, settings: &crate::settings::Settings) {
        let minutes = settings.sleep_tabs_after_minutes;
        // Sleeping turned off means every tab stays loaded, limit or not
        self.sleep_after = (minutes > 0).then(|| Duration::from_secs(minutes * 60));
        self.max_hidden = settings.max_hidden_tabs_loaded.filter(|_| minutes > 0);
        self.snapshots = settings.reload_snapshots;
        self.previews = settings.tab_previews;
        self.restore = settings.restore_tabs;
        self.split_view = settings.split_view;
        self.uniform_split = settings.uniform_split;
        self.scale = crate::settings::scale(settings);
        if !self.restore {
            self.saved_session = None;
        }
        if !self.snapshots && !self.previews {
            for tab in self.tabs.iter_mut().filter(|tab| !tab.revealing) {
                tab.snapshot = None;
            }
        }
    }

    /// The focused window's active tab
    fn active(&self) -> u32 {
        self.active_in(&self.focused)
    }

    fn active_in(&self, window: &str) -> u32 {
        self.actives.get(window).copied().unwrap_or(HOME_ID)
    }

    fn is_active(&self, tab: &Tab) -> bool {
        self.actives.get(&tab.window) == Some(&tab.id)
    }

    /// Window `window`'s tabs, in order
    fn tabs_in<'a>(&'a self, window: &'a str) -> impl Iterator<Item = &'a Tab> + 'a {
        self.tabs.iter().filter(move |tab| tab.window == window)
    }

    /// Puts `tab` at `index` among its window's tabs in its group: the
    /// pinned tabs, or the others. Home stays first
    fn insert(&mut self, tab: Tab, index: usize) {
        let same = |other: &Tab| other.window == tab.window && other.id != HOME_ID;
        let group: Vec<usize> = (0..self.tabs.len())
            .filter(|&i| same(&self.tabs[i]) && self.tabs[i].pinned == tab.pinned)
            .collect();
        let last = (0..self.tabs.len()).rev().find(|&i| self.tabs[i].window == tab.window);
        let at = match (group.get(index), group.last()) {
            (Some(&i), _) => i,
            (None, Some(&i)) => i + 1,
            // The first pinned tab goes before the others
            (None, None) if tab.pinned => (0..self.tabs.len())
                .find(|&i| same(&self.tabs[i]))
                .or(last.map(|i| i + 1))
                .unwrap_or(self.tabs.len()),
            (None, None) => last.map_or(self.tabs.len(), |i| i + 1),
        };
        self.tabs.insert(at, tab);
    }
}

/// Applies changed settings. Tabs now past a shorter timeout or a lower
/// limit go to sleep right away
pub fn apply_settings(app: &AppHandle, settings: &crate::settings::Settings) {
    let rescaled = {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        let scale = state.scale;
        state.configure(settings);
        state.scale != scale
    };
    if rescaled {
        apply_scale(app);
    }
    if !settings.tab_previews {
        crate::preview::hide(app);
    }
    if !settings.split_view {
        let windows: Vec<String> = app.state::<Tabs>().inner().0.lock().unwrap().splits.keys().cloned().collect();
        for window in windows {
            end_split(app, &window);
        }
    }
    if settings.restore_tabs {
        save_session(app);
    } else if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::remove_file(dir.join(SESSION_FILE));
    }
    rest_idle(app);
}

/// Opens the tabs saved by the last session, asleep so none loads until
/// it's selected, then selects the one that was active. Opens Home alone
/// if there are none, or `restoreTabs` is off
pub fn restore(app: &AppHandle, settings: &crate::settings::Settings) -> tauri::Result<()> {
    let session = settings
        .restore_tabs
        .then(|| app.path().app_data_dir().ok())
        .flatten()
        .and_then(|dir| std::fs::read_to_string(dir.join(SESSION_FILE)).ok())
        .and_then(|json| serde_json::from_str::<Session>(&json).ok())
        .filter(|session| !session.tabs.is_empty());
    let Some(session) = session else {
        return open(app, crate::new_home::home_url());
    };
    let active = {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        let mut active = HOME_ID;
        for (index, saved) in session.tabs.into_iter().enumerate() {
            let url = Url::parse(&saved.url).ok().filter(is_figma);
            let home = state.tabs.is_empty();
            // Home comes back as the home page the setting asks for, or
            // where it was in Figma's file browser
            let url = match url {
                Some(url) if home && (crate::new_home::is_url(&url) || crate::new_home::is_url(&crate::new_home::home_url())) => Some(crate::new_home::home_url()),
                url => url,
            };
            let Some(url) = url.or_else(|| home.then(crate::new_home::home_url)) else {
                continue;
            };
            let id = state.next_id;
            state.next_id += 1;
            if index == session.active {
                active = id;
            }
            state.tabs.push(Tab {
                id,
                title: saved.title,
                kind: if home { "home" } else { kind(&url) },
                asleep: true,
                loading: false,
                pinned: saved.pinned && !home,
                file_key: if home { None } else { file_key(&url) },
                url,
                hidden_since: None,
                suspended: false,
                snapshot: None,
                crop: None,
                revealing: false,
                window: MAIN.to_owned(),
            });
        }
        active
    };
    select(app, active)
}

/// Saves the session now, with each loaded tab where it is, for `main`'s
/// timer: a Figma tab keeps where you are in a file in its address (see
/// view_state.rs) without loading anything, which nothing else hears of
pub fn keep_session(app: &AppHandle) {
    let awake: Vec<u32> = app.state::<Tabs>().inner().0.lock().unwrap().tabs.iter().filter(|tab| !tab.asleep).map(|tab| tab.id).collect();
    for id in awake {
        let Some(url) = app.get_webview(&label(id)).and_then(|webview| webview.url().ok()).filter(is_figma) else {
            continue;
        };
        // Only the same file: opening another is a navigation, followed
        // as such
        with_tab(app, id, |tab| {
            if tab.file_key.is_some() && tab.file_key == file_key(&url) {
                tab.url = url;
            }
        });
    }
    save_session(app);
}

/// Writes the tabs to `SESSION_FILE` if they changed and `restoreTabs` is on
fn save_session(app: &AppHandle) {
    let json = {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        // No tabs yet: launching, and `restore` hasn't read the file
        if !state.restore || state.tabs.is_empty() {
            return;
        }
        // All in the main window next time, its own first
        let tabs = state.tabs_in(MAIN).chain(state.tabs.iter().filter(|tab| tab.window != MAIN));
        let session = Session {
            tabs: tabs
                .map(|tab| SavedTab {
                    url: tab.url.to_string(),
                    title: tab.title.clone(),
                    pinned: tab.pinned,
                })
                .collect(),
            active: state.tabs_in(MAIN).position(|tab| tab.id == state.active_in(MAIN)).unwrap_or(0),
        };
        let json = serde_json::to_string_pretty(&session).unwrap();
        if state.saved_session.as_ref() == Some(&json) {
            return;
        }
        state.saved_session = Some(json.clone());
        json
    };
    if let Ok(dir) = app.path().app_data_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(SESSION_FILE), json);
    }
}

/// Reads Figma's theme colors. Figma sets `data-preferred-theme` on <body>
/// and defines its palette as CSS variables there; FigJam and Dev Mode
/// override them, so this follows the active editor too. Also measures
/// the editor's left navigation rail, in CSS pixels, for Home to line up
/// with. The tab bar has the same zoom, so the width holds as is, and
/// follows a change of interface scale at once
const THEME_JS: &str = r#"(() => {
  const el = document.querySelector("[data-preferred-theme]");
  if (!el) return null;
  const style = getComputedStyle(el);
  const v = (name) => style.getPropertyValue(name).trim();
  const rail = document.querySelector('[data-testid="navigation-bar"]')?.getBoundingClientRect();
  return {
    railWidth: rail?.width && rail.left === 0 ? rail.right : null,
    bg: v("--color-bg"),
    // A file's loading screen, but FigJam's, which is white
    loading: el.matches('[data-editor-theme="whiteboard"]') ? null : v("--color-loading-progress-cover"),
    bgSecondary: v("--color-bg-secondary"),
    bgTertiary: v("--color-bg-tertiary"),
    text: v("--color-text"),
    textSecondary: v("--color-text-secondary"),
    textTertiary: v("--color-text-tertiary"),
    border: v("--color-border"),
    brand: v("--color-bg-brand"),
    menu: v("--color-bg-menu"),
    menuSelected: v("--color-bg-menu-selected"),
    menuText: v("--color-text-menu"),
    menuTextSecondary: v("--color-text-menu-secondary"),
    menuBorder: v("--color-border-menu"),
  };
})()"#;

fn label(id: u32) -> String {
    format!("tab-{id}")
}

pub fn is_figma(url: &Url) -> bool {
    url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| host == "figma.com" || host.ends_with(".figma.com"))
}

/// Content Figma serves from its own domains inside its pages, like the
/// `*.figma.site` previews of Make and Sites. On macOS `on_navigation` also
/// sees iframe loads, so these must stay in the page rather than go to the
/// browser
fn is_figma_content(url: &Url) -> bool {
    url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| host == "figma.site" || host.ends_with(".figma.site"))
}

/// Tab icon for a Figma URL, from the first path segment (`/design/<key>/...`)
fn kind(url: &Url) -> &'static str {
    match url.path_segments().and_then(|mut s| s.next()) {
        Some("design" | "file") => "design",
        Some("board") => "board",
        Some("slides" | "deck" | "presenter") => "slides",
        Some("proto") => "proto",
        Some("make") => "make",
        Some("site") => "site",
        Some("buzz") => "buzz",
        Some("spec") => "spec",
        _ => "page",
    }
}

/// Key of the Figma file an editor URL points at, if any
pub fn file_key(url: &Url) -> Option<String> {
    if !is_figma(url) || kind(url) == "page" {
        return None;
    }
    url.path_segments()?
        .nth(1)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
}

/// How a Figma URL shows its file: in its editor, or in one of the
/// presentations that share the file's key. Slides presents in two: the
/// presenter view and the audience's slideshow (`deck`)
fn view(url: &Url) -> &'static str {
    match url.path_segments().and_then(|mut s| s.next()) {
        Some("proto") => "proto",
        Some("deck") => "deck",
        Some("presenter") => "presenter",
        _ => kind(url),
    }
}

/// Opens `url` in a new tab in the focused window, or focuses the tab
/// already showing that file the same way, in whichever window, so
/// presenting a file again goes back to its presentation
pub fn open(app: &AppHandle, url: Url) -> tauri::Result<()> {
    let key = file_key(&url);
    let id = {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        let existing = key.as_ref().and_then(|key| {
            let tab = state.tabs.iter().find(|tab| {
                tab.file_key.as_ref() == Some(key) && view(&tab.url) == view(&url)
            })?;
            Some((tab.id, tab.window.clone()))
        });
        if let Some((id, window)) = existing {
            drop(state);
            if let Some(window) = app.get_window(&window) {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            return select(app, id);
        }
        // Not into a window that's going
        let window = if app.get_window(&state.focused).is_some() { state.focused.clone() } else { MAIN.to_owned() };
        let id = state.next_id;
        state.next_id += 1;
        state.tabs.push(Tab {
            id,
            title: String::new(),
            kind: if id == HOME_ID { "home" } else { kind(&url) },
            asleep: false,
            loading: false,
            pinned: false,
            file_key: key,
            url: url.clone(),
            hidden_since: None,
            suspended: false,
            snapshot: None,
            crop: None,
            revealing: false,
            window,
        });
        id
    };
    create_webview(app, id, url)?;
    select(app, id)
}

/// Loads tab `id`'s webview, for a new tab or one waking from sleep or
/// reopened from the last session
fn create_webview(app: &AppHandle, id: u32, url: Url) -> tauri::Result<()> {
    let window = with_tab(app, id, |tab| tab.window.clone())
        .and_then(|label| app.get_window(&label))
        .or_else(|| app.get_window(MAIN))
        .expect("main window exists");
    let (nav_app, popup_app, title_app) = (app.clone(), app.clone(), app.clone());
    #[allow(unused_mut)]
    let mut builder = WebviewBuilder::new(label(id), WebviewUrl::External(url))
        // Let Figma receive HTML5 drag-and-drop (image/file imports)
        .disable_drag_drop_handler()
        .on_navigation(move |url| on_navigation(&nav_app, id, url))
        .on_new_window(move |url, _| {
            on_new_window(&popup_app, id, url);
            NewWindowResponse::Deny
        })
        .on_download(crate::downloads::on_download)
        .on_document_title_changed(move |webview, title| {
            on_title_changed(&title_app, id, &webview, &title)
        })
        // In case a new page starts at the default zoom
        .on_page_load(move |webview, payload| {
            if let PageLoadEvent::Finished = payload.event() {
                let _ = webview.set_zoom(scale(webview.app_handle()));
                crate::plugins::push_to(&webview);
                crate::appearance::push_to(&webview);
                finish_loading(webview.app_handle(), id);
            }
        })
        .initialization_script(BEFOREUNLOAD_JS)
        .initialization_script(crate::auth::PAGE_JS)
        .initialization_script(NEW_WINDOW_JS)
        .initialization_script(MAIN_MENU_JS)
        .initialization_script(DESKTOP_APP_JS)
        .initialization_script(crate::plugins::page_js())
        .initialization_script(crate::appearance::page_js(app))
        .initialization_script(crate::loading_logo::PAGE_JS)
        .initialization_script(crate::modules::PAGE_JS)
        .initialization_script(crate::layers::page_js())
        .initialization_script(crate::frame_colors::page_js())
        .initialization_script(crate::home::page_js())
        .initialization_script(crate::new_home::SETTINGS_JS)
        .initialization_script(crate::toolbar::page_js())
        .initialization_script(crate::adjustments::page_js())
        .initialization_script(crate::ai_actions::page_js())
        .initialization_script(crate::copy_code::page_js())
        .initialization_script(crate::lasso::page_js())
        .initialization_script(crate::page_avatars::PAGE_JS)
        .initialization_script(crate::mcp_bridge::PANEL_JS)
        .initialization_script(crate::split::PAGE_JS)
        .initialization_script(crate::split::CARRY_JS)
        .initialization_script(crate::view_state::PAGE_JS);
    // Ligma's home page, which Home may load whatever the setting was when
    // it was made
    if id == HOME_ID {
        builder = builder.initialization_script(crate::new_home::page_js());
    }
    #[cfg(target_os = "macos")]
    {
        builder = builder
            .user_agent(USER_AGENT)
            .initialization_script(NEW_TAB_JS)
            .initialization_script(TEXT_UNDO_JS);
    }
    #[cfg(windows)]
    {
        builder = builder
            .initialization_script(LOCAL_NETWORK_ACCESS_JS)
            .initialization_script(TITLE_EMOJI_JS);
    }
    let background = page_background(app, with_tab(app, id, |tab| tab.kind).unwrap_or("page"));
    let webview = window.add_child(
        builder.background_color(background),
        LogicalPosition::new(0.0, bar_height(app)),
        content_size(app, &window)?,
    )?;
    #[cfg(target_os = "macos")]
    crate::wkwebview::set_background(&webview, background);
    webview.set_zoom(scale(app))?;
    #[cfg(target_os = "macos")]
    crate::wkwebview::pass_undo_to_pages(&webview);
    #[cfg(windows)]
    {
        crate::webview2::drop_request_filters(&webview);
        crate::webview2::add_tab_shortcuts(&webview);
        crate::webview2::allow_downloads(&webview);
    }
    Ok(())
}

/// `open` from a webview callback. Those run on the main thread, and
/// creating a webview there would block on itself
fn open_later(app: &AppHandle, url: Url) {
    let app = app.clone();
    std::thread::spawn(move || {
        let _ = open(&app, url);
    });
}

fn open_in_browser(app: &AppHandle, url: &Url) {
    let _ = app.opener().open_url(url.as_str(), None::<&str>);
}

/// Points Home at `url` (see new_home.rs), loading it now if Home is awake
pub fn set_home_url(app: &AppHandle, url: Url) {
    {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        if let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == HOME_ID) {
            tab.url = url.clone();
        }
    }
    if let Some(webview) = app.get_webview(&label(HOME_ID)) {
        let _ = webview.navigate(url);
    }
}

fn on_navigation(app: &AppHandle, id: u32, url: &Url) -> bool {
    #[cfg(target_os = "macos")]
    if url.host_str() == Some(NEW_TAB_HOST) {
        let target = url.query_pairs().find(|(key, _)| key == "url");
        if let Some(Ok(target)) = target.map(|(_, target)| Url::parse(&target)) {
            if is_figma(&target) {
                open_later(app, target);
            }
        }
        return false;
    }
    if url.host_str() == Some(crate::auth::SIGNAL_HOST) {
        crate::auth::on_signal(app, id, url);
        return false;
    }
    if url.host_str() == Some(crate::plugins::SIGNAL_HOST) {
        crate::plugins::on_signal(app, id, url);
        return false;
    }
    if url.host_str() == Some(crate::toolbar::SIGNAL_HOST) {
        crate::toolbar::on_signal(app, id, url);
        return false;
    }
    if url.host_str() == Some(crate::adjustments::SIGNAL_HOST) {
        crate::adjustments::on_signal(app, id, url);
        return false;
    }
    if url.host_str() == Some(crate::mcp_bridge::SIGNAL_HOST) {
        crate::mcp_bridge::on_signal(app, url);
        return false;
    }
    if url.host_str() == Some(crate::new_home::SIGNAL_HOST) {
        crate::new_home::on_signal(app, id, url);
        return false;
    }
    if crate::auth::is_google_sign_in(url) {
        sign_in_later(app, id);
        return false;
    }
    match url.scheme() {
        "http" | "https" => {}
        // Not to the OS, which hands them to the Figma desktop app
        "figma" => {
            figma_link_later(app, url.clone());
            return false;
        }
        // blob:, data:, about: and friends stay in the page
        _ => return true,
    }
    if is_figma_content(url) {
        return true;
    }
    if !is_figma(url) {
        open_in_browser(app, url);
        return false;
    }
    // Like the desktop app, files opened from Home get their own tab
    if id == HOME_ID && file_key(url).is_some() {
        open_later(app, url.clone());
        return false;
    }
    true
}

/// Has tab `id` start Google sign-in in the browser (see auth.rs). Not from
/// within the webview's own callback, which is still deciding on the page
fn sign_in_later(app: &AppHandle, id: u32) {
    let app = app.clone();
    std::thread::spawn(move || {
        if let Some(webview) = app.get_webview(&label(id)) {
            crate::auth::start(&webview);
        }
    });
}

/// Opens a `figma://` link the way Ligma opens one from the OS (see auth.rs)
fn figma_link_later(app: &AppHandle, url: Url) {
    let app = app.clone();
    std::thread::spawn(move || crate::auth::on_figma_link(&app, &url));
}

fn on_new_window(app: &AppHandle, id: u32, url: Url) {
    if url.host_str() == Some(crate::plugins::SIGNAL_HOST) {
        crate::plugins::on_signal(app, id, &url);
    } else if url.host_str() == Some(crate::toolbar::SIGNAL_HOST) {
        crate::toolbar::on_signal(app, id, &url);
    } else if url.host_str() == Some(crate::adjustments::SIGNAL_HOST) {
        crate::adjustments::on_signal(app, id, &url);
    } else if url.host_str() == Some(crate::mcp_bridge::SIGNAL_HOST) {
        crate::mcp_bridge::on_signal(app, &url);
    } else if url.host_str() == Some(crate::new_home::SIGNAL_HOST) {
        crate::new_home::on_signal(app, id, &url);
    } else if crate::auth::is_google_sign_in(&url) {
        sign_in_later(app, id);
    } else if is_figma(&url) {
        open_later(app, url);
    } else if url.scheme() == "figma" {
        figma_link_later(app, url);
    } else if matches!(url.scheme(), "http" | "https") {
        open_in_browser(app, &url);
    }
}

fn on_title_changed(app: &AppHandle, id: u32, webview: &Webview, title: &str) {
    let url = webview.url().ok();
    // Figma can switch pages client-side without a navigation, so a file
    // can still end up in Home. Move it to its own tab
    if id == HOME_ID {
        if let Some(url) = url.as_ref().filter(|url| file_key(url).is_some()) {
            let _ = webview.navigate(crate::new_home::home_url());
            open_later(app, url.clone());
            return;
        }
    }
    {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        let title = tab_title(title, url.as_ref().map_or("page", kind));
        // A page that's loading is titled "Figma" or the like before its
        // own title comes. Keep the name it had meanwhile
        let same_page = url.as_ref().is_none_or(|url| match &tab.file_key {
            Some(key) => file_key(url).as_ref() == Some(key),
            None => url.path() == tab.url.path(),
        });
        if !(same_page && is_loading_title(&title) && !is_loading_title(&tab.title)) {
            tab.title = title;
        }
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

/// Whether `title` is one a page shows while it loads rather than its own:
/// none, a bare product name or "Loading…"
fn is_loading_title(title: &str) -> bool {
    let title = title.trim().trim_end_matches(['.', '…']);
    title.is_empty() || title.eq_ignore_ascii_case("loading") || PRODUCTS.contains(&title)
}

/// Products Figma ends page titles with ("<name> – FigJam")
const PRODUCTS: [&str; 7] =
    ["Figma", "FigJam", "Figma Slides", "Figma Make", "Figma Buzz", "Figma Sites", "Figma Specs"];

/// A tab's title from its page's: without Figma's product suffix, or the
/// "▶" a prototype's title starts with, which its tab's icon already shows
fn tab_title(title: &str, kind: &str) -> String {
    // An en dash usually; a hyphen in Slides' presentation titles
    let title = PRODUCTS
        .iter()
        .flat_map(|product| [" – ", " - "].map(move |dash| (dash, product)))
        .find_map(|(dash, product)| title.strip_suffix(product)?.strip_suffix(dash))
        .unwrap_or(title);
    let title = if kind == "proto" {
        title.trim_start_matches(['▶', '\u{fe0f}']).trim_start()
    } else {
        title
    };
    title.to_owned()
}

/// Makes tab `id` the active one of its window, and that window the
/// focused one
pub fn select(app: &AppHandle, id: u32) -> tauri::Result<()> {
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
    let (window, previous, wake, resume, snapshot, split, unsplit) = {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        // Previews keep snapshots too; only `reloadSnapshots` shows them here
        let snapshots = state.snapshots;
        let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == id) else {
            return Ok(());
        };
        tab.hidden_since = None;
        // On macOS resuming takes a round trip; `suspended` stays set until
        // the page runs again
        let resume = if cfg!(target_os = "macos") {
            tab.suspended
        } else {
            std::mem::take(&mut tab.suspended)
        };
        let wake = std::mem::take(&mut tab.asleep).then(|| tab.url.clone());
        tab.loading |= wake.is_some();
        // Waking with a snapshot: show it while the page loads
        tab.revealing |= snapshots && wake.is_some() && tab.snapshot.is_some() && tab.file_key.is_some();
        // Kept otherwise, for the tab's preview once it's hidden again
        let snapshot = tab.snapshot.clone().filter(|_| tab.revealing);
        let window = tab.window.clone();
        // Only a tab still in this window: one moved out carries on there
        let previous = state.actives.insert(window.clone(), id).filter(|&previous| {
            previous != id && state.tabs.iter().any(|tab| tab.id == previous && tab.window == window)
        });
        if let Some(previous) = previous {
            state.previous.insert(window.clone(), previous);
        }
        // Within a split, the tab it leaves stays shown in its half. Any
        // other tab ends the split, and both its tabs go
        let (split, unsplit) = match state.splits.get(&window).copied() {
            Some(pair) if pair.contains(&id) => (Some(pair), None),
            Some(pair) => {
                state.splits.remove(&window);
                state.split_ratios.remove(&window);
                (None, Some(pair))
            }
            None => (None, None),
        };
        let previous = previous.filter(|previous| !split.is_some_and(|pair| pair.contains(previous)));
        let hiding: Vec<u32> = previous
            .into_iter()
            .chain(unsplit.into_iter().flatten().filter(|&tab| tab != id && Some(tab) != previous))
            .collect();
        for tab in state.tabs.iter_mut().filter(|tab| hiding.contains(&tab.id)) {
            tab.hidden_since = Some(Instant::now());
        }
        state.focused = window.clone();
        (window, hiding, wake, resume, snapshot, split, unsplit)
    };
    if let Some(pair) = unsplit {
        for tab in pair {
            if let Some(webview) = app.get_webview(&label(tab)) {
                crate::split::unclip(tab, &webview);
            }
        }
        crate::split::hide_divider(app, &window);
        fit_tab_bar(app, &window);
    }
    show_snapshot(app, &window, snapshot.as_deref());
    if let Some(url) = wake {
        // Fails if the old webview is still closing. Stay asleep, so
        // selecting the tab again retries
        if create_webview(app, id, url).is_err() {
            with_tab(app, id, |tab| {
                tab.asleep = true;
                tab.loading = false;
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
    for &previous in &previous {
        if let Some(webview) = app.get_webview(&label(previous)) {
            // WebView2 can't be relied on to capture a webview it's hiding;
            // on Windows the tab bar has `capture_active` take it beforehand
            #[cfg(target_os = "macos")]
            capture(app, previous, &webview);
            webview.hide()?;
            #[cfg(windows)]
            crate::webview2::on_hidden(&webview);
        }
    }
    // A tab past `max_hidden` sleeps now, not at the next timer tick
    if !previous.is_empty() {
        rest_idle(app);
    }
    if split.is_some() {
        layout_split(app, &window);
    }
    push_state(app);
    Ok(())
}

/// Splits window `window` between tabs `first` and `second`, in that
/// order, and focuses `focus`, one of them. Both must be file tabs in it,
/// and neither Home
fn split_pair(app: &AppHandle, window: &str, first: u32, second: u32, focus: u32) -> tauri::Result<()> {
    {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        let fits = |id: u32| state.tabs_in(window).any(|tab| tab.id == id && tab.file_key.is_some());
        if !state.split_view || first == second || first == HOME_ID || second == HOME_ID || !fits(first) || !fits(second) {
            return Ok(());
        }
    }
    end_split(app, window);
    app.state::<Tabs>().inner().0.lock().unwrap().splits.insert(window.to_owned(), [first, second]);
    fit_tab_bar(app, window);
    // Each selected in turn, so both are awake and shown
    select(app, if focus == first { second } else { first })?;
    select(app, focus)
}

/// Ends window `window`'s split view, if it has one: its active tab fills
/// the canvas again and the other hides
fn end_split(app: &AppHandle, window: &str) {
    let (pair, other) = {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        let Some(pair) = state.splits.remove(window) else {
            return;
        };
        state.split_ratios.remove(window);
        let active = state.active_in(window);
        let other = pair.into_iter().find(|&tab| tab != active);
        if let Some(tab) = state.tabs.iter_mut().find(|tab| Some(tab.id) == other) {
            tab.hidden_since = Some(Instant::now());
        }
        (pair, other)
    };
    for tab in pair {
        if let Some(webview) = app.get_webview(&label(tab)) {
            crate::split::unclip(tab, &webview);
            if Some(tab) == other {
                let _ = webview.hide();
                #[cfg(windows)]
                crate::webview2::on_hidden(&webview);
            }
        }
    }
    crate::split::hide_divider(app, window);
    fit_tab_bar(app, window);
    push_state(app);
}

/// The tabs in split views, in every window
pub fn split_tabs(app: &AppHandle) -> Vec<u32> {
    let state = app.state::<Tabs>().inner().0.lock().unwrap();
    state.splits.values().flatten().copied().collect()
}

/// The other tab of the split view tab `id` is in, if any
pub fn split_partner(app: &AppHandle, id: u32) -> Option<u32> {
    let state = app.state::<Tabs>().inner().0.lock().unwrap();
    let window = &state.tabs.iter().find(|tab| tab.id == id)?.window;
    state.splits.get(window)?.iter().copied().find(|&tab| tab != id)
}

/// Ends the split views tabs `ids` are in
fn end_splits_of(app: &AppHandle, ids: &[u32]) {
    let windows: Vec<String> = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        let split = |(_, pair): &(&String, &[u32; 2])| pair.iter().any(|tab| ids.contains(tab));
        state.splits.iter().filter(split).map(|(window, _)| window.clone()).collect()
    };
    for window in windows {
        end_split(app, &window);
    }
}

/// Clips window `window`'s split tabs to their halves, for its focused one
fn layout_split(app: &AppHandle, window: &str) {
    let found = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        state.splits.get(window).copied().and_then(|pair| {
            let active = state.active_in(window);
            let other = pair.into_iter().find(|&tab| tab != active).filter(|_| pair.contains(&active))?;
            let ratio = state.split_ratios.get(window).copied().unwrap_or(0.5);
            Some((active, other, pair[0] == other, ratio, state.uniform_split))
        })
    };
    if let Some((focused, other, other_first, ratio, uniform)) = found {
        crate::split::layout(app, focused, other, other_first, ratio, uniform);
    }
}

/// Lays out every split view again, for Figma's toolbar, menus and panels
/// changing. Call every `split::LAYOUT_EVERY`
pub fn refresh_splits(app: &AppHandle) {
    let windows: Vec<String> = app.state::<Tabs>().inner().0.lock().unwrap().splits.keys().cloned().collect();
    for window in windows {
        layout_split(app, &window);
    }
}

/// Shows `snapshot` where tab webviews go in window `window`, or hides it
fn show_snapshot(app: &AppHandle, window: &str, snapshot: Option<&str>) {
    let Some(bar) = app.get_webview(window) else {
        return;
    };
    {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        state.snapshot_shown.retain(|shown| shown != window);
        if snapshot.is_some() {
            state.snapshot_shown.push(window.to_owned());
        }
    }
    let _ = match snapshot {
        Some(url) => bar.eval(format!("window.showSnapshot?.({})", serde_json::to_string(url).unwrap())),
        None => bar.eval("window.hideSnapshot?.()"),
    };
    fit_tab_bar(app, window);
}

/// Sizes the tab bar's webview to the bar, or to the whole window while it
/// shows a snapshot. Kept off the area under the bar otherwise: tab webviews
/// lie on top of it there, and WebKit would never see the mouse leave the
/// bar, leaving the last hovered tab highlighted
fn fit_tab_bar(app: &AppHandle, label: &str) {
    let (Some(window), Some(bar)) = (app.get_window(label), app.get_webview(label)) else {
        return;
    };
    let (Ok(size), Ok(scale)) = (window.inner_size(), window.scale_factor()) else {
        return;
    };
    let size = size.to_logical::<f64>(scale);
    let full = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        // Split, it shows between the halves (see split.rs)
        state.snapshot_shown.iter().any(|shown| shown == label) || state.splits.contains_key(label)
    };
    let height = if full { size.height } else { bar_height(app).min(size.height) };
    let _ = bar.set_size(LogicalSize::new(size.width, height));
}

/// Keeps an image of tab `id` for when it wakes from sleep (file tabs) and
/// for its preview. Skipped for Home, while frozen (WebKit can't) or
/// reloading (the old image is better)
fn capture(app: &AppHandle, id: u32, webview: &Webview) {
    let wanted = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        let wanted = |tab: &Tab| {
            let kept = state.previews || (state.snapshots && tab.file_key.is_some());
            kept && tab.id != HOME_ID && !tab.asleep && !tab.suspended && !tab.revealing
        };
        state.tabs.iter().find(|tab| tab.id == id).is_some_and(wanted)
    };
    if !wanted {
        return;
    }
    // Measured alongside, not first: on macOS the image must be taken
    // before the webview is hidden
    let crop_app = app.clone();
    let _ = webview.eval_with_callback(CROP_JS, move |json| {
        let crop = serde_json::from_str::<Option<[f64; 4]>>(&json).ok().flatten();
        with_tab(&crop_app, id, |tab| tab.crop = crop);
    });
    let app = app.clone();
    let keep = move |url: String| {
        with_tab(&app, id, |tab| tab.snapshot = Some(url));
    };
    #[cfg(target_os = "macos")]
    crate::wkwebview::snapshot(webview, move |url| {
        if let Some(url) = url {
            keep(url);
        }
    });
    #[cfg(windows)]
    crate::webview2::capture(webview, keep);
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = (webview, keep);
}

/// Finds the open canvas in a Figma editor, as `[left, top, width, height]`
/// fractions of the page, or null outside one. That's the largest
/// `<canvas>`, less whatever Figma lays over its edges, like the floating
/// tool bar: scanning a quarter of the way in from each side through the
/// middle, the crop goes a little inside the innermost point where
/// something other than the canvas (or a layer spanning it, like Figma's
/// input layer) is on top. The tool bar floats clear of the bottom edge,
/// so the scan can't stop at the first open point
const CROP_JS: &str = r#"(() => {
  const area = (r) => r.width * r.height;
  const canvases = [...document.querySelectorAll("canvas")].map((c) => [c, c.getBoundingClientRect()]);
  const [canvas, r] = canvases.reduce((a, b) => (area(b[1]) > area(a[1]) ? b : a), [null, { width: 0, height: 0 }]);
  const W = innerWidth, H = innerHeight;
  if (!canvas || area(r) < W * H * 0.2) return null;
  const left = Math.max(0, r.left), top = Math.max(0, r.top);
  const right = Math.min(W, r.right), bottom = Math.min(H, r.bottom);
  const open = (x, y) => {
    const el = document.elementFromPoint(x, y);
    if (!el) return false;
    if (el === canvas || el.contains(canvas)) return true;
    const e = el.getBoundingClientRect();
    return e.width >= (right - left) * 0.9 && e.height >= (bottom - top) * 0.9;
  };
  const cx = (left + right) / 2, cy = (top + bottom) / 2;
  const edge = (from, depth, at) => {
    const step = Math.sign(depth) * 4;
    let inner = from;
    for (let v = from; Math.abs(v - from) <= Math.abs(depth); v += step) if (!at(v)) inner = v + step * 3;
    return inner;
  };
  const x0 = edge(left, (right - left) / 4, (x) => open(x, cy));
  const x1 = edge(right - 1, -(right - left) / 4, (x) => open(x, cy));
  const y0 = edge(top, (bottom - top) / 4, (y) => open(cx, y));
  const y1 = edge(bottom - 1, -(bottom - top) / 4, (y) => open(cx, y));
  return [x0 / W, y0 / H, (x1 - x0) / W, (y1 - y0) / H];
})()"#;

/// Least time between two `capture_active` images
const CAPTURE_EVERY: Duration = Duration::from_secs(1);

/// Takes an image of the active tab for its preview, at most every
/// `CAPTURE_EVERY`
pub fn capture_active(app: &AppHandle) {
    let active = {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        if !state.previews || state.last_capture.is_some_and(|at| at.elapsed() < CAPTURE_EVERY) {
            return;
        }
        state.last_capture = Some(Instant::now());
        state.active()
    };
    if let Some(webview) = app.get_webview(&label(active)) {
        capture(app, active, &webview);
    }
}

/// True if tab `id` may be previewed: `tabPreviews` is on, and it's open
/// and neither Home nor its window's active tab
pub fn can_preview(app: &AppHandle, id: u32) -> bool {
    let state = app.state::<Tabs>().inner().0.lock().unwrap();
    let split = |tab: &Tab| state.splits.get(&tab.window).is_some_and(|pair| pair.contains(&tab.id));
    state.previews && id != HOME_ID && state.tabs.iter().any(|tab| tab.id == id && !state.is_active(tab) && !split(tab))
}

/// What the preview of a tab shows
#[derive(Serialize)]
pub struct Preview {
    title: String,
    kind: &'static str,
    asleep: bool,
    /// Its last image (see `capture`), as a `data:` URL,
    image: Option<String>,
    /// and the part of it to show (see `CROP_JS`)
    crop: Option<[f64; 4]>,
}

/// The preview of tab `id`, if it may be previewed (see `can_preview`)
pub fn preview(app: &AppHandle, id: u32) -> Option<Preview> {
    if !can_preview(app, id) {
        return None;
    }
    with_tab(app, id, |tab| Preview {
        title: tab.title.clone(),
        kind: tab.kind,
        asleep: tab.asleep,
        image: tab.snapshot.clone(),
        crop: tab.crop,
    })
}

/// Ends loading tab `id`'s spinner once its page has loaded: a file's once
/// `READY_JS` says it's drawn, or after `REVEAL_TIMEOUT`
fn finish_loading(app: &AppHandle, id: u32) {
    match with_tab(app, id, |tab| (tab.loading, tab.file_key.is_some())) {
        Some((true, false)) => {
            with_tab(app, id, |tab| tab.loading = false);
            push_state(app);
        }
        Some((true, true)) => {
            let app = app.clone();
            std::thread::spawn(move || {
                let started = Instant::now();
                loop {
                    std::thread::sleep(Duration::from_millis(250));
                    // Not while it's frozen: see `suspended`
                    match with_tab(&app, id, |tab| (tab.loading, tab.suspended)) {
                        Some((true, false)) => {}
                        Some((true, true)) if started.elapsed() < REVEAL_TIMEOUT => continue,
                        Some((true, true)) => break,
                        _ => return,
                    }
                    let Some(webview) = app.get_webview(&label(id)) else {
                        return;
                    };
                    let (tx, rx) = std::sync::mpsc::channel();
                    let _ = webview.eval_with_callback(READY_JS, move |json| {
                        let _ = tx.send(json == "true");
                    });
                    let ready = rx.recv_timeout(Duration::from_secs(1)).unwrap_or(false);
                    if ready || started.elapsed() >= REVEAL_TIMEOUT {
                        break;
                    }
                }
                if with_tab(&app, id, |tab| std::mem::take(&mut tab.loading)) == Some(true) {
                    push_state(&app);
                }
            });
        }
        _ => {}
    }
}

/// Shows reloading tab `id` once `READY_JS` has said the file is drawn
/// twice in a row, or after `REVEAL_TIMEOUT`
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
        let window = {
            let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
            let Some(tab) = state.tabs.iter_mut().find(|tab| tab.id == id) else {
                return;
            };
            tab.revealing = false;
            let window = tab.window.clone();
            state.actives.get(&window).is_some_and(|&active| active == id).then_some(window)
        };
        if let Some(window) = window {
            if let Some(webview) = app.get_webview(&label(id)) {
                let _ = webview.show();
                let _ = webview.set_focus();
            }
            show_snapshot(&app, &window, None);
        }
    });
}

/// Closes a tab. Home can't be closed
pub fn close(app: &AppHandle, id: u32) -> tauri::Result<()> {
    close_tabs(app, &[id])
}

/// Closes tabs `ids`, except Home. If a window's active tab is among them,
/// the nearest open tab to its right is selected, else the last one. A
/// window left without tabs closes
fn close_tabs(app: &AppHandle, ids: &[u32]) -> tauri::Result<()> {
    let ids: Vec<u32> = ids.iter().copied().filter(|&id| id != HOME_ID).collect();
    end_splits_of(app, &ids);
    let (next, emptied) = {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        let closing = |tab: &Tab| ids.contains(&tab.id);
        let (mut next, mut emptied) = (Vec::new(), Vec::new());
        for (window, &active) in &state.actives {
            let tabs: Vec<&Tab> = state.tabs_in(window).collect();
            let Some(index) = tabs.iter().position(|tab| tab.id == active && closing(tab)) else {
                continue;
            };
            // Prefer the tab to the right, like the desktop app. Home is
            // always left and never closed, so the main window keeps one
            let right = tabs[index..].iter().find(|tab| !closing(tab));
            match right.or_else(|| tabs.iter().rev().find(|tab| !closing(tab))) {
                Some(tab) => next.push(tab.id),
                None => emptied.push(window.clone()),
            }
        }
        state.tabs.retain(|tab| !closing(tab));
        for window in &emptied {
            state.actives.remove(window);
        }
        (next, emptied)
    };
    for next in next {
        select(app, next)?;
    }
    for id in ids {
        if let Some(webview) = app.get_webview(&label(id)) {
            webview.close()?;
        }
    }
    for window in emptied.iter().filter(|window| *window != MAIN) {
        if let Some(window) = app.get_window(window) {
            window.destroy()?;
        }
    }
    push_state(app);
    Ok(())
}

/// Pins or unpins tab `id`. Pinned tabs sit left, after Home, in the order
/// they were pinned; an unpinned tab goes right after them
fn set_pinned(app: &AppHandle, id: u32, pinned: bool) {
    {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        let Some(index) = state.tabs.iter().position(|tab| tab.id == id && id != HOME_ID) else {
            return;
        };
        let mut tab = state.tabs.remove(index);
        tab.pinned = pinned;
        let index = if pinned { usize::MAX } else { 0 };
        state.insert(tab, index);
    }
    push_state(app);
}

/// Moves tab `id` to `index` among its window's tabs in its group: the
/// pinned tabs, or the others. Home stays first
fn move_tab(app: &AppHandle, id: u32, index: usize) {
    {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        if let Some(from) = state.tabs.iter().position(|tab| tab.id == id && id != HOME_ID) {
            let tab = state.tabs.remove(from);
            state.insert(tab, index);
        }
    }
    push_state(app);
}

/// Moves tab `id` into window `target`, at `index` among its group there
/// (see `move_tab`), and selects it. Its webview is reparented, so the page
/// carries on. The window it leaves selects its neighbor, or with `back`
/// the tab active before it, or closes if that was its last tab
fn move_to_window(app: &AppHandle, id: u32, target: &str, index: usize, back: bool) -> tauri::Result<()> {
    let Some(target_window) = app.get_window(target) else {
        return Ok(());
    };
    end_splits_of(app, &[id]);
    let (source, next) = {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        let Some(from) = state.tabs.iter().position(|tab| tab.id == id && id != HOME_ID) else {
            return Ok(());
        };
        let source = state.tabs[from].window.clone();
        if source == target {
            drop(state);
            move_tab(app, id, index);
            return Ok(());
        }
        let next = (state.actives.get(&source) == Some(&id)).then(|| {
            let tabs: Vec<&Tab> = state.tabs_in(&source).collect();
            let before = state.previous.get(&source).filter(|&&before| before != id);
            let before = before.filter(|&&before| back && tabs.iter().any(|tab| tab.id == before));
            // Else the neighbor to the right, like closing it would select
            let at = tabs.iter().position(|tab| tab.id == id).unwrap_or(0);
            let neighbor = || tabs.get(at + 1).or_else(|| at.checked_sub(1).and_then(|at| tabs.get(at))).map(|tab| tab.id);
            before.copied().or_else(neighbor)
        });
        if next.is_some() {
            state.actives.remove(&source);
        }
        let mut tab = state.tabs.remove(from);
        tab.window = target.to_owned();
        state.insert(tab, index);
        (source, next)
    };
    if let Some(webview) = app.get_webview(&label(id)) {
        // Hidden while it moves, so `select` shows it afresh in its new
        // window, rather than a stale image of it
        webview.hide()?;
        // On the main thread: elsewhere `reparent` holds the webview's lock
        // while it waits for the main thread, which hangs Ligma if the main
        // thread needs the webview meanwhile (a title change, a move)
        let (tx, rx) = std::sync::mpsc::channel();
        let (moving, to) = (webview.clone(), target_window.clone());
        app.run_on_main_thread(move || {
            let _ = tx.send(moving.reparent(&to));
        })?;
        rx.recv().map_err(|_| tauri::Error::FailedToReceiveMessage)??;
        webview.set_bounds(tab_bounds(app, &target_window)?)?;
        #[cfg(windows)]
        crate::webview2::on_moved(&webview);
    }
    match next {
        Some(Some(next)) => select(app, next)?,
        Some(None) if source != MAIN => {
            if let Some(window) = app.get_window(&source) {
                window.destroy()?;
            }
        }
        _ => {}
    }
    select(app, id)?;
    // A new window is shown once it's ready, by `move_to_new_window`
    if target_window.is_visible()? {
        let _ = target_window.unminimize();
        let _ = target_window.set_focus();
    }
    push_state(app);
    Ok(())
}

/// Where a window's first tab is held when it's dragged, in CSS pixels
/// from the window's top left: past the traffic lights on macOS
const GRIP: (f64, f64) = (if cfg!(target_os = "macos") { 118.0 } else { 40.0 }, TAB_BAR_HEIGHT / 2.0);

/// Where a window goes to have its first tab under the cursor at `at`, in
/// screen pixels
fn grip_position(app: &AppHandle, window: &Window, at: PhysicalPosition<f64>) -> PhysicalPosition<i32> {
    let monitor = app.monitor_from_point(at.x, at.y).ok().flatten();
    let factor = monitor.map_or_else(|| window.scale_factor().unwrap_or(1.0), |monitor| monitor.scale_factor());
    let px = factor * scale(app);
    PhysicalPosition::new(at.x - GRIP.0 * px, at.y - GRIP.1 * px).cast::<i32>()
}

/// Moves tab `id` into a new window, with its first tab under the cursor at
/// `at`, in screen pixels. A window's only tab stays, and the window moves
/// there instead. With `back`, the window it leaves goes back to the tab
/// active before it (see `move_to_window`). Gives the label of the window
/// the tab is now in
fn move_to_new_window(
    app: &AppHandle,
    id: u32,
    at: PhysicalPosition<f64>,
    back: bool,
) -> tauri::Result<Option<String>> {
    let Some((source, alone)) = ({
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        state.tabs.iter().find(|tab| tab.id == id && id != HOME_ID).map(|tab| {
            (tab.window.clone(), state.tabs_in(&tab.window).count() == 1)
        })
    }) else {
        return Ok(None);
    };
    let Some(source_window) = app.get_window(&source) else {
        return Ok(None);
    };
    let position = grip_position(app, &source_window, at);
    // Never the main window, which keeps Home
    if alone {
        source_window.set_position(position)?;
        source_window.set_focus()?;
        return Ok(Some(source));
    }
    // As large as the window it leaves, unless that fills the screen
    let size = if source_window.is_maximized()? || source_window.is_fullscreen()? {
        LogicalSize::new(1200.0, 800.0)
    } else {
        source_window.inner_size()?.to_logical::<f64>(source_window.scale_factor()?)
    };
    let label = {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        state.next_window += 1;
        format!("{WINDOW_PREFIX}{}", state.next_window - 1)
    };
    // Shown only once it's in place with its tab, so it doesn't come
    // together on screen
    let window = build_window(app, &label, size, false)?;
    window.set_position(position)?;
    move_to_window(app, id, &label, 0, back)?;
    window.show()?;
    window.set_focus()?;
    // Fitted again to the size it has now it's shown
    layout(app);
    Ok(Some(label))
}

/// True while the main mouse button is held down, anywhere
#[cfg(windows)]
pub fn mouse_down() -> bool {
    crate::webview2::left_button_down()
}

#[cfg(target_os = "macos")]
pub fn mouse_down() -> bool {
    use objc2::{class, msg_send};
    let buttons: usize = unsafe { msg_send![class!(NSEvent), pressedMouseButtons] };
    buttons & 1 != 0
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn mouse_down() -> bool {
    false
}

/// A window's tab bar, as `follow_cursor` finds it under the cursor
struct Bar {
    label: String,
    origin: PhysicalPosition<i32>,
    width: f64,
    /// Screen pixels per CSS pixel in the bar
    px: f64,
}

impl Bar {
    /// Every visible window's tab bar but window `except`'s
    fn all(app: &AppHandle, except: &str) -> Vec<Bar> {
        let bar = |window: Window| {
            if window.label() == except || !window.is_visible().ok()? || window.is_minimized().ok()? {
                return None;
            }
            Some(Bar {
                label: window.label().to_owned(),
                origin: window.inner_position().ok()?,
                width: window.inner_size().ok()?.width as f64,
                px: window.scale_factor().ok()? * scale(app),
            })
        };
        tab_windows(app).into_iter().filter_map(bar).collect()
    }

    /// The CSS pixel in the bar `at` is on, in screen pixels, if it's on it
    fn x_at(&self, at: PhysicalPosition<f64>) -> Option<f64> {
        let (x, y) = (at.x - self.origin.x as f64, at.y - self.origin.y as f64);
        (x >= 0.0 && x < self.width && y >= 0.0 && y < TAB_BAR_HEIGHT * self.px).then(|| x / self.px)
    }
}

/// A half of a window's canvas, where a dragged file tab splits it with
/// the window's active tab (see split.rs)
struct Zone {
    window: String,
    partner: u32,
    /// In screen pixels
    rect: crate::split::Rect,
    /// The dragged tab goes in the first half, not the second
    first: bool,
}

impl Zone {
    /// Every visible window's zone but window `except`'s, for dragged tab
    /// `id`. None if it's no file
    fn all(app: &AppHandle, except: &str, id: u32) -> Vec<Zone> {
        let (split_view, uniform) = {
            let state = app.state::<Tabs>().inner().0.lock().unwrap();
            (state.split_view, state.uniform_split)
        };
        if !split_view || with_tab(app, id, |tab| tab.file_key.is_none()).unwrap_or(true) {
            return Vec::new();
        }
        let zones = |window: Window| {
            if window.label() == except || !window.is_visible().ok()? || window.is_minimized().ok()? {
                return None;
            }
            let partner = {
                let state = app.state::<Tabs>().inner().0.lock().unwrap();
                let active = state.active_in(window.label());
                state.tabs.iter().find(|tab| tab.id == active && tab.file_key.is_some() && active != id)?.id
            };
            // Without uniform layout, the halves are of where tabs go
            let canvas = if uniform {
                crate::split::canvas_on_screen(app, &window, partner)?
            } else {
                tabs_on_screen(app, &window)?
            };
            let (halves, _) = crate::split::halves(canvas, 0.5, 0.0);
            let label = window.label().to_owned();
            let zone = |index: usize| Zone { window: label.clone(), partner, rect: halves[index], first: index == 0 };
            Some([zone(0), zone(1)])
        };
        tab_windows(app).into_iter().filter_map(zones).flatten().collect()
    }

    fn contains(&self, at: PhysicalPosition<f64>) -> bool {
        let [l, t, r, b] = self.rect;
        at.x >= l && at.x < r && at.y >= t && at.y < b
    }
}

/// Where tabs go in `window`, in screen pixels
fn tabs_on_screen(app: &AppHandle, window: &Window) -> Option<crate::split::Rect> {
    let origin = window.inner_position().ok()?;
    let factor = window.scale_factor().ok()?;
    let size = content_size(app, window).ok()?;
    let (left, top) = (origin.x as f64, origin.y as f64 + bar_height(app) * factor);
    Some([left, top, left + size.width * factor, top + size.height * factor])
}

/// Keeps window `label`, holding dragged tab `id`, under the cursor until
/// the mouse button is let go. Over another window's tab bar the window
/// hides, and the bar shows where the tab would go (`dragOver` in
/// index.html); let go there, it moves in. Over the far half of a window's
/// canvas it hides too, and a hint marks the half; let go there, the tab
/// moves in and splits the canvas with the tab there
fn follow_cursor(app: &AppHandle, id: u32, label: &str) {
    let Some(window) = app.get_window(label) else {
        return;
    };
    let Some(tab) = with_tab(app, id, |tab| {
        serde_json::json!({ "title": tab.title, "kind": tab.kind, "pinned": tab.pinned })
    }) else {
        return;
    };
    // The other windows stay put meanwhile
    let bars = Bar::all(app, label);
    let zones = Zone::all(app, label, id);
    let mut last = None;
    // The bar the tab is over, and where in it, or else the zone
    let mut over: Option<(&Bar, f64)> = None;
    let mut zone: Option<&Zone> = None;
    while mouse_down() {
        let at = app.cursor_position().ok();
        if let Some(at) = at.filter(|&at| last != Some(at)) {
            last = Some(at);
            let now = bars.iter().find_map(|bar| Some((bar, bar.x_at(at)?)));
            let now_zone = zones.iter().find(|zone| now.is_none() && zone.contains(at));
            let hidden = over.is_some() || zone.is_some();
            if over.map(|(bar, _)| &bar.label) != now.map(|(bar, _)| &bar.label) {
                if let Some(webview) = over.and_then(|(bar, _)| app.get_webview(&bar.label)) {
                    let _ = webview.eval("window.dragOver?.(null)");
                }
            }
            if zone.map(|zone| (&zone.window, zone.first)) != now_zone.map(|zone| (&zone.window, zone.first)) {
                match now_zone.and_then(|zone| Some((zone, app.get_window(&zone.window)?))) {
                    Some((zone, owner)) => crate::split::show_hint(app, &owner, zone.rect),
                    None => crate::split::hide_hint(app),
                }
            }
            match now {
                Some((bar, x)) => {
                    if let Some(webview) = app.get_webview(&bar.label) {
                        let _ = webview.eval(format!("window.dragOver?.({x}, {tab})"));
                    }
                }
                None => {
                    let _ = window.set_position(grip_position(app, &window, at));
                }
            }
            // Shown again once it's back under the cursor
            let hide = now.is_some() || now_zone.is_some();
            if hide != hidden {
                let _ = if hide { window.hide() } else { window.show() };
            }
            over = now;
            zone = now_zone;
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    crate::split::hide_hint(app);
    if let Some((bar, x)) = over {
        drop_on_bar(app, id, label, &bar.label, x);
    } else if let Some(zone) = zone {
        if move_to_window(app, id, &zone.window, usize::MAX, false).is_ok() {
            let (first, second) = if zone.first { (id, zone.partner) } else { (zone.partner, id) };
            let _ = split_pair(app, &zone.window, first, second, id);
        } else {
            let _ = window.show();
        }
    }
}

/// Moves dragged tab `id` into window `target`, let go `x` CSS pixels into
/// its tab bar, from window `from`, hidden meanwhile. If it can't, `from`
/// shows again
fn drop_on_bar(app: &AppHandle, id: u32, from: &str, target: &str, x: f64) {
    fn show(app: &AppHandle, label: &str) {
        if let Some(window) = app.get_window(label) {
            let _ = window.show();
        }
    }
    let pinned = with_tab(app, id, |tab| tab.pinned).unwrap_or(false);
    let Some(bar) = app.get_webview(target) else {
        return show(app, from);
    };
    let (handle, target_label, from_label) = (app.clone(), target.to_owned(), from.to_owned());
    let script = format!("(() => {{ window.dragOver?.(null); return window.dropIndex?.({x}, {pinned}) ?? 1e9; }})()");
    let sent = bar.eval_with_callback(script, move |json| {
        let index = json.parse::<f64>().map_or(usize::MAX, |index| index.max(0.0) as usize);
        // Not from inside the webview's own callback
        let (app, target, from) = (handle.clone(), target_label.clone(), from_label.clone());
        std::thread::spawn(move || {
            if move_to_window(&app, id, &target, index, false).is_err() {
                show(&app, &from);
            }
        });
    });
    if sent.is_err() {
        show(app, from);
    }
}

/// Reloads tab `id`. A frozen tab is put to sleep instead, so it loads
/// afresh when selected, and an asleep one already will
fn reload(app: &AppHandle, id: u32) {
    let Some(webview) = app.get_webview(&label(id)) else {
        return;
    };
    let url = webview.url().ok();
    let frozen = with_tab(app, id, |tab| {
        let frozen = tab.suspended && !tab.asleep;
        if frozen {
            tab.asleep = true;
            tab.suspended = false;
            tab.revealing = false;
            if let Some(url) = url {
                tab.url = url;
            }
        } else {
            tab.loading = true;
        }
        frozen
    });
    match frozen {
        Some(true) => {
            let _ = webview.close();
            push_state(app);
        }
        Some(false) => {
            let _ = webview.reload();
            push_state(app);
        }
        None => {}
    }
}

/// WebKit ended `webview`'s web content process, most often a hidden tab
/// over its memory limit. Tauri's own handler reloads right inside WebKit's
/// callback, which throws and aborts Ligma when the tab is frozen. Here a
/// hidden tab is put to sleep, so it loads afresh when selected, and the
/// rest reload. Off the main thread, so it all runs after the callback
#[cfg(target_os = "macos")]
pub fn on_process_terminated(webview: &Webview) {
    let app = webview.app_handle().clone();
    let name = webview.label().to_string();
    std::thread::spawn(move || {
        let Some(id) = name.strip_prefix("tab-").and_then(|id| id.parse::<u32>().ok()) else {
            if let Some(webview) = app.get_webview(&name) {
                let _ = webview.reload();
            }
            return;
        };
        let hidden = {
            let state = app.state::<Tabs>().inner().0.lock().unwrap();
            state.tabs.iter().find(|tab| tab.id == id).map(|tab| (!state.is_active(tab), tab.hidden_since))
        };
        match hidden {
            Some((true, since)) => sleep_now(&app, id, since),
            Some((false, _)) => reload(&app, id),
            None => {}
        }
    });
}

/// Loads `url` in tab `id` and selects it, or in a new tab if `id` is
/// gone. A frozen tab is put to sleep first, like in `reload`, and wakes
/// with `url`
pub fn navigate(app: &AppHandle, id: u32, url: Url) {
    let webview = app.get_webview(&label(id));
    let live = with_tab(app, id, |tab| {
        if tab.suspended && !tab.asleep {
            tab.asleep = true;
            tab.suspended = false;
            tab.revealing = false;
        }
        tab.snapshot = None;
        tab.crop = None;
        tab.url = url.clone();
        !tab.asleep
    });
    match (live, webview) {
        (None, _) => {
            let _ = open(app, url);
            return;
        }
        (Some(true), Some(webview)) => {
            let _ = webview.navigate(url);
        }
        (Some(false), Some(webview)) => {
            let _ = webview.close();
        }
        _ => {}
    }
    let _ = select(app, id);
}

/// Address tab `id` shows now, or showed when it went to sleep
fn tab_url(app: &AppHandle, id: u32) -> Option<Url> {
    let live = app.get_webview(&label(id)).and_then(|webview| webview.url().ok());
    live.or_else(|| with_tab(app, id, |tab| tab.url.clone()))
}

/// Key of tab `id`'s file, if it's one whose name can carry an emoji. A
/// prototype's tab keeps its play icon
pub fn file_key_of(app: &AppHandle, id: u32) -> Option<String> {
    with_tab(app, id, |tab| tab.file_key.clone().filter(|_| tab.kind != "proto")).flatten()
}

/// Whether a file name starts with an emoji and a space, for the menu's
/// wording. The tab bar tells emoji apart exactly (`splitEmoji`)
fn has_emoji(title: &str) -> bool {
    title.split_once(' ').is_some_and(|(first, rest)| {
        !first.is_empty() && !rest.trim().is_empty() && first.chars().all(|c| !c.is_ascii() && !c.is_alphanumeric())
    })
}

/// File `key` was renamed `name` from Ligma: retitle its tabs. Figma shows
/// the new name in the editor, but doesn't always retitle the page when
/// only the leading emoji changed
pub fn renamed(app: &AppHandle, key: &str, name: &str) {
    {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        for tab in state.tabs.iter_mut().filter(|tab| tab.file_key.as_deref() == Some(key) && tab.kind != "proto") {
            tab.title = name.to_owned();
        }
    }
    push_state(app);
}

/// Shows `emoji` as tab `id`'s while its file is renamed ("" for none), or
/// the file's own again with None
pub fn show_pending_emoji(app: &AppHandle, id: u32, emoji: Option<&str>) {
    if let Some(bar) = window_of(app, id).and_then(|window| app.get_webview(window.label())) {
        let emoji = serde_json::to_string(&emoji).unwrap();
        let _ = bar.eval(format!("window.pendingEmoji?.({id}, {emoji})"));
    }
}

/// Shows the right-click menu of tab `id` (see tab_menu.rs). Its items
/// run through `on_menu`
fn show_menu(app: &AppHandle, id: u32) -> tauri::Result<()> {
    let Some((pinned, key, window)) = with_tab(app, id, |tab| (tab.pinned, tab.file_key.clone(), tab.window.clone()))
    else {
        return Ok(());
    };
    let (alone, split) = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        // Already in a window of its own
        let alone = state.tabs_in(&window).count() == 1;
        let pair = state.splits.get(&window).copied();
        let active = state.active_in(&window);
        let file = |id: u32| state.tabs.iter().any(|tab| tab.id == id && tab.file_key.is_some());
        let split = if pair.is_some_and(|pair| pair.contains(&id)) {
            Some(("unsplit", "End Split View"))
        } else {
            // With the active file tab
            (state.split_view && id != active && active != HOME_ID && file(id) && file(active)).then_some(("split", "Split View"))
        };
        (alone, split)
    };
    // Files only, while Discord is shown anything
    let discord = key.filter(|_| crate::presence::is_on(app)).map(|key| {
        if crate::presence::is_hidden(app, &key) {
            ("discord-show", "Show File on Discord")
        } else {
            ("discord-hide", "Hide File on Discord")
        }
    });
    // A file's emoji, picked under its tab (see emoji_picker.rs)
    let emoji = file_key_of(app, id).map(|_| {
        if with_tab(app, id, |tab| has_emoji(&tab.title)).unwrap_or(false) {
            ("emoji", "Change Emoji")
        } else {
            ("emoji", "Add Emoji")
        }
    });
    let items = if id == HOME_ID {
        vec![Some(("copy", "Copy Link")), Some(("reload", "Reload")), Some(("close-others", "Close Other Tabs"))]
    } else {
        let mut items = vec![Some(if pinned { ("unpin", "Unpin Tab") } else { ("pin", "Pin Tab") })];
        if emoji.is_some() {
            items.push(emoji);
        }
        items.extend([
            Some(("copy", "Copy Link")),
            Some(("browser", "Open in Browser")),
            Some(("reload", "Reload")),
        ]);
        if !alone {
            items.push(Some(("new-window", "Move to New Window")));
        }
        if split.is_some() {
            items.push(split);
        }
        if discord.is_some() {
            items.extend([None, discord]);
        }
        items.extend([
            None,
            Some(("close", "Close")),
            Some(("close-others", "Close Other Tabs")),
            Some(("close-all", "Close All Tabs")),
        ]);
        items
    };
    crate::tab_menu::show(app, id, items, Default::default(), |app, id, action| {
        let _ = on_menu(app, &format!("{action}:{id}"));
    })
}

/// Runs a tab menu item, given its id without the `tab-menu:` prefix
pub fn on_menu(app: &AppHandle, item: &str) -> tauri::Result<()> {
    let Some((action, id)) = item.split_once(':') else {
        return Ok(());
    };
    let Ok(id) = id.parse::<u32>() else {
        return Ok(());
    };
    // All tabs in tab `id`'s window but `keep`, and but pinned ones unless
    // `pinned_too`
    let others = |keep: u32, pinned_too: bool| -> Vec<u32> {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        let Some(window) = state.tabs.iter().find(|tab| tab.id == id).map(|tab| tab.window.clone()) else {
            return Vec::new();
        };
        let tabs = state.tabs_in(&window).filter(|tab| pinned_too || !tab.pinned);
        tabs.map(|tab| tab.id).filter(|&other| other != keep).collect()
    };
    match action {
        "pin" => set_pinned(app, id, true),
        "unpin" => set_pinned(app, id, false),
        "copy" => {
            if let Some(url) = tab_url(app, id) {
                let _ = app.clipboard().write_text(url.to_string());
            }
        }
        "browser" => {
            if let Some(url) = tab_url(app, id) {
                open_in_browser(app, &url);
            }
        }
        "reload" => reload(app, id),
        // The tab bar knows where the tab is and opens it from there
        "emoji" => {
            if let Some(bar) = window_of(app, id).and_then(|window| app.get_webview(window.label())) {
                let _ = bar.eval(format!("window.openEmojiPicker?.({id})"));
            }
        }
        "split" => {
            let pair = with_tab(app, id, |tab| tab.window.clone()).map(|window| {
                let active = app.state::<Tabs>().inner().0.lock().unwrap().active_in(&window);
                (window, active)
            });
            if let Some((window, active)) = pair {
                split_pair(app, &window, active, id, id)?;
            }
        }
        "unsplit" => {
            if let Some(window) = with_tab(app, id, |tab| tab.window.clone()) {
                end_split(app, &window);
            }
        }
        "new-window" => {
            // A little right of and below the window it leaves
            if let Some(window) = window_of(app, id) {
                let (origin, px) = (window.inner_position()?, window.scale_factor()? * scale(app));
                let at = PhysicalPosition::new(
                    origin.x as f64 + (GRIP.0 + 32.0) * px,
                    origin.y as f64 + (GRIP.1 + 32.0) * px,
                );
                move_to_new_window(app, id, at, false)?;
            }
        }
        "discord-hide" | "discord-show" => {
            if let Some(Some(key)) = with_tab(app, id, |tab| tab.file_key.clone()) {
                crate::presence::set_hidden(app, &key, action == "discord-hide");
            }
        }
        "close" => close(app, id)?,
        "close-others" => {
            select(app, id)?;
            close_tabs(app, &others(id, false))?;
        }
        "close-all" => close_tabs(app, &others(HOME_ID, true))?,
        _ => {}
    }
    Ok(())
}

pub fn close_active(app: &AppHandle) -> tauri::Result<()> {
    let active = app.state::<Tabs>().inner().0.lock().unwrap().active();
    close(app, active)
}

/// Selects the tab `offset` places from the focused window's active one,
/// wrapping around
pub fn cycle(app: &AppHandle, offset: isize) -> tauri::Result<()> {
    let id = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        let tabs: Vec<&Tab> = state.tabs_in(&state.focused).collect();
        if tabs.is_empty() {
            return Ok(());
        }
        let index = tabs.iter().position(|tab| tab.id == state.active()).unwrap_or(0) as isize;
        tabs[(index + offset).rem_euclid(tabs.len() as isize) as usize].id
    };
    select(app, id)
}

pub fn new_tab(app: &AppHandle) -> tauri::Result<()> {
    open(app, HOME_URL.parse().unwrap())
}

/// Webview of tab `id` if its page can run JavaScript now: loaded, and not
/// frozen or still resuming
pub fn scriptable(app: &AppHandle, id: u32) -> Option<Webview> {
    with_tab(app, id, |tab| !tab.asleep && !tab.suspended)?
        .then(|| app.get_webview(&label(id)))
        .flatten()
}

/// Webviews of the tabs that can run JavaScript now (see `scriptable`)
pub fn scriptable_tabs(app: &AppHandle) -> Vec<Webview> {
    let ids: Vec<u32> = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        state.tabs.iter().filter(|tab| !tab.asleep && !tab.suspended).map(|tab| tab.id).collect()
    };
    ids.into_iter().filter_map(|id| app.get_webview(&label(id))).collect()
}

/// A file open in its editor, in a tab that can run JavaScript now
pub struct EditorFile {
    pub tab: u32,
    pub key: String,
    pub name: String,
    /// Whether its tab is on screen
    pub shown: bool,
}

/// Files open in Figma Design, FigJam, Slides or Buzz, the editors with a
/// plugin API, in tabs that can run JavaScript now (see `scriptable`). A
/// file open in more than one tab comes once, in the tab on screen if any
pub fn editor_files(app: &AppHandle) -> Vec<EditorFile> {
    let state = app.state::<Tabs>().inner().0.lock().unwrap();
    let mut files: Vec<EditorFile> = Vec::new();
    for tab in state.tabs.iter().filter(|tab| !tab.asleep && !tab.suspended) {
        let Some(key) = tab.file_key.clone() else {
            continue;
        };
        if !matches!(view(&tab.url), "design" | "board" | "slides" | "buzz") {
            continue;
        }
        let shown = tab.hidden_since.is_none();
        let file = EditorFile { tab: tab.id, key, name: tab.title.clone(), shown };
        match files.iter_mut().find(|other| other.key == file.key) {
            Some(other) if shown && !other.shown => *other = file,
            Some(_) => {}
            None => files.push(file),
        }
    }
    files
}

/// The file in the active tab
pub struct ActiveFile {
    pub name: String,
    /// How it's shown (see `view`)
    pub view: &'static str,
    pub key: String,
    /// Product whose editor opens it (see `kind`), presentations aside
    pub editor: &'static str,
    pub webview: Webview,
}

/// The focused window's active tab's file, if it shows a Figma file and
/// can run JavaScript (not still resuming)
pub fn active_file(app: &AppHandle) -> Option<ActiveFile> {
    let (id, name, view, key, editor) = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        let tab = state.tabs.iter().find(|tab| tab.id == state.active() && !tab.suspended)?;
        let key = tab.file_key.clone()?;
        let mut name = tab.title.as_str();
        // "Presenter · <name>" and "Audience · <name>" in Slides
        if matches!(view(&tab.url), "deck" | "presenter") {
            name = name.split_once(" · ").map_or(name, |(_, name)| name);
        }
        // Prototypes are of Design files
        let editor = match kind(&tab.url) {
            "proto" => "design",
            kind => kind,
        };
        (tab.id, name.to_owned(), view(&tab.url), key, editor)
    };
    let webview = app.get_webview(&label(id))?;
    Some(ActiveFile { name, view, key, editor, webview })
}

/// Opens the file at `<editor>/<key>` (see `ActiveFile`) in its editor.
/// From a Discord invite, so anything else is ignored
pub fn open_shared(app: &AppHandle, path: &str) -> tauri::Result<()> {
    let Some((editor, key)) = path.split_once('/') else {
        return Ok(());
    };
    let editors = ["design", "board", "slides", "make", "site", "buzz", "spec"];
    if !editors.contains(&editor) || key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Ok(());
    }
    let Ok(url) = format!("https://www.figma.com/{editor}/{key}").parse() else {
        return Ok(());
    };
    // Not yet, while a Ligma started for this is still setting up
    let Some(window) = focused_window(app) else {
        return Ok(());
    };
    let _ = window.unminimize();
    let _ = window.set_focus();
    open(app, url)
}

/// Runs `f` on tab `id`, if it's still open
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
/// never gets tabs closed
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
/// periodically, and more often while `under_pressure`
pub fn rest_idle(app: &AppHandle) {
    let now = Instant::now();
    let due: Vec<(u32, Instant, Rest, bool)> = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        let sleep_after = sleep_after(state.sleep_after);
        // Loaded hidden tabs are ranked most recently hidden first, but Home
        // last: it reloads quickly, so it's the first to sleep. A tab with
        // `max_hidden` tabs ranked above it sleeps whatever its time
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
        // it could rest
        if suspended {
            rest_now(app, id, since, rest);
            continue;
        }
        let app = app.clone();
        let _ = webview.eval_with_callback(CAN_REST_JS, move |json| {
            if json == "true" {
                // Not from inside the webview's own callback
                let app = app.clone();
                std::thread::spawn(move || rest_now(&app, id, since, rest));
            }
        });
    }
}

/// True while macOS reports memory pressure, when `rest_idle` should run
/// more often than usual
pub fn under_pressure() -> bool {
    #[cfg(target_os = "macos")]
    return crate::pressure::current() != crate::pressure::Pressure::Normal;
    #[cfg(not(target_os = "macos"))]
    false
}

/// Applies `rest` to tab `id`, unless it was shown again after `since`
fn rest_now(app: &AppHandle, id: u32, since: Instant, rest: Rest) {
    let Some(webview) = app.get_webview(&label(id)) else {
        return;
    };
    let still_hidden = move |tab: &Tab| tab.hidden_since == Some(since) && !tab.asleep;
    match rest {
        #[cfg(any(windows, target_os = "macos"))]
        Rest::Suspend => {
            // Marked before freezing, so nothing evals in the page meanwhile
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
                // Once shown again, `select` owns the flag
                with_tab(&app, id, |tab| {
                    if still_hidden(tab) {
                        tab.suspended = ok;
                    }
                });
            });
        }
        Rest::Sleep => sleep_now(app, id, Some(since)),
    }
}

/// Closes tab `id`'s webview, unless it was shown or hidden again since
/// `since`, its `hidden_since` when it was found due
fn sleep_now(app: &AppHandle, id: u32, since: Option<Instant>) {
    let Some(webview) = app.get_webview(&label(id)) else {
        return;
    };
    let url = webview.url().ok();
    let slept = with_tab(app, id, |tab| {
        if tab.asleep || tab.hidden_since != since {
            return false;
        }
        tab.asleep = true;
        tab.loading = false;
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

/// Puts every loaded tab to sleep, the ones on screen too, for game mode
/// (see game_mode.rs). Like `rest_idle`, only when `CAN_REST_JS` says no
/// work would be lost
pub fn sleep_all(app: &AppHandle) {
    let loaded: Vec<(u32, Option<Instant>, bool)> = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        state.tabs.iter().filter(|tab| !tab.asleep).map(|tab| (tab.id, tab.hidden_since, tab.suspended)).collect()
    };
    for (id, since, suspended) in loaded {
        let Some(webview) = app.get_webview(&label(id)) else {
            continue;
        };
        if suspended {
            sleep_now(app, id, since);
            continue;
        }
        let app = app.clone();
        let _ = webview.eval_with_callback(CAN_REST_JS, move |json| {
            if json == "true" {
                let app = app.clone();
                std::thread::spawn(move || sleep_now(&app, id, since));
            }
        });
    }
}

/// Reloads the tabs on screen that `sleep_all` put to sleep: each window's
/// active tab and the other half of its split view. The focused window's go
/// last, so it stays focused
pub fn wake_shown(app: &AppHandle) {
    let ids: Vec<u32> = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        let asleep = |id: u32| state.tabs.iter().any(|tab| tab.id == id && tab.asleep);
        let mut windows: Vec<&String> = state.actives.keys().collect();
        windows.sort_by_key(|&window| *window == state.focused);
        windows
            .into_iter()
            .flat_map(|window| {
                let active = state.active_in(window);
                let other = state.splits.get(window).and_then(|pair| pair.iter().copied().find(|&tab| tab != active));
                // Both halves, the active one last so it stays active
                let shown: Vec<u32> = other.into_iter().chain([active]).collect();
                if shown.iter().any(|&id| asleep(id)) { shown } else { Vec::new() }
            })
            .collect()
    };
    for id in ids {
        let _ = select(app, id);
    }
}

/// Matches the tab bars to the focused window's active tab's Figma theme.
/// Runs on tab switch and on a timer, since Figma changes theme without any
/// event Ligma sees
pub fn sync_theme(app: &AppHandle) {
    let active = {
        let state = app.state::<Tabs>().inner().0.lock().unwrap();
        if state.tabs.iter().any(|tab| tab.id == state.active() && tab.suspended) {
            return;
        }
        state.active()
    };
    let Some(webview) = app.get_webview(&label(active)) else {
        return;
    };
    let app = app.clone();
    let _ = webview.eval_with_callback(THEME_JS, move |json| {
        // null until Figma has rendered its UI
        let Ok(theme @ serde_json::Value::Object(_)) = serde_json::from_str(&json) else {
            return;
        };
        {
            let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
            if state.active() != active || state.theme.as_ref() == Some(&theme) {
                return;
            }
            state.theme = Some(theme.clone());
        }
        push_state(&app);
        repaint_page_backgrounds(&app);
        crate::settings::apply_theme(&app, &theme);
        crate::prompt::apply_theme(&app, &theme);
        #[cfg(windows)]
        crate::frame::match_theme_all(&app);
    });
}

/// The active tab's Figma theme colors (see `THEME_JS`), once known
pub fn theme(app: &AppHandle) -> Option<serde_json::Value> {
    app.state::<Tabs>().inner().0.lock().unwrap().theme.clone()
}

/// What a tab of `kind` shows until its page draws, and between pages,
/// rather than the webview's white: the color of Figma's loading screen
/// for it. A file's has a cover of its own (white in FigJam), other pages
/// Figma's background. Before a tab has reported the theme, Figma's
/// defaults for the color scheme
fn page_background(app: &AppHandle, kind: &str) -> Color {
    const WHITE: Color = Color(255, 255, 255, 255);
    if kind == "board" {
        return WHITE;
    }
    let file = !matches!(kind, "page" | "home");
    let key = if file { "loading" } else { "bg" };
    if let Some(color) = theme(app).as_ref().and_then(|theme| crate::settings::theme_color(theme, key)) {
        return color;
    }
    let light = app.get_window(MAIN).and_then(|window| window.theme().ok()) == Some(Theme::Light);
    match (light, file) {
        (true, true) => Color(230, 230, 230, 255),
        (true, false) => WHITE,
        (false, true) => Color(30, 30, 30, 255),
        (false, false) => Color(44, 44, 44, 255),
    }
}

/// `page_background` for every tab, after the theme changed
fn repaint_page_backgrounds(app: &AppHandle) {
    let tabs: Vec<(u32, &'static str)> =
        app.state::<Tabs>().inner().0.lock().unwrap().tabs.iter().map(|tab| (tab.id, tab.kind)).collect();
    for (id, kind) in tabs {
        let Some(webview) = app.get_webview(&label(id)) else {
            continue;
        };
        let background = page_background(app, kind);
        let _ = webview.set_background_color(Some(background));
        #[cfg(target_os = "macos")]
        crate::wkwebview::set_background(&webview, background);
    }
}

/// The interface scale (`interfaceScale`), 1 at 100%
pub fn scale(app: &AppHandle) -> f64 {
    app.state::<Tabs>().inner().0.lock().unwrap().scale
}

/// The tab bar's height at the interface scale
pub fn bar_height(app: &AppHandle) -> f64 {
    TAB_BAR_HEIGHT * scale(app)
}

pub fn content_size(app: &AppHandle, window: &Window) -> tauri::Result<LogicalSize<f64>> {
    let size = window.inner_size()?.to_logical::<f64>(window.scale_factor()?);
    Ok(LogicalSize::new(size.width, (size.height - bar_height(app)).max(0.0)))
}

/// Where tab webviews go in `window`: below the tab bar. Set as one, not
/// with `set_position` and `set_size`: on Windows a webview moved to another
/// window measures its position against the old one, so `set_size` would
/// put it back out of place
pub fn tab_bounds(app: &AppHandle, window: &Window) -> tauri::Result<tauri::Rect> {
    Ok(tauri::Rect {
        position: LogicalPosition::new(0.0, bar_height(app)).into(),
        size: content_size(app, window)?.into(),
    })
}

/// Zooms the tab bar, Figma tabs and open prompts to the interface scale,
/// and fits the tabs under the bar at its new height
pub fn apply_scale(app: &AppHandle) {
    let scale = scale(app);
    for window in tab_windows(app) {
        for webview in window.webviews() {
            let label = webview.label();
            if is_tab_window(label) || label.starts_with("tab-") {
                let _ = webview.set_zoom(scale);
            }
        }
    }
    crate::tab_menu::hide(app);
    crate::preview::hide(app);
    crate::prompt::apply_scale(app, scale);
    crate::settings::apply_scale(app, scale);
    layout(app);
}

/// Refits tab webviews to their windows. Call on resize
pub fn layout(app: &AppHandle) {
    for window in tab_windows(app) {
        let Ok(bounds) = tab_bounds(app, &window) else {
            continue;
        };
        for webview in window.webviews() {
            if webview.label().starts_with("tab-") {
                let _ = webview.set_bounds(bounds);
            }
        }
        fit_tab_bar(app, window.label());
    }
    crate::split::forget_placed();
    refresh_splits(app);
    push_state(app);
}

/// True for the label of a window with tabs, and of its tab bar
pub fn is_tab_window(label: &str) -> bool {
    label == MAIN || label.starts_with(WINDOW_PREFIX)
}

/// The windows with tabs, the main one first
fn tab_windows(app: &AppHandle) -> Vec<Window> {
    let mut windows: Vec<Window> = app.windows().into_values().filter(|window| is_tab_window(window.label())).collect();
    windows.sort_by_key(|window| window.label() != MAIN);
    windows
}

/// The window with tabs last focused, for what Ligma shows over it
pub fn focused_window(app: &AppHandle) -> Option<Window> {
    let focused = app.state::<Tabs>().inner().0.lock().unwrap().focused.clone();
    app.get_window(&focused).or_else(|| app.get_window(MAIN))
}

/// The window tab `id` is in
pub fn window_of(app: &AppHandle, id: u32) -> Option<Window> {
    with_tab(app, id, |tab| tab.window.clone()).and_then(|label| app.get_window(&label))
}

/// True while any window with tabs has focus
pub fn any_focused(app: &AppHandle) -> bool {
    tab_windows(app).iter().any(|window| window.is_focused().unwrap_or(false))
}

/// True while a window with tabs is the one in front, so Ligma may show a
/// prompt over it without taking focus from another app. On Windows a Figma
/// tab's webview holds the keyboard focus, not its window, so this asks for
/// the foreground window instead
#[cfg(windows)]
pub fn in_front(app: &AppHandle) -> bool {
    let foreground = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
    tab_windows(app).iter().any(|window| window.hwnd().is_ok_and(|hwnd| hwnd.0 as isize == foreground.0 as isize))
}

#[cfg(not(windows))]
pub fn in_front(app: &AppHandle) -> bool {
    any_focused(app)
}

/// Makes a window for tabs, `size` within. Its own webview is the tab bar;
/// tabs are added as child webviews below it
pub fn build_window(app: &AppHandle, label: &str, size: LogicalSize<f64>, visible: bool) -> tauri::Result<Window> {
    #[allow(unused_mut)]
    let mut builder = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title("Ligma")
        .inner_size(size.width, size.height)
        .min_inner_size(800.0, 600.0)
        .visible(visible)
        .background_color(tauri::window::Color(44, 44, 44, 255));
    #[cfg(target_os = "macos")]
    {
        builder = builder.title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true);
    }
    // Elsewhere the tab bar draws its own window controls
    #[cfg(not(target_os = "macos"))]
    {
        builder = builder.decorations(false);
    }
    let window = builder.build()?;
    // The app icon picked in settings, on the taskbar (see app_icon.rs)
    #[cfg(windows)]
    crate::app_icon::apply_to(&window);
    // Built hidden, it would keep a title bar's worth of frame until shown
    // (see frame.rs)
    #[cfg(windows)]
    if !visible {
        crate::frame::refresh(&window);
    }
    // Traffic lights centered on the tab bar (see titlebar.rs)
    #[cfg(target_os = "macos")]
    {
        let ns_window = window.clone();
        window.run_on_main_thread(move || {
            if let Ok(ns_window) = ns_window.ns_window() {
                unsafe { crate::titlebar::center_traffic_lights(ns_window.cast()) };
            }
        })?;
    }
    // `layout` sizes the tab bar's webview from here on
    if let Some(bar) = app.get_webview(label) {
        bar.set_auto_resize(false)?;
        bar.set_zoom(scale(app))?;
        #[cfg(windows)]
        crate::webview2::add_tab_shortcuts(&bar);
    }
    let handle = app.clone();
    let label = label.to_owned();
    window.on_window_event(move |event| match event {
        WindowEvent::Resized(_) => {
            crate::preview::hide(&handle);
            layout(&handle);
        }
        WindowEvent::Moved(_) => {
            crate::preview::hide(&handle);
            #[cfg(windows)]
            if let Some(window) = handle.get_window(&label) {
                for webview in window.webviews().iter().filter(|webview| webview.label().starts_with("tab-")) {
                    crate::webview2::on_moved(webview);
                }
            }
        }
        WindowEvent::Focused(false) => crate::preview::hide(&handle),
        WindowEvent::Focused(true) => {
            let changed = {
                let mut state = handle.state::<Tabs>().inner().0.lock().unwrap();
                std::mem::replace(&mut state.focused, label.clone()) != label
            };
            if changed {
                sync_theme(&handle);
            }
        }
        WindowEvent::Destroyed => forget_window(&handle, &label),
        _ => {}
    });
    Ok(window.as_ref().window())
}

/// Drops the tabs of window `label`, which is gone along with their
/// webviews
fn forget_window(app: &AppHandle, label: &str) {
    {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        // The main window's tabs are kept for the next launch
        if label == MAIN {
            return;
        }
        state.tabs.retain(|tab| tab.window != label);
        state.actives.remove(label);
        state.previous.remove(label);
        state.splits.remove(label);
        state.split_ratios.remove(label);
        state.snapshot_shown.retain(|shown| shown != label);
        if state.focused == label {
            state.focused = MAIN.to_owned();
        }
    }
    push_state(app);
}

/// Closes every window with tabs but the main one, for when that goes
pub fn close_other_windows(app: &AppHandle) {
    for window in tab_windows(app).into_iter().filter(|window| window.label() != MAIN) {
        let _ = window.destroy();
    }
}

/// Sends the tab list to the tab bar
fn push_state(app: &AppHandle) {
    #[derive(Serialize)]
    struct View<'a> {
        tabs: &'a [&'a Tab],
        active: u32,
        fullscreen: bool,
        theme: &'a Option<serde_json::Value>,
        platform: &'static str,
        /// The window's split view's tabs, if any
        split: Option<[u32; 2]>,
        /// The version downloaded and ready to install, if any
        update: Option<String>,
    }

    let update = crate::updater::ready_version(app);
    // Not `get_webview_window`: that returns None once the window holds
    // more than one webview
    for window in tab_windows(app) {
        let Some(bar) = app.get_webview(window.label()) else {
            continue;
        };
        let fullscreen = window.is_fullscreen().unwrap_or(false);
        let json = {
            let state = app.state::<Tabs>().inner().0.lock().unwrap();
            let tabs: Vec<&Tab> = state.tabs_in(window.label()).collect();
            serde_json::to_string(&View {
                tabs: &tabs,
                active: state.active_in(window.label()),
                fullscreen,
                theme: &state.theme,
                platform: std::env::consts::OS,
                split: state.splits.get(window.label()).copied(),
                update: update.clone(),
            })
            .unwrap()
        };
        let _ = bar.eval(format!("window.render?.({json})"));
    }
    save_session(app);
}

/// Draws the tab bars again, for when an update is ready
pub fn refresh_bars(app: &AppHandle) {
    push_state(app);
}

/// Puts the tab bars' Update Now button back after the install failed
pub fn update_failed(app: &AppHandle) {
    for window in tab_windows(app) {
        if let Some(bar) = app.get_webview(window.label()) {
            let _ = bar.eval("window.updateFailed?.()");
        }
    }
}

/// Only the local tab bar may drive tabs. Figma pages have no IPC access
/// anyway (no capability grants it), this is a second fence
pub fn from_tab_bar(webview: &Webview) -> bool {
    is_tab_window(webview.label())
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
pub async fn tab_menu(app: AppHandle, webview: Webview, id: u32) {
    if from_tab_bar(&webview) {
        let _ = show_menu(&app, id);
    }
}

#[tauri::command]
pub async fn move_tab_to(app: AppHandle, webview: Webview, id: u32, index: usize) {
    if from_tab_bar(&webview) {
        move_tab(&app, id, index);
    }
}

#[tauri::command]
pub async fn open_new_tab(app: AppHandle, webview: Webview) {
    if from_tab_bar(&webview) {
        app.state::<Tabs>().inner().0.lock().unwrap().focused = webview.label().to_owned();
        let _ = new_tab(&app);
    }
}

/// The divider of the tab bar's window's split view was dragged: its first
/// half now takes `ratio` of the canvas
#[tauri::command]
pub async fn set_split_ratio(app: AppHandle, webview: Webview, ratio: f64) {
    if !from_tab_bar(&webview) || !ratio.is_finite() {
        return;
    }
    let window = webview.label().to_owned();
    {
        let mut state = app.state::<Tabs>().inner().0.lock().unwrap();
        if !state.splits.contains_key(&window) {
            return;
        }
        state.split_ratios.insert(window.clone(), ratio);
    }
    layout_split(&app, &window);
}

/// Tab `id` was dragged off its bar, the mouse button still held. It goes
/// into a window of its own right away, which follows the cursor until
/// it's let go (see `follow_cursor`). A tab that wasn't `was_active` was
/// only selected by pressing it, so its window goes back to the tab it
/// showed before
#[tauri::command]
pub async fn tear_off_tab(app: AppHandle, webview: Webview, id: u32, was_active: bool) {
    if !from_tab_bar(&webview) {
        return;
    }
    let Ok(at) = app.cursor_position() else {
        return;
    };
    if let Ok(Some(label)) = move_to_new_window(&app, id, at, !was_active) {
        std::thread::spawn(move || follow_cursor(&app, id, &label));
    }
}

/// The tab bar's Update Now button: installs the downloaded update and
/// restarts into it
#[tauri::command]
pub async fn update_now(app: AppHandle, webview: Webview) {
    if from_tab_bar(&webview) {
        std::thread::spawn(move || crate::updater::install_now(&app));
    }
}

/// Minimize, maximize and close buttons the tab bar draws where the window
/// has no native title bar (Windows, Linux)
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
        "settings" => crate::settings::open_window(window.app_handle()),
        _ => Ok(()),
    };
}
