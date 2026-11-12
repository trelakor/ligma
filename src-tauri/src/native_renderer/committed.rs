//! A memory allocator for Skia that gives textures and buffers memory of
//! their own, freed the moment Skia lets go of them. Skia's own allocator
//! carves them out of heaps it grows to 32, 64 and 128 MB and never gives
//! back while anything at all is left in them, which held hundreds of MB
//! for a few images
//!
//! Memory of its own takes at least a 64 KiB page, so textures small enough
//! to be placed at 4 KiB, masks and tables Skia makes by the thousand, share
//! 4 MiB heaps instead, each freed once nothing is left in it
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
    D3D12_HEAP_FLAGS, D3D12_HEAP_FLAG_ALLOW_ONLY_NON_RT_DS_TEXTURES, D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE, D3D12_HEAP_TYPE_DEFAULT,
    D3D12_RESOURCE_DESC, D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
    D3D12_RESOURCE_STATES,
};

/// Its memory isn't cleared first, as with Skia's own allocator: Skia
/// clears or draws over what it uses
const NOT_ZEROED: D3D12_HEAP_FLAGS = D3D12_HEAP_FLAGS(0x1000);

/// Small textures' heaps, and the steps they're placed in
const SLAB: u64 = 4 << 20;
const SLOT: u64 = 4096;
const SLOTS: usize = (SLAB / SLOT) as usize;
const SMALL: u64 = 64 << 10;

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
    pool: Arc<Mutex<Pool>>,
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

#[derive(Default)]
struct Pool {
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
        for (index, slab) in self.slabs.iter_mut().enumerate() {
            let Some(slab) = slab else { continue };
            if SLOTS - slab.in_use < count {
                continue;
            }
            let mut run = 0;
            for slot in 0..SLOTS {
                run = if slab.used[slot] { 0 } else { run + 1 };
                if run == count {
                    let first = slot + 1 - count;
                    slab.used[first..=slot].fill(true);
                    slab.in_use += count;
                    return Some((index, first, slab.heap.clone()));
                }
            }
        }
        let desc = D3D12_HEAP_DESC {
            SizeInBytes: SLAB,
            Properties: D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() },
            Alignment: 0,
            Flags: D3D12_HEAP_FLAG_ALLOW_ONLY_NON_RT_DS_TEXTURES,
        };
        let mut heap: Option<ID3D12Heap> = None;
        unsafe { device.CreateHeap(&desc, &mut heap) }.map_err(|error| log::warn!("native renderer: GPU memory: {error}")).ok()?;
        let heap = heap?;
        let mut used = vec![false; SLOTS];
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
            let others_have_room = self.slabs.iter().enumerate().any(|(other, slab)| other != index && slab.as_ref().is_some_and(|slab| slab.in_use < SLOTS));
            if others_have_room {
                self.slabs[index] = None;
            }
        }
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
    let allocator = Box::into_raw(Box::new(Allocator { vtable: &VTABLE, refs: AtomicI32::new(1), device: device.clone(), zeroed_only, pool: Arc::default() }));
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

/// `createResource`: a small texture in a shared heap, anything else a
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
        return out;
    }
    let properties = D3D12_HEAP_PROPERTIES { Type: heap, ..Default::default() };
    let flags = if heap == D3D12_HEAP_TYPE_DEFAULT && !this.zeroed_only { NOT_ZEROED } else { D3D12_HEAP_FLAG_NONE };
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

/// A small texture placed in a shared heap, if it is one
unsafe fn placed(
    this: &Allocator,
    heap: D3D12_HEAP_TYPE,
    desc: &D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
    allocation: *mut *mut Alloc,
    clear: Option<*const D3D12_CLEAR_VALUE>,
) -> Option<ID3D12Resource> {
    let targets = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET.0 | D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL.0;
    if heap != D3D12_HEAP_TYPE_DEFAULT || desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D || desc.SampleDesc.Count > 1 || desc.Flags.0 & targets != 0 || allocation.is_null() {
        return None;
    }
    let mut small = *desc;
    small.Alignment = SLOT;
    let info = this.device.GetResourceAllocationInfo(0, &[small]);
    if info.Alignment != SLOT || info.SizeInBytes > SMALL || info.SizeInBytes == 0 {
        return None;
    }
    let count = info.SizeInBytes.div_ceil(SLOT) as usize;
    let (slab, first, heap) = this.pool.lock().unwrap().take(&this.device, count)?;
    let mut resource: Option<ID3D12Resource> = None;
    if let Err(error) = this.device.CreatePlacedResource(&heap, first as u64 * SLOT, &small, state, clear, &mut resource) {
        log::warn!("native renderer: GPU memory: {error}");
        this.pool.lock().unwrap().give(slab, first, count);
        return None;
    }
    let resource = resource?;
    // Skia hands over an empty holder
    let old = *allocation;
    if !old.is_null() {
        unref(old);
    }
    *allocation = Box::into_raw(Box::new(Alloc { vtable: &ALLOC_VTABLE, refs: AtomicI32::new(1), pool: this.pool.clone(), slab, first, count }));
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
