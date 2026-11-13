//! A memory allocator for Skia that gives textures and buffers memory of
//! their own, freed the moment Skia lets go of them. Skia's own allocator
//! carves them out of heaps it grows to 32, 64 and 128 MB and never gives
//! back while anything at all is left in them, which held hundreds of MB
//! for a few images
//!
//! Memory of its own takes at least a 64 KiB page, and making it takes the
//! driver a while, up to a millisecond, so resources up to 1 MiB, and
//! render targets up to 32, share heaps of their kind instead, each freed
//! once nothing is left in it: textures small enough to be placed at 4 KiB,
//! masks and tables Skia makes by the thousand, other textures, render
//! targets small and big, and buffers to upload through. Every texture
//! starts out all transparent, wherever its memory comes from: Skia reads
//! parts of some before it writes them, as a blur does of a new layer or of
//! the copy of what's behind it, which in memory used before showed another
//! image's pixels. A render target placed in memory used before is cleared,
//! another texture copied zeros into, and one of its own is made zeroed
//! (see `Clears`)
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
    ID3D12CommandAllocator, ID3D12CommandList, ID3D12CommandQueue, ID3D12DescriptorHeap, ID3D12Device, ID3D12Fence, D3D12_DESCRIPTOR_HEAP_DESC,
    D3D12_DESCRIPTOR_HEAP_TYPE_RTV, ID3D12GraphicsCommandList, ID3D12Heap, ID3D12Resource,
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_FENCE_FLAG_NONE, D3D12_HEAP_FLAG_ALLOW_ONLY_RT_DS_TEXTURES, D3D12_RESOURCE_ALIASING_BARRIER, D3D12_RESOURCE_BARRIER,
    D3D12_RESOURCE_BARRIER_0, D3D12_RESOURCE_BARRIER_FLAG_NONE, D3D12_RESOURCE_BARRIER_TYPE_ALIASING, D3D12_CLEAR_VALUE, D3D12_FEATURE_D3D12_OPTIONS7, D3D12_FEATURE_DATA_D3D12_OPTIONS7, D3D12_HEAP_DESC,
    D3D12_HEAP_FLAGS, D3D12_HEAP_FLAG_ALLOW_ONLY_BUFFERS, D3D12_HEAP_FLAG_ALLOW_ONLY_NON_RT_DS_TEXTURES,
    D3D12_HEAP_FLAG_NONE, D3D12_HEAP_PROPERTIES, D3D12_HEAP_TYPE, D3D12_HEAP_TYPE_DEFAULT, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_DESC,
    D3D12_RESOURCE_DIMENSION_BUFFER, D3D12_RESOURCE_DIMENSION_TEXTURE2D, D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL, D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
    D3D12_RESOURCE_STATES, D3D12_PLACED_SUBRESOURCE_FOOTPRINT, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
    D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_GENERIC_READ, D3D12_RESOURCE_TRANSITION_BARRIER, D3D12_TEXTURE_COPY_LOCATION, D3D12_TEXTURE_COPY_LOCATION_0,
    D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT, D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX, D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;

/// Its memory isn't cleared first, for heaps, whose resources are each
/// cleared as they're placed
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
const BIG_TARGETS: usize = 3;
const UPLOADS: usize = 4;

static KINDS_PLACED: [Kind; 5] = [
    Kind { heap: D3D12_HEAP_TYPE_DEFAULT, flags: D3D12_HEAP_FLAG_ALLOW_ONLY_NON_RT_DS_TEXTURES, slab: 4 << 20, slot: 4 << 10, largest: 64 << 10 },
    Kind { heap: D3D12_HEAP_TYPE_DEFAULT, flags: D3D12_HEAP_FLAG_ALLOW_ONLY_NON_RT_DS_TEXTURES, slab: 8 << 20, slot: 64 << 10, largest: 1 << 20 },
    Kind { heap: D3D12_HEAP_TYPE_DEFAULT, flags: D3D12_HEAP_FLAG_ALLOW_ONLY_RT_DS_TEXTURES, slab: 16 << 20, slot: 64 << 10, largest: 4 << 20 },
    Kind { heap: D3D12_HEAP_TYPE_DEFAULT, flags: D3D12_HEAP_FLAG_ALLOW_ONLY_RT_DS_TEXTURES, slab: 64 << 20, slot: 1 << 20, largest: 32 << 20 },
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
    pools: [Arc<Mutex<Pool>>; 5],
    /// None if it couldn't be set up, and render targets get memory of
    /// their own, which starts cleared
    clears: Option<Mutex<Clears>>,
}

/// Clears textures as they're placed, to transparent, on the queue Skia
/// draws with, ahead of anything Skia sends it that uses them: render
/// targets cleared, others copied zeros into. Depth and stencil, which
/// Skia clears before it uses, are discarded instead. Each goes in a
/// command list of its own, sent at once, as Skia gives no word of when it
/// sends its own. They all come from one command allocator, reset whenever
/// the GPU's done with all it holds
struct Clears {
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    fence: ID3D12Fence,
    sent: u64,
    /// Where a target's view is made to clear it through, read as the
    /// clear's recorded
    views: ID3D12DescriptorHeap,
    /// Zeros to copy from, as big as the biggest texture copied into yet
    zeros: Option<(ID3D12Resource, u64)>,
    /// Zeros outgrown, let go once the GPU passes the fence value with each
    outgrown: Vec<(u64, ID3D12Resource)>,
}

impl Clears {
    fn new(device: &ID3D12Device, queue: &ID3D12CommandQueue) -> windows::core::Result<Self> {
        unsafe {
            let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
            let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)?;
            list.Close()?;
            let fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
            let views = device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV, NumDescriptors: 1, ..Default::default() })?;
            Ok(Clears { device: device.clone(), queue: queue.clone(), allocator, list, fence, sent: 0, views, zeros: None, outgrown: Vec::new() })
        }
    }

    /// `resource`, just placed, in the state Skia makes render targets in,
    /// render target or depth write, as clearing and discarding take
    fn clear(&mut self, resource: &ID3D12Resource, depth: bool) -> windows::core::Result<()> {
        unsafe {
            self.begin(resource)?;
            if depth {
                self.list.DiscardResource(resource, None);
            } else {
                let view = self.views.GetCPUDescriptorHandleForHeapStart();
                self.device.CreateRenderTargetView(resource, None, view);
                self.list.ClearRenderTargetView(view, &[0.0; 4], None);
            }
            self.send()
        }
    }

    /// `resource`, a texture just placed with one level as `desc` says, in
    /// `state`, made all zeros
    fn zero(&mut self, resource: &ID3D12Resource, desc: &D3D12_RESOURCE_DESC, state: D3D12_RESOURCE_STATES) -> windows::core::Result<()> {
        unsafe {
            let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
            let mut total = 0u64;
            self.device.GetCopyableFootprints(desc, 0, 1, 0, Some(&mut footprint), None, None, Some(&mut total));
            if total == 0 || total == u64::MAX {
                return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG));
            }
            let done = self.fence.GetCompletedValue();
            self.outgrown.retain(|(until, _)| *until > done);
            if !self.zeros.as_ref().is_some_and(|(_, size)| *size >= total) {
                let size = total.max(1 << 20);
                let buffer = D3D12_RESOURCE_DESC {
                    Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                    Width: size,
                    Height: 1,
                    DepthOrArraySize: 1,
                    MipLevels: 1,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                    ..Default::default()
                };
                let properties = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_UPLOAD, ..Default::default() };
                let mut zeros: Option<ID3D12Resource> = None;
                self.device.CreateCommittedResource(&properties, D3D12_HEAP_FLAG_NONE, &buffer, D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut zeros)?;
                let zeros = zeros.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_OUTOFMEMORY))?;
                // Made zeroed, but written so it's sure to be
                let mut at = std::ptr::null_mut();
                zeros.Map(0, None, Some(&mut at))?;
                std::ptr::write_bytes(at as *mut u8, 0, size as usize);
                zeros.Unmap(0, None);
                if let Some((old, _)) = self.zeros.replace((zeros, size)) {
                    self.outgrown.push((self.sent + 1, old));
                }
            }
            let Some((zeros, _)) = &self.zeros else { return Ok(()) };
            let zeros = zeros.clone();
            self.begin(resource)?;
            let moving = state != D3D12_RESOURCE_STATE_COPY_DEST;
            if moving {
                self.list.ResourceBarrier(&[transition(resource, state, D3D12_RESOURCE_STATE_COPY_DEST)]);
            }
            let to = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(resource),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
            };
            let from = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::transmute_copy(&zeros),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: footprint },
            };
            self.list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
            if moving {
                self.list.ResourceBarrier(&[transition(resource, D3D12_RESOURCE_STATE_COPY_DEST, state)]);
            }
            self.send()
        }
    }

    /// Starts a command list for `resource`, just placed
    unsafe fn begin(&mut self, resource: &ID3D12Resource) -> windows::core::Result<()> {
        if self.fence.GetCompletedValue() >= self.sent {
            self.allocator.Reset()?;
        }
        self.list.Reset(&self.allocator, None)?;
        let barrier = D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_ALIASING,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Aliasing: std::mem::ManuallyDrop::new(D3D12_RESOURCE_ALIASING_BARRIER {
                    pResourceBefore: std::mem::ManuallyDrop::new(None),
                    // Borrowed, not dropped
                    pResourceAfter: std::mem::transmute_copy(resource),
                }),
            },
        };
        self.list.ResourceBarrier(&[barrier]);
        Ok(())
    }

    unsafe fn send(&mut self) -> windows::core::Result<()> {
        self.list.Close()?;
        self.queue.ExecuteCommandLists(&[Some(self.list.cast::<ID3D12CommandList>()?)]);
        self.sent += 1;
        self.queue.Signal(&self.fence, self.sent)
    }
}

/// A barrier taking all of `resource`, borrowed, from `before` to `after`
unsafe fn transition(resource: &ID3D12Resource, before: D3D12_RESOURCE_STATES, after: D3D12_RESOURCE_STATES) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: std::mem::transmute_copy(resource),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
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

/// An allocator for a Skia context on `device` drawing with `queue`
pub fn allocator(device: &ID3D12Device, queue: &ID3D12CommandQueue) -> MemoryAllocator {
    // Older Windows can't skip clearing
    let zeroed_only = {
        let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS7::default();
        unsafe { device.CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS7, &mut options as *mut _ as *mut c_void, std::mem::size_of_val(&options) as u32) }.is_err()
    };
    let pools: [Arc<Mutex<Pool>>; 5] = std::array::from_fn(|kind| Arc::new(Mutex::new(Pool { kind: &KINDS_PLACED[kind], not_zeroed: !zeroed_only, slabs: Vec::new() })));
    POOLS.lock().unwrap().extend(pools.iter().map(Arc::downgrade));
    let clears = Clears::new(device, queue).map_err(|error| log::warn!("native renderer: clears: {error}")).ok().map(Mutex::new);
    let allocator = Box::into_raw(Box::new(Allocator { vtable: &VTABLE, refs: AtomicI32::new(1), device: device.clone(), zeroed_only, pools, clears }));
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
    // Only made by Skia making mipmaps on the GPU, which it mustn't be
    // asked to (see `draw::sampled`)
    if (*desc).Flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 != 0 {
        super::render::trace(|| "Skia made mipmaps on the GPU".to_string());
    }
    let _tally = Tally(heap, *desc, started);
    let properties = D3D12_HEAP_PROPERTIES { Type: heap, ..Default::default() };
    // Made zeroed, see the top
    let mut resource: Option<ID3D12Resource> = None;
    match this.device.CreateCommittedResource(&properties, D3D12_HEAP_FLAG_NONE, desc, state, clear, &mut resource) {
        Ok(()) => {
            if let Some(resource) = resource {
                let _ = resource.SetName(windows::core::w!("Ligma committed"));
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
    // Mipmapped images have memory of their own
    if allocation.is_null() || desc.SampleDesc.Count > 1 || desc.MipLevels > 1 {
        return None;
    }
    let targets = desc.Flags.0 & (D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET.0 | D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL.0) != 0;
    let mut desc = *desc;
    // Textures placed are cleared, which takes `clears`
    let texture = heap == D3D12_HEAP_TYPE_DEFAULT && desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    if texture && this.clears.is_none() {
        return None;
    }
    let kind = if heap == D3D12_HEAP_TYPE_UPLOAD && desc.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER {
        UPLOADS
    } else if heap != D3D12_HEAP_TYPE_DEFAULT || desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D {
        return None;
    } else if targets {
        let size = this.device.GetResourceAllocationInfo(0, &[desc]).SizeInBytes;
        if size > KINDS_PLACED[TARGETS].largest { BIG_TARGETS } else { TARGETS }
    } else {
        // Placed at 4 KiB if it's small enough to be, which takes it being
        // 64 KiB at most, so only those that could be are asked about
        let mut small = desc;
        small.Alignment = KINDS_PLACED[TINY_TEXTURES].slot;
        let could = small_tiles(&desc) <= 16;
        let info = if could { this.device.GetResourceAllocationInfo(0, &[small]) } else { Default::default() };
        if could && info.Alignment == small.Alignment && info.SizeInBytes <= KINDS_PLACED[TINY_TEXTURES].largest {
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
    let Some(resource) = resource else {
        pool.lock().unwrap().give(slab, first, count);
        return None;
    };
    if let (true, Some(clears)) = (texture, &this.clears) {
        let depth = desc.Flags.0 & D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL.0 != 0;
        let cleared = if targets { clears.lock().unwrap().clear(&resource, depth) } else { clears.lock().unwrap().zero(&resource, &desc, state) };
        if let Err(error) = cleared {
            log::warn!("native renderer: clear: {error}");
            drop(resource);
            pool.lock().unwrap().give(slab, first, count);
            return None;
        }
    }
    let _ = resource.SetName(windows::core::w!("Ligma placed"));
    // Skia hands over an empty holder
    let old = *allocation;
    if !old.is_null() {
        unref(old);
    }
    *allocation = Box::into_raw(Box::new(Alloc { vtable: &ALLOC_VTABLE, refs: AtomicI32::new(1), pool: pool.clone(), slab, first, count }));
    Some(resource)
}

/// The 4 KiB tiles the top level of texture `desc` takes, which can be 16
/// at most for it to be placed at 4 KiB. Formats not known count as 16
/// bytes a pixel, which is never fewer tiles than they take
fn small_tiles(desc: &D3D12_RESOURCE_DESC) -> u64 {
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8_UNORM, DXGI_FORMAT_R8_UNORM};
    // A tile's width and height in pixels, by bytes a pixel
    let (width, height) = match desc.Format {
        DXGI_FORMAT_A8_UNORM | DXGI_FORMAT_R8_UNORM => (64, 64),
        DXGI_FORMAT_R8G8_UNORM => (64, 32),
        DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM => (32, 32),
        DXGI_FORMAT_R16G16B16A16_FLOAT => (32, 16),
        _ => (16, 16),
    };
    desc.Width.div_ceil(width) * (desc.Height as u64).div_ceil(height)
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
