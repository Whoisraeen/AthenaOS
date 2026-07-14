//! kernel32.dll — Win32 base API surface for AthBridge.
//!
//! Each function operates on a [`CompatContext`] that maps Win32 semantics
//! to AthenaOS native state.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    translate_win_path, wide_to_string, CompatContext, CreateSyncResult, DWord, HandleType,
    LargeInteger, MemoryBasicInformation, OsVersionInfoExW, SyncKind, SyncObject, SystemInfo,
    VirtualRegion, WinBool, WinHandle, CREATE_ALWAYS, CREATE_NEW, ERROR_ALREADY_EXISTS,
    ERROR_ENVVAR_NOT_FOUND, ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_HANDLE,
    ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY, ERROR_NOT_OWNER, ERROR_NOT_SUPPORTED,
    ERROR_NO_MORE_FILES, ERROR_PROC_NOT_FOUND, ERROR_SUCCESS, ERROR_TOO_MANY_POSTS, FALSE,
    FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_NORMAL, GENERIC_ALL, GENERIC_READ,
    INVALID_FILE_ATTRIBUTES, INVALID_HANDLE_VALUE, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE,
    NULL_HANDLE, OPEN_ALWAYS, OPEN_EXISTING, PAGE_READWRITE, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
    STD_OUTPUT_HANDLE, TRUE, TRUNCATE_EXISTING, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};

// =========================================================================
// Internal helpers
// =========================================================================

fn set_last_error(ctx: &mut CompatContext, code: u32) {
    ctx.last_error = code;
}

// =========================================================================
// Error retrieval
// =========================================================================

pub fn get_last_error(ctx: &CompatContext) -> DWord {
    DWord(ctx.last_error)
}

pub fn set_last_error_api(ctx: &mut CompatContext, error: DWord) {
    ctx.last_error = error.0;
}

// =========================================================================
// File I/O
// =========================================================================

pub fn create_file_w(
    ctx: &mut CompatContext,
    file_name: &[u16],
    desired_access: u32,
    _share_mode: u32,
    _security_attributes: u64,
    creation_disposition: u32,
    _flags_and_attributes: u32,
    _template_file: WinHandle,
) -> WinHandle {
    let name = wide_to_string(file_name);
    if name.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return INVALID_HANDLE_VALUE;
    }

    // Per-app bucket: each app's C:\ is namespaced so it cannot see another
    // app's files (Concept per-app data isolation). Falls back to the shared
    // mapping for non-drive paths inside `win_path_to_vfs`.
    let native_path = ctx.win_path_to_vfs(&name);

    match creation_disposition {
        CREATE_NEW => set_last_error(ctx, ERROR_SUCCESS),
        CREATE_ALWAYS | OPEN_ALWAYS => set_last_error(ctx, ERROR_SUCCESS),
        OPEN_EXISTING => set_last_error(ctx, ERROR_SUCCESS),
        TRUNCATE_EXISTING => set_last_error(ctx, ERROR_SUCCESS),
        _ => {
            set_last_error(ctx, ERROR_INVALID_PARAMETER);
            return INVALID_HANDLE_VALUE;
        }
    }

    let fd = unsafe { crate::syscalls::sys_open(native_path.as_bytes(), 0) };
    // SYS_OPEN signals errors as u64::MAX..=MAX-3 (MAX-1 = not found); treating
    // only == MAX as failure let a not-found fd through and writes silently faulted.
    if fd >= u64::MAX - 3 {
        set_last_error(ctx, ERROR_FILE_NOT_FOUND);
        return INVALID_HANDLE_VALUE;
    }

    let h = ctx
        .handle_table
        .allocate(HandleType::File, desired_access, Some(name));

    if let Some(entry) = ctx.handle_table.get_mut(h) {
        entry.native_id = Some(fd);
    }

    set_last_error(ctx, ERROR_SUCCESS);
    WinHandle(h)
}

pub fn read_file(
    ctx: &mut CompatContext,
    handle: WinHandle,
    buffer: &mut [u8],
    bytes_to_read: u32,
    bytes_read: &mut u32,
    _overlapped: u64,
) -> WinBool {
    let native_id = match ctx.handle_table.get(handle.0) {
        Some(entry) => entry.native_id,
        None => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            *bytes_read = 0;
            return FALSE;
        }
    };

    if let Some(fd) = native_id {
        let actual = core::cmp::min(bytes_to_read as usize, buffer.len());
        let read = unsafe { crate::syscalls::sys_read(fd, &mut buffer[..actual]) };
        *bytes_read = read as u32;
        set_last_error(ctx, ERROR_SUCCESS);
        TRUE
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        *bytes_read = 0;
        FALSE
    }
}

pub fn write_file(
    ctx: &mut CompatContext,
    handle: WinHandle,
    buffer: &[u8],
    bytes_to_write: u32,
    bytes_written: &mut u32,
    _overlapped: u64,
) -> WinBool {
    let native_id = match ctx.handle_table.get(handle.0) {
        Some(entry) => entry.native_id,
        None => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            *bytes_written = 0;
            return FALSE;
        }
    };

    if let Some(fd) = native_id {
        let actual = core::cmp::min(bytes_to_write as usize, buffer.len());
        let written = unsafe { crate::syscalls::sys_write(fd, &buffer[..actual]) };
        *bytes_written = written as u32;
        set_last_error(ctx, ERROR_SUCCESS);
        TRUE
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        *bytes_written = 0;
        FALSE
    }
}

pub fn close_handle(ctx: &mut CompatContext, handle: WinHandle) -> WinBool {
    if let Some(entry) = ctx.handle_table.get(handle.0) {
        if let Some(fd) = entry.native_id {
            unsafe {
                crate::syscalls::sys_close(fd);
            }
        }
    }

    // Release this handle's reference to any backing sync object (frees the
    // object + its name when the last handle closes).
    ctx.close_sync_handle(handle.0);

    if ctx.handle_table.close(handle.0) {
        set_last_error(ctx, ERROR_SUCCESS);
        TRUE
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        FALSE
    }
}

pub fn get_file_size(ctx: &mut CompatContext, handle: WinHandle, file_size_high: &mut u32) -> u32 {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0xFFFFFFFF;
    }

    *file_size_high = 0;
    set_last_error(ctx, ERROR_SUCCESS);
    0
}

pub fn set_file_pointer(
    ctx: &mut CompatContext,
    handle: WinHandle,
    distance_to_move: i32,
    _distance_to_move_high: Option<&mut i32>,
    move_method: u32,
) -> u32 {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0xFFFFFFFF;
    }

    let _ = distance_to_move;
    let _ = move_method;
    set_last_error(ctx, ERROR_SUCCESS);
    0
}

pub fn delete_file_w(ctx: &mut CompatContext, file_name: &[u16]) -> WinBool {
    let name = wide_to_string(file_name);
    if name.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return FALSE;
    }

    let _native_path = translate_win_path(&name);
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn find_first_file_w(
    ctx: &mut CompatContext,
    file_name: &[u16],
    find_data: &mut Win32FindDataW,
) -> WinHandle {
    let name = wide_to_string(file_name);
    if name.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return INVALID_HANDLE_VALUE;
    }

    let _native_path = translate_win_path(&name);

    find_data.file_attributes = FILE_ATTRIBUTE_ARCHIVE;
    find_data.file_size_low = 0;
    find_data.file_size_high = 0;
    find_data.file_name = name;

    let h = ctx.handle_table.allocate(
        HandleType::Directory,
        GENERIC_READ,
        Some(String::from("FindFile")),
    );
    set_last_error(ctx, ERROR_SUCCESS);
    WinHandle(h)
}

pub fn find_next_file_w(
    ctx: &mut CompatContext,
    handle: WinHandle,
    _find_data: &mut Win32FindDataW,
) -> WinBool {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }

    set_last_error(ctx, ERROR_NO_MORE_FILES);
    FALSE
}

pub fn find_close(ctx: &mut CompatContext, handle: WinHandle) -> WinBool {
    close_handle(ctx, handle)
}

pub fn get_file_attributes_w(ctx: &mut CompatContext, file_name: &[u16]) -> u32 {
    let name = wide_to_string(file_name);
    if name.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return INVALID_FILE_ATTRIBUTES;
    }

    let _native = translate_win_path(&name);
    set_last_error(ctx, ERROR_SUCCESS);
    FILE_ATTRIBUTE_NORMAL
}

pub fn create_directory_w(
    ctx: &mut CompatContext,
    path_name: &[u16],
    _security_attributes: u64,
) -> WinBool {
    let name = wide_to_string(path_name);
    if name.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return FALSE;
    }
    let _native = translate_win_path(&name);
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn remove_directory_w(ctx: &mut CompatContext, path_name: &[u16]) -> WinBool {
    let name = wide_to_string(path_name);
    if name.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return FALSE;
    }
    let _native = translate_win_path(&name);
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn move_file_w(ctx: &mut CompatContext, existing_file: &[u16], new_file: &[u16]) -> WinBool {
    let src = wide_to_string(existing_file);
    let dst = wide_to_string(new_file);
    if src.is_empty() || dst.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return FALSE;
    }
    let _nsrc = translate_win_path(&src);
    let _ndst = translate_win_path(&dst);
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn copy_file_w(
    ctx: &mut CompatContext,
    existing_file: &[u16],
    new_file: &[u16],
    fail_if_exists: WinBool,
) -> WinBool {
    let src = wide_to_string(existing_file);
    let dst = wide_to_string(new_file);
    if src.is_empty() || dst.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return FALSE;
    }
    if fail_if_exists.is_true() {
        // Would check if dest exists and fail
    }
    let _nsrc = translate_win_path(&src);
    let _ndst = translate_win_path(&dst);
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn get_temp_path_w(ctx: &mut CompatContext, buf: &mut [u16]) -> u32 {
    let temp = ctx
        .environment
        .get("TEMP")
        .cloned()
        .unwrap_or_else(|| String::from("C:\\Temp"));

    let wide: Vec<u16> = temp
        .encode_utf16()
        .chain(core::iter::once(0x005C))
        .collect();
    let needed = wide.len() + 1; // +1 for null terminator
    if buf.len() < needed {
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return needed as u32;
    }
    for (i, &ch) in wide.iter().enumerate() {
        buf[i] = ch;
    }
    buf[wide.len()] = 0;
    set_last_error(ctx, ERROR_SUCCESS);
    wide.len() as u32
}

pub fn get_temp_file_name_w(
    ctx: &mut CompatContext,
    path_name: &[u16],
    prefix: &[u16],
    unique: u32,
    temp_file_name: &mut [u16],
) -> u32 {
    let dir = wide_to_string(path_name);
    let pfx = wide_to_string(prefix);
    let id = if unique == 0 {
        ctx.current_thread_id
    } else {
        unique
    };

    let mut name = dir;
    if !name.ends_with('\\') {
        name.push('\\');
    }
    name.push_str(&pfx);
    // Build a hex suffix from the id
    let hex_chars = [
        b'0', b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'A', b'B', b'C', b'D', b'E',
        b'F',
    ];
    for shift in (0..8).rev() {
        let nibble = ((id >> (shift * 4)) & 0xF) as usize;
        name.push(hex_chars[nibble] as char);
    }
    name.push_str(".tmp");

    let wide: Vec<u16> = name.encode_utf16().chain(core::iter::once(0)).collect();
    if temp_file_name.len() < wide.len() {
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return 0;
    }
    for (i, &ch) in wide.iter().enumerate() {
        temp_file_name[i] = ch;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    id
}

// =========================================================================
// Find data structure
// =========================================================================

#[derive(Debug, Clone)]
pub struct Win32FindDataW {
    pub file_attributes: u32,
    pub creation_time: u64,
    pub last_access_time: u64,
    pub last_write_time: u64,
    pub file_size_high: u32,
    pub file_size_low: u32,
    pub file_name: String,
    pub alternate_file_name: String,
}

impl Win32FindDataW {
    pub fn new() -> Self {
        Self {
            file_attributes: 0,
            creation_time: 0,
            last_access_time: 0,
            last_write_time: 0,
            file_size_high: 0,
            file_size_low: 0,
            file_name: String::new(),
            alternate_file_name: String::new(),
        }
    }
}

// =========================================================================
// Process and thread management
// =========================================================================

pub fn create_process_w(
    ctx: &mut CompatContext,
    application_name: Option<&[u16]>,
    command_line: Option<&[u16]>,
    _process_attributes: u64,
    _thread_attributes: u64,
    inherit_handles: WinBool,
    creation_flags: u32,
    _environment: u64,
    current_directory: Option<&[u16]>,
    process_info: &mut ProcessInformation,
) -> WinBool {
    let _ = inherit_handles;
    let _ = creation_flags;
    let _ = current_directory;

    let app = application_name.map(wide_to_string);
    let cmd = command_line.map(wide_to_string);

    if app.is_none() && cmd.is_none() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return FALSE;
    }

    let target = app.or(cmd).unwrap_or_default();

    let mut path_bytes = target.clone().into_bytes();
    path_bytes.push(0); // null terminate
    let pid_result = unsafe { crate::syscalls::sys_spawn(&path_bytes) };

    if pid_result == u64::MAX {
        set_last_error(ctx, ERROR_FILE_NOT_FOUND);
        return FALSE;
    }

    let proc_handle = ctx
        .handle_table
        .allocate(HandleType::Process, GENERIC_ALL, Some(target));
    if let Some(entry) = ctx.handle_table.get_mut(proc_handle) {
        entry.native_id = Some(pid_result);
    }

    let thread_handle = ctx.handle_table.allocate(
        HandleType::Thread,
        GENERIC_ALL,
        Some(String::from("main_thread")),
    );

    let tid = (pid_result as u32) + 1; // Dummy TID

    process_info.process = WinHandle(proc_handle);
    process_info.thread = WinHandle(thread_handle);
    process_info.process_id = pid_result as u32;
    process_info.thread_id = tid;

    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

#[derive(Debug, Clone)]
pub struct ProcessInformation {
    pub process: WinHandle,
    pub thread: WinHandle,
    pub process_id: u32,
    pub thread_id: u32,
}

impl ProcessInformation {
    pub fn new() -> Self {
        Self {
            process: NULL_HANDLE,
            thread: NULL_HANDLE,
            process_id: 0,
            thread_id: 0,
        }
    }
}

pub fn exit_process(_ctx: &mut CompatContext, exit_code: u32) {
    unsafe { crate::syscalls::sys_exit(exit_code as u64) };
}

pub fn get_current_process_id(ctx: &CompatContext) -> u32 {
    ctx.current_process_id
}

pub fn get_current_thread_id(ctx: &CompatContext) -> u32 {
    ctx.current_thread_id
}

pub fn create_thread(
    ctx: &mut CompatContext,
    _stack_size: u64,
    _start_address: u64,
    _parameter: u64,
    creation_flags: u32,
    thread_id: &mut u32,
) -> WinHandle {
    // Threading is NOT supported in the native AthenaOS kernel yet (no SYS_THREAD_CREATE).
    // We return ERROR_NOT_SUPPORTED.
    let _ = creation_flags;
    let _ = thread_id;
    set_last_error(ctx, ERROR_NOT_SUPPORTED);
    NULL_HANDLE
}

pub fn exit_thread(_ctx: &mut CompatContext, _exit_code: u32) {
    // Thread termination — in a real implementation this would unwind the
    // thread's stack and update scheduling data structures.
}

pub fn terminate_process(ctx: &mut CompatContext, handle: WinHandle, exit_code: u32) -> WinBool {
    if let Some(entry) = ctx.handle_table.get(handle.0) {
        if let Some(pid) = entry.native_id {
            unsafe { crate::syscalls::sys_kill(pid) };
            let _ = exit_code;
            set_last_error(ctx, ERROR_SUCCESS);
            return TRUE;
        }
    }
    set_last_error(ctx, ERROR_INVALID_HANDLE);
    FALSE
}

pub fn wait_for_single_object(
    ctx: &mut CompatContext,
    handle: WinHandle,
    milliseconds: u32,
) -> u32 {
    let _ = milliseconds;
    // A process handle waits natively on the child via SYS_WAIT.
    if let Some(entry) = ctx.handle_table.get(handle.0) {
        if entry.handle_type == HandleType::Process {
            if let Some(pid) = entry.native_id {
                let code = unsafe { crate::syscalls::sys_wait(pid) };
                if code == u64::MAX {
                    return WAIT_FAILED;
                }
                set_last_error(ctx, ERROR_SUCCESS);
                return WAIT_OBJECT_0;
            }
            // No native pid yet — treat as already signaled.
            set_last_error(ctx, ERROR_SUCCESS);
            return WAIT_OBJECT_0;
        }
    }

    let tid = ctx.current_thread_id;
    match try_acquire_sync(ctx, handle.0, tid) {
        Some(code) => {
            set_last_error(ctx, ERROR_SUCCESS);
            code
        }
        None => {
            // Not a sync object and not a process handle. A valid handle of
            // another type (thread, file) is treated as signaled; an unknown
            // handle is an error.
            if ctx.handle_table.get(handle.0).is_some() {
                set_last_error(ctx, ERROR_SUCCESS);
                WAIT_OBJECT_0
            } else {
                set_last_error(ctx, ERROR_INVALID_HANDLE);
                WAIT_FAILED
            }
        }
    }
}

pub fn wait_for_multiple_objects(
    ctx: &mut CompatContext,
    handles: &[WinHandle],
    wait_all: WinBool,
    milliseconds: u32,
) -> u32 {
    let _ = milliseconds;
    if handles.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return WAIT_FAILED;
    }
    for h in handles {
        if ctx.handle_table.get(h.0).is_none() {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            return WAIT_FAILED;
        }
    }

    let tid = ctx.current_thread_id;

    if wait_all.is_true() {
        // All-or-nothing: only acquire if every sync object is satisfiable now.
        let all_ready = handles.iter().all(|h| {
            match ctx
                .sync_object(h.0)
                .map(|o| (o.kind, o.signaled, o.owner_thread))
            {
                None => true, // non-sync handle counts as signaled
                Some((SyncKind::Mutex, signaled, owner)) => signaled || owner == tid,
                Some((_, signaled, _)) => signaled,
            }
        });
        if !all_ready {
            set_last_error(ctx, ERROR_SUCCESS);
            return WAIT_TIMEOUT;
        }
        for h in handles {
            let _ = try_acquire_sync(ctx, h.0, tid);
        }
        set_last_error(ctx, ERROR_SUCCESS);
        return WAIT_OBJECT_0;
    }

    // wait_any: acquire the first satisfiable object, returning its index.
    for (i, h) in handles.iter().enumerate() {
        match try_acquire_sync(ctx, h.0, tid) {
            Some(WAIT_OBJECT_0) => {
                set_last_error(ctx, ERROR_SUCCESS);
                return WAIT_OBJECT_0 + i as u32;
            }
            // A non-sync handle: treat as signaled at this index.
            None => {
                set_last_error(ctx, ERROR_SUCCESS);
                return WAIT_OBJECT_0 + i as u32;
            }
            _ => {}
        }
    }
    set_last_error(ctx, ERROR_SUCCESS);
    WAIT_TIMEOUT
}

pub fn sleep(_ctx: &mut CompatContext, milliseconds: u32) {
    let _ = milliseconds;
}

pub fn sleep_ex(_ctx: &mut CompatContext, milliseconds: u32, alertable: WinBool) -> u32 {
    let _ = milliseconds;
    let _ = alertable;
    0
}

pub fn get_exit_code_process(
    ctx: &mut CompatContext,
    handle: WinHandle,
    exit_code: &mut u32,
) -> WinBool {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    *exit_code = 0;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn get_exit_code_thread(
    ctx: &mut CompatContext,
    handle: WinHandle,
    exit_code: &mut u32,
) -> WinBool {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    *exit_code = 0;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn resume_thread(ctx: &mut CompatContext, handle: WinHandle) -> u32 {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0xFFFFFFFF;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    1
}

pub fn suspend_thread(ctx: &mut CompatContext, handle: WinHandle) -> u32 {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0xFFFFFFFF;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    0
}

pub fn set_thread_priority(ctx: &mut CompatContext, handle: WinHandle, priority: i32) -> WinBool {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    let _ = priority;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn get_thread_priority(ctx: &mut CompatContext, handle: WinHandle) -> i32 {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0x7FFFFFFF; // THREAD_PRIORITY_ERROR_RETURN
    }
    set_last_error(ctx, ERROR_SUCCESS);
    0 // THREAD_PRIORITY_NORMAL
}

// =========================================================================
// Memory management
// =========================================================================

pub fn virtual_alloc(
    ctx: &mut CompatContext,
    address: u64,
    size: u64,
    allocation_type: u32,
    protect: u32,
) -> u64 {
    if size == 0 {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return 0;
    }

    let page_size: u64 = 4096;
    let aligned_size = (size + page_size - 1) & !(page_size - 1);

    let prot = 3; // PROT_READ | PROT_WRITE
    let flags = 0; // MAP_ANON | MAP_PRIVATE

    let base =
        unsafe { crate::syscalls::sys_mmap(address, aligned_size, prot, flags, u64::MAX, 0) };

    if base == u64::MAX {
        set_last_error(ctx, ERROR_NOT_ENOUGH_MEMORY);
        return 0;
    }

    let region = VirtualRegion {
        base_address: base,
        size: aligned_size,
        state: allocation_type & (MEM_COMMIT | MEM_RESERVE),
        protect,
        allocation_type,
    };
    ctx.virtual_regions.insert(base, region);
    set_last_error(ctx, ERROR_SUCCESS);
    base
}

pub fn virtual_free(ctx: &mut CompatContext, address: u64, size: u64, free_type: u32) -> WinBool {
    if address == 0 {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return FALSE;
    }

    if free_type == MEM_RELEASE {
        if size != 0 {
            set_last_error(ctx, ERROR_INVALID_PARAMETER);
            return FALSE;
        }
        if let Some(region) = ctx.virtual_regions.remove(&address) {
            unsafe {
                crate::syscalls::sys_munmap(address, region.size);
            }
            set_last_error(ctx, ERROR_SUCCESS);
            return TRUE;
        }
    }

    set_last_error(ctx, ERROR_INVALID_PARAMETER);
    FALSE
}

pub fn virtual_protect(
    ctx: &mut CompatContext,
    address: u64,
    _size: u64,
    new_protect: u32,
    old_protect: &mut u32,
) -> WinBool {
    if let Some(region) = ctx.virtual_regions.get_mut(&address) {
        *old_protect = region.protect;
        region.protect = new_protect;
        set_last_error(ctx, ERROR_SUCCESS);
        TRUE
    } else {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        *old_protect = 0;
        FALSE
    }
}

pub fn virtual_query(
    ctx: &mut CompatContext,
    address: u64,
    info: &mut MemoryBasicInformation,
) -> u64 {
    if let Some(region) = ctx.virtual_regions.get(&address) {
        info.base_address = region.base_address;
        info.allocation_base = region.base_address;
        info.allocation_protect = region.protect;
        info.region_size = region.size;
        info.state = region.state;
        info.protect = region.protect;
        info.mem_type = 0x00020000; // MEM_PRIVATE
        set_last_error(ctx, ERROR_SUCCESS);
        core::mem::size_of::<MemoryBasicInformation>() as u64
    } else {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        0
    }
}

pub fn heap_create(
    ctx: &mut CompatContext,
    _options: u32,
    initial_size: u64,
    _maximum_size: u64,
) -> WinHandle {
    let heap_id = ctx.next_heap_id;
    ctx.next_heap_id += 0x10000;

    let region = VirtualRegion {
        base_address: heap_id,
        size: if initial_size > 0 {
            initial_size
        } else {
            0x100000
        },
        state: MEM_COMMIT,
        protect: PAGE_READWRITE,
        allocation_type: MEM_COMMIT | MEM_RESERVE,
    };
    ctx.virtual_regions.insert(heap_id, region);

    set_last_error(ctx, ERROR_SUCCESS);
    WinHandle(heap_id)
}

pub fn heap_destroy(ctx: &mut CompatContext, heap: WinHandle) -> WinBool {
    if ctx.virtual_regions.remove(&heap.0).is_some() {
        set_last_error(ctx, ERROR_SUCCESS);
        TRUE
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        FALSE
    }
}

/// HeapAlloc flag: zero the returned memory.
pub const HEAP_ZERO_MEMORY: u32 = 0x0000_0008;

/// Win32 heap allocations are 16-byte aligned on x64 (MSVC CRT relies on it
/// for SSE stores into heap blocks).
const HEAP_ALIGN: u64 = 16;

pub fn heap_alloc(ctx: &mut CompatContext, heap: WinHandle, flags: u32, bytes: u64) -> u64 {
    if ctx.virtual_regions.get(&heap.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0;
    }

    // HeapAlloc(_, _, 0) returns a valid unique pointer on Windows; round
    // a zero-byte request up to one aligned cell instead of refusing it.
    let size = core::cmp::max(bytes, 1);
    let layout = match core::alloc::Layout::from_size_align(size as usize, HEAP_ALIGN as usize) {
        Ok(l) => l,
        Err(_) => {
            set_last_error(ctx, ERROR_NOT_ENOUGH_MEMORY);
            return 0;
        }
    };

    let ptr = unsafe {
        if flags & HEAP_ZERO_MEMORY != 0 {
            alloc::alloc::alloc_zeroed(layout)
        } else {
            alloc::alloc::alloc(layout)
        }
    };
    if ptr.is_null() {
        set_last_error(ctx, ERROR_NOT_ENOUGH_MEMORY);
        return 0;
    }

    ctx.heap_allocations.insert(ptr as u64, (size, HEAP_ALIGN));
    set_last_error(ctx, ERROR_SUCCESS);
    ptr as u64
}

pub fn heap_free(ctx: &mut CompatContext, heap: WinHandle, _flags: u32, mem: u64) -> WinBool {
    if ctx.virtual_regions.get(&heap.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    // HeapFree(NULL) is a no-op success on Windows.
    if mem == 0 {
        set_last_error(ctx, ERROR_SUCCESS);
        return TRUE;
    }
    match ctx.heap_allocations.remove(&mem) {
        Some((size, align)) => {
            // SAFETY: (mem, size, align) came from the matching heap_alloc
            // insert above, and the entry was just removed so this pointer
            // cannot be freed twice through this path.
            unsafe {
                let layout =
                    core::alloc::Layout::from_size_align_unchecked(size as usize, align as usize);
                alloc::alloc::dealloc(mem as *mut u8, layout);
            }
            set_last_error(ctx, ERROR_SUCCESS);
            TRUE
        }
        None => {
            set_last_error(ctx, ERROR_INVALID_PARAMETER);
            FALSE
        }
    }
}

pub fn heap_realloc(
    ctx: &mut CompatContext,
    heap: WinHandle,
    flags: u32,
    mem: u64,
    bytes: u64,
) -> u64 {
    if mem == 0 {
        return heap_alloc(ctx, heap, flags, bytes);
    }
    let old_size = match ctx.heap_allocations.get(&mem) {
        Some(&(size, _)) => size,
        None => {
            set_last_error(ctx, ERROR_INVALID_PARAMETER);
            return 0;
        }
    };
    let new_ptr = heap_alloc(ctx, heap, flags, bytes);
    if new_ptr == 0 {
        return 0;
    }
    let copy = core::cmp::min(old_size, core::cmp::max(bytes, 1)) as usize;
    // SAFETY: both pointers are live heap blocks of at least `copy` bytes:
    // `mem` was verified against heap_allocations above and `new_ptr` was
    // just allocated with size >= max(bytes, 1).
    unsafe {
        core::ptr::copy_nonoverlapping(mem as *const u8, new_ptr as *mut u8, copy);
    }
    heap_free(ctx, heap, 0, mem);
    new_ptr
}

pub fn get_process_heap(ctx: &mut CompatContext) -> WinHandle {
    let default_heap = 0x00100000u64;
    if ctx.virtual_regions.get(&default_heap).is_none() {
        let region = VirtualRegion {
            base_address: default_heap,
            size: 0x100000,
            state: MEM_COMMIT,
            protect: PAGE_READWRITE,
            allocation_type: MEM_COMMIT | MEM_RESERVE,
        };
        ctx.virtual_regions.insert(default_heap, region);
    }
    set_last_error(ctx, ERROR_SUCCESS);
    WinHandle(default_heap)
}

pub fn global_alloc(ctx: &mut CompatContext, flags: u32, bytes: u64) -> u64 {
    let heap = get_process_heap(ctx);
    heap_alloc(ctx, heap, flags, bytes)
}

pub fn global_free(ctx: &mut CompatContext, mem: u64) -> u64 {
    let heap = get_process_heap(ctx);
    if heap_free(ctx, heap, 0, mem).is_true() {
        0
    } else {
        mem
    }
}

pub fn local_alloc(ctx: &mut CompatContext, flags: u32, bytes: u64) -> u64 {
    global_alloc(ctx, flags, bytes)
}

pub fn local_free(ctx: &mut CompatContext, mem: u64) -> u64 {
    global_free(ctx, mem)
}

// =========================================================================
// Synchronization primitives
// =========================================================================

/// Common create-or-open path: build the handle for a `create_sync_object`
/// result and translate it to Win32 last-error semantics (`ERROR_ALREADY_EXISTS`
/// for a named reopen, `ERROR_INVALID_HANDLE` for a name/kind mismatch).
fn finish_create_sync(ctx: &mut CompatContext, result: CreateSyncResult) -> WinHandle {
    match result {
        CreateSyncResult::Created(h) => {
            set_last_error(ctx, ERROR_SUCCESS);
            WinHandle(h)
        }
        CreateSyncResult::Opened(h) => {
            // Windows returns a valid handle AND sets ERROR_ALREADY_EXISTS so
            // the caller can tell it didn't create the object (the standard
            // "am I the first instance?" single-instance-app idiom).
            set_last_error(ctx, ERROR_ALREADY_EXISTS);
            WinHandle(h)
        }
        CreateSyncResult::TypeMismatch => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            NULL_HANDLE
        }
    }
}

pub fn create_mutex_w(
    ctx: &mut CompatContext,
    _security_attributes: u64,
    initial_owner: WinBool,
    name: Option<&[u16]>,
) -> WinHandle {
    let label = name.map(wide_to_string).filter(|s| !s.is_empty());
    let owner = if initial_owner.is_true() {
        ctx.current_thread_id
    } else {
        0
    };
    let result = ctx.create_sync_object(SyncObject::mutex(owner, label));
    finish_create_sync(ctx, result)
}

pub fn open_mutex_w(
    ctx: &mut CompatContext,
    _desired_access: u32,
    _inherit: WinBool,
    name: Option<&[u16]>,
) -> WinHandle {
    let label = name.map(wide_to_string).filter(|s| !s.is_empty());
    match label.and_then(|n| ctx.open_sync_object(SyncKind::Mutex, &n)) {
        Some(h) => {
            set_last_error(ctx, ERROR_SUCCESS);
            WinHandle(h)
        }
        None => {
            set_last_error(ctx, ERROR_FILE_NOT_FOUND);
            NULL_HANDLE
        }
    }
}

pub fn release_mutex(ctx: &mut CompatContext, handle: WinHandle) -> WinBool {
    let tid = ctx.current_thread_id;
    match ctx.sync_object_mut(handle.0) {
        Some(obj) if obj.kind == SyncKind::Mutex => {
            if obj.owner_thread != tid {
                set_last_error(ctx, ERROR_NOT_OWNER);
                return FALSE;
            }
            obj.recursion = obj.recursion.saturating_sub(1);
            if obj.recursion == 0 {
                obj.owner_thread = 0;
                obj.signaled = true; // now free
            }
            set_last_error(ctx, ERROR_SUCCESS);
            TRUE
        }
        _ => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            FALSE
        }
    }
}

pub fn create_event_w(
    ctx: &mut CompatContext,
    _security_attributes: u64,
    manual_reset: WinBool,
    initial_state: WinBool,
    name: Option<&[u16]>,
) -> WinHandle {
    let label = name.map(wide_to_string).filter(|s| !s.is_empty());
    let result = ctx.create_sync_object(SyncObject::event(
        manual_reset.is_true(),
        initial_state.is_true(),
        label,
    ));
    finish_create_sync(ctx, result)
}

pub fn open_event_w(
    ctx: &mut CompatContext,
    _desired_access: u32,
    _inherit: WinBool,
    name: Option<&[u16]>,
) -> WinHandle {
    let label = name.map(wide_to_string).filter(|s| !s.is_empty());
    match label.and_then(|n| ctx.open_sync_object(SyncKind::Event, &n)) {
        Some(h) => {
            set_last_error(ctx, ERROR_SUCCESS);
            WinHandle(h)
        }
        None => {
            set_last_error(ctx, ERROR_FILE_NOT_FOUND);
            NULL_HANDLE
        }
    }
}

pub fn set_event(ctx: &mut CompatContext, handle: WinHandle) -> WinBool {
    match ctx.sync_object_mut(handle.0) {
        Some(obj) if obj.kind == SyncKind::Event => {
            obj.signaled = true;
            set_last_error(ctx, ERROR_SUCCESS);
            TRUE
        }
        _ => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            FALSE
        }
    }
}

pub fn reset_event(ctx: &mut CompatContext, handle: WinHandle) -> WinBool {
    match ctx.sync_object_mut(handle.0) {
        Some(obj) if obj.kind == SyncKind::Event => {
            obj.signaled = false;
            set_last_error(ctx, ERROR_SUCCESS);
            TRUE
        }
        _ => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            FALSE
        }
    }
}

/// `PulseEvent`: release waiters then immediately reset. With the non-blocking
/// in-process model (no parked waiters), the observable net effect for a manual-
/// reset event is "transiently signaled then cleared" — we model it as leaving
/// the event reset, which matches what a poller sees after the pulse returns.
pub fn pulse_event(ctx: &mut CompatContext, handle: WinHandle) -> WinBool {
    match ctx.sync_object_mut(handle.0) {
        Some(obj) if obj.kind == SyncKind::Event => {
            obj.signaled = false;
            set_last_error(ctx, ERROR_SUCCESS);
            TRUE
        }
        _ => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            FALSE
        }
    }
}

pub fn create_semaphore_w(
    ctx: &mut CompatContext,
    _security_attributes: u64,
    initial_count: i32,
    maximum_count: i32,
    name: Option<&[u16]>,
) -> WinHandle {
    if initial_count < 0 || maximum_count <= 0 || initial_count > maximum_count {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return NULL_HANDLE;
    }

    let label = name.map(wide_to_string).filter(|s| !s.is_empty());
    let result = ctx.create_sync_object(SyncObject::semaphore(initial_count, maximum_count, label));
    finish_create_sync(ctx, result)
}

pub fn open_semaphore_w(
    ctx: &mut CompatContext,
    _desired_access: u32,
    _inherit: WinBool,
    name: Option<&[u16]>,
) -> WinHandle {
    let label = name.map(wide_to_string).filter(|s| !s.is_empty());
    match label.and_then(|n| ctx.open_sync_object(SyncKind::Semaphore, &n)) {
        Some(h) => {
            set_last_error(ctx, ERROR_SUCCESS);
            WinHandle(h)
        }
        None => {
            set_last_error(ctx, ERROR_FILE_NOT_FOUND);
            NULL_HANDLE
        }
    }
}

pub fn release_semaphore(
    ctx: &mut CompatContext,
    handle: WinHandle,
    release_count: i32,
    previous_count: Option<&mut i32>,
) -> WinBool {
    if release_count < 1 {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return FALSE;
    }
    match ctx.sync_object_mut(handle.0) {
        Some(obj) if obj.kind == SyncKind::Semaphore => {
            if obj.count as i64 + release_count as i64 > obj.max_count as i64 {
                set_last_error(ctx, ERROR_TOO_MANY_POSTS);
                return FALSE;
            }
            let prev = obj.count;
            obj.count += release_count;
            obj.signaled = obj.count > 0;
            if let Some(p) = previous_count {
                *p = prev;
            }
            set_last_error(ctx, ERROR_SUCCESS);
            TRUE
        }
        _ => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            FALSE
        }
    }
}

/// Non-blocking acquire of one waitable object for `thread_id`, performing the
/// state transition when satisfiable. Returns `Some(WAIT_OBJECT_0)` on success,
/// `Some(WAIT_TIMEOUT)` when not currently satisfiable, or `None` if the handle
/// is not a sync object. True blocking across threads is the broker's job
/// (`docs/components/raebridge-wine-strategy.md` §6.1, slice 2).
fn try_acquire_sync(ctx: &mut CompatContext, handle: u64, thread_id: u32) -> Option<u32> {
    let obj = ctx.sync_object_mut(handle)?;
    let code = match obj.kind {
        SyncKind::Mutex => {
            if obj.owner_thread == thread_id && obj.recursion > 0 {
                obj.recursion += 1; // recursive re-acquire by the owner
                WAIT_OBJECT_0
            } else if obj.signaled {
                obj.signaled = false;
                obj.owner_thread = thread_id;
                obj.recursion = 1;
                WAIT_OBJECT_0
            } else {
                WAIT_TIMEOUT
            }
        }
        SyncKind::Event => {
            if obj.signaled {
                if !obj.manual_reset {
                    obj.signaled = false; // auto-reset consumes the signal
                }
                WAIT_OBJECT_0
            } else {
                WAIT_TIMEOUT
            }
        }
        SyncKind::Semaphore => {
            if obj.count > 0 {
                obj.count -= 1;
                obj.signaled = obj.count > 0;
                WAIT_OBJECT_0
            } else {
                WAIT_TIMEOUT
            }
        }
    };
    Some(code)
}

#[derive(Debug)]
pub struct CriticalSection {
    pub lock_count: i32,
    pub recursion_count: i32,
    pub owning_thread: u32,
}

pub fn initialize_critical_section(cs: &mut CriticalSection) {
    cs.lock_count = -1;
    cs.recursion_count = 0;
    cs.owning_thread = 0;
}

pub fn enter_critical_section(ctx: &CompatContext, cs: &mut CriticalSection) {
    cs.lock_count += 1;
    cs.recursion_count += 1;
    cs.owning_thread = ctx.current_thread_id;
}

pub fn leave_critical_section(cs: &mut CriticalSection) {
    cs.recursion_count -= 1;
    if cs.recursion_count == 0 {
        cs.owning_thread = 0;
    }
    cs.lock_count -= 1;
}

pub fn delete_critical_section(cs: &mut CriticalSection) {
    cs.lock_count = -1;
    cs.recursion_count = 0;
    cs.owning_thread = 0;
}

// =========================================================================
// DLL / module loading
// =========================================================================

pub fn load_library_w(ctx: &mut CompatContext, lib_file_name: &[u16]) -> WinHandle {
    let name = wide_to_string(lib_file_name);
    if name.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return NULL_HANDLE;
    }

    let lower = {
        let mut s = String::new();
        for c in name.chars() {
            s.push(if c.is_ascii_uppercase() {
                (c as u8 + 32) as char
            } else {
                c
            });
        }
        s
    };

    if let Some(&base) = ctx.loaded_modules.get(&lower) {
        set_last_error(ctx, ERROR_SUCCESS);
        return WinHandle(base);
    }

    let base_addr = 0x7000_0000u64 + (ctx.loaded_modules.len() as u64) * 0x0010_0000;
    ctx.loaded_modules.insert(lower, base_addr);
    set_last_error(ctx, ERROR_SUCCESS);
    WinHandle(base_addr)
}

pub fn free_library(ctx: &mut CompatContext, module: WinHandle) -> WinBool {
    let found = ctx
        .loaded_modules
        .iter()
        .find(|(_, &v)| v == module.0)
        .map(|(k, _)| k.clone());

    if let Some(key) = found {
        ctx.loaded_modules.remove(&key);
        set_last_error(ctx, ERROR_SUCCESS);
        TRUE
    } else {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        FALSE
    }
}

/// Resolve an exported function address by (module handle, name).
///
/// REAL resolution: the module handle is mapped back to its DLL name via the
/// session module list, then the (dll, name) pair is looked up in the win64
/// shim table (`winapi_shims::resolve_shim`). A hit returns the live shim
/// address — the exact address the IAT patcher would write — so a CRT that
/// resolves a function dynamically gets the same callable code as a statically
/// imported one. An unknown name returns NULL with ERROR_PROC_NOT_FOUND, which
/// is what real Windows does (and what the CRT's feature-probing expects).
pub fn get_proc_address(ctx: &mut CompatContext, module: WinHandle, proc_name: &str) -> u64 {
    // Map the handle back to the DLL name it was registered under.
    let dll = ctx
        .loaded_modules
        .iter()
        .find(|(_, &v)| v == module.0)
        .map(|(k, _)| k.clone());

    let dll = match dll {
        Some(d) => d,
        None => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            return 0;
        }
    };

    if let Some(addr) = crate::winapi_shims::resolve_shim(&dll, proc_name) {
        set_last_error(ctx, ERROR_SUCCESS);
        return addr;
    }

    set_last_error(ctx, ERROR_PROC_NOT_FOUND);
    0
}

pub fn get_module_handle_w(ctx: &mut CompatContext, module_name: Option<&[u16]>) -> WinHandle {
    match module_name {
        None => {
            set_last_error(ctx, ERROR_SUCCESS);
            WinHandle(0x0040_0000) // conventional base for main executable
        }
        Some(name) => {
            let s = wide_to_string(name);
            let lower = {
                let mut r = String::new();
                for c in s.chars() {
                    r.push(if c.is_ascii_uppercase() {
                        (c as u8 + 32) as char
                    } else {
                        c
                    });
                }
                r
            };

            if let Some(&base) = ctx.loaded_modules.get(&lower) {
                set_last_error(ctx, ERROR_SUCCESS);
                WinHandle(base)
            } else {
                set_last_error(ctx, ERROR_FILE_NOT_FOUND);
                NULL_HANDLE
            }
        }
    }
}

pub fn get_module_file_name_w(
    ctx: &mut CompatContext,
    module: WinHandle,
    filename: &mut [u16],
) -> u32 {
    let path = if module.0 == 0 || module.0 == 0x0040_0000 {
        ctx.session.image_name.clone()
    } else {
        let found = ctx
            .loaded_modules
            .iter()
            .find(|(_, &v)| v == module.0)
            .map(|(k, _)| k.clone());
        match found {
            Some(name) => name,
            None => {
                set_last_error(ctx, ERROR_INVALID_HANDLE);
                return 0;
            }
        }
    };

    let wide: Vec<u16> = path.encode_utf16().chain(core::iter::once(0)).collect();
    if filename.len() < wide.len() {
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return filename.len() as u32;
    }
    for (i, &ch) in wide.iter().enumerate() {
        filename[i] = ch;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    (wide.len() - 1) as u32 // exclude null
}

// =========================================================================
// System information
// =========================================================================

pub fn get_system_info(_ctx: &mut CompatContext, info: &mut SystemInfo) {
    info.processor_architecture = 9; // PROCESSOR_ARCHITECTURE_AMD64
    info.page_size = 4096;
    info.min_app_address = 0x0001_0000;
    info.max_app_address = 0x7FFF_FFFE_FFFF;
    info.active_processor_mask = 0xFF;
    info.number_of_processors = 8;
    info.processor_type = 8664;
    info.allocation_granularity = 65536;
    info.processor_level = 6;
    info.processor_revision = 0x5F07;
}

pub fn get_version_ex_w(_ctx: &mut CompatContext, info: &mut OsVersionInfoExW) {
    info.major_version = 10;
    info.minor_version = 0;
    info.build_number = 22631;
    info.platform_id = 2; // VER_PLATFORM_WIN32_NT
    info.service_pack_major = 0;
    info.service_pack_minor = 0;
    info.suite_mask = 0x0300; // VER_SUITE_TERMINAL | VER_SUITE_SINGLEUSERTS
    info.product_type = 1; // VER_NT_WORKSTATION
}

pub fn get_system_time_as_file_time(_ctx: &mut CompatContext) -> u64 {
    // Windows FILETIME epoch is 1601-01-01. Return a plausible value
    // representing roughly 2025-01-01 in 100ns ticks.
    133_500_000_000_000_000u64
}

pub fn query_performance_counter(_ctx: &mut CompatContext, counter: &mut LargeInteger) {
    static COUNTER: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(1_000_000);
    let val = COUNTER.fetch_add(1000, core::sync::atomic::Ordering::Relaxed);
    counter.0 = val;
}

pub fn query_performance_frequency(_ctx: &mut CompatContext, frequency: &mut LargeInteger) {
    frequency.0 = 10_000_000; // 10 MHz — typical Windows QPC frequency
}

pub fn get_tick_count(_ctx: &mut CompatContext) -> u32 {
    static TICKS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(60_000);
    TICKS.fetch_add(16, core::sync::atomic::Ordering::Relaxed)
}

pub fn get_tick_count_64(_ctx: &mut CompatContext) -> u64 {
    static TICKS64: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(60_000);
    TICKS64.fetch_add(16, core::sync::atomic::Ordering::Relaxed)
}

pub fn get_computer_name_w(ctx: &mut CompatContext, buffer: &mut [u16], size: &mut u32) -> WinBool {
    let name = "ATHENAOS";
    let wide: Vec<u16> = name.encode_utf16().chain(core::iter::once(0)).collect();
    if (*size as usize) < wide.len() {
        *size = wide.len() as u32;
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return FALSE;
    }
    for (i, &ch) in wide.iter().enumerate() {
        buffer[i] = ch;
    }
    *size = (wide.len() - 1) as u32;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn get_user_name_w(ctx: &mut CompatContext, buffer: &mut [u16], size: &mut u32) -> WinBool {
    let user = ctx
        .environment
        .get("USERNAME")
        .cloned()
        .unwrap_or_else(|| String::from("user"));

    let wide: Vec<u16> = user.encode_utf16().chain(core::iter::once(0)).collect();
    if (*size as usize) < wide.len() {
        *size = wide.len() as u32;
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return FALSE;
    }
    for (i, &ch) in wide.iter().enumerate() {
        buffer[i] = ch;
    }
    *size = (wide.len() - 1) as u32;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

// =========================================================================
// Console I/O
// =========================================================================

pub fn get_std_handle(ctx: &mut CompatContext, std_handle: u32) -> WinHandle {
    let h = match std_handle {
        STD_INPUT_HANDLE => 4,
        STD_OUTPUT_HANDLE => 8,
        STD_ERROR_HANDLE => 12,
        _ => {
            set_last_error(ctx, ERROR_INVALID_PARAMETER);
            return INVALID_HANDLE_VALUE;
        }
    };
    set_last_error(ctx, ERROR_SUCCESS);
    WinHandle(h)
}

pub fn write_console_w(
    ctx: &mut CompatContext,
    handle: WinHandle,
    buffer: &[u16],
    chars_written: &mut u32,
) -> WinBool {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        *chars_written = 0;
        return FALSE;
    }
    *chars_written = buffer.len() as u32;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn read_console_w(
    ctx: &mut CompatContext,
    handle: WinHandle,
    buffer: &mut [u16],
    chars_read: &mut u32,
) -> WinBool {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        *chars_read = 0;
        return FALSE;
    }
    for ch in buffer.iter_mut() {
        *ch = 0;
    }
    *chars_read = 0;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn set_console_ctrl_handler(_ctx: &mut CompatContext, _handler: u64, _add: WinBool) -> WinBool {
    TRUE
}

pub fn alloc_console(ctx: &mut CompatContext) -> WinBool {
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn free_console(ctx: &mut CompatContext) -> WinBool {
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

// =========================================================================
// Environment and directory
// =========================================================================

pub fn get_environment_variable_w(
    ctx: &mut CompatContext,
    name: &[u16],
    buffer: &mut [u16],
) -> u32 {
    let key = wide_to_string(name);
    match ctx.environment.get(&key) {
        Some(val) => {
            let wide: Vec<u16> = val.encode_utf16().chain(core::iter::once(0)).collect();
            if buffer.len() < wide.len() {
                set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
                return wide.len() as u32;
            }
            for (i, &ch) in wide.iter().enumerate() {
                buffer[i] = ch;
            }
            set_last_error(ctx, ERROR_SUCCESS);
            (wide.len() - 1) as u32
        }
        None => {
            set_last_error(ctx, ERROR_ENVVAR_NOT_FOUND);
            0
        }
    }
}

pub fn set_environment_variable_w(
    ctx: &mut CompatContext,
    name: &[u16],
    value: Option<&[u16]>,
) -> WinBool {
    let key = wide_to_string(name);
    if key.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return FALSE;
    }

    match value {
        Some(v) => {
            let val = wide_to_string(v);
            ctx.environment.insert(key, val);
        }
        None => {
            ctx.environment.remove(&key);
        }
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn get_command_line_w(ctx: &CompatContext) -> &str {
    &ctx.command_line
}

pub fn get_current_directory_w(ctx: &mut CompatContext, buffer: &mut [u16]) -> u32 {
    let dir = ctx.working_directory.clone();
    let wide: Vec<u16> = dir.encode_utf16().chain(core::iter::once(0)).collect();
    if buffer.len() < wide.len() {
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return wide.len() as u32;
    }
    for (i, &ch) in wide.iter().enumerate() {
        buffer[i] = ch;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    (wide.len() - 1) as u32
}

pub fn set_current_directory_w(ctx: &mut CompatContext, path_name: &[u16]) -> WinBool {
    let path = wide_to_string(path_name);
    if path.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return FALSE;
    }
    ctx.working_directory = path;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

// =========================================================================
// ANSI string helpers
// =========================================================================

fn cstr_to_string(ptr: &[u8]) -> String {
    let end = ptr.iter().position(|&b| b == 0).unwrap_or(ptr.len());
    let mut s = String::new();
    for &b in &ptr[..end] {
        s.push(b as char);
    }
    s
}

fn string_to_ansi_buf(s: &str, buf: &mut [u8]) -> usize {
    let bytes = s.as_bytes();
    let copy_len = core::cmp::min(bytes.len(), buf.len().saturating_sub(1));
    buf[..copy_len].copy_from_slice(&bytes[..copy_len]);
    if copy_len < buf.len() {
        buf[copy_len] = 0;
    }
    copy_len
}

// =========================================================================
// ANSI File I/O variants
// =========================================================================

pub fn create_file_a(
    ctx: &mut CompatContext,
    file_name: &[u8],
    desired_access: u32,
    share_mode: u32,
    security_attributes: u64,
    creation_disposition: u32,
    flags_and_attributes: u32,
    template_file: WinHandle,
) -> WinHandle {
    let name = cstr_to_string(file_name);
    let wide: Vec<u16> = name.encode_utf16().chain(core::iter::once(0)).collect();
    create_file_w(
        ctx,
        &wide,
        desired_access,
        share_mode,
        security_attributes,
        creation_disposition,
        flags_and_attributes,
        template_file,
    )
}

// =========================================================================
// Process handle
// =========================================================================

pub fn get_current_process(_ctx: &CompatContext) -> WinHandle {
    WinHandle(0xFFFF_FFFF_FFFF_FFFE) // pseudo-handle, same as Windows
}

// =========================================================================
// ANSI Console I/O
// =========================================================================

pub fn write_console_a(
    ctx: &mut CompatContext,
    handle: WinHandle,
    buffer: &[u8],
    chars_written: &mut u32,
) -> WinBool {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        *chars_written = 0;
        return FALSE;
    }
    *chars_written = buffer.len() as u32;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn set_console_text_attribute(
    ctx: &mut CompatContext,
    handle: WinHandle,
    attributes: u16,
) -> WinBool {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    let _ = attributes;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn set_console_title_w(ctx: &mut CompatContext, _title: &[u16]) -> WinBool {
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn get_console_mode(ctx: &mut CompatContext, handle: WinHandle, mode: &mut u32) -> WinBool {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    *mode = 0x0007; // ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn set_console_mode(ctx: &mut CompatContext, handle: WinHandle, mode: u32) -> WinBool {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    let _ = mode;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

// =========================================================================
// ANSI module loading variants
// =========================================================================

pub fn load_library_a(ctx: &mut CompatContext, lib_file_name: &[u8]) -> WinHandle {
    let name = cstr_to_string(lib_file_name);
    let wide: Vec<u16> = name.encode_utf16().chain(core::iter::once(0)).collect();
    load_library_w(ctx, &wide)
}

pub fn get_module_handle_a(ctx: &mut CompatContext, module_name: Option<&[u8]>) -> WinHandle {
    match module_name {
        None => get_module_handle_w(ctx, None),
        Some(name) => {
            let s = cstr_to_string(name);
            let wide: Vec<u16> = s.encode_utf16().chain(core::iter::once(0)).collect();
            get_module_handle_w(ctx, Some(&wide))
        }
    }
}

pub fn get_module_file_name_a(
    ctx: &mut CompatContext,
    module: WinHandle,
    filename: &mut [u8],
) -> u32 {
    let path = if module.0 == 0 || module.0 == 0x0040_0000 {
        ctx.session.image_name.clone()
    } else {
        let found = ctx
            .loaded_modules
            .iter()
            .find(|(_, &v)| v == module.0)
            .map(|(k, _)| k.clone());
        match found {
            Some(name) => name,
            None => {
                set_last_error(ctx, ERROR_INVALID_HANDLE);
                return 0;
            }
        }
    };

    let written = string_to_ansi_buf(&path, filename);
    set_last_error(ctx, ERROR_SUCCESS);
    written as u32
}

// =========================================================================
// Output debug string
// =========================================================================

pub fn output_debug_string_a(_ctx: &mut CompatContext, _output_string: &[u8]) {
    // In a real implementation this would forward to a debugger or log.
}

pub fn output_debug_string_w(_ctx: &mut CompatContext, _output_string: &[u16]) {
    // In a real implementation this would forward to a debugger or log.
}

// =========================================================================
// Interlocked operations (single-threaded shims)
// =========================================================================

pub fn interlocked_increment(addend: &mut i32) -> i32 {
    *addend += 1;
    *addend
}

pub fn interlocked_decrement(addend: &mut i32) -> i32 {
    *addend -= 1;
    *addend
}

pub fn interlocked_exchange(target: &mut i32, value: i32) -> i32 {
    let old = *target;
    *target = value;
    old
}

pub fn interlocked_compare_exchange(destination: &mut i32, exchange: i32, comparand: i32) -> i32 {
    let old = *destination;
    if old == comparand {
        *destination = exchange;
    }
    old
}

// =========================================================================
// TLS (Thread Local Storage)
// =========================================================================
//
// Real per-session slot array (CRT startup calls TlsAlloc and round-trips a
// value through TlsSetValue/TlsGetValue during __scrt setup). The session is
// single-threaded today (no SYS_THREAD_CREATE), so one slot array backs the
// whole process — correct for the CRT-startup-to-main path. TLS_OUT_OF_INDEXES
// (0xFFFFFFFF) is the documented failure sentinel.

/// `TLS_OUT_OF_INDEXES` — returned by `TlsAlloc` when no slot is available.
pub const TLS_OUT_OF_INDEXES: u32 = 0xFFFF_FFFF;

/// Windows guarantees at least 64 (TLS_MINIMUM_AVAILABLE), modern builds 1088.
const TLS_MAX_SLOTS: usize = 1088;

pub fn tls_alloc(ctx: &mut CompatContext) -> u32 {
    // Reuse a freed slot first (a free slot is `None`).
    if let Some(idx) = ctx.tls_slots.iter().position(|s| s.is_none()) {
        ctx.tls_slots[idx] = Some(0);
        return idx as u32;
    }
    if ctx.tls_slots.len() >= TLS_MAX_SLOTS {
        set_last_error(ctx, ERROR_NOT_ENOUGH_MEMORY);
        return TLS_OUT_OF_INDEXES;
    }
    let idx = ctx.tls_slots.len();
    ctx.tls_slots.push(Some(0));
    idx as u32
}

pub fn tls_free(ctx: &mut CompatContext, tls_index: u32) -> WinBool {
    let i = tls_index as usize;
    match ctx.tls_slots.get_mut(i) {
        Some(slot) if slot.is_some() => {
            *slot = None;
            set_last_error(ctx, ERROR_SUCCESS);
            TRUE
        }
        _ => {
            set_last_error(ctx, ERROR_INVALID_PARAMETER);
            FALSE
        }
    }
}

pub fn tls_get_value(ctx: &mut CompatContext, tls_index: u32) -> u64 {
    let i = tls_index as usize;
    let value = ctx.tls_slots.get(i).copied().flatten();
    match value {
        Some(v) => {
            // GetLastError is cleared to ERROR_SUCCESS on a successful read so
            // the caller can disambiguate a stored 0 from a bad index.
            set_last_error(ctx, ERROR_SUCCESS);
            v
        }
        None => {
            set_last_error(ctx, ERROR_INVALID_PARAMETER);
            0
        }
    }
}

pub fn tls_set_value(ctx: &mut CompatContext, tls_index: u32, tls_value: u64) -> WinBool {
    let i = tls_index as usize;
    match ctx.tls_slots.get_mut(i) {
        Some(slot) if slot.is_some() => {
            *slot = Some(tls_value);
            set_last_error(ctx, ERROR_SUCCESS);
            TRUE
        }
        _ => {
            set_last_error(ctx, ERROR_INVALID_PARAMETER);
            FALSE
        }
    }
}

// =========================================================================
// Miscellaneous
// =========================================================================

pub fn is_debugger_present(_ctx: &CompatContext) -> WinBool {
    FALSE
}

pub fn get_current_process_id_token(ctx: &CompatContext) -> WinHandle {
    let _ = ctx;
    WinHandle(0xFFFF_FFFF_FFFF_FFFC) // pseudo-handle
}

pub fn flush_file_buffers(ctx: &mut CompatContext, handle: WinHandle) -> WinBool {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn get_file_type(ctx: &mut CompatContext, handle: WinHandle) -> u32 {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0x0000; // FILE_TYPE_UNKNOWN
    }
    set_last_error(ctx, ERROR_SUCCESS);
    0x0001 // FILE_TYPE_DISK
}

pub fn get_system_directory_w(ctx: &mut CompatContext, buffer: &mut [u16]) -> u32 {
    let sysdir = "C:\\Windows\\System32";
    let wide: Vec<u16> = sysdir.encode_utf16().chain(core::iter::once(0)).collect();
    if buffer.len() < wide.len() {
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return wide.len() as u32;
    }
    for (i, &ch) in wide.iter().enumerate() {
        buffer[i] = ch;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    (wide.len() - 1) as u32
}

pub fn get_windows_directory_w(ctx: &mut CompatContext, buffer: &mut [u16]) -> u32 {
    let windir = "C:\\Windows";
    let wide: Vec<u16> = windir.encode_utf16().chain(core::iter::once(0)).collect();
    if buffer.len() < wide.len() {
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return wide.len() as u32;
    }
    for (i, &ch) in wide.iter().enumerate() {
        buffer[i] = ch;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    (wide.len() - 1) as u32
}

pub fn set_handle_information(
    ctx: &mut CompatContext,
    handle: WinHandle,
    mask: u32,
    flags: u32,
) -> WinBool {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    let _ = mask;
    let _ = flags;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn duplicate_handle(
    ctx: &mut CompatContext,
    source_process: WinHandle,
    source_handle: WinHandle,
    _target_process: WinHandle,
    target_handle: &mut WinHandle,
    desired_access: u32,
    inherit: WinBool,
    options: u32,
) -> WinBool {
    let _ = source_process;
    let _ = inherit;
    let _ = options;

    let ht = match ctx.handle_table.get(source_handle.0) {
        Some(h) => h.handle_type,
        None => {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            return FALSE;
        }
    };

    let new_h = ctx.handle_table.allocate(ht, desired_access, None);
    *target_handle = WinHandle(new_h);
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

// =========================================================================
// Codepage / locale
// =========================================================================

pub fn get_acp(_ctx: &CompatContext) -> u32 {
    1252 // Windows-1252 (Western European)
}

pub fn get_oemcp(_ctx: &CompatContext) -> u32 {
    437 // OEM US
}

pub fn is_valid_code_page(_ctx: &CompatContext, code_page: u32) -> WinBool {
    match code_page {
        437 | 850 | 1250 | 1251 | 1252 | 1253 | 1254 | 1255 | 1256 | 1257 | 1258 | 20127
        | 28591 | 65001 => TRUE,
        _ => FALSE,
    }
}

pub fn multi_byte_to_wide_char(
    _ctx: &mut CompatContext,
    _code_page: u32,
    _flags: u32,
    multi_byte_str: &[u8],
    wide_char_str: Option<&mut [u16]>,
) -> i32 {
    match wide_char_str {
        None => multi_byte_str.len() as i32,
        Some(buf) => {
            let copy_len = core::cmp::min(multi_byte_str.len(), buf.len());
            for i in 0..copy_len {
                buf[i] = multi_byte_str[i] as u16;
            }
            copy_len as i32
        }
    }
}

pub fn wide_char_to_multi_byte(
    _ctx: &mut CompatContext,
    _code_page: u32,
    _flags: u32,
    wide_char_str: &[u16],
    multi_byte_str: Option<&mut [u8]>,
    _default_char: Option<&u8>,
    _used_default: Option<&mut WinBool>,
) -> i32 {
    match multi_byte_str {
        None => wide_char_str.len() as i32,
        Some(buf) => {
            let copy_len = core::cmp::min(wide_char_str.len(), buf.len());
            for i in 0..copy_len {
                buf[i] = if wide_char_str[i] <= 0xFF {
                    wide_char_str[i] as u8
                } else {
                    b'?'
                };
            }
            copy_len as i32
        }
    }
}

pub fn get_locale_info_w(
    ctx: &mut CompatContext,
    _locale: u32,
    lc_type: u32,
    buf: Option<&mut [u16]>,
) -> i32 {
    let result: &str = match lc_type & 0xFFFF {
        0x0001 => "en-US",         // LOCALE_SLANGUAGE
        0x0002 => "English",       // LOCALE_SENGLANGUAGE
        0x0003 => "ENU",           // LOCALE_SABBREVLANGNAME
        0x0005 => "United States", // LOCALE_SCOUNTRY
        0x0006 => "USA",           // LOCALE_SABBREVCTRYNAME
        0x000E => ".",             // LOCALE_SDECIMAL
        0x000F => ",",             // LOCALE_STHOUSAND
        0x0014 => "AM",            // LOCALE_S1159
        0x0015 => "PM",            // LOCALE_S2359
        0x001D => "2",             // LOCALE_IDIGITS
        0x002A => "yyyy-MM-dd",    // LOCALE_SLONGDATE
        0x001F => "M/d/yyyy",      // LOCALE_SSHORTDATE
        0x0059 => "en-US",         // LOCALE_SNAME
        _ => "",
    };
    let needed = result.len() as i32 + 1;
    match buf {
        None => needed,
        Some(b) => {
            if (b.len() as i32) < needed {
                set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
                return 0;
            }
            for (i, ch) in result.bytes().enumerate() {
                b[i] = ch as u16;
            }
            b[result.len()] = 0;
            needed
        }
    }
}

pub fn get_user_default_lcid(_ctx: &CompatContext) -> u32 {
    0x0409 // en-US
}

pub fn get_system_default_lcid(_ctx: &CompatContext) -> u32 {
    0x0409
}

pub fn get_user_default_lang_id(_ctx: &CompatContext) -> u16 {
    0x0409
}

// =========================================================================
// Startup info
// =========================================================================

#[repr(C)]
#[derive(Debug)]
pub struct StartupInfoW {
    pub cb: u32,
    pub desktop: [u16; 1],
    pub title: [u16; 1],
    pub x: u32,
    pub y: u32,
    pub x_size: u32,
    pub y_size: u32,
    pub x_count_chars: u32,
    pub y_count_chars: u32,
    pub fill_attribute: u32,
    pub flags: u32,
    pub show_window: u16,
    pub std_input: WinHandle,
    pub std_output: WinHandle,
    pub std_error: WinHandle,
}

pub fn get_startup_info_w(_ctx: &CompatContext, info: &mut StartupInfoW) {
    info.cb = core::mem::size_of::<StartupInfoW>() as u32;
    info.x = 0;
    info.y = 0;
    info.x_size = 800;
    info.y_size = 600;
    info.x_count_chars = 120;
    info.y_count_chars = 30;
    info.fill_attribute = 0;
    info.flags = 0;
    info.show_window = 1; // SW_SHOWNORMAL
    info.std_input = NULL_HANDLE;
    info.std_output = NULL_HANDLE;
    info.std_error = NULL_HANDLE;
}

// =========================================================================
// File mapping (Section objects)
// =========================================================================

pub fn create_file_mapping_w(
    ctx: &mut CompatContext,
    file: WinHandle,
    _security: u64,
    protect: u32,
    max_size_high: u32,
    max_size_low: u32,
    _name: Option<&[u16]>,
) -> WinHandle {
    let size = ((max_size_high as u64) << 32) | (max_size_low as u64);
    let _ = file;
    let _ = protect;
    if size == 0 && file.0 == 0 {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return NULL_HANDLE;
    }
    let h = ctx
        .handle_table
        .allocate(HandleType::Section, 0xF001F, None);
    set_last_error(ctx, ERROR_SUCCESS);
    WinHandle(h)
}

pub fn map_view_of_file(
    ctx: &mut CompatContext,
    mapping: WinHandle,
    desired_access: u32,
    offset_high: u32,
    offset_low: u32,
    bytes_to_map: u64,
) -> u64 {
    if ctx.handle_table.get(mapping.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return 0;
    }
    let _ = desired_access;
    let _ = offset_high;
    let _ = offset_low;
    let alloc_size = if bytes_to_map == 0 {
        4096
    } else {
        bytes_to_map
    };
    let aligned = (alloc_size + 4095) & !4095;
    let base = unsafe { crate::syscalls::sys_mmap(0, aligned, 3, 0, u64::MAX, 0) };
    if base == u64::MAX {
        set_last_error(ctx, ERROR_NOT_ENOUGH_MEMORY);
        return 0;
    }
    let region = VirtualRegion {
        base_address: base,
        size: aligned,
        state: MEM_COMMIT,
        protect: PAGE_READWRITE,
        allocation_type: MEM_COMMIT | MEM_RESERVE,
    };
    ctx.virtual_regions.insert(base, region);
    set_last_error(ctx, ERROR_SUCCESS);
    base
}

pub fn unmap_view_of_file(ctx: &mut CompatContext, base_address: u64) -> WinBool {
    if let Some(region) = ctx.virtual_regions.remove(&base_address) {
        unsafe {
            crate::syscalls::sys_munmap(base_address, region.size);
        }
        set_last_error(ctx, ERROR_SUCCESS);
        TRUE
    } else {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        FALSE
    }
}

// =========================================================================
// Fiber-Local Storage (FLS)
// =========================================================================

pub fn fls_alloc(_ctx: &mut CompatContext, _callback: u64) -> u32 {
    static NEXT_FLS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    NEXT_FLS.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

pub fn fls_free(_ctx: &mut CompatContext, _fls_index: u32) -> WinBool {
    TRUE
}

pub fn fls_get_value(_ctx: &CompatContext, _fls_index: u32) -> u64 {
    0
}

pub fn fls_set_value(_ctx: &mut CompatContext, _fls_index: u32, _fls_data: u64) -> WinBool {
    TRUE
}

// =========================================================================
// Init-once
// =========================================================================

#[repr(C)]
#[derive(Debug)]
pub struct InitOnce {
    pub state: core::sync::atomic::AtomicU32,
}

pub const INIT_ONCE_STATIC_INIT: u32 = 0;

pub fn init_once_begin_initialize(
    init_once: &InitOnce,
    _flags: u32,
    pending: &mut WinBool,
    _context: &mut u64,
) -> WinBool {
    let prev = init_once.state.compare_exchange(
        0,
        1,
        core::sync::atomic::Ordering::AcqRel,
        core::sync::atomic::Ordering::Acquire,
    );
    match prev {
        Ok(_) => {
            *pending = TRUE;
        }
        Err(2) => {
            *pending = FALSE;
        }
        Err(_) => {
            *pending = FALSE;
        }
    }
    TRUE
}

pub fn init_once_complete(init_once: &InitOnce, _flags: u32, _context: u64) -> WinBool {
    init_once
        .state
        .store(2, core::sync::atomic::Ordering::Release);
    TRUE
}

// =========================================================================
// Path helpers
// =========================================================================

pub fn get_full_path_name_w(
    ctx: &mut CompatContext,
    file_name: &[u16],
    buf: &mut [u16],
    _file_part: &mut u64,
) -> u32 {
    let name = crate::wide_to_string(file_name);
    if name.is_empty() {
        set_last_error(ctx, ERROR_INVALID_PARAMETER);
        return 0;
    }
    let full = if name.contains(':') || name.starts_with('\\') {
        name
    } else {
        let mut cwd = ctx.working_directory.clone();
        if !cwd.ends_with('\\') {
            cwd.push('\\');
        }
        cwd.push_str(&name);
        cwd
    };
    let needed = full.len() as u32 + 1;
    if (buf.len() as u32) < needed {
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return needed;
    }
    for (i, ch) in full.bytes().enumerate() {
        buf[i] = ch as u16;
    }
    buf[full.len()] = 0;
    set_last_error(ctx, ERROR_SUCCESS);
    full.len() as u32
}

pub fn get_long_path_name_w(ctx: &mut CompatContext, short_path: &[u16], buf: &mut [u16]) -> u32 {
    let name = crate::wide_to_string(short_path);
    let needed = name.len() as u32 + 1;
    if (buf.len() as u32) < needed {
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return needed;
    }
    for (i, ch) in name.bytes().enumerate() {
        buf[i] = ch as u16;
    }
    buf[name.len()] = 0;
    set_last_error(ctx, ERROR_SUCCESS);
    name.len() as u32
}

pub fn get_short_path_name_w(ctx: &mut CompatContext, long_path: &[u16], buf: &mut [u16]) -> u32 {
    get_long_path_name_w(ctx, long_path, buf)
}

pub fn search_path_w(
    ctx: &mut CompatContext,
    _path: Option<&[u16]>,
    file_name: &[u16],
    _extension: Option<&[u16]>,
    buf: &mut [u16],
    _file_part: &mut u64,
) -> u32 {
    let name = crate::wide_to_string(file_name);
    let sys = String::from(r"C:\Windows\system32\");
    let full = {
        let mut p = sys;
        p.push_str(&name);
        p
    };
    let needed = full.len() as u32 + 1;
    if (buf.len() as u32) < needed {
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return needed;
    }
    for (i, ch) in full.bytes().enumerate() {
        buf[i] = ch as u16;
    }
    buf[full.len()] = 0;
    set_last_error(ctx, ERROR_SUCCESS);
    full.len() as u32
}

// =========================================================================
// Disk / volume
// =========================================================================

pub fn get_disk_free_space_ex_w(
    ctx: &mut CompatContext,
    _directory_name: Option<&[u16]>,
    free_bytes_available: &mut u64,
    total_bytes: &mut u64,
    total_free_bytes: &mut u64,
) -> WinBool {
    *total_bytes = 500 * 1024 * 1024 * 1024; // 500 GB
    *total_free_bytes = 200 * 1024 * 1024 * 1024; // 200 GB free
    *free_bytes_available = *total_free_bytes;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn get_drive_type_w(_ctx: &CompatContext, _root: Option<&[u16]>) -> u32 {
    3 // DRIVE_FIXED
}

pub fn get_volume_information_w(
    ctx: &mut CompatContext,
    _root: Option<&[u16]>,
    volume_name: Option<&mut [u16]>,
    volume_serial: &mut u32,
    max_component_len: &mut u32,
    fs_flags: &mut u32,
    fs_name: Option<&mut [u16]>,
) -> WinBool {
    if let Some(vn) = volume_name {
        let label = "AthenaOS";
        for (i, ch) in label.bytes().enumerate() {
            if i >= vn.len() {
                break;
            }
            vn[i] = ch as u16;
        }
        if label.len() < vn.len() {
            vn[label.len()] = 0;
        }
    }
    *volume_serial = 0xAE00_0001_u32.wrapping_add(42);
    *max_component_len = 255;
    *fs_flags = 0x0002_0206; // case-sensitive, unicode, compression, named-streams
    if let Some(fsn) = fs_name {
        let ntfs = "NTFS";
        for (i, ch) in ntfs.bytes().enumerate() {
            if i >= fsn.len() {
                break;
            }
            fsn[i] = ch as u16;
        }
        if ntfs.len() < fsn.len() {
            fsn[ntfs.len()] = 0;
        }
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn get_logical_drives(_ctx: &CompatContext) -> u32 {
    0b0100 // drive C:
}

pub fn get_logical_drive_strings_w(ctx: &mut CompatContext, buf: &mut [u16]) -> u32 {
    let drives = "C:\\\0\0";
    let needed = drives.len() as u32;
    if (buf.len() as u32) < needed {
        set_last_error(ctx, ERROR_INSUFFICIENT_BUFFER);
        return needed;
    }
    for (i, ch) in drives.bytes().enumerate() {
        buf[i] = ch as u16;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    needed - 1
}

// =========================================================================
// I/O Completion Ports
// =========================================================================

pub fn create_io_completion_port(
    ctx: &mut CompatContext,
    file_handle: WinHandle,
    existing_port: WinHandle,
    _completion_key: u64,
    _threads: u32,
) -> WinHandle {
    if existing_port.0 != 0 {
        if ctx.handle_table.get(existing_port.0).is_none() {
            set_last_error(ctx, ERROR_INVALID_HANDLE);
            return NULL_HANDLE;
        }
        set_last_error(ctx, ERROR_SUCCESS);
        return existing_port;
    }
    let _ = file_handle;
    let h = ctx
        .handle_table
        .allocate(HandleType::IoCompletion, 0x1F0003, None);
    set_last_error(ctx, ERROR_SUCCESS);
    WinHandle(h)
}

pub fn get_queued_completion_status(
    ctx: &mut CompatContext,
    port: WinHandle,
    bytes_transferred: &mut u32,
    completion_key: &mut u64,
    overlapped: &mut u64,
    _timeout: u32,
) -> WinBool {
    if ctx.handle_table.get(port.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    *bytes_transferred = 0;
    *completion_key = 0;
    *overlapped = 0;
    set_last_error(ctx, WAIT_TIMEOUT);
    FALSE
}

pub fn post_queued_completion_status(
    ctx: &mut CompatContext,
    port: WinHandle,
    _bytes_transferred: u32,
    _completion_key: u64,
    _overlapped: u64,
) -> WinBool {
    if ctx.handle_table.get(port.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

// =========================================================================
// Pipes
// =========================================================================

pub fn create_pipe(
    ctx: &mut CompatContext,
    read_pipe: &mut WinHandle,
    write_pipe: &mut WinHandle,
    _security: u64,
    _size: u32,
) -> WinBool {
    let rh = ctx
        .handle_table
        .allocate(HandleType::File, GENERIC_READ, None);
    let wh = ctx
        .handle_table
        .allocate(HandleType::File, GENERIC_ALL, None);
    *read_pipe = WinHandle(rh);
    *write_pipe = WinHandle(wh);
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn create_named_pipe_w(
    ctx: &mut CompatContext,
    _name: &[u16],
    _open_mode: u32,
    _pipe_mode: u32,
    _max_instances: u32,
    _out_buffer_size: u32,
    _in_buffer_size: u32,
    _default_timeout: u32,
    _security: u64,
) -> WinHandle {
    let h = ctx
        .handle_table
        .allocate(HandleType::File, GENERIC_ALL, None);
    set_last_error(ctx, ERROR_SUCCESS);
    WinHandle(h)
}

pub fn connect_named_pipe(ctx: &mut CompatContext, pipe: WinHandle, _overlapped: u64) -> WinBool {
    if ctx.handle_table.get(pipe.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn peek_named_pipe(
    ctx: &mut CompatContext,
    pipe: WinHandle,
    _buffer: u64,
    _buffer_size: u32,
    bytes_read: &mut u32,
    total_avail: &mut u32,
    _bytes_left: &mut u32,
) -> WinBool {
    if ctx.handle_table.get(pipe.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    *bytes_read = 0;
    *total_avail = 0;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

// =========================================================================
// Dynamic processor / NUMA
// =========================================================================

pub fn get_native_system_info(ctx: &mut CompatContext, info: &mut SystemInfo) {
    get_system_info(ctx, info);
}

pub fn is_processor_feature_present(_ctx: &CompatContext, feature: u32) -> WinBool {
    match feature {
        10 => TRUE, // PF_XMMI64_INSTRUCTIONS_AVAILABLE (SSE2)
        3 => TRUE,  // PF_XMMI_INSTRUCTIONS_AVAILABLE (SSE)
        23 => TRUE, // PF_SSE3_INSTRUCTIONS_AVAILABLE
        _ => FALSE,
    }
}

pub fn get_system_time(_ctx: &CompatContext, sys_time: &mut [u16; 8]) {
    sys_time[0] = 2024; // year
    sys_time[1] = 1; // month
    sys_time[2] = 1; // day of week (Monday)
    sys_time[3] = 1; // day
    sys_time[4] = 0; // hour
    sys_time[5] = 0; // minute
    sys_time[6] = 0; // second
    sys_time[7] = 0; // milliseconds
}

pub fn get_local_time(ctx: &CompatContext, sys_time: &mut [u16; 8]) {
    get_system_time(ctx, sys_time);
}

pub fn file_time_to_system_time(
    _ctx: &CompatContext,
    file_time: u64,
    sys_time: &mut [u16; 8],
) -> WinBool {
    let _ = file_time;
    sys_time[0] = 2024;
    sys_time[1] = 1;
    sys_time[2] = 1;
    sys_time[3] = 1;
    sys_time[4] = 0;
    sys_time[5] = 0;
    sys_time[6] = 0;
    sys_time[7] = 0;
    TRUE
}

pub fn system_time_to_file_time(
    _ctx: &CompatContext,
    _sys_time: &[u16; 8],
    file_time: &mut u64,
) -> WinBool {
    *file_time = 132_539_328_000_000_000; // Jan 1 2024 in FILETIME
    TRUE
}

// =========================================================================
// Misc process / system
// =========================================================================

pub fn get_process_times(
    _ctx: &CompatContext,
    _process: WinHandle,
    creation: &mut u64,
    exit: &mut u64,
    kernel: &mut u64,
    user: &mut u64,
) -> WinBool {
    *creation = 132_539_328_000_000_000;
    *exit = 0;
    *kernel = 1_000_000;
    *user = 5_000_000;
    TRUE
}

pub fn set_thread_affinity_mask(_ctx: &mut CompatContext, _thread: WinHandle, mask: u64) -> u64 {
    mask // return previous mask (pretend same)
}

pub fn set_process_affinity_mask(
    _ctx: &mut CompatContext,
    _process: WinHandle,
    _mask: u64,
) -> WinBool {
    TRUE
}

pub fn get_process_affinity_mask(
    _ctx: &CompatContext,
    _process: WinHandle,
    process_mask: &mut u64,
    system_mask: &mut u64,
) -> WinBool {
    *process_mask = 0xFF; // 8 cores
    *system_mask = 0xFF;
    TRUE
}

pub fn set_priority_class(
    _ctx: &mut CompatContext,
    _process: WinHandle,
    _priority_class: u32,
) -> WinBool {
    TRUE
}

pub fn get_priority_class(_ctx: &CompatContext, _process: WinHandle) -> u32 {
    0x20 // NORMAL_PRIORITY_CLASS
}

pub fn get_process_id(_ctx: &CompatContext, _process: WinHandle) -> u32 {
    1000
}

pub fn get_thread_id(_ctx: &CompatContext, _thread: WinHandle) -> u32 {
    1001
}

pub fn is_wow64_process(_ctx: &CompatContext, _process: WinHandle, wow64: &mut WinBool) -> WinBool {
    *wow64 = FALSE; // native 64-bit
    TRUE
}

// =========================================================================
// Additional console
// =========================================================================

pub fn get_console_screen_buffer_info(
    ctx: &mut CompatContext,
    handle: WinHandle,
    info: &mut ConsoleScreenBufferInfo,
) -> WinBool {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    info.size_x = 120;
    info.size_y = 30;
    info.cursor_x = 0;
    info.cursor_y = 0;
    info.attributes = 7; // light gray on black
    info.window_left = 0;
    info.window_top = 0;
    info.window_right = 119;
    info.window_bottom = 29;
    info.max_window_x = 120;
    info.max_window_y = 30;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

#[derive(Debug)]
pub struct ConsoleScreenBufferInfo {
    pub size_x: i16,
    pub size_y: i16,
    pub cursor_x: i16,
    pub cursor_y: i16,
    pub attributes: u16,
    pub window_left: i16,
    pub window_top: i16,
    pub window_right: i16,
    pub window_bottom: i16,
    pub max_window_x: i16,
    pub max_window_y: i16,
}

pub fn set_console_cursor_position(
    ctx: &mut CompatContext,
    handle: WinHandle,
    _x: i16,
    _y: i16,
) -> WinBool {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

pub fn fill_console_output_character_w(
    ctx: &mut CompatContext,
    handle: WinHandle,
    _character: u16,
    _length: u32,
    _coord_x: i16,
    _coord_y: i16,
    chars_written: &mut u32,
) -> WinBool {
    if ctx.handle_table.get(handle.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    *chars_written = _length;
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

// =========================================================================
// Waitable timer
// =========================================================================

pub fn create_waitable_timer_w(
    ctx: &mut CompatContext,
    _security: u64,
    manual_reset: WinBool,
    _name: Option<&[u16]>,
) -> WinHandle {
    let _ = manual_reset;
    let h = ctx.handle_table.allocate(HandleType::Event, 0x1F0003, None);
    set_last_error(ctx, ERROR_SUCCESS);
    WinHandle(h)
}

pub fn set_waitable_timer(
    ctx: &mut CompatContext,
    timer: WinHandle,
    _due_time: &i64,
    _period: i32,
    _completion: u64,
    _arg: u64,
    _resume: WinBool,
) -> WinBool {
    if ctx.handle_table.get(timer.0).is_none() {
        set_last_error(ctx, ERROR_INVALID_HANDLE);
        return FALSE;
    }
    set_last_error(ctx, ERROR_SUCCESS);
    TRUE
}

// =========================================================================
// Condition variable / SRW lock
// =========================================================================

pub fn initialize_condition_variable(_cv: &mut u64) {
    // no-op, zero-init is valid
}

pub fn wake_condition_variable(_cv: &mut u64) {
    // no-op in single-threaded emulation
}

pub fn wake_all_condition_variable(_cv: &mut u64) {
    // no-op
}

pub fn initialize_srw_lock(_lock: &mut u64) {
    // zero-init is valid
}

pub fn acquire_srw_lock_exclusive(_lock: &mut u64) {
    // no-op in single-threaded emulation
}

pub fn release_srw_lock_exclusive(_lock: &mut u64) {
    // no-op
}

pub fn acquire_srw_lock_shared(_lock: &mut u64) {
    // no-op
}

pub fn release_srw_lock_shared(_lock: &mut u64) {
    // no-op
}

// =========================================================================
// Enclave / misc stubs for Steam CEG
// =========================================================================

pub fn set_dll_directory_w(_ctx: &mut CompatContext, _path: Option<&[u16]>) -> WinBool {
    TRUE
}

pub fn add_dll_directory(_ctx: &mut CompatContext, _path: &[u16]) -> u64 {
    1 // non-null cookie
}

pub fn set_default_dll_directories(_ctx: &mut CompatContext, _flags: u32) -> WinBool {
    TRUE
}

pub fn get_string_type_w(
    _ctx: &CompatContext,
    _info_type: u32,
    src: &[u16],
    char_type: &mut [u16],
) -> WinBool {
    for (i, &ch) in src.iter().enumerate() {
        if i >= char_type.len() {
            break;
        }
        char_type[i] = if ch <= 0x7F { 0x0004 } else { 0x0008 }; // C1_ALPHA / C1_UPPER approx
    }
    TRUE
}

pub fn lc_map_string_w(
    ctx: &mut CompatContext,
    _locale: u32,
    flags: u32,
    src: &[u16],
    dest: Option<&mut [u16]>,
) -> i32 {
    const LCMAP_LOWERCASE: u32 = 0x0100;
    const LCMAP_UPPERCASE: u32 = 0x0200;
    match dest {
        None => src.len() as i32,
        Some(buf) => {
            let copy_len = core::cmp::min(src.len(), buf.len());
            for i in 0..copy_len {
                let ch = src[i];
                buf[i] = if flags & LCMAP_LOWERCASE != 0 && ch >= b'A' as u16 && ch <= b'Z' as u16 {
                    ch + 32
                } else if flags & LCMAP_UPPERCASE != 0 && ch >= b'a' as u16 && ch <= b'z' as u16 {
                    ch - 32
                } else {
                    ch
                };
            }
            set_last_error(ctx, ERROR_SUCCESS);
            copy_len as i32
        }
    }
}
