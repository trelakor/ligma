//! Skia compiles each GPU program it meets with D3DCompile, which takes 20
//! to 300 ms a program, and keeps them only while it runs, so a pan into
//! something not drawn yet waited on the compiler. Ligma's own import of
//! D3DCompile is pointed here, which keeps what it compiles on disk, so a
//! program is compiled once ever, then read back in well under a millisecond

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
    fn VirtualProtect(address: *mut c_void, size: usize, protect: u32, old: *mut u32) -> i32;
}

#[link(name = "d3dcompiler")]
extern "system" {
    fn D3DCreateBlob(size: usize, blob: *mut *mut c_void) -> HRESULT;
}

const PAGE_READWRITE: u32 = 0x04;

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
