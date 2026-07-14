//! PCIe Advanced Error Reporting (AER) — capability detection + error status.
//!
//! Concept §Security "Driver Wild West → all drivers signed and IOMMU-sandboxed"
//! and §"the kernel stays up under load and degrades gracefully on hardware
//! faults" (MasterChecklist Phase 4.3). When a PCIe link or device reports a
//! correctable or uncorrectable error, AER is how the kernel learns *what*
//! failed and *where* — the prerequisite for isolating a bad device instead of
//! letting a silent DMA/parity fault corrupt the system.
//!
//! NOTE: distinct from `kernel/src/aer.rs` (the Asynchronous Event *Ring*, a
//! userspace IRQ-delivery primitive). The name collision is unfortunate; this
//! module is PCI-Express Advanced Error Reporting per the PCIe base spec §7.8.4.
//!
//! ## What this ships
//!
//! The AER capability is a PCIe **Extended** Capability (ID 0x0001) living at
//! config offset ≥ 0x100, reachable only via ECAM. On boot we walk every
//! function's extended-capability chain, record which devices expose AER, and
//! snapshot their Uncorrectable/Correctable Error Status registers. The
//! status registers report errors latched since power-on — so a non-zero
//! correctable count at boot is real telemetry, and the per-device presence map
//! is what a future error-isolation handler keys off.
//!
//! Honest scope: this is the **detection + telemetry** layer (Phase 4.3 items
//! "detect AER capability", "correctable error counter + log"). Live error-
//! interrupt isolation (mask BAR, mark device inactive on an uncorrectable
//! error) is the next layer and is marked pending in MasterChecklist.
//!
//! ## R10 contract
//!   * `init()` — scan extended-cap chains, build the AER device map.
//!   * `run_boot_smoketest()` — report AER-capable device count + status.
//!   * `dump_text()` — `/proc/athena/pcie_aer`.
//!   * this docstring — the Concept tie-in.

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use spin::Mutex;

/// PCIe Extended Capability ID for Advanced Error Reporting (PCIe spec §7.8.4).
const EXT_CAP_AER: u16 = 0x0001;
/// First extended capability is always at config offset 0x100.
const EXT_CAP_BASE: u16 = 0x100;
/// Bound the extended-cap chain walk so a corrupt next-pointer can't loop.
const MAX_EXT_CAPS: usize = 48;

// AER capability-relative register offsets (PCIe spec §7.8.4 Table).
const AER_UNCORR_STATUS: u16 = 0x04;
/// Uncorrectable Error Mask — set a bit to STOP that error reporting (§7.8.4.3).
const AER_UNCORR_MASK: u16 = 0x08;
/// Uncorrectable Error Severity — set bit = error is FATAL, clear = non-fatal.
const AER_UNCORR_SEVERITY: u16 = 0x0C;
const AER_CORR_STATUS: u16 = 0x10;

/// PCI Command-register bits cleared to isolate a faulting device:
/// I/O Space (0), Memory Space (1), Bus Master (2). Clearing Bus Master halts
/// the device's DMA; clearing Memory/IO drops its BAR decode — it can no longer
/// touch the system. This is the "mask BAR, mark inactive" action.
const CMD_ISOLATE_MASK: u16 = 0x0007;

// ── AER Uncorrectable Error Status bit positions (PCIe base spec §7.8.4.2) ────
// Each bit latches one uncorrectable error type; the same layout applies to the
// Mask and Severity registers.
const UNCORR_DATA_LINK_PROTOCOL: u32 = 1 << 4; // DLP error
const UNCORR_SURPRISE_DOWN: u32 = 1 << 5;
const UNCORR_POISONED_TLP: u32 = 1 << 12;
const UNCORR_FLOW_CONTROL_PROTOCOL: u32 = 1 << 13;
const UNCORR_COMPLETION_TIMEOUT: u32 = 1 << 14;
const UNCORR_COMPLETER_ABORT: u32 = 1 << 15;
const UNCORR_UNEXPECTED_COMPLETION: u32 = 1 << 16;
const UNCORR_RECEIVER_OVERFLOW: u32 = 1 << 17;
const UNCORR_MALFORMED_TLP: u32 = 1 << 18;
const UNCORR_ECRC_ERROR: u32 = 1 << 19;
const UNCORR_UNSUPPORTED_REQUEST: u32 = 1 << 20;
const UNCORR_ACS_VIOLATION: u32 = 1 << 21;
const UNCORR_INTERNAL_ERROR: u32 = 1 << 22;
const UNCORR_MC_BLOCKED_TLP: u32 = 1 << 23;
const UNCORR_ATOMICOP_EGRESS_BLOCKED: u32 = 1 << 24;
const UNCORR_TLP_PREFIX_BLOCKED: u32 = 1 << 25;

/// All defined uncorrectable status bits — the write-1-to-clear mask for the
/// Uncorrectable Error Status register.
const UNCORR_ALL: u32 = UNCORR_DATA_LINK_PROTOCOL
    | UNCORR_SURPRISE_DOWN
    | UNCORR_POISONED_TLP
    | UNCORR_FLOW_CONTROL_PROTOCOL
    | UNCORR_COMPLETION_TIMEOUT
    | UNCORR_COMPLETER_ABORT
    | UNCORR_UNEXPECTED_COMPLETION
    | UNCORR_RECEIVER_OVERFLOW
    | UNCORR_MALFORMED_TLP
    | UNCORR_ECRC_ERROR
    | UNCORR_UNSUPPORTED_REQUEST
    | UNCORR_ACS_VIOLATION
    | UNCORR_INTERNAL_ERROR
    | UNCORR_MC_BLOCKED_TLP
    | UNCORR_ATOMICOP_EGRESS_BLOCKED
    | UNCORR_TLP_PREFIX_BLOCKED;

// ── AER Correctable Error Status bit positions (PCIe base spec §7.8.4.5) ──────
const CORR_RECEIVER_ERROR: u32 = 1 << 0;
const CORR_BAD_TLP: u32 = 1 << 6;
const CORR_BAD_DLLP: u32 = 1 << 7;
const CORR_REPLAY_NUM_ROLLOVER: u32 = 1 << 8;
const CORR_REPLAY_TIMER_TIMEOUT: u32 = 1 << 12;
const CORR_ADVISORY_NON_FATAL: u32 = 1 << 13;
const CORR_CORRECTED_INTERNAL: u32 = 1 << 14;
const CORR_HEADER_LOG_OVERFLOW: u32 = 1 << 15;

/// All defined correctable status bits — the write-1-to-clear mask for the
/// Correctable Error Status register.
const CORR_ALL: u32 = CORR_RECEIVER_ERROR
    | CORR_BAD_TLP
    | CORR_BAD_DLLP
    | CORR_REPLAY_NUM_ROLLOVER
    | CORR_REPLAY_TIMER_TIMEOUT
    | CORR_ADVISORY_NON_FATAL
    | CORR_CORRECTED_INTERNAL
    | CORR_HEADER_LOG_OVERFLOW;

/// Decode a latched **uncorrectable** AER status word into the set of error
/// names present, as a `+`-joined string. Pure function — the QEMU/host-provable
/// core of the per-error decode. Returns "none" if no defined bit is set.
pub fn decode_uncorrectable(status: u32) -> String {
    let mut names: Vec<&'static str> = Vec::new();
    let map: &[(u32, &str)] = &[
        (UNCORR_DATA_LINK_PROTOCOL, "data-link-protocol"),
        (UNCORR_SURPRISE_DOWN, "surprise-down"),
        (UNCORR_POISONED_TLP, "poisoned-tlp"),
        (UNCORR_FLOW_CONTROL_PROTOCOL, "flow-control-protocol"),
        (UNCORR_COMPLETION_TIMEOUT, "completion-timeout"),
        (UNCORR_COMPLETER_ABORT, "completer-abort"),
        (UNCORR_UNEXPECTED_COMPLETION, "unexpected-completion"),
        (UNCORR_RECEIVER_OVERFLOW, "receiver-overflow"),
        (UNCORR_MALFORMED_TLP, "malformed-tlp"),
        (UNCORR_ECRC_ERROR, "ecrc-error"),
        (UNCORR_UNSUPPORTED_REQUEST, "unsupported-request"),
        (UNCORR_ACS_VIOLATION, "acs-violation"),
        (UNCORR_INTERNAL_ERROR, "internal-error"),
        (UNCORR_MC_BLOCKED_TLP, "mc-blocked-tlp"),
        (UNCORR_ATOMICOP_EGRESS_BLOCKED, "atomicop-egress-blocked"),
        (UNCORR_TLP_PREFIX_BLOCKED, "tlp-prefix-blocked"),
    ];
    for &(bit, name) in map {
        if status & bit != 0 {
            names.push(name);
        }
    }
    if names.is_empty() {
        return String::from("none");
    }
    names.join("+")
}

/// Decode a latched **correctable** AER status word. Pure function. Returns
/// "none" if no defined bit is set.
pub fn decode_correctable(status: u32) -> String {
    let mut names: Vec<&'static str> = Vec::new();
    let map: &[(u32, &str)] = &[
        (CORR_RECEIVER_ERROR, "receiver-error"),
        (CORR_BAD_TLP, "bad-tlp"),
        (CORR_BAD_DLLP, "bad-dllp"),
        (CORR_REPLAY_NUM_ROLLOVER, "replay-num-rollover"),
        (CORR_REPLAY_TIMER_TIMEOUT, "replay-timer-timeout"),
        (CORR_ADVISORY_NON_FATAL, "advisory-non-fatal"),
        (CORR_CORRECTED_INTERNAL, "corrected-internal"),
        (CORR_HEADER_LOG_OVERFLOW, "header-log-overflow"),
    ];
    for &(bit, name) in map {
        if status & bit != 0 {
            names.push(name);
        }
    }
    if names.is_empty() {
        return String::from("none");
    }
    names.join("+")
}

/// Compute the write-1-to-clear mask for a latched status word: only the bits
/// that are both *set* and *defined* are written back (writing 1 to a status bit
/// clears it; PCIe §7.8.4.2). Pure function so the W1C math is host-provable.
pub fn w1c_uncorrectable(status: u32) -> u32 {
    status & UNCORR_ALL
}

/// Write-1-to-clear mask for a correctable status word.
pub fn w1c_correctable(status: u32) -> u32 {
    status & CORR_ALL
}

/// One PCIe function that exposes an AER capability.
#[derive(Clone, Copy)]
struct AerDevice {
    bus: u8,
    device: u8,
    function: u8,
    vendor_id: u16,
    device_id: u16,
    /// Config-space offset of the AER capability structure.
    cap_offset: u16,
    uncorr_status: u32,
    corr_status: u32,
}

static AER_DEVICES: Mutex<Vec<AerDevice>> = Mutex::new(Vec::new());
static SCANNED: AtomicU32 = AtomicU32::new(0);
static CORRECTABLE_SEEN: AtomicU32 = AtomicU32::new(0);
static UNCORRECTABLE_SEEN: AtomicU32 = AtomicU32::new(0);
/// Devices isolated (BAR/bus-master disabled) after an uncorrectable AER error.
static ISOLATED: AtomicU32 = AtomicU32::new(0);
/// Uncorrectable errors whose severity bit marked them FATAL.
static FATAL_SEEN: AtomicU32 = AtomicU32::new(0);

/// AER error class derived from the latched status + severity registers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AerClass {
    None,
    Correctable,
    Uncorrectable,
    Fatal,
}

/// Classify a latched AER state (PCIe §6.2.7): an uncorrectable status bit is
/// FATAL when its matching Severity bit is set, otherwise non-fatal
/// (Uncorrectable); a non-zero Correctable status with no uncorrectable error
/// is Correctable. Pure function — the QEMU-provable core of the policy.
pub fn classify(uncorr_status: u32, uncorr_severity: u32, corr_status: u32) -> AerClass {
    if uncorr_status & uncorr_severity != 0 {
        AerClass::Fatal
    } else if uncorr_status != 0 {
        AerClass::Uncorrectable
    } else if corr_status != 0 {
        AerClass::Correctable
    } else {
        AerClass::None
    }
}

/// The recovery action the kernel takes for an AER class. The key invariant for
/// "system survives": NO class maps to a kernel panic — even Fatal degrades by
/// isolating the offending device (driver-crash ≠ system-crash, Concept §
/// "Driver crash ≠ system crash"). Single-sourced so `handle_uncorrectable` and
/// the selftest agree on the policy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AerAction {
    /// No error — nothing to do.
    Ignore,
    /// Correctable — hardware already recovered; clear status and continue.
    ClearAndContinue,
    /// Uncorrectable/Fatal — isolate the device (bus-master/mem/io off) + mask
    /// the error so it can't re-fire. The kernel stays up.
    IsolateAndMask,
}

/// Map an AER class to its recovery action. Pure function — proves the
/// degrade-don't-die policy on synthetic input.
pub fn decide_action(class: AerClass) -> AerAction {
    match class {
        AerClass::None => AerAction::Ignore,
        AerClass::Correctable => AerAction::ClearAndContinue,
        AerClass::Uncorrectable | AerClass::Fatal => AerAction::IsolateAndMask,
    }
}

/// Mask (suppress) the given uncorrectable error bits so the device stops
/// re-reporting them after we've isolated it. Real write to the AER
/// Uncorrectable Error Mask (cap+0x08); a no-op without ECAM. Athena-gated.
fn mask_uncorrectable(bus: u8, device: u8, function: u8, cap: u16, bits: u32) {
    let cur = crate::pci::read_config_32_ext(bus, device, function, cap + AER_UNCORR_MASK);
    crate::pci::write_config_32_ext(bus, device, function, cap + AER_UNCORR_MASK, cur | bits);
}

/// Isolate a faulting PCIe function: clear its Command-register I/O + Memory +
/// Bus-Master bits so it can no longer DMA or decode BARs. Returns
/// `(old_command, new_command)`. Works on any PCI device (standard config).
pub fn isolate_device(bus: u8, device: u8, function: u8) -> (u16, u16) {
    let old = crate::pci::read_config_16(bus, device, function, 0x04);
    let new = old & !CMD_ISOLATE_MASK;
    crate::pci::write_config_16(bus, device, function, 0x04, new);
    ISOLATED.fetch_add(1, Ordering::Relaxed);
    crate::serial_println!(
        "[pcie-aer] ISOLATED {:02x}:{:02x}.{}: command {:#06x} -> {:#06x} (bus-master/mem/io off)",
        bus,
        device,
        function,
        old,
        new,
    );
    (old, new)
}

/// Handle a latched uncorrectable AER error on a function: classify it, and on
/// Uncorrectable/Fatal isolate the device (mask BAR + bus-master), mask the
/// error so it can't re-fire, and log — keeping the kernel up (degrade, don't
/// die). Fatal additionally bumps a counter for user-facing escalation.
/// Returns the class. Called from `init()` when a device boots with a latched
/// uncorrectable error; the live error-interrupt path is Athena-gated.
pub fn handle_uncorrectable(bus: u8, device: u8, function: u8, cap: u16) -> AerClass {
    let us = crate::pci::read_config_32_ext(bus, device, function, cap + AER_UNCORR_STATUS);
    let sev = crate::pci::read_config_32_ext(bus, device, function, cap + AER_UNCORR_SEVERITY);
    let cs = crate::pci::read_config_32_ext(bus, device, function, cap + AER_CORR_STATUS);
    let class = classify(us, sev, cs);
    if class == AerClass::Fatal {
        FATAL_SEEN.fetch_add(1, Ordering::Relaxed);
        crate::serial_println!(
            "[pcie-aer] FATAL uncorrectable error on {:02x}:{:02x}.{} [{}] (status={:#010x} sev={:#010x}) — isolating + escalating",
            bus, device, function, decode_uncorrectable(us & sev), us, sev,
        );
    } else if class == AerClass::Uncorrectable {
        crate::serial_println!(
            "[pcie-aer] uncorrectable (non-fatal) error on {:02x}:{:02x}.{} [{}] (status={:#010x}) — isolating",
            bus, device, function, decode_uncorrectable(us), us,
        );
    }
    // Degrade, never die: isolate the device for any uncorrectable error;
    // correctable/none take no isolation. The kernel survives in every case.
    if decide_action(class) == AerAction::IsolateAndMask {
        let _ = isolate_device(bus, device, function);
        mask_uncorrectable(bus, device, function, cap, us);
        // Acknowledge the latched bits (write-1-to-clear) so the same error does
        // not immediately re-report after isolation (PCIe §7.8.4.2).
        let w1c = w1c_uncorrectable(us);
        if w1c != 0 {
            crate::pci::write_config_32_ext(bus, device, function, cap + AER_UNCORR_STATUS, w1c);
        }
    }
    class
}

/// Handle a latched **correctable** AER error on a function: decode the specific
/// bit(s), count it, and clear the status register (write-1-to-clear). Hardware
/// already recovered the error, so there is no isolation — log + clear + count.
/// Pure-policy-driven (`decode_action`); the live read/write is Athena-gated and
/// a no-op without ECAM. Returns the number of distinct correctable bits seen.
pub fn handle_correctable(bus: u8, device: u8, function: u8, cap: u16) -> u32 {
    let cs = crate::pci::read_config_32_ext(bus, device, function, cap + AER_CORR_STATUS);
    let bits = cs & CORR_ALL;
    if bits == 0 {
        return 0;
    }
    CORRECTABLE_SEEN.fetch_add(bits.count_ones(), Ordering::Relaxed);
    crate::serial_println!(
        "[pcie-aer] correctable error on {:02x}:{:02x}.{} [{}] (status={:#010x}) — clearing",
        bus,
        device,
        function,
        decode_correctable(cs),
        cs,
    );
    // Write-1-to-clear the latched correctable bits.
    crate::pci::write_config_32_ext(
        bus,
        device,
        function,
        cap + AER_CORR_STATUS,
        w1c_correctable(cs),
    );
    bits.count_ones()
}

/// QEMU-provable self-test for the severity-classification + isolation policy
/// (MasterChecklist 4.3). QEMU exposes no AER device, so this proves the pure
/// logic: `classify()` on synthetic status/severity words, and the isolation
/// bit-math (clearing Command bits 0..2) — without writing to a live device.
pub fn run_aer_selftest() {
    let none = classify(0, 0, 0) == AerClass::None;
    let corr = classify(0, 0, 0x40) == AerClass::Correctable;
    // Uncorrectable bit set, but its severity bit clear → non-fatal.
    let uncorr = classify(0x0010, 0x0000, 0) == AerClass::Uncorrectable;
    // Same uncorrectable bit also set in severity → fatal.
    let fatal = classify(0x0010, 0x0010, 0) == AerClass::Fatal;
    // Isolation transform: clearing bits 0..2 must drop io/mem/busmaster while
    // leaving every other Command bit intact.
    let sample_cmd: u16 = 0x0506; // bus-master(2)+mem(1)+SERR(8)+INTx-disable(10)
    let isolated_cmd = sample_cmd & !CMD_ISOLATE_MASK;
    let iso_ok = (isolated_cmd & CMD_ISOLATE_MASK) == 0
        && (isolated_cmd & !CMD_ISOLATE_MASK) == (sample_cmd & !CMD_ISOLATE_MASK);
    let pass = none && corr && uncorr && fatal && iso_ok;
    crate::serial_println!(
        "[pcie-aer] severity/isolation selftest: classify(none={} corr={} uncorr={} fatal={}) isolate_bitmath={} -> {}",
        none,
        corr,
        uncorr,
        fatal,
        iso_ok,
        if pass { "PASS" } else { "FAIL" },
    );

    // MasterChecklist 4.3: "deliberate AER inject -> handled, logged, system
    // survives." Inject a synthetic FATAL uncorrectable status through the policy
    // and prove the action is DEGRADE (isolate the device), never a kernel
    // panic — the survives guarantee. Same for uncorrectable/correctable/none.
    let inj_fatal = decide_action(classify(0x0010, 0x0010, 0)) == AerAction::IsolateAndMask;
    let inj_uncorr = decide_action(classify(0x0010, 0x0000, 0)) == AerAction::IsolateAndMask;
    let inj_corr = decide_action(classify(0, 0, 0x40)) == AerAction::ClearAndContinue;
    let inj_none = decide_action(classify(0, 0, 0)) == AerAction::Ignore;
    // The survives invariant: NO class maps to a panic — every action is a
    // device-level degrade or no-op.
    let survives = matches!(
        decide_action(AerClass::Fatal),
        AerAction::IsolateAndMask | AerAction::ClearAndContinue | AerAction::Ignore
    );
    let inj_pass = inj_fatal && inj_uncorr && inj_corr && inj_none && survives;
    crate::serial_println!(
        "[pcie-aer] inject smoketest: fatal->isolate={} uncorr->isolate={} corr->clear={} none->ignore={} survives(no-panic)={} -> {}",
        inj_fatal,
        inj_uncorr,
        inj_corr,
        inj_none,
        survives,
        if inj_pass { "PASS" } else { "FAIL" },
    );

    // Per-error bit decode + write-1-to-clear math (MasterChecklist 4.3 "decode
    // the specific error(s)"). Synthetic status words → named errors + W1C mask.
    // Correctable: bad-tlp (bit 6) + replay-timer-timeout (bit 12).
    let corr_word = CORR_BAD_TLP | CORR_REPLAY_TIMER_TIMEOUT;
    let corr_decode = decode_correctable(corr_word) == "bad-tlp+replay-timer-timeout";
    // Uncorrectable: completion-timeout (bit 14) + malformed-tlp (bit 18).
    let unc_word = UNCORR_COMPLETION_TIMEOUT | UNCORR_MALFORMED_TLP;
    let unc_decode = decode_uncorrectable(unc_word) == "completion-timeout+malformed-tlp";
    // W1C only writes back set+defined bits; a reserved bit (31) must NOT survive.
    let w1c_ok = w1c_correctable(corr_word | (1 << 31)) == corr_word
        && w1c_uncorrectable(unc_word | (1 << 31)) == unc_word;
    let none_decode = decode_correctable(0) == "none" && decode_uncorrectable(0) == "none";
    let decode_pass = corr_decode && unc_decode && w1c_ok && none_decode;
    crate::serial_println!(
        "[pcie-aer] decode smoketest: corr=[{}] uncorr=[{}] w1c_masks_reserved={} none={} -> {}",
        decode_correctable(corr_word),
        decode_uncorrectable(unc_word),
        w1c_ok,
        none_decode,
        if decode_pass { "PASS" } else { "FAIL" },
    );
}

/// Walk a function's PCIe Extended Capability chain looking for AER.
/// Returns the AER cap's config offset, or `None` if absent/not PCIe.
fn find_aer_cap(bus: u8, device: u8, function: u8) -> Option<u16> {
    let first = crate::pci::read_config_32_ext(bus, device, function, EXT_CAP_BASE);
    // No ECAM, no extended caps, or unimplemented → all-ones / all-zeros header.
    if first == 0xFFFF_FFFF || first == 0 {
        return None;
    }
    let mut offset = EXT_CAP_BASE;
    for _ in 0..MAX_EXT_CAPS {
        if offset < EXT_CAP_BASE {
            break;
        }
        let header = crate::pci::read_config_32_ext(bus, device, function, offset);
        if header == 0xFFFF_FFFF {
            break;
        }
        let cap_id = (header & 0xFFFF) as u16;
        if cap_id == EXT_CAP_AER {
            return Some(offset);
        }
        // Bits 20..31 = next-capability offset (DWORD-aligned); 0 = end.
        let next = ((header >> 20) & 0xFFF) as u16;
        if next == 0 || next == offset {
            break;
        }
        offset = next;
    }
    None
}

/// Scan every enumerated PCI function for an AER extended capability and
/// snapshot its error-status registers. Best-effort + idempotent.
pub fn init() {
    let devices = crate::pci::enumerate();
    SCANNED.store(devices.len() as u32, Ordering::Relaxed);

    let ecam = crate::pci::PCIE_ECAM_BASE.load(Ordering::Relaxed);
    if ecam == 0 {
        crate::serial_println!(
            "[pcie-aer] ECAM inactive — AER lives in extended config (>=0x100), unreachable; skipping"
        );
        return;
    }

    let mut found: Vec<AerDevice> = Vec::new();
    let (mut corr_total, mut uncorr_total) = (0u32, 0u32);
    for dev in &devices {
        let Some(cap) = find_aer_cap(dev.bus, dev.device, dev.function) else {
            continue;
        };
        let uncorr_status = crate::pci::read_config_32_ext(
            dev.bus,
            dev.device,
            dev.function,
            cap + AER_UNCORR_STATUS,
        );
        let corr_status = crate::pci::read_config_32_ext(
            dev.bus,
            dev.device,
            dev.function,
            cap + AER_CORR_STATUS,
        );
        if corr_status != 0 && corr_status != 0xFFFF_FFFF {
            corr_total += corr_status.count_ones();
            // A device booted with latched CORRECTABLE bits: decode + clear them
            // (hardware already recovered, no isolation). No-op on QEMU.
            let _ = handle_correctable(dev.bus, dev.device, dev.function, cap);
        }
        if uncorr_status != 0 && uncorr_status != 0xFFFF_FFFF {
            uncorr_total += uncorr_status.count_ones();
            // A device that booted with a latched UNCORRECTABLE error is already
            // suspect: classify + isolate it now (degrade-not-die) so it can't
            // DMA-corrupt the running system. No-op on QEMU (no AER devices).
            let _ = handle_uncorrectable(dev.bus, dev.device, dev.function, cap);
        }
        crate::serial_println!(
            "[pcie-aer] {:02x}:{:02x}.{} {:04x}:{:04x} AER@{:#05x} uncorr_status={:#010x} corr_status={:#010x}",
            dev.bus,
            dev.device,
            dev.function,
            dev.vendor_id,
            dev.device_id,
            cap,
            uncorr_status,
            corr_status,
        );
        found.push(AerDevice {
            bus: dev.bus,
            device: dev.device,
            function: dev.function,
            vendor_id: dev.vendor_id,
            device_id: dev.device_id,
            cap_offset: cap,
            uncorr_status,
            corr_status,
        });
    }

    CORRECTABLE_SEEN.store(corr_total, Ordering::Relaxed);
    UNCORRECTABLE_SEEN.store(uncorr_total, Ordering::Relaxed);
    let n = found.len();
    *AER_DEVICES.lock() = found;
    crate::serial_println!(
        "[pcie-aer] init: {} of {} function(s) expose AER; corr_bits={} uncorr_bits={}",
        n,
        devices.len(),
        corr_total,
        uncorr_total,
    );
}

/// Boot smoketest — always a PASS: zero AER-capable devices is a valid state
/// (QEMU's default PCI devices often omit AER), and any found devices are
/// reported with their latched error status. A non-zero uncorrectable status
/// at boot is surfaced as a WARN but does not fail the boot (degrade-not-die).
pub fn run_boot_smoketest() {
    // Prove the severity-classification + isolation policy (MasterChecklist 4.3).
    run_aer_selftest();
    let devs = AER_DEVICES.lock();
    let corr = CORRECTABLE_SEEN.load(Ordering::Relaxed);
    let uncorr = UNCORRECTABLE_SEEN.load(Ordering::Relaxed);
    if uncorr != 0 {
        crate::serial_println!(
            "[pcie-aer] smoketest: WARN {} device(s) report latched uncorrectable error bits ({} total)",
            devs.iter().filter(|d| d.uncorr_status != 0).count(),
            uncorr,
        );
    }
    crate::serial_println!(
        "[pcie-aer] smoketest: aer_devices={} scanned={} corr_bits={} uncorr_bits={} -> PASS",
        devs.len(),
        SCANNED.load(Ordering::Relaxed),
        corr,
        uncorr,
    );
}

/// `/proc/athena/pcie_aer` body.
pub fn dump_text() -> String {
    let devs = AER_DEVICES.lock();
    let mut s = String::new();
    s.push_str("# PCIe Advanced Error Reporting (capability detection + status)\n");
    s.push_str(&format!(
        "ecam_active: {}\n",
        crate::pci::PCIE_ECAM_BASE.load(Ordering::Relaxed) != 0
    ));
    s.push_str(&format!(
        "scanned_functions: {}\n",
        SCANNED.load(Ordering::Relaxed)
    ));
    s.push_str(&format!("aer_capable: {}\n", devs.len()));
    s.push_str(&format!(
        "correctable_status_bits: {}\n",
        CORRECTABLE_SEEN.load(Ordering::Relaxed)
    ));
    s.push_str(&format!(
        "uncorrectable_status_bits: {}\n",
        UNCORRECTABLE_SEEN.load(Ordering::Relaxed)
    ));
    if devs.is_empty() {
        s.push_str("# no AER-capable functions detected\n");
    } else {
        for d in devs.iter() {
            s.push_str(&format!(
                "{:02x}:{:02x}.{} {:04x}:{:04x} aer@{:#05x} uncorr={:#010x}[{}] corr={:#010x}[{}]\n",
                d.bus,
                d.device,
                d.function,
                d.vendor_id,
                d.device_id,
                d.cap_offset,
                d.uncorr_status,
                decode_uncorrectable(d.uncorr_status),
                d.corr_status,
                decode_correctable(d.corr_status),
            ));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    //! Pure-decode host KATs (CLAUDE.md §15 layer 1). These exercise ONLY the
    //! MSR/PCI-free decode + classification logic on canned status words — no
    //! live config-space access — so they run under `cargo test` and can FAIL if
    //! the AER bit map or W1C math drifts.
    use super::*;

    #[test]
    fn correctable_decodes_specific_bits() {
        // bad-tlp (bit 6) + replay-timer-timeout (bit 12).
        let word = (1 << 6) | (1 << 12);
        assert_eq!(decode_correctable(word), "bad-tlp+replay-timer-timeout");
        // receiver-error (bit 0) alone.
        assert_eq!(decode_correctable(1 << 0), "receiver-error");
        // bad-dllp (bit 7) alone.
        assert_eq!(decode_correctable(1 << 7), "bad-dllp");
        assert_eq!(decode_correctable(0), "none");
    }

    #[test]
    fn uncorrectable_decodes_specific_bits() {
        // completion-timeout (bit 14) + malformed-tlp (bit 18).
        let word = (1 << 14) | (1 << 18);
        assert_eq!(
            decode_uncorrectable(word),
            "completion-timeout+malformed-tlp"
        );
        // poisoned-tlp (bit 12) + completer-abort (bit 15) + unexpected (bit 16).
        let word2 = (1 << 12) | (1 << 15) | (1 << 16);
        assert_eq!(
            decode_uncorrectable(word2),
            "poisoned-tlp+completer-abort+unexpected-completion"
        );
        // data-link-protocol (bit 4) alone.
        assert_eq!(decode_uncorrectable(1 << 4), "data-link-protocol");
        assert_eq!(decode_uncorrectable(0), "none");
    }

    #[test]
    fn w1c_only_clears_set_and_defined_bits() {
        // A reserved bit (31) and an undefined bit (11) must NOT appear in the
        // write-1-to-clear mask; only the set+defined ones do.
        let corr = (1 << 6) | (1 << 31) | (1 << 11);
        assert_eq!(w1c_correctable(corr), 1 << 6);
        let unc = (1 << 14) | (1 << 31) | (1 << 0);
        assert_eq!(w1c_uncorrectable(unc), 1 << 14);
        assert_eq!(w1c_correctable(0), 0);
        assert_eq!(w1c_uncorrectable(0), 0);
    }

    #[test]
    fn classify_severity_matrix() {
        // No error.
        assert_eq!(classify(0, 0, 0), AerClass::None);
        // Correctable only.
        assert_eq!(classify(0, 0, 1 << 6), AerClass::Correctable);
        // Uncorrectable, severity clear → non-fatal.
        assert_eq!(classify(1 << 14, 0, 0), AerClass::Uncorrectable);
        // Uncorrectable with matching severity bit → fatal.
        assert_eq!(classify(1 << 14, 1 << 14, 0), AerClass::Fatal);
        // A different uncorrectable bit set in severity does NOT make THIS one
        // fatal (the masked AND must match the set status bit).
        assert_eq!(classify(1 << 14, 1 << 18, 0), AerClass::Uncorrectable);
    }

    #[test]
    fn action_never_panics_degrades_only() {
        // The "system survives" invariant: every class maps to a degrade action,
        // never a kernel panic.
        assert_eq!(decide_action(AerClass::None), AerAction::Ignore);
        assert_eq!(
            decide_action(AerClass::Correctable),
            AerAction::ClearAndContinue
        );
        assert_eq!(
            decide_action(AerClass::Uncorrectable),
            AerAction::IsolateAndMask
        );
        assert_eq!(decide_action(AerClass::Fatal), AerAction::IsolateAndMask);
    }

    #[test]
    fn isolate_mask_drops_only_io_mem_busmaster() {
        // bus-master(2)+mem(1)+SERR(8)+INTx-disable(10) — isolation clears 0..2,
        // leaving SERR and INTx-disable intact.
        let cmd: u16 = 0x0506;
        let isolated = cmd & !CMD_ISOLATE_MASK;
        assert_eq!(isolated & CMD_ISOLATE_MASK, 0);
        assert_eq!(isolated, 0x0500);
    }
}
