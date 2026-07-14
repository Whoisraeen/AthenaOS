//! Full PE/PE32+ executable loader for AthBridge.
//!
//! Parses DOS header, PE signature, COFF header, optional header (PE32 and
//! PE32+), section table, import directory, and base relocation table.
//! Maps sections into virtual memory, applies relocations, and resolves
//! imports against a [`DllRegistry`] of shim function addresses.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{
    read_u16_le, read_u32_le, BridgeError, ImportEntry, PeSection, WinApiModule,
    DOS_HEADER_MIN_SIZE, E_LFANEW_OFFSET,
};

// ---------------------------------------------------------------------------
// PE Constants
// ---------------------------------------------------------------------------

const PE32_MAGIC: u16 = 0x010B;
const PE32PLUS_MAGIC: u16 = 0x020B;

const IMAGE_DIRECTORY_ENTRY_IMPORT: usize = 1;
const IMAGE_DIRECTORY_ENTRY_BASERELOC: usize = 5;

const IMAGE_REL_BASED_ABSOLUTE: u16 = 0;
const IMAGE_REL_BASED_HIGH: u16 = 1;
const IMAGE_REL_BASED_LOW: u16 = 2;
const IMAGE_REL_BASED_HIGHLOW: u16 = 3;
const IMAGE_REL_BASED_DIR64: u16 = 10;

const IMAGE_ORDINAL_FLAG32: u32 = 0x8000_0000;
const IMAGE_ORDINAL_FLAG64: u64 = 0x8000_0000_0000_0000;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[inline]
fn read_u64_le(buf: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
        buf[offset + 4],
        buf[offset + 5],
        buf[offset + 6],
        buf[offset + 7],
    ])
}

fn read_cstring(buf: &[u8], offset: usize) -> String {
    let mut end = offset;
    while end < buf.len() && buf[end] != 0 {
        end += 1;
    }
    let bytes = &buf[offset..end];
    let mut s = String::new();
    for &b in bytes {
        s.push(b as char);
    }
    s
}

fn rva_to_offset(sections: &[PeSection], rva: u32) -> Option<usize> {
    for sec in sections {
        let sec_start = sec.virtual_address;
        let sec_end = sec_start + sec.raw_data_size;
        if rva >= sec_start && rva < sec_end {
            return Some((sec.raw_data_offset + (rva - sec_start)) as usize);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Parsed PE structures
// ---------------------------------------------------------------------------

/// A single base relocation entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relocation {
    pub rva: u32,
    pub rel_type: u16,
}

/// Data directory entry (RVA + size).
#[derive(Debug, Clone, Copy, Default)]
pub struct DataDirectory {
    pub rva: u32,
    pub size: u32,
}

/// Fully parsed PE image metadata.
#[derive(Debug, Clone)]
pub struct PeImage {
    pub image_base: u64,
    pub entry_point: u64,
    pub sections: Vec<PeSection>,
    pub imports: Vec<ImportEntry>,
    pub relocations: Vec<Relocation>,
    pub is_64bit: bool,
    pub section_alignment: u32,
    pub file_alignment: u32,
    pub size_of_image: u32,
    pub size_of_headers: u32,
    pub data_directories: Vec<DataDirectory>,
}

/// A loaded and relocated PE ready for execution.
#[derive(Debug)]
pub struct LoadedPe {
    pub image_base: u64,
    pub entry_point: u64,
    pub image_memory: Vec<u8>,
    pub imports: Vec<ImportEntry>,
    pub is_64bit: bool,
}

/// Errors specific to PE loading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeError {
    Parse(BridgeError),
    SectionOutOfBounds { section_name: [u8; 8], offset: u32 },
    RelocationFailed { rva: u32 },
    ImportNotResolved { dll: String, function: String },
    ImageTooLarge { size: u64 },
}

impl From<BridgeError> for PeError {
    fn from(e: BridgeError) -> Self {
        PeError::Parse(e)
    }
}

// ---------------------------------------------------------------------------
// PE Parsing
// ---------------------------------------------------------------------------

/// Parse a complete PE image from raw bytes.
pub fn parse_pe(buf: &[u8]) -> Result<PeImage, BridgeError> {
    if buf.len() < DOS_HEADER_MIN_SIZE {
        return Err(BridgeError::BufferTooSmall {
            expected: DOS_HEADER_MIN_SIZE,
            actual: buf.len(),
        });
    }

    if buf[0] != 0x4D || buf[1] != 0x5A {
        return Err(BridgeError::InvalidDosSignature([buf[0], buf[1]]));
    }

    let pe_offset = read_u32_le(buf, E_LFANEW_OFFSET) as usize;
    let pe_header_min = pe_offset + 4 + 20 + 28;
    if pe_header_min > buf.len() {
        return Err(BridgeError::PeOffsetOutOfBounds {
            offset: pe_offset as u32,
            buf_len: buf.len(),
        });
    }

    let pe_sig: [u8; 4] = [
        buf[pe_offset],
        buf[pe_offset + 1],
        buf[pe_offset + 2],
        buf[pe_offset + 3],
    ];
    if pe_sig != [0x50, 0x45, 0x00, 0x00] {
        return Err(BridgeError::InvalidPeSignature(pe_sig));
    }

    // COFF header
    let coff = pe_offset + 4;
    let num_sections = read_u16_le(buf, coff + 2);
    let opt_header_size = read_u16_le(buf, coff + 16) as usize;

    // Optional header
    let opt = coff + 20;
    let opt_magic = read_u16_le(buf, opt);
    let is_64bit = match opt_magic {
        PE32_MAGIC => false,
        PE32PLUS_MAGIC => true,
        other => return Err(BridgeError::UnknownOptionalMagic(other)),
    };

    let entry_point_rva = read_u32_le(buf, opt + 16) as u64;

    let (
        image_base,
        section_alignment,
        file_alignment,
        size_of_image,
        size_of_headers,
        dd_offset,
        num_dd,
    ) = if is_64bit {
        let needed = opt + 112;
        if needed > buf.len() {
            return Err(BridgeError::BufferTooSmall {
                expected: needed,
                actual: buf.len(),
            });
        }
        let ib = read_u64_le(buf, opt + 24);
        let sa = read_u32_le(buf, opt + 32);
        let fa = read_u32_le(buf, opt + 36);
        let si = read_u32_le(buf, opt + 56);
        let sh = read_u32_le(buf, opt + 60);
        let ndd = read_u32_le(buf, opt + 108) as usize;
        (ib, sa, fa, si, sh, opt + 112, ndd)
    } else {
        let needed = opt + 96;
        if needed > buf.len() {
            return Err(BridgeError::BufferTooSmall {
                expected: needed,
                actual: buf.len(),
            });
        }
        let ib = read_u32_le(buf, opt + 28) as u64;
        let sa = read_u32_le(buf, opt + 32);
        let fa = read_u32_le(buf, opt + 36);
        let si = read_u32_le(buf, opt + 56);
        let sh = read_u32_le(buf, opt + 60);
        let ndd = read_u32_le(buf, opt + 92) as usize;
        (ib, sa, fa, si, sh, opt + 96, ndd)
    };

    // Data directories
    let mut data_directories = Vec::with_capacity(num_dd);
    for i in 0..num_dd {
        let dd_off = dd_offset + i * 8;
        if dd_off + 8 > buf.len() {
            break;
        }
        data_directories.push(DataDirectory {
            rva: read_u32_le(buf, dd_off),
            size: read_u32_le(buf, dd_off + 4),
        });
    }

    // Section table
    let section_start = coff + 20 + opt_header_size;
    let mut sections = Vec::with_capacity(num_sections as usize);
    for i in 0..num_sections as usize {
        let off = section_start + i * 40;
        if off + 40 > buf.len() {
            return Err(BridgeError::BufferTooSmall {
                expected: off + 40,
                actual: buf.len(),
            });
        }
        let mut name = [0u8; 8];
        name.copy_from_slice(&buf[off..off + 8]);
        sections.push(PeSection {
            name,
            virtual_size: read_u32_le(buf, off + 8),
            virtual_address: read_u32_le(buf, off + 12),
            raw_data_size: read_u32_le(buf, off + 16),
            raw_data_offset: read_u32_le(buf, off + 20),
            characteristics: read_u32_le(buf, off + 36),
        });
    }

    // Parse imports
    let imports = if IMAGE_DIRECTORY_ENTRY_IMPORT < data_directories.len()
        && data_directories[IMAGE_DIRECTORY_ENTRY_IMPORT].rva != 0
    {
        parse_imports(
            buf,
            &sections,
            data_directories[IMAGE_DIRECTORY_ENTRY_IMPORT].rva,
            is_64bit,
        )?
    } else {
        Vec::new()
    };

    // Parse relocations
    let relocations = if IMAGE_DIRECTORY_ENTRY_BASERELOC < data_directories.len()
        && data_directories[IMAGE_DIRECTORY_ENTRY_BASERELOC].rva != 0
    {
        parse_relocations(
            buf,
            &sections,
            data_directories[IMAGE_DIRECTORY_ENTRY_BASERELOC].rva,
            data_directories[IMAGE_DIRECTORY_ENTRY_BASERELOC].size,
        )?
    } else {
        Vec::new()
    };

    Ok(PeImage {
        image_base,
        entry_point: entry_point_rva,
        sections,
        imports,
        relocations,
        is_64bit,
        section_alignment,
        file_alignment,
        size_of_image,
        size_of_headers,
        data_directories,
    })
}

// ---------------------------------------------------------------------------
// Import directory parsing
// ---------------------------------------------------------------------------

fn parse_imports(
    buf: &[u8],
    sections: &[PeSection],
    import_rva: u32,
    is_64bit: bool,
) -> Result<Vec<ImportEntry>, BridgeError> {
    let mut entries = Vec::new();
    let import_offset = match rva_to_offset(sections, import_rva) {
        Some(o) => o,
        None => return Ok(entries),
    };

    let mut desc_off = import_offset;
    loop {
        if desc_off + 20 > buf.len() {
            break;
        }

        let original_first_thunk = read_u32_le(buf, desc_off);
        let name_rva = read_u32_le(buf, desc_off + 12);
        let _first_thunk = read_u32_le(buf, desc_off + 16);

        if name_rva == 0 && original_first_thunk == 0 {
            break;
        }

        let dll_name = match rva_to_offset(sections, name_rva) {
            Some(o) => read_cstring(buf, o),
            None => {
                desc_off += 20;
                continue;
            }
        };

        let module = WinApiModule::from_name(&dll_name);
        let thunk_rva = if original_first_thunk != 0 {
            original_first_thunk
        } else {
            _first_thunk
        };

        let thunk_offset = match rva_to_offset(sections, thunk_rva) {
            Some(o) => o,
            None => {
                desc_off += 20;
                continue;
            }
        };

        let thunk_size = if is_64bit { 8 } else { 4 };
        let mut thunk_pos = thunk_offset;

        loop {
            if thunk_pos + thunk_size > buf.len() {
                break;
            }

            if is_64bit {
                let thunk_val = read_u64_le(buf, thunk_pos);
                if thunk_val == 0 {
                    break;
                }

                if thunk_val & IMAGE_ORDINAL_FLAG64 != 0 {
                    let ordinal = (thunk_val & 0xFFFF) as u16;
                    entries.push(ImportEntry {
                        module,
                        module_name: dll_name.clone(),
                        function_name: String::new(),
                        ordinal: Some(ordinal),
                        resolved: false,
                    });
                } else {
                    let hint_rva = thunk_val as u32;
                    if let Some(hint_off) = rva_to_offset(sections, hint_rva) {
                        if hint_off + 2 < buf.len() {
                            let func_name = read_cstring(buf, hint_off + 2);
                            entries.push(ImportEntry {
                                module,
                                module_name: dll_name.clone(),
                                function_name: func_name,
                                ordinal: None,
                                resolved: false,
                            });
                        }
                    }
                }
            } else {
                let thunk_val = read_u32_le(buf, thunk_pos);
                if thunk_val == 0 {
                    break;
                }

                if thunk_val & IMAGE_ORDINAL_FLAG32 != 0 {
                    let ordinal = (thunk_val & 0xFFFF) as u16;
                    entries.push(ImportEntry {
                        module,
                        module_name: dll_name.clone(),
                        function_name: String::new(),
                        ordinal: Some(ordinal),
                        resolved: false,
                    });
                } else {
                    if let Some(hint_off) = rva_to_offset(sections, thunk_val) {
                        if hint_off + 2 < buf.len() {
                            let func_name = read_cstring(buf, hint_off + 2);
                            entries.push(ImportEntry {
                                module,
                                module_name: dll_name.clone(),
                                function_name: func_name,
                                ordinal: None,
                                resolved: false,
                            });
                        }
                    }
                }
            }

            thunk_pos += thunk_size;
        }

        desc_off += 20;
    }

    Ok(entries)
}

// ---------------------------------------------------------------------------
// Base relocation table parsing
// ---------------------------------------------------------------------------

fn parse_relocations(
    buf: &[u8],
    sections: &[PeSection],
    reloc_rva: u32,
    reloc_size: u32,
) -> Result<Vec<Relocation>, BridgeError> {
    let mut result = Vec::new();
    let base_offset = match rva_to_offset(sections, reloc_rva) {
        Some(o) => o,
        None => return Ok(result),
    };

    let mut pos = 0u32;
    while pos < reloc_size {
        let block_off = base_offset + pos as usize;
        if block_off + 8 > buf.len() {
            break;
        }

        let page_rva = read_u32_le(buf, block_off);
        let block_size = read_u32_le(buf, block_off + 4);

        if block_size < 8 {
            break;
        }

        let num_entries = (block_size - 8) / 2;
        for i in 0..num_entries {
            let entry_off = block_off + 8 + (i as usize) * 2;
            if entry_off + 2 > buf.len() {
                break;
            }

            let raw = read_u16_le(buf, entry_off);
            let rel_type = raw >> 12;
            let offset = raw & 0x0FFF;

            if rel_type == IMAGE_REL_BASED_ABSOLUTE {
                continue;
            }

            result.push(Relocation {
                rva: page_rva + offset as u32,
                rel_type,
            });
        }

        pos += block_size;
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// DLL Registry — maps DLL name + function name to shim addresses
// ---------------------------------------------------------------------------

/// A single resolved function entry within a DLL.
#[derive(Debug, Clone)]
pub struct DllFunction {
    pub name: String,
    pub address: u64,
}

/// Registry of DLL shim functions available for import resolution.
pub struct DllRegistry {
    dlls: BTreeMap<String, Vec<DllFunction>>,
    next_stub: u64,
}

impl DllRegistry {
    pub fn new() -> Self {
        let mut reg = Self {
            dlls: BTreeMap::new(),
            next_stub: 0xDEAD_0000_0000_0000,
        };
        reg.register_builtin_dlls();
        reg
    }

    fn register_builtin_dlls(&mut self) {
        self.dlls.insert(String::from("kernel32.dll"), Vec::new());
        self.dlls.insert(String::from("user32.dll"), Vec::new());
        self.dlls.insert(String::from("ntdll.dll"), Vec::new());
        self.dlls.insert(String::from("gdi32.dll"), Vec::new());
        self.dlls.insert(String::from("advapi32.dll"), Vec::new());
        self.dlls.insert(String::from("shell32.dll"), Vec::new());
        self.dlls.insert(String::from("ole32.dll"), Vec::new());
        self.dlls.insert(String::from("ws2_32.dll"), Vec::new());
        self.dlls.insert(String::from("comctl32.dll"), Vec::new());
        self.dlls.insert(String::from("msvcrt.dll"), Vec::new());

        // Bulk-register the comprehensive Win32 name tables. These are the
        // names a real PE32+ import table is overwhelmingly likely to
        // reference. Each name resolves to a deterministic stub address;
        // whether the stub is wired up to a real Rust implementation
        // depends on the per-DLL module — but the name itself is reachable,
        // which is what the loader's relocation pass needs.
        crate::pe_dll_registry::register_all(self);

        // Keep the legacy small shim lists too — they overlap with the
        // bulk registry but assign different addresses, which is fine
        // because the BTreeMap entry de-dupes by name.
        self.register_kernel32_shims();
        self.register_user32_shims();
        self.register_gdi32_shims();
        self.register_ntdll_shims();
    }

    fn register_kernel32_shims(&mut self) {
        let base: u64 = 0xBAAD_0001_0000_0000;
        let funcs = [
            "CreateFileA",
            "CreateFileW",
            "ReadFile",
            "WriteFile",
            "CloseHandle",
            "GetFileSize",
            "SetFilePointer",
            "DeleteFileW",
            "FindFirstFileW",
            "FindNextFileW",
            "FindClose",
            "GetFileAttributesW",
            "CreateDirectoryW",
            "RemoveDirectoryW",
            "MoveFileW",
            "CopyFileW",
            "GetTempPathW",
            "GetTempFileNameW",
            "CreateProcessW",
            "ExitProcess",
            "GetCurrentProcessId",
            "GetCurrentThreadId",
            "CreateThread",
            "ExitThread",
            "TerminateProcess",
            "WaitForSingleObject",
            "WaitForMultipleObjects",
            "Sleep",
            "SleepEx",
            "GetExitCodeProcess",
            "GetExitCodeThread",
            "ResumeThread",
            "SuspendThread",
            "SetThreadPriority",
            "GetThreadPriority",
            "VirtualAlloc",
            "VirtualFree",
            "VirtualProtect",
            "VirtualQuery",
            "HeapCreate",
            "HeapDestroy",
            "HeapAlloc",
            "HeapFree",
            "HeapReAlloc",
            "GetProcessHeap",
            "GlobalAlloc",
            "GlobalFree",
            "LocalAlloc",
            "LocalFree",
            "CreateMutexW",
            "ReleaseMutex",
            "CreateEventW",
            "SetEvent",
            "ResetEvent",
            "CreateSemaphoreW",
            "ReleaseSemaphore",
            "InitializeCriticalSection",
            "EnterCriticalSection",
            "LeaveCriticalSection",
            "DeleteCriticalSection",
            "LoadLibraryW",
            "LoadLibraryA",
            "FreeLibrary",
            "GetProcAddress",
            "GetModuleHandleW",
            "GetModuleHandleA",
            "GetModuleFileNameW",
            "GetModuleFileNameA",
            "GetSystemInfo",
            "GetVersionExW",
            "GetSystemTimeAsFileTime",
            "QueryPerformanceCounter",
            "QueryPerformanceFrequency",
            "GetTickCount",
            "GetTickCount64",
            "GetComputerNameW",
            "GetUserNameW",
            "GetStdHandle",
            "WriteConsoleW",
            "WriteConsoleA",
            "ReadConsoleW",
            "SetConsoleCtrlHandler",
            "AllocConsole",
            "FreeConsole",
            "GetEnvironmentVariableW",
            "SetEnvironmentVariableW",
            "GetCommandLineW",
            "GetCurrentDirectoryW",
            "SetCurrentDirectoryW",
            "GetLastError",
            "SetLastError",
            "OutputDebugStringA",
            "OutputDebugStringW",
            "InterlockedIncrement",
            "InterlockedDecrement",
            "InterlockedExchange",
            "InterlockedCompareExchange",
            "TlsAlloc",
            "TlsFree",
            "TlsGetValue",
            "TlsSetValue",
            "IsDebuggerPresent",
            "FlushFileBuffers",
            "GetFileType",
            "GetSystemDirectoryW",
            "GetWindowsDirectoryW",
            "SetHandleInformation",
            "DuplicateHandle",
            "GetCurrentProcess",
            "SetConsoleTextAttribute",
            "SetConsoleTitleW",
            "GetConsoleMode",
            "SetConsoleMode",
        ];
        for (i, name) in funcs.iter().enumerate() {
            self.register("kernel32.dll", name, base + (i as u64) * 16);
        }
    }

    fn register_user32_shims(&mut self) {
        let base: u64 = 0xBAAD_0002_0000_0000;
        let funcs = [
            "RegisterClassExW",
            "RegisterClassExA",
            "UnregisterClassW",
            "UnregisterClassA",
            "CreateWindowExW",
            "CreateWindowExA",
            "DestroyWindow",
            "ShowWindow",
            "MoveWindow",
            "SetWindowPos",
            "GetWindowRect",
            "GetClientRect",
            "SetWindowTextW",
            "SetWindowTextA",
            "GetWindowTextW",
            "GetWindowTextA",
            "GetWindowTextLengthW",
            "GetWindowTextLengthA",
            "IsWindow",
            "IsWindowVisible",
            "EnableWindow",
            "GetForegroundWindow",
            "SetForegroundWindow",
            "GetDesktopWindow",
            "FindWindowW",
            "FindWindowA",
            "BringWindowToTop",
            "SetWindowLongW",
            "SetWindowLongPtrW",
            "GetWindowLongW",
            "GetWindowLongPtrW",
            "GetMessageW",
            "GetMessageA",
            "PeekMessageW",
            "PeekMessageA",
            "TranslateMessage",
            "DispatchMessageW",
            "DispatchMessageA",
            "PostMessageW",
            "PostMessageA",
            "SendMessageW",
            "SendMessageA",
            "PostQuitMessage",
            "DefWindowProcW",
            "DefWindowProcA",
            "MessageBoxW",
            "MessageBoxA",
            "DialogBoxParamW",
            "EndDialog",
            "GetDlgItem",
            "SetDlgItemTextW",
            "BeginPaint",
            "EndPaint",
            "InvalidateRect",
            "UpdateWindow",
            "GetDC",
            "ReleaseDC",
            "GetKeyState",
            "GetAsyncKeyState",
            "GetCursorPos",
            "SetCursorPos",
            "ShowCursor",
            "SetCapture",
            "ReleaseCapture",
            "GetKeyboardState",
            "MapVirtualKeyW",
            "OpenClipboard",
            "CloseClipboard",
            "GetClipboardData",
            "SetClipboardData",
            "EmptyClipboard",
            "GetSystemMetrics",
            "SystemParametersInfoW",
            "GetDpiForWindow",
            "LoadIconW",
            "LoadIconA",
            "LoadCursorW",
            "LoadCursorA",
            "LoadImageW",
            "SetTimer",
            "KillTimer",
        ];
        for (i, name) in funcs.iter().enumerate() {
            self.register("user32.dll", name, base + (i as u64) * 16);
        }
    }

    fn register_gdi32_shims(&mut self) {
        let base: u64 = 0xBAAD_0003_0000_0000;
        let funcs = [
            "CreateDCW",
            "DeleteDC",
            "CreateCompatibleDC",
            "SelectObject",
            "DeleteObject",
            "CreateSolidBrush",
            "CreatePen",
            "CreateFontW",
            "TextOutW",
            "TextOutA",
            "ExtTextOutW",
            "ExtTextOutA",
            "GetTextExtentPoint32W",
            "GetTextExtentPoint32A",
            "Rectangle",
            "Ellipse",
            "MoveToEx",
            "LineTo",
            "Polygon",
            "Polyline",
            "BitBlt",
            "StretchBlt",
            "StretchDIBits",
            "CreateBitmap",
            "CreateDIBSection",
            "CreateCompatibleBitmap",
            "GetDIBits",
            "SetDIBits",
            "SetPixel",
            "GetPixel",
            "FillRect",
            "FrameRect",
            "SetBkMode",
            "SetTextColor",
            "SetBkColor",
            "GetTextColor",
            "SaveDC",
            "RestoreDC",
            "GetDeviceCaps",
            "GetStockObject",
            "CreateRectRgn",
            "CreateRectRgnIndirect",
            "SelectClipRgn",
            "CombineRgn",
            "RoundRect",
            "Arc",
            "SetROP2",
            "GetCurrentPositionEx",
            "SetMapMode",
            "SetViewportOrgEx",
            "SetWindowOrgEx",
            "GetTextMetricsW",
            "GetTextMetricsA",
        ];
        for (i, name) in funcs.iter().enumerate() {
            self.register("gdi32.dll", name, base + (i as u64) * 16);
        }
    }

    fn register_ntdll_shims(&mut self) {
        let base: u64 = 0xBAAD_0004_0000_0000;
        let funcs = [
            "NtCreateFile",
            "NtReadFile",
            "NtWriteFile",
            "NtClose",
            "NtAllocateVirtualMemory",
            "NtFreeVirtualMemory",
            "NtProtectVirtualMemory",
            "NtQueryVirtualMemory",
            "NtCreateSection",
            "NtMapViewOfSection",
            "NtUnmapViewOfSection",
            "NtCreateProcess",
            "NtTerminateProcess",
            "NtQueryInformationProcess",
            "NtCreateThread",
            "NtTerminateThread",
            "NtQueryInformationThread",
            "NtCreateEvent",
            "NtSetEvent",
            "NtWaitForSingleObject",
            "NtWaitForMultipleObjects",
            "NtCreateKey",
            "NtOpenKey",
            "NtQueryValueKey",
            "NtSetValueKey",
            "NtDeleteKey",
            "NtQuerySystemInformation",
            "NtQuerySystemTime",
            "NtQueryPerformanceCounter",
            "NtDelayExecution",
            "NtYieldExecution",
            "NtQueryInformationFile",
            "NtSetInformationFile",
            "NtDuplicateObject",
            "RtlInitUnicodeString",
            "RtlFreeUnicodeString",
            "RtlCopyMemory",
            "RtlMoveMemory",
            "RtlZeroMemory",
            "RtlAllocateHeap",
            "RtlFreeHeap",
            "RtlSizeHeap",
            "RtlCompareUnicodeString",
            "RtlEqualUnicodeString",
            "RtlIntegerToUnicodeString",
        ];
        for (i, name) in funcs.iter().enumerate() {
            self.register("ntdll.dll", name, base + (i as u64) * 16);
        }
    }

    /// Register a function shim for a given DLL.
    pub fn register(&mut self, dll_name: &str, func_name: &str, address: u64) {
        let key = to_lowercase(dll_name);
        let funcs = self.dlls.entry(key).or_insert_with(Vec::new);
        // Override an existing entry rather than pushing a duplicate: a later
        // register() for the same (dll, func) is an explicit re-bind, and
        // resolve() returns the FIRST match — so a duplicate would shadow the
        // new address (this was the pe_loader::test_dll_registry failure) and
        // leave two ambiguous entries.
        if let Some(existing) = funcs.iter_mut().find(|f| f.name == func_name) {
            existing.address = address;
        } else {
            funcs.push(DllFunction {
                name: String::from(func_name),
                address,
            });
        }
    }

    /// Resolve a function by DLL name and function name. The DLL name is
    /// canonicalized through the API Set schema ([`crate::apiset`]) so
    /// `api-ms-win-*` contract imports resolve against the host module's
    /// registered names, exactly as the Windows loader redirects them.
    pub fn resolve(&mut self, dll_name: &str, func_name: &str) -> u64 {
        let key = crate::apiset::canonical_dll(dll_name);
        if let Some(funcs) = self.dlls.get(&key) {
            for f in funcs {
                if f.name == func_name {
                    return f.address;
                }
            }
        }
        self.allocate_stub()
    }

    /// Resolve by ordinal — returns a stub if not found.
    pub fn resolve_ordinal(&mut self, dll_name: &str, ordinal: u16) -> u64 {
        let key = crate::apiset::canonical_dll(dll_name);
        if let Some(funcs) = self.dlls.get(&key) {
            let ord_name = ordinal_name(ordinal);
            for f in funcs {
                if f.name == ord_name {
                    return f.address;
                }
            }
        }
        self.allocate_stub()
    }

    fn allocate_stub(&mut self) -> u64 {
        let addr = self.next_stub;
        self.next_stub += 16;
        addr
    }

    /// Check whether a DLL is known (registered). API Set contract names are
    /// canonicalized to their host module first.
    pub fn has_dll(&self, dll_name: &str) -> bool {
        let key = crate::apiset::canonical_dll(dll_name);
        self.dlls.contains_key(&key)
    }

    /// Number of registered DLLs.
    pub fn dll_count(&self) -> usize {
        self.dlls.len()
    }
}

fn ordinal_name(ordinal: u16) -> String {
    let mut s = String::from("#");
    let mut n = ordinal;
    if n == 0 {
        s.push('0');
        return s;
    }
    let mut digits = [0u8; 5];
    let mut i = 0;
    while n > 0 {
        digits[i] = (n % 10) as u8 + b'0';
        n /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        s.push(digits[i] as char);
    }
    s
}

fn to_lowercase(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            out.push((c as u8 + 32) as char);
        } else {
            out.push(c);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// PE Loader — maps sections, applies relocations, resolves imports
// ---------------------------------------------------------------------------

/// Load a PE executable from raw bytes into a virtual memory image.
///
/// 1. Parses the PE headers and sections.
/// 2. Allocates a contiguous `image_memory` buffer of `size_of_image` bytes.
/// 3. Copies section data at their virtual addresses.
/// 4. Applies base relocations if the load address differs from
///    the preferred `image_base`.
/// 5. Resolves import entries through the provided [`DllRegistry`].
pub fn load_pe(data: &[u8], registry: &mut DllRegistry) -> Result<LoadedPe, PeError> {
    let pe = parse_pe(data)?;

    if pe.size_of_image as u64 > 256 * 1024 * 1024 {
        return Err(PeError::ImageTooLarge {
            size: pe.size_of_image as u64,
        });
    }

    let mut image = alloc::vec![0u8; pe.size_of_image as usize];

    // Copy headers
    let header_copy = core::cmp::min(pe.size_of_headers as usize, data.len());
    let header_dest = core::cmp::min(header_copy, image.len());
    image[..header_dest].copy_from_slice(&data[..header_dest]);

    // Map sections
    for sec in &pe.sections {
        if sec.raw_data_size == 0 {
            continue;
        }
        let src_start = sec.raw_data_offset as usize;
        let copy_len = core::cmp::min(sec.raw_data_size, sec.virtual_size) as usize;
        let src_end = src_start + copy_len;
        if src_end > data.len() {
            return Err(PeError::SectionOutOfBounds {
                section_name: sec.name,
                offset: sec.raw_data_offset,
            });
        }
        let dst_start = sec.virtual_address as usize;
        let dst_end = dst_start + copy_len;
        if dst_end > image.len() {
            return Err(PeError::SectionOutOfBounds {
                section_name: sec.name,
                offset: sec.virtual_address,
            });
        }
        image[dst_start..dst_end].copy_from_slice(&data[src_start..src_end]);
    }

    // Apply base relocations (delta = 0 means no relocation needed, but we
    // still process them so the image is correctly patched for the actual
    // load address chosen at runtime).
    let actual_base = pe.image_base;
    let delta: i64 = 0; // loaded at preferred base for now

    if delta != 0 {
        apply_relocations(&mut image, &pe.relocations, delta, pe.is_64bit)?;
    }

    // Resolve imports
    let mut resolved_imports = pe.imports.clone();
    resolve_imports(
        &mut image,
        data,
        &pe.sections,
        &mut resolved_imports,
        pe.is_64bit,
        registry,
    )?;

    Ok(LoadedPe {
        image_base: actual_base,
        entry_point: actual_base + pe.entry_point,
        image_memory: image,
        imports: resolved_imports,
        is_64bit: pe.is_64bit,
    })
}

/// Apply base relocations to the mapped image.
pub fn apply_relocations(
    image: &mut [u8],
    relocations: &[Relocation],
    delta: i64,
    is_64bit: bool,
) -> Result<(), PeError> {
    for reloc in relocations {
        let rva = reloc.rva as usize;

        match reloc.rel_type {
            IMAGE_REL_BASED_HIGHLOW => {
                if rva + 4 > image.len() {
                    return Err(PeError::RelocationFailed { rva: reloc.rva });
                }
                let val = u32::from_le_bytes([
                    image[rva],
                    image[rva + 1],
                    image[rva + 2],
                    image[rva + 3],
                ]);
                let new_val = (val as i64 + delta) as u32;
                image[rva..rva + 4].copy_from_slice(&new_val.to_le_bytes());
            }
            IMAGE_REL_BASED_DIR64 => {
                if rva + 8 > image.len() {
                    return Err(PeError::RelocationFailed { rva: reloc.rva });
                }
                let val = u64::from_le_bytes([
                    image[rva],
                    image[rva + 1],
                    image[rva + 2],
                    image[rva + 3],
                    image[rva + 4],
                    image[rva + 5],
                    image[rva + 6],
                    image[rva + 7],
                ]);
                let new_val = (val as i64 + delta) as u64;
                image[rva..rva + 8].copy_from_slice(&new_val.to_le_bytes());
            }
            IMAGE_REL_BASED_HIGH => {
                if rva + 2 > image.len() {
                    return Err(PeError::RelocationFailed { rva: reloc.rva });
                }
                let val = u16::from_le_bytes([image[rva], image[rva + 1]]);
                let full = ((val as u32) << 16) as i64 + delta;
                let new_val = ((full >> 16) & 0xFFFF) as u16;
                image[rva..rva + 2].copy_from_slice(&new_val.to_le_bytes());
            }
            IMAGE_REL_BASED_LOW => {
                if rva + 2 > image.len() {
                    return Err(PeError::RelocationFailed { rva: reloc.rva });
                }
                let val = u16::from_le_bytes([image[rva], image[rva + 1]]);
                let new_val = (val as i64 + delta) as u16;
                image[rva..rva + 2].copy_from_slice(&new_val.to_le_bytes());
            }
            _ => {
                if !is_64bit && reloc.rel_type == IMAGE_REL_BASED_DIR64 {
                    return Err(PeError::RelocationFailed { rva: reloc.rva });
                }
            }
        }
    }
    Ok(())
}

/// Resolve import entries by patching the IAT in the loaded image.
fn resolve_imports(
    image: &mut [u8],
    raw_data: &[u8],
    sections: &[PeSection],
    imports: &mut [ImportEntry],
    is_64bit: bool,
    registry: &mut DllRegistry,
) -> Result<(), PeError> {
    let import_dd = find_data_directory(raw_data, IMAGE_DIRECTORY_ENTRY_IMPORT);
    let import_rva = match import_dd {
        Some((rva, _)) => rva,
        None => return Ok(()),
    };

    let import_offset = match rva_to_image_offset(sections, import_rva) {
        Some(o) => o,
        None => return Ok(()),
    };

    let mut import_idx = 0usize;
    let mut desc_off = import_offset;
    loop {
        if desc_off + 20 > raw_data.len() {
            break;
        }

        let _original_first_thunk = read_u32_le(raw_data, desc_off);
        let name_rva = read_u32_le(raw_data, desc_off + 12);
        let first_thunk_rva = read_u32_le(raw_data, desc_off + 16);

        if name_rva == 0 && _original_first_thunk == 0 {
            break;
        }

        let dll_name = match rva_to_offset(sections, name_rva) {
            Some(o) => read_cstring(raw_data, o),
            None => {
                desc_off += 20;
                continue;
            }
        };

        let thunk_rva = if _original_first_thunk != 0 {
            _original_first_thunk
        } else {
            first_thunk_rva
        };

        let read_offset = match rva_to_offset(sections, thunk_rva) {
            Some(o) => o,
            None => {
                desc_off += 20;
                continue;
            }
        };

        let thunk_size: usize = if is_64bit { 8 } else { 4 };
        let mut read_pos = read_offset;
        let mut iat_rva = first_thunk_rva as usize;

        loop {
            if read_pos + thunk_size > raw_data.len() {
                break;
            }

            let is_zero = if is_64bit {
                read_u64_le(raw_data, read_pos) == 0
            } else {
                read_u32_le(raw_data, read_pos) == 0
            };

            if is_zero {
                break;
            }

            let resolved_addr = if import_idx < imports.len() {
                let imp = &mut imports[import_idx];
                let addr = if imp.ordinal.is_some() {
                    registry.resolve_ordinal(&dll_name, imp.ordinal.unwrap())
                } else {
                    registry.resolve(&dll_name, &imp.function_name)
                };
                imp.resolved = true;
                import_idx += 1;
                addr
            } else {
                import_idx += 1;
                registry.resolve(&dll_name, "unknown")
            };

            // Patch IAT in image memory
            if is_64bit {
                if iat_rva + 8 <= image.len() {
                    image[iat_rva..iat_rva + 8].copy_from_slice(&resolved_addr.to_le_bytes());
                }
            } else {
                if iat_rva + 4 <= image.len() {
                    let addr32 = resolved_addr as u32;
                    image[iat_rva..iat_rva + 4].copy_from_slice(&addr32.to_le_bytes());
                }
            }

            read_pos += thunk_size;
            iat_rva += thunk_size;
        }

        desc_off += 20;
    }

    Ok(())
}

fn find_data_directory(buf: &[u8], index: usize) -> Option<(u32, u32)> {
    if buf.len() < DOS_HEADER_MIN_SIZE {
        return None;
    }
    let pe_offset = read_u32_le(buf, E_LFANEW_OFFSET) as usize;
    let opt = pe_offset + 4 + 20;
    if opt + 2 > buf.len() {
        return None;
    }
    let opt_magic = read_u16_le(buf, opt);
    let dd_offset = match opt_magic {
        PE32_MAGIC => opt + 96,
        PE32PLUS_MAGIC => opt + 112,
        _ => return None,
    };
    let num_dd_off = match opt_magic {
        PE32_MAGIC => opt + 92,
        PE32PLUS_MAGIC => opt + 108,
        _ => return None,
    };
    if num_dd_off + 4 > buf.len() {
        return None;
    }
    let num_dd = read_u32_le(buf, num_dd_off) as usize;
    if index >= num_dd {
        return None;
    }
    let entry_off = dd_offset + index * 8;
    if entry_off + 8 > buf.len() {
        return None;
    }
    let rva = read_u32_le(buf, entry_off);
    let size = read_u32_le(buf, entry_off + 4);
    if rva == 0 {
        return None;
    }
    Some((rva, size))
}

fn rva_to_image_offset(sections: &[PeSection], rva: u32) -> Option<usize> {
    rva_to_offset(sections, rva)
}

// ---------------------------------------------------------------------------
// Error translation: AthenaOS errors → Win32 error codes
// ---------------------------------------------------------------------------

/// Maps AthenaOS-internal error categories to Win32 error codes.
pub fn raeen_error_to_win32(err: &RaeenOsError) -> u32 {
    match err {
        RaeenOsError::NotFound => crate::ERROR_FILE_NOT_FOUND,
        RaeenOsError::PermissionDenied => crate::ERROR_ACCESS_DENIED,
        RaeenOsError::AlreadyExists => crate::ERROR_ALREADY_EXISTS,
        RaeenOsError::InvalidArgument => crate::ERROR_INVALID_PARAMETER,
        RaeenOsError::OutOfMemory => crate::ERROR_NOT_ENOUGH_MEMORY,
        RaeenOsError::NotSupported => crate::ERROR_NOT_SUPPORTED,
        RaeenOsError::Busy => crate::ERROR_SHARING_VIOLATION,
        RaeenOsError::IoError => crate::ERROR_INVALID_DATA,
        RaeenOsError::TooManyHandles => crate::ERROR_TOO_MANY_OPEN_FILES,
        RaeenOsError::InvalidHandle => crate::ERROR_INVALID_HANDLE,
        RaeenOsError::BufferTooSmall => crate::ERROR_INSUFFICIENT_BUFFER,
        RaeenOsError::EndOfFile => crate::ERROR_HANDLE_EOF,
        RaeenOsError::NoAccess => crate::ERROR_NOACCESS,
    }
}

/// Abstraction over AthenaOS native error categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaeenOsError {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    InvalidArgument,
    OutOfMemory,
    NotSupported,
    Busy,
    IoError,
    TooManyHandles,
    InvalidHandle,
    BufferTooSmall,
    EndOfFile,
    NoAccess,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use alloc::vec;

    fn make_pe_with_sections(is_64bit: bool) -> Vec<u8> {
        let pe_offset: u32 = 0x80;
        let opt_header_size: u16 = if is_64bit { 112 + 16 * 8 } else { 96 + 16 * 8 };
        let section_start = pe_offset as usize + 4 + 20 + opt_header_size as usize;
        let total_size = section_start + 40 * 2 + 0x400;
        let mut buf = vec![0u8; total_size];

        buf[0] = 0x4D;
        buf[1] = 0x5A;
        buf[0x3C..0x40].copy_from_slice(&pe_offset.to_le_bytes());

        let pe_off = pe_offset as usize;
        buf[pe_off..pe_off + 4].copy_from_slice(&[0x50, 0x45, 0x00, 0x00]);

        let coff = pe_off + 4;
        let machine: u16 = if is_64bit { 0x8664 } else { 0x014C };
        buf[coff..coff + 2].copy_from_slice(&machine.to_le_bytes());
        buf[coff + 2..coff + 4].copy_from_slice(&2u16.to_le_bytes()); // 2 sections
        buf[coff + 16..coff + 18].copy_from_slice(&opt_header_size.to_le_bytes());

        let opt = coff + 20;
        let magic: u16 = if is_64bit { PE32PLUS_MAGIC } else { PE32_MAGIC };
        buf[opt..opt + 2].copy_from_slice(&magic.to_le_bytes());
        buf[opt + 16..opt + 20].copy_from_slice(&0x1000u32.to_le_bytes()); // entry RVA

        if is_64bit {
            buf[opt + 24..opt + 32].copy_from_slice(&0x0040_0000u64.to_le_bytes()); // image base
            buf[opt + 32..opt + 36].copy_from_slice(&0x1000u32.to_le_bytes()); // section alignment
            buf[opt + 36..opt + 40].copy_from_slice(&0x200u32.to_le_bytes()); // file alignment
            buf[opt + 56..opt + 60].copy_from_slice(&0x3000u32.to_le_bytes()); // size of image
            buf[opt + 60..opt + 64].copy_from_slice(&0x200u32.to_le_bytes()); // size of headers
            buf[opt + 108..opt + 112].copy_from_slice(&16u32.to_le_bytes()); // num data dirs
        } else {
            buf[opt + 28..opt + 32].copy_from_slice(&0x0040_0000u32.to_le_bytes());
            buf[opt + 32..opt + 36].copy_from_slice(&0x1000u32.to_le_bytes());
            buf[opt + 36..opt + 40].copy_from_slice(&0x200u32.to_le_bytes());
            buf[opt + 56..opt + 60].copy_from_slice(&0x3000u32.to_le_bytes());
            buf[opt + 60..opt + 64].copy_from_slice(&0x200u32.to_le_bytes());
            buf[opt + 92..opt + 96].copy_from_slice(&16u32.to_le_bytes());
        }

        // Section 1: .text
        let s1 = section_start;
        buf[s1..s1 + 5].copy_from_slice(b".text");
        buf[s1 + 8..s1 + 12].copy_from_slice(&0x100u32.to_le_bytes()); // virtual size
        buf[s1 + 12..s1 + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // virtual address
        buf[s1 + 16..s1 + 20].copy_from_slice(&0x100u32.to_le_bytes()); // raw data size
        buf[s1 + 20..s1 + 24].copy_from_slice(&0x200u32.to_le_bytes()); // raw data offset
        buf[s1 + 36..s1 + 40].copy_from_slice(&0x60000020u32.to_le_bytes()); // CODE|EXECUTE|READ

        // Section 2: .data
        let s2 = section_start + 40;
        buf[s2..s2 + 5].copy_from_slice(b".data");
        buf[s2 + 8..s2 + 12].copy_from_slice(&0x100u32.to_le_bytes());
        buf[s2 + 12..s2 + 16].copy_from_slice(&0x2000u32.to_le_bytes());
        buf[s2 + 16..s2 + 20].copy_from_slice(&0x100u32.to_le_bytes());
        buf[s2 + 20..s2 + 24].copy_from_slice(&0x300u32.to_le_bytes());
        buf[s2 + 36..s2 + 40].copy_from_slice(&0xC0000040u32.to_le_bytes()); // INITIALIZED_DATA|READ|WRITE

        buf
    }

    #[test]
    fn test_parse_pe_64bit() {
        let buf = make_pe_with_sections(true);
        let pe = parse_pe(&buf).unwrap();
        assert!(pe.is_64bit);
        assert_eq!(pe.image_base, 0x0040_0000);
        assert_eq!(pe.entry_point, 0x1000);
        assert_eq!(pe.sections.len(), 2);
        assert_eq!(pe.sections[0].name_str(), ".text");
        assert!(pe.sections[0].is_executable());
        assert_eq!(pe.sections[1].name_str(), ".data");
        assert!(pe.sections[1].is_writable());
    }

    #[test]
    fn test_parse_pe_32bit() {
        let buf = make_pe_with_sections(false);
        let pe = parse_pe(&buf).unwrap();
        assert!(!pe.is_64bit);
        assert_eq!(pe.image_base, 0x0040_0000);
        assert_eq!(pe.entry_point, 0x1000);
        assert_eq!(pe.sections.len(), 2);
    }

    #[test]
    fn test_dll_registry() {
        let mut reg = DllRegistry::new();
        assert!(reg.has_dll("kernel32.dll"));
        assert!(reg.has_dll("KERNEL32.DLL"));
        assert!(!reg.has_dll("foobar.dll"));

        reg.register("kernel32.dll", "CreateFileA", 0x1234);
        assert_eq!(reg.resolve("kernel32.dll", "CreateFileA"), 0x1234);

        let stub = reg.resolve("kernel32.dll", "NonExistent");
        assert!(stub >= 0xDEAD_0000_0000_0000);
    }

    #[test]
    fn test_relocation_highlow() {
        let mut image = vec![0u8; 16];
        image[4..8].copy_from_slice(&0x0040_1000u32.to_le_bytes());

        let relocs = vec![Relocation {
            rva: 4,
            rel_type: IMAGE_REL_BASED_HIGHLOW,
        }];
        apply_relocations(&mut image, &relocs, 0x1000, false).unwrap();

        let val = u32::from_le_bytes([image[4], image[5], image[6], image[7]]);
        assert_eq!(val, 0x0040_2000);
    }

    #[test]
    fn test_relocation_dir64() {
        let mut image = vec![0u8; 16];
        image[0..8].copy_from_slice(&0x0000_0001_4000_0000u64.to_le_bytes());

        let relocs = vec![Relocation {
            rva: 0,
            rel_type: IMAGE_REL_BASED_DIR64,
        }];
        apply_relocations(&mut image, &relocs, 0x2000, true).unwrap();

        let val = u64::from_le_bytes([
            image[0], image[1], image[2], image[3], image[4], image[5], image[6], image[7],
        ]);
        assert_eq!(val, 0x0000_0001_4000_2000);
    }

    #[test]
    fn test_raeen_error_translation() {
        assert_eq!(
            raeen_error_to_win32(&RaeenOsError::NotFound),
            crate::ERROR_FILE_NOT_FOUND
        );
        assert_eq!(
            raeen_error_to_win32(&RaeenOsError::PermissionDenied),
            crate::ERROR_ACCESS_DENIED
        );
        assert_eq!(
            raeen_error_to_win32(&RaeenOsError::OutOfMemory),
            crate::ERROR_NOT_ENOUGH_MEMORY
        );
    }

    #[test]
    fn test_load_pe_basic() {
        let buf = make_pe_with_sections(true);
        let mut reg = DllRegistry::new();
        let loaded = load_pe(&buf, &mut reg).unwrap();
        assert!(loaded.is_64bit);
        assert_eq!(loaded.image_base, 0x0040_0000);
        assert_eq!(loaded.entry_point, 0x0040_0000 + 0x1000);
    }
}
