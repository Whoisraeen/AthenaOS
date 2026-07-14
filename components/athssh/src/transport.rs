//! The `chacha20-poly1305@openssh.com` authenticated packet cipher (OpenSSH
//! `PROTOCOL.chacha20poly1305`) — the encrypted transport that a `SSH_MSG_NEWKEYS`
//! exchange switches on. Pure logic on `ath_crypto`'s KAT-verified ChaCha20 +
//! Poly1305: given the KEX-derived key material and a monotonic packet sequence
//! number it seals/opens SSH binary packets, verifying the tag in constant time
//! BEFORE releasing any plaintext. No socket — host-KAT-provable.
//!
//! Construction (per the spec): the 64-byte key is split into `K_2` (bytes
//! 0..32, the "main" key for the payload) and `K_1` (bytes 32..64, the "header"
//! key for the 4-byte length field). The nonce is the 64-bit packet sequence
//! number, big-endian, in the low 8 bytes of the 96-bit ChaCha20 nonce
//! (`[0;4] ++ seqnr.to_be_bytes()`), with the ChaCha20 block counter used
//! explicitly: counter 0 encrypts the length field and also yields the one-time
//! Poly1305 key (its first 32 keystream bytes), counter 1 encrypts the payload.
//! The Poly1305 MAC covers `encrypted_length || encrypted_payload`.

use crate::SshError;
use alloc::vec::Vec;
use ath_crypto::chacha20poly1305::{chacha20_xor, poly1305_mac};

/// A directional packet cipher (one per direction: client→server, server→
/// client), keyed from that direction's 64 bytes of KEX-derived key material.
pub struct ChaChaPolyCipher {
    /// K_2 — payload / Poly1305-key stream (key material bytes 0..32).
    k_main: [u8; 32],
    /// K_1 — length-field stream (key material bytes 32..64).
    k_header: [u8; 32],
}

/// Bytes of the Poly1305 tag appended to every packet.
pub const TAG_LEN: usize = 16;
/// Bytes of the (encrypted) packet-length prefix.
pub const LEN_LEN: usize = 4;
/// SSH requires `padding_length || payload || padding` to be a multiple of this
/// (the cipher "block size"; ChaCha20 is a stream cipher so it is 8). The 4-byte
/// length field is NOT included — it rides its own keystream.
const BLOCK: usize = 8;
/// Minimum random padding (RFC 4253 §6).
const MIN_PAD: usize = 4;
/// Upper bound on a decrypted packet length, matching the plaintext framing cap
/// (`crate::MAX_PACKET`) so a hostile encrypted length can't drive a huge alloc.
const MAX_INNER: u32 = crate::MAX_PACKET as u32;

impl ChaChaPolyCipher {
    /// Split 64 bytes of KEX-derived key material into the two ChaCha20 keys.
    /// (For SSH the two directions use derivation letters C/D — see
    /// [`crate::kex::derive_key`] — each producing 64 bytes.)
    pub fn from_key_material(km: &[u8; 64]) -> Self {
        let mut k_main = [0u8; 32];
        let mut k_header = [0u8; 32];
        k_main.copy_from_slice(&km[0..32]);
        k_header.copy_from_slice(&km[32..64]);
        Self { k_main, k_header }
    }

    /// The 96-bit ChaCha20 nonce for a given packet sequence number: the low 8
    /// bytes carry the sequence number big-endian, the high 4 are zero.
    fn nonce(seqnr: u64) -> [u8; 12] {
        let mut n = [0u8; 12];
        n[4..12].copy_from_slice(&seqnr.to_be_bytes());
        n
    }

    /// The one-time Poly1305 key for this packet: the first 32 bytes of the
    /// `K_2` keystream at counter 0.
    fn poly_key(&self, nonce: &[u8; 12]) -> [u8; 32] {
        let ks = chacha20_xor(&self.k_main, nonce, 0, &[0u8; 32]);
        let mut pk = [0u8; 32];
        pk.copy_from_slice(&ks);
        pk
    }

    /// Seal a `payload` into a wire packet: `enc_len(4) || enc_payload || tag(16)`.
    /// `pad_fill` fills the random-padding bytes (production passes CSPRNG bytes;
    /// tests pass a fixed value for determinism). The padding makes
    /// `padding_length + payload + padding` a multiple of 8 with ≥4 pad bytes.
    pub fn seal(&self, seqnr: u64, payload: &[u8], pad_fill: u8) -> Vec<u8> {
        // padding_length byte (1) + payload, padded to a multiple of BLOCK.
        let unpadded = 1 + payload.len();
        let mut pad_len = BLOCK - (unpadded % BLOCK);
        if pad_len < MIN_PAD {
            pad_len += BLOCK;
        }
        // The inner packet: [padding_length][payload][padding].
        let inner_len = 1 + payload.len() + pad_len; // == packet_length field value
        let mut inner = Vec::with_capacity(inner_len);
        inner.push(pad_len as u8);
        inner.extend_from_slice(payload);
        inner.resize(inner_len, pad_fill);

        let nonce = Self::nonce(seqnr);
        let enc_len = chacha20_xor(&self.k_header, &nonce, 0, &(inner_len as u32).to_be_bytes());
        let enc_payload = chacha20_xor(&self.k_main, &nonce, 1, &inner);

        let mut wire = Vec::with_capacity(LEN_LEN + enc_payload.len() + TAG_LEN);
        wire.extend_from_slice(&enc_len);
        wire.extend_from_slice(&enc_payload);
        let tag = poly1305_mac(&self.poly_key(&nonce), &wire);
        wire.extend_from_slice(&tag);
        wire
    }

    /// Decrypt just the 4-byte length prefix (RFC needs this to know how many
    /// bytes to read from the socket before the whole packet has arrived).
    /// Returns the inner packet length (the count of encrypted payload bytes
    /// between the length field and the tag). Rejects an out-of-range length so
    /// a hostile peer can't request an enormous read/alloc.
    pub fn decrypt_length(&self, seqnr: u64, enc_len: &[u8; 4]) -> Result<u32, SshError> {
        let nonce = Self::nonce(seqnr);
        let dec = chacha20_xor(&self.k_header, &nonce, 0, enc_len);
        let len = u32::from_be_bytes([dec[0], dec[1], dec[2], dec[3]]);
        // Inner = padding_length(1) + payload + padding(≥4); minimum 8, block-aligned.
        if len < (BLOCK as u32) || len > MAX_INNER || (len as usize) % BLOCK != 0 {
            return Err(SshError::Malformed);
        }
        Ok(len)
    }

    /// Verify + open a complete wire packet, returning the decrypted payload
    /// (with the padding stripped). Verifies the Poly1305 tag in constant time
    /// and releases NO plaintext on any failure. `wire` must be the full
    /// `enc_len(4) || enc_payload || tag(16)`; use [`Self::decrypt_length`] to
    /// size the read first. Returns `NeedMoreData` if `wire` is short.
    pub fn open(&self, seqnr: u64, wire: &[u8]) -> Result<Vec<u8>, SshError> {
        if wire.len() < LEN_LEN + TAG_LEN {
            return Err(SshError::NeedMoreData);
        }
        let enc_len: [u8; 4] = wire[0..4].try_into().map_err(|_| SshError::Malformed)?;
        let inner_len = self.decrypt_length(seqnr, &enc_len)? as usize;
        let total = LEN_LEN + inner_len + TAG_LEN;
        if wire.len() < total {
            return Err(SshError::NeedMoreData);
        }
        let nonce = Self::nonce(seqnr);
        // Authenticate enc_len || enc_payload (everything before the tag).
        let mac_data = &wire[..LEN_LEN + inner_len];
        let recv_tag = &wire[LEN_LEN + inner_len..total];
        let expect = poly1305_mac(&self.poly_key(&nonce), mac_data);
        let mut diff = 0u8;
        for i in 0..TAG_LEN {
            diff |= expect[i] ^ recv_tag[i];
        }
        if diff != 0 {
            return Err(SshError::BadMac);
        }
        // Tag OK — decrypt the inner packet and strip the padding.
        let inner = chacha20_xor(&self.k_main, &nonce, 1, &wire[LEN_LEN..LEN_LEN + inner_len]);
        let pad_len = inner[0] as usize;
        // padding_length(1) + payload(≥0) + padding(pad_len); pad_len ≥ 4.
        if pad_len < MIN_PAD || 1 + pad_len > inner_len {
            return Err(SshError::Malformed);
        }
        let payload = inner[1..inner_len - pad_len].to_vec();
        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn km(seed: u8) -> [u8; 64] {
        let mut k = [0u8; 64];
        for (i, b) in k.iter_mut().enumerate() {
            *b = seed.wrapping_add(i as u8);
        }
        k
    }

    #[test]
    fn seal_open_roundtrip_various_lengths() {
        let c = ChaChaPolyCipher::from_key_material(&km(1));
        for len in [0usize, 1, 5, 7, 8, 31, 100, 1000] {
            let payload: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
            let wire = c.seal(42, &payload, 0);
            // Wire = 4 (len) + inner + 16 (tag); inner is block-aligned ≥ 8.
            let inner = wire.len() - LEN_LEN - TAG_LEN;
            assert_eq!(inner % BLOCK, 0, "inner must be block-aligned");
            assert!(inner >= BLOCK);
            assert_eq!(c.open(42, &wire).unwrap(), payload);
        }
    }

    #[test]
    fn padding_is_block_aligned_and_at_least_four() {
        let c = ChaChaPolyCipher::from_key_material(&km(9));
        // Decrypt the inner packet ourselves to inspect the padding_length byte.
        for len in 0..40usize {
            let payload = vec![0xABu8; len];
            let wire = c.seal(0, &payload, 0);
            let inner_len = c
                .decrypt_length(0, &wire[0..4].try_into().unwrap())
                .unwrap() as usize;
            let inner = chacha20_xor(
                &c.k_main,
                &ChaChaPolyCipher::nonce(0),
                1,
                &wire[4..4 + inner_len],
            );
            let pad_len = inner[0] as usize;
            assert!(
                pad_len >= MIN_PAD,
                "pad {} < 4 for payload {}",
                pad_len,
                len
            );
            assert_eq!((1 + len + pad_len) % BLOCK, 0);
        }
    }

    #[test]
    fn decrypt_length_matches_sealed_length() {
        let c = ChaChaPolyCipher::from_key_material(&km(3));
        let payload = vec![7u8; 50];
        let wire = c.seal(1234, &payload, 0);
        let inner_len = c
            .decrypt_length(1234, &wire[0..4].try_into().unwrap())
            .unwrap() as usize;
        assert_eq!(inner_len, wire.len() - LEN_LEN - TAG_LEN);
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let c = ChaChaPolyCipher::from_key_material(&km(5));
        let payload = vec![1u8, 2, 3, 4, 5];
        // Flip each region: a payload byte, a length byte, a tag byte.
        for idx in [5usize, 0 /* tag */] {
            let mut wire = c.seal(7, &payload, 0);
            wire[idx] ^= 0x01;
            assert!(matches!(
                c.open(7, &wire),
                Err(SshError::BadMac) | Err(SshError::Malformed)
            ));
        }
        let mut wire = c.seal(7, &payload, 0);
        let n = wire.len();
        wire[n - 1] ^= 0x01; // tag
        assert_eq!(c.open(7, &wire), Err(SshError::BadMac));
    }

    #[test]
    fn wrong_sequence_number_is_rejected() {
        // The seqnr is the nonce — replaying a packet under a different seqnr (or
        // a reordering attacker) must fail the MAC, not silently decrypt.
        let c = ChaChaPolyCipher::from_key_material(&km(11));
        let wire = c.seal(100, b"secret", 0);
        assert!(matches!(
            c.open(101, &wire),
            Err(SshError::BadMac) | Err(SshError::Malformed)
        ));
        assert_eq!(c.open(100, &wire).unwrap(), b"secret");
    }

    #[test]
    fn wrong_key_is_rejected() {
        let a = ChaChaPolyCipher::from_key_material(&km(1));
        let b = ChaChaPolyCipher::from_key_material(&km(2));
        let wire = a.seal(0, b"hello", 0);
        assert!(matches!(
            b.open(0, &wire),
            Err(SshError::BadMac) | Err(SshError::Malformed)
        ));
    }

    #[test]
    fn short_and_hostile_wire_never_panics() {
        let c = ChaChaPolyCipher::from_key_material(&km(1));
        assert_eq!(c.open(0, &[]), Err(SshError::NeedMoreData));
        assert_eq!(c.open(0, &[0u8; 10]), Err(SshError::NeedMoreData));
        // 20 bytes: enough for len+tag but the decrypted length will be random —
        // must be rejected (Malformed) or need-more, never a panic/huge alloc.
        assert!(matches!(
            c.open(0, &[0u8; 20]),
            Err(SshError::Malformed) | Err(SshError::NeedMoreData) | Err(SshError::BadMac)
        ));
    }

    // Anchor the nonce/counter mapping to ath_crypto's RFC-8439-KAT'd ChaCha20:
    // the payload keystream is K_main at counter 1 with our nonce, so XORing the
    // sealed inner packet back with that exact keystream must recover the inner
    // plaintext. This ties the construction to an externally-proven primitive
    // (self-consistency alone can't catch a symmetric counter/nonce bug).
    #[test]
    fn payload_stream_is_k_main_counter_one() {
        let c = ChaChaPolyCipher::from_key_material(&km(1));
        let payload = b"map-to-primitive";
        let wire = c.seal(9, payload, 0xEE);
        let inner_len = wire.len() - LEN_LEN - TAG_LEN;
        let enc_inner = &wire[LEN_LEN..LEN_LEN + inner_len];
        let nonce = ChaChaPolyCipher::nonce(9);
        let recovered = chacha20_xor(&c.k_main, &nonce, 1, enc_inner);
        assert_eq!(recovered[0] as usize + 1 + payload.len(), inner_len); // padlen + payload + pad
        assert_eq!(&recovered[1..1 + payload.len()], payload);
    }
}
