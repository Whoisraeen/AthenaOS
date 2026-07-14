//! Arch-neutral memory-address types (Slice 1, sub-slice 1a) — aarch64 backend.
//!
//! Concept §Architecture Reach: *"RaeenOS refuses ISA lock-in: the kernel sits
//! on a clean `arch::` abstraction layer … so the same OS boots x86_64, aarch64
//! (ARM 64-bit), and i686 (32-bit x86) — each proven independently."*
//!
//! The `x86_64` crate's `PhysAddr`/`VirtAddr`/`PhysFrame` types are x86-specific
//! and DO NOT exist on aarch64. This module supplies the aarch64 realizations of
//! the SAME `arch::{PhysAddr, VirtAddr, Frame}` seam so shared MM logic never
//! names a per-ISA address type. These are REAL (not stubs): simple, `u64`-backed
//! newtypes implementing the exact method surface the shared kernel + the arch
//! smoketest call (`::new`, `.as_u64()`, `.as_mut_ptr()`, `.align_up()`,
//! `.align_down()`, `Frame::containing_address()`, `.start_address()`).
//!
//! ## aarch64 canonical VA form (the divergence from x86)
//! A 48-bit VA: bits `[63:48]` must be all-ones (TTBR1, kernel/higher half) or
//! all-zeros (TTBR0, user/lower half). This differs from x86's bit-47
//! sign-extension only cosmetically (both demand a canonical high region); the
//! newtype stores the raw `u64` and the *arithmetic* the shared code needs
//! (align/offset/round-trip) is canonical-form-agnostic, so A2 keeps it minimal
//! and correct. The TTBR0/1 *split* (which half a VA belongs to) is a paging
//! concern (A4), not an address-type one.

/// Arch-neutral physical address. **aarch64:** a `u64`-backed newtype holding an
/// up-to-48-bit PA (the QEMU-virt model; the real PARange comes from
/// `ID_AA64MMFR0_EL1` later). Zero-cost (`#[repr(transparent)]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct PhysAddr(u64);

impl PhysAddr {
    /// Construct from a raw physical address.
    #[inline]
    pub const fn new(addr: u64) -> Self {
        PhysAddr(addr)
    }

    /// The raw physical address as a `u64`.
    #[inline]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// Align DOWN to `align` (must be a power of two).
    #[inline]
    pub const fn align_down(self, align: u64) -> Self {
        PhysAddr(self.0 & !(align - 1))
    }

    /// Align UP to `align` (must be a power of two).
    #[inline]
    pub const fn align_up(self, align: u64) -> Self {
        PhysAddr((self.0 + align - 1) & !(align - 1))
    }
}

/// Arch-neutral virtual address. **aarch64:** a `u64`-backed newtype enforcing
/// nothing beyond storage in A2 (the 48-bit canonical-form check is an A4
/// concern; the shared arithmetic this round is canonical-agnostic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct VirtAddr(u64);

impl VirtAddr {
    /// Construct from a raw virtual address.
    #[inline]
    pub const fn new(addr: u64) -> Self {
        VirtAddr(addr)
    }

    /// The raw virtual address as a `u64`.
    #[inline]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// A mutable raw pointer to this virtual address (for the live map-write
    /// smoketest + physmap accessors). Mirrors `x86_64::VirtAddr::as_mut_ptr`.
    #[inline]
    pub const fn as_mut_ptr<T>(self) -> *mut T {
        self.0 as *mut T
    }

    /// A const raw pointer to this virtual address.
    #[inline]
    pub const fn as_ptr<T>(self) -> *const T {
        self.0 as *const T
    }

    /// Align DOWN to `align` (must be a power of two).
    #[inline]
    pub const fn align_down(self, align: u64) -> Self {
        VirtAddr(self.0 & !(align - 1))
    }

    /// Align UP to `align` (must be a power of two).
    #[inline]
    pub const fn align_up(self, align: u64) -> Self {
        VirtAddr((self.0 + align - 1) & !(align - 1))
    }
}

/// Arch-neutral physical frame — a 4 KiB-granule physical allocation unit.
/// **aarch64:** a page-aligned [`PhysAddr`] wrapper (matches `PAGE_SIZE = 4096`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Frame(PhysAddr);

impl Frame {
    /// The frame CONTAINING `addr` — floors `addr` to its 4 KiB frame base.
    #[inline]
    pub const fn containing_address(addr: PhysAddr) -> Self {
        Frame(addr.align_down(PAGE_SIZE_U64))
    }

    /// The page-aligned physical start address of this frame.
    #[inline]
    pub const fn start_address(self) -> PhysAddr {
        self.0
    }
}

/// The base page granule in bytes — local so [`roundtrip_ok`] is a self-contained
/// host-KAT-able pure-arithmetic predicate (mirrors [`super::PAGE_SIZE`]).
const PAGE_SIZE_U64: u64 = 4096;

/// Prove the aarch64 address types behave correctly: identity-map round-trip,
/// align-up/down monotonicity + alignment, and frame start-address alignment.
/// Identical assertions to the x86 backend's `roundtrip_ok` (same shared
/// smoketest), so a regression on aarch64 prints `addr=ROUNDTRIP-BAD -> FAIL`.
/// Pure arithmetic behind no hardware → host-KAT-able (see [`tests`]).
pub fn roundtrip_ok() -> bool {
    // A synthetic higher-half identity-map offset (the aarch64 TTBR1 kernel half
    // is the all-ones-high region — same shape as the x86 physmap offset).
    const SYNTHETIC_OFFSET: u64 = 0xFFFF_8000_0000_0000;

    // 1. Identity-map round-trip: phys -> virt -> phys must recover the original.
    let p = PhysAddr::new(0x0000_0001_2345_6000);
    let v = VirtAddr::new(p.as_u64() + SYNTHETIC_OFFSET);
    let recovered = PhysAddr::new(v.as_u64() - SYNTHETIC_OFFSET);
    if recovered.as_u64() != p.as_u64() {
        return false;
    }

    // 2. Align math on a deliberately non-aligned VA.
    let x = VirtAddr::new(0xFFFF_8000_0001_2345);
    let down = x.align_down(PAGE_SIZE_U64);
    let up = x.align_up(PAGE_SIZE_U64);
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

    // An already-aligned address is a fixpoint of both align operations.
    let aligned = VirtAddr::new(0xFFFF_8000_0001_2000);
    if aligned.align_down(PAGE_SIZE_U64).as_u64() != aligned.as_u64() {
        return false;
    }
    if aligned.align_up(PAGE_SIZE_U64).as_u64() != aligned.as_u64() {
        return false;
    }

    // 3. Frame floors to its containing 4 KiB frame.
    let frame = Frame::containing_address(PhysAddr::new(0x0000_0000_0010_0FFF));
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

    /// Host KAT (CLAUDE.md §15): pure address arithmetic, proven under
    /// `cargo test` on the dev box — the same predicate the aarch64 boot
    /// smoketest will assert.
    #[test]
    fn roundtrip_ok_holds_on_host() {
        assert!(roundtrip_ok(), "aarch64 address round-trip must hold");
    }

    #[test]
    fn frame_floors_to_page() {
        let f = Frame::containing_address(PhysAddr::new(0x100_0FFF));
        assert_eq!(f.start_address().as_u64(), 0x100_000);
    }
}
