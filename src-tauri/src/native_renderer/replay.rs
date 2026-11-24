//! For benchmarks: records the frames the drawn tab hands over
//! (`LIGMA_NATIVE_RECORD=<path>`), and plays them back in place of the tab's
//! (`LIGMA_NATIVE_REPLAY=<path>`) at a fixed rate with Figma idle, so a drag
//! is drawn from the same frames at the same pace every run. See
//! `run::replay_frames`

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::Duration;

use super::tree::Frame;

/// Appends frames to a file as they come, each a little-endian u32 length
/// then the frame as the page sent it
pub struct Recorder(BufWriter<File>);

impl Recorder {
    pub fn open() -> Option<Recorder> {
        let path = std::env::var_os("LIGMA_NATIVE_RECORD")?;
        match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => {
                log::info!("native renderer: recording frames to {}", Path::new(&path).display());
                Some(Recorder(BufWriter::with_capacity(4 << 20, file)))
            }
            Err(error) => {
                log::warn!("native renderer: can't record frames to {}: {error}", Path::new(&path).display());
                None
            }
        }
    }

    pub fn write(&mut self, body: &[u8]) {
        let written = self.0.write_all(&(body.len() as u32).to_le_bytes()).and_then(|()| self.0.write_all(body));
        if let Err(error) = written {
            log::warn!("native renderer: a frame couldn't be recorded: {error}");
        }
    }

    pub fn flush(&mut self) {
        let _ = self.0.flush();
    }
}

/// Frames recorded, to play back
pub struct Replay {
    /// Up to and including the whole scene the drag starts from, the last
    /// that one
    pub base: Vec<Vec<u8>>,
    /// From the frame after it on
    pub deltas: Vec<Vec<u8>>,
    pub wait: Duration,
    pub every: Duration,
    pub loops: u32,
}

impl Replay {
    pub fn load() -> Option<Replay> {
        let path = std::env::var_os("LIGMA_NATIVE_REPLAY")?;
        match read(Path::new(&path)) {
            Ok(replay) => Some(replay),
            Err(error) => {
                log::warn!("native renderer: frames can't be played back from {}: {error}", Path::new(&path).display());
                None
            }
        }
    }
}

fn env_ms(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|value| value.trim().parse().ok()).unwrap_or(default)
}

fn read(path: &Path) -> Result<Replay, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let mut frames = Vec::new();
    let mut at = 0;
    while let Some(length) = bytes.get(at..at + 4) {
        let start = at + 4;
        // The last cut short, by Ligma closing as it was written
        let Some(body) = bytes.get(start..start + u32::from_le_bytes(length.try_into().unwrap()) as usize) else { break };
        at = start + body.len();
        match Frame::parse(body.to_vec()) {
            Ok(frame) => frames.push(frame),
            Err(error) => log::warn!("native renderer: a recorded frame couldn't be read: {error}"),
        }
    }
    // A file recorded to more than once holds several loads of the page, of
    // which the last is played
    let session = frames.last().ok_or("no frames")?.header.session.clone();
    frames.retain(|frame| frame.header.session == session);
    frames.sort_by_key(|frame| frame.header.seq);
    frames.dedup_by_key(|frame| frame.header.seq);
    // The drag starts with the first change made as the pointer moves, or
    // with the last whole scene if none was
    let moved = |frame: &Frame| frame.header.page.as_ref().and_then(|page| page.get("moves")?.as_f64()).unwrap_or(0.0) > 0.0;
    let drag = frames.iter().position(|frame| frame.header.mode != 0 && moved(frame)).unwrap_or(frames.len());
    let base = frames[..drag].iter().rposition(|frame| frame.header.mode == 0).ok_or("no whole scene before the drag")?;
    let mut deltas = frames.split_off(base + 1);
    // Frames missing would have Figma send the whole scene again, so it's
    // played up to the first
    let mut seq = frames[base].header.seq;
    if let Some(gap) = deltas.iter().position(|frame| {
        seq += 1;
        frame.header.seq != seq
    }) {
        log::warn!("native renderer: recorded frame {seq} is missing, so it's played up to there");
        deltas.truncate(gap);
    }
    log::info!(
        "native renderer: playing back {} frames to start from, up to the whole scene of frame {}, then {} of the drag from frame {}",
        frames.len(),
        frames[base].header.seq,
        deltas.len(),
        deltas.first().map_or(0, |frame| frame.header.seq)
    );
    Ok(Replay {
        base: frames.into_iter().map(Frame::into_body).collect(),
        deltas: deltas.into_iter().map(Frame::into_body).collect(),
        wait: Duration::from_millis(env_ms("LIGMA_NATIVE_REPLAY_WAIT_MS", 8000)),
        every: Duration::from_millis(env_ms("LIGMA_NATIVE_REPLAY_EVERY_MS", 8).max(1)),
        loops: env_ms("LIGMA_NATIVE_REPLAY_LOOPS", 1).clamp(1, 10_000) as u32,
    })
}
