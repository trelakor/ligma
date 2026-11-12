//! Runs the native renderer: its window, the threads that keep the scene and
//! viewport in step with the Figma tab, and the dev-only parity harness and
//! benchmark

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde::Serialize;
use serde_json::{json, Value};
use skia_safe::Rect;
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, Webview, WebviewBuilder, WebviewUrl, WindowEvent};

use super::adjust::Adjust;
use super::draw::{Img, RENDER, RENDER_LIMIT};
use super::gpu_memory::GpuMemory;
use super::present::{Device, Hub, Screen};
use super::render::{Gpu, Msg, Offscreen, Painter, Request, View};
use super::scene::{Batch, Begin, Changes, Scene};
use super::{percentile, stats, OVERLAY, WINDOW};

const PAGE_JS: &str = include_str!("page.js");

/// Characters per `slice()`
const CHUNK: usize = 8 << 20;

/// Height of the overlay bar, in logical pixels
const BAR: f64 = 34.0;

/// What the threads share
struct Ctx {
    app: AppHandle,
    stop: Arc<AtomicBool>,
    messages: Sender<Msg>,
    /// The tab whose page is mirrored
    webview: Mutex<Option<Webview>>,
    /// Views seen while a benchmark runs
    recorded: Mutex<Option<Vec<View>>>,
    /// Highest GPU memory seen while a benchmark runs
    peak: Mutex<Option<super::gpu_memory::Sample>>,
    /// Asks the sync loop to read the whole page again
    resync: AtomicBool,
    /// The parity harness or benchmark is running
    busy: AtomicBool,
    /// Image fills' original bytes, kept to decode again at a bigger size
    /// when zooming in, without asking Figma again
    encoded: Mutex<HashMap<String, Arc<Vec<u8>>>>,
    /// Memory the mirrored tab writes its view to every frame, and the tab
    shared: Mutex<Option<(String, crate::webview2::Shared)>>,
    /// What the render and present threads share
    hub: Arc<Hub>,
}

/// What page.js read, handed over by the page as each job is done rather
/// than asked for again and again, which while Figma opens a file waits
/// behind it for hundreds of milliseconds a time. The page sends it to an
/// address on its own site that only this run of Ligma knows, which Ligma
/// answers itself (see `webview2::intercept`)
struct Pushed {
    secret: String,
    bodies: Mutex<Bodies>,
    arrived: Condvar,
    /// Tabs whose requests to that address Ligma answers
    tabs: Mutex<std::collections::HashSet<String>>,
}

fn pushed() -> &'static Pushed {
    static PUSHED: std::sync::OnceLock<Pushed> = std::sync::OnceLock::new();
    PUSHED.get_or_init(|| {
        use std::hash::{BuildHasher, Hasher};
        let mut secret = String::new();
        for _ in 0..2 {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
            secret.push_str(&format!("{:016x}", hasher.finish()));
        }
        Pushed { secret, bodies: Mutex::default(), arrived: Condvar::new(), tabs: Mutex::default() }
    })
}

#[derive(Default)]
struct Bodies {
    waiting: HashMap<String, (Instant, Vec<u8>)>,
    /// Keys lately taken, as an image can come twice when Figma and the
    /// cache both answer
    taken: std::collections::VecDeque<String>,
}

impl Pushed {
    fn put(&self, key: &str, body: Vec<u8>) {
        let mut bodies = self.bodies.lock().unwrap();
        if bodies.taken.iter().any(|taken| taken == key) {
            return;
        }
        // What no one came for: a page that went away, or images in view
        // handed over before they were asked for that weren't after all
        bodies.waiting.retain(|key, (at, _)| at.elapsed() < Duration::from_secs(if key.starts_with('i') { 20 } else { 120 }));
        bodies.waiting.insert(key.to_string(), (Instant::now(), body));
        self.arrived.notify_all();
    }

    /// The body for `key`, waiting for it until `until`
    fn wait(&self, key: &str, until: Instant) -> Option<Vec<u8>> {
        let mut bodies = self.bodies.lock().unwrap();
        loop {
            if let Some((_, body)) = bodies.waiting.remove(key) {
                if bodies.taken.len() >= 256 {
                    bodies.taken.pop_front();
                }
                bodies.taken.push_back(key.to_string());
                return Some(body);
            }
            let left = until.checked_duration_since(Instant::now()).filter(|left| !left.is_zero())?;
            bodies = self.arrived.wait_timeout(bodies, left).unwrap().0;
        }
    }
}

/// Has tab `webview` hand over what page.js reads as it's done, from now
/// on and after it loads again. False if it can't
fn start_pushing(webview: &Webview) -> bool {
    let label = webview.label().to_string();
    if !pushed().tabs.lock().unwrap().contains(&label) {
        let Ok(url) = webview.url() else {
            return false;
        };
        let Some(host) = url.host_str() else {
            return false;
        };
        let prefix = format!("{}://{host}/__ligma_native/{}/", url.scheme(), pushed().secret);
        let (send, receive) = mpsc::channel();
        crate::webview2::intercept(
            webview,
            prefix,
            |key, body| pushed().put(key, body),
            move |added| {
                let _ = send.send(added);
            },
        );
        if !receive.recv_timeout(Duration::from_secs(5)).unwrap_or(false) {
            return false;
        }
        pushed().tabs.lock().unwrap().insert(label);
    }
    let path = serde_json::to_string(&format!("/__ligma_native/{}/", pushed().secret)).unwrap();
    eval_value(webview, &format!("window.__ligmaNative.pushTo({path})")).and_then(|value| value.as_bool()) == Some(true)
}

static CTX: Mutex<Option<Arc<Ctx>>> = Mutex::new(None);

fn set_status(status: impl Into<String>) {
    stats().lock().unwrap().status = status.into();
}

pub fn start(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(error) = open(&app) {
            log::warn!("native renderer: {error}");
            set_status(format!("Couldn't start: {error}"));
        }
    });
}

pub fn stop(app: &AppHandle) {
    if let Some(ctx) = CTX.lock().unwrap().take() {
        ctx.stop.store(true, Ordering::Relaxed);
        ctx.hub.stop.store(true, Ordering::Relaxed);
        ctx.hub.poke();
        let _ = ctx.messages.send(Msg::Quit);
    }
    if let Some(window) = app.get_window(WINDOW) {
        let _ = window.destroy();
    }
}

fn open(app: &AppHandle) -> Result<(), String> {
    if CTX.lock().unwrap().is_some() {
        return Ok(());
    }
    *stats().lock().unwrap() = super::Stats { dev: super::dev(), status: "Starting".into(), ..Default::default() };
    let window = match app.get_window(WINDOW) {
        Some(window) => window,
        None => tauri::window::WindowBuilder::new(app, WINDOW)
            .title("Native renderer (experimental)")
            .inner_size(1200.0, 800.0)
            .build()
            .map_err(|error| error.to_string())?,
    };
    let size = window.inner_size().map_err(|error| error.to_string())?;
    let hwnd = window.hwnd().map_err(|error| error.to_string())?.0 as isize;
    let overlay = WebviewBuilder::new(OVERLAY, WebviewUrl::App("native.html".into()));
    let scale = window.scale_factor().unwrap_or(1.0);
    window
        .add_child(overlay, LogicalPosition::new(0.0, 0.0), LogicalSize::new(size.width as f64 / scale, BAR))
        .map_err(|error| error.to_string())?;

    let (messages, receive) = mpsc::channel();
    let (requests, requested) = mpsc::channel();
    let (jobs, job_queue) = mpsc::channel();
    let ctx = Arc::new(Ctx {
        app: app.clone(),
        stop: Arc::new(AtomicBool::new(false)),
        messages,
        webview: Mutex::new(None),
        recorded: Mutex::new(None),
        peak: Mutex::new(None),
        resync: AtomicBool::new(false),
        busy: AtomicBool::new(false),
        encoded: Mutex::new(HashMap::new()),
        shared: Mutex::new(None),
        hub: Arc::new(Hub::new((size.width, size.height))),
    });
    *CTX.lock().unwrap() = Some(ctx.clone());

    // Skia's GPU objects stay on the thread that made them: the render
    // thread draws, the present thread shows what it has drawn
    let device = Device::open();
    // Before anything draws, see effects.rs
    let code: super::effects::Fetch = {
        let ctx = ctx.clone();
        Box::new(move |id| effect_code(&ctx, id))
    };
    let kept = super::effects::Kept::default();
    super::effects::spawn(job_queue, ctx.messages.clone(), device.as_ref().ok().map(|device| device.name.clone()), code, kept.clone());
    let (started, start) = mpsc::channel();
    let (hub, render_device) = (ctx.hub.clone(), device.clone());
    std::thread::Builder::new()
        .name("native-render".into())
        .spawn(move || match render_device.and_then(|device| Gpu::new(&device, size.width, size.height)) {
            Ok(gpu) => {
                let _ = started.send(Ok(gpu.adapter.clone()));
                Painter::new(gpu, requests, jobs, kept, hub).run(receive);
            }
            Err(error) => {
                let _ = started.send(Err(error));
            }
        })
        .map_err(|error| error.to_string())?;
    let adapter = start.recv().map_err(|error| error.to_string()).and_then(|started| started);
    let adapter = adapter.and_then(|adapter| {
        let (shown, show) = mpsc::channel();
        let (hub, device) = (ctx.hub.clone(), device.clone());
        std::thread::Builder::new()
            .name("native-present".into())
            .spawn(move || match device.and_then(|device| Screen::new(&device, hwnd, size.width, size.height)) {
                Ok(screen) => {
                    let _ = shown.send(Ok(()));
                    screen.run(hub);
                }
                Err(error) => {
                    let _ = shown.send(Err(error));
                }
            })
            .map_err(|error| error.to_string())?;
        show.recv().map_err(|error| error.to_string()).and_then(|shown| shown).map(|()| adapter)
    });
    match adapter {
        Ok(adapter) => stats().lock().unwrap().adapter = adapter,
        Err(error) => {
            ctx.stop.store(true, Ordering::Relaxed);
            ctx.hub.stop.store(true, Ordering::Relaxed);
            let _ = ctx.messages.send(Msg::Quit);
            *CTX.lock().unwrap() = None;
            return Err(error);
        }
    }

    let events = ctx.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::Resized(size) => {
            *events.hub.size.lock().unwrap() = (size.width, size.height);
            events.hub.poke();
            let _ = events.messages.send(Msg::Resize(size.width, size.height));
            if let Some(overlay) = events.app.get_webview(OVERLAY) {
                let scale = events.app.get_window(WINDOW).and_then(|window| window.scale_factor().ok()).unwrap_or(1.0);
                let _ = overlay.set_size(LogicalSize::new(size.width as f64 / scale, BAR));
            }
        }
        WindowEvent::Destroyed => {
            events.stop.store(true, Ordering::Relaxed);
            events.hub.stop.store(true, Ordering::Relaxed);
            events.hub.poke();
            let _ = events.messages.send(Msg::Quit);
            let mut current = CTX.lock().unwrap();
            if current.as_ref().is_some_and(|current| Arc::ptr_eq(current, &events)) {
                *current = None;
            }
        }
        _ => {}
    });

    spawn("native-view", ctx.clone(), poll_view);
    spawn("native-memory", ctx.clone(), sample_memory);
    let requested = Arc::new(Mutex::new(requested));
    // Images come from the page in parallel, each decoded on its own thread
    for _ in 0..6 {
        let requested = requested.clone();
        spawn("native-fetch", ctx.clone(), move |ctx| fetch(ctx, requested));
    }
    sync(ctx);
    Ok(())
}

fn spawn(name: &str, ctx: Arc<Ctx>, work: impl FnOnce(Arc<Ctx>) + Send + 'static) {
    let _ = std::thread::Builder::new().name(name.into()).spawn(move || work(ctx));
}

fn sleep(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Result of `script` in `webview`, as JSON
fn eval(webview: &Webview, script: &str, timeout: Duration) -> Option<String> {
    let (send, receive) = mpsc::channel();
    webview
        .eval_with_callback(script, move |result| {
            let _ = send.send(result);
        })
        .ok()?;
    receive.recv_timeout(timeout).ok()
}

fn eval_value(webview: &Webview, script: &str) -> Option<Value> {
    serde_json::from_str(&eval(webview, script, Duration::from_secs(30))?).ok()
}

/// Waits for the job kept under `key` in the page and reads its result,
/// handed over by the page as it's done where it can be (see `Pushed`),
/// otherwise read from it in pieces
fn read_key(webview: &Webview, key: &str, timeout: Duration) -> Result<Vec<u8>, String> {
    let key_json = serde_json::to_string(key).unwrap();
    let start = Instant::now();
    let mut pushing = pushed().tabs.lock().unwrap().contains(webview.label());
    let (len, binary) = loop {
        if pushing {
            // Asked about now and then all the same, in case it can't be
            // handed over
            if let Some(body) = pushed().wait(key, Instant::now() + Duration::from_millis(250)) {
                let _ = webview.eval(format!("window.__ligmaNative.drop({key_json})"));
                return Ok(body);
            }
        }
        let status = eval_value(webview, &format!("window.__ligmaNative.status({key_json})")).ok_or("the tab didn't answer")?;
        pushing = status["push"].as_bool() == Some(true);
        match status["state"].as_str() {
            Some("done") if status["pushed"].as_bool() == Some(true) => {
                let body = pushed().wait(key, Instant::now() + Duration::from_secs(2)).ok_or("lost on its way")?;
                let _ = webview.eval(format!("window.__ligmaNative.drop({key_json})"));
                return Ok(body);
            }
            Some("done") if status["pushing"].as_bool() == Some(true) => {}
            Some("done") => break (status["len"].as_u64().unwrap_or(0) as usize, status["binary"].as_bool() == Some(true)),
            Some("pending") if start.elapsed() < timeout => {
                if !pushing {
                    sleep(15);
                }
            }
            Some("pending") => return Err("timed out".into()),
            _ => return Err(status["error"].as_str().unwrap_or("failed").to_string()),
        }
    };
    let mut out = String::with_capacity(len);
    let mut at = 0;
    while at < len {
        let end = (at + CHUNK).min(len);
        let piece = eval(webview, &format!("window.__ligmaNative.slice({key_json}, {at}, {end})"), Duration::from_secs(60))
            .ok_or("the tab didn't answer")?;
        let piece: String = serde_json::from_str(&piece).map_err(|error| error.to_string())?;
        out.push_str(&piece);
        at = end;
    }
    let _ = webview.eval(format!("window.__ligmaNative.drop({key_json})"));
    if binary {
        return base64::engine::general_purpose::STANDARD.decode(out).map_err(|error| error.to_string());
    }
    Ok(out.into_bytes())
}

/// Starts job `call` (a `__ligmaNative` method call) and reads its result
fn job(webview: &Webview, call: &str, timeout: Duration) -> Result<String, String> {
    let key = eval_value(webview, &format!("window.__ligmaNative.{call}")).ok_or("the tab didn't answer")?;
    let key = key.as_str().ok_or("no job")?;
    String::from_utf8(read_key(webview, key, timeout)?).map_err(|error| error.to_string())
}

/// Custom effect `id`'s code, as Figma bundles it to run it
fn effect_code(ctx: &Ctx, id: &str) -> Result<String, String> {
    let webview = current(ctx).ok_or("no file open")?;
    let data = job(&webview, &format!("effectCode({})", serde_json::to_string(id).unwrap()), Duration::from_secs(60))?;
    let value: Value = serde_json::from_str(&data).map_err(|error| error.to_string())?;
    value["code"].as_str().map(str::to_string).ok_or_else(|| "no code".into())
}

fn current(ctx: &Ctx) -> Option<Webview> {
    ctx.webview.lock().unwrap().clone()
}

/// Keeps the mirrored scene in step with the active tab's page
fn sync(ctx: Arc<Ctx>) {
    let mut mirrored: Option<(String, String)> = None;
    let mut autoran = false;
    while !ctx.stop.load(Ordering::Relaxed) {
        let Some(file) = crate::tabs::active_file(&ctx.app).filter(|file| file.editor == "design") else {
            set_status("Open a Figma Design file");
            *ctx.webview.lock().unwrap() = None;
            mirrored = None;
            sleep(500);
            continue;
        };
        let ready = eval_value(&file.webview, PAGE_JS);
        match ready.as_ref().and_then(Value::as_str) {
            Some("ready") => {}
            // The tab loaded again, so what was read from it is gone
            Some("installed") => mirrored = None,
            _ => {
                set_status("Waiting for the file to load");
                sleep(100);
                continue;
            }
        }
        *ctx.webview.lock().unwrap() = Some(file.webview.clone());
        let label = file.webview.label().to_string();
        let shared_here = ctx.shared.lock().unwrap().as_ref().is_some_and(|(tab, _)| *tab == label);
        if ready.as_ref().and_then(Value::as_str) == Some("installed") || !shared_here {
            *ctx.shared.lock().unwrap() = None;
            let to = ctx.clone();
            crate::webview2::share(&file.webview, VIEW_SLOTS * 8, move |shared| match shared {
                Some(shared) => *to.shared.lock().unwrap() = Some((label, shared)),
                None => log::info!("native renderer: the tab can't share memory, so its view is asked for"),
            });
        }
        let page = eval_value(&file.webview, "window.__ligmaNative.view()?.page ?? null")
            .and_then(|page| page.as_str().map(str::to_owned));
        let Some(page) = page else {
            set_status("Waiting for the file to load");
            // Checked often, as the page is read the moment Figma has it
            sleep(50);
            continue;
        };
        let key = (file.key.clone(), page);
        if mirrored.as_ref() != Some(&key) || ctx.resync.swap(false, Ordering::Relaxed) {
            if !start_pushing(&file.webview) {
                log::info!("native renderer: the tab can't hand over what it reads, so it's asked for it");
            }
            match snapshot(&ctx, &file.webview) {
                Ok(()) => mirrored = Some(key),
                Err(error) => {
                    set_status(format!("Couldn't read the page: {error}"));
                    sleep(2000);
                    continue;
                }
            }
            if super::autorun() && !autoran {
                autoran = true;
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    // Time to fetch what's in view
                    sleep(20_000);
                    autorun(&ctx);
                });
            }
            continue;
        }
        match changes(&ctx, &file.webview) {
            Ok(true) => ctx.resync.store(true, Ordering::Relaxed),
            Ok(false) => {}
            Err(error) => log::warn!("native renderer changes: {error}"),
        }
        sleep(200);
    }
}

/// Reads the current page, streamed in nearest the view first, each part
/// drawn as it comes
fn snapshot(ctx: &Ctx, webview: &Webview) -> Result<(), String> {
    set_status("Reading the page");
    super::mark(|| "reading the page".into());
    let start = Instant::now();
    let begin = eval_value(webview, "window.__ligmaNative.snapshot()").ok_or("the tab didn't answer")?;
    let begin: Begin = serde_json::from_value(begin).map_err(|error| error.to_string())?;
    let page = begin.page.clone();
    let stream = begin.stream.clone();
    let _ = ctx.messages.send(Msg::Begin(Box::new(Scene::new(begin))));
    let (mut transfer, mut parse, mut bytes, mut first) = (Duration::ZERO, Duration::ZERO, 0usize, None);
    let mut next = 0;
    let in_page = loop {
        if ctx.stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let read_start = Instant::now();
        let json = read_key(webview, &format!("{stream}{next}"), Duration::from_secs(120))?;
        next += 1;
        transfer += read_start.elapsed();
        // The one after the last says it's done, or why it stopped
        if json.len() < 256 && !json.starts_with(b"{\"items\"") {
            let end: Value = serde_json::from_slice(&json).map_err(|error| error.to_string())?;
            match end.get("done") {
                Some(_) => {
                    super::mark(|| format!("page read: {:.0} ms working, {:.0} ms waiting for the page", end["work"].as_f64().unwrap_or(0.0), end["waited"].as_f64().unwrap_or(0.0)));
                    break end["ms"].as_f64().unwrap_or(0.0);
                }
                None => return Err(end["error"].as_str().unwrap_or("failed").to_string()),
            }
        }
        bytes += json.len();
        let parse_start = Instant::now();
        let batch: Batch = serde_json::from_slice(&json).map_err(|error| error.to_string())?;
        let prepared = batch.prepare();
        parse += parse_start.elapsed();
        let _ = ctx.messages.send(Msg::Changes(prepared));
        if first.is_none() {
            super::mark(|| "first part of the page read".into());
            first = Some(start.elapsed());
            stats().lock().unwrap().page = page.clone();
            set_status("Drawing, reading the rest of the page");
        }
    };
    let first = first.unwrap_or_default().as_secs_f64() * 1000.0;
    {
        let mut stats = stats().lock().unwrap();
        stats.first_batch_ms = first;
        stats.snapshot_ms = in_page;
        stats.transfer_ms = transfer.as_secs_f64() * 1000.0;
        stats.parse_ms = parse.as_secs_f64() * 1000.0;
        stats.snapshot_bytes = bytes;
        stats.page = page;
        stats.status = "Drawing".into();
    }
    log::info!(
        "native renderer: read the page, {:.1} MB, first part in {first:.0} ms, all in {:.0} ms ({in_page:.0} ms in the page, {:.0} ms moving it, {:.0} ms parsing)",
        bytes as f64 / 1e6,
        start.elapsed().as_secs_f64() * 1000.0,
        transfer.as_secs_f64() * 1000.0,
        parse.as_secs_f64() * 1000.0,
    );
    Ok(())
}

/// Passes on the page's changes. True if it has to be read again
fn changes(ctx: &Ctx, webview: &Webview) -> Result<bool, String> {
    let result = eval_value(webview, "window.__ligmaNative.changes()").ok_or("the tab didn't answer")?;
    let changes: Changes = match result {
        Value::String(key) => serde_json::from_slice(&read_key(webview, &key, Duration::from_secs(60))?).map_err(|error| error.to_string())?,
        Value::Null => return Ok(false),
        value => serde_json::from_value(value).map_err(|error| error.to_string())?,
    };
    if changes.resync {
        return Ok(true);
    }
    if !changes.updates.is_empty() || !changes.removed.is_empty() {
        let _ = ctx.messages.send(Msg::Changes(changes.prepare()));
    }
    Ok(false)
}

/// Slots of the view the page writes to shared memory (see page.js)
const VIEW_SLOTS: usize = 16;

/// The view the page last wrote to `shared`, with its count of writes and
/// when it was written, in ms since 1970
fn shared_view(shared: crate::webview2::Shared) -> Option<(u64, View, f64)> {
    use std::sync::atomic::{fence, Ordering::Acquire};
    let base = shared.ptr as *const f64;
    for _ in 0..8 {
        // Odd while the page writes it, and different after
        let count = unsafe { std::ptr::read_volatile(base) };
        if count == 0.0 || count % 2.0 == 1.0 {
            std::hint::spin_loop();
            continue;
        }
        fence(Acquire);
        let mut slots = [0.0f64; VIEW_SLOTS];
        for (i, slot) in slots.iter_mut().enumerate() {
            *slot = unsafe { std::ptr::read_volatile(base.add(i)) };
        }
        fence(Acquire);
        if unsafe { std::ptr::read_volatile(base) } != count {
            continue;
        }
        let [_, x, y, w, h, zoom, cx, cy, cw, ch, dpr, visible, a, b, at, ..] = slots;
        let _ = (cx, cy);
        let page = format!("{}:{}", a as i64, b as i64);
        return Some((count as u64, View { x, y, w, h, zoom, cw, ch, dpr, page, visible: visible != 0.0 }, at));
    }
    None
}

/// Follows Figma's viewport: read from memory the tab shares, written every
/// frame it draws, the moment it changes, or else asked for about once a
/// frame
fn poll_view(ctx: Arc<Ctx>) {
    let mut seen = 0;
    while !ctx.stop.load(Ordering::Relaxed) {
        let start = Instant::now();
        let shared = ctx.shared.lock().unwrap().as_ref().map(|(_, shared)| *shared);
        if let Some(shared) = shared {
            if let Some((count, view, at)) = shared_view(shared) {
                if count != seen {
                    seen = count;
                    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64() * 1000.0;
                    super::render::trace(|| format!("view x={:.1} y={:.1} zoom={:.3} {:.1}ms after the page wrote it", view.x, view.y, view.zoom, now - at));
                    send_view(&ctx, view);
                }
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
        }
        if let Some(webview) = current(&ctx) {
            let view = eval_value(&webview, "window.__ligmaNative?.view() ?? null")
                .and_then(|value| serde_json::from_value::<View>(value).ok());
            if let Some(view) = view {
                send_view(&ctx, view);
            }
        }
        let elapsed = start.elapsed();
        std::thread::sleep(Duration::from_millis(8).saturating_sub(elapsed));
    }
}

fn send_view(ctx: &Ctx, view: View) {
    if let Some(recorded) = ctx.recorded.lock().unwrap().as_mut() {
        recorded.push(view.clone());
    }
    ctx.hub.set_view(view.clone());
    let _ = ctx.messages.send(Msg::View(view));
}

fn sample_memory(ctx: Arc<Ctx>) {
    let Some(memory) = GpuMemory::new() else {
        return;
    };
    while !ctx.stop.load(Ordering::Relaxed) {
        let sample = memory.sample();
        {
            let mut stats = stats().lock().unwrap();
            stats.vram_ligma = sample.ligma;
            stats.vram_webview = sample.webview;
        }
        if let Some(peak) = ctx.peak.lock().unwrap().as_mut() {
            peak.ligma = peak.ligma.max(sample.ligma);
            peak.webview = peak.webview.max(sample.webview);
        }
        sleep(500);
    }
}

/// Fetches the images the render thread asks for
fn fetch(ctx: Arc<Ctx>, requests: Arc<Mutex<Receiver<Request>>>) {
    while !ctx.stop.load(Ordering::Relaxed) {
        let request = requests.lock().unwrap().recv_timeout(Duration::from_millis(500));
        let Ok(Request { key, width }) = request else {
            continue;
        };
        let Some(webview) = current(&ctx) else {
            continue;
        };
        // Figma can take a while to render a node, the first time a custom
        // effect runs most of all, so image fills don't wait behind it
        if let Some(id) = key.strip_prefix(RENDER) {
            let (ctx, id) = (ctx.clone(), id.to_string());
            std::thread::spawn(move || match fetch_node(&webview, &id, width) {
                Ok(img) => {
                    let _ = ctx.messages.send(Msg::Image(key, img));
                }
                Err(error) => {
                    log::warn!("native renderer render {key}: {error}");
                    let _ = ctx.messages.send(Msg::Failed(key));
                }
            });
            continue;
        }
        super::mark(|| format!("image {} asked", key.get(..8).unwrap_or(&key)));
        let fetched = fetch_image(&ctx, &webview, &key, width);
        match fetched {
            Ok(img) => {
                let _ = ctx.messages.send(Msg::Image(key, img));
            }
            Err(error) => {
                log::warn!("native renderer image {key}: {error}");
                let _ = ctx.messages.send(Msg::Failed(key));
            }
        }
    }
}

/// Largest side an image is ever kept at. Below it, images are kept at
/// about the size they show at, and decoded again bigger when zoomed into
const IMAGE_LIMIT: u32 = 4096;

/// Image fill `key` (its hash, then `#` and its adjustments if it has
/// any), decoded at about `width` pixels wide
fn fetch_image(ctx: &Ctx, webview: &Webview, key: &str, width: f64) -> Result<Img, String> {
    let (hash, adjust) = match key.split_once('#') {
        Some((hash, adjust)) => (hash, Adjust::parse(adjust)),
        None => (key, None),
    };
    let started = Instant::now();
    let short = hash.get(..8).unwrap_or(hash);
    // In view, so handed over before it was asked for
    let kept = ctx.encoded.lock().unwrap().get(hash).cloned();
    let cached = kept.or_else(|| {
        let bytes = Arc::new(pushed().wait(&format!("i{hash}"), Instant::now())?);
        super::mark(|| format!("image {short} was handed over"));
        ctx.encoded.lock().unwrap().insert(hash.to_string(), bytes.clone());
        Some(bytes)
    });
    let bytes = match cached {
        Some(bytes) => bytes,
        None => {
            let asked = Instant::now();
            let mut started_in_page = Duration::ZERO;
            let mut tries = 0;
            let bytes = loop {
                tries += 1;
                let key = eval_value(webview, &format!("window.__ligmaNative.image({})", serde_json::to_string(hash).unwrap())).ok_or("the tab didn't answer")?;
                started_in_page = started_in_page.max(asked.elapsed());
                let key = key.as_str().ok_or("no job")?.to_string();
                let bytes = read_key(webview, &key, Duration::from_secs(120))?;
                // Handed over under another key while it was asked for
                if !bytes.starts_with(b"{\"under\":") {
                    break bytes;
                }
                let under: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
                let under = under["under"].as_str().ok_or("no key")?;
                // Or taken by another size of it asked for at the same time.
                // Handed over long before it was asked for, it's gone, and
                // the page reads it again
                let found = pushed().wait(under, Instant::now() + Duration::from_secs(5)).or_else(|| ctx.encoded.lock().unwrap().get(hash).map(|bytes| bytes.to_vec()));
                match found {
                    Some(bytes) => break bytes,
                    None if tries < 2 => continue,
                    None => return Err("lost on its way".into()),
                }
            };
            let bytes = Arc::new(bytes);
            super::mark(|| format!("image {short} read, the job started after {} ms, read after {} ms", started_in_page.as_millis(), asked.elapsed().as_millis()));
            ctx.encoded.lock().unwrap().insert(hash.to_string(), bytes.clone());
            bytes
        }
    };
    let fetched = started.elapsed();
    let image = image::load_from_memory(&bytes).map_err(|error| error.to_string())?.to_rgba8();
    let decoded = started.elapsed();
    let original = image.dimensions();
    let (w, h) = (original.0 as f64, original.1 as f64);
    let side = (w.max(h) * (width.max(1.0) * 1.25 / w)).ceil();
    // Kept whole, or halved as many times as it can be and still be big
    // enough, each time by averaging 2 by 2 texels, which is how Figma
    // makes the smaller copies of images it draws from. Any other size
    // would sample differently from Figma
    let mut image = image;
    for _ in 0..halvings(original.0.max(original.1), side) {
        image = halve(&image);
    }
    if let Some(adjust) = adjust {
        adjust.apply(&mut image);
    }
    let (iw, ih) = image.dimensions();
    let info = skia_safe::ImageInfo::new((iw as i32, ih as i32), skia_safe::ColorType::RGBA8888, skia_safe::AlphaType::Unpremul, None);
    let image = skia_safe::images::raster_from_data(&info, skia_safe::Data::new_copy(image.as_raw()), (iw * 4) as usize)
        .ok_or("couldn't make the image")?;
    if super::dev() {
        let ms = |d: Duration| d.as_millis();
        log::info!(
            "native renderer image {hash}: {} KB {}x{} for {width:.0} kept {iw}x{ih} fetched {} ms, decoded {} ms, ready {} ms",
            bytes.len() / 1000,
            original.0,
            original.1,
            ms(fetched),
            ms(decoded - fetched),
            ms(started.elapsed())
        );
    }
    Ok(Img { image, original, area: None, stale: false })
}

/// Smallest an image is kept at. Drawn with mipmaps, a copy halved down to
/// it samples as the GPU's own smaller copies of a bigger one would, and
/// far out hundreds of images show at a few pixels each
const IMAGE_FLOOR: f64 = 64.0;

/// Times an image `longest` pixels on its longer side is halved to still
/// be `side` pixels, no less than `IMAGE_FLOOR`, and no more than
/// `IMAGE_LIMIT`
pub(super) fn halvings(longest: u32, side: f64) -> u32 {
    let mut k = 0;
    while k < 16 && ((longest >> (k + 1)) as f64 >= side.max(IMAGE_FLOOR) || (longest >> k) > IMAGE_LIMIT) {
        k += 1;
    }
    k
}

/// `image` half the size, each pixel the mean of 2 by 2, weighted by alpha
fn halve(image: &image::RgbaImage) -> image::RgbaImage {
    let (w, h) = ((image.width() / 2).max(1), (image.height() / 2).max(1));
    image::RgbaImage::from_fn(w, h, |x, y| {
        let mut sum = [0u32; 4];
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let p = image.get_pixel((x * 2 + dx).min(image.width() - 1), (y * 2 + dy).min(image.height() - 1));
            let a = p[3] as u32;
            for c in 0..3 {
                sum[c] += p[c] as u32 * a;
            }
            sum[3] += a;
        }
        if sum[3] == 0 {
            return image::Rgba([0, 0, 0, 0]);
        }
        let color = |c: usize| ((sum[c] + sum[3] / 2) / sum[3]) as u8;
        image::Rgba([color(0), color(1), color(2), ((sum[3] + 2) / 4) as u8])
    })
}

/// Node `id` as Figma renders it, about `width` pixels wide
fn fetch_node(webview: &Webview, id: &str, width: f64) -> Result<Img, String> {
    let call = format!("render({}, {}, {RENDER_LIMIT})", serde_json::to_string(id).unwrap(), width.max(1.0));
    let data = job(webview, &call, Duration::from_secs(180))?;
    let value: Value = serde_json::from_str(&data).map_err(|error| error.to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value["png"].as_str().ok_or("no image")?)
        .map_err(|error| error.to_string())?;
    let rb: [f32; 4] = serde_json::from_value(value["rb"].clone()).map_err(|_| "no bounds")?;
    let scale = value["scale"].as_f64().ok_or("no scale")? as f32;
    let image = image::load_from_memory(&bytes).map_err(|error| error.to_string())?.to_rgba8();
    let (iw, ih) = image.dimensions();
    let info = skia_safe::ImageInfo::new((iw as i32, ih as i32), skia_safe::ColorType::RGBA8888, skia_safe::AlphaType::Unpremul, None);
    let image = skia_safe::images::raster_from_data(&info, skia_safe::Data::new_copy(image.as_raw()), (iw * 4) as usize)
        .ok_or("couldn't make the image")?;
    // Figma's export starts at the render bounds and rounds its size up
    let area = Rect::from_xywh(rb[0], rb[1], iw as f32 / scale, ih as f32 / scale);
    Ok(Img { image, original: (iw, ih), area: Some(area), stale: false })
}

/// Figma's render of node `id` at `scale`, and the bounds it covers
fn fetch_render(webview: &Webview, id: &str, scale: f64) -> Result<(image::RgbaImage, Rect), String> {
    let data = job(webview, &format!("exportNode({}, {scale})", serde_json::to_string(id).unwrap()), Duration::from_secs(120))?;
    let value: Value = serde_json::from_str(&data).map_err(|error| error.to_string())?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value["png"].as_str().ok_or("no image")?)
        .map_err(|error| error.to_string())?;
    let rb: [f32; 4] = serde_json::from_value(value["rb"].clone()).map_err(|_| "no bounds")?;
    let image = image::load_from_memory(&bytes).map_err(|error| error.to_string())?.to_rgba8();
    Ok((image, Rect::from_xywh(rb[0], rb[1], rb[2], rb[3])))
}

/// Node `id` as Figma renders it and as drawn natively, at the same size
struct Pair {
    kind: String,
    figma: image::RgbaImage,
    native: Offscreen,
}

/// Renders node `id` through Figma and natively, at up to `side` pixels
/// on its longer side
/// At `scale`, or so its longest side is `side` pixels
fn render_pair(ctx: &Ctx, webview: &Webview, id: &str, side: f64, scale: Option<f64>) -> Result<Pair, String> {
    let info = eval_value(
        webview,
        &format!(
            "(() => {{ const n = figma.getNodeById({}); const b = n?.absoluteRenderBounds ?? n?.absoluteBoundingBox; return b ? [b.x, b.y, b.width, b.height, n.type, !!n.effects?.some((e) => e.type === 'BACKGROUND_BLUR' && e.visible !== false)] : null }})()",
            serde_json::to_string(id).unwrap()
        ),
    );
    let info = info.and_then(|info| info.as_array().cloned()).ok_or("no such node")?;
    let longest = info[2].as_f64().unwrap_or(1.0).max(info[3].as_f64().unwrap_or(1.0));
    let scale = scale.unwrap_or((side / longest).min(2.0));
    let (figma, bounds) = fetch_render(webview, id, scale)?;
    // Figma's export of a node that blurs what's behind it has what's
    // behind it in it
    let backdrop = info[5].as_bool().unwrap_or(false);
    let (width, height) = figma.dimensions();
    // Figma's export starts at the render bounds and rounds its size up, so
    // it covers a little more than them, at exactly `scale`
    let bounds = Rect::from_xywh(bounds.left, bounds.top, width as f32 / scale as f32, height as f32 / scale as f32);
    // Again until the images it uses have come
    let mut drawn = None;
    for _ in 0..30 {
        let (send, receive) = mpsc::channel();
        let _ = ctx.messages.send(Msg::Offscreen { id: id.to_string(), bounds, width, height, backdrop, reply: send });
        drawn = receive.recv_timeout(Duration::from_secs(60)).ok().flatten();
        if drawn.as_ref().is_none_or(|drawn| drawn.waiting == 0) {
            break;
        }
        sleep(500);
    }
    let native = drawn.ok_or("not drawn")?;
    Ok(Pair { kind: info[4].as_str().unwrap_or("").into(), figma, native })
}

/// Writes node `id` as Figma renders it, as drawn natively, and where they
/// differ, to a folder in Ligma's log folder
/// `id`, or `id@scale`
fn compare_node(ctx: &Ctx, id: &str) -> Result<PathBuf, String> {
    let webview = current(ctx).ok_or("no file")?;
    let (id, scale) = match id.split_once('@') {
        Some((id, scale)) => (id, scale.parse().ok()),
        None => (id, None),
    };
    let pair = render_pair(ctx, &webview, id, 1600.0, scale)?;
    let (width, height) = pair.figma.dimensions();
    let dir = report_dir(ctx, &format!("compare-{}", id.replace([':', ';'], "-")))?;
    let (matched, mean_error, diff) = compare(pair.figma.as_raw(), &pair.native.pixels, width, height);
    let _ = pair.figma.save(dir.join("figma.png"));
    if let Some(native) = image::RgbaImage::from_raw(width, height, pair.native.pixels) {
        let _ = native.save(dir.join("native.png"));
    }
    if let Some(diff) = image::RgbaImage::from_raw(width, height, diff) {
        let _ = diff.save(dir.join("diff.png"));
    }
    let report = json!({
        "id": id,
        "kind": pair.kind,
        "width": width,
        "height": height,
        "matched": matched,
        "meanError": mean_error,
        "approximateReason": pair.native.approximate_reason,
    });
    std::fs::write(dir.join("report.json"), serde_json::to_string_pretty(&report).unwrap()).map_err(|error| error.to_string())?;
    Ok(dir)
}

pub fn action(app: &AppHandle, action: &str) -> Result<String, String> {
    let ctx = CTX.lock().unwrap().clone().ok_or("not running")?;
    let _ = app;
    match action {
        "tint" | "untint" => {
            let _ = ctx.messages.send(Msg::Tint(action == "tint"));
            Ok(String::new())
        }
        _ if action.starts_with("audit:") && super::dev() => {
            let report = audit(&ctx, &action["audit:".len()..])?;
            Ok(report.to_string())
        }
        "rastercheck" if super::dev() => {
            // The view drawn with units drawn into images of their own and
            // without, which should come out the same
            let view = ctx.hub.view.lock().unwrap().clone().ok_or("no view")?;
            let (width, height) = ((view.cw * view.dpr).round() as u32, (view.ch * view.dpr).round() as u32);
            let render = |on: bool| -> Result<Vec<u8>, String> {
                let _ = ctx.messages.send(Msg::Rasters(on));
                let mut drawn = None;
                for _ in 0..60 {
                    let (send, receive) = mpsc::channel();
                    let _ = ctx.messages.send(Msg::Render { view: view.clone(), width, height, reply: send });
                    drawn = receive.recv_timeout(Duration::from_secs(120)).ok().flatten();
                    if drawn.as_ref().is_none_or(|(_, waiting)| *waiting == 0) {
                        break;
                    }
                    sleep(500);
                }
                drawn.map(|(pixels, _)| pixels).ok_or_else(|| "not drawn".to_string())
            };
            let started = Instant::now();
            let with = render(true)?;
            let with_ms = started.elapsed().as_millis();
            let started = Instant::now();
            let without = render(false)?;
            let without_ms = started.elapsed().as_millis();
            let again = render(true)?;
            let compare = |a: &[u8], b: &[u8]| {
                let (mut differ, mut worst) = (0u64, 0i32);
                for (a, b) in a.chunks(4).zip(b.chunks(4)) {
                    let error = (0..4).map(|c| (a[c] as i32 - b[c] as i32).abs()).max().unwrap_or(0);
                    worst = worst.max(error);
                    if error > 0 {
                        differ += 1;
                    }
                }
                (differ, worst)
            };
            let (differ, worst) = compare(&with, &without);
            // What differs between two draws alone, as Figma's renders of
            // some nodes come at other sizes
            let (noise, noise_worst) = compare(&with, &again);
            let dir = report_dir(&ctx, "rastercheck")?;
            if let (Some(a), Some(b)) = (image::RgbaImage::from_raw(width, height, with), image::RgbaImage::from_raw(width, height, without)) {
                let _ = a.save(dir.join("with.png"));
                let _ = b.save(dir.join("without.png"));
            }
            if let Some(c) = image::RgbaImage::from_raw(width, height, again.clone()) {
                let _ = c.save(dir.join("again.png"));
            }
            Ok(json!({ "differ": differ, "worst": worst, "noise": noise, "noiseWorst": noise_worst, "pixels": width as u64 * height as u64, "withMs": with_ms, "withoutMs": without_ms, "dir": dir }).to_string())
        }
        "live" if super::dev() => {
            let (send, receive) = mpsc::channel();
            let _ = ctx.messages.send(Msg::Live(send));
            let (shown, fresh, width, height) =
                receive.recv_timeout(Duration::from_secs(120)).ok().flatten().ok_or("the renderer didn't answer")?;
            let shown = image::RgbaImage::from_raw(width, height, shown).ok_or("bad size")?;
            let fresh = image::RgbaImage::from_raw(width, height, fresh).ok_or("bad size")?;
            let dir = report_dir(&ctx, "live")?;
            let mut diff = image::RgbaImage::new(width, height);
            let mut differ = 0u64;
            for ((a, b), out) in shown.pixels().zip(fresh.pixels()).zip(diff.pixels_mut()) {
                let error = (0..3).map(|c| (a[c] as i32 - b[c] as i32).abs()).max().unwrap_or(0);
                if error > AUDIT_TOLERANCE {
                    differ += 1;
                }
                *out = if error > AUDIT_TOLERANCE { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([a[0] / 3, a[1] / 3, a[2] / 3, 255]) };
            }
            let _ = shown.save(dir.join("shown.png"));
            let _ = fresh.save(dir.join("fresh.png"));
            let _ = diff.save(dir.join("diff.png"));
            Ok(json!({ "dir": dir, "differ": differ, "pixels": width as u64 * height as u64 }).to_string())
        }
        _ if action.starts_with("profile:") && super::dev() => {
            let parts: Vec<&str> = action["profile:".len()..].split(',').collect();
            let [id, x, y, px, ss] = parts[..] else { return Err("profile:id,x,y,px,ss".into()) };
            let num = |v: &str| v.parse::<f32>().map_err(|e| e.to_string());
            let (send, receive) = mpsc::channel();
            let _ = ctx.messages.send(Msg::Profile { id: id.to_string(), x: num(x)?, y: num(y)?, px: num(px)?, ss: num(ss)? as i32, reply: send });
            receive.recv_timeout(Duration::from_secs(120)).map_err(|_| "the renderer didn't answer".to_string())
        }
        _ if action.starts_with("shown:") && super::dev() => {
            let (send, receive) = mpsc::channel();
            let _ = ctx.messages.send(Msg::Shown(send));
            let (pixels, width, height) = receive.recv_timeout(Duration::from_secs(30)).ok().flatten().ok_or("the renderer didn't answer")?;
            let shown = image::RgbaImage::from_raw(width, height, pixels).ok_or("bad size")?;
            shown.save(&action["shown:".len()..]).map_err(|error| error.to_string())?;
            Ok(String::new())
        }
        "purge" if super::dev() => {
            let (send, receive) = mpsc::channel();
            let _ = ctx.messages.send(Msg::Purge(send));
            let [before, purged, freed] = receive.recv_timeout(Duration::from_secs(10)).map_err(|_| "the renderer didn't answer")?;
            Ok(format!("{} MB, {} MB purged, {} MB freed", before / 1_000_000, purged / 1_000_000, freed / 1_000_000))
        }
        "census" if super::dev() => Ok(super::gpu_census::report()),
        "holes" if super::dev() => {
            let mut stats = stats().lock().unwrap();
            let longest = stats.longest_hole_ms;
            stats.longest_hole_ms = 0.0;
            Ok(format!("{longest:.0}"))
        }
        "resync" => {
            ctx.resync.store(true, Ordering::Relaxed);
            Ok(String::new())
        }
        _ if action.starts_with("compare") && super::dev() => {
            let id = match action.strip_prefix("compare:") {
                Some(id) => id.to_string(),
                None => {
                    let webview = current(&ctx).ok_or("no file")?;
                    let id = eval_value(&webview, "figma.currentPage.selection[0]?.id ?? null");
                    id.and_then(|id| id.as_str().map(str::to_owned)).ok_or("select a layer first")?
                }
            };
            let shown = compare_node(&ctx, &id)?.display().to_string();
            stats().lock().unwrap().report = Some(shown.clone());
            Ok(shown)
        }
        "parity" | "bench" if super::dev() => {
            if ctx.busy.swap(true, Ordering::Relaxed) {
                return Err("already running".into());
            }
            let result = if action == "parity" { parity(&ctx, 60) } else { bench(&ctx) };
            ctx.busy.store(false, Ordering::Relaxed);
            let path = result?;
            let shown = path.display().to_string();
            stats().lock().unwrap().report = Some(shown.clone());
            Ok(shown)
        }
        // Up to n of each thing Figma draws that the page has
        _ if super::dev() && action.starts_with("features:") => {
            let each: usize = action["features:".len()..].parse().map_err(|_| "features:<how many of each>")?;
            if ctx.busy.swap(true, Ordering::Relaxed) {
                return Err("already running".into());
            }
            let result = parity_by_feature(&ctx, each);
            ctx.busy.store(false, Ordering::Relaxed);
            let path = result?;
            let shown = path.display().to_string();
            stats().lock().unwrap().report = Some(shown.clone());
            Ok(shown)
        }
        _ => Err("unknown action".into()),
    }
}

fn report_dir(ctx: &Ctx, name: &str) -> Result<PathBuf, String> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |time| time.as_secs());
    let dir = ctx.app.path().app_log_dir().map_err(|error| error.to_string())?.join("native-renderer").join(format!("{name}-{stamp}"));
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct NodeScore {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    feature: Option<String>,
    kind: String,
    width: u32,
    height: u32,
    /// Share of pixels within 16 of 255 on every channel, over white
    matched: f64,
    /// The same after blurring both by a pixel, which forgives how images
    /// and edges are resampled
    matched_soft: f64,
    /// Mean difference per channel, out of 255
    mean_error: f64,
    /// Why something in it is only drawn close to Figma's look, if it is
    approximate_reason: Option<String>,
}

/// Exports `count` nodes through Figma, draws the same nodes natively,
/// and scores each pair. Writes `report.json` and the images to a folder
/// in Ligma's log folder
fn parity(ctx: &Ctx, count: usize) -> Result<PathBuf, String> {
    let webview = current(ctx).ok_or("no file")?;
    set_status("Parity: picking nodes");
    let ids: Vec<String> = serde_json::from_str(&job(&webview, &format!("sample({count})"), Duration::from_secs(120))?)
        .map_err(|error| error.to_string())?;
    let picked = ids.into_iter().map(|id| (id, None)).collect();
    score_nodes(ctx, &webview, picked, "parity")
}

/// Parity for up to `each` nodes with each thing Figma draws that the page
/// has, scored by feature as well
fn parity_by_feature(ctx: &Ctx, each: usize) -> Result<PathBuf, String> {
    let webview = current(ctx).ok_or("no file")?;
    set_status("Parity: finding what the page uses");
    #[derive(serde::Deserialize)]
    struct Picked {
        id: String,
        feature: String,
    }
    let picked: Vec<Picked> = serde_json::from_str(&job(&webview, &format!("features({each})"), Duration::from_secs(300))?)
        .map_err(|error| error.to_string())?;
    score_nodes(ctx, &webview, picked.into_iter().map(|picked| (picked.id, Some(picked.feature))).collect(), "features")
}

fn score_nodes(ctx: &Ctx, webview: &Webview, picked: Vec<(String, Option<String>)>, name: &str) -> Result<PathBuf, String> {
    let dir = report_dir(ctx, name)?;
    let mut scores: Vec<NodeScore> = Vec::new();
    let mut done: HashMap<String, usize> = HashMap::new();
    for (i, (id, feature)) in picked.iter().enumerate() {
        // A node with several features is drawn once
        if let Some(&at) = done.get(id) {
            let mut again: NodeScore = scores[at].clone();
            again.feature = feature.clone();
            scores.push(again);
            continue;
        }
        set_status(format!("Parity: node {} of {}", i + 1, picked.len()));
        let Ok(Pair { kind, figma, native }) = render_pair(ctx, &webview, id, 1024.0, None) else {
            continue;
        };
        let (width, height) = figma.dimensions();
        let Offscreen { pixels: native, approximate_reason, .. } = native;
        let (matched, mean_error, diff) = compare(figma.as_raw(), &native, width, height);
        let matched_soft = compare(&blur(figma.as_raw(), width, height), &blur(&native, width, height), width, height).0;
        let _ = figma.save(dir.join(format!("{i:03}-figma.png")));
        if let Some(native) = image::RgbaImage::from_raw(width, height, native) {
            let _ = native.save(dir.join(format!("{i:03}-native.png")));
        }
        if let Some(diff) = image::RgbaImage::from_raw(width, height, diff) {
            let _ = diff.save(dir.join(format!("{i:03}-diff.png")));
        }
        done.insert(id.clone(), scores.len());
        scores.push(NodeScore { id: id.clone(), feature: feature.clone(), kind, width, height, matched, matched_soft, mean_error, approximate_reason });
    }
    let summarize = |scores: &[&NodeScore]| {
        let mut matched: Vec<f64> = scores.iter().map(|score| score.matched).collect();
        matched.sort_by(f64::total_cmp);
        let mean = matched.iter().sum::<f64>() / matched.len().max(1) as f64;
        let error = scores.iter().map(|score| score.mean_error).sum::<f64>() / scores.len().max(1) as f64;
        let mut soft: Vec<f64> = scores.iter().map(|score| score.matched_soft).collect();
        soft.sort_by(f64::total_cmp);
        json!({
            "nodes": scores.len(),
            "meanMatchedSoft": soft.iter().sum::<f64>() / soft.len().max(1) as f64,
            "medianMatchedSoft": percentile(&soft, 0.5),
            "meanMatched": mean,
            "medianMatched": percentile(&matched, 0.5),
            "atLeast99": matched.iter().filter(|&&m| m >= 0.99).count(),
            "atLeast95": matched.iter().filter(|&&m| m >= 0.95).count(),
            "meanError": error,
        })
    };
    let all: Vec<&NodeScore> = scores.iter().collect();
    let exact: Vec<&NodeScore> = scores.iter().filter(|score| score.approximate_reason.is_none()).collect();
    let mut features: Vec<&str> = scores.iter().filter_map(|score| score.feature.as_deref()).collect();
    features.sort_unstable();
    features.dedup();
    let by_feature: serde_json::Map<String, Value> = features
        .iter()
        .map(|&feature| {
            let these: Vec<&NodeScore> = scores.iter().filter(|score| score.feature.as_deref() == Some(feature)).collect();
            (feature.to_string(), summarize(&these))
        })
        .collect();
    let report = json!({
        "all": summarize(&all),
        "noApproximations": summarize(&exact),
        "byFeature": by_feature,
        "nodes": scores,
    });
    std::fs::write(dir.join("report.json"), serde_json::to_string_pretty(&report).unwrap()).map_err(|error| error.to_string())?;
    log::info!("native renderer parity: {}", report["all"]);
    set_status("Drawing");
    Ok(dir)
}

#[derive(serde::Deserialize)]
struct AuditSpec {
    view: View,
    /// A screenshot of Figma's page, a PNG at device pixels, and where its
    /// canvas is in it
    figma: String,
    crop: [u32; 4],
    name: String,
}


/// A difference found by `audit`, in pixels of the view
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Region {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    /// Absolute bounds of the same area
    world: [f32; 4],
    /// Pixels in it differing by more than `AUDIT_TOLERANCE`
    pixels: usize,
    mean_error: f64,
    max_error: i32,
    /// The nodes under its middle, from the top-level one down
    nodes: Vec<String>,
    image: String,
}

/// A pixel differs when a channel is off by more than this
const AUDIT_TOLERANCE: i32 = 6;

/// Draws the view a screenshot of Figma's canvas shows, at the same size,
/// compares the two, and writes both, where they differ, and each region
/// that differs, side by side, to a folder in Ligma's log folder. Returns
/// the report
fn audit(ctx: &Ctx, spec: &str) -> Result<Value, String> {
    let spec: AuditSpec = serde_json::from_str(spec).map_err(|error| error.to_string())?;
    let page = image::open(&spec.figma).map_err(|error| error.to_string())?.to_rgba8();
    let [cx, cy, cw, ch] = spec.crop;
    let cw = cw.min(page.width().saturating_sub(cx));
    let ch = ch.min(page.height().saturating_sub(cy));
    let figma = image::imageops::crop_imm(&page, cx, cy, cw, ch).to_image();
    let (width, height) = figma.dimensions();
    let mut view = spec.view;
    let px = view.zoom * view.dpr;
    view.w = width as f64 / px;
    view.h = height as f64 / px;
    let mut drawn = None;
    for _ in 0..120 {
        let (send, receive) = mpsc::channel();
        let _ = ctx.messages.send(Msg::Render { view: view.clone(), width, height, reply: send });
        drawn = receive.recv_timeout(Duration::from_secs(120)).ok().flatten();
        if drawn.as_ref().is_none_or(|(_, waiting)| *waiting == 0) {
            break;
        }
        sleep(500);
    }
    let (native, waiting) = drawn.ok_or("not drawn")?;
    let native = image::RgbaImage::from_raw(width, height, native).ok_or("bad size")?;
    let dir = report_dir(ctx, &format!("audit-{}", spec.name))?;
    let _ = figma.save(dir.join("figma.png"));
    let _ = native.save(dir.join("native.png"));

    // Worst channel difference per pixel
    let error: Vec<i32> = figma
        .pixels()
        .zip(native.pixels())
        .map(|(a, b)| (0..3).map(|c| (a[c] as i32 - b[c] as i32).abs()).max().unwrap_or(0))
        .collect();
    let mut diff = image::RgbaImage::new(width, height);
    for (i, pixel) in diff.pixels_mut().enumerate() {
        let (x, y) = (i as u32 % width, i as u32 / width);
        let grey = (figma.get_pixel(x, y).0.iter().take(3).map(|&v| v as u32).sum::<u32>() / 9 + 170) as u8;
        *pixel = if error[i] > AUDIT_TOLERANCE { image::Rgba([255, (255 - error[i].min(255)) as u8 / 2, 0, 255]) } else { image::Rgba([grey, grey, grey, 255]) };
    }
    let _ = diff.save(dir.join("diff.png"));

    // Blocks of 8 pixels with differences, joined into regions
    const BLOCK: u32 = 8;
    let (bw, bh) = (width.div_ceil(BLOCK), height.div_ceil(BLOCK));
    let mut bad = vec![false; (bw * bh) as usize];
    for by in 0..bh {
        for bx in 0..bw {
            let mut count = 0;
            for y in by * BLOCK..((by + 1) * BLOCK).min(height) {
                for x in bx * BLOCK..((bx + 1) * BLOCK).min(width) {
                    if error[(y * width + x) as usize] > AUDIT_TOLERANCE {
                        count += 1;
                    }
                }
            }
            bad[(by * bw + bx) as usize] = count >= 2;
        }
    }
    let mut seen = vec![false; bad.len()];
    let mut regions = Vec::new();
    for start in 0..bad.len() {
        if !bad[start] || seen[start] {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(block) = stack.pop() {
            let (bx, by) = (block as u32 % bw, block as u32 / bw);
            (x0, y0, x1, y1) = (x0.min(bx), y0.min(by), x1.max(bx), y1.max(by));
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let (nx, ny) = (bx as i32 + dx, by as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= bw as i32 || ny >= bh as i32 {
                        continue;
                    }
                    let next = (ny as u32 * bw + nx as u32) as usize;
                    if bad[next] && !seen[next] {
                        seen[next] = true;
                        stack.push(next);
                    }
                }
            }
        }
        let (x, y) = (x0 * BLOCK, y0 * BLOCK);
        let (w, h) = (((x1 + 1) * BLOCK).min(width) - x, ((y1 + 1) * BLOCK).min(height) - y);
        let mut pixels = 0;
        let (mut sum, mut max) = (0i64, 0);
        for yy in y..y + h {
            for xx in x..x + w {
                let e = error[(yy * width + xx) as usize];
                if e > AUDIT_TOLERANCE {
                    pixels += 1;
                    sum += e as i64;
                    max = max.max(e);
                }
            }
        }
        regions.push((x, y, w, h, pixels, sum as f64 / pixels.max(1) as f64, max));
    }
    regions.sort_by_key(|region| std::cmp::Reverse(region.4));
    let world = |x: u32, y: u32| ((view.x + x as f64 / px) as f32, (view.y + y as f64 / px) as f32);
    let middles: Vec<(f32, f32)> = regions.iter().map(|&(x, y, w, h, ..)| world(x + w / 2, y + h / 2)).collect();
    let (send, receive) = mpsc::channel();
    let _ = ctx.messages.send(Msg::HitTest { points: middles, reply: send });
    let hits = receive.recv_timeout(Duration::from_secs(10)).unwrap_or_default();
    let mut found = Vec::new();
    for (i, &(x, y, w, h, pixels, mean_error, max_error)) in regions.iter().enumerate() {
        let image = if i < 40 {
            let name = format!("region-{i:02}.png");
            let pad = 24;
            let (cx, cy) = (x.saturating_sub(pad), y.saturating_sub(pad));
            let (cw, ch) = ((w + pad * 2).min(width - cx), (h + pad * 2).min(height - cy));
            let zoom = (480 / cw.max(ch)).clamp(1, 8);
            let mut strip = image::RgbaImage::from_pixel((cw * 3 + 8) * zoom, ch * zoom, image::Rgba([255, 0, 255, 255]));
            for (k, source) in [&figma, &native, &diff].into_iter().enumerate() {
                let crop = image::imageops::crop_imm(source, cx, cy, cw, ch).to_image();
                let crop = image::imageops::resize(&crop, cw * zoom, ch * zoom, image::imageops::FilterType::Nearest);
                image::imageops::replace(&mut strip, &crop, (k as u32 * (cw + 4) * zoom) as i64, 0);
            }
            let _ = strip.save(dir.join(&name));
            name
        } else {
            String::new()
        };
        let (wx, wy) = world(x, y);
        found.push(Region {
            x,
            y,
            width: w,
            height: h,
            world: [wx, wy, (w as f64 / px) as f32, (h as f64 / px) as f32],
            pixels,
            mean_error,
            max_error,
            nodes: hits.get(i).cloned().unwrap_or_default(),
            image,
        });
    }
    let total = (width * height) as f64;
    let differing = error.iter().filter(|&&e| e > AUDIT_TOLERANCE).count();
    let report = json!({
        "name": spec.name,
        "dir": dir.display().to_string(),
        "view": view,
        "width": width,
        "height": height,
        "matched": 1.0 - differing as f64 / total,
        "meanError": error.iter().map(|&e| e as f64).sum::<f64>() / total,
        "exact": error.iter().filter(|&&e| e == 0).count() as f64 / total,
        "imagesWaiting": waiting,
        "regions": found,
    });
    std::fs::write(dir.join("report.json"), serde_json::to_string_pretty(&report).unwrap()).map_err(|error| error.to_string())?;
    Ok(report)
}

/// `rgba` blurred by a 3 by 3 box
fn blur(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as i64, height as i64);
    let mut out = vec![0u8; rgba.len()];
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0u32; 4];
            let mut n = 0;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (sx, sy) = (x + dx, y + dy);
                    if sx < 0 || sy < 0 || sx >= w || sy >= h {
                        continue;
                    }
                    let i = ((sy * w + sx) * 4) as usize;
                    for c in 0..4 {
                        sum[c] += rgba[i + c] as u32;
                    }
                    n += 1;
                }
            }
            let i = ((y * w + x) * 4) as usize;
            for c in 0..4 {
                out[i + c] = (sum[c] / n) as u8;
            }
        }
    }
    out
}

/// Pixels within this of each other on every channel count as matching
const TOLERANCE: i32 = 16;

/// Share of matching pixels, mean error per channel, and a difference image
fn compare(a: &[u8], b: &[u8], width: u32, height: u32) -> (f64, f64, Vec<u8>) {
    let pixels = (width * height) as usize;
    let mut matched = 0usize;
    let mut total = 0u64;
    let mut diff = Vec::with_capacity(pixels * 4);
    // Over white, so transparency differences count as color differences
    let over_white = |p: &[u8], i: usize| -> [i32; 3] {
        let alpha = p[i + 3] as i32;
        [0, 1, 2].map(|c| (p[i + c] as i32 * alpha + 255 * (255 - alpha)) / 255)
    };
    for i in (0..pixels * 4).step_by(4) {
        let (x, y) = (over_white(a, i), over_white(b, i));
        let d = [0, 1, 2].map(|c| (x[c] - y[c]).abs());
        let worst = d[0].max(d[1]).max(d[2]);
        if worst <= TOLERANCE {
            matched += 1;
        }
        total += (d[0] + d[1] + d[2]) as u64;
        let grey = ((x[0] + x[1] + x[2]) / 3 / 3 + 170) as u8;
        if worst <= TOLERANCE {
            diff.extend_from_slice(&[grey, grey, grey, 255]);
        } else {
            diff.extend_from_slice(&[255, (255 - worst.min(255)) as u8 / 2, 0, 255]);
        }
    }
    (matched as f64 / pixels.max(1) as f64, total as f64 / (pixels.max(1) * 3) as f64, diff)
}

/// Pans and zooms Figma's canvas for 6 seconds, timing Figma's frames and
/// GPU memory, then draws the same views natively as fast as it can
fn bench(ctx: &Ctx) -> Result<PathBuf, String> {
    let webview = current(ctx).ok_or("no file")?;
    set_status("Benchmark: Figma");
    let memory = GpuMemory::new();
    let before = memory.as_ref().map(GpuMemory::sample).unwrap_or_default();
    *ctx.recorded.lock().unwrap() = Some(Vec::new());
    *ctx.peak.lock().unwrap() = Some(before);
    stats().lock().unwrap().log = Some(Vec::new());
    let figma = job(&webview, "bench(6000)", Duration::from_secs(60));
    let views = ctx.recorded.lock().unwrap().take().unwrap_or_default();
    let peak = ctx.peak.lock().unwrap().take().unwrap_or_default();
    let following = stats().lock().unwrap().log.take().unwrap_or_default();
    let figma: Value = serde_json::from_str(&figma?).map_err(|error| error.to_string())?;
    let mut intervals: Vec<f64> = serde_json::from_value(figma["intervals"].clone()).unwrap_or_default();
    intervals.sort_by(f64::total_cmp);

    set_status("Benchmark: native");
    let (send, receive) = mpsc::channel();
    let _ = ctx.messages.send(Msg::Replay { views: views.clone(), reply: send });
    let replay = receive.recv_timeout(Duration::from_secs(300)).map_err(|_| "the renderer didn't answer")?;
    let mut cold: Vec<f64> = replay.iter().map(|&(cold, _)| cold).collect();
    let mut warm: Vec<f64> = replay.iter().map(|&(_, warm)| warm).collect();
    cold.sort_by(f64::total_cmp);
    warm.sort_by(f64::total_cmp);
    let after = memory.as_ref().map(GpuMemory::sample).unwrap_or_default();
    let mut following_sorted = following.clone();
    following_sorted.sort_by(f64::total_cmp);

    let summary = |sorted: &[f64]| {
        json!({
            "frames": sorted.len(),
            "mean": sorted.iter().sum::<f64>() / sorted.len().max(1) as f64,
            "p50": percentile(sorted, 0.5),
            "p95": percentile(sorted, 0.95),
            "p99": percentile(sorted, 0.99),
            "max": sorted.last().copied().unwrap_or(0.0),
        })
    };
    let s = stats().lock().unwrap().clone();
    let report = json!({
        "figmaFrameIntervalsMs": summary(&intervals),
        "figmaLongFrames": intervals.iter().filter(|&&ms| ms > 25.0).count(),
        "nativeReplayColdFrameMs": summary(&cold),
        "nativeReplayWarmFrameMs": summary(&warm),
        "replaySize": views.first().map(|view| [(view.cw * view.dpr).round(), (view.ch * view.dpr).round()]),
        "nativeFollowingFrameMs": summary(&following_sorted),
        "viewsRecorded": views.len(),
        "vramBytes": {
            "before": before,
            "peakDuringFigmaPan": peak,
            "after": after,
        },
        "scene": {
            "nodes": s.nodes,
            "lastFrame": { "native": s.native, "approximate": s.approximate, "skipped": s.skipped },
            "images": s.images,
            "imageBytes": s.image_bytes,
            "tiles": s.tiles,
            "tileBytes": s.tile_bytes,
            "skiaCacheBytes": s.cache_bytes,
            "firstBatchMs": s.first_batch_ms,
            "snapshotMs": s.snapshot_ms,
            "transferMs": s.transfer_ms,
            "parseMs": s.parse_ms,
            "snapshotBytes": s.snapshot_bytes,
        },
        "adapter": s.adapter,
        "windowViews": views.first(),
    });
    let dir = report_dir(ctx, "bench")?;
    std::fs::write(dir.join("report.json"), serde_json::to_string_pretty(&report).unwrap()).map_err(|error| error.to_string())?;
    log::info!("native renderer bench: {report}");
    set_status("Drawing");
    Ok(dir)
}

fn autorun(ctx: &Arc<Ctx>) {
    ctx.busy.store(true, Ordering::Relaxed);
    let parity = parity(ctx, 60);
    let bench = bench(ctx);
    ctx.busy.store(false, Ordering::Relaxed);
    let summary = json!({
        "parity": parity.map(|path| path.display().to_string()).map_err(|error| error.to_string()),
        "bench": bench.map(|path| path.display().to_string()).map_err(|error| error.to_string()),
    });
    if let Ok(dir) = ctx.app.path().app_log_dir() {
        let _ = std::fs::write(dir.join("native-renderer").join("autorun.json"), summary.to_string());
    }
    log::info!("native renderer autorun: {summary}");
}
