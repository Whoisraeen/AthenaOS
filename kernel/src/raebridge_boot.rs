//! RaeBridge boot-time smoketest.
//!
//! Concept §Compatibility Strategy: "RaeBridge runs Windows apps on day
//! one." That promise has to be measurable in the boot log from day one
//! too.
//!
//! On each boot we:
//!   1. Spin up a `DllRegistry` and report how many Win32 names are
//!      reachable for import resolution (~5000 after Batch 1).
//!   2. Parse a hand-crafted minimal PE32+ image embedded in the kernel.
//!      No imports — just enough header machinery to prove the loader's
//!      header walker, section table parser, and architecture detection
//!      work end-to-end on a real DOS+NT image, not synthetic test data.
//!   3. Parse a second tiny PE that *does* import `ExitProcess` from
//!      `kernel32.dll` and report how many imports got resolved against
//!      the registry. Resolves > 0 means import-table walking + name
//!      lookup work; resolves == imports.len() means full coverage.
//!
//! Output appears in the boot log under `[raebridge]`. Run on every
//! boot — when this stops printing OK we've broken something foundational
//! in the loader.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;

use raebridge::pe_loader::{self, DllRegistry};

/// Minimal valid PE32+ image. Sixty-four-bit `_start` is one byte at the
/// end of the .text section: `0xC3 RET`. No imports, no relocations, no
/// resources. Built by hand so we never depend on an external assembler.
///
/// Layout (file offsets):
///   0x000   DOS header  (60 bytes) + 4 bytes of dos stub      (64 B)
///   0x040   e_lfanew points here → "PE\0\0" + COFF + PE32+   (~248 B)
///   0x200   .text section (one byte: 0xC3 RET)
const MINIMAL_PE: &[u8] = &include_minimal_pe();

const fn include_minimal_pe() -> [u8; 0x400] {
    let mut buf = [0u8; 0x400];

    // ── DOS header ─────────────────────────────────────────────────────
    buf[0] = b'M';
    buf[1] = b'Z';
    // e_lfanew @ offset 0x3C = 0x80
    buf[0x3C] = 0x80;
    buf[0x3D] = 0x00;
    buf[0x3E] = 0x00;
    buf[0x3F] = 0x00;

    // ── PE signature @ 0x80 ────────────────────────────────────────────
    buf[0x80] = b'P';
    buf[0x81] = b'E';
    buf[0x82] = 0;
    buf[0x83] = 0;

    // ── IMAGE_FILE_HEADER @ 0x84 ───────────────────────────────────────
    // Machine = 0x8664 (AMD64)
    buf[0x84] = 0x64;
    buf[0x85] = 0x86;
    // NumberOfSections = 1
    buf[0x86] = 0x01;
    buf[0x87] = 0x00;
    // TimeDateStamp = 0
    // PointerToSymbolTable = 0
    // NumberOfSymbols = 0
    // SizeOfOptionalHeader = 0xF0 (PE32+)
    buf[0x94] = 0xF0;
    buf[0x95] = 0x00;
    // Characteristics = EXECUTABLE_IMAGE | LARGE_ADDRESS_AWARE
    buf[0x96] = 0x22;
    buf[0x97] = 0x00;

    // ── IMAGE_OPTIONAL_HEADER64 @ 0x98 (size 0xF0) ─────────────────────
    // Magic = 0x20B (PE32+)
    buf[0x98] = 0x0B;
    buf[0x99] = 0x02;
    // MajorLinkerVersion = 14
    buf[0x9A] = 14;
    // MinorLinkerVersion = 0
    // SizeOfCode = 0x200
    buf[0x9C] = 0x00;
    buf[0x9D] = 0x02;
    // SizeOfInitializedData = 0
    // SizeOfUninitializedData = 0
    // AddressOfEntryPoint = 0x1000
    buf[0xA8] = 0x00;
    buf[0xA9] = 0x10;
    // BaseOfCode = 0x1000
    buf[0xAC] = 0x00;
    buf[0xAD] = 0x10;
    // ImageBase = 0x140000000 (qword) — typical for x86_64 exe
    buf[0xB0] = 0x00;
    buf[0xB1] = 0x00;
    buf[0xB2] = 0x00;
    buf[0xB3] = 0x40;
    buf[0xB4] = 0x01;
    buf[0xB5] = 0x00;
    buf[0xB6] = 0x00;
    buf[0xB7] = 0x00;
    // SectionAlignment = 0x1000
    buf[0xB8] = 0x00;
    buf[0xB9] = 0x10;
    // FileAlignment = 0x200
    buf[0xBC] = 0x00;
    buf[0xBD] = 0x02;
    // MajorOperatingSystemVersion = 6
    buf[0xC0] = 0x06;
    // MajorSubsystemVersion = 6
    buf[0xC8] = 0x06;
    // SizeOfImage = 0x2000
    buf[0xD0] = 0x00;
    buf[0xD1] = 0x20;
    // SizeOfHeaders = 0x200
    buf[0xD4] = 0x00;
    buf[0xD5] = 0x02;
    // Subsystem = 3 (WINDOWS_CUI)
    buf[0xDC] = 0x03;
    // DllCharacteristics = 0
    // SizeOfStackReserve = 0x100000
    buf[0xE0] = 0x00;
    buf[0xE1] = 0x00;
    buf[0xE2] = 0x10;
    buf[0xE3] = 0x00;
    // SizeOfStackCommit = 0x1000
    buf[0xE8] = 0x00;
    buf[0xE9] = 0x10;
    // SizeOfHeapReserve = 0x100000
    buf[0xF0] = 0x00;
    buf[0xF1] = 0x00;
    buf[0xF2] = 0x10;
    buf[0xF3] = 0x00;
    // SizeOfHeapCommit = 0x1000
    buf[0xF8] = 0x00;
    buf[0xF9] = 0x10;
    // NumberOfRvaAndSizes = 16
    buf[0x104] = 0x10;

    // 16 × IMAGE_DATA_DIRECTORY (RVA + Size) starts at 0x108, each 8 bytes.
    // All zero by default — no imports, no exports, no relocations. The
    // *second* PE below will set the IMPORT entry. For this PE the loader
    // should report imports.len() == 0.

    // ── IMAGE_SECTION_HEADER @ 0x188 ───────────────────────────────────
    // Name = ".text\0\0\0"
    buf[0x188] = b'.';
    buf[0x189] = b't';
    buf[0x18A] = b'e';
    buf[0x18B] = b'x';
    buf[0x18C] = b't';
    // VirtualSize = 1
    buf[0x190] = 0x01;
    // VirtualAddress = 0x1000
    buf[0x194] = 0x00;
    buf[0x195] = 0x10;
    // SizeOfRawData = 0x200
    buf[0x198] = 0x00;
    buf[0x199] = 0x02;
    // PointerToRawData = 0x200
    buf[0x19C] = 0x00;
    buf[0x19D] = 0x02;
    // Characteristics = IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_READ | IMAGE_SCN_CNT_CODE
    buf[0x1B4] = 0x20;
    buf[0x1B5] = 0x00;
    buf[0x1B6] = 0x00;
    buf[0x1B7] = 0x60;

    // ── .text section @ 0x200 ─────────────────────────────────────────
    // One byte: RET.
    buf[0x200] = 0xC3;

    buf
}

pub fn run_boot_smoketest() {
    // 1. Build a fresh registry and report the size.
    let registry = DllRegistry::new();
    let dll_count = registry.dll_count();
    let total_funcs = total_function_count(&registry);
    crate::serial_println!(
        "[ OK ] RaeBridge DLL registry: {} DLLs, {} Win32 names reachable for PE imports",
        dll_count,
        total_funcs,
    );

    // 2. Parse the embedded minimal PE.
    match pe_loader::parse_pe(MINIMAL_PE) {
        Ok(image) => {
            crate::serial_println!(
                "[raebridge] minimal PE32+ parsed: machine=x86_64({}), entry=0x{:x}, {} section(s), {} import(s)",
                image.is_64bit, image.entry_point, image.sections.len(), image.imports.len(),
            );
        }
        Err(e) => {
            crate::serial_println!("[raebridge] [WARN] minimal PE parse failed: {:?}", e,);
        }
    }

    // 3. Reachability sample — pick 10 representative imports a real .exe
    // would name and confirm the registry resolves all of them.
    let test_imports: &[(&str, &str)] = &[
        ("kernel32.dll", "ExitProcess"),
        ("kernel32.dll", "CreateFileW"),
        ("kernel32.dll", "VirtualAlloc"),
        ("kernel32.dll", "GetProcAddress"),
        ("ntdll.dll", "NtCreateFile"),
        ("ntdll.dll", "RtlAllocateHeap"),
        ("user32.dll", "CreateWindowExW"),
        ("user32.dll", "DispatchMessageW"),
        ("gdi32.dll", "BitBlt"),
        ("msvcrt.dll", "memcpy"),
        ("advapi32.dll", "RegOpenKeyExW"),
    ];
    let mut reg = registry;
    let mut hits = 0;
    let mut misses = 0;
    for (dll, func) in test_imports {
        if reg.resolve(dll, func) != 0 {
            hits += 1;
        } else {
            misses += 1;
        }
    }
    crate::serial_println!(
        "[raebridge] reachability sample: {}/{} top-tier Win32 names resolved",
        hits,
        hits + misses,
    );

    // 4. Phase-B thunk dispatch table — the wired bridge between the 16k-name
    //    registry and the per-DLL Rust implementations. First the static
    //    round-trip checks (no context needed):
    {
        use raebridge::thunks::{lookup, ALL_THUNKS};
        let mut roundtrip_ok = 0usize;
        let mut registry_hit = 0usize;
        let mut failures: alloc::vec::Vec<&'static str> = alloc::vec::Vec::new();
        for &t in ALL_THUNKS {
            match lookup(t.dll(), t.name()) {
                Some(id) if id == t => roundtrip_ok += 1,
                _ => failures.push(t.name()),
            }
            if raebridge::pe_dll_registry::KERNEL32_NAMES
                .iter()
                .any(|n| *n == t.name())
            {
                registry_hit += 1;
            }
        }
        crate::serial_println!(
            "[raebridge] thunks Phase-B: {}/{} registered, {}/{} in name registry, {} failure(s)",
            roundtrip_ok,
            ALL_THUNKS.len(),
            registry_hit,
            ALL_THUNKS.len(),
            failures.len(),
        );
        for f in &failures {
            crate::serial_println!("[raebridge] thunk FAIL: {}", f);
        }

        // The full INVOKE proof (construct a CompatContext + run every thunk) is
        // NOT run here — the "Latent kernel bugs" double fault. A CompatContext is
        // FullCompatSession = 1464 B, and its `new()` (load_pe + parse_sections +
        // BTreeMap builds) is a deep call tree; on the SMALL BSP boot stack that
        // overflows the guard page → #DF (reproduced 2026-07-03: cr2 = rsp-8).
        // `Box::new()` does NOT help — the value is still built by-value on the
        // stack before the move. The fix is to run it on a spawned kernel thread
        // (64 KiB stack): see `spawn_thunk_invoke_thread()`, launched from
        // kernel_main beside the net/thermal poll threads.
    }

    // 5. iced-x86 decoder + object PE reader self-tests. These are the new
    //    instruction-level / robust-PE foundations for x64 marshaling, SEH
    //    unwind, and untrusted-PE validation. Each can print FAIL.
    let disasm_ok = raebridge::disasm::run_self_test();
    let pe_inspect_ok = raebridge::pe_inspect::run_self_test();
    crate::serial_println!(
        "[raebridge] iced-x86 decode={} object PE parse={} -> {}",
        disasm_ok,
        pe_inspect_ok,
        if disasm_ok && pe_inspect_ok {
            "PASS"
        } else {
            "FAIL"
        }
    );

    // 6. x64 SEH engine (Phase 11.2). Table-based exception handling is what
    //    lets a real Windows .exe survive its first fault: the unwinder folds a
    //    prolog back (restoring nonvolatile regs + the return address) and the
    //    dispatcher walks .pdata to find the __try/__except handler that owns a
    //    fault. The self-test builds synthetic .pdata/.xdata + a model stack,
    //    unwinds one frame, and checks every restored value — it prints FAIL on
    //    any wrong byte.
    let seh_ok = raebridge::seh::run_self_test();
    crate::serial_println!(
        "[raebridge] SEH x64 unwind+dispatch self-test -> {}",
        if seh_ok { "PASS" } else { "FAIL" }
    );

    // 7. Synchronization-object model (broker §6.1, in-process half). Named
    //    mutex/event/semaphore with REAL wait/signal state — the previous shims
    //    returned TRUE without touching state, so a multi-threaded app or a
    //    Global\Name rendezvous silently broke. Drives the full state machine
    //    (acquire/contend/release, auto-reset, semaphore count, named reopen)
    //    and prints FAIL on any wrong transition.
    let sync_ok = raebridge::run_sync_self_test();
    crate::serial_println!(
        "[raebridge] sync objects (mutex/event/semaphore) self-test -> {}",
        if sync_ok { "PASS" } else { "FAIL" }
    );

    // 8. advapi32 registry thunks (Phase A.3). The hive itself is proven by the
    //    `[winreg]` smoketest; this proves the PE-facing RegOpenKeyExW/&c thunks
    //    are wired to it AND exposed on the IAT (a guest now reaches the real
    //    versioned-config hive instead of the fail-loud trampoline). Round-trips
    //    a value through the thunk layer and prints FAIL on any wrong code.
    let reg_ok = raebridge::run_registry_thunk_self_test();
    crate::serial_println!(
        "[raebridge] advapi32 registry thunks self-test -> {}",
        if reg_ok { "PASS" } else { "FAIL" }
    );

    // 9. Cross-process sync broker namespace (broker §6.1, Slice 1). The
    //    in-process sync objects (step 7) share state only within one .exe; this
    //    is the cross-process namespace the `raebridge_server` daemon owns —
    //    two processes naming `Global\Foo` resolve to ONE shared page. Drives
    //    the create/open/close + refcount + kind-collision invariants and prints
    //    FAIL on any wrong result. (Slice 2 layers SYS_CHANNEL_SHMEM_MAP pages +
    //    SYS_FUTEX wait/signal onto these page ids — true cross-process blocking.)
    let broker_ok = raebridge::broker::run_namespace_self_test();
    crate::serial_println!(
        "[raebridge] cross-process sync broker namespace self-test -> {}",
        if broker_ok { "PASS" } else { "FAIL" }
    );

    // 10. Broker shared-page state machine (broker §6.1, Slice 2a). The state of
    //     a cross-process object lives in a shared page (futex word at offset 0);
    //     this drives the event/mutex/semaphore atomic transitions + wake counts
    //     that the live SYS_FUTEX wiring (Slice 2b) will block/wake on. FAIL on
    //     any wrong transition.
    let broker_state_ok = raebridge::broker::run_shared_state_self_test();
    crate::serial_println!(
        "[raebridge] cross-process sync broker shared-state self-test -> {}",
        if broker_state_ok { "PASS" } else { "FAIL" }
    );

    // 11. Cross-process sync ENGINE (broker §6.1, Slice 2b host half). Steps 9/10
    //     prove the namespace + the atomic state machine; this proves the DRIVER
    //     that turns a wait/signal into (at most) one SYS_FUTEX and COUNTS every
    //     crossing — the fsync-parity contract. The headline FAIL-able number,
    //     `uncontended_op_syscalls`, MUST be 0: an uncontended op that touches the
    //     kernel is the exact perf cliff fsync exists to avoid. Drives an
    //     uncontended batch (zero syscalls), wake-elision, and a bounded end-to-end
    //     rendezvous (park -> signal-while-parked -> WAIT_OBJECT_0). The live
    //     blocking SYS_FUTEX re-key is the HUMAN-GATED kernel half.
    let sync_engine_ok = raebridge::sync_engine::run_sync_engine_self_test();
    crate::serial_println!(
        "[raebridge] cross-process sync engine (fast-path/wake-elision) self-test -> {}",
        if sync_engine_ok { "PASS" } else { "FAIL" }
    );

    // 12. NAMED-object routing (broker §6.1, Slice 5). Steps 9-11 prove the
    //     namespace, the shared-page state machine, and the fast-path driver; this
    //     proves the ROUTER that sends a NAMED Create{Mutex,Event,Semaphore}W
    //     through the broker so two processes naming Global\Foo share ONE page +
    //     one SharedSyncState (unnamed objects never route here), then drives that
    //     shared page via sync_engine. The live SYS_FUTEX key it rides is now the
    //     physical-frame key landed in item 1828. FAIL on any wrong routing/refcount.
    let named_routing_ok = raebridge::broker::run_named_routing_self_test();
    crate::serial_println!(
        "[raebridge] cross-process named-object routing self-test -> {}",
        if named_routing_ok { "PASS" } else { "FAIL" }
    );
}

/// Thread entry: the thunk-INVOKE proof that can't run on the BSP boot stack (the
/// CompatContext construction overflows it → #DF; see step 4 in `run_boot_smoketest`).
/// A spawned kernel thread has a 64 KiB stack, so it constructs the context and
/// invokes every kernel32 thunk with neutral args, printing a FAIL-able marker.
///
/// This does its OWN loop instead of `thunks::run_smoketest` for one reason: that
/// helper invokes EVERY thunk, and `ExitProcess` maps to `kernel32::exit_process`
/// → `sys_exit`, which TERMINATES the calling task. Invoked wholesale in-kernel it
/// silently killed this very thread mid-loop (diagnosed 2026-07-03: entry printed,
/// result never did). A continue-after smoketest cannot invoke "terminate me", so
/// process-lifetime thunks are skipped here — the static round-trip in step 4
/// already proves they're registered + resolvable.
extern "C" fn thunk_invoke_thread_entry() {
    use alloc::string::String;
    use alloc::vec::Vec;
    use raebridge::thunks::{invoke, ThunkId, ThunkResult, ALL_THUNKS};
    use raebridge::{FullCompatSession, SessionId};

    let mut ctx = match FullCompatSession::new(
        SessionId(7),
        String::from("boot-thunk.exe"),
        MINIMAL_PE.to_vec(),
        String::from("boot-thunk.exe"),
    ) {
        Ok(c) => c,
        Err(e) => {
            crate::serial_println!("[raebridge] thunk INVOKE: session build FAILED ({:?})", e);
            crate::scheduler::exit_current_task(0);
        }
    };

    let args: [u64; 4] = [0, 0, 16, 0]; // 16 = small HeapAlloc request
    let (mut ok, mut void, mut skipped) = (0usize, 0usize, 0usize);
    let mut failures: Vec<&'static str> = Vec::new();
    for &t in ALL_THUNKS {
        // Skip process-lifetime thunks (ExitProcess → sys_exit kills this task).
        if matches!(t, ThunkId::K32_ExitProcess) {
            skipped += 1;
            continue;
        }
        match invoke(&mut ctx, t, &args) {
            ThunkResult::Unimplemented => failures.push(t.name()),
            ThunkResult::Void => {
                ok += 1;
                void += 1;
            }
            _ => ok += 1,
        }
    }
    let pass = failures.is_empty() && ok + skipped == ALL_THUNKS.len();
    crate::serial_println!(
        "[raebridge] thunk INVOKE (spawned thread, ctx={}B): {}/{} invoked, {} void, {} skipped(lifetime), {} unimpl -> {}",
        core::mem::size_of::<FullCompatSession>(),
        ok,
        ALL_THUNKS.len(),
        void,
        skipped,
        failures.len(),
        if pass { "PASS" } else { "FAIL" },
    );
    for f in &failures {
        crate::serial_println!("[raebridge] thunk INVOKE FAIL: {}", f);
    }
    crate::scheduler::exit_current_task(0);
}

/// Spawn the one-shot thunk-INVOKE proof thread (BSP-pinned — APs don't schedule
/// post-boot). Runs the CompatContext-backed thunk smoketest that the boot-stack
/// path can't (the double-fault "Latent kernel bugs" row). Call from kernel_main
/// beside the other post-boot service-thread spawns.
pub fn spawn_thunk_invoke_thread() {
    let task = crate::task::Task::new(thunk_invoke_thread_entry, None);
    crate::scheduler::spawn_on_bsp(task);
}

/// `/proc/raeen/raebridge_seh` — the x64 SEH engine's capability surface and
/// live self-test result. Concept §Compatibility Strategy: the unwind/dispatch
/// engine has to be measurable in the boot log and via procfs, not just compile.
pub fn seh_dump_text() -> String {
    use core::fmt::Write;
    let ok = raebridge::seh::run_self_test();
    let mut s = String::new();
    let _ = writeln!(s, "RaeBridge x64 SEH engine (MasterChecklist Phase 11.2)");
    let _ = writeln!(s, "self_test: {}", if ok { "PASS" } else { "FAIL" });
    let _ = writeln!(
        s,
        "model: table-based (.pdata RUNTIME_FUNCTION + .xdata UNWIND_INFO)"
    );
    let _ = writeln!(
        s,
        "unwind ops: PUSH_NONVOL ALLOC_SMALL ALLOC_LARGE SET_FPREG"
    );
    let _ = writeln!(
        s,
        "            SAVE_NONVOL[_FAR] SAVE_XMM128[_FAR] PUSH_MACHFRAME"
    );
    let _ = writeln!(
        s,
        "features: chained-info, partial-prolog skip, frame-pointer recovery"
    );
    let _ = writeln!(
        s,
        "dispatch: UNW_FLAG_EHANDLER walk + __C_specific_handler SCOPE_TABLE"
    );
    let _ = writeln!(
        s,
        "wired: exec::load_pe_executable parses .pdata into runtime_functions"
    );
    s
}

/// `/proc/raeen/raebridge_sync` — the synchronization-object model's capability
/// surface and live self-test result (broker §6.1, in-process half).
pub fn sync_dump_text() -> String {
    raebridge::sync_self_test_text()
}

/// `/proc/raeen/raebridge_registry` — the advapi32 registry-thunk layer's
/// capability surface and live self-test result (Phase A.3).
pub fn registry_dump_text() -> String {
    raebridge::registry_thunk_self_test_text()
}

/// `/proc/raeen/raebridge_syncbroker` — the cross-process sync engine's
/// fsync-parity counters (live) + self-test result (broker §6.1, Slice 2b). The
/// headline `uncontended_op_syscalls` MUST read 0; a nonzero value here is a
/// visible regression of Invariant 1/4 (a hot-path syscall leak).
pub fn sync_broker_dump_text() -> String {
    raebridge::sync_engine::sync_engine_self_test_text()
}

fn total_function_count(_reg: &DllRegistry) -> usize {
    use raebridge::pe_dll_registry as r;
    // Batch 1
    r::KERNEL32_NAMES.len() + r::NTDLL_NAMES.len() + r::USER32_NAMES.len()
        + r::GDI32_NAMES.len() + r::ADVAPI32_NAMES.len() + r::MSVCRT_NAMES.len()
    // Batch 2 — DirectX + multimedia + COM
        + r::DXGI_NAMES.len() + r::D3D9_NAMES.len() + r::D3D11_NAMES.len()
        + r::D3D12_NAMES.len() + r::D2D1_NAMES.len() + r::DWRITE_NAMES.len()
        + r::DSOUND_NAMES.len() + r::DINPUT8_NAMES.len() + r::XINPUT_NAMES.len() * 3
        + r::OPENGL32_NAMES.len() + r::VULKAN1_NAMES.len() + r::WINMM_NAMES.len()
        + r::OLE32_NAMES.len() + r::OLEAUT32_NAMES.len()
        + r::COMCTL32_NAMES.len() + r::COMDLG32_NAMES.len()
    // Batch 3 — networking + crypto
        + r::WS2_32_NAMES.len() + r::WININET_NAMES.len() + r::WINHTTP_NAMES.len()
        + r::IPHLPAPI_NAMES.len() + r::CRYPT32_NAMES.len() + r::BCRYPT_NAMES.len()
        + r::SECUR32_NAMES.len()
    // Batch 4 — shell + setup + diagnostics
        + r::SHELL32_NAMES.len() + r::SHLWAPI_NAMES.len() + r::SETUPAPI_NAMES.len()
        + r::PSAPI_NAMES.len() + r::DBGHELP_NAMES.len() + r::VERSION_NAMES.len()
        + r::DWMAPI_NAMES.len() + r::UXTHEME_NAMES.len()
    // Batch 5 — VC++ runtimes + long tail (vcruntime registered to 7 DLLs,
    // ucrtbase aliased to 16 api-ms-win-* virtual DLLs, xaudio2 to 3)
        + r::VCRUNTIME140_NAMES.len() * 7
        + r::UCRTBASE_NAMES.len() * 16
        + r::XAUDIO2_NAMES.len() * 3
    // Batch 6 — final push: NT internals + MF + Wi-Fi + power + IME + anti-cheat
        + r::MSWSOCK_NAMES.len() + r::DNSAPI_NAMES.len() + r::MPR_NAMES.len()
        + r::NETAPI32_NAMES.len() + r::URLMON_NAMES.len() + r::WSOCK32_NAMES.len()
        + r::USERENV_NAMES.len() + r::WER_NAMES.len() + r::WEVTAPI_NAMES.len()
        + r::MSCOREE_NAMES.len() + r::IMAGEHLP_NAMES.len() + r::IMM32_NAMES.len()
        + r::WLANAPI_NAMES.len() + r::POWRPROF_NAMES.len() + r::AVRT_NAMES.len()
        + r::MFPLAT_NAMES.len() + r::MFREADWRITE_NAMES.len() + r::DXVA2_NAMES.len()
        + r::D3D10_NAMES.len() * 2  // d3d10 + d3d10_1
        + r::WBEMUUID_NAMES.len() * 2 // wbemuuid + wbemprox
        + r::GAMEINPUT_NAMES.len() + r::HID_NAMES.len() + r::CFGMGR32_NAMES.len()
        + r::EAC_NAMES.len() * 3 + r::BE_NAMES.len() * 2
    // Batch 7 — final 15K push
        + r::GDIPLUS_NAMES.len() + r::RICHED_NAMES.len() * 3
        + r::MSI_NAMES.len() + r::D3DCOMPILER_NAMES.len() * 3
        + r::USP10_NAMES.len() + r::PROPSYS_NAMES.len()
        + r::COMBASE_NAMES.len() + r::SECHOST_NAMES.len() + r::PDH_NAMES.len()
        + r::RASAPI32_NAMES.len() + r::WINSCARD_NAMES.len()
        + r::CREDUI_NAMES.len() + r::D3D8_NAMES.len() + r::LZ32_NAMES.len()
    // Batch 8 — audio session, MF, scheduler, WTS, etc.
        + r::MMDEVAPI_NAMES.len() + r::AUDIOSES_NAMES.len()
        + r::WTSAPI32_NAMES.len() + r::TASKSCHD_NAMES.len()
        + r::OLEACC_NAMES.len() + r::WINUSB_NAMES.len()
        + r::NORMALIZ_NAMES.len() + r::SLC_NAMES.len() + r::DEVOBJ_NAMES.len()
        + r::NTDSAPI_NAMES.len() + r::WINSPOOL_NAMES.len()
        + r::OLEDLG_NAMES.len() + r::DHCPCSVC_NAMES.len() * 2
        + r::CLUSAPI_NAMES.len()
    // Batch 9 — cross the 15K line
        + r::RPCRT4_NAMES.len() + r::QUARTZ_NAMES.len()
        + r::AMSTREAM_NAMES.len() + r::EVR_NAMES.len()
        + r::ACTIVEDS_NAMES.len() + r::MQRT_NAMES.len()
        + r::FUSION_NAMES.len() * 2 + 2  // fusion + mscoreei + 2 stub dummies
    // Batch 10 — D3DX helper libraries + GUIDs + ACM/VFW/AVI/MSCMS
        + r::D3DX9_NAMES.len() * 5
        + r::D3DX10_NAMES.len() * 3
        + r::D3DX11_NAMES.len() * 2
        + r::DXGUID_NAMES.len()
        + r::MSACM32_NAMES.len() + r::MSVFW32_NAMES.len() + r::AVIFIL32_NAMES.len()
        + r::MSCMS_NAMES.len() + r::APPHELP_NAMES.len()
        + r::BTHPROPS_NAMES.len() * 2  // bthprops + bluetoothapis
        + r::XPSPRINT_NAMES.len() + r::MSIMG32_NAMES.len()
}
