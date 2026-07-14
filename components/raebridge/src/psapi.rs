//! psapi.dll — Process status, module enumeration, memory information, performance
//! counters, working set queries, device drivers, and Tool Help snapshots for RaeBridge.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    WinHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER,
    ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, INVALID_HANDLE_VALUE, NULL_HANDLE,
};

// =========================================================================
// Module filter flags — EnumProcessModulesEx
// =========================================================================

pub const LIST_MODULES_DEFAULT: u32 = 0x00;
pub const LIST_MODULES_32BIT: u32 = 0x01;
pub const LIST_MODULES_64BIT: u32 = 0x02;
pub const LIST_MODULES_ALL: u32 = 0x03;

// =========================================================================
// Tool Help snapshot flags
// =========================================================================

pub const TH32CS_SNAPHEAPLIST: u32 = 0x00000001;
pub const TH32CS_SNAPPROCESS: u32 = 0x00000002;
pub const TH32CS_SNAPTHREAD: u32 = 0x00000004;
pub const TH32CS_SNAPMODULE: u32 = 0x00000008;
pub const TH32CS_SNAPMODULE32: u32 = 0x00000010;
pub const TH32CS_SNAPALL: u32 =
    TH32CS_SNAPHEAPLIST | TH32CS_SNAPPROCESS | TH32CS_SNAPTHREAD | TH32CS_SNAPMODULE;
pub const TH32CS_INHERIT: u32 = 0x80000000;

// =========================================================================
// Working set protection flags
// =========================================================================

pub const WSLE_PAGE_READONLY: u32 = 0x001;
pub const WSLE_PAGE_EXECUTE: u32 = 0x002;
pub const WSLE_PAGE_EXECUTE_READ: u32 = 0x003;
pub const WSLE_PAGE_READWRITE: u32 = 0x004;
pub const WSLE_PAGE_WRITECOPY: u32 = 0x005;
pub const WSLE_PAGE_EXECUTE_READWRITE: u32 = 0x006;
pub const WSLE_PAGE_EXECUTE_WRITECOPY: u32 = 0x007;
pub const WSLE_PAGE_SHAREABLE: u32 = 0x100;

// =========================================================================
// Data structures
// =========================================================================

#[derive(Debug, Clone)]
pub struct ModuleInfo {
    pub base_of_dll: u64,
    pub size_of_image: u32,
    pub entry_point: u64,
}

#[derive(Debug, Clone)]
pub struct ProcessMemoryCountersEx {
    pub cb: u32,
    pub page_fault_count: u32,
    pub peak_working_set_size: u64,
    pub working_set_size: u64,
    pub quota_peak_paged_pool_usage: u64,
    pub quota_paged_pool_usage: u64,
    pub quota_peak_non_paged_pool_usage: u64,
    pub quota_non_paged_pool_usage: u64,
    pub pagefile_usage: u64,
    pub peak_pagefile_usage: u64,
    pub private_usage: u64,
}

impl ProcessMemoryCountersEx {
    pub fn new() -> Self {
        Self {
            cb: 80,
            page_fault_count: 0,
            peak_working_set_size: 0,
            working_set_size: 0,
            quota_peak_paged_pool_usage: 0,
            quota_paged_pool_usage: 0,
            quota_peak_non_paged_pool_usage: 0,
            quota_non_paged_pool_usage: 0,
            pagefile_usage: 0,
            peak_pagefile_usage: 0,
            private_usage: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PerformanceInformation {
    pub cb: u32,
    pub commit_total: u64,
    pub commit_limit: u64,
    pub commit_peak: u64,
    pub physical_total: u64,
    pub physical_available: u64,
    pub system_cache: u64,
    pub kernel_total: u64,
    pub kernel_paged: u64,
    pub kernel_nonpaged: u64,
    pub page_size: u64,
    pub handle_count: u32,
    pub process_count: u32,
    pub thread_count: u32,
}

impl PerformanceInformation {
    pub fn new() -> Self {
        Self {
            cb: 104,
            commit_total: 0,
            commit_limit: 0,
            commit_peak: 0,
            physical_total: 0,
            physical_available: 0,
            system_cache: 0,
            kernel_total: 0,
            kernel_paged: 0,
            kernel_nonpaged: 0,
            page_size: 4096,
            handle_count: 0,
            process_count: 0,
            thread_count: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkingSetBlock {
    pub protection: u32,
    pub share_count: u32,
    pub shared: bool,
    pub node: u32,
    pub virtual_page: u64,
}

#[derive(Debug, Clone)]
pub struct ProcessEntry32 {
    pub dw_size: u32,
    pub cnt_usage: u32,
    pub th32_process_id: u32,
    pub th32_default_heap_id: u64,
    pub th32_module_id: u32,
    pub cnt_threads: u32,
    pub th32_parent_process_id: u32,
    pub pc_pri_class_base: i32,
    pub dw_flags: u32,
    pub sz_exe_file: String,
}

impl ProcessEntry32 {
    pub fn new() -> Self {
        Self {
            dw_size: 568,
            cnt_usage: 0,
            th32_process_id: 0,
            th32_default_heap_id: 0,
            th32_module_id: 0,
            cnt_threads: 0,
            th32_parent_process_id: 0,
            pc_pri_class_base: 0,
            dw_flags: 0,
            sz_exe_file: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ThreadEntry32 {
    pub dw_size: u32,
    pub cnt_usage: u32,
    pub th32_thread_id: u32,
    pub th32_owner_process_id: u32,
    pub tp_base_pri: i32,
    pub tp_delta_pri: i32,
    pub dw_flags: u32,
}

impl ThreadEntry32 {
    pub fn new() -> Self {
        Self {
            dw_size: 28,
            cnt_usage: 0,
            th32_thread_id: 0,
            th32_owner_process_id: 0,
            tp_base_pri: 8,
            tp_delta_pri: 0,
            dw_flags: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModuleEntry32 {
    pub dw_size: u32,
    pub th32_module_id: u32,
    pub th32_process_id: u32,
    pub glbl_cnt_usage: u32,
    pub proc_cnt_usage: u32,
    pub mod_base_addr: u64,
    pub mod_base_size: u32,
    pub h_module: u64,
    pub sz_module: String,
    pub sz_exe_path: String,
}

impl ModuleEntry32 {
    pub fn new() -> Self {
        Self {
            dw_size: 1080,
            th32_module_id: 0,
            th32_process_id: 0,
            glbl_cnt_usage: 0,
            proc_cnt_usage: 0,
            mod_base_addr: 0,
            mod_base_size: 0,
            h_module: 0,
            sz_module: String::new(),
            sz_exe_path: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HeapList32 {
    pub dw_size: u32,
    pub th32_process_id: u32,
    pub th32_heap_id: u64,
    pub dw_flags: u32,
}

#[derive(Debug, Clone)]
pub struct HeapEntry32 {
    pub dw_size: u32,
    pub h_handle: u64,
    pub dw_address: u64,
    pub dw_block_size: u64,
    pub dw_flags: u32,
    pub dw_lock_count: u32,
    pub dw_resvd: u32,
    pub th32_process_id: u32,
    pub th32_heap_id: u64,
}

#[derive(Debug, Clone)]
struct EmulatedProcess {
    pid: u32,
    parent_pid: u32,
    exe: String,
    image_path: String,
    threads: u32,
    priority: i32,
    working_set: u64,
    private_bytes: u64,
    modules: Vec<EmulatedModule>,
}

#[derive(Debug, Clone)]
struct EmulatedModule {
    name: String,
    path: String,
    base: u64,
    size: u32,
    entry_point: u64,
}

#[derive(Debug, Clone)]
struct EmulatedDriver {
    name: String,
    path: String,
    base: u64,
}

struct ToolhelpSnapshot {
    handle: WinHandle,
    flags: u32,
    pid: u32,
    proc_index: usize,
    thread_index: usize,
    module_index: usize,
    heap_index: usize,
    heap_entry_index: usize,
}

// =========================================================================
// Global state
// =========================================================================

pub struct ProcessApi {
    next_handle: u64,
    processes: Vec<EmulatedProcess>,
    drivers: Vec<EmulatedDriver>,
    snapshots: BTreeMap<u64, ToolhelpSnapshot>,
}

impl ProcessApi {
    const fn new() -> Self {
        Self {
            next_handle: 0xC000_0000,
            processes: Vec::new(),
            drivers: Vec::new(),
            snapshots: BTreeMap::new(),
        }
    }

    fn alloc_handle(&mut self) -> WinHandle {
        let h = self.next_handle;
        self.next_handle += 1;
        WinHandle(h)
    }

    fn populate_defaults(&mut self) {
        let system_modules = [
            (
                "ntoskrnl.exe",
                "C:\\Windows\\System32\\ntoskrnl.exe",
                0xFFFFF800_00000000u64,
                0x00A0_0000u32,
            ),
            (
                "hal.dll",
                "C:\\Windows\\System32\\hal.dll",
                0xFFFFF800_00A00000,
                0x0010_0000,
            ),
            (
                "ntdll.dll",
                "C:\\Windows\\System32\\ntdll.dll",
                0x7FFE_0000_0000,
                0x001E_0000,
            ),
            (
                "kernel32.dll",
                "C:\\Windows\\System32\\kernel32.dll",
                0x7FFE_0020_0000,
                0x0012_0000,
            ),
        ];

        let procs = [
            (
                0u32,
                0u32,
                "[System Process]",
                "\\SystemRoot\\System32\\smss.exe",
                1,
                0i32,
                0x1000u64,
                0x1000u64,
            ),
            (
                4,
                0,
                "System",
                "\\SystemRoot\\System32\\ntoskrnl.exe",
                120,
                8,
                0x200000,
                0x100000,
            ),
            (
                456,
                4,
                "smss.exe",
                "C:\\Windows\\System32\\smss.exe",
                2,
                11,
                0x800000,
                0x400000,
            ),
            (
                600,
                456,
                "csrss.exe",
                "C:\\Windows\\System32\\csrss.exe",
                12,
                13,
                0x2000000,
                0x1000000,
            ),
            (
                664,
                456,
                "wininit.exe",
                "C:\\Windows\\System32\\wininit.exe",
                3,
                13,
                0x1000000,
                0x800000,
            ),
            (
                672,
                600,
                "csrss.exe",
                "C:\\Windows\\System32\\csrss.exe",
                14,
                13,
                0x2000000,
                0x1000000,
            ),
            (
                748,
                664,
                "services.exe",
                "C:\\Windows\\System32\\services.exe",
                8,
                9,
                0x2000000,
                0x1000000,
            ),
            (
                756,
                664,
                "lsass.exe",
                "C:\\Windows\\System32\\lsass.exe",
                10,
                9,
                0x3000000,
                0x1800000,
            ),
            (
                860,
                748,
                "svchost.exe",
                "C:\\Windows\\System32\\svchost.exe",
                22,
                8,
                0x8000000,
                0x4000000,
            ),
            (
                1000,
                860,
                "explorer.exe",
                "C:\\Windows\\explorer.exe",
                35,
                8,
                0x20000000,
                0x10000000,
            ),
            (
                2000,
                1000,
                "app.exe",
                "C:\\Users\\user\\app.exe",
                4,
                8,
                0x4000000,
                0x2000000,
            ),
        ];

        for (pid, ppid, exe, path, threads, pri, ws, priv_b) in procs {
            let mut modules = Vec::new();
            let mut base = 0x00400000u64;
            modules.push(EmulatedModule {
                name: String::from(exe),
                path: String::from(path),
                base,
                size: 0x0010_0000,
                entry_point: base + 0x1000,
            });
            for &(name, mpath, mbase, msize) in &system_modules[2..] {
                base = mbase;
                modules.push(EmulatedModule {
                    name: String::from(name),
                    path: String::from(mpath),
                    base,
                    size: msize,
                    entry_point: base + 0x1000,
                });
            }
            self.processes.push(EmulatedProcess {
                pid,
                parent_pid: ppid,
                exe: String::from(exe),
                image_path: String::from(path),
                threads,
                priority: pri,
                working_set: ws,
                private_bytes: priv_b,
                modules,
            });
        }

        self.drivers = [
            (
                "ntoskrnl.exe",
                "\\SystemRoot\\System32\\ntoskrnl.exe",
                0xFFFFF800_00000000u64,
            ),
            (
                "hal.dll",
                "\\SystemRoot\\System32\\hal.dll",
                0xFFFFF800_00A00000,
            ),
            (
                "ndis.sys",
                "\\SystemRoot\\System32\\drivers\\ndis.sys",
                0xFFFFF800_01000000,
            ),
            (
                "NETIO.SYS",
                "\\SystemRoot\\System32\\drivers\\NETIO.SYS",
                0xFFFFF800_01200000,
            ),
            (
                "fltMgr.sys",
                "\\SystemRoot\\System32\\drivers\\fltMgr.sys",
                0xFFFFF800_01400000,
            ),
            (
                "disk.sys",
                "\\SystemRoot\\System32\\drivers\\disk.sys",
                0xFFFFF800_01600000,
            ),
            (
                "USBPORT.SYS",
                "\\SystemRoot\\System32\\drivers\\USBPORT.SYS",
                0xFFFFF800_01800000,
            ),
            (
                "storport.sys",
                "\\SystemRoot\\System32\\drivers\\storport.sys",
                0xFFFFF800_01A00000,
            ),
        ]
        .iter()
        .map(|&(n, p, b)| EmulatedDriver {
            name: String::from(n),
            path: String::from(p),
            base: b,
        })
        .collect();
    }
}

static mut PROCESS_API: Option<ProcessApi> = None;

pub fn init() {
    unsafe {
        let mut api = ProcessApi::new();
        api.populate_defaults();
        PROCESS_API = Some(api);
    }
}

fn papi() -> &'static mut ProcessApi {
    unsafe {
        PROCESS_API
            .as_mut()
            .expect("psapi not initialized — call init()")
    }
}

// =========================================================================
// Process enumeration
// =========================================================================

pub fn enum_processes(pids: &mut [u32], cb: u32, bytes_returned: &mut u32) -> bool {
    let pa = papi();
    let max_count = (cb as usize) / 4;
    let count = pa.processes.len().min(max_count).min(pids.len());
    for i in 0..count {
        pids[i] = pa.processes[i].pid;
    }
    *bytes_returned = (count * 4) as u32;
    true
}

pub fn get_process_image_file_name_w(
    _process: WinHandle,
    pid: u32,
    buf: &mut [u16],
    size: u32,
) -> u32 {
    let pa = papi();
    let proc = match pa.processes.iter().find(|p| p.pid == pid) {
        Some(p) => p,
        None => return 0,
    };
    let wide = crate::string_to_wide(&proc.image_path);
    if (size as usize) < wide.len() || buf.len() < wide.len() {
        return 0;
    }
    let n = wide.len().min(buf.len());
    buf[..n].copy_from_slice(&wide[..n]);
    wide.len() as u32
}

// =========================================================================
// Module enumeration
// =========================================================================

pub fn enum_process_modules(
    _process: WinHandle,
    pid: u32,
    modules: &mut [u64],
    cb: u32,
    needed: &mut u32,
) -> bool {
    let pa = papi();
    let proc = match pa.processes.iter().find(|p| p.pid == pid) {
        Some(p) => p,
        None => return false,
    };
    *needed = (proc.modules.len() * 8) as u32;
    let max_count = (cb as usize / 8).min(modules.len());
    let count = proc.modules.len().min(max_count);
    for i in 0..count {
        modules[i] = proc.modules[i].base;
    }
    true
}

pub fn enum_process_modules_ex(
    process: WinHandle,
    pid: u32,
    modules: &mut [u64],
    cb: u32,
    needed: &mut u32,
    _filter_flag: u32,
) -> bool {
    enum_process_modules(process, pid, modules, cb, needed)
}

pub fn get_module_base_name_w(
    _process: WinHandle,
    pid: u32,
    module_base: u64,
    buf: &mut [u16],
    size: u32,
) -> u32 {
    let pa = papi();
    let proc = match pa.processes.iter().find(|p| p.pid == pid) {
        Some(p) => p,
        None => return 0,
    };
    let module = match proc.modules.iter().find(|m| m.base == module_base) {
        Some(m) => m,
        None => return 0,
    };
    let wide = crate::string_to_wide(&module.name);
    if (size as usize) < wide.len() || buf.len() < wide.len() {
        return 0;
    }
    let n = wide.len().min(buf.len());
    buf[..n].copy_from_slice(&wide[..n]);
    wide.len() as u32
}

pub fn get_module_file_name_ex_w(
    _process: WinHandle,
    pid: u32,
    module_base: u64,
    buf: &mut [u16],
    size: u32,
) -> u32 {
    let pa = papi();
    let proc = match pa.processes.iter().find(|p| p.pid == pid) {
        Some(p) => p,
        None => return 0,
    };
    let module = match proc.modules.iter().find(|m| m.base == module_base) {
        Some(m) => m,
        None => return 0,
    };
    let wide = crate::string_to_wide(&module.path);
    if (size as usize) < wide.len() || buf.len() < wide.len() {
        return 0;
    }
    let n = wide.len().min(buf.len());
    buf[..n].copy_from_slice(&wide[..n]);
    wide.len() as u32
}

pub fn get_module_information(
    _process: WinHandle,
    pid: u32,
    module_base: u64,
    info: &mut ModuleInfo,
    _cb: u32,
) -> bool {
    let pa = papi();
    let proc = match pa.processes.iter().find(|p| p.pid == pid) {
        Some(p) => p,
        None => return false,
    };
    let module = match proc.modules.iter().find(|m| m.base == module_base) {
        Some(m) => m,
        None => return false,
    };
    info.base_of_dll = module.base;
    info.size_of_image = module.size;
    info.entry_point = module.entry_point;
    true
}

// =========================================================================
// Process memory information
// =========================================================================

pub fn get_process_memory_info(
    _process: WinHandle,
    pid: u32,
    counters: &mut ProcessMemoryCountersEx,
    _cb: u32,
) -> bool {
    let pa = papi();
    let proc = match pa.processes.iter().find(|p| p.pid == pid) {
        Some(p) => p,
        None => return false,
    };
    counters.working_set_size = proc.working_set;
    counters.peak_working_set_size = proc.working_set + (proc.working_set >> 2);
    counters.private_usage = proc.private_bytes;
    counters.pagefile_usage = proc.private_bytes;
    counters.peak_pagefile_usage = proc.private_bytes + (proc.private_bytes >> 3);
    counters.page_fault_count = (proc.working_set / 4096) as u32;
    counters.quota_paged_pool_usage = proc.private_bytes >> 4;
    counters.quota_peak_paged_pool_usage = proc.private_bytes >> 3;
    counters.quota_non_paged_pool_usage = proc.private_bytes >> 6;
    counters.quota_peak_non_paged_pool_usage = proc.private_bytes >> 5;
    true
}

// =========================================================================
// Performance information
// =========================================================================

pub fn get_performance_info(info: &mut PerformanceInformation, _cb: u32) -> bool {
    let pa = papi();
    info.page_size = 4096;
    info.physical_total = 16 * 1024 * 1024 * 1024 / 4096; // 16 GB in pages
    info.physical_available = 8 * 1024 * 1024 * 1024 / 4096;
    info.system_cache = 2 * 1024 * 1024 * 1024 / 4096;
    info.commit_limit = 32 * 1024 * 1024 * 1024 / 4096;
    info.commit_total = 12 * 1024 * 1024 * 1024 / 4096;
    info.commit_peak = 14 * 1024 * 1024 * 1024 / 4096;
    info.kernel_total = 512 * 1024 * 1024 / 4096;
    info.kernel_paged = 384 * 1024 * 1024 / 4096;
    info.kernel_nonpaged = 128 * 1024 * 1024 / 4096;
    info.process_count = pa.processes.len() as u32;
    info.thread_count = pa.processes.iter().map(|p| p.threads).sum();
    info.handle_count = info.process_count * 50;
    true
}

// =========================================================================
// Working set
// =========================================================================

pub fn query_working_set(
    _process: WinHandle,
    pid: u32,
    blocks: &mut Vec<WorkingSetBlock>,
    _buf_size: u32,
) -> bool {
    let pa = papi();
    let proc = match pa.processes.iter().find(|p| p.pid == pid) {
        Some(p) => p,
        None => return false,
    };
    let pages = proc.working_set / 4096;
    blocks.clear();
    let mut addr = 0x00400000u64;
    for _ in 0..pages.min(64) {
        blocks.push(WorkingSetBlock {
            protection: WSLE_PAGE_READWRITE,
            share_count: 1,
            shared: false,
            node: 0,
            virtual_page: addr / 4096,
        });
        addr += 4096;
    }
    true
}

pub fn query_working_set_ex(
    _process: WinHandle,
    pid: u32,
    blocks: &mut Vec<WorkingSetBlock>,
    _buf_size: u32,
) -> bool {
    query_working_set(_process, pid, blocks, _buf_size)
}

pub fn empty_working_set(_process: WinHandle) -> bool {
    true
}

// =========================================================================
// Device drivers
// =========================================================================

pub fn enum_device_drivers(bases: &mut [u64], cb: u32, needed: &mut u32) -> bool {
    let pa = papi();
    *needed = (pa.drivers.len() * 8) as u32;
    let max = (cb as usize / 8).min(bases.len());
    let count = pa.drivers.len().min(max);
    for i in 0..count {
        bases[i] = pa.drivers[i].base;
    }
    true
}

pub fn get_device_driver_base_name_w(base: u64, buf: &mut [u16], size: u32) -> u32 {
    let pa = papi();
    let drv = match pa.drivers.iter().find(|d| d.base == base) {
        Some(d) => d,
        None => return 0,
    };
    let wide = crate::string_to_wide(&drv.name);
    if (size as usize) < wide.len() || buf.len() < wide.len() {
        return 0;
    }
    let n = wide.len().min(buf.len());
    buf[..n].copy_from_slice(&wide[..n]);
    wide.len() as u32
}

pub fn get_device_driver_file_name_w(base: u64, buf: &mut [u16], size: u32) -> u32 {
    let pa = papi();
    let drv = match pa.drivers.iter().find(|d| d.base == base) {
        Some(d) => d,
        None => return 0,
    };
    let wide = crate::string_to_wide(&drv.path);
    if (size as usize) < wide.len() || buf.len() < wide.len() {
        return 0;
    }
    let n = wide.len().min(buf.len());
    buf[..n].copy_from_slice(&wide[..n]);
    wide.len() as u32
}

// =========================================================================
// Mapped files
// =========================================================================

pub fn get_mapped_file_name_w(
    _process: WinHandle,
    _address: u64,
    buf: &mut [u16],
    size: u32,
) -> u32 {
    let placeholder = "\\Device\\HarddiskVolume1\\Windows\\System32\\ntdll.dll";
    let wide = crate::string_to_wide(placeholder);
    if (size as usize) < wide.len() || buf.len() < wide.len() {
        return 0;
    }
    let n = wide.len().min(buf.len());
    buf[..n].copy_from_slice(&wide[..n]);
    wide.len() as u32
}

// =========================================================================
// Tool Help snapshot operations
// =========================================================================

pub fn create_toolhelp32_snapshot(flags: u32, pid: u32) -> WinHandle {
    let pa = papi();
    let handle = pa.alloc_handle();
    pa.snapshots.insert(
        handle.0,
        ToolhelpSnapshot {
            handle,
            flags,
            pid,
            proc_index: 0,
            thread_index: 0,
            module_index: 0,
            heap_index: 0,
            heap_entry_index: 0,
        },
    );
    handle
}

pub fn process32_first_w(snapshot: WinHandle, entry: &mut ProcessEntry32) -> bool {
    let pa = papi();
    let snap = match pa.snapshots.get_mut(&snapshot.0) {
        Some(s) => s,
        None => return false,
    };
    snap.proc_index = 0;
    if pa.processes.is_empty() {
        return false;
    }
    fill_process_entry(entry, &pa.processes[0]);
    true
}

pub fn process32_next_w(snapshot: WinHandle, entry: &mut ProcessEntry32) -> bool {
    let pa = papi();
    let snap = match pa.snapshots.get_mut(&snapshot.0) {
        Some(s) => s,
        None => return false,
    };
    snap.proc_index += 1;
    if snap.proc_index >= pa.processes.len() {
        return false;
    }
    let idx = snap.proc_index;
    fill_process_entry(entry, &pa.processes[idx]);
    true
}

fn fill_process_entry(entry: &mut ProcessEntry32, proc: &EmulatedProcess) {
    entry.th32_process_id = proc.pid;
    entry.th32_parent_process_id = proc.parent_pid;
    entry.cnt_threads = proc.threads;
    entry.pc_pri_class_base = proc.priority;
    entry.sz_exe_file = proc.exe.clone();
    entry.cnt_usage = 0;
    entry.dw_flags = 0;
}

pub fn thread32_first(snapshot: WinHandle, entry: &mut ThreadEntry32) -> bool {
    let pa = papi();
    let snap = match pa.snapshots.get_mut(&snapshot.0) {
        Some(s) => s,
        None => return false,
    };
    snap.thread_index = 0;
    if pa.processes.is_empty() {
        return false;
    }
    entry.th32_thread_id = pa.processes[0].pid * 4;
    entry.th32_owner_process_id = pa.processes[0].pid;
    entry.tp_base_pri = pa.processes[0].priority;
    entry.tp_delta_pri = 0;
    true
}

pub fn thread32_next(snapshot: WinHandle, entry: &mut ThreadEntry32) -> bool {
    let pa = papi();
    let snap = match pa.snapshots.get_mut(&snapshot.0) {
        Some(s) => s,
        None => return false,
    };
    snap.thread_index += 1;
    if snap.thread_index >= pa.processes.len() {
        return false;
    }
    let idx = snap.thread_index;
    entry.th32_thread_id = pa.processes[idx].pid * 4;
    entry.th32_owner_process_id = pa.processes[idx].pid;
    entry.tp_base_pri = pa.processes[idx].priority;
    entry.tp_delta_pri = 0;
    true
}

pub fn module32_first_w(snapshot: WinHandle, entry: &mut ModuleEntry32) -> bool {
    let pa = papi();
    let snap = match pa.snapshots.get_mut(&snapshot.0) {
        Some(s) => s,
        None => return false,
    };
    snap.module_index = 0;
    let target_pid = snap.pid;
    let proc = match pa.processes.iter().find(|p| p.pid == target_pid) {
        Some(p) => p,
        None => return false,
    };
    if proc.modules.is_empty() {
        return false;
    }
    fill_module_entry(entry, &proc.modules[0], target_pid);
    true
}

pub fn module32_next_w(snapshot: WinHandle, entry: &mut ModuleEntry32) -> bool {
    let pa = papi();
    let snap = match pa.snapshots.get_mut(&snapshot.0) {
        Some(s) => s,
        None => return false,
    };
    snap.module_index += 1;
    let target_pid = snap.pid;
    let proc = match pa.processes.iter().find(|p| p.pid == target_pid) {
        Some(p) => p,
        None => return false,
    };
    let idx = snap.module_index;
    if idx >= proc.modules.len() {
        return false;
    }
    fill_module_entry(entry, &proc.modules[idx], target_pid);
    true
}

fn fill_module_entry(entry: &mut ModuleEntry32, module: &EmulatedModule, pid: u32) {
    entry.th32_process_id = pid;
    entry.mod_base_addr = module.base;
    entry.mod_base_size = module.size;
    entry.h_module = module.base;
    entry.sz_module = module.name.clone();
    entry.sz_exe_path = module.path.clone();
}

pub fn heap32_list_first(snapshot: WinHandle, entry: &mut HeapList32) -> bool {
    let pa = papi();
    let snap = match pa.snapshots.get_mut(&snapshot.0) {
        Some(s) => s,
        None => return false,
    };
    snap.heap_index = 0;
    entry.th32_process_id = snap.pid;
    entry.th32_heap_id = 0x0010_0000;
    entry.dw_flags = 1; // HF32_DEFAULT
    true
}

pub fn heap32_list_next(snapshot: WinHandle, _entry: &mut HeapList32) -> bool {
    let pa = papi();
    let snap = match pa.snapshots.get_mut(&snapshot.0) {
        Some(s) => s,
        None => return false,
    };
    snap.heap_index += 1;
    snap.heap_index < 2
}

pub fn heap32_first(entry: &mut HeapEntry32, pid: u32, heap_id: u64) -> bool {
    entry.th32_process_id = pid;
    entry.th32_heap_id = heap_id;
    entry.dw_address = heap_id + 0x1000;
    entry.dw_block_size = 4096;
    entry.dw_flags = 1; // LF32_FIXED
    entry.dw_lock_count = 0;
    true
}

pub fn heap32_next(_entry: &mut HeapEntry32) -> bool {
    false
}
