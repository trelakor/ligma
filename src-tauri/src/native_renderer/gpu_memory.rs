//! GPU memory each process has on the card, from Windows' `GPU Process
//! Memory` performance counters, for Ligma's own process, where the native
//! renderer draws, and for WebView2's processes under it, where Figma draws.
//! Not the dedicated figure Task Manager shows, which counts memory meant
//! for the card whether it's there or not, memory the driver keeps after
//! it's freed too, and so climbs past what's used after a busy moment

use std::collections::HashMap;

use windows::core::w;
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_LARGE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
};

pub struct GpuMemory {
    query: PDH_HQUERY,
    local: PDH_HCOUNTER,
}

// The handles are only used from the thread that samples
unsafe impl Send for GpuMemory {}

#[derive(Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    /// Ligma's own process: the native renderer
    pub ligma: u64,
    /// WebView2's processes under Ligma, nearly all of it its GPU process:
    /// Figma's renderer, and the tab bar and other pages
    pub webview: u64,
}

impl GpuMemory {
    pub fn new() -> Option<Self> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(None, 0, &mut query) != 0 {
                return None;
            }
            let mut local = PDH_HCOUNTER::default();
            if PdhAddEnglishCounterW(query, w!("\\GPU Process Memory(*)\\Local Usage"), 0, &mut local) != 0 {
                PdhCloseQuery(query);
                return None;
            }
            PdhCollectQueryData(query);
            Some(Self { query, local })
        }
    }

    /// Bytes on the card by process id
    fn by_process(&self) -> HashMap<u32, u64> {
        let mut out = HashMap::new();
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return out;
            }
            let (mut size, mut count) = (0u32, 0u32);
            if PdhGetFormattedCounterArrayW(self.local, PDH_FMT_LARGE, &mut size, &mut count, None) != PDH_MORE_DATA {
                return out;
            }
            // u64-aligned, as the items hold 64-bit values
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            let items = buffer.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            if PdhGetFormattedCounterArrayW(self.local, PDH_FMT_LARGE, &mut size, &mut count, Some(items)) != 0 {
                return out;
            }
            for item in std::slice::from_raw_parts(items, count as usize) {
                // "pid_1234_luid_0x00000000_0x0000D1B4_phys_0"
                let name = item.szName.to_string().unwrap_or_default();
                let Some(pid) = name.strip_prefix("pid_").and_then(|rest| rest.split('_').next()).and_then(|pid| pid.parse().ok()) else {
                    continue;
                };
                *out.entry(pid).or_default() += item.FmtValue.Anonymous.largeValue.max(0) as u64;
            }
        }
        out
    }

    pub fn sample(&self) -> Sample {
        let by_process = self.by_process();
        let me = std::process::id();
        let descendants = descendants(me);
        Sample {
            ligma: by_process.get(&me).copied().unwrap_or(0),
            webview: descendants.iter().filter_map(|pid| by_process.get(pid)).sum(),
        }
    }
}

impl Drop for GpuMemory {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}

/// Processes started by `root`, and by them, and so on, from one query of
/// Windows', as Toolhelp's snapshot takes tens of milliseconds
fn descendants(root: u32) -> Vec<u32> {
    super::monitor::descendants(&super::monitor::system_processes(&mut Vec::new(), 0), root)
}
