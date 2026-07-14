//! GICv2 register offset + bit math (pure arithmetic, NO MMIO).
//!
//! Given an IRQ number / target CPU set, compute the byte offset within the
//! GIC distributor (`GICD_*`) or CPU interface (`GICC_*`) and the bit/field
//! position to read or write. The kernel's `arch/aarch64/irq.rs` (spec slice
//! A5) does the actual `read_volatile`/`write_volatile` at
//! `dist_base + offset` / `cpu_base + offset`; this crate decides WHERE.
//!
//! ## Grounding (ARM Generic Interrupt Controller Architecture Specification,
//! GIC v2, ARM IHI 0048B — register maps)
//!
//! Distributor (`GICD_*`), relative to the distributor base:
//! ```text
//!   0x000  GICD_CTLR        Distributor control
//!   0x004  GICD_TYPER       Controller type (ITLinesNumber etc.)
//!   0x100  GICD_ISENABLERn  Set-enable     (1 bit per IRQ, 32 IRQs/word)
//!   0x180  GICD_ICENABLERn  Clear-enable   (1 bit per IRQ)
//!   0x400  GICD_IPRIORITYRn Priority       (8 bits per IRQ, 4 IRQs/word)
//!   0x800  GICD_ITARGETSRn  CPU targets    (8 bits per IRQ, 4 IRQs/word)
//!   0xC00  GICD_ICFGRn      Config (edge/level, 2 bits per IRQ)
//!   0xF00  GICD_SGIR        Software Generated Interrupt
//! ```
//! CPU interface (`GICC_*`), relative to the CPU-interface base:
//! ```text
//!   0x000  GICC_CTLR  CPU interface control
//!   0x004  GICC_PMR   Priority mask  (write 0xFF to allow all priorities)
//!   0x008  GICC_BPR   Binary point
//!   0x00C  GICC_IAR   Interrupt Acknowledge (read: ID + CPUID)
//!   0x010  GICC_EOIR  End Of Interrupt      (write the value read from IAR)
//! ```
//! `GICD_SGIR` fields (GICv2 spec, "Software Generated Interrupt Register"):
//! ```text
//!   bits 25:24  TargetListFilter (0b00 = use CPUTargetList, 0b10 = self only)
//!   bits 23:16  CPUTargetList    (1 bit per target CPU, CPU0 = bit16)
//!   bit  15     NSATT
//!   bits 3:0    SGIINTID         (the SGI number, 0..15)
//! ```
//! `GICC_IAR`/`GICC_EOIR` fields:
//! ```text
//!   bits 9:0    Interrupt ID  (0..1019; 1023 = spurious)
//!   bits 12:10  CPUID         (for SGIs: the source CPU)
//! ```

// ---- Distributor register base offsets ----
/// `GICD_CTLR`.
pub const GICD_CTLR: usize = 0x000;
/// `GICD_TYPER`.
pub const GICD_TYPER: usize = 0x004;
/// `GICD_ISENABLERn` base.
pub const GICD_ISENABLER: usize = 0x100;
/// `GICD_ICENABLERn` base.
pub const GICD_ICENABLER: usize = 0x180;
/// `GICD_IPRIORITYRn` base.
pub const GICD_IPRIORITYR: usize = 0x400;
/// `GICD_ITARGETSRn` base.
pub const GICD_ITARGETSR: usize = 0x800;
/// `GICD_ICFGRn` base.
pub const GICD_ICFGR: usize = 0xC00;
/// `GICD_SGIR`.
pub const GICD_SGIR: usize = 0xF00;

// ---- CPU interface register offsets ----
/// `GICC_CTLR`.
pub const GICC_CTLR: usize = 0x000;
/// `GICC_PMR`.
pub const GICC_PMR: usize = 0x004;
/// `GICC_BPR`.
pub const GICC_BPR: usize = 0x008;
/// `GICC_IAR`.
pub const GICC_IAR: usize = 0x00C;
/// `GICC_EOIR`.
pub const GICC_EOIR: usize = 0x010;

/// A word offset plus the bit position of a 1-bit-per-IRQ register
/// (`ISENABLER`/`ICENABLER`/`ISPEND`/...).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BitLocation {
    /// Byte offset of the containing 32-bit word, relative to the dist base.
    pub word_offset: usize,
    /// Bit index (0..31) within that word.
    pub bit: u32,
}

/// A byte offset plus the byte lane of an 8-bit-per-IRQ register
/// (`IPRIORITYR`/`ITARGETSR`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteLocation {
    /// Byte offset of the containing 32-bit word, relative to the dist base.
    pub word_offset: usize,
    /// Byte lane (0..3) within that word.
    pub byte: u32,
}

/// Locate the `ISENABLER` bit for `irq`. 32 IRQs per 4-byte word.
pub const fn isenabler(irq: u32) -> BitLocation {
    BitLocation {
        word_offset: GICD_ISENABLER + ((irq / 32) as usize) * 4,
        bit: irq % 32,
    }
}

/// Locate the `ICENABLER` bit for `irq`.
pub const fn icenabler(irq: u32) -> BitLocation {
    BitLocation {
        word_offset: GICD_ICENABLER + ((irq / 32) as usize) * 4,
        bit: irq % 32,
    }
}

/// Locate the `IPRIORITYR` byte for `irq`. 4 IRQs per 4-byte word; the priority
/// byte lives at the IRQ's own byte lane.
pub const fn ipriorityr(irq: u32) -> ByteLocation {
    ByteLocation {
        word_offset: GICD_IPRIORITYR + ((irq / 4) as usize) * 4,
        byte: irq % 4,
    }
}

/// Locate the `ITARGETSR` byte for `irq` (8-bit CPU-target mask per IRQ).
pub const fn itargetsr(irq: u32) -> ByteLocation {
    ByteLocation {
        word_offset: GICD_ITARGETSR + ((irq / 4) as usize) * 4,
        byte: irq % 4,
    }
}

/// `TargetListFilter` values for `GICD_SGIR`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SgiTarget {
    /// 0b00 — deliver to the CPUs in `cpu_mask` (bit i = CPU i).
    List { cpu_mask: u8 },
    /// 0b01 — deliver to all CPUs except self.
    AllOther,
    /// 0b10 — deliver to self only.
    SelfOnly,
}

/// Compose a `GICD_SGIR` write value for `sgi_id` (0..15) to the given target.
///
/// Panics-free: an out-of-range `sgi_id` is masked to 4 bits (the field width),
/// matching what the hardware register would latch.
pub const fn sgir(sgi_id: u8, target: SgiTarget) -> u32 {
    let intid = (sgi_id as u32) & 0xF;
    match target {
        SgiTarget::List { cpu_mask } => {
            // TargetListFilter=0b00, CPUTargetList at bits[23:16].
            (0b00 << 24) | ((cpu_mask as u32) << 16) | intid
        }
        SgiTarget::AllOther => (0b01 << 24) | intid,
        SgiTarget::SelfOnly => (0b10 << 24) | intid,
    }
}

/// Extract the Interrupt ID (bits[9:0]) from a value read from `GICC_IAR`.
pub const fn iar_interrupt_id(iar: u32) -> u32 {
    iar & 0x3FF
}

/// Extract the source CPUID (bits[12:10]) from a `GICC_IAR` value (meaningful
/// for SGIs).
pub const fn iar_cpuid(iar: u32) -> u32 {
    (iar >> 10) & 0x7
}

/// The GICv2 spurious-interrupt ID (a read of `IAR` that returns this means
/// there was no pending interrupt; do NOT `EOI` it).
pub const SPURIOUS_INTID: u32 = 1023;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isenabler_known_irqs() {
        // IRQ 0 -> word 0x100, bit 0.
        assert_eq!(
            isenabler(0),
            BitLocation {
                word_offset: 0x100,
                bit: 0
            }
        );
        // IRQ 30 (the generic-timer PPI) -> still word 0x100, bit 30.
        assert_eq!(
            isenabler(30),
            BitLocation {
                word_offset: 0x100,
                bit: 30
            }
        );
        // IRQ 32 (first SPI) -> word 0x104, bit 0.
        assert_eq!(
            isenabler(32),
            BitLocation {
                word_offset: 0x104,
                bit: 0
            }
        );
        // IRQ 79 -> 79/32 = 2 => word 0x108, bit 79%32 = 15.
        assert_eq!(
            isenabler(79),
            BitLocation {
                word_offset: 0x108,
                bit: 15
            }
        );
    }

    #[test]
    fn ipriorityr_known_irqs() {
        // IRQ 0 -> 0x400, byte 0.
        assert_eq!(
            ipriorityr(0),
            ByteLocation {
                word_offset: 0x400,
                byte: 0
            }
        );
        // IRQ 30 -> 30/4 = 7 => 0x400 + 28 = 0x41C, byte 30%4 = 2.
        assert_eq!(
            ipriorityr(30),
            ByteLocation {
                word_offset: 0x41C,
                byte: 2
            }
        );
        // IRQ 35 -> 35/4 = 8 => 0x420, byte 3.
        assert_eq!(
            ipriorityr(35),
            ByteLocation {
                word_offset: 0x420,
                byte: 3
            }
        );
    }

    #[test]
    fn itargetsr_known_irq() {
        // IRQ 33 -> 33/4 = 8 => 0x800 + 32 = 0x820, byte 1.
        assert_eq!(
            itargetsr(33),
            ByteLocation {
                word_offset: 0x820,
                byte: 1
            }
        );
    }

    #[test]
    fn sgir_self_ipi_known_value() {
        // Self-SGI #15 (the smoketest IPI): TargetListFilter=0b10, INTID=15.
        // Expected: (0b10 << 24) | 0xF = 0x0200_000F.
        assert_eq!(sgir(15, SgiTarget::SelfOnly), 0x0200_000F);
    }

    #[test]
    fn sgir_targeted_cpu_set_known_value() {
        // SGI #1 to CPUs {0,2}: cpu_mask = 0b0000_0101 = 0x05 at bits[23:16].
        // Expected: (0b00<<24) | (0x05<<16) | 1 = 0x0005_0001.
        assert_eq!(
            sgir(
                1,
                SgiTarget::List {
                    cpu_mask: 0b0000_0101
                }
            ),
            0x0005_0001
        );
    }

    #[test]
    fn sgir_all_other() {
        // SGI #3 to all-other: (0b01<<24) | 3 = 0x0100_0003.
        assert_eq!(sgir(3, SgiTarget::AllOther), 0x0100_0003);
    }

    #[test]
    fn iar_extraction_known_value() {
        // IAR reporting SGI ID 15 from source CPU 2:
        //   ID 15 in bits[9:0] = 0xF ; CPUID 2 in bits[12:10] = 2<<10 = 0x800.
        let iar = 0x800 | 0xF;
        assert_eq!(iar_interrupt_id(iar), 15);
        assert_eq!(iar_cpuid(iar), 2);
        // A larger SPI ID (e.g. 79) with CPUID 0.
        assert_eq!(iar_interrupt_id(79), 79);
        assert_eq!(iar_cpuid(79), 0);
    }

    // ---- FAIL-DEMONSTRATION ----
    #[test]
    fn faildemo_wrong_isenabler_stride_mismatches() {
        // A common bug: indexing ISENABLER as if it were byte-per-IRQ (stride
        // /8 *1) instead of bit-per-IRQ (/32 *4). Show the buggy offset differs
        // from the correct one for IRQ 32 — proving the known-value asserts can
        // catch the stride error.
        let correct = isenabler(32).word_offset; // 0x104
        let buggy = GICD_ISENABLER + (32 / 8); // 0x104? no -> 0x100+4 = 0x104 collides
                                               // Use a clearer divergence: byte-per-IRQ would put IRQ 64 at 0x100+8,
                                               // but the correct word is 0x100 + (64/32)*4 = 0x108.
        let correct64 = isenabler(64).word_offset; // 0x108
        let buggy64 = GICD_ISENABLER + (64 / 8); // 0x108 again — pick IRQ 96
        let correct96 = isenabler(96).word_offset; // 0x100 + 3*4 = 0x10C
        let buggy96 = GICD_ISENABLER + (96 / 8); // 0x100 + 12 = 0x10C — still collides at multiples
                                                 // The robust divergence: bit position. byte-per-IRQ has no bit concept;
                                                 // bit-per-IRQ for IRQ 33 must be bit 1, not bit 0.
        assert_eq!(isenabler(33).bit, 1, "IRQ33 is bit1 of word 0x104");
        let _ = (correct, buggy, correct64, buggy64, correct96, buggy96);
        // Direct FAIL-able divergence on the WORD for IRQ 31 vs 32:
        assert_ne!(
            isenabler(31).word_offset,
            isenabler(32).word_offset,
            "IRQ31 and IRQ32 MUST land in different ISENABLER words"
        );
    }

    #[test]
    fn faildemo_wrong_sgir_filter_field() {
        // If SelfOnly were wrongly encoded with filter 0b00 (List) it would
        // require a cpu_mask and produce a different word — proving the
        // self-IPI known-value assert is FAIL-able.
        let correct = sgir(15, SgiTarget::SelfOnly); // 0x0200_000F
        let buggy = 0x0000_000F; // filter 0b00, empty target list
        assert_ne!(correct, buggy);
    }

    #[test]
    fn faildemo_wrong_iar_mask_changes_id() {
        // Reading the IAR ID with an 8-bit mask instead of 10-bit would drop
        // IDs >= 256. Show ID 300 survives the correct 10-bit mask.
        let iar = 300;
        assert_eq!(iar_interrupt_id(iar), 300);
        assert_ne!(
            iar & 0xFF,
            iar_interrupt_id(iar),
            "8-bit mask would corrupt"
        );
    }
}
