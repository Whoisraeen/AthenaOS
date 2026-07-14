//! Per-CPU fault-fixup (exception) table — Linux `extable` equivalent.
//!
//! Lets a kernel critical section (today: `copy_from_user` / `copy_to_user`)
//! declare a range `[start_rip, end_rip)` of instructions that are allowed to
//! page-fault, plus a `fixup_rip` to land on instead. When `page_fault_inner`
//! detects a kernel-CS fault whose saved RIP falls inside the declared range
//! for the running CPU, it rewrites the saved RIP on the iretq frame to
//! `fixup_rip` and returns. The faulting instruction never re-executes — the
//! caller resumes at the fixup label, which returns an error sentinel to the
//! syscall.
//!
//! # Why this exists
//!
//! `validate_user_range` walks the page tables before `copy_nonoverlapping`,
//! so a syscall pointer that's bogus at validate time is rejected without
//! ever touching kernel-mode unsafe code. The window we close here is a TOCTOU
//! race: between validate and the actual copy, a sibling CPU can unmap the
//! page (rare, but possible — for example, mmap teardown, swap, IOMMU
//! revoke). The previous behavior on such a fault was:
//!
//!   `page_fault_inner` → `scheduler::has_current_task()` → `SCHEDULER.lock()`
//!
//! If the interrupted code was inside a `with_current_task` closure (which
//! holds `SCHEDULER`), that lock acquire would spin forever — single-CPU it
//! just hangs, on bare-metal SMP it triple-faults (#DF without IST) and the
//! box silently resets. The extable makes the recovery path a single per-CPU
//! atomic load + a write to the iretq frame: no locks, no scheduler call.
//!
//! # Concurrency
//!
//! Each CPU writes only its own slot (`current_cpu_id()` gates `install` and
//! `clear`). `page_fault_inner` reads only its own slot (the fault runs on
//! the same CPU that's mid-copy). Cross-CPU visibility is irrelevant — the
//! fault handler observes the slot the local CPU itself wrote moments
//! earlier on the same core, so even `Relaxed` ordering would work. We use
//! `Release` on the gating store (`start_rip`) paired with `Acquire` on the
//! reader for clarity and to keep the contract explicit if this is ever
//! shared cross-CPU in the future.

extern crate alloc;

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

/// One slot per CPU. `start_rip == 0` is the "not armed" sentinel.
#[repr(C)]
pub struct FaultFixup {
    /// Inclusive lower bound of the protected RIP range.
    /// `0` means "no fixup installed on this CPU."
    pub start_rip: AtomicU64,
    /// Exclusive upper bound of the protected RIP range.
    pub end_rip: AtomicU64,
    /// Where to land on a fault inside `[start_rip, end_rip)`.
    pub fixup_rip: AtomicU64,
}

impl FaultFixup {
    pub const fn new() -> Self {
        Self {
            start_rip: AtomicU64::new(0),
            end_rip: AtomicU64::new(0),
            fixup_rip: AtomicU64::new(0),
        }
    }
}

pub static FIXUP: [FaultFixup; crate::gdt::MAX_CPUS] =
    [const { FaultFixup::new() }; crate::gdt::MAX_CPUS];

/// Counts of fixup events for boot smoketest + procfs.
pub static FIXUP_HITS: AtomicUsize = AtomicUsize::new(0);
pub static FIXUP_INSTALLS: AtomicUsize = AtomicUsize::new(0);

/// Arm a fixup for the current CPU. The slot is freed by [`clear`] once the
/// critical section returns normally. Must be called with interrupts
/// disabled, or from a path where preemption can't migrate this thread to a
/// different CPU before the matching `clear`.
#[inline]
pub fn install(start: u64, end: u64, fixup: u64) {
    debug_assert!(
        start != 0,
        "extable: start_rip == 0 is reserved as 'disarmed'"
    );
    debug_assert!(end > start, "extable: end must be > start");
    let cpu = crate::gdt::current_cpu_id();
    if cpu < crate::gdt::MAX_CPUS {
        // Order: end and fixup must be visible before start. The reader's
        // Acquire on start_rip pairs with this Release.
        FIXUP[cpu].end_rip.store(end, Ordering::Relaxed);
        FIXUP[cpu].fixup_rip.store(fixup, Ordering::Relaxed);
        FIXUP[cpu].start_rip.store(start, Ordering::Release);
        FIXUP_INSTALLS.fetch_add(1, Ordering::Relaxed);
    }
}

/// Disarm the current CPU's fixup. Idempotent.
#[inline]
pub fn clear() {
    let cpu = crate::gdt::current_cpu_id();
    if cpu < crate::gdt::MAX_CPUS {
        FIXUP[cpu].start_rip.store(0, Ordering::Release);
    }
}

/// If `rip` falls inside the current CPU's installed fixup range, return the
/// fixup target RIP; otherwise `None`. Lock-free; safe to call from an
/// interrupt handler.
#[inline]
pub fn check(rip: u64) -> Option<u64> {
    let cpu = crate::gdt::current_cpu_id();
    if cpu >= crate::gdt::MAX_CPUS {
        return None;
    }
    let start = FIXUP[cpu].start_rip.load(Ordering::Acquire);
    if start == 0 {
        return None;
    }
    let end = FIXUP[cpu].end_rip.load(Ordering::Relaxed);
    if rip < start || rip >= end {
        return None;
    }
    FIXUP_HITS.fetch_add(1, Ordering::Relaxed);
    Some(FIXUP[cpu].fixup_rip.load(Ordering::Relaxed))
}

// ═══════════════════════════════════════════════════════════════════════════
// global_asm: the user-memory copy stub
// ═══════════════════════════════════════════════════════════════════════════
//
// One small function, with three exported labels:
//   _raeen_copy_user_movsb        — entry, called from Rust
//   _raeen_copy_user_movsb_start  — first instruction of the rep movsb
//   _raeen_copy_user_movsb_end    — first instruction AFTER the rep movsb
//   _raeen_copy_user_movsb_fixup  — recovery point reached on page fault
//
// Sys V ABI: rdi=src, rsi=dst, rdx=len. Returns u64 in rax (0 = OK, 1 = fault).
//
// We swap rsi/rdi (movsb's source/destination expectation), set rcx=len, then
// `rep movsb`. The fixup label sits OUTSIDE the protected range and just sets
// rax=1 and rets — page_fault_inner will iret to it on fault.

core::arch::global_asm!(
    ".text",
    ".global _raeen_copy_user_movsb",
    ".global _raeen_copy_user_movsb_start",
    ".global _raeen_copy_user_movsb_end",
    ".global _raeen_copy_user_movsb_fixup",
    "_raeen_copy_user_movsb:",
    "    mov rcx, rdx",  // count → rcx
    "    xchg rsi, rdi", // movsb wants rsi=src, rdi=dst; we got rdi=src, rsi=dst
    "_raeen_copy_user_movsb_start:",
    "    rep movsb",
    "_raeen_copy_user_movsb_end:",
    "    xor rax, rax", // success: return 0
    "    ret",
    "_raeen_copy_user_movsb_fixup:",
    "    mov rax, 1", // fault: return 1
    "    ret",
);

// The SMAP variant: identical rep-movsb copy, bracketed by stac/clac so the
// user-page touch is legal while CR4.SMAP is set. The fixup label ALSO runs
// clac — the #PF iretq restores RFLAGS from the faulted frame, i.e. with
// AC=1 still set, and the fixup path must not leave the kernel's user-access
// window open after an aborted copy. stac/clac #UD on CPUs without CPUID
// SMAP, so this stub is only dispatched once `SMAP_ON` is armed (see
// `cpu_features::enable_smap`'s ordering contract).
core::arch::global_asm!(
    ".text",
    ".global _raeen_copy_user_movsb_smap",
    ".global _raeen_copy_user_movsb_smap_start",
    ".global _raeen_copy_user_movsb_smap_end",
    ".global _raeen_copy_user_movsb_smap_fixup",
    "_raeen_copy_user_movsb_smap:",
    "    mov rcx, rdx",
    "    xchg rsi, rdi",
    "    stac", // open the supervisor→user access window
    "_raeen_copy_user_movsb_smap_start:",
    "    rep movsb",
    "_raeen_copy_user_movsb_smap_end:",
    "    clac", // close the window
    "    xor rax, rax",
    "    ret",
    "_raeen_copy_user_movsb_smap_fixup:",
    "    clac", // iretq restored AC=1 from the faulted frame — close it
    "    mov rax, 1",
    "    ret",
);

extern "C" {
    pub fn _raeen_copy_user_movsb(src: *const u8, dst: *mut u8, len: usize) -> u64;
    pub fn _raeen_copy_user_movsb_start();
    pub fn _raeen_copy_user_movsb_end();
    pub fn _raeen_copy_user_movsb_fixup();
    pub fn _raeen_copy_user_movsb_smap(src: *const u8, dst: *mut u8, len: usize) -> u64;
    pub fn _raeen_copy_user_movsb_smap_start();
    pub fn _raeen_copy_user_movsb_smap_end();
    pub fn _raeen_copy_user_movsb_smap_fixup();
}

/// True once the stac/clac copy stubs are the dispatched variant. Armed by
/// `cpu_features::enable_smap` BEFORE any CPU sets `CR4.SMAP` (stac/clac are
/// legal from the moment CPUID advertises SMAP, independent of CR4), so no
/// CPU ever runs the plain stub against an SMAP-active CR4.
static SMAP_STUBS: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Switch `copy_user_with_fixup` to the stac/clac stub variant. Called by
/// `cpu_features::enable_smap` on the first CPU to enable SMAP (idempotent).
pub fn arm_smap_stubs() {
    SMAP_STUBS.store(true, Ordering::SeqCst);
}

/// True if user copies currently run the stac/clac-bracketed stub.
pub fn smap_stubs_armed() -> bool {
    SMAP_STUBS.load(Ordering::Relaxed)
}

/// TEST-ONLY (SMAP behavioral probe): run the PLAIN (non-stac) movsb stub with
/// full extable protection. With `CR4.SMAP` active, a supervisor read of a
/// user page through this MUST #PF → fixup → `Err(())`; that observable fault
/// is exactly what the hardening smoketest asserts. Never use for real copies.
pub unsafe fn copy_user_no_stac_probe(src: *const u8, dst: *mut u8, len: usize) -> Result<(), ()> {
    if len == 0 {
        return Ok(());
    }
    let result = x86_64::instructions::interrupts::without_interrupts(|| {
        install(
            _raeen_copy_user_movsb_start as u64,
            _raeen_copy_user_movsb_end as u64,
            _raeen_copy_user_movsb_fixup as u64,
        );
        let status = unsafe { _raeen_copy_user_movsb(src, dst, len) };
        clear();
        status
    });
    if result == 0 {
        Ok(())
    } else {
        Err(())
    }
}

/// Safe wrapper: arm the fixup, run the movsb stub, disarm. Returns
/// `Err(())` if the kernel page-faulted inside `[start, end)` and was
/// recovered to the fixup label.
///
/// # Safety
///
/// - `src` must be a valid pointer for `len` bytes of read in the current
///   address space (typically a freshly `validate_user_range`-checked user
///   pointer). If a fault occurs we recover; any other UB on the unsafe
///   read is on the caller.
/// - `dst` must be a valid pointer for `len` bytes of write in the kernel
///   half of the address space. Faults on `dst` are kernel bugs and won't
///   be covered by this fixup.
pub unsafe fn copy_user_with_fixup(src: *const u8, dst: *mut u8, len: usize) -> Result<(), ()> {
    if len == 0 {
        return Ok(());
    }
    // Hold interrupts off so the fixup install→call→clear sequence is
    // atomic with respect to this CPU's preemption. Cross-CPU concurrency
    // is fine — each CPU's slot is private. Once SMAP is armed, dispatch the
    // stac/clac-bracketed stub so the user-page touch is legal under
    // CR4.SMAP; both variants carry their own extable labels.
    let result = x86_64::instructions::interrupts::without_interrupts(|| {
        if SMAP_STUBS.load(Ordering::Relaxed) {
            install(
                _raeen_copy_user_movsb_smap_start as u64,
                _raeen_copy_user_movsb_smap_end as u64,
                _raeen_copy_user_movsb_smap_fixup as u64,
            );
            let status = unsafe { _raeen_copy_user_movsb_smap(src, dst, len) };
            clear();
            status
        } else {
            install(
                _raeen_copy_user_movsb_start as u64,
                _raeen_copy_user_movsb_end as u64,
                _raeen_copy_user_movsb_fixup as u64,
            );
            let status = unsafe { _raeen_copy_user_movsb(src, dst, len) };
            clear();
            status
        }
    });
    if result == 0 {
        Ok(())
    } else {
        Err(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Boot smoketest
// ═══════════════════════════════════════════════════════════════════════════

pub fn run_boot_smoketest() {
    use core::sync::atomic::Ordering::Relaxed;

    // Sanity: install + check + clear cycle on the current CPU.
    install(0x1000, 0x2000, 0xDEADBEEF);
    let hit_inside = check(0x1500);
    let hit_outside = check(0x3000);
    clear();
    let after_clear = check(0x1500);

    let installs_before = FIXUP_INSTALLS.load(Relaxed);
    let hits_before = FIXUP_HITS.load(Relaxed);

    // Live test: copy from a kernel-resident buffer into another kernel
    // buffer (using the same copy_user_with_fixup path). No fault should
    // occur — we're just proving the happy path doesn't break.
    let src = [0xABu8; 64];
    let mut dst = [0u8; 64];
    let live_ok = unsafe { copy_user_with_fixup(src.as_ptr(), dst.as_mut_ptr(), src.len()) }
        .is_ok()
        && dst == src;

    let installs_after = FIXUP_INSTALLS.load(Relaxed);

    let pass = hit_inside == Some(0xDEADBEEF)
        && hit_outside.is_none()
        && after_clear.is_none()
        && live_ok
        && installs_after >= installs_before + 1;

    crate::serial_println!(
        "[extable] smoketest: install_check={} miss_check={} clear_check={} live_copy={} installs+={} hits={} -> {}",
        hit_inside == Some(0xDEADBEEF),
        hit_outside.is_none(),
        after_clear.is_none(),
        live_ok,
        installs_after - installs_before,
        FIXUP_HITS.load(Relaxed) - hits_before,
        if pass { "PASS" } else { "FAIL" },
    );
}

pub fn dump_text() -> alloc::string::String {
    use core::fmt::Write;
    let mut s = alloc::string::String::new();
    let _ = writeln!(s, "# extable — copy_from_user fault-fixup");
    let _ = writeln!(s, "installs: {}", FIXUP_INSTALLS.load(Ordering::Relaxed));
    let _ = writeln!(s, "hits:     {}", FIXUP_HITS.load(Ordering::Relaxed));
    let _ = writeln!(s, "per-cpu armed slots:");
    for (i, slot) in FIXUP.iter().enumerate() {
        let start = slot.start_rip.load(Ordering::Relaxed);
        if start != 0 {
            let _ = writeln!(
                s,
                "  cpu{}: start={:#x} end={:#x} fixup={:#x}",
                i,
                start,
                slot.end_rip.load(Ordering::Relaxed),
                slot.fixup_rip.load(Ordering::Relaxed),
            );
        }
    }
    s
}
