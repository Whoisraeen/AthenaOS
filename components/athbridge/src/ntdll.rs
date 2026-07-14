//! ntdll.dll — NT Native API stubs for AthBridge.
//!
//! These map directly to NT kernel-level operations, below the Win32 layer.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    wide_to_string, CompatContext, HandleType, LargeInteger, MemoryBasicInformation, NtStatus,
    OsVersionInfoExW, VirtualRegion, WinHandle, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE,
    STATUS_ACCESS_DENIED, STATUS_BUFFER_TOO_SMALL, STATUS_INVALID_HANDLE, STATUS_INVALID_PARAMETER,
    STATUS_NOT_IMPLEMENTED, STATUS_NO_MEMORY, STATUS_OBJECT_NAME_NOT_FOUND, STATUS_SUCCESS,
};

// =========================================================================
// File operations
// =========================================================================

pub fn nt_create_file(
    ctx: &mut CompatContext,
    file_handle: &mut WinHandle,
    desired_access: u32,
    object_name: &[u16],
    _io_status: &mut IoStatusBlock,
    _allocation_size: Option<&LargeInteger>,
    file_attributes: u32,
    share_access: u32,
    create_disposition: u32,
    create_options: u32,
) -> NtStatus {
    let name = wide_to_string(object_name);
    if name.is_empty() {
        return NtStatus(STATUS_INVALID_PARAMETER);
    }

    // Per-app bucket, same as CreateFileW: a guest opening C:\ via the NT API
    // must land in its own bucket too, else it would escape the isolation.
    let native = ctx.win_path_to_vfs(&name);
    let _ = (
        file_attributes,
        share_access,
        create_disposition,
        create_options,
    );

    let mut path_bytes = native.into_bytes();
    path_bytes.push(0); // null terminate

    let flags = 0; // O_RDWR implicitly
    let fd = unsafe { crate::syscalls::sys_open(&path_bytes, flags) };
    if fd == u64::MAX {
        return NtStatus(STATUS_OBJECT_NAME_NOT_FOUND);
    }

    let h = ctx
        .handle_table
        .allocate(HandleType::File, desired_access, Some(name));
    if let Some(entry) = ctx.handle_table.get_mut(h) {
        entry.native_id = Some(fd);
    }
    *file_handle = WinHandle(h);
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_read_file(
    ctx: &mut CompatContext,
    file_handle: WinHandle,
    _event: WinHandle,
    _apc_routine: u64,
    _apc_context: u64,
    io_status: &mut IoStatusBlock,
    buffer: &mut [u8],
    length: u32,
    _byte_offset: Option<&LargeInteger>,
    _key: Option<&u32>,
) -> NtStatus {
    let native_id = match ctx.handle_table.get(file_handle.0) {
        Some(entry) => entry.native_id,
        None => return NtStatus(STATUS_INVALID_HANDLE),
    };

    let actual = core::cmp::min(length as usize, buffer.len());

    if let Some(fd) = native_id {
        let read = unsafe { crate::syscalls::sys_read(fd, &mut buffer[..actual]) };
        io_status.status = STATUS_SUCCESS;
        io_status.information = read as u64;
    } else {
        for b in &mut buffer[..actual] {
            *b = 0;
        }
        io_status.status = STATUS_SUCCESS;
        io_status.information = actual as u64;
    }
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_write_file(
    ctx: &mut CompatContext,
    file_handle: WinHandle,
    _event: WinHandle,
    _apc_routine: u64,
    _apc_context: u64,
    io_status: &mut IoStatusBlock,
    buffer: &[u8],
    length: u32,
    _byte_offset: Option<&LargeInteger>,
    _key: Option<&u32>,
) -> NtStatus {
    let native_id = match ctx.handle_table.get(file_handle.0) {
        Some(entry) => entry.native_id,
        None => return NtStatus(STATUS_INVALID_HANDLE),
    };

    let actual = core::cmp::min(length as usize, buffer.len());
    if let Some(fd) = native_id {
        let written = unsafe { crate::syscalls::sys_write(fd, &buffer[..actual]) };
        io_status.status = STATUS_SUCCESS;
        io_status.information = written as u64;
    } else {
        io_status.status = STATUS_SUCCESS;
        io_status.information = actual as u64;
    }
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_close(ctx: &mut CompatContext, handle: WinHandle) -> NtStatus {
    if let Some(entry) = ctx.handle_table.get(handle.0) {
        if let Some(fd) = entry.native_id {
            unsafe { crate::syscalls::sys_close(fd) };
        }
    }
    if ctx.handle_table.close(handle.0) {
        NtStatus(STATUS_SUCCESS)
    } else {
        NtStatus(STATUS_INVALID_HANDLE)
    }
}

// =========================================================================
// Virtual memory
// =========================================================================

pub fn nt_allocate_virtual_memory(
    ctx: &mut CompatContext,
    _process_handle: WinHandle,
    base_address: &mut u64,
    _zero_bits: u64,
    region_size: &mut u64,
    allocation_type: u32,
    protect: u32,
) -> NtStatus {
    if *region_size == 0 {
        return NtStatus(STATUS_INVALID_PARAMETER);
    }

    let page_size: u64 = 4096;
    let aligned = (*region_size + page_size - 1) & !(page_size - 1);

    let prot = 3; // PROT_READ | PROT_WRITE
    let flags = 0; // MAP_ANON | MAP_PRIVATE

    let base =
        unsafe { crate::syscalls::sys_mmap(*base_address, aligned, prot, flags, u64::MAX, 0) };

    if base == u64::MAX {
        return NtStatus(STATUS_NO_MEMORY);
    }

    let region = VirtualRegion {
        base_address: base,
        size: aligned,
        state: allocation_type & (MEM_COMMIT | MEM_RESERVE),
        protect,
        allocation_type,
    };
    ctx.virtual_regions.insert(base, region);
    *base_address = base;
    *region_size = aligned;
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_free_virtual_memory(
    ctx: &mut CompatContext,
    _process_handle: WinHandle,
    base_address: &mut u64,
    region_size: &mut u64,
    free_type: u32,
) -> NtStatus {
    if *base_address == 0 {
        return NtStatus(STATUS_INVALID_PARAMETER);
    }

    if free_type == MEM_RELEASE {
        if let Some(region) = ctx.virtual_regions.remove(base_address) {
            unsafe {
                crate::syscalls::sys_munmap(*base_address, region.size);
            }
            *region_size = 0;
            return NtStatus(STATUS_SUCCESS);
        }
        return NtStatus(STATUS_INVALID_PARAMETER);
    }

    NtStatus(STATUS_INVALID_PARAMETER)
}

pub fn nt_protect_virtual_memory(
    ctx: &mut CompatContext,
    _process_handle: WinHandle,
    base_address: &mut u64,
    _region_size: &mut u64,
    new_protect: u32,
    old_protect: &mut u32,
) -> NtStatus {
    if let Some(region) = ctx.virtual_regions.get_mut(base_address) {
        *old_protect = region.protect;
        region.protect = new_protect;
        NtStatus(STATUS_SUCCESS)
    } else {
        *old_protect = 0;
        NtStatus(STATUS_INVALID_PARAMETER)
    }
}

pub fn nt_query_virtual_memory(
    ctx: &mut CompatContext,
    _process_handle: WinHandle,
    base_address: u64,
    _info_class: u32,
    info: &mut MemoryBasicInformation,
    _info_length: u64,
    return_length: &mut u64,
) -> NtStatus {
    if let Some(region) = ctx.virtual_regions.get(&base_address) {
        info.base_address = region.base_address;
        info.allocation_base = region.base_address;
        info.allocation_protect = region.protect;
        info.region_size = region.size;
        info.state = region.state;
        info.protect = region.protect;
        info.mem_type = 0x00020000;
        *return_length = core::mem::size_of::<MemoryBasicInformation>() as u64;
        NtStatus(STATUS_SUCCESS)
    } else {
        *return_length = 0;
        NtStatus(STATUS_INVALID_PARAMETER)
    }
}

// =========================================================================
// Section (memory-mapped file) operations
// =========================================================================

pub fn nt_create_section(
    ctx: &mut CompatContext,
    section_handle: &mut WinHandle,
    desired_access: u32,
    _object_attributes: u64,
    maximum_size: Option<&LargeInteger>,
    section_page_protection: u32,
    allocation_attributes: u32,
    file_handle: WinHandle,
) -> NtStatus {
    let _ = (
        maximum_size,
        section_page_protection,
        allocation_attributes,
        file_handle,
    );
    let h = ctx.handle_table.allocate(
        HandleType::Section,
        desired_access,
        Some(String::from("section")),
    );
    *section_handle = WinHandle(h);
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_map_view_of_section(
    ctx: &mut CompatContext,
    section_handle: WinHandle,
    _process_handle: WinHandle,
    base_address: &mut u64,
    _zero_bits: u64,
    _commit_size: u64,
    _section_offset: Option<&mut LargeInteger>,
    view_size: &mut u64,
    _inherit_disposition: u32,
    _allocation_type: u32,
    protect: u32,
) -> NtStatus {
    if ctx.handle_table.get(section_handle.0).is_none() {
        return NtStatus(STATUS_INVALID_HANDLE);
    }

    let base = 0x3000_0000u64 + (ctx.virtual_regions.len() as u64) * 0x10000;
    let size = if *view_size == 0 { 0x10000 } else { *view_size };

    let region = VirtualRegion {
        base_address: base,
        size,
        state: MEM_COMMIT,
        protect,
        allocation_type: MEM_COMMIT | MEM_RESERVE,
    };
    ctx.virtual_regions.insert(base, region);
    *base_address = base;
    *view_size = size;
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_unmap_view_of_section(
    ctx: &mut CompatContext,
    _process_handle: WinHandle,
    base_address: u64,
) -> NtStatus {
    if ctx.virtual_regions.remove(&base_address).is_some() {
        NtStatus(STATUS_SUCCESS)
    } else {
        NtStatus(STATUS_INVALID_PARAMETER)
    }
}

// =========================================================================
// Process management
// =========================================================================

pub fn nt_create_process(
    ctx: &mut CompatContext,
    process_handle: &mut WinHandle,
    desired_access: u32,
    _object_attributes: u64,
    _parent_process: WinHandle,
    _inherit_object_table: bool,
    _section_handle: WinHandle,
    _debug_port: WinHandle,
    _exception_port: WinHandle,
) -> NtStatus {
    let path = b"nt_process\0";
    let pid_result = unsafe { crate::syscalls::sys_spawn(path) };

    let h = ctx.handle_table.allocate(
        HandleType::Process,
        desired_access,
        Some(String::from("nt_process")),
    );
    if let Some(entry) = ctx.handle_table.get_mut(h) {
        entry.native_id = Some(pid_result);
    }
    *process_handle = WinHandle(h);
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_terminate_process(
    ctx: &mut CompatContext,
    process_handle: WinHandle,
    exit_status: i32,
) -> NtStatus {
    if process_handle.0 == 0 || process_handle.0 == u64::MAX {
        unsafe { crate::syscalls::sys_exit(exit_status as u64) };
    }
    if let Some(entry) = ctx.handle_table.get(process_handle.0) {
        if let Some(pid) = entry.native_id {
            unsafe { crate::syscalls::sys_kill(pid) };
            return NtStatus(STATUS_SUCCESS);
        }
        return NtStatus(STATUS_SUCCESS);
    } else {
        NtStatus(STATUS_INVALID_HANDLE)
    }
}

pub fn nt_query_information_process(
    ctx: &mut CompatContext,
    process_handle: WinHandle,
    info_class: u32,
    buffer: &mut [u8],
    _return_length: &mut u32,
) -> NtStatus {
    if process_handle.0 != 0 && ctx.handle_table.get(process_handle.0).is_none() {
        return NtStatus(STATUS_INVALID_HANDLE);
    }

    match info_class {
        0 => {
            // ProcessBasicInformation — PID at offset 16
            if buffer.len() < 48 {
                return NtStatus(STATUS_BUFFER_TOO_SMALL);
            }
            for b in buffer.iter_mut() {
                *b = 0;
            }
            let pid = ctx.current_process_id;
            buffer[16..20].copy_from_slice(&pid.to_le_bytes());
            *_return_length = 48;
            NtStatus(STATUS_SUCCESS)
        }
        _ => NtStatus(STATUS_NOT_IMPLEMENTED),
    }
}

// =========================================================================
// Thread management
// =========================================================================

pub fn nt_create_thread(
    ctx: &mut CompatContext,
    thread_handle: &mut WinHandle,
    desired_access: u32,
    _object_attributes: u64,
    _process_handle: WinHandle,
    _client_id: &mut ClientId,
    _context: u64,
    _initial_teb: u64,
    _create_suspended: bool,
) -> NtStatus {
    let h = ctx.handle_table.allocate(
        HandleType::Thread,
        desired_access,
        Some(String::from("nt_thread")),
    );
    *thread_handle = WinHandle(h);
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_terminate_thread(
    ctx: &mut CompatContext,
    thread_handle: WinHandle,
    _exit_status: i32,
) -> NtStatus {
    if ctx.handle_table.get(thread_handle.0).is_some() {
        NtStatus(STATUS_SUCCESS)
    } else {
        NtStatus(STATUS_INVALID_HANDLE)
    }
}

pub fn nt_query_information_thread(
    ctx: &mut CompatContext,
    thread_handle: WinHandle,
    info_class: u32,
    buffer: &mut [u8],
    _return_length: &mut u32,
) -> NtStatus {
    if ctx.handle_table.get(thread_handle.0).is_none() {
        return NtStatus(STATUS_INVALID_HANDLE);
    }

    match info_class {
        0 => {
            // ThreadBasicInformation
            if buffer.len() < 40 {
                return NtStatus(STATUS_BUFFER_TOO_SMALL);
            }
            for b in buffer.iter_mut() {
                *b = 0;
            }
            let tid = ctx.current_thread_id;
            buffer[16..20].copy_from_slice(&tid.to_le_bytes());
            *_return_length = 40;
            NtStatus(STATUS_SUCCESS)
        }
        _ => NtStatus(STATUS_NOT_IMPLEMENTED),
    }
}

// =========================================================================
// Synchronization objects
// =========================================================================

pub fn nt_create_event(
    ctx: &mut CompatContext,
    event_handle: &mut WinHandle,
    desired_access: u32,
    _object_attributes: u64,
    event_type: u32,
    initial_state: bool,
) -> NtStatus {
    let _ = (event_type, initial_state);
    let h = ctx.handle_table.allocate(
        HandleType::Event,
        desired_access,
        Some(String::from("nt_event")),
    );
    *event_handle = WinHandle(h);
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_set_event(
    ctx: &mut CompatContext,
    event_handle: WinHandle,
    previous_state: Option<&mut i32>,
) -> NtStatus {
    if ctx.handle_table.get(event_handle.0).is_none() {
        return NtStatus(STATUS_INVALID_HANDLE);
    }
    if let Some(prev) = previous_state {
        *prev = 0;
    }
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_wait_for_single_object(
    ctx: &mut CompatContext,
    handle: WinHandle,
    alertable: bool,
    timeout: Option<&LargeInteger>,
) -> NtStatus {
    if ctx.handle_table.get(handle.0).is_none() {
        return NtStatus(STATUS_INVALID_HANDLE);
    }
    let _ = (alertable, timeout);
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_wait_for_multiple_objects(
    ctx: &mut CompatContext,
    handles: &[WinHandle],
    wait_type: u32,
    alertable: bool,
    timeout: Option<&LargeInteger>,
) -> NtStatus {
    for h in handles {
        if ctx.handle_table.get(h.0).is_none() {
            return NtStatus(STATUS_INVALID_HANDLE);
        }
    }
    let _ = (wait_type, alertable, timeout);
    NtStatus(STATUS_SUCCESS)
}

// =========================================================================
// Registry operations
// =========================================================================

pub fn nt_create_key(
    ctx: &mut CompatContext,
    key_handle: &mut WinHandle,
    desired_access: u32,
    key_path: &[u16],
    _create_options: u32,
    _disposition: &mut u32,
) -> NtStatus {
    let path = wide_to_string(key_path);
    if path.is_empty() {
        return NtStatus(STATUS_INVALID_PARAMETER);
    }

    if !ctx.registry.key_exists(&path) {
        ctx.registry
            .set_value(&path, "", crate::RegValue::String(String::new()));
    }

    let h = ctx
        .handle_table
        .allocate(HandleType::RegKey, desired_access, Some(path));
    *key_handle = WinHandle(h);
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_open_key(
    ctx: &mut CompatContext,
    key_handle: &mut WinHandle,
    desired_access: u32,
    key_path: &[u16],
) -> NtStatus {
    let path = wide_to_string(key_path);
    if !ctx.registry.key_exists(&path) {
        return NtStatus(STATUS_OBJECT_NAME_NOT_FOUND);
    }

    let h = ctx
        .handle_table
        .allocate(HandleType::RegKey, desired_access, Some(path));
    *key_handle = WinHandle(h);
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_query_value_key(
    ctx: &mut CompatContext,
    key_handle: WinHandle,
    value_name: &[u16],
    _info_class: u32,
    buffer: &mut [u8],
    _result_length: &mut u32,
) -> NtStatus {
    let entry = ctx.handle_table.get(key_handle.0);
    let key_path = match entry {
        Some(e) => match &e.name {
            Some(n) => n.clone(),
            None => return NtStatus(STATUS_INVALID_HANDLE),
        },
        None => return NtStatus(STATUS_INVALID_HANDLE),
    };

    let name = wide_to_string(value_name);
    match ctx.registry.get_value(&key_path, &name) {
        Some(_val) => {
            for b in buffer.iter_mut() {
                *b = 0;
            }
            *_result_length = 0;
            NtStatus(STATUS_SUCCESS)
        }
        None => NtStatus(STATUS_OBJECT_NAME_NOT_FOUND),
    }
}

pub fn nt_set_value_key(
    ctx: &mut CompatContext,
    key_handle: WinHandle,
    value_name: &[u16],
    _value_type: u32,
    data: &[u8],
) -> NtStatus {
    let entry = ctx.handle_table.get(key_handle.0);
    let key_path = match entry {
        Some(e) => match &e.name {
            Some(n) => n.clone(),
            None => return NtStatus(STATUS_INVALID_HANDLE),
        },
        None => return NtStatus(STATUS_INVALID_HANDLE),
    };

    let name = wide_to_string(value_name);
    ctx.registry
        .set_value(&key_path, &name, crate::RegValue::Binary(data.to_vec()));
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_delete_key(ctx: &mut CompatContext, key_handle: WinHandle) -> NtStatus {
    if ctx.handle_table.get(key_handle.0).is_some() {
        ctx.handle_table.close(key_handle.0);
        NtStatus(STATUS_SUCCESS)
    } else {
        NtStatus(STATUS_INVALID_HANDLE)
    }
}

// =========================================================================
// System information
// =========================================================================

pub fn nt_query_system_information(
    _ctx: &mut CompatContext,
    info_class: u32,
    buffer: &mut [u8],
    _return_length: &mut u32,
) -> NtStatus {
    match info_class {
        0 => {
            // SystemBasicInformation
            if buffer.len() < 44 {
                return NtStatus(STATUS_BUFFER_TOO_SMALL);
            }
            for b in buffer.iter_mut() {
                *b = 0;
            }
            // Page size at offset 4
            buffer[4..8].copy_from_slice(&4096u32.to_le_bytes());
            // Number of processors at offset 40
            buffer[40..44].copy_from_slice(&8u32.to_le_bytes());
            *_return_length = 44;
            NtStatus(STATUS_SUCCESS)
        }
        _ => NtStatus(STATUS_NOT_IMPLEMENTED),
    }
}

pub fn nt_query_system_time(_ctx: &mut CompatContext, time: &mut LargeInteger) {
    time.0 = 133_500_000_000_000_000;
}

pub fn nt_query_performance_counter(
    _ctx: &mut CompatContext,
    counter: &mut LargeInteger,
    frequency: Option<&mut LargeInteger>,
) -> NtStatus {
    static COUNTER: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(1_000_000);
    counter.0 = COUNTER.fetch_add(1000, core::sync::atomic::Ordering::Relaxed);
    if let Some(freq) = frequency {
        freq.0 = 10_000_000;
    }
    NtStatus(STATUS_SUCCESS)
}

// =========================================================================
// Scheduling and yielding
// =========================================================================

pub fn nt_delay_execution(
    _ctx: &mut CompatContext,
    alertable: bool,
    delay_interval: &LargeInteger,
) -> NtStatus {
    let _ = (alertable, delay_interval);
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_yield_execution(_ctx: &mut CompatContext) -> NtStatus {
    NtStatus(STATUS_SUCCESS)
}

// =========================================================================
// Rtl utility functions
// =========================================================================

#[derive(Debug, Clone)]
pub struct UnicodeString {
    pub length: u16,
    pub maximum_length: u16,
    pub buffer: Vec<u16>,
}

pub fn rtl_init_unicode_string(target: &mut UnicodeString, source: &[u16]) {
    let len = source.iter().position(|&c| c == 0).unwrap_or(source.len());
    target.length = (len * 2) as u16;
    target.maximum_length = ((len + 1) * 2) as u16;
    target.buffer = source[..len].to_vec();
}

pub fn rtl_free_unicode_string(target: &mut UnicodeString) {
    target.buffer.clear();
    target.length = 0;
    target.maximum_length = 0;
}

pub fn rtl_copy_memory(dest: &mut [u8], src: &[u8], length: usize) {
    let count = core::cmp::min(length, core::cmp::min(dest.len(), src.len()));
    dest[..count].copy_from_slice(&src[..count]);
}

pub fn rtl_move_memory(dest: &mut [u8], src: &[u8], length: usize) {
    let count = core::cmp::min(length, core::cmp::min(dest.len(), src.len()));
    // For overlapping regions, copy_from_slice is safe in Rust since the
    // borrow checker prevents aliased mutable references. Use a temporary
    // buffer for the conceptual memmove semantics.
    let mut tmp = Vec::new();
    tmp.extend_from_slice(&src[..count]);
    dest[..count].copy_from_slice(&tmp);
}

pub fn rtl_zero_memory(dest: &mut [u8], length: usize) {
    let count = core::cmp::min(length, dest.len());
    for b in &mut dest[..count] {
        *b = 0;
    }
}

// =========================================================================
// I/O status block
// =========================================================================

#[derive(Debug, Clone, Copy, Default)]
pub struct IoStatusBlock {
    pub status: i32,
    pub information: u64,
}

// =========================================================================
// Client ID (process + thread pair)
// =========================================================================

#[derive(Debug, Clone, Copy, Default)]
pub struct ClientId {
    pub unique_process: u64,
    pub unique_thread: u64,
}

// =========================================================================
// NT information classes
// =========================================================================

pub const PROCESS_BASIC_INFORMATION: u32 = 0;
pub const THREAD_BASIC_INFORMATION: u32 = 0;
pub const SYSTEM_BASIC_INFORMATION: u32 = 0;
pub const SYSTEM_PROCESSOR_INFORMATION: u32 = 1;
pub const SYSTEM_PERFORMANCE_INFORMATION: u32 = 2;
pub const SYSTEM_TIME_OF_DAY_INFORMATION: u32 = 3;

// Event types
pub const NOTIFICATION_EVENT: u32 = 0;
pub const SYNCHRONIZATION_EVENT: u32 = 1;

// Wait types
pub const WAIT_ALL: u32 = 0;
pub const WAIT_ANY: u32 = 1;

// File information classes
pub const FILE_BASIC_INFORMATION: u32 = 4;
pub const FILE_STANDARD_INFORMATION: u32 = 5;
pub const FILE_NAME_INFORMATION: u32 = 9;
pub const FILE_POSITION_INFORMATION: u32 = 14;

// =========================================================================
// File information query
// =========================================================================

pub fn nt_query_information_file(
    ctx: &mut CompatContext,
    file_handle: WinHandle,
    io_status: &mut IoStatusBlock,
    buffer: &mut [u8],
    info_class: u32,
) -> NtStatus {
    if ctx.handle_table.get(file_handle.0).is_none() {
        return NtStatus(STATUS_INVALID_HANDLE);
    }

    match info_class {
        FILE_BASIC_INFORMATION => {
            if buffer.len() < 40 {
                return NtStatus(STATUS_BUFFER_TOO_SMALL);
            }
            for b in buffer[..40].iter_mut() {
                *b = 0;
            }
            io_status.status = STATUS_SUCCESS;
            io_status.information = 40;
            NtStatus(STATUS_SUCCESS)
        }
        FILE_STANDARD_INFORMATION => {
            if buffer.len() < 24 {
                return NtStatus(STATUS_BUFFER_TOO_SMALL);
            }
            for b in buffer[..24].iter_mut() {
                *b = 0;
            }
            io_status.status = STATUS_SUCCESS;
            io_status.information = 24;
            NtStatus(STATUS_SUCCESS)
        }
        FILE_POSITION_INFORMATION => {
            if buffer.len() < 8 {
                return NtStatus(STATUS_BUFFER_TOO_SMALL);
            }
            for b in buffer[..8].iter_mut() {
                *b = 0;
            }
            io_status.status = STATUS_SUCCESS;
            io_status.information = 8;
            NtStatus(STATUS_SUCCESS)
        }
        FILE_NAME_INFORMATION => {
            let name = ctx
                .handle_table
                .get(file_handle.0)
                .and_then(|h| h.name.clone())
                .unwrap_or_default();
            let wide: Vec<u16> = name.encode_utf16().collect();
            let name_bytes = wide.len() * 2;
            let needed = 4 + name_bytes;
            if buffer.len() < needed {
                return NtStatus(STATUS_BUFFER_TOO_SMALL);
            }
            buffer[0..4].copy_from_slice(&(name_bytes as u32).to_le_bytes());
            for (i, &ch) in wide.iter().enumerate() {
                let off = 4 + i * 2;
                buffer[off..off + 2].copy_from_slice(&ch.to_le_bytes());
            }
            io_status.status = STATUS_SUCCESS;
            io_status.information = needed as u64;
            NtStatus(STATUS_SUCCESS)
        }
        _ => NtStatus(STATUS_NOT_IMPLEMENTED),
    }
}

// =========================================================================
// NtSetInformationFile
// =========================================================================

pub fn nt_set_information_file(
    ctx: &mut CompatContext,
    file_handle: WinHandle,
    io_status: &mut IoStatusBlock,
    _buffer: &[u8],
    info_class: u32,
) -> NtStatus {
    if ctx.handle_table.get(file_handle.0).is_none() {
        return NtStatus(STATUS_INVALID_HANDLE);
    }
    let _ = info_class;
    io_status.status = STATUS_SUCCESS;
    io_status.information = 0;
    NtStatus(STATUS_SUCCESS)
}

// =========================================================================
// NtDuplicateObject
// =========================================================================

pub fn nt_duplicate_object(
    ctx: &mut CompatContext,
    _source_process: WinHandle,
    source_handle: WinHandle,
    _target_process: WinHandle,
    target_handle: &mut WinHandle,
    desired_access: u32,
    _attributes: u32,
    _options: u32,
) -> NtStatus {
    let ht = match ctx.handle_table.get(source_handle.0) {
        Some(h) => h.handle_type,
        None => return NtStatus(STATUS_INVALID_HANDLE),
    };

    let h = ctx.handle_table.allocate(ht, desired_access, None);
    *target_handle = WinHandle(h);
    NtStatus(STATUS_SUCCESS)
}

// =========================================================================
// Rtl heap (process heap operations at NT layer)
// =========================================================================

pub fn rtl_allocate_heap(
    ctx: &mut CompatContext,
    _heap_handle: u64,
    _flags: u32,
    size: u64,
) -> u64 {
    if size == 0 {
        return 0;
    }
    let base = 0x4000_0000u64 + (ctx.virtual_regions.len() as u64) * 0x10000;
    let region = VirtualRegion {
        base_address: base,
        size,
        state: MEM_COMMIT,
        protect: crate::PAGE_READWRITE,
        allocation_type: MEM_COMMIT | MEM_RESERVE,
    };
    ctx.virtual_regions.insert(base, region);
    base
}

pub fn rtl_free_heap(
    ctx: &mut CompatContext,
    _heap_handle: u64,
    _flags: u32,
    base_address: u64,
) -> bool {
    ctx.virtual_regions.remove(&base_address).is_some()
}

pub fn rtl_size_heap(
    ctx: &CompatContext,
    _heap_handle: u64,
    _flags: u32,
    base_address: u64,
) -> u64 {
    ctx.virtual_regions
        .get(&base_address)
        .map(|r| r.size)
        .unwrap_or(0)
}

// =========================================================================
// Rtl string comparison
// =========================================================================

pub fn rtl_compare_unicode_string(
    s1: &UnicodeString,
    s2: &UnicodeString,
    case_insensitive: bool,
) -> i32 {
    let len = core::cmp::min(s1.buffer.len(), s2.buffer.len());
    for i in 0..len {
        let mut c1 = s1.buffer[i];
        let mut c2 = s2.buffer[i];
        if case_insensitive {
            if c1 >= b'A' as u16 && c1 <= b'Z' as u16 {
                c1 += 32;
            }
            if c2 >= b'A' as u16 && c2 <= b'Z' as u16 {
                c2 += 32;
            }
        }
        if c1 != c2 {
            return (c1 as i32) - (c2 as i32);
        }
    }
    (s1.buffer.len() as i32) - (s2.buffer.len() as i32)
}

pub fn rtl_equal_unicode_string(
    s1: &UnicodeString,
    s2: &UnicodeString,
    case_insensitive: bool,
) -> bool {
    rtl_compare_unicode_string(s1, s2, case_insensitive) == 0
}

// =========================================================================
// Rtl number conversion
// =========================================================================

pub fn rtl_integer_to_unicode_string(
    value: u32,
    base: u32,
    target: &mut UnicodeString,
) -> NtStatus {
    let radix = if base == 0 { 10 } else { base };
    if radix < 2 || radix > 36 {
        return NtStatus(STATUS_INVALID_PARAMETER);
    }

    let mut buf = [0u8; 32];
    let mut pos = buf.len();
    let mut v = value;
    if v == 0 {
        pos -= 1;
        buf[pos] = b'0';
    } else {
        while v > 0 {
            pos -= 1;
            let digit = (v % radix) as u8;
            buf[pos] = if digit < 10 {
                b'0' + digit
            } else {
                b'A' + digit - 10
            };
            v /= radix;
        }
    }

    let chars = &buf[pos..];
    target.buffer.clear();
    for &b in chars {
        target.buffer.push(b as u16);
    }
    target.length = (target.buffer.len() * 2) as u16;
    target.maximum_length = target.length + 2;
    NtStatus(STATUS_SUCCESS)
}

// =========================================================================
// Rtl exception / unwind
// =========================================================================

#[repr(C)]
#[derive(Debug, Clone)]
pub struct ExceptionRecord {
    pub exception_code: u32,
    pub exception_flags: u32,
    pub exception_record: u64,
    pub exception_address: u64,
    pub number_parameters: u32,
    pub exception_information: [u64; 4],
}

pub fn rtl_unwind(
    _target_frame: u64,
    _target_ip: u64,
    _exception_record: Option<&ExceptionRecord>,
    _return_value: u64,
) {
    // SEH unwind — no-op in emulation (longjmp semantics handled by AthBridge runtime)
}

pub fn rtl_capture_context(_context_record: u64) {
    // captures CPU context — stub for compatibility
}

pub fn rtl_lookup_function_entry(
    _control_pc: u64,
    _image_base: &mut u64,
    _history_table: u64,
) -> u64 {
    0 // no function table entry found
}

pub fn rtl_virtual_unwind(
    _handler_type: u32,
    _image_base: u64,
    _control_pc: u64,
    _function_entry: u64,
    _context: u64,
    _handler_data: &mut u64,
    _establisher_frame: &mut u64,
    _context_pointers: u64,
) -> u64 {
    0 // no handler
}

pub fn rtl_raise_exception(_exception_record: &ExceptionRecord) {
    // in a full implementation, this would walk the SEH chain
}

pub fn rtl_add_vectored_exception_handler(_first: u32, _handler: u64) -> u64 {
    1 // non-null cookie
}

pub fn rtl_remove_vectored_exception_handler(_handle: u64) -> u32 {
    1
}

pub fn rtl_add_vectored_continue_handler(_first: u32, _handler: u64) -> u64 {
    1
}

pub fn rtl_remove_vectored_continue_handler(_handle: u64) -> u32 {
    1
}

// =========================================================================
// Additional NtQuerySystemInformation classes
// =========================================================================

pub fn nt_query_system_information_ex(
    _ctx: &mut CompatContext,
    info_class: u32,
    _input: u64,
    _input_len: u32,
    output: &mut [u8],
    return_length: &mut u32,
) -> NtStatus {
    match info_class {
        _ => {
            let _ = output;
            *return_length = 0;
            NtStatus(STATUS_NOT_IMPLEMENTED)
        }
    }
}

// =========================================================================
// NtCreateMutant / NtOpenMutant
// =========================================================================

pub fn nt_create_mutant(
    ctx: &mut CompatContext,
    handle: &mut WinHandle,
    _desired_access: u32,
    _object_attributes: u64,
    initial_owner: bool,
) -> NtStatus {
    let _ = initial_owner;
    let h = ctx
        .handle_table
        .allocate(HandleType::Mutex, 0x001F0001, None);
    *handle = WinHandle(h);
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_open_mutant(
    ctx: &mut CompatContext,
    handle: &mut WinHandle,
    _desired_access: u32,
    _object_attributes: u64,
) -> NtStatus {
    let h = ctx
        .handle_table
        .allocate(HandleType::Mutex, 0x001F0001, None);
    *handle = WinHandle(h);
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_release_mutant(
    ctx: &mut CompatContext,
    mutant: WinHandle,
    _previous_count: Option<&mut i32>,
) -> NtStatus {
    if ctx.handle_table.get(mutant.0).is_none() {
        return NtStatus(STATUS_INVALID_HANDLE);
    }
    NtStatus(STATUS_SUCCESS)
}

// =========================================================================
// NtCreateSemaphore / NtOpenSemaphore
// =========================================================================

pub fn nt_create_semaphore(
    ctx: &mut CompatContext,
    handle: &mut WinHandle,
    _desired_access: u32,
    _object_attributes: u64,
    _initial_count: i32,
    _maximum_count: i32,
) -> NtStatus {
    let h = ctx
        .handle_table
        .allocate(HandleType::Semaphore, 0x001F0003, None);
    *handle = WinHandle(h);
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_release_semaphore(
    ctx: &mut CompatContext,
    semaphore: WinHandle,
    _release_count: i32,
    _previous_count: Option<&mut i32>,
) -> NtStatus {
    if ctx.handle_table.get(semaphore.0).is_none() {
        return NtStatus(STATUS_INVALID_HANDLE);
    }
    NtStatus(STATUS_SUCCESS)
}

// =========================================================================
// NtCreateTimer / NtSetTimer
// =========================================================================

pub fn nt_create_timer(
    ctx: &mut CompatContext,
    handle: &mut WinHandle,
    _desired_access: u32,
    _object_attributes: u64,
    _timer_type: u32,
) -> NtStatus {
    let h = ctx
        .handle_table
        .allocate(HandleType::Event, 0x001F0003, None);
    *handle = WinHandle(h);
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_set_timer(
    ctx: &mut CompatContext,
    timer: WinHandle,
    _due_time: &i64,
    _apc_routine: u64,
    _apc_context: u64,
    _resume: bool,
    _period: i32,
    _previous_state: Option<&mut bool>,
) -> NtStatus {
    if ctx.handle_table.get(timer.0).is_none() {
        return NtStatus(STATUS_INVALID_HANDLE);
    }
    NtStatus(STATUS_SUCCESS)
}

pub fn nt_cancel_timer(
    ctx: &mut CompatContext,
    timer: WinHandle,
    _current_state: Option<&mut bool>,
) -> NtStatus {
    if ctx.handle_table.get(timer.0).is_none() {
        return NtStatus(STATUS_INVALID_HANDLE);
    }
    NtStatus(STATUS_SUCCESS)
}

// =========================================================================
// Rtl string / buffer utilities
// =========================================================================

pub fn rtl_init_ansi_string(target: &mut AnsiString, source: &[u8]) {
    let len = source.iter().position(|&b| b == 0).unwrap_or(source.len());
    target.buffer = Vec::from(&source[..len]);
    target.length = len as u16;
    target.maximum_length = (len + 1) as u16;
}

#[derive(Debug, Clone)]
pub struct AnsiString {
    pub length: u16,
    pub maximum_length: u16,
    pub buffer: Vec<u8>,
}

impl AnsiString {
    pub fn new() -> Self {
        Self {
            length: 0,
            maximum_length: 0,
            buffer: Vec::new(),
        }
    }
}

pub fn rtl_ansi_string_to_unicode_string(
    dest: &mut UnicodeString,
    src: &AnsiString,
    alloc_dest: bool,
) -> NtStatus {
    if alloc_dest {
        dest.buffer.clear();
    }
    for &b in &src.buffer {
        dest.buffer.push(b as u16);
    }
    dest.length = (dest.buffer.len() * 2) as u16;
    dest.maximum_length = dest.length + 2;
    NtStatus(STATUS_SUCCESS)
}

pub fn rtl_unicode_string_to_ansi_string(
    dest: &mut AnsiString,
    src: &UnicodeString,
    _alloc_dest: bool,
) -> NtStatus {
    dest.buffer.clear();
    for &ch in &src.buffer {
        dest.buffer.push(if ch <= 0xFF { ch as u8 } else { b'?' });
    }
    dest.length = dest.buffer.len() as u16;
    dest.maximum_length = dest.length + 1;
    NtStatus(STATUS_SUCCESS)
}

pub fn rtl_string_cb_copy_w(dest: &mut [u16], _cb_dest: usize, src: &[u16]) -> NtStatus {
    let copy_len = core::cmp::min(src.len(), dest.len().saturating_sub(1));
    dest[..copy_len].copy_from_slice(&src[..copy_len]);
    if copy_len < dest.len() {
        dest[copy_len] = 0;
    }
    NtStatus(STATUS_SUCCESS)
}

// =========================================================================
// Additional Rtl helpers
// =========================================================================

pub fn rtl_get_version(info: &mut OsVersionInfoExW) -> NtStatus {
    info.major_version = 10;
    info.minor_version = 0;
    info.build_number = 22631;
    info.platform_id = 2;
    info.service_pack_major = 0;
    info.service_pack_minor = 0;
    NtStatus(STATUS_SUCCESS)
}

pub fn rtl_nt_status_to_dos_error(status: NtStatus) -> u32 {
    match status.0 {
        0 => 0,            // STATUS_SUCCESS -> ERROR_SUCCESS
        0x00000103 => 997, // STATUS_PENDING -> ERROR_IO_PENDING
        s if s == STATUS_INVALID_HANDLE => 6,
        s if s == STATUS_INVALID_PARAMETER => 87,
        s if s == STATUS_ACCESS_DENIED => 5,
        s if s == STATUS_OBJECT_NAME_NOT_FOUND => 2,
        s if s == STATUS_NO_MEMORY => 8,
        s if s == STATUS_NOT_IMPLEMENTED => 50,
        _ => 317, // ERROR_MR_MID_NOT_FOUND
    }
}

pub fn rtl_encode_pointer(ptr: u64) -> u64 {
    ptr ^ 0xBAD_C0DE_FEED_FACE
}

pub fn rtl_decode_pointer(ptr: u64) -> u64 {
    ptr ^ 0xBAD_C0DE_FEED_FACE
}

pub fn rtl_random_ex(seed: &mut u32) -> u32 {
    let mut s = *seed;
    s ^= s << 13;
    s ^= s >> 17;
    s ^= s << 5;
    *seed = s;
    s
}
