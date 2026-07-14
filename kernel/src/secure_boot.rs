//! Secure-boot trust anchor — MasterChecklist Phase 3.7 / LEGACY_GAMING_CONCEPT.md
//! §Security ("every artifact the system trusts is signed; the kernel holds
//! only the PUBLIC verification key").
//!
//! This is the kernel-side trust anchor: an embedded Ed25519 public key against
//! which signed payloads (atomic update packages, app bundles, a next-stage
//! payload) are verified. It carries NO private key — signing happens OFFLINE
//! with `tools/raesign` against the matching dev key, so a compromise of the
//! running system cannot mint trusted signatures.
//!
//! [`verify_against_anchor`] is the reusable verification primitive that
//! raeupdate / AthGuard bundle-install call. The boot smoketest proves the
//! anchor verifies an EXTERNALLY-produced signature (made by `raesign`, never
//! by this kernel) with public-key-only material, and rejects tampered
//! signatures fail-closed — distinct from `crypto.rs`'s self-contained Ed25519
//! KAT, which has the private seed.
//!
//! Surface: `/proc/raeen/secure_boot` ([`dump_text`]).
//!
//! Boot-manifest chain (Phase 3.7, landed 2026-06-12): xtask signs a manifest
//! of `sha256(initramfs.tar)` with the dev key every build, embedded as
//! `boot_manifest.bin`; [`verify_boot_manifest`]/[`run_manifest_smoketest`]
//! verify the signature against the embedded dev pubkey AND re-hash the
//! running `crate::INITRAMFS` to prove it is the exact authentic image —
//! tamper-evident. Remaining for the FULL chain: embedding the verify in the
//! BOOTLOADER so it refuses an unsigned/forged KERNEL before handoff (this
//! module proves the kernel→initramfs link; the bootloader→kernel link is the
//! open half), and TPM PCR measurement.

#![allow(dead_code)]

use core::sync::atomic::{AtomicU8, Ordering};

/// The DEV code-signing public key (same key `rae_manifest` verifies app
/// bundles with). xtask signs the boot manifest with the matching private
/// key in `keys/`. Build-time trust root; production swaps in an HSM key.
static DEV_SIGNING_PUBKEY: [u8; 32] = *include_bytes!("../../keys/dev-signing.pub");

/// Signed boot manifest produced by xtask at build time (regenerated every
/// build AFTER `initramfs.tar` is finalized, BEFORE the kernel compiles).
/// Layout: 8-byte magic "RAEBOOT1", 8-byte initramfs length (LE), 32-byte
/// SHA-256 of `initramfs.tar`, then a 64-byte Ed25519 signature over the
/// 48-byte manifest.
static BOOT_MANIFEST: &[u8] = include_bytes!("boot_manifest.bin");

const BM_MAGIC: &[u8; 8] = b"RAEBOOT1";
const BM_MANIFEST_LEN: usize = 48;
const BM_TOTAL_LEN: usize = BM_MANIFEST_LEN + 64;

/// 0 = not run, 1 = PASS, 2 = FAIL.
static MANIFEST_STATUS: AtomicU8 = AtomicU8::new(0);

/// Trust-anchor Ed25519 public key. The matching private key is the OFFLINE dev
/// key (`raesign keygen "athenaos-secureboot-dev-v1"`) and is never present in
/// the kernel. Rotating the anchor = replace these 32 bytes and re-sign.
const ANCHOR_PUBKEY: [u8; 32] = [
    0xbe, 0xf1, 0xf9, 0x6d, 0x8a, 0xe4, 0x97, 0xb0, 0x13, 0x7b, 0xe4, 0x9c, 0xac, 0x46, 0x92, 0xda,
    0x6f, 0x32, 0x64, 0xb2, 0x07, 0x63, 0x4f, 0x0b, 0x27, 0x5c, 0x30, 0x52, 0xc8, 0xc9, 0xec, 0xd6,
];

/// Boot-smoketest vector: a fixed message and a detached Ed25519 signature over
/// it, both produced offline by `raesign` with the dev key. Verified here with
/// `ANCHOR_PUBKEY` alone — proof the kernel verifies signatures it did not make.
const ANCHOR_MESSAGE: &[u8] = b"AthenaOS secure-boot trust anchor v1";
const ANCHOR_SIG: [u8; 64] = [
    0xbc, 0x06, 0xc8, 0xcf, 0x6a, 0xf5, 0x56, 0x3f, 0x36, 0xf8, 0x06, 0xff, 0xcb, 0x20, 0xb4, 0x63,
    0x25, 0xe7, 0x3a, 0x7b, 0x11, 0xc3, 0x58, 0xab, 0xda, 0xf8, 0x5e, 0xf0, 0x19, 0xca, 0xf5, 0xed,
    0xea, 0x59, 0xbc, 0x3f, 0xd5, 0xa7, 0x16, 0xa6, 0x94, 0xf6, 0x2b, 0x4c, 0xd0, 0x14, 0x4f, 0xde,
    0x14, 0x4c, 0xc0, 0xf0, 0x22, 0xd5, 0xbc, 0xb8, 0x4e, 0xc0, 0x81, 0x1a, 0x2c, 0x80, 0x89, 0x05,
];

/// 0 = not run, 1 = PASS, 2 = FAIL.
static SECBOOT_STATUS: AtomicU8 = AtomicU8::new(0);

/// Verify a detached Ed25519 signature over `blob` against the embedded
/// trust-anchor public key. Fail-closed: returns `false` on any decode error,
/// off-curve key, or forged signature.
pub fn verify_against_anchor(blob: &[u8], sig: &[u8; 64]) -> bool {
    crate::crypto::Ed25519Context::with_public_key(ANCHOR_PUBKEY)
        .verify(blob, sig)
        .unwrap_or(false)
}

/// The trust-anchor public key (for callers that want to display/compare it).
pub fn anchor_public_key() -> [u8; 32] {
    ANCHOR_PUBKEY
}

/// The Ed25519-signed boot manifest bytes (manifest + detached signature). It commits
/// the kernel build's identity — including the embedded initramfs hash — so measuring
/// it into a PCR records the exact signed kernel that booted (`measured_boot` uses
/// this for the kernel/boot-manager stage of the measured-boot chain).
pub fn boot_manifest() -> &'static [u8] {
    BOOT_MANIFEST
}

/// The offline-signed test vector (message + detached signature). Lets other
/// modules (update_slots' staging gate) prove their accept path with a REAL
/// anchor-signed payload without shipping any private key in the kernel.
pub(crate) fn anchor_test_vector() -> (&'static [u8], [u8; 64]) {
    (ANCHOR_MESSAGE, ANCHOR_SIG)
}

pub fn init() {
    crate::serial_println!(
        "[ OK ] Secure-boot trust anchor loaded (Ed25519 {:02x}{:02x}..{:02x}{:02x}, no private key in kernel)",
        ANCHOR_PUBKEY[0],
        ANCHOR_PUBKEY[1],
        ANCHOR_PUBKEY[30],
        ANCHOR_PUBKEY[31],
    );
}

/// R10 smoketest: verify the externally-signed anchor message with
/// public-key-only material, and confirm a tampered signature and a wrong
/// message are both rejected. Proves the secure-boot verification primitive on
/// real hardware (fail-closed). Pure deterministic computation.
pub fn run_boot_smoketest() {
    let accept = verify_against_anchor(ANCHOR_MESSAGE, &ANCHOR_SIG);

    let mut tampered = ANCHOR_SIG;
    tampered[0] ^= 0x01;
    let reject_tamper = !verify_against_anchor(ANCHOR_MESSAGE, &tampered);

    let reject_wrongmsg = !verify_against_anchor(b"not the anchor message", &ANCHOR_SIG);

    let pass = accept && reject_tamper && reject_wrongmsg;
    SECBOOT_STATUS.store(if pass { 1 } else { 2 }, Ordering::Relaxed);

    crate::serial_println!(
        "[secboot] trust-anchor verify (pubkey-only): accept={} reject_tamper={} reject_wrongmsg={} -> {}",
        accept,
        reject_tamper,
        reject_wrongmsg,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// Verify the signed boot manifest: signature authentic (dev key), magic
/// right, embedded `initramfs.tar` SHA-256 matches — i.e. the running
/// kernel's initramfs is the exact, untampered image this build signed.
/// Returns `(sig_ok, magic_ok, hash_match)`. Hashes only the in-memory
/// `crate::INITRAMFS`, never the 26 MiB kernel ELF.
pub fn verify_boot_manifest() -> (bool, bool, bool) {
    if BOOT_MANIFEST.len() != BM_TOTAL_LEN {
        return (false, false, false);
    }
    let manifest = &BOOT_MANIFEST[..BM_MANIFEST_LEN];
    let mut sig = [0u8; 64];
    sig.copy_from_slice(&BOOT_MANIFEST[BM_MANIFEST_LEN..]);

    let sig_ok = rae_crypto::ed25519::verify(&DEV_SIGNING_PUBKEY, manifest, &sig);
    let magic_ok = &manifest[0..8] == BM_MAGIC;

    let claimed_len = u64::from_le_bytes([
        manifest[8],
        manifest[9],
        manifest[10],
        manifest[11],
        manifest[12],
        manifest[13],
        manifest[14],
        manifest[15],
    ]);
    let claimed_hash = &manifest[16..48];
    let actual = rae_crypto::sha256::sha256(crate::INITRAMFS);
    let hash_match = claimed_len as usize == crate::INITRAMFS.len() && claimed_hash == actual;

    (sig_ok, magic_ok, hash_match)
}

/// R10 smoketest for the secure-boot manifest (Phase 3.7): the build-signed
/// manifest verifies against the embedded dev key AND its initramfs hash
/// matches the running kernel's embedded `initramfs.tar`. Tamper-evident end
/// to end — flip an initramfs byte → hash fails; forge the manifest → sig fails.
pub fn run_manifest_smoketest() {
    let (sig_ok, magic_ok, hash_match) = verify_boot_manifest();

    let tamper_rejected = if BOOT_MANIFEST.len() == BM_TOTAL_LEN {
        let mut m = [0u8; BM_MANIFEST_LEN];
        m.copy_from_slice(&BOOT_MANIFEST[..BM_MANIFEST_LEN]);
        m[16] ^= 0x01; // flip a hash byte
        let mut sig = [0u8; 64];
        sig.copy_from_slice(&BOOT_MANIFEST[BM_MANIFEST_LEN..]);
        !rae_crypto::ed25519::verify(&DEV_SIGNING_PUBKEY, &m, &sig)
    } else {
        false
    };

    let pass = sig_ok && magic_ok && hash_match && tamper_rejected;
    MANIFEST_STATUS.store(if pass { 1 } else { 2 }, Ordering::Relaxed);
    crate::serial_println!(
        "[secboot] boot-manifest verify: sig={} magic={} initramfs_hash_match={} tamper_rejected={} -> {}",
        sig_ok,
        magic_ok,
        hash_match,
        tamper_rejected,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// `/proc/raeen/secure_boot` contents.
pub fn dump_text() -> alloc::string::String {
    use alloc::format;
    let status = match SECBOOT_STATUS.load(Ordering::Relaxed) {
        1 => "verify=PASS (pubkey-only, fail-closed)",
        2 => "verify=FAIL",
        _ => "verify=not-run",
    };
    format!(
        "Secure-boot trust anchor (Phase 3.7)\n\
         algorithm: Ed25519 (RFC 8032)\n\
         anchor_pubkey: {}\n\
         private_key_in_kernel: no\n\
         smoketest: {}\n\
         note: artifact signing (xtask) + bootloader verify still pending\n",
        hex32(&ANCHOR_PUBKEY),
        status,
    )
}

fn hex32(b: &[u8; 32]) -> alloc::string::String {
    use alloc::string::String;
    use core::fmt::Write;
    let mut s = String::with_capacity(64);
    for byte in b {
        let _ = write!(s, "{:02x}", byte);
    }
    s
}
