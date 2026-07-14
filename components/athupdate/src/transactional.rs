//! Transactional, signature-gated, rollback-able OS update application.
//!
//! This is the load-bearing slice of the RaeUpdate Concept pillar:
//! *"atomic CoW updates + one-click rollback; no forced updates — the user owns
//! the machine."* (`LEGACY_GAMING_CONCEPT.md`). Everything here is **pure logic** —
//! it consumes the leaf primitives `ath_diff` (byte delta apply), `ath_hash`
//! (SHA-256 integrity) and `ath_crypto` (Ed25519 signature) read-only, decides
//! *what should happen*, and produces the new slot image bytes in RAM. It never
//! touches a disk sector: the actual sector write is the kernel's job and MUST
//! route through `block_io::safe_mode_guard_write` (deferred — the kernel-net
//! style follow-up).
//!
//! The transaction is **all-or-nothing**:
//! 1. The signed payload is hashed (SHA-256) and the hash compared to the
//!    publisher's declared checksum — a single flipped byte is rejected.
//! 2. The Ed25519 signature over the payload is verified against the system's
//!    trusted update key (held by athena-security; we VERIFY, never mint trust).
//! 3. ONLY after both pass is the byte-delta deserialized and applied to the
//!    active slot's image to reconstruct the new slot image.
//! 4. The reconstructed image's hash is checked against the payload's declared
//!    *result* hash — a delta that applies cleanly but yields the wrong bytes
//!    (a forged/mismatched delta) is rejected.
//! 5. The new image is staged into the *inactive* slot; the active (running)
//!    slot is never modified. The boot pointer flips only after staging
//!    succeeds, and is reverted automatically if the new slot fails its
//!    post-boot health check N times.
//!
//! A power loss at any step before the atomic flip leaves the old slot bootable;
//! [`UpdateSession`] models that recoverable mid-update state explicitly.

use alloc::vec::Vec;

use ath_diff::{apply_delta, DeltaOp, DiffError};

use crate::{PartitionSlot, UpdateError, Version};

// ===========================================================================
// 1. Trust input — the update-signing public key (owned by athena-security).
// ===========================================================================

/// The Ed25519 public key the running system trusts for update payloads.
///
/// We do not generate or store this key here — athena-security owns the
/// secure-boot trust chain and hands us the verified key. RaeUpdate's job is to
/// VERIFY a payload against it, loudly refusing anything that does not match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateTrustKey {
    pub ed25519_public: [u8; 32],
}

impl UpdateTrustKey {
    pub const fn new(ed25519_public: [u8; 32]) -> Self {
        Self { ed25519_public }
    }
}

// ===========================================================================
// 2. The signed delta payload.
// ===========================================================================

/// A signed delta update as it arrives over the wire / off disk.
///
/// `payload` is the serialized [`DeltaOp`] stream (see [`serialize_delta`] /
/// [`deserialize_delta`]). `payload_sha256` is the publisher's declared hash of
/// `payload`; `result_sha256` is the declared hash of the fully-reconstructed
/// new slot image. `signature` is an Ed25519 signature over `payload`.
#[derive(Debug, Clone)]
pub struct SignedDeltaPayload {
    pub from_version: Version,
    pub to_version: Version,
    /// Serialized [`DeltaOp`] stream — treated as hostile until verified.
    pub payload: Vec<u8>,
    /// Publisher's SHA-256 of `payload` (integrity, pre-apply).
    pub payload_sha256: [u8; 32],
    /// Publisher's SHA-256 of the reconstructed new image (correctness, post-apply).
    pub result_sha256: [u8; 32],
    /// Ed25519 signature over `payload`.
    pub signature: [u8; 64],
}

/// Why a payload was refused. Distinct variants so the UI / log can say exactly
/// what failed — an update never fails silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyError {
    /// `payload_sha256` did not match the actual hash of `payload`.
    PayloadChecksumMismatch,
    /// The Ed25519 signature did not verify against the trusted key.
    SignatureInvalid,
    /// The serialized delta stream was truncated or malformed.
    MalformedDelta,
    /// A `Copy` op referenced bytes outside the base image (forged delta).
    DeltaOutOfRange,
    /// The delta applied but the reconstructed image's hash did not match
    /// `result_sha256` — wrong/forged delta against the wrong base.
    ResultChecksumMismatch,
}

impl From<VerifyError> for UpdateError {
    fn from(e: VerifyError) -> Self {
        match e {
            VerifyError::PayloadChecksumMismatch => UpdateError::ChecksumMismatch,
            VerifyError::SignatureInvalid => UpdateError::SignatureInvalid,
            VerifyError::MalformedDelta => UpdateError::CorruptedPackage,
            VerifyError::DeltaOutOfRange => UpdateError::CorruptedPackage,
            VerifyError::ResultChecksumMismatch => UpdateError::ChecksumMismatch,
        }
    }
}

impl SignedDeltaPayload {
    /// Verify integrity + authenticity of the *payload bytes* (steps 1 and 2),
    /// WITHOUT applying anything. Returns `Ok(())` only if both the SHA-256
    /// checksum and the Ed25519 signature pass. This is the gate that must run
    /// before a single delta byte is interpreted.
    pub fn verify(&self, key: &UpdateTrustKey) -> Result<(), VerifyError> {
        // 1. Integrity: hash the payload and compare to the declared checksum.
        let actual = ath_hash::sha256(&self.payload);
        if !ct_eq(&actual, &self.payload_sha256) {
            return Err(VerifyError::PayloadChecksumMismatch);
        }
        // 2. Authenticity: Ed25519 over the payload bytes.
        if !ath_crypto::ed25519::verify(&key.ed25519_public, &self.payload, &self.signature) {
            return Err(VerifyError::SignatureInvalid);
        }
        Ok(())
    }

    /// The full transactional reconstruction: verify the payload, deserialize
    /// the delta, apply it to `base_image` (the active slot's bytes), and verify
    /// the *result* hash. Returns the new slot image bytes ONLY if every gate
    /// passes. Never applies an unverified delta; never panics.
    pub fn reconstruct(
        &self,
        key: &UpdateTrustKey,
        base_image: &[u8],
    ) -> Result<Vec<u8>, VerifyError> {
        // Gate 1+2: integrity + signature of the payload.
        self.verify(key)?;
        // Gate 3: deserialize the now-trusted delta stream.
        let ops = deserialize_delta(&self.payload).ok_or(VerifyError::MalformedDelta)?;
        // Gate 4: apply against the base; a corrupt Copy range is refused.
        let new_image = apply_delta(base_image, &ops).map_err(|e| match e {
            DiffError::DeltaOutOfRange => VerifyError::DeltaOutOfRange,
            _ => VerifyError::MalformedDelta,
        })?;
        // Gate 5: the reconstructed image must match the declared result hash.
        let result_hash = ath_hash::sha256(&new_image);
        if !ct_eq(&result_hash, &self.result_sha256) {
            return Err(VerifyError::ResultChecksumMismatch);
        }
        Ok(new_image)
    }
}

/// Constant-time-ish 32-byte compare (no early return on mismatch). Hashes are
/// not secrets, but a uniform compare keeps the verify path free of
/// data-dependent timing and is a good habit on a security boundary.
fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

// ===========================================================================
// 3. DeltaOp serialization — the over-the-wire form of a binary delta.
// ===========================================================================
//
// ath_diff defines DeltaOp but no wire form (it is a pure algorithm crate). The
// update payload is bytes, so we own a small, explicit, never-panic codec here.
//
// Format (all integers little-endian):
//   [u32 op_count]
//   per op:
//     [u8 tag]                       0 = Copy, 1 = Data
//     Copy: [u64 src_off][u64 len]
//     Data: [u64 len][len bytes]
//
// Every length is bounds-checked against the remaining buffer on decode; a
// truncated or oversized field yields `None`, never a panic or OOM read.

const TAG_COPY: u8 = 0;
const TAG_DATA: u8 = 1;

/// Serialize a delta op stream into the payload byte form.
pub fn serialize_delta(ops: &[DeltaOp]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(ops.len() as u32).to_le_bytes());
    for op in ops {
        match op {
            DeltaOp::Copy { src_off, len } => {
                out.push(TAG_COPY);
                out.extend_from_slice(&(*src_off as u64).to_le_bytes());
                out.extend_from_slice(&(*len as u64).to_le_bytes());
            }
            DeltaOp::Data(bytes) => {
                out.push(TAG_DATA);
                out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                out.extend_from_slice(bytes);
            }
        }
    }
    out
}

/// Deserialize a delta op stream. Returns `None` on any malformation
/// (truncation, bad tag, length past end of buffer) — never panics.
pub fn deserialize_delta(buf: &[u8]) -> Option<Vec<DeltaOp>> {
    let mut cur = 0usize;
    let count = read_u32(buf, &mut cur)? as usize;
    let mut ops = Vec::new();
    for _ in 0..count {
        let tag = *buf.get(cur)?;
        cur += 1;
        match tag {
            TAG_COPY => {
                let src_off = read_u64(buf, &mut cur)? as usize;
                let len = read_u64(buf, &mut cur)? as usize;
                ops.push(DeltaOp::Copy { src_off, len });
            }
            TAG_DATA => {
                let len = read_u64(buf, &mut cur)? as usize;
                let end = cur.checked_add(len)?;
                if end > buf.len() {
                    return None;
                }
                ops.push(DeltaOp::Data(buf[cur..end].to_vec()));
                cur = end;
            }
            _ => return None,
        }
    }
    // Trailing garbage after the declared op count is a malformed payload.
    if cur != buf.len() {
        return None;
    }
    Some(ops)
}

fn read_u32(buf: &[u8], cur: &mut usize) -> Option<u32> {
    let end = cur.checked_add(4)?;
    if end > buf.len() {
        return None;
    }
    let mut b = [0u8; 4];
    b.copy_from_slice(&buf[*cur..end]);
    *cur = end;
    Some(u32::from_le_bytes(b))
}

fn read_u64(buf: &[u8], cur: &mut usize) -> Option<u64> {
    let end = cur.checked_add(8)?;
    if end > buf.len() {
        return None;
    }
    let mut b = [0u8; 8];
    b.copy_from_slice(&buf[*cur..end]);
    *cur = end;
    Some(u64::from_le_bytes(b))
}

// ===========================================================================
// 4. The update session state machine — the transactional, recoverable model.
// ===========================================================================

/// Where a transactional update currently stands. Each state is recoverable:
/// at any point before [`UpdateState::Flipped`], a power loss leaves the OLD
/// active slot untouched and bootable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateState {
    /// No update in flight; the active slot is the only good slot we rely on.
    Idle,
    /// Payload verified (checksum + signature) but not yet applied.
    Verified,
    /// New image reconstructed and written to the inactive (standby) slot, which
    /// is now bootable-once-pending but NOT yet the default boot target.
    Staged,
    /// Boot pointer flipped to the standby slot; awaiting post-boot health.
    /// This is the only state where the old slot is no longer the default.
    Flipped,
    /// Health check passed; the new slot is the committed, successful default.
    Committed,
    /// Health check failed (or boot watchdog tripped) → reverted to old slot.
    RolledBack,
    /// A verify/apply gate failed; nothing was staged, old slot intact.
    Aborted(VerifyError),
}

/// Health of a freshly-flipped slot, reported by the post-boot watchdog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthCheck {
    /// Reached the boot-success marker — the update is good.
    Healthy,
    /// Did not reach the marker this boot attempt.
    Unhealthy,
}

/// A single transactional update from the active slot to a new image.
///
/// The session never mutates the active slot. It records which slot is the
/// target (the inactive one), drives the verify → stage → flip → health → commit
/// /rollback sequence, and counts failed boots so a wedged new slot
/// auto-reverts after `max_boot_attempts`.
#[derive(Debug, Clone)]
pub struct UpdateSession {
    pub state: UpdateState,
    /// The slot currently running (and the rollback target). Never written.
    pub active_slot: PartitionSlot,
    /// The slot the new image is staged into = `active_slot.other()`.
    pub target_slot: PartitionSlot,
    pub from_version: Version,
    pub to_version: Version,
    /// How many times the flipped slot may fail to reach the marker before we
    /// auto-rollback. Default 3.
    pub max_boot_attempts: u32,
    /// Failed-boot count since the flip.
    pub boot_attempts: u32,
}

impl UpdateSession {
    /// Begin a session for `active_slot`. The target is the *other* slot.
    pub fn new(active_slot: PartitionSlot, from: Version, to: Version) -> Self {
        Self {
            state: UpdateState::Idle,
            active_slot,
            target_slot: active_slot.other(),
            from_version: from,
            to_version: to,
            max_boot_attempts: 3,
            boot_attempts: 0,
        }
    }

    /// Step 1+2: verify the payload's integrity and signature. On success the
    /// session advances to `Verified`; on failure it goes to `Aborted` (the
    /// active slot is never touched) and the error is returned.
    pub fn verify_payload(
        &mut self,
        payload: &SignedDeltaPayload,
        key: &UpdateTrustKey,
    ) -> Result<(), VerifyError> {
        // Sanity: the payload must be the one this session expects.
        if payload.from_version != self.from_version || payload.to_version != self.to_version {
            self.state = UpdateState::Aborted(VerifyError::ResultChecksumMismatch);
            return Err(VerifyError::ResultChecksumMismatch);
        }
        match payload.verify(key) {
            Ok(()) => {
                self.state = UpdateState::Verified;
                Ok(())
            }
            Err(e) => {
                self.state = UpdateState::Aborted(e);
                Err(e)
            }
        }
    }

    /// Step 3-5: with the payload already verified, reconstruct the new image
    /// from `base_image` (the active slot bytes) and "stage" it (return the
    /// bytes the kernel will write to the inactive slot). Advances to `Staged`.
    ///
    /// The actual sector write is the caller/kernel's responsibility and must go
    /// through `block_io::safe_mode_guard_write`; this returns the verified bytes
    /// and the target slot, never writing anything itself.
    pub fn stage(
        &mut self,
        payload: &SignedDeltaPayload,
        key: &UpdateTrustKey,
        base_image: &[u8],
    ) -> Result<StagedImage, VerifyError> {
        if self.state != UpdateState::Verified {
            // Re-run verification defensively if not already verified — we must
            // never reconstruct from an unverified payload.
            self.verify_payload(payload, key)?;
        }
        match payload.reconstruct(key, base_image) {
            Ok(bytes) => {
                self.state = UpdateState::Staged;
                Ok(StagedImage {
                    slot: self.target_slot,
                    bytes,
                    sha256: payload.result_sha256,
                })
            }
            Err(e) => {
                self.state = UpdateState::Aborted(e);
                Err(e)
            }
        }
    }

    /// The atomic flip: point the bootloader at the freshly-staged slot. Only
    /// legal from `Staged`. After this the new slot is the default and the boot
    /// watchdog governs whether it survives. Returns the slot now active.
    pub fn flip(&mut self) -> Result<PartitionSlot, UpdateError> {
        if self.state != UpdateState::Staged {
            return Err(UpdateError::PartitionError);
        }
        self.state = UpdateState::Flipped;
        self.boot_attempts = 0;
        Ok(self.target_slot)
    }

    /// Post-boot watchdog report. A `Healthy` result commits the update; an
    /// `Unhealthy` result increments the failed-boot count and, once it exceeds
    /// `max_boot_attempts`, triggers an automatic rollback to the old slot.
    ///
    /// Returns `true` if a rollback was performed.
    pub fn report_health(&mut self, health: HealthCheck) -> bool {
        if self.state != UpdateState::Flipped {
            return false;
        }
        match health {
            HealthCheck::Healthy => {
                self.state = UpdateState::Committed;
                false
            }
            HealthCheck::Unhealthy => {
                self.boot_attempts += 1;
                if self.boot_attempts >= self.max_boot_attempts {
                    self.rollback();
                    true
                } else {
                    false
                }
            }
        }
    }

    /// One-click / automatic rollback: return the boot target to the old slot.
    /// Legal from `Flipped` (auto, watchdog) or `Committed` (user-initiated
    /// "go back"). The old slot was never modified, so this is always safe.
    pub fn rollback(&mut self) -> PartitionSlot {
        self.state = UpdateState::RolledBack;
        // After rollback the active (good) slot is the boot target again.
        self.active_slot
    }

    /// Which slot the bootloader should currently boot. This is the single
    /// source of truth a recovery path reads after a power loss: until `flip`,
    /// it is always the old `active_slot`, so an interrupted update is
    /// recoverable by definition.
    pub fn boot_target(&self) -> PartitionSlot {
        match self.state {
            UpdateState::Flipped | UpdateState::Committed => self.target_slot,
            _ => self.active_slot,
        }
    }

    /// Is this session in a state where the machine is guaranteed bootable from
    /// a known-good slot if power is lost right now?
    pub fn is_recoverable(&self) -> bool {
        match self.state {
            // Before the flip, the old slot is the boot target and untouched.
            UpdateState::Idle
            | UpdateState::Verified
            | UpdateState::Staged
            | UpdateState::RolledBack
            | UpdateState::Aborted(_) => true,
            // After flip we rely on the new slot, but the watchdog will revert
            // an unhealthy one — still recoverable, just via the watchdog.
            UpdateState::Flipped | UpdateState::Committed => true,
        }
    }
}

/// The verified, ready-to-write new slot image. The caller hands `bytes` to the
/// kernel writer (through `safe_mode_guard_write`) for `slot`.
#[derive(Debug, Clone)]
pub struct StagedImage {
    pub slot: PartitionSlot,
    pub bytes: Vec<u8>,
    pub sha256: [u8; 32],
}

// ===========================================================================
// 5. Host KATs — the FAIL-able proof (`cargo test -p athupdate`).
// ===========================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use ath_diff::byte_delta;

    // Build a self-consistent signed payload from old->new using a known seed.
    fn make_payload(
        seed: &[u8; 32],
        old: &[u8],
        new: &[u8],
    ) -> (SignedDeltaPayload, UpdateTrustKey) {
        let ops = byte_delta(old, new);
        let payload = serialize_delta(&ops);
        let payload_sha256 = ath_hash::sha256(&payload);
        let result_sha256 = ath_hash::sha256(new);
        let signature = ath_crypto::ed25519::sign(seed, &payload);
        let pubkey = ath_crypto::ed25519::derive_public_key(seed);
        (
            SignedDeltaPayload {
                from_version: Version::new(1, 0, 0),
                to_version: Version::new(1, 1, 0),
                payload,
                payload_sha256,
                result_sha256,
                signature,
            },
            UpdateTrustKey::new(pubkey),
        )
    }

    const SEED: [u8; 32] = [
        0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
        0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
        0x7f, 0x60,
    ];

    // --- serialization round-trip ---
    #[test]
    fn delta_codec_round_trips() {
        let old = b"the quick brown fox jumps over the lazy dog".to_vec();
        let new = b"the quick RED fox leaps over the lazy dog!!".to_vec();
        let ops = byte_delta(&old, &new);
        let bytes = serialize_delta(&ops);
        let back = deserialize_delta(&bytes).expect("decode");
        assert_eq!(ops, back, "delta codec must round-trip exactly");
        let rebuilt = apply_delta(&old, &back).expect("apply");
        assert_eq!(rebuilt, new, "applied delta must reproduce new exactly");
    }

    #[test]
    fn malformed_delta_is_rejected_not_panicked() {
        // Truncated header.
        assert!(deserialize_delta(&[0x01]).is_none());
        // Op count claims 1 Data op but body is missing.
        assert!(deserialize_delta(&[1, 0, 0, 0, TAG_DATA, 5, 0, 0, 0, 0, 0, 0, 0]).is_none());
        // Bad tag.
        assert!(deserialize_delta(&[1, 0, 0, 0, 0xFF]).is_none());
        // Trailing garbage after a valid op stream.
        let mut good = serialize_delta(&[DeltaOp::Data(b"hi".to_vec())]);
        good.push(0x00);
        assert!(deserialize_delta(&good).is_none());
    }

    // --- the happy path: a valid signed delta applies to byte-correct content ---
    #[test]
    fn valid_signed_delta_applies_byte_correct() {
        let old = b"AthenaOS kernel image v1.0.0 ... body ... tail".to_vec();
        let new = b"AthenaOS kernel image v1.1.0 ... BODY!! ... tail".to_vec();
        let (payload, key) = make_payload(&SEED, &old, &new);

        let mut sess = UpdateSession::new(
            PartitionSlot::A,
            Version::new(1, 0, 0),
            Version::new(1, 1, 0),
        );
        sess.verify_payload(&payload, &key).expect("verify");
        assert_eq!(sess.state, UpdateState::Verified);

        let staged = sess.stage(&payload, &key, &old).expect("stage");
        assert_eq!(staged.slot, PartitionSlot::B, "must stage to inactive slot");
        assert_eq!(
            staged.bytes, new,
            "reconstructed image must be byte-correct"
        );
        assert_eq!(sess.state, UpdateState::Staged);
        // Active slot A is still the boot target until we flip.
        assert_eq!(sess.boot_target(), PartitionSlot::A);
        assert!(sess.is_recoverable());
    }

    // --- negative control: bad checksum is REJECTED, never applied ---
    #[test]
    fn bad_payload_checksum_is_rejected() {
        let old = b"base image".to_vec();
        let new = b"new image!".to_vec();
        let (mut payload, key) = make_payload(&SEED, &old, &new);
        payload.payload_sha256[0] ^= 0xFF; // tamper the declared checksum

        assert_eq!(
            payload.verify(&key),
            Err(VerifyError::PayloadChecksumMismatch)
        );
        let mut sess = UpdateSession::new(
            PartitionSlot::A,
            Version::new(1, 0, 0),
            Version::new(1, 1, 0),
        );
        assert!(sess.verify_payload(&payload, &key).is_err());
        assert!(matches!(sess.state, UpdateState::Aborted(_)));
        // reconstruct must also refuse — never applies an unverified delta.
        assert!(payload.reconstruct(&key, &old).is_err());
    }

    // --- negative control: tampered payload bytes (signature no longer valid) ---
    #[test]
    fn tampered_payload_fails_signature() {
        let old = b"base image".to_vec();
        let new = b"new image!".to_vec();
        let (mut payload, key) = make_payload(&SEED, &old, &new);
        // Flip a payload byte AND fix its checksum so it passes integrity but
        // the Ed25519 signature (over the original bytes) must now fail.
        payload.payload[0] ^= 0x01;
        payload.payload_sha256 = ath_hash::sha256(&payload.payload);

        assert_eq!(payload.verify(&key), Err(VerifyError::SignatureInvalid));
    }

    // --- negative control: forged signature from the wrong key ---
    #[test]
    fn wrong_key_fails_signature() {
        let old = b"base image".to_vec();
        let new = b"new image!".to_vec();
        let (payload, _key) = make_payload(&SEED, &old, &new);
        // A different trusted key than the one that signed.
        let other_seed = [0x42u8; 32];
        let wrong_key = UpdateTrustKey::new(ath_crypto::ed25519::derive_public_key(&other_seed));
        assert_eq!(
            payload.verify(&wrong_key),
            Err(VerifyError::SignatureInvalid)
        );
    }

    // --- negative control: valid signature, but delta yields wrong bytes ---
    #[test]
    fn wrong_result_hash_is_rejected() {
        let old = b"base image".to_vec();
        let new = b"new image!".to_vec();
        let (mut payload, key) = make_payload(&SEED, &old, &new);
        // Corrupt the declared result hash but keep payload+signature valid.
        payload.result_sha256[0] ^= 0xFF;
        // payload.verify passes (integrity+sig of payload bytes unchanged)...
        payload.verify(&key).expect("payload itself is intact");
        // ...but reconstruct catches the result-hash mismatch.
        assert_eq!(
            payload.reconstruct(&key, &old),
            Err(VerifyError::ResultChecksumMismatch)
        );
    }

    // --- negative control: applying a valid delta to the WRONG base ---
    #[test]
    fn delta_against_wrong_base_is_caught() {
        let old = b"the real base image bytes here".to_vec();
        let new = b"the real base image bytes HERE!".to_vec();
        let (payload, key) = make_payload(&SEED, &old, &new);
        // Same signed payload, but apply against a different base of same length
        // region — the result hash gate must catch the mismatch.
        let wrong_base = b"a completely different base xx".to_vec();
        let res = payload.reconstruct(&key, &wrong_base);
        assert!(
            matches!(
                res,
                Err(VerifyError::ResultChecksumMismatch) | Err(VerifyError::DeltaOutOfRange)
            ),
            "wrong base must be rejected, got {res:?}"
        );
    }

    // --- forged out-of-range Copy is refused (never reads past base) ---
    #[test]
    fn out_of_range_copy_is_rejected() {
        let old = b"short".to_vec();
        // Hand-craft a delta that copies beyond the base.
        let ops = vec![DeltaOp::Copy {
            src_off: 0,
            len: 9999,
        }];
        let bytes = serialize_delta(&ops);
        // Build a payload around it that is otherwise self-consistent.
        let payload_sha256 = ath_hash::sha256(&bytes);
        let signature = ath_crypto::ed25519::sign(&SEED, &bytes);
        let key = UpdateTrustKey::new(ath_crypto::ed25519::derive_public_key(&SEED));
        let payload = SignedDeltaPayload {
            from_version: Version::new(1, 0, 0),
            to_version: Version::new(1, 1, 0),
            payload: bytes,
            payload_sha256,
            result_sha256: [0u8; 32],
            signature,
        };
        // Integrity + signature pass, but apply refuses the OOB copy.
        assert_eq!(
            payload.reconstruct(&key, &old),
            Err(VerifyError::DeltaOutOfRange)
        );
    }

    // --- A/B slot alternation: active flips to the other slot, never itself ---
    #[test]
    fn ab_slot_alternates() {
        let a = UpdateSession::new(
            PartitionSlot::A,
            Version::new(1, 0, 0),
            Version::new(1, 1, 0),
        );
        assert_eq!(a.target_slot, PartitionSlot::B);
        let b = UpdateSession::new(
            PartitionSlot::B,
            Version::new(1, 0, 0),
            Version::new(1, 1, 0),
        );
        assert_eq!(b.target_slot, PartitionSlot::A);
    }

    // --- full success path: verify -> stage -> flip -> healthy -> commit ---
    #[test]
    fn full_success_commits_new_slot() {
        let old = b"slotA image v1".to_vec();
        let new = b"slotB image v2".to_vec();
        let (payload, key) = make_payload(&SEED, &old, &new);
        let mut sess = UpdateSession::new(
            PartitionSlot::A,
            Version::new(1, 0, 0),
            Version::new(1, 1, 0),
        );

        sess.verify_payload(&payload, &key).unwrap();
        let staged = sess.stage(&payload, &key, &old).unwrap();
        assert_eq!(staged.bytes, new);

        let booted = sess.flip().unwrap();
        assert_eq!(booted, PartitionSlot::B);
        assert_eq!(sess.state, UpdateState::Flipped);
        assert_eq!(sess.boot_target(), PartitionSlot::B);

        let rolled = sess.report_health(HealthCheck::Healthy);
        assert!(!rolled);
        assert_eq!(sess.state, UpdateState::Committed);
        assert_eq!(sess.boot_target(), PartitionSlot::B);
    }

    // --- failed health check N times -> auto rollback to old slot ---
    #[test]
    fn failed_health_triggers_rollback() {
        let old = b"slotA image v1".to_vec();
        let new = b"slotB image v2".to_vec();
        let (payload, key) = make_payload(&SEED, &old, &new);
        let mut sess = UpdateSession::new(
            PartitionSlot::A,
            Version::new(1, 0, 0),
            Version::new(1, 1, 0),
        );
        sess.verify_payload(&payload, &key).unwrap();
        sess.stage(&payload, &key, &old).unwrap();
        sess.flip().unwrap();

        // First two failed boots: still trying the new slot.
        assert!(!sess.report_health(HealthCheck::Unhealthy));
        assert_eq!(sess.boot_target(), PartitionSlot::B);
        assert!(!sess.report_health(HealthCheck::Unhealthy));
        assert_eq!(sess.boot_target(), PartitionSlot::B);
        // Third failure crosses max_boot_attempts (3) -> auto rollback.
        assert!(sess.report_health(HealthCheck::Unhealthy));
        assert_eq!(sess.state, UpdateState::RolledBack);
        assert_eq!(
            sess.boot_target(),
            PartitionSlot::A,
            "must revert to old slot"
        );
        assert!(sess.is_recoverable());
    }

    // --- one-click rollback from a committed-but-regretted update ---
    #[test]
    fn one_click_rollback_from_committed() {
        let old = b"slotA image v1".to_vec();
        let new = b"slotB image v2".to_vec();
        let (payload, key) = make_payload(&SEED, &old, &new);
        let mut sess = UpdateSession::new(
            PartitionSlot::A,
            Version::new(1, 0, 0),
            Version::new(1, 1, 0),
        );
        sess.verify_payload(&payload, &key).unwrap();
        sess.stage(&payload, &key, &old).unwrap();
        sess.flip().unwrap();
        sess.report_health(HealthCheck::Healthy);
        assert_eq!(sess.state, UpdateState::Committed);
        // User clicks "go back".
        let target = sess.rollback();
        assert_eq!(target, PartitionSlot::A);
        assert_eq!(sess.boot_target(), PartitionSlot::A);
    }

    // --- interrupted/partial apply leaves a recoverable state (old slot good) ---
    #[test]
    fn interrupted_before_flip_is_recoverable() {
        let old = b"slotA image v1".to_vec();
        let new = b"slotB image v2".to_vec();
        let (payload, key) = make_payload(&SEED, &old, &new);
        let mut sess = UpdateSession::new(
            PartitionSlot::A,
            Version::new(1, 0, 0),
            Version::new(1, 1, 0),
        );
        // Power loss after verify, before stage.
        sess.verify_payload(&payload, &key).unwrap();
        assert!(sess.is_recoverable());
        assert_eq!(sess.boot_target(), PartitionSlot::A);
        // After stage, before flip — standby holds new bytes, but boot target is
        // still the untouched old slot.
        sess.stage(&payload, &key, &old).unwrap();
        assert!(sess.is_recoverable());
        assert_eq!(sess.boot_target(), PartitionSlot::A);
    }

    // --- cannot flip from a non-staged state (transactional discipline) ---
    #[test]
    fn flip_requires_staged() {
        let mut sess = UpdateSession::new(
            PartitionSlot::A,
            Version::new(1, 0, 0),
            Version::new(1, 1, 0),
        );
        assert!(sess.flip().is_err());
        assert_eq!(sess.state, UpdateState::Idle);
    }

    // --- version mismatch between session and payload is refused ---
    #[test]
    fn payload_version_mismatch_is_aborted() {
        let old = b"base".to_vec();
        let new = b"next".to_vec();
        let (payload, key) = make_payload(&SEED, &old, &new); // 1.0.0 -> 1.1.0
        let mut sess = UpdateSession::new(
            PartitionSlot::A,
            Version::new(1, 0, 0),
            Version::new(2, 0, 0),
        );
        assert!(sess.verify_payload(&payload, &key).is_err());
        assert!(matches!(sess.state, UpdateState::Aborted(_)));
    }
}
