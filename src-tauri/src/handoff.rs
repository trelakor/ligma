//! Figma tabs get no IPC, so the image bar's Remove background, Vectorize
//! and Select object (see ai_actions.rs) hand Ligma an image or a request
//! as a download named `ligma-background-<job>`,
//! `ligma-vectorize-<job>@<pixel>` or `ligma-object-<job>`, which
//! `on_requested` saves to a temporary file instead of asking where. Once
//! it's saved, it's handled off the main thread and the result goes back to
//! the page through `window.__ligmaJob.done(job, result)`, or `failed(job,
//! reason)`. While a model downloads, `progress(job, text)` says how far
//! it's got. `<pixel>` is how wide a pixel of the original image is in the
//! one handed over, in hundredths (see vectorize.rs)

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
}

const KINDS: [(&str, Kind); 3] =
    [("ligma-background-", Kind::Background), ("ligma-vectorize-", Kind::Vectorize), ("ligma-object-", Kind::Object)];

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
    let app = webview.app_handle().clone();
    std::thread::spawn(move || {
        let bytes = if success { read(&temp) } else { None };
        let _ = std::fs::remove_file(&temp);
        let reply = match (kind, bytes) {
            (_, None) => Err("failed"),
            (Kind::Background, Some(bytes)) => crate::background::remove(&app, tab, job, &bytes),
            (Kind::Vectorize, Some(bytes)) => crate::vectorize::trace(&bytes, pixel.unwrap_or(100) as f64 / 100.0),
            (Kind::Object, Some(bytes)) => crate::objects::handle(&app, tab, job, &bytes),
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
    let Some(webview) = crate::tabs::scriptable(app, tab) else {
        return;
    };
    let script = match reply {
        Ok(result) => format!("window.__ligmaJob?.done({job}, {result:?})"),
        Err(reason) => format!("window.__ligmaJob?.failed({job}, {reason:?})"),
    };
    let _ = webview.eval(script);
}
