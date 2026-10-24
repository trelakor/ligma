//! Discord Rich Presence: shows the open file, where in it the user is
//! (the page, slide, chat, asset or webpage) and the selected tool, as
//! "Ligma". The tool's icon is the small image, blue while the window has
//! focus and grey while it doesn't. Discord names the activity after the Discord application, so
//! the application behind `CLIENT_ID` must be called "Ligma".
//!
//! Talking to Discord blocks, so a worker thread owns the connection. The
//! app sends it the wanted status every second and it forwards changes,
//! reconnecting whenever Discord is (re)started.
//!
//! With invites on (`discordInvites`), the open file is also a Discord
//! party: one per file, sized by the people Figma shows in it. Friends can
//! then be invited from Discord's + menu, or ask to join from the user's
//! profile (Ligma asks the user first). Joining starts Ligma through the
//! launch command `register_launch` leaves for Discord, and Discord hands
//! the party's secret, `<editor>/<file key>`, to `ACTIVITY_JOIN`. Ligma
//! opens that file, and Figma shows it to whoever has access.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use discord_rich_presence::activity::{Activity, Assets, Party, Secrets, Timestamps};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};

use crate::discord_ipc::Ipc;

/// Application ID from the Discord Developer Portal. Empty: presence off.
const CLIENT_ID: &str = "1563151879788167309";
const LOGO_URL: &str = "https://raw.githubusercontent.com/trelakor/ligma/main/assets/discord-logo.png";
/// Tool icons are art assets of the Discord application, uploaded in the
/// Developer Portal (Rich Presence > Art Assets) and named
/// `<product>-<focused|unfocused>-<slug>`. Only Figma Design's are there so
/// far. `npm run discord-assets` checks that every one is uploaded.
const TOOLS_PRODUCT: &str = "design";

/// Discord allows 5 activity updates per 20 seconds.
const MIN_INTERVAL: Duration = Duration::from_secs(4);
/// Wait between attempts to reach Discord when it isn't running.
const RETRY_AFTER: Duration = Duration::from_secs(15);
/// Party sizes Discord shows as the most people a file can hold, the
/// smallest one with room to spare. Discord only offers invites to a party
/// with a size and room left in it; Figma has no such limit to report.
const PARTY_SIZES: [i32; 4] = [10, 25, 50, 100];

/// Party size for `people` in a file: "3 of 10", "12 of 25".
fn party_max(people: i32) -> i32 {
    PARTY_SIZES.into_iter().find(|&max| max > people).unwrap_or(people + 1)
}

/// Name of the selected tool in the toolbar, or null. Figma marks the
/// active tool button with `aria-pressed` or `aria-checked`; its label may
/// carry a shortcut ("Pen – P") or name a tool group ("Move tools").
const TOOL_JS: &str = r#"(() => {
  const tools = [
    "Move", "Hand", "Scale", "Frame", "Section", "Slice", "Rectangle", "Line",
    "Arrow", "Ellipse", "Polygon", "Star", "Image", "Pen", "Pencil", "Text",
    "Comment", "Sticky", "Shape", "Connector", "Marker", "Highlighter",
    "Eraser", "Washi tape", "Stamp", "Table", "Text on path", "Annotation",
    "Measurement",
  ];
  const byName = new Map(tools.map((tool) => [tool.toLowerCase(), tool]));
  // Dev Mode names these after the action.
  byName.set("annotate", "Annotation");
  byName.set("measure", "Measurement");
  for (const el of document.querySelectorAll('[aria-pressed="true"], [aria-checked="true"]')) {
    const label = (el.getAttribute("aria-label") || el.getAttribute("data-tooltip") || "")
      .replace(/\s+[–—-]\s+.*$/, "")
      .replace(/\s+tools?$/i, "")
      .trim()
      .toLowerCase();
    if (byName.has(label)) return byName.get(label);
  }
  return null;
})()"#;

/// Where in the file the user is, shown under its name, or null. A
/// function of the file's view (see `tabs::view`). Pages come from Figma's
/// app state, the rest from its panels. Buzz shows an asset's name only on
/// the canvas, so it's remembered from when the asset itself is selected.
const SUBTEXT_JS: &str = r#"((view) => {
  // Props of the component `up` levels above `el` in React's tree.
  const props = (el, up) => {
    const key = el && Object.keys(el).find((key) => key.startsWith("__reactFiber"));
    let fiber = key && el[key];
    for (let i = 0; fiber && i < up; i++) fiber = fiber.return;
    return fiber?.memoizedProps;
  };
  const text = (selector) => document.querySelector(selector)?.textContent.trim() || null;
  const mirror = window.store?.getState?.().mirror;
  switch (view) {
    case "design":
    case "board": {
      const app = mirror?.appModel;
      const page = app?.pagesList?.find((page) => page.nodeId === app.currentPage);
      return page?.name ? `Page: ${page.name}` : null;
    }
    // Presenting: "3 / 12" under the slide or frame.
    case "proto":
    case "deck":
    case "presenter": {
      const step = /^(\d+)\s*\/\s*(\d+)$/.exec(text('[aria-label="Stepper"] [class*="toolbeltLabel"]') || "");
      return step && `${view === "proto" ? "Frame" : "Slide"} ${step[1]} of ${step[2]}`;
    }
    case "slides": {
      const carousel = document.querySelector('[data-testid="slides-carousel"]');
      const number = carousel?.querySelector('[aria-selected="true"][aria-label^="Slide "]')
        ?.getAttribute("aria-label").slice("Slide ".length);
      if (!number) return null;
      // The carousel only draws the slides in view, so count them in its data.
      let total = 0;
      for (let up = 1; up < 5 && !total; up++) {
        const items = props(carousel, up)?.carouselData?.carouselItemsById;
        if (items) total = Math.max(0, ...Object.values(items).map((item) => item.number || 0));
      }
      return total ? `Slide ${number} of ${total}` : `Slide ${number}`;
    }
    case "make": {
      const chat = text('[data-testid="chat-thread-row"] [aria-current="true"] span')
        || text('[data-testid="chat-threads-header"] span');
      return chat && `Chat: ${chat}`;
    }
    case "buzz": {
      const names = (window.__ligmaAssetNames ||= {});
      const item = '[data-testid="carousel-item"]';
      const selected = Object.keys(mirror?.sceneGraphSelection || {});
      const name = mirror?.selectionProperties?.name;
      if (selected.length === 1 && name && document.querySelector(`${item}[value="${selected[0]}"]`)) {
        names[selected[0]] = name;
      }
      const current = document.querySelector(`${item}:has([aria-selected="true"])`)?.getAttribute("value");
      return names[current] ? `Asset: ${names[current]}` : null;
    }
    case "site": {
      // The webpage with the selection in it, else the last one that had it.
      const names = new Map();
      for (const row of document.querySelectorAll('[data-testid="left-panel-body"] [data-fpl-audit]')) {
        const page = props(row, 2);
        if (!page?.recordingKey?.startsWith("webpagesList.row")) continue;
        names.set(page.guid, row.textContent.trim());
        if (page.selected) window.__ligmaWebpage = page.guid;
      }
      const name = names.get(window.__ligmaWebpage);
      return name ? `Webpage: ${name}` : null;
    }
  }
  return null;
})"#;

/// Number of people in the file: Figma's avatars of live sessions in the
/// toolbar, the user's own included (Figma shows one per person), and any
/// "+N" for those that don't fit.
const PEOPLE_JS: &str = r#"(() => {
  const sessions = new Set();
  for (const el of document.querySelectorAll("[data-tooltip-user-session-id]")) {
    sessions.add(el.getAttribute("data-tooltip-user-session-id"));
  }
  let more = 0;
  for (const el of document.querySelectorAll('[data-onboarding-key="multiplayer-row-container"] *')) {
    const count = el.children.length ? null : /^\+(\d+)$/.exec(el.textContent.trim());
    if (count) more = Math.max(more, Number(count[1]));
  }
  return Math.max(1, sessions.size + more);
})()"#;

/// The open file as a Discord party.
#[derive(Clone, PartialEq)]
struct Invite {
    /// Same for everyone in the file, without giving its key away.
    party: String,
    /// Handed to whoever joins: `<editor>/<file key>`.
    secret: String,
    people: i32,
}

#[derive(Clone, Default, PartialEq)]
struct Status {
    /// Name of the open file. None outside a file (Home, other pages).
    file: Option<String>,
    /// What's done with the file ("Designing", "Presenting", ...).
    verb: &'static str,
    /// Where in the file the user is ("Page: Cover"), under its name.
    subtext: Option<String>,
    tool: Option<String>,
    /// Whether the Ligma window has focus.
    focused: bool,
    /// Only with invites on, in a file.
    invite: Option<Invite>,
}

enum Message {
    Status(Status),
    /// The user's answer to Discord user `user` asking to join.
    Answer { user: String, invite: bool },
}

pub struct Presence {
    /// Channel to the worker. None when presence is off.
    tx: Mutex<Option<Sender<Message>>>,
    /// The settings in effect, for what to show.
    settings: Mutex<crate::settings::Settings>,
}

fn worker(app: &AppHandle, settings: &crate::settings::Settings) -> Option<Sender<Message>> {
    if CLIENT_ID.is_empty() || !settings.discord_rich_presence {
        return None;
    }
    register_launch(app);
    let (tx, rx) = channel();
    let app = app.clone();
    std::thread::spawn(move || run(&app, rx));
    Some(tx)
}

pub fn start(app: &AppHandle, settings: &crate::settings::Settings) -> Presence {
    Presence {
        tx: Mutex::new(worker(app, settings)),
        settings: Mutex::new(settings.clone()),
    }
}

/// Tells Discord how to start Ligma for someone joining a file, the way
/// Discord's own RPC library does. Joining while Ligma runs goes straight
/// to it instead.
fn register_launch(_app: &AppHandle) {
    #[cfg(windows)]
    {
        use windows_registry::CURRENT_USER;
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let exe = exe.display().to_string();
        let key = format!(r"Software\Classes\discord-{CLIENT_ID}");
        let _ = CURRENT_USER.create(&key).and_then(|key| {
            key.set_string("", format!("URL:Run game {CLIENT_ID} protocol"))?;
            key.set_string("URL Protocol", "")
        });
        let _ = CURRENT_USER
            .create(format!(r"{key}\DefaultIcon"))
            .and_then(|key| key.set_string("", &exe));
        let _ = CURRENT_USER
            .create(format!(r"{key}\shell\open\command"))
            .and_then(|key| key.set_string("", format!("\"{exe}\"")));
    }
    // Discord reads the command from its games folder, if it's installed.
    #[cfg(target_os = "macos")]
    {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let discord = std::path::Path::new(&home).join("Library/Application Support/discord");
        if !discord.is_dir() {
            return;
        }
        let games = discord.join("games");
        let command = format!("open -b {}", _app.config().identifier);
        let _ = std::fs::create_dir_all(&games);
        let _ = std::fs::write(
            games.join(format!("{CLIENT_ID}.json")),
            json!({ "command": command }).to_string(),
        );
    }
}

/// Turns presence on or off. Dropping the channel stops the worker, which
/// disconnects from Discord and so clears the status.
pub fn apply_settings(app: &AppHandle, settings: &crate::settings::Settings) {
    let presence = app.state::<Presence>();
    *presence.settings.lock().unwrap() = settings.clone();
    let mut tx = presence.tx.lock().unwrap();
    if tx.is_some() != settings.discord_rich_presence {
        *tx = worker(app, settings);
    }
}

/// Reports the active tab to Discord. Call periodically.
pub fn sync(app: &AppHandle) {
    let presence = app.state::<Presence>();
    let Some(tx) = presence.tx.lock().unwrap().clone() else {
        return;
    };
    // Not `get_webview_window`: that returns None once the window holds
    // more than one webview, i.e. whenever a file tab is open.
    let focused = app
        .get_window("main")
        .and_then(|window| window.is_focused().ok())
        .unwrap_or(false);
    let Some(active) = crate::tabs::active_file(app) else {
        let _ = tx.send(Message::Status(Status { focused, ..Status::default() }));
        return;
    };
    let settings = presence.settings.lock().unwrap().clone();
    let (verb, presenting) = verb(active.view);
    // An empty name shows as "Designing a file" and so on.
    let file = if settings.discord_show_file { active.name } else { String::new() };
    let invite = settings.discord_invites.then(|| Invite {
        party: party_id(&active.key),
        secret: format!("{}/{}", active.editor, active.key),
        people: 1,
    });
    // Presentations have no tools.
    let show_tool = !presenting && settings.discord_show_tool;
    let show_subtext = shows_subtext(&settings, active.view);
    if !show_tool && !show_subtext && invite.is_none() {
        let status = Status { file: Some(file), verb, focused, invite, ..Status::default() };
        let _ = tx.send(Message::Status(status));
        return;
    }
    let script = format!(
        "[{}, {}, {}]",
        if show_tool { TOOL_JS } else { "null" },
        if invite.is_some() { PEOPLE_JS } else { "1" },
        if show_subtext { format!("{SUBTEXT_JS}({:?})", active.view) } else { "null".to_owned() },
    );
    let _ = active.webview.eval_with_callback(script, move |json| {
        let (tool, people, subtext) =
            serde_json::from_str::<(Option<String>, i32, Option<String>)>(&json).unwrap_or((None, 1, None));
        let invite = invite.clone().map(|invite| Invite { people: people.max(1), ..invite });
        let status = Status { file: Some(file.clone()), verb, subtext, tool, focused, invite };
        let _ = tx.send(Message::Status(status));
    });
}

/// Whether `settings` show where in the file the user is, for a file shown
/// in `view` (see `tabs::view`).
fn shows_subtext(settings: &crate::settings::Settings, view: &str) -> bool {
    match view {
        "design" => settings.discord_show_page,
        "board" => settings.discord_show_figjam_page,
        "slides" | "deck" | "presenter" | "proto" => settings.discord_show_slide,
        "make" => settings.discord_show_chat,
        "buzz" => settings.discord_show_asset,
        "site" => settings.discord_show_webpage,
        _ => false,
    }
}

/// Discord party of the file with key `key`: an FNV-1a hash, the same in
/// every Ligma.
fn party_id(key: &str) -> String {
    let hash = format!("ligma:{key}").bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| time.as_millis() as i64)
}

/// Slug of the tool's icon among the art assets, if it has one.
fn tool_icon(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "Move" => "move",
        "Hand" => "hand",
        "Scale" => "scale",
        "Frame" => "frame",
        "Section" => "section",
        "Slice" => "slice",
        "Rectangle" => "rectangle",
        "Line" => "line",
        "Arrow" => "arrow",
        "Ellipse" => "ellipse",
        "Polygon" => "polygon",
        "Star" => "star",
        "Pen" => "pen",
        "Pencil" => "pencil",
        "Text" => "text",
        "Text on path" => "text-on-path",
        "Comment" => "comment",
        "Annotation" => "annotation",
        "Measurement" => "measurement",
        _ => return None,
    })
}

/// What's done with a file shown in `view` (see `tabs::view`), and
/// whether that's presenting it.
fn verb(view: &str) -> (&'static str, bool) {
    match view {
        "proto" | "deck" | "presenter" => ("Presenting", true),
        "board" => ("Jamming", false),
        "slides" => ("Decking", false),
        "make" => ("Generating", false),
        "buzz" => ("Buzzing", false),
        "site" => ("Page-building", false),
        _ => ("Designing", false),
    }
}

/// `version` is Ligma's, shown when hovering the logo.
fn activity<'a>(status: &'a Status, started: i64, version: &str) -> Activity<'a> {
    // "Designing in <file>", but "Presenting <file>".
    let verb = match status.verb {
        "Presenting" => "Presenting".to_owned(),
        verb => format!("{verb} in"),
    };
    let details = match status.file.as_deref() {
        // "Figma" is the title while the file is still loading.
        Some("" | "Figma") => format!("{verb} a file"),
        Some(file) => format!("{verb} {file}"),
        None => "Browsing files".to_owned(),
    };
    let mut assets = Assets::new().large_image(LOGO_URL).large_text(format!("Ligma {version}"));
    let mut activity = Activity::new()
        .name("Ligma")
        .details(details)
        .timestamps(Timestamps::new().start(started));
    // The tool is only the small image, named when hovered.
    if let Some((tool, icon)) = status.tool.as_deref().and_then(|tool| Some((tool, tool_icon(tool)?))) {
        let state = if status.focused { "focused" } else { "unfocused" };
        assets = assets
            .small_image(format!("{TOOLS_PRODUCT}-{state}-{icon}"))
            .small_text(format!("{tool} tool"));
    }
    // Discord takes 2 to 128 characters.
    if let Some(subtext) = status.subtext.as_deref().filter(|subtext| subtext.chars().count() >= 2) {
        activity = activity.state(subtext.chars().take(128).collect::<String>());
    }
    if let Some(invite) = &status.invite {
        activity = activity
            .party(Party::new().id(invite.party.as_str()).size([invite.people, party_max(invite.people)]))
            .secrets(Secrets::new().join(invite.secret.as_str()));
    }
    activity.assets(assets)
}

/// Handles an event Discord sent (see `subscribe`).
fn on_event(app: &AppHandle, message: &Value, file: Option<&str>) {
    let data = &message["data"];
    match message["evt"].as_str() {
        // The user joined a friend's file, maybe starting Ligma for it.
        Some("ACTIVITY_JOIN") => {
            if let Some(secret) = data["secret"].as_str() {
                let (app, secret) = (app.clone(), secret.to_owned());
                // Off this thread: opening a tab waits on the main thread.
                std::thread::spawn(move || crate::tabs::open_shared(&app, &secret));
            }
        }
        // A friend asked to join the user's file.
        Some("ACTIVITY_JOIN_REQUEST") => {
            let user = &data["user"];
            let Some(id) = user["id"].as_str() else {
                return;
            };
            let name = [&user["global_name"], &user["username"]]
                .into_iter()
                .find_map(|name| name.as_str().filter(|name| !name.is_empty()))
                .unwrap_or("Someone");
            let file = match file {
                Some("" | "Figma") | None => "your file".to_owned(),
                Some(file) => file.to_owned(),
            };
            let (app, id) = (app.clone(), id.to_owned());
            app.clone()
                .dialog()
                .message(format!(
                    "{name} asked on Discord to join {file}. If you invite them, it opens in \
                     their Ligma, for them to see if they have access."
                ))
                .title("Join Request")
                .buttons(MessageDialogButtons::OkCancelCustom("Invite".into(), "Ignore".into()))
                .show(move |invite| {
                    let tx = app.state::<Presence>().tx.lock().unwrap().clone();
                    if let Some(tx) = tx {
                        let _ = tx.send(Message::Answer { user: id, invite });
                    }
                });
        }
        _ => {}
    }
}

/// Connects to Discord and subscribes to the events `on_event` handles.
fn connect() -> std::io::Result<Ipc> {
    let mut ipc = Ipc::connect(CLIENT_ID)?;
    ipc.subscribe("ACTIVITY_JOIN")?;
    ipc.subscribe("ACTIVITY_JOIN_REQUEST")?;
    Ok(ipc)
}

fn run(app: &AppHandle, rx: Receiver<Message>) {
    let mut client: Option<Ipc> = None;
    let mut retry_at = Instant::now();
    let mut wanted = Status::default();
    // What Discord shows. None after connecting, so it gets sent again.
    let mut shown: Option<Status> = None;
    let mut sent_at: Option<Instant> = None;
    // The elapsed time restarts with each file, and when presenting starts
    // or stops.
    let mut started = ((None, ""), now_ms());
    // Answers to join requests, to send.
    let mut answers = Vec::new();
    loop {
        let mut message = match rx.recv_timeout(Duration::from_secs(1)) {
            Ok(message) => Some(message),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(mut client) = client {
                    let args = json!({ "pid": std::process::id(), "activity": null });
                    let _ = client.command("SET_ACTIVITY", args);
                }
                return;
            }
        };
        while let Some(next) = message.take().or_else(|| rx.try_recv().ok()) {
            match next {
                Message::Status(status) => wanted = status,
                Message::Answer { user, invite } => answers.push((user, invite)),
            }
        }
        if let Some(ipc) = &mut client {
            let mut talk = || -> std::io::Result<()> {
                for (user, invite) in answers.drain(..) {
                    let cmd = if invite { "SEND_ACTIVITY_JOIN_INVITE" } else { "CLOSE_ACTIVITY_REQUEST" };
                    ipc.command(cmd, json!({ "user_id": user }))?;
                }
                // Also Discord's responses to commands, which go unread.
                while let Some(message) = ipc.poll()? {
                    on_event(app, &message, shown.as_ref().and_then(|shown| shown.file.as_deref()));
                }
                Ok(())
            };
            if talk().is_err() {
                // Discord quit. Reconnect on the next tick.
                client = None;
                shown = None;
            }
        }
        if shown.as_ref() == Some(&wanted) || sent_at.is_some_and(|at| at.elapsed() < MIN_INTERVAL) {
            continue;
        }
        if client.is_none() {
            if Instant::now() < retry_at {
                continue;
            }
            match connect() {
                Ok(new) => client = Some(new),
                Err(_) => {
                    retry_at = Instant::now() + RETRY_AFTER;
                    continue;
                }
            }
        }
        let key = (wanted.file.clone(), wanted.verb);
        if started.0 != key {
            started = (key, now_ms());
        }
        let version = app.package_info().version.to_string();
        let args = json!({ "pid": std::process::id(), "activity": activity(&wanted, started.1, &version) });
        let ok = client
            .as_mut()
            .is_some_and(|client| client.command("SET_ACTIVITY", args).is_ok());
        sent_at = Some(Instant::now());
        if ok {
            shown = Some(wanted.clone());
        } else {
            // Discord quit. Reconnect on the next change or tick.
            client = None;
            shown = None;
        }
    }
}
