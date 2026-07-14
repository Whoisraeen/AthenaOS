//! aarch64 (ARM64) backend for the `arch::` Hardware Abstraction Layer (Slice A2).
//!
//! Concept §Architecture Reach: *"RaeenOS refuses ISA lock-in: the kernel sits
//! on a clean `arch::` abstraction layer (boot, MMU, interrupts, timers, SMP,
//! context switch, syscall entry, firmware discovery) so the same OS boots
//! x86_64, aarch64 (ARM 64-bit), and i686 (32-bit x86) — each proven
//! independently. … Portability is the anti-walled-garden property — you own
//! the machine, on the silicon you choose."*
//!
//! aarch64 is the load-bearing proof of that clause — it is the ISA macOS is
//! welded to, and the one that forces the `arch::` boundary to be genuinely
//! neutral rather than "x86 with tweaks."
//!
//! ## Scope (Slice A2 — SKELETON, not yet bootable — honest about it)
//! This is the SHELL that satisfies the `arch::` seam contract for aarch64 so the
//! aarch64 build gets PAST "no arch backend" (the `compile_error!` guard in
//! [`crate::arch`]) and INTO the real x86-coupling errors in shared kernel code —
//! the gap inventory that becomes the A3–A9 to-do list.
//!
//! Honesty rule (CLAUDE.md §9 — no silent stubs): every seam whose real aarch64
//! implementation is a later slice is an `unimplemented!("aarch64 A<N>: …")` with
//! a `// MasterChecklist aarch64 Slice A<N>` reference — a panic-marked
//! unimplemented is honest (it announces exactly what is not yet built and where
//! it lands); a silent empty-`Ok(())` that fakes success is NOT and is forbidden
//! here. The pieces that ARE real this round are the ones backed by the
//! host-KAT'd [`aarch64_logic`] crate (the VMSAv8 descriptor encoder, the GIC
//! register math, the `ESR_EL1` decode) plus the pure identity constants and the
//! address-type newtypes (which are real, simple `u64`-backed arithmetic).
//!
//! ## What is real vs. honest-unimplemented in this skeleton
//! - REAL: the arch identity consts (`NAME`/`PAGE_SIZE`/`INTERRUPT_CONTROLLER`/…),
//!   `cpu_relax`/`halt` (`yield`/`wfi` hints are safe to emit), the DAIF
//!   interrupt-flag save/restore (real `mrs`/`msr daif` — the `without_interrupts`
//!   contract the shared smoketest exercises), the [`addr`] PhysAddr/VirtAddr/Frame
//!   newtypes + `roundtrip_ok`, and the [`mmu`] `PageFlags`→VMSAv8 lowering via
//!   [`aarch64_logic::mmu`].
//! - HONEST-UNIMPLEMENTED (later slices): `interrupts::load_idt` (A4 — `VBAR_EL1`),
//!   `cpu::load_gdt` (A3 — `SP_EL1`/`CPACR_EL1`, aarch64 has no GDT),
//!   `interrupt_controller::eoi` (A5 — `GICC_EOIR`), `timer::arm_periodic` (A5 —
//!   generic timer), the [`mmu`] `AddressSpace` map/translate/unmap verbs (A4 —
//!   they need a live aarch64 frame allocator + `TTBR1_EL1` root, which do not
//!   exist until the A3/A4 boot path runs).
//!
//! ## How a caller reaches this
//! [`crate::arch`] selects this backend with `#[cfg(target_arch = "aarch64")]`
//! and re-exports it as the public `arch::` surface — identical item names to the
//! x86_64 backend, so shared kernel code that calls `arch::cpu_relax()` /
//! `arch::without_interrupts(..)` compiles unchanged. The aarch64 build then
//! fails on the SHARED code that still names x86 primitives directly (the A3–A9
//! work); this skeleton is what makes those the *next* errors instead of "no
//! backend at all."

// ---------------------------------------------------------------------------
// 1. Arch IDENTITY — const facts about this ISA (REAL).
// ---------------------------------------------------------------------------

/// Stable, human-readable ISA name. Matches the `target_arch` token.
pub const NAME: &str = "aarch64";

/// Native pointer width in bits.
pub const POINTER_WIDTH: usize = 64;

/// Smallest hardware page granule in bytes (aarch64 4 KiB base page — the
/// granule ADR 0009 §2 / the spec target, `TG0=4KiB`).
pub const PAGE_SIZE: usize = 4096;

/// `true` if this ISA is little-endian. aarch64 runs little-endian here (QEMU
/// `virt` + the AArch64 Linux boot protocol both little-endian).
pub const IS_LITTLE_ENDIAN: bool = true;

/// Interrupt-controller family name (for `/proc/raeen/arch` reporting). GICv2
/// is the spec's first target (ADR 0009 §2); GICv3 is a later swap behind the
/// same seam.
pub const INTERRUPT_CONTROLLER: &str = "GICv2";

/// Time-source family name (for `/proc/raeen/arch` reporting). The ARM Generic
/// Timer is the LAPIC-timer equivalent (spec A5).
pub const TIMER_SOURCE: &str = "ARM generic timer";

// ---------------------------------------------------------------------------
// 1b. Memory ADDRESS value types (Slice 1, sub-slice 1a — aarch64 backend).
// ---------------------------------------------------------------------------

pub mod addr;
pub use addr::{Frame, PhysAddr, VirtAddr};

// ---------------------------------------------------------------------------
// 1c. Paging-TABLE seam (Slice 1.5 / A4 — aarch64 backend).
// ---------------------------------------------------------------------------

pub mod mmu;
pub use mmu::{AddressSpace, CacheType, MmuError, PageFlags, PageProt, Root};

// ---------------------------------------------------------------------------
// 2. CPU control — REAL where it is safe to emit the instruction now.
// ---------------------------------------------------------------------------

/// Spin-loop hint. aarch64: a `yield` hint (the spin-loop relax instruction;
/// `core::hint::spin_loop()` lowers to `yield` on aarch64). REAL.
#[inline(always)]
pub fn cpu_relax() {
    core::hint::spin_loop();
}

/// Halt the calling CPU until the next interrupt. aarch64: `wfi` (Wait For
/// Interrupt). REAL — `wfi` is safe to emit at any EL.
#[inline(always)]
pub fn halt() {
    // SAFETY: `wfi` has no operands and no memory effect; it parks the core
    // until an interrupt/event. Safe to execute at EL1 unconditionally.
    unsafe {
        core::arch::asm!("wfi", options(nomem, nostack, preserves_flags));
    }
}

/// `true` if maskable IRQs are currently enabled. aarch64: read `DAIF` and test
/// the I (IRQ-mask) bit — DAIF.I *set* means IRQs are MASKED, so "enabled" is
/// the bit being clear. REAL.
#[inline(always)]
pub fn interrupts_enabled() -> bool {
    let daif: u64;
    // SAFETY: `mrs` of DAIF is a side-effect-free read of the current PSTATE
    // interrupt-mask bits.
    unsafe {
        core::arch::asm!("mrs {0}, daif", out(reg) daif, options(nomem, nostack, preserves_flags));
    }
    // DAIF.I is bit 7 of the DAIF read format. Set = masked.
    (daif & (1 << 7)) == 0
}

/// Disable maskable IRQs on the calling CPU. aarch64: `msr daifset, #2` (set the
/// I mask bit). REAL.
///
/// # Safety
/// Disabling interrupts changes global execution state; the caller must restore
/// the prior state (prefer [`without_interrupts`] for scoped use).
#[inline(always)]
pub unsafe fn disable_interrupts() {
    core::arch::asm!("msr daifset, #2", options(nomem, nostack, preserves_flags));
}

/// Enable maskable IRQs on the calling CPU. aarch64: `msr daifclr, #2` (clear
/// the I mask bit). REAL.
///
/// # Safety
/// Enabling interrupts before `VBAR_EL1` + the GIC are configured can take an
/// exception into an unconfigured vector. The caller must ensure the trap path
/// is ready (A4/A5).
#[inline(always)]
pub unsafe fn enable_interrupts() {
    core::arch::asm!("msr daifclr, #2", options(nomem, nostack, preserves_flags));
}

/// Run `f` with maskable IRQs disabled, then restore the prior DAIF.I state
/// (save+restore — re-enables only if IRQs were enabled on entry). aarch64:
/// save `DAIF`, `msr daifset,#2`, run, restore `DAIF`. REAL — this is the exact
/// contract the SHARED `arch::run_boot_smoketest` IF-save/restore assertion
/// checks, so it must be honest on aarch64.
#[inline(always)]
pub fn without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let was_enabled = interrupts_enabled();
    // SAFETY: scoped mask; we restore the entry state before returning.
    unsafe {
        disable_interrupts();
    }
    let r = f();
    if was_enabled {
        // SAFETY: only re-enable if IRQs were enabled on entry — the
        // save+restore contract (never enable a CPU that entered masked).
        unsafe {
            enable_interrupts();
        }
    }
    r
}

// ---------------------------------------------------------------------------
// 3. Port I/O — aarch64 has NO port space. MMIO is the only path.
//    The x86 firmware paths that call `arch::port::*` are x86-only and do not
//    compile into the aarch64 build; any stray caller that DID reach here is a
//    bug, so each verb is an honest `unimplemented!` (a panic-marked "this ISA
//    has no port space" is the correct, FAIL-loud behavior — never a silent
//    no-op that would fake a successful I/O).
// ---------------------------------------------------------------------------

pub mod port {
    /// aarch64 has no I/O port space — port reads are an x86-ism. A caller
    /// reaching here on aarch64 is a portability bug (an x86-only firmware path
    /// that should be `cfg`'d out). FAIL loudly rather than silently return 0.
    ///
    /// # Safety
    /// Unreachable on aarch64 (no port space); signature mirrors x86 for the
    /// seam contract.
    #[inline(always)]
    pub unsafe fn inb(_port: u16) -> u8 {
        // MasterChecklist aarch64 Slice A3: x86 port-IO callers must be cfg'd
        // out / lowered to MMIO; aarch64 has no port space.
        unimplemented!(
            "aarch64 has no I/O port space (inb) — port-IO is x86-only; cfg the caller out"
        )
    }

    /// aarch64 has no I/O port space — see [`inb`].
    ///
    /// # Safety
    /// Unreachable on aarch64 (no port space).
    #[inline(always)]
    pub unsafe fn outb(_port: u16, _value: u8) {
        // MasterChecklist aarch64 Slice A3.
        unimplemented!(
            "aarch64 has no I/O port space (outb) — port-IO is x86-only; cfg the caller out"
        )
    }

    /// aarch64 has no I/O port space — see [`inb`].
    ///
    /// # Safety
    /// Unreachable on aarch64 (no port space).
    #[inline(always)]
    pub unsafe fn inw(_port: u16) -> u16 {
        // MasterChecklist aarch64 Slice A3.
        unimplemented!(
            "aarch64 has no I/O port space (inw) — port-IO is x86-only; cfg the caller out"
        )
    }

    /// aarch64 has no I/O port space — see [`inb`].
    ///
    /// # Safety
    /// Unreachable on aarch64 (no port space).
    #[inline(always)]
    pub unsafe fn outw(_port: u16, _value: u16) {
        // MasterChecklist aarch64 Slice A3.
        unimplemented!(
            "aarch64 has no I/O port space (outw) — port-IO is x86-only; cfg the caller out"
        )
    }

    /// aarch64 has no I/O port space — see [`inb`].
    ///
    /// # Safety
    /// Unreachable on aarch64 (no port space).
    #[inline(always)]
    pub unsafe fn inl(_port: u16) -> u32 {
        // MasterChecklist aarch64 Slice A3.
        unimplemented!(
            "aarch64 has no I/O port space (inl) — port-IO is x86-only; cfg the caller out"
        )
    }

    /// aarch64 has no I/O port space — see [`inb`].
    ///
    /// # Safety
    /// Unreachable on aarch64 (no port space).
    #[inline(always)]
    pub unsafe fn outl(_port: u16, _value: u32) {
        // MasterChecklist aarch64 Slice A3.
        unimplemented!(
            "aarch64 has no I/O port space (outl) — port-IO is x86-only; cfg the caller out"
        )
    }
}

// ---------------------------------------------------------------------------
// 4. Interrupt-vector table install (seam: arch::interrupts::{load_idt,
//    vectors_installed}). aarch64 equivalent of LIDT is programming VBAR_EL1
//    with the 16-entry, 0x80-aligned exception vector table (spec A4).
// ---------------------------------------------------------------------------

pub mod interrupts {
    /// Install this CPU's exception vector table.
    ///
    /// aarch64 (A4): build the 16-entry (4 groups × {Sync,IRQ,FIQ,SError}),
    /// 0x800-aligned `VBAR_EL1` table and write `VBAR_EL1` + `isb`. Not built
    /// yet — the vector stubs + the `ESR_EL1`-decoding Sync handler (over
    /// [`aarch64_logic::esr`]) are Slice A4.
    #[inline]
    pub fn load_idt() {
        // MasterChecklist aarch64 Slice A4: VBAR_EL1 vector table + ESR_EL1
        // Sync-exception decode (aarch64_logic::esr).
        unimplemented!("aarch64 A4: VBAR_EL1 exception-vector install not yet implemented")
    }

    /// `true` if a non-empty exception vector table is installed on this CPU.
    ///
    /// aarch64 (A4): read `VBAR_EL1` and check it is the expected non-zero,
    /// 0x800-aligned base. Until A4 there is no table to report, so this is a
    /// FAIL-loud unimplemented (an honest "not yet" — never a fake `false`/`true`
    /// that the shared smoketest would silently believe).
    #[inline]
    pub fn vectors_installed() -> bool {
        // MasterChecklist aarch64 Slice A4.
        unimplemented!("aarch64 A4: VBAR_EL1 readback (vectors_installed) not yet implemented")
    }
}

// ---------------------------------------------------------------------------
// 5. CPU descriptor-table load (seam: arch::cpu::{load_gdt, gdt_loaded}).
//    aarch64 has NO GDT/segmentation; the equivalent boot-time CPU-structure
//    step is programming SP_EL1 (the exception stack) + enabling FP/SIMD via
//    CPACR_EL1.FPEN (spec A3). `gdt_loaded()` reports that readiness fact.
// ---------------------------------------------------------------------------

pub mod cpu {
    /// Set up this CPU's per-EL structures (BSP). aarch64 (A3): program `SP_EL1`
    /// (exception/kernel stack) + `CPACR_EL1.FPEN` (FP/SIMD enable). There is NO
    /// `LGDT`/`LTR` equivalent — aarch64 has no segmentation.
    #[inline]
    pub fn load_gdt() {
        // MasterChecklist aarch64 Slice A3: SP_EL1 / CPACR_EL1 setup (no GDT).
        unimplemented!(
            "aarch64 A3: SP_EL1 / CPACR_EL1 CPU-structure setup not yet implemented (no GDT)"
        )
    }

    /// `true` if this CPU's per-EL structures are ready (the GDT-less equivalent
    /// of "descriptor table loaded"). aarch64 (A3): `SP_EL1` programmed + FP/SIMD
    /// enabled. FAIL-loud until A3.
    #[inline]
    pub fn gdt_loaded() -> bool {
        // MasterChecklist aarch64 Slice A3.
        unimplemented!(
            "aarch64 A3: CPU-structures-ready predicate (gdt_loaded) not yet implemented"
        )
    }
}

// ---------------------------------------------------------------------------
// 6. Interrupt-controller acknowledge (seam: arch::interrupt_controller::eoi).
//    aarch64 equivalent of the LAPIC EOI is writing the interrupt ID back to
//    GICC_EOIR (GICv2) — the offset/encoding math is host-KAT'd in
//    aarch64_logic::gic; the MMIO write itself is Slice A5.
// ---------------------------------------------------------------------------

pub mod interrupt_controller {
    /// Acknowledge the in-service interrupt at the GIC CPU interface (GICv2:
    /// write the IAR value to `GICC_EOIR`). HOT PATH (tail of every IRQ handler).
    ///
    /// aarch64 (A5): write `GICC_EOIR` at `cpu_base + gic::GICC_EOIR`. The
    /// register offsets are real ([`aarch64_logic::gic`]); the live GIC CPU-iface
    /// base + the MMIO write are A5. Until then this is FAIL-loud — a silent
    /// no-op EOI would wedge the GIC after the first interrupt with no error.
    #[inline(always)]
    pub fn eoi() {
        // MasterChecklist aarch64 Slice A5: GICC_EOIR write (aarch64_logic::gic
        // supplies the offset; needs the live GIC CPU-interface MMIO base).
        unimplemented!("aarch64 A5: GICC_EOIR end-of-interrupt write not yet implemented")
    }
}

// ---------------------------------------------------------------------------
// 7. Per-CPU periodic timer arm (seam: arch::timer::{arm_periodic,
//    periodic_armable}). aarch64 equivalent of the LAPIC periodic timer is the
//    ARM Generic Timer: CNTP_TVAL_EL0 + CNTP_CTL_EL0.ENABLE, PPI 30 at the GIC
//    (spec A5).
// ---------------------------------------------------------------------------

pub mod timer {
    /// Arm this CPU's periodic scheduler-tick timer.
    ///
    /// aarch64 (A5): write `CNTP_TVAL_EL0` with the next-fire delta derived from
    /// `CNTFRQ_EL0`, set `CNTP_CTL_EL0.ENABLE=1` (`IMASK=0`), and unmask the
    /// timer PPI (INTID 30) at the GIC. FAIL-loud until A5.
    #[inline]
    pub fn arm_periodic() {
        // MasterChecklist aarch64 Slice A5: ARM Generic Timer CNTP arm.
        unimplemented!("aarch64 A5: ARM generic-timer CNTP periodic arm not yet implemented")
    }

    /// `true` if this CPU's periodic timer can be armed.
    ///
    /// aarch64 (A5): on aarch64 the Generic Timer frequency (`CNTFRQ_EL0`) is
    /// firmware-programmed and always available, so the real predicate will read
    /// `CNTFRQ_EL0 != 0` — armable earlier than x86's calibrated reload count.
    /// FAIL-loud until A5 (the timer module is not wired).
    #[inline]
    pub fn periodic_armable() -> bool {
        // MasterChecklist aarch64 Slice A5.
        unimplemented!("aarch64 A5: CNTFRQ_EL0 readback (periodic_armable) not yet implemented")
    }
}
