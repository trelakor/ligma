//! Figma tabs get no IPC, so the image bar's Remove background, Vectorize,
//! Select object, Spot healing and Upscale (see ai_actions.rs) hand
//! Ligma an image or a request as a download named
//! `ligma-background-<job>`, `ligma-vectorize-<job>@<pixel>`,
//! `ligma-object-<job>`, `ligma-heal-<job>` or
//! `ligma-upscale-<job>`, which
//! `on_requested` saves to a temporary file instead of asking where. Once
//! it's saved, it's handled off the main thread and the result goes back to
//! the page through `window.__ligmaJob.done(job, result)`, or `failed(job,
//! reason)`. While a model downloads, `progress(job, text)` says how far
//! it's got. `<pixel>` is how wide a pixel of the original image is in the
//! one handed over, in hundredths (see vectorize.rs). The X on Figma's toast
//! cancels a job, which the page says with a download named
//! `ligma-cancel-<job>`: its model download and inference stop where they
//! can, and its result isn't sent

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager, Url, Webview};

/// Larger than Figma accepts, so anything bigger isn't an image of a file
pub const MAX_BYTES: u64 = 64 << 20;

#[derive(Clone, Copy)]
enum Kind {
    Background,
    Vectorize,
    Object,
    Heal,
    Upscale,
    Cancel,
}

const KINDS: [(&str, Kind); 6] = [
    ("ligma-background-", Kind::Background),
    ("ligma-vectorize-", Kind::Vectorize),
    ("ligma-object-", Kind::Object),
    ("ligma-heal-", Kind::Heal),
    ("ligma-upscale-", Kind::Upscale),
    ("ligma-cancel-", Kind::Cancel),
];

/// An image on its way
struct Pending {
    url: Url,
    kind: Kind,
    tab: u32,
    job: u64,
    /// After `@` in the name, in hundredths
    pixel: Option<u64>,
    temp: PathBuf,
}

static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());

/// A job until its result is sent, by tab and job
#[derive(Default)]
struct Running {
    cancelled: bool,
    /// What stops it, run when it's cancelled
    stops: Vec<Box<dyn FnOnce() + Send>>,
}

static RUNNING: Mutex<BTreeMap<(u32, u64), Running>> = Mutex::new(BTreeMap::new());

/// An image a tab hands over: saved to a temporary file. False for other
/// downloads
pub fn on_requested(webview: &Webview, url: &Url, destination: &mut PathBuf) -> bool {
    if url.scheme() != "blob" {
        return false;
    }
    // WebView2 may add an extension, like ".txt", or " (1)" to the name
    let Some((prefix, kind, job, pixel)) = destination.file_name().and_then(|name| {
        let name = name.to_str()?;
        let (prefix, kind) = KINDS.iter().find(|(prefix, _)| name.starts_with(prefix))?;
        let (job, rest) = number(&name[prefix.len()..]);
        let pixel = rest.strip_prefix('@').and_then(|rest| number(rest).0);
        Some((*prefix, *kind, job?, pixel))
    }) else {
        return false;
    };
    let Some(tab) = webview.label().strip_prefix("tab-").and_then(|id| id.parse().ok()) else {
        return false;
    };
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_nanos());
    let temp = std::env::temp_dir().join(format!("{prefix}{nanos}"));
    if !matches!(kind, Kind::Cancel) {
        RUNNING.lock().unwrap().insert((tab, job), Running::default());
    }
    log::info!("Handoff: {prefix}{job} from tab {tab}");
    PENDING.lock().unwrap().push(Pending { url: url.clone(), kind, tab, job, pixel, temp: temp.clone() });
    *destination = temp;
    true
}

/// The number `text` starts with, and the rest
fn number(text: &str) -> (Option<u64>, &str) {
    let digits = text.find(|char: char| !char.is_ascii_digit()).unwrap_or(text.len());
    (text[..digits].parse().ok(), &text[digits..])
}

/// A download `on_requested` took is saved: handles it off the main thread
/// and sends the result to its tab. False for other downloads
pub fn on_finished(webview: &Webview, url: &Url, success: bool) -> bool {
    let pending = {
        let mut pending = PENDING.lock().unwrap();
        let index = pending.iter().position(|pending| pending.url == *url);
        index.map(|index| pending.remove(index))
    };
    let Some(Pending { kind, tab, job, pixel, temp, .. }) = pending else {
        return false;
    };
    if let Kind::Cancel = kind {
        let _ = std::fs::remove_file(&temp);
        cancel(tab, job);
        return true;
    }
    let app = webview.app_handle().clone();
    std::thread::spawn(move || {
        let bytes = if success { read(&temp) } else { None };
        let _ = std::fs::remove_file(&temp);
        let reply = match (kind, bytes) {
            (_, None) => Err("failed"),
            (Kind::Background, Some(bytes)) => crate::background::remove(&app, tab, job, &bytes),
            (Kind::Vectorize, Some(bytes)) => crate::vectorize::trace(&bytes, pixel.unwrap_or(100) as f64 / 100.0),
            (Kind::Object, Some(bytes)) => crate::objects::handle(&app, tab, job, &bytes),
            (Kind::Heal, Some(bytes)) => crate::heal::heal(tab, job, &bytes),
            (Kind::Upscale, Some(bytes)) => crate::upscale::handle(&app, tab, job, &bytes),
            (Kind::Cancel, Some(_)) => unreachable!(),
        };
        send(&app, tab, job, reply);
    });
    true
}

fn read(path: &Path) -> Option<Vec<u8>> {
    if std::fs::metadata(path).ok()?.len() > MAX_BYTES {
        return None;
    }
    std::fs::read(path).ok()
}

/// Whether the page cancelled `job` in tab `tab`
pub fn cancelled(tab: u32, job: u64) -> bool {
    RUNNING.lock().unwrap().get(&(tab, job)).is_some_and(|running| running.cancelled)
}

/// Has `stop` run if the page cancels `job` in tab `tab`, now if it has
#[cfg_attr(not(birefnet), allow(dead_code))]
pub fn on_cancel(tab: u32, job: u64, stop: impl FnOnce() + Send + 'static) {
    let mut running = RUNNING.lock().unwrap();
    match running.get_mut(&(tab, job)) {
        Some(running) if !running.cancelled => running.stops.push(Box::new(stop)),
        Some(_) => {
            drop(running);
            stop();
        }
        None => {}
    }
}

fn cancel(tab: u32, job: u64) {
    let stops = RUNNING
        .lock()
        .unwrap()
        .get_mut(&(tab, job))
        .map(|running| {
            running.cancelled = true;
            std::mem::take(&mut running.stops)
        })
        .unwrap_or_default();
    for stop in stops {
        stop();
    }
    // The image Select object read for it, even if it's been sent
    crate::objects::close(job);
}

/// Tells the page in tab `tab` how far `job` has got
#[cfg_attr(not(birefnet), allow(dead_code))]
pub fn progress(app: &AppHandle, tab: u32, job: u64, text: &str) {
    if let Some(webview) = crate::tabs::scriptable(app, tab) {
        let _ = webview.eval(format!("window.__ligmaJob?.progress({job}, {text:?})"));
    }
}

/// Sends `reply`, a result or the reason there's none, for `job` to tab
/// `tab`, if it can still run JavaScript. Otherwise the page gives up on it
/// in time
fn send(app: &AppHandle, tab: u32, job: u64, reply: Result<String, &str>) {
    if RUNNING.lock().unwrap().remove(&(tab, job)).is_some_and(|running| running.cancelled) {
        return;
    }
    let Some(webview) = crate::tabs::scriptable(app, tab) else {
        return;
    };
    let script = match reply {
        // A large result, like an upscaled image, goes over in parts, as
        // one script that size takes the page long to take in
        Ok(result) if result.len() > PART => {
            let mut rest = result.as_str();
            while !rest.is_empty() {
                let mut end = PART.min(rest.len());
                while !rest.is_char_boundary(end) {
                    end -= 1;
                }
                let _ = webview.eval(format!("window.__ligmaJob?.part({job}, {:?})", &rest[..end]));
                rest = &rest[end..];
            }
            format!("window.__ligmaJob?.done({job}, null)")
        }
        Ok(result) => format!("window.__ligmaJob?.done({job}, {result:?})"),
        Err(reason) => format!("window.__ligmaJob?.failed({job}, {reason:?})"),
    };
    let _ = webview.eval(script);
}

/// Results longer than this many bytes go to the page in parts this long
const PART: usize = 1 << 20;
