//! # aarch64_logic — the host-KAT-able aarch64 (ARM64) pure-logic foundation
//!
//! Concept clause this crate serves (verbatim from `kernel/src/arch/mod.rs`,
//! added by ADR 0007 to RaeenOS_Concept.md §Architecture / "Architecture Reach"):
//!
//! > "RaeenOS refuses ISA lock-in: the kernel sits on a clean `arch::`
//! > abstraction layer … so the same OS boots x86_64, aarch64 (ARM 64-bit),
//! > and i686 (32-bit x86) — each proven independently."
//!
//! aarch64 is the load-bearing proof of that clause (it is the ISA macOS is
//! welded to). Per the bring-up spec (`docs/research/aarch64-bringup-spec.md`
//! §Risks "Host-KAT-able before boot") and ADR 0009 §6, FOUR pieces of the
//! aarch64 backend are pure logic and can be proven on the host BEFORE any
//! aarch64 boot path exists — the same off-target methodology that root-caused
//! the ACPI 0->159-device bug entirely on the dev box. This crate is those four
//! pieces, and nothing else:
//!
//! 1. [`mmu`]  — VMSAv8-64 stage-1 page-table descriptor + `TCR_EL1`/`MAIR_EL1`
//!               encoder (4 KiB granule, 48-bit VA, TTBR0/TTBR1 split).
//! 2. [`esr`]  — `ESR_EL1` exception-syndrome decode (data/instruction abort,
//!               `SVC`, unknown).
//! 3. [`gic`]  — GICv2 distributor/CPU-interface register offset + bit math
//!               (`ISENABLER`/`IPRIORITYR`/`ITARGETSR`, `GICD_SGIR`, `IAR`/`EOIR`).
//! 4. [`dtb`]  — Flattened-DeviceTree walk (CPU count, RAM/UART/GIC bases) over
//!               the `fdt` crate, against a checked-in QEMU-virt fixture.
//!
//! ## Invariants
//! - `#![no_std]`, pure logic: NO aarch64 assembly, NO MMIO, NO kernel deps.
//!   Every function takes integers/slices and returns integers/values. This is
//!   what makes it host-testable AND what makes the eventual kernel import safe.
//! - Every encoding is grounded in a cited spec value (ARM Architecture
//!   Reference Manual ARMv8-A / GICv2 Architecture Specification / QEMU
//!   `hw/arm/virt.c`). Where the manual is ambiguous the code says so rather
//!   than inventing a bit.
//!
//! The crate is consumed by the kernel's future `arch/aarch64/{mmu,irq,firmware}.rs`
//! (spec slices A4/A5/A8); it is intentionally NOT wired into the kernel build
//! yet so it cannot regress the live x86_64 boot.

#![no_std]
#![forbid(unsafe_code)]

pub mod dtb;
pub mod esr;
pub mod gic;
pub mod mmu;
