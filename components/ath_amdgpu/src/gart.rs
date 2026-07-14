//! gfx11 GART (GPUVM) page-table builder — the PURE, host-provable half of GART
//! (roadmap §4, `docs/research/gfx11-cp-imu-startup.md`).
//!
//! The GFX/SDMA rings live at GPU virtual addresses that the CP fetches THROUGH
//! GPUVM; `gfx_v11_0_cp_gfx_resume` programs `CP_RB0_BASE = ring->gpu_addr >> 8`.
//! With no GART, no engine can fetch its ring — this is the end-to-end blocker.
//!
//! GART on gfx11 is a flat (PAGE_TABLE_DEPTH=0) page table: a linear array of
//! 64-bit PTEs, one per GPU page, that VMID0 walks. Each PTE carries the system
//! page's physical address plus permission/cache flags. This module is the PTE
//! encoding + table-fill — PURE logic with NO MMIO, so it is fully host-KAT'able
//! and SAFE (it never touches the live GPU). The risky half — programming the
//! gfxhub GMC registers (`GCVM_CONTEXT0_*`, `GCMC_VM_SYSTEM_APERTURE_*`, the page-
//! table base) on the display-driving GPU — is intentionally NOT here: it is
//! gated on the iron VM-dump decision (inherit the firmware's VM vs build ours)
//! and is documented as a register sequence in the research doc, to be wired only
//! once that data says "build".
//!
//! PTE bit layout + flags transcribed from `amdgpu_vm.h` (`AMDGPU_PTE_*`,
//! `AMDGPU_PTE_MTYPE_NV10`). The address occupies bits [47:12] (page-aligned).

use alloc::vec;
use alloc::vec::Vec;

/// PTE entry is valid (the page is mapped).
pub const PTE_VALID: u64 = 1 << 0;
/// The page lives in SYSTEM memory (GTT), not VRAM.
pub const PTE_SYSTEM: u64 = 1 << 1;
/// The GPU access is SNOOPED (cache-coherent with the CPU) — required for system
/// pages the CPU also writes (our ring buffers, filled by the daemon).
pub const PTE_SNOOPED: u64 = 1 << 2;
pub const PTE_EXECUTABLE: u64 = 1 << 4;
pub const PTE_READABLE: u64 = 1 << 5;
pub const PTE_WRITEABLE: u64 = 1 << 6;

/// NV10-style memory type sits in PTE bits [50:48] (`AMDGPU_PTE_MTYPE_NV10`).
#[inline]
pub const fn pte_mtype_nv10(mtype: u64) -> u64 {
    (mtype & 0x7) << 48
}

/// gfx11 NV10 memory type: UNCACHED. amdgpu's `gmc_v11_0` sets
/// `gart.gart_pte_flags = AMDGPU_PTE_MTYPE_NV10(0, MTYPE_UC) | EXECUTABLE`.
/// (Cache hint, non-structural — confirm against iron behavior; NV10 enum: NC=0,
/// WC=1, CC=2, UC=3.)
pub const MTYPE_UC: u64 = 3;

/// GPU page size is 4 KiB; the PTE address field is the page-aligned physical
/// address in bits [47:12].
pub const GPU_PAGE_SHIFT: u64 = 12;
pub const GPU_PAGE_SIZE: u64 = 1 << GPU_PAGE_SHIFT;
/// Address mask for the PTE's physical-address field (bits [47:12]).
pub const PTE_ADDR_MASK: u64 = 0x0000_FFFF_FFFF_F000;

/// Standard GART PTE flags for a snooped, system-memory, uncached page that the
/// GPU may read/write/execute — what the GFX/SDMA ring pages need.
#[inline]
pub fn gart_sys_pte_flags() -> u64 {
    PTE_VALID
        | PTE_SYSTEM
        | PTE_SNOOPED
        | PTE_READABLE
        | PTE_WRITEABLE
        | PTE_EXECUTABLE
        | pte_mtype_nv10(MTYPE_UC)
}

/// Encode one GART PTE: a system physical page address OR'd with `flags`. The
/// caller passes a page-aligned `sys_phys`; the mask drops any stray low bits.
#[inline]
pub fn encode_pte(sys_phys: u64, flags: u64) -> u64 {
    (sys_phys & PTE_ADDR_MASK) | flags
}

/// Decode a PTE's physical page address (bits [47:12]).
#[inline]
pub fn pte_addr(pte: u64) -> u64 {
    pte & PTE_ADDR_MASK
}

/// Build a flat GART page table that identity-maps `num_pages` GPU pages starting
/// at GPU-page 0 onto the contiguous system pages `[base_phys, base_phys +
/// num_pages*4K)`. This is the table VMID0 walks; loading it (and programming the
/// gfxhub registers) is the gated iron step. `base_phys` is page-aligned.
pub fn build_identity_gart(base_phys: u64, num_pages: usize) -> Vec<u64> {
    let flags = gart_sys_pte_flags();
    let mut table = vec![0u64; num_pages];
    for (i, pte) in table.iter_mut().enumerate() {
        let page_phys = base_phys + (i as u64) * GPU_PAGE_SIZE;
        *pte = encode_pte(page_phys, flags);
    }
    table
}

/// Number of GPU pages needed to cover `bytes`.
#[inline]
pub fn pages_for(bytes: u64) -> usize {
    ((bytes + GPU_PAGE_SIZE - 1) / GPU_PAGE_SIZE) as usize
}

// ── gfxhub_v3_0_gart_enable register programming (the build path) ────────────
//
// The firmware leaves GFX GPUVM unconfigured (boot 162824: all gfxhub VM regs 0),
// so the CP can't establish a ring — we must program it. This is the PURE sequence
// builder: given the discovery-resolved register offsets and the computed memory
// layout, it returns the ordered (reg, value) writes a caller applies via MMIO.
// Keeping it pure makes every field encoding HOST-KAT'able (a wrong shift is caught
// by a test, not by faulting the live CP). Field shifts from gc_11_0_0_sh_mask.h.

/// gfxhub GART register offsets (absolute MMIO bytes), resolved from IP discovery
/// by [`crate::regs::gfxhub_gart_regs`]. All GC seg0 except where noted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GfxhubGartRegs {
    pub l2_cntl: u32,
    pub l2_cntl2: u32,
    pub l2_cntl3: u32,
    pub fb_location_base: u32,
    pub fb_location_top: u32,
    pub agp_base: u32,
    pub agp_bot: u32,
    pub agp_top: u32,
    pub sys_aperture_low: u32,
    pub sys_aperture_high: u32,
    pub sys_default_lsb: u32,
    pub sys_default_msb: u32,
    pub mx_l1_tlb_cntl: u32,
    pub context0_cntl: u32,
    pub context0_ptb_lo32: u32,
    pub context0_ptb_hi32: u32,
    pub context0_start_lo32: u32,
    pub context0_start_hi32: u32,
    pub context0_end_lo32: u32,
    pub context0_end_hi32: u32,
    pub invalidate_eng0_req: u32,
    pub invalidate_eng0_ack: u32,
}

/// A FULL VMID0 legacy-mode TLB invalidate request (`gfxhub_v3_0_get_invalidate_req`
/// with vmid=0, flush_type=0): PER_VMID_INVALIDATE_REQ=1<<0, FLUSH_TYPE=0, and every
/// L2 PTE/PDE0-2 + L1 PTE invalidate bit set. Writing a bare `0x1` (our old value)
/// only set PER_VMID and skipped the L2/L1 flush, so the GMC walker kept stale/empty
/// entries and VMID0 translation faulted — the suspected reason the CP *and* SDMA
/// both stalled with RPTR=0 despite a correct page table. Field shifts from
/// gc_11_0_0_sh_mask.h (L2_PTES@19, PDE0@20, PDE1@21, PDE2@22, L1_PTES@23).
pub const GCVM_INVALIDATE_VMID0_FULL_REQ: u32 = (1 << 0) // PER_VMID_INVALIDATE_REQ (vmid0)
    | (1 << 19) // INVALIDATE_L2_PTES
    | (1 << 20) // INVALIDATE_L2_PDE0
    | (1 << 21) // INVALIDATE_L2_PDE1
    | (1 << 22) // INVALIDATE_L2_PDE2
    | (1 << 23); // INVALIDATE_L1_PTES

/// The ACK bit the GMC sets for VMID0 once the invalidate completes (poll this).
pub const GCVM_INVALIDATE_VMID0_ACK: u32 = 1 << 0;

/// The computed APU memory layout the GART build needs. Physical/bus addresses;
/// the daemon computes these from the GMC carve-out (`gfxhub_v3_0_init_gart`/
/// `amdgpu_gmc_*_location` equivalents) — that layout computation is the iron step
/// gated separately, so a wrong layout never reaches the live GMC unverified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GartConfig {
    /// Physical address of the GART page table (from [`build_identity_gart`]).
    pub table_phys: u64,
    /// GPUVM range VMID0 covers: [start, end) GPU virtual addresses.
    pub gart_va_start: u64,
    pub gart_va_end: u64,
    /// Frame-buffer (UMA carve-out) physical range.
    pub fb_base: u64,
    pub fb_top: u64,
    /// AGP aperture physical range.
    pub agp_bot: u64,
    pub agp_top: u64,
    /// System aperture physical range (covers system RAM the GPU may reach flat).
    pub sys_low: u64,
    pub sys_high: u64,
    /// Default page physical address (for unmapped/faulting accesses).
    pub default_page: u64,
}

// GCMC_VM_MX_L1_TLB_CNTL field shifts (gc_11_0_0_sh_mask.h). Calibrated to Athena's
// amdgpu cold-boot iron trace: GCMC_VM_MX_L1_TLB_CNTL ends at 0x1859. NOTE amdgpu
// CLEARS SYSTEM_APERTURE_UNMAPPED_ACCESS (bit 5) — unmapped accesses are handled by
// CONTEXT0_CNTL's fault routing, not the TLB — so we must not set it.
const L1_TLB_ENABLE_L1_TLB: u32 = 1 << 0;
const L1_TLB_SYSTEM_ACCESS_MODE_SHIFT: u32 = 3; // 3 = always (system access)
const L1_TLB_ENABLE_ADVANCED_DRIVER_MODEL: u32 = 1 << 6;
const L1_TLB_MTYPE_SHIFT: u32 = 11; // MTYPE field [13:11]; UC=3 => 0x1800
                                    // GCVM_CONTEXT0_CNTL field shifts.
const CTX_ENABLE_CONTEXT: u32 = 1 << 0;
const CTX_PAGE_TABLE_DEPTH_SHIFT: u32 = 1; // 0 = flat (single level)
                                           // Every GCVM_CONTEXT0_CNTL protection-fault-enable bit [24:9]: on a translation
                                           // fault, route to the default page instead of wedging the engine. amdgpu enables
                                           // them all (Athena iron trace: GCVM_CONTEXT0_CNTL = 0x1fffe01). A bare ENABLE_CONTEXT
                                           // (=0x1) leaves fault handling off, which silently stalls SDMA on any stray access.
const CTX_ALL_FAULT_ENABLE: u32 = 0x1fff_e00;
// GCVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32 bit 0 = the page-directory VALID flag.
// amdgpu sets it (Athena iron trace: PAGE_TABLE_BASE_LO = 0x5fd00001 on a page-
// aligned table — the low 1 can only be a flag), and AthenaOS was omitting it, so the
// GMC saw the root PDB as not-present and never walked the table.
const PTB_VALID: u32 = 1 << 0;
// GCVM_L2_CNTL: ENABLE_L2_CACHE bit 0.
const L2_ENABLE_L2_CACHE: u32 = 1 << 0;
// GCVM_L2_CNTL2: invalidate-all bits (INVALIDATE_ALL_L1_TLBS, INVALIDATE_L2_CACHE).
const L2_CNTL2_INVALIDATE_ALL: u32 = (1 << 0) | (1 << 1);

/// Build the ordered (reg, value) writes that enable gfxhub GPUVM + GART for VMID0
/// — the essential subset of `gfxhub_v3_0_gart_enable`: L2 cache on, FB/AGP/system
/// aperture located, VMID0 flat page table at `config.table_phys` covering
/// [start,end), L1 TLB on with system access, CONTEXT0 enabled, then TLB-invalidate.
/// Addresses are encoded per the hardware's register units (FB/AGP >>24, system
/// aperture >>18, page-table base raw 64-bit lo/hi, VA start/end >>12).
pub fn build_gart_enable_sequence(r: &GfxhubGartRegs, c: &GartConfig) -> Vec<(u32, u32)> {
    let mut w: Vec<(u32, u32)> = Vec::new();
    // L2 cache on + invalidate.
    w.push((r.l2_cntl, L2_ENABLE_L2_CACHE));
    w.push((r.l2_cntl2, L2_CNTL2_INVALIDATE_ALL));
    w.push((r.l2_cntl3, 0x80000000)); // L2_CACHE_BIGK_FRAGMENT_SIZE default-ish
                                      // FB location (>>24, 16 MiB units), low in BASE high in TOP.
                                      // FB window: only write it when the caller KNOWS the VRAM MC window. The working
                                      // driver NEVER writes GCMC_VM_FB_LOCATION (zero writes in the cold mmiotrace) —
                                      // it inherits the POST/IMU value (live umr: BASE=0x8000 = MC 0x80_0000_0000).
                                      // Writing 0/0 here (the pre-2026-07-01 behavior when the window was unknown)
                                      // CLOBBERS that inherited window and points the hub's FB range at MC [0,16MB) —
                                      // a divergence active exactly when the MES set_hw_resources handler hangs.
    if c.fb_base != 0 || c.fb_top != 0 {
        w.push((r.fb_location_base, (c.fb_base >> 24) as u32));
        w.push((r.fb_location_top, (c.fb_top >> 24) as u32));
    }
    // AGP aperture (>>24).
    w.push((r.agp_base, 0));
    w.push((r.agp_bot, (c.agp_bot >> 24) as u32));
    w.push((r.agp_top, (c.agp_top >> 24) as u32));
    // System aperture (>>18, 256 KiB units).
    w.push((r.sys_aperture_low, (c.sys_low >> 18) as u32));
    w.push((r.sys_aperture_high, (c.sys_high >> 18) as u32));
    // Default page (>>12, 4 KiB units), lo/hi.
    let def = c.default_page >> 12;
    w.push((r.sys_default_lsb, def as u32));
    w.push((r.sys_default_msb, (def >> 32) as u32));
    // VMID0 flat page table at table_phys, covering [start,end). Bit 0 of the LO
    // word is the PDB VALID flag (PTB_VALID) the GMC requires to walk the table.
    w.push((
        r.context0_ptb_lo32,
        ((c.table_phys & 0xFFFF_FFFF) as u32) | PTB_VALID,
    ));
    w.push((r.context0_ptb_hi32, (c.table_phys >> 32) as u32));
    let s = c.gart_va_start >> GPU_PAGE_SHIFT;
    let e = c.gart_va_end >> GPU_PAGE_SHIFT;
    w.push((r.context0_start_lo32, s as u32));
    w.push((r.context0_start_hi32, (s >> 32) as u32));
    w.push((r.context0_end_lo32, e as u32));
    w.push((r.context0_end_hi32, (e >> 32) as u32));
    // L1 TLB on, calibrated to Athena's amdgpu iron trace (GCMC_VM_MX_L1_TLB_CNTL =
    // 0x1859): enable + SYSTEM_ACCESS_MODE=3 + ENABLE_ADVANCED_DRIVER_MODEL +
    // MTYPE=UC. SYSTEM_APERTURE_UNMAPPED_ACCESS stays clear (see the const comment).
    let l1 = L1_TLB_ENABLE_L1_TLB
        | (3u32 << L1_TLB_SYSTEM_ACCESS_MODE_SHIFT)
        | L1_TLB_ENABLE_ADVANCED_DRIVER_MODEL
        | ((MTYPE_UC as u32) << L1_TLB_MTYPE_SHIFT);
    w.push((r.mx_l1_tlb_cntl, l1));
    // Enable CONTEXT0 (VMID0), flat (PAGE_TABLE_DEPTH=0), with every protection-fault
    // handler enabled (Athena iron trace: GCVM_CONTEXT0_CNTL = 0x1fffe01).
    let ctx = CTX_ENABLE_CONTEXT | (0u32 << CTX_PAGE_TABLE_DEPTH_SHIFT) | CTX_ALL_FAULT_ENABLE;
    w.push((r.context0_cntl, ctx));
    // Kick a FULL TLB invalidate (L2+L1 PTE/PDE flush) so the new VMID0 page table
    // takes effect. The caller must poll `invalidate_eng0_ack` bit0 after this — a
    // bare PER_VMID write that never completes leaves the walker stale.
    w.push((r.invalidate_eng0_req, GCVM_INVALIDATE_VMID0_FULL_REQ));
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pte_encodes_addr_and_flags() {
        let flags = gart_sys_pte_flags();
        // VALID|SYSTEM|SNOOPED|READABLE|WRITEABLE|EXECUTABLE present.
        assert_ne!(flags & PTE_VALID, 0);
        assert_ne!(flags & PTE_SYSTEM, 0);
        assert_ne!(flags & PTE_SNOOPED, 0);
        assert_ne!(flags & (PTE_READABLE | PTE_WRITEABLE | PTE_EXECUTABLE), 0);
        // MTYPE_UC lands in bits [50:48].
        assert_eq!((flags >> 48) & 0x7, MTYPE_UC);
        // A page-aligned address round-trips; flags occupy only the low/ mtype bits.
        let pte = encode_pte(0x1_2345_6000, flags);
        assert_eq!(pte_addr(pte), 0x1_2345_6000);
        assert_eq!(pte & 0xff, flags & 0xff);
        // Stray low bits in the address are masked off (never corrupt flags).
        let pte2 = encode_pte(0x1_2345_6ABC, PTE_VALID);
        assert_eq!(pte_addr(pte2), 0x1_2345_6000);
        assert_eq!(pte2 & 0xfff, PTE_VALID);
    }

    #[test]
    fn identity_gart_maps_contiguous_pages() {
        let base = 0x4_0000_0000u64; // a 4 GiB-aligned system base
        let table = build_identity_gart(base, 4);
        assert_eq!(table.len(), 4);
        for (i, &pte) in table.iter().enumerate() {
            assert_ne!(pte & PTE_VALID, 0, "every entry valid");
            assert_eq!(pte_addr(pte), base + (i as u64) * GPU_PAGE_SIZE);
        }
    }

    #[test]
    fn pages_for_rounds_up() {
        assert_eq!(pages_for(0), 0);
        assert_eq!(pages_for(1), 1);
        assert_eq!(pages_for(GPU_PAGE_SIZE), 1);
        assert_eq!(pages_for(GPU_PAGE_SIZE + 1), 2);
        assert_eq!(pages_for(256 * 1024), 64); // a 256 KiB ring = 64 pages
    }

    #[test]
    fn gart_enable_sequence_encodes_fields_and_units() {
        // Distinct offsets so we can find each write by register.
        let r = GfxhubGartRegs {
            l2_cntl: 0x100,
            l2_cntl2: 0x104,
            l2_cntl3: 0x108,
            fb_location_base: 0x10c,
            fb_location_top: 0x110,
            agp_base: 0x114,
            agp_bot: 0x118,
            agp_top: 0x11c,
            sys_aperture_low: 0x120,
            sys_aperture_high: 0x124,
            sys_default_lsb: 0x128,
            sys_default_msb: 0x12c,
            mx_l1_tlb_cntl: 0x130,
            context0_cntl: 0x134,
            context0_ptb_lo32: 0x138,
            context0_ptb_hi32: 0x13c,
            context0_start_lo32: 0x140,
            context0_start_hi32: 0x144,
            context0_end_lo32: 0x148,
            context0_end_hi32: 0x14c,
            invalidate_eng0_req: 0x150,
            invalidate_eng0_ack: 0x154,
        };
        let c = GartConfig {
            table_phys: 0x1_2345_6000,
            gart_va_start: 0,
            gart_va_end: 0x20_0000, // 2 MiB GART VA
            fb_base: 0x8000_0000,
            fb_top: 0x8800_0000,
            agp_bot: 0,
            agp_top: 0,
            sys_low: 0x4_0000_0000,
            sys_high: 0x4_4000_0000,
            default_page: 0x9000,
        };
        let seq = build_gart_enable_sequence(&r, &c);
        let find = |reg: u32| seq.iter().find(|(rr, _)| *rr == reg).map(|(_, v)| *v);
        // L2 cache enabled (bit 0).
        assert_eq!(find(r.l2_cntl).unwrap() & 1, 1);
        // FB location is >>24 (16 MiB units).
        assert_eq!(
            find(r.fb_location_base),
            Some((0x8000_0000u64 >> 24) as u32)
        );
        // System aperture is >>18 (256 KiB units).
        assert_eq!(
            find(r.sys_aperture_low),
            Some((0x4_0000_0000u64 >> 18) as u32)
        );
        // Page-table base is table_phys lo/hi, with bit 0 = PDB VALID set in the LO.
        assert_eq!(find(r.context0_ptb_lo32), Some(0x2345_6001));
        assert_eq!(find(r.context0_ptb_hi32), Some(0x1));
        // Page-table END is >>12 (GPU page units): 2 MiB = 0x200 pages.
        assert_eq!(find(r.context0_end_lo32), Some(0x200));
        // L1 TLB enabled (bit 0) + advanced driver model (bit 6).
        let l1 = find(r.mx_l1_tlb_cntl).unwrap();
        assert_ne!(l1 & 1, 0);
        assert_ne!(l1 & (1 << 6), 0);
        // CONTEXT0 enabled (bit 0).
        assert_ne!(find(r.context0_cntl).unwrap() & 1, 0);
        // A TLB invalidate is the last write.
        assert_eq!(seq.last().map(|(rr, _)| *rr), Some(r.invalidate_eng0_req));
    }

    /// Distinct register offsets so each write is findable by register in a test.
    fn distinct_offsets() -> GfxhubGartRegs {
        GfxhubGartRegs {
            l2_cntl: 0x100,
            l2_cntl2: 0x104,
            l2_cntl3: 0x108,
            fb_location_base: 0x10c,
            fb_location_top: 0x110,
            agp_base: 0x114,
            agp_bot: 0x118,
            agp_top: 0x11c,
            sys_aperture_low: 0x120,
            sys_aperture_high: 0x124,
            sys_default_lsb: 0x128,
            sys_default_msb: 0x12c,
            mx_l1_tlb_cntl: 0x130,
            context0_cntl: 0x134,
            context0_ptb_lo32: 0x138,
            context0_ptb_hi32: 0x13c,
            context0_start_lo32: 0x140,
            context0_start_hi32: 0x144,
            context0_end_lo32: 0x148,
            context0_end_hi32: 0x14c,
            invalidate_eng0_req: 0x150,
            invalidate_eng0_ack: 0x154,
        }
    }

    /// Ground-truth calibration against the Athena amdgpu COLD-BOOT iron trace
    /// (`docs/gpu-oracle/cold_mmio.txt`, captured 2026-06-24): the builder must
    /// reproduce the exact gfxhub register values real amdgpu wrote on this silicon,
    /// right after it confirmed the RLC autoload. This is the gate that lets GART be
    /// wired onto the live, display-driving GPU with confidence instead of a guess.
    #[test]
    fn gart_enable_matches_athena_amdgpu_iron_trace() {
        // Athena's GMC layout, read back from amdgpu's writes in the cold trace:
        //   FB (UMA carve-out) at GPU-phys 0x80_0000_0000, 2 GiB; system aperture =
        //   fb_start/end >> 18 = 0x200000 / 0x201fff. GART VMID0 covers 512 MiB at
        //   GPU VA 0x7fff_0000_0000 (START_LO/HI = 0xfff00000/0x7; END is INCLUSIVE,
        //   END_LO/HI = 0xfff1ffff/0x7).
        let r = distinct_offsets();
        let c = GartConfig {
            table_phys: 0x45_fd00_0000, // AthenaOS-allocated (amdgpu's was 0x4_5fd00000)
            gart_va_start: 0x7fff_0000_0000,
            gart_va_end: 0x7fff_1fff_ffff, // inclusive last byte, matching amdgpu's END
            fb_base: 0x80_0000_0000,
            fb_top: 0x80_7fff_c000,
            agp_bot: 0,
            agp_top: 0,
            sys_low: 0x80_0000_0000,
            sys_high: 0x80_7fff_c000,
            default_page: 0,
        };
        let seq = build_gart_enable_sequence(&r, &c);
        let find = |reg: u32| seq.iter().find(|(rr, _)| *rr == reg).map(|(_, v)| *v);
        // The two values the trace caught AthenaOS getting wrong:
        assert_eq!(
            find(r.mx_l1_tlb_cntl),
            Some(0x1859),
            "GCMC_VM_MX_L1_TLB_CNTL"
        );
        assert_eq!(find(r.context0_cntl), Some(0x1fffe01), "GCVM_CONTEXT0_CNTL");
        // System aperture (>>18 units) — exact match to the iron writes.
        assert_eq!(find(r.sys_aperture_low), Some(0x200000), "SYS_APERTURE_LOW");
        assert_eq!(
            find(r.sys_aperture_high),
            Some(0x201fff),
            "SYS_APERTURE_HIGH"
        );
        // CONTEXT0 page-table START/END (VA >>12), lo/hi — the GART VMID0 window.
        assert_eq!(
            find(r.context0_start_lo32),
            Some(0xfff00000),
            "CONTEXT0_START_LO"
        );
        assert_eq!(find(r.context0_start_hi32), Some(0x7), "CONTEXT0_START_HI");
        assert_eq!(
            find(r.context0_end_lo32),
            Some(0xfff1ffff),
            "CONTEXT0_END_LO"
        );
        assert_eq!(find(r.context0_end_hi32), Some(0x7), "CONTEXT0_END_HI");
    }
}
