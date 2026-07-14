//! Hand-assembled PE32+ test executables for AthBridge bring-up.
//!
//! Concept §Compatibility Strategy: every execution milestone needs a PE we
//! fully control, built without an external assembler or linker (same
//! policy as the kernel's embedded smoketest PE in `raebridge_boot`).
//!
//! [`build_exit_process_exe`] produces the canonical "first Windows
//! program": a PE32+ image whose entry point does
//!
//! ```text
//! sub  rsp, 0x28                      ; shadow space, align stack
//! mov  ecx, 42                        ; uExitCode
//! call qword [rip + IAT_ExitProcess]  ; kernel32!ExitProcess
//! ```
//!
//! If the process exits with code 42, the whole chain — loader, section
//! mapping, IAT patch, win64 shim dispatch, SYS_EXIT — provably executed.

use alloc::vec;
use alloc::vec::Vec;

/// Exit code the test image passes to `ExitProcess`. The host's parent
/// (user_init) asserts this exact value after `sys_wait`.
pub const TEST_EXE_EXIT_CODE: u32 = 42;

/// The exact bytes the hello-world image hands to `WriteFile`. The smoketest
/// asserts these reached `sys_write` (via the stdout capture tee), so a
/// botched marshal of the WriteFile arguments fails loud instead of silently
/// writing garbage.
pub const HELLO_MSG: &[u8] = b"Hello from Windows\n";

// ---------------------------------------------------------------------------
// Real MSVC /MT console .exe fixture (THE "real Windows software runs" proof)
// ---------------------------------------------------------------------------

/// A real, unmodified MSVC `/MT` console `.exe` produced by VS2022's `cl.exe`
/// from `fixtures/real_msvc_mt_hello.c`:
///
/// ```c
/// #include <windows.h>
/// int main(void) {
///     const char *s = "real windows exe\n";
///     DWORD n = 0;
///     WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), s, 17, &n, 0);
///     return 0;
/// }
/// ```
///
/// Compiled `cl /MT /O1 /GS- hello.c /link /SUBSYSTEM:CONSOLE` → PE32+, x64,
/// console subsystem, **73 KERNEL32 imports** (verified with `dumpbin /imports`,
/// captured in `fixtures/real_msvc_mt_hello.dumpbin.txt`). This is the genuine
/// MSVC CRT (`mainCRTStartup` → `__scrt_common_main_seh` → `main`), not a
/// hand-assembled stand-in. If AthBridge loads it, resolves every IAT slot to a
/// real shim, runs the CRT startup to `main`, and `main` prints
/// [`REAL_EXE_MSG`] via WriteFile and returns 0 — that is the milestone:
/// real Windows software running on AthenaOS.
pub const REAL_MSVC_MT_EXE: &[u8] = include_bytes!("../fixtures/real_msvc_mt_hello.exe");

/// The exact bytes `main()` in the real exe hands to WriteFile.
pub const REAL_EXE_MSG: &[u8] = b"real windows exe\n";

/// Borrow the embedded real-MSVC `/MT` exe bytes.
pub fn real_msvc_mt_exe() -> &'static [u8] {
    REAL_MSVC_MT_EXE
}

/// A real, unmodified MSVC `/MT` console `.exe` produced by VS2022's `cl.exe`
/// from `fixtures/real_msvc_mt_printf.c`:
///
/// ```c
/// #include <stdio.h>
/// int main(void) {
///     printf("printf says hi %d %s\n", 42, "from a real exe");
///     return 0;
/// }
/// ```
///
/// Compiled `cl /MT /O1 /GS- printf.c /link /SUBSYSTEM:CONSOLE` → PE32+, x64,
/// console subsystem, **74 KERNEL32 imports** (verified with `dumpbin /imports`,
/// captured in `fixtures/real_msvc_mt_printf.dumpbin.txt`). This broadens the
/// "real Windows software runs" milestone past the WriteFile fixture: unlike the
/// hello fixture (which calls WriteFile directly from `main`), this drives the
/// genuine MSVC CRT **buffered-stdio + format engine** — `printf` →
/// `_vfprintf_l` → the CRT's internal `__stdio_common_vfprintf` formatter →
/// `_write`/`fwrite` → `WriteFile`. That format/buffering layer is what the vast
/// majority of real console apps use. Compared to the WriteFile fixture it pulls
/// exactly ONE additional KERNEL32 import: `GetFileSizeEx` (the CRT sizes its
/// stdio buffers per std handle). Everything else is CRT-internal in `/MT`.
///
/// If AthBridge loads it, resolves every IAT slot to a real shim, runs the CRT
/// startup to `main`, and `main`'s `printf` emits [`REAL_PRINTF_MSG`] through the
/// CRT format engine + WriteFile and returns 0 — that proves the CRT's format
/// engine ran correctly through our shims, the printf-C-program case.
pub const REAL_MSVC_MT_PRINTF_EXE: &[u8] = include_bytes!("../fixtures/real_msvc_mt_printf.exe");

/// The exact formatted bytes the printf exe's `main()` emits through the CRT
/// format engine, AS THEY REACH WriteFile. The smoketest asserts the captured
/// stdout equals this — a botched `%d`/`%s` marshal or a buffering bug produces
/// different bytes and the smoketest FAILs loud.
///
/// Note the trailing `\r\n`, not `\n`: the MSVC CRT opens stdout in **text
/// mode** by default, so its stdio layer translates the source `\n` from
/// `printf("...\n")` into a CRLF before handing the bytes to WriteFile. The
/// WriteFile fixture never saw this (it called WriteFile directly, bypassing the
/// CRT). That CRLF in the captured tee is itself proof the genuine CRT
/// buffered-stdio + text-mode translation layer ran — exactly the layer this
/// fixture exists to exercise.
pub const REAL_PRINTF_MSG: &[u8] = b"printf says hi 42 from a real exe\r\n";

/// Borrow the embedded real-MSVC `/MT` printf exe bytes.
pub fn real_msvc_mt_printf_exe() -> &'static [u8] {
    REAL_MSVC_MT_PRINTF_EXE
}

/// A real, unmodified MSVC `/MT` **C++** console `.exe` produced by VS2022's
/// `cl.exe` from `fixtures/real_msvc_mt_cpp.cpp`:
///
/// ```cpp
/// #include <cstdio>
/// struct Init { Init() { printf("ctor ran\n"); } };
/// static Init g_init;                       // static (namespace-scope) initializer
/// int main() { printf("hello from c++ %d\n", 7); return 0; }
/// ```
///
/// Compiled `cl /MT /O1 /GS- /EHsc cpp.cpp /link /SUBSYSTEM:CONSOLE` → PE32+,
/// x64, console subsystem. This is the **C++-runtime** broadening over the
/// plain-C printf milestone: most real Windows software is C++, and a C++
/// program's defining startup behavior is the **static-initializer table walk**.
/// The MSVC `/MT` CRT, before calling `main`, runs `__scrt_common_main_seh` →
/// `_initterm`/`_initterm_e` over the `.CRT$XC*` section, invoking every
/// namespace-scope constructor. Here that is `g_init`'s ctor, which prints
/// `"ctor ran\n"`. Only AFTER all static ctors run does the CRT call `main`,
/// which prints `"hello from c++ 7\n"`.
///
/// The proof the C++ runtime ran (not just `main`): BOTH `ctor ran` AND
/// `hello from c++ 7` appear in captured stdout, IN ORDER, and the process
/// exits 0. A binary that skipped static-init would print only the `main` line.
///
/// Import-surface note: `dumpbin /imports` (see
/// `fixtures/real_msvc_mt_cpp.dumpbin.txt`) shows this fixture imports the
/// **exact same 74 KERNEL32 functions as the printf fixture — ZERO new
/// imports**. In `/MT`, the C++ static-ctor table walk (`_initterm`), the
/// `atexit`/`_register_onexit_function` registration, and the C++ EH personality
/// are all **CRT-internal** (statically linked into the image), not imported. So
/// the C++-runtime delta is internal CRT code executing over the already-resolved
/// import surface — no new shim is required.
pub const REAL_MSVC_MT_CPP_EXE: &[u8] = include_bytes!("../fixtures/real_msvc_mt_cpp.exe");

/// A real cl.exe-compiled **GUI** `.exe` (pure Win32, custom `rae_entry` that
/// RETURNS so the loader regains control): registers a class, creates+shows a
/// window, and `UpdateWindow`s it to drive a synchronous `WM_PAINT` that paints
/// a white background + "HI" text. Imports only the user32/gdi32 names AthBridge
/// has IAT-wired (verified by `dumpbin /imports`). Source: `fixtures/gui_window.c`.
/// This is the guest-machine-code half of the notepad-class gate.
pub const GUI_WINDOW_EXE: &[u8] = include_bytes!("../fixtures/gui_window.exe");

/// A real cl.exe-compiled GUI `.exe` that TYPES then SAVES (the notepad flow):
/// injects 'H','I' as WM_KEYDOWN to its own window, the pump turns them into
/// WM_CHAR, the WndProc accumulates them and writes "HI" to `C:\out.txt`
/// (`CreateFileW` CREATE_ALWAYS + `WriteFile`). Source: `fixtures/gui_save.c`.
pub const GUI_SAVE_EXE: &[u8] = include_bytes!("../fixtures/gui_save.exe");

/// The notepad-class CAPSTONE `.exe` — a real cl.exe Win32 program that
/// integrates every notepad piece: a main window + WndProc, a system "EDIT"
/// child holding the text, a File menu (Save/Exit), typing into the EDIT, and a
/// menu-driven File->Save that reads the EDIT via `GetWindowTextW`, picks a path
/// via `GetSaveFileNameW`, and writes it with `CreateFileW`/`WriteFile`
/// (`C:\note.txt`), then File->Exit. Imports only IAT-wired names (18, verified
/// by `dumpbin /imports`: 14 user32 + GetSaveFileNameW + 3 kernel32). Source:
/// `fixtures/gui_notepad.c`. The C3 gate: a notepad-class .exe runs/types/saves.
pub const GUI_NOTEPAD_EXE: &[u8] = include_bytes!("../fixtures/gui_notepad.exe");

/// The exact bytes the C++ fixture emits, in order, AS THEY REACH WriteFile:
/// the static ctor's line first, then `main`'s line. Note the `\r\n` line
/// endings: the MSVC CRT opens stdout in text mode and translates each source
/// `\n` to CRLF before WriteFile — the same buffered-stdio + text-mode layer the
/// printf fixture exercises. The smoketest asserts captured stdout equals this
/// exactly; if the ctor line is missing (static-init skipped) it FAILs loud.
pub const REAL_CPP_MSG: &[u8] = b"ctor ran\r\nhello from c++ 7\r\n";

/// Just the static initializer's line (CRT text-mode CRLF). Used by the FAIL
/// path to report whether the static ctor fired at all — distinguishing
/// "static-init skipped entirely" from "ctor ran but bytes mismatched".
pub const CPP_CTOR_LINE: &[u8] = b"ctor ran\r\n";

/// Borrow the embedded real-MSVC `/MT` C++ exe bytes.
pub fn real_msvc_mt_cpp_exe() -> &'static [u8] {
    REAL_MSVC_MT_CPP_EXE
}

const FILE_SIZE: usize = 0x800;
const PE_OFFSET: usize = 0x80;
const OPT_OFFSET: usize = PE_OFFSET + 4 + 20; // 0x98
const DATA_DIR_OFFSET: usize = OPT_OFFSET + 112; // 0x108
const SECTION_TABLE_OFFSET: usize = DATA_DIR_OFFSET + 16 * 8; // 0x188

fn put_u16(buf: &mut [u8], off: usize, v: u16) {
    buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn put_u32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_u64(buf: &mut [u8], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

/// Build the ExitProcess(42) test image. Preferred base 0x140000000,
/// .text at RVA 0x1000 (file 0x400), .idata at RVA 0x2000 (file 0x600),
/// one import: kernel32.dll!ExitProcess through the IAT slot at RVA 0x2038.
pub fn build_exit_process_exe() -> Vec<u8> {
    let mut b = vec![0u8; FILE_SIZE];

    // ── DOS header ────────────────────────────────────────────────────
    b[0] = b'M';
    b[1] = b'Z';
    put_u32(&mut b, 0x3C, PE_OFFSET as u32); // e_lfanew

    // ── PE signature + COFF header ────────────────────────────────────
    b[PE_OFFSET] = b'P';
    b[PE_OFFSET + 1] = b'E';
    put_u16(&mut b, PE_OFFSET + 4, 0x8664); // Machine = AMD64
    put_u16(&mut b, PE_OFFSET + 6, 2); // NumberOfSections
    put_u16(&mut b, PE_OFFSET + 20, 0xF0); // SizeOfOptionalHeader (PE32+)
    put_u16(&mut b, PE_OFFSET + 22, 0x0022); // EXECUTABLE | LARGE_ADDRESS_AWARE

    // ── Optional header (PE32+) ───────────────────────────────────────
    put_u16(&mut b, OPT_OFFSET, 0x020B); // Magic
    b[OPT_OFFSET + 2] = 14; // MajorLinkerVersion
    put_u32(&mut b, OPT_OFFSET + 4, 0x200); // SizeOfCode
    put_u32(&mut b, OPT_OFFSET + 16, 0x1000); // AddressOfEntryPoint
    put_u32(&mut b, OPT_OFFSET + 20, 0x1000); // BaseOfCode
    put_u64(&mut b, OPT_OFFSET + 24, 0x1_4000_0000); // ImageBase
    put_u32(&mut b, OPT_OFFSET + 32, 0x1000); // SectionAlignment
    put_u32(&mut b, OPT_OFFSET + 36, 0x200); // FileAlignment
    put_u16(&mut b, OPT_OFFSET + 40, 6); // MajorOperatingSystemVersion
    put_u16(&mut b, OPT_OFFSET + 48, 6); // MajorSubsystemVersion
    put_u32(&mut b, OPT_OFFSET + 56, 0x3000); // SizeOfImage
    put_u32(&mut b, OPT_OFFSET + 60, 0x400); // SizeOfHeaders
    put_u16(&mut b, OPT_OFFSET + 68, 3); // Subsystem = WINDOWS_CUI
    put_u64(&mut b, OPT_OFFSET + 72, 0x10_0000); // SizeOfStackReserve
    put_u64(&mut b, OPT_OFFSET + 80, 0x1000); // SizeOfStackCommit
    put_u64(&mut b, OPT_OFFSET + 88, 0x10_0000); // SizeOfHeapReserve
    put_u64(&mut b, OPT_OFFSET + 96, 0x1000); // SizeOfHeapCommit
    put_u32(&mut b, OPT_OFFSET + 108, 16); // NumberOfRvaAndSizes

    // ── Data directories ──────────────────────────────────────────────
    // [1] IMPORT: descriptor table at RVA 0x2000
    put_u32(&mut b, DATA_DIR_OFFSET + 8, 0x2000);
    put_u32(&mut b, DATA_DIR_OFFSET + 12, 0x28);
    // [12] IAT: the patched slots at RVA 0x2038
    put_u32(&mut b, DATA_DIR_OFFSET + 96, 0x2038);
    put_u32(&mut b, DATA_DIR_OFFSET + 100, 0x10);

    // ── Section table ─────────────────────────────────────────────────
    // .text — RVA 0x1000, file 0x400
    let s0 = SECTION_TABLE_OFFSET;
    b[s0..s0 + 5].copy_from_slice(b".text");
    put_u32(&mut b, s0 + 8, 0x200); // VirtualSize
    put_u32(&mut b, s0 + 12, 0x1000); // VirtualAddress
    put_u32(&mut b, s0 + 16, 0x200); // SizeOfRawData
    put_u32(&mut b, s0 + 20, 0x400); // PointerToRawData
    put_u32(&mut b, s0 + 36, 0x6000_0020); // CODE | EXECUTE | READ

    // .idata — RVA 0x2000, file 0x600
    let s1 = SECTION_TABLE_OFFSET + 40;
    b[s1..s1 + 6].copy_from_slice(b".idata");
    put_u32(&mut b, s1 + 8, 0x200);
    put_u32(&mut b, s1 + 12, 0x2000);
    put_u32(&mut b, s1 + 16, 0x200);
    put_u32(&mut b, s1 + 20, 0x600);
    put_u32(&mut b, s1 + 36, 0xC000_0040); // INITIALIZED_DATA | READ | WRITE

    // ── .text @ file 0x400 (RVA 0x1000) ───────────────────────────────
    // sub rsp, 0x28
    b[0x400..0x404].copy_from_slice(&[0x48, 0x83, 0xEC, 0x28]);
    // mov ecx, TEST_EXE_EXIT_CODE
    b[0x404] = 0xB9;
    put_u32(&mut b, 0x405, TEST_EXE_EXIT_CODE);
    // call qword [rip + disp32] → IAT slot at RVA 0x2038.
    // Next instruction RVA = 0x100F, so disp32 = 0x2038 - 0x100F = 0x1029.
    b[0x409] = 0xFF;
    b[0x40A] = 0x15;
    put_u32(&mut b, 0x40B, 0x1029);
    // int3 — ExitProcess never returns; trap loudly if it somehow does.
    b[0x40F] = 0xCC;

    // ── .idata @ file 0x600 (RVA 0x2000) ──────────────────────────────
    // IMAGE_IMPORT_DESCRIPTOR for kernel32.dll
    put_u32(&mut b, 0x600, 0x2028); // OriginalFirstThunk (ILT)
    put_u32(&mut b, 0x60C, 0x2048); // Name
    put_u32(&mut b, 0x610, 0x2038); // FirstThunk (IAT)
                                    // (null terminator descriptor: bytes 0x614..0x628 stay zero)

    // ILT @ RVA 0x2028: one hint/name entry, then 0
    put_u64(&mut b, 0x628, 0x2058);
    // IAT @ RVA 0x2038: same entry pre-patch, then 0
    put_u64(&mut b, 0x638, 0x2058);

    // DLL name @ RVA 0x2048
    b[0x648..0x648 + 13].copy_from_slice(b"kernel32.dll\0");

    // Hint/name @ RVA 0x2058: u16 hint then NUL-terminated name
    b[0x658] = 0;
    b[0x659] = 0;
    b[0x65A..0x65A + 12].copy_from_slice(b"ExitProcess\0");

    b
}

/// Build the "Hello from Windows" test image — the second AthBridge bring-up
/// rung: a PE32+ that produces *visible output*.
///
/// Its entry point (Microsoft x64 ABI) does:
///
/// ```text
/// sub  rsp, 0x38                    ; shadow(0x20) + 5th-arg slot + bytesWritten
/// mov  ecx, STD_OUTPUT_HANDLE       ; (-11)
/// call qword [rip + IAT_GetStdHandle]
/// mov  rcx, rax                     ; hFile = stdout handle
/// lea  rdx, [rip + msg]             ; lpBuffer  -> "Hello from Windows\n"
/// mov  r8d, 19                      ; nNumberOfBytesToWrite
/// lea  r9,  [rsp + 0x28]            ; lpNumberOfBytesWritten (stack scratch)
/// mov  qword [rsp + 0x20], 0        ; lpOverlapped = NULL (5th arg, on stack)
/// call qword [rip + IAT_WriteFile]
/// add  rsp, 0x38
/// ret                               ; return to the host harness
/// ```
///
/// It imports `kernel32!{GetStdHandle, WriteFile, ExitProcess}` so import
/// resolution covers all three; `ExitProcess` is resolved-but-uncalled because
/// the host runs this image *and then* a second image in the same process —
/// the entry returns rather than terminating. WriteFile routes through the
/// win64 shim → `sys_write` to the seeded stdout fd (1), which is also tee'd
/// into the smoketest's capture buffer so "Hello from Windows" is provably
/// emitted by guest Windows machine code.
///
/// Preferred base 0x140000000; .text RVA 0x1000 (file 0x400), .idata RVA
/// 0x2000 (file 0x600). Two IAT slots are *called* (GetStdHandle at 0x2058,
/// WriteFile at 0x2060); ExitProcess sits in the IAT at 0x2068, unused.
pub fn build_hello_world_exe() -> Vec<u8> {
    let mut b = vec![0u8; FILE_SIZE];

    // ── DOS header ────────────────────────────────────────────────────
    b[0] = b'M';
    b[1] = b'Z';
    put_u32(&mut b, 0x3C, PE_OFFSET as u32);

    // ── PE signature + COFF header ────────────────────────────────────
    b[PE_OFFSET] = b'P';
    b[PE_OFFSET + 1] = b'E';
    put_u16(&mut b, PE_OFFSET + 4, 0x8664); // Machine = AMD64
    put_u16(&mut b, PE_OFFSET + 6, 2); // NumberOfSections
    put_u16(&mut b, PE_OFFSET + 20, 0xF0); // SizeOfOptionalHeader (PE32+)
    put_u16(&mut b, PE_OFFSET + 22, 0x0022); // EXECUTABLE | LARGE_ADDRESS_AWARE

    // ── Optional header (PE32+) ───────────────────────────────────────
    put_u16(&mut b, OPT_OFFSET, 0x020B); // Magic
    b[OPT_OFFSET + 2] = 14; // MajorLinkerVersion
    put_u32(&mut b, OPT_OFFSET + 4, 0x200); // SizeOfCode
    put_u32(&mut b, OPT_OFFSET + 16, 0x1000); // AddressOfEntryPoint
    put_u32(&mut b, OPT_OFFSET + 20, 0x1000); // BaseOfCode
                                              // Preferred base 0x150000000 — deliberately distinct from the exit-code
                                              // image's 0x140000000 so the harness can map BOTH in one process without
                                              // either needing a .reloc section (neither carries relocations). All the
                                              // code is RIP-relative, so the base value never reaches the encoded bytes.
    put_u64(&mut b, OPT_OFFSET + 24, 0x1_5000_0000); // ImageBase
    put_u32(&mut b, OPT_OFFSET + 32, 0x1000); // SectionAlignment
    put_u32(&mut b, OPT_OFFSET + 36, 0x200); // FileAlignment
    put_u16(&mut b, OPT_OFFSET + 40, 6); // MajorOperatingSystemVersion
    put_u16(&mut b, OPT_OFFSET + 48, 6); // MajorSubsystemVersion
    put_u32(&mut b, OPT_OFFSET + 56, 0x3000); // SizeOfImage
    put_u32(&mut b, OPT_OFFSET + 60, 0x400); // SizeOfHeaders
    put_u16(&mut b, OPT_OFFSET + 68, 3); // Subsystem = WINDOWS_CUI
    put_u64(&mut b, OPT_OFFSET + 72, 0x10_0000); // SizeOfStackReserve
    put_u64(&mut b, OPT_OFFSET + 80, 0x1000); // SizeOfStackCommit
    put_u64(&mut b, OPT_OFFSET + 88, 0x10_0000); // SizeOfHeapReserve
    put_u64(&mut b, OPT_OFFSET + 96, 0x1000); // SizeOfHeapCommit
    put_u32(&mut b, OPT_OFFSET + 108, 16); // NumberOfRvaAndSizes

    // ── Data directories ──────────────────────────────────────────────
    // [1] IMPORT: descriptor table at RVA 0x2000 (one real + one null = 0x28)
    put_u32(&mut b, DATA_DIR_OFFSET + 8, 0x2000);
    put_u32(&mut b, DATA_DIR_OFFSET + 12, 0x28);
    // [12] IAT: the three patched slots at RVA 0x2058 (3 * 8 = 0x18)
    put_u32(&mut b, DATA_DIR_OFFSET + 96, 0x2058);
    put_u32(&mut b, DATA_DIR_OFFSET + 100, 0x18);

    // ── Section table ─────────────────────────────────────────────────
    // .text — RVA 0x1000, file 0x400
    let s0 = SECTION_TABLE_OFFSET;
    b[s0..s0 + 5].copy_from_slice(b".text");
    put_u32(&mut b, s0 + 8, 0x200); // VirtualSize
    put_u32(&mut b, s0 + 12, 0x1000); // VirtualAddress
    put_u32(&mut b, s0 + 16, 0x200); // SizeOfRawData
    put_u32(&mut b, s0 + 20, 0x400); // PointerToRawData
    put_u32(&mut b, s0 + 36, 0x6000_0020); // CODE | EXECUTE | READ

    // .idata — RVA 0x2000, file 0x600
    let s1 = SECTION_TABLE_OFFSET + 40;
    b[s1..s1 + 6].copy_from_slice(b".idata");
    put_u32(&mut b, s1 + 8, 0x200);
    put_u32(&mut b, s1 + 12, 0x2000);
    put_u32(&mut b, s1 + 16, 0x200);
    put_u32(&mut b, s1 + 20, 0x600);
    put_u32(&mut b, s1 + 36, 0xC000_0040); // INITIALIZED_DATA | READ | WRITE

    // ── .text @ file 0x400 (RVA 0x1000) ───────────────────────────────
    // The IAT lives at RVA 0x2058 (GetStdHandle), 0x2060 (WriteFile),
    // 0x2068 (ExitProcess). `call qword [rip+disp32]` uses disp =
    // target_RVA - next_instr_RVA.
    let mut p = 0x400usize; // file cursor, RVA = p - 0x400 + 0x1000

    // sub rsp, 0x38
    b[p..p + 4].copy_from_slice(&[0x48, 0x83, 0xEC, 0x38]);
    p += 4; // -> RVA 0x1004
            // mov ecx, STD_OUTPUT_HANDLE (-11 = 0xFFFFFFF5)
    b[p] = 0xB9;
    put_u32(&mut b, p + 1, 0xFFFF_FFF5);
    p += 5; // -> RVA 0x1009
            // call qword [rip + disp] -> GetStdHandle IAT @ 0x2058. next = 0x100F.
    b[p] = 0xFF;
    b[p + 1] = 0x15;
    put_u32(&mut b, p + 2, 0x2058 - 0x100F);
    p += 6; // -> RVA 0x100F
            // mov rcx, rax
    b[p..p + 3].copy_from_slice(&[0x48, 0x89, 0xC1]);
    p += 3; // -> RVA 0x1012
            // lea rdx, [rip + disp] -> msg @ RVA 0x1040. next = 0x1019.
    b[p] = 0x48;
    b[p + 1] = 0x8D;
    b[p + 2] = 0x15;
    put_u32(&mut b, p + 3, 0x1040 - 0x1019);
    p += 7; // -> RVA 0x1019
            // mov r8d, 19
    b[p] = 0x41;
    b[p + 1] = 0xB8;
    put_u32(&mut b, p + 2, HELLO_MSG.len() as u32);
    p += 6; // -> RVA 0x101F
            // lea r9, [rsp + 0x28]  (lpNumberOfBytesWritten)
    b[p..p + 5].copy_from_slice(&[0x4C, 0x8D, 0x4C, 0x24, 0x28]);
    p += 5; // -> RVA 0x1024
            // mov qword [rsp + 0x20], 0  (lpOverlapped = NULL, 5th stack arg)
    b[p..p + 9].copy_from_slice(&[0x48, 0xC7, 0x44, 0x24, 0x20, 0x00, 0x00, 0x00, 0x00]);
    p += 9; // -> RVA 0x102D
            // call qword [rip + disp] -> WriteFile IAT @ 0x2060. next = 0x1033.
    b[p] = 0xFF;
    b[p + 1] = 0x15;
    put_u32(&mut b, p + 2, 0x2060 - 0x1033);
    p += 6; // -> RVA 0x1033
            // add rsp, 0x38
    b[p..p + 4].copy_from_slice(&[0x48, 0x83, 0xC4, 0x38]);
    p += 4; // -> RVA 0x1037
            // ret — back to the host harness, which then runs the exit-code image.
    b[p] = 0xC3;

    // Message bytes @ RVA 0x1040 (file 0x440).
    let msg_file = 0x440usize;
    b[msg_file..msg_file + HELLO_MSG.len()].copy_from_slice(HELLO_MSG);

    // ── .idata @ file 0x600 (RVA 0x2000) ──────────────────────────────
    // IMAGE_IMPORT_DESCRIPTOR for kernel32.dll
    put_u32(&mut b, 0x600, 0x2028); // OriginalFirstThunk (ILT)
    put_u32(&mut b, 0x60C, 0x2070); // Name
    put_u32(&mut b, 0x610, 0x2058); // FirstThunk (IAT)
                                    // (null descriptor at 0x614..0x628 stays zero)

    // ILT @ RVA 0x2028 (file 0x628): three hint/name pointers, then 0.
    put_u64(&mut b, 0x628, 0x2080); // GetStdHandle
    put_u64(&mut b, 0x630, 0x2090); // WriteFile
    put_u64(&mut b, 0x638, 0x20A0); // ExitProcess
                                    // 0x640: ILT terminator (zero)

    // IAT @ RVA 0x2058 (file 0x658): same three, then 0. patch_iat overwrites
    // these with shim addresses; pre-patch they mirror the ILT.
    put_u64(&mut b, 0x658, 0x2080); // GetStdHandle slot (RVA 0x2058)
    put_u64(&mut b, 0x660, 0x2090); // WriteFile slot    (RVA 0x2060)
    put_u64(&mut b, 0x668, 0x20A0); // ExitProcess slot  (RVA 0x2068)
                                    // 0x670: IAT terminator (zero)

    // DLL name @ RVA 0x2070 (file 0x670)
    b[0x670..0x670 + 13].copy_from_slice(b"kernel32.dll\0");

    // Hint/name entries: u16 hint (0) then NUL-terminated name.
    // GetStdHandle @ RVA 0x2080 (file 0x680)
    b[0x680] = 0;
    b[0x681] = 0;
    b[0x682..0x682 + 13].copy_from_slice(b"GetStdHandle\0");
    // WriteFile @ RVA 0x2090 (file 0x690)
    b[0x690] = 0;
    b[0x691] = 0;
    b[0x692..0x692 + 10].copy_from_slice(b"WriteFile\0");
    // ExitProcess @ RVA 0x20A0 (file 0x6A0)
    b[0x6A0] = 0;
    b[0x6A1] = 0;
    b[0x6A2..0x6A2 + 12].copy_from_slice(b"ExitProcess\0");

    b
}

/// The exact wide string the api-exercise image hands to `WriteConsoleW` on a
/// full pass (7 UTF-16 code units, including the trailing newline). The
/// smoketest asserts the down-converted ASCII bytes reached the capture tee, so
/// a botched WriteConsoleW marshal fails loud instead of silently dropping it.
pub const API_OK_MSG: &str = "API OK\n";

/// Names of the kernel32 imports the api-exercise image pulls in, in IAT order.
/// Each must resolve to a real, non-null shim or the loader stubs it fail-loud;
/// the host KAT asserts every one resolves before the image ever executes.
pub const API_EXERCISE_IMPORTS: &[&str] = &[
    "GetModuleHandleW", // 0
    "GetProcAddress",   // 1
    "GetProcessHeap",   // 2
    "HeapAlloc",        // 3
    "HeapReAlloc",      // 4
    "HeapSize",         // 5
    "HeapFree",         // 6
    "TlsAlloc",         // 7
    "TlsSetValue",      // 8
    "TlsGetValue",      // 9
    "TlsFree",          // 10
    "GetStdHandle",     // 11
    "WriteConsoleW",    // 12
];

/// Per-step result codes the api-exercise entry returns in RAX. `0` = every
/// newly-broadened shim behaved correctly end-to-end; any non-zero value is the
/// number of the FIRST step that failed, so the smoketest can name it exactly.
pub const API_PASS: u64 = 0;
pub const API_FAIL_GETMODULEHANDLE: u64 = 1;
pub const API_FAIL_GETPROCADDRESS: u64 = 2;
pub const API_FAIL_HEAPALLOC: u64 = 3;
pub const API_FAIL_HEAPREALLOC: u64 = 4;
pub const API_FAIL_TLS: u64 = 5;
pub const API_FAIL_WRITECONSOLE: u64 = 6;

/// Build the "API exercise" image — the third AthBridge bring-up rung. Where
/// hello-world proved a single output call, this PE32+ *exercises the broadened
/// shim surface* end-to-end from guest Windows machine code and self-verifies
/// each result, returning a step code in RAX (0 = all-pass).
///
/// Entry point (Microsoft x64 ABI, `extern "win64" fn() -> u64`):
///
/// ```text
/// 1. GetModuleHandleW(L"kernel32.dll")          -> non-null base   (else ret 1)
/// 2. GetProcAddress(hK32, "ExitProcess")        -> non-null addr   (else ret 2)
/// 3. p = HeapAlloc(GetProcessHeap(), 0, 64)     -> non-null,
///    p[0]=0xA5; p[63]=0x5A; HeapSize(p) == 64                      (else ret 3)
/// 4. q = HeapReAlloc(heap, 0, p, 128)           -> non-null,
///    q[0]==0xA5 && q[63]==0x5A (bytes survived); HeapFree(q)       (else ret 4)
/// 5. i = TlsAlloc() (!= -1); TlsSetValue(i,0xCAFE);
///    TlsGetValue(i) == 0xCAFE; TlsFree(i)                          (else ret 5)
/// 6. WriteConsoleW(GetStdHandle(-11), L"API OK\n", 7, &n, 0) != 0  (else ret 6)
///    -> ret 0
/// ```
///
/// The entry *returns* (it does not call ExitProcess), so the host harness runs
/// it in-process between hello-world and the exit-code terminator, reads RAX for
/// the verdict, and asserts the capture tee saw "API OK". A distinct base
/// (0x160000000) lets it coexist with both other images without relocations
/// (all code is RIP-relative). All callee-saved registers it clobbers (rbx, rsi,
/// rdi, r12–r14) are preserved per the Win64 ABI so returning to Rust is clean.
pub fn build_api_exercise_exe() -> Vec<u8> {
    const SIZE: usize = 0x1000;
    let mut b = vec![0u8; SIZE];

    // ── DOS header ────────────────────────────────────────────────────
    b[0] = b'M';
    b[1] = b'Z';
    put_u32(&mut b, 0x3C, PE_OFFSET as u32);

    // ── PE signature + COFF header ────────────────────────────────────
    b[PE_OFFSET] = b'P';
    b[PE_OFFSET + 1] = b'E';
    put_u16(&mut b, PE_OFFSET + 4, 0x8664); // Machine = AMD64
    put_u16(&mut b, PE_OFFSET + 6, 2); // NumberOfSections (.text + .idata)
    put_u16(&mut b, PE_OFFSET + 20, 0xF0); // SizeOfOptionalHeader (PE32+)
    put_u16(&mut b, PE_OFFSET + 22, 0x0022); // EXECUTABLE | LARGE_ADDRESS_AWARE

    // ── Optional header (PE32+) ───────────────────────────────────────
    put_u16(&mut b, OPT_OFFSET, 0x020B); // Magic
    b[OPT_OFFSET + 2] = 14; // MajorLinkerVersion
    put_u32(&mut b, OPT_OFFSET + 4, 0x400); // SizeOfCode
    put_u32(&mut b, OPT_OFFSET + 16, 0x1000); // AddressOfEntryPoint
    put_u32(&mut b, OPT_OFFSET + 20, 0x1000); // BaseOfCode
                                              // Distinct base so all three bring-up images map in one process without
                                              // any needing a .reloc section; code is fully RIP-relative.
    put_u64(&mut b, OPT_OFFSET + 24, 0x1_6000_0000); // ImageBase
    put_u32(&mut b, OPT_OFFSET + 32, 0x1000); // SectionAlignment
    put_u32(&mut b, OPT_OFFSET + 36, 0x200); // FileAlignment
    put_u16(&mut b, OPT_OFFSET + 40, 6); // MajorOperatingSystemVersion
    put_u16(&mut b, OPT_OFFSET + 48, 6); // MajorSubsystemVersion
    put_u32(&mut b, OPT_OFFSET + 56, 0x4000); // SizeOfImage (headers + .text + .idata)
    put_u32(&mut b, OPT_OFFSET + 60, 0x400); // SizeOfHeaders
    put_u16(&mut b, OPT_OFFSET + 68, 3); // Subsystem = WINDOWS_CUI
    put_u64(&mut b, OPT_OFFSET + 72, 0x10_0000); // SizeOfStackReserve
    put_u64(&mut b, OPT_OFFSET + 80, 0x1000); // SizeOfStackCommit
    put_u64(&mut b, OPT_OFFSET + 88, 0x10_0000); // SizeOfHeapReserve
    put_u64(&mut b, OPT_OFFSET + 96, 0x1000); // SizeOfHeapCommit
    put_u32(&mut b, OPT_OFFSET + 108, 16); // NumberOfRvaAndSizes

    // ── Data directories ──────────────────────────────────────────────
    // [1] IMPORT: descriptor table at RVA 0x3000 (one real + one null = 0x28).
    put_u32(&mut b, DATA_DIR_OFFSET + 8, 0x3000);
    put_u32(&mut b, DATA_DIR_OFFSET + 12, 0x28);
    // [12] IAT: 13 patched slots at RVA 0x3100 (13 * 8 = 0x68).
    put_u32(&mut b, DATA_DIR_OFFSET + 96, 0x3100);
    put_u32(
        &mut b,
        DATA_DIR_OFFSET + 100,
        (API_EXERCISE_IMPORTS.len() * 8) as u32,
    );

    // ── Section table ─────────────────────────────────────────────────
    let s0 = SECTION_TABLE_OFFSET;
    b[s0..s0 + 5].copy_from_slice(b".text");
    put_u32(&mut b, s0 + 8, 0x400); // VirtualSize
    put_u32(&mut b, s0 + 12, 0x1000); // VirtualAddress
    put_u32(&mut b, s0 + 16, 0x400); // SizeOfRawData
    put_u32(&mut b, s0 + 20, 0x400); // PointerToRawData
    put_u32(&mut b, s0 + 36, 0x6000_0020); // CODE | EXECUTE | READ

    let s1 = SECTION_TABLE_OFFSET + 40;
    b[s1..s1 + 6].copy_from_slice(b".idata");
    put_u32(&mut b, s1 + 8, 0x600); // VirtualSize (descriptors + ILT/IAT + names)
    put_u32(&mut b, s1 + 12, 0x3000); // VirtualAddress
    put_u32(&mut b, s1 + 16, 0x600); // SizeOfRawData
    put_u32(&mut b, s1 + 20, 0x800); // PointerToRawData (file 0x800..0xE00)
    put_u32(&mut b, s1 + 36, 0xC000_0040); // INITIALIZED_DATA | READ | WRITE

    // ── .text @ file 0x400 (RVA 0x1000) ───────────────────────────────
    //
    // Mini-assembler. `emit` appends bytes at the cursor; the cursor's RVA is
    // (cursor - TEXT_FILE + TEXT_RVA). `call_iat(slot)` emits a `call
    // qword [rip+disp]` against IAT slot `slot` (IAT base RVA 0x3100). `lea`
    // and conditional jumps use absolute target RVAs resolved against the
    // next-instruction RVA, so an off-by-one in the layout fails the disasm KAT.
    const TEXT_FILE: usize = 0x400;
    const TEXT_RVA: u32 = 0x1000;
    const IAT_RVA: u32 = 0x3100;

    let mut cur = TEXT_FILE;
    macro_rules! rva {
        () => {
            (cur as u32 - TEXT_FILE as u32 + TEXT_RVA)
        };
    }
    macro_rules! emit {
        ($($byte:expr),* $(,)?) => {{
            $( b[cur] = $byte; cur += 1; )*
        }};
    }
    // call qword [rip + (IAT_slot - next_rva)]
    macro_rules! call_iat {
        ($slot:expr) => {{
            let target = IAT_RVA + ($slot as u32) * 8;
            // FF 15 <disp32>; instruction is 6 bytes.
            let next = rva!() + 6;
            emit!(0xFF, 0x15);
            let disp = (target as i64 - next as i64) as i32 as u32;
            for byte in disp.to_le_bytes() {
                b[cur] = byte;
                cur += 1;
            }
        }};
    }

    // Forward-referenced label for the shared failure/epilogue tail. We patch
    // the jump displacements once the tail's RVA is known.
    let mut fail_fixups: Vec<(usize, u32)> = Vec::new(); // (disp32 file offset, next_rva)

    // Emit `mov eax, <code>; jmp epilogue` and record the fixup.
    macro_rules! fail_with {
        ($code:expr) => {{
            // B8 <imm32>  mov eax, code
            emit!(0xB8);
            for byte in ($code as u32).to_le_bytes() {
                b[cur] = byte;
                cur += 1;
            }
            // E9 <disp32>  jmp epilogue (patched later)
            emit!(0xE9);
            let disp_off = cur;
            emit!(0x00, 0x00, 0x00, 0x00);
            let next = rva!();
            fail_fixups.push((disp_off, next));
        }};
    }

    // jcc rel32 to fail-with-code: emits the test/cmp's branch. `cond` is the
    // 0x0F-prefixed opcode 2nd byte (e.g. 0x84 = JE, 0x85 = JNE).
    // We don't take that path here; instead each check uses an explicit
    // compare then `je/jne short` over a `fail_with` block, keeping the encoder
    // simple and the disasm KAT readable.

    // ── prologue: save callee-saved regs we use, reserve frame ──────────
    // push rbx; push rsi; push rdi; push r12; push r13; push r14
    emit!(0x53); // push rbx
    emit!(0x56); // push rsi
    emit!(0x57); // push rdi
    emit!(0x41, 0x54); // push r12
    emit!(0x41, 0x55); // push r13
    emit!(0x41, 0x56); // push r14
                       // sub rsp, 0x40  (shadow 0x20 + bytesWritten scratch @ [rsp+0x30], aligned)
    emit!(0x48, 0x83, 0xEC, 0x40);

    // We need the RVA of the wide string L"kernel32.dll" and the ANSI string
    // "ExitProcess" and L"API OK\n". They live at fixed RVAs in .text after the
    // code (see DATA section below). Resolve via lea [rip+disp].
    const WSTR_KERNEL32_RVA: u32 = 0x1300; // L"kernel32.dll\0"
    const ASTR_EXITPROCESS_RVA: u32 = 0x1340; // "ExitProcess\0"
    const WSTR_API_OK_RVA: u32 = 0x1360; // L"API OK\n\0"

    macro_rules! lea {
        // lea <reg>, [rip + (target - next)] ; reg encoded in modrm via $rexw/$modrm
        ($rex:expr, $modrm:expr, $target:expr) => {{
            emit!($rex, 0x8D, $modrm);
            let next = rva!() + 4;
            let disp = ($target as i64 - next as i64) as i32 as u32;
            for byte in disp.to_le_bytes() {
                b[cur] = byte;
                cur += 1;
            }
        }};
    }

    // ── Step 1: GetModuleHandleW(L"kernel32.dll") ──────────────────────
    // lea rcx, [rip+kernel32]   (48 8D 0D)
    lea!(0x48, 0x0D, WSTR_KERNEL32_RVA);
    call_iat!(0); // -> rax = base
                  // test rax, rax ; je fail(1)
    emit!(0x48, 0x85, 0xC0); // test rax,rax
    emit!(0x0F, 0x84); // je rel32 -> fail(1) block (patched: jump just over to the block we emit next)
                       // We emit JE to skip *into* a fail block placed inline right after; simpler:
                       // jump if ZERO to a local fail block. Record displacement to that block.
    let je1_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let je1_next = rva!();
    // mov rsi, rax  (save hKernel32)
    emit!(0x48, 0x89, 0xC6);
    // jmp over the fail(1) block (we lay the fail block immediately, then continue)
    emit!(0xE9);
    let skip1_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let skip1_next = rva!();
    // fail(1) block target:
    let fail1_rva = rva!();
    fail_with!(API_FAIL_GETMODULEHANDLE);
    // patch je1 -> fail1
    {
        let disp = (fail1_rva as i64 - je1_next as i64) as i32 as u32;
        b[je1_off..je1_off + 4].copy_from_slice(&disp.to_le_bytes());
    }
    // continuation after step 1:
    let cont1_rva = rva!();
    {
        let disp = (cont1_rva as i64 - skip1_next as i64) as i32 as u32;
        b[skip1_off..skip1_off + 4].copy_from_slice(&disp.to_le_bytes());
    }

    // ── Step 2: GetProcAddress(rsi, "ExitProcess") ─────────────────────
    // mov rcx, rsi
    emit!(0x48, 0x89, 0xF1);
    // lea rdx, [rip+ExitProcess]   (48 8D 15)
    lea!(0x48, 0x15, ASTR_EXITPROCESS_RVA);
    call_iat!(1); // -> rax = proc addr
    emit!(0x48, 0x85, 0xC0); // test rax,rax
    emit!(0x0F, 0x84);
    let je2_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let je2_next = rva!();
    emit!(0xE9);
    let skip2_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let skip2_next = rva!();
    let fail2_rva = rva!();
    fail_with!(API_FAIL_GETPROCADDRESS);
    {
        let disp = (fail2_rva as i64 - je2_next as i64) as i32 as u32;
        b[je2_off..je2_off + 4].copy_from_slice(&disp.to_le_bytes());
    }
    let cont2_rva = rva!();
    {
        let disp = (cont2_rva as i64 - skip2_next as i64) as i32 as u32;
        b[skip2_off..skip2_off + 4].copy_from_slice(&disp.to_le_bytes());
    }

    // ── Step 3: HeapAlloc(GetProcessHeap(),0,64); write; HeapSize==64 ──
    call_iat!(2); // GetProcessHeap -> rax
                  // mov rdi, rax  (save heap handle)
    emit!(0x48, 0x89, 0xC7);
    // mov rcx, rdi ; xor edx,edx ; mov r8d, 64
    emit!(0x48, 0x89, 0xF9); // mov rcx, rdi
    emit!(0x31, 0xD2); // xor edx, edx
    emit!(0x41, 0xB8, 0x40, 0x00, 0x00, 0x00); // mov r8d, 64
    call_iat!(3); // HeapAlloc -> rax = p
    emit!(0x48, 0x85, 0xC0); // test rax,rax
    emit!(0x0F, 0x84);
    let je3_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let je3_next = rva!();
    emit!(0xE9);
    let skip3_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let skip3_next = rva!();
    let fail3_rva = rva!();
    fail_with!(API_FAIL_HEAPALLOC);
    {
        let disp = (fail3_rva as i64 - je3_next as i64) as i32 as u32;
        b[je3_off..je3_off + 4].copy_from_slice(&disp.to_le_bytes());
    }
    let cont3_rva = rva!();
    {
        let disp = (cont3_rva as i64 - skip3_next as i64) as i32 as u32;
        b[skip3_off..skip3_off + 4].copy_from_slice(&disp.to_le_bytes());
    }
    // mov r12, rax  (save p)
    emit!(0x49, 0x89, 0xC4);
    // byte [r12] = 0xA5      (41 C6 04 24 A5)
    emit!(0x41, 0xC6, 0x04, 0x24, 0xA5);
    // byte [r12+63] = 0x5A   (41 C6 44 24 3F 5A)
    emit!(0x41, 0xC6, 0x44, 0x24, 0x3F, 0x5A);
    // HeapSize(rdi, 0, r12): mov rcx,rdi; xor edx,edx; mov r8, r12
    emit!(0x48, 0x89, 0xF9); // mov rcx, rdi
    emit!(0x31, 0xD2); // xor edx, edx
    emit!(0x4C, 0x89, 0xE0); // mov rax, r12  (use rax as scratch then move to r8)
    emit!(0x49, 0x89, 0xC0); // mov r8, rax
    call_iat!(5); // HeapSize -> rax
                  // cmp rax, 64 ; jne fail(3)
    emit!(0x48, 0x83, 0xF8, 0x40); // cmp rax, 0x40
    emit!(0x0F, 0x85);
    let jne3b_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let jne3b_next = rva!();
    {
        // reuse fail3 block.
        let disp = (fail3_rva as i64 - jne3b_next as i64) as i32 as u32;
        b[jne3b_off..jne3b_off + 4].copy_from_slice(&disp.to_le_bytes());
    }

    // ── Step 4: HeapReAlloc to 128; bytes survive; HeapFree ────────────
    // HeapReAlloc(rdi,0,r12,128): mov rcx,rdi; xor edx,edx; mov r8,r12; mov r9d,128
    emit!(0x48, 0x89, 0xF9); // mov rcx, rdi
    emit!(0x31, 0xD2); // xor edx, edx
    emit!(0x4D, 0x89, 0xE0); // mov r8, r12
    emit!(0x41, 0xB9, 0x80, 0x00, 0x00, 0x00); // mov r9d, 128
    call_iat!(4); // HeapReAlloc -> rax = q
    emit!(0x48, 0x85, 0xC0); // test rax,rax
    emit!(0x0F, 0x84);
    let je4_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let je4_next = rva!();
    emit!(0xE9);
    let skip4_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let skip4_next = rva!();
    let fail4_rva = rva!();
    fail_with!(API_FAIL_HEAPREALLOC);
    {
        let disp = (fail4_rva as i64 - je4_next as i64) as i32 as u32;
        b[je4_off..je4_off + 4].copy_from_slice(&disp.to_le_bytes());
    }
    let cont4_rva = rva!();
    {
        let disp = (cont4_rva as i64 - skip4_next as i64) as i32 as u32;
        b[skip4_off..skip4_off + 4].copy_from_slice(&disp.to_le_bytes());
    }
    // mov r12, rax  (q is the new live pointer)
    emit!(0x49, 0x89, 0xC4);
    // cmp byte [r12], 0xA5 ; jne fail(4)   (41 80 3C 24 A5)
    emit!(0x41, 0x80, 0x3C, 0x24, 0xA5);
    emit!(0x0F, 0x85);
    let jne4a_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let jne4a_next = rva!();
    {
        let disp = (fail4_rva as i64 - jne4a_next as i64) as i32 as u32;
        b[jne4a_off..jne4a_off + 4].copy_from_slice(&disp.to_le_bytes());
    }
    // cmp byte [r12+63], 0x5A ; jne fail(4)  (41 80 7C 24 3F 5A)
    emit!(0x41, 0x80, 0x7C, 0x24, 0x3F, 0x5A);
    emit!(0x0F, 0x85);
    let jne4b_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let jne4b_next = rva!();
    {
        let disp = (fail4_rva as i64 - jne4b_next as i64) as i32 as u32;
        b[jne4b_off..jne4b_off + 4].copy_from_slice(&disp.to_le_bytes());
    }
    // HeapFree(rdi,0,r12): mov rcx,rdi; xor edx,edx; mov r8,r12
    emit!(0x48, 0x89, 0xF9); // mov rcx, rdi
    emit!(0x31, 0xD2); // xor edx, edx
    emit!(0x4D, 0x89, 0xE0); // mov r8, r12
    call_iat!(6); // HeapFree -> eax
                  // test eax,eax ; je fail(4)
    emit!(0x85, 0xC0); // test eax, eax
    emit!(0x0F, 0x84);
    let je4c_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let je4c_next = rva!();
    {
        let disp = (fail4_rva as i64 - je4c_next as i64) as i32 as u32;
        b[je4c_off..je4c_off + 4].copy_from_slice(&disp.to_le_bytes());
    }

    // ── Step 5: TlsAlloc/Set/Get/Free round-trip ──────────────────────
    call_iat!(7); // TlsAlloc -> eax = index
                  // cmp eax, 0xFFFFFFFF ; je fail(5)
    emit!(0x83, 0xF8, 0xFF); // cmp eax, -1
    emit!(0x0F, 0x84);
    let je5a_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let je5a_next = rva!();
    // (patch later to fail5)
    // mov r13d, eax  (save index)  41 89 C5
    emit!(0x41, 0x89, 0xC5);
    // TlsSetValue(r13, 0xCAFE): mov ecx, r13d ; mov edx, 0xCAFE
    emit!(0x44, 0x89, 0xE9); // mov ecx, r13d
    emit!(0xBA, 0xFE, 0xCA, 0x00, 0x00); // mov edx, 0xCAFE
    call_iat!(8); // TlsSetValue -> eax
    emit!(0x85, 0xC0); // test eax,eax
    emit!(0x0F, 0x84);
    let je5b_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let je5b_next = rva!();
    // TlsGetValue(r13): mov ecx, r13d
    emit!(0x44, 0x89, 0xE9); // mov ecx, r13d
    call_iat!(9); // TlsGetValue -> rax
                  // cmp rax, 0xCAFE ; jne fail(5)
    emit!(0x48, 0x3D, 0xFE, 0xCA, 0x00, 0x00); // cmp rax, 0xCAFE
    emit!(0x0F, 0x85);
    let jne5c_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let jne5c_next = rva!();
    // TlsFree(r13): mov ecx, r13d
    emit!(0x44, 0x89, 0xE9); // mov ecx, r13d
    call_iat!(10); // TlsFree -> eax
    emit!(0x85, 0xC0); // test eax,eax
    emit!(0x0F, 0x84);
    let je5d_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let je5d_next = rva!();
    // jmp over the fail(5) block
    emit!(0xE9);
    let skip5_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let skip5_next = rva!();
    let fail5_rva = rva!();
    fail_with!(API_FAIL_TLS);
    for (off, next) in [
        (je5a_off, je5a_next),
        (je5b_off, je5b_next),
        (jne5c_off, jne5c_next),
        (je5d_off, je5d_next),
    ] {
        let disp = (fail5_rva as i64 - next as i64) as i32 as u32;
        b[off..off + 4].copy_from_slice(&disp.to_le_bytes());
    }
    let cont5_rva = rva!();
    {
        let disp = (cont5_rva as i64 - skip5_next as i64) as i32 as u32;
        b[skip5_off..skip5_off + 4].copy_from_slice(&disp.to_le_bytes());
    }

    // ── Step 6: WriteConsoleW(GetStdHandle(-11), L"API OK\n", 7, &n, 0) ─
    // mov ecx, -11 (STD_OUTPUT_HANDLE)
    emit!(0xB9, 0xF5, 0xFF, 0xFF, 0xFF);
    call_iat!(11); // GetStdHandle -> rax
                   // mov rcx, rax
    emit!(0x48, 0x89, 0xC1);
    // lea rdx, [rip+API_OK]
    lea!(0x48, 0x15, WSTR_API_OK_RVA);
    // mov r8d, 7   (chars)
    emit!(0x41, 0xB8, 0x07, 0x00, 0x00, 0x00);
    // lea r9, [rsp+0x30]   (lpNumberOfCharsWritten)  4C 8D 4C 24 30
    emit!(0x4C, 0x8D, 0x4C, 0x24, 0x30);
    // mov qword [rsp+0x20], 0   (lpReserved 5th arg)  48 C7 44 24 20 00 00 00 00
    emit!(0x48, 0xC7, 0x44, 0x24, 0x20, 0x00, 0x00, 0x00, 0x00);
    call_iat!(12); // WriteConsoleW -> eax
    emit!(0x85, 0xC0); // test eax,eax
    emit!(0x0F, 0x84);
    let je6_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let je6_next = rva!();
    emit!(0xE9);
    let skip6_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let skip6_next = rva!();
    let fail6_rva = rva!();
    fail_with!(API_FAIL_WRITECONSOLE);
    {
        let disp = (fail6_rva as i64 - je6_next as i64) as i32 as u32;
        b[je6_off..je6_off + 4].copy_from_slice(&disp.to_le_bytes());
    }
    let cont6_rva = rva!();
    {
        let disp = (cont6_rva as i64 - skip6_next as i64) as i32 as u32;
        b[skip6_off..skip6_off + 4].copy_from_slice(&disp.to_le_bytes());
    }

    // ── success: xor eax,eax (ret 0) then fall into the epilogue ───────
    emit!(0x31, 0xC0); // xor eax, eax

    // ── epilogue (shared by success + all fail_with blocks) ────────────
    let epilogue_rva = rva!();
    // add rsp, 0x40
    emit!(0x48, 0x83, 0xC4, 0x40);
    // pop r14; pop r13; pop r12; pop rdi; pop rsi; pop rbx
    emit!(0x41, 0x5E); // pop r14
    emit!(0x41, 0x5D); // pop r13
    emit!(0x41, 0x5C); // pop r12
    emit!(0x5F); // pop rdi
    emit!(0x5E); // pop rsi
    emit!(0x5B); // pop rbx
                 // ret
    emit!(0xC3);

    // Patch every fail_with jmp to the epilogue.
    for (disp_off, next) in &fail_fixups {
        let disp = (epilogue_rva as i64 - *next as i64) as i32 as u32;
        b[*disp_off..*disp_off + 4].copy_from_slice(&disp.to_le_bytes());
    }

    // Guard: all code must fit before the data strings at RVA 0x1300.
    debug_assert!(
        rva!() <= WSTR_KERNEL32_RVA,
        "api-exercise code overran string area"
    );

    // ── .text data: strings at fixed RVAs ──────────────────────────────
    // L"kernel32.dll\0" @ RVA 0x1300 (file 0x600)
    let mut wf = TEXT_FILE + (WSTR_KERNEL32_RVA - TEXT_RVA) as usize;
    for c in "kernel32.dll".encode_utf16() {
        b[wf..wf + 2].copy_from_slice(&c.to_le_bytes());
        wf += 2;
    }
    // (trailing UTF-16 NUL already zero)
    // "ExitProcess\0" (ANSI) @ RVA 0x1340 (file 0x640)
    let af = TEXT_FILE + (ASTR_EXITPROCESS_RVA - TEXT_RVA) as usize;
    b[af..af + 11].copy_from_slice(b"ExitProcess");
    // L"API OK\n\0" @ RVA 0x1360 (file 0x660)
    let mut okf = TEXT_FILE + (WSTR_API_OK_RVA - TEXT_RVA) as usize;
    for c in API_OK_MSG.encode_utf16() {
        b[okf..okf + 2].copy_from_slice(&c.to_le_bytes());
        okf += 2;
    }

    // ── .idata @ file 0x800 (RVA 0x3000) ───────────────────────────────
    // IMAGE_IMPORT_DESCRIPTOR for kernel32.dll
    const IDATA_FILE: usize = 0x800;
    const IDATA_RVA: u32 = 0x3000;
    let idf = |rva: u32| IDATA_FILE + (rva - IDATA_RVA) as usize;

    // Layout inside .idata:
    //   0x3000: import descriptor (0x14) + null descriptor (0x14) = 0x28
    //   0x3030: ILT (13 entries + null) -> hint/name RVAs
    //   0x30A0: (reserved gap)
    //   0x3100: IAT (13 entries + null), pre-patch == ILT
    //   0x31A0: DLL name "kernel32.dll\0"
    //   0x3200..: hint/name entries
    let ilt_rva: u32 = 0x3030;
    let iat_rva: u32 = IAT_RVA; // 0x3100
    let dllname_rva: u32 = 0x31A0;
    let hint_base_rva: u32 = 0x31C0;
    let hint_stride: u32 = 0x20;

    // Descriptor: OriginalFirstThunk(ILT), TimeDateStamp, ForwarderChain, Name, FirstThunk(IAT)
    put_u32(&mut b, idf(0x3000), ilt_rva); // OriginalFirstThunk
    put_u32(&mut b, idf(0x3000) + 12, dllname_rva); // Name
    put_u32(&mut b, idf(0x3000) + 16, iat_rva); // FirstThunk
                                                // null descriptor at 0x3014..0x3028 stays zero.

    // ILT + IAT entries -> hint/name RVAs.
    for (i, _name) in API_EXERCISE_IMPORTS.iter().enumerate() {
        let hint_rva = hint_base_rva + (i as u32) * hint_stride;
        put_u64(&mut b, idf(ilt_rva) + i * 8, hint_rva as u64);
        put_u64(&mut b, idf(iat_rva) + i * 8, hint_rva as u64);
    }
    // ILT/IAT terminators (the slot after the last entry) stay zero.

    // DLL name.
    let dn = idf(dllname_rva);
    b[dn..dn + 13].copy_from_slice(b"kernel32.dll\0");

    // Hint/name entries: u16 hint (0) then NUL-terminated name.
    for (i, name) in API_EXERCISE_IMPORTS.iter().enumerate() {
        let hf = idf(hint_base_rva + (i as u32) * hint_stride);
        b[hf] = 0;
        b[hf + 1] = 0;
        let nb = name.as_bytes();
        b[hf + 2..hf + 2 + nb.len()].copy_from_slice(nb);
        // trailing NUL already zero.
    }

    b
}

/// The TEB scratch sentinel the gs-base image writes into `arbitrary_user_pointer`
/// (TEB+0x28) before yielding and re-checks after. Survival proves the TEB memory
/// the GS base points at is stable across the context switch.
pub const GSBASE_SENTINEL: u32 = 0xCAFE_F00D;

/// Imports the gs-base image pulls in (present in its IAT but not *called* — the
/// reschedule uses an inline `SYS_YIELD` syscall, not a Sleep shim, so the test
/// does not depend on Sleep actually yielding). The host KAT asserts both
/// resolve to real shims so the IAT-patch path is exercised on this image too.
pub const GSBASE_IMPORTS: &[&str] = &["Sleep", "ExitProcess"];

/// Per-step verdict codes the gs-base entry returns in RAX. `0` = the TEB was
/// visible via `gs:[0x30]`, self-consistent, AND survived a reschedule; any
/// non-zero value names the exact check that failed so the smoketest is
/// diagnosable from the serial log alone.
pub const GSBASE_PASS: u64 = 0;
/// `gs:[0x30]` read back zero before the reschedule — GS base never set, or
/// `SYS_SET_GS_BASE` wrote the wrong MSR (the spec's named wrong-MSR failure).
pub const GSBASE_FAIL_TEB_ZERO: u64 = 1;
/// `[gs:[0x30] + 0x30]` (the TEB self-pointer field) did not equal `gs:[0x30]` —
/// GS base points at something that is not our TEB.
pub const GSBASE_FAIL_SELF_PTR: u64 = 2;
/// `gs:[0x30]` changed across the reschedule — the scheduler did not restore
/// `Task::gs_base` (the spec's named missing-save/restore failure).
pub const GSBASE_FAIL_NOT_RESTORED: u64 = 3;
/// The TEB scratch sentinel did not survive the reschedule — the TEB memory was
/// clobbered or GS pointed elsewhere after the switch.
pub const GSBASE_FAIL_SENTINEL: u64 = 4;

/// Build the "gs-base" image — the FAIL-able proof that `SYS_SET_GS_BASE` points
/// guest `gs:[0x30]` at the TEB and the scheduler preserves it across a context
/// switch. This is the foundation for running real MSVC-CRT `.exe`s, whose
/// `__scrt_common_main_seh` entry reads `gs:[0x30]` before doing anything else.
///
/// Entry point (Microsoft x64 ABI, `extern "win64" fn() -> u64`):
///
/// ```text
/// mov  rax, gs:[0x30]            ; TEB self-pointer
/// test rax, rax ; je fail(1)     ; GS base must be set (non-zero)
/// mov  rsi, rax                  ; save TEB addr (callee-saved)
/// mov  rax, [rsi+0x30]           ; TEB.NtTib.Self field
/// cmp  rax, rsi ; jne fail(2)    ; self-pointer must round-trip
/// mov  eax, GSBASE_SENTINEL
/// mov  [rsi+0x28], rax           ; write sentinel into TEB scratch (0x28)
/// mov  eax, 28 ; syscall         ; SYS_YIELD -> force a context switch
/// mov  rax, gs:[0x30]            ; re-read after the switch
/// cmp  rax, rsi ; jne fail(3)    ; GS base must have survived the switch
/// mov  rax, [rsi+0x28]           ; re-read the sentinel
/// mov  ecx, GSBASE_SENTINEL
/// cmp  rax, rcx ; jne fail(4)    ; sentinel must have survived
/// xor  eax, eax                  ; PASS
/// ret
/// ```
///
/// Preferred base 0x170000000 (distinct from the other bring-up images so all
/// can coexist without relocations; code is RIP-relative). It imports
/// kernel32!{Sleep, ExitProcess} (present, uncalled) to exercise IAT patching.
/// The entry *returns* the verdict in RAX; the host harness reads it. It saves
/// and restores rsi (the only callee-saved register it uses) per the Win64 ABI.
pub fn build_gsbase_exe() -> Vec<u8> {
    const SIZE: usize = 0x800;
    let mut b = vec![0u8; SIZE];

    // ── DOS header ────────────────────────────────────────────────────
    b[0] = b'M';
    b[1] = b'Z';
    put_u32(&mut b, 0x3C, PE_OFFSET as u32);

    // ── PE signature + COFF header ────────────────────────────────────
    b[PE_OFFSET] = b'P';
    b[PE_OFFSET + 1] = b'E';
    put_u16(&mut b, PE_OFFSET + 4, 0x8664); // Machine = AMD64
    put_u16(&mut b, PE_OFFSET + 6, 2); // NumberOfSections
    put_u16(&mut b, PE_OFFSET + 20, 0xF0); // SizeOfOptionalHeader (PE32+)
    put_u16(&mut b, PE_OFFSET + 22, 0x0022); // EXECUTABLE | LARGE_ADDRESS_AWARE

    // ── Optional header (PE32+) ───────────────────────────────────────
    put_u16(&mut b, OPT_OFFSET, 0x020B); // Magic
    b[OPT_OFFSET + 2] = 14; // MajorLinkerVersion
    put_u32(&mut b, OPT_OFFSET + 4, 0x200); // SizeOfCode
    put_u32(&mut b, OPT_OFFSET + 16, 0x1000); // AddressOfEntryPoint
    put_u32(&mut b, OPT_OFFSET + 20, 0x1000); // BaseOfCode
    put_u64(&mut b, OPT_OFFSET + 24, 0x1_7000_0000); // ImageBase (distinct)
    put_u32(&mut b, OPT_OFFSET + 32, 0x1000); // SectionAlignment
    put_u32(&mut b, OPT_OFFSET + 36, 0x200); // FileAlignment
    put_u16(&mut b, OPT_OFFSET + 40, 6); // MajorOperatingSystemVersion
    put_u16(&mut b, OPT_OFFSET + 48, 6); // MajorSubsystemVersion
    put_u32(&mut b, OPT_OFFSET + 56, 0x3000); // SizeOfImage
    put_u32(&mut b, OPT_OFFSET + 60, 0x400); // SizeOfHeaders
    put_u16(&mut b, OPT_OFFSET + 68, 3); // Subsystem = WINDOWS_CUI
    put_u64(&mut b, OPT_OFFSET + 72, 0x10_0000); // SizeOfStackReserve
    put_u64(&mut b, OPT_OFFSET + 80, 0x1000); // SizeOfStackCommit
    put_u64(&mut b, OPT_OFFSET + 88, 0x10_0000); // SizeOfHeapReserve
    put_u64(&mut b, OPT_OFFSET + 96, 0x1000); // SizeOfHeapCommit
    put_u32(&mut b, OPT_OFFSET + 108, 16); // NumberOfRvaAndSizes

    // ── Data directories ──────────────────────────────────────────────
    // [1] IMPORT: descriptor table at RVA 0x2000.
    put_u32(&mut b, DATA_DIR_OFFSET + 8, 0x2000);
    put_u32(&mut b, DATA_DIR_OFFSET + 12, 0x28);
    // [12] IAT: two patched slots at RVA 0x2058 (2 * 8 = 0x10).
    put_u32(&mut b, DATA_DIR_OFFSET + 96, 0x2058);
    put_u32(&mut b, DATA_DIR_OFFSET + 100, 0x10);

    // ── Section table ─────────────────────────────────────────────────
    let s0 = SECTION_TABLE_OFFSET;
    b[s0..s0 + 5].copy_from_slice(b".text");
    put_u32(&mut b, s0 + 8, 0x200); // VirtualSize
    put_u32(&mut b, s0 + 12, 0x1000); // VirtualAddress
    put_u32(&mut b, s0 + 16, 0x200); // SizeOfRawData
    put_u32(&mut b, s0 + 20, 0x400); // PointerToRawData
    put_u32(&mut b, s0 + 36, 0x6000_0020); // CODE | EXECUTE | READ

    let s1 = SECTION_TABLE_OFFSET + 40;
    b[s1..s1 + 6].copy_from_slice(b".idata");
    put_u32(&mut b, s1 + 8, 0x200);
    put_u32(&mut b, s1 + 12, 0x2000);
    put_u32(&mut b, s1 + 16, 0x200);
    put_u32(&mut b, s1 + 20, 0x600);
    put_u32(&mut b, s1 + 36, 0xC000_0040); // INITIALIZED_DATA | READ | WRITE

    // ── .text @ file 0x400 (RVA 0x1000) ───────────────────────────────
    const TEXT_FILE: usize = 0x400;
    const TEXT_RVA: u32 = 0x1000;
    let mut cur = TEXT_FILE;
    macro_rules! rva {
        () => {
            (cur as u32 - TEXT_FILE as u32 + TEXT_RVA)
        };
    }
    macro_rules! emit {
        ($($byte:expr),* $(,)?) => {{
            $( b[cur] = $byte; cur += 1; )*
        }};
    }
    macro_rules! emit_u32 {
        ($v:expr) => {{
            for byte in ($v as u32).to_le_bytes() {
                b[cur] = byte;
                cur += 1;
            }
        }};
    }

    // Forward fixups to the shared epilogue (each fail block jmps to it).
    let mut epilogue_fixups: Vec<(usize, u32)> = Vec::new(); // (disp32 file off, next_rva)
    macro_rules! fail_with {
        ($code:expr) => {{
            emit!(0xB8); // mov eax, imm32
            emit_u32!($code);
            emit!(0xE9); // jmp rel32 -> epilogue (patched later)
            let disp_off = cur;
            emit!(0x00, 0x00, 0x00, 0x00);
            epilogue_fixups.push((disp_off, rva!()));
        }};
    }

    // ── prologue: save rsi (callee-saved per Win64; we use it for the TEB) ─
    emit!(0x56); // push rsi
                 // mov rax, gs:[0x30]   (65 48 8B 04 25 30 00 00 00)
    emit!(0x65, 0x48, 0x8B, 0x04, 0x25);
    emit_u32!(0x30u32);
    // test rax, rax ; je fail(1)
    emit!(0x48, 0x85, 0xC0);
    emit!(0x0F, 0x84);
    let je1_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let je1_next = rva!();
    // mov rsi, rax  (save TEB)
    emit!(0x48, 0x89, 0xC6);
    // mov rax, [rsi+0x30]   (48 8B 46 30)
    emit!(0x48, 0x8B, 0x46, 0x30);
    // cmp rax, rsi ; jne fail(2)
    emit!(0x48, 0x39, 0xF0);
    emit!(0x0F, 0x85);
    let jne2_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let jne2_next = rva!();
    // mov eax, GSBASE_SENTINEL
    emit!(0xB8);
    emit_u32!(GSBASE_SENTINEL);
    // mov [rsi+0x28], rax   (48 89 46 28)
    emit!(0x48, 0x89, 0x46, 0x28);
    // mov eax, 28 (SYS_YIELD)
    emit!(0xB8);
    emit_u32!(28u32);
    // syscall   (0F 05)
    emit!(0x0F, 0x05);
    // mov rax, gs:[0x30]   (re-read after the switch)
    emit!(0x65, 0x48, 0x8B, 0x04, 0x25);
    emit_u32!(0x30u32);
    // cmp rax, rsi ; jne fail(3)
    emit!(0x48, 0x39, 0xF0);
    emit!(0x0F, 0x85);
    let jne3_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let jne3_next = rva!();
    // mov rax, [rsi+0x28]   (re-read sentinel)  (48 8B 46 28)
    emit!(0x48, 0x8B, 0x46, 0x28);
    // mov ecx, GSBASE_SENTINEL
    emit!(0xB9);
    emit_u32!(GSBASE_SENTINEL);
    // cmp rax, rcx ; jne fail(4)
    emit!(0x48, 0x39, 0xC8);
    emit!(0x0F, 0x85);
    let jne4_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let jne4_next = rva!();
    // ── success: xor eax, eax ; jmp epilogue ──────────────────────────
    emit!(0x31, 0xC0); // xor eax, eax
    emit!(0xE9); // jmp epilogue
    let skip_ok_off = cur;
    emit!(0x00, 0x00, 0x00, 0x00);
    let skip_ok_next = rva!();

    // ── inline fail blocks ────────────────────────────────────────────
    let fail1_rva = rva!();
    fail_with!(GSBASE_FAIL_TEB_ZERO);
    let fail2_rva = rva!();
    fail_with!(GSBASE_FAIL_SELF_PTR);
    let fail3_rva = rva!();
    fail_with!(GSBASE_FAIL_NOT_RESTORED);
    let fail4_rva = rva!();
    fail_with!(GSBASE_FAIL_SENTINEL);

    // ── shared epilogue (success + every fail block lands here) ───────
    let epilogue_rva = rva!();
    emit!(0x5E); // pop rsi (balance the single prologue push)
    emit!(0xC3); // ret (RAX already holds the verdict)

    // Patch conditional jumps to their fail blocks.
    for (off, next, target) in [
        (je1_off, je1_next, fail1_rva),
        (jne2_off, jne2_next, fail2_rva),
        (jne3_off, jne3_next, fail3_rva),
        (jne4_off, jne4_next, fail4_rva),
    ] {
        let disp = (target as i64 - next as i64) as i32 as u32;
        b[off..off + 4].copy_from_slice(&disp.to_le_bytes());
    }
    // Patch the success jmp-over and every fail block's jmp to the epilogue.
    {
        let disp = (epilogue_rva as i64 - skip_ok_next as i64) as i32 as u32;
        b[skip_ok_off..skip_ok_off + 4].copy_from_slice(&disp.to_le_bytes());
    }
    for (disp_off, next) in &epilogue_fixups {
        let disp = (epilogue_rva as i64 - *next as i64) as i32 as u32;
        b[*disp_off..*disp_off + 4].copy_from_slice(&disp.to_le_bytes());
    }

    debug_assert!(rva!() <= 0x1200, "gs-base code overran .text");

    // ── .idata @ file 0x600 (RVA 0x2000) ──────────────────────────────
    // IMAGE_IMPORT_DESCRIPTOR for kernel32.dll
    put_u32(&mut b, 0x600, 0x2028); // OriginalFirstThunk (ILT)
    put_u32(&mut b, 0x60C, 0x2070); // Name
    put_u32(&mut b, 0x610, 0x2058); // FirstThunk (IAT)
                                    // (null descriptor at 0x614..0x628 stays zero)

    // ILT @ RVA 0x2028 (file 0x628): two hint/name pointers, then 0.
    put_u64(&mut b, 0x628, 0x2080); // Sleep
    put_u64(&mut b, 0x630, 0x2090); // ExitProcess
                                    // 0x638: ILT terminator (zero)

    // IAT @ RVA 0x2058 (file 0x658): same two, then 0.
    put_u64(&mut b, 0x658, 0x2080); // Sleep slot       (RVA 0x2058)
    put_u64(&mut b, 0x660, 0x2090); // ExitProcess slot (RVA 0x2060)
                                    // 0x668: IAT terminator (zero)

    // DLL name @ RVA 0x2070 (file 0x670)
    b[0x670..0x670 + 13].copy_from_slice(b"kernel32.dll\0");

    // Hint/name @ RVA 0x2080 (file 0x680): Sleep
    b[0x680] = 0;
    b[0x681] = 0;
    b[0x682..0x682 + 6].copy_from_slice(b"Sleep\0");
    // Hint/name @ RVA 0x2090 (file 0x690): ExitProcess
    b[0x690] = 0;
    b[0x691] = 0;
    b[0x692..0x692 + 12].copy_from_slice(b"ExitProcess\0");

    b
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use crate::{pe_loader, winapi_shims};
    use alloc::string::String;

    #[test]
    fn hello_world_pe_parses_as_amd64() {
        let exe = build_hello_world_exe();
        let info = crate::load_pe(&exe).expect("hello-world PE must parse");
        assert_eq!(info.machine, crate::MachineType::Amd64);
        assert_eq!(info.format, crate::PeFormat::Pe32Plus);
        assert_eq!(info.num_sections, 2);
        assert_eq!(info.entry_point_rva, 0x1000);
    }

    #[test]
    fn hello_world_pe_object_crate_agrees() {
        // The strict `object` reader must also accept the image (so a hostile
        // truncated PE is rejected before mapping).
        let exe = build_hello_world_exe();
        let summary = crate::pe_inspect::inspect(&exe).expect("object must parse hello-world PE");
        assert_eq!(summary.arch, object::Architecture::X86_64);
        assert_ne!(summary.entry, 0);
        assert!(summary.sections >= 2);
    }

    #[test]
    fn hello_world_imports_resolve_to_the_three_shims() {
        // The IAT-patch contract: every imported name must resolve to a real,
        // distinct, non-null win64 shim. If any of the three is missing this
        // FAILS — the guest would otherwise hit a fail-loud stub at call time.
        let exe = build_hello_world_exe();
        let pe = pe_loader::parse_pe(&exe).expect("parse_pe");
        let names: alloc::vec::Vec<String> =
            pe.imports.iter().map(|i| i.function_name.clone()).collect();
        assert!(
            names.iter().any(|n| n == "GetStdHandle"),
            "imports={names:?}"
        );
        assert!(names.iter().any(|n| n == "WriteFile"), "imports={names:?}");
        assert!(
            names.iter().any(|n| n == "ExitProcess"),
            "imports={names:?}"
        );

        for n in ["GetStdHandle", "WriteFile", "ExitProcess"] {
            let a = winapi_shims::resolve_shim("kernel32.dll", n);
            assert!(a.is_some(), "{n} must resolve to a shim");
            assert_ne!(a.unwrap(), 0, "{n} shim address must be non-null");
        }
        // The three shims must be three *distinct* addresses.
        let g = winapi_shims::resolve_shim("kernel32.dll", "GetStdHandle").unwrap();
        let w = winapi_shims::resolve_shim("kernel32.dll", "WriteFile").unwrap();
        let e = winapi_shims::resolve_shim("kernel32.dll", "ExitProcess").unwrap();
        assert_ne!(g, w);
        assert_ne!(w, e);
        assert_ne!(g, e);
    }

    #[test]
    fn hello_world_iat_patches_in_vec_backed_loader() {
        // Map + IAT-patch with the host-safe Vec-backed loader (NO sys_mmap —
        // that raw syscall faults on the host test box, see the linuxkpi-harness
        // memory). Confirms the import directory walks, all three thunks are
        // marked resolved, and the patched IAT slots hold the registry's
        // non-zero GetStdHandle/WriteFile addresses (not the pre-patch hint
        // RVAs). A botched .idata layout fails this loudly.
        let exe = build_hello_world_exe();
        let mut reg = pe_loader::DllRegistry::new();
        let loaded = pe_loader::load_pe(&exe, &mut reg).expect("Vec-backed load_pe");
        assert!(loaded.is_64bit);
        assert_eq!(loaded.entry_point, loaded.image_base + 0x1000);
        assert_eq!(loaded.imports.len(), 3, "three imports");
        assert!(loaded.imports.iter().all(|i| i.resolved), "all resolved");

        // IAT slots in the mapped Vec image: RVA 0x2058 (GetStdHandle),
        // 0x2060 (WriteFile). They must now hold the registry addresses.
        let img = &loaded.image_memory;
        let read_u64 = |off: usize| {
            let mut bb = [0u8; 8];
            bb.copy_from_slice(&img[off..off + 8]);
            u64::from_le_bytes(bb)
        };
        let g_addr = reg.resolve("kernel32.dll", "GetStdHandle");
        let w_addr = reg.resolve("kernel32.dll", "WriteFile");
        assert_ne!(g_addr, 0);
        assert_ne!(w_addr, 0);
        assert_eq!(read_u64(0x2058), g_addr, "GetStdHandle IAT slot patched");
        assert_eq!(read_u64(0x2060), w_addr, "WriteFile IAT slot patched");
    }

    #[test]
    fn hello_world_text_decodes_to_the_expected_sequence() {
        // Disassemble the entry .text and confirm it is the GetStdHandle →
        // WriteFile → ret sequence (not a corrupted hand-assembly). This is the
        // FAIL-able guard against an off-by-one in the encoded bytes.
        let exe = build_hello_world_exe();
        let text = &exe[0x400..0x440];
        let decoded = crate::disasm::disassemble(text, 0x1000, 16);
        use iced_x86::Mnemonic;
        let mnem: alloc::vec::Vec<Mnemonic> = decoded.iter().map(|d| d.mnemonic).collect();
        // sub, mov, call, mov, lea, mov, lea, mov, call, add, ret
        assert_eq!(mnem[0], Mnemonic::Sub);
        assert_eq!(mnem[1], Mnemonic::Mov);
        assert_eq!(mnem[2], Mnemonic::Call);
        assert!(mnem.contains(&Mnemonic::Lea));
        assert!(mnem.contains(&Mnemonic::Ret));
        // Exactly two calls (GetStdHandle + WriteFile).
        assert_eq!(
            mnem.iter().filter(|&&m| m == Mnemonic::Call).count(),
            2,
            "expected two calls (GetStdHandle, WriteFile)"
        );
    }

    #[test]
    fn hello_msg_is_nineteen_bytes() {
        assert_eq!(HELLO_MSG.len(), 19);
        assert_eq!(HELLO_MSG, b"Hello from Windows\n");
    }

    // ── api-exercise image (broadened-shim runtime coverage) ───────────────

    #[test]
    fn api_exercise_pe_parses_as_amd64() {
        let exe = build_api_exercise_exe();
        let info = crate::load_pe(&exe).expect("api-exercise PE must parse");
        assert_eq!(info.machine, crate::MachineType::Amd64);
        assert_eq!(info.format, crate::PeFormat::Pe32Plus);
        assert_eq!(info.num_sections, 2);
        assert_eq!(info.entry_point_rva, 0x1000);
    }

    #[test]
    fn api_exercise_pe_object_crate_agrees() {
        // The strict `object` reader must also accept the image.
        let exe = build_api_exercise_exe();
        let summary = crate::pe_inspect::inspect(&exe).expect("object must parse api-exercise PE");
        assert_eq!(summary.arch, object::Architecture::X86_64);
        assert_ne!(summary.entry, 0);
        assert!(summary.sections >= 2);
    }

    #[test]
    fn api_exercise_imports_resolve_to_real_shims() {
        // Every name the image imports must resolve to a real, distinct,
        // non-null kernel32 shim. A missing one would be IAT-stubbed fail-loud
        // and the guest would trap at call time — this FAILS first instead.
        let exe = build_api_exercise_exe();
        let pe = pe_loader::parse_pe(&exe).expect("parse_pe");
        let names: alloc::vec::Vec<String> =
            pe.imports.iter().map(|i| i.function_name.clone()).collect();
        // The parsed import set is exactly API_EXERCISE_IMPORTS (order-agnostic).
        assert_eq!(
            names.len(),
            API_EXERCISE_IMPORTS.len(),
            "parsed imports={names:?}"
        );
        for want in API_EXERCISE_IMPORTS {
            assert!(
                names.iter().any(|n| n == want),
                "missing import {want}; parsed={names:?}"
            );
            let a = winapi_shims::resolve_shim("kernel32.dll", want);
            assert!(a.is_some(), "{want} must resolve to a shim");
            assert_ne!(a.unwrap(), 0, "{want} shim address must be non-null");
        }
        // FAIL-demo guard: a name NOT in the table resolves to None, proving the
        // positive assertions above are not vacuously true.
        assert!(winapi_shims::resolve_shim("kernel32.dll", "NoSuchExport_API").is_none());
    }

    #[test]
    fn api_exercise_iat_patches_in_vec_backed_loader() {
        // Map + IAT-patch with the host-safe Vec-backed loader (NO sys_mmap),
        // confirm every thunk resolved and each IAT slot now holds the
        // registry's shim address (not the pre-patch hint RVA). A botched
        // .idata layout fails this loudly.
        let exe = build_api_exercise_exe();
        let mut reg = pe_loader::DllRegistry::new();
        let loaded = pe_loader::load_pe(&exe, &mut reg).expect("Vec-backed load_pe");
        assert!(loaded.is_64bit);
        assert_eq!(loaded.entry_point, loaded.image_base + 0x1000);
        assert_eq!(loaded.imports.len(), API_EXERCISE_IMPORTS.len());
        assert!(loaded.imports.iter().all(|i| i.resolved), "all resolved");

        let img = &loaded.image_memory;
        let read_u64 = |off: usize| {
            let mut bb = [0u8; 8];
            bb.copy_from_slice(&img[off..off + 8]);
            u64::from_le_bytes(bb)
        };
        // IAT base RVA 0x3100; slot i corresponds to API_EXERCISE_IMPORTS[i].
        for (i, name) in API_EXERCISE_IMPORTS.iter().enumerate() {
            let want = reg.resolve("kernel32.dll", name);
            assert_ne!(want, 0, "{name} registry addr must be non-null");
            assert_eq!(
                read_u64(0x3100 + i * 8),
                want,
                "IAT slot {i} ({name}) must be patched to the shim address"
            );
        }
    }

    #[test]
    fn api_exercise_text_decodes_to_the_expected_call_sequence() {
        // Disassemble the entry .text and confirm the broadened-shim call
        // sequence is intact (not a corrupted hand-assembly). The image makes
        // exactly 13 indirect calls (one per imported shim) and ends every path
        // through a single `ret`. This is the FAIL-able guard against an
        // off-by-one in the encoded displacements.
        let exe = build_api_exercise_exe();
        // Code occupies .text from file 0x400 up to the string area (RVA 0x1300
        // => file 0x700). Decode that whole window; trailing zero padding decodes
        // to `add [rax],al` which we ignore (we only count calls/mnemonics).
        let text = &exe[0x400..0x700];
        let decoded = crate::disasm::disassemble(text, 0x1000, 256);
        use iced_x86::Mnemonic;
        let calls = decoded
            .iter()
            .filter(|d| d.mnemonic == Mnemonic::Call)
            .count();
        assert_eq!(
            calls,
            API_EXERCISE_IMPORTS.len(),
            "expected one indirect call per imported shim ({})",
            API_EXERCISE_IMPORTS.len()
        );
        // First real instruction is the prologue push rbx.
        assert_eq!(decoded[0].mnemonic, Mnemonic::Push);
        // The sequence must contain at least one ret (the shared epilogue).
        assert!(
            decoded.iter().any(|d| d.mnemonic == Mnemonic::Ret),
            "epilogue ret missing"
        );
        // It must perform the heap pattern writes (Mov to memory) and the TLS
        // compares (Cmp). Presence of Lea proves the RIP-relative string loads.
        assert!(decoded.iter().any(|d| d.mnemonic == Mnemonic::Lea));
        assert!(decoded.iter().any(|d| d.mnemonic == Mnemonic::Cmp));
    }

    #[test]
    fn api_exercise_strings_are_embedded_at_fixed_rvas() {
        // The image embeds L"kernel32.dll" (step 1 arg), "ExitProcess" (step 2
        // arg), and L"API OK\n" (step 6 output). Verify the exact bytes landed
        // at the RVAs the code's lea displacements point to (file = RVA - 0x1000
        // + 0x400 inside .text).
        let exe = build_api_exercise_exe();
        // .text: PointerToRawData 0x400, VirtualAddress 0x1000, so
        // file = RVA - 0x1000 + 0x400.
        // L"kernel32.dll" @ RVA 0x1300 -> file 0x700
        let w: alloc::vec::Vec<u16> = "kernel32.dll".encode_utf16().collect();
        for (i, &c) in w.iter().enumerate() {
            let off = 0x700 + i * 2;
            assert_eq!(u16::from_le_bytes([exe[off], exe[off + 1]]), c);
        }
        // "ExitProcess" @ RVA 0x1340 -> file 0x740
        assert_eq!(&exe[0x740..0x740 + 11], b"ExitProcess");
        assert_eq!(exe[0x740 + 11], 0, "ExitProcess must be NUL-terminated");
        // L"API OK\n" @ RVA 0x1360 -> file 0x760
        let ok: alloc::vec::Vec<u16> = API_OK_MSG.encode_utf16().collect();
        assert_eq!(ok.len(), 7);
        for (i, &c) in ok.iter().enumerate() {
            let off = 0x760 + i * 2;
            assert_eq!(u16::from_le_bytes([exe[off], exe[off + 1]]), c);
        }
    }

    #[test]
    fn api_pass_is_zero_and_fail_codes_are_distinct() {
        // The host decodes the RAX step code; the constants must be the 0..=6
        // contract the smoketest message documents.
        let codes = [
            API_PASS,
            API_FAIL_GETMODULEHANDLE,
            API_FAIL_GETPROCADDRESS,
            API_FAIL_HEAPALLOC,
            API_FAIL_HEAPREALLOC,
            API_FAIL_TLS,
            API_FAIL_WRITECONSOLE,
        ];
        assert_eq!(API_PASS, 0);
        for i in 0..codes.len() {
            for j in (i + 1)..codes.len() {
                assert_ne!(codes[i], codes[j], "fail codes must be distinct");
            }
        }
        assert_eq!(API_FAIL_WRITECONSOLE, 6);
    }

    // ── gs-base image ──────────────────────────────────────────────────────

    #[test]
    fn gsbase_pe_parses_as_amd64() {
        let exe = build_gsbase_exe();
        let info = crate::load_pe(&exe).expect("gs-base PE must parse");
        assert_eq!(info.machine, crate::MachineType::Amd64);
        assert_eq!(info.format, crate::PeFormat::Pe32Plus);
        assert_eq!(info.num_sections, 2);
        assert_eq!(info.entry_point_rva, 0x1000);
    }

    #[test]
    fn gsbase_pe_object_crate_agrees() {
        // The strict `object` reader must also accept the image.
        let exe = build_gsbase_exe();
        let summary = crate::pe_inspect::inspect(&exe).expect("object must parse gs-base PE");
        assert_eq!(summary.arch, object::Architecture::X86_64);
        assert_ne!(summary.entry, 0);
        assert!(summary.sections >= 2);
    }

    #[test]
    fn gsbase_pe_verdict_codes_are_distinct() {
        let codes = [
            GSBASE_PASS,
            GSBASE_FAIL_TEB_ZERO,
            GSBASE_FAIL_SELF_PTR,
            GSBASE_FAIL_NOT_RESTORED,
            GSBASE_FAIL_SENTINEL,
        ];
        assert_eq!(GSBASE_PASS, 0);
        for i in 0..codes.len() {
            for j in (i + 1)..codes.len() {
                assert_ne!(codes[i], codes[j], "gs-base verdict codes must be distinct");
            }
        }
    }

    #[test]
    fn gsbase_pe_text_decodes_to_the_expected_sequence() {
        // Disassemble the entry .text and confirm the hand-assembly is the
        // gs:[0x30] read -> reschedule -> re-read sequence (not corrupted bytes).
        // FAIL-able guard against an off-by-one in the encoded displacements.
        let exe = build_gsbase_exe();
        // .text: file 0x400, RVA 0x1000. Code is well under 0x100 bytes.
        let text = &exe[0x400..0x4C0];
        let decoded = crate::disasm::disassemble(text, 0x1000, 64);
        use iced_x86::Mnemonic;
        // First instruction is the prologue `push rsi`.
        assert_eq!(decoded[0].mnemonic, Mnemonic::Push);
        // Exactly one `syscall` (the SYS_YIELD reschedule).
        assert_eq!(
            decoded
                .iter()
                .filter(|d| d.mnemonic == Mnemonic::Syscall)
                .count(),
            1,
            "expected exactly one syscall (SYS_YIELD)"
        );
        // It reads gs:[0x30] at least twice (before + after the reschedule):
        // those are `mov` with a GS segment override. We at least require two
        // `mov` reads of memory plus the comparisons + final `ret`.
        assert!(
            decoded.iter().any(|d| d.mnemonic == Mnemonic::Ret),
            "ret missing"
        );
        assert!(
            decoded
                .iter()
                .filter(|d| d.mnemonic == Mnemonic::Cmp)
                .count()
                >= 2,
            "expected the self-pointer + survival compares"
        );
    }
}
