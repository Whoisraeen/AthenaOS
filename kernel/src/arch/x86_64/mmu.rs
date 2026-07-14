//! Arch-neutral PAGING seam (Slice 1.5, sub-slice 1.5a) — x86_64 backend.
//!
//! Concept §Architecture Reach: *"AthenaOS refuses ISA lock-in: the kernel sits
//! on a clean `arch::` abstraction layer (boot, **MMU**, interrupts, timers, SMP,
//! context switch, syscall entry, firmware discovery) so the same OS boots
//! x86_64, aarch64 (ARM 64-bit), and i686 (32-bit x86) — each proven
//! independently. … Portability is the anti-walled-garden property — you own the
//! machine, on the silicon you choose."*
//!
//! The MMU is the load-bearing word in that clause. x86_64 has ONE translation
//! root (`CR3`) and maps the kernel into the higher half of every address space;
//! aarch64 has TWO (`TTBR1_EL1` kernel + `TTBR0_EL1` user). The page-table
//! *mechanism* — `OffsetPageTable`/`Mapper`/`Cr3` on x86 vs raw VMSAv8 descriptor
//! writes on aarch64 — has no common crate; it MUST be hidden behind this
//! hand-written seam so shared kernel code never names a per-ISA page-table type.
//!
//! ## Scope (sub-slice 1.5a — a DELEGATING wrapper, ZERO behavior change)
//! This module DEFINES the seam: the arch-neutral [`PageFlags`] flag set + its
//! lowering to x86 `PageTableFlags`, an [`MmuError`], and a concrete
//! [`AddressSpace`] whose inherent verbs (`map_page` / `translate` /
//! `unmap_page` / `flush`) each FORWARD to the EXISTING `crate::memory::*`
//! function (`kernel()` wraps `KERNEL_PML4`; `current_user()` wraps the active
//! CR3; `map_page` → `crate::memory::map_page_in_pml4_fallible`; `translate` →
//! `crate::memory::kernel_translate_addr` / active translate; `unmap_page` →
//! `crate::memory::kernel_page_table().unmap`). **No `memory.rs` body is
//! rewritten this round** — the x86 paging core is untouched and behaves
//! byte-identically; only a new name is introduced in front of it. Migrating the
//! 26-file / ~319-site Cluster-B call sites onto this seam, and (last)
//! reimplementing these bodies to stop delegating, are sub-slices 1.5b–1.5h
//! (docs/research/slice1_5-arch-paging-trait.md §4).
//!
//! ## aarch64 counterpart notes (for the future A4 backend — spec §3)
//! The same [`AddressSpace`] surface, backed by `components/aarch64_logic`'s
//! host-KAT'd VMSAv8 descriptor encoder instead of `OffsetPageTable`:
//! - [`AddressSpace::kernel`] wraps the `TTBR1_EL1` root (programmed once at boot,
//!   never swapped — the kernel is always resident, no per-space cloning).
//! - [`AddressSpace::current_user`] wraps the `TTBR0_EL1` root (swapped per task).
//! - [`PageFlags`] lowers to the encoder's `LeafAttrs` (the §1 table's aarch64
//!   column: `WRITABLE`/`USER` → `ap`, `NO_EXECUTE` → `uxn`/`pxn`,
//!   [`CacheType`] → MAIR attr-index + shareability) instead of `PageTableFlags`.
//! - `flush(v)` lowers to `tlbi vae1is` + `dsb;isb` instead of `invlpg`.
//! The descriptor encoder + `TCR_EL1`/`MAIR_EL1` builders already exist
//! (`aarch64_logic::mmu`, commit a4c9f5b); A4 supplies only the glue.

use x86_64::structures::paging::{Mapper, Page, PageTableFlags, PhysFrame, Size4KiB, Translate};

// ── Arch-neutral root handle ────────────────────────────────────────────────

/// An opaque translation-table root.
///
/// **x86_64:** the `PhysFrame` of a PML4. **aarch64 (later):** the `PhysAddr` of
/// an L0 table page, tagged with its TTBR. Shared code only stashes it in a task
/// and hands it back to the seam; it never inspects the inside.
pub type Root = PhysFrame<Size4KiB>;

// ── Cache policy (an enum, not a bit: device + normal modes don't fit 1 bit) ─

/// Memory cache policy for a mapping. Arch-neutral; lowered per-ISA.
///
/// x86 maps `WriteBack` to "no PCD/PWT" and `Device`/`Uncached` to PCD
/// (`NO_CACHE`); aarch64 maps these to MAIR attr-indices + shareability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheType {
    /// Normal write-back cacheable RAM (the default for kernel/user data+code).
    WriteBack,
    /// Normal write-through cacheable.
    WriteThrough,
    /// Uncached normal memory.
    Uncached,
    /// Device/MMIO memory (strongly ordered, non-cacheable).
    Device,
}

bitflags::bitflags! {
    /// Arch-neutral page-protection flags. The single flag set the kernel uses
    /// everywhere, lowered per-arch by [`PageFlags::to_x86`] (x86) / the aarch64
    /// `LeafAttrs` encoder (later). The cache policy is carried separately as a
    /// [`CacheType`] field of [`PageFlags`] (see [`PageFlags::cache`]).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct PageProt: u8 {
        /// The mapping is present/valid. (x86 `PRESENT`; aarch64 `VALID`+`AF`.)
        const PRESENT    = 1 << 0;
        /// Writes are permitted. (x86 `WRITABLE`; aarch64 `AP[2:1]` RW.)
        const WRITABLE   = 1 << 1;
        /// User (EL0) may access. (x86 `USER_ACCESSIBLE`; aarch64 `AP` EL0+EL1.)
        const USER       = 1 << 2;
        /// Instruction fetch is forbidden. (x86 `NO_EXECUTE`; aarch64 `UXN`/`PXN`.)
        const NO_EXECUTE = 1 << 3;
        /// Global mapping (not flushed on address-space switch). (x86 `GLOBAL`;
        /// aarch64 `nG` cleared.)
        const GLOBAL     = 1 << 4;
    }
}

/// Arch-neutral page-table flags: a [`PageProt`] bitset plus a [`CacheType`].
///
/// Constructed by kernel code (and by syscall handlers from raw `prot` bits) and
/// lowered to the per-ISA leaf flags by [`PageFlags::to_x86`]. The split keeps
/// the cache policy (which needs >1 bit) out of the protection bitflags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageFlags {
    /// Protection bits (present/writable/user/NX/global).
    pub prot: PageProt,
    /// Cache policy for the mapping.
    pub cache: CacheType,
}

impl PageFlags {
    /// A present, writable, non-executable, write-back **kernel data** mapping —
    /// the common default for kernel RAM (e.g. stacks, scratch).
    pub const KERNEL_DATA: PageFlags = PageFlags {
        prot: PageProt::from_bits_truncate(
            PageProt::PRESENT.bits() | PageProt::WRITABLE.bits() | PageProt::NO_EXECUTE.bits(),
        ),
        cache: CacheType::WriteBack,
    };

    /// A present, writable, non-executable **device/MMIO** mapping (uncached).
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

    /// Lower these arch-neutral flags to x86 `PageTableFlags` (the §1 table's x86
    /// column). PURE LOGIC — host-KAT'd in [`tests`].
    ///
    /// | PageFlags                         | x86 `PageTableFlags`             |
    /// |-----------------------------------|----------------------------------|
    /// | `PRESENT`                         | `PRESENT`                        |
    /// | `WRITABLE`                        | `WRITABLE`                       |
    /// | `USER`                            | `USER_ACCESSIBLE`                |
    /// | `NO_EXECUTE`                      | `NO_EXECUTE`                     |
    /// | `GLOBAL`                          | `GLOBAL`                         |
    /// | `cache=Device`/`Uncached`         | `NO_CACHE` (PCD)                 |
    /// | `cache=WriteThrough`              | `WRITE_THROUGH` (PWT)            |
    /// | `cache=WriteBack`                 | (no PCD/PWT)                     |
    #[inline]
    pub fn to_x86(self) -> PageTableFlags {
        let mut f = PageTableFlags::empty();
        if self.prot.contains(PageProt::PRESENT) {
            f |= PageTableFlags::PRESENT;
        }
        if self.prot.contains(PageProt::WRITABLE) {
            f |= PageTableFlags::WRITABLE;
        }
        if self.prot.contains(PageProt::USER) {
            f |= PageTableFlags::USER_ACCESSIBLE;
        }
        if self.prot.contains(PageProt::NO_EXECUTE) {
            f |= PageTableFlags::NO_EXECUTE;
        }
        if self.prot.contains(PageProt::GLOBAL) {
            f |= PageTableFlags::GLOBAL;
        }
        match self.cache {
            CacheType::WriteBack => {}
            CacheType::WriteThrough => f |= PageTableFlags::WRITE_THROUGH,
            CacheType::Uncached | CacheType::Device => f |= PageTableFlags::NO_CACHE,
        }
        f
    }
}

// ── Errors ──────────────────────────────────────────────────────────────────

/// A page-table operation failure. Arch-neutral; the x86 backend maps its
/// `MapToError`/`UnmapError` cases onto these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmuError {
    /// The map operation could not install the entry (frame-alloc failure,
    /// unrecoverable already-mapped, or huge-page conflict).
    MapFailed,
    /// The requested virtual address was not mapped (unmap/translate of a hole).
    NotMapped,
}

// ── The address space (x86_64 internals; cfg-selected) ──────────────────────

/// Which translation domain an [`AddressSpace`] addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Space {
    /// The always-resident kernel mappings (x86: `KERNEL_PML4`; aarch64: TTBR1).
    Kernel,
    /// A per-task user mapping (x86: a user PML4 root; aarch64: a TTBR0 root).
    User,
}

/// An arch-neutral handle to one translation root.
///
/// **x86_64:** wraps a PML4 `PhysFrame` plus which domain it is. Every method
/// monomorphizes to the underlying `OffsetPageTable` primitive with zero
/// indirection. **aarch64 (later):** wraps an L0 `PhysAddr` + TTBR tag and drives
/// the `aarch64_logic` descriptor encoder behind the identical method surface.
///
/// 1.5a: each verb DELEGATES to the existing `crate::memory::*` function (no
/// reimplemented paging).
#[derive(Debug, Clone, Copy)]
pub struct AddressSpace {
    root: Root,
    kind: Space,
}

/// Handle to the kernel address space — the always-resident kernel mappings.
///
/// x86: backed by `KERNEL_PML4` (so `translate` is safe while a user CR3 is
/// active — the §10.2 keystone). aarch64: backed by the `TTBR1_EL1` root.
/// Mapping into this space is visible in EVERY address space (x86: the kernel
/// half is cloned into every PML4; aarch64: TTBR1 is shared by construction).
///
/// Delegates to `crate::memory::KERNEL_PML4` / `crate::memory::kernel_*`.
#[inline]
pub fn kernel() -> AddressSpace {
    let root = *crate::memory::KERNEL_PML4
        .get()
        .expect("KERNEL_PML4 not initialized");
    AddressSpace {
        root,
        kind: Space::Kernel,
    }
}

/// Handle to the CURRENT task's user address space.
///
/// x86: backed by the active `CR3`. aarch64: the active `TTBR0_EL1` root. Use
/// this ONLY to translate/map USER virtual addresses — kernel-VA translation
/// must go through [`kernel`] (R1: active-CR3 vs `KERNEL_PML4` confusion).
#[inline]
pub fn current_user() -> AddressSpace {
    use x86_64::registers::control::Cr3;
    let (frame, _) = Cr3::read();
    AddressSpace {
        root: frame,
        kind: Space::User,
    }
}

/// Wrap a NAMED translation root (e.g. the `map_page_in_pml4` call sites that map
/// into a specific, non-current PML4). Treated as a user-domain space.
#[inline]
pub fn from_root(root: Root) -> AddressSpace {
    AddressSpace {
        root,
        kind: Space::User,
    }
}

/// Create a fresh USER address space and return its translation [`Root`].
///
/// **x86_64 (this backend):** DELEGATES verbatim to
/// `crate::memory::create_new_pml4()` — allocate a 4 KiB PML4, zero it, and
/// **clone the kernel higher half** into it (so kernel VAs resolve under this
/// CR3) plus the deep-copy of the user-low PD chain (the §10.3 / spec-R5
/// frame-collision fix — child base-0 text must not REPLACE the running
/// parent's pages). 1.5f does NOT reimplement any of that: the body is the
/// existing `create_new_pml4`, untouched, and the returned `Root` is the same
/// PML4 `PhysFrame` callers stash in `Task.pml4` and later load into `CR3`.
/// The result is `Space::User` by construction (it is a per-task user space).
///
/// **§10.3 ordering is the caller's contract, NOT this fn's:** the per-task
/// kernel stack MUST be allocated (`crate::memory::alloc_kernel_stack`) BEFORE
/// this call so the kernel-half clone captures the stack mapping. The migrated
/// call sites preserve that order exactly (they only renamed the call); this
/// seam does not move *when* `new_user()` runs relative to the stack alloc.
///
/// **aarch64 (later, A4):** allocates a fresh, EMPTY L0 table page (4 KiB, via
/// `allocate_contiguous_frames` — spec R6) and returns its `PhysAddr` as the
/// `Root`. It does **NOT** clone the kernel half: on aarch64 the kernel lives in
/// `TTBR1_EL1` (programmed once at boot, always resident), so a new user space is
/// just an empty `TTBR0_EL1` root. This is the core x86-vs-aarch64 divergence
/// (spec §3) — hidden entirely inside this fn so shared spawn code never knows
/// whether the kernel half is cloned (x86) or ambient (aarch64).
#[inline]
pub fn new_user() -> Root {
    crate::memory::create_new_pml4()
}

/// Flush this CPU's TLB entry for a single virtual address.
///
/// x86: `invlpg`. aarch64 (later): `tlbi vae1is` + `dsb;isb`. Explicit (not
/// implicit in map/unmap) so a missing flush is a visible, FAIL-able omission
/// (spec R3).
#[inline]
pub fn flush(v: crate::arch::VirtAddr) {
    x86_64::instructions::tlb::flush(v);
}

/// Flush this CPU's entire TLB.
///
/// x86: reload-style `flush_all`. aarch64 (later): `tlbi vmalle1is`.
#[inline]
pub fn flush_all() {
    x86_64::instructions::tlb::flush_all();
}

impl AddressSpace {
    /// The translation root backing this space (for stashing in `Task.pml4`).
    #[inline]
    pub fn root(&self) -> Root {
        self.root
    }

    /// Map one 4 KiB page `v -> p` with `flags` into this space.
    ///
    /// DELEGATES to `crate::memory::map_page_in_pml4_fallible` (the proven x86
    /// map-with-stale-entry-recovery path) — zero behavior change. The arch-
    /// neutral [`PageFlags`] are lowered via [`PageFlags::to_x86`].
    pub fn map_page(
        &mut self,
        v: crate::arch::VirtAddr,
        p: crate::arch::PhysAddr,
        flags: PageFlags,
    ) -> Result<(), MmuError> {
        let page = Page::<Size4KiB>::containing_address(v);
        let frame = PhysFrame::<Size4KiB>::containing_address(p);
        let ok = unsafe {
            crate::memory::map_page_in_pml4_fallible(self.root, page, frame, flags.to_x86())
        };
        if ok {
            Ok(())
        } else {
            Err(MmuError::MapFailed)
        }
    }

    /// Map a PHYSICALLY-CONTIGUOUS range `[p, p+len)` to the virtual range
    /// `[v, v+len)` (page-by-page, 4 KiB granularity) with `flags` into this
    /// space. `len` is rounded UP to a whole number of 4 KiB pages.
    ///
    /// 1.5e: DELEGATES per page to [`AddressSpace::map_page`] (which forwards to
    /// `crate::memory::map_page_in_pml4_fallible`) — zero new paging mechanism;
    /// byte-identical to a manual `map_page` loop over the contiguous frames. On
    /// the FIRST page that fails to map this returns [`MmuError::MapFailed`]
    /// (it does NOT roll back the pages already mapped — the caller owns failure
    /// policy, e.g. the kernel-stack path treats a stack-map failure as fatal).
    ///
    /// NOTE: this maps a CONTIGUOUS physical span. The per-task **kernel-stack**
    /// path ([`crate::memory::alloc_kernel_stack`]) allocates its frames
    /// independently (NON-contiguous, one `allocate_frame` per page) and so maps
    /// page-by-page via [`AddressSpace::map_page`] with the frame it just
    /// allocated — it does NOT route through `map_range`. `map_range` is for
    /// callers that already hold a contiguous physical region
    /// (`allocate_contiguous_frames(order)`; spec R6/§10.7).
    ///
    /// aarch64 (later): the same loop drives the `aarch64_logic` descriptor
    /// encoder into the `TTBR1`-resident kernel L0..L3 tables (kernel domain) or
    /// the `TTBR0`-resident user tables (user domain) behind the identical
    /// surface — the contiguous-range kernel map A4's MMU bring-up needs.
    pub fn map_range(
        &mut self,
        v: crate::arch::VirtAddr,
        p: crate::arch::PhysAddr,
        len: usize,
        flags: PageFlags,
    ) -> Result<(), MmuError> {
        let len = (len as u64 + 4095) & !4095;
        let mut off: u64 = 0;
        while off < len {
            self.map_page(
                crate::arch::VirtAddr::new(v.as_u64() + off),
                crate::arch::PhysAddr::new(p.as_u64() + off),
                flags,
            )?;
            off += 4096;
        }
        Ok(())
    }

    /// Translate a virtual address to its backing physical address, if mapped.
    ///
    /// For a kernel-domain space, DELEGATES to
    /// `crate::memory::kernel_translate_addr` (reads `KERNEL_PML4` directly — safe
    /// while a user CR3 is active, the §10.2 keystone). For a user-domain space
    /// whose root is the active CR3, uses the active page table; otherwise walks
    /// the named root via the physmap.
    pub fn translate(&self, v: crate::arch::VirtAddr) -> Option<crate::arch::PhysAddr> {
        match self.kind {
            Space::Kernel => crate::memory::kernel_translate_addr(v),
            Space::User => {
                use x86_64::registers::control::Cr3;
                let (active, _) = Cr3::read();
                if active == self.root {
                    crate::memory::active_page_table().translate_addr(v)
                } else {
                    crate::memory::pml4_page_frame(
                        self.root,
                        Page::<Size4KiB>::containing_address(v),
                    )
                    .map(|f| {
                        let off = v.as_u64() & 0xFFF;
                        crate::arch::PhysAddr::new(f.start_address().as_u64() + off)
                    })
                }
            }
        }
    }

    /// Map a device/MMIO window `[p, p+len)` into the (kernel) address space and
    /// return the physmap-consistent virtual address for `p` (sub-page offset
    /// preserved, so callers using `offset + bar` still match).
    ///
    /// 1.5b: DELEGATES to `crate::memory::map_mmio_region` — the proven leaf-BAR
    /// mapping path that creates `PRESENT|WRITABLE|NO_CACHE|NO_EXECUTE` PTEs and
    /// re-applies WRITABLE on already-physmapped low BARs — so behavior is
    /// byte-identical to a raw `map_mmio_region` call. `flags` MUST carry a
    /// device/uncached cache policy ([`PageFlags::DEVICE`] / `CacheType::Device` /
    /// `CacheType::Uncached`); the x86 `map_mmio_region` always maps uncached, and
    /// asserting here keeps any future cacheable misuse from silently corrupting
    /// device state (spec R7). Mapping always lands in the always-resident kernel
    /// mappings on x86 (the active page table during boot bring-up == kernel), so
    /// this is only meaningful on a kernel-domain [`AddressSpace`].
    ///
    /// aarch64 (later): the same delegation point lowers to a `TTBR1`-resident
    /// Device-nGnRnE range map via the `aarch64_logic` encoder.
    #[inline]
    pub fn map_mmio_range(
        &mut self,
        p: crate::arch::PhysAddr,
        len: usize,
        flags: PageFlags,
    ) -> crate::arch::VirtAddr {
        debug_assert!(
            matches!(flags.cache, CacheType::Device | CacheType::Uncached),
            "map_mmio_range requires a device/uncached cache policy"
        );
        debug_assert!(
            self.kind == Space::Kernel,
            "map_mmio_range maps the kernel-resident MMIO window"
        );
        crate::memory::map_mmio_region(p.as_u64(), len)
    }

    /// Unmap one 4 KiB page from this space and return the freed frame's physical
    /// address. Flushes the TLB for the page (spec R3).
    ///
    /// DELEGATES to the existing `crate::memory::kernel_page_table()` mapper +
    /// `Mapper::unmap` (the same path `crate::memory::free_kernel_stack` uses) for
    /// the kernel domain. The page's frame is NOT freed here (the caller owns the
    /// frame's lifetime — the smoketest frees it explicitly).
    pub fn unmap_page(
        &mut self,
        v: crate::arch::VirtAddr,
    ) -> Result<crate::arch::PhysAddr, MmuError> {
        let page = Page::<Size4KiB>::containing_address(v);
        let _guard = crate::memory::PAGE_TABLE_LOCK.lock();
        let mut mapper = match self.kind {
            Space::Kernel => crate::memory::kernel_page_table(),
            Space::User => crate::memory::active_page_table(),
        };
        match mapper.unmap(page) {
            Ok((frame, tlb)) => {
                tlb.flush();
                Ok(crate::arch::PhysAddr::new(frame.start_address().as_u64()))
            }
            Err(_) => Err(MmuError::NotMapped),
        }
    }

    /// Change the protection flags of an ALREADY-MAPPED 4 KiB page `v` in this
    /// space to `flags`, WITHOUT remapping (the page keeps its backing frame).
    /// This is the `mprotect` verb — the RW↔RX W^X flip (CLAUDE.md §10): a
    /// relocated `.text` mapping is created RW+NX, then flipped to RX (WRITABLE
    /// cleared, NO_EXECUTE cleared) once the loader is done patching it.
    ///
    /// 1.5d: implemented over the same `OffsetPageTable::update_flags` machinery
    /// `crate::memory::sys_mprotect` uses per page (the proven flag-flip path) —
    /// behavior-preserving, the `memory.rs` body is untouched. The arch-neutral
    /// [`PageFlags`] are lowered to the exact x86 `PageTableFlags` via
    /// [`PageFlags::to_x86`], so the W^X-critical WRITABLE / NO_EXECUTE bits are
    /// preserved bit-for-bit (a dropped NX or a stray WRITABLE here is a security
    /// hole — spec R7). The page's frame is NOT touched; only the leaf PTE flags.
    ///
    /// **TLB flush is mandatory** (spec R3): a missed flush leaves the OLD
    /// protection live and is a silent W^X bypass, so this flushes the page
    /// unconditionally on success.
    ///
    /// Returns [`MmuError::NotMapped`] if `v` is not mapped in this space (a hole
    /// is an error — this is `mprotect`, not `mmap`).
    ///
    /// aarch64 (later): the same op rewrites the leaf descriptor's `AP[2:1]`
    /// (R/W) + `UXN`/`PXN` (execute-never) fields via the `aarch64_logic`
    /// encoder, then `tlbi vae1is; dsb; isb` — the W^X flip expressed in VMSAv8
    /// permission bits instead of x86 PTE flags.
    pub fn update_flags(
        &mut self,
        v: crate::arch::VirtAddr,
        flags: PageFlags,
    ) -> Result<(), MmuError> {
        let page = Page::<Size4KiB>::containing_address(v);
        let _guard = crate::memory::PAGE_TABLE_LOCK.lock();
        let mut mapper = match self.kind {
            Space::Kernel => crate::memory::kernel_page_table(),
            Space::User => crate::memory::active_page_table(),
        };
        match unsafe { mapper.update_flags(page, flags.to_x86()) } {
            Ok(tlb) => {
                tlb.flush();
                Ok(())
            }
            Err(_) => Err(MmuError::NotMapped),
        }
    }

    /// Map a physical DEVICE/MMIO range `[start_phys, start_phys+length)` to the
    /// user virtual range `[user_virt, user_virt+length)` in this (current-user)
    /// address space, uncached (UC + write-through), user-accessible.
    ///
    /// 1.5c (named-root / per-task tier): DELEGATES to
    /// `crate::memory::map_phys_mmio_into_current_task`, which targets the CURRENT
    /// task's stashed PML4 (`Task.pml4`, == the active CR3 for the running task —
    /// the ACTIVE/user address space, NOT the kernel one; CLAUDE.md §10.2). So this
    /// is only meaningful on a [`current_user`]-domain [`AddressSpace`], asserted
    /// below. Behavior is byte-identical to the raw call (same page loop, same
    /// `PRESENT|WRITABLE|USER|NO_CACHE|WRITE_THROUGH` PTE flags, same single
    /// `flush_all` at the end).
    ///
    /// FLAG-SEMANTICS NOTE: the underlying flags are `NO_CACHE | WRITE_THROUGH`
    /// (PCD+PWT), which [`PageFlags`]/[`CacheType::Device`] cannot yet express
    /// (it lowers to PCD only). We therefore delegate to the helper rather than
    /// reconstruct flags from [`PageFlags`], preserving the exact UC+PWT lowering;
    /// the `CacheType` widening to a PCD+PWT mode is a later (1.5h) refinement.
    ///
    /// aarch64 (later): the same delegation point lowers to a `TTBR0`-resident
    /// Device-nGnRnE user range map via the `aarch64_logic` encoder.
    #[inline]
    pub fn map_phys_device_range(
        &mut self,
        start_phys: crate::arch::PhysAddr,
        length: usize,
        user_virt: crate::arch::VirtAddr,
    ) -> Result<(), u64> {
        debug_assert!(
            self.kind == Space::User,
            "map_phys_device_range maps the CURRENT task's user PML4 (active CR3), not kernel()"
        );
        crate::memory::map_phys_mmio_into_current_task(
            start_phys.as_u64(),
            length,
            user_virt.as_u64(),
        )
    }

    /// Map a physically-contiguous NORMAL-RAM range `[start_phys, start_phys+length)`
    /// to the user virtual range `[user_virt, user_virt+length)` in this
    /// (current-user) address space, WRITE-BACK cached, user-accessible.
    ///
    /// 1.5c (named-root / per-task tier): DELEGATES to
    /// `crate::memory::map_phys_ram_into_current_task` (the firmware-blob path).
    /// Like its device sibling it targets the CURRENT task's stashed PML4
    /// (`Task.pml4`, == the active CR3 for the running task — the ACTIVE/user
    /// address space, NOT the kernel one; CLAUDE.md §10.2). Byte-identical to the
    /// raw call (same `PRESENT|WRITABLE|USER` WB PTE flags, same page loop, same
    /// final `flush_all`).
    ///
    /// WB (not UC) is load-bearing: the daemon's view must stay cache-coherent with
    /// the kernel's WB physmap fill of the same frames (the amdgpu iron firmware-read
    /// hang; memory `amdgpu-iron-hang-uc-firmware-read`).
    #[inline]
    pub fn map_phys_ram_range(
        &mut self,
        start_phys: crate::arch::PhysAddr,
        length: usize,
        user_virt: crate::arch::VirtAddr,
    ) -> Result<(), u64> {
        debug_assert!(
            self.kind == Space::User,
            "map_phys_ram_range maps the CURRENT task's user PML4 (active CR3), not kernel()"
        );
        crate::memory::map_phys_ram_into_current_task(
            start_phys.as_u64(),
            length,
            user_virt.as_u64(),
        )
    }
}

/// The opaque value the context-switch path loads to make a USER space active.
///
/// x86: the PML4 phys frame address (→ `CR3`). aarch64 (later): the `TTBR0_EL1`
/// value (table base | ASID). Shared scheduler/context code reads this token and
/// hands it to `arch::context::switch` WITHOUT naming CR3 or TTBR. (The switch asm
/// itself stays per-arch and is NOT migrated in 1.5a — this fn only produces the
/// token; see spec §3 + sub-slice 1.5g.)
#[inline]
pub fn user_root_token(r: Root) -> u64 {
    r.start_address().as_u64()
}

// ── Host KAT (CLAUDE.md §15) — pure flag-lowering logic, FAIL-able ──────────

#[cfg(test)]
mod tests {
    use super::*;

    fn prot(bits: u8) -> PageFlags {
        PageFlags::new(PageProt::from_bits_truncate(bits))
    }

    /// Each §1 x86-column row asserted (the FAIL-able pure-logic proof).
    #[test]
    fn to_x86_lowers_each_flag() {
        assert_eq!(
            prot(PageProt::PRESENT.bits()).to_x86(),
            PageTableFlags::PRESENT
        );
        assert_eq!(
            prot(PageProt::PRESENT.bits() | PageProt::WRITABLE.bits()).to_x86(),
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE
        );
        assert_eq!(
            prot(PageProt::PRESENT.bits() | PageProt::USER.bits()).to_x86(),
            PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE
        );
        assert_eq!(
            prot(PageProt::PRESENT.bits() | PageProt::NO_EXECUTE.bits()).to_x86(),
            PageTableFlags::PRESENT | PageTableFlags::NO_EXECUTE
        );
        assert_eq!(
            prot(PageProt::PRESENT.bits() | PageProt::GLOBAL.bits()).to_x86(),
            PageTableFlags::PRESENT | PageTableFlags::GLOBAL
        );
    }

    /// Cache policy lowers to the correct PCD/PWT combination.
    #[test]
    fn to_x86_lowers_cache() {
        let base = PageProt::PRESENT;
        assert_eq!(
            PageFlags::new(base)
                .with_cache(CacheType::WriteBack)
                .to_x86(),
            PageTableFlags::PRESENT
        );
        assert_eq!(
            PageFlags::new(base)
                .with_cache(CacheType::WriteThrough)
                .to_x86(),
            PageTableFlags::PRESENT | PageTableFlags::WRITE_THROUGH
        );
        assert_eq!(
            PageFlags::new(base).with_cache(CacheType::Device).to_x86(),
            PageTableFlags::PRESENT | PageTableFlags::NO_CACHE
        );
        assert_eq!(
            PageFlags::new(base)
                .with_cache(CacheType::Uncached)
                .to_x86(),
            PageTableFlags::PRESENT | PageTableFlags::NO_CACHE
        );
    }

    /// FAIL-demo: the kernel-data preset must NOT carry USER (a security-relevant
    /// mis-lowering would flip this).
    #[test]
    fn kernel_data_is_not_user_accessible() {
        let f = PageFlags::KERNEL_DATA.to_x86();
        assert!(f.contains(PageTableFlags::PRESENT));
        assert!(f.contains(PageTableFlags::WRITABLE));
        assert!(f.contains(PageTableFlags::NO_EXECUTE));
        assert!(!f.contains(PageTableFlags::USER_ACCESSIBLE));
    }

    /// The device preset must be uncached (PCD set) — a dropped PCD would make
    /// MMIO writes cacheable and silently corrupt device state.
    #[test]
    fn device_preset_is_uncached() {
        assert!(PageFlags::DEVICE
            .to_x86()
            .contains(PageTableFlags::NO_CACHE));
    }

    /// The user-root token is the PML4 phys base (what x86 `CR3` wants).
    #[test]
    fn user_root_token_is_pml4_phys_base() {
        let r =
            PhysFrame::<Size4KiB>::containing_address(crate::arch::PhysAddr::new(0x1_2345_6000));
        assert_eq!(user_root_token(r), 0x1_2345_6000);
    }
}
