//! Arch-neutral memory-address types (Slice 1, sub-slice 1a) — x86_64 backend.
//!
//! Concept §Architecture Reach: *"RaeenOS refuses ISA lock-in: the kernel sits
//! on a clean `arch::` abstraction layer (boot, MMU, interrupts, timers, SMP,
//! context switch, syscall entry, firmware discovery) so the same OS boots
//! x86_64, aarch64 (ARM 64-bit), and i686 (32-bit x86) — each proven
//! independently. … Portability is the anti-walled-garden property — you own
//! the machine, on the silicon you choose."*
//!
//! This module relocates a *type* (not a call): the address VALUE types that the
//! `x86_64` crate spells `PhysAddr` / `VirtAddr` / `PhysFrame` are x86-specific
//! and **do not exist on aarch64**. Until they live behind `arch::`, no aarch64
//! backend can supply its own 48-bit-VA / TTBR-based address representation
//! without forking the shared MM logic (ADR 0007 §4 gates aarch64 on this).
//!
//! ## Design (spec §2, Option (a) — transparent type-aliases)
//! On x86_64 these are zero-cost transparent aliases to the proven `x86_64`-crate
//! types, so every existing method (`::new()`, `.as_u64()`, `.as_mut_ptr()`,
//! `.align_up()`, `.align_down()`, `PhysFrame::start_address()`) works
//! byte-identically with **zero behavior change and zero codegen change** — the
//! alias compiles to the identical underlying type, no wrapper, no overhead.
//! Shared kernel code names `arch::{PhysAddr, VirtAddr, Frame}`; the future
//! aarch64 backend defines its OWN types of the same names behind the same seam.
//!
//! ## aarch64 counterpart notes (for the future backend — spec §5)
//! - [`VirtAddr`] on aarch64 = a **48-bit VA** newtype: bits `[63:48]` must be
//!   all-ones (TTBR1, kernel/higher half) or all-zeros (TTBR0, user/lower half).
//!   This canonical-form check differs from x86's bit-47 sign-extension; the
//!   alias hides it on x86, the aarch64 `VirtAddr::new` enforces the aarch64
//!   form. (The TTBR0/1 *split* is a paging-trait concern, Slice 1.5 — the
//!   address newtype only needs the high bits to know which half it is canonical
//!   for, so these types are aarch64-ready without naming TTBR.)
//! - [`PhysAddr`] on aarch64 = up to a 48-bit PA (IPA size from
//!   `ID_AA64MMFR0_EL1.PARange`); the identity round-trip uses the aarch64
//!   physical-memory offset.
//! - [`Frame`] on aarch64 = a 4 KiB-granule physical frame (same
//!   `PAGE_SIZE = 4096`); the `start_address()` alignment assertion ports
//!   unchanged.

use x86_64::structures::paging::{PhysFrame, Size4KiB};

/// Arch-neutral physical address. **x86_64:** transparent alias to
/// `x86_64::PhysAddr` (a validated 52-bit physical address) — zero-cost.
///
/// aarch64 backend (later): its own up-to-48-bit PA type (PARange-sized).
pub type PhysAddr = x86_64::PhysAddr;

/// Arch-neutral virtual address. **x86_64:** transparent alias to
/// `x86_64::VirtAddr` (a canonical, sign-extended 48-bit virtual address) —
/// zero-cost.
///
/// aarch64 backend (later): its own 48-bit VA newtype enforcing the aarch64
/// canonical form (bits `[63:48]` all-ones for TTBR1 / all-zeros for TTBR0).
pub type VirtAddr = x86_64::VirtAddr;

/// Arch-neutral physical frame — a page-aligned physical allocation unit.
/// **x86_64:** transparent alias to `x86_64::structures::paging::PhysFrame<Size4KiB>`
/// (a 4 KiB frame) — zero-cost.
///
/// `Frame` (not `PhysFrame`) drops the x86 "Phys" framing noun: a `Frame` is a
/// page-aligned physical allocation unit on every arch. The `Size4KiB` parameter
/// is pinned here so shared code names the simplest correct concrete handle (the
/// base 4 KiB granule, `PAGE_SIZE = 4096`, common to x86_64 and aarch64); larger
/// page-table granules are a paging-trait concern (Slice 1.5), not an
/// address-type one.
///
/// aarch64 backend (later): a 4 KiB-granule physical frame handle.
pub type Frame = PhysFrame<Size4KiB>;

/// The base page granule in bytes — the unit the align/frame checks below use.
/// Mirrors [`super::PAGE_SIZE`]; kept local so [`roundtrip_ok`] is a
/// self-contained, host-KAT-able pure-arithmetic predicate.
const PAGE_SIZE_U64: u64 = 4096;

/// Prove the arch-neutral address types behave correctly: an identity-map
/// phys→virt→phys round-trip, align-up/align-down monotonicity + alignment, and
/// frame start-address alignment. Returns `true` iff every check holds.
///
/// This is **pure address arithmetic behind no hardware** — it constructs a
/// synthetic physical-memory offset locally (the same `virt = phys + offset` /
/// `phys = virt - offset` identity map that `memory::phys_to_virt` /
/// `virt_to_phys` perform) rather than calling the kernel's MM helpers. That is
/// deliberate: the arch smoketest runs EARLY in `kernel_main` (right after
/// `arch::init`), BEFORE `memory::PHYS_MEM_OFFSET` is initialized — calling the
/// real helpers here would panic. Using a local offset keeps the predicate
/// FAIL-able and **host-KAT-able** (CLAUDE.md §15): the identical arithmetic runs
/// under `cargo test` on the dev box behind no hardware (see [`tests`]).
///
/// FAIL-able by construction: a wrong alias (an address type whose `as_u64()` /
/// `::new()` / `align_up` / `align_down` semantics differ, or a `Frame` whose
/// `start_address()` is not page-aligned) makes one of these checks return
/// `false` → the smoketest prints `addr=ROUNDTRIP-BAD -> FAIL`.
pub fn roundtrip_ok() -> bool {
    // A synthetic higher-half identity-map offset (page-aligned, canonical), in
    // the same spirit as the kernel's real `PHYS_MEM_OFFSET`. Using a constant
    // here makes this pure arithmetic with no dependency on boot state.
    const SYNTHETIC_OFFSET: u64 = 0xFFFF_8000_0000_0000;

    // 1. Identity-map round-trip: phys -> virt -> phys must recover the original.
    //    Exercises VirtAddr::new / .as_u64() and PhysAddr::new / .as_u64().
    let p: PhysAddr = PhysAddr::new(0x0000_0001_2345_6000);
    let v: VirtAddr = VirtAddr::new(p.as_u64() + SYNTHETIC_OFFSET);
    let recovered: PhysAddr = PhysAddr::new(v.as_u64() - SYNTHETIC_OFFSET);
    if recovered.as_u64() != p.as_u64() {
        return false;
    }

    // 2. Align math: for a deliberately non-page-aligned virtual address,
    //    align_down(PAGE) <= x <= align_up(PAGE), both results page-aligned, and
    //    the gap between them is exactly one page (since x is not aligned).
    let x: VirtAddr = VirtAddr::new(0xFFFF_8000_0001_2345);
    let down: VirtAddr = x.align_down(PAGE_SIZE_U64);
    let up: VirtAddr = x.align_up(PAGE_SIZE_U64);
    if down.as_u64() > x.as_u64() {
        return false;
    }
    if up.as_u64() < x.as_u64() {
        return false;
    }
    if down.as_u64() % PAGE_SIZE_U64 != 0 {
        return false;
    }
    if up.as_u64() % PAGE_SIZE_U64 != 0 {
        return false;
    }
    if up.as_u64() - down.as_u64() != PAGE_SIZE_U64 {
        return false;
    }

    // An already-aligned address must be a fixpoint of both align operations.
    let aligned: VirtAddr = VirtAddr::new(0xFFFF_8000_0001_2000);
    if aligned.align_down(PAGE_SIZE_U64).as_u64() != aligned.as_u64() {
        return false;
    }
    if aligned.align_up(PAGE_SIZE_U64).as_u64() != aligned.as_u64() {
        return false;
    }

    // 3. Frame alignment: a Frame's start_address() is always PAGE_SIZE-aligned,
    //    even when built from a non-aligned physical address (the constructor
    //    floors to the containing frame).
    let frame: Frame = Frame::containing_address(PhysAddr::new(0x0000_0000_0010_0FFF));
    let start = frame.start_address().as_u64();
    if start % PAGE_SIZE_U64 != 0 {
        return false;
    }
    if start != 0x0010_0000 {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Host KAT (CLAUDE.md §15): the round-trip predicate is pure address
    /// arithmetic behind no hardware, so it runs and proves itself under
    /// `cargo test` on the dev box — the cheapest real proof, before QEMU.
    #[test]
    fn roundtrip_ok_holds_on_host() {
        assert!(roundtrip_ok(), "arch-neutral address round-trip must hold");
    }

    #[test]
    fn frame_floors_to_page() {
        let f = Frame::containing_address(PhysAddr::new(0x100_0FFF));
        assert_eq!(f.start_address().as_u64(), 0x100_000);
    }
}
