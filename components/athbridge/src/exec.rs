//! Executable PE loading — maps a PE32+ image into the host's address
//! space, patches the IAT with real `extern "win64"` shim addresses, and
//! returns a callable entry point.
//!
//! Concept §Compatibility Strategy: this is the step that turns AthBridge
//! from a Windows-process *simulation* into Windows-code *execution*
//! (MasterChecklist Phase 11.2: trampoline emitter + import patching).
//! It runs in the AthBridge host process (Wine model): guest VAs are host
//! VAs, so `sys_mmap`-backed memory is directly executable — AthenaOS user
//! pages are mapped without NX.
//!
//! Import resolution policy:
//!   • Name resolves in [`crate::winapi_shims`] → the shim's address goes
//!     straight into the IAT slot. The Microsoft x64 calling convention is
//!     handled by rustc's `extern "win64"`.
//!   • Name does NOT resolve → the IAT slot points at an emitted 24-byte
//!     machine-code stub that loads the import's index into r10d and jumps
//!     to [`missing_import_trampoline`], which reports the DLL!name on the
//!     serial port and terminates with exit code 0xDEAD. Unimplemented
//!     imports fail loud at *call* time, not load time — exactly how a
//!     missing export behaves under a delay-load, and the only policy that
//!     lets real apps run before all 16k names have implementations.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::ldr::ProcessEnv;
use crate::pe_loader::{self, PeImage};
use crate::syscalls::{PROT_EXEC, PROT_READ, PROT_WRITE};
use crate::{seh, syscalls, winapi_shims, BridgeError};

/// `IMAGE_SCN_MEM_EXECUTE` — set on `.text` and any code section.
const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
/// `IMAGE_SCN_MEM_WRITE`.
const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// A PE image mapped into executable memory with a patched IAT.
///
/// Not `#[derive(Debug)]`: it owns a `Box<ProcessEnv>` (a multi-KiB TEB/PEB with
/// large reserved arrays that do not implement `Debug`). A manual `Debug` impl
/// below prints the scalar fields only.
pub struct ExecutablePe {
    /// Base address the image actually lives at.
    pub image_base: u64,
    /// Absolute VA of the PE entry point (AddressOfEntryPoint + base).
    pub entry_point: u64,
    /// Mapped size (SizeOfImage, page-rounded by the kernel).
    pub image_size: u64,
    /// Relocation delta applied (0 = loaded at preferred base).
    pub relocation_delta: i64,
    /// Imports patched to real shim addresses.
    pub resolved_imports: usize,
    /// Imports patched to fail-loud missing-import stubs.
    pub stubbed_imports: usize,
    /// The image's `.pdata` `RUNTIME_FUNCTION` table, parsed from the mapped
    /// exception directory. The SEH dispatcher ([`crate::seh`]) walks these when
    /// a fault arrives in this image. Empty for a `.pdata`-less PE (a leaf-only
    /// stub) — real MSVC binaries always carry one.
    pub runtime_functions: Vec<seh::RuntimeFunction>,
    /// The Win32 process environment (TEB/PEB/params) whose TEB this image's GS
    /// base points at. **Must outlive the guest thread** — the GS base dangles
    /// if this `Box` is dropped while the guest runs, so the caller holds the
    /// returned `ExecutablePe` across the entry-point call. `None` only if
    /// building the environment failed (the image still runs; `gs:[off]` then
    /// reads stale state, which is harmless for a PE that ignores the TEB).
    pub process_env: Option<Box<ProcessEnv>>,
    /// Whether `.text` (and any other executable section) was successfully
    /// flipped RW→RX via `SYS_MPROTECT`. `false` means the kernel mprotect was
    /// unavailable or refused; on RWX mmap the image still runs, but the W^X
    /// hardening did not take. Surfaced so the smoketest can assert it.
    pub wx_flip_ok: bool,
    /// Whether `SYS_SET_GS_BASE` succeeded, so guest `gs:[0x30]` now reads the
    /// TEB. `false` means the syscall failed (the image still runs; TEB reads
    /// hit stale GS state).
    pub gs_base_set: bool,
    /// The load steps this load actually performed, in execution order. The
    /// host KAT asserts this equals [`EXPECTED_LOAD_ORDER`] so a reordering
    /// (e.g. mprotect-before-reloc, or set_gs_base-before-mprotect) is caught
    /// off-target. Recorded live in `load_pe_executable`.
    pub load_steps: Vec<LoadStep>,
}

impl core::fmt::Debug for ExecutablePe {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ExecutablePe")
            .field("image_base", &self.image_base)
            .field("entry_point", &self.entry_point)
            .field("image_size", &self.image_size)
            .field("relocation_delta", &self.relocation_delta)
            .field("resolved_imports", &self.resolved_imports)
            .field("stubbed_imports", &self.stubbed_imports)
            .field("runtime_functions", &self.runtime_functions.len())
            .field("process_env", &self.process_env.is_some())
            .field("wx_flip_ok", &self.wx_flip_ok)
            .field("gs_base_set", &self.gs_base_set)
            .field("load_steps", &self.load_steps)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecError {
    Parse(BridgeError),
    /// Only PE32+ (x86_64) images can execute in-process.
    Not64Bit,
    /// SizeOfImage exceeds the loader's sanity cap.
    ImageTooLarge {
        size: u64,
    },
    /// sys_mmap refused both the preferred base and an anonymous placement.
    MmapFailed,
    /// Image must move (preferred base taken) but carries no relocations.
    NotRelocatable {
        preferred_base: u64,
    },
    /// A relocation RVA fell outside the mapped image.
    RelocationFailed {
        rva: u32,
    },
    /// A section's raw data or virtual placement fell outside its bounds.
    SectionOutOfBounds {
        rva: u32,
    },
}

impl From<BridgeError> for ExecError {
    fn from(e: BridgeError) -> Self {
        ExecError::Parse(e)
    }
}

const MAX_IMAGE_SIZE: u64 = 256 * 1024 * 1024;
const IMAGE_ORDINAL_FLAG64: u64 = 0x8000_0000_0000_0000;
const PAGE_SIZE: u64 = 0x1000;

// ---------------------------------------------------------------------------
// W^X / load-sequence pure logic (host-KAT-able, no syscalls)
// ---------------------------------------------------------------------------

/// The `prot` (a `PROT_*` bitmask) a section should hold *after* the loader has
/// copied + relocated + IAT-patched it. The W^X rule:
///   • executable section (`IMAGE_SCN_MEM_EXECUTE`) → `PROT_READ | PROT_EXEC`
///     (drop WRITE so it is never W+X; the relocation writes already happened
///     while the whole image was RW).
///   • writable, non-executable section (`.data`/`.bss`) → `PROT_READ |
///     PROT_WRITE` (stays writable, never executable).
///   • read-only, non-executable (`.rdata`) → `PROT_READ` (hardening).
///
/// Pure function of the section `characteristics` — unit-tested off-target so a
/// wrong W^X classification is caught before any page is flipped.
pub fn section_prot(characteristics: u32) -> u64 {
    let exec = characteristics & IMAGE_SCN_MEM_EXECUTE != 0;
    let write = characteristics & IMAGE_SCN_MEM_WRITE != 0;
    if exec {
        // RX — never carry WRITE alongside EXEC (W^X).
        PROT_READ | PROT_EXEC
    } else if write {
        PROT_READ | PROT_WRITE
    } else {
        PROT_READ
    }
}

/// Page-align a `[va, va+size)` span down/up to page boundaries, returning
/// `(aligned_base, aligned_len)`. `size == 0` yields a zero-length span (the
/// caller skips it). Overflow-safe rounding. Pure — unit-tested.
pub fn page_align_span(va: u64, size: u64) -> (u64, u64) {
    if size == 0 {
        return (va & !(PAGE_SIZE - 1), 0);
    }
    let start = va & !(PAGE_SIZE - 1);
    let end = va.saturating_add(size).saturating_add(PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    (start, end - start)
}

/// One step in the spec'd load sequence, recorded for the FAIL-able ordering
/// KAT. The spec (athbridge-real-crt-abi.md §4) mandates the exact order:
/// mmap RW → copy/reloc/IAT → mprotect `.text` RX → build TEB/PEB →
/// set_gs_base → jump. A reordering (e.g. mprotect-before-reloc, which would
/// fault the relocation writes; or set_gs_base-before-mprotect) is a bug the
/// test catches by asserting this exact sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadStep {
    MmapRw,
    CopyRelocIat,
    MprotectTextRx,
    BuildTebPeb,
    SetGsBase,
}

/// The canonical, correct load-step order. `load_pe_executable` records the
/// steps it actually performs into a `Vec<LoadStep>` so the host KAT can assert
/// the live path matches this exactly.
pub const EXPECTED_LOAD_ORDER: &[LoadStep] = &[
    LoadStep::MmapRw,
    LoadStep::CopyRelocIat,
    LoadStep::MprotectTextRx,
    LoadStep::BuildTebPeb,
    LoadStep::SetGsBase,
];

// ---------------------------------------------------------------------------
// Missing-import registry (index → "dll!name" for the fail-loud path)
// ---------------------------------------------------------------------------

struct MissingImports {
    lock: AtomicBool,
    names: UnsafeCell<Vec<String>>,
}

// SAFETY: all access to `names` is serialized on the `lock` spinlock via
// `register_missing` and `missing_name`.
unsafe impl Sync for MissingImports {}

static MISSING_IMPORTS: MissingImports = MissingImports {
    lock: AtomicBool::new(false),
    names: UnsafeCell::new(Vec::new()),
};

fn missing_lock() {
    while MISSING_IMPORTS
        .lock
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

fn register_missing(name: String) -> u32 {
    missing_lock();
    // SAFETY: spinlock held — exclusive access to the Vec.
    let idx = unsafe {
        let names = &mut *MISSING_IMPORTS.names.get();
        names.push(name);
        (names.len() - 1) as u32
    };
    MISSING_IMPORTS.lock.store(false, Ordering::Release);
    idx
}

fn missing_name(idx: u32) -> String {
    missing_lock();
    // SAFETY: spinlock held — exclusive access to the Vec.
    let name = unsafe {
        let names = &*MISSING_IMPORTS.names.get();
        names
            .get(idx as usize)
            .cloned()
            .unwrap_or_else(|| String::from("<unknown import index>"))
    };
    MISSING_IMPORTS.lock.store(false, Ordering::Release);
    name
}

// ---------------------------------------------------------------------------
// Import-coverage worklist (data-driven shim prioritization)
// ---------------------------------------------------------------------------
//
// `register_missing` already records EVERY import that fell through to a
// fail-loud stub (i.e. has no shim yet), accumulated across every PE loaded
// this session. Ranking that flat list by frequency turns it into the coverage
// worklist: the highest-count names are the imports the most binaries need, so
// they get a real `winapi_shims` implementation first. This replaces guessing
// which Win32 names to implement next with the data of what real binaries
// actually import. See `docs/components/athbridge-wine-strategy.md` §7.

/// Tally a flat list of `"dll!name"` strings into a frequency-ranked report:
/// `(name, count)` sorted by count descending, ties broken by name ascending.
/// Pure logic so the host KAT can prove the ranking without touching the global
/// registry or running a guest.
fn rank_missing(names: &[String]) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for n in names {
        *counts.entry(n.as_str()).or_insert(0) += 1;
    }
    let mut ranked: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(k, v)| (String::from(k), v))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
}

/// Frequency-ranked report of every import that resolved to a fail-loud stub,
/// accumulated across every PE loaded this session. The data-driven shim
/// worklist — highest count first.
pub fn missing_import_report() -> Vec<(String, usize)> {
    missing_lock();
    // SAFETY: spinlock held — exclusive read of the Vec.
    let ranked = unsafe {
        let names = &*MISSING_IMPORTS.names.get();
        rank_missing(names)
    };
    MISSING_IMPORTS.lock.store(false, Ordering::Release);
    ranked
}

/// Emit the cumulative missing-import report to the debug console — a header
/// plus one grep-able `[athbridge] import-gap: <DLL>!<name> xN` line per name,
/// frequency-ranked. Returns the number of DISTINCT missing names. Called
/// automatically when a load leaves any IAT slot stubbed, so a boot that loads
/// a real binary dumps its coverage gap straight into the bootlog.
pub fn log_missing_import_report() -> usize {
    let ranked = missing_import_report();
    let distinct = ranked.len();
    let header = format!(
        "[athbridge] import-coverage gap (cumulative): {} distinct missing import(s)\n",
        distinct
    );
    // SAFETY: SYS_DEBUG_PRINT only reads the buffer it is handed.
    unsafe {
        syscalls::sys_debug_print(header.as_bytes());
        for (name, count) in &ranked {
            let line = format!("[athbridge] import-gap: {} x{}\n", name, count);
            syscalls::sys_debug_print(line.as_bytes());
        }
    }
    distinct
}

/// Final destination of a call through an unresolved IAT slot. Reports the
/// import that was hit, then terminates the Windows process.
extern "win64" fn missing_import_handler(idx: u32) -> ! {
    let name = missing_name(idx);
    let msg = format!("[athbridge] FATAL: call into unresolved import: {}\n", name);
    // SAFETY: SYS_DEBUG_PRINT only reads the buffer; sys_exit never returns.
    unsafe {
        syscalls::sys_debug_print(msg.as_bytes());
        syscalls::sys_exit(0xDEAD);
    }
}

/// Entered from an emitted stub with the missing-import index in r10d
/// (r10 is scratch in the Microsoft x64 convention, so the stub can use it
/// without disturbing the original call's arguments). Moves the index into
/// rcx (first win64 argument) and tail-jumps into Rust.
#[unsafe(naked)]
unsafe extern "win64" fn missing_import_trampoline() -> ! {
    core::arch::naked_asm!(
        "mov ecx, r10d",
        "jmp {handler}",
        handler = sym missing_import_handler,
    )
}

// ---------------------------------------------------------------------------
// Stub page emitter
// ---------------------------------------------------------------------------

/// Stride of one emitted missing-import stub.
const STUB_SIZE: usize = 24;

/// Encode one stub at `buf`:
///   0:  41 BA xx xx xx xx     mov  r10d, imm32 (missing-import index)
///   6:  FF 25 04 00 00 00     jmp  qword [rip+4]      ; → qword at offset 16
///  12:  CC CC CC CC           int3 padding
///  16:  8-byte absolute address of `missing_import_trampoline`
fn encode_stub(buf: &mut [u8], index: u32, target: u64) {
    buf[0] = 0x41;
    buf[1] = 0xBA;
    buf[2..6].copy_from_slice(&index.to_le_bytes());
    buf[6] = 0xFF;
    buf[7] = 0x25;
    buf[8..12].copy_from_slice(&4u32.to_le_bytes());
    buf[12..16].copy_from_slice(&[0xCC; 4]);
    buf[16..24].copy_from_slice(&target.to_le_bytes());
}

/// One mmap'd page of fail-loud stubs, filled front to back.
struct StubPage {
    base: u64,
    used: usize,
    capacity: usize,
}

impl StubPage {
    fn new() -> Option<Self> {
        // SAFETY: anonymous user mapping; the kernel chooses the address.
        let base = unsafe { syscalls::sys_mmap(0, 4096, 3, 0, u64::MAX, 0) };
        if base == u64::MAX {
            return None;
        }
        Some(Self {
            base,
            used: 0,
            capacity: 4096 / STUB_SIZE,
        })
    }

    fn emit(&mut self, dll: &str, name: &str) -> Option<u64> {
        if self.used >= self.capacity {
            return None;
        }
        let idx = register_missing(format!("{}!{}", dll, name));
        let addr = self.base + (self.used * STUB_SIZE) as u64;
        let target = missing_import_trampoline as unsafe extern "win64" fn() -> ! as usize as u64;
        // SAFETY: addr..addr+STUB_SIZE lies inside the page this StubPage
        // mmap'd above and `used < capacity` keeps it in bounds.
        let buf = unsafe { core::slice::from_raw_parts_mut(addr as *mut u8, STUB_SIZE) };
        encode_stub(buf, idx, target);
        self.used += 1;
        Some(addr)
    }
}

// ---------------------------------------------------------------------------
// The executable loader
// ---------------------------------------------------------------------------

/// Map `data` (a PE32+ image) into executable memory, apply relocations if
/// the preferred base is unavailable, patch every IAT slot to either a real
/// shim or a fail-loud stub, flip executable sections RW→RX (W^X), build the
/// Win32 TEB/PEB, and point the task's GS base at the TEB. On success the
/// returned entry point is directly callable as `extern "win64"` and a guest
/// `gs:[0x30]` read returns the TEB self-pointer.
///
/// Sequence (athbridge-real-crt-abi.md §4, recorded into `load_steps`):
/// mmap RW → copy/reloc/IAT → mprotect `.text` RX → build TEB/PEB →
/// set_gs_base → (caller jumps to entry).
pub fn load_pe_executable(data: &[u8]) -> Result<ExecutablePe, ExecError> {
    let mut load_steps: Vec<LoadStep> = Vec::new();
    let pe = pe_loader::parse_pe(data)?;
    if !pe.is_64bit {
        return Err(ExecError::Not64Bit);
    }
    let size = pe.size_of_image as u64;
    if size > MAX_IMAGE_SIZE {
        return Err(ExecError::ImageTooLarge { size });
    }

    // 1. Reserve address space — preferred base first, anywhere as fallback.
    //    Mapped RW (prot=3 == PROT_READ|PROT_WRITE) so copy/reloc/IAT can write.
    // SAFETY: anonymous user mappings; the kernel validates the range.
    let mut base = unsafe { syscalls::sys_mmap(pe.image_base, size, 3, 0, u64::MAX, 0) };
    if base == u64::MAX {
        base = unsafe { syscalls::sys_mmap(0, size, 3, 0, u64::MAX, 0) };
    }
    if base == u64::MAX {
        return Err(ExecError::MmapFailed);
    }
    load_steps.push(LoadStep::MmapRw);

    let delta = base.wrapping_sub(pe.image_base) as i64;
    if delta != 0 && pe.relocations.is_empty() {
        return Err(ExecError::NotRelocatable {
            preferred_base: pe.image_base,
        });
    }

    // SAFETY: base..base+size was just mapped read/write for this process
    // and nothing else holds a reference into it.
    let image = unsafe { core::slice::from_raw_parts_mut(base as *mut u8, size as usize) };

    // 2. Copy headers + sections at their RVAs.
    let header_len = core::cmp::min(pe.size_of_headers as usize, data.len()).min(image.len());
    image[..header_len].copy_from_slice(&data[..header_len]);
    for sec in &pe.sections {
        if sec.raw_data_size == 0 {
            continue;
        }
        let copy_len = core::cmp::min(sec.raw_data_size, sec.virtual_size) as usize;
        let src_start = sec.raw_data_offset as usize;
        let dst_start = sec.virtual_address as usize;
        if src_start + copy_len > data.len() || dst_start + copy_len > image.len() {
            return Err(ExecError::SectionOutOfBounds {
                rva: sec.virtual_address,
            });
        }
        image[dst_start..dst_start + copy_len]
            .copy_from_slice(&data[src_start..src_start + copy_len]);
    }

    // 3. Relocate if we didn't get the preferred base.
    if delta != 0 {
        pe_loader::apply_relocations(image, &pe.relocations, delta, true)
            .map_err(|_| ExecError::RelocationFailed { rva: 0 })?;
    }

    // 4. Patch the IAT. (Steps 3+4 are the single "writable image is finalized"
    //    phase — record it once the last write into the RW image is done.)
    let (resolved, stubbed) = patch_iat(image, &pe)?;
    load_steps.push(LoadStep::CopyRelocIat);

    // 5. Parse the exception directory (.pdata) for the SEH dispatcher. The
    //    mapped image is RVA-indexed (offset == RVA after section mapping), so
    //    the table walker reads it directly.
    let (runtime_functions, pdata_rva, pdata_size) = match pe
        .data_directories
        .get(seh::IMAGE_DIRECTORY_ENTRY_EXCEPTION)
    {
        Some(dd) if dd.rva != 0 => (seh::parse_pdata(image, dd.rva, dd.size), dd.rva, dd.size),
        _ => (Vec::new(), 0u32, 0u32),
    };

    // Register this image in the process-global module table so the Rtl* SEH
    // shims (RtlLookupFunctionEntry / RtlPcToFileHeader) can map a guest PC back
    // to its base + .pdata. The CRT installs these during startup; without a
    // registered module their lookups return 0 (the "leaf / no unwind data"
    // answer the no-throw startup path expects).
    crate::crt_startup::register_module(crate::crt_startup::LoadedModule {
        base,
        size,
        pdata_va: if pdata_rva != 0 {
            base + pdata_rva as u64
        } else {
            0
        },
        pdata_size,
    });
    // Record this image's REAL base as the main-module base so
    // GetModuleHandle(NULL) returns where the image actually lives. The MSVC CRT
    // dereferences that HMODULE to read the PE header after main; a synthetic
    // 0x400000 there page-faults (it points at unmapped memory). First-image-
    // wins, so a later DLL load cannot override it.
    crate::crt_startup::set_main_module_base(base);

    // 4b. W^X flip. Now that every relocation + IAT write is done, drop WRITE
    //     from executable sections and add EXEC (RW→RX). Writable data sections
    //     stay RW; read-only data flips to PROT_READ. Flipping .text BEFORE the
    //     relocations would fault those writes — hence this runs after step 4.
    //     A non-executable/unmapped span makes mprotect return error; we treat
    //     a refusal as "W^X did not take" (the RWX mmap still runs the code).
    let mut wx_flip_ok = true;
    for sec in &pe.sections {
        if sec.virtual_size == 0 && sec.raw_data_size == 0 {
            continue;
        }
        let span_size = core::cmp::max(sec.virtual_size, sec.raw_data_size) as u64;
        let (aligned_base, aligned_len) =
            page_align_span(base + sec.virtual_address as u64, span_size);
        if aligned_len == 0 {
            continue;
        }
        let prot = section_prot(sec.characteristics);
        // SAFETY: aligned_base..+aligned_len lies inside the image we mmap'd;
        // SYS_MPROTECT only adjusts flags on this task's own pages.
        let r = unsafe { syscalls::sys_mprotect(aligned_base, aligned_len, prot) };
        if r == u64::MAX {
            wx_flip_ok = false;
        }
    }
    load_steps.push(LoadStep::MprotectTextRx);

    // 5b. Build the TEB/PEB/process-parameters set. The TEB self-pointer
    //     (gs:[0x30]) is `env.gs_base()`; the PEB (gs:[0x60]), TLS array
    //     (gs:[0x58]) and LastError (gs:[0x68]) all resolve against it. The
    //     stack range is approximate (the host runs the guest on its own
    //     stack); a real spawn would pass the guest stack bounds.
    let entry_point = base + pe.entry_point;
    let stack_base = base + size; // top of image as a stand-in stack base
    let stack_limit = base; // bottom — TEB stack-limit field (informational)
    let process_env = ProcessEnv::build(base, stack_base, stack_limit, "");
    let teb_addr = process_env.gs_base();
    load_steps.push(LoadStep::BuildTebPeb);

    // 6. Point the task's user GS base at the TEB. After this, guest
    //    `gs:[0x30]` reads the TEB self-pointer and survives context switches
    //    (the kernel saves/restores Task::gs_base). Harmless for a PE that
    //    ignores GS. A failure leaves the image runnable but TEB-less.
    // SAFETY: SYS_SET_GS_BASE writes only this task's GS-base MSR.
    let gs_base_set = unsafe { syscalls::sys_set_gs_base(teb_addr) } == 0;
    load_steps.push(LoadStep::SetGsBase);

    // Data-driven coverage: if any IAT slot fell through to a fail-loud stub,
    // dump the cumulative frequency-ranked missing-import worklist to the
    // bootlog so shim work is prioritized by what real binaries actually need
    // (docs/components/athbridge-wine-strategy.md §7). Silent when the image
    // fully resolves (e.g. the real /MT exe: resolved=73 stubbed=0).
    if stubbed > 0 {
        let _ = log_missing_import_report();
    }

    Ok(ExecutablePe {
        image_base: base,
        entry_point,
        image_size: size,
        relocation_delta: delta,
        resolved_imports: resolved,
        stubbed_imports: stubbed,
        runtime_functions,
        process_env: Some(process_env),
        wx_flip_ok,
        gs_base_set,
        load_steps,
    })
}

/// Walk the import descriptor table *in the mapped image* (every RVA is a
/// direct offset once sections are mapped) and write an absolute handler
/// address into each 64-bit IAT slot.
fn patch_iat(image: &mut [u8], pe: &PeImage) -> Result<(usize, usize), ExecError> {
    const IMPORT_DIR: usize = 1;
    let dd = match pe.data_directories.get(IMPORT_DIR) {
        Some(dd) if dd.rva != 0 => *dd,
        _ => return Ok((0, 0)), // no imports — valid (our minimal smoketest PE)
    };

    let mut resolved = 0usize;
    let mut stubbed = 0usize;
    let mut stub_page: Option<StubPage> = None;

    // Build the shim index ONCE for the whole image. Binding the IAT then costs
    // one table build + O(log n) per import, instead of rebuilding the entire
    // shim table (a fresh Vec + every fn-pointer cast) for each of the image's
    // hundreds of imports. See `winapi_shims::ShimResolver`.
    let resolver = winapi_shims::ShimResolver::new();

    let mut desc_off = dd.rva as usize;
    loop {
        if desc_off + 20 > image.len() {
            break;
        }
        let original_first_thunk = read_u32(image, desc_off);
        let name_rva = read_u32(image, desc_off + 12) as usize;
        let first_thunk = read_u32(image, desc_off + 16) as usize;
        if original_first_thunk == 0 && name_rva == 0 && first_thunk == 0 {
            break;
        }

        let dll = read_cstr(image, name_rva);
        let lookup_rva = if original_first_thunk != 0 {
            original_first_thunk as usize
        } else {
            first_thunk
        };

        let mut i = 0usize;
        loop {
            let lookup_off = lookup_rva + i * 8;
            let iat_off = first_thunk + i * 8;
            if lookup_off + 8 > image.len() || iat_off + 8 > image.len() {
                break;
            }
            let entry = read_u64(image, lookup_off);
            if entry == 0 {
                break;
            }

            let func_name = if entry & IMAGE_ORDINAL_FLAG64 != 0 {
                format!("#{}", entry & 0xFFFF)
            } else {
                read_cstr(image, (entry as u32 & 0x7FFF_FFFF) as usize + 2)
            };

            let addr = match resolver.resolve(&dll, &func_name) {
                Some(a) => {
                    resolved += 1;
                    a
                }
                None => {
                    if stub_page.as_ref().map_or(true, |p| p.used >= p.capacity) {
                        stub_page = StubPage::new();
                    }
                    match stub_page.as_mut().and_then(|p| p.emit(&dll, &func_name)) {
                        Some(a) => {
                            stubbed += 1;
                            a
                        }
                        None => return Err(ExecError::MmapFailed),
                    }
                }
            };
            image[iat_off..iat_off + 8].copy_from_slice(&addr.to_le_bytes());
            i += 1;
        }

        desc_off += 20;
    }

    Ok((resolved, stubbed))
}

fn read_u32(buf: &[u8], off: usize) -> u32 {
    if off + 4 > buf.len() {
        return 0;
    }
    u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

fn read_u64(buf: &[u8], off: usize) -> u64 {
    if off + 8 > buf.len() {
        return 0;
    }
    let mut b = [0u8; 8];
    b.copy_from_slice(&buf[off..off + 8]);
    u64::from_le_bytes(b)
}

fn read_cstr(buf: &[u8], off: usize) -> String {
    let mut s = String::new();
    let mut i = off;
    while i < buf.len() && buf[i] != 0 && s.len() < 256 {
        s.push(buf[i] as char);
        i += 1;
    }
    s
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use crate::{pe_loader, testpe, winapi_shims};
    use alloc::string::ToString;

    // ── Import-coverage ranking (pure, FAIL-able) ──────────────────────────

    #[test]
    fn rank_missing_orders_by_frequency() {
        // 3× HeapAlloc, 2× CreateWindowExW, 1× TextOutW -> most-needed first.
        let names = [
            "KERNEL32.dll!HeapAlloc".to_string(),
            "USER32.dll!CreateWindowExW".to_string(),
            "KERNEL32.dll!HeapAlloc".to_string(),
            "GDI32.dll!TextOutW".to_string(),
            "USER32.dll!CreateWindowExW".to_string(),
            "KERNEL32.dll!HeapAlloc".to_string(),
        ];
        let ranked = rank_missing(&names);
        assert_eq!(
            ranked,
            alloc::vec![
                ("KERNEL32.dll!HeapAlloc".to_string(), 3),
                ("USER32.dll!CreateWindowExW".to_string(), 2),
                ("GDI32.dll!TextOutW".to_string(), 1),
            ],
            "highest-frequency missing import must rank first"
        );
    }

    #[test]
    fn rank_missing_breaks_ties_by_name_ascending() {
        // Equal counts must order deterministically by name, not insertion.
        let names = ["B.dll!z".to_string(), "A.dll!a".to_string()];
        let ranked = rank_missing(&names);
        assert_eq!(
            ranked,
            alloc::vec![("A.dll!a".to_string(), 1), ("B.dll!z".to_string(), 1)],
            "equal counts tie-break by name ascending (stable worklist)"
        );
    }

    #[test]
    fn rank_missing_empty_is_empty() {
        assert!(
            rank_missing(&[]).is_empty(),
            "no missing imports -> empty report"
        );
    }

    // ── W^X protection math (off-target, FAIL-able) ────────────────────────

    #[test]
    fn section_prot_executable_is_rx_never_wx() {
        // .text: CODE | EXECUTE | READ (0x60000020) -> RX, WRITE dropped.
        let prot = section_prot(0x6000_0020);
        assert_eq!(prot, PROT_READ | PROT_EXEC, "executable -> RX");
        assert_eq!(
            prot & PROT_WRITE,
            0,
            "executable must never be writable (W^X)"
        );
        // Even a (pathological) W+X section header collapses to RX, never W+X.
        let wx = section_prot(IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_WRITE | 0x4000_0000);
        assert_eq!(wx, PROT_READ | PROT_EXEC);
        assert_eq!(wx & PROT_WRITE, 0);
    }

    #[test]
    fn section_prot_data_is_rw_never_executable() {
        // .data: INITIALIZED_DATA | READ | WRITE (0xC0000040) -> RW, no EXEC.
        let prot = section_prot(0xC000_0040);
        assert_eq!(prot, PROT_READ | PROT_WRITE, "writable data -> RW");
        assert_eq!(prot & PROT_EXEC, 0, "data must never be executable");
    }

    #[test]
    fn section_prot_readonly_is_read_only() {
        // .rdata: INITIALIZED_DATA | READ (0x40000040) -> PROT_READ only.
        let prot = section_prot(0x4000_0040);
        assert_eq!(prot, PROT_READ);
        assert_eq!(prot & PROT_WRITE, 0);
        assert_eq!(prot & PROT_EXEC, 0);
    }

    #[test]
    fn page_align_span_rounds_to_pages() {
        // A section at RVA 0x1000 size 0x200 -> [0x1000, 0x2000) is one page.
        assert_eq!(page_align_span(0x1000, 0x200), (0x1000, 0x1000));
        // Unaligned base rounds down; end rounds up across a page boundary.
        assert_eq!(page_align_span(0x1FF0, 0x20), (0x1000, 0x2000));
        // Zero size -> zero len (the loop skips it).
        assert_eq!(page_align_span(0x4000, 0), (0x4000, 0));
        // Exactly one page stays one page.
        assert_eq!(page_align_span(0x3000, 0x1000), (0x3000, 0x1000));
    }

    // ── Load-sequence ordering (off-target, FAIL-able) ─────────────────────

    #[test]
    fn expected_load_order_is_the_spec_sequence() {
        // The spec mandates: mmap RW -> copy/reloc/IAT -> mprotect .text RX ->
        // build TEB/PEB -> set_gs_base. A reordering here is a real bug.
        assert_eq!(
            EXPECTED_LOAD_ORDER,
            &[
                LoadStep::MmapRw,
                LoadStep::CopyRelocIat,
                LoadStep::MprotectTextRx,
                LoadStep::BuildTebPeb,
                LoadStep::SetGsBase,
            ]
        );
        let pos = |s: LoadStep| EXPECTED_LOAD_ORDER.iter().position(|x| *x == s).unwrap();
        // copy/reloc/IAT MUST precede mprotect RX (else those writes fault on an
        // RX page); the TEB MUST be built before set_gs_base.
        assert!(pos(LoadStep::CopyRelocIat) < pos(LoadStep::MprotectTextRx));
        assert!(pos(LoadStep::BuildTebPeb) < pos(LoadStep::SetGsBase));
    }

    // ── gs-base PE parses, imports resolve, TEB self-pointer is consistent ──

    #[test]
    fn gsbase_exe_parses_as_amd64() {
        let exe = testpe::build_gsbase_exe();
        let info = crate::load_pe(&exe).expect("gs-base PE must parse");
        assert_eq!(info.machine, crate::MachineType::Amd64);
        assert_eq!(info.format, crate::PeFormat::Pe32Plus);
        assert_eq!(info.entry_point_rva, 0x1000);
    }

    #[test]
    fn gsbase_exe_imports_resolve_to_real_shims() {
        // Sleep + ExitProcess (the only two imports) must resolve to real,
        // non-null kernel32 shims or the guest would hit a fail-loud stub at
        // call time. FAIL-able.
        let exe = testpe::build_gsbase_exe();
        let pe = pe_loader::parse_pe(&exe).expect("parse_pe");
        let names: alloc::vec::Vec<alloc::string::String> =
            pe.imports.iter().map(|i| i.function_name.clone()).collect();
        for want in testpe::GSBASE_IMPORTS {
            assert!(
                names.iter().any(|n| n == want),
                "missing import {want}; got {names:?}"
            );
            let a = winapi_shims::resolve_shim("kernel32.dll", want);
            assert!(a.is_some(), "{want} must resolve to a shim");
            assert_ne!(a.unwrap(), 0, "{want} shim address must be non-null");
        }
        // FAIL-demo: a bogus name must NOT resolve (the asserts aren't vacuous).
        assert!(winapi_shims::resolve_shim("kernel32.dll", "NoSuchExport_GS").is_none());
    }

    #[test]
    fn gsbase_exe_iat_patches_in_vec_backed_loader() {
        // Vec-backed load (no sys_mmap, host-safe): every import must resolve,
        // proving the .idata layout.
        let exe = testpe::build_gsbase_exe();
        let mut reg = pe_loader::DllRegistry::new();
        let loaded = pe_loader::load_pe(&exe, &mut reg).expect("Vec-backed load_pe");
        assert!(loaded.is_64bit);
        assert!(loaded.imports.iter().all(|i| i.resolved), "all resolved");
    }

    #[test]
    fn process_env_teb_self_pointer_equals_gs_base() {
        // The value handed to sys_set_gs_base (env.gs_base()) MUST equal the TEB
        // self-pointer the guest reads at gs:[0x30]. If these diverge the gs-base
        // survival check can never pass. This is the off-target invariant behind
        // the live test's "gs:[0x30] == TEB" assertion.
        let env = ProcessEnv::build(0x1_4000_0000, 0x1_4001_0000, 0x1_4000_0000, "");
        assert_eq!(env.teb.self_ptr, env.gs_base());
        assert_ne!(env.gs_base(), 0);
    }
}
