//! AthBridge — Windows app compatibility layer.
//!
//! See `docs/components/raebridge.md` for the design.
// no_std for real builds; std under `cargo test` so the ldr/disasm/pe_inspect
// host KATs link (pe_loader's registry test is fixed, so the crate is testable).
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod advapi32;
pub mod apiset;
pub mod broker;
pub mod comctl32;
pub mod comdlg32;
pub mod crt_startup;
pub mod crypt32;
pub mod d3d11;
pub mod d3d12;
pub mod d3d9;
pub mod d3d_translate;
pub mod dbghelp;
pub mod dinput;
pub mod disasm;
pub mod dwmapi;
pub mod dwrite;
pub mod dxbc_spirv;
pub mod dxgi;
pub mod exec;
pub mod gdi32;
pub mod handoff;
pub mod iphlpapi;
pub mod kernel32;
pub mod launcher;
pub mod ldr;
pub mod msvcrt;
pub mod ntdll;
pub mod ole32;
pub mod pe_dll_registry;
pub mod pe_inspect;
pub mod pe_loader;
pub mod psapi;
pub mod registry;
pub mod seh;
pub mod setupapi;
pub mod shell32;
pub mod shlwapi;
pub mod sync_engine;
pub mod syscalls;
pub mod testpe;
pub mod thunks;
pub mod user32;
pub mod userenv;
pub mod uxtheme;
pub mod version;
pub mod wevtapi;
pub mod winapi_shims;
pub mod winhttp;
pub mod winmm;
pub mod winsock2;
pub mod ws2_32;
pub mod xinput_deep;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

// ---------------------------------------------------------------------------
// Win32 primitive types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WinHandle(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WinBool(pub i32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DWord(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Word(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HResult(pub i32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NtStatus(pub i32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LargeInteger(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ULargeInteger(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Atom(pub u16);

impl WinBool {
    pub fn is_true(self) -> bool {
        self.0 != 0
    }
    pub fn from_bool(v: bool) -> Self {
        Self(if v { 1 } else { 0 })
    }
}

pub const INVALID_HANDLE_VALUE: WinHandle = WinHandle(u64::MAX);
pub const TRUE: WinBool = WinBool(1);
pub const FALSE: WinBool = WinBool(0);
pub const NULL_HANDLE: WinHandle = WinHandle(0);

// Standard handles
pub const STD_INPUT_HANDLE: u32 = 0xFFFFFFF6;
pub const STD_OUTPUT_HANDLE: u32 = 0xFFFFFFF5;
pub const STD_ERROR_HANDLE: u32 = 0xFFFFFFF4;

// ---------------------------------------------------------------------------
// Win32 error codes
// ---------------------------------------------------------------------------

pub const ERROR_SUCCESS: u32 = 0;
pub const ERROR_INVALID_FUNCTION: u32 = 1;
pub const ERROR_FILE_NOT_FOUND: u32 = 2;
pub const ERROR_PATH_NOT_FOUND: u32 = 3;
pub const ERROR_TOO_MANY_OPEN_FILES: u32 = 4;
pub const ERROR_ACCESS_DENIED: u32 = 5;
pub const ERROR_INVALID_HANDLE: u32 = 6;
pub const ERROR_NOT_ENOUGH_MEMORY: u32 = 8;
pub const ERROR_INVALID_DATA: u32 = 13;
pub const ERROR_OUTOFMEMORY: u32 = 14;
pub const ERROR_NO_MORE_FILES: u32 = 18;
pub const ERROR_SHARING_VIOLATION: u32 = 32;
pub const ERROR_HANDLE_EOF: u32 = 38;
pub const ERROR_NOT_SUPPORTED: u32 = 50;
pub const ERROR_INVALID_PARAMETER: u32 = 87;
pub const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
pub const ERROR_ALREADY_EXISTS: u32 = 183;
pub const ERROR_ENVVAR_NOT_FOUND: u32 = 203;
pub const ERROR_MORE_DATA: u32 = 234;
pub const ERROR_NO_MORE_ITEMS: u32 = 259;
pub const ERROR_DIRECTORY: u32 = 267;
pub const ERROR_PROC_NOT_FOUND: u32 = 127;
pub const ERROR_MOD_NOT_FOUND: u32 = 126;
pub const ERROR_IO_PENDING: u32 = 997;
pub const ERROR_NOACCESS: u32 = 998;
/// `ReleaseMutex` from a thread that does not own the mutex.
pub const ERROR_NOT_OWNER: u32 = 288;
/// `ReleaseSemaphore` past the maximum count.
pub const ERROR_TOO_MANY_POSTS: u32 = 298;

// Wait return values
pub const WAIT_OBJECT_0: u32 = 0x00000000;
pub const WAIT_ABANDONED_0: u32 = 0x00000080;
pub const WAIT_TIMEOUT: u32 = 0x00000102;
pub const WAIT_FAILED: u32 = 0xFFFFFFFF;
pub const INFINITE: u32 = 0xFFFFFFFF;

// Memory allocation types
pub const MEM_COMMIT: u32 = 0x00001000;
pub const MEM_RESERVE: u32 = 0x00002000;
pub const MEM_DECOMMIT: u32 = 0x00004000;
pub const MEM_RELEASE: u32 = 0x00008000;
pub const MEM_FREE: u32 = 0x00010000;

// Memory protection constants
pub const PAGE_NOACCESS: u32 = 0x01;
pub const PAGE_READONLY: u32 = 0x02;
pub const PAGE_READWRITE: u32 = 0x04;
pub const PAGE_WRITECOPY: u32 = 0x08;
pub const PAGE_EXECUTE: u32 = 0x10;
pub const PAGE_EXECUTE_READ: u32 = 0x20;
pub const PAGE_EXECUTE_READWRITE: u32 = 0x40;
pub const PAGE_GUARD: u32 = 0x100;

// File creation dispositions
pub const CREATE_NEW: u32 = 1;
pub const CREATE_ALWAYS: u32 = 2;
pub const OPEN_EXISTING: u32 = 3;
pub const OPEN_ALWAYS: u32 = 4;
pub const TRUNCATE_EXISTING: u32 = 5;

// Generic access rights
pub const GENERIC_READ: u32 = 0x80000000;
pub const GENERIC_WRITE: u32 = 0x40000000;
pub const GENERIC_EXECUTE: u32 = 0x20000000;
pub const GENERIC_ALL: u32 = 0x10000000;

// File attributes
pub const FILE_ATTRIBUTE_READONLY: u32 = 0x00000001;
pub const FILE_ATTRIBUTE_HIDDEN: u32 = 0x00000002;
pub const FILE_ATTRIBUTE_SYSTEM: u32 = 0x00000004;
pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x00000010;
pub const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x00000020;
pub const FILE_ATTRIBUTE_NORMAL: u32 = 0x00000080;
pub const FILE_ATTRIBUTE_TEMPORARY: u32 = 0x00000100;
pub const INVALID_FILE_ATTRIBUTES: u32 = 0xFFFFFFFF;

// File pointer move methods
pub const FILE_BEGIN: u32 = 0;
pub const FILE_CURRENT: u32 = 1;
pub const FILE_END: u32 = 2;

// ShowWindow commands
pub const SW_HIDE: i32 = 0;
pub const SW_SHOWNORMAL: i32 = 1;
pub const SW_SHOWMINIMIZED: i32 = 2;
pub const SW_SHOWMAXIMIZED: i32 = 3;
pub const SW_SHOW: i32 = 5;
pub const SW_MINIMIZE: i32 = 6;
pub const SW_RESTORE: i32 = 9;

// Window messages
pub const WM_NULL: u32 = 0x0000;
pub const WM_CREATE: u32 = 0x0001;
pub const WM_DESTROY: u32 = 0x0002;
pub const WM_MOVE: u32 = 0x0003;
pub const WM_SIZE: u32 = 0x0005;
pub const WM_SETTEXT: u32 = 0x000C;
pub const WM_GETTEXT: u32 = 0x000D;
pub const WM_GETTEXTLENGTH: u32 = 0x000E;
pub const WM_CLOSE: u32 = 0x0010;
pub const WM_QUIT: u32 = 0x0012;
pub const WM_PAINT: u32 = 0x000F;
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_CHAR: u32 = 0x0102;
pub const WM_COMMAND: u32 = 0x0111;
pub const WM_TIMER: u32 = 0x0113;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_USER: u32 = 0x0400;

// MessageBox flags
pub const MB_OK: u32 = 0x00000000;
pub const MB_OKCANCEL: u32 = 0x00000001;
pub const MB_YESNO: u32 = 0x00000004;
pub const MB_ICONERROR: u32 = 0x00000010;
pub const MB_ICONWARNING: u32 = 0x00000030;
pub const MB_ICONINFORMATION: u32 = 0x00000040;

// MessageBox return values
pub const IDOK: i32 = 1;
pub const IDCANCEL: i32 = 2;
pub const IDYES: i32 = 6;
pub const IDNO: i32 = 7;

// NT status codes
pub const STATUS_SUCCESS: i32 = 0;
pub const STATUS_INVALID_HANDLE: i32 = 0xC0000008_u32 as i32;
pub const STATUS_INVALID_PARAMETER: i32 = 0xC000000D_u32 as i32;
pub const STATUS_ACCESS_DENIED: i32 = 0xC0000022_u32 as i32;
pub const STATUS_OBJECT_NAME_NOT_FOUND: i32 = 0xC0000034_u32 as i32;
pub const STATUS_NO_MEMORY: i32 = 0xC0000017_u32 as i32;
pub const STATUS_NOT_IMPLEMENTED: i32 = 0xC0000002_u32 as i32;
pub const STATUS_BUFFER_TOO_SMALL: i32 = 0xC0000023_u32 as i32;
pub const STATUS_PENDING: i32 = 0x00000103;

// Socket constants
pub const SOCKET_ERROR: i32 = -1;
pub const INVALID_SOCKET: u64 = u64::MAX;

// GDI stock objects
pub const WHITE_BRUSH: i32 = 0;
pub const LTGRAY_BRUSH: i32 = 1;
pub const GRAY_BRUSH: i32 = 2;
pub const DKGRAY_BRUSH: i32 = 3;
pub const BLACK_BRUSH: i32 = 4;
pub const NULL_BRUSH: i32 = 5;
pub const WHITE_PEN: i32 = 6;
pub const BLACK_PEN: i32 = 7;
pub const NULL_PEN: i32 = 8;
pub const SYSTEM_FONT: i32 = 13;
pub const DEFAULT_GUI_FONT: i32 = 17;

// GDI background modes
pub const TRANSPARENT: i32 = 1;
pub const OPAQUE: i32 = 2;

// ---------------------------------------------------------------------------
// Win32 composite structures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Size {
    pub cx: i32,
    pub cy: i32,
}

#[derive(Debug, Clone)]
pub struct Msg {
    pub hwnd: WinHandle,
    pub message: u32,
    pub wparam: u64,
    pub lparam: i64,
    pub time: u32,
    pub pt: Point,
}

#[derive(Debug, Clone)]
pub struct PaintStruct {
    pub hdc: WinHandle,
    pub erase: WinBool,
    pub rc_paint: Rect,
}

#[derive(Debug, Clone)]
pub struct SystemInfo {
    pub processor_architecture: u16,
    pub page_size: u32,
    pub min_app_address: u64,
    pub max_app_address: u64,
    pub active_processor_mask: u64,
    pub number_of_processors: u32,
    pub processor_type: u32,
    pub allocation_granularity: u32,
    pub processor_level: u16,
    pub processor_revision: u16,
}

#[derive(Debug, Clone)]
pub struct OsVersionInfoExW {
    pub major_version: u32,
    pub minor_version: u32,
    pub build_number: u32,
    pub platform_id: u32,
    pub service_pack_major: u16,
    pub service_pack_minor: u16,
    pub suite_mask: u16,
    pub product_type: u8,
}

#[derive(Debug, Clone)]
pub struct MemoryBasicInformation {
    pub base_address: u64,
    pub allocation_base: u64,
    pub allocation_protect: u32,
    pub region_size: u64,
    pub state: u32,
    pub protect: u32,
    pub mem_type: u32,
}

#[derive(Debug, Clone)]
pub struct WsaData {
    pub version: u16,
    pub high_version: u16,
    pub description: String,
    pub system_status: String,
    pub max_sockets: u16,
    pub max_udp_dg: u16,
}

#[derive(Debug, Clone)]
pub struct WndClassExW {
    pub style: u32,
    /// The guest's `lpfnWndProc` (a Win64 function pointer in the guest's own
    /// address space). Captured at `RegisterClassEx` time and invoked by
    /// `DispatchMessage`/`SendMessage` — the window's message handler. 0 = none
    /// (the marshaling layer that reads it from the guest `WNDCLASSEXW` is the
    /// IAT-wiring follow-up; the dispatch path resolves and calls it).
    pub wnd_proc: u64,
    pub class_name: String,
    pub icon: WinHandle,
    pub cursor: WinHandle,
    pub background: WinHandle,
    pub menu_name: Option<String>,
    pub icon_sm: WinHandle,
}

// ---------------------------------------------------------------------------
// Virtual memory tracking
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct VirtualRegion {
    pub base_address: u64,
    pub size: u64,
    pub state: u32,
    pub protect: u32,
    pub allocation_type: u32,
}

// ---------------------------------------------------------------------------
// Window object for user32 emulation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct WindowObject {
    pub handle: WinHandle,
    pub class_name: String,
    pub title: String,
    pub style: u32,
    pub ex_style: u32,
    pub rect: Rect,
    pub client_rect: Rect,
    pub parent: WinHandle,
    pub visible: bool,
    pub enabled: bool,
    pub user_data: i64,
    pub surface_id: Option<u64>,
    pub surface_vaddr: Option<u64>,
}

// ---------------------------------------------------------------------------
// Wide-string helper
// ---------------------------------------------------------------------------

pub fn wide_to_string(wide: &[u16]) -> String {
    let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
    let mut s = String::new();
    for &code in &wide[..end] {
        if code < 0x80 {
            s.push(code as u8 as char);
        } else {
            s.push(char::REPLACEMENT_CHARACTER);
        }
    }
    s
}

pub fn string_to_wide(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Errors produced by the AthBridge subsystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeError {
    /// The input buffer is too small to contain the expected structure.
    BufferTooSmall { expected: usize, actual: usize },
    /// The DOS header magic (`MZ`) is invalid.
    InvalidDosSignature([u8; 2]),
    /// The PE signature (`PE\0\0`) is invalid.
    InvalidPeSignature([u8; 4]),
    /// The PE optional header magic is unrecognized.
    UnknownOptionalMagic(u16),
    /// The `e_lfanew` offset points outside the buffer.
    PeOffsetOutOfBounds { offset: u32, buf_len: usize },
    /// A translation layer operation is not yet implemented.
    Unimplemented(&'static str),
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BufferTooSmall { expected, actual } => {
                write!(f, "buffer too small: need {expected} bytes, got {actual}")
            }
            Self::InvalidDosSignature(sig) => {
                write!(
                    f,
                    "invalid DOS signature: [{:#04x}, {:#04x}]",
                    sig[0], sig[1]
                )
            }
            Self::InvalidPeSignature(sig) => {
                write!(
                    f,
                    "invalid PE signature: [{:#04x}, {:#04x}, {:#04x}, {:#04x}]",
                    sig[0], sig[1], sig[2], sig[3]
                )
            }
            Self::UnknownOptionalMagic(m) => write!(f, "unknown optional header magic: {m:#06x}"),
            Self::PeOffsetOutOfBounds { offset, buf_len } => {
                write!(
                    f,
                    "e_lfanew ({offset:#x}) out of bounds (buf len {buf_len})"
                )
            }
            Self::Unimplemented(msg) => write!(f, "not implemented: {msg}"),
        }
    }
}

// ---------------------------------------------------------------------------
// PE header parsing
// ---------------------------------------------------------------------------

const DOS_MAGIC: [u8; 2] = [0x4D, 0x5A]; // "MZ"
const PE_SIGNATURE: [u8; 4] = [0x50, 0x45, 0x00, 0x00]; // "PE\0\0"

/// Offset of the `e_lfanew` field within the DOS header.
pub(crate) const E_LFANEW_OFFSET: usize = 0x3C;
/// Minimum size of a DOS header (through `e_lfanew`).
pub(crate) const DOS_HEADER_MIN_SIZE: usize = E_LFANEW_OFFSET + 4;

/// Machine architecture as declared in the COFF header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineType {
    I386,
    Amd64,
    Arm,
    Arm64,
    Unknown(u16),
}

impl From<u16> for MachineType {
    fn from(v: u16) -> Self {
        match v {
            0x014C => Self::I386,
            0x8664 => Self::Amd64,
            0x01C0 => Self::Arm,
            0xAA64 => Self::Arm64,
            other => Self::Unknown(other),
        }
    }
}

/// Whether the PE is 32-bit (PE32) or 64-bit (PE32+).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeFormat {
    Pe32,
    Pe32Plus,
}

/// Summary information extracted from a PE image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeInfo {
    pub machine: MachineType,
    pub format: PeFormat,
    pub num_sections: u16,
    pub entry_point_rva: u32,
}

/// Read a little-endian `u16` from a byte slice at `offset`.
#[inline]
pub(crate) fn read_u16_le(buf: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([buf[offset], buf[offset + 1]])
}

/// Read a little-endian `u32` from a byte slice at `offset`.
#[inline]
pub(crate) fn read_u32_le(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ])
}

/// Parse the DOS + PE headers from a raw byte slice and return [`PeInfo`].
///
/// This validates the DOS `MZ` magic, the `PE\0\0` signature, reads the COFF
/// header for machine type and section count, and reads the optional header
/// magic + entry point RVA.
pub fn load_pe(buf: &[u8]) -> Result<PeInfo, BridgeError> {
    if buf.len() < DOS_HEADER_MIN_SIZE {
        return Err(BridgeError::BufferTooSmall {
            expected: DOS_HEADER_MIN_SIZE,
            actual: buf.len(),
        });
    }

    // Validate DOS magic.
    let dos_sig: [u8; 2] = [buf[0], buf[1]];
    if dos_sig != DOS_MAGIC {
        return Err(BridgeError::InvalidDosSignature(dos_sig));
    }

    // Read e_lfanew — offset to PE signature.
    let pe_offset = read_u32_le(buf, E_LFANEW_OFFSET) as usize;

    // We need at least: PE sig (4) + COFF header (20) + optional header magic (2) + 16 bytes
    // for the entry point field depending on format.
    let pe_header_min = pe_offset + 4 + 20 + 24;
    if pe_header_min > buf.len() {
        return Err(BridgeError::PeOffsetOutOfBounds {
            offset: pe_offset as u32,
            buf_len: buf.len(),
        });
    }

    // Validate PE signature.
    let pe_sig: [u8; 4] = [
        buf[pe_offset],
        buf[pe_offset + 1],
        buf[pe_offset + 2],
        buf[pe_offset + 3],
    ];
    if pe_sig != PE_SIGNATURE {
        return Err(BridgeError::InvalidPeSignature(pe_sig));
    }

    // COFF header starts immediately after the 4-byte PE signature.
    let coff_offset = pe_offset + 4;
    let machine_raw = read_u16_le(buf, coff_offset);
    let num_sections = read_u16_le(buf, coff_offset + 2);

    // Optional header starts at coff_offset + 20.
    let opt_offset = coff_offset + 20;
    let opt_magic = read_u16_le(buf, opt_offset);

    let format = match opt_magic {
        0x010B => PeFormat::Pe32,
        0x020B => PeFormat::Pe32Plus,
        other => return Err(BridgeError::UnknownOptionalMagic(other)),
    };

    // Entry point RVA is at optional header offset + 16 for both PE32 and PE32+.
    let entry_point_rva = read_u32_le(buf, opt_offset + 16);

    Ok(PeInfo {
        machine: MachineType::from(machine_raw),
        format,
        num_sections,
        entry_point_rva,
    })
}

// ---------------------------------------------------------------------------
// Compatibility session
// ---------------------------------------------------------------------------

/// Unique identifier for a compatibility session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub u64);

/// State of a running compatibility session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// The session has been created but not yet started.
    Created,
    /// The PE image has been loaded and relocated.
    Loaded,
    /// The translated process is actively executing.
    Running,
    /// Execution has been suspended (e.g. breakpoint or host request).
    Suspended,
    /// The session has terminated with the given exit code.
    Terminated(i32),
}

/// A running Windows compatibility session.
///
/// Owns the loaded PE image memory and tracks execution state.
#[derive(Debug)]
pub struct CompatSession {
    pub id: SessionId,
    pub state: SessionState,
    pub image_name: String,
    /// Raw loaded PE image bytes (relocated in the future).
    image: Vec<u8>,
    pub pe_info: PeInfo,
}

impl CompatSession {
    /// Create a new session from raw PE image bytes.
    pub fn new(id: SessionId, image_name: String, image: Vec<u8>) -> Result<Self, BridgeError> {
        let pe_info = load_pe(&image)?;
        Ok(Self {
            id,
            state: SessionState::Loaded,
            image_name,
            image,
            pe_info,
        })
    }

    /// Get a reference to the raw image bytes.
    pub fn image_bytes(&self) -> &[u8] {
        &self.image
    }

    /// Transition the session to the `Running` state.
    pub fn start(&mut self) -> Result<(), BridgeError> {
        match self.state {
            SessionState::Loaded | SessionState::Suspended => {
                self.state = SessionState::Running;
                Ok(())
            }
            _ => Err(BridgeError::Unimplemented(
                "cannot start session from current state",
            )),
        }
    }

    /// Suspend the running session.
    pub fn suspend(&mut self) -> Result<(), BridgeError> {
        match self.state {
            SessionState::Running => {
                self.state = SessionState::Suspended;
                Ok(())
            }
            _ => Err(BridgeError::Unimplemented(
                "cannot suspend session that is not running",
            )),
        }
    }

    /// Terminate the session with an exit code.
    pub fn terminate(&mut self, exit_code: i32) {
        self.state = SessionState::Terminated(exit_code);
    }
}

// ---------------------------------------------------------------------------
// Translation layer trait
// ---------------------------------------------------------------------------

/// Result type for syscall translation operations.
pub type TranslateResult = Result<u64, BridgeError>;

/// Interface for translating Windows NT syscalls into AthenaOS native calls.
///
/// Implementors map Windows kernel32/ntdll syscall numbers and semantics to the
/// host kernel's equivalent operations. Each method receives the session context
/// and raw register-level arguments.
pub trait TranslationLayer {
    /// Translate an `NtCreateFile`-equivalent open call.
    fn nt_create_file(
        &self,
        session: &mut CompatSession,
        desired_access: u32,
        path_ptr: u64,
        path_len: u32,
    ) -> TranslateResult;

    /// Translate an `NtReadFile`-equivalent read call.
    fn nt_read_file(
        &self,
        session: &mut CompatSession,
        handle: u64,
        buffer_ptr: u64,
        length: u32,
    ) -> TranslateResult;

    /// Translate an `NtWriteFile`-equivalent write call.
    fn nt_write_file(
        &self,
        session: &mut CompatSession,
        handle: u64,
        buffer_ptr: u64,
        length: u32,
    ) -> TranslateResult;

    /// Translate an `NtClose`-equivalent handle close.
    fn nt_close(&self, session: &mut CompatSession, handle: u64) -> TranslateResult;

    /// Translate a `NtAllocateVirtualMemory`-equivalent allocation.
    fn nt_allocate_virtual_memory(
        &self,
        session: &mut CompatSession,
        base_address: u64,
        size: u64,
        allocation_type: u32,
        protect: u32,
    ) -> TranslateResult;

    /// Translate an `NtFreeVirtualMemory`-equivalent deallocation.
    fn nt_free_virtual_memory(
        &self,
        session: &mut CompatSession,
        base_address: u64,
        size: u64,
        free_type: u32,
    ) -> TranslateResult;

    /// Generic fallback for unrecognized syscall numbers.
    fn dispatch_syscall(
        &self,
        session: &mut CompatSession,
        syscall_number: u32,
        args: &[u64],
    ) -> TranslateResult;
}

// ---------------------------------------------------------------------------
// PE section parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeSection {
    pub name: [u8; 8],
    pub virtual_size: u32,
    pub virtual_address: u32,
    pub raw_data_size: u32,
    pub raw_data_offset: u32,
    pub characteristics: u32,
}

impl PeSection {
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(8);
        core::str::from_utf8(&self.name[..end]).unwrap_or("???")
    }

    pub fn is_executable(&self) -> bool {
        self.characteristics & 0x20000000 != 0
    }

    pub fn is_writable(&self) -> bool {
        self.characteristics & 0x80000000 != 0
    }

    pub fn is_readable(&self) -> bool {
        self.characteristics & 0x40000000 != 0
    }
}

pub fn parse_sections(buf: &[u8], pe_info: &PeInfo) -> Result<Vec<PeSection>, BridgeError> {
    let pe_offset = read_u32_le(buf, E_LFANEW_OFFSET) as usize;
    let coff_offset = pe_offset + 4;
    let opt_header_size = read_u16_le(buf, coff_offset + 16) as usize;
    let section_start = coff_offset + 20 + opt_header_size;

    let mut sections = Vec::new();
    for i in 0..pe_info.num_sections as usize {
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
    Ok(sections)
}

// ---------------------------------------------------------------------------
// Windows API stub table
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinApiModule {
    Kernel32,
    User32,
    Gdi32,
    Advapi32,
    Ntdll,
    Ole32,
    Shell32,
    Ws2_32,
    Comctl32,
    Winmm,
    DxgiDll,
    D3d9Dll,
    D3d11Dll,
    D3d12Dll,
    XInput,
    DInput8,
    Version,
    Shlwapi,
    Msvcrt,
    WinHttp,
    Iphlpapi,
    Userenv,
    Setupapi,
    Wevtapi,
    Psapi,
    Dbghelp,
    Crypt32,
    DWrite,
    Dwmapi,
    Uxtheme,
    Unknown,
}

pub(crate) fn ascii_eq_ignore_case(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).all(|(x, y)| {
        let lx = if x >= b'A' && x <= b'Z' { x + 32 } else { x };
        let ly = if y >= b'A' && y <= b'Z' { y + 32 } else { y };
        lx == ly
    })
}

impl WinApiModule {
    pub fn from_name(name: &str) -> Self {
        // Redirect API Set contract DLLs (api-ms-win-*, ext-ms-win-*) and host
        // aliases (kernelbase/ucrtbase/combase/...) to the canonical AthBridge
        // module before classifying — modern binaries import through these, not
        // the physical DLL. Grounded in the real Win10 schema (crate::apiset).
        let canon = crate::apiset::canonical_dll(name);
        let name: &str = canon.as_str();
        if ascii_eq_ignore_case(name, "kernel32.dll") {
            return Self::Kernel32;
        }
        if ascii_eq_ignore_case(name, "user32.dll") {
            return Self::User32;
        }
        if ascii_eq_ignore_case(name, "gdi32.dll") {
            return Self::Gdi32;
        }
        if ascii_eq_ignore_case(name, "advapi32.dll") {
            return Self::Advapi32;
        }
        if ascii_eq_ignore_case(name, "ntdll.dll") {
            return Self::Ntdll;
        }
        if ascii_eq_ignore_case(name, "ole32.dll") {
            return Self::Ole32;
        }
        if ascii_eq_ignore_case(name, "shell32.dll") {
            return Self::Shell32;
        }
        if ascii_eq_ignore_case(name, "ws2_32.dll") {
            return Self::Ws2_32;
        }
        if ascii_eq_ignore_case(name, "comctl32.dll") {
            return Self::Comctl32;
        }
        if ascii_eq_ignore_case(name, "winmm.dll") {
            return Self::Winmm;
        }
        if ascii_eq_ignore_case(name, "dxgi.dll") {
            return Self::DxgiDll;
        }
        if ascii_eq_ignore_case(name, "d3d11.dll") {
            return Self::D3d11Dll;
        }
        if ascii_eq_ignore_case(name, "d3d12.dll") {
            return Self::D3d12Dll;
        }
        if ascii_eq_ignore_case(name, "xinput1_3.dll") {
            return Self::XInput;
        }
        if ascii_eq_ignore_case(name, "xinput1_4.dll") {
            return Self::XInput;
        }
        if ascii_eq_ignore_case(name, "xinput9_1_0.dll") {
            return Self::XInput;
        }
        if ascii_eq_ignore_case(name, "dinput8.dll") {
            return Self::DInput8;
        }
        if ascii_eq_ignore_case(name, "dinput.dll") {
            return Self::DInput8;
        }
        if ascii_eq_ignore_case(name, "version.dll") {
            return Self::Version;
        }
        if ascii_eq_ignore_case(name, "shlwapi.dll") {
            return Self::Shlwapi;
        }
        if ascii_eq_ignore_case(name, "msvcrt.dll") {
            return Self::Msvcrt;
        }
        if ascii_eq_ignore_case(name, "msvcr100.dll") {
            return Self::Msvcrt;
        }
        if ascii_eq_ignore_case(name, "msvcr110.dll") {
            return Self::Msvcrt;
        }
        if ascii_eq_ignore_case(name, "msvcr120.dll") {
            return Self::Msvcrt;
        }
        if ascii_eq_ignore_case(name, "msvcr140.dll") {
            return Self::Msvcrt;
        }
        if ascii_eq_ignore_case(name, "vcruntime140.dll") {
            return Self::Msvcrt;
        }
        if ascii_eq_ignore_case(name, "ucrtbase.dll") {
            return Self::Msvcrt;
        }
        if ascii_eq_ignore_case(name, "winhttp.dll") {
            return Self::WinHttp;
        }
        if ascii_eq_ignore_case(name, "wininet.dll") {
            return Self::WinHttp;
        }
        if ascii_eq_ignore_case(name, "iphlpapi.dll") {
            return Self::Iphlpapi;
        }
        if ascii_eq_ignore_case(name, "userenv.dll") {
            return Self::Userenv;
        }
        if ascii_eq_ignore_case(name, "setupapi.dll") {
            return Self::Setupapi;
        }
        if ascii_eq_ignore_case(name, "wevtapi.dll") {
            return Self::Wevtapi;
        }
        if ascii_eq_ignore_case(name, "psapi.dll") {
            return Self::Psapi;
        }
        if ascii_eq_ignore_case(name, "dbghelp.dll") {
            return Self::Dbghelp;
        }
        if ascii_eq_ignore_case(name, "crypt32.dll") {
            return Self::Crypt32;
        }
        if ascii_eq_ignore_case(name, "dwrite.dll") {
            return Self::DWrite;
        }
        if ascii_eq_ignore_case(name, "dwmapi.dll") {
            return Self::Dwmapi;
        }
        if ascii_eq_ignore_case(name, "uxtheme.dll") {
            return Self::Uxtheme;
        }
        if ascii_eq_ignore_case(name, "d3d9.dll") {
            return Self::D3d9Dll;
        }
        Self::Unknown
    }
}

#[derive(Debug, Clone)]
pub struct ImportEntry {
    pub module: WinApiModule,
    pub module_name: String,
    pub function_name: String,
    pub ordinal: Option<u16>,
    pub resolved: bool,
}

// ---------------------------------------------------------------------------
// Windows NT handle table
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleType {
    File,
    Directory,
    Process,
    Thread,
    Event,
    Mutex,
    Semaphore,
    RegKey,
    Section,
    Token,
    IoCompletion,
    GdiObj,
}

#[derive(Debug, Clone)]
pub struct NtHandle {
    pub handle_value: u64,
    pub handle_type: HandleType,
    pub access_mask: u32,
    pub native_id: Option<u64>,
    pub name: Option<String>,
}

pub struct HandleTable {
    handles: BTreeMap<u64, NtHandle>,
    next_handle: u64,
}

impl HandleTable {
    pub fn new() -> Self {
        Self {
            handles: BTreeMap::new(),
            next_handle: 4,
        }
    }

    pub fn allocate(
        &mut self,
        handle_type: HandleType,
        access_mask: u32,
        name: Option<String>,
    ) -> u64 {
        let h = self.next_handle;
        self.next_handle += 4;
        self.handles.insert(
            h,
            NtHandle {
                handle_value: h,
                handle_type,
                access_mask,
                native_id: None,
                name,
            },
        );
        h
    }

    pub fn get(&self, handle: u64) -> Option<&NtHandle> {
        self.handles.get(&handle)
    }

    pub fn get_mut(&mut self, handle: u64) -> Option<&mut NtHandle> {
        self.handles.get_mut(&handle)
    }

    pub fn close(&mut self, handle: u64) -> bool {
        self.handles.remove(&handle).is_some()
    }

    pub fn count(&self) -> usize {
        self.handles.len()
    }
}

// ---------------------------------------------------------------------------
// Windows synchronization objects (mutex / event / semaphore)
// ---------------------------------------------------------------------------
//
// Concept §Compatibility: "apps run naturally." A real multi-threaded Windows
// app (and Steam itself) relies on `CreateMutex`/`WaitForSingleObject` actually
// *blocking* and on a `Global\Name` object being shared. The previous shims
// allocated a handle and returned TRUE without touching any state — a single-
// threaded app that never contends survived, anything that depended on the
// object's semantics broke (`docs/components/raebridge-wine-strategy.md` §6.1).
//
// This is the *in-process* object model: correct state-machine transitions and
// a per-process named namespace (`Local\`/`Global\` collapse to one map here).
// The non-blocking acquire path is exact; true cross-thread blocking and the
// cross-*process* namespace are the `raebridge_server` broker's job (slice 2)
// and layer on top of this same object store.

/// Which kind of waitable kernel object a [`SyncObject`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncKind {
    Mutex,
    Event,
    Semaphore,
}

/// State backing one named-or-anonymous synchronization object. Multiple
/// handles (named `CreateMutexW` reopen, `OpenMutexW`, `DuplicateHandle`) can
/// reference the same object; [`FullCompatSession::sync_objects`] is keyed by an
/// internal object id and [`FullCompatSession::handle_to_sync`] maps each open
/// handle to it.
#[derive(Debug, Clone)]
pub struct SyncObject {
    pub kind: SyncKind,
    /// Generic "is signaled" flag. Event: set/reset. Semaphore: count > 0.
    /// Mutex: true means *free* (unowned), false means held.
    pub signaled: bool,
    /// Event only: stays signaled until `ResetEvent` when true; auto-resets
    /// after releasing one waiter when false.
    pub manual_reset: bool,
    /// Mutex only: owning guest thread id, 0 = unowned.
    pub owner_thread: u32,
    /// Mutex only: recursive acquisition depth held by `owner_thread`.
    pub recursion: u32,
    /// Semaphore only: current count.
    pub count: i32,
    /// Semaphore only: maximum count.
    pub max_count: i32,
    /// Open handles referencing this object. The object is dropped when this
    /// reaches zero (and the name, if any, is freed).
    pub refs: u32,
    /// Optional global name — the namespace key.
    pub name: Option<String>,
}

impl SyncObject {
    pub fn mutex(initial_owner: u32, name: Option<String>) -> Self {
        let owned = initial_owner != 0;
        Self {
            kind: SyncKind::Mutex,
            signaled: !owned,
            manual_reset: false,
            owner_thread: if owned { initial_owner } else { 0 },
            recursion: if owned { 1 } else { 0 },
            count: 0,
            max_count: 0,
            refs: 1,
            name,
        }
    }

    pub fn event(manual_reset: bool, initial_state: bool, name: Option<String>) -> Self {
        Self {
            kind: SyncKind::Event,
            signaled: initial_state,
            manual_reset,
            owner_thread: 0,
            recursion: 0,
            count: 0,
            max_count: 0,
            refs: 1,
            name,
        }
    }

    pub fn semaphore(initial: i32, maximum: i32, name: Option<String>) -> Self {
        Self {
            kind: SyncKind::Semaphore,
            signaled: initial > 0,
            manual_reset: false,
            owner_thread: 0,
            recursion: 0,
            count: initial,
            max_count: maximum,
            refs: 1,
            name,
        }
    }
}

/// Result of [`FullCompatSession::create_sync_object`].
pub enum CreateSyncResult {
    /// Brand-new object; handle value is the field.
    Created(u64),
    /// A named object of the same kind already existed — this is a new handle to
    /// it. Windows sets `ERROR_ALREADY_EXISTS` (but still returns the handle).
    Opened(u64),
    /// A named object exists with a *different* kind. Windows fails the create
    /// with `ERROR_INVALID_HANDLE`.
    TypeMismatch,
}

// ---------------------------------------------------------------------------
// Windows registry emulation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum RegValue {
    String(String),
    DWord(u32),
    QWord(u64),
    Binary(Vec<u8>),
    MultiString(Vec<String>),
    ExpandString(String),
}

/// Backward-compatible flat-path registry API backed by the tree-based
/// `registry::RegistryHive`.  The flat `key_path` format uses backslash-
/// separated components with the root abbreviated (`HKLM`, `HKCU`, etc.).
pub struct RegistryHive {
    tree: registry::RegistryHive,
    key_cache: BTreeMap<String, bool>,
}

impl RegistryHive {
    pub fn new() -> Self {
        Self {
            tree: registry::RegistryHive::new(),
            key_cache: BTreeMap::new(),
        }
    }

    pub fn tree(&self) -> &registry::RegistryHive {
        &self.tree
    }

    pub fn tree_mut(&mut self) -> &mut registry::RegistryHive {
        &mut self.tree
    }

    fn to_tree_value(v: &RegValue) -> registry::RegistryValue {
        match v {
            RegValue::String(s) => registry::RegistryValue::Sz(s.clone()),
            RegValue::DWord(d) => registry::RegistryValue::DWord(*d),
            RegValue::QWord(q) => registry::RegistryValue::QWord(*q),
            RegValue::Binary(b) => registry::RegistryValue::Binary(b.clone()),
            RegValue::MultiString(ss) => registry::RegistryValue::MultiSz(ss.clone()),
            RegValue::ExpandString(s) => registry::RegistryValue::ExpandSz(s.clone()),
        }
    }

    fn from_tree_value(v: &registry::RegistryValue) -> RegValue {
        match v {
            registry::RegistryValue::Sz(s) => RegValue::String(s.clone()),
            registry::RegistryValue::ExpandSz(s) => RegValue::ExpandString(s.clone()),
            registry::RegistryValue::DWord(d) => RegValue::DWord(*d),
            registry::RegistryValue::DWordBigEndian(d) => RegValue::DWord(*d),
            registry::RegistryValue::QWord(q) => RegValue::QWord(*q),
            registry::RegistryValue::Binary(b) => RegValue::Binary(b.clone()),
            registry::RegistryValue::MultiSz(ss) => RegValue::MultiString(ss.clone()),
            registry::RegistryValue::Link(s) => RegValue::String(s.clone()),
            registry::RegistryValue::None => RegValue::Binary(Vec::new()),
        }
    }

    pub fn set_value(&mut self, key_path: &str, name: &str, value: RegValue) {
        self.key_cache.insert(String::from(key_path), true);
        self.tree
            .set_value_by_path(key_path, name, Self::to_tree_value(&value));
    }

    pub fn get_value(&self, key_path: &str, name: &str) -> Option<RegValue> {
        self.tree
            .get_value_by_path(key_path, name)
            .map(Self::from_tree_value)
    }

    pub fn delete_value(&mut self, key_path: &str, name: &str) -> bool {
        self.tree.delete_value_by_path(key_path, name)
    }

    pub fn key_exists(&self, key_path: &str) -> bool {
        self.tree.key_exists_by_path(key_path)
    }

    pub fn enumerate_values(&self, key_path: &str) -> Option<Vec<(String, RegValue)>> {
        self.tree.enumerate_values_by_path(key_path).map(|entries| {
            entries
                .into_iter()
                .map(|(n, v)| (n.clone(), Self::from_tree_value(v)))
                .collect()
        })
    }

    pub fn enumerate_keys(&self) -> Vec<String> {
        let mut all = Vec::new();
        self.collect_paths(&self.tree.hkcr, "HKCR", &mut all);
        self.collect_paths(&self.tree.hkcu, "HKCU", &mut all);
        self.collect_paths(&self.tree.hklm, "HKLM", &mut all);
        self.collect_paths(&self.tree.hku, "HKU", &mut all);
        self.collect_paths(&self.tree.hkcc, "HKCC", &mut all);
        all
    }

    fn collect_paths(&self, key: &registry::RegistryKey, prefix: &str, out: &mut Vec<String>) {
        out.push(String::from(prefix));
        for (name, child) in &key.subkeys {
            let mut child_path = String::from(prefix);
            child_path.push('\\');
            child_path.push_str(name);
            self.collect_paths(child, &child_path, out);
        }
    }
}

// ---------------------------------------------------------------------------
// Windows path translation
// ---------------------------------------------------------------------------

pub fn translate_win_path(win_path: &str) -> String {
    let path = win_path.replace('\\', "/");

    if path.len() >= 2 && path.as_bytes()[1] == b':' {
        let drive = (path.as_bytes()[0] as char).to_ascii_lowercase();
        let rest = if path.len() > 2 { &path[2..] } else { "" };
        let mut result = String::from("/mnt/win_");
        result.push(drive);
        result.push_str(rest);
        return result;
    }

    if path.starts_with("//./") || path.starts_with("//?/") {
        let rest = &path[4..];
        let mut result = String::from("/dev/win/");
        result.push_str(rest);
        return result;
    }

    path
}

/// A filesystem-safe, stable per-app bucket id derived from the app's identity
/// (its image name). Distinct apps get distinct buckets; the same app is stable
/// across runs. This namespaces each app's virtual `C:\` so one app cannot see
/// another's files — the Concept "per-app data buckets" isolation, at the path
/// level. (AthFS bucket-key binding is the deeper kernel follow-up.)
pub fn app_bucket_id(identity: &str) -> String {
    // Basename (drop any path), lowercased, non-alphanumeric -> '_'.
    let base = identity
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or(identity);
    let mut safe = String::new();
    for c in base.chars() {
        if c.is_ascii_alphanumeric() {
            safe.push(c.to_ascii_lowercase());
        } else {
            safe.push('_');
        }
    }
    // FNV-1a over the FULL identity disambiguates names that sanitize alike
    // (e.g. "a-b.exe" vs "a_b.exe") so distinct apps never collide.
    let mut h: u32 = 0x811c_9dc5;
    for b in identity.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    alloc::format!("{}_{:08x}", safe, h)
}

/// Translate a Windows path to its VFS path under the app's per-app `bucket`. A
/// drive path (`C:\...`) is namespaced as `/mnt/win_<drive>/<bucket>/...` — the
/// tmpfs flattens that to a distinct `win_<drive>_<bucket>_...` name, so two
/// apps' `C:\out.txt` resolve to different storage (isolation). Non-drive paths
/// fall through to [`translate_win_path`].
pub fn translate_win_path_bucketed(win_path: &str, bucket: &str) -> String {
    let path = win_path.replace('\\', "/");
    if path.len() >= 2 && path.as_bytes()[1] == b':' {
        let drive = (path.as_bytes()[0] as char).to_ascii_lowercase();
        let rest = if path.len() > 2 { &path[2..] } else { "" };
        let mut result = alloc::format!("/mnt/win_{}/{}", drive, bucket);
        if !rest.starts_with('/') {
            result.push('/');
        }
        result.push_str(rest);
        return result;
    }
    translate_win_path(win_path)
}

/// Convenience: the VFS path a Windows `win_path` maps to under the app
/// `identity`'s bucket. Pure, so the host harness can compute the same path a
/// guest's `CreateFileW` wrote to.
pub fn app_bucket_vfs_path(identity: &str, win_path: &str) -> String {
    translate_win_path_bucketed(win_path, &app_bucket_id(identity))
}

// ---------------------------------------------------------------------------
// DirectX → AthGFX translation layer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DxApiVersion {
    DirectX9,
    DirectX10,
    DirectX11,
    DirectX12,
    Vulkan,
}

#[derive(Debug, Clone)]
pub struct DxTranslationStats {
    pub api_version: DxApiVersion,
    pub draw_calls_translated: u64,
    pub state_changes_translated: u64,
    pub shaders_translated: u64,
    pub buffers_created: u64,
    pub textures_created: u64,
    pub frames_presented: u64,
}

impl DxTranslationStats {
    pub fn new(api: DxApiVersion) -> Self {
        Self {
            api_version: api,
            draw_calls_translated: 0,
            state_changes_translated: 0,
            shaders_translated: 0,
            buffers_created: 0,
            textures_created: 0,
            frames_presented: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Extended compat session with full Windows state
// ---------------------------------------------------------------------------

pub struct FullCompatSession {
    pub session: CompatSession,
    pub handle_table: HandleTable,
    pub registry: RegistryHive,
    pub imports: Vec<ImportEntry>,
    pub sections: Vec<PeSection>,
    pub dx_stats: Option<DxTranslationStats>,
    pub environment: BTreeMap<String, String>,
    pub working_directory: String,
    pub command_line: String,
    pub last_error: u32,
    pub current_process_id: u32,
    pub current_thread_id: u32,
    pub virtual_regions: BTreeMap<u64, VirtualRegion>,
    pub next_heap_id: u64,
    pub windows: BTreeMap<u64, WindowObject>,
    pub next_window_id: u64,
    pub registered_classes: BTreeMap<String, WndClassExW>,
    /// GDI objects (brushes/pens/DCs) keyed by handle. A DC stores its target
    /// window so `FillRect`/`TextOut` can reach that window's surface buffer.
    pub gdi_objects: BTreeMap<u64, crate::gdi32::GdiObject>,
    /// Cumulative pixels a guest has rastered into window surfaces (FillRect +
    /// TextOut). The observable proof that a GUI guest actually painted — the
    /// harness reads it after a GUI fixture runs (a real surface can't be
    /// eyeballed headlessly).
    pub gui_paint_pixels: u64,
    pub message_queue: Vec<Msg>,
    pub quit_posted: bool,
    pub clipboard: BTreeMap<u32, Vec<u8>>,
    pub clipboard_open: bool,
    pub loaded_modules: BTreeMap<String, u64>,
    pub wsa_initialized: bool,
    pub wsa_error: i32,
    /// Live HeapAlloc allocations: pointer → (size, align). Backs the real
    /// allocator in `kernel32::heap_alloc` so HeapFree can rebuild the Layout.
    pub heap_allocations: BTreeMap<u64, (u64, u64)>,
    /// NUL-terminated UTF-16 command line. `GetCommandLineW` returns a raw
    /// pointer into this buffer, so it must stay allocated (and unmodified)
    /// for the lifetime of the session.
    pub command_line_w: Vec<u16>,
    /// NUL-terminated 8-bit (ANSI) command line, backing `GetCommandLineA`.
    /// Same lifetime contract as `command_line_w`.
    pub command_line_a: Vec<u8>,
    /// Per-process TLS slots (`TlsAlloc`/`TlsGetValue`/`TlsSetValue`/`TlsFree`).
    /// A slot is `Some(value)` when allocated, `None` when free. Index into
    /// this vec is the TLS index the CRT round-trips. Single-threaded model:
    /// one slot array for the whole session (the guest has one thread until
    /// SYS_THREAD_CREATE lands).
    pub tls_slots: Vec<Option<u64>>,
    /// Per-process FLS (fiber-local storage) slots backing
    /// `FlsAlloc`/`FlsGetValue`/`FlsSetValue`/`FlsFree`. Same model as
    /// [`Self::tls_slots`] but each slot also carries the optional destructor
    /// callback `FlsAlloc` was given. The MSVC `/MT` CRT keeps its per-thread
    /// `_ptd` in an FLS slot, so this is on the path to `main`.
    pub fls_slots: Vec<Option<crt_startup::FlsSlot>>,
    /// NUL-terminated, double-NUL-delimited UTF-16 environment block backing
    /// `GetEnvironmentStringsW`. Rebuilt lazily; the pointer handed out must
    /// stay valid until `FreeEnvironmentStringsW`, which is a no-op here (the
    /// block is owned by the session, not the guest).
    pub environment_block_w: Vec<u16>,
    /// Synchronization-object store keyed by internal object id. See
    /// [`SyncObject`]. Multiple handles can alias one object (named reopen,
    /// `OpenMutexW`, `DuplicateHandle`).
    pub sync_objects: BTreeMap<u64, SyncObject>,
    /// Maps each open sync handle value → the object id it references.
    pub handle_to_sync: BTreeMap<u64, u64>,
    /// In-process named-object namespace: name → object id.
    pub named_sync: BTreeMap<String, u64>,
    /// Monotonic object-id allocator for [`Self::sync_objects`].
    pub next_sync_id: u64,
    /// Menu objects keyed by HMENU. A menu is a flat ordered item list; a
    /// submenu is an item whose `submenu` is a child HMENU. See [`user32::Menu`].
    pub menus: BTreeMap<u64, crate::user32::Menu>,
    /// Each window's attached menu bar (HWND → HMENU), set by `SetMenu` or the
    /// `CreateWindowEx` hMenu argument.
    pub window_menus: BTreeMap<u64, u64>,
    /// Monotonic HMENU allocator.
    pub next_menu_id: u64,
}

/// Alias for the full compatibility context used by Win32 API functions.
pub type CompatContext = FullCompatSession;

impl FullCompatSession {
    pub fn new(
        id: SessionId,
        image_name: String,
        image: Vec<u8>,
        command_line: String,
    ) -> Result<Self, BridgeError> {
        let pe_info = load_pe(&image)?;
        let sections = parse_sections(&image, &pe_info)?;
        let session = CompatSession {
            id,
            state: SessionState::Loaded,
            image_name: image_name.clone(),
            image,
            pe_info,
        };

        let mut env = BTreeMap::new();
        env.insert(String::from("OS"), String::from("Windows_NT"));
        env.insert(
            String::from("PROCESSOR_ARCHITECTURE"),
            String::from("AMD64"),
        );
        env.insert(String::from("SystemRoot"), String::from("C:\\Windows"));
        env.insert(String::from("windir"), String::from("C:\\Windows"));
        env.insert(
            String::from("TEMP"),
            String::from("C:\\Users\\user\\AppData\\Local\\Temp"),
        );
        env.insert(
            String::from("TMP"),
            String::from("C:\\Users\\user\\AppData\\Local\\Temp"),
        );
        env.insert(String::from("USERNAME"), String::from("user"));
        env.insert(String::from("USERPROFILE"), String::from("C:\\Users\\user"));

        let command_line_w: Vec<u16> = command_line
            .encode_utf16()
            .chain(core::iter::once(0))
            .collect();
        let command_line_a: Vec<u8> = command_line.bytes().chain(core::iter::once(0)).collect();

        // Seed the three standard handles. The handle *values* (4/8/12) are the
        // ones `kernel32::get_std_handle` hands back for STD_INPUT/OUTPUT/ERROR,
        // so a guest's GetStdHandle → WriteFile/ReadFile round-trips straight to
        // the host's native stdin/stdout/stderr fds (0/1/2) via `native_id`.
        let mut handle_table = HandleTable::new();
        let h_in = handle_table.allocate(HandleType::File, 0x1, Some(String::from("STDIN")));
        let h_out = handle_table.allocate(HandleType::File, 0x2, Some(String::from("STDOUT")));
        let h_err = handle_table.allocate(HandleType::File, 0x2, Some(String::from("STDERR")));
        if let Some(e) = handle_table.get_mut(h_in) {
            e.native_id = Some(0);
        }
        if let Some(e) = handle_table.get_mut(h_out) {
            e.native_id = Some(1);
        }
        if let Some(e) = handle_table.get_mut(h_err) {
            e.native_id = Some(2);
        }

        // Seed the module list with the always-present system DLLs at stable
        // synthetic bases, plus the main executable at its conventional base.
        // GetModuleHandle*/GetProcAddress resolve against this map, so a CRT
        // that does `GetProcAddress(GetModuleHandleW(L"kernel32.dll"), ...)`
        // round-trips to a real, non-null shim address (see winapi_shims).
        let mut loaded_modules = BTreeMap::new();
        loaded_modules.insert(String::from("kernel32.dll"), KERNEL32_MODULE_BASE);
        loaded_modules.insert(String::from("ntdll.dll"), NTDLL_MODULE_BASE);

        Ok(Self {
            session,
            handle_table,
            registry: RegistryHive::new(),
            imports: Vec::new(),
            sections,
            dx_stats: None,
            environment: env,
            working_directory: String::from("C:\\Users\\user"),
            command_line,
            last_error: 0,
            current_process_id: 1000,
            current_thread_id: 1004,
            virtual_regions: BTreeMap::new(),
            next_heap_id: 0x00100000,
            windows: BTreeMap::new(),
            next_window_id: 0x00010000,
            registered_classes: BTreeMap::new(),
            gdi_objects: BTreeMap::new(),
            gui_paint_pixels: 0,
            message_queue: Vec::new(),
            quit_posted: false,
            clipboard: BTreeMap::new(),
            clipboard_open: false,
            loaded_modules,
            wsa_initialized: false,
            wsa_error: 0,
            heap_allocations: BTreeMap::new(),
            command_line_w,
            command_line_a,
            tls_slots: Vec::new(),
            fls_slots: Vec::new(),
            environment_block_w: Vec::new(),
            sync_objects: BTreeMap::new(),
            handle_to_sync: BTreeMap::new(),
            named_sync: BTreeMap::new(),
            next_sync_id: 1,
            menus: BTreeMap::new(),
            window_menus: BTreeMap::new(),
            next_menu_id: 0x00020000,
        })
    }

    /// The VFS path a guest's Windows `path` maps to under THIS app's per-app
    /// bucket (derived from the session image name). Routing `CreateFileW`
    /// through this gives each app an isolated virtual `C:\`.
    pub fn win_path_to_vfs(&self, path: &str) -> String {
        translate_win_path_bucketed(path, &app_bucket_id(&self.session.image_name))
    }

    // -----------------------------------------------------------------------
    // Synchronization-object store (mutex / event / semaphore)
    // -----------------------------------------------------------------------

    /// Map a [`SyncKind`] to the handle-table type recorded for its handles.
    fn sync_handle_type(kind: SyncKind) -> HandleType {
        match kind {
            SyncKind::Mutex => HandleType::Mutex,
            SyncKind::Event => HandleType::Event,
            SyncKind::Semaphore => HandleType::Semaphore,
        }
    }

    /// Create a new sync object, or open an existing named one of the same kind.
    /// The caller supplies the freshly-built [`SyncObject`] (with its initial
    /// state); when a same-named, same-kind object already exists it is reused
    /// and `fresh` is discarded. Allocates and wires up a handle on success.
    pub fn create_sync_object(&mut self, fresh: SyncObject) -> CreateSyncResult {
        let kind = fresh.kind;
        if let Some(name) = fresh.name.clone() {
            if let Some(&obj_id) = self.named_sync.get(&name) {
                let existing_kind = self.sync_objects.get(&obj_id).map(|o| o.kind);
                if existing_kind != Some(kind) {
                    return CreateSyncResult::TypeMismatch;
                }
                if let Some(o) = self.sync_objects.get_mut(&obj_id) {
                    o.refs += 1;
                }
                let h = self.handle_table.allocate(
                    Self::sync_handle_type(kind),
                    GENERIC_ALL,
                    Some(name),
                );
                self.handle_to_sync.insert(h, obj_id);
                return CreateSyncResult::Opened(h);
            }
        }

        let obj_id = self.next_sync_id;
        self.next_sync_id += 1;
        let name = fresh.name.clone();
        self.sync_objects.insert(obj_id, fresh);
        let h = self
            .handle_table
            .allocate(Self::sync_handle_type(kind), GENERIC_ALL, name.clone());
        self.handle_to_sync.insert(h, obj_id);
        if let Some(name) = name {
            self.named_sync.insert(name, obj_id);
        }
        CreateSyncResult::Created(h)
    }

    /// Open an *existing* named object of `kind` (the `OpenMutexW`/`OpenEventW`/
    /// `OpenSemaphoreW` path). Returns a new handle, or `None` if no such named
    /// object of that kind exists.
    pub fn open_sync_object(&mut self, kind: SyncKind, name: &str) -> Option<u64> {
        let &obj_id = self.named_sync.get(name)?;
        if self.sync_objects.get(&obj_id).map(|o| o.kind) != Some(kind) {
            return None;
        }
        if let Some(o) = self.sync_objects.get_mut(&obj_id) {
            o.refs += 1;
        }
        let h = self.handle_table.allocate(
            Self::sync_handle_type(kind),
            GENERIC_ALL,
            Some(String::from(name)),
        );
        self.handle_to_sync.insert(h, obj_id);
        Some(h)
    }

    pub fn sync_object(&self, handle: u64) -> Option<&SyncObject> {
        let obj_id = self.handle_to_sync.get(&handle)?;
        self.sync_objects.get(obj_id)
    }

    pub fn sync_object_mut(&mut self, handle: u64) -> Option<&mut SyncObject> {
        let obj_id = *self.handle_to_sync.get(&handle)?;
        self.sync_objects.get_mut(&obj_id)
    }

    /// Release one handle's reference to a sync object. Returns true if the
    /// handle referenced a sync object (so `CloseHandle` knows it handled it).
    /// Frees the underlying object (and its name) when the last handle closes.
    pub fn close_sync_handle(&mut self, handle: u64) -> bool {
        let obj_id = match self.handle_to_sync.remove(&handle) {
            Some(id) => id,
            None => return false,
        };
        let drop_obj = if let Some(o) = self.sync_objects.get_mut(&obj_id) {
            o.refs = o.refs.saturating_sub(1);
            o.refs == 0
        } else {
            false
        };
        if drop_obj {
            if let Some(o) = self.sync_objects.remove(&obj_id) {
                if let Some(name) = o.name {
                    self.named_sync.remove(&name);
                }
            }
        }
        true
    }
}

/// Conventional base address of the main executable's image (the `None`
/// argument to `GetModuleHandle`). Matches `kernel32::get_module_handle_w`.
pub const MAIN_MODULE_BASE: u64 = 0x0040_0000;
/// Synthetic load base for `kernel32.dll` in the seeded module list. The
/// value is opaque to the guest — it only ever flows back into
/// `GetProcAddress`, which maps it to the DLL name and then to a real shim.
pub const KERNEL32_MODULE_BASE: u64 = 0x7FFF_0001_0000;
/// Synthetic load base for `ntdll.dll`.
pub const NTDLL_MODULE_BASE: u64 = 0x7FFF_0002_0000;

// ---------------------------------------------------------------------------
// Error translation: AthenaOS → Win32
// ---------------------------------------------------------------------------

pub use pe_loader::{raeen_error_to_win32, RaeenOsError};

/// Convenience: set `last_error` on a session from a AthenaOS error.
pub fn set_last_error_from_raeen(ctx: &mut FullCompatSession, err: &pe_loader::RaeenOsError) {
    ctx.last_error = pe_loader::raeen_error_to_win32(err);
}

/// Boot smoketest for the synchronization-object model (the in-process half of
/// the `raebridge_server` broker — `docs/components/raebridge-wine-strategy.md`
/// §6.1). Builds a throwaway session and drives the mutex / event / semaphore
/// state machines through their load-bearing transitions, returning `false` on
/// any wrong result. FAIL-able by construction: each `&&` folds in a real,
/// observed `WAIT_*`/last-error code, so a regression in the state machine
/// flips the boot marker to FAIL (Concept §Compatibility: "apps run naturally"
/// — a multi-threaded app and Steam itself need these to actually block/share).
pub fn run_sync_self_test() -> bool {
    let exe = testpe::build_exit_process_exe();
    let mut ctx = match FullCompatSession::new(
        SessionId(0x5C),
        String::from("sync-selftest.exe"),
        exe,
        String::from("sync-selftest.exe"),
    ) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let mut ok = true;

    // Mutex: thread 1 acquires, thread 2 contends + cannot release, thread 1
    // releases, thread 2 then takes it.
    ctx.current_thread_id = 1;
    let m = kernel32::create_mutex_w(&mut ctx, 0, WinBool(0), None);
    ok &= m != NULL_HANDLE;
    ok &= kernel32::wait_for_single_object(&mut ctx, m, 0) == WAIT_OBJECT_0;
    ctx.current_thread_id = 2;
    ok &= kernel32::wait_for_single_object(&mut ctx, m, 0) == WAIT_TIMEOUT;
    ok &= !kernel32::release_mutex(&mut ctx, m).is_true(); // non-owner -> FALSE
    ctx.current_thread_id = 1;
    ok &= kernel32::release_mutex(&mut ctx, m).is_true(); // owner -> TRUE
    ctx.current_thread_id = 2;
    ok &= kernel32::wait_for_single_object(&mut ctx, m, 0) == WAIT_OBJECT_0;

    // Auto-reset event: a single wait consumes the signal.
    let e = kernel32::create_event_w(&mut ctx, 0, WinBool(0), WinBool(1), None);
    ok &= kernel32::wait_for_single_object(&mut ctx, e, 0) == WAIT_OBJECT_0;
    ok &= kernel32::wait_for_single_object(&mut ctx, e, 0) == WAIT_TIMEOUT;

    // Semaphore: one unit, drained, refilled.
    let s = kernel32::create_semaphore_w(&mut ctx, 0, 1, 2, None);
    ok &= kernel32::wait_for_single_object(&mut ctx, s, 0) == WAIT_OBJECT_0;
    ok &= kernel32::wait_for_single_object(&mut ctx, s, 0) == WAIT_TIMEOUT;
    ok &= kernel32::release_semaphore(&mut ctx, s, 1, None).is_true();
    ok &= kernel32::wait_for_single_object(&mut ctx, s, 0) == WAIT_OBJECT_0;

    // Named reopen shares one object and reports ERROR_ALREADY_EXISTS.
    let name: Vec<u16> = "Global\\AthBridgeSelfTest".encode_utf16().collect();
    let n1 = kernel32::create_mutex_w(&mut ctx, 0, WinBool(0), Some(&name));
    ok &= n1 != NULL_HANDLE && ctx.last_error == ERROR_SUCCESS;
    let _n2 = kernel32::create_mutex_w(&mut ctx, 0, WinBool(0), Some(&name));
    ok &= ctx.last_error == ERROR_ALREADY_EXISTS;

    ok
}

/// Boot smoketest for the advapi32 registry thunks (`RegOpenKeyExW` &c → the
/// real versioned-config-backed hive — Phase A.3,
/// `docs/components/raebridge-wine-strategy.md` §7). Builds a throwaway session
/// and round-trips a value through the *thunk* layer (create → set → query →
/// enumerate), then confirms the thunks are actually reachable from the IAT
/// shim table. FAIL-able: any wrong return code or a missing shim flips it.
pub fn run_registry_thunk_self_test() -> bool {
    let exe = testpe::build_exit_process_exe();
    let mut ctx = match FullCompatSession::new(
        SessionId(0x5D),
        String::from("reg-selftest.exe"),
        exe,
        String::from("reg-selftest.exe"),
    ) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let mut ok = true;

    let mut hk = 0u64;
    let mut disp = 0u32;
    ok &= advapi32::reg_create_key_ex_w(
        &mut ctx,
        advapi32::HKEY_CURRENT_USER,
        "Software\\AthBridge\\SelfTest",
        0,
        None,
        0,
        0xF003F,
        0,
        &mut hk,
        &mut disp,
    ) == 0;
    ok &= hk != 0;

    let data = 0xDEAD_BEEFu32.to_le_bytes();
    ok &= advapi32::reg_set_value_ex_w(&mut ctx, hk, "Magic", 0, advapi32::REG_DWORD, &data) == 0;

    let mut ty = 0u32;
    let mut buf = [0u8; 4];
    let mut size = 4u32;
    ok &=
        advapi32::reg_query_value_ex_w(&mut ctx, hk, "Magic", 0, &mut ty, &mut buf, &mut size) == 0;
    ok &= ty == advapi32::REG_DWORD && u32::from_le_bytes(buf) == 0xDEAD_BEEF;

    // Enumerate values and confirm "Magic" appears. (reg_create seeds an empty
    // default value to mark key existence, so it is not necessarily index 0.)
    let mut found_magic = false;
    for idx in 0..16u32 {
        let mut name_buf = [0u16; 64];
        let mut name_size = 64u32;
        let mut ety = 0u32;
        let mut edata = [0u8; 16];
        let mut edata_size = 16u32;
        let r = advapi32::reg_enum_value_w(
            &mut ctx,
            hk,
            idx,
            &mut name_buf,
            &mut name_size,
            &mut ety,
            &mut edata,
            &mut edata_size,
        );
        if r != 0 {
            break; // ERROR_NO_MORE_ITEMS
        }
        if wide_to_string(&name_buf[..name_size as usize]) == "Magic" {
            found_magic = true;
        }
    }
    ok &= found_magic;

    let _ = advapi32::reg_close_key(&mut ctx, hk);

    // The thunks must be reachable from the IAT shim table.
    for name in [
        "RegOpenKeyExW",
        "RegQueryValueExW",
        "RegSetValueExW",
        "RegCloseKey",
    ] {
        ok &= winapi_shims::resolve_shim("advapi32.dll", name).is_some();
    }

    ok
}

/// One-line snapshot of the registry-thunk layer for `/proc/raeen/*`.
pub fn registry_thunk_self_test_text() -> String {
    let ok = run_registry_thunk_self_test();
    let mut s = String::new();
    let _ = core::fmt::Write::write_fmt(
        &mut s,
        format_args!(
            "AthBridge advapi32 registry thunks (Phase A.3)\nself_test: {}\n\
             thunks: RegOpen/Create/Close/Query/Set/Delete/Enum/QueryInfo/Flush (W+A)\n\
             backing: ctx.registry hive -> win_registry versioned config (snapshot/rollback)\n\
             exposed: IAT shim table under advapi32.dll (was fail-loud trampoline)\n",
            if ok { "PASS" } else { "FAIL" }
        ),
    );
    s
}

/// One-line snapshot of the synchronization-object model for `/proc/raeen/*`.
pub fn sync_self_test_text() -> String {
    let ok = run_sync_self_test();
    let mut s = String::new();
    let _ = core::fmt::Write::write_fmt(
        &mut s,
        format_args!(
            "AthBridge sync objects (broker §6.1, in-process)\nself_test: {}\n\
             objects: mutex(owner+recursion) event(manual/auto) semaphore(count/max)\n\
             namespace: named create/open + ERROR_ALREADY_EXISTS, refcounted close\n\
             waits: WaitForSingleObject + WaitForMultipleObjects (any/all)\n\
             note: cross-process + true blocking = raebridge_server daemon (slice 2)\n",
            if ok { "PASS" } else { "FAIL" }
        ),
    );
    s
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    #[test]
    fn compat_context_stack_footprint() {
        // MasterChecklist "Latent kernel bugs": the boot thunk smoketest from the
        // BSP double-faulted — a `CompatContext` (= FullCompatSession) constructed
        // by value on the boot stack. This test pins the by-value footprint (and
        // prints the per-field breakdown when it fails) so the type can never
        // silently regrow past what a kernel stack construction tolerates.
        let total = core::mem::size_of::<FullCompatSession>();
        let parts = [
            ("CompatSession", core::mem::size_of::<CompatSession>()),
            ("HandleTable", core::mem::size_of::<HandleTable>()),
            ("RegistryHive", core::mem::size_of::<RegistryHive>()),
            ("PeInfo", core::mem::size_of::<PeInfo>()),
            ("WndClassExW", core::mem::size_of::<WndClassExW>()),
        ];
        let breakdown = parts
            .iter()
            .map(|(n, s)| alloc::format!("{n}={s}B"))
            .collect::<alloc::vec::Vec<_>>()
            .join(" ");
        assert!(
            total <= 4096,
            "FullCompatSession is {total}B by value (boot-stack hazard; parts: {breakdown})"
        );
    }

    #[test]
    fn per_app_bucket_isolates_c_drive() {
        // The isolation property: the SAME Windows path for TWO different apps
        // maps to DIFFERENT VFS storage, so app A cannot read app B's C:\ files.
        let pa = app_bucket_vfs_path("appA.exe", "C:\\save.dat");
        let pb = app_bucket_vfs_path("appB.exe", "C:\\save.dat");
        assert_ne!(
            pa, pb,
            "two apps' C:\\save.dat must resolve to different storage"
        );
        // Both under the C: mount; each carries its own bucket id.
        assert!(pa.starts_with("/mnt/win_c/"));
        assert!(pb.starts_with("/mnt/win_c/"));
        assert!(pa.contains(&app_bucket_id("appA.exe")));
        assert!(pb.contains(&app_bucket_id("appB.exe")));
        // Deterministic: the same app always resolves the same path.
        assert_eq!(pa, app_bucket_vfs_path("appA.exe", "C:\\save.dat"));
    }

    #[test]
    fn ctx_win_path_to_vfs_uses_session_bucket() {
        // The method both CreateFileW and NtCreateFile route C:\ opens through:
        // it must use THIS session's bucket, so two sessions isolate their C:\.
        let exe = crate::testpe::build_exit_process_exe();
        let a = FullCompatSession::new(
            SessionId(1),
            "appA.exe".to_string(),
            exe.clone(),
            "appA.exe".to_string(),
        )
        .expect("session A");
        let b = FullCompatSession::new(
            SessionId(2),
            "appB.exe".to_string(),
            exe,
            "appB.exe".to_string(),
        )
        .expect("session B");
        let pa = a.win_path_to_vfs("C:\\save.dat");
        assert_eq!(pa, app_bucket_vfs_path("appA.exe", "C:\\save.dat"));
        assert_ne!(
            pa,
            b.win_path_to_vfs("C:\\save.dat"),
            "two sessions must isolate C:\\"
        );
    }

    #[test]
    fn app_bucket_id_stable_distinct_and_fs_safe() {
        let a = app_bucket_id("notepad.exe");
        assert_eq!(a, app_bucket_id("notepad.exe"), "same app -> stable");
        assert_ne!(a, app_bucket_id("calc.exe"), "distinct apps -> distinct");
        // Filesystem-safe: no separators that would escape the bucket subtree.
        assert!(!a.contains('/') && !a.contains('\\') && !a.contains(':'));
    }

    #[test]
    fn bucketed_path_preserves_filename_and_falls_through() {
        let p = app_bucket_vfs_path("notepad.exe", "C:\\note.txt");
        assert!(p.starts_with("/mnt/win_c/"));
        assert!(p.ends_with("/note.txt"), "filename preserved: {p}");
        // A non-drive path is unchanged by bucketing.
        assert_eq!(
            translate_win_path_bucketed("relative/file.txt", "bkt"),
            translate_win_path("relative/file.txt")
        );
    }

    /// Construct a minimal valid PE image for testing.
    fn make_minimal_pe(
        machine: u16,
        num_sections: u16,
        entry_rva: u32,
        pe32_plus: bool,
    ) -> Vec<u8> {
        let pe_offset: u32 = 0x80; // place PE header at offset 0x80
        let total_size = pe_offset as usize + 4 + 20 + 28; // sig + COFF + enough optional
        let mut buf = vec![0u8; total_size];

        // DOS header
        buf[0] = 0x4D; // 'M'
        buf[1] = 0x5A; // 'Z'
                       // e_lfanew at 0x3C
        let offset_bytes = pe_offset.to_le_bytes();
        buf[0x3C..0x40].copy_from_slice(&offset_bytes);

        let pe_off = pe_offset as usize;
        // PE signature
        buf[pe_off] = b'P';
        buf[pe_off + 1] = b'E';
        buf[pe_off + 2] = 0;
        buf[pe_off + 3] = 0;

        // COFF header
        let coff = pe_off + 4;
        buf[coff..coff + 2].copy_from_slice(&machine.to_le_bytes());
        buf[coff + 2..coff + 4].copy_from_slice(&num_sections.to_le_bytes());

        // Optional header
        let opt = coff + 20;
        let magic: u16 = if pe32_plus { 0x020B } else { 0x010B };
        buf[opt..opt + 2].copy_from_slice(&magic.to_le_bytes());
        // Entry point RVA at opt + 16
        buf[opt + 16..opt + 20].copy_from_slice(&entry_rva.to_le_bytes());

        buf
    }

    #[test]
    fn test_load_pe_amd64() {
        let image = make_minimal_pe(0x8664, 4, 0x1000, true);
        let info = load_pe(&image).unwrap();
        assert_eq!(info.machine, MachineType::Amd64);
        assert_eq!(info.format, PeFormat::Pe32Plus);
        assert_eq!(info.num_sections, 4);
        assert_eq!(info.entry_point_rva, 0x1000);
    }

    #[test]
    fn test_load_pe_i386() {
        let image = make_minimal_pe(0x014C, 3, 0x2000, false);
        let info = load_pe(&image).unwrap();
        assert_eq!(info.machine, MachineType::I386);
        assert_eq!(info.format, PeFormat::Pe32);
        assert_eq!(info.num_sections, 3);
        assert_eq!(info.entry_point_rva, 0x2000);
    }

    #[test]
    fn test_invalid_dos_magic() {
        let mut image = make_minimal_pe(0x8664, 1, 0, true);
        image[0] = 0x00;
        let err = load_pe(&image).unwrap_err();
        assert_eq!(err, BridgeError::InvalidDosSignature([0x00, 0x5A]));
    }

    #[test]
    fn test_invalid_pe_signature() {
        let mut image = make_minimal_pe(0x8664, 1, 0, true);
        let pe_off = read_u32_le(&image, 0x3C) as usize;
        image[pe_off] = 0xFF;
        let err = load_pe(&image).unwrap_err();
        assert!(matches!(err, BridgeError::InvalidPeSignature(_)));
    }

    #[test]
    fn test_buffer_too_small() {
        let buf = [0x4D, 0x5A]; // only 2 bytes
        let err = load_pe(&buf).unwrap_err();
        assert!(matches!(err, BridgeError::BufferTooSmall { .. }));
    }

    #[test]
    fn test_std_handles_seeded_with_native_fds() {
        // A freshly constructed session must hand the three standard handle
        // values back through GetStdHandle, and each must carry the host's
        // native fd (0/1/2) so WriteFile/ReadFile to a console handle reaches
        // the kernel rather than failing INVALID_HANDLE. This is the contract
        // every console Windows app relies on after GetStdHandle.
        let exe = testpe::build_exit_process_exe();
        let mut ctx =
            FullCompatSession::new(SessionId(7), "t.exe".to_string(), exe, "t.exe".to_string())
                .unwrap();

        let h_out = kernel32::get_std_handle(&mut ctx, STD_OUTPUT_HANDLE);
        let h_in = kernel32::get_std_handle(&mut ctx, STD_INPUT_HANDLE);
        let h_err = kernel32::get_std_handle(&mut ctx, STD_ERROR_HANDLE);
        assert_eq!(h_out.0, 8);
        assert_eq!(h_in.0, 4);
        assert_eq!(h_err.0, 12);

        assert_eq!(ctx.handle_table.get(h_in.0).unwrap().native_id, Some(0));
        assert_eq!(ctx.handle_table.get(h_out.0).unwrap().native_id, Some(1));
        assert_eq!(ctx.handle_table.get(h_err.0).unwrap().native_id, Some(2));

        // An unknown selector is an error, not a silent valid handle.
        let bad = kernel32::get_std_handle(&mut ctx, 0x1234);
        assert_eq!(bad, INVALID_HANDLE_VALUE);
    }

    #[test]
    fn test_get_std_handle_is_in_shim_table() {
        // The IAT-patch table must expose GetStdHandle to a distinct, non-null
        // shim address, or console apps silently get a fail-loud stub.
        assert!(winapi_shims::resolve_shim("kernel32.dll", "GetStdHandle").is_some());
        assert!(winapi_shims::resolve_shim("KERNEL32.DLL", "GetStdHandle").is_some());
        let (len, verified) = winapi_shims::shim_selftest();
        assert_eq!(
            len, verified,
            "every shim entry must be distinct + resolvable"
        );
    }

    #[test]
    fn test_compat_session_lifecycle() {
        let image = make_minimal_pe(0x8664, 2, 0x3000, true);
        let mut session = CompatSession::new(SessionId(1), "test.exe".to_string(), image).unwrap();

        assert_eq!(session.state, SessionState::Loaded);
        session.start().unwrap();
        assert_eq!(session.state, SessionState::Running);
        session.suspend().unwrap();
        assert_eq!(session.state, SessionState::Suspended);
        session.start().unwrap();
        assert_eq!(session.state, SessionState::Running);
        session.terminate(0);
        assert_eq!(session.state, SessionState::Terminated(0));
    }

    // -----------------------------------------------------------------------
    // Synchronization-object state machine (broker §6.1, in-process half)
    // -----------------------------------------------------------------------

    fn sync_ctx() -> FullCompatSession {
        let exe = testpe::build_exit_process_exe();
        FullCompatSession::new(
            SessionId(9),
            "sync.exe".to_string(),
            exe,
            "sync.exe".to_string(),
        )
        .unwrap()
    }

    fn wname(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn test_mutex_acquire_release_recursion() {
        let mut ctx = sync_ctx();
        ctx.current_thread_id = 100;
        // Unowned mutex starts free -> first wait acquires it.
        let h = kernel32::create_mutex_w(&mut ctx, 0, FALSE, None);
        assert_ne!(h, NULL_HANDLE);
        assert_eq!(ctx.last_error, ERROR_SUCCESS);
        assert_eq!(
            kernel32::wait_for_single_object(&mut ctx, h, 0),
            WAIT_OBJECT_0
        );
        // Recursive re-acquire by the same thread succeeds (depth 2).
        assert_eq!(
            kernel32::wait_for_single_object(&mut ctx, h, 0),
            WAIT_OBJECT_0
        );
        // Another thread cannot take it while held.
        ctx.current_thread_id = 200;
        assert_eq!(
            kernel32::wait_for_single_object(&mut ctx, h, 0),
            WAIT_TIMEOUT
        );
        // A non-owner release is rejected.
        assert_eq!(kernel32::release_mutex(&mut ctx, h), FALSE);
        assert_eq!(ctx.last_error, ERROR_NOT_OWNER);
        // Owner must release twice (recursion) before it frees.
        ctx.current_thread_id = 100;
        assert_eq!(kernel32::release_mutex(&mut ctx, h), TRUE);
        ctx.current_thread_id = 200;
        assert_eq!(
            kernel32::wait_for_single_object(&mut ctx, h, 0),
            WAIT_TIMEOUT
        );
        ctx.current_thread_id = 100;
        assert_eq!(kernel32::release_mutex(&mut ctx, h), TRUE);
        // Now free -> the other thread can take it.
        ctx.current_thread_id = 200;
        assert_eq!(
            kernel32::wait_for_single_object(&mut ctx, h, 0),
            WAIT_OBJECT_0
        );
    }

    #[test]
    fn test_mutex_initial_owner() {
        let mut ctx = sync_ctx();
        ctx.current_thread_id = 100;
        let h = kernel32::create_mutex_w(&mut ctx, 0, TRUE, None);
        // Created already owned by the creating thread -> another thread waits.
        ctx.current_thread_id = 200;
        assert_eq!(
            kernel32::wait_for_single_object(&mut ctx, h, 0),
            WAIT_TIMEOUT
        );
        ctx.current_thread_id = 100;
        assert_eq!(kernel32::release_mutex(&mut ctx, h), TRUE);
        ctx.current_thread_id = 200;
        assert_eq!(
            kernel32::wait_for_single_object(&mut ctx, h, 0),
            WAIT_OBJECT_0
        );
    }

    #[test]
    fn test_event_manual_and_auto_reset() {
        let mut ctx = sync_ctx();
        // Manual-reset event, initially non-signaled.
        let man = kernel32::create_event_w(&mut ctx, 0, TRUE, FALSE, None);
        assert_eq!(
            kernel32::wait_for_single_object(&mut ctx, man, 0),
            WAIT_TIMEOUT
        );
        assert_eq!(kernel32::set_event(&mut ctx, man), TRUE);
        // Manual-reset stays signaled across multiple waits.
        assert_eq!(
            kernel32::wait_for_single_object(&mut ctx, man, 0),
            WAIT_OBJECT_0
        );
        assert_eq!(
            kernel32::wait_for_single_object(&mut ctx, man, 0),
            WAIT_OBJECT_0
        );
        assert_eq!(kernel32::reset_event(&mut ctx, man), TRUE);
        assert_eq!(
            kernel32::wait_for_single_object(&mut ctx, man, 0),
            WAIT_TIMEOUT
        );

        // Auto-reset event, initially signaled -> one wait consumes the signal.
        let auto = kernel32::create_event_w(&mut ctx, 0, FALSE, TRUE, None);
        assert_eq!(
            kernel32::wait_for_single_object(&mut ctx, auto, 0),
            WAIT_OBJECT_0
        );
        assert_eq!(
            kernel32::wait_for_single_object(&mut ctx, auto, 0),
            WAIT_TIMEOUT
        );
    }

    #[test]
    fn test_semaphore_count() {
        let mut ctx = sync_ctx();
        let h = kernel32::create_semaphore_w(&mut ctx, 0, 2, 3, None);
        assert_ne!(h, NULL_HANDLE);
        // Two units available.
        assert_eq!(
            kernel32::wait_for_single_object(&mut ctx, h, 0),
            WAIT_OBJECT_0
        );
        assert_eq!(
            kernel32::wait_for_single_object(&mut ctx, h, 0),
            WAIT_OBJECT_0
        );
        // Drained.
        assert_eq!(
            kernel32::wait_for_single_object(&mut ctx, h, 0),
            WAIT_TIMEOUT
        );
        // Release 2 back; previous count was 0.
        let mut prev = -1i32;
        assert_eq!(
            kernel32::release_semaphore(&mut ctx, h, 2, Some(&mut prev)),
            TRUE
        );
        assert_eq!(prev, 0);
        // Releasing past max fails with ERROR_TOO_MANY_POSTS.
        assert_eq!(kernel32::release_semaphore(&mut ctx, h, 2, None), FALSE);
        assert_eq!(ctx.last_error, ERROR_TOO_MANY_POSTS);
    }

    #[test]
    fn test_named_object_reopen_and_close() {
        let mut ctx = sync_ctx();
        let name = wname("Global\\AthBridgeTestMutex");
        let h1 = kernel32::create_mutex_w(&mut ctx, 0, FALSE, Some(&name));
        assert_eq!(ctx.last_error, ERROR_SUCCESS);
        // Second create with the same name returns a DISTINCT handle to the
        // SAME object and sets ERROR_ALREADY_EXISTS.
        let h2 = kernel32::create_mutex_w(&mut ctx, 0, FALSE, Some(&name));
        assert_ne!(h1.0, h2.0);
        assert_eq!(ctx.last_error, ERROR_ALREADY_EXISTS);
        // They share state: acquire via h1, observe held via h2.
        ctx.current_thread_id = 1;
        assert_eq!(
            kernel32::wait_for_single_object(&mut ctx, h1, 0),
            WAIT_OBJECT_0
        );
        ctx.current_thread_id = 2;
        assert_eq!(
            kernel32::wait_for_single_object(&mut ctx, h2, 0),
            WAIT_TIMEOUT
        );
        // OpenMutexW finds the existing named object.
        let h3 = kernel32::open_mutex_w(&mut ctx, 0, FALSE, Some(&name));
        assert_ne!(h3, NULL_HANDLE);
        // Closing all handles frees the object; OpenMutexW then fails.
        assert_eq!(kernel32::close_handle(&mut ctx, h1), TRUE);
        assert_eq!(kernel32::close_handle(&mut ctx, h2), TRUE);
        assert_eq!(kernel32::close_handle(&mut ctx, h3), TRUE);
        let h4 = kernel32::open_mutex_w(&mut ctx, 0, FALSE, Some(&name));
        assert_eq!(h4, NULL_HANDLE);
        assert_eq!(ctx.last_error, ERROR_FILE_NOT_FOUND);
    }

    #[test]
    fn test_named_type_mismatch() {
        let mut ctx = sync_ctx();
        let name = wname("Local\\Dup");
        let _ev = kernel32::create_event_w(&mut ctx, 0, TRUE, FALSE, Some(&name));
        // Creating a mutex with a name already owned by an event fails.
        let m = kernel32::create_mutex_w(&mut ctx, 0, FALSE, Some(&name));
        assert_eq!(m, NULL_HANDLE);
        assert_eq!(ctx.last_error, ERROR_INVALID_HANDLE);
    }

    #[test]
    fn test_wait_multiple_any_and_all() {
        let mut ctx = sync_ctx();
        let e0 = kernel32::create_event_w(&mut ctx, 0, TRUE, FALSE, None);
        let e1 = kernel32::create_event_w(&mut ctx, 0, TRUE, TRUE, None);
        // wait-any returns the index of the first signaled object (index 1).
        let r = kernel32::wait_for_multiple_objects(&mut ctx, &[e0, e1], FALSE, 0);
        assert_eq!(r, WAIT_OBJECT_0 + 1);
        // wait-all: not all signaled yet (e0 still reset) -> timeout, no acquire.
        assert_eq!(
            kernel32::wait_for_multiple_objects(&mut ctx, &[e0, e1], TRUE, 0),
            WAIT_TIMEOUT
        );
        kernel32::set_event(&mut ctx, e0);
        assert_eq!(
            kernel32::wait_for_multiple_objects(&mut ctx, &[e0, e1], TRUE, 0),
            WAIT_OBJECT_0
        );
    }

    // -----------------------------------------------------------------------
    // advapi32 registry thunks -> versioned-config-backed hive (Phase A.3)
    // -----------------------------------------------------------------------

    #[test]
    fn test_registry_thunk_dword_roundtrip() {
        use advapi32 as adv;
        let mut ctx = sync_ctx();
        // Create HKCU\Software\AthBridgeTest, set a DWORD, read it back.
        let mut hk = 0u64;
        let mut disp = 0u32;
        let r = adv::reg_create_key_ex_w(
            &mut ctx,
            adv::HKEY_CURRENT_USER,
            "Software\\AthBridgeTest",
            0,
            None,
            0,
            0xF003F,
            0,
            &mut hk,
            &mut disp,
        );
        assert_eq!(r, 0);
        assert_ne!(hk, 0);
        let data = 0x1234_5678u32.to_le_bytes();
        assert_eq!(
            adv::reg_set_value_ex_w(&mut ctx, hk, "Answer", 0, adv::REG_DWORD, &data),
            0
        );
        let mut ty = 0u32;
        let mut buf = [0u8; 4];
        let mut size = 4u32;
        assert_eq!(
            adv::reg_query_value_ex_w(&mut ctx, hk, "Answer", 0, &mut ty, &mut buf, &mut size),
            0
        );
        assert_eq!(ty, adv::REG_DWORD);
        assert_eq!(u32::from_le_bytes(buf), 0x1234_5678);
        assert_eq!(size, 4);

        // RegEnumValueW finds "Answer" among the key's values (reg_create seeds
        // an empty default value, so scan rather than assume index 0).
        let mut found = false;
        for idx in 0..16u32 {
            let mut nb = [0u16; 64];
            let mut ns = 64u32;
            let mut ety = 0u32;
            let mut eb = [0u8; 16];
            let mut es = 16u32;
            if adv::reg_enum_value_w(
                &mut ctx, hk, idx, &mut nb, &mut ns, &mut ety, &mut eb, &mut es,
            ) != 0
            {
                break;
            }
            if wide_to_string(&nb[..ns as usize]) == "Answer" {
                found = true;
                assert_eq!(ety, adv::REG_DWORD);
            }
        }
        assert!(found, "RegEnumValueW did not return the 'Answer' value");

        assert_eq!(adv::reg_close_key(&mut ctx, hk), 0);
    }

    #[test]
    fn test_registry_thunk_string_and_size_query() {
        use advapi32 as adv;
        let mut ctx = sync_ctx();
        let mut hk = 0u64;
        let mut disp = 0u32;
        adv::reg_create_key_ex_w(
            &mut ctx,
            adv::HKEY_LOCAL_MACHINE,
            "SOFTWARE\\Rae",
            0,
            None,
            0,
            0xF003F,
            0,
            &mut hk,
            &mut disp,
        );
        // Store a REG_SZ value (UTF-16 LE + NUL).
        let s: Vec<u16> = "hello".encode_utf16().chain(core::iter::once(0)).collect();
        let bytes: Vec<u8> = s.iter().flat_map(|w| w.to_le_bytes()).collect();
        assert_eq!(
            adv::reg_set_value_ex_w(&mut ctx, hk, "Greeting", 0, adv::REG_SZ, &bytes),
            0
        );
        // Size query: a zero-length buffer reports ERROR_MORE_DATA + needed size.
        let mut ty = 0u32;
        let mut size = 0u32;
        let r = adv::reg_query_value_ex_w(&mut ctx, hk, "Greeting", 0, &mut ty, &mut [], &mut size);
        assert_eq!(r, ERROR_MORE_DATA as i32);
        assert_eq!(size as usize, bytes.len());
        // Real read with a big-enough buffer returns the exact bytes.
        let mut buf = alloc::vec![0u8; size as usize];
        assert_eq!(
            adv::reg_query_value_ex_w(&mut ctx, hk, "Greeting", 0, &mut ty, &mut buf, &mut size),
            0
        );
        assert_eq!(ty, adv::REG_SZ);
        assert_eq!(buf, bytes);
        // A missing value is ERROR_FILE_NOT_FOUND, not a silent success.
        let mut s2 = 0u32;
        assert_eq!(
            adv::reg_query_value_ex_w(&mut ctx, hk, "Nope", 0, &mut ty, &mut [], &mut s2),
            ERROR_FILE_NOT_FOUND as i32
        );
    }

    #[test]
    fn test_registry_thunks_in_table() {
        for name in [
            "RegOpenKeyExW",
            "RegCreateKeyExW",
            "RegQueryValueExW",
            "RegSetValueExW",
            "RegCloseKey",
            "RegEnumKeyExW",
            "RegEnumValueW",
            "RegQueryInfoKeyW",
            "RegDeleteValueW",
        ] {
            assert!(
                winapi_shims::resolve_shim("advapi32.dll", name).is_some(),
                "{name} missing from advapi32 shim table"
            );
            // DLL match is case-insensitive (PE descriptors mix cases).
            assert!(winapi_shims::resolve_shim("ADVAPI32.dll", name).is_some());
        }
    }

    #[test]
    fn test_sync_shims_in_table() {
        // Each sync export must resolve to a real, distinct shim address, or a
        // guest importing it silently hits the fail-loud trampoline.
        for name in [
            "CreateMutexW",
            "ReleaseMutex",
            "CreateEventW",
            "SetEvent",
            "ResetEvent",
            "CreateSemaphoreW",
            "ReleaseSemaphore",
            "WaitForSingleObject",
            "WaitForMultipleObjects",
            "OpenMutexW",
        ] {
            assert!(
                winapi_shims::resolve_shim("kernel32.dll", name).is_some(),
                "{name} missing from shim table"
            );
        }
        let (len, verified) = winapi_shims::shim_selftest();
        assert_eq!(
            len, verified,
            "every shim entry must be distinct + resolvable"
        );
    }

    // Regression fence for the integrated boot self-tests. The individual
    // thunks pass above, but the iron sweep (2026-06-27) caught the INTEGRATED
    // `run_registry_thunk_self_test` failing at boot while host stayed green —
    // because nothing exercised the whole-self-test path on the host. These two
    // tests close that gap: a regression in either boot self-test now fails on
    // the dev box, not only on Athena.
    #[test]
    fn boot_sync_self_test_passes() {
        assert!(run_sync_self_test(), "in-process sync self-test regressed");
    }

    #[test]
    fn boot_registry_thunk_self_test_passes() {
        assert!(
            run_registry_thunk_self_test(),
            "advapi32 registry thunk self-test regressed (iron 2026-06-27 FAIL)"
        );
    }
}
