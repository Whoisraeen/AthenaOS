#![allow(dead_code)]

extern crate alloc;

use alloc::vec::Vec;
use spin::Mutex;

use crate::tpm;

// ─── PCR Assignments (TCG PC Client spec + AthGuard extensions) ─────────────

pub const PCR_FIRMWARE: u32 = 0;
pub const PCR_BOOTLOADER: u32 = 4;
pub const PCR_SECURE_BOOT_POLICY: u32 = 7;
pub const PCR_KERNEL_IMAGE: u32 = 8;
pub const PCR_KERNEL_CMDLINE: u32 = 9;
pub const PCR_RAESHIELD_POLICY: u32 = 14;

// ─── Boot Stages ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootStage {
    Firmware,
    Bootloader,
    SecureBootPolicy,
    Kernel,
    KernelCommandLine,
    AthGuardPolicy,
    InitProcess,
    Compositor,
    UserSpace,
}

impl BootStage {
    pub const fn pcr_index(self) -> u32 {
        match self {
            BootStage::Firmware => PCR_FIRMWARE,
            BootStage::Bootloader => PCR_BOOTLOADER,
            BootStage::SecureBootPolicy => PCR_SECURE_BOOT_POLICY,
            BootStage::Kernel => PCR_KERNEL_IMAGE,
            BootStage::KernelCommandLine => PCR_KERNEL_CMDLINE,
            BootStage::AthGuardPolicy => PCR_RAESHIELD_POLICY,
            BootStage::InitProcess => PCR_KERNEL_IMAGE,
            BootStage::Compositor => PCR_KERNEL_IMAGE,
            BootStage::UserSpace => PCR_KERNEL_IMAGE,
        }
    }
}

// ─── Measurement Entry ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MeasurementEntry {
    pub stage: BootStage,
    pub hash: [u8; 32],
    pub pcr_index: u32,
    pub timestamp: u64,
    pub verified: bool,
}

// ─── Boot Chain ──────────────────────────────────────────────────────────────

pub struct BootChain {
    pub measurements: Vec<MeasurementEntry>,
    sealed: bool,
}

impl BootChain {
    pub fn new() -> Self {
        Self {
            measurements: Vec::new(),
            sealed: false,
        }
    }

    pub fn measure(&mut self, stage: BootStage, hash: [u8; 32], timestamp: u64) {
        if self.sealed {
            return;
        }
        let pcr_index = stage.pcr_index();
        self.measurements.push(MeasurementEntry {
            stage,
            hash,
            pcr_index,
            timestamp,
            verified: false,
        });
    }

    /// Replay the measurement log over `baseline` PCR values, returning the
    /// EXPECTED final PCR state (TCG event-log replay: each measurement folds
    /// in as `PCR = SHA-256(PCR_old || hash)`). Verification then compares
    /// this against the live PCR values — the log is intact iff it reproduces
    /// them. The pre-2026-06-11 `verify_chain` set `verified = true` on every
    /// entry and returned `all(verified)` — a tautology that made tampering
    /// with the log (or PCR divergence) undetectable and rendered
    /// `generate_attestation_quote` meaningless (Audit.md HIGH:
    /// security.rs:90).
    pub fn replay(&self, baseline: &[[u8; 32]; 24]) -> [[u8; 32]; 24] {
        let mut expected = *baseline;
        for entry in &self.measurements {
            let idx = entry.pcr_index as usize;
            if idx >= 24 {
                continue;
            }
            let mut hasher = crate::crypto::Sha256Context::new();
            use crate::crypto::HashAlgorithm;
            hasher.init();
            hasher.update(&expected[idx]);
            hasher.update(&entry.hash);
            let mut new_val = [0u8; 32];
            hasher.finalize(&mut new_val);
            expected[idx] = new_val;
        }
        expected
    }

    pub fn is_sealed(&self) -> bool {
        self.sealed
    }

    pub fn seal(&mut self) {
        self.sealed = true;
    }
}

// ─── TpmState (legacy compatibility wrapper) ─────────────────────────────────

pub struct TpmState {
    pub available: bool,
    pub version: u8,
    pub pcr_values: [[u8; 32]; 24],
}

impl TpmState {
    pub fn new() -> Self {
        Self {
            available: false,
            version: 2,
            pcr_values: [[0u8; 32]; 24],
        }
    }

    pub fn extend_pcr(&mut self, index: usize, data: &[u8; 32]) {
        if index >= 24 {
            return;
        }
        // Use SHA-256(old || new) via the crypto module
        let mut hasher = crate::crypto::Sha256Context::new();
        use crate::crypto::HashAlgorithm;
        hasher.init();
        hasher.update(&self.pcr_values[index]);
        hasher.update(data);
        let mut new_val = [0u8; 32];
        hasher.finalize(&mut new_val);
        self.pcr_values[index] = new_val;
    }

    pub fn read_pcr(&self, index: usize) -> Option<&[u8; 32]> {
        if index < 24 {
            Some(&self.pcr_values[index])
        } else {
            None
        }
    }

    /// Sync our cached PCR values from the real TPM device.
    pub fn sync_from_tpm(&mut self) {
        if let Some(ref device) = *tpm::TPM.lock() {
            self.available = device.is_hardware();
            for i in 0..24 {
                if let Some(val) = device.read_pcr(i as u32) {
                    self.pcr_values[i] = val;
                }
            }
        }
    }
}

// ─── SecureBoot ──────────────────────────────────────────────────────────────

pub struct SecureBoot {
    pub enabled: bool,
    pub chain: BootChain,
    pub tpm: TpmState,
    /// PCR values captured BEFORE our first `measure_stage` (the pre-OS
    /// state: whatever firmware/bootloader already extended). Log replay
    /// starts from here, so firmware-era extends don't false-fail us.
    pub baseline_pcrs: [[u8; 32]; 24],
}

impl SecureBoot {
    pub fn new() -> Self {
        Self {
            enabled: false,
            chain: BootChain::new(),
            tpm: TpmState::new(),
            baseline_pcrs: [[0u8; 32]; 24],
        }
    }

    /// Snapshot the current PCR state as the replay baseline. Must be called
    /// once, after `sync_from_tpm` and before the first `measure_stage`.
    pub fn capture_baseline(&mut self) {
        self.baseline_pcrs = self.tpm.pcr_values;
    }

    /// Measure a boot stage: record in the chain and extend the correct TPM PCR.
    pub fn measure_stage(&mut self, stage: BootStage, hash: [u8; 32], timestamp: u64) {
        let pcr_index = stage.pcr_index();
        self.chain.measure(stage, hash, timestamp);

        // Extend the real TPM PCR (hardware or software fallback)
        if let Some(ref mut device) = *tpm::TPM.lock() {
            let _ = device.extend_pcr(pcr_index, &hash);
        }
        // Also update our local cache
        self.tpm.extend_pcr(pcr_index as usize, &hash);
    }

    /// Verify the measurement log against the live PCR values: replay the
    /// log over the captured baseline and require the result to match the
    /// current PCR state, per touched PCR. An empty log never verifies.
    pub fn verify_boot(&mut self) -> bool {
        self.tpm.sync_from_tpm();
        if self.chain.measurements.is_empty() {
            return false;
        }
        let expected = self.chain.replay(&self.baseline_pcrs);
        let mut all_ok = true;
        for entry in self.chain.measurements.iter_mut() {
            let idx = entry.pcr_index as usize;
            let ok = idx < 24 && expected[idx] == self.tpm.pcr_values[idx];
            entry.verified = ok;
            all_ok &= ok;
        }
        all_ok
    }

    pub fn tpm_available(&self) -> bool {
        if let Some(ref device) = *tpm::TPM.lock() {
            device.is_hardware()
        } else {
            false
        }
    }
}

// ─── PCR Extension Helpers ──────────────────────────────────────────────────

/// Extend a PCR with raw data (hashes data first via SHA-256).
pub fn extend_pcr(index: u32, data: &[u8]) {
    let digest = tpm::sha256(data);
    if let Some(ref mut sb) = *SECURE_BOOT.lock() {
        if let Some(ref mut device) = *tpm::TPM.lock() {
            let _ = device.extend_pcr(index, &digest);
        }
        sb.tpm.extend_pcr(index as usize, &digest);
    }
}

/// Read all measured boot PCRs and compare against expected values.
/// Returns true if the boot chain is intact.
pub fn verify_boot_chain() -> bool {
    if let Some(ref mut sb) = *SECURE_BOOT.lock() {
        sb.verify_boot()
    } else {
        false
    }
}

/// Generate an attestation quote blob suitable for anti-cheat vendors.
/// Includes PCR values from the measured boot + a TPM2_Quote signature.
pub fn generate_attestation_quote(nonce: &[u8; 32]) -> Vec<u8> {
    let pcr_selection = [
        PCR_FIRMWARE,
        PCR_BOOTLOADER,
        PCR_SECURE_BOOT_POLICY,
        PCR_KERNEL_IMAGE,
        PCR_KERNEL_CMDLINE,
        PCR_RAESHIELD_POLICY,
    ];

    if let Some(ref mut device) = *tpm::TPM.lock() {
        // ONLY a hardware TPM produces an Attestation-Key-signed TPMS_ATTEST a
        // remote verifier can root in the manufacturer's EK certificate. A
        // SoftTpm's `quote` returns an UNSIGNED, publicly-computable blob
        // (magic || nonce || SHA256(pcrs||nonce)) — handing that out as a
        // "TPM quote" is precisely the forgeable-attestation hole BUG-33 set
        // out to close, since `TpmDevice::quote` returns Ok(..) for software.
        // So gate on `is_hardware()`: a software TPM falls through to the
        // fail-closed empty quote below, and the caller relies on the Tier-1
        // platform HMAC (`sign_attestation`) instead.
        if device.is_hardware() {
            if let Ok(quote) = device.quote(&pcr_selection, nonce) {
                return quote;
            }
        }
    }

    // BUG-33: NO unsigned fallback. The old path returned a raw PCR+nonce blob
    // ("RAEB") carrying no Attestation-Key signature, so any userspace agent
    // could forge arbitrary PCR values and defeat remote anti-cheat validation.
    // Attestation must fail CLOSED: with no cryptographically signed TPM quote
    // (no hardware TPM, or a hardware quote error) we emit an empty quote, which
    // every verifier's signature check rejects — far safer than a plausible-
    // looking forgeable blob.
    crate::serial_println!(
        "[security] attestation: no hardware-signed TPM quote available -> failing closed (empty quote)"
    );
    Vec::new()
}

// ─── Global State ────────────────────────────────────────────────────────────

pub static SECURE_BOOT: Mutex<Option<SecureBoot>> = Mutex::new(None);

pub fn init() {
    let mut sb = SecureBoot::new();

    // Capture the pre-OS PCR state FIRST: replay-verification starts from
    // this baseline, so anything firmware extended before us is accounted
    // for rather than false-failing the chain.
    sb.tpm.sync_from_tpm();
    sb.capture_baseline();

    // Measure known boot stages into their respective PCRs.
    // In production these hashes come from the actual binary contents;
    // at this point in boot, we record placeholder measurements that
    // the bootloader should have already extended into real hardware PCRs.
    sb.measure_stage(BootStage::Firmware, tpm::sha256(b"uefi-firmware"), 0);
    sb.measure_stage(BootStage::Bootloader, tpm::sha256(b"athenaos-bootloader"), 0);
    sb.measure_stage(
        BootStage::SecureBootPolicy,
        tpm::sha256(b"secure-boot-policy"),
        0,
    );
    sb.measure_stage(BootStage::Kernel, tpm::sha256(b"athenaos-kernel"), 0);
    sb.measure_stage(BootStage::KernelCommandLine, tpm::sha256(b""), 0);
    sb.measure_stage(
        BootStage::AthGuardPolicy,
        tpm::sha256(b"raeshield-policy-v1"),
        0,
    );

    sb.enabled = true;
    *SECURE_BOOT.lock() = Some(sb);
}

/// R10 boot smoketest — regression fence for the verify_chain tautology
/// (Audit.md HIGH: security.rs:90). Proves on every boot that:
///   1. the intact measurement log replays to the live PCR values (PASS), and
///   2. a deliberately tampered log is DETECTED (verification returns false),
/// then restores the log and re-verifies.
pub fn run_boot_smoketest() {
    let intact = verify_boot_chain();

    // Tamper: flip one byte of the kernel measurement's recorded hash. The
    // replay now diverges from the live PCRs, so verification MUST fail.
    let tampered_detected = {
        let mut guard = SECURE_BOOT.lock();
        if let Some(ref mut sb) = *guard {
            let saved = sb
                .chain
                .measurements
                .iter()
                .position(|e| e.stage == BootStage::Kernel)
                .map(|i| (i, sb.chain.measurements[i].hash));
            match saved {
                Some((i, original)) => {
                    sb.chain.measurements[i].hash[0] ^= 0xFF;
                    let detected = !sb.verify_boot();
                    sb.chain.measurements[i].hash = original;
                    detected
                }
                None => false,
            }
        } else {
            false
        }
    };

    // Restore: the untampered log must verify again.
    let restored = verify_boot_chain();

    // BUG-33 regression fence: `generate_attestation_quote` must fail CLOSED on
    // any platform WITHOUT a hardware TPM. A software TPM returns Ok(unsigned
    // blob) from `device.quote`, so the invariant we assert is: a non-empty
    // quote is emitted ONLY when the live TPM is real hardware. On QEMU/KVM (no
    // TPM2) the quote MUST be empty — a forgeable blob here defeats remote
    // anti-cheat validation. This test can FAIL: if the hardware gate regresses,
    // the software path yields a non-empty blob with `have_hw == false`.
    let quote_fails_closed = {
        let have_hw = matches!(*tpm::TPM.lock(), Some(ref d) if d.is_hardware());
        let quote = generate_attestation_quote(&[0x5Au8; 32]);
        have_hw || quote.is_empty()
    };

    let pass = intact && tampered_detected && restored && quote_fails_closed;
    crate::serial_println!(
        "[secboot-chain] smoketest: intact={} tamper_detected={} restored={} quote_fails_closed={} -> {}",
        intact,
        tampered_detected,
        restored,
        quote_fails_closed,
        if pass { "PASS" } else { "FAIL" },
    );
}
