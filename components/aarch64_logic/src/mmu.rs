//! VMSAv8-64 stage-1 page-table descriptor encoder (4 KiB granule, 48-bit VA).
//!
//! Pure bit/integer math: given the mapping parameters, produce the 64-bit
//! descriptor the MMU walker consumes — plus `TCR_EL1`/`MAIR_EL1` builder
//! helpers. NO MMIO, NO sysreg access; the kernel's `arch/aarch64/mmu.rs`
//! (spec slice A4) writes these computed values to the real registers.
//!
//! ## Grounding (ARM Architecture Reference Manual, ARMv8-A, AArch64)
//! Field layout of a stage-1 Block/Page descriptor with the 4 KiB granule, per
//! the VMSAv8-64 translation-table format (ARM DDI 0487, section D8 "The
//! AArch64 Virtual Memory System Architecture" — "Translation table descriptor
//! formats"). Bit positions below are the documented layout:
//!
//! ```text
//!  bit  0       : VALID            (1 = descriptor valid)
//!  bit  1       : descriptor TYPE  (L0..L2: 0=Block, 1=Table/next-level;
//!                                    L3: 1=Page — an L3 block-bit-0 entry is reserved)
//!  bits 4:2     : AttrIndx[2:0]    (index into MAIR_EL1, selects a memory type)
//!  bit  5       : NS              (non-secure; 0 here — single security state)
//!  bits 7:6     : AP[2:1]         (data access permissions)
//!  bits 9:8     : SH[1:0]         (shareability)
//!  bit  10      : AF              (access flag)
//!  bit  11      : nG              (not-global)
//!  bits 47:12   : output address  (4 KiB granule => OA[47:12], lower 12 RES0)
//!  bit  52      : Contiguous hint
//!  bit  53      : PXN             (privileged execute-never)
//!  bit  54      : UXN/XN          (unprivileged / EL0 execute-never)
//! ```
//!
//! AP[2:1] encoding (stage 1, ARM ARM table "Data access permissions"):
//!   0b00 = RW, EL1 only          0b01 = RW, EL0+EL1
//!   0b10 = RO, EL1 only          0b11 = RO, EL0+EL1
//! SH[1:0] encoding: 0b00 Non-shareable, 0b10 Outer-shareable, 0b11 Inner-shareable
//! (0b01 is reserved).

/// Descriptor bit 0: the descriptor is valid.
pub const DESC_VALID: u64 = 1 << 0;
/// Descriptor bit 1: at L0..L2 selects Table (1) vs Block (0); at L3 must be 1
/// for a valid Page.
pub const DESC_TYPE_TABLE_OR_PAGE: u64 = 1 << 1;
/// Bit 10: Access Flag. Hardware (or software) must set this or the first
/// access faults; we set it on leaf maps.
pub const DESC_AF: u64 = 1 << 10;
/// Bit 11: not-Global.
pub const DESC_NG: u64 = 1 << 11;
/// Bit 5: Non-secure (single security state in our model => left 0).
pub const DESC_NS: u64 = 1 << 5;
/// Bit 52: Contiguous hint.
pub const DESC_CONTIG: u64 = 1 << 52;
/// Bit 53: Privileged eXecute-Never.
pub const DESC_PXN: u64 = 1 << 53;
/// Bit 54: unprivileged/EL0 eXecute-Never (XN at EL1&0 stage 1).
pub const DESC_UXN: u64 = 1 << 54;

/// Output-address field mask: OA[47:12] for the 4 KiB granule (48-bit PA).
pub const OA_MASK_4K: u64 = 0x0000_FFFF_FFFF_F000;

/// Data access permissions (AP[2:1], stage-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessPerm {
    /// AP=0b00: read/write, EL1 only (kernel data).
    RwEl1 = 0b00,
    /// AP=0b01: read/write, EL0 and EL1 (user data).
    RwEl0El1 = 0b01,
    /// AP=0b10: read-only, EL1 only.
    RoEl1 = 0b10,
    /// AP=0b11: read-only, EL0 and EL1.
    RoEl0El1 = 0b11,
}

/// Shareability (SH[1:0]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shareability {
    /// SH=0b00.
    NonShareable = 0b00,
    /// SH=0b10.
    OuterShareable = 0b10,
    /// SH=0b11.
    InnerShareable = 0b11,
}

/// The kind of descriptor to emit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DescKind {
    /// A block mapping at L1 (1 GiB) or L2 (2 MiB) — bit[1]=0.
    Block,
    /// A page mapping at L3 (4 KiB) — bit[1]=1 (and bit[0]=1).
    Page,
    /// A table descriptor at L0..L2 pointing at the next-level table — bit[1]=1.
    Table,
}

/// Parameters for a leaf (Block or Page) mapping.
#[derive(Clone, Copy, Debug)]
pub struct LeafAttrs {
    /// Index into `MAIR_EL1` (selects the memory type). 3-bit field.
    pub attr_index: u8,
    /// Data access permission.
    pub ap: AccessPerm,
    /// Shareability.
    pub sh: Shareability,
    /// Access Flag (almost always true for a freshly-built map).
    pub af: bool,
    /// not-Global (per-ASID) mapping.
    pub ng: bool,
    /// Privileged execute-never.
    pub pxn: bool,
    /// Unprivileged execute-never.
    pub uxn: bool,
}

/// Errors the encoder can report (so a wrong call FAILS loudly instead of
/// silently producing a corrupt descriptor).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MmuError {
    /// The output PA is not 4 KiB aligned, or sets bits above 48.
    MisalignedOrOversizedPa,
    /// `attr_index` does not fit the 3-bit AttrIndx field.
    AttrIndexTooLarge,
    /// A Block descriptor was requested at a level that has no block format
    /// (only L1 and L2 have blocks for the 4 KiB granule).
    BlockLevelInvalid,
    /// A Page descriptor was requested at a level other than L3.
    PageLevelInvalid,
}

const fn pa_ok(pa: u64) -> bool {
    // 4 KiB aligned and within 48-bit PA space.
    (pa & 0xFFF) == 0 && (pa & !OA_MASK_4K & 0x000F_FFFF_FFFF_FFFF) == 0
}

fn encode_leaf_attrs(a: &LeafAttrs) -> Result<u64, MmuError> {
    if a.attr_index > 0b111 {
        return Err(MmuError::AttrIndexTooLarge);
    }
    let mut bits = 0u64;
    bits |= (a.attr_index as u64) << 2; // AttrIndx[2:0] at bits[4:2]
    bits |= (a.ap as u64) << 6; // AP[2:1] at bits[7:6]
    bits |= (a.sh as u64) << 8; // SH[1:0] at bits[9:8]
    if a.af {
        bits |= DESC_AF;
    }
    if a.ng {
        bits |= DESC_NG;
    }
    if a.pxn {
        bits |= DESC_PXN;
    }
    if a.uxn {
        bits |= DESC_UXN;
    }
    Ok(bits)
}

/// Encode a leaf (Block at L1/L2, Page at L3) descriptor.
///
/// `level` is the translation level (0..=3). For the 4 KiB granule, blocks are
/// valid only at level 1 (1 GiB) and level 2 (2 MiB); pages only at level 3.
pub fn encode_leaf(pa: u64, level: u8, kind: DescKind, attrs: LeafAttrs) -> Result<u64, MmuError> {
    if !pa_ok(pa) {
        return Err(MmuError::MisalignedOrOversizedPa);
    }
    match kind {
        DescKind::Block => {
            if level != 1 && level != 2 {
                return Err(MmuError::BlockLevelInvalid);
            }
        }
        DescKind::Page => {
            if level != 3 {
                return Err(MmuError::PageLevelInvalid);
            }
        }
        DescKind::Table => return encode_table(pa),
    }

    let mut d = DESC_VALID;
    // bit[1]: Block => 0, Page (L3) => 1.
    if matches!(kind, DescKind::Page) {
        d |= DESC_TYPE_TABLE_OR_PAGE;
    }
    d |= pa & OA_MASK_4K;
    d |= encode_leaf_attrs(&attrs)?;
    Ok(d)
}

/// Encode a Table descriptor at L0..L2 pointing at the next-level table page.
/// (Table descriptors carry no memory attributes; those live in the leaf.)
pub fn encode_table(next_table_pa: u64) -> Result<u64, MmuError> {
    if !pa_ok(next_table_pa) {
        return Err(MmuError::MisalignedOrOversizedPa);
    }
    // bit[0]=1 valid, bit[1]=1 table, OA[47:12] = next table base.
    Ok(DESC_VALID | DESC_TYPE_TABLE_OR_PAGE | (next_table_pa & OA_MASK_4K))
}

/// Extract the output PA from a leaf/table descriptor (inverse of the encoder,
/// used by the smoketest round-trip).
pub fn descriptor_output_pa(desc: u64) -> u64 {
    desc & OA_MASK_4K
}

// ---------------------------------------------------------------------------
// MAIR_EL1 / TCR_EL1 builders
// ---------------------------------------------------------------------------

/// One `MAIR_EL1` attribute byte. The register holds 8 such bytes (Attr0..7).
///
/// Grounding (ARM ARM, `MAIR_EL1` description):
/// - `0x00`            = Device-nGnRnE (strongly ordered device memory).
/// - `0x04`            = Device-nGnRE.
/// - `0xFF`            = Normal, Inner+Outer Write-Back non-transient, R+W alloc.
/// - `0x44`            = Normal, Inner+Outer Non-cacheable.
pub mod mair {
    /// Normal memory, Inner & Outer Write-Back, Read+Write-Allocate, non-transient.
    /// Outer = high nibble 0b1111, Inner = low nibble 0b1111.
    pub const NORMAL_WB: u8 = 0xFF;
    /// Device-nGnRnE (MMIO that tolerates no gathering/reordering/early-ack).
    pub const DEVICE_NGNRNE: u8 = 0x00;
    /// Device-nGnRE (MMIO permitting early write acknowledge).
    pub const DEVICE_NGNRE: u8 = 0x04;
    /// Normal, Inner & Outer Non-cacheable.
    pub const NORMAL_NC: u8 = 0x44;

    /// Build a full 64-bit `MAIR_EL1` from eight attribute bytes (Attr0 in the
    /// low byte). Index `n` selects `attrs[n]`; descriptors reference it via
    /// `AttrIndx`.
    pub const fn build(attrs: [u8; 8]) -> u64 {
        (attrs[0] as u64)
            | (attrs[1] as u64) << 8
            | (attrs[2] as u64) << 16
            | (attrs[3] as u64) << 24
            | (attrs[4] as u64) << 32
            | (attrs[5] as u64) << 40
            | (attrs[6] as u64) << 48
            | (attrs[7] as u64) << 56
    }

    /// RaeenOS's canonical MAIR for QEMU-virt bring-up:
    /// index 0 = Normal WB (RAM), index 1 = Device-nGnRnE (MMIO), rest unused.
    pub const fn raeen_default() -> u64 {
        build([NORMAL_WB, DEVICE_NGNRNE, 0, 0, 0, 0, 0, 0])
    }
}

/// `TCR_EL1` builder for the RaeenOS aarch64 model: 4 KiB granule on BOTH
/// halves, 48-bit VA (T0SZ=T1SZ=16), inner/outer WB cacheable, inner-shareable
/// table walks, and 48-bit IPS.
///
/// Grounding (ARM ARM, `TCR_EL1` field description):
/// ```text
///   T0SZ  bits 5:0    : 64 - VA_bits (16 => 48-bit VA for TTBR0)
///   IRGN0 bits 9:8    : 0b01 = Normal WB RA WA cacheable (TTBR0 walk)
///   ORGN0 bits 11:10  : 0b01 = Normal WB RA WA cacheable
///   SH0   bits 13:12  : 0b11 = Inner-shareable
///   TG0   bits 15:14  : 0b00 = 4 KiB granule (TTBR0)
///   T1SZ  bits 21:16  : 16 => 48-bit VA for TTBR1 (high half)
///   IRGN1 bits 25:24  : 0b01
///   ORGN1 bits 27:26  : 0b01
///   SH1   bits 29:28  : 0b11
///   TG1   bits 31:30  : 0b10 = 4 KiB granule (TTBR1 — note TG1's 4 KiB code
///                              is 0b10, which DIFFERS from TG0's 0b00)
///   IPS   bits 34:32  : 0b101 = 48-bit (256 TiB) physical address size
/// ```
/// The TG1 vs TG0 granule encoding asymmetry is a genuine ARM ARM footgun:
/// TG0 4 KiB = 0b00, but TG1 4 KiB = 0b10. Encoded explicitly below.
pub fn tcr_el1_4k_48bit() -> u64 {
    let t0sz: u64 = 16;
    let t1sz: u64 = 16;
    let mut tcr = 0u64;
    tcr |= t0sz; // T0SZ[5:0]
    tcr |= 0b01 << 8; // IRGN0 = WB RA WA
    tcr |= 0b01 << 10; // ORGN0 = WB RA WA
    tcr |= 0b11 << 12; // SH0   = Inner shareable
    tcr |= 0b00 << 14; // TG0   = 4 KiB
    tcr |= t1sz << 16; // T1SZ[21:16]
    tcr |= 0b01 << 24; // IRGN1
    tcr |= 0b01 << 26; // ORGN1
    tcr |= 0b11 << 28; // SH1
    tcr |= 0b10 << 30; // TG1   = 4 KiB (note: 0b10, not 0b00)
    tcr |= 0b101 << 32; // IPS = 48-bit
    tcr
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: typical kernel Normal-WB RW map at EL1, inner-shareable.
    fn kernel_normal() -> LeafAttrs {
        LeafAttrs {
            attr_index: 0, // MAIR index 0 = Normal WB
            ap: AccessPerm::RwEl1,
            sh: Shareability::InnerShareable,
            af: true,
            ng: false,
            pxn: false,
            uxn: true, // kernel data: not executable
        }
    }

    #[test]
    fn block_2mib_l2_known_value() {
        // Map VA->PA 0x4020_0000 (a 2 MiB-aligned RAM block in QEMU-virt RAM),
        // L2 block, Normal-WB, RW@EL1, inner-shareable, AF set, UXN.
        // Expected bits, computed from the ARM ARM layout:
        //   VALID(0)            = 0x1
        //   type block          = 0 (bit1 clear)
        //   AttrIndx=0          = 0
        //   AP=0b00 (RwEl1)     = 0
        //   SH=0b11 @ bits[9:8] = 0x300
        //   AF  bit10           = 0x400
        //   UXN bit54           = 1<<54
        //   OA                  = 0x4020_0000
        let pa = 0x4020_0000;
        let d = encode_leaf(pa, 2, DescKind::Block, kernel_normal()).unwrap();
        let expected = 0x1 | 0x300 | 0x400 | (1u64 << 54) | pa;
        assert_eq!(d, expected, "got {:#018x} want {:#018x}", d, expected);
        // Reserved/RES0 sanity: bit1 (table) must be 0 for a block.
        assert_eq!(d & DESC_TYPE_TABLE_OR_PAGE, 0, "block must have bit1=0");
        // OA round-trips.
        assert_eq!(descriptor_output_pa(d), pa);
    }

    #[test]
    fn page_4kib_l3_user_rw_exec() {
        // User RW page, MAIR index 0, outer-shareable, executable at EL0
        // (UXN=0), PA 0x4100_1000.
        let pa = 0x4100_1000;
        let attrs = LeafAttrs {
            attr_index: 0,
            ap: AccessPerm::RwEl0El1,
            sh: Shareability::OuterShareable,
            af: true,
            ng: true,
            pxn: true,  // not executable at EL1
            uxn: false, // executable at EL0
        };
        let d = encode_leaf(pa, 3, DescKind::Page, attrs).unwrap();
        // VALID|PAGE = 0x3; AttrIndx0=0; AP=0b01<<6=0x40; SH=0b10<<8=0x200;
        // AF=0x400; nG=0x800; PXN=1<<53; OA.
        let expected = 0x3 | 0x40 | 0x200 | 0x400 | 0x800 | (1u64 << 53) | pa;
        assert_eq!(d, expected, "got {:#018x} want {:#018x}", d, expected);
        // A valid L3 page MUST have both bit0 and bit1 set.
        assert_eq!(d & 0b11, 0b11, "L3 page needs bits[1:0]=0b11");
    }

    #[test]
    fn table_descriptor_known_value() {
        // Next-level table at PA 0x4007_F000.
        let next = 0x4007_F000;
        let d = encode_table(next).unwrap();
        assert_eq!(d, 0x3 | next, "table = valid|table-bit|OA");
        // Table descriptors carry NO attribute bits in [11:2] or [54:52].
        assert_eq!(d & 0x00F0_0000_0000_0FFC, 0, "table attr bits must be 0");
    }

    #[test]
    fn device_mmio_page_attr_index_1() {
        // Device-nGnRnE page for the PL011 UART (MAIR index 1), non-shareable,
        // RW@EL1, execute-never, PA 0x0900_0000.
        let pa = 0x0900_0000;
        let attrs = LeafAttrs {
            attr_index: 1, // MAIR index 1 = Device
            ap: AccessPerm::RwEl1,
            sh: Shareability::NonShareable,
            af: true,
            ng: false,
            pxn: true,
            uxn: true,
        };
        let d = encode_leaf(pa, 3, DescKind::Page, attrs).unwrap();
        // AttrIndx=1 => bits[4:2]=0b001 => 0x4.
        assert_eq!(d & 0b11100, 0x4, "AttrIndx must be 1 (<<2)");
    }

    #[test]
    fn rejects_misaligned_pa() {
        let r = encode_leaf(0x4020_0001, 2, DescKind::Block, kernel_normal());
        assert_eq!(r, Err(MmuError::MisalignedOrOversizedPa));
    }

    #[test]
    fn rejects_block_at_l3() {
        let r = encode_leaf(0x4020_0000, 3, DescKind::Block, kernel_normal());
        assert_eq!(r, Err(MmuError::BlockLevelInvalid));
    }

    #[test]
    fn rejects_oversized_attr_index() {
        let mut a = kernel_normal();
        a.attr_index = 8; // does not fit 3 bits
        let r = encode_leaf(0x4020_0000, 2, DescKind::Block, a);
        assert_eq!(r, Err(MmuError::AttrIndexTooLarge));
    }

    // ---- FAIL-DEMONSTRATION: a wrong AttrIndx / wrong OA shift breaks the
    // assert. We prove the test is FAIL-able by computing what a buggy encoder
    // WOULD produce and asserting it differs from the correct value. ----
    #[test]
    fn faildemo_wrong_attrindx_would_mismatch() {
        let pa = 0x4020_0000;
        let correct = encode_leaf(pa, 2, DescKind::Block, kernel_normal()).unwrap();
        // A buggy encoder that put AttrIndx at bits[3:1] instead of [4:2], or
        // used attr_index 1 instead of 0, yields a different word — the
        // known-value asserts above would catch it.
        let mut wrong_attrs = kernel_normal();
        wrong_attrs.attr_index = 1;
        let wrong = encode_leaf(pa, 2, DescKind::Block, wrong_attrs).unwrap();
        assert_ne!(
            correct, wrong,
            "if a wrong AttrIndx produced the same word the KAT could not fail"
        );
    }

    #[test]
    fn faildemo_wrong_oa_shift_would_mismatch() {
        // Demonstrate the OA-shift class of bug: masking PA with the WRONG
        // mask (e.g. dropping bit 12) changes the descriptor, so the
        // known-value asserts can catch a bad shift.
        let pa = 0x4100_1000;
        let good = encode_leaf(pa, 3, DescKind::Page, kernel_normal()).unwrap();
        let bad_oa = pa & 0x0000_FFFF_FFFF_E000; // wrongly clears bit 12
        let bad = (good & !OA_MASK_4K) | bad_oa;
        assert_ne!(good, bad, "a dropped OA bit must change the descriptor");
    }

    #[test]
    fn mair_default_known_value() {
        // index0 Normal-WB(0xFF), index1 Device-nGnRnE(0x00).
        assert_eq!(mair::raeen_default(), 0x0000_0000_0000_00FF);
        // FAIL-able: putting device at index0 would change the low byte.
        let swapped = mair::build([mair::DEVICE_NGNRNE, mair::NORMAL_WB, 0, 0, 0, 0, 0, 0]);
        assert_ne!(mair::raeen_default(), swapped);
    }

    #[test]
    fn tcr_el1_known_fields() {
        let tcr = tcr_el1_4k_48bit();
        assert_eq!(tcr & 0x3F, 16, "T0SZ=16");
        assert_eq!((tcr >> 16) & 0x3F, 16, "T1SZ=16");
        assert_eq!((tcr >> 14) & 0b11, 0b00, "TG0=4KiB(0b00)");
        assert_eq!((tcr >> 30) & 0b11, 0b10, "TG1=4KiB(0b10) — the asymmetry");
        assert_eq!((tcr >> 32) & 0b111, 0b101, "IPS=48-bit");
        // FAIL-demo: if TG1 were wrongly encoded as 0b00 (copying TG0) the
        // value would differ — proving the asymmetry assert can fail.
        let buggy = (tcr & !(0b11 << 30)) | (0b00 << 30);
        assert_ne!(tcr, buggy, "TG1 0b10 vs buggy 0b00 must differ");
    }
}
