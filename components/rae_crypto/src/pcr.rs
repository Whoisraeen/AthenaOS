//! Measured-boot PCR bank — a TPM 2.0-style SHA-256 Platform Configuration Register
//! accumulator, in software.
//!
//! A measured boot proves *what ran*: each stage of the chain (bootloader → kernel →
//! init → compositor) hashes the next stage and **extends** a PCR before handing off.
//! A PCR is a hash accumulator — `PCR := SHA256(PCR ‖ measurement)` — so its final
//! value is a cryptographic commitment to the exact contents AND order of everything
//! that ran. Tamper with any stage, reorder the chain, or swap the kernel, and the PCR
//! diverges; a verifier (a local sealing policy or a remote attestation service)
//! recomputes the expected value and compares.
//!
//! This is the shared **measurement core**, used both by the kernel's boot-time
//! measurement (`kernel::measured_boot`) and AthGuard's userspace attestation API. Its
//! behavior is identical to a hardware TPM for the same events, so the software chain
//! and a real TPM's PCRs agree. The hardware half — the TPM chip (TIS/CRB MMIO) and key
//! sealing to PCR state — is iron-gated and lives elsewhere; this pure-logic core is
//! host-KAT'd. Concept §AthGuard: "you don't need kernel access; here's a better
//! primitive."

use alloc::string::String;
use alloc::vec::Vec;

use crate::sha256::sha256;

/// TPM 2.0 defines 24 PCRs.
pub const NUM_PCRS: usize = 24;

/// One entry in the measured-boot event log: which PCR was extended, the SHA-256
/// digest of the measured data, and a human label for what was measured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeasurementEvent {
    pub pcr: u8,
    pub digest: [u8; 32],
    pub description: String,
}

/// A bank of SHA-256 PCRs plus the ordered event log that produced them.
pub struct PcrBank {
    pcrs: [[u8; 32]; NUM_PCRS],
    log: Vec<MeasurementEvent>,
}

impl Default for PcrBank {
    fn default() -> Self {
        Self::new()
    }
}

impl PcrBank {
    /// A fresh bank: all PCRs zeroed (the TPM reset state), empty log.
    pub fn new() -> Self {
        Self {
            pcrs: [[0u8; 32]; NUM_PCRS],
            log: Vec::new(),
        }
    }

    /// `TPM2_PCR_Extend`: `PCR[index] := SHA256(PCR[index] ‖ digest)`. Returns `None`
    /// for an out-of-range index — never panics (this runs on attacker-influenced
    /// input). Does NOT log (use [`measure`](Self::measure) for the logged form).
    pub fn extend(&mut self, index: u8, digest: &[u8; 32]) -> Option<()> {
        let pcr = self.pcrs.get_mut(index as usize)?;
        let mut buf = [0u8; 64];
        buf[..32].copy_from_slice(pcr);
        buf[32..].copy_from_slice(digest);
        *pcr = sha256(&buf);
        Some(())
    }

    /// Measure a boot-chain stage: hash `data`, extend `PCR[index]` with that digest,
    /// and append it to the event log. This is what one stage calls on the next.
    pub fn measure(&mut self, index: u8, description: &str, data: &[u8]) -> Option<()> {
        let digest = sha256(data);
        self.extend(index, &digest)?;
        self.log.push(MeasurementEvent {
            pcr: index,
            digest,
            description: description.into(),
        });
        Some(())
    }

    /// The current value of `PCR[index]`, if in range.
    pub fn pcr(&self, index: u8) -> Option<[u8; 32]> {
        self.pcrs.get(index as usize).copied()
    }

    /// The measured-boot event log, in measurement order.
    pub fn log(&self) -> &[MeasurementEvent] {
        &self.log
    }

    /// An attestation quote over a PCR `selection` bound to a verifier `nonce`:
    /// `SHA256( PCR[sel0] ‖ PCR[sel1] ‖ … ‖ nonce )`. Deterministic for a given boot
    /// chain, so a remote verifier recomputes it from the expected PCRs + the nonce it
    /// issued and compares. The nonce defeats replay of a stale quote. `None` if any
    /// selected index is out of range.
    pub fn quote(&self, selection: &[u8], nonce: &[u8]) -> Option<[u8; 32]> {
        let mut buf = Vec::with_capacity(selection.len() * 32 + nonce.len());
        for &i in selection {
            buf.extend_from_slice(&self.pcr(i)?);
        }
        buf.extend_from_slice(nonce);
        Some(sha256(&buf))
    }
}

/// Verify a measured-boot log against a sealed golden PCR value: replay the log into a
/// fresh bank and check that `PCR[expected_pcr]` reproduces `expected`. Returns `false`
/// on any divergence — a tampered, reordered, or truncated log cannot reproduce the
/// golden value. (The log's digests are the attacker-supplied claim; the PCR math is
/// what makes the claim unforgeable.)
pub fn verify_log(log: &[MeasurementEvent], expected_pcr: u8, expected: &[u8; 32]) -> bool {
    let mut bank = PcrBank::new();
    for ev in log {
        if bank.extend(ev.pcr, &ev.digest).is_none() {
            return false;
        }
    }
    bank.pcr(expected_pcr).as_ref() == Some(expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extend_matches_measure() {
        // measure(data) == extend(sha256(data)) on the same PCR.
        let data = b"kernel-x86_64";
        let mut a = PcrBank::new();
        let mut b = PcrBank::new();
        a.measure(4, "kernel", data).unwrap();
        b.extend(4, &sha256(data)).unwrap();
        assert_eq!(a.pcr(4), b.pcr(4));
        // And it moved off the reset (zero) value.
        assert_ne!(a.pcr(4).unwrap(), [0u8; 32]);
    }

    #[test]
    fn measured_boot_is_deterministic() {
        // The same chain measured twice yields identical PCRs + quote.
        let chain: [(&str, &[u8]); 3] = [
            ("bootloader", b"BL v1"),
            ("kernel", b"kernel bytes"),
            ("init", b"user_init"),
        ];
        let build = || {
            let mut bank = PcrBank::new();
            for (desc, data) in chain {
                bank.measure(0, desc, data).unwrap();
            }
            bank
        };
        let one = build();
        let two = build();
        assert_eq!(one.pcr(0), two.pcr(0));
        assert_eq!(one.quote(&[0], b"nonce"), two.quote(&[0], b"nonce"));
    }

    #[test]
    fn extend_is_order_sensitive() {
        // SHA256(SHA256(0‖A)‖B) != SHA256(SHA256(0‖B)‖A): reordering the chain
        // (e.g. swapping which stage ran first) changes the PCR — the whole point.
        let a = sha256(b"stageA");
        let b = sha256(b"stageB");
        let mut ab = PcrBank::new();
        ab.extend(0, &a).unwrap();
        ab.extend(0, &b).unwrap();
        let mut ba = PcrBank::new();
        ba.extend(0, &b).unwrap();
        ba.extend(0, &a).unwrap();
        assert_ne!(ab.pcr(0), ba.pcr(0));
    }

    #[test]
    fn tampering_diverges() {
        let mut good = PcrBank::new();
        good.measure(4, "kernel", b"genuine kernel").unwrap();
        let mut evil = PcrBank::new();
        evil.measure(4, "kernel", b"trojan kernel").unwrap();
        assert_ne!(good.pcr(4), evil.pcr(4));
    }

    #[test]
    fn out_of_range_index_is_safe() {
        let mut bank = PcrBank::new();
        assert_eq!(bank.extend(NUM_PCRS as u8, &[0u8; 32]), None);
        assert_eq!(bank.measure(99, "x", b"y"), None);
        assert_eq!(bank.pcr(NUM_PCRS as u8), None);
        assert_eq!(bank.quote(&[0, 99], b"n"), None); // a bad index in the selection
    }

    #[test]
    fn verify_log_round_trips_and_catches_tamper() {
        let mut bank = PcrBank::new();
        for (desc, data) in [
            ("bootloader", b"BL".as_slice()),
            ("kernel", b"KRN".as_slice()),
            ("compositor", b"CMP".as_slice()),
        ] {
            bank.measure(7, desc, data).unwrap();
        }
        let golden = bank.pcr(7).unwrap();
        let log = bank.log().to_vec();
        // The genuine log reproduces the golden PCR.
        assert!(verify_log(&log, 7, &golden));

        // Flip one byte of one measured digest → verification fails.
        let mut tampered = log.clone();
        tampered[1].digest[0] ^= 0x01;
        assert!(!verify_log(&tampered, 7, &golden));

        // Reorder the log → also fails (extend is non-commutative).
        let mut reordered = log.clone();
        reordered.swap(0, 2);
        assert!(!verify_log(&reordered, 7, &golden));

        // Drop the last stage (truncated boot) → fails.
        let mut truncated = log.clone();
        truncated.pop();
        assert!(!verify_log(&truncated, 7, &golden));
    }

    #[test]
    fn quote_binds_to_the_nonce() {
        let mut bank = PcrBank::new();
        bank.measure(0, "kernel", b"k").unwrap();
        let q1 = bank.quote(&[0], b"nonce-1").unwrap();
        let q2 = bank.quote(&[0], b"nonce-2").unwrap();
        assert_ne!(q1, q2); // a fresh nonce defeats replay of a stale quote
    }
}
