//! Dedicated GPU memory per process, from Windows' `GPU Process Memory`
//! performance counters (what Task Manager shows), for Ligma's own process,
//! where the native renderer draws, and for WebView2's processes under it,
//! where Figma draws

use std::collections::HashMap;

use windows::core::w;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_LARGE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
};

pub struct GpuMemory {
    query: PDH_HQUERY,
    dedicated: PDH_HCOUNTER,
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
            let mut dedicated = PDH_HCOUNTER::default();
            if PdhAddEnglishCounterW(query, w!("\\GPU Process Memory(*)\\Dedicated Usage"), 0, &mut dedicated) != 0 {
                PdhCloseQuery(query);
                return None;
            }
            PdhCollectQueryData(query);
            Some(Self { query, dedicated })
        }
    }

    /// Dedicated bytes by process id
    fn by_process(&self) -> HashMap<u32, u64> {
        let mut out = HashMap::new();
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return out;
            }
            let (mut size, mut count) = (0u32, 0u32);
            if PdhGetFormattedCounterArrayW(self.dedicated, PDH_FMT_LARGE, &mut size, &mut count, None) != PDH_MORE_DATA {
                return out;
            }
            // u64-aligned, as the items hold 64-bit values
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            let items = buffer.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            if PdhGetFormattedCounterArrayW(self.dedicated, PDH_FMT_LARGE, &mut size, &mut count, Some(items)) != 0 {
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

/// Processes started by `root`, and by them, and so on
fn descendants(root: u32) -> Vec<u32> {
    let mut parents: Vec<(u32, u32)> = Vec::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return Vec::new();
        };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                parents.push((entry.th32ProcessID, entry.th32ParentProcessID));
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    let mut found = vec![root];
    let mut at = 0;
    while at < found.len() {
        let parent = found[at];
        for &(pid, ppid) in &parents {
            if ppid == parent && pid != parent && !found.contains(&pid) {
                found.push(pid);
            }
        }
        at += 1;
    }
    found.remove(0);
    found
}
