//! Skia compiles each GPU program it meets with D3DCompile, which takes 20
//! to 300 ms a program, and keeps them only while it runs, so a pan into
//! something not drawn yet waited on the compiler. Ligma's own import of
//! D3DCompile is pointed here, which keeps what it compiles on disk, so a
//! program is compiled once ever, then read back in well under a millisecond.
//! Custom effects' programs, compiled by wgpu, which finds D3DCompile by name,
//! go through the cache too

use std::ffi::{c_char, c_void, CStr};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

use windows::core::{Interface, HRESULT};
use windows::Win32::Graphics::Direct3D::ID3DBlob;

type Compile = unsafe extern "system" fn(
    source: *const c_void,
    size: usize,
    name: *const c_char,
    defines: *const [*const c_char; 2],
    include: *mut c_void,
    entry: *const c_char,
    target: *const c_char,
    flags1: u32,
    flags2: u32,
    code: *mut *mut c_void,
    errors: *mut *mut c_void,
) -> HRESULT;

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut u8;
    fn GetModuleHandleExW(flags: u32, name: *const u16, module: *mut *mut u8) -> i32;
    fn VirtualProtect(address: *mut c_void, size: usize, protect: u32, old: *mut u32) -> i32;
    fn VirtualAlloc(address: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut u8;
    fn FlushInstructionCache(process: *mut c_void, address: *const c_void, size: usize) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
}

#[link(name = "d3dcompiler")]
extern "system" {
    fn D3DCreateBlob(size: usize, blob: *mut *mut c_void) -> HRESULT;
}

const PAGE_READWRITE: u32 = 0x04;
const PAGE_EXECUTE_READWRITE: u32 = 0x40;
const MEM_COMMIT_RESERVE: u32 = 0x3000;

struct Cache {
    original: Compile,
    dir: Option<PathBuf>,
}

static CACHE: OnceLock<Cache> = OnceLock::new();

/// Points Ligma's import of D3DCompile at the cache, once
pub fn install() {
    CACHE.get_or_init(|| {
        let dir = std::env::var_os("LOCALAPPDATA").map(|local| PathBuf::from(local).join("dev.ligma.client").join("native-shaders"));
        if let Some(dir) = &dir {
            let _ = std::fs::create_dir_all(dir);
        }
        let original = match unsafe { patch(b"d3dcompiler_47.dll", b"D3DCompile", cached as *const c_void) } {
            Some(original) => unsafe { std::mem::transmute::<*const c_void, Compile>(original) },
            None => {
                log::warn!("native renderer: couldn't find D3DCompile to keep its programs");
                fallback
            }
        };
        if original as *const () != fallback as *const () && !unsafe { patch_export(b"D3DCompile\0", original as *const c_void, cached as *const c_void) } {
            log::warn!("native renderer: couldn't keep effects' programs");
        }
        Cache { original, dir }
    });
}

/// Only reached if the import isn't found, when nothing calls it
unsafe extern "system" fn fallback(_: *const c_void, _: usize, _: *const c_char, _: *const [*const c_char; 2], _: *mut c_void, _: *const c_char, _: *const c_char, _: u32, _: u32, _: *mut *mut c_void, _: *mut *mut c_void) -> HRESULT {
    HRESULT(0x80004005u32 as i32)
}

/// Replaces `function` of `dll` in this executable's import table with
/// `with`, returning what it was
unsafe fn patch(dll: &[u8], function: &[u8], with: *const c_void) -> Option<*const c_void> {
    let base = GetModuleHandleW(std::ptr::null());
    if base.is_null() {
        return None;
    }
    let read32 = |offset: usize| (base.add(offset) as *const u32).read_unaligned();
    let nt = read32(0x3c) as usize;
    // The import directory, second of the optional header's data directories
    let imports = read32(nt + 24 + 112 + 8) as usize;
    if imports == 0 {
        return None;
    }
    let mut descriptor = imports;
    loop {
        let (names, name, thunks) = (read32(descriptor) as usize, read32(descriptor + 12) as usize, read32(descriptor + 16) as usize);
        if name == 0 {
            return None;
        }
        let module = CStr::from_ptr(base.add(name) as *const c_char).to_bytes();
        if module.eq_ignore_ascii_case(dll) {
            let lookup = if names != 0 { names } else { thunks };
            let mut index = 0;
            loop {
                let entry = (base.add(lookup + index * 8) as *const u64).read_unaligned();
                if entry == 0 {
                    break;
                }
                if entry & (1 << 63) == 0 {
                    let imported = CStr::from_ptr(base.add(entry as usize + 2) as *const c_char).to_bytes();
                    if imported == function {
                        let slot = base.add(thunks + index * 8) as *mut *const c_void;
                        let mut old = 0;
                        if VirtualProtect(slot as *mut c_void, 8, PAGE_READWRITE, &mut old) == 0 {
                            return None;
                        }
                        let original = slot.read_volatile();
                        slot.write_volatile(with);
                        VirtualProtect(slot as *mut c_void, 8, old, &mut old);
                        return Some(original);
                    }
                }
                index += 1;
            }
        }
        descriptor += 20;
    }
}

/// Points `function` of the loaded DLL that holds `real` at `with` for
/// whoever looks it up by name from now on. An export is an offset from the
/// DLL, so it goes by way of a jump placed within reach of it
unsafe fn patch_export(function: &[u8], real: *const c_void, with: *const c_void) -> bool {
    let mut module = std::ptr::null_mut();
    // GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | UNCHANGED_REFCOUNT
    if GetModuleHandleExW(0x4 | 0x2, real as *const u16, &mut module) == 0 || module.is_null() {
        return false;
    }
    let base = module;
    let read32 = |offset: usize| (base.add(offset) as *const u32).read_unaligned();
    let nt = read32(0x3c) as usize;
    let image = read32(nt + 24 + 56) as usize;
    let exports = read32(nt + 24 + 112) as usize;
    if exports == 0 {
        return false;
    }
    let (count, functions, names, ordinals) = (read32(exports + 24) as usize, read32(exports + 28) as usize, read32(exports + 32) as usize, read32(exports + 36) as usize);
    let wanted = CStr::from_bytes_with_nul(function).unwrap();
    let Some(index) = (0..count).find(|&index| CStr::from_ptr(base.add(read32(names + index * 4) as usize) as *const c_char) == wanted) else {
        return false;
    };
    let ordinal = (base.add(ordinals + index * 2) as *const u16).read_unaligned() as usize;
    let slot = base.add(functions + ordinal * 4) as *mut u32;
    if base.add(slot.read_unaligned() as usize) as *const c_void != real {
        return false;
    }
    // The first free 64 KiB above the DLL, to hold `jmp [rip]` and where to
    let mut at = (base as usize + image + 0xffff) & !0xffff;
    let jump = loop {
        if at - base as usize > 0x7000_0000 {
            return false;
        }
        let made = VirtualAlloc(at as *mut c_void, 4096, MEM_COMMIT_RESERVE, PAGE_EXECUTE_READWRITE);
        if !made.is_null() {
            break made;
        }
        at += 0x10000;
    };
    let mut code = [0xff, 0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    code[6..].copy_from_slice(&(with as usize as u64).to_le_bytes());
    std::ptr::copy_nonoverlapping(code.as_ptr(), jump, code.len());
    FlushInstructionCache(GetCurrentProcess(), jump as *const c_void, code.len());
    let mut old = 0;
    if VirtualProtect(slot as *mut c_void, 4, PAGE_READWRITE, &mut old) == 0 {
        return false;
    }
    slot.write_volatile((jump as usize - base as usize) as u32);
    VirtualProtect(slot as *mut c_void, 4, old, &mut old);
    true
}

/// A 128 bit key for what's compiled and how
fn key(source: &[u8], entry: &[u8], target: &[u8], flags: (u32, u32), defines: &[(&[u8], &[u8])]) -> u128 {
    let half = |seed: u64| {
        let mut hasher = std::hash::DefaultHasher::new();
        seed.hash(&mut hasher);
        source.hash(&mut hasher);
        entry.hash(&mut hasher);
        target.hash(&mut hasher);
        flags.hash(&mut hasher);
        defines.hash(&mut hasher);
        hasher.finish()
    };
    ((half(0x9e37_79b9_7f4a_7c15) as u128) << 64) | half(0x2545_f491_4f6c_dd1d) as u128
}

fn blob(bytes: &[u8]) -> Option<*mut c_void> {
    let mut blob = std::ptr::null_mut();
    if unsafe { D3DCreateBlob(bytes.len(), &mut blob) }.is_err() || blob.is_null() {
        return None;
    }
    let made = unsafe { ID3DBlob::from_raw_borrowed(&blob) }?;
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), made.GetBufferPointer() as *mut u8, bytes.len()) };
    Some(blob)
}

/// Dev tools: programs asked for so far
pub static ASKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static ASKED_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

unsafe extern "system" fn cached(
    source: *const c_void,
    size: usize,
    name: *const c_char,
    defines: *const [*const c_char; 2],
    include: *mut c_void,
    entry: *const c_char,
    target: *const c_char,
    flags1: u32,
    flags2: u32,
    code: *mut *mut c_void,
    errors: *mut *mut c_void,
) -> HRESULT {
    ASKED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let started = Instant::now();
    let result = cached_inner(source, size, name, defines, include, entry, target, flags1, flags2, code, errors);
    ASKED_NS.fetch_add(started.elapsed().as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
    result
}

#[allow(clippy::too_many_arguments)]
unsafe fn cached_inner(
    source: *const c_void,
    size: usize,
    name: *const c_char,
    defines: *const [*const c_char; 2],
    include: *mut c_void,
    entry: *const c_char,
    target: *const c_char,
    flags1: u32,
    flags2: u32,
    code: *mut *mut c_void,
    errors: *mut *mut c_void,
) -> HRESULT {
    let cache = CACHE.get().unwrap();
    let compile = || (cache.original)(source, size, name, defines, include, entry, target, flags1, flags2, code, errors);
    // What an include handler adds isn't known, so it's compiled each time
    if source.is_null() || code.is_null() || !include.is_null() {
        return compile();
    }
    let text = std::slice::from_raw_parts(source as *const u8, size);
    let string = |pointer: *const c_char| if pointer.is_null() { &[][..] } else { CStr::from_ptr(pointer).to_bytes() };
    let mut macros = Vec::new();
    if !defines.is_null() {
        let mut at = defines;
        while !(*at)[0].is_null() {
            macros.push((string((*at)[0]), string((*at)[1])));
            at = at.add(1);
        }
    }
    let key = key(text, string(entry), string(target), (flags1, flags2), &macros);
    let file = cache.dir.as_ref().map(|dir| dir.join(format!("{key:032x}.dxbc")));
    let known = file.as_ref().and_then(|file| std::fs::read(file).ok());
    if let Some(bytes) = known {
        if let Some(made) = blob(&bytes) {
            *code = made;
            if !errors.is_null() {
                *errors = std::ptr::null_mut();
            }
            return HRESULT(0);
        }
    }
    let start = Instant::now();
    let result = compile();
    let took = start.elapsed();
    if result.is_ok() && !(*code).is_null() {
        let made = ID3DBlob::from_raw_borrowed(&*code).unwrap();
        let bytes = std::slice::from_raw_parts(made.GetBufferPointer() as *const u8, made.GetBufferSize());
        if let Some(file) = &file {
            // Written aside first, so a file is never read half written
            let part = file.with_extension("part");
            if std::fs::write(&part, bytes).is_ok() {
                let _ = std::fs::rename(&part, file);
            }
        }
    }
    super::render::trace(|| format!("compiled {} program {:.1}ms", String::from_utf8_lossy(string(target)), took.as_secs_f64() * 1000.0));
    result
}
