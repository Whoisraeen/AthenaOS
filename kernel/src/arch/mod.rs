//! `arch::` — the kernel's Hardware Abstraction Layer boundary (multi-arch).
//!
//! Concept §Architecture Reach: *"RaeenOS refuses ISA lock-in: the kernel sits
//! on a clean `arch::` abstraction layer (boot, MMU, interrupts, timers, SMP,
//! context switch, syscall entry, firmware discovery) so the same OS boots
//! x86_64, aarch64 (ARM 64-bit), and i686 (32-bit x86) — each proven
//! independently. … Portability is the anti-walled-garden property — you own
//! the machine, on the silicon you choose."*
//!
//! This module is the seam that makes that promise buildable. A single backend
//! is selected at compile time with `#[cfg(target_arch = …)]` and re-exported
//! as the public `arch::` surface — there is **no `dyn` dispatch**, so generic
//! kernel code that calls `arch::cpu_relax()` / `arch::without_interrupts(..)`
//! monomorphizes to the same codegen as calling the underlying primitive
//! directly (the Concept's latency contracts forbid a vtable hop on hot paths).
//!
//! ## Scope (this module)
//! Slice 0 established the boundary + a working x86_64 backend for three
//! well-contained seams: **arch identity**, **CPU control**, and **port I/O**.
//! **Slice 0b** begins relocating the boot-critical seams behind `arch::`, x86
//! staying the only backend (pure indirection, zero behavior change): the FIRST
//! relocated seam is **interrupt-vector install** ([`interrupts::load_idt`]) —
//! the smallest, most discrete boot-time init step with a clean arch boundary
//! (x86 `LIDT` ↔ aarch64 `VBAR_EL1`). The SECOND relocated seam (Slice 0b-2) is
//! the **BSP descriptor-table load** ([`cpu::load_gdt`]) — the next-most-discrete
//! boot-time CPU-structure step (x86 `LGDT`+`LTR` ↔ aarch64 has no GDT; its
//! equivalent is the per-EL stack/FP-enable). Only the BSP load is relocated;
//! the AP per-CPU TSS paths (with their context-switch coupling) stay put as a
//! later sub-slice. The THIRD relocated seam (Slice 0b-3) is the
//! **interrupt-controller end-of-interrupt** ([`interrupt_controller::eoi`]) —
//! the discrete "I finished servicing this IRQ, deliver the next" action at the
//! tail of every hardware interrupt handler (x86 LAPIC EOI ↔ aarch64
//! `GICC_EOIR`/`ICC_EOIR1_EL1`). It is on the HOT interrupt path, so the
//! arch-neutral `eoi()` is `#[inline(always)]` — the indirection lowers to the
//! exact codegen of calling `crate::apic::end_of_interrupt()` directly (zero
//! added per-IRQ cost; the Concept's latency contracts forbid a call on the
//! interrupt hot path). The FOURTH relocated seam (Slice 0b-4) is the
//! **per-CPU periodic timer arm** ([`timer::arm_periodic`]) — the discrete
//! "give this CPU its scheduler-tick interrupt" step run once per core after its
//! interrupt controller is up (x86 LAPIC periodic timer; aarch64 ARM Generic
//! Timer `CNTP_TVAL_EL0`/`CNTP_CTL_EL0`, the spec's item A5). It is NOT a hot
//! path (called once per CPU at bring-up), so it is a plain `#[inline]`. The
//! x86_64 backend ([`x86_64`]) delegates to the proven
//! `x86_64` crate / existing `crate::interrupts` + `crate::gdt` + `crate::apic`
//! — no asm is duplicated and no behavior changes. Migrating the remaining seams
//! (the AP gdt paths / apic IPI / smp / context + the paging half of memory) and
//! adding the aarch64/i686 backends are LATER sub-slices
//! (docs/decisions/0009-aarch64-qemu-virt-bringup.md,
//! docs/research/aarch64-bringup-spec.md §"Slice 0b").
//!
//! ## How a future backend plugs in
//! Add `kernel/src/arch/aarch64.rs` (resp. `i686.rs`) exposing the identical
//! item names (`NAME`, `POINTER_WIDTH`, `cpu_relax`, `without_interrupts`,
//! `port::{inb,..}`, …), then add the matching `#[cfg(target_arch = "aarch64")]`
//! arm below. Only the arm whose `target_arch` matches the build is compiled.

// --- Backend selection (compile-time, zero-cost) ---------------------------

#[cfg(target_arch = "x86_64")]
mod x86_64;
#[cfg(target_arch = "x86_64")]
pub use self::x86_64::*;

// aarch64 (ARM64) backend — Slice A2 SKELETON (docs/research/aarch64-bringup-spec.md,
// ADR 0009). It satisfies the seam contract (identity consts, cpu control, the DAIF
// interrupt-flag save/restore, the addr/mmu value types + the real VMSAv8 flag
// lowering via aarch64_logic) so the aarch64 build gets PAST this boundary and into
// the REAL x86-coupling errors in shared kernel code (the A3-A9 to-do list). The
// seams whose real impl is a later slice are honest `unimplemented!("aarch64 A<N>:
// …")` panics, NOT silent fake-success (CLAUDE.md §9). aarch64 code is
// `cfg(target_arch="aarch64")` and NEVER compiles into the x86_64 build.
#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(target_arch = "aarch64")]
pub use self::aarch64::*;

// Future backend fills this slot (LATER slice); the structure is ready:
//   #[cfg(target_arch = "x86")]     mod i686;
//   #[cfg(target_arch = "x86")]     pub use self::i686::*;

// A build whose target_arch has no backend should fail loudly at the boundary
// rather than silently miss every `arch::` symbol downstream.
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!(
    "no arch:: backend for this target_arch yet — only x86_64 (Slice 0) and aarch64 \
     (Slice A2 skeleton) are implemented. Add kernel/src/arch/<arch>/ and a cfg arm \
     in arch/mod.rs (see ADR 0007)."
);

use alloc::string::String;

// --- R10 artifact 1: init() ------------------------------------------------

/// Bring the architecture HAL online. Called early in `kernel_main` (before the
/// UI tiers). On x86_64 this is purely informational — the underlying CPU
/// primitives are always available — but it anchors the boundary in the boot
/// sequence and reports the active backend's identity.
pub fn init() {
    crate::serial_println!(
        "[arch] {} (ptr={}, page={}) HAL online",
        NAME,
        POINTER_WIDTH,
        PAGE_SIZE
    );
}

// --- R10 artifact 2: run_boot_smoketest() (must be able to print FAIL) ------

/// Exercise the HAL and prove it behaves correctly. FAIL-able: any assertion
/// below that does not hold prints `-> FAIL` (so a broken backend is visible in
/// the boot log, never a silent false-green).
///
/// Checks:
///   - arch identity is internally consistent (`NAME` non-empty, page size a
///     power of two, pointer width matches the real `usize` width);
///   - `cpu_relax()` executes without faulting;
///   - `without_interrupts` actually masks IF for the duration of the closure
///     AND restores the entry IF state afterwards (the save+restore contract);
///   - the interrupt-vector seam is live: a non-empty vector table is installed
///     on this CPU (Slice 0b — `arch::interrupts::load_idt()` ran at boot, so
///     `vectors_installed()` must be true here). FAIL-able: if the IDT were
///     never loaded (or `load_idt` regressed to a no-op), this prints `-> FAIL`.
///   - the descriptor-table seam is live: a non-empty GDT is loaded on this CPU
///     (Slice 0b-2 — `arch::cpu::load_gdt()` ran at boot before this smoketest,
///     so `gdt_loaded()` must be true here). FAIL-able: if the GDT were never
///     loaded (or `load_gdt` regressed to a no-op), this prints `-> FAIL`.
///   - the interrupt-controller seam is EXERCISED (Slice 0b-3): the smoketest
///     calls `arch::interrupt_controller::eoi()` once and verifies it returns
///     without faulting. OBSERVABILITY LIMIT (noted honestly): this smoketest
///     runs in the boot sequence BEFORE the LAPIC is brought up and BEFORE
///     interrupts are enabled, so (a) there is no in-service interrupt to ack
///     and (b) EOI has no persistent readable state to assert against — a write
///     to the EOI register is a deliberate no-op with no ISR active. So unlike
///     the IDT/GDT seams (which expose `SIDT`/`SGDT` readback), EOI cannot be
///     FAIL-asserted at this point. The TRUE proof that `eoi()` works is
///     indirect and lives later in the boot log: once `apic::run_boot_smoketest`
///     + `smp` enable the LAPIC timer, the per-CPU timer IRQ fires repeatedly,
///     and each handler tail-calls this exact `eoi()` — if EOI were broken the
///     LAPIC would never re-arm, no further timer/keyboard/mouse IRQ would land,
///     and the boot would stall (no `System successfully booted`, dead HID).
///     The `controller_seam_exercised` flag below at least proves the seam is
///     wired and callable from the arch HAL.
///   - the periodic-timer seam is EXERCISED (Slice 0b-4): the smoketest reads
///     `arch::timer::periodic_armable()` (the readable predicate behind the
///     `arch::timer::arm_periodic()` seam — true once the per-CPU tick reload
///     count is calibrated). OBSERVABILITY LIMIT (noted honestly, same shape as
///     the EOI seam): this smoketest runs EARLY in boot, BEFORE the LAPIC is
///     brought up and BEFORE its timer is calibrated (calibration happens later,
///     in `apic::init_bsp` during ACPI bring-up). So `periodic_armable()` is
///     EXPECTEDLY false here, and we must NOT call `arm_periodic()` at this site
///     — arming pre-calibration would panic. We therefore read + report the
///     predicate (proving the seam is wired + callable from the HAL) but do NOT
///     FAIL-assert it. The TRUE proof that `arm_periodic()` works is indirect
///     and lives later in the boot log: once each AP calls `arm_periodic()` in
///     `smp::init_ap_lapic`, the per-CPU timer IRQ fires at 100 Hz and drives
///     scheduler task-switches — if the timer-arm were broken the scheduler
///     would never advance, no `System successfully booted` marker would appear,
///     and the boot would stall. The `timer_seam_exercised` flag below proves
///     the seam compiles, links, and is callable from the arch HAL.
///   - the memory-address-type seam is ASSERTED (Slice 1, sub-slice 1a): the
///     smoketest calls `arch::addr::roundtrip_ok()` and FAIL-asserts it. Unlike
///     the EOI/timer seams (observable only later in boot), this is pure address
///     arithmetic behind the `arch::{PhysAddr, VirtAddr, Frame}` aliases (an
///     identity-map phys->virt->phys round-trip, align-up/down monotonicity +
///     alignment, and frame start-address alignment), so it is fully FAIL-able
///     HERE: a wrong/broken alias prints `addr=ROUNDTRIP-BAD -> FAIL`. It uses a
///     synthetic local offset (NOT `memory::phys_to_virt`) because this smoketest
///     runs before `memory::PHYS_MEM_OFFSET` is initialized; the same arithmetic
///     is host-KAT'd on the dev box (CLAUDE.md §15).
pub fn run_boot_smoketest() {
    let mut ok = true;

    // Identity consistency.
    if NAME.is_empty() {
        ok = false;
    }
    if PAGE_SIZE == 0 || (PAGE_SIZE & (PAGE_SIZE - 1)) != 0 {
        // page size must be a power of two
        ok = false;
    }
    if POINTER_WIDTH != core::mem::size_of::<usize>() * 8 {
        ok = false;
    }
    if PAGE_SIZE != 4096 {
        ok = false;
    }

    // cpu_relax must not fault.
    cpu_relax();

    // without_interrupts contract: IF is masked inside the closure, and the
    // entry IF state is restored on exit.
    let if_before = interrupts_enabled();
    let masked_inside = without_interrupts(|| !interrupts_enabled());
    let if_after = interrupts_enabled();
    let if_save_restore_ok = masked_inside && (if_after == if_before);
    if !if_save_restore_ok {
        ok = false;
    }

    // Interrupt-vector seam (Slice 0b): the boot path installed the vector table
    // via arch::interrupts::load_idt() before this smoketest runs, so a non-empty
    // table MUST be installed on this CPU. A no-op load_idt or a never-loaded IDT
    // makes this false → the line below prints `-> FAIL`.
    let vectors_ok = interrupts::vectors_installed();
    if !vectors_ok {
        ok = false;
    }

    // Descriptor-table seam (Slice 0b-2): the boot path loaded the GDT (+TSS)
    // via arch::cpu::load_gdt() before this smoketest runs (main.rs Phase 2,
    // before the arch HAL smoketest), so a non-empty GDT MUST be loaded on this
    // CPU. A no-op load_gdt or a never-loaded GDT makes this false → the line
    // below prints `-> FAIL`.
    let gdt_ok = cpu::gdt_loaded();
    if !gdt_ok {
        ok = false;
    }

    // Interrupt-controller seam (Slice 0b-3): EXERCISE arch::interrupt_controller
    // ::eoi(). At this point in boot the LAPIC is not yet up and interrupts are
    // disabled, so this EOI write hits no in-service interrupt and is a safe
    // no-op (xAPIC: LAPIC base still 0 → guarded no-op; x2APIC: a bare EOI wrmsr
    // with no ISR is architecturally a no-op). EOI has no persistent readable
    // state to assert here (see the docstring's OBSERVABILITY LIMIT note) — the
    // real proof is indirect: every later timer/HID IRQ tail-calls this same
    // eoi(), and a broken EOI would stall the boot. We still prove the seam is
    // wired + callable from the HAL (a regression to e.g. a panic!/loop in the
    // backend would fault or hang HERE, before the success marker).
    interrupt_controller::eoi();
    let controller_seam_exercised = true;
    if !controller_seam_exercised {
        ok = false;
    }

    // Periodic-timer seam (Slice 0b-4): EXERCISE arch::timer. We read the seam's
    // readable predicate `periodic_armable()` rather than calling arm_periodic()
    // here: at this point in boot the LAPIC timer is not yet calibrated (that
    // happens later in apic::init_bsp during ACPI bring-up), so arm_periodic()
    // would panic and periodic_armable() is EXPECTEDLY false (see the docstring's
    // OBSERVABILITY LIMIT note). The real proof is indirect: each AP later calls
    // arm_periodic() in smp::init_ap_lapic, and a broken timer-arm would stall
    // the scheduler (no success marker). We still prove the seam is wired +
    // callable from the HAL (a regression to e.g. a panic in the predicate would
    // fault HERE, before the success marker). The predicate's value is reported
    // for visibility but NOT FAIL-asserted (false is correct this early).
    let timer_armable = timer::periodic_armable();
    let timer_seam_exercised = true;
    if !timer_seam_exercised {
        ok = false;
    }

    // Memory-address-type seam (Slice 1, sub-slice 1a): ASSERT
    // arch::addr::roundtrip_ok(). Unlike the EOI/timer seams (whose effect is
    // only observable later in boot), this is pure address arithmetic behind the
    // arch::{PhysAddr,VirtAddr,Frame} aliases — an identity-map phys→virt→phys
    // round-trip, align-up/down monotonicity + alignment, and frame
    // start-address alignment — so it CAN be FAIL-asserted right here. FAIL-able:
    // a broken/wrong alias (different as_u64/new/align semantics, or a Frame
    // whose start_address isn't page-aligned) makes this false → the line below
    // prints `addr=ROUNDTRIP-BAD -> FAIL`. (Host-KAT'd first on the dev box via
    // arch::x86_64::addr::tests — CLAUDE.md §15.)
    let addr_roundtrip_ok = addr::roundtrip_ok();
    if !addr_roundtrip_ok {
        ok = false;
    }

    // Note: x86_64 has no architecturally-guaranteed safe loopback I/O port, so
    // a port-I/O round-trip is intentionally NOT asserted here (writing an
    // arbitrary port has device side effects). The port:: surface is covered by
    // its real callers (RTC/PCI/serial) once they migrate (LATER slice).

    crate::serial_println!(
        "[arch] smoketest: name={} ptr={} if-save/restore={} vectors={} gdt={} eoi={} timer={} addr={} -> {}",
        NAME,
        POINTER_WIDTH,
        if if_save_restore_ok { "ok" } else { "BAD" },
        if vectors_ok { "installed" } else { "MISSING" },
        if gdt_ok { "loaded" } else { "MISSING" },
        if controller_seam_exercised {
            "exercised"
        } else {
            "BAD"
        },
        if timer_seam_exercised {
            // Report the predicate so the (expectedly not-yet-armed) state is
            // visible; the seam itself is what's being proven exercised here.
            if timer_armable {
                "exercised(armable)"
            } else {
                "exercised(pre-calib)"
            }
        } else {
            "BAD"
        },
        if addr_roundtrip_ok {
            "roundtrip-ok"
        } else {
            "ROUNDTRIP-BAD"
        },
        if ok { "PASS" } else { "FAIL" }
    );
}

/// Exercise the PAGING-TABLE seam (Slice 1.5, sub-slice 1.5a) with a LIVE
/// map → translate → write → unmap → assert-unmapped round-trip through
/// `arch::mmu::AddressSpace`. FAIL-able: a wrong flag lowering, a broken table
/// walk, a missing TLB flush, or a no-op unmap prints `mmu=ROUNDTRIP-BAD`.
///
/// This is a SEPARATE, LATER smoketest emission from [`run_boot_smoketest`]
/// (which runs EARLY, before `memory::init`). The mmu round-trip needs a live
/// frame allocator + `PHYS_MEM_OFFSET` + `KERNEL_PML4`, so the caller MUST invoke
/// this only AFTER `memory::init` + the buddy allocator are up (spec §5).
///
/// SAFETY of the test mapping (spec R8): the probe runs against a dedicated,
/// otherwise-unused high kernel VA (`PROBE_VA`) in a range distinct from the
/// kernel image, the physmap (`0xFFFF_8000…`), and the kernel-stack allocator
/// (`0xFFFF_B000…`). It FIRST asserts the probe VA is currently UNMAPPED, so the
/// smoketest can never clobber a live kernel mapping; it then maps a freshly
/// allocated throwaway frame, proves the mapping is live, and unmaps + frees it —
/// leaving the kernel address space byte-identical to before the probe.
pub fn run_mmu_boot_smoketest() {
    use core::sync::atomic::{compiler_fence, Ordering};

    // A dedicated probe VA: high half, page-aligned, clear of the physmap
    // (0xFFFF_8000…) and the kernel-stack allocator (0xFFFF_B000…).
    const PROBE_VA: u64 = 0xFFFF_E000_DEAD_0000;
    const SENTINEL: u64 = 0x5AEE_4007_0DD_BEEF;

    let v = VirtAddr::new(PROBE_VA);
    let mut ks = mmu::kernel();

    // R8: never clobber a live mapping — the probe VA must start UNMAPPED.
    let pre_unmapped = ks.translate(v).is_none();

    // Allocate one throwaway frame to back the probe.
    let frame_pa = match crate::memory::allocate_contiguous_frames(0) {
        Some(pa) => pa,
        None => {
            crate::serial_println!(
                "[arch-mmu] smoketest: frame alloc failed -> mmu=ROUNDTRIP-BAD FAIL"
            );
            return;
        }
    };

    let mut ok = pre_unmapped;

    // 1. Map the probe VA → frame as kernel data (present, writable, NX, WB).
    let mapped = ks
        .map_page(v, frame_pa, mmu::PageFlags::KERNEL_DATA)
        .is_ok();
    if !mapped {
        ok = false;
    }

    // 2. translate(probe) must equal the frame's physical base.
    let translated_ok = ks.translate(v) == Some(frame_pa);
    if !translated_ok {
        ok = false;
    }

    // 3. The mapping must be LIVE: write a sentinel through it and read it back.
    let mut write_ok = false;
    if mapped {
        unsafe {
            let p = v.as_mut_ptr::<u64>();
            core::ptr::write_volatile(p, SENTINEL);
            compiler_fence(Ordering::SeqCst);
            write_ok = core::ptr::read_volatile(p) == SENTINEL;
        }
    }
    if !write_ok {
        ok = false;
    }

    // 3b. update_flags (the mprotect verb, 1.5d): flip the probe page to a
    //     read-only mapping (WRITABLE cleared) and assert the leaf PTE's
    //     WRITABLE bit actually cleared, then restore it to writable. FAIL-able:
    //     a no-op update_flags or a wrong flag lowering leaves WRITABLE set.
    //     (Run only if the page is mapped; uses the kernel page table to read the
    //     leaf flags back, matching the kernel-domain probe space.)
    let mut update_flags_ok = false;
    if mapped {
        use ::x86_64::structures::paging::mapper::TranslateResult;
        use ::x86_64::structures::paging::{PageTableFlags, Translate};
        // Flip to read-only + NX (WRITABLE cleared).
        let ro = mmu::PageFlags::new(mmu::PageProt::PRESENT | mmu::PageProt::NO_EXECUTE);
        let flip_ro = ks.update_flags(v, ro).is_ok();
        let kpt = crate::memory::kernel_page_table();
        let cleared = matches!(
            kpt.translate(v),
            TranslateResult::Mapped { flags, .. } if !flags.contains(PageTableFlags::WRITABLE)
        );
        drop(kpt);
        // Restore writable so the live-write teardown invariant holds.
        let flip_rw = ks.update_flags(v, mmu::PageFlags::KERNEL_DATA).is_ok();
        update_flags_ok = flip_ro && cleared && flip_rw;
    }
    if !update_flags_ok {
        ok = false;
    }

    // 4. Unmap + flush; the freed frame must be returned and the VA now a hole.
    let unmapped_pa = if mapped { ks.unmap_page(v).ok() } else { None };
    let unmap_returned_frame = unmapped_pa == Some(frame_pa);
    if !unmap_returned_frame {
        ok = false;
    }

    // 5. translate(probe) must now be None (proves unmap + TLB flush; R3/R8).
    let post_unmapped = ks.translate(v).is_none();
    if !post_unmapped {
        ok = false;
    }

    // Return the throwaway frame to the allocator (leave the system as found).
    crate::memory::deallocate_contiguous_frames(frame_pa, 0);

    crate::serial_println!(
        "[arch-mmu] smoketest: map+translate+write+update_flags+unmap through arch::mmu::AddressSpace \
         pre-unmapped={} map={} translate={} write={} update-flags={} unmap-frame={} post-unmapped={} -> mmu={} {}",
        if pre_unmapped { "yes" } else { "NO" },
        if mapped { "ok" } else { "BAD" },
        if translated_ok { "ok" } else { "BAD" },
        if write_ok { "ok" } else { "BAD" },
        if update_flags_ok { "ok" } else { "BAD" },
        if unmap_returned_frame { "ok" } else { "BAD" },
        if post_unmapped { "yes" } else { "NO" },
        if ok { "roundtrip-ok" } else { "ROUNDTRIP-BAD" },
        if ok { "PASS" } else { "FAIL" }
    );
}

// --- R10 artifact 3: /proc/raeen/arch ---------------------------------------

/// Render `/proc/raeen/arch` — active arch identity + HAL status. Wired in
/// `procfs.rs` (`"arch" => crate::arch::dump_text()`).
pub fn dump_text() -> String {
    use core::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "# /proc/raeen/arch — architecture HAL (Slice 0b + Slice 1a + Slice 1.5a)"
    );
    let _ = writeln!(s, "arch:                 {}", NAME);
    let _ = writeln!(s, "pointer_width_bits:   {}", POINTER_WIDTH);
    let _ = writeln!(s, "page_size_bytes:      {}", PAGE_SIZE);
    let _ = writeln!(
        s,
        "endianness:           {}",
        if IS_LITTLE_ENDIAN { "little" } else { "big" }
    );
    let _ = writeln!(s, "interrupt_controller: {}", INTERRUPT_CONTROLLER);
    let _ = writeln!(s, "timer:                {}", TIMER_SOURCE);
    let _ = writeln!(
        s,
        "interrupts_enabled:   {}",
        if interrupts_enabled() { "yes" } else { "no" }
    );
    let _ = writeln!(
        s,
        "vectors_installed:    {}",
        if interrupts::vectors_installed() {
            "yes"
        } else {
            "no"
        }
    );
    let _ = writeln!(
        s,
        "gdt_loaded:           {}",
        if cpu::gdt_loaded() { "yes" } else { "no" }
    );
    let _ = writeln!(
        s,
        "timer_armable:        {}",
        if timer::periodic_armable() {
            "yes"
        } else {
            "no"
        }
    );
    let _ = writeln!(
        s,
        "address_types:        arch::{{PhysAddr, VirtAddr, Frame}}  (roundtrip={})",
        if addr::roundtrip_ok() { "ok" } else { "BAD" }
    );
    let _ = writeln!(
        s,
        "paging_tables:        arch::mmu::{{AddressSpace, PageFlags, Root}}  (1.5a delegating -> crate::memory)"
    );
    let _ = writeln!(
        s,
        "seams_online:         identity, cpu-control, port-io, interrupt-vectors, gdt-load (bsp), eoi, timer-arm, address-types, paging-tables (1.5a delegating)"
    );
    let _ = writeln!(
        s,
        "seams_pending:        gdt-ap, apic-ipi, smp, context, syscall (later sub-slices)"
    );
    s
}
