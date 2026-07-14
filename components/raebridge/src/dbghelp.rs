//! dbghelp.dll — Debug Help Library: symbol resolution, stack walking, minidump
//! creation/reading, type information, image helpers, and exception handling
//! for RaeBridge.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    WinHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_HANDLE, ERROR_INVALID_PARAMETER,
    ERROR_SUCCESS, INVALID_HANDLE_VALUE, NULL_HANDLE,
};

// =========================================================================
// SYMOPT — Symbol options
// =========================================================================

pub const SYMOPT_CASE_INSENSITIVE: u32 = 0x00000001;
pub const SYMOPT_UNDNAME: u32 = 0x00000002;
pub const SYMOPT_DEFERRED_LOADS: u32 = 0x00000004;
pub const SYMOPT_LOAD_LINES: u32 = 0x00000010;
pub const SYMOPT_FAIL_CRITICAL_ERRORS: u32 = 0x00000200;
pub const SYMOPT_EXACT_SYMBOLS: u32 = 0x00000400;
pub const SYMOPT_ALLOW_ABSOLUTE_SYMBOLS: u32 = 0x00000800;
pub const SYMOPT_IGNORE_NT_SYMPATH: u32 = 0x00001000;
pub const SYMOPT_INCLUDE_32BIT_MODULES: u32 = 0x00002000;
pub const SYMOPT_PUBLICS_ONLY: u32 = 0x00004000;
pub const SYMOPT_NO_PUBLICS: u32 = 0x00008000;
pub const SYMOPT_AUTO_PUBLICS: u32 = 0x00010000;
pub const SYMOPT_NO_IMAGE_SEARCH: u32 = 0x00020000;
pub const SYMOPT_DEBUG: u32 = 0x80000000;

pub const SYMOPT_DEFAULT: u32 = SYMOPT_UNDNAME | SYMOPT_DEFERRED_LOADS | SYMOPT_CASE_INSENSITIVE;

// =========================================================================
// Symbol tag enumeration
// =========================================================================

pub const SYM_TAG_NULL: u32 = 0;
pub const SYM_TAG_FUNCTION: u32 = 5;
pub const SYM_TAG_DATA: u32 = 7;
pub const SYM_TAG_PUBLIC_SYMBOL: u32 = 10;
pub const SYM_TAG_UDT: u32 = 11;
pub const SYM_TAG_ENUM: u32 = 12;
pub const SYM_TAG_FUNCTION_TYPE: u32 = 13;
pub const SYM_TAG_POINTER_TYPE: u32 = 14;
pub const SYM_TAG_ARRAY_TYPE: u32 = 15;
pub const SYM_TAG_BASE_TYPE: u32 = 16;
pub const SYM_TAG_TYPEDEF: u32 = 17;
pub const SYM_TAG_BASE_CLASS: u32 = 18;

// =========================================================================
// Symbol flags
// =========================================================================

pub const SYMFLAG_VALUEPRESENT: u32 = 0x00000001;
pub const SYMFLAG_REGISTER: u32 = 0x00000008;
pub const SYMFLAG_REGREL: u32 = 0x00000010;
pub const SYMFLAG_FRAMEREL: u32 = 0x00000020;
pub const SYMFLAG_PARAMETER: u32 = 0x00000040;
pub const SYMFLAG_LOCAL: u32 = 0x00000080;
pub const SYMFLAG_CONSTANT: u32 = 0x00000100;
pub const SYMFLAG_EXPORT: u32 = 0x00000200;
pub const SYMFLAG_FUNCTION: u32 = 0x00000800;
pub const SYMFLAG_VIRTUAL: u32 = 0x00001000;
pub const SYMFLAG_THUNK: u32 = 0x00002000;
pub const SYMFLAG_TLSREL: u32 = 0x00004000;
pub const SYMFLAG_PUBLIC_CODE: u32 = 0x00400000;

// =========================================================================
// MINIDUMP_TYPE flags
// =========================================================================

pub const MINIDUMP_NORMAL: u32 = 0x00000000;
pub const MINIDUMP_WITH_DATA_SEGS: u32 = 0x00000001;
pub const MINIDUMP_WITH_FULL_MEMORY: u32 = 0x00000002;
pub const MINIDUMP_WITH_HANDLE_DATA: u32 = 0x00000004;
pub const MINIDUMP_FILTER_MEMORY: u32 = 0x00000008;
pub const MINIDUMP_SCAN_MEMORY: u32 = 0x00000010;
pub const MINIDUMP_WITH_UNLOADED_MODULES: u32 = 0x00000020;
pub const MINIDUMP_WITH_INDIRECTLY_REFERENCED_MEMORY: u32 = 0x00000040;
pub const MINIDUMP_FILTER_MODULE_PATHS: u32 = 0x00000080;
pub const MINIDUMP_WITH_PROCESS_THREAD_DATA: u32 = 0x00000100;
pub const MINIDUMP_WITH_PRIVATE_READ_WRITE_MEMORY: u32 = 0x00000200;
pub const MINIDUMP_WITHOUT_OPTIONAL_DATA: u32 = 0x00000400;
pub const MINIDUMP_WITH_FULL_MEMORY_INFO: u32 = 0x00000800;
pub const MINIDUMP_WITH_THREAD_INFO: u32 = 0x00001000;
pub const MINIDUMP_WITH_CODE_SEGS: u32 = 0x00002000;
pub const MINIDUMP_WITHOUT_AUXILIARY_STATE: u32 = 0x00004000;
pub const MINIDUMP_WITH_FULL_AUXILIARY_STATE: u32 = 0x00008000;
pub const MINIDUMP_WITH_PRIVATE_WRITE_COPY_MEMORY: u32 = 0x00010000;
pub const MINIDUMP_IGNORE_INACCESSIBLE_MEMORY: u32 = 0x00020000;
pub const MINIDUMP_WITH_TOKEN_INFORMATION: u32 = 0x00040000;
pub const MINIDUMP_FILTER_TRIAGE: u32 = 0x00100000;

// =========================================================================
// MINIDUMP_STREAM_TYPE values
// =========================================================================

pub const UNUSED_STREAM: u32 = 0;
pub const RESERVED_STREAM_0: u32 = 1;
pub const RESERVED_STREAM_1: u32 = 2;
pub const THREAD_LIST_STREAM: u32 = 3;
pub const MODULE_LIST_STREAM: u32 = 4;
pub const MEMORY_LIST_STREAM: u32 = 5;
pub const EXCEPTION_STREAM: u32 = 6;
pub const SYSTEM_INFO_STREAM: u32 = 7;
pub const THREAD_EX_LIST_STREAM: u32 = 8;
pub const MEMORY64_LIST_STREAM: u32 = 9;
pub const COMMENT_STREAM_A: u32 = 10;
pub const COMMENT_STREAM_W: u32 = 11;
pub const HANDLE_DATA_STREAM: u32 = 12;
pub const FUNCTION_TABLE_STREAM: u32 = 13;
pub const UNLOADED_MODULE_LIST_STREAM: u32 = 14;
pub const MISC_INFO_STREAM: u32 = 15;
pub const MEMORY_INFO_LIST_STREAM: u32 = 16;
pub const THREAD_INFO_LIST_STREAM: u32 = 17;
pub const HANDLE_OPERATION_LIST_STREAM: u32 = 18;
pub const TOKEN_STREAM: u32 = 19;

// =========================================================================
// Exception codes
// =========================================================================

pub const EXCEPTION_ACCESS_VIOLATION: u32 = 0xC0000005;
pub const EXCEPTION_BREAKPOINT: u32 = 0x80000003;
pub const EXCEPTION_SINGLE_STEP: u32 = 0x80000004;
pub const EXCEPTION_ARRAY_BOUNDS_EXCEEDED: u32 = 0xC000008C;
pub const EXCEPTION_DATATYPE_MISALIGNMENT: u32 = 0x80000002;
pub const EXCEPTION_FLT_DIVIDE_BY_ZERO: u32 = 0xC000008E;
pub const EXCEPTION_INT_DIVIDE_BY_ZERO: u32 = 0xC0000094;
pub const EXCEPTION_ILLEGAL_INSTRUCTION: u32 = 0xC000001D;
pub const EXCEPTION_STACK_OVERFLOW: u32 = 0xC00000FD;
pub const EXCEPTION_GUARD_PAGE: u32 = 0x80000001;
pub const EXCEPTION_NONCONTINUABLE_EXCEPTION: u32 = 0xC0000025;

// =========================================================================
// Data structures
// =========================================================================

#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub size_of_struct: u32,
    pub type_index: u32,
    pub size: u64,
    pub mod_base: u64,
    pub flags: u32,
    pub value: u64,
    pub address: u64,
    pub register: u32,
    pub scope: u32,
    pub tag: u32,
    pub name_len: u32,
    pub max_name_len: u32,
    pub name: String,
}

impl SymbolInfo {
    pub fn new() -> Self {
        Self {
            size_of_struct: 88,
            type_index: 0,
            size: 0,
            mod_base: 0,
            flags: 0,
            value: 0,
            address: 0,
            register: 0,
            scope: 0,
            tag: SYM_TAG_NULL,
            name_len: 0,
            max_name_len: 256,
            name: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImagehlpLine64 {
    pub size_of_struct: u32,
    pub key: u64,
    pub line_number: u32,
    pub file_name: String,
    pub address: u64,
}

impl ImagehlpLine64 {
    pub fn new() -> Self {
        Self {
            size_of_struct: 32,
            key: 0,
            line_number: 0,
            file_name: String::new(),
            address: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Address64 {
    pub offset: u64,
    pub segment: u16,
    pub mode: u32,
}

impl Address64 {
    pub fn new() -> Self {
        Self {
            offset: 0,
            segment: 0,
            mode: 0,
        }
    }

    pub fn from_offset(offset: u64) -> Self {
        Self {
            offset,
            segment: 0,
            mode: 3,
        } // AddrModeFlat
    }
}

#[derive(Debug, Clone)]
pub struct StackFrame64 {
    pub addr_pc: Address64,
    pub addr_return: Address64,
    pub addr_frame: Address64,
    pub addr_stack: Address64,
    pub addr_b_store: Address64,
    pub func_table_entry: u64,
    pub params: [u64; 4],
    pub far: bool,
    pub virtual_frame: bool,
}

impl StackFrame64 {
    pub fn new() -> Self {
        Self {
            addr_pc: Address64::new(),
            addr_return: Address64::new(),
            addr_frame: Address64::new(),
            addr_stack: Address64::new(),
            addr_b_store: Address64::new(),
            func_table_entry: 0,
            params: [0u64; 4],
            far: false,
            virtual_frame: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExceptionRecord {
    pub exception_code: u32,
    pub exception_flags: u32,
    pub exception_record: u64,
    pub exception_address: u64,
    pub number_parameters: u32,
    pub exception_information: [u64; 15],
}

impl ExceptionRecord {
    pub fn new() -> Self {
        Self {
            exception_code: 0,
            exception_flags: 0,
            exception_record: 0,
            exception_address: 0,
            number_parameters: 0,
            exception_information: [0u64; 15],
        }
    }
}

#[derive(Debug, Clone)]
pub struct MinidumpExceptionInfo {
    pub thread_id: u32,
    pub exception_pointers: u64,
    pub client_pointers: bool,
}

#[derive(Debug, Clone)]
pub struct ImageNtHeaders {
    pub signature: u32,
    pub machine: u16,
    pub number_of_sections: u16,
    pub time_date_stamp: u32,
    pub characteristics: u16,
    pub image_base: u64,
    pub size_of_image: u32,
    pub entry_point_rva: u32,
    pub checksum: u32,
    pub subsystem: u16,
    pub dll_characteristics: u16,
}

#[derive(Debug, Clone)]
pub struct LoadedImage {
    pub module_name: String,
    pub file_handle: WinHandle,
    pub mapped_address: u64,
    pub size_of_image: u32,
    pub number_of_sections: u16,
    pub read_only: bool,
    pub system_image: bool,
}

// =========================================================================
// Internal state
// =========================================================================

#[derive(Debug, Clone)]
struct SymbolEntry {
    name: String,
    address: u64,
    size: u64,
    mod_base: u64,
    tag: u32,
    flags: u32,
    file_name: Option<String>,
    line_number: Option<u32>,
}

#[derive(Debug, Clone)]
struct LoadedModule {
    name: String,
    base: u64,
    size: u32,
    symbols: Vec<SymbolEntry>,
    source_files: Vec<String>,
}

struct DebugSession {
    process: WinHandle,
    search_path: String,
    modules: BTreeMap<u64, LoadedModule>,
}

struct MinidumpData {
    handle: WinHandle,
    dump_type: u32,
    process_id: u32,
    thread_count: u32,
    module_count: u32,
    streams: Vec<(u32, Vec<u8>)>,
}

// =========================================================================
// Global state
// =========================================================================

pub struct DebugHelp {
    options: u32,
    sessions: BTreeMap<u64, DebugSession>,
    minidumps: BTreeMap<u64, MinidumpData>,
    next_handle: u64,
    exception_filter: Option<u64>,
    vectored_handlers: Vec<u64>,
}

impl DebugHelp {
    const fn new() -> Self {
        Self {
            options: SYMOPT_DEFAULT,
            sessions: BTreeMap::new(),
            minidumps: BTreeMap::new(),
            next_handle: 0xDB00_0000,
            exception_filter: None,
            vectored_handlers: Vec::new(),
        }
    }

    fn alloc_handle(&mut self) -> WinHandle {
        let h = self.next_handle;
        self.next_handle += 1;
        WinHandle(h)
    }
}

static mut DEBUG_HELP: Option<DebugHelp> = None;

pub fn init() {
    unsafe {
        DEBUG_HELP = Some(DebugHelp::new());
    }
}

fn dbg() -> &'static mut DebugHelp {
    unsafe {
        DEBUG_HELP
            .as_mut()
            .expect("dbghelp not initialized — call init()")
    }
}

// =========================================================================
// Symbol options
// =========================================================================

pub fn sym_set_options(options: u32) -> u32 {
    let dh = dbg();
    let old = dh.options;
    dh.options = options;
    old
}

pub fn sym_get_options() -> u32 {
    dbg().options
}

// =========================================================================
// Symbol initialization
// =========================================================================

pub fn sym_initialize_w(
    process: WinHandle,
    search_path: Option<&str>,
    _invade_process: bool,
) -> bool {
    let dh = dbg();
    let sp = match search_path {
        Some(p) => String::from(p),
        None => String::from("SRV*C:\\Symbols*https://msdl.microsoft.com/download/symbols"),
    };
    dh.sessions.insert(
        process.0,
        DebugSession {
            process,
            search_path: sp,
            modules: BTreeMap::new(),
        },
    );
    true
}

pub fn sym_cleanup(process: WinHandle) -> bool {
    dbg().sessions.remove(&process.0).is_some()
}

// =========================================================================
// Module loading
// =========================================================================

pub fn sym_load_module_ex_w(
    process: WinHandle,
    _file: WinHandle,
    image_name: &str,
    _module_name: Option<&str>,
    base_of_dll: u64,
    dll_size: u32,
    _data: u64,
    _flags: u32,
) -> u64 {
    let dh = dbg();
    let session = match dh.sessions.get_mut(&process.0) {
        Some(s) => s,
        None => return 0,
    };
    let mut symbols = Vec::new();
    symbols.push(SymbolEntry {
        name: String::from("DllMain"),
        address: base_of_dll + 0x1000,
        size: 256,
        mod_base: base_of_dll,
        tag: SYM_TAG_FUNCTION,
        flags: SYMFLAG_FUNCTION,
        file_name: Some({
            let mut f = String::from(image_name);
            f.push_str(".c");
            f
        }),
        line_number: Some(1),
    });
    symbols.push(SymbolEntry {
        name: String::from("_init"),
        address: base_of_dll + 0x1100,
        size: 64,
        mod_base: base_of_dll,
        tag: SYM_TAG_FUNCTION,
        flags: SYMFLAG_FUNCTION | SYMFLAG_EXPORT,
        file_name: None,
        line_number: None,
    });
    symbols.push(SymbolEntry {
        name: String::from("g_GlobalData"),
        address: base_of_dll + 0x5000,
        size: 1024,
        mod_base: base_of_dll,
        tag: SYM_TAG_DATA,
        flags: SYMFLAG_EXPORT,
        file_name: None,
        line_number: None,
    });

    let mut sources = Vec::new();
    sources.push({
        let mut s = String::from(image_name);
        s.push_str(".c");
        s
    });

    session.modules.insert(
        base_of_dll,
        LoadedModule {
            name: String::from(image_name),
            base: base_of_dll,
            size: dll_size,
            symbols,
            source_files: sources,
        },
    );

    base_of_dll
}

pub fn sym_unload_module64(process: WinHandle, base_of_dll: u64) -> bool {
    let dh = dbg();
    let session = match dh.sessions.get_mut(&process.0) {
        Some(s) => s,
        None => return false,
    };
    session.modules.remove(&base_of_dll).is_some()
}

pub fn sym_refresh_module_list(process: WinHandle) -> bool {
    dbg().sessions.contains_key(&process.0)
}

// =========================================================================
// Symbol lookup
// =========================================================================

pub fn sym_from_addr(
    process: WinHandle,
    address: u64,
    displacement: &mut u64,
    info: &mut SymbolInfo,
) -> bool {
    let dh = dbg();
    let session = match dh.sessions.get(&process.0) {
        Some(s) => s,
        None => return false,
    };
    for module in session.modules.values() {
        for sym in &module.symbols {
            if address >= sym.address && address < sym.address + sym.size {
                *displacement = address - sym.address;
                fill_symbol_info(info, sym);
                return true;
            }
        }
    }
    false
}

pub fn sym_from_name(process: WinHandle, name: &str, info: &mut SymbolInfo) -> bool {
    let dh = dbg();
    let session = match dh.sessions.get(&process.0) {
        Some(s) => s,
        None => return false,
    };
    let case_insensitive = dh.options & SYMOPT_CASE_INSENSITIVE != 0;
    for module in session.modules.values() {
        for sym in &module.symbols {
            let matches = if case_insensitive {
                sym.name.eq_ignore_ascii_case(name)
            } else {
                sym.name == name
            };
            if matches {
                fill_symbol_info(info, sym);
                return true;
            }
        }
    }
    false
}

pub fn sym_enum_symbols_w(
    process: WinHandle,
    base_of_dll: u64,
    mask: Option<&str>,
    results: &mut Vec<SymbolInfo>,
) -> bool {
    let dh = dbg();
    let session = match dh.sessions.get(&process.0) {
        Some(s) => s,
        None => return false,
    };
    let module = match session.modules.get(&base_of_dll) {
        Some(m) => m,
        None => return false,
    };
    for sym in &module.symbols {
        let matches = match mask {
            Some(m) => {
                if m == "*" {
                    true
                } else {
                    sym.name.contains(m.trim_matches('*'))
                }
            }
            None => true,
        };
        if matches {
            let mut info = SymbolInfo::new();
            fill_symbol_info(&mut info, sym);
            results.push(info);
        }
    }
    true
}

pub fn sym_enum_symbols_ex_w(
    process: WinHandle,
    base_of_dll: u64,
    mask: Option<&str>,
    _options: u32,
    results: &mut Vec<SymbolInfo>,
) -> bool {
    sym_enum_symbols_w(process, base_of_dll, mask, results)
}

pub fn sym_get_sym_from_addr64(
    process: WinHandle,
    address: u64,
    displacement: &mut u64,
    info: &mut SymbolInfo,
) -> bool {
    sym_from_addr(process, address, displacement, info)
}

fn fill_symbol_info(info: &mut SymbolInfo, entry: &SymbolEntry) {
    info.name = entry.name.clone();
    info.name_len = entry.name.len() as u32;
    info.address = entry.address;
    info.size = entry.size;
    info.mod_base = entry.mod_base;
    info.tag = entry.tag;
    info.flags = entry.flags;
}

// =========================================================================
// Source line resolution
// =========================================================================

pub fn sym_get_line_from_addr64(
    process: WinHandle,
    address: u64,
    displacement: &mut u32,
    line: &mut ImagehlpLine64,
) -> bool {
    let dh = dbg();
    let session = match dh.sessions.get(&process.0) {
        Some(s) => s,
        None => return false,
    };
    for module in session.modules.values() {
        for sym in &module.symbols {
            if address >= sym.address && address < sym.address + sym.size {
                *displacement = (address - sym.address) as u32;
                line.address = sym.address;
                line.line_number = sym.line_number.unwrap_or(0);
                line.file_name = sym.file_name.clone().unwrap_or_default();
                return true;
            }
        }
    }
    false
}

pub fn sym_get_line_from_name64(
    process: WinHandle,
    _module_name: Option<&str>,
    file_name: &str,
    line_number: u32,
    displacement: &mut i64,
    line: &mut ImagehlpLine64,
) -> bool {
    let dh = dbg();
    let session = match dh.sessions.get(&process.0) {
        Some(s) => s,
        None => return false,
    };
    for module in session.modules.values() {
        for sym in &module.symbols {
            if let Some(ref fname) = sym.file_name {
                if fname == file_name {
                    *displacement = 0;
                    line.address = sym.address;
                    line.line_number = line_number;
                    line.file_name = fname.clone();
                    return true;
                }
            }
        }
    }
    false
}

pub fn sym_enum_lines(
    process: WinHandle,
    base: u64,
    _obj: Option<&str>,
    file: Option<&str>,
    results: &mut Vec<ImagehlpLine64>,
) -> bool {
    let dh = dbg();
    let session = match dh.sessions.get(&process.0) {
        Some(s) => s,
        None => return false,
    };
    let module = match session.modules.get(&base) {
        Some(m) => m,
        None => return false,
    };
    for sym in &module.symbols {
        let include = match (file, &sym.file_name) {
            (Some(f), Some(sf)) => sf == f,
            (None, Some(_)) => true,
            _ => false,
        };
        if include {
            results.push(ImagehlpLine64 {
                size_of_struct: 32,
                key: 0,
                line_number: sym.line_number.unwrap_or(0),
                file_name: sym.file_name.clone().unwrap_or_default(),
                address: sym.address,
            });
        }
    }
    true
}

pub fn sym_enum_source_files_w(
    process: WinHandle,
    base: u64,
    _mask: Option<&str>,
    results: &mut Vec<String>,
) -> bool {
    let dh = dbg();
    let session = match dh.sessions.get(&process.0) {
        Some(s) => s,
        None => return false,
    };
    let module = match session.modules.get(&base) {
        Some(m) => m,
        None => return false,
    };
    for f in &module.source_files {
        results.push(f.clone());
    }
    true
}

// =========================================================================
// Stack walking
// =========================================================================

pub fn stack_walk64(
    _machine_type: u32,
    process: WinHandle,
    _thread: WinHandle,
    frame: &mut StackFrame64,
    _context: u64,
    _read_memory: Option<u64>,
    _function_table: Option<u64>,
    _get_module_base: Option<u64>,
    _translate_address: Option<u64>,
) -> bool {
    if frame.addr_pc.offset == 0 {
        return false;
    }
    let dh = dbg();
    let _session = match dh.sessions.get(&process.0) {
        Some(s) => s,
        None => return false,
    };
    let ret = frame.addr_return.offset;
    if ret == 0 {
        return false;
    }
    frame.addr_pc = frame.addr_return;
    frame.addr_return = Address64::from_offset(0);
    frame.addr_frame.offset = frame.addr_frame.offset.wrapping_add(0x30);
    true
}

pub fn stack_walk_ex(
    machine_type: u32,
    process: WinHandle,
    thread: WinHandle,
    frame: &mut StackFrame64,
    context: u64,
    read_memory: Option<u64>,
    function_table: Option<u64>,
    get_module_base: Option<u64>,
    translate_address: Option<u64>,
    _flags: u32,
) -> bool {
    stack_walk64(
        machine_type,
        process,
        thread,
        frame,
        context,
        read_memory,
        function_table,
        get_module_base,
        translate_address,
    )
}

// =========================================================================
// Minidump creation
// =========================================================================

pub fn mini_dump_write_dump(
    process: WinHandle,
    process_id: u32,
    file: WinHandle,
    dump_type: u32,
    _exception_param: Option<&MinidumpExceptionInfo>,
    _user_stream: u64,
    _callback: u64,
) -> bool {
    if file.0 == 0 || file == INVALID_HANDLE_VALUE {
        return false;
    }
    let dh = dbg();
    let handle = dh.alloc_handle();
    let mut streams = Vec::new();
    streams.push((SYSTEM_INFO_STREAM, Vec::new()));
    streams.push((THREAD_LIST_STREAM, Vec::new()));
    streams.push((MODULE_LIST_STREAM, Vec::new()));
    if dump_type & MINIDUMP_WITH_FULL_MEMORY != 0 {
        streams.push((MEMORY64_LIST_STREAM, Vec::new()));
    }
    if dump_type & MINIDUMP_WITH_HANDLE_DATA != 0 {
        streams.push((HANDLE_DATA_STREAM, Vec::new()));
    }
    if dump_type & MINIDUMP_WITH_THREAD_INFO != 0 {
        streams.push((THREAD_INFO_LIST_STREAM, Vec::new()));
    }
    if dump_type & MINIDUMP_WITH_UNLOADED_MODULES != 0 {
        streams.push((UNLOADED_MODULE_LIST_STREAM, Vec::new()));
    }
    if dump_type & MINIDUMP_WITH_FULL_MEMORY_INFO != 0 {
        streams.push((MEMORY_INFO_LIST_STREAM, Vec::new()));
    }
    if dump_type & MINIDUMP_WITH_TOKEN_INFORMATION != 0 {
        streams.push((TOKEN_STREAM, Vec::new()));
    }

    dh.minidumps.insert(
        handle.0,
        MinidumpData {
            handle,
            dump_type,
            process_id,
            thread_count: 4,
            module_count: 3,
            streams,
        },
    );
    true
}

// =========================================================================
// Minidump reading
// =========================================================================

pub fn mini_dump_read_dump_stream(
    _base: u64,
    stream_type: u32,
    dir_out: &mut u64,
    stream_ptr: &mut u64,
    stream_size: &mut u32,
) -> bool {
    let dh = dbg();
    for dump in dh.minidumps.values() {
        for (st, data) in &dump.streams {
            if *st == stream_type {
                *dir_out = 0;
                *stream_ptr = 0;
                *stream_size = data.len() as u32;
                return true;
            }
        }
    }
    false
}

// =========================================================================
// Type information
// =========================================================================

pub fn sym_get_type_info(
    process: WinHandle,
    mod_base: u64,
    _type_id: u32,
    _get_type: u32,
    info: &mut u64,
) -> bool {
    let dh = dbg();
    let session = match dh.sessions.get(&process.0) {
        Some(s) => s,
        None => return false,
    };
    if !session.modules.contains_key(&mod_base) {
        return false;
    }
    *info = 0;
    true
}

pub fn sym_enum_types(process: WinHandle, mod_base: u64, results: &mut Vec<SymbolInfo>) -> bool {
    let dh = dbg();
    let session = match dh.sessions.get(&process.0) {
        Some(s) => s,
        None => return false,
    };
    let module = match session.modules.get(&mod_base) {
        Some(m) => m,
        None => return false,
    };
    for sym in &module.symbols {
        if sym.tag == SYM_TAG_UDT || sym.tag == SYM_TAG_ENUM || sym.tag == SYM_TAG_TYPEDEF {
            let mut info = SymbolInfo::new();
            fill_symbol_info(&mut info, sym);
            results.push(info);
        }
    }
    true
}

// =========================================================================
// Undecorated names
// =========================================================================

pub fn un_decorate_symbol_name_w(
    name: &str,
    output: &mut String,
    _max_len: u32,
    _flags: u32,
) -> u32 {
    let undecorated = if name.starts_with('?') {
        let end = name.find("@@").unwrap_or(name.len());
        String::from(&name[1..end])
    } else if name.starts_with('_') {
        let at_pos = name.find('@').unwrap_or(name.len());
        String::from(&name[1..at_pos])
    } else {
        String::from(name)
    };
    let len = undecorated.len() as u32;
    *output = undecorated;
    len
}

// =========================================================================
// Image functions
// =========================================================================

pub fn image_nt_header(base: u64, _size: u32) -> Option<ImageNtHeaders> {
    if base == 0 {
        return None;
    }
    Some(ImageNtHeaders {
        signature: 0x00004550, // PE\0\0
        machine: 0x8664,
        number_of_sections: 4,
        time_date_stamp: 0x6000_0000,
        characteristics: 0x0022,
        image_base: base,
        size_of_image: 0x0010_0000,
        entry_point_rva: 0x1000,
        checksum: 0,
        subsystem: 3, // IMAGE_SUBSYSTEM_WINDOWS_CUI
        dll_characteristics: 0x8160,
    })
}

pub fn image_directory_entry_to_data(
    _base: u64,
    _mapped_as_image: bool,
    _directory_entry: u16,
    size: &mut u32,
) -> u64 {
    *size = 0;
    0
}

pub fn map_and_load(
    image_name: &str,
    _dll_path: Option<&str>,
    image: &mut LoadedImage,
    _dot_dll: bool,
    _read_only: bool,
) -> bool {
    if image_name.is_empty() {
        return false;
    }
    let handle = dbg().alloc_handle();
    image.module_name = String::from(image_name);
    image.file_handle = handle;
    image.mapped_address = 0x1000_0000;
    image.size_of_image = 0x0010_0000;
    image.number_of_sections = 4;
    image.read_only = true;
    image.system_image = false;
    true
}

pub fn un_map_and_load(image: &mut LoadedImage) -> bool {
    image.mapped_address = 0;
    image.size_of_image = 0;
    true
}

// =========================================================================
// Exception handling
// =========================================================================

pub fn set_unhandled_exception_filter(filter: u64) -> u64 {
    let dh = dbg();
    let prev = dh.exception_filter.unwrap_or(0);
    dh.exception_filter = Some(filter);
    prev
}

pub fn add_vectored_exception_handler(first: bool, handler: u64) -> u64 {
    let dh = dbg();
    if first {
        dh.vectored_handlers.insert(0, handler);
    } else {
        dh.vectored_handlers.push(handler);
    }
    handler
}

pub fn remove_vectored_exception_handler(handler: u64) -> bool {
    let dh = dbg();
    if let Some(pos) = dh.vectored_handlers.iter().position(|&h| h == handler) {
        dh.vectored_handlers.remove(pos);
        true
    } else {
        false
    }
}
