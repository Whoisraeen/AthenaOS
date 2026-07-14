//! curve25519-sha256 key exchange (RFC 8731) + the exchange hash and key
//! derivation (RFC 4253 §7.2/§8) — the cryptographic core of the SSH handshake,
//! all on `ath_crypto` (`x25519`, `sha256`, `ed25519`). Pure logic: given the
//! peer's ephemeral public and our secrets it produces the shared secret `K`,
//! the exchange hash `H`, the ssh-ed25519 signature over `H`, and the six
//! directional session keys — no socket, host-KAT-provable.

use crate::{write_string, SshError};
use alloc::vec::Vec;
use ath_crypto::ed25519;
use ath_crypto::sha256::sha256;
use ath_crypto::x25519::x25519;

/// SSH KEX message numbers (RFC 5656 / 8731).
pub const SSH_MSG_KEX_ECDH_INIT: u8 = 30;
pub const SSH_MSG_KEX_ECDH_REPLY: u8 = 31;

/// The Curve25519 base point `u = 9` (RFC 7748): `X25519(scalar, BASEPOINT)` is
/// the public key for `scalar`.
pub const X25519_BASEPOINT: [u8; 32] = {
    let mut b = [0u8; 32];
    b[0] = 9;
    b
};

/// Encode an unsigned big-endian integer as an SSH `mpint` CONTENT (RFC 4251
/// §5): drop leading zero bytes, and if the result's high bit is set prepend a
/// `0x00` so it reads as positive two's-complement. Zero encodes as empty.
pub fn encode_mpint(unsigned_be: &[u8]) -> Vec<u8> {
    let first_nonzero = unsigned_be.iter().position(|&b| b != 0);
    let trimmed = match first_nonzero {
        Some(i) => &unsigned_be[i..],
        None => return Vec::new(), // the integer is zero
    };
    let mut out = Vec::with_capacity(trimmed.len() + 1);
    if trimmed[0] & 0x80 != 0 {
        out.push(0);
    }
    out.extend_from_slice(trimmed);
    out
}

/// Append an unsigned integer as a full SSH `mpint` (a length-prefixed string
/// wrapping [`encode_mpint`]).
pub fn write_mpint(out: &mut Vec<u8>, unsigned_be: &[u8]) {
    let content = encode_mpint(unsigned_be);
    write_string(out, &content);
}

/// The ssh-ed25519 public host-key blob `K_S` (RFC 8709):
/// `string "ssh-ed25519" | string pubkey`.
pub fn ed25519_hostkey_blob(pubkey: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(51);
    write_string(&mut out, b"ssh-ed25519");
    write_string(&mut out, pubkey);
    out
}

/// The ssh-ed25519 signature blob (RFC 8709):
/// `string "ssh-ed25519" | string signature`.
pub fn ed25519_sig_blob(sig: &[u8; 64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(83);
    write_string(&mut out, b"ssh-ed25519");
    write_string(&mut out, sig);
    out
}

/// The exchange hash `H = SHA256(V_C || V_S || I_C || I_S || K_S || Q_C || Q_S
/// || K)` (RFC 4253 §8, curve25519 form). Every field is length-prefixed; `K`
/// is an mpint. `H` is both what the host key signs and (on the first exchange)
/// the session identifier.
#[allow(clippy::too_many_arguments)]
pub fn exchange_hash(
    v_c: &[u8],
    v_s: &[u8],
    i_c: &[u8],
    i_s: &[u8],
    k_s: &[u8],
    q_c: &[u8],
    q_s: &[u8],
    k_unsigned: &[u8],
) -> [u8; 32] {
    let mut buf = Vec::with_capacity(256);
    write_string(&mut buf, v_c);
    write_string(&mut buf, v_s);
    write_string(&mut buf, i_c);
    write_string(&mut buf, i_s);
    write_string(&mut buf, k_s);
    write_string(&mut buf, q_c);
    write_string(&mut buf, q_s);
    write_mpint(&mut buf, k_unsigned);
    sha256(&buf)
}

/// One of the six directional keys (RFC 4253 §7.2). `letter` is `b'A'..=b'F'`:
/// A/B = IVs, C/D = encryption keys, E/F = MAC keys (client→server / server→
/// client). Derives `need` bytes via `K1 = HASH(K||H||letter||session_id)` then
/// `Kn = HASH(K||H||K1||…||K{n-1})` until long enough.
pub fn derive_key(
    k_unsigned: &[u8],
    h: &[u8; 32],
    letter: u8,
    session_id: &[u8; 32],
    need: usize,
) -> Vec<u8> {
    let mut k_mpint = Vec::new();
    write_mpint(&mut k_mpint, k_unsigned);

    // K1 = HASH(K || H || letter || session_id)
    let mut first = Vec::with_capacity(k_mpint.len() + 32 + 1 + 32);
    first.extend_from_slice(&k_mpint);
    first.extend_from_slice(h);
    first.push(letter);
    first.extend_from_slice(session_id);

    let mut out = Vec::with_capacity(((need + 31) / 32) * 32);
    out.extend_from_slice(&sha256(&first));
    while out.len() < need {
        // K_{n+1} = HASH(K || H || K1 || … || Kn)
        let mut next = Vec::with_capacity(k_mpint.len() + 32 + out.len());
        next.extend_from_slice(&k_mpint);
        next.extend_from_slice(h);
        next.extend_from_slice(&out);
        out.extend_from_slice(&sha256(&next));
    }
    out.truncate(need);
    out
}

/// Parse a `SSH_MSG_KEX_ECDH_INIT` (msg 30) payload, returning the client's
/// 32-byte ephemeral public `Q_C`. Rejects a wrong message type / bad length.
pub fn parse_ecdh_init(payload: &[u8]) -> Result<[u8; 32], SshError> {
    if payload.first() != Some(&SSH_MSG_KEX_ECDH_INIT) {
        return Err(SshError::Unexpected);
    }
    let (qc, _) = crate::read_string(payload, 1)?;
    let arr: [u8; 32] = qc.try_into().map_err(|_| SshError::Malformed)?;
    Ok(arr)
}

/// The server side of curve25519 KEX. Takes the transcript (`v_c/v_s/i_c/i_s`),
/// the ed25519 host key (`host_seed`/`host_pub`), the server ephemeral secret
/// (`server_eph_secret` — random per connection; a test supplies a fixed one),
/// and the client's ephemeral public `q_c`. Produces the exchange hash `H`, the
/// signature over it, and the ready-to-send `SSH_MSG_KEX_ECDH_REPLY` payload,
/// plus the shared secret `K` for key derivation.
pub struct KexResult {
    pub h: [u8; 32],
    pub k_unsigned: [u8; 32],
    pub q_s: [u8; 32],
    pub reply_payload: Vec<u8>,
}

#[allow(clippy::too_many_arguments)]
pub fn server_ecdh(
    v_c: &[u8],
    v_s: &[u8],
    i_c: &[u8],
    i_s: &[u8],
    host_seed: &[u8; 32],
    host_pub: &[u8; 32],
    server_eph_secret: &[u8; 32],
    q_c: &[u8; 32],
) -> KexResult {
    let q_s = x25519(server_eph_secret, &X25519_BASEPOINT);
    let k_unsigned = x25519(server_eph_secret, q_c);
    let k_s = ed25519_hostkey_blob(host_pub);
    let h = exchange_hash(v_c, v_s, i_c, i_s, &k_s, q_c, &q_s, &k_unsigned);
    let sig = ed25519::sign(host_seed, &h);
    let sig_blob = ed25519_sig_blob(&sig);

    // SSH_MSG_KEX_ECDH_REPLY: byte 31 | string K_S | string Q_S | string sig
    let mut reply = Vec::with_capacity(1 + k_s.len() + 36 + sig_blob.len());
    reply.push(SSH_MSG_KEX_ECDH_REPLY);
    write_string(&mut reply, &k_s);
    write_string(&mut reply, &q_s);
    write_string(&mut reply, &sig_blob);

    KexResult {
        h,
        k_unsigned,
        q_s,
        reply_payload: reply,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
    fn hex32(s: &str) -> [u8; 32] {
        hex(s).try_into().unwrap()
    }

    #[test]
    fn mpint_rfc4251_vectors() {
        // RFC 4251 §5 mpint examples (CONTENT only; the wire adds a length).
        assert_eq!(encode_mpint(&[]), Vec::<u8>::new()); // zero
        assert_eq!(encode_mpint(&[0, 0, 0]), Vec::<u8>::new()); // zero
                                                                // 0x09a378f9b2e332a7 -> unchanged (high byte 0x09, MSB clear).
        assert_eq!(
            encode_mpint(&hex("09a378f9b2e332a7")),
            hex("09a378f9b2e332a7")
        );
        // 0x80 -> prepend 0x00 (MSB set, must stay positive).
        assert_eq!(encode_mpint(&[0x80]), vec![0x00, 0x80]);
        // leading zeros are stripped first.
        assert_eq!(encode_mpint(&[0x00, 0x00, 0x01]), vec![0x01]);
    }

    #[test]
    fn x25519_rfc7748_vector_and_basepoint() {
        // RFC 7748 §6.1 Alice/Bob.
        let a_priv = hex32("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
        let a_pub = hex32("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a");
        let b_priv = hex32("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb");
        let b_pub = hex32("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f");
        let shared = hex32("4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");
        // Public keys derive from private via the base point.
        assert_eq!(x25519(&a_priv, &X25519_BASEPOINT), a_pub);
        assert_eq!(x25519(&b_priv, &X25519_BASEPOINT), b_pub);
        // Both sides compute the same shared secret.
        assert_eq!(x25519(&a_priv, &b_pub), shared);
        assert_eq!(x25519(&b_priv, &a_pub), shared);
    }

    #[test]
    fn hostkey_and_sig_blobs_are_wellformed() {
        let pk = [0x11u8; 32];
        let blob = ed25519_hostkey_blob(&pk);
        // "ssh-ed25519" (len 11) + pubkey (len 32) as two strings.
        let (algo, next) = crate::read_string(&blob, 0).unwrap();
        assert_eq!(algo, b"ssh-ed25519");
        let (key, end) = crate::read_string(&blob, next).unwrap();
        assert_eq!(key, &pk);
        assert_eq!(end, blob.len());
    }

    #[test]
    fn full_server_kex_signature_verifies_and_reply_parses() {
        // A deterministic end-to-end KEX (fixed ephemerals) — no citable byte
        // vector exists with pinned randomness, so the FAIL-able invariant is:
        // the host-key signature over H verifies, and the reply is parseable.
        let host_seed = [0x42u8; 32];
        let host_pub = ed25519::derive_public_key(&host_seed);
        let client_secret =
            hex32("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a");
        let q_c = x25519(&client_secret, &X25519_BASEPOINT);
        let server_secret = [0x03u8; 32];

        let r = server_ecdh(
            b"SSH-2.0-OpenSSH_9.6",
            crate::IDENT,
            &[20, 1, 2, 3], // stand-in I_C payload
            &[20, 4, 5, 6], // stand-in I_S payload
            &host_seed,
            &host_pub,
            &server_secret,
            &q_c,
        );

        // The signature the client will check MUST verify over H.
        // (Re-extract the sig from the reply to prove the wire form is right.)
        let (k_s, p1) = crate::read_string(&r.reply_payload, 1).unwrap();
        let (q_s_wire, p2) = crate::read_string(&r.reply_payload, p1).unwrap();
        let (sig_blob, _) = crate::read_string(&r.reply_payload, p2).unwrap();
        assert_eq!(k_s, ed25519_hostkey_blob(&host_pub));
        assert_eq!(q_s_wire, &r.q_s);
        let (sig_algo, sp) = crate::read_string(sig_blob, 0).unwrap();
        assert_eq!(sig_algo, b"ssh-ed25519");
        let (sig_bytes, _) = crate::read_string(sig_blob, sp).unwrap();
        let sig: [u8; 64] = sig_bytes.try_into().unwrap();
        assert!(
            ed25519::verify(&host_pub, &r.h, &sig),
            "H signature must verify"
        );

        // The client independently computes the SAME shared secret.
        assert_eq!(x25519(&client_secret, &r.q_s), r.k_unsigned);

        // A tampered H must NOT verify (the guard actually catches a bad sig).
        let mut bad_h = r.h;
        bad_h[0] ^= 1;
        assert!(!ed25519::verify(&host_pub, &bad_h, &sig));
    }

    #[test]
    fn key_derivation_is_deterministic_and_extends_past_one_block() {
        let k = [0x07u8; 32];
        let h = [0x09u8; 32];
        // A 64-byte key (chacha20-poly1305@openssh needs two 32-byte halves)
        // forces the multi-block extension path.
        let c2s = derive_key(&k, &h, b'C', &h, 64);
        assert_eq!(c2s.len(), 64);
        assert_eq!(derive_key(&k, &h, b'C', &h, 64), c2s); // deterministic
                                                           // Different letters -> different keys.
        assert_ne!(derive_key(&k, &h, b'D', &h, 64), c2s);
        // The first 32 bytes match a 32-byte derivation (prefix stability).
        assert_eq!(derive_key(&k, &h, b'C', &h, 32), c2s[..32]);
    }

    #[test]
    fn parse_ecdh_init_extracts_client_public() {
        let q_c = [0x55u8; 32];
        let mut payload = vec![SSH_MSG_KEX_ECDH_INIT];
        write_string(&mut payload, &q_c);
        assert_eq!(parse_ecdh_init(&payload).unwrap(), q_c);
        // Wrong message code.
        payload[0] = SSH_MSG_KEX_ECDH_REPLY;
        assert_eq!(parse_ecdh_init(&payload), Err(SshError::Unexpected));
    }
}
