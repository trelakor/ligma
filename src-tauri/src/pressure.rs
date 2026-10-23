//! System memory pressure (macOS only).
//!
//! WKWebView has no API to trim a page like WebView2 does, and freezing one
//! (see `wkwebview`) keeps its memory. WebKit already throttles timers in
//! hidden pages and cleans up web processes with no visible page; the one
//! thing that frees a hidden Figma file's memory is closing its webview. So while macOS is short of memory, tabs go to sleep
//! sooner (see `tabs::rest_after`).

/// How short the system is on memory.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pressure {
    Normal,
    Warn,
    Critical,
}

/// Reads `kern.memorystatus_vm_pressure_level`, the level behind
/// `DISPATCH_SOURCE_TYPE_MEMORYPRESSURE` (1 normal, 2 warn, 4 critical).
pub fn current() -> Pressure {
    let mut level: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let result = unsafe {
        libc::sysctlbyname(
            c"kern.memorystatus_vm_pressure_level".as_ptr(),
            (&mut level as *mut libc::c_int).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    match level {
        _ if result != 0 => Pressure::Normal,
        4 => Pressure::Critical,
        2 => Pressure::Warn,
        _ => Pressure::Normal,
    }
}
