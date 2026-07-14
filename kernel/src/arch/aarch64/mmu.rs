//! Arch-neutral PAGING seam (Slice 1.5 / A4) — aarch64 backend.
//!
//! Concept §Architecture Reach: *"AthenaOS refuses ISA lock-in: the kernel sits
//! on a clean `arch::` abstraction layer (boot, **MMU**, interrupts, timers, SMP,
//! context switch, syscall entry, firmware discovery) so the same OS boots
//! x86_64, aarch64 (ARM 64-bit), and i686 (32-bit x86) — each proven
//! independently."*
//!
//! The MMU is the load-bearing word in that clause. x86_64 has ONE translation
//! root (`CR3`); aarch64 has TWO — `TTBR1_EL1` (kernel/high half) + `TTBR0_EL1`
//! (user/low half). This module supplies the aarch64 realization of the SAME
//! `arch::mmu` surface the x86 backend exposes, so shared kernel code never names
//! a per-ISA page-table type.
//!
//! ## What is REAL vs. honest-unimplemented in this skeleton (Slice A2)
//! - REAL: [`PageProt`]/[`CacheType`]/[`PageFlags`] (the arch-neutral flag set,
//!   identical to the x86 backend's) AND their lowering to VMSAv8 leaf attributes
//!   via the host-KAT'd [`aarch64_logic::mmu`] encoder ([`PageFlags::to_aarch64`])
//!   — the W^X-critical AP/UXN/PXN mapping the kernel relies on, proven on the
//!   host. [`MmuError`], [`Root`], and the [`AddressSpace`] *handle* are real
//!   value types.
//! - HONEST-UNIMPLEMENTED (Slice A4): every [`AddressSpace`] *verb* (`map_page`/
//!   `translate`/`unmap_page`/`update_flags`/…) and the `kernel()`/`current_user()`
//!   /`new_user()` constructors — they need a LIVE aarch64 frame allocator + a
//!   programmed `TTBR1_EL1` kernel root + the actual VMSAv8 table walk, none of
//!   which exist until the A3/A4 boot path runs. These are FAIL-loud
//!   `unimplemented!` (a silent fake-success map would corrupt translation with
//!   no error). The descriptor *encoding* they will call is already real in
//!   [`aarch64_logic::mmu`].

use super::addr::{Frame, PhysAddr, VirtAddr};

// ── Arch-neutral root handle ────────────────────────────────────────────────

/// An opaque translation-table root. **aarch64:** the physical base address of an
/// L0 table page (tagged with its TTBR by the [`AddressSpace`] domain). Shared
/// code only stashes it in a task and hands it back to the seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct Root(PhysAddr);

impl Root {
    /// Wrap an L0-table physical base as a translation root.
    #[inline]
    pub const fn new(base: PhysAddr) -> Self {
        Root(base)
    }

    /// The L0-table physical base address (the value `TTBR{0,1}_EL1` wants, modulo
    /// the ASID bits the context-switch path adds at A9).
    #[inline]
    pub const fn start_address(self) -> PhysAddr {
        self.0
    }
}

// ── Cache policy ─────────────────────────────────────────────────────────────

/// Memory cache policy for a mapping. Arch-neutral; lowered per-ISA. On aarch64
/// these map to `MAIR_EL1` attribute indices ([`aarch64_logic::mmu::mair`]):
/// `WriteBack`→index 0 (Normal-WB), `Device`→index 1 (Device-nGnRnE).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheType {
    /// Normal write-back cacheable RAM (default for kernel/user data+code).
    WriteBack,
    /// Normal write-through cacheable.
    WriteThrough,
    /// Uncached normal memory.
    Uncached,
    /// Device/MMIO memory (Device-nGnRnE).
    Device,
}

bitflags::bitflags! {
    /// Arch-neutral page-protection flags — identical surface to the x86 backend.
    /// Lowered to VMSAv8 leaf attributes by [`PageFlags::to_aarch64`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PageProt: u8 {
        /// The mapping is present/valid. (aarch64 `VALID` + `AF`.)
        const PRESENT    = 1 << 0;
        /// Writes are permitted. (aarch64 `AP[2:1]` RW.)
        const WRITABLE   = 1 << 1;
        /// User (EL0) may access. (aarch64 `AP` EL0+EL1.)
        const USER       = 1 << 2;
        /// Instruction fetch is forbidden. (aarch64 `UXN`/`PXN`.)
        const NO_EXECUTE = 1 << 3;
        /// Global mapping (aarch64 `nG` cleared).
        const GLOBAL     = 1 << 4;
    }
}

/// Arch-neutral page-table flags: a [`PageProt`] bitset plus a [`CacheType`].
/// Identical to the x86 backend's `PageFlags`; lowered to aarch64 VMSAv8 leaf
/// attributes by [`PageFlags::to_aarch64`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageFlags {
    /// Protection bits (present/writable/user/NX/global).
    pub prot: PageProt,
    /// Cache policy for the mapping.
    pub cache: CacheType,
}

impl PageFlags {
    /// MAIR_EL1 attribute index for Normal write-back RAM (the canonical AthenaOS
    /// aarch64 layout: index 0 = Normal-WB — see [`aarch64_logic::mmu::mair`]).
    pub const MAIR_NORMAL_WB: u8 = 0;
    /// MAIR_EL1 attribute index for Device-nGnRnE MMIO (index 1).
    pub const MAIR_DEVICE: u8 = 1;

    /// A present, writable, non-executable, write-back **kernel data** mapping.
    pub const KERNEL_DATA: PageFlags = PageFlags {
        prot: PageProt::from_bits_truncate(
            PageProt::PRESENT.bits() | PageProt::WRITABLE.bits() | PageProt::NO_EXECUTE.bits(),
        ),
        cache: CacheType::WriteBack,
    };

    /// A present, writable, non-executable **device/MMIO** mapping (Device-nGnRnE).
    pub const DEVICE: PageFlags = PageFlags {
        prot: PageProt::from_bits_truncate(
            PageProt::PRESENT.bits() | PageProt::WRITABLE.bits() | PageProt::NO_EXECUTE.bits(),
        ),
        cache: CacheType::Device,
    };

    /// Build from a [`PageProt`] bitset with write-back caching.
    #[inline]
    pub const fn new(prot: PageProt) -> Self {
        PageFlags {
            prot,
            cache: CacheType::WriteBack,
        }
    }

    /// Replace the cache policy (builder style).
    #[inline]
    pub const fn with_cache(mut self, cache: CacheType) -> Self {
        self.cache = cache;
        self
    }

    /// Lower these arch-neutral flags to the VMSAv8 [`aarch64_logic::mmu::LeafAttrs`]
    /// the descriptor encoder consumes. PURE LOGIC — REAL this round, host-KAT'd in
    /// [`tests`] (the x86 backend's analogue is `to_x86`).
    ///
    /// | PageFlags                  | VMSAv8 leaf attribute                       |
    /// |----------------------------|---------------------------------------------|
    /// | `WRITABLE` + `USER`        | `AP = RwEl0El1`                             |
    /// | `WRITABLE`, !`USER`        | `AP = RwEl1`                               |
    /// | !`WRITABLE` + `USER`       | `AP = RoEl0El1`                            |
    /// | !`WRITABLE`, !`USER`       | `AP = RoEl1`                              |
    /// | `NO_EXECUTE`               | `UXN=1, PXN=1` (execute-never both ELs)    |
    /// | !`NO_EXECUTE`              | `UXN=0, PXN=0`                            |
    /// | !`GLOBAL`                  | `nG = 1` (per-ASID)                        |
    /// | `cache=WriteBack`/`WT`/`UC`| `attr_index = MAIR_NORMAL_WB` (0)          |
    /// | `cache=Device`            | `attr_index = MAIR_DEVICE` (1), non-shareable |
    pub fn to_aarch64(self) -> aarch64_logic::mmu::LeafAttrs {
        use aarch64_logic::mmu::{AccessPerm, LeafAttrs, Shareability};

        let user = self.prot.contains(PageProt::USER);
        let writable = self.prot.contains(PageProt::WRITABLE);
        let ap = match (writable, user) {
            (true, true) => AccessPerm::RwEl0El1,
            (true, false) => AccessPerm::RwEl1,
            (false, true) => AccessPerm::RoEl0El1,
            (false, false) => AccessPerm::RoEl1,
        };

        let nx = self.prot.contains(PageProt::NO_EXECUTE);
        let ng = !self.prot.contains(PageProt::GLOBAL);

        let (attr_index, sh) = match self.cache {
            // Device memory is non-shareable (the GIC/UART MMIO model).
            CacheType::Device => (Self::MAIR_DEVICE, Shareability::NonShareable),
            // Normal RAM is inner-shareable for SMP coherence.
            CacheType::WriteBack | CacheType::WriteThrough | CacheType::Uncached => {
                (Self::MAIR_NORMAL_WB, Shareability::InnerShareable)
            }
        };

        LeafAttrs {
            attr_index,
            ap,
            sh,
            af: true, // freshly-built maps set the Access Flag
            ng,
            pxn: nx,
            uxn: nx,
        }
    }
}

// ── Errors ──────────────────────────────────────────────────────────────────

/// A page-table operation failure. Arch-neutral; identical to the x86 backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmuError {
    /// The map operation could not install the entry.
    MapFailed,
    /// The requested virtual address was not mapped.
    NotMapped,
}

// ── The address space (aarch64 internals) ───────────────────────────────────

/// Which translation domain an [`AddressSpace`] addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Space {
    /// The always-resident kernel mappings (aarch64: `TTBR1_EL1`).
    Kernel,
    /// A per-task user mapping (aarch64: a `TTBR0_EL1` root).
    User,
}

/// An arch-neutral handle to one translation root. **aarch64:** wraps an L0
/// `Root` + its domain; the verbs drive the [`aarch64_logic::mmu`] descriptor
/// encoder into the resident kernel (`TTBR1`) or user (`TTBR0`) tables — Slice A4.
#[derive(Debug, Clone, Copy)]
pub struct AddressSpace {
    #[allow(dead_code)] // Read by the A4 verbs; stored now so the handle is real.
    root: Root,
    #[allow(dead_code)]
    kind: Space,
}

/// Handle to the kernel address space (`TTBR1_EL1` root).
///
/// aarch64 (A4): the kernel L0 root is programmed once at boot and never swapped
/// (the kernel is always resident in the high half). FAIL-loud until A4 — there
/// is no live kernel root to wrap yet.
#[inline]
pub fn kernel() -> AddressSpace {
    // MasterChecklist aarch64 Slice A4: TTBR1_EL1 kernel root (programmed in the
    // A4 MMU bring-up) — no live root to wrap until then.
    unimplemented!("aarch64 A4: kernel() needs the live TTBR1_EL1 root (MMU not up yet)")
}

/// Handle to the CURRENT task's user address space (`TTBR0_EL1` root).
///
/// aarch64 (A9): reads the active `TTBR0_EL1`. FAIL-loud until the context-switch
/// path programs TTBR0 (A9).
#[inline]
pub fn current_user() -> AddressSpace {
    // MasterChecklist aarch64 Slice A9: active TTBR0_EL1 read (no task TTBR0 yet).
    unimplemented!("aarch64 A9: current_user() needs the active TTBR0_EL1 (context switch not up)")
}

/// Wrap a NAMED translation root. The aarch64 handle is real; the verbs that act
/// on it are A4.
#[inline]
pub fn from_root(root: Root) -> AddressSpace {
    AddressSpace {
        root,
        kind: Space::User,
    }
}

/// Create a fresh USER address space and return its `TTBR0_EL1` [`Root`].
///
/// aarch64 (A4/A9): allocate a fresh EMPTY L0 table page (4 KiB, via
/// `allocate_contiguous_frames` — spec R6) and return its `PhysAddr` as the
/// `Root`. Unlike x86 it does NOT clone the kernel half — on aarch64 the kernel
/// lives in `TTBR1_EL1` (always resident), so a new user space is just an empty
/// `TTBR0` root. FAIL-loud until the live frame allocator exists (A4).
#[inline]
pub fn new_user() -> Root {
    // MasterChecklist aarch64 Slice A4: empty TTBR0 L0 page via the live frame
    // allocator (no kernel-half clone — kernel is TTBR1-resident).
    unimplemented!("aarch64 A4: new_user() needs the live frame allocator (no TTBR0 root yet)")
}

/// Flush this CPU's TLB entry for a single virtual address.
///
/// aarch64 (A4): `tlbi vae1is, <va>` + `dsb ish; isb`. FAIL-loud until A4 (issuing
/// `tlbi` before the MMU is up is meaningless).
#[inline]
pub fn flush(_v: VirtAddr) {
    // MasterChecklist aarch64 Slice A4: tlbi vae1is + dsb;isb.
    unimplemented!("aarch64 A4: per-page TLB flush (tlbi vae1is) not yet implemented")
}

/// Flush this CPU's entire TLB. aarch64 (A4): `tlbi vmalle1is` + `dsb;isb`.
#[inline]
pub fn flush_all() {
    // MasterChecklist aarch64 Slice A4: tlbi vmalle1is + dsb;isb.
    unimplemented!("aarch64 A4: full TLB flush (tlbi vmalle1is) not yet implemented")
}

impl AddressSpace {
    /// The translation root backing this space (for stashing in a task).
    #[inline]
    pub fn root(&self) -> Root {
        self.root
    }

    /// Map one 4 KiB page `v -> p` with `flags` into this space.
    ///
    /// aarch64 (A4): walk/allocate the L0..L3 tables for `v`, encode the L3 page
    /// descriptor via [`aarch64_logic::mmu::encode_leaf`] with
    /// [`PageFlags::to_aarch64`], write it, and flush. FAIL-loud until the live
    /// table walk + frame allocator exist (A4).
    pub fn map_page(
        &mut self,
        _v: VirtAddr,
        _p: PhysAddr,
        _flags: PageFlags,
    ) -> Result<(), MmuError> {
        // MasterChecklist aarch64 Slice A4: VMSAv8 table walk + encode_leaf write.
        unimplemented!("aarch64 A4: AddressSpace::map_page (VMSAv8 walk) not yet implemented")
    }

    /// Map a physically-contiguous range `[p, p+len)` to `[v, v+len)` page-by-page.
    ///
    /// aarch64 (A4): the same per-page loop over [`AddressSpace::map_page`].
    pub fn map_range(
        &mut self,
        _v: VirtAddr,
        _p: PhysAddr,
        _len: usize,
        _flags: PageFlags,
    ) -> Result<(), MmuError> {
        // MasterChecklist aarch64 Slice A4.
        unimplemented!("aarch64 A4: AddressSpace::map_range not yet implemented")
    }

    /// Translate a virtual address to its backing physical address, if mapped.
    ///
    /// aarch64 (A4): walk the L0..L3 tables of this space's root (or use `AT
    /// s1e1r` for the active space). FAIL-loud until A4.
    pub fn translate(&self, _v: VirtAddr) -> Option<PhysAddr> {
        // MasterChecklist aarch64 Slice A4: VMSAv8 table walk / AT s1e1r.
        unimplemented!("aarch64 A4: AddressSpace::translate (VMSAv8 walk) not yet implemented")
    }

    /// Map a device/MMIO window and return its virtual address.
    ///
    /// aarch64 (A4): a `TTBR1`-resident Device-nGnRnE range map.
    #[inline]
    pub fn map_mmio_range(&mut self, _p: PhysAddr, _len: usize, _flags: PageFlags) -> VirtAddr {
        // MasterChecklist aarch64 Slice A4: Device-nGnRnE MMIO range map.
        unimplemented!("aarch64 A4: AddressSpace::map_mmio_range not yet implemented")
    }

    /// Unmap one 4 KiB page and return the freed frame's physical address.
    ///
    /// aarch64 (A4): clear the L3 descriptor + `tlbi vae1is`. FAIL-loud until A4.
    pub fn unmap_page(&mut self, _v: VirtAddr) -> Result<PhysAddr, MmuError> {
        // MasterChecklist aarch64 Slice A4.
        unimplemented!("aarch64 A4: AddressSpace::unmap_page not yet implemented")
    }

    /// Change the protection flags of an already-mapped page (the `mprotect`
    /// verb — the W^X RW↔RX flip).
    ///
    /// aarch64 (A4): rewrite the L3 descriptor's `AP[2:1]`/`UXN`/`PXN` via the
    /// encoder, then `tlbi vae1is; dsb; isb`. FAIL-loud until A4.
    pub fn update_flags(&mut self, _v: VirtAddr, _flags: PageFlags) -> Result<(), MmuError> {
        // MasterChecklist aarch64 Slice A4: AP/UXN/PXN rewrite + flush.
        unimplemented!("aarch64 A4: AddressSpace::update_flags not yet implemented")
    }

    /// Map a physical DEVICE/MMIO range into the current-user space.
    ///
    /// aarch64 (A9): a `TTBR0`-resident Device-nGnRnE user range map.
    #[inline]
    pub fn map_phys_device_range(
        &mut self,
        _start_phys: PhysAddr,
        _length: usize,
        _user_virt: VirtAddr,
    ) -> Result<(), u64> {
        // MasterChecklist aarch64 Slice A9: TTBR0 device range map.
        unimplemented!("aarch64 A9: AddressSpace::map_phys_device_range not yet implemented")
    }

    /// Map a physically-contiguous NORMAL-RAM range into the current-user space.
    ///
    /// aarch64 (A9): a `TTBR0`-resident Normal-WB user range map.
    #[inline]
    pub fn map_phys_ram_range(
        &mut self,
        _start_phys: PhysAddr,
        _length: usize,
        _user_virt: VirtAddr,
    ) -> Result<(), u64> {
        // MasterChecklist aarch64 Slice A9: TTBR0 RAM range map.
        unimplemented!("aarch64 A9: AddressSpace::map_phys_ram_range not yet implemented")
    }
}

/// The opaque value the context-switch path loads to make a USER space active.
/// aarch64 (A9): the `TTBR0_EL1` value (L0 table base, ASID added at A9). The
/// token PRODUCTION is real (it just reads the root base); the switch asm is A9.
#[inline]
pub fn user_root_token(r: Root) -> u64 {
    r.start_address().as_u64()
}

// ── Host KAT (CLAUDE.md §15) — the REAL flag-lowering logic, FAIL-able ───────

#[cfg(test)]
mod tests {
    use super::*;
    use aarch64_logic::mmu::{AccessPerm, Shareability};

    fn flags(prot_bits: u8, cache: CacheType) -> PageFlags {
        PageFlags::new(PageProt::from_bits_truncate(prot_bits)).with_cache(cache)
    }

    /// Kernel RW data: AP=RwEl1, execute-never, Normal-WB inner-shareable.
    #[test]
    fn kernel_data_lowers_to_rw_el1_nx_wb() {
        let a = PageFlags::KERNEL_DATA.to_aarch64();
        assert_eq!(a.ap, AccessPerm::RwEl1, "kernel data is RW@EL1 only");
        assert!(a.uxn && a.pxn, "kernel data is execute-never");
        assert_eq!(a.attr_index, PageFlags::MAIR_NORMAL_WB);
        assert_eq!(a.sh, Shareability::InnerShareable);
        assert!(a.af, "freshly-built map sets AF");
    }

    /// User RW executable code: AP=RwEl0El1, NOT execute-never.
    #[test]
    fn user_rw_exec_lowers_to_rw_el0el1_exec() {
        let f = flags(
            PageProt::PRESENT.bits() | PageProt::WRITABLE.bits() | PageProt::USER.bits(),
            CacheType::WriteBack,
        );
        let a = f.to_aarch64();
        assert_eq!(a.ap, AccessPerm::RwEl0El1, "user RW is EL0+EL1");
        assert!(!a.uxn && !a.pxn, "executable: not execute-never");
        assert!(a.ng, "user mapping is per-ASID (nG set, GLOBAL clear)");
    }

    /// Read-only user: AP=RoEl0El1.
    #[test]
    fn ro_user_lowers_to_ro_el0el1() {
        let f = flags(
            PageProt::PRESENT.bits() | PageProt::USER.bits(),
            CacheType::WriteBack,
        );
        assert_eq!(f.to_aarch64().ap, AccessPerm::RoEl0El1);
    }

    /// Device MMIO: MAIR index 1, non-shareable (a dropped device index would
    /// make MMIO cacheable and silently corrupt device state).
    #[test]
    fn device_lowers_to_mair_device_nonshareable() {
        let a = PageFlags::DEVICE.to_aarch64();
        assert_eq!(a.attr_index, PageFlags::MAIR_DEVICE);
        assert_eq!(a.sh, Shareability::NonShareable);
        assert!(a.uxn && a.pxn, "MMIO is execute-never");
    }

    /// GLOBAL mapping clears nG (kernel global pages survive an ASID switch).
    #[test]
    fn global_clears_ng() {
        let f = flags(
            PageProt::PRESENT.bits() | PageProt::GLOBAL.bits(),
            CacheType::WriteBack,
        );
        assert!(!f.to_aarch64().ng, "GLOBAL mapping => nG clear");
    }

    /// FAIL-demo: kernel data must NOT be user-accessible — a security-relevant
    /// mis-lowering would flip AP to an EL0-readable value.
    #[test]
    fn kernel_data_is_not_el0_accessible() {
        let ap = PageFlags::KERNEL_DATA.to_aarch64().ap;
        assert!(
            ap == AccessPerm::RwEl1 || ap == AccessPerm::RoEl1,
            "kernel data AP must be EL1-only, got {:?}",
            ap
        );
    }

    /// The user-root token is the L0 table phys base (what `TTBR0_EL1` wants).
    #[test]
    fn user_root_token_is_l0_phys_base() {
        let r = Root::new(PhysAddr::new(0x4007_F000));
        assert_eq!(user_root_token(r), 0x4007_F000);
    }
}
