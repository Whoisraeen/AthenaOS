//! Measured-boot attestation — AthGuard's view of the shared PCR measurement core.
//!
//! Concept §AthGuard: "You don't need kernel access on our OS; here's a better
//! primitive." Anti-cheat and remote attestation want proof of *what booted*, not
//! ring-0. A measured boot gives exactly that: each boot stage hashes the next and
//! extends a Platform Configuration Register, so the final PCR is a cryptographic
//! commitment to the exact contents AND order of everything that ran.
//!
//! The measurement engine itself lives in [`rae_crypto::pcr`] so the kernel's boot-time
//! measurement and this userspace attestation API share one implementation (and agree
//! with a hardware TPM for the same events). AthGuard re-exports it here as the
//! attestation surface that anti-cheat vendors and remote verifiers consume instead of
//! a kernel driver.

pub use rae_crypto::pcr::{verify_log, MeasurementEvent, PcrBank, NUM_PCRS};

#[cfg(test)]
mod tests {
    use super::*;

    /// AthGuard-level smoke: the re-exported core builds a boot chain, and a remote
    /// verifier reproduces the sealed PCR from the log — but a tampered log cannot.
    #[test]
    fn attestation_flow_round_trips() {
        let mut bank = PcrBank::new();
        bank.measure(0, "bootloader", b"BL").unwrap();
        bank.measure(0, "kernel", b"KRN").unwrap();
        let golden = bank.pcr(0).unwrap();
        let log = bank.log().to_vec();
        assert!(verify_log(&log, 0, &golden));

        let mut tampered = log;
        tampered[1].digest[0] ^= 0xFF;
        assert!(!verify_log(&tampered, 0, &golden));
        assert_eq!(NUM_PCRS, 24);
    }
}
