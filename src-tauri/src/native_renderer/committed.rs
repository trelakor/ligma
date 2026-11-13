//! A memory allocator for Skia that gives textures and buffers memory of
//! their own, freed the moment Skia lets go of them. Skia's own allocator
//! carves them out of heaps it grows to 32, 64 and 128 MB and never gives
//! back while anything at all is left in them, which held hundreds of MB
//! for a few images
//!
//! Memory of its own takes at least a 64 KiB page, and making it takes the
//! driver a while, up to a millisecond, so resources up to 1 MiB share heaps
//! of their kind instead, each freed once nothing is left in it: textures
//! small enough to be placed at 4 KiB, masks and tables Skia makes by the
//! thousand, other textures, render targets, and buffers to upload through
//!
//! Skia takes C++ objects here, `GrD3DMemoryAllocator` and `GrD3DAlloc`, so
//! this lays them out as MSVC does: a vtable, starting with
//! `~SkRefCntBase` and `internal_dispose`, then the reference count

use std::ffi::c_void;
use std::sync::atomic::AtomicI32;
use std::sync::{Arc, Mutex};

use skia_safe::gpu::d3d::MemoryAllocator;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::{
    ID3D12Device, ID3D12Heap, ID3D12Resource, D3D12_CLEAR_VALUE, D3D12_FEATURE_D3D12_OPTIONS7, D3D12_FEATURE_DATA_D3D12_OPTIONS7, D3D12_HEAP_DESC,
    D3D12_HEAP_FLAGS, D3D12_HEAP_FLAG_ALLOW_ONLY_BUFFERS, D3D12_HEAP_FLAG_ALLOW_ONLY_NON_RT_DS_TEXTURES, D3D12_HEAP_FLAG_ALLOW_ONLY_RT_DS_TEXTURES,
    D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE, D3D12_HEAP_TYPE_DEFAULT, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_DESC,
    D3D12_RESOURCE_DIMENSION_BUFFER, D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
    D3D12_RESOURCE_STATES,
};

/// Its memory isn't cleared first, for what's written whole before it's
/// read: images and buffers uploaded. Render targets are cleared, as
/// Skia's own allocator has big ones: a blur reads some of a new layer
/// before drawing it, which otherwise shows what was in that memory, as
/// another frame's pixels
const NOT_ZEROED: D3D12_HEAP_FLAGS = D3D12_HEAP_FLAGS(0x1000);

/// A kind of resource placed in shared heaps: the heaps', their size, the
/// steps resources are placed in, and the largest placed
struct Kind {
    heap: D3D12_HEAP_TYPE,
    flags: D3D12_HEAP_FLAGS,
    slab: u64,
    slot: u64,
    largest: u64,
}

const TINY_TEXTURES: usize = 0;
const TEXTURES: usize = 1;
const TARGETS: usize = 2;
const UPLOADS: usize = 3;

static KINDS_PLACED: [Kind; 4] = [
    Kind { heap: D3D12_HEAP_TYPE_DEFAULT, flags: D3D12_HEAP_FLAG_ALLOW_ONLY_NON_RT_DS_TEXTURES, slab: 4 << 20, slot: 4 << 10, largest: 64 << 10 },
    Kind { heap: D3D12_HEAP_TYPE_DEFAULT, flags: D3D12_HEAP_FLAG_ALLOW_ONLY_NON_RT_DS_TEXTURES, slab: 8 << 20, slot: 64 << 10, largest: 1 << 20 },
    Kind { heap: D3D12_HEAP_TYPE_DEFAULT, flags: D3D12_HEAP_FLAG_ALLOW_ONLY_RT_DS_TEXTURES, slab: 8 << 20, slot: 64 << 10, largest: 1 << 20 },
    Kind { heap: D3D12_HEAP_TYPE_UPLOAD, flags: D3D12_HEAP_FLAG_ALLOW_ONLY_BUFFERS, slab: 4 << 20, slot: 64 << 10, largest: 1 << 20 },
];

#[repr(C)]
struct Vtable {
    destroy: unsafe extern "system" fn(*mut Allocator, u32) -> *mut c_void,
    dispose: unsafe extern "system" fn(*mut Allocator),
    create: unsafe extern "system" fn(*mut Allocator, *mut *mut c_void, D3D12_HEAP_TYPE, *const D3D12_RESOURCE_DESC, D3D12_RESOURCE_STATES, *mut *mut Alloc, *const D3D12_CLEAR_VALUE) -> *mut *mut c_void,
    alias: unsafe extern "system" fn(*mut Allocator, *mut *mut c_void, *mut c_void, u64, *const D3D12_RESOURCE_DESC, D3D12_RESOURCE_STATES, *const D3D12_CLEAR_VALUE) -> *mut *mut c_void,
}

#[repr(C)]
struct Allocator {
    vtable: &'static Vtable,
    refs: AtomicI32,
    device: ID3D12Device,
    zeroed_only: bool,
    pools: [Arc<Mutex<Pool>>; 4],
}

static VTABLE: Vtable = Vtable { destroy, dispose, create, alias };

/// A small texture's place in a shared heap, as Skia holds it
#[repr(C)]
struct Alloc {
    vtable: &'static AllocVtable,
    refs: AtomicI32,
    pool: Arc<Mutex<Pool>>,
    slab: usize,
    first: usize,
    count: usize,
}

#[repr(C)]
struct AllocVtable {
    destroy: unsafe extern "system" fn(*mut Alloc, u32) -> *mut c_void,
    dispose: unsafe extern "system" fn(*mut Alloc),
}

static ALLOC_VTABLE: AllocVtable = AllocVtable { destroy: destroy_alloc, dispose: dispose_alloc };

struct Pool {
    kind: &'static Kind,
    /// Heaps can be made without clearing them first
    not_zeroed: bool,
    slabs: Vec<Option<Slab>>,
}

struct Slab {
    heap: ID3D12Heap,
    used: Vec<bool>,
    in_use: usize,
}

impl Pool {
    /// `count` slots in a row, in a heap made if none has room
    fn take(&mut self, device: &ID3D12Device, count: usize) -> Option<(usize, usize, ID3D12Heap)> {
        let slots = self.slots();
        for (index, slab) in self.slabs.iter_mut().enumerate() {
            let Some(slab) = slab else { continue };
            if slots - slab.in_use < count {
                continue;
            }
            let mut run = 0;
            for slot in 0..slots {
                run = if slab.used[slot] { 0 } else { run + 1 };
                if run == count {
                    let first = slot + 1 - count;
                    slab.used[first..=slot].fill(true);
                    slab.in_use += count;
                    return Some((index, first, slab.heap.clone()));
                }
            }
        }
        let not_zeroed = if self.not_zeroed && self.kind.heap == D3D12_HEAP_TYPE_DEFAULT { NOT_ZEROED.0 } else { 0 };
        let desc = D3D12_HEAP_DESC {
            SizeInBytes: self.kind.slab,
            Properties: D3D12_HEAP_PROPERTIES { Type: self.kind.heap, ..Default::default() },
            Alignment: 0,
            Flags: D3D12_HEAP_FLAGS(self.kind.flags.0 | not_zeroed),
        };
        let mut heap: Option<ID3D12Heap> = None;
        unsafe { device.CreateHeap(&desc, &mut heap) }.map_err(|error| log::warn!("native renderer: GPU memory: {error}")).ok()?;
        let heap = heap?;
        let mut used = vec![false; slots];
        used[..count].fill(true);
        let slab = Slab { heap: heap.clone(), used, in_use: count };
        let index = match self.slabs.iter().position(Option::is_none) {
            Some(index) => {
                self.slabs[index] = Some(slab);
                index
            }
            None => {
                self.slabs.push(Some(slab));
                self.slabs.len() - 1
            }
        };
        Some((index, 0, heap))
    }

    /// Gives back slots, and a heap left empty unless it's the only one
    /// with room
    fn give(&mut self, index: usize, first: usize, count: usize) {
        let Some(Some(slab)) = self.slabs.get_mut(index) else {
            return;
        };
        slab.used[first..first + count].fill(false);
        slab.in_use -= count;
        if slab.in_use == 0 {
            let slots = self.slots();
            let others_have_room = self.slabs.iter().enumerate().any(|(other, slab)| other != index && slab.as_ref().is_some_and(|slab| slab.in_use < slots));
            if others_have_room {
                self.slabs[index] = None;
            }
        }
    }

    fn slots(&self) -> usize {
        (self.kind.slab / self.kind.slot) as usize
    }

    /// Frees the heaps left empty
    fn trim(&mut self) {
        for slab in &mut self.slabs {
            if slab.as_ref().is_some_and(|slab| slab.in_use == 0) {
                *slab = None;
            }
        }
    }
}

/// The pools of the allocators made, to trim
static POOLS: Mutex<Vec<std::sync::Weak<Mutex<Pool>>>> = Mutex::new(Vec::new());

/// Frees heaps nothing is placed in, kept meanwhile for what's made next
pub fn trim() {
    let mut pools = POOLS.lock().unwrap();
    pools.retain(|pool| pool.strong_count() > 0);
    for pool in pools.iter().filter_map(std::sync::Weak::upgrade) {
        pool.lock().unwrap().trim();
    }
}

static MADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static MAKING_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Dev tools: resources made so far, and the ms that took
pub fn made() -> (u64, f64) {
    use std::sync::atomic::Ordering::Relaxed;
    (MADE.load(Relaxed), MAKING_NS.load(Relaxed) as f64 / 1e6)
}

/// An allocator for a Skia context on `device`
pub fn allocator(device: &ID3D12Device) -> MemoryAllocator {
    // Older Windows can't skip clearing
    let zeroed_only = {
        let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS7::default();
        unsafe { device.CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS7, &mut options as *mut _ as *mut c_void, std::mem::size_of_val(&options) as u32) }.is_err()
    };
    let pools: [Arc<Mutex<Pool>>; 4] = std::array::from_fn(|kind| Arc::new(Mutex::new(Pool { kind: &KINDS_PLACED[kind], not_zeroed: !zeroed_only && kind != TARGETS, slabs: Vec::new() })));
    POOLS.lock().unwrap().extend(pools.iter().map(Arc::downgrade));
    let allocator = Box::into_raw(Box::new(Allocator { vtable: &VTABLE, refs: AtomicI32::new(1), device: device.clone(), zeroed_only, pools }));
    // A handle is a pointer to the object, which it now owns
    unsafe { std::mem::transmute::<*mut Allocator, MemoryAllocator>(allocator) }
}

unsafe extern "system" fn destroy(this: *mut Allocator, flags: u32) -> *mut c_void {
    if flags & 1 != 0 {
        drop(Box::from_raw(this));
    } else {
        std::ptr::drop_in_place(this);
    }
    this as *mut c_void
}

unsafe extern "system" fn dispose(this: *mut Allocator) {
    drop(Box::from_raw(this));
}

unsafe extern "system" fn destroy_alloc(this: *mut Alloc, flags: u32) -> *mut c_void {
    release_slots(&*this);
    if flags & 1 != 0 {
        drop(Box::from_raw(this));
    } else {
        std::ptr::drop_in_place(this);
    }
    this as *mut c_void
}

unsafe extern "system" fn dispose_alloc(this: *mut Alloc) {
    release_slots(&*this);
    drop(Box::from_raw(this));
}

fn release_slots(alloc: &Alloc) {
    alloc.pool.lock().unwrap().give(alloc.slab, alloc.first, alloc.count);
}

/// `createResource`: up to 1 MiB, in a shared heap, anything else a
/// committed resource, its own memory. Skia gets no allocation object back
/// for those, as for a texture it wraps
unsafe extern "system" fn create(
    this: *mut Allocator,
    out: *mut *mut c_void,
    heap: D3D12_HEAP_TYPE,
    desc: *const D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
    allocation: *mut *mut Alloc,
    clear: *const D3D12_CLEAR_VALUE,
) -> *mut *mut c_void {
    *out = std::ptr::null_mut();
    let this = &*this;
    let started = std::time::Instant::now();
    let _made = Made(started);
    let clear = (!clear.is_null()).then_some(clear);
    if let Some(resource) = placed(this, heap, &*desc, state, allocation, clear) {
        *out = resource.into_raw();
        tally(heap, &*desc, true, started);
        return out;
    }
    let _tally = Tally(heap, *desc, started);
    let properties = D3D12_HEAP_PROPERTIES { Type: heap, ..Default::default() };
    let targets = (*desc).Flags.0 & (D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET.0 | D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL.0) != 0;
    let flags = if heap == D3D12_HEAP_TYPE_DEFAULT && !this.zeroed_only && !targets { NOT_ZEROED } else { D3D12_HEAP_FLAG_NONE };
    let mut resource: Option<ID3D12Resource> = None;
    match this.device.CreateCommittedResource(&properties, flags, desc, state, clear, &mut resource) {
        Ok(()) => {
            if let Some(resource) = resource {
                *out = resource.into_raw();
            }
        }
        Err(error) => log::warn!("native renderer: GPU memory: {error}"),
    }
    out
}

struct Made(std::time::Instant);

impl Drop for Made {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::Relaxed;
        MADE.fetch_add(1, Relaxed);
        MAKING_NS.fetch_add(self.0.elapsed().as_nanos() as u64, Relaxed);
    }
}

/// Dev tools: resources made while tracing, by kind, with their count and
/// the ns they took
static KINDS: std::sync::Mutex<Option<std::collections::HashMap<String, (u64, u64)>>> = std::sync::Mutex::new(None);

struct Tally(D3D12_HEAP_TYPE, D3D12_RESOURCE_DESC, std::time::Instant);

impl Drop for Tally {
    fn drop(&mut self) {
        tally(self.0, &self.1, false, self.2);
    }
}

fn tally(heap: D3D12_HEAP_TYPE, desc: &D3D12_RESOURCE_DESC, placed: bool, started: std::time::Instant) {
    if !super::render::tracing() {
        return;
    }
    let kind = if desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D {
        format!("buffer heap={} {}KB", heap.0, desc.Width >> 10)
    } else {
        format!("texture {}x{} format={} flags={} samples={} placed={placed}", desc.Width, desc.Height, desc.Format.0, desc.Flags.0, desc.SampleDesc.Count)
    };
    let ns = started.elapsed().as_nanos() as u64;
    let mut kinds = KINDS.lock().unwrap();
    let entry = kinds.get_or_insert_with(Default::default).entry(kind).or_default();
    entry.0 += 1;
    entry.1 += ns;
}

/// Dev tools: the kinds of resource made since last asked, most time first
pub fn take_kinds() -> String {
    let Some(kinds) = KINDS.lock().unwrap().take() else {
        return String::new();
    };
    let mut kinds: Vec<_> = kinds.into_iter().collect();
    kinds.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));
    kinds.iter().map(|(kind, (count, ns))| format!("{count}x {kind} {:.1}ms", *ns as f64 / 1e6)).collect::<Vec<_>>().join("; ")
}

/// The resource placed in a shared heap of its kind, if it's one placed
unsafe fn placed(
    this: &Allocator,
    heap: D3D12_HEAP_TYPE,
    desc: &D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
    allocation: *mut *mut Alloc,
    clear: Option<*const D3D12_CLEAR_VALUE>,
) -> Option<ID3D12Resource> {
    if allocation.is_null() || desc.SampleDesc.Count > 1 {
        return None;
    }
    let targets = desc.Flags.0 & (D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET.0 | D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL.0) != 0;
    let mut desc = *desc;
    let kind = if heap == D3D12_HEAP_TYPE_UPLOAD && desc.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER {
        UPLOADS
    } else if heap != D3D12_HEAP_TYPE_DEFAULT || desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D {
        return None;
    } else if targets {
        TARGETS
    } else {
        // Placed at 4 KiB if it's small enough to be
        let mut small = desc;
        small.Alignment = KINDS_PLACED[TINY_TEXTURES].slot;
        let info = this.device.GetResourceAllocationInfo(0, &[small]);
        if info.Alignment == small.Alignment && info.SizeInBytes <= KINDS_PLACED[TINY_TEXTURES].largest {
            desc = small;
            TINY_TEXTURES
        } else {
            TEXTURES
        }
    };
    let pool = &this.pools[kind];
    let slot = KINDS_PLACED[kind].slot;
    let info = this.device.GetResourceAllocationInfo(0, &[desc]);
    if info.Alignment > slot || info.SizeInBytes > KINDS_PLACED[kind].largest || info.SizeInBytes == 0 {
        return None;
    }
    let count = info.SizeInBytes.div_ceil(slot) as usize;
    let (slab, first, heap) = pool.lock().unwrap().take(&this.device, count)?;
    let mut resource: Option<ID3D12Resource> = None;
    if let Err(error) = this.device.CreatePlacedResource(&heap, first as u64 * slot, &desc, state, clear, &mut resource) {
        log::warn!("native renderer: GPU memory: {error}");
        pool.lock().unwrap().give(slab, first, count);
        return None;
    }
    let resource = resource?;
    // Skia hands over an empty holder
    let old = *allocation;
    if !old.is_null() {
        unref(old);
    }
    *allocation = Box::into_raw(Box::new(Alloc { vtable: &ALLOC_VTABLE, refs: AtomicI32::new(1), pool: pool.clone(), slab, first, count }));
    Some(resource)
}

/// SkRefCnt's `unref`
unsafe fn unref(alloc: *mut Alloc) {
    if (*alloc).refs.fetch_sub(1, std::sync::atomic::Ordering::AcqRel) == 1 {
        ((*alloc).vtable.dispose)(alloc);
    }
}

/// `createAliasingResource`, which Skia never calls
unsafe extern "system" fn alias(
    _this: *mut Allocator,
    out: *mut *mut c_void,
    _allocation: *mut c_void,
    _offset: u64,
    _desc: *const D3D12_RESOURCE_DESC,
    _state: D3D12_RESOURCE_STATES,
    _clear: *const D3D12_CLEAR_VALUE,
) -> *mut *mut c_void {
    *out = std::ptr::null_mut();
    out
}
