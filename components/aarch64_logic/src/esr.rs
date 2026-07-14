//! `ESR_EL1` exception-syndrome decode (the classes the boot path needs).
//!
//! Pure bit extraction: given a raw 64-bit `ESR_EL1` value (as the AArch64 Sync
//! exception vector would read via `mrs x, esr_el1`), decode the exception
//! class and the per-class ISS fields. NO sysreg access here — the kernel's
//! `arch/aarch64/irq.rs` (spec slice A4) reads `ESR_EL1` and calls this.
//!
//! ## Grounding (ARM Architecture Reference Manual, ARMv8-A — `ESR_EL1`)
//! ```text
//!   bits 31:26  EC   Exception Class
//!   bit  25     IL   Instruction Length (1 = 32-bit trapped instr, 0 = 16-bit)
//!   bits 24:0   ISS  Instruction-Specific Syndrome (EC-dependent)
//! ```
//! Exception Class encodings used here (ARM ARM, "ESR_ELx" EC table):
//! ```text
//!   0x00  Unknown reason
//!   0x15  SVC instruction execution in AArch64 state
//!   0x20  Instruction Abort from a lower Exception level
//!   0x21  Instruction Abort taken without a change in Exception level
//!   0x24  Data Abort from a lower Exception level
//!   0x25  Data Abort taken without a change in Exception level
//! ```
//! Data Abort ISS (ARM ARM, "ISS encoding for an exception from a Data Abort"):
//! ```text
//!   bit  24     ISV   Instruction syndrome valid
//!   bit  9      EA    External abort type
//!   bit  8      CM    Cache maintenance
//!   bit  7      S1PTW Stage-1 walk caused stage-2 abort
//!   bit  6      WnR   Write (1) not Read (0)
//!   bits 5:0    DFSC  Data Fault Status Code
//! ```
//! Instruction Abort ISS uses the same low bits with `IFSC` at bits[5:0].
//! DFSC/IFSC values of interest:
//! ```text
//!   0b000100 (0x04) Translation fault, level 0
//!   0b000101 (0x05) Translation fault, level 1
//!   0b000110 (0x06) Translation fault, level 2
//!   0b000111 (0x07) Translation fault, level 3
//!   0b001001 (0x09) Access flag fault, level 1
//!   0b001111 (0x0F) Permission fault, level 3
//! ```
//! SVC ISS: bits[15:0] = the 16-bit immediate of the `SVC #imm16` instruction.

/// Decoded `ESR_EL1` exception class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExceptionClass {
    /// EC 0x00 — unknown / undefined instruction.
    Unknown,
    /// EC 0x15 — `SVC` (syscall) from AArch64.
    Svc,
    /// EC 0x20/0x21 — instruction abort (lower / same EL).
    InstructionAbort { same_el: bool },
    /// EC 0x24/0x25 — data abort (lower / same EL).
    DataAbort { same_el: bool },
    /// Any EC this boot-path decoder does not special-case.
    Other(u8),
}

/// A fully-decoded `ESR_EL1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Esr {
    /// Raw EC field (bits[31:26]).
    pub ec: u8,
    /// Instruction Length bit (bit[25]).
    pub il_32bit: bool,
    /// Raw ISS field (bits[24:0]).
    pub iss: u32,
    /// The interpreted exception class.
    pub class: ExceptionClass,
}

/// Fault status (the DFSC/IFSC class the page-fault handler needs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultStatus {
    /// Translation fault at the given level (0..=3).
    Translation(u8),
    /// Access-flag fault at the given level.
    AccessFlag(u8),
    /// Permission fault at the given level.
    Permission(u8),
    /// Any other DFSC/IFSC code, raw.
    Other(u8),
}

const EC_SHIFT: u32 = 26;
const EC_MASK: u64 = 0x3F;
const IL_BIT: u64 = 1 << 25;
const ISS_MASK: u64 = 0x01FF_FFFF; // 25 bits

/// Decode the EC/IL/ISS skeleton of an `ESR_EL1` value.
pub fn decode(esr: u64) -> Esr {
    let ec = ((esr >> EC_SHIFT) & EC_MASK) as u8;
    let il_32bit = (esr & IL_BIT) != 0;
    let iss = (esr & ISS_MASK) as u32;
    let class = match ec {
        0x00 => ExceptionClass::Unknown,
        0x15 => ExceptionClass::Svc,
        0x20 => ExceptionClass::InstructionAbort { same_el: false },
        0x21 => ExceptionClass::InstructionAbort { same_el: true },
        0x24 => ExceptionClass::DataAbort { same_el: false },
        0x25 => ExceptionClass::DataAbort { same_el: true },
        other => ExceptionClass::Other(other),
    };
    Esr {
        ec,
        il_32bit,
        iss,
        class,
    }
}

/// For a data abort, extract `WnR` (true = write fault, false = read fault).
/// Returns `None` if the ESR is not a data abort.
pub fn data_abort_is_write(esr: &Esr) -> Option<bool> {
    match esr.class {
        ExceptionClass::DataAbort { .. } => Some((esr.iss >> 6) & 1 == 1),
        _ => None,
    }
}

/// Decode the DFSC/IFSC fault status from a data or instruction abort ISS.
pub fn fault_status(esr: &Esr) -> Option<FaultStatus> {
    match esr.class {
        ExceptionClass::DataAbort { .. } | ExceptionClass::InstructionAbort { .. } => {
            let code = (esr.iss & 0x3F) as u8; // bits[5:0]
            Some(decode_fault_code(code))
        }
        _ => None,
    }
}

fn decode_fault_code(code: u8) -> FaultStatus {
    match code {
        0x04 => FaultStatus::Translation(0),
        0x05 => FaultStatus::Translation(1),
        0x06 => FaultStatus::Translation(2),
        0x07 => FaultStatus::Translation(3),
        0x08 => FaultStatus::AccessFlag(0),
        0x09 => FaultStatus::AccessFlag(1),
        0x0A => FaultStatus::AccessFlag(2),
        0x0B => FaultStatus::AccessFlag(3),
        0x0C => FaultStatus::Permission(0),
        0x0D => FaultStatus::Permission(1),
        0x0E => FaultStatus::Permission(2),
        0x0F => FaultStatus::Permission(3),
        other => FaultStatus::Other(other),
    }
}

/// For an `SVC`, extract the 16-bit immediate (the syscall selector immediate;
/// RaeenOS uses `svc #0` and carries the syscall number in `x8`). Returns
/// `None` if the ESR is not an SVC.
pub fn svc_immediate(esr: &Esr) -> Option<u16> {
    match esr.class {
        ExceptionClass::Svc => Some((esr.iss & 0xFFFF) as u16),
        _ => None,
    }
}

/// Compose a raw `ESR_EL1` from fields — used by tests and by any code that
/// needs to synthesize a syndrome (kept here so the encode/decode are one
/// auditable pair).
pub fn compose(ec: u8, il_32bit: bool, iss: u32) -> u64 {
    let mut v = ((ec as u64) & EC_MASK) << EC_SHIFT;
    if il_32bit {
        v |= IL_BIT;
    }
    v |= (iss as u64) & ISS_MASK;
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_data_abort_write_translation_l2_same_el() {
        // EC=0x25 (data abort, same EL), IL=1, WnR=1 (write), DFSC=0x06
        // (translation fault level 2). ISS = WnR(bit6)=0x40 | DFSC=0x06 = 0x46.
        let iss = 0x40 | 0x06;
        let raw = compose(0x25, true, iss);
        // Cross-check the exact raw word against the documented layout:
        //   EC 0x25 << 26 = 0x9400_0000 ; IL bit25 = 0x0200_0000 ; ISS 0x46.
        assert_eq!(raw, 0x9400_0000 | 0x0200_0000 | 0x46);
        let e = decode(raw);
        assert_eq!(e.class, ExceptionClass::DataAbort { same_el: true });
        assert_eq!(e.ec, 0x25);
        assert!(e.il_32bit);
        assert_eq!(data_abort_is_write(&e), Some(true));
        assert_eq!(fault_status(&e), Some(FaultStatus::Translation(2)));
    }

    #[test]
    fn decode_data_abort_read_permission_l3_lower_el() {
        // EC=0x24 (data abort, lower EL), WnR=0 (read), DFSC=0x0F (permission L3).
        let iss = 0x0F; // WnR bit6 clear
        let raw = compose(0x24, true, iss);
        let e = decode(raw);
        assert_eq!(e.class, ExceptionClass::DataAbort { same_el: false });
        assert_eq!(data_abort_is_write(&e), Some(false));
        assert_eq!(fault_status(&e), Some(FaultStatus::Permission(3)));
    }

    #[test]
    fn decode_svc_zero() {
        // `svc #0`: EC=0x15, IL=1, ISS imm16 = 0.
        let raw = compose(0x15, true, 0);
        assert_eq!(raw, 0x15u64 << 26 | (1 << 25));
        let e = decode(raw);
        assert_eq!(e.class, ExceptionClass::Svc);
        assert_eq!(svc_immediate(&e), Some(0));
        // Not a data abort => no WnR / fault status.
        assert_eq!(data_abort_is_write(&e), None);
        assert_eq!(fault_status(&e), None);
    }

    #[test]
    fn decode_svc_nonzero_immediate() {
        let raw = compose(0x15, true, 0x1234);
        let e = decode(raw);
        assert_eq!(svc_immediate(&e), Some(0x1234));
    }

    #[test]
    fn decode_instruction_abort() {
        // EC=0x21 (instruction abort same EL), IFSC=0x05 (translation L1).
        let raw = compose(0x21, true, 0x05);
        let e = decode(raw);
        assert_eq!(e.class, ExceptionClass::InstructionAbort { same_el: true });
        assert_eq!(fault_status(&e), Some(FaultStatus::Translation(1)));
        // Instruction aborts have no WnR.
        assert_eq!(data_abort_is_write(&e), None);
    }

    #[test]
    fn decode_unknown() {
        let raw = compose(0x00, false, 0);
        let e = decode(raw);
        assert_eq!(e.class, ExceptionClass::Unknown);
        assert!(!e.il_32bit);
    }

    // ---- FAIL-DEMONSTRATION ----
    #[test]
    fn faildemo_wrong_ec_shift_changes_class() {
        // If the decoder used the WRONG EC shift (e.g. >>25 instead of >>26),
        // a data-abort syndrome would decode to a different EC. We prove the
        // assert can fail by showing the misread value differs.
        let raw = compose(0x25, true, 0x46);
        let correct_ec = ((raw >> 26) & 0x3F) as u8;
        let buggy_ec = ((raw >> 25) & 0x3F) as u8; // off-by-one shift
        assert_eq!(correct_ec, 0x25);
        assert_ne!(
            correct_ec, buggy_ec,
            "a wrong EC shift must misdecode — proves the EC assert is FAIL-able"
        );
    }

    #[test]
    fn faildemo_wrong_wnr_bit_changes_result() {
        // Reading WnR at the wrong bit (bit 7 instead of bit 6) flips the
        // write/read verdict for this syndrome — the WnR assert is FAIL-able.
        let raw = compose(0x25, true, 0x40); // WnR=1 at bit6
        let e = decode(raw);
        let correct = (e.iss >> 6) & 1; // = 1 (write)
        let buggy = (e.iss >> 7) & 1; // = 0 (misread)
        assert_ne!(correct, buggy);
        assert_eq!(data_abort_is_write(&e), Some(true));
    }
}
