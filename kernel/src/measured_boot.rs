//! measured_boot — kernel-side measured boot (Concept §RaeShield: "you don't need
//! kernel access; here's a better primitive").
//!
//! The companion to [`crate::secure_boot`]: secure boot *verifies* a signature before
//! running an image; measured boot *records* what actually ran into a TPM 2.0-style PCR
//! bank — `PCR := SHA256(PCR ‖ measurement)` — so the final PCR is a cryptographic
//! commitment to the exact bytes AND order of the boot chain. A remote attestation
//! service (the RaeShield anti-cheat primitive) or a local sealing policy recomputes
//! the expected PCRs and compares; any tamper, reorder, or swap diverges.
//!
//! Here the kernel measures the authentic in-memory `crate::INITRAMFS` (the userspace
//! image that actually booted) into a boot-chain PCR. The measurement engine is the
//! shared [`rae_crypto::pcr`] core (host-KAT'd), so this matches a hardware TPM for the
//! same events; the hardware TPM chip + key sealing remain the iron-gated half.

use core::sync::atomic::{AtomicU32, Ordering};
use rae_crypto::pcr::PcrBank;
use spin::Mutex;

/// PCR index for the kernel/boot-manager stage — the signed boot manifest, which
/// commits the kernel build (mirrors TPM convention: PCR 4 = boot manager code).
pub const PCR_KERNEL: u8 = 4;
/// PCR index for the userspace image (mirrors TPM convention: PCR 8 = OS/loader data).
pub const PCR_INITRAMFS: u8 = 8;

/// The kernel's live measured-boot PCR bank, populated at [`init`].
static PCR_BANK: Mutex<Option<PcrBank>> = Mutex::new(None);
/// 0 = not run, 1 = smoketest PASS, 2 = FAIL — exported via procfs.
static MB_STATUS: AtomicU32 = AtomicU32::new(0);

/// Measure the authentic boot-chain images into the PCR bank. Called once during boot,
/// after the initramfs is available.
pub fn init() {
    let mut bank = PcrBank::new();
    // Two-stage boot-chain measurement: the signed boot manifest (kernel identity,
    // PCR 4) then the userspace image (PCR 8) — the exact code that actually booted.
    bank.measure(
        PCR_KERNEL,
        "boot-manifest",
        crate::secure_boot::boot_manifest(),
    );
    bank.measure(PCR_INITRAMFS, "initramfs", crate::INITRAMFS);
    let kp = bank.pcr(PCR_KERNEL).unwrap_or([0u8; 32]);
    let ip = bank.pcr(PCR_INITRAMFS).unwrap_or([0u8; 32]);
    crate::serial_println!(
        "[ OK ] Measured boot: PCR[{}]=kernel-manifest {:02x}{:02x}..{:02x}{:02x}  PCR[{}]=initramfs ({} bytes) {:02x}{:02x}..{:02x}{:02x}",
        PCR_KERNEL,
        kp[0],
        kp[1],
        kp[30],
        kp[31],
        PCR_INITRAMFS,
        crate::INITRAMFS.len(),
        ip[0],
        ip[1],
        ip[30],
        ip[31],
    );
    *PCR_BANK.lock() = Some(bank);
}

/// R10 smoketest: prove the measurement primitive on real hardware, fail-closed.
/// Verifies (a) `measure` equals an independent `extend(sha256(data))`, (b) the PCR
/// extend is order-sensitive (A‖B differs from B‖A — the property that makes a reorder
/// detectable), and (c) the live initramfs PCR is non-zero AND reproducible by an
/// independent re-measurement of the same bytes (determinism on the REAL image).
pub fn run_boot_smoketest() {
    use rae_crypto::sha256::sha256;

    // (a) measure == extend(sha256(data)).
    let probe = b"measured-boot-probe";
    let mut m = PcrBank::new();
    let mut e = PcrBank::new();
    m.measure(0, "probe", probe);
    e.extend(0, &sha256(probe));
    let measure_eq_extend = m.pcr(0) == e.pcr(0) && m.pcr(0) != Some([0u8; 32]);

    // (b) order sensitivity.
    let (da, db) = (sha256(b"A"), sha256(b"B"));
    let mut ab = PcrBank::new();
    ab.extend(0, &da);
    ab.extend(0, &db);
    let mut ba = PcrBank::new();
    ba.extend(0, &db);
    ba.extend(0, &da);
    let order_sensitive = ab.pcr(0) != ba.pcr(0);

    // (c) determinism on the real, live initramfs measurement.
    let live = PCR_BANK.lock().as_ref().and_then(|b| b.pcr(PCR_INITRAMFS));
    let mut fresh = PcrBank::new();
    fresh.measure(PCR_INITRAMFS, "initramfs", crate::INITRAMFS);
    let recomputed = fresh.pcr(PCR_INITRAMFS);
    let live_deterministic = live.is_some() && live == recomputed && live != Some([0u8; 32]);

    let pass = measure_eq_extend && order_sensitive && live_deterministic;
    MB_STATUS.store(if pass { 1 } else { 2 }, Ordering::Relaxed);
    crate::serial_println!(
        "[measured-boot] measure==extend={} order_sensitive={} live_deterministic={} -> {}",
        measure_eq_extend,
        order_sensitive,
        live_deterministic,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// `/proc/raeen/measured_boot` — the live PCR bank + measurement log, the attestation
/// evidence a verifier reads.
pub fn dump_text() -> alloc::string::String {
    use core::fmt::Write;
    let mut out = alloc::string::String::new();
    let status = match MB_STATUS.load(Ordering::Relaxed) {
        1 => "PASS",
        2 => "FAIL",
        _ => "not-run",
    };
    let _ = writeln!(out, "measured_boot smoketest: {status}");
    let guard = PCR_BANK.lock();
    match guard.as_ref() {
        None => {
            let _ = writeln!(out, "(bank not initialized)");
        }
        Some(bank) => {
            for ev in bank.log() {
                let d = ev.digest;
                let _ = writeln!(
                    out,
                    "event pcr={} {} digest={:02x}{:02x}{:02x}{:02x}..",
                    ev.pcr, ev.description, d[0], d[1], d[2], d[3]
                );
            }
            for idx in [PCR_KERNEL, PCR_INITRAMFS] {
                if let Some(p) = bank.pcr(idx) {
                    let _ = write!(out, "PCR[{idx}]=");
                    for b in p {
                        let _ = write!(out, "{b:02x}");
                    }
                    let _ = writeln!(out);
                }
            }
        }
    }
    out
}
