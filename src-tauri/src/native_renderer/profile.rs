//! Dev tools: samples the render thread's stack every millisecond, with
//! `LIGMA_NATIVE_PROFILE=1`, for `profile` to report where its time goes

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

#[repr(C, align(16))]
struct Context([u8; 1232]);

impl Context {
    fn get(&self, at: usize) -> u64 {
        u64::from_le_bytes(self.0[at..at + 8].try_into().unwrap())
    }
    fn set(&mut self, at: usize, value: u64) {
        self.0[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }
}

const RSP: usize = 152;
const RIP: usize = 248;
const CONTEXT_FULL: u32 = 0x10000B;

#[link(name = "kernel32")]
extern "system" {
    fn OpenThread(access: u32, inherit: i32, id: u32) -> *mut c_void;
    fn SuspendThread(thread: *mut c_void) -> u32;
    fn ResumeThread(thread: *mut c_void) -> u32;
    fn GetThreadContext(thread: *mut c_void, context: *mut Context) -> i32;
    fn GetCurrentThreadId() -> u32;
    fn RtlLookupFunctionEntry(pc: u64, base: *mut u64, history: *mut c_void) -> *mut c_void;
    fn RtlVirtualUnwind(kind: u32, base: u64, pc: u64, entry: *mut c_void, context: *mut Context, data: *mut *mut c_void, frame: *mut u64, pointers: *mut c_void) -> *mut c_void;
}

static THREAD: AtomicU32 = AtomicU32::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);
static SAMPLES: Mutex<Option<Samples>> = Mutex::new(None);

#[derive(Default)]
struct Samples {
    count: u64,
    /// Frames by address, counted once a sample
    inclusive: HashMap<u64, u64>,
    leaf: HashMap<u64, u64>,
    /// Image base of each address seen
    bases: HashMap<u64, u64>,
}

/// Called on the render thread: starts sampling it
pub fn register() {
    if !super::dev() || std::env::var("LIGMA_NATIVE_PROFILE").ok().as_deref() != Some("1") || STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
    std::thread::spawn(sample);
}

fn sample() {
    // THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION
    let thread = unsafe { OpenThread(0x0002 | 0x0008 | 0x0040, 0, THREAD.load(Ordering::SeqCst)) };
    if thread.is_null() {
        return;
    }
    let mut frames = [(0u64, 0u64); 48];
    loop {
        std::thread::sleep(Duration::from_millis(1));
        let mut depth = 0;
        unsafe {
            if SuspendThread(thread) == u32::MAX {
                return;
            }
            let mut context = Context([0; 1232]);
            context.0[48..52].copy_from_slice(&CONTEXT_FULL.to_le_bytes());
            if GetThreadContext(thread, &mut context) != 0 {
                while depth < frames.len() {
                    let pc = context.get(RIP);
                    if pc == 0 {
                        break;
                    }
                    let mut base = 0u64;
                    let entry = RtlLookupFunctionEntry(pc, &mut base, std::ptr::null_mut());
                    frames[depth] = (pc, base);
                    depth += 1;
                    if entry.is_null() {
                        // A leaf: its return address is on top of the stack
                        let rsp = context.get(RSP);
                        if rsp == 0 {
                            break;
                        }
                        context.set(RIP, *(rsp as *const u64));
                        context.set(RSP, rsp + 8);
                    } else {
                        let mut data = std::ptr::null_mut();
                        let mut frame = 0u64;
                        RtlVirtualUnwind(0, base, pc, entry, &mut context, &mut data, &mut frame, std::ptr::null_mut());
                    }
                }
            }
            ResumeThread(thread);
        }
        let mut samples = SAMPLES.lock().unwrap();
        let samples = samples.get_or_insert_with(Default::default);
        samples.count += 1;
        let mut seen = std::collections::HashSet::new();
        for (i, &(pc, base)) in frames[..depth].iter().enumerate() {
            samples.bases.insert(pc, base);
            if i == 0 {
                *samples.leaf.entry(pc).or_default() += 1;
            }
            if seen.insert(pc) {
                *samples.inclusive.entry(pc).or_default() += 1;
            }
        }
    }
}

/// Clears what's sampled so far
pub fn reset() {
    *SAMPLES.lock().unwrap() = None;
}

/// The samples so far: `samples N`, then for each frame `kind count
/// base offset` with kind `i` inclusive or `l` leaf, most first
pub fn report() -> String {
    let samples = SAMPLES.lock().unwrap();
    let Some(samples) = samples.as_ref() else {
        return "no samples".into();
    };
    let mut out = format!("samples {}\n", samples.count);
    for (kind, map) in [("l", &samples.leaf), ("i", &samples.inclusive)] {
        let mut rows: Vec<_> = map.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1));
        for (pc, count) in rows.into_iter().take(600) {
            let base = samples.bases.get(pc).copied().unwrap_or(0);
            out += &format!("{kind} {count} {base:#x} {:#x}\n", pc - base);
        }
    }
    out
}
