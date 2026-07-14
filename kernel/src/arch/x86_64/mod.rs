//! x86_64 backend for the `arch::` Hardware Abstraction Layer (Slice 0).
//!
//! This is the concrete x86_64 implementation behind the arch-neutral surface
//! declared in [`crate::arch`]. It does NOT introduce any new mechanism: every
//! function here delegates to the already-proven x86_64 primitives (the
//! `x86_64` crate's `instructions::interrupts` and `instructions::port`), so
//! behavior on x86_64 is byte-identical to calling those primitives directly.
//!
//! Future backends (`arch/aarch64.rs`, `arch/i686.rs`) provide the same item
//! names; [`crate::arch`] selects exactly one via `#[cfg(target_arch = …)]`.
//!
//! Seams relocated so far: arch identity, CPU control, and port I/O (Slice 0);
//! interrupt-vector install (Slice 0b-1), BSP descriptor-table load (Slice 0b-2),
//! interrupt-controller end-of-interrupt (Slice 0b-3), per-CPU periodic timer
//! arm (Slice 0b-4), and the arch-neutral memory-ADDRESS value types
//! `PhysAddr`/`VirtAddr`/`Frame` (Slice 1, sub-slice 1a — see [`addr`]). The
//! paging-TABLE half (the `OffsetPageTable`/`Mapper` ops that consume those
//! addresses) / APIC IPI / context-switch / SMP / syscall-entry / AP
//! descriptor-table paths are later slices (see
//! docs/research/multi-arch-abstraction.md §"The seam list" +
//! docs/research/slice1-arch-neutral-mm-newtypes.md).

// ---------------------------------------------------------------------------
// 1. Arch IDENTITY — const facts about this ISA.
// ---------------------------------------------------------------------------

/// Stable, human-readable ISA name. Matches `core::env!`/target_arch token.
pub const NAME: &str = "x86_64";

/// Native pointer width in bits.
pub const POINTER_WIDTH: usize = 64;

/// Smallest hardware page granule in bytes (x86_64 4 KiB base page).
pub const PAGE_SIZE: usize = 4096;

/// `true` if this ISA is little-endian. x86_64 is always little-endian.
pub const IS_LITTLE_ENDIAN: bool = true;

/// Interrupt-controller family name (for `/proc/raeen/arch` reporting).
pub const INTERRUPT_CONTROLLER: &str = "APIC";

/// Time-source family name (for `/proc/raeen/arch` reporting).
pub const TIMER_SOURCE: &str = "TSC+LAPIC";

// ---------------------------------------------------------------------------
// 1b. Memory ADDRESS value types (Slice 1, sub-slice 1a).
//     The arch-neutral PhysAddr/VirtAddr/Frame seam. On x86_64 these are
//     zero-cost transparent aliases to the proven x86_64-crate types; the
//     aarch64 backend (later) supplies its own types of the same names. See
//     [`addr`] for the full design + aarch64 counterpart notes.
// ---------------------------------------------------------------------------

pub mod addr;
pub use addr::{Frame, PhysAddr, VirtAddr};

// ---------------------------------------------------------------------------
// 1c. Paging-TABLE seam (Slice 1.5, sub-slice 1.5a).
//     The arch-neutral mmu seam: a concrete `AddressSpace` (cfg-selected
//     internals) + `PageFlags`/`CacheType` + the free fns (`kernel`,
//     `current_user`, `from_root`, `flush`, `user_root_token`). On x86_64 each
//     verb DELEGATES to the existing `crate::memory::*` paging functions (zero
//     behavior change this round); the aarch64 backend (later) supplies its own
//     `OffsetPageTable`-vs-`aarch64_logic` internals behind the same surface.
//     See [`mmu`] for the full design + aarch64 counterpart notes.
// ---------------------------------------------------------------------------

pub mod mmu;
pub use mmu::{AddressSpace, CacheType, MmuError, PageFlags, PageProt, Root};

// ---------------------------------------------------------------------------
// 2. CPU control — delegates to the existing x86_64-crate primitives.
// ---------------------------------------------------------------------------

/// Spin-loop hint (`pause`). Used by busy-wait loops to relax the core.
#[inline(always)]
pub fn cpu_relax() {
    core::hint::spin_loop();
}

/// Halt the calling CPU until the next interrupt (`hlt`). Never returns to the
/// caller's control flow voluntarily; callers that need to resume should put
/// this in a loop.
#[inline(always)]
pub fn halt() {
    x86_64::instructions::hlt();
}

/// `true` if maskable interrupts are currently enabled (RFLAGS.IF set).
#[inline(always)]
pub fn interrupts_enabled() -> bool {
    x86_64::instructions::interrupts::are_enabled()
}

/// Disable maskable interrupts on the calling CPU (`cli`).
///
/// # Safety
/// Disabling interrupts changes global execution state. Callers must ensure the
/// IF flag is restored (prefer [`without_interrupts`] for scoped use).
#[inline(always)]
pub unsafe fn disable_interrupts() {
    x86_64::instructions::interrupts::disable();
}

/// Enable maskable interrupts on the calling CPU (`sti`).
///
/// # Safety
/// Enabling interrupts before the IDT/APIC are configured can take a fault into
/// an unconfigured handler. Callers must ensure the interrupt path is ready.
#[inline(always)]
pub unsafe fn enable_interrupts() {
    x86_64::instructions::interrupts::enable();
}

/// Run `f` with maskable interrupts disabled, then restore the prior IF state
/// (save+restore — re-enables only if interrupts were enabled on entry).
#[inline(always)]
pub fn without_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    x86_64::instructions::interrupts::without_interrupts(f)
}

// ---------------------------------------------------------------------------
// 3. Port I/O — delegates to the existing x86_64-crate `Port` type.
//    (aarch64/i686 backends: aarch64 has no port space and will lower these to
//    a compile error or MMIO-shim; i686 reuses this same PIO.)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 4. Interrupt-vector table install (Slice 0b — first relocated seam).
//    x86_64: load the IDT (LIDT). aarch64 equivalent: program VBAR_EL1 to point
//    at the 16-entry, 0x80-aligned exception vector table. Both make the CPU's
//    trap/exception entry path live; the arch-neutral kernel only needs "install
//    the vectors" + "are they installed" without naming IDT-vs-VBAR.
// ---------------------------------------------------------------------------

pub mod interrupts {
    use x86_64::instructions::tables::sidt;

    /// Install this CPU's interrupt/exception vector table.
    ///
    /// x86_64: executes `LIDT`, pointing the CPU at the kernel's global
    /// `InterruptDescriptorTable` (breakpoint, double-fault, page-fault, timer,
    /// keyboard, …). Idempotent and safe to call on every core (BSP and each AP
    /// reload the same shared table).
    ///
    /// aarch64 backend (later slice): writes `VBAR_EL1` with the base of the
    /// 16-entry exception vector table + `isb`.
    #[inline]
    pub fn load_idt() {
        // Delegate to the existing proven seam — byte-identical behavior. The
        // global IDT and all handler stubs stay in `crate::interrupts`; this is
        // pure indirection (Slice 0b: relocate the *call* behind `arch::`, not
        // the table).
        crate::interrupts::init_idt();
    }

    /// `true` if a non-empty interrupt vector table is currently installed on
    /// this CPU. Used by the arch smoketest to prove [`load_idt`] took effect.
    ///
    /// x86_64: reads `IDTR` via `SIDT` and checks the base is non-null and the
    /// limit is non-zero (a loaded IDT has limit = 16*N-1 > 0 and a real base).
    ///
    /// aarch64 backend (later slice): reads `VBAR_EL1` and checks it is the
    /// expected, non-zero, 0x800-aligned vector-table base.
    #[inline]
    pub fn vectors_installed() -> bool {
        let ptr = sidt();
        ptr.base.as_u64() != 0 && ptr.limit != 0
    }
}

// ---------------------------------------------------------------------------
// 5. CPU descriptor-table load (Slice 0b — second relocated seam).
//    x86_64: load the GDT (LGDT) + set the segment registers + the TSS so the
//    CPU has kernel/user code+data segments and a kernel stack to trap onto.
//    aarch64 has NO segmentation/GDT — its equivalent boot-time CPU-structure
//    step is programming the exception stacks (SP_EL1) + FP/SIMD enable; there
//    is no descriptor table to load. The arch-neutral kernel only needs "set up
//    this CPU's descriptor/segment structures" + "are they loaded" without
//    naming GDT-vs-(nothing). On a GDT-less arch `gdt_loaded()` returns the
//    backend's equivalent readiness fact.
//
//    SCOPE (this slice): only the BSP descriptor-table load is relocated behind
//    the seam (`crate::gdt::init`). The AP paths (`gdt::init_ap`,
//    `gdt::init_ap_percpu`) carry the per-CPU TSS allocation + the
//    current_cpu_id/set_rsp0 context-switch coupling and stay where they are —
//    they are a LATER sub-slice (per docs/research/aarch64-bringup-spec.md
//    §"Slice 0b", which calls out gdt as more entangled than the IDT seam).
// ---------------------------------------------------------------------------

pub mod cpu {
    use x86_64::instructions::tables::sgdt;

    /// Load this CPU's segment-descriptor structures (BSP).
    ///
    /// x86_64: executes `LGDT`, points the CPU at the kernel's `GlobalDescriptorTable`
    /// (kernel/user code+data segments), sets CS/SS/DS/ES/FS/GS, and loads the
    /// BSP's TSS (RSP0 + double-fault IST) via `LTR`. Delegates to the existing,
    /// proven `crate::gdt::init` — the GDT, TSS, selectors, and per-CPU bookkeeping
    /// all stay in `crate::gdt`; this is pure indirection (Slice 0b: relocate the
    /// *call* behind `arch::`, not the table).
    ///
    /// aarch64 backend (later slice): NO GDT — sets up the per-EL stack pointers
    /// (`SP_EL1`) and enables FP/SIMD (`CPACR_EL1.FPEN`); there is no `LGDT`/`LTR`
    /// equivalent because aarch64 has no segmentation.
    #[inline]
    pub fn load_gdt() {
        crate::gdt::init();
    }

    /// `true` if a non-empty descriptor table is currently loaded on this CPU.
    /// Used by the arch smoketest to prove [`load_gdt`] took effect.
    ///
    /// x86_64: reads `GDTR` via `SGDT` and checks the base is non-null and the
    /// limit is non-zero (a loaded GDT has limit = 8*N-1 > 0 and a real base).
    ///
    /// aarch64 backend (later slice): aarch64 has no GDTR; this returns the
    /// equivalent CPU-structures-ready fact (e.g. `SP_EL1` programmed).
    #[inline]
    pub fn gdt_loaded() -> bool {
        let ptr = sgdt();
        ptr.base.as_u64() != 0 && ptr.limit != 0
    }
}

// ---------------------------------------------------------------------------
// 6. Interrupt-controller acknowledge (Slice 0b — third relocated seam).
//    x86_64: signal end-of-interrupt to the Local APIC (write 0 to the LAPIC
//    EOI register at MMIO offset 0xB0, or `wrmsr` MSR 0x80B in x2APIC mode).
//    aarch64 equivalent: write the interrupt ID back to `GICC_EOIR` (GICv2) /
//    `ICC_EOIR1_EL1` (GICv3) to deactivate the active interrupt. Both mean the
//    SAME arch-neutral thing — "I have finished servicing this interrupt; the
//    controller may deliver the next one" — without naming LAPIC-vs-GIC.
//
//    HOT PATH: this runs at the tail of EVERY hardware interrupt handler (timer,
//    keyboard, mouse, NIC, NVMe, MSI, …), in IF=0 interrupt context (§10). It is
//    `#[inline(always)]` so the arch:: indirection lowers to the exact same
//    codegen as calling `crate::apic::end_of_interrupt()` directly — ZERO added
//    per-interrupt cost (the Concept's latency contracts forbid a call on the
//    interrupt hot path).
// ---------------------------------------------------------------------------

pub mod interrupt_controller {
    /// Acknowledge the in-service interrupt at this CPU's interrupt controller
    /// (end-of-interrupt). Must be the last controller action of every hardware
    /// interrupt handler, so the controller can deliver the next interrupt.
    ///
    /// x86_64: writes the LAPIC EOI register (MMIO `0xB0`, or `wrmsr 0x80B` in
    /// x2APIC). Delegates to the existing, proven, lock-free
    /// `crate::apic::end_of_interrupt()` — the LAPIC base / x2APIC mode and the
    /// volatile write all stay in `crate::apic`; this is pure indirection
    /// (Slice 0b: relocate the *call* behind `arch::`, not the mechanism).
    ///
    /// aarch64 backend (later slice): writes the interrupt ID to `GICC_EOIR`
    /// (GICv2) / `ICC_EOIR1_EL1` (GICv3) to deactivate the active interrupt.
    ///
    /// `#[inline(always)]`: hot interrupt path — zero added per-IRQ cost.
    #[inline(always)]
    pub fn eoi() {
        crate::apic::end_of_interrupt();
    }
}

// ---------------------------------------------------------------------------
// 7. Per-CPU periodic timer arm (Slice 0b — fourth relocated seam).
//    x86_64: arm the Local APIC timer in periodic mode at the kernel's
//    `InterruptIndex::Timer` vector, using the globally-calibrated reload count
//    (`LAPIC_TIMER_TICKS`, measured once on the BSP against HPET/PIT for a 10 ms
//    / 100 Hz tick). Each CPU calls this once after its LAPIC is enabled to get
//    its own scheduler tick.
//    aarch64 equivalent: program the ARM Generic Timer physical counter —
//    write `CNTP_TVAL_EL0` (or `CNTP_CVAL_EL0`) with the next-fire delta derived
//    from `CNTFRQ_EL0`, set `CNTP_CTL_EL0.ENABLE=1` (`IMASK=0`), and route the
//    PPI (INTID 30) at the GIC. Both mean the SAME arch-neutral thing — "arm
//    this CPU's periodic scheduler-tick interrupt" — without naming
//    LAPIC-vs-CNTP. (spec A5 in docs/research/aarch64-bringup-spec.md.)
//
//    NOT hot-path: called once per CPU at bring-up, so a plain `#[inline]`
//    (not `inline(always)`) is sufficient.
// ---------------------------------------------------------------------------

pub mod timer {
    use core::sync::atomic::Ordering;

    /// Arm this CPU's periodic scheduler-tick timer.
    ///
    /// x86_64: configures the Local APIC timer (periodic mode, Div16) at the
    /// kernel's timer vector and loads the globally-calibrated reload count, so
    /// this CPU starts taking its 100 Hz scheduler tick. Delegates to the
    /// existing, proven `crate::apic::start_lapic_timer()` — the xAPIC-vs-x2APIC
    /// selection, the SVR/LVT programming, and the MMIO/MSR writes all stay in
    /// `crate::apic`; this is pure indirection (Slice 0b: relocate the *call*
    /// behind `arch::`, not the mechanism).
    ///
    /// Precondition: the timer must already be calibrated (the BSP's
    /// `apic::init_bsp` measures `LAPIC_TIMER_TICKS` once); the underlying call
    /// panics if invoked before calibration. This is only called from the AP
    /// bring-up path (`smp::init_ap_lapic`), well after BSP calibration.
    ///
    /// aarch64 backend (later slice): writes `CNTP_TVAL_EL0` + enables
    /// `CNTP_CTL_EL0` (ARM Generic Timer) and unmasks the timer PPI at the GIC.
    #[inline]
    pub fn arm_periodic() {
        crate::apic::start_lapic_timer();
    }

    /// `true` if this CPU's periodic timer can be armed — i.e. the global tick
    /// reload count has been calibrated. Used by the arch smoketest to read the
    /// seam's state without firing the timer (arming pre-calibration would
    /// panic). At the smoketest site this is expectedly `false` (calibration
    /// happens later, during ACPI/APIC bring-up); the true proof that
    /// [`arm_periodic`] works is the scheduler making progress after the boot
    /// marker (timer IRQs driving task switches).
    ///
    /// x86_64: reads the calibrated `LAPIC_TIMER_TICKS` (non-zero once the BSP
    /// has measured the 10 ms reload count against HPET/PIT).
    ///
    /// aarch64 backend (later slice): reads `CNTFRQ_EL0` (the Generic Timer
    /// frequency is firmware-programmed and always available, so the aarch64
    /// equivalent is ready earlier).
    #[inline]
    pub fn periodic_armable() -> bool {
        crate::apic::LAPIC_TIMER_TICKS.load(Ordering::SeqCst) != 0
    }
}

pub mod port {
    use x86_64::instructions::port::Port;

    /// Read a byte from an x86 I/O port.
    ///
    /// # Safety
    /// Port I/O has device-specific side effects; the caller must own the port.
    #[inline(always)]
    pub unsafe fn inb(port: u16) -> u8 {
        let mut p: Port<u8> = Port::new(port);
        p.read()
    }

    /// Write a byte to an x86 I/O port.
    ///
    /// # Safety
    /// Port I/O has device-specific side effects; the caller must own the port.
    #[inline(always)]
    pub unsafe fn outb(port: u16, value: u8) {
        let mut p: Port<u8> = Port::new(port);
        p.write(value);
    }

    /// Read a word (16-bit) from an x86 I/O port.
    ///
    /// # Safety
    /// Port I/O has device-specific side effects; the caller must own the port.
    #[inline(always)]
    pub unsafe fn inw(port: u16) -> u16 {
        let mut p: Port<u16> = Port::new(port);
        p.read()
    }

    /// Write a word (16-bit) to an x86 I/O port.
    ///
    /// # Safety
    /// Port I/O has device-specific side effects; the caller must own the port.
    #[inline(always)]
    pub unsafe fn outw(port: u16, value: u16) {
        let mut p: Port<u16> = Port::new(port);
        p.write(value);
    }

    /// Read a dword (32-bit) from an x86 I/O port.
    ///
    /// # Safety
    /// Port I/O has device-specific side effects; the caller must own the port.
    #[inline(always)]
    pub unsafe fn inl(port: u16) -> u32 {
        let mut p: Port<u32> = Port::new(port);
        p.read()
    }

    /// Write a dword (32-bit) to an x86 I/O port.
    ///
    /// # Safety
    /// Port I/O has device-specific side effects; the caller must own the port.
    #[inline(always)]
    pub unsafe fn outl(port: u16, value: u32) {
        let mut p: Port<u32> = Port::new(port);
        p.write(value);
    }
}
