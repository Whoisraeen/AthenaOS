//! `ssh-userauth` — publickey authentication (RFC 4252 §7), ed25519 only.
//!
//! After NEWKEYS the client requests the `ssh-userauth` service, then sends a
//! `SSH_MSG_USERAUTH_REQUEST` with method `publickey`. Two forms (RFC 4252 §7):
//! a *query* (`has_signature=false`) asking "would this key be accepted?" — the
//! server answers `SSH_MSG_USERAUTH_PK_OK` — and a real *attempt*
//! (`has_signature=true`) carrying a signature the server verifies. The
//! signature is over a transcript that includes the session identifier `H`, so
//! it is bound to THIS connection and cannot be replayed onto another.
//!
//! Pure logic on `ath_crypto::ed25519`: parse a request, rebuild the exact
//! signed transcript, verify against an allow-list of authorized keys. Never
//! panics on hostile bytes; a wrong/unauthorized/tampered key fails CLOSED.

use crate::{read_string, write_string, SshError, SSH_MSG_USERAUTH_REQUEST};
use alloc::string::String;
use alloc::vec::Vec;
use ath_crypto::ed25519;

/// The connection service authenticated into (RFC 4254).
pub const SERVICE_CONNECTION: &str = "ssh-connection";
/// The one public-key algorithm we accept.
pub const ALGO_ED25519: &str = "ssh-ed25519";

/// An ed25519 key the server will accept (the `authorized_keys` allow-list
/// entry) — the raw 32-byte public key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorizedKey {
    pub pubkey: [u8; 32],
}

/// A parsed `publickey` `SSH_MSG_USERAUTH_REQUEST`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublickeyRequest {
    pub user: String,
    pub service: String,
    /// The advertised public-key algorithm (`ssh-ed25519`).
    pub algo: String,
    /// The raw 32-byte ed25519 public key (unwrapped from its blob).
    pub pubkey: [u8; 32],
    /// The full public-key blob as it appeared on the wire (`string "ssh-ed25519"
    /// || string pubkey`) — needed verbatim to rebuild the signed transcript.
    pub key_blob: Vec<u8>,
    /// Present only on a real attempt (`has_signature=true`): the raw 64-byte
    /// ed25519 signature (unwrapped from its `string "ssh-ed25519" || string sig`
    /// blob).
    pub signature: Option<[u8; 64]>,
}

/// What the server should do with a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthOutcome {
    /// Authenticated — send `SSH_MSG_USERAUTH_SUCCESS`.
    Success,
    /// A query for an authorized key — send `SSH_MSG_USERAUTH_PK_OK` (the client
    /// will then send the signed attempt).
    PkOk,
    /// Rejected (unknown/unauthorized key, bad signature, wrong service/algo) —
    /// send `SSH_MSG_USERAUTH_FAILURE`. Fail closed.
    Failure,
}

/// Unwrap a `string "ssh-ed25519" || string blob` structure, returning the inner
/// `blob` bytes (the 32-byte key or 64-byte signature). Rejects a wrong algo.
fn unwrap_ed25519_blob(blob: &[u8]) -> Result<&[u8], SshError> {
    let (algo, pos) = read_string(blob, 0)?;
    if algo != ALGO_ED25519.as_bytes() {
        return Err(SshError::Unexpected);
    }
    let (inner, end) = read_string(blob, pos)?;
    // The blob must be exactly these two strings, nothing trailing.
    if end != blob.len() {
        return Err(SshError::Malformed);
    }
    Ok(inner)
}

/// Parse a `publickey` `SSH_MSG_USERAUTH_REQUEST` payload (RFC 4252 §7). Rejects
/// a non-publickey method / wrong message code / structurally bad bytes without
/// panicking (a remote attacker controls every byte).
pub fn parse_publickey_request(payload: &[u8]) -> Result<PublickeyRequest, SshError> {
    if payload.first() != Some(&SSH_MSG_USERAUTH_REQUEST) {
        return Err(SshError::Unexpected);
    }
    let mut pos = 1;
    let user_bytes;
    (user_bytes, pos) = read_string(payload, pos)?;
    let service_bytes;
    (service_bytes, pos) = read_string(payload, pos)?;
    let method;
    (method, pos) = read_string(payload, pos)?;
    if method != b"publickey" {
        return Err(SshError::Unexpected);
    }
    // boolean has_signature (RFC 4251 §5: a byte, 0 or 1).
    if pos >= payload.len() {
        return Err(SshError::NeedMoreData);
    }
    let has_signature = payload[pos] != 0;
    pos += 1;
    let algo_bytes;
    (algo_bytes, pos) = read_string(payload, pos)?;
    let key_blob_bytes;
    (key_blob_bytes, pos) = read_string(payload, pos)?;

    // The advertised algo and the algo inside the blob must both be ed25519, and
    // the inner key must be exactly 32 bytes.
    if algo_bytes != ALGO_ED25519.as_bytes() {
        return Err(SshError::Unexpected);
    }
    let raw_key = unwrap_ed25519_blob(key_blob_bytes)?;
    let pubkey: [u8; 32] = raw_key.try_into().map_err(|_| SshError::Malformed)?;

    let signature = if has_signature {
        let (sig_blob, end) = read_string(payload, pos)?;
        if end != payload.len() {
            return Err(SshError::Malformed);
        }
        let raw_sig = unwrap_ed25519_blob(sig_blob)?;
        let sig: [u8; 64] = raw_sig.try_into().map_err(|_| SshError::Malformed)?;
        Some(sig)
    } else {
        None
    };

    Ok(PublickeyRequest {
        user: String::from_utf8(user_bytes.to_vec()).map_err(|_| SshError::Malformed)?,
        service: String::from_utf8(service_bytes.to_vec()).map_err(|_| SshError::Malformed)?,
        algo: String::from_utf8(algo_bytes.to_vec()).map_err(|_| SshError::Malformed)?,
        pubkey,
        key_blob: key_blob_bytes.to_vec(),
        signature,
    })
}

/// The exact byte string a publickey signature covers (RFC 4252 §7):
/// `string session_id | byte 50 | string user | string service | string
/// "publickey" | boolean TRUE | string "ssh-ed25519" | string pubkey_blob`.
/// Both signer (client) and verifier (server) build this identically.
pub fn signed_data(session_id: &[u8; 32], req: &PublickeyRequest) -> Vec<u8> {
    let mut buf = Vec::with_capacity(160 + req.key_blob.len());
    write_string(&mut buf, session_id);
    buf.push(SSH_MSG_USERAUTH_REQUEST);
    write_string(&mut buf, req.user.as_bytes());
    write_string(&mut buf, req.service.as_bytes());
    write_string(&mut buf, b"publickey");
    buf.push(1); // has_signature = TRUE (always TRUE in the signed transcript)
    write_string(&mut buf, ALGO_ED25519.as_bytes());
    write_string(&mut buf, &req.key_blob);
    buf
}

/// The server decision for a parsed request, given the session identifier `H`
/// and the allow-list. Enforces (in order, all fail-closed): the service is
/// `ssh-connection`, the algo is ed25519, the key is authorized, and — on a real
/// attempt — the ed25519 signature over [`signed_data`] verifies. A query for an
/// authorized key returns `PkOk`; anything wrong returns `Failure`.
pub fn authenticate(
    session_id: &[u8; 32],
    req: &PublickeyRequest,
    authorized: &[AuthorizedKey],
) -> AuthOutcome {
    if req.service != SERVICE_CONNECTION || req.algo != ALGO_ED25519 {
        return AuthOutcome::Failure;
    }
    if !authorized.iter().any(|k| k.pubkey == req.pubkey) {
        return AuthOutcome::Failure;
    }
    match req.signature {
        None => AuthOutcome::PkOk,
        Some(sig) => {
            let data = signed_data(session_id, req);
            if ed25519::verify(&req.pubkey, &data, &sig) {
                AuthOutcome::Success
            } else {
                AuthOutcome::Failure
            }
        }
    }
}

/// Build a `publickey` `SSH_MSG_USERAUTH_REQUEST` (client side / test helper).
/// If `signature` is `Some`, it is appended and `has_signature` is TRUE.
pub fn build_publickey_request(
    user: &str,
    service: &str,
    pubkey: &[u8; 32],
    signature: Option<&[u8; 64]>,
) -> Vec<u8> {
    let mut key_blob = Vec::with_capacity(51);
    write_string(&mut key_blob, ALGO_ED25519.as_bytes());
    write_string(&mut key_blob, pubkey);

    let mut out = Vec::with_capacity(96 + key_blob.len());
    out.push(SSH_MSG_USERAUTH_REQUEST);
    write_string(&mut out, user.as_bytes());
    write_string(&mut out, service.as_bytes());
    write_string(&mut out, b"publickey");
    out.push(signature.is_some() as u8);
    write_string(&mut out, ALGO_ED25519.as_bytes());
    write_string(&mut out, &key_blob);
    if let Some(sig) = signature {
        let mut sig_blob = Vec::with_capacity(83);
        write_string(&mut sig_blob, ALGO_ED25519.as_bytes());
        write_string(&mut sig_blob, sig);
        write_string(&mut out, &sig_blob);
    }
    out
}

/// Build `SSH_MSG_USERAUTH_PK_OK` (RFC 4252 §7): echoes the algo + key blob the
/// query offered, telling the client to send the signed attempt.
pub fn build_pk_ok(pubkey: &[u8; 32]) -> Vec<u8> {
    let mut key_blob = Vec::with_capacity(51);
    write_string(&mut key_blob, ALGO_ED25519.as_bytes());
    write_string(&mut key_blob, pubkey);
    let mut out = Vec::with_capacity(8 + key_blob.len());
    out.push(crate::SSH_MSG_USERAUTH_PK_OK);
    write_string(&mut out, ALGO_ED25519.as_bytes());
    write_string(&mut out, &key_blob);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // A deterministic ed25519 keypair for the tests.
    fn keypair(seed_byte: u8) -> ([u8; 32], [u8; 32]) {
        let seed = [seed_byte; 32];
        (seed, ed25519::derive_public_key(&seed))
    }

    /// Client builds a query, signs the transcript, server accepts — the full
    /// happy path, and the signature genuinely verifies via ath_crypto.
    #[test]
    fn authorized_signed_attempt_succeeds() {
        let sid = [0x11u8; 32];
        let (seed, pubkey) = keypair(7);
        let authorized = [AuthorizedKey { pubkey }];

        // 1) Query (no signature) -> PkOk.
        let query = build_publickey_request("athena", SERVICE_CONNECTION, &pubkey, None);
        let qreq = parse_publickey_request(&query).unwrap();
        assert_eq!(authenticate(&sid, &qreq, &authorized), AuthOutcome::PkOk);

        // 2) Real attempt: sign the transcript, server verifies -> Success.
        let unsigned = PublickeyRequest {
            signature: None,
            ..qreq.clone()
        };
        let sig = ed25519::sign(&seed, &signed_data(&sid, &unsigned));
        let attempt = build_publickey_request("athena", SERVICE_CONNECTION, &pubkey, Some(&sig));
        let areq = parse_publickey_request(&attempt).unwrap();
        assert_eq!(areq.signature, Some(sig));
        assert_eq!(authenticate(&sid, &areq, &authorized), AuthOutcome::Success);
    }

    /// A key not on the allow-list is rejected even with a valid self-signature.
    #[test]
    fn unauthorized_key_fails() {
        let sid = [0x22u8; 32];
        let (seed, pubkey) = keypair(9);
        let (_other_seed, other_pub) = keypair(10);
        let authorized = [AuthorizedKey { pubkey: other_pub }]; // NOT our key

        let base = parse_publickey_request(&build_publickey_request(
            "athena",
            SERVICE_CONNECTION,
            &pubkey,
            None,
        ))
        .unwrap();
        let sig = ed25519::sign(&seed, &signed_data(&sid, &base));
        let attempt = build_publickey_request("athena", SERVICE_CONNECTION, &pubkey, Some(&sig));
        let req = parse_publickey_request(&attempt).unwrap();
        assert_eq!(authenticate(&sid, &req, &authorized), AuthOutcome::Failure);
    }

    /// A tampered signature (authorized key) fails — the signature is real crypto.
    #[test]
    fn tampered_signature_fails() {
        let sid = [0x33u8; 32];
        let (seed, pubkey) = keypair(3);
        let authorized = [AuthorizedKey { pubkey }];
        let base = parse_publickey_request(&build_publickey_request(
            "athena",
            SERVICE_CONNECTION,
            &pubkey,
            None,
        ))
        .unwrap();
        let mut sig = ed25519::sign(&seed, &signed_data(&sid, &base));
        sig[0] ^= 0x01;
        let attempt = build_publickey_request("athena", SERVICE_CONNECTION, &pubkey, Some(&sig));
        let req = parse_publickey_request(&attempt).unwrap();
        assert_eq!(authenticate(&sid, &req, &authorized), AuthOutcome::Failure);
    }

    /// A signature bound to a DIFFERENT session id fails — replay/channel-binding
    /// protection (the whole point of hashing `H` into the transcript).
    #[test]
    fn signature_from_another_session_fails() {
        let (seed, pubkey) = keypair(4);
        let authorized = [AuthorizedKey { pubkey }];
        let base = parse_publickey_request(&build_publickey_request(
            "athena",
            SERVICE_CONNECTION,
            &pubkey,
            None,
        ))
        .unwrap();
        // Sign under session A, verify under session B.
        let sig_a = ed25519::sign(&seed, &signed_data(&[0xAAu8; 32], &base));
        let attempt = build_publickey_request("athena", SERVICE_CONNECTION, &pubkey, Some(&sig_a));
        let req = parse_publickey_request(&attempt).unwrap();
        assert_eq!(
            authenticate(&[0xBBu8; 32], &req, &authorized),
            AuthOutcome::Failure
        );
    }

    /// Wrong service name is rejected even with a valid signature.
    #[test]
    fn wrong_service_fails() {
        let sid = [0x55u8; 32];
        let (seed, pubkey) = keypair(6);
        let authorized = [AuthorizedKey { pubkey }];
        let base = parse_publickey_request(&build_publickey_request(
            "athena",
            "ssh-userauth", // not ssh-connection
            &pubkey,
            None,
        ))
        .unwrap();
        let sig = ed25519::sign(&seed, &signed_data(&sid, &base));
        let attempt = build_publickey_request("athena", "ssh-userauth", &pubkey, Some(&sig));
        let req = parse_publickey_request(&attempt).unwrap();
        assert_eq!(authenticate(&sid, &req, &authorized), AuthOutcome::Failure);
    }

    #[test]
    fn parse_rejects_hostile_bytes() {
        // Empty / wrong message code.
        assert_eq!(parse_publickey_request(&[]), Err(SshError::Unexpected));
        assert_eq!(parse_publickey_request(&[99]), Err(SshError::Unexpected));
        // Truncated after the message code.
        assert!(matches!(
            parse_publickey_request(&[SSH_MSG_USERAUTH_REQUEST, 0, 0, 0, 5]),
            Err(SshError::NeedMoreData)
        ));
        // A non-publickey method is refused.
        let mut pw = Vec::new();
        pw.push(SSH_MSG_USERAUTH_REQUEST);
        write_string(&mut pw, b"athena");
        write_string(&mut pw, SERVICE_CONNECTION.as_bytes());
        write_string(&mut pw, b"password");
        assert_eq!(parse_publickey_request(&pw), Err(SshError::Unexpected));
    }

    #[test]
    fn pk_ok_roundtrips_the_key() {
        let (_s, pubkey) = keypair(2);
        let msg = build_pk_ok(&pubkey);
        assert_eq!(msg[0], crate::SSH_MSG_USERAUTH_PK_OK);
        let (algo, pos) = read_string(&msg, 1).unwrap();
        assert_eq!(algo, ALGO_ED25519.as_bytes());
        let (blob, _) = read_string(&msg, pos).unwrap();
        assert_eq!(unwrap_ed25519_blob(blob).unwrap(), &pubkey);
    }
}
