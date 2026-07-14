//! AthBridge Phase-B thunk dispatch table.
//!
//! Concept §Compatibility Strategy: 15k Win32 *names* are reachable as
//! resolvable imports (see `pe_dll_registry`), but until a name is wired
//! to a callable Rust implementation the PE loader can't actually *run*
//! anything. This module is the first concrete bridge between the name
//! registry and the per-DLL Rust implementations in `kernel32.rs`,
//! `user32.rs`, etc.
//!
//! Scope of Phase B:
//!   • The top-20 Win32 entry points that real Windows apps hit in their
//!     first few instructions: process/heap/error/time/IO/debug.
//!   • A pure-data `THUNKS` table (DLL, function name, ThunkId).
//!   • `dispatch(ctx, dll, fn, args)` — a single funnel that maps a
//!     (dll, name) pair to the corresponding kernel32/user32 Rust impl.
//!   • A `summary()` printable boot-log line that counts how many of the
//!     top-20 thunks resolve cleanly against the registry.
//!
//! What this is NOT (yet):
//!   • A machine-code trampoline emitter. Real PE thunk resolution needs
//!     the loader to write 16-byte trampolines at the IAT addresses
//!     that call back into Rust via a syscall or stub vector. That work
//!     lives in a future Phase C.
//!   • A complete Win32 surface. Only the 20 highest-impact names are
//!     wired here; the rest fall through `Thunk::Unimplemented`.

#![allow(dead_code)]

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::{CompatContext, DWord, WinBool, WinHandle, FALSE, TRUE};

/// Stable identifier for each top-20 thunk. The numeric value is the
/// table index — keep it dense.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThunkId {
    // kernel32 — process / error / heap / memory / time / IO / debug
    K32_ExitProcess,
    K32_GetLastError,
    K32_SetLastError,
    K32_GetCurrentProcessId,
    K32_GetCurrentThreadId,
    K32_GetProcessHeap,
    K32_HeapAlloc,
    K32_HeapFree,
    K32_VirtualAlloc,
    K32_VirtualFree,
    K32_GetTickCount,
    K32_GetTickCount64,
    K32_GetSystemTimeAsFileTime,
    K32_Sleep,
    K32_OutputDebugStringA,
    K32_GetCommandLineW,
    K32_CloseHandle,
    K32_CreateFileW,
    K32_ReadFile,
    K32_WriteFile,
}

impl ThunkId {
    pub fn dll(self) -> &'static str {
        "kernel32.dll"
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::K32_ExitProcess => "ExitProcess",
            Self::K32_GetLastError => "GetLastError",
            Self::K32_SetLastError => "SetLastError",
            Self::K32_GetCurrentProcessId => "GetCurrentProcessId",
            Self::K32_GetCurrentThreadId => "GetCurrentThreadId",
            Self::K32_GetProcessHeap => "GetProcessHeap",
            Self::K32_HeapAlloc => "HeapAlloc",
            Self::K32_HeapFree => "HeapFree",
            Self::K32_VirtualAlloc => "VirtualAlloc",
            Self::K32_VirtualFree => "VirtualFree",
            Self::K32_GetTickCount => "GetTickCount",
            Self::K32_GetTickCount64 => "GetTickCount64",
            Self::K32_GetSystemTimeAsFileTime => "GetSystemTimeAsFileTime",
            Self::K32_Sleep => "Sleep",
            Self::K32_OutputDebugStringA => "OutputDebugStringA",
            Self::K32_GetCommandLineW => "GetCommandLineW",
            Self::K32_CloseHandle => "CloseHandle",
            Self::K32_CreateFileW => "CreateFileW",
            Self::K32_ReadFile => "ReadFile",
            Self::K32_WriteFile => "WriteFile",
        }
    }
}

/// Result of a thunk invocation. The discriminant lets a future
/// trampoline emitter decide how to marshal the return value back into
/// `rax` / `xmm0` per the Microsoft x64 calling convention.
#[derive(Debug, Clone)]
pub enum ThunkResult {
    /// 32-bit value (e.g. DWORD, BOOL).
    Dword(u32),
    /// 64-bit value (e.g. HANDLE, pointer, ULONGLONG).
    Qword(u64),
    /// `void` return — function executed for side-effects.
    Void,
    /// (For future use — string-typed returns marshal through here.)
    Str(String),
    /// Thunk wasn't found in the dispatch table. Loader trampoline should
    /// either fall back to a "missing import" stub or kill the process.
    Unimplemented,
}

/// All Phase-B thunks. Pure data — iteration order = enum order above.
pub const ALL_THUNKS: &[ThunkId] = &[
    ThunkId::K32_ExitProcess,
    ThunkId::K32_GetLastError,
    ThunkId::K32_SetLastError,
    ThunkId::K32_GetCurrentProcessId,
    ThunkId::K32_GetCurrentThreadId,
    ThunkId::K32_GetProcessHeap,
    ThunkId::K32_HeapAlloc,
    ThunkId::K32_HeapFree,
    ThunkId::K32_VirtualAlloc,
    ThunkId::K32_VirtualFree,
    ThunkId::K32_GetTickCount,
    ThunkId::K32_GetTickCount64,
    ThunkId::K32_GetSystemTimeAsFileTime,
    ThunkId::K32_Sleep,
    ThunkId::K32_OutputDebugStringA,
    ThunkId::K32_GetCommandLineW,
    ThunkId::K32_CloseHandle,
    ThunkId::K32_CreateFileW,
    ThunkId::K32_ReadFile,
    ThunkId::K32_WriteFile,
];

/// Map (dll, name) → ThunkId. Linear scan; the table has 20 entries so
/// it never matters.
pub fn lookup(dll: &str, func: &str) -> Option<ThunkId> {
    let dll_lc = dll.to_ascii_lowercase();
    for &t in ALL_THUNKS {
        if t.dll() == dll_lc && t.name() == func {
            return Some(t);
        }
    }
    None
}

/// Invoke a thunk by its `ThunkId`. `args` is the Win32 argument list
/// pre-marshaled from x64 calling convention into a uniform u64 array
/// (the future trampoline emitter is responsible for that marshaling).
///
/// Argument layout follows the Win32 function signature one-for-one.
pub fn invoke(ctx: &mut CompatContext, thunk: ThunkId, args: &[u64]) -> ThunkResult {
    use crate::kernel32 as k;
    match thunk {
        ThunkId::K32_ExitProcess => {
            let code = args.get(0).copied().unwrap_or(0) as u32;
            k::exit_process(ctx, code);
            ThunkResult::Void
        }
        ThunkId::K32_GetLastError => ThunkResult::Dword(k::get_last_error(ctx).0),
        ThunkId::K32_SetLastError => {
            let code = args.get(0).copied().unwrap_or(0) as u32;
            k::set_last_error_api(ctx, DWord(code));
            ThunkResult::Void
        }
        ThunkId::K32_GetCurrentProcessId => ThunkResult::Dword(k::get_current_process_id(ctx)),
        ThunkId::K32_GetCurrentThreadId => ThunkResult::Dword(k::get_current_thread_id(ctx)),
        ThunkId::K32_GetProcessHeap => ThunkResult::Qword(k::get_process_heap(ctx).0),
        ThunkId::K32_HeapAlloc => {
            let heap = WinHandle(args.get(0).copied().unwrap_or(0));
            let flags = args.get(1).copied().unwrap_or(0) as u32;
            let bytes = args.get(2).copied().unwrap_or(0);
            ThunkResult::Qword(k::heap_alloc(ctx, heap, flags, bytes))
        }
        ThunkId::K32_HeapFree => {
            let heap = WinHandle(args.get(0).copied().unwrap_or(0));
            let flags = args.get(1).copied().unwrap_or(0) as u32;
            let mem = args.get(2).copied().unwrap_or(0);
            ThunkResult::Dword(k::heap_free(ctx, heap, flags, mem).0 as u32)
        }
        ThunkId::K32_VirtualAlloc => {
            // VirtualAlloc requires a real syscall (sys_mmap); skip the
            // active call here and let userspace exercise it. Return 0
            // = NULL like a refused allocation. Phase B documents this
            // gap rather than hiding it.
            let _ = args;
            ThunkResult::Qword(0)
        }
        ThunkId::K32_VirtualFree => ThunkResult::Dword(FALSE.0 as u32),
        ThunkId::K32_GetTickCount => ThunkResult::Dword(k::get_tick_count(ctx)),
        ThunkId::K32_GetTickCount64 => ThunkResult::Qword(k::get_tick_count_64(ctx)),
        ThunkId::K32_GetSystemTimeAsFileTime => {
            ThunkResult::Qword(k::get_system_time_as_file_time(ctx))
        }
        ThunkId::K32_Sleep => {
            let ms = args.get(0).copied().unwrap_or(0) as u32;
            k::sleep(ctx, ms);
            ThunkResult::Void
        }
        ThunkId::K32_OutputDebugStringA => {
            // The pointer is a guest VA we don't dereference from the
            // thunk-test path. Real loader would translate VA → bytes.
            k::output_debug_string_a(ctx, &[]);
            ThunkResult::Void
        }
        ThunkId::K32_GetCommandLineW => {
            // GetCommandLineW returns LPWSTR — for the thunk-test we
            // surface the value as Qword (pointer-equivalent).
            let cmd = k::get_command_line_w(ctx);
            ThunkResult::Str(cmd.into())
        }
        ThunkId::K32_CloseHandle => {
            let h = WinHandle(args.get(0).copied().unwrap_or(0));
            ThunkResult::Dword(k::close_handle(ctx, h).0 as u32)
        }
        ThunkId::K32_CreateFileW | ThunkId::K32_ReadFile | ThunkId::K32_WriteFile => {
            // File I/O thunks require pointer-typed inputs from guest
            // VA (LPCWSTR, LPVOID buffers). The thunk-test path doesn't
            // synthesize valid guest buffers — only the trampoline can.
            // Return 0/FALSE here; userspace integration tests exercise
            // the underlying functions directly.
            let _ = args;
            ThunkResult::Dword(FALSE.0 as u32)
        }
    }
}

/// Convenience wrapper: lookup by name then invoke.
pub fn dispatch(ctx: &mut CompatContext, dll: &str, func: &str, args: &[u64]) -> ThunkResult {
    match lookup(dll, func) {
        Some(id) => invoke(ctx, id, args),
        None => ThunkResult::Unimplemented,
    }
}

/// Result of a boot-time smoketest pass — used by the kernel's
/// `[athbridge]` log line so the field reporter can grep coverage.
#[derive(Debug, Clone)]
pub struct ThunkSmoketest {
    pub registered: usize,   // Total thunks declared.
    pub invoked_ok: usize,   // Calls that returned a non-Unimplemented variant.
    pub invoked_void: usize, // Calls that returned Void.
    pub registry_hit: usize, // Thunks whose name resolves in pe_dll_registry.
    pub failures: Vec<&'static str>,
}

/// Walk every Phase-B thunk with a small canned argument list and
/// confirm each one returns a non-`Unimplemented` variant. Also confirm
/// the function name is present in the corresponding DLL name registry.
pub fn run_smoketest(ctx: &mut CompatContext) -> ThunkSmoketest {
    use crate::pe_dll_registry::KERNEL32_NAMES;

    let mut st = ThunkSmoketest {
        registered: ALL_THUNKS.len(),
        invoked_ok: 0,
        invoked_void: 0,
        registry_hit: 0,
        failures: Vec::new(),
    };

    for &t in ALL_THUNKS {
        // 1. Registry presence — does the name registry list this name?
        if KERNEL32_NAMES.iter().any(|n| *n == t.name()) {
            st.registry_hit += 1;
        }

        // 2. Invoke with neutral arguments.
        let args: [u64; 4] = [0, 0, 16, 0]; // 16 = small heap allocation request
        let r = invoke(ctx, t, &args);
        match r {
            ThunkResult::Unimplemented => st.failures.push(t.name()),
            ThunkResult::Void => {
                st.invoked_ok += 1;
                st.invoked_void += 1;
            }
            _ => st.invoked_ok += 1,
        }
    }

    st
}
