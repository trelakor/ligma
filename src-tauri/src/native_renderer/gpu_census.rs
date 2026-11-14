//! Dev tools: with LIGMA_NATIVE_CENSUS=1, every GPU heap and committed
//! resource D3D12 makes in this process is counted by the thread that made
//! it, and dropped from the count once let go, so `census` can say what
//! holds the process's GPU memory

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::core::{Interface, GUID, HRESULT};
use windows::Win32::Graphics::Direct3D12::*;

#[link(name = "kernel32")]
extern "system" {
    fn VirtualProtect(address: *mut c_void, size: usize, protect: u32, old: *mut u32) -> i32;
}

const PAGE_READWRITE: u32 = 0x04;

struct Entry {
    bytes: u64,
    /// Width, height and format, for textures
    shape: (u64, u32, i32),
    thread: String,
    kind: &'static str,
    purpose: &'static str,
}

thread_local! {
    static PURPOSE: std::cell::Cell<&'static str> = const { std::cell::Cell::new("") };
}

/// What this thread's making is for, see `purpose`
pub fn current() -> &'static str {
    PURPOSE.with(|cell| cell.get())
}

/// What this thread makes from now until the guard goes is counted as for
/// `purpose`
pub fn purpose(purpose: &'static str) -> impl Drop {
    struct Restore(&'static str);
    impl Drop for Restore {
        fn drop(&mut self) {
            let old = self.0;
            PURPOSE.with(|cell| cell.set(old));
        }
    }
    Restore(PURPOSE.with(|cell| cell.replace(purpose)))
}

fn live() -> &'static Mutex<HashMap<usize, Entry>> {
    static LIVE: OnceLock<Mutex<HashMap<usize, Entry>>> = OnceLock::new();
    LIVE.get_or_init(Default::default)
}

/// Each object kind's own Release, by its vtable
fn releases() -> &'static Mutex<HashMap<usize, usize>> {
    static RELEASES: OnceLock<Mutex<HashMap<usize, usize>>> = OnceLock::new();
    RELEASES.get_or_init(Default::default)
}

static COMMITTED: AtomicUsize = AtomicUsize::new(0);
static COMMITTED1: AtomicUsize = AtomicUsize::new(0);
static COMMITTED2: AtomicUsize = AtomicUsize::new(0);
static COMMITTED3: AtomicUsize = AtomicUsize::new(0);
static HEAP: AtomicUsize = AtomicUsize::new(0);
static HEAP1: AtomicUsize = AtomicUsize::new(0);
static DEVICE: AtomicUsize = AtomicUsize::new(0);
static PIPELINE: AtomicUsize = AtomicUsize::new(0);
/// Pipelines made, and the ns that took
pub static PIPELINES: AtomicUsize = AtomicUsize::new(0);
pub static PIPELINES_NS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn pipeline(this: *mut c_void, desc: *const c_void, riid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    type Real = unsafe extern "system" fn(*mut c_void, *const c_void, *const GUID, *mut *mut c_void) -> HRESULT;
    let real: Real = std::mem::transmute(PIPELINE.load(Ordering::SeqCst));
    let started = std::time::Instant::now();
    let result = real(this, desc, riid, out);
    PIPELINES.fetch_add(1, Ordering::Relaxed);
    PIPELINES_NS.fetch_add(started.elapsed().as_nanos() as usize, Ordering::Relaxed);
    result
}

pub fn on() -> bool {
    super::dev() && std::env::var("LIGMA_NATIVE_CENSUS").is_ok_and(|value| value == "1")
}

unsafe fn patch(vtable: *const c_void, offset: usize, with: usize) -> usize {
    let slot = (vtable as *mut u8).add(offset) as *mut usize;
    let mut old = 0;
    if VirtualProtect(slot as *mut c_void, 8, PAGE_READWRITE, &mut old) == 0 {
        return 0;
    }
    let real = *slot;
    *slot = with;
    VirtualProtect(slot as *mut c_void, 8, old, &mut old);
    real
}

/// Hooks `device`'s kind of device, once
pub fn install(device: &ID3D12Device) {
    if !on() || DEVICE.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    unsafe {
        let vtable = device.vtable() as *const _ as *const c_void;
        COMMITTED.store(patch(vtable, std::mem::offset_of!(ID3D12Device_Vtbl, CreateCommittedResource), committed as *const () as usize), Ordering::SeqCst);
        HEAP.store(patch(vtable, std::mem::offset_of!(ID3D12Device_Vtbl, CreateHeap), heap as *const () as usize), Ordering::SeqCst);
        PIPELINE.store(patch(vtable, std::mem::offset_of!(ID3D12Device_Vtbl, CreateGraphicsPipelineState), pipeline as *const () as usize), Ordering::SeqCst);
        if let Ok(device4) = device.cast::<ID3D12Device4>() {
            let vtable = device4.vtable() as *const _ as *const c_void;
            COMMITTED1.store(patch(vtable, std::mem::offset_of!(ID3D12Device4_Vtbl, CreateCommittedResource1), committed1 as *const () as usize), Ordering::SeqCst);
            HEAP1.store(patch(vtable, std::mem::offset_of!(ID3D12Device4_Vtbl, CreateHeap1), heap1 as *const () as usize), Ordering::SeqCst);
        }
        if let Ok(device8) = device.cast::<ID3D12Device8>() {
            let vtable = device8.vtable() as *const _ as *const c_void;
            COMMITTED2.store(patch(vtable, std::mem::offset_of!(ID3D12Device8_Vtbl, CreateCommittedResource2), committed2 as *const () as usize), Ordering::SeqCst);
        }
        if let Ok(device10) = device.cast::<ID3D12Device10>() {
            let vtable = device10.vtable() as *const _ as *const c_void;
            COMMITTED3.store(patch(vtable, std::mem::offset_of!(ID3D12Device10_Vtbl, CreateCommittedResource3), committed3 as *const () as usize), Ordering::SeqCst);
        }
    }
    log::info!("native renderer: GPU census on");
}

unsafe fn record(device: *mut c_void, object: *mut c_void, bytes: u64, kind: &'static str) {
    record_shaped(device, object, bytes, kind, (0, 0, 0));
}

unsafe fn record_shaped(device: *mut c_void, object: *mut c_void, bytes: u64, kind: &'static str, shape: (u64, u32, i32)) {
    let _ = device;
    if object.is_null() {
        return;
    }
    let thread = std::thread::current().name().unwrap_or("?").to_string();
    // Hooks the object's Release, once for each kind
    let vtable = *(object as *const *const c_void);
    {
        let mut releases = releases().lock().unwrap();
        if !releases.contains_key(&(vtable as usize)) {
            let real = patch(vtable, 2 * 8, release as *const () as usize);
            releases.insert(vtable as usize, real);
        }
    }
    let purpose = PURPOSE.with(|cell| cell.get());
    live().lock().unwrap().insert(object as usize, Entry { bytes, shape, thread, kind, purpose });
}

unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    let vtable = *(this as *const *const c_void);
    let real = releases().lock().unwrap().get(&(vtable as usize)).copied();
    let Some(real) = real else {
        return 0;
    };
    let real: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(real);
    let count = real(this);
    if count == 0 {
        live().lock().unwrap().remove(&(this as usize));
    }
    count
}

unsafe fn size(device: *mut c_void, desc: &D3D12_RESOURCE_DESC) -> u64 {
    let Some(device) = ID3D12Device::from_raw_borrowed(&device) else {
        return 0;
    };
    device.GetResourceAllocationInfo(0, &[*desc]).SizeInBytes
}

fn heap_kind(properties: *const D3D12_HEAP_PROPERTIES, desc: &D3D12_RESOURCE_DESC) -> &'static str {
    let heap = unsafe { properties.as_ref() }.map_or(D3D12_HEAP_TYPE_DEFAULT, |properties| properties.Type);
    match (heap, desc.Dimension) {
        (D3D12_HEAP_TYPE_UPLOAD, _) => "upload",
        (D3D12_HEAP_TYPE_READBACK, _) => "readback",
        (_, D3D12_RESOURCE_DIMENSION_BUFFER) => "buffer",
        _ => "texture",
    }
}

fn desc_of(desc: &D3D12_RESOURCE_DESC1) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: desc.Dimension,
        Alignment: desc.Alignment,
        Width: desc.Width,
        Height: desc.Height,
        DepthOrArraySize: desc.DepthOrArraySize,
        MipLevels: desc.MipLevels,
        Format: desc.Format,
        SampleDesc: desc.SampleDesc,
        Layout: desc.Layout,
        Flags: desc.Flags,
    }
}

unsafe extern "system" fn committed(
    this: *mut c_void,
    properties: *const D3D12_HEAP_PROPERTIES,
    flags: D3D12_HEAP_FLAGS,
    desc: *const D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
    clear: *const D3D12_CLEAR_VALUE,
    riid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    let real: unsafe extern "system" fn(*mut c_void, *const D3D12_HEAP_PROPERTIES, D3D12_HEAP_FLAGS, *const D3D12_RESOURCE_DESC, D3D12_RESOURCE_STATES, *const D3D12_CLEAR_VALUE, *const GUID, *mut *mut c_void) -> HRESULT =
        std::mem::transmute(COMMITTED.load(Ordering::SeqCst));
    let result = real(this, properties, flags, desc, state, clear, riid, out);
    if result.is_ok() && !out.is_null() {
        record_shaped(this, *out, size(this, &*desc), heap_kind(properties, &*desc), ((*desc).Width, (*desc).Height, (*desc).Format.0));
    }
    result
}

unsafe extern "system" fn committed1(
    this: *mut c_void,
    properties: *const D3D12_HEAP_PROPERTIES,
    flags: D3D12_HEAP_FLAGS,
    desc: *const D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
    clear: *const D3D12_CLEAR_VALUE,
    session: *mut c_void,
    riid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    let real: unsafe extern "system" fn(*mut c_void, *const D3D12_HEAP_PROPERTIES, D3D12_HEAP_FLAGS, *const D3D12_RESOURCE_DESC, D3D12_RESOURCE_STATES, *const D3D12_CLEAR_VALUE, *mut c_void, *const GUID, *mut *mut c_void) -> HRESULT =
        std::mem::transmute(COMMITTED1.load(Ordering::SeqCst));
    let result = real(this, properties, flags, desc, state, clear, session, riid, out);
    if result.is_ok() && !out.is_null() {
        record_shaped(this, *out, size(this, &*desc), heap_kind(properties, &*desc), ((*desc).Width, (*desc).Height, (*desc).Format.0));
    }
    result
}

unsafe extern "system" fn committed2(
    this: *mut c_void,
    properties: *const D3D12_HEAP_PROPERTIES,
    flags: D3D12_HEAP_FLAGS,
    desc: *const D3D12_RESOURCE_DESC1,
    state: D3D12_RESOURCE_STATES,
    clear: *const D3D12_CLEAR_VALUE,
    session: *mut c_void,
    riid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    let real: unsafe extern "system" fn(*mut c_void, *const D3D12_HEAP_PROPERTIES, D3D12_HEAP_FLAGS, *const D3D12_RESOURCE_DESC1, D3D12_RESOURCE_STATES, *const D3D12_CLEAR_VALUE, *mut c_void, *const GUID, *mut *mut c_void) -> HRESULT =
        std::mem::transmute(COMMITTED2.load(Ordering::SeqCst));
    let result = real(this, properties, flags, desc, state, clear, session, riid, out);
    if result.is_ok() && !out.is_null() {
        let desc = desc_of(&*desc);
        record_shaped(this, *out, size(this, &desc), heap_kind(properties, &desc), (desc.Width, desc.Height, desc.Format.0));
    }
    result
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn committed3(
    this: *mut c_void,
    properties: *const D3D12_HEAP_PROPERTIES,
    flags: D3D12_HEAP_FLAGS,
    desc: *const D3D12_RESOURCE_DESC1,
    layout: D3D12_BARRIER_LAYOUT,
    clear: *const D3D12_CLEAR_VALUE,
    session: *mut c_void,
    formats: u32,
    castable: *const c_void,
    riid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    let real: unsafe extern "system" fn(*mut c_void, *const D3D12_HEAP_PROPERTIES, D3D12_HEAP_FLAGS, *const D3D12_RESOURCE_DESC1, D3D12_BARRIER_LAYOUT, *const D3D12_CLEAR_VALUE, *mut c_void, u32, *const c_void, *const GUID, *mut *mut c_void) -> HRESULT =
        std::mem::transmute(COMMITTED3.load(Ordering::SeqCst));
    let result = real(this, properties, flags, desc, layout, clear, session, formats, castable, riid, out);
    if result.is_ok() && !out.is_null() {
        let desc = desc_of(&*desc);
        record_shaped(this, *out, size(this, &desc), heap_kind(properties, &desc), (desc.Width, desc.Height, desc.Format.0));
    }
    result
}

fn heap_name(desc: &D3D12_HEAP_DESC) -> &'static str {
    match desc.Properties.Type {
        D3D12_HEAP_TYPE_UPLOAD => "upload heap",
        D3D12_HEAP_TYPE_READBACK => "readback heap",
        _ => "heap",
    }
}

unsafe extern "system" fn heap(this: *mut c_void, desc: *const D3D12_HEAP_DESC, riid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    let real: unsafe extern "system" fn(*mut c_void, *const D3D12_HEAP_DESC, *const GUID, *mut *mut c_void) -> HRESULT = std::mem::transmute(HEAP.load(Ordering::SeqCst));
    let result = real(this, desc, riid, out);
    if result.is_ok() && !out.is_null() {
        record(this, *out, (*desc).SizeInBytes, heap_name(&*desc));
    }
    result
}

unsafe extern "system" fn heap1(this: *mut c_void, desc: *const D3D12_HEAP_DESC, session: *mut c_void, riid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    let real: unsafe extern "system" fn(*mut c_void, *const D3D12_HEAP_DESC, *mut c_void, *const GUID, *mut *mut c_void) -> HRESULT = std::mem::transmute(HEAP1.load(Ordering::SeqCst));
    let result = real(this, desc, session, riid, out);
    if result.is_ok() && !out.is_null() {
        record(this, *out, (*desc).SizeInBytes, heap_name(&*desc));
    }
    result
}

/// What's held, by thread and kind, biggest first
pub fn report() -> String {
    let live = live().lock().unwrap();
    let mut sums: HashMap<(String, String), (u64, usize)> = HashMap::new();
    for entry in live.values() {
        let sum = sums.entry((entry.thread.clone(), format!("{} {}", entry.kind, entry.purpose))).or_default();
        sum.0 += entry.bytes;
        sum.1 += 1;
    }
    let mut rows: Vec<_> = sums.into_iter().collect();
    rows.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    let total: u64 = live.values().filter(|entry| !entry.kind.starts_with("upload") && !entry.kind.starts_with("readback")).map(|entry| entry.bytes).sum();
    let mut out = format!("{:.1} MB on the GPU\n", total as f64 / 1e6);
    for ((thread, kind), (bytes, count)) in rows {
        out += &format!("{:>8.1} MB {count:>5} {kind} on {thread}\n", bytes as f64 / 1e6);
    }
    // The commonest shapes of textures, which says what they're for
    let mut shapes: HashMap<(u64, u32, i32, &'static str), (u64, usize)> = HashMap::new();
    for entry in live.values().filter(|entry| entry.shape.0 > 0 && (entry.kind == "texture" || entry.kind == "buffer")) {
        let sum = shapes.entry((entry.shape.0, entry.shape.1, entry.shape.2, entry.purpose)).or_default();
        sum.0 += entry.bytes;
        sum.1 += 1;
    }
    let mut shapes: Vec<_> = shapes.into_iter().collect();
    shapes.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    for ((width, height, format, purpose), (bytes, count)) in shapes.into_iter().take(12) {
        out += &format!("{:>8.1} MB {count:>5} of {width}x{height} format {format} {purpose}\n", bytes as f64 / 1e6);
    }
    out
}
