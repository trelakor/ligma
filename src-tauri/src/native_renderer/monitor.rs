//! Dev tools: while tracing, what the machine is, once, then a few lines a
//! second of where CPU, memory and the GPU go, for Ligma's process and its
//! threads, WebView2's processes under it, where Figma runs, and the whole
//! system. On a thread of its own, so it goes on while the render thread's
//! held up, which is when it's wanted most

use std::collections::HashMap;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::core::{w, Interface, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, LocalFree, FILETIME, HANDLE, HLOCAL};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIDevice, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
use windows::Win32::Graphics::Gdi::{EnumDisplayDevicesW, EnumDisplaySettingsW, DEVMODEW, DISPLAY_DEVICEW, DISPLAY_DEVICE_ACTIVE, ENUM_CURRENT_SETTINGS};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_FMT_LARGE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::{
    GetSystemTimes, GetThreadDescription, OpenProcess, OpenThread, PROCESS_QUERY_LIMITED_INFORMATION, THREAD_QUERY_LIMITED_INFORMATION,
};

use super::render::trace;

const MB: f64 = 1024.0 * 1024.0;

/// Presents this second, the slowest call to Present, the longest time
/// between two, in microseconds, and when the last was
static PRESENTS: std::sync::Mutex<(u32, u64, u64, Option<Instant>)> = std::sync::Mutex::new((0, 0, 0, None));

/// The present thread showed a frame, its call to Present taking `took`
pub fn presented(took: Duration) {
    if !super::render::tracing() {
        return;
    }
    let mut presents = PRESENTS.lock().unwrap();
    let now = Instant::now();
    presents.0 += 1;
    presents.1 = presents.1.max(took.as_micros() as u64);
    if let Some(last) = presents.3 {
        presents.2 = presents.2.max(now.duration_since(last).as_micros() as u64);
    }
    presents.3 = Some(now);
}

/// Starts the lines, once, if tracing
pub fn start() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !super::render::tracing() || STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new().name("native-monitor".into()).spawn(|| {
        trace(machine);
        let mut monitor = Monitor::new();
        loop {
            std::thread::sleep(Duration::from_secs(1));
            monitor.tick();
        }
    });
}

/// What the machine is and how Ligma was started
fn machine() -> String {
    let mut out = format!("machine: ligma {} {}", env!("CARGO_PKG_VERSION"), option_env!("LIGMA_COMMIT").unwrap_or("?"));
    if cfg!(debug_assertions) {
        out += " debug";
    }
    let windows = |value: PCWSTR| registry_string(w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"), value);
    out += &format!(
        " | {} {} build {}.{}",
        windows(w!("ProductName")).unwrap_or_default(),
        windows(w!("DisplayVersion")).unwrap_or_default(),
        windows(w!("CurrentBuild")).unwrap_or_default(),
        registry_dword(w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion"), w!("UBR")).unwrap_or(0)
    );
    let cpu = registry_string(w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0"), w!("ProcessorNameString")).unwrap_or_default();
    let mhz = registry_dword(w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0"), w!("~MHz")).unwrap_or(0);
    out += &format!(" | cpu {} at {mhz} MHz, {} threads", cpu.trim(), std::thread::available_parallelism().map_or(0, |n| n.get()));
    let mut memory = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    if unsafe { GlobalMemoryStatusEx(&mut memory) }.is_ok() {
        out += &format!(" | ram {:.1} GB, {:.1} GB free", memory.ullTotalPhys as f64 / MB / 1024.0, memory.ullAvailPhys as f64 / MB / 1024.0);
    }
    out += &format!(" | power {}", power());
    // Every GPU, with its driver, as a laptop's has two
    if let Ok(factory) = unsafe { CreateDXGIFactory1::<IDXGIFactory1>() } {
        let mut index = 0;
        let mut seen = Vec::new();
        while let Ok(adapter) = unsafe { factory.EnumAdapters1(index) } {
            index += 1;
            let Ok(desc) = (unsafe { adapter.GetDesc1() }) else { continue };
            // Listed once for each of its screens at times
            let luid = (desc.AdapterLuid.HighPart, desc.AdapterLuid.LowPart);
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 || seen.contains(&luid) {
                continue;
            }
            seen.push(luid);
            let name = String::from_utf16_lossy(&desc.Description).trim_end_matches('\0').to_string();
            let driver = unsafe { adapter.CheckInterfaceSupport(&IDXGIDevice::IID) }.map_or_else(|_| "?".to_string(), |version| format!("{}.{}.{}.{}", (version >> 48) & 0xffff, (version >> 32) & 0xffff, (version >> 16) & 0xffff, version & 0xffff));
            out += &format!(
                " | gpu {name} driver {driver}, {:.0} MB dedicated, {:.0} MB shared",
                desc.DedicatedVideoMemory as f64 / MB,
                desc.SharedSystemMemory as f64 / MB
            );
        }
    }
    // Every screen on, with its refresh rate
    let mut index = 0;
    loop {
        let mut device = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), index, &mut device, 0) }.as_bool() {
            break;
        }
        index += 1;
        if (device.StateFlags & DISPLAY_DEVICE_ACTIVE).0 == 0 {
            continue;
        }
        let mut mode = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
        if unsafe { EnumDisplaySettingsW(PCWSTR(device.DeviceName.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode) }.as_bool() {
            let name = String::from_utf16_lossy(&device.DeviceString).trim_end_matches('\0').to_string();
            out += &format!(" | screen {}x{} at {} Hz on {name}", mode.dmPelsWidth, mode.dmPelsHeight, mode.dmDisplayFrequency);
        }
    }
    let mut vars: Vec<String> = std::env::vars().filter(|(key, _)| key.starts_with("LIGMA_") || key.starts_with("WEBVIEW2_")).map(|(key, value)| format!("{key}={value}")).collect();
    vars.sort();
    out += &format!(" | env {}", vars.join(" "));
    out
}

fn power() -> String {
    let mut status = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut status) }.is_err() {
        return "?".into();
    }
    let source = match status.ACLineStatus {
        0 => "battery",
        1 => "plugged in",
        _ => "?",
    };
    let battery = if status.BatteryLifePercent <= 100 { format!(" {}%", status.BatteryLifePercent) } else { String::new() };
    let saver = if status.SystemStatusFlag == 1 { " battery saver" } else { "" };
    format!("{source}{battery}{saver}")
}

fn registry_string(key: PCWSTR, value: PCWSTR) -> Option<String> {
    let mut buffer = [0u16; 256];
    let mut size = (buffer.len() * 2) as u32;
    let result = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key, value, RRF_RT_REG_SZ, None, Some(buffer.as_mut_ptr().cast()), Some(&mut size)) };
    result.is_ok().then(|| String::from_utf16_lossy(&buffer[..(size as usize / 2).saturating_sub(1)]))
}

fn registry_dword(key: PCWSTR, value: PCWSTR) -> Option<u32> {
    let mut out = 0u32;
    let mut size = 4u32;
    let result = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key, value, RRF_RT_REG_DWORD, None, Some((&mut out as *mut u32).cast()), Some(&mut size)) };
    result.is_ok().then_some(out)
}

fn ticks(time: FILETIME) -> u64 {
    ((time.dwHighDateTime as u64) << 32) | time.dwLowDateTime as u64
}

#[link(name = "ntdll")]
extern "system" {
    fn NtQueryInformationProcess(process: HANDLE, class: u32, info: *mut c_void, length: u32, returned: *mut u32) -> i32;
}

/// What a WebView2 process is, from its command line: browser, renderer,
/// gpu-process, or a utility and what it does
fn kind_of(process: HANDLE, exe: &str) -> String {
    let mut buffer = vec![0u64; 8192];
    let mut returned = 0u32;
    // ProcessCommandLineInformation: a UNICODE_STRING, its text after it
    let status = unsafe { NtQueryInformationProcess(process, 60, buffer.as_mut_ptr().cast(), (buffer.len() * 8) as u32, &mut returned) };
    if status < 0 {
        return exe.into();
    }
    let line = unsafe {
        let length = *(buffer.as_ptr() as *const u16) as usize / 2;
        let text = *(buffer.as_ptr().cast::<u8>().add(8) as *const *const u16);
        String::from_utf16_lossy(std::slice::from_raw_parts(text, length))
    };
    let flag = |name: &str| line.split_whitespace().find_map(|part| part.trim_matches('"').strip_prefix(name).map(str::to_string));
    match (flag("--type="), flag("--utility-sub-type=")) {
        (Some(kind), Some(sub)) => format!("{kind}:{}", sub.rsplit('.').next().unwrap_or(&sub)),
        (Some(kind), None) => kind,
        (None, _) if exe.to_lowercase().contains("webview") => "browser".into(),
        (None, _) => exe.into(),
    }
}

/// A process, as Windows lists them all at once
pub(super) struct ProcessInfo {
    pub pid: u32,
    pub parent: u32,
    pub name: String,
    /// Kernel and user time, in 100ns ticks
    pub cpu: u64,
    pub working_set: u64,
    pub private: u64,
    pub faults: u32,
    /// Id, kernel and user time and context switches, for the process
    /// asked for alone
    pub threads: Vec<(u32, u64, u32)>,
}

#[link(name = "ntdll")]
extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut c_void, length: u32, returned: *mut u32) -> i32;
}

/// Every process, and the threads of `threads_of`, in one call, far
/// quicker than Toolhelp's snapshots, which take a hundred milliseconds
/// or more with all of a system's threads
pub(super) fn system_processes(buffer: &mut Vec<u64>, threads_of: u32) -> Vec<ProcessInfo> {
    let mut out = Vec::new();
    if buffer.is_empty() {
        buffer.resize(1 << 17, 0);
    }
    // SystemProcessInformation, grown till it fits
    loop {
        let mut returned = 0u32;
        let status = unsafe { NtQuerySystemInformation(5, buffer.as_mut_ptr().cast(), (buffer.len() * 8) as u32, &mut returned) };
        if status == 0xC000_0004u32 as i32 {
            let grown = (returned as usize / 8 + 4096).max(buffer.len() * 2);
            buffer.resize(grown, 0);
            continue;
        }
        if status < 0 {
            return out;
        }
        break;
    }
    let bytes = buffer.as_ptr().cast::<u8>();
    let mut at = 0usize;
    unsafe {
        loop {
            let entry = bytes.add(at);
            let read = |offset: usize| *(entry.add(offset) as *const u64);
            let next = *(entry as *const u32) as usize;
            let thread_count = *(entry.add(0x04) as *const u32) as usize;
            let pid = read(0x50) as u32;
            let name_length = *(entry.add(0x38) as *const u16) as usize / 2;
            let name_text = read(0x40) as *const u16;
            let name = if name_text.is_null() { "System Idle".to_string() } else { String::from_utf16_lossy(std::slice::from_raw_parts(name_text, name_length)) };
            let mut threads = Vec::new();
            if pid == threads_of {
                for index in 0..thread_count {
                    let thread = entry.add(0x100 + index * 0x50);
                    let kernel = *(thread as *const u64);
                    let user = *(thread.add(0x08) as *const u64);
                    let tid = *(thread.add(0x30) as *const u64) as u32;
                    let switches = *(thread.add(0x40) as *const u32);
                    threads.push((tid, kernel + user, switches));
                }
            }
            out.push(ProcessInfo {
                pid,
                parent: read(0x58) as u32,
                name,
                cpu: read(0x28) + read(0x30),
                working_set: read(0x90),
                private: read(0xC8),
                faults: *(entry.add(0x80) as *const u32),
                threads,
            });
            if next == 0 {
                break;
            }
            at += next;
        }
    }
    out
}

/// The processes under `root`, and under them, and so on
pub(super) fn descendants(processes: &[ProcessInfo], root: u32) -> Vec<u32> {
    let mut found = vec![root];
    let mut at = 0;
    while at < found.len() {
        let parent = found[at];
        for process in processes {
            if process.parent == parent && process.pid != parent && !found.contains(&process.pid) {
                found.push(process.pid);
            }
        }
        at += 1;
    }
    found.remove(0);
    found
}

fn thread_name(tid: u32) -> String {
    let Ok(handle) = (unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, tid) }) else {
        return format!("#{tid}");
    };
    let name = unsafe { GetThreadDescription(handle) }
        .ok()
        .map(|text| {
            let name = unsafe { text.to_string() }.unwrap_or_default();
            let _ = unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
            name
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("#{tid}"));
    let _ = unsafe { CloseHandle(handle) };
    name
}

struct Process {
    kind: String,
    cpu: u64,
    faults: u32,
}

struct Monitor {
    pdh: Option<Pdh>,
    buffer: Vec<u64>,
    system: Option<(u64, u64)>,
    processes: HashMap<u32, Process>,
    /// Name, CPU time and context switches, by thread id
    threads: HashMap<u32, (String, u64, u32)>,
    at: Instant,
    power: String,
}

impl Monitor {
    fn new() -> Self {
        Monitor { pdh: Pdh::new(), buffer: Vec::new(), system: None, processes: HashMap::new(), threads: HashMap::new(), at: Instant::now(), power: power() }
    }

    fn tick(&mut self) {
        let started = Instant::now();
        let wall = self.at.elapsed().as_secs_f64();
        self.at = Instant::now();
        // CPU time is in 100ns ticks, as a percent of a core
        let percent = |ticks: u64| ticks as f64 / 1e7 / wall * 100.0;
        let me = std::process::id();

        // The system's CPU, all of it, and memory
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        let mut system = String::new();
        if unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.is_ok() {
            let (busy, all) = (ticks(kernel) + ticks(user) - ticks(idle), ticks(kernel) + ticks(user));
            if let Some((was_busy, was_all)) = self.system.replace((busy, all)) {
                system = format!("{:.0}%", (busy - was_busy) as f64 * 100.0 / (all - was_all).max(1) as f64);
            }
        }
        let mut memory = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        let _ = unsafe { GlobalMemoryStatusEx(&mut memory) };

        // Ligma's process and those under it
        let all = system_processes(&mut self.buffer, me);
        let mut ours = vec![me];
        ours.extend(descendants(&all, me));
        let gpu = self.pdh.as_ref().map(Pdh::sample).unwrap_or_default();
        let mut rows: Vec<Row> = Vec::new();
        let mut seen = HashMap::new();
        for process in all.iter().filter(|process| ours.contains(&process.pid)) {
            let pid = process.pid;
            let was = self.processes.remove(&pid);
            let kind = match &was {
                Some(was) => was.kind.clone(),
                None if pid == me => "ligma".into(),
                None => match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
                    Ok(handle) => {
                        let kind = kind_of(handle, &process.name);
                        let _ = unsafe { CloseHandle(handle) };
                        kind
                    }
                    Err(_) => process.name.clone(),
                },
            };
            let (cpu, faults) = match &was {
                Some(was) => (percent(process.cpu.saturating_sub(was.cpu)), process.faults.saturating_sub(was.faults)),
                None => (0.0, 0),
            };
            let (gpu_memory, gpu_shared, three_d) = gpu.processes.get(&pid).copied().unwrap_or_default();
            rows.push(Row { pid, kind: kind.clone(), cpu, working_set: process.working_set as f64 / MB, private: process.private as f64 / MB, faults, gpu: gpu_memory / MB, gpu_shared: gpu_shared / MB, three_d });
            seen.insert(pid, Process { kind, cpu: process.cpu, faults: process.faults });
        }
        self.processes = seen;
        let (ligma, mut webview): (Vec<&Row>, Vec<&Row>) = rows.iter().partition(|row| row.pid == me);
        webview.sort_by(|a, b| b.private.total_cmp(&a.private));
        let sum = |at: fn(&Row) -> f64| webview.iter().map(|row| at(row)).sum::<f64>();
        let ours_3d: f64 = rows.iter().map(|row| row.three_d).sum();
        trace(|| {
            format!(
                "monitor: system cpu={system} ram used={}% free={:.0}MB commit={:.1}/{:.1}GB | gpu card dedicated={:.0}MB shared={:.0}MB 3d={:.0}% (not ours {:.0}%) | ligma {} | webview cpu={:.0}% ws={:.0} priv={:.0} gpu={:.0}MB 3d={:.0}%",
                memory.dwMemoryLoad,
                memory.ullAvailPhys as f64 / MB,
                (memory.ullTotalPageFile - memory.ullAvailPageFile) as f64 / MB / 1024.0,
                memory.ullTotalPageFile as f64 / MB / 1024.0,
                gpu.dedicated / MB,
                gpu.shared / MB,
                gpu.three_d,
                (gpu.three_d - ours_3d).max(0.0),
                ligma.first().map(|ligma| ligma.text()).unwrap_or_default(),
                sum(|row| row.cpu),
                sum(|row| row.working_set),
                sum(|row| row.private),
                sum(|row| row.gpu),
                sum(|row| row.three_d),
            )
        });
        trace(|| format!("monitor: webview {}", webview.iter().map(|row| row.text()).collect::<Vec<_>>().join(" | ")));

        // Ligma's busiest threads, those of a name together, as encoding
        // and fetching run on several
        let mut named: Vec<(String, f64, u32, usize)> = Vec::new();
        let mut seen = HashMap::new();
        if let Some(process) = all.iter().find(|process| process.pid == me) {
            for &(tid, cpu, switches) in &process.threads {
                let (name, was) = match self.threads.remove(&tid) {
                    Some((name, cpu, switches)) => (name, Some((cpu, switches))),
                    None => (thread_name(tid), None),
                };
                if let Some((was_cpu, was_switches)) = was {
                    let group = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '-' || c == '#').trim().to_string();
                    let group = if group.is_empty() { "unnamed".to_string() } else { group };
                    let (used, switched) = (percent(cpu.saturating_sub(was_cpu)), switches.saturating_sub(was_switches));
                    match named.iter_mut().find(|(other, ..)| *other == group) {
                        Some(entry) => {
                            entry.1 += used;
                            entry.2 += switched;
                            entry.3 += 1;
                        }
                        None => named.push((group, used, switched, 1)),
                    }
                }
                seen.insert(tid, (name, cpu, switches));
            }
        }
        self.threads = seen;
        named.sort_by(|a, b| b.1.total_cmp(&a.1));
        trace(|| {
            let busy: Vec<String> = named
                .iter()
                .filter(|(_, cpu, ..)| *cpu >= 1.0)
                .take(14)
                .map(|(name, cpu, switches, count)| if *count > 1 { format!("{name}x{count} {cpu:.0}% {switches}sw") } else { format!("{name} {cpu:.0}% {switches}sw") })
                .collect();
            format!("monitor: ligma threads {}", busy.join(", "))
        });
        let (presents, slowest, gap) = {
            let mut presents = PRESENTS.lock().unwrap();
            let out = (presents.0, presents.1, presents.2);
            (presents.0, presents.1, presents.2) = (0, 0, 0);
            out
        };
        let took = started.elapsed();
        trace(|| format!("monitor: present {presents} frames, slowest Present call {:.1}ms, longest between two {:.1}ms | sampled in {:.1}ms", slowest as f64 / 1000.0, gap as f64 / 1000.0, took.as_secs_f64() * 1000.0));
        let now = power();
        if now != self.power {
            trace(|| format!("monitor: power now {now}"));
            self.power = now;
        }
    }
}

struct Row {
    pid: u32,
    kind: String,
    cpu: f64,
    working_set: f64,
    private: f64,
    faults: u32,
    gpu: f64,
    gpu_shared: f64,
    three_d: f64,
}

impl Row {
    fn text(&self) -> String {
        format!(
            "{}#{} cpu={:.0}% ws={:.0} priv={:.0} faults={} gpu={:.0}MB gpu_shared={:.0}MB 3d={:.0}%",
            self.kind, self.pid, self.cpu, self.working_set, self.private, self.faults, self.gpu, self.gpu_shared, self.three_d
        )
    }
}

/// Windows' GPU counters: by process, memory on the card and off it, and
/// how busy it keeps the card's 3D and compute engines, and the card's
/// memory in all
struct Pdh {
    query: PDH_HQUERY,
    local: PDH_HCOUNTER,
    shared: PDH_HCOUNTER,
    engines: PDH_HCOUNTER,
    dedicated: PDH_HCOUNTER,
    adapter_shared: PDH_HCOUNTER,
}

unsafe impl Send for Pdh {}

#[derive(Default)]
struct GpuSample {
    /// By process id: bytes on the card, bytes of system memory, and the
    /// percent of the 3D and compute engines' time it took
    processes: HashMap<u32, (f64, f64, f64)>,
    dedicated: f64,
    shared: f64,
    three_d: f64,
}

impl Pdh {
    fn new() -> Option<Self> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(None, 0, &mut query) != 0 {
                return None;
            }
            let add = |path: PCWSTR| {
                let mut counter = PDH_HCOUNTER::default();
                (PdhAddEnglishCounterW(query, path, 0, &mut counter) == 0).then_some(counter)
            };
            let pdh = Pdh {
                query,
                local: add(w!("\\GPU Process Memory(*)\\Local Usage"))?,
                shared: add(w!("\\GPU Process Memory(*)\\Shared Usage"))?,
                engines: add(w!("\\GPU Engine(*)\\Utilization Percentage"))?,
                dedicated: add(w!("\\GPU Adapter Memory(*)\\Dedicated Usage"))?,
                adapter_shared: add(w!("\\GPU Adapter Memory(*)\\Shared Usage"))?,
            };
            PdhCollectQueryData(query);
            Some(pdh)
        }
    }

    fn array(counter: PDH_HCOUNTER, double: bool) -> Vec<(String, f64)> {
        let mut out = Vec::new();
        let format = if double { PDH_FMT_DOUBLE } else { PDH_FMT_LARGE };
        unsafe {
            let (mut size, mut count) = (0u32, 0u32);
            if PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, None) != PDH_MORE_DATA {
                return out;
            }
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            let items = buffer.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            if PdhGetFormattedCounterArrayW(counter, format, &mut size, &mut count, Some(items)) != 0 {
                return out;
            }
            for item in std::slice::from_raw_parts(items, count as usize) {
                let name = item.szName.to_string().unwrap_or_default();
                let value = if double { item.FmtValue.Anonymous.doubleValue } else { item.FmtValue.Anonymous.largeValue as f64 };
                out.push((name, value));
            }
        }
        out
    }

    fn sample(&self) -> GpuSample {
        let mut out = GpuSample::default();
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return out;
        }
        // "pid_1234_luid_0x00000000_0x0000D1B4_phys_0[_eng_0_engtype_3D]"
        let pid = |name: &str| name.strip_prefix("pid_").and_then(|rest| rest.split('_').next()).and_then(|pid| pid.parse::<u32>().ok());
        for (name, value) in Self::array(self.local, false) {
            if let Some(pid) = pid(&name) {
                out.processes.entry(pid).or_default().0 += value.max(0.0);
            }
        }
        for (name, value) in Self::array(self.shared, false) {
            if let Some(pid) = pid(&name) {
                out.processes.entry(pid).or_default().1 += value.max(0.0);
            }
        }
        for (name, value) in Self::array(self.engines, true) {
            if !(name.ends_with("engtype_3D") || name.ends_with("engtype_Compute")) {
                continue;
            }
            out.three_d += value.max(0.0);
            if let Some(pid) = pid(&name) {
                out.processes.entry(pid).or_default().2 += value.max(0.0);
            }
        }
        out.dedicated = Self::array(self.dedicated, false).iter().map(|(_, value)| value).sum();
        out.shared = Self::array(self.adapter_shared, false).iter().map(|(_, value)| value).sum();
        out
    }
}
