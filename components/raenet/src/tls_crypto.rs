//! TLS 1.3 crypto surface — Concept §"AthNet: real TLS 1.3, not a toy".
//!
//! The vetted RustCrypto primitives the AthNet userspace TLS path uses to go
//! from a bare handshake to a verified connection: the HKDF key schedule
//! (RFC 8446 §7.1 `HKDF-Expand-Label`), the AES-GCM record cipher, and X.509
//! certificate parsing + ECDSA(P-256/P-384)/RSA signature verification for the
//! server-certificate step. Gated behind the `tls13` feature so the kernel's
//! no_std link of raenet never pulls these in; the userspace daemon and the
//! host KATs (`cargo test -p raenet --features tls13`) do.

extern crate alloc;
use alloc::format;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

// ── HKDF key schedule (RFC 8446 §7.1) ────────────────────────────────────────

/// `HKDF-Expand-Label(Secret, Label, Context, Length)` exactly as TLS 1.3
/// defines it: the info is `len:u16 || "tls13 "+label (len-prefixed) ||
/// context (len-prefixed)`. The `secret` is a 32-byte PRK (the HKDF-Extract
/// output of a handshake stage).
pub fn hkdf_expand_label(secret: &[u8], label: &str, context: &[u8], length: usize) -> Vec<u8> {
    use hkdf::Hkdf;
    use sha2::Sha256;

    let full_label = format!("tls13 {}", label);
    let mut info = Vec::with_capacity(4 + full_label.len() + context.len());
    info.extend_from_slice(&(length as u16).to_be_bytes());
    info.push(full_label.len() as u8);
    info.extend_from_slice(full_label.as_bytes());
    info.push(context.len() as u8);
    info.extend_from_slice(context);

    let hk = Hkdf::<Sha256>::from_prk(secret).expect("HKDF PRK must be >= HashLen (32)");
    let mut okm = vec![0u8; length];
    hk.expand(&info, &mut okm)
        .expect("HKDF expand length within bound");
    okm
}

// ── AES-GCM record cipher ────────────────────────────────────────────────────

/// Seal `plaintext` with AES-256-GCM (TLS_AES_256_GCM_SHA384's AEAD). Returns
/// ciphertext||tag, or `None` on an internal failure.
pub fn aes256gcm_seal(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    plaintext: &[u8],
) -> Option<Vec<u8>> {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use aes_gcm::{Aes256Gcm, Key, Nonce};
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .ok()
}

/// Open an AES-256-GCM ciphertext||tag. Returns `None` if the tag fails (the
/// whole point — a tampered record must not decrypt).
pub fn aes256gcm_open(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    ciphertext: &[u8],
) -> Option<Vec<u8>> {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use aes_gcm::{Aes256Gcm, Key, Nonce};
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .ok()
}

// ── X.509 certificate parse ──────────────────────────────────────────────────

/// Parse a DER certificate far enough to confirm it is well-formed and pull the
/// serial number. Real callers (cert-chain validation) use the same entry to
/// reach the SPKI; a malformed cert must yield `None`, not panic.
pub fn parse_cert_serial(der_bytes: &[u8]) -> Option<Vec<u8>> {
    use der::Decode;
    use x509_cert::Certificate;
    let cert = Certificate::from_der(der_bytes).ok()?;
    Some(cert.tbs_certificate.serial_number.as_bytes().to_vec())
}

// ── Certificate chain validation (RFC 8446 §4.4.2, RFC 5280 §6) ──────────────
//
// The handshake derives session keys from ECDHE alone; without the steps below
// the server is never *authenticated* — any MITM that completes an ECDHE looks
// identical to the real server. These functions are the authentication gate:
// parse the server's Certificate flight, walk leaf→intermediate→trusted-root
// verifying each issuer signature, bind the leaf to the hostname, and verify the
// server's CertificateVerify signature over the transcript. Concept §"AthNet:
// real TLS 1.3, not a toy" — this is the line between "encrypted" and "safe".

use der::oid::ObjectIdentifier;

/// id-ecPublicKey (1.2.840.10045.2.1) — an EC SubjectPublicKeyInfo.
const OID_EC_PUBLIC_KEY: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.2.1");
/// rsaEncryption (1.2.840.113549.1.1.1) — an RSA SubjectPublicKeyInfo.
const OID_RSA_ENCRYPTION: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.1");
/// ecdsa-with-SHA256 (1.2.840.10045.4.3.2) — the cert/CertVerify signature alg.
const OID_ECDSA_WITH_SHA256: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.2");
/// ecdsa-with-SHA384 (1.2.840.10045.4.3.3) — P-384 leaf/intermediate cert sig alg.
const OID_ECDSA_WITH_SHA384: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.4.3.3");
/// sha256WithRSAEncryption (1.2.840.113549.1.1.11) — PKCS#1 v1.5 over SHA-256.
const OID_SHA256_WITH_RSA: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.113549.1.1.11");
/// namedCurve secp256r1 / prime256v1 (1.2.840.10045.3.1.7) — the EC params OID
/// carried in a P-256 SPKI algorithm parameter; distinguishes P-256 from P-384.
const OID_SECP256R1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.3.1.7");
/// namedCurve secp384r1 (1.3.132.0.34) — the EC params OID in a P-384 SPKI.
const OID_SECP384R1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.132.0.34");
/// id-at-commonName (2.5.4.3) — the CN attribute, used as a SAN fallback.
const OID_COMMON_NAME: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.4.3");

/// A subject public key extracted from an X.509 SPKI, normalized to the exact
/// byte form the existing `p256_verify` / `rsa_pkcs1_sha256_verify` primitives
/// accept. Carrying it as an enum keeps the verify call alg-agnostic.
#[derive(Clone)]
pub enum CertPublicKey {
    /// SEC1 uncompressed P-256 point (0x04 || X || Y) — feeds `p256_verify`.
    P256(Vec<u8>),
    /// SEC1 uncompressed P-384 point (0x04 || X || Y) — feeds `p384_verify`.
    P384(Vec<u8>),
    /// PKCS#1 `RSAPublicKey` DER — feeds `rsa_pkcs1_sha256_verify`.
    Rsa(Vec<u8>),
}

/// Why a chain/cert/CertificateVerify check was rejected. The driver records this
/// out-of-band; the handshake itself just moves to `Failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertError {
    /// The Certificate handshake message was truncated or malformed.
    MalformedMessage,
    /// A certificate body did not parse as DER X.509.
    MalformedCert,
    /// A certificate carried a key type we do not verify (not P-256 / P-384 / RSA).
    UnsupportedKey,
    /// A certificate used a signature algorithm we do not verify.
    UnsupportedSigAlg,
    /// An issuer signature over a certificate did not verify.
    BadSignature,
    /// The chain did not terminate at a configured trust anchor.
    UntrustedRoot,
    /// notBefore/notAfter rejected the cert against the supplied clock.
    Expired,
    /// The empty certificate list (server sent no certs).
    EmptyChain,
    /// The leaf's SAN/CN did not match the requested hostname.
    HostnameMismatch,
    /// The CertificateVerify signature did not verify over the transcript.
    BadCertificateVerify,
}

/// A store of trusted root certificates (DER). Chain validation succeeds only if
/// the walk reaches a certificate whose issuer is one of these anchors AND that
/// anchor's signature over the top intermediate (or the leaf, for a 1-deep
/// chain) verifies. Anchors are matched by subject-Name DER equality, exactly
/// as RFC 5280 path building does.
#[derive(Clone, Default)]
pub struct TrustStore {
    anchors: Vec<Vec<u8>>,
}

impl TrustStore {
    pub fn new() -> Self {
        Self {
            anchors: Vec::new(),
        }
    }

    /// Add a trusted root certificate (DER). Returns false if the cert does not
    /// parse — a malformed anchor is silently useless, never a panic.
    pub fn add_root_der(&mut self, der_bytes: &[u8]) -> bool {
        use der::Decode;
        use x509_cert::Certificate;
        if Certificate::from_der(der_bytes).is_err() {
            return false;
        }
        self.anchors.push(der_bytes.to_vec());
        true
    }

    pub fn len(&self) -> usize {
        self.anchors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.anchors.is_empty()
    }

    /// Find a trust anchor whose Subject DER matches `issuer_der` (the issuer
    /// Name of the certificate we are trying to anchor). Returns the anchor's
    /// parsed public key.
    fn anchor_key_for_issuer(&self, issuer_der: &[u8]) -> Option<CertPublicKey> {
        use der::{Decode, Encode};
        use x509_cert::Certificate;
        for anchor in &self.anchors {
            let cert = match Certificate::from_der(anchor) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let subj = match cert.tbs_certificate.subject.to_der() {
                Ok(s) => s,
                Err(_) => continue,
            };
            if subj.as_slice() == issuer_der {
                return cert_public_key(anchor);
            }
        }
        None
    }
}

/// Parse a TLS 1.3 Certificate handshake message (RFC 8446 §4.4.2) into the
/// ordered list of DER certificates (leaf first). Layout (after the 4-byte
/// handshake header, which the caller strips):
///   opaque certificate_request_context<0..2^8-1>
///   CertificateEntry certificate_list<0..2^24-1>
/// where each CertificateEntry = `cert_data<1..2^24-1> || extensions<0..2^16-1>`.
/// Fully bounds-checked: any truncation yields `Err(MalformedMessage)`.
pub fn parse_certificate_message(body: &[u8]) -> Result<Vec<Vec<u8>>, CertError> {
    let mut p = Reader::new(body);
    // certificate_request_context (u8 length).
    let ctx_len = p.u8().ok_or(CertError::MalformedMessage)? as usize;
    p.skip(ctx_len).ok_or(CertError::MalformedMessage)?;
    // certificate_list (u24 length).
    let list_len = read_u24(&mut p).ok_or(CertError::MalformedMessage)?;
    let list = p.bytes(list_len).ok_or(CertError::MalformedMessage)?;

    let mut certs = Vec::new();
    let mut lr = Reader::new(list);
    while lr.remaining() > 0 {
        let cert_len = read_u24(&mut lr).ok_or(CertError::MalformedMessage)?;
        let cert_data = lr.bytes(cert_len).ok_or(CertError::MalformedMessage)?;
        let ext_len = lr.u16().ok_or(CertError::MalformedMessage)? as usize;
        lr.skip(ext_len).ok_or(CertError::MalformedMessage)?;
        certs.push(cert_data.to_vec());
    }
    if certs.is_empty() {
        return Err(CertError::EmptyChain);
    }
    Ok(certs)
}

/// Read a 24-bit big-endian length from a `Reader`.
fn read_u24(r: &mut Reader) -> Option<usize> {
    let a = r.u8()? as usize;
    let b = r.u8()? as usize;
    let c = r.u8()? as usize;
    Some((a << 16) | (b << 8) | c)
}

/// Extract the subject public key from a DER certificate, normalized to the
/// form the verify primitives expect. `None` on a parse failure or a key type we
/// cannot verify (caller maps to `UnsupportedKey`).
pub fn cert_public_key(der_bytes: &[u8]) -> Option<CertPublicKey> {
    use der::Decode;
    use x509_cert::Certificate;
    let cert = Certificate::from_der(der_bytes).ok()?;
    let spki = &cert.tbs_certificate.subject_public_key_info;
    let key_bytes = spki.subject_public_key.as_bytes()?; // strips the unused-bits octet
    let alg = spki.algorithm.oid;
    if alg == OID_EC_PUBLIC_KEY {
        // An EC SubjectPublicKeyInfo carries the namedCurve OID as the algorithm
        // parameter; the SEC1 point itself is curve-agnostic, so we MUST read the
        // parameter to know whether p256_verify or p384_verify can consume it.
        // A missing/unknown curve is fail-closed (`None` → UnsupportedKey).
        let curve = spki
            .algorithm
            .parameters
            .clone()
            .and_then(|p| p.decode_as::<ObjectIdentifier>().ok())?;
        if curve == OID_SECP256R1 {
            // SEC1 point; p256_verify checks it parses as a valid curve point.
            Some(CertPublicKey::P256(key_bytes.to_vec()))
        } else if curve == OID_SECP384R1 {
            // SEC1 point; p384_verify checks it parses as a valid curve point.
            Some(CertPublicKey::P384(key_bytes.to_vec()))
        } else {
            None
        }
    } else if alg == OID_RSA_ENCRYPTION {
        // The bitstring body IS the PKCS#1 RSAPublicKey DER.
        Some(CertPublicKey::Rsa(key_bytes.to_vec()))
    } else {
        None
    }
}

/// Verify that `cert_der` was signed by the holder of `issuer_key`. Reconstructs
/// the exact signed bytes (the DER `TBSCertificate`) and dispatches on the
/// certificate's signatureAlgorithm to the matching primitive. Never panics.
pub fn verify_cert_signature(cert_der: &[u8], issuer_key: &CertPublicKey) -> Result<(), CertError> {
    use der::{Decode, Encode};
    use x509_cert::Certificate;
    let cert = Certificate::from_der(cert_der).map_err(|_| CertError::MalformedCert)?;
    // The signature covers the DER encoding of the TBSCertificate.
    let tbs = cert
        .tbs_certificate
        .to_der()
        .map_err(|_| CertError::MalformedCert)?;
    let sig = cert.signature.as_bytes().ok_or(CertError::MalformedCert)?;
    let alg = cert.signature_algorithm.oid;

    let ok = if alg == OID_ECDSA_WITH_SHA256 {
        match issuer_key {
            CertPublicKey::P256(pk) => {
                // ECDSA-with-SHA256: p256_verify hashes the message with SHA-256
                // internally and the cert signature is the ASN.1 ECDSA-Sig-Value.
                p256_verify(pk, &tbs, sig)
            }
            _ => return Err(CertError::BadSignature),
        }
    } else if alg == OID_ECDSA_WITH_SHA384 {
        match issuer_key {
            CertPublicKey::P384(pk) => {
                // ECDSA-with-SHA384: p384_verify prehashes with SHA-384 internally;
                // the cert signature is the ASN.1 ECDSA-Sig-Value over the TBS.
                p384_verify(pk, &tbs, sig)
            }
            _ => return Err(CertError::BadSignature),
        }
    } else if alg == OID_SHA256_WITH_RSA {
        match issuer_key {
            CertPublicKey::Rsa(pk) => rsa_pkcs1_sha256_verify(pk, &tbs, sig),
            _ => return Err(CertError::BadSignature),
        }
    } else {
        return Err(CertError::UnsupportedSigAlg);
    };
    if ok {
        Ok(())
    } else {
        Err(CertError::BadSignature)
    }
}

/// Check a single certificate's validity window against `now_unix` (seconds since
/// the Unix epoch). `None` skips the check (caller has no clock yet — documented
/// follow-up: wire AthNet's monotonic time so this is never skipped in
/// production). When supplied, both notBefore<=now and now<=notAfter must hold.
fn cert_time_valid(cert_der: &[u8], now_unix: Option<u64>) -> Result<(), CertError> {
    let now = match now_unix {
        Some(n) => n,
        None => return Ok(()), // validity check skipped — see follow-up above.
    };
    use der::Decode;
    use x509_cert::Certificate;
    let cert = Certificate::from_der(cert_der).map_err(|_| CertError::MalformedCert)?;
    let nb = cert
        .tbs_certificate
        .validity
        .not_before
        .to_unix_duration()
        .as_secs();
    let na = cert
        .tbs_certificate
        .validity
        .not_after
        .to_unix_duration()
        .as_secs();
    if now < nb || now > na {
        return Err(CertError::Expired);
    }
    Ok(())
}

/// Extract the DNS names a certificate presents: the SAN dNSName entries (the
/// authoritative RFC 6125 source), falling back to the subject CN if no SAN is
/// present. Returns lowercased owned strings ready for `verify_hostname`.
pub fn extract_dns_names(cert_der: &[u8]) -> Vec<alloc::string::String> {
    use der::Decode;
    use x509_cert::ext::pkix::name::GeneralName;
    use x509_cert::ext::pkix::SubjectAltName;
    use x509_cert::Certificate;

    let mut names = Vec::new();
    let cert = match Certificate::from_der(cert_der) {
        Ok(c) => c,
        Err(_) => return names,
    };
    // SAN extension (preferred).
    if let Ok(Some((_critical, san))) = cert.tbs_certificate.get::<SubjectAltName>() {
        for gn in san.0.iter() {
            if let GeneralName::DnsName(dns) = gn {
                names.push(dns.as_str().to_lowercase());
            }
        }
    }
    // CN fallback only when SAN gave nothing.
    if names.is_empty() {
        for rdn in cert.tbs_certificate.subject.0.iter() {
            for atv in rdn.0.iter() {
                if atv.oid == OID_COMMON_NAME {
                    if let Ok(s) = core::str::from_utf8(atv.value.value()) {
                        names.push(s.to_lowercase());
                    }
                }
            }
        }
    }
    names
}

/// Validate a server certificate chain (leaf first, as TLS sends it) against the
/// trust store: each cert must be signed by the next, the top must be signed by
/// a trusted anchor, and every cert must be within its validity window. Returns
/// the verified leaf's parsed public key (for the CertificateVerify step) on
/// success. RFC 5280 §6 path validation, minimal profile (no name constraints /
/// EKU / revocation — documented follow-ups).
pub fn validate_chain(
    chain_der: &[Vec<u8>],
    store: &TrustStore,
    now_unix: Option<u64>,
) -> Result<CertPublicKey, CertError> {
    use der::{Decode, Encode};
    use x509_cert::Certificate;

    if chain_der.is_empty() {
        return Err(CertError::EmptyChain);
    }
    // Every cert must at least be well-formed and in-window.
    for c in chain_der {
        cert_time_valid(c, now_unix)?;
    }

    // Walk: for i in 0..n-1, cert[i] must be signed by cert[i+1], and the issuer
    // Name of cert[i] must equal the subject Name of cert[i+1] (path chaining).
    for i in 0..chain_der.len() - 1 {
        let child = Certificate::from_der(&chain_der[i]).map_err(|_| CertError::MalformedCert)?;
        let parent =
            Certificate::from_der(&chain_der[i + 1]).map_err(|_| CertError::MalformedCert)?;
        let child_issuer = child
            .tbs_certificate
            .issuer
            .to_der()
            .map_err(|_| CertError::MalformedCert)?;
        let parent_subject = parent
            .tbs_certificate
            .subject
            .to_der()
            .map_err(|_| CertError::MalformedCert)?;
        if child_issuer != parent_subject {
            return Err(CertError::UntrustedRoot);
        }
        let parent_key = cert_public_key(&chain_der[i + 1]).ok_or(CertError::UnsupportedKey)?;
        verify_cert_signature(&chain_der[i], &parent_key)?;
    }

    // Anchor the top-of-chain cert: its issuer must be a trusted root, and that
    // root's key must verify the top cert's signature.
    let top =
        Certificate::from_der(chain_der.last().unwrap()).map_err(|_| CertError::MalformedCert)?;
    let top_issuer = top
        .tbs_certificate
        .issuer
        .to_der()
        .map_err(|_| CertError::MalformedCert)?;
    let anchor_key = store
        .anchor_key_for_issuer(&top_issuer)
        .ok_or(CertError::UntrustedRoot)?;
    verify_cert_signature(chain_der.last().unwrap(), &anchor_key)?;

    // Success: hand back the LEAF's public key for CertificateVerify.
    cert_public_key(&chain_der[0]).ok_or(CertError::UnsupportedKey)
}

/// Verify the server's CertificateVerify signature (RFC 8446 §4.4.3). The signed
/// content is `octet 0x20 * 64 || "TLS 1.3, server CertificateVerify" || 0x00 ||
/// Transcript-Hash(messages through Certificate)`. `transcript_hash` is that
/// hash; `leaf_key` is the verified leaf public key; `sig_scheme` is the on-wire
/// SignatureScheme u16; `signature` is the raw signature bytes from the message.
pub fn verify_certificate_verify(
    leaf_key: &CertPublicKey,
    sig_scheme: u16,
    signature: &[u8],
    transcript_hash: &[u8],
) -> Result<(), CertError> {
    // Build the §4.4.3 signed content.
    let mut content = Vec::with_capacity(64 + 33 + 1 + transcript_hash.len());
    content.extend_from_slice(&[0x20u8; 64]);
    content.extend_from_slice(b"TLS 1.3, server CertificateVerify");
    content.push(0x00);
    content.extend_from_slice(transcript_hash);

    let ok = match sig_scheme {
        // ecdsa_secp256r1_sha256.
        0x0403 => match leaf_key {
            CertPublicKey::P256(pk) => p256_verify(pk, &content, signature),
            _ => return Err(CertError::BadCertificateVerify),
        },
        // ecdsa_secp384r1_sha384.
        0x0503 => match leaf_key {
            CertPublicKey::P384(pk) => p384_verify(pk, &content, signature),
            _ => return Err(CertError::BadCertificateVerify),
        },
        // rsa_pkcs1_sha256.
        0x0401 => match leaf_key {
            CertPublicKey::Rsa(pk) => rsa_pkcs1_sha256_verify(pk, &content, signature),
            _ => return Err(CertError::BadCertificateVerify),
        },
        _ => return Err(CertError::UnsupportedSigAlg),
    };
    if ok {
        Ok(())
    } else {
        Err(CertError::BadCertificateVerify)
    }
}

// ── Signature verification (the cert-chain trust step) ───────────────────────

/// Verify a P-256 ECDSA signature over `msg`. `pubkey_sec1` is the uncompressed
/// SEC1 point (0x04 || X || Y); `sig_der` is the ASN.1 DER ECDSA signature.
pub fn p256_verify(pubkey_sec1: &[u8], msg: &[u8], sig_der: &[u8]) -> bool {
    use p256::ecdsa::signature::Verifier;
    use p256::ecdsa::{Signature, VerifyingKey};
    let vk = match VerifyingKey::from_sec1_bytes(pubkey_sec1) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let sig = match Signature::from_der(sig_der) {
        Ok(s) => s,
        Err(_) => return false,
    };
    vk.verify(msg, &sig).is_ok()
}

/// Verify a P-384 ECDSA signature over `msg`. `pubkey_sec1` is the uncompressed
/// SEC1 point (0x04 || X || Y); `sig_der` is the ASN.1 DER ECDSA signature.
/// `vk.verify` prehashes `msg` with SHA-384 (p384's `DigestPrimitive::Digest`),
/// so callers pass the raw message exactly as with [`p256_verify`]. Fail-closed:
/// any parse or verification error yields `false`, never a panic.
pub fn p384_verify(pubkey_sec1: &[u8], msg: &[u8], sig_der: &[u8]) -> bool {
    use p384::ecdsa::signature::Verifier;
    use p384::ecdsa::{Signature, VerifyingKey};
    let vk = match VerifyingKey::from_sec1_bytes(pubkey_sec1) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let sig = match Signature::from_der(sig_der) {
        Ok(s) => s,
        Err(_) => return false,
    };
    vk.verify(msg, &sig).is_ok()
}

/// Verify an RSA PKCS#1 v1.5 (SHA-256) signature over `msg`. `pubkey_der` is the
/// PKCS#1 RSAPublicKey DER. Returns false on any parse/verify failure.
pub fn rsa_pkcs1_sha256_verify(pubkey_der: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    use rsa::pkcs1::DecodeRsaPublicKey;
    use rsa::pkcs1v15::{Signature, VerifyingKey};
    use rsa::sha2::Sha256;
    use rsa::signature::Verifier;
    use rsa::RsaPublicKey;

    let pk = match RsaPublicKey::from_pkcs1_der(pubkey_der) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let vk = VerifyingKey::<Sha256>::new(pk);
    let sig = match Signature::try_from(sig) {
        Ok(s) => s,
        Err(_) => return false,
    };
    vk.verify(msg, &sig).is_ok()
}

// ════════════════════════════════════════════════════════════════════════════
// TLS 1.3 handshake state machine — Concept §"AthNet: real TLS 1.3, not a toy".
//
// The primitives above (HKDF-Expand-Label, AES-GCM, X.509 + sig verify) are the
// building blocks; the pieces below are the *orchestration* that an HTTPS client
// (browser, package manager) actually drives: the record layer, the RFC 8446 §7.1
// key schedule (Early→Handshake→Master secrets, derive-secret bound to a running
// transcript hash), traffic-key derivation, the ClientHello/ServerHello on-wire
// formats, the Finished MAC, and the hostname (SAN/CN, wildcard) check. This is
// pure logic + crypto — fully host-KAT-able here; the kernel socket I/O that
// pumps records in/out of this state machine is the deferred follow-up.
// ════════════════════════════════════════════════════════════════════════════

use sha2::{Digest, Sha256};

/// SHA-256 output size — the hash for TLS_AES_128_GCM_SHA256 (the mandatory
/// TLS 1.3 suite) and the key-schedule HKDF.
pub const HASH_LEN: usize = 32;

// ── Record layer (RFC 8446 §5.1) ─────────────────────────────────────────────

/// TLS record `ContentType` (the first byte of a TLSPlaintext).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ContentType {
    ChangeCipherSpec = 20,
    Alert = 21,
    Handshake = 22,
    ApplicationData = 23,
}

impl ContentType {
    pub fn from_u8(b: u8) -> Option<Self> {
        match b {
            20 => Some(Self::ChangeCipherSpec),
            21 => Some(Self::Alert),
            22 => Some(Self::Handshake),
            23 => Some(Self::ApplicationData),
            _ => None,
        }
    }
}

/// A parsed TLS record (the 5-byte header + fragment). For TLS 1.3 the
/// legacy_record_version is always 0x0303 on the wire after ClientHello.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsRecord {
    pub content_type: ContentType,
    pub fragment: Vec<u8>,
}

/// Maximum on-wire TLS record fragment length we will accept (RFC 8446 §5.2):
/// `TLSCiphertext.length` is at most `2^14 + 256` to allow for the AEAD
/// expansion (tag + inner content-type byte) over a full `2^14` plaintext.
/// A header declaring more than this is a protocol violation and is rejected
/// HARD — never treated as "need more bytes" (that distinction is the whole
/// point of [`RecordParse`], and is what keeps a hostile length header from
/// growing `in_buf` without bound — criterion #6).
pub const MAX_RECORD_LEN: usize = 16640; // 2^14 + 256

/// Three-state result of [`TlsRecord::parse`]. The previous `Option` collapsed
/// "need more bytes" and "the declared length is illegally huge" into the same
/// `None`, so a streaming reader would `recv` forever on a hostile 5-byte header
/// `[0x16,0x03,0x03,0xFF,0xFF]` (length 65535) — unbounded `in_buf` growth (OOM)
/// or a hang. Keeping them distinct lets callers fail closed on `Invalid`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordParse {
    /// `buf` does not yet contain a full record; the caller should buffer more.
    NeedMore,
    /// A complete record was parsed; `usize` is the bytes consumed from `buf`.
    Record(TlsRecord, usize),
    /// The header is structurally invalid (bad content type, or a declared
    /// fragment length exceeding [`MAX_RECORD_LEN`]). HARD reject — never retry.
    Invalid,
}

impl TlsRecord {
    /// Serialize to the on-wire `type || 0x0303 || u16 len || fragment`.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(5 + self.fragment.len());
        out.push(self.content_type as u8);
        out.extend_from_slice(&[0x03, 0x03]);
        out.extend_from_slice(&(self.fragment.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.fragment);
        out
    }

    /// Parse one record from the front of `buf`, distinguishing "need more bytes"
    /// from "structurally invalid" (see [`RecordParse`]). A truncated/garbage
    /// header must never panic. This is the fail-closed entry point streaming
    /// readers MUST use so a hostile length header cannot cause an unbounded loop.
    pub fn parse_state(buf: &[u8]) -> RecordParse {
        if buf.len() < 5 {
            return RecordParse::NeedMore;
        }
        let content_type = match ContentType::from_u8(buf[0]) {
            Some(c) => c,
            None => return RecordParse::Invalid, // bad content type = HARD reject.
        };
        let len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
        // RFC 8446 §5.2: TLSCiphertext.length <= 2^14 + 256. A larger declared
        // length is illegal — reject HARD rather than wait for bytes that, by
        // definition, can never form a valid record.
        if len > MAX_RECORD_LEN {
            return RecordParse::Invalid;
        }
        if buf.len() < 5 + len {
            return RecordParse::NeedMore; // genuinely need more bytes.
        }
        RecordParse::Record(
            TlsRecord {
                content_type,
                fragment: buf[5..5 + len].to_vec(),
            },
            5 + len,
        )
    }

    /// Backwards-compatible `Option` wrapper over [`Self::parse_state`]. Both
    /// `NeedMore` and `Invalid` map to `None`; new streaming code that must fail
    /// closed on a hostile header should call `parse_state` directly.
    pub fn parse(buf: &[u8]) -> Option<(TlsRecord, usize)> {
        match Self::parse_state(buf) {
            RecordParse::Record(r, used) => Some((r, used)),
            RecordParse::NeedMore | RecordParse::Invalid => None,
        }
    }
}

/// Per-record AEAD nonce (RFC 8446 §5.3): the 96-bit write_iv XORed with the
/// big-endian 64-bit record sequence number in the low 8 bytes. Each record
/// increments the sequence; the nonce MUST be unique per (key, record).
pub fn record_nonce(write_iv: &[u8; 12], seq: u64) -> [u8; 12] {
    let mut nonce = *write_iv;
    let seq_be = seq.to_be_bytes();
    for i in 0..8 {
        nonce[4 + i] ^= seq_be[i];
    }
    nonce
}

// ── Transcript hash (RFC 8446 §4.4.1) ────────────────────────────────────────

/// Running SHA-256 over the concatenated handshake messages, in order. The key
/// schedule's `derive_secret` is bound to a snapshot of this at each stage.
#[derive(Clone)]
pub struct Transcript {
    hasher: Sha256,
}

impl Transcript {
    pub fn new() -> Self {
        Self {
            hasher: Sha256::new(),
        }
    }

    /// Feed one full handshake message (the 4-byte header + body, exactly as it
    /// appears inside a Handshake record).
    pub fn update(&mut self, handshake_msg: &[u8]) {
        self.hasher.update(handshake_msg);
    }

    /// Current `Transcript-Hash(messages so far)` without consuming the state.
    pub fn current_hash(&self) -> [u8; HASH_LEN] {
        let out = self.hasher.clone().finalize();
        let mut h = [0u8; HASH_LEN];
        h.copy_from_slice(&out);
        h
    }
}

impl Default for Transcript {
    fn default() -> Self {
        Self::new()
    }
}

// ── Key schedule (RFC 8446 §7.1) ─────────────────────────────────────────────

/// HKDF-Extract over SHA-256 (the key schedule's `Extract`). `salt`/`ikm` empty
/// is the spec's "0" input; the Early Secret is `Extract(0, 0||PSK)`.
fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; HASH_LEN] {
    use hkdf::Hkdf;
    let (prk, _hk) = Hkdf::<Sha256>::extract(Some(salt), ikm);
    let mut out = [0u8; HASH_LEN];
    out.copy_from_slice(&prk);
    out
}

/// `Derive-Secret(Secret, Label, Messages)` = `HKDF-Expand-Label(Secret, Label,
/// Transcript-Hash(Messages), Hash.length)` (RFC 8446 §7.1).
pub fn derive_secret(
    secret: &[u8; HASH_LEN],
    label: &str,
    transcript_hash: &[u8],
) -> [u8; HASH_LEN] {
    let okm = hkdf_expand_label(secret, label, transcript_hash, HASH_LEN);
    let mut out = [0u8; HASH_LEN];
    out.copy_from_slice(&okm);
    out
}

/// The TLS 1.3 key schedule driven stage-by-stage. Holds the chain of secrets
/// (Early → Handshake → Master) and exposes the per-direction handshake/
/// application traffic secrets a record layer turns into {key, iv}.
#[derive(Clone)]
pub struct KeySchedule {
    early_secret: [u8; HASH_LEN],
    handshake_secret: Option<[u8; HASH_LEN]>,
    master_secret: Option<[u8; HASH_LEN]>,
    pub client_handshake_traffic: Option<[u8; HASH_LEN]>,
    pub server_handshake_traffic: Option<[u8; HASH_LEN]>,
    pub client_application_traffic: Option<[u8; HASH_LEN]>,
    pub server_application_traffic: Option<[u8; HASH_LEN]>,
}

impl KeySchedule {
    /// Start the schedule with no PSK (the common full-handshake case): the Early
    /// Secret is `Extract(salt=0, IKM=0)`.
    pub fn new() -> Self {
        let early = hkdf_extract(&[0u8; HASH_LEN], &[0u8; HASH_LEN]);
        Self {
            early_secret: early,
            handshake_secret: None,
            master_secret: None,
            client_handshake_traffic: None,
            server_handshake_traffic: None,
            client_application_traffic: None,
            server_application_traffic: None,
        }
    }

    /// Stage 2: mix in the ECDHE shared secret to get the Handshake Secret, then
    /// derive the client/server handshake traffic secrets bound to the transcript
    /// through ServerHello. (RFC 8446 §7.1.)
    pub fn derive_handshake_secrets(&mut self, ecdhe: &[u8], transcript_through_sh: &[u8]) {
        // HS = Extract(Derive-Secret(ES, "derived", ""), ECDHE)
        let empty_hash = derive_secret(&self.early_secret, "derived", &Sha256::digest(b""));
        let hs = hkdf_extract(&empty_hash, ecdhe);
        self.client_handshake_traffic =
            Some(derive_secret(&hs, "c hs traffic", transcript_through_sh));
        self.server_handshake_traffic =
            Some(derive_secret(&hs, "s hs traffic", transcript_through_sh));
        self.handshake_secret = Some(hs);
    }

    /// Stage 3: the Master Secret + application traffic secrets, bound to the
    /// transcript through the server Finished. Call after the handshake completes.
    pub fn derive_application_secrets(&mut self, transcript_through_sf: &[u8]) -> bool {
        let hs = match self.handshake_secret {
            Some(h) => h,
            None => return false,
        };
        let derived = derive_secret(&hs, "derived", &Sha256::digest(b""));
        let ms = hkdf_extract(&derived, &[0u8; HASH_LEN]);
        self.client_application_traffic =
            Some(derive_secret(&ms, "c ap traffic", transcript_through_sf));
        self.server_application_traffic =
            Some(derive_secret(&ms, "s ap traffic", transcript_through_sf));
        self.master_secret = Some(ms);
        true
    }
}

impl Default for KeySchedule {
    fn default() -> Self {
        Self::new()
    }
}

/// Derive the AEAD `{key, iv}` from a traffic secret (RFC 8446 §7.3). `key_len`
/// is 16 for AES-128-GCM, 32 for AES-256-GCM; the IV is always 12 bytes.
pub fn traffic_key_iv(traffic_secret: &[u8; HASH_LEN], key_len: usize) -> (Vec<u8>, [u8; 12]) {
    let key = hkdf_expand_label(traffic_secret, "key", b"", key_len);
    let iv_v = hkdf_expand_label(traffic_secret, "iv", b"", 12);
    let mut iv = [0u8; 12];
    iv.copy_from_slice(&iv_v);
    (key, iv)
}

/// The `finished_key` (RFC 8446 §4.4.4): `HKDF-Expand-Label(BaseKey, "finished",
/// "", Hash.length)`. The Finished message's verify_data is then
/// `HMAC(finished_key, Transcript-Hash(messages so far))`.
pub fn finished_key(base_traffic_secret: &[u8; HASH_LEN]) -> [u8; HASH_LEN] {
    let okm = hkdf_expand_label(base_traffic_secret, "finished", b"", HASH_LEN);
    let mut out = [0u8; HASH_LEN];
    out.copy_from_slice(&okm);
    out
}

/// Compute `verify_data = HMAC-SHA256(finished_key, transcript_hash)`.
pub fn finished_verify_data(
    finished_key: &[u8; HASH_LEN],
    transcript_hash: &[u8],
) -> [u8; HASH_LEN] {
    use hkdf::hmac::{Hmac, Mac};
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(finished_key).expect("HMAC accepts any key length");
    mac.update(transcript_hash);
    let out = mac.finalize().into_bytes();
    let mut v = [0u8; HASH_LEN];
    v.copy_from_slice(&out);
    v
}

/// Constant-time verify a peer's Finished `verify_data` against our recomputed
/// value. Returns true only on an exact match (a forged Finished must fail).
pub fn verify_finished(
    finished_key: &[u8; HASH_LEN],
    transcript_hash: &[u8],
    peer_verify_data: &[u8],
) -> bool {
    use subtle::ConstantTimeEq;
    let expected = finished_verify_data(finished_key, transcript_hash);
    if peer_verify_data.len() != expected.len() {
        return false;
    }
    expected.ct_eq(peer_verify_data).into()
}

// ── Handshake messages: ClientHello / ServerHello (RFC 8446 §4.1) ─────────────

/// The mandatory-to-implement TLS 1.3 cipher suite (RFC 8446 §9.1) plus the two
/// other widely deployed AEAD suites. Value is the on-wire u16.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherSuite {
    Aes128GcmSha256,
    Aes256GcmSha384,
    ChaCha20Poly1305Sha256,
}

impl CipherSuite {
    pub fn as_u16(&self) -> u16 {
        match self {
            Self::Aes128GcmSha256 => 0x1301,
            Self::Aes256GcmSha384 => 0x1302,
            Self::ChaCha20Poly1305Sha256 => 0x1303,
        }
    }
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0x1301 => Some(Self::Aes128GcmSha256),
            0x1302 => Some(Self::Aes256GcmSha384),
            0x1303 => Some(Self::ChaCha20Poly1305Sha256),
            _ => None,
        }
    }
    /// AEAD key length in bytes for this suite.
    pub fn key_len(&self) -> usize {
        match self {
            Self::Aes128GcmSha256 => 16,
            Self::Aes256GcmSha384 => 32,
            Self::ChaCha20Poly1305Sha256 => 32,
        }
    }
}

/// Wrap a handshake body in its 1-byte msg_type + 3-byte length header.
fn wrap_handshake(msg_type: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + body.len());
    out.push(msg_type);
    let len = body.len();
    out.push((len >> 16) as u8);
    out.push((len >> 8) as u8);
    out.push(len as u8);
    out.extend_from_slice(body);
    out
}

/// Build a TLS 1.3 ClientHello handshake message (the full 4-byte-header form,
/// ready to feed the transcript and wrap in a Handshake record). Carries:
/// supported_versions=TLS1.3, the given suites, key_share=x25519(`pubkey`),
/// signature_algorithms, and SNI=`server_name`. `random`/`session_id` are caller
/// supplied (the daemon fills them with CSPRNG bytes).
pub fn build_client_hello(
    random: &[u8; 32],
    session_id: &[u8],
    suites: &[CipherSuite],
    x25519_pubkey: &[u8; 32],
    server_name: &str,
) -> Vec<u8> {
    let mut body = Vec::new();
    // legacy_version = 0x0303 (TLS 1.2, per RFC 8446 §4.1.2).
    body.extend_from_slice(&[0x03, 0x03]);
    body.extend_from_slice(random);
    // legacy_session_id (<=32 bytes).
    body.push(session_id.len() as u8);
    body.extend_from_slice(session_id);
    // cipher_suites (u16 length prefix, then each suite u16).
    body.extend_from_slice(&((suites.len() * 2) as u16).to_be_bytes());
    for s in suites {
        body.extend_from_slice(&s.as_u16().to_be_bytes());
    }
    // legacy_compression_methods = [0] (null only).
    body.push(1);
    body.push(0);

    // ── extensions ──
    let mut ext = Vec::new();

    // server_name (0) — SNI host_name.
    {
        let mut sni = Vec::new();
        let name = server_name.as_bytes();
        // ServerNameList: u16 list len, then name_type(0)=host_name + u16 len.
        let entry_len = 1 + 2 + name.len();
        sni.extend_from_slice(&(entry_len as u16).to_be_bytes());
        sni.push(0); // host_name
        sni.extend_from_slice(&(name.len() as u16).to_be_bytes());
        sni.extend_from_slice(name);
        push_ext(&mut ext, 0x0000, &sni);
    }
    // supported_versions (43) — list = [TLS1.3 = 0x0304].
    push_ext(&mut ext, 0x002b, &[2, 0x03, 0x04]);
    // supported_groups (10) — [x25519 = 0x001d].
    push_ext(&mut ext, 0x000a, &[0x00, 0x02, 0x00, 0x1d]);
    // signature_algorithms (13) — ecdsa_secp256r1_sha256(0x0403),
    // ecdsa_secp384r1_sha384(0x0503), rsa_pss_rsae_sha256(0x0804),
    // rsa_pkcs1_sha256(0x0401). Advertising 0x0503 lets a server offer a P-384
    // leaf/intermediate chain (enterprise / government / some CDN PKIs).
    push_ext(
        &mut ext,
        0x000d,
        &[0x00, 0x08, 0x04, 0x03, 0x05, 0x03, 0x08, 0x04, 0x04, 0x01],
    );
    // key_share (51) — one entry: x25519, the 32-byte public key.
    {
        let mut ks = Vec::new();
        let entry_len = 2 + 2 + 32; // group + keylen + key
        ks.extend_from_slice(&(entry_len as u16).to_be_bytes());
        ks.extend_from_slice(&[0x00, 0x1d]); // x25519
        ks.extend_from_slice(&[0x00, 0x20]); // 32
        ks.extend_from_slice(x25519_pubkey);
        push_ext(&mut ext, 0x0033, &ks);
    }

    body.extend_from_slice(&(ext.len() as u16).to_be_bytes());
    body.extend_from_slice(&ext);

    wrap_handshake(0x01, &body) // client_hello = 1
}

/// Append one extension `type(u16) || len(u16) || data` to `out`.
fn push_ext(out: &mut Vec<u8>, ext_type: u16, data: &[u8]) {
    out.extend_from_slice(&ext_type.to_be_bytes());
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
}

/// What we extract from a ServerHello: the negotiated suite and the server's
/// x25519 key share (the other half of the ECDHE).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedServerHello {
    pub cipher_suite: CipherSuite,
    pub server_key_share: [u8; 32],
    pub is_hello_retry_request: bool,
}

/// The fixed SHA-256 of the HelloRetryRequest "random" sentinel (RFC 8446 §4.1.3).
const HRR_RANDOM: [u8; 32] = [
    0xCF, 0x21, 0xAD, 0x74, 0xE5, 0x9A, 0x61, 0x11, 0xBE, 0x1D, 0x8C, 0x02, 0x1E, 0x65, 0xB8, 0x91,
    0xC2, 0xA2, 0x11, 0x16, 0x7A, 0xBB, 0x8C, 0x5E, 0x07, 0x9E, 0x09, 0xE2, 0xC8, 0xA8, 0x33, 0x9C,
];

/// Parse a ServerHello handshake body (NOT including the 4-byte handshake
/// header — pass the body the caller already framed). Pulls the negotiated
/// cipher suite and the x25519 key_share. Returns `None` on any malformed input
/// (never panics — this is attacker-controlled data) or a non-x25519 group.
pub fn parse_server_hello(body: &[u8]) -> Option<ParsedServerHello> {
    let mut p = Reader::new(body);
    let _legacy_version = p.u16()?; // 0x0303
    let random = p.bytes(32)?;
    let is_hrr = random == HRR_RANDOM;
    let sid_len = p.u8()? as usize;
    p.skip(sid_len)?;
    let suite = CipherSuite::from_u16(p.u16()?)?;
    let _legacy_compression = p.u8()?; // must be 0
    let ext_total = p.u16()? as usize;
    let ext_bytes = p.bytes(ext_total)?;

    let mut server_key_share = [0u8; 32];
    let mut found_share = false;
    let mut e = Reader::new(ext_bytes);
    while e.remaining() >= 4 {
        let etype = e.u16()?;
        let elen = e.u16()? as usize;
        let edata = e.bytes(elen)?;
        if etype == 0x0033 {
            // key_share: group(u16) || key_len(u16) || key
            let mut k = Reader::new(edata);
            let group = k.u16()?;
            let klen = k.u16()? as usize;
            let key = k.bytes(klen)?;
            if group == 0x001d && klen == 32 {
                server_key_share.copy_from_slice(key);
                found_share = true;
            } else {
                return None; // unsupported group
            }
        }
    }
    if !found_share && !is_hrr {
        return None;
    }
    Some(ParsedServerHello {
        cipher_suite: suite,
        server_key_share,
        is_hello_retry_request: is_hrr,
    })
}

/// A tiny bounds-checked byte reader (no panics on short input — every accessor
/// returns `Option`). Used to parse attacker-controlled handshake bytes.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }
    fn u8(&mut self) -> Option<u8> {
        let b = *self.buf.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }
    fn u16(&mut self) -> Option<u16> {
        let hi = self.u8()? as u16;
        let lo = self.u8()? as u16;
        Some((hi << 8) | lo)
    }
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.remaining() < n {
            return None;
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Some(s)
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        if self.remaining() < n {
            return None;
        }
        self.pos += n;
        Some(())
    }
}

// ── Hostname verification (RFC 6125 — SAN/CN, wildcard) ──────────────────────

/// Match a presented certificate identity (`cert_name`, e.g. a SAN dNSName or
/// the CN) against the `hostname` we connected to. Implements the single
/// left-most `*` wildcard rule: `*.example.com` matches `a.example.com` but NOT
/// `example.com` (no label) nor `a.b.example.com` (wildcard spans one label
/// only). Case-insensitive. Pure logic, no allocation in the hot path.
pub fn hostname_matches(cert_name: &str, hostname: &str) -> bool {
    let cert = cert_name.trim().trim_end_matches('.');
    let host = hostname.trim().trim_end_matches('.');
    if cert.is_empty() || host.is_empty() {
        return false;
    }

    if let Some(rest) = cert.strip_prefix("*.") {
        // Wildcard must not contain another '*', and `rest` must be non-empty
        // and itself contain a dot (no `*.com`-style public-suffix wildcards).
        if rest.contains('*') || !rest.contains('.') {
            return false;
        }
        // The host must have exactly one leading label replaced by '*'.
        let host_dot = match host.find('.') {
            Some(i) => i,
            None => return false, // host is a single label; wildcard needs >=2
        };
        let host_rest = &host[host_dot + 1..];
        let host_first = &host[..host_dot];
        // First label must be non-empty (reject "*.example.com" vs ".example.com").
        return !host_first.is_empty() && host_rest.eq_ignore_ascii_case(rest);
    }

    cert.eq_ignore_ascii_case(host)
}

/// Verify a hostname against any of the cert's presented names (the SAN list, or
/// the CN as a fallback). Returns true if ANY matches.
pub fn verify_hostname(presented_names: &[&str], hostname: &str) -> bool {
    presented_names
        .iter()
        .any(|n| hostname_matches(n, hostname))
}

// ── Client handshake driver (the state machine an HTTPS client steps) ─────────

/// The TLS 1.3 client handshake state (RFC 8446 §A.1). The HTTPS client drives
/// transitions by feeding it the server's flight; an out-of-order or malformed
/// message moves it to `Failed`, never panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeState {
    /// Nothing sent yet.
    Start,
    /// ClientHello sent; awaiting ServerHello.
    WaitServerHello,
    /// ServerHello processed (handshake keys derived); awaiting
    /// EncryptedExtensions … server Finished (the encrypted flight).
    WaitFlight,
    /// Server Finished verified; application keys derived; ready for app data.
    Connected,
    /// A protocol or verification error occurred. Carries no data here; the
    /// driver records the reason out-of-band.
    Failed,
}

/// Drives the client side of a TLS 1.3 full handshake: maintains the transcript,
/// the key schedule, and the state. It does NOT do socket I/O — the caller pumps
/// bytes (record fragments) in and reads handshake messages to send out. This is
/// the seam the kernel net path wires into later.
pub struct ClientHandshake {
    pub state: HandshakeState,
    transcript: Transcript,
    schedule: KeySchedule,
    suite: Option<CipherSuite>,
    /// Set once ServerHello is processed: the negotiated server handshake
    /// traffic secret (used to verify the server Finished).
    server_hs_secret: Option<[u8; HASH_LEN]>,
    /// The trust anchors the server certificate chain must terminate at. Empty
    /// => the authenticated flight path refuses to connect (fail closed).
    trust_store: TrustStore,
    /// The hostname we dialed; the leaf cert's SAN/CN must match it.
    server_name: alloc::string::String,
    /// Wall-clock seconds for validity checks; `None` skips the window check.
    now_unix: Option<u64>,
    /// The reason the handshake failed, when `state == Failed` (auth path).
    pub fail_reason: Option<CertError>,
}

impl ClientHandshake {
    pub fn new() -> Self {
        Self {
            state: HandshakeState::Start,
            transcript: Transcript::new(),
            schedule: KeySchedule::new(),
            suite: None,
            server_hs_secret: None,
            trust_store: TrustStore::new(),
            server_name: alloc::string::String::new(),
            now_unix: None,
            fail_reason: None,
        }
    }

    /// Build an authenticating handshake: the certificate chain MUST terminate at
    /// one of `trust_store`'s anchors and the leaf MUST match `server_name`. This
    /// is the constructor the HTTPS client uses; `new()` (no anchors) cannot pass
    /// the authenticated flight path and is for the key-schedule-only KATs.
    pub fn new_authenticated(
        trust_store: TrustStore,
        server_name: &str,
        now_unix: Option<u64>,
    ) -> Self {
        let mut h = Self::new();
        h.trust_store = trust_store;
        h.server_name = server_name.to_string();
        h.now_unix = now_unix;
        h
    }

    /// Record the ClientHello we are sending (full handshake message bytes) and
    /// advance to `WaitServerHello`. Returns the bytes unchanged for convenience
    /// (the caller wraps them in a Handshake record and sends them).
    pub fn send_client_hello(&mut self, client_hello: &[u8]) {
        self.transcript.update(client_hello);
        self.state = HandshakeState::WaitServerHello;
    }

    /// Feed the ServerHello handshake message (full 4-byte-header form). On
    /// success: derives the handshake traffic secrets from the ECDHE shared
    /// secret and advances to `WaitFlight`. Returns the negotiated cipher suite.
    pub fn recv_server_hello(
        &mut self,
        server_hello_msg: &[u8],
        ecdhe_shared: &[u8],
    ) -> Option<CipherSuite> {
        if self.state != HandshakeState::WaitServerHello {
            self.state = HandshakeState::Failed;
            return None;
        }
        // Strip the 4-byte handshake header to reach the ServerHello body.
        if server_hello_msg.len() < 4 || server_hello_msg[0] != 0x02 {
            self.state = HandshakeState::Failed;
            return None;
        }
        let body = &server_hello_msg[4..];
        let sh = match parse_server_hello(body) {
            Some(s) => s,
            None => {
                self.state = HandshakeState::Failed;
                return None;
            }
        };
        // (HelloRetryRequest is not handled here — single round-trip only.)
        if sh.is_hello_retry_request {
            self.state = HandshakeState::Failed;
            return None;
        }
        self.transcript.update(server_hello_msg);
        let th = self.transcript.current_hash();
        self.schedule.derive_handshake_secrets(ecdhe_shared, &th);
        self.server_hs_secret = self.schedule.server_handshake_traffic;
        self.suite = Some(sh.cipher_suite);
        self.state = HandshakeState::WaitFlight;
        Some(sh.cipher_suite)
    }

    /// Feed the server's encrypted flight as already-decrypted handshake messages
    /// (EncryptedExtensions, Certificate, CertificateVerify) BEFORE the server
    /// Finished, then verify the server Finished `verify_data`. On success:
    /// derives application traffic secrets and moves to `Connected`.
    ///
    /// `flight_before_finished` is the concatenation of the handshake messages
    /// that precede Finished (each in full 4-byte-header form). `server_finished`
    /// is the full Finished handshake message (type 0x14).
    pub fn recv_server_flight(
        &mut self,
        flight_before_finished: &[&[u8]],
        server_finished: &[u8],
    ) -> bool {
        if self.state != HandshakeState::WaitFlight {
            self.state = HandshakeState::Failed;
            return false;
        }
        let server_hs = match self.server_hs_secret {
            Some(s) => s,
            None => {
                self.state = HandshakeState::Failed;
                return false;
            }
        };
        // Feed the pre-Finished messages into the transcript.
        for msg in flight_before_finished {
            self.transcript.update(msg);
        }
        // Verify the server Finished against the transcript *up to but not
        // including* Finished (RFC 8446 §4.4.4).
        if server_finished.len() < 4 || server_finished[0] != 0x14 {
            self.state = HandshakeState::Failed;
            return false;
        }
        let verify_data = &server_finished[4..];
        let fk = finished_key(&server_hs);
        let th_before_finished = self.transcript.current_hash();
        if !verify_finished(&fk, &th_before_finished, verify_data) {
            self.state = HandshakeState::Failed;
            return false;
        }
        // Finished is now part of the transcript; derive application secrets.
        self.transcript.update(server_finished);
        let th_after_sf = self.transcript.current_hash();
        if !self.schedule.derive_application_secrets(&th_after_sf) {
            self.state = HandshakeState::Failed;
            return false;
        }
        self.state = HandshakeState::Connected;
        true
    }

    /// The authenticating server-flight path (RFC 8446 §4.4): process the server's
    /// decrypted handshake flight — EncryptedExtensions, **Certificate**,
    /// **CertificateVerify**, Finished — and only reach `Connected` if ALL of:
    ///   1. the certificate chain validates to a configured trust anchor,
    ///   2. the leaf's SAN/CN matches the dialed hostname,
    ///   3. the CertificateVerify signature verifies over the transcript, and
    ///   4. the server Finished MAC verifies.
    /// Any failure → `Failed` (with `fail_reason` set) and NO application keys are
    /// derived. This is the gate that turns "encrypted" into "authenticated".
    ///
    /// Each argument is a full handshake message (4-byte header + body), exactly
    /// as decrypted from the handshake records. `encrypted_extensions` is fed to
    /// the transcript verbatim (its contents are not interpreted here).
    pub fn recv_authenticated_flight(
        &mut self,
        encrypted_extensions: &[u8],
        certificate_msg: &[u8],
        certificate_verify_msg: &[u8],
        server_finished: &[u8],
    ) -> Result<(), CertError> {
        if self.state != HandshakeState::WaitFlight {
            return self.fail(CertError::MalformedMessage);
        }
        let server_hs = match self.server_hs_secret {
            Some(s) => s,
            None => return self.fail(CertError::MalformedMessage),
        };
        // Fail closed: an empty trust store can never authenticate anyone.
        if self.trust_store.is_empty() {
            return self.fail(CertError::UntrustedRoot);
        }

        // ── EncryptedExtensions → transcript (opaque) ──
        if encrypted_extensions.len() < 4 || encrypted_extensions[0] != 0x08 {
            return self.fail(CertError::MalformedMessage);
        }
        self.transcript.update(encrypted_extensions);

        // ── Certificate: parse, validate the chain, bind the hostname ──
        if certificate_msg.len() < 4 || certificate_msg[0] != 0x0b {
            return self.fail(CertError::MalformedMessage);
        }
        let chain = parse_certificate_message(&certificate_msg[4..])?;
        let leaf_key = match validate_chain(&chain, &self.trust_store, self.now_unix) {
            Ok(k) => k,
            Err(e) => return self.fail(e),
        };
        // Hostname binding against the verified leaf.
        let names = extract_dns_names(&chain[0]);
        let name_refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        if !verify_hostname(&name_refs, &self.server_name) {
            return self.fail(CertError::HostnameMismatch);
        }
        // Certificate is now bound into the transcript; the CertificateVerify
        // signature is over the transcript hash *through Certificate*.
        self.transcript.update(certificate_msg);
        let th_through_cert = self.transcript.current_hash();

        // ── CertificateVerify: parse scheme+signature, verify over transcript ──
        if certificate_verify_msg.len() < 4 || certificate_verify_msg[0] != 0x0f {
            return self.fail(CertError::MalformedMessage);
        }
        let mut cv = Reader::new(&certificate_verify_msg[4..]);
        let scheme = match cv.u16() {
            Some(s) => s,
            None => return self.fail(CertError::MalformedMessage),
        };
        let sig_len = match cv.u16() {
            Some(l) => l as usize,
            None => return self.fail(CertError::MalformedMessage),
        };
        let signature = match cv.bytes(sig_len) {
            Some(s) => s,
            None => return self.fail(CertError::MalformedMessage),
        };
        if let Err(e) = verify_certificate_verify(&leaf_key, scheme, signature, &th_through_cert) {
            return self.fail(e);
        }
        self.transcript.update(certificate_verify_msg);

        // ── Server Finished: MAC over the transcript through CertificateVerify ──
        if server_finished.len() < 4 || server_finished[0] != 0x14 {
            return self.fail(CertError::MalformedMessage);
        }
        let verify_data = &server_finished[4..];
        let fk = finished_key(&server_hs);
        let th_before_finished = self.transcript.current_hash();
        if !verify_finished(&fk, &th_before_finished, verify_data) {
            return self.fail(CertError::BadCertificateVerify);
        }
        self.transcript.update(server_finished);
        let th_after_sf = self.transcript.current_hash();
        if !self.schedule.derive_application_secrets(&th_after_sf) {
            return self.fail(CertError::MalformedMessage);
        }
        self.state = HandshakeState::Connected;
        Ok(())
    }

    /// Move to `Failed`, record the reason, and return it as an `Err`. Centralizes
    /// the "no app keys on any failure" invariant: `state != Connected` means
    /// `client_app_key_iv()` / `server_app_key_iv()` return `None`.
    fn fail(&mut self, reason: CertError) -> Result<(), CertError> {
        self.state = HandshakeState::Failed;
        self.fail_reason = Some(reason);
        Err(reason)
    }

    /// The negotiated suite, once ServerHello is processed.
    pub fn cipher_suite(&self) -> Option<CipherSuite> {
        self.suite
    }

    /// The client application traffic {key, iv} once `Connected` — what the
    /// record layer encrypts outbound application data with.
    pub fn client_app_key_iv(&self) -> Option<(Vec<u8>, [u8; 12])> {
        let secret = self.schedule.client_application_traffic?;
        let key_len = self.suite?.key_len();
        Some(traffic_key_iv(&secret, key_len))
    }

    /// The server application traffic {key, iv} once `Connected` — what the
    /// record layer decrypts inbound application data with.
    pub fn server_app_key_iv(&self) -> Option<(Vec<u8>, [u8; 12])> {
        let secret = self.schedule.server_application_traffic?;
        let key_len = self.suite?.key_len();
        Some(traffic_key_iv(&secret, key_len))
    }
}

impl Default for ClientHandshake {
    fn default() -> Self {
        Self::new()
    }
}

// ── Host KATs (cargo test -p raenet --features tls13) ────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hkdf_expand_label_is_deterministic_and_sized() {
        let secret = [0x0bu8; 32];
        let a = hkdf_expand_label(&secret, "key", b"", 16);
        let b = hkdf_expand_label(&secret, "key", b"", 16);
        let c = hkdf_expand_label(&secret, "iv", b"", 12);
        assert_eq!(a.len(), 16);
        assert_eq!(c.len(), 12);
        assert_eq!(a, b, "same inputs -> same OKM");
        assert_ne!(&a[..12], &c[..], "different label -> different OKM");
    }

    #[test]
    fn aes256gcm_round_trip_and_tamper_detect() {
        let key = [0x42u8; 32];
        let nonce = [0x07u8; 12];
        let aad = b"tls13-record-header";
        let pt = b"the quick brown fox jumps over the lazy dog";
        let mut ct = aes256gcm_seal(&key, &nonce, aad, pt).expect("seal");
        assert_ne!(&ct[..pt.len()], &pt[..], "ciphertext != plaintext");
        let rt = aes256gcm_open(&key, &nonce, aad, &ct).expect("open");
        assert_eq!(&rt, pt);
        // Tamper: flip a ciphertext byte -> tag must reject.
        ct[0] ^= 0x01;
        assert!(aes256gcm_open(&key, &nonce, aad, &ct).is_none());
    }

    #[test]
    fn p256_sign_verify_round_trip_and_tamper() {
        use p256::ecdsa::signature::Signer;
        use p256::ecdsa::{Signature, SigningKey};
        let sk = SigningKey::random(&mut rand::rngs::OsRng);
        let vk = sk.verifying_key();
        let pubkey_sec1 = vk.to_encoded_point(false);
        let msg = b"server certificate transcript";
        let sig: Signature = sk.sign(msg);
        let sig_der = sig.to_der();
        assert!(p256_verify(pubkey_sec1.as_bytes(), msg, sig_der.as_bytes()));
        assert!(!p256_verify(
            pubkey_sec1.as_bytes(),
            b"forged",
            sig_der.as_bytes()
        ));
    }

    #[test]
    fn rsa_pkcs1_sha256_sign_verify_round_trip() {
        use rsa::pkcs1::EncodeRsaPublicKey;
        use rsa::pkcs1v15::SigningKey;
        use rsa::sha2::Sha256;
        use rsa::signature::{SignatureEncoding, Signer};
        use rsa::RsaPrivateKey;

        let mut rng = rand::rngs::OsRng;
        let priv_key = RsaPrivateKey::new(&mut rng, 2048).expect("rsa keygen");
        let pub_der = priv_key.to_public_key().to_pkcs1_der().expect("pub der");
        let signing_key = SigningKey::<Sha256>::new(priv_key);
        let msg = b"certificate to be signed";
        let sig = signing_key.sign(msg);
        assert!(rsa_pkcs1_sha256_verify(
            pub_der.as_bytes(),
            msg,
            &sig.to_bytes()
        ));
        assert!(!rsa_pkcs1_sha256_verify(
            pub_der.as_bytes(),
            b"forged",
            &sig.to_bytes()
        ));
    }

    #[test]
    fn parse_cert_rejects_garbage() {
        assert!(parse_cert_serial(&[0x30, 0x00, 0x01, 0x02]).is_none());
        assert!(parse_cert_serial(b"not a certificate").is_none());
    }

    // ── TLS 1.3 handshake state machine KATs ─────────────────────────────────
    use x25519_dalek::{PublicKey, StaticSecret};

    /// A deterministic x25519 keypair from a seed byte (test-only; clamps via
    /// StaticSecret::from). Returns (secret, public_bytes).
    fn x25519_pair(seed: u8) -> (StaticSecret, [u8; 32]) {
        let sk = StaticSecret::from([seed; 32]);
        let pk = PublicKey::from(&sk);
        (sk, pk.to_bytes())
    }

    #[test]
    fn record_roundtrip_and_truncation() {
        let r = TlsRecord {
            content_type: ContentType::Handshake,
            fragment: vec![0xAA, 0xBB, 0xCC],
        };
        let bytes = r.to_bytes();
        // type(22) || 0x0303 || len(0x0003) || fragment
        assert_eq!(&bytes[..5], &[22, 0x03, 0x03, 0x00, 0x03]);
        let (parsed, used) = TlsRecord::parse(&bytes).expect("parse");
        assert_eq!(used, bytes.len());
        assert_eq!(parsed, r);
        // A truncated buffer => need-more (None), not a panic.
        assert!(TlsRecord::parse(&bytes[..4]).is_none());
        assert!(TlsRecord::parse(&bytes[..7]).is_none());
        // Bad content type => None.
        let mut bad = bytes.clone();
        bad[0] = 99;
        assert!(TlsRecord::parse(&bad).is_none());
        // Two records back to back: parse consumes exactly one.
        let mut two = r.to_bytes();
        two.extend_from_slice(&r.to_bytes());
        let (_p1, used1) = TlsRecord::parse(&two).unwrap();
        assert_eq!(used1, bytes.len());
    }

    #[test]
    fn record_nonce_xors_sequence_into_low_bytes() {
        let iv = [0u8; 12];
        // seq 0 => unchanged.
        assert_eq!(record_nonce(&iv, 0), [0u8; 12]);
        // seq 1 => low byte flips.
        let n1 = record_nonce(&iv, 1);
        assert_eq!(n1[11], 1);
        assert_eq!(&n1[..11], &[0u8; 11]);
        // The first 4 bytes are never touched (the IV's fixed prefix).
        let iv2 = [0xFFu8; 12];
        let n = record_nonce(&iv2, 0x0102_0304_0506_0708);
        assert_eq!(&n[..4], &[0xFF; 4]);
        // Distinct sequences give distinct nonces (uniqueness invariant).
        assert_ne!(record_nonce(&iv2, 5), record_nonce(&iv2, 6));
    }

    #[test]
    fn derive_secret_binds_to_transcript_and_label() {
        let secret = [0x11u8; HASH_LEN];
        let th_a = Sha256::digest(b"transcript A");
        let th_b = Sha256::digest(b"transcript B");
        let a = derive_secret(&secret, "c hs traffic", &th_a);
        let a2 = derive_secret(&secret, "c hs traffic", &th_a);
        let diff_label = derive_secret(&secret, "s hs traffic", &th_a);
        let diff_th = derive_secret(&secret, "c hs traffic", &th_b);
        assert_eq!(a, a2, "deterministic");
        assert_ne!(a, diff_label, "label is mixed in");
        assert_ne!(a, diff_th, "transcript hash is mixed in");
    }

    #[test]
    fn key_schedule_both_sides_agree_on_ecdhe() {
        // Client and server perform x25519; both derive the SAME handshake +
        // application traffic secrets from the shared secret + transcript.
        let (c_sk, c_pub) = x25519_pair(1);
        let (s_sk, s_pub) = x25519_pair(2);
        let client_shared = c_sk.diffie_hellman(&PublicKey::from(s_pub)).to_bytes();
        let server_shared = s_sk.diffie_hellman(&PublicKey::from(c_pub)).to_bytes();
        assert_eq!(client_shared, server_shared, "x25519 sanity");

        let transcript_sh = b"...transcript through ServerHello...";
        let mut client_ks = KeySchedule::new();
        let mut server_ks = KeySchedule::new();
        client_ks.derive_handshake_secrets(&client_shared, transcript_sh);
        server_ks.derive_handshake_secrets(&server_shared, transcript_sh);

        assert_eq!(
            client_ks.client_handshake_traffic, server_ks.client_handshake_traffic,
            "both sides agree on c hs traffic"
        );
        assert_eq!(
            client_ks.server_handshake_traffic, server_ks.server_handshake_traffic,
            "both sides agree on s hs traffic"
        );
        // A different ECDHE produces a different schedule (negative control).
        let mut wrong = KeySchedule::new();
        wrong.derive_handshake_secrets(&[0u8; 32], transcript_sh);
        assert_ne!(
            wrong.client_handshake_traffic,
            client_ks.client_handshake_traffic
        );
    }

    #[test]
    fn traffic_key_iv_sizes_and_determinism() {
        let secret = [0x22u8; HASH_LEN];
        let (k128, iv) = traffic_key_iv(&secret, 16);
        let (k256, iv2) = traffic_key_iv(&secret, 32);
        assert_eq!(k128.len(), 16);
        assert_eq!(k256.len(), 32);
        assert_eq!(iv.len(), 12);
        assert_eq!(iv, iv2, "iv derivation independent of key length");
        // key and iv differ (different labels).
        assert_ne!(&k128[..12], &iv[..]);
    }

    #[test]
    fn client_hello_is_parseable_and_carries_keyshare_and_sni() {
        let random = [0x33u8; 32];
        let (_sk, pub_bytes) = x25519_pair(7);
        let ch = build_client_hello(
            &random,
            &[],
            &[
                CipherSuite::Aes128GcmSha256,
                CipherSuite::ChaCha20Poly1305Sha256,
            ],
            &pub_bytes,
            "example.com",
        );
        // It is a client_hello (type 1) with a correct 3-byte length.
        assert_eq!(ch[0], 0x01);
        let declared = ((ch[1] as usize) << 16) | ((ch[2] as usize) << 8) | ch[3] as usize;
        assert_eq!(declared, ch.len() - 4);
        // SNI host bytes and the x25519 pubkey both appear in the extensions.
        assert!(ch.windows(11).any(|w| w == b"example.com"));
        assert!(ch.windows(32).any(|w| w == pub_bytes));
    }

    /// Build a minimal ServerHello handshake message echoing one suite + the
    /// server's x25519 key share (enough for parse_server_hello / the driver).
    fn make_server_hello(suite: CipherSuite, server_pub: &[u8; 32]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&[0x03, 0x03]); // legacy_version
        body.extend_from_slice(&[0x44u8; 32]); // random
        body.push(0); // session_id len
        body.extend_from_slice(&suite.as_u16().to_be_bytes());
        body.push(0); // legacy_compression
        let mut ext = Vec::new();
        // supported_versions selected = TLS1.3.
        push_ext(&mut ext, 0x002b, &[0x03, 0x04]);
        // key_share: x25519 || len 32 || pub
        let mut ks = Vec::new();
        ks.extend_from_slice(&[0x00, 0x1d, 0x00, 0x20]);
        ks.extend_from_slice(server_pub);
        push_ext(&mut ext, 0x0033, &ks);
        body.extend_from_slice(&(ext.len() as u16).to_be_bytes());
        body.extend_from_slice(&ext);
        wrap_handshake(0x02, &body) // server_hello = 2
    }

    #[test]
    fn parse_server_hello_extracts_suite_and_share() {
        let (_s, s_pub) = x25519_pair(9);
        let sh = make_server_hello(CipherSuite::Aes256GcmSha384, &s_pub);
        let parsed = parse_server_hello(&sh[4..]).expect("parse SH body");
        assert_eq!(parsed.cipher_suite, CipherSuite::Aes256GcmSha384);
        assert_eq!(parsed.server_key_share, s_pub);
        assert!(!parsed.is_hello_retry_request);
        // Truncated / garbage body must not panic and must return None.
        assert!(parse_server_hello(&sh[4..10]).is_none());
        assert!(parse_server_hello(b"\x00").is_none());
    }

    #[test]
    fn full_client_handshake_round_trip_to_connected() {
        // End-to-end: a test "server" mirrors the spec to produce a valid
        // ServerHello + Finished; the ClientHandshake must reach Connected and
        // both sides must derive the SAME application keys.
        let (c_sk, c_pub) = x25519_pair(0x10);
        let (s_sk, s_pub) = x25519_pair(0x20);

        // Client sends ClientHello.
        let mut client = ClientHandshake::new();
        let ch = build_client_hello(
            &[0x55u8; 32],
            &[],
            &[CipherSuite::Aes128GcmSha256],
            &c_pub,
            "example.com",
        );
        client.send_client_hello(&ch);
        assert_eq!(client.state, HandshakeState::WaitServerHello);

        // Server side: same transcript, derive handshake secrets, build Finished.
        let suite = CipherSuite::Aes128GcmSha256;
        let sh = make_server_hello(suite, &s_pub);
        let server_shared = s_sk.diffie_hellman(&PublicKey::from(c_pub)).to_bytes();
        let mut server_ts = Transcript::new();
        server_ts.update(&ch);
        server_ts.update(&sh);
        let th_sh = server_ts.current_hash();
        let mut server_ks = KeySchedule::new();
        server_ks.derive_handshake_secrets(&server_shared, &th_sh);

        // Client processes ServerHello with its own ECDHE.
        let client_shared = c_sk.diffie_hellman(&PublicKey::from(s_pub)).to_bytes();
        let got_suite = client
            .recv_server_hello(&sh, &client_shared)
            .expect("server hello accepted");
        assert_eq!(got_suite, suite);
        assert_eq!(client.state, HandshakeState::WaitFlight);

        // Server's encrypted flight: EncryptedExtensions (empty) then Finished.
        let ee = wrap_handshake(0x08, &[0x00, 0x00]); // encrypted_extensions, empty
        server_ts.update(&ee);
        let th_before_fin = server_ts.current_hash();
        let s_hs = server_ks.server_handshake_traffic.unwrap();
        let fk = finished_key(&s_hs);
        let verify_data = finished_verify_data(&fk, &th_before_fin);
        let server_finished = wrap_handshake(0x14, &verify_data);

        // Client verifies the flight + Finished => Connected.
        let ok = client.recv_server_flight(&[&ee], &server_finished);
        assert!(ok, "valid server Finished accepted");
        assert_eq!(client.state, HandshakeState::Connected);

        // Both sides derive the SAME application traffic keys (the payoff).
        server_ts.update(&server_finished);
        let th_after_sf = server_ts.current_hash();
        server_ks.derive_application_secrets(&th_after_sf);
        let (c_key, c_iv) = client.client_app_key_iv().expect("client app keys");
        let expected = traffic_key_iv(&server_ks.client_application_traffic.unwrap(), 16);
        assert_eq!((c_key, c_iv), expected, "client app keys match the server");
        let (s_key, s_iv) = client.server_app_key_iv().unwrap();
        let exp_s = traffic_key_iv(&server_ks.server_application_traffic.unwrap(), 16);
        assert_eq!((s_key, s_iv), exp_s);
    }

    #[test]
    fn forged_finished_is_rejected() {
        // Same setup but the server Finished is corrupted => must NOT connect.
        let (c_sk, c_pub) = x25519_pair(0x11);
        let (s_sk, s_pub) = x25519_pair(0x21);
        let mut client = ClientHandshake::new();
        let ch = build_client_hello(
            &[0x66u8; 32],
            &[],
            &[CipherSuite::Aes128GcmSha256],
            &c_pub,
            "h",
        );
        client.send_client_hello(&ch);
        let sh = make_server_hello(CipherSuite::Aes128GcmSha256, &s_pub);
        let client_shared = c_sk.diffie_hellman(&PublicKey::from(s_pub)).to_bytes();
        client.recv_server_hello(&sh, &client_shared).unwrap();
        let _ = s_sk; // server secret unused on the forged path

        let ee = wrap_handshake(0x08, &[0x00, 0x00]);
        // Bogus verify_data (all zeros) — will not match the real MAC.
        let bad_finished = wrap_handshake(0x14, &[0u8; HASH_LEN]);
        let ok = client.recv_server_flight(&[&ee], &bad_finished);
        assert!(!ok, "forged Finished must be rejected");
        assert_eq!(client.state, HandshakeState::Failed);
        // No application keys are exposed on a failed handshake.
        assert!(client.client_app_key_iv().is_none());
    }

    #[test]
    fn out_of_order_server_hello_fails_cleanly() {
        let mut client = ClientHandshake::new();
        // recv_server_hello before send_client_hello => Failed, no panic.
        let (_s, s_pub) = x25519_pair(3);
        let sh = make_server_hello(CipherSuite::Aes128GcmSha256, &s_pub);
        assert!(client.recv_server_hello(&sh, &[7u8; 32]).is_none());
        assert_eq!(client.state, HandshakeState::Failed);
    }

    #[test]
    fn server_hello_with_unknown_suite_is_rejected() {
        let mut client = ClientHandshake::new();
        let ch = build_client_hello(
            &[0u8; 32],
            &[],
            &[CipherSuite::Aes128GcmSha256],
            &[1u8; 32],
            "h",
        );
        client.send_client_hello(&ch);
        // Hand-craft a ServerHello with an unsupported suite 0x00FF.
        let mut body = Vec::new();
        body.extend_from_slice(&[0x03, 0x03]);
        body.extend_from_slice(&[0u8; 32]);
        body.push(0);
        body.extend_from_slice(&[0x00, 0xFF]); // unknown suite
        body.push(0);
        body.extend_from_slice(&[0x00, 0x00]); // no extensions
        let sh = wrap_handshake(0x02, &body);
        assert!(client.recv_server_hello(&sh, &[2u8; 32]).is_none());
        assert_eq!(client.state, HandshakeState::Failed);
    }

    // ── Certificate chain + CertificateVerify authentication KATs ────────────
    //
    // These build a self-consistent PKI in-test (test root signs intermediate
    // signs leaf, all P-256 ECDSA) using only core/alloc + the crate's own
    // primitives, then drive the full authenticated flight. The negative
    // controls dominate: forged leaf sig, untrusted root, tampered transcript,
    // hostname mismatch, truncated message — each must reject without panicking.
    use der::asn1::{Any, BitString, GeneralizedTime, OctetString};
    use der::{Encode, Tag};
    use p256::ecdsa::signature::Signer as P256Signer;
    use p256::ecdsa::{Signature as P256Sig, SigningKey as P256Signing};
    use x509_cert::ext::pkix::name::GeneralName;
    use x509_cert::ext::pkix::SubjectAltName;
    use x509_cert::spki;
    use x509_cert::{Certificate, TbsCertificate};

    /// namedCurve secp256r1 (prime256v1) — the EC params OID for a P-256 SPKI.
    const OID_SECP256R1: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.2.840.10045.3.1.7");
    /// id-ce-subjectAltName (2.5.4 / 2.5.29.17).
    const OID_SAN: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.17");

    /// A deterministic P-256 signing key from a 32-byte seed (test-only).
    fn p256_key(seed: u8) -> P256Signing {
        // A non-zero, in-range scalar: seed in the high byte keeps it < n.
        let mut bytes = [1u8; 32];
        bytes[0] = seed;
        P256Signing::from_bytes(&bytes.into()).expect("valid p256 scalar")
    }

    /// Build a DER X.509 certificate: `subject` signed by `issuer_key` with
    /// `subject_key`'s public key as the SPKI, optional SAN dNSName, validity
    /// [nb, na] (unix secs). All P-256 ECDSA-with-SHA256.
    fn make_cert(
        subject_dn: &str,
        issuer_dn: &str,
        subject_key: &P256Signing,
        issuer_key: &P256Signing,
        san_dns: Option<&str>,
        nb: u64,
        na: u64,
        serial: u8,
    ) -> Vec<u8> {
        use core::str::FromStr;
        use core::time::Duration;
        use x509_cert::name::Name;
        use x509_cert::serial_number::SerialNumber;
        use x509_cert::time::{Time, Validity};

        let pub_point = subject_key.verifying_key().to_encoded_point(false);
        let spki_alg = spki::AlgorithmIdentifier {
            oid: OID_EC_PUBLIC_KEY,
            parameters: Some(Any::new(Tag::ObjectIdentifier, OID_SECP256R1.as_bytes()).unwrap()),
        };
        let spki = spki::SubjectPublicKeyInfo {
            algorithm: spki_alg,
            subject_public_key: BitString::from_bytes(pub_point.as_bytes()).unwrap(),
        };

        let sig_alg = spki::AlgorithmIdentifier {
            oid: OID_ECDSA_WITH_SHA256,
            parameters: None,
        };

        let validity = Validity {
            not_before: Time::GeneralTime(
                GeneralizedTime::from_unix_duration(Duration::from_secs(nb)).unwrap(),
            ),
            not_after: Time::GeneralTime(
                GeneralizedTime::from_unix_duration(Duration::from_secs(na)).unwrap(),
            ),
        };

        let extensions = san_dns.map(|dns| {
            let san = SubjectAltName(vec![GeneralName::DnsName(
                der::asn1::Ia5String::new(dns).unwrap(),
            )]);
            let san_der = san.to_der().unwrap();
            vec![x509_cert::ext::Extension {
                extn_id: OID_SAN,
                critical: false,
                extn_value: OctetString::new(san_der).unwrap(),
            }]
        });

        let tbs = TbsCertificate {
            version: x509_cert::certificate::Version::V3,
            serial_number: SerialNumber::new(&[serial]).unwrap(),
            signature: sig_alg.clone(),
            issuer: Name::from_str(issuer_dn).unwrap(),
            validity,
            subject: Name::from_str(subject_dn).unwrap(),
            subject_public_key_info: spki,
            issuer_unique_id: None,
            subject_unique_id: None,
            extensions,
        };

        let tbs_der = tbs.to_der().unwrap();
        let sig: P256Sig = issuer_key.sign(&tbs_der);
        let cert = Certificate {
            tbs_certificate: tbs,
            signature_algorithm: sig_alg,
            signature: BitString::from_bytes(sig.to_der().as_bytes()).unwrap(),
        };
        cert.to_der().unwrap()
    }

    /// A built test PKI: root (self-signed), intermediate, leaf, + the keys.
    struct TestPki {
        root_der: Vec<u8>,
        inter_der: Vec<u8>,
        leaf_der: Vec<u8>,
        leaf_key: P256Signing,
    }

    /// Build root→intermediate→leaf, leaf SAN = `leaf_dns`, all valid at t=1000.
    fn build_pki(leaf_dns: &str) -> TestPki {
        let root_k = p256_key(0x11);
        let inter_k = p256_key(0x22);
        let leaf_k = p256_key(0x33);
        let root_der = make_cert(
            "CN=Test Root",
            "CN=Test Root",
            &root_k,
            &root_k,
            None,
            0,
            100000,
            1,
        );
        let inter_der = make_cert(
            "CN=Test Intermediate",
            "CN=Test Root",
            &inter_k,
            &root_k,
            None,
            0,
            100000,
            2,
        );
        let leaf_der = make_cert(
            "CN=example.com",
            "CN=Test Intermediate",
            &leaf_k,
            &inter_k,
            Some(leaf_dns),
            0,
            100000,
            3,
        );
        TestPki {
            root_der,
            inter_der,
            leaf_der,
            leaf_key: leaf_k,
        }
    }

    /// Wrap a DER cert list into a TLS 1.3 Certificate handshake message.
    fn make_certificate_msg(chain: &[&[u8]]) -> Vec<u8> {
        let mut list = Vec::new();
        for cert in chain {
            // u24 cert length || cert || u16 ext len (0).
            list.push((cert.len() >> 16) as u8);
            list.push((cert.len() >> 8) as u8);
            list.push(cert.len() as u8);
            list.extend_from_slice(cert);
            list.extend_from_slice(&[0x00, 0x00]);
        }
        let mut body = Vec::new();
        body.push(0x00); // certificate_request_context = empty
        body.push((list.len() >> 16) as u8);
        body.push((list.len() >> 8) as u8);
        body.push(list.len() as u8);
        body.extend_from_slice(&list);
        wrap_handshake(0x0b, &body) // certificate = 11
    }

    /// Build a CertificateVerify message: ecdsa_secp256r1_sha256 over the §4.4.3
    /// content for `transcript_hash`, signed by `leaf_key`.
    fn make_cert_verify(leaf_key: &P256Signing, transcript_hash: &[u8]) -> Vec<u8> {
        let mut content = Vec::new();
        content.extend_from_slice(&[0x20u8; 64]);
        content.extend_from_slice(b"TLS 1.3, server CertificateVerify");
        content.push(0x00);
        content.extend_from_slice(transcript_hash);
        let sig: P256Sig = leaf_key.sign(&content);
        let sig_der = sig.to_der();
        let mut body = Vec::new();
        body.extend_from_slice(&0x0403u16.to_be_bytes()); // ecdsa_secp256r1_sha256
        body.extend_from_slice(&(sig_der.as_bytes().len() as u16).to_be_bytes());
        body.extend_from_slice(sig_der.as_bytes());
        wrap_handshake(0x0f, &body) // certificate_verify = 15
    }

    /// Drive a ClientHandshake to WaitFlight and run a test-server flight,
    /// returning the (client, outcome, server's would-be app secrets). The
    /// closure may corrupt the Certificate/CertVerify/Finished before delivery.
    fn run_authenticated(
        trust: TrustStore,
        host: &str,
        leaf_key: &P256Signing,
        cert_msg: Vec<u8>,
        tamper_transcript: bool,
    ) -> (ClientHandshake, Result<(), CertError>) {
        let (c_sk, c_pub) = x25519_pair(0x40);
        let (s_sk, s_pub) = x25519_pair(0x50);

        let mut client = ClientHandshake::new_authenticated(trust, host, Some(1000));
        let ch = build_client_hello(
            &[0x77u8; 32],
            &[],
            &[CipherSuite::Aes128GcmSha256],
            &c_pub,
            host,
        );
        client.send_client_hello(&ch);

        let suite = CipherSuite::Aes128GcmSha256;
        let sh = make_server_hello(suite, &s_pub);
        let server_shared = s_sk.diffie_hellman(&PublicKey::from(c_pub)).to_bytes();
        let mut sts = Transcript::new();
        sts.update(&ch);
        sts.update(&sh);
        let th_sh = sts.current_hash();
        let mut sks = KeySchedule::new();
        sks.derive_handshake_secrets(&server_shared, &th_sh);

        let client_shared = c_sk.diffie_hellman(&PublicKey::from(s_pub)).to_bytes();
        client.recv_server_hello(&sh, &client_shared).unwrap();

        // Server flight transcript: EE, Certificate, then CertificateVerify over
        // the transcript-through-Certificate.
        let ee = wrap_handshake(0x08, &[0x00, 0x00]);
        sts.update(&ee);
        sts.update(&cert_msg);
        let mut th_cert = sts.current_hash();
        if tamper_transcript {
            th_cert[0] ^= 0xFF; // server signs a DIFFERENT transcript than client sees
        }
        let cv = make_cert_verify(leaf_key, &th_cert);
        sts.update(&cv);
        let th_before_fin = sts.current_hash();
        let s_hs = sks.server_handshake_traffic.unwrap();
        let fk = finished_key(&s_hs);
        let vd = finished_verify_data(&fk, &th_before_fin);
        let fin = wrap_handshake(0x14, &vd);

        let outcome = client.recv_authenticated_flight(&ee, &cert_msg, &cv, &fin);
        (client, outcome)
    }

    #[test]
    fn authenticated_handshake_valid_chain_reaches_connected() {
        let pki = build_pki("example.com");
        let mut trust = TrustStore::new();
        assert!(trust.add_root_der(&pki.root_der));
        let cert_msg = make_certificate_msg(&[&pki.leaf_der, &pki.inter_der]);
        let (client, outcome) =
            run_authenticated(trust, "example.com", &pki.leaf_key, cert_msg, false);
        assert!(
            outcome.is_ok(),
            "valid chain + CertVerify => Ok, got {:?}",
            outcome
        );
        assert_eq!(client.state, HandshakeState::Connected);
        // Application keys are exposed only on success.
        assert!(client.client_app_key_iv().is_some());
        assert!(client.server_app_key_iv().is_some());
    }

    #[test]
    fn forged_leaf_signature_is_rejected() {
        // Sign the leaf with the WRONG issuer (root, not intermediate) => the
        // intermediate's key will not verify the leaf signature.
        let root_k = p256_key(0x11);
        let inter_k = p256_key(0x22);
        let leaf_k = p256_key(0x33);
        let root_der = make_cert(
            "CN=Test Root",
            "CN=Test Root",
            &root_k,
            &root_k,
            None,
            0,
            100000,
            1,
        );
        let inter_der = make_cert(
            "CN=Test Intermediate",
            "CN=Test Root",
            &inter_k,
            &root_k,
            None,
            0,
            100000,
            2,
        );
        // Leaf claims issuer=Intermediate but is signed by the ROOT key (forgery).
        let leaf_der = make_cert(
            "CN=example.com",
            "CN=Test Intermediate",
            &leaf_k,
            &root_k,
            Some("example.com"),
            0,
            100000,
            3,
        );
        let mut trust = TrustStore::new();
        trust.add_root_der(&root_der);
        let cert_msg = make_certificate_msg(&[&leaf_der, &inter_der]);
        let (client, outcome) = run_authenticated(trust, "example.com", &leaf_k, cert_msg, false);
        assert_eq!(outcome, Err(CertError::BadSignature));
        assert_eq!(client.state, HandshakeState::Failed);
        assert!(
            client.client_app_key_iv().is_none(),
            "no app keys on failure"
        );
    }

    #[test]
    fn untrusted_root_is_rejected() {
        let pki = build_pki("example.com");
        // Trust store holds a DIFFERENT root => the chain cannot anchor.
        let other_root_k = p256_key(0x99);
        let other_root = make_cert(
            "CN=Other Root",
            "CN=Other Root",
            &other_root_k,
            &other_root_k,
            None,
            0,
            100000,
            1,
        );
        let mut trust = TrustStore::new();
        trust.add_root_der(&other_root);
        let cert_msg = make_certificate_msg(&[&pki.leaf_der, &pki.inter_der]);
        let (client, outcome) =
            run_authenticated(trust, "example.com", &pki.leaf_key, cert_msg, false);
        assert_eq!(outcome, Err(CertError::UntrustedRoot));
        assert_eq!(client.state, HandshakeState::Failed);
        assert!(client.client_app_key_iv().is_none());
    }

    #[test]
    fn empty_trust_store_fails_closed() {
        let pki = build_pki("example.com");
        let trust = TrustStore::new(); // no anchors
        let cert_msg = make_certificate_msg(&[&pki.leaf_der, &pki.inter_der]);
        let (client, outcome) =
            run_authenticated(trust, "example.com", &pki.leaf_key, cert_msg, false);
        assert_eq!(outcome, Err(CertError::UntrustedRoot));
        assert_eq!(client.state, HandshakeState::Failed);
    }

    #[test]
    fn tampered_transcript_certificate_verify_is_rejected() {
        // The server signs the CertificateVerify over a transcript hash that
        // differs from what the client computed (a MITM splice) => reject.
        let pki = build_pki("example.com");
        let mut trust = TrustStore::new();
        trust.add_root_der(&pki.root_der);
        let cert_msg = make_certificate_msg(&[&pki.leaf_der, &pki.inter_der]);
        let (client, outcome) =
            run_authenticated(trust, "example.com", &pki.leaf_key, cert_msg, true);
        assert_eq!(outcome, Err(CertError::BadCertificateVerify));
        assert_eq!(client.state, HandshakeState::Failed);
        assert!(client.client_app_key_iv().is_none());
    }

    #[test]
    fn hostname_mismatch_is_rejected() {
        // Leaf SAN = example.com but we dialed evil.com.
        let pki = build_pki("example.com");
        let mut trust = TrustStore::new();
        trust.add_root_der(&pki.root_der);
        let cert_msg = make_certificate_msg(&[&pki.leaf_der, &pki.inter_der]);
        let (client, outcome) =
            run_authenticated(trust, "evil.com", &pki.leaf_key, cert_msg, false);
        assert_eq!(outcome, Err(CertError::HostnameMismatch));
        assert_eq!(client.state, HandshakeState::Failed);
        assert!(client.client_app_key_iv().is_none());
    }

    #[test]
    fn expired_certificate_is_rejected() {
        // Leaf valid only [0, 500]; the handshake clock is 1000 => Expired.
        let root_k = p256_key(0x11);
        let inter_k = p256_key(0x22);
        let leaf_k = p256_key(0x33);
        let root_der = make_cert(
            "CN=Test Root",
            "CN=Test Root",
            &root_k,
            &root_k,
            None,
            0,
            100000,
            1,
        );
        let inter_der = make_cert(
            "CN=Test Intermediate",
            "CN=Test Root",
            &inter_k,
            &root_k,
            None,
            0,
            100000,
            2,
        );
        let leaf_der = make_cert(
            "CN=example.com",
            "CN=Test Intermediate",
            &leaf_k,
            &inter_k,
            Some("example.com"),
            0,
            500, // notAfter = 500 < now(1000)
            3,
        );
        let mut trust = TrustStore::new();
        trust.add_root_der(&root_der);
        let cert_msg = make_certificate_msg(&[&leaf_der, &inter_der]);
        let (client, outcome) = run_authenticated(trust, "example.com", &leaf_k, cert_msg, false);
        assert_eq!(outcome, Err(CertError::Expired));
        assert_eq!(client.state, HandshakeState::Failed);
    }

    #[test]
    fn truncated_certificate_message_is_clean_err_not_panic() {
        // A Certificate body that lies about lengths must Err, never panic.
        assert_eq!(
            parse_certificate_message(&[]).err(),
            Some(CertError::MalformedMessage)
        );
        // ctx len says 5 but no bytes follow.
        assert_eq!(
            parse_certificate_message(&[0x05]).err(),
            Some(CertError::MalformedMessage)
        );
        // ctx=0, list len = 0x000010 but list is short.
        assert_eq!(
            parse_certificate_message(&[0x00, 0x00, 0x00, 0x10, 0xAA]).err(),
            Some(CertError::MalformedMessage)
        );
        // Well-formed framing but cert bytes are not DER => caught at validate.
        let pki = build_pki("example.com");
        let mut bad = make_certificate_msg(&[&pki.leaf_der]);
        // Corrupt the leaf DER body (flip a byte deep inside) — chain build fails.
        let mid = bad.len() / 2;
        bad[mid] ^= 0xFF;
        let mut trust = TrustStore::new();
        trust.add_root_der(&pki.root_der);
        // Drive through the flight; outcome must be an Err (no panic, no connect).
        let body = &bad[4..];
        // parse may succeed (framing intact) but validate_chain must reject.
        if let Ok(chain) = parse_certificate_message(body) {
            let r = validate_chain(&chain, &trust, Some(1000));
            assert!(r.is_err(), "corrupted cert must not validate");
        }
    }

    #[test]
    fn out_of_order_authenticated_flight_fails_cleanly() {
        // Calling the authenticated flight before ServerHello => Failed, no panic.
        let pki = build_pki("example.com");
        let mut trust = TrustStore::new();
        trust.add_root_der(&pki.root_der);
        let mut client = ClientHandshake::new_authenticated(trust, "example.com", Some(1000));
        let cert_msg = make_certificate_msg(&[&pki.leaf_der, &pki.inter_der]);
        let ee = wrap_handshake(0x08, &[0x00, 0x00]);
        let cv = wrap_handshake(0x0f, &[0x04, 0x03, 0x00, 0x00]);
        let fin = wrap_handshake(0x14, &[0u8; HASH_LEN]);
        let r = client.recv_authenticated_flight(&ee, &cert_msg, &cv, &fin);
        assert_eq!(r, Err(CertError::MalformedMessage));
        assert_eq!(client.state, HandshakeState::Failed);
    }

    #[test]
    fn extract_dns_names_prefers_san_then_cn() {
        let pki = build_pki("api.example.com");
        let names = extract_dns_names(&pki.leaf_der);
        assert!(
            names.iter().any(|n| n == "api.example.com"),
            "SAN dNSName present"
        );
        // A cert with no SAN falls back to the CN.
        let k = p256_key(0x44);
        let no_san = make_cert(
            "CN=cn-only.example",
            "CN=cn-only.example",
            &k,
            &k,
            None,
            0,
            100000,
            7,
        );
        let cn_names = extract_dns_names(&no_san);
        assert!(
            cn_names.iter().any(|n| n == "cn-only.example"),
            "CN fallback"
        );
    }

    #[test]
    fn hostname_matching_exact_and_wildcard() {
        // Exact, case-insensitive.
        assert!(hostname_matches("example.com", "example.com"));
        assert!(hostname_matches("Example.COM", "example.com"));
        assert!(!hostname_matches("example.com", "evil.com"));
        // Single left-most wildcard matches exactly one label.
        assert!(hostname_matches("*.example.com", "a.example.com"));
        assert!(hostname_matches("*.example.com", "WWW.example.com"));
        // Wildcard does NOT match the bare domain or a deeper subdomain.
        assert!(!hostname_matches("*.example.com", "example.com"));
        assert!(!hostname_matches("*.example.com", "a.b.example.com"));
        // Public-suffix wildcards and multi-* are rejected.
        assert!(!hostname_matches("*.com", "evil.com"));
        assert!(!hostname_matches("*", "anything"));
        assert!(!hostname_matches("a*.example.com", "ab.example.com")); // partial-* not allowed
                                                                        // Trailing dots normalized.
        assert!(hostname_matches("example.com.", "example.com"));
        // verify_hostname: any SAN entry matching wins.
        assert!(verify_hostname(
            &["foo.com", "*.example.com"],
            "api.example.com"
        ));
        assert!(!verify_hostname(&["foo.com", "bar.com"], "api.example.com"));
        assert!(!verify_hostname(&[], "example.com"));
    }

    // ── Real openssl-issued certs as external oracles ───────────────────────
    // The synthetic `make_cert` fixtures above round-trip our own encoder, so a
    // bug shared between our encode and decode would hide. These two certs were
    // minted by `openssl req -x509` (Ed25519 and P-256) with fixed validity
    // windows, so they exercise `cert_time_valid` / `cert_public_key` against an
    // independent implementation — the class of oracle that caught the P-256
    // DNSSEC label bug that self-signed round-trips masked.
    //
    // ED25519_CERT: CN=raeentested, valid 2025-01-01T00:00:00Z .. 2035-01-01Z.
    const ED25519_CERT: [u8; 324] = [
        0x30, 0x82, 0x01, 0x40, 0x30, 0x81, 0xf3, 0xa0, 0x03, 0x02, 0x01, 0x02, 0x02, 0x14, 0x0b,
        0x78, 0xf6, 0x23, 0x06, 0x8b, 0xc0, 0xa1, 0xdf, 0x0d, 0x3c, 0x12, 0x14, 0xee, 0x8b, 0xb1,
        0xa3, 0x2f, 0x06, 0xa8, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x30, 0x16, 0x31, 0x14,
        0x30, 0x12, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x0b, 0x72, 0x61, 0x65, 0x65, 0x6e, 0x74,
        0x65, 0x73, 0x74, 0x65, 0x64, 0x30, 0x1e, 0x17, 0x0d, 0x32, 0x35, 0x30, 0x31, 0x30, 0x31,
        0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a, 0x17, 0x0d, 0x33, 0x35, 0x30, 0x31, 0x30, 0x31,
        0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a, 0x30, 0x16, 0x31, 0x14, 0x30, 0x12, 0x06, 0x03,
        0x55, 0x04, 0x03, 0x0c, 0x0b, 0x72, 0x61, 0x65, 0x65, 0x6e, 0x74, 0x65, 0x73, 0x74, 0x65,
        0x64, 0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00, 0x1a, 0x5a,
        0x42, 0x0c, 0xb1, 0xb9, 0xc2, 0x5d, 0x09, 0x82, 0x26, 0x99, 0x32, 0x6f, 0x6f, 0xba, 0x5b,
        0xbc, 0x54, 0xa9, 0x04, 0x65, 0xaa, 0xcc, 0x2c, 0xe0, 0x63, 0xcd, 0x2f, 0xb9, 0x0c, 0x61,
        0xa3, 0x53, 0x30, 0x51, 0x30, 0x1d, 0x06, 0x03, 0x55, 0x1d, 0x0e, 0x04, 0x16, 0x04, 0x14,
        0xb8, 0xa9, 0xa5, 0x45, 0x33, 0x01, 0x96, 0x41, 0x93, 0xb8, 0x75, 0xc3, 0x04, 0xc2, 0xbd,
        0x4a, 0x2f, 0x4a, 0x93, 0x3b, 0x30, 0x1f, 0x06, 0x03, 0x55, 0x1d, 0x23, 0x04, 0x18, 0x30,
        0x16, 0x80, 0x14, 0xb8, 0xa9, 0xa5, 0x45, 0x33, 0x01, 0x96, 0x41, 0x93, 0xb8, 0x75, 0xc3,
        0x04, 0xc2, 0xbd, 0x4a, 0x2f, 0x4a, 0x93, 0x3b, 0x30, 0x0f, 0x06, 0x03, 0x55, 0x1d, 0x13,
        0x01, 0x01, 0xff, 0x04, 0x05, 0x30, 0x03, 0x01, 0x01, 0xff, 0x30, 0x05, 0x06, 0x03, 0x2b,
        0x65, 0x70, 0x03, 0x41, 0x00, 0x80, 0x1b, 0x5a, 0x62, 0xc7, 0x16, 0xef, 0xba, 0x13, 0x8b,
        0x05, 0xb2, 0x55, 0x41, 0xd2, 0xbe, 0x2c, 0x27, 0xec, 0xee, 0xdf, 0x1e, 0xb4, 0x30, 0x65,
        0x44, 0x56, 0x53, 0xca, 0x26, 0x60, 0xc4, 0x4f, 0x82, 0xa3, 0xe1, 0x21, 0xbf, 0x90, 0xa2,
        0x86, 0x85, 0xe0, 0x0e, 0x05, 0xeb, 0xed, 0xb8, 0x1f, 0xe7, 0x06, 0x1a, 0x94, 0x26, 0xac,
        0xc1, 0xe0, 0xc5, 0xf8, 0x6e, 0xab, 0xfc, 0x0d, 0x05,
    ];
    // P256_CERT: CN=raeentestp256, valid 2025-06-01T00:00:00Z .. 2034-06-01Z.
    const P256_CERT: [u8; 393] = [
        0x30, 0x82, 0x01, 0x85, 0x30, 0x82, 0x01, 0x2b, 0xa0, 0x03, 0x02, 0x01, 0x02, 0x02, 0x14,
        0x1f, 0x61, 0xd6, 0x0b, 0x39, 0x58, 0x64, 0x29, 0x5e, 0x45, 0x7e, 0x09, 0xd8, 0x22, 0xb4,
        0xf4, 0xcd, 0x60, 0xce, 0x5c, 0x30, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04,
        0x03, 0x02, 0x30, 0x18, 0x31, 0x16, 0x30, 0x14, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x0d,
        0x72, 0x61, 0x65, 0x65, 0x6e, 0x74, 0x65, 0x73, 0x74, 0x70, 0x32, 0x35, 0x36, 0x30, 0x1e,
        0x17, 0x0d, 0x32, 0x35, 0x30, 0x36, 0x30, 0x31, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a,
        0x17, 0x0d, 0x33, 0x34, 0x30, 0x36, 0x30, 0x31, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a,
        0x30, 0x18, 0x31, 0x16, 0x30, 0x14, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x0d, 0x72, 0x61,
        0x65, 0x65, 0x6e, 0x74, 0x65, 0x73, 0x74, 0x70, 0x32, 0x35, 0x36, 0x30, 0x59, 0x30, 0x13,
        0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce,
        0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00, 0x04, 0xc5, 0x93, 0xf0, 0x87, 0x78, 0xf6, 0x22,
        0xc0, 0xe0, 0xde, 0x3d, 0xb4, 0x65, 0x59, 0xda, 0x1e, 0x2f, 0x2e, 0xc4, 0x3e, 0xbd, 0x80,
        0xeb, 0x2e, 0x06, 0xcf, 0x22, 0xd6, 0x42, 0x3b, 0xad, 0xe5, 0x6a, 0x46, 0x42, 0x96, 0x32,
        0x7a, 0x52, 0xde, 0xf6, 0x0e, 0xf4, 0xf5, 0xc3, 0x0b, 0x3b, 0x25, 0x22, 0x36, 0xa1, 0xd0,
        0x6c, 0x76, 0x87, 0x21, 0xff, 0x41, 0x7d, 0x8c, 0x71, 0x4b, 0xea, 0x81, 0xa3, 0x53, 0x30,
        0x51, 0x30, 0x1d, 0x06, 0x03, 0x55, 0x1d, 0x0e, 0x04, 0x16, 0x04, 0x14, 0xfc, 0x35, 0xbd,
        0xe8, 0x22, 0x77, 0xbd, 0x6b, 0xb2, 0x35, 0x19, 0xf2, 0x9e, 0xab, 0xc1, 0xd8, 0xf0, 0xab,
        0xa9, 0x6b, 0x30, 0x1f, 0x06, 0x03, 0x55, 0x1d, 0x23, 0x04, 0x18, 0x30, 0x16, 0x80, 0x14,
        0xfc, 0x35, 0xbd, 0xe8, 0x22, 0x77, 0xbd, 0x6b, 0xb2, 0x35, 0x19, 0xf2, 0x9e, 0xab, 0xc1,
        0xd8, 0xf0, 0xab, 0xa9, 0x6b, 0x30, 0x0f, 0x06, 0x03, 0x55, 0x1d, 0x13, 0x01, 0x01, 0xff,
        0x04, 0x05, 0x30, 0x03, 0x01, 0x01, 0xff, 0x30, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce,
        0x3d, 0x04, 0x03, 0x02, 0x03, 0x48, 0x00, 0x30, 0x45, 0x02, 0x21, 0x00, 0xbe, 0x4d, 0xb5,
        0x52, 0xfa, 0x88, 0x01, 0x25, 0x7c, 0xb6, 0xe5, 0x37, 0xe4, 0x9e, 0xa0, 0x22, 0x09, 0xa0,
        0xb9, 0x71, 0xb3, 0x8d, 0x38, 0x05, 0x23, 0x58, 0x7d, 0x86, 0xee, 0x3f, 0x09, 0x18, 0x02,
        0x20, 0x1c, 0x9a, 0x6c, 0xda, 0xbd, 0xa1, 0xb1, 0x82, 0x11, 0xf0, 0x64, 0xd7, 0x4b, 0xf1,
        0x10, 0xda, 0x82, 0xa0, 0x01, 0x78, 0xd3, 0xaa, 0x29, 0x6d, 0x03, 0x05, 0x82, 0x2a, 0x73,
        0x4a, 0xaf, 0xa2,
    ];

    #[test]
    fn cert_time_valid_enforces_the_validity_window() {
        // In-window (2027-ish, epoch 1_800_000_000) → accepted.
        assert!(cert_time_valid(&ED25519_CERT, Some(1_800_000_000)).is_ok());
        // Past notAfter (2036-ish) → Expired. This is the arm that stops a MITM
        // presenting a long-revoked-and-expired cert; neutering the `now > na`
        // half of the check makes this assertion fail.
        assert_eq!(
            cert_time_valid(&ED25519_CERT, Some(2_100_000_000)),
            Err(CertError::Expired)
        );
        // Before notBefore (2023-ish) → Expired. Guards the `now < nb` half.
        assert_eq!(
            cert_time_valid(&ED25519_CERT, Some(1_700_000_000)),
            Err(CertError::Expired)
        );
        // No clock supplied → check intentionally skipped (documented follow-up:
        // wire AthNet's wall clock so this branch never runs in production).
        assert!(cert_time_valid(&ED25519_CERT, None).is_ok());
        // Malformed DER with a clock → MalformedCert, never a panic, never Ok.
        assert_eq!(
            cert_time_valid(&[0x30, 0x00], Some(1_800_000_000)),
            Err(CertError::MalformedCert)
        );
    }

    #[test]
    fn cert_public_key_extracts_p256_and_rejects_unsupported() {
        // The exact SEC1 point openssl put in the P-256 cert's SPKI.
        const P256_POINT: [u8; 65] = [
            0x04, 0xc5, 0x93, 0xf0, 0x87, 0x78, 0xf6, 0x22, 0xc0, 0xe0, 0xde, 0x3d, 0xb4, 0x65,
            0x59, 0xda, 0x1e, 0x2f, 0x2e, 0xc4, 0x3e, 0xbd, 0x80, 0xeb, 0x2e, 0x06, 0xcf, 0x22,
            0xd6, 0x42, 0x3b, 0xad, 0xe5, 0x6a, 0x46, 0x42, 0x96, 0x32, 0x7a, 0x52, 0xde, 0xf6,
            0x0e, 0xf4, 0xf5, 0xc3, 0x0b, 0x3b, 0x25, 0x22, 0x36, 0xa1, 0xd0, 0x6c, 0x76, 0x87,
            0x21, 0xff, 0x41, 0x7d, 0x8c, 0x71, 0x4b, 0xea, 0x81,
        ];
        match cert_public_key(&P256_CERT) {
            Some(CertPublicKey::P256(pt)) => assert_eq!(pt, P256_POINT.to_vec()),
            _ => panic!("expected a P-256 key extracted from the real cert"),
        }
        // Ed25519 SPKI is a key type we deliberately do not verify → None (the
        // caller maps this to UnsupportedKey, fail-closed — not a silent accept).
        assert!(cert_public_key(&ED25519_CERT).is_none());
        // Garbage DER → None, no panic.
        assert!(cert_public_key(&[0x30, 0x00]).is_none());
    }

    // ── RSA (rsaEncryption / sha256WithRSAEncryption) certificate support ─────
    //
    // Real openssl-issued 2048-bit RSA self-signed cert (external oracle):
    //   `openssl genrsa 2048` + `openssl req -x509 -sha256`, SAN=raenet-rsa.example.com,
    //   valid 2025-01-01 .. 2035-01-01, self-verifies (`openssl verify` → OK).
    // This is the regression guard for the cert_public_key RSA path: the branch used
    // to mislabel an rsaEncryption SPKI as `CertPublicKey::P256`, which silently broke
    // EVERY RSA chain (an RSA modulus is not a P-256 point). The bug survived because
    // only the P-256/Ed25519 extraction paths were tested — this vector closes that hole.
    const RSA_CERT: [u8; 842] = [
        0x30, 0x82, 0x03, 0x46, 0x30, 0x82, 0x02, 0x2e, 0xa0, 0x03, 0x02, 0x01, 0x02, 0x02, 0x14,
        0x4a, 0x55, 0xc1, 0x13, 0x95, 0x42, 0xdf, 0xec, 0xef, 0xf7, 0x63, 0x56, 0xe5, 0x65, 0x51,
        0xe2, 0xa6, 0xd7, 0xd6, 0x1d, 0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d,
        0x01, 0x01, 0x0b, 0x05, 0x00, 0x30, 0x21, 0x31, 0x1f, 0x30, 0x1d, 0x06, 0x03, 0x55, 0x04,
        0x03, 0x0c, 0x16, 0x72, 0x61, 0x65, 0x6e, 0x65, 0x74, 0x2d, 0x72, 0x73, 0x61, 0x2e, 0x65,
        0x78, 0x61, 0x6d, 0x70, 0x6c, 0x65, 0x2e, 0x63, 0x6f, 0x6d, 0x30, 0x1e, 0x17, 0x0d, 0x32,
        0x35, 0x30, 0x31, 0x30, 0x31, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a, 0x17, 0x0d, 0x33,
        0x35, 0x30, 0x31, 0x30, 0x31, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a, 0x30, 0x21, 0x31,
        0x1f, 0x30, 0x1d, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x16, 0x72, 0x61, 0x65, 0x6e, 0x65,
        0x74, 0x2d, 0x72, 0x73, 0x61, 0x2e, 0x65, 0x78, 0x61, 0x6d, 0x70, 0x6c, 0x65, 0x2e, 0x63,
        0x6f, 0x6d, 0x30, 0x82, 0x01, 0x22, 0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7,
        0x0d, 0x01, 0x01, 0x01, 0x05, 0x00, 0x03, 0x82, 0x01, 0x0f, 0x00, 0x30, 0x82, 0x01, 0x0a,
        0x02, 0x82, 0x01, 0x01, 0x00, 0xe0, 0xd1, 0xdd, 0xca, 0x91, 0x95, 0x27, 0x23, 0x83, 0x47,
        0x76, 0x7d, 0x19, 0xdc, 0x34, 0x0d, 0x7b, 0x37, 0xb2, 0xf8, 0x4c, 0xcf, 0x34, 0xa2, 0xff,
        0x41, 0xb4, 0x2b, 0xac, 0xae, 0xb3, 0x80, 0xf8, 0x74, 0x69, 0xba, 0x64, 0xcd, 0x5a, 0xef,
        0x7a, 0xde, 0x8f, 0x66, 0xde, 0x54, 0x97, 0xf2, 0x10, 0xc1, 0x1f, 0xce, 0x6c, 0x01, 0x9b,
        0xeb, 0x8f, 0xc6, 0xb4, 0x23, 0xd0, 0xa1, 0x01, 0x5b, 0x56, 0xcb, 0x4e, 0x77, 0x6a, 0x55,
        0xd5, 0x38, 0x74, 0x2c, 0xbe, 0x2c, 0x8c, 0x8d, 0xa0, 0xde, 0x97, 0x3d, 0x2e, 0xad, 0x6f,
        0x2c, 0xdc, 0xdf, 0x7a, 0x8d, 0xe1, 0xd1, 0xc9, 0x9a, 0xea, 0x18, 0xb3, 0xd1, 0x8d, 0x05,
        0x0c, 0xf8, 0xe7, 0x08, 0xf7, 0x26, 0x0e, 0x93, 0x3b, 0xab, 0xf9, 0x6a, 0xc4, 0x33, 0x90,
        0xbb, 0x55, 0x80, 0x77, 0xd3, 0xf1, 0x0c, 0x6d, 0xc4, 0xc3, 0xad, 0xae, 0xfa, 0x0d, 0xd3,
        0x92, 0xed, 0xda, 0xa8, 0x25, 0x9f, 0xab, 0x80, 0x8b, 0x1a, 0x02, 0x07, 0x93, 0xb6, 0xd9,
        0x4f, 0xcb, 0x26, 0x40, 0xe1, 0xbe, 0x5f, 0xe0, 0xd7, 0x1e, 0xeb, 0xce, 0x25, 0x0e, 0x9b,
        0x61, 0xb7, 0x14, 0xcf, 0x33, 0x7b, 0x19, 0x4b, 0x06, 0x8a, 0xf2, 0x31, 0x0a, 0x65, 0xb5,
        0x53, 0x8c, 0xa9, 0xb3, 0x65, 0x19, 0x5e, 0x0e, 0x9a, 0x08, 0xcb, 0xf9, 0x1a, 0xf2, 0x5f,
        0x60, 0xb0, 0x7b, 0xc1, 0x81, 0x1a, 0xca, 0xa7, 0xa7, 0x59, 0xa3, 0xf6, 0x99, 0xf7, 0x2f,
        0x46, 0xc3, 0x94, 0x00, 0x28, 0xb7, 0x8b, 0xdd, 0x2c, 0x17, 0x9d, 0x91, 0x06, 0xbc, 0x37,
        0xb9, 0xb9, 0xec, 0x4b, 0x14, 0xb3, 0x5b, 0x23, 0x5a, 0x1c, 0x64, 0xd4, 0x8a, 0x13, 0x32,
        0xa1, 0x6a, 0x93, 0xc5, 0xf4, 0x7e, 0x4e, 0x03, 0x3a, 0x89, 0x0a, 0xf4, 0x0d, 0x4b, 0x7f,
        0xbf, 0xf6, 0x1c, 0x4f, 0x60, 0x83, 0x02, 0x03, 0x01, 0x00, 0x01, 0xa3, 0x76, 0x30, 0x74,
        0x30, 0x1d, 0x06, 0x03, 0x55, 0x1d, 0x0e, 0x04, 0x16, 0x04, 0x14, 0xb7, 0xf5, 0x5b, 0xf7,
        0x03, 0xfe, 0x95, 0xe4, 0x27, 0x79, 0x13, 0x09, 0x2e, 0x37, 0x39, 0x3b, 0x94, 0xcb, 0xdc,
        0x59, 0x30, 0x1f, 0x06, 0x03, 0x55, 0x1d, 0x23, 0x04, 0x18, 0x30, 0x16, 0x80, 0x14, 0xb7,
        0xf5, 0x5b, 0xf7, 0x03, 0xfe, 0x95, 0xe4, 0x27, 0x79, 0x13, 0x09, 0x2e, 0x37, 0x39, 0x3b,
        0x94, 0xcb, 0xdc, 0x59, 0x30, 0x0f, 0x06, 0x03, 0x55, 0x1d, 0x13, 0x01, 0x01, 0xff, 0x04,
        0x05, 0x30, 0x03, 0x01, 0x01, 0xff, 0x30, 0x21, 0x06, 0x03, 0x55, 0x1d, 0x11, 0x04, 0x1a,
        0x30, 0x18, 0x82, 0x16, 0x72, 0x61, 0x65, 0x6e, 0x65, 0x74, 0x2d, 0x72, 0x73, 0x61, 0x2e,
        0x65, 0x78, 0x61, 0x6d, 0x70, 0x6c, 0x65, 0x2e, 0x63, 0x6f, 0x6d, 0x30, 0x0d, 0x06, 0x09,
        0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b, 0x05, 0x00, 0x03, 0x82, 0x01, 0x01,
        0x00, 0xba, 0x63, 0xb0, 0x91, 0x37, 0xdb, 0x0a, 0x0f, 0x44, 0x30, 0x22, 0x21, 0xab, 0x9c,
        0x4f, 0x87, 0x0d, 0xe0, 0xb7, 0x65, 0xae, 0x7f, 0xdf, 0x18, 0xf6, 0xe0, 0x19, 0xe0, 0xb5,
        0x2c, 0xe4, 0x76, 0x2a, 0xc7, 0x31, 0xab, 0x13, 0x46, 0xf8, 0xa0, 0x2c, 0xc9, 0xe8, 0xea,
        0x2c, 0x10, 0xd5, 0x9b, 0xd7, 0x9e, 0x39, 0x9b, 0xaa, 0xe1, 0x54, 0x5f, 0x9b, 0xa9, 0xdd,
        0x22, 0x0d, 0x80, 0xa8, 0x8c, 0x48, 0x78, 0x84, 0x93, 0x8e, 0x5c, 0x1f, 0x1d, 0x61, 0xa4,
        0xde, 0xab, 0x03, 0x9f, 0xec, 0x75, 0x77, 0x05, 0x30, 0x17, 0x22, 0x74, 0x26, 0x42, 0xb7,
        0x15, 0x38, 0x77, 0x42, 0x2a, 0xfa, 0x35, 0xa1, 0xd0, 0x52, 0xd4, 0xc5, 0x9a, 0x65, 0x47,
        0xe7, 0x21, 0x03, 0x10, 0x14, 0xbd, 0x69, 0xbe, 0x93, 0xe0, 0x12, 0x28, 0xe7, 0xaf, 0x4d,
        0x79, 0x6c, 0xeb, 0xc1, 0xdb, 0x80, 0x42, 0x5b, 0xcb, 0x6e, 0xe7, 0xfd, 0xfa, 0xbe, 0xa3,
        0x2e, 0xda, 0x30, 0x3f, 0xae, 0x13, 0x59, 0xa0, 0x02, 0x65, 0xfc, 0x06, 0x01, 0x57, 0xba,
        0xb2, 0x02, 0x2b, 0x9d, 0xa0, 0x8d, 0xb2, 0x08, 0x04, 0x29, 0xc1, 0xd9, 0x05, 0x7f, 0x74,
        0xf5, 0x1e, 0xa4, 0x92, 0x60, 0x87, 0x8b, 0xb9, 0xdc, 0x0d, 0xa6, 0xf2, 0xef, 0x28, 0xc0,
        0x79, 0xe4, 0x1b, 0x7f, 0x98, 0x04, 0x42, 0x9d, 0x47, 0x48, 0x5d, 0xd6, 0xec, 0x69, 0xc3,
        0x6c, 0x50, 0xc6, 0xcd, 0xf9, 0x75, 0x3f, 0xa8, 0xd3, 0x0c, 0xd6, 0x61, 0x96, 0x7a, 0xf0,
        0x50, 0xcc, 0x18, 0x12, 0xfb, 0xa0, 0xf1, 0x77, 0xd8, 0xd9, 0xe8, 0x0b, 0xa9, 0xd1, 0x16,
        0x33, 0x7e, 0x64, 0x4d, 0xd2, 0x23, 0x82, 0xdb, 0x55, 0x11, 0x66, 0xe3, 0x50, 0x9b, 0x7f,
        0xbe, 0x15, 0x4c, 0x8c, 0x67, 0xd1, 0x1c, 0x05, 0x9f, 0x93, 0x00, 0x44, 0x3c, 0xfc, 0x9a,
        0xce, 0x8b,
    ];

    #[test]
    fn cert_public_key_extracts_rsa() {
        // The RSA path MUST yield the Rsa variant, not P256. This assertion goes
        // RED if the branch is reverted to `CertPublicKey::P256(...)` — it is the
        // guard the mislabel bug slipped past for lack of an RSA extraction test.
        match cert_public_key(&RSA_CERT) {
            Some(CertPublicKey::Rsa(pk)) => {
                // The extracted bytes must be a well-formed PKCS#1 RSAPublicKey DER
                // (a P-256 misroute would carry these same bytes but under the wrong
                // variant, so verify_cert_signature could never consume them).
                assert_eq!(pk[0], 0x30, "RSAPublicKey is a DER SEQUENCE");
                assert!(pk.len() > 256, "a 2048-bit modulus is >256 bytes");
            }
            other => panic!(
                "expected CertPublicKey::Rsa from the real RSA cert, got Rsa={}",
                matches!(other, Some(CertPublicKey::Rsa(_)))
            ),
        }
    }

    #[test]
    fn verify_cert_signature_rsa_accepts_genuine_rejects_tampered() {
        // Issuer key = the self-signed RSA cert's own key.
        let rsa_key = cert_public_key(&RSA_CERT).expect("RSA key");
        assert!(matches!(rsa_key, CertPublicKey::Rsa(_)));

        // (a) The genuine self-signed RSA cert verifies under its own key. This is
        // the end-to-end proof the fix restores: with the P256 mislabel this arm
        // returned Err(BadSignature) because verify_cert_signature's SHA256_WITH_RSA
        // branch only matches `CertPublicKey::Rsa`.
        assert_eq!(verify_cert_signature(&RSA_CERT, &rsa_key), Ok(()));

        // (b) A flipped byte in the trailing signature BIT STRING (offset 822) keeps
        // the DER structurally valid but breaks the RSA signature → BadSignature.
        let mut tampered = RSA_CERT.to_vec();
        tampered[822] ^= 0x01;
        assert_eq!(
            verify_cert_signature(&tampered, &rsa_key),
            Err(CertError::BadSignature),
            "a tampered RSA signature must NOT verify"
        );

        // (c) Presenting a P-256 key for a sha256WithRSAEncryption signature is a
        // type mismatch → BadSignature (fail-closed).
        let wrong_type = CertPublicKey::P256(vec![0x04; 65]);
        assert_eq!(
            verify_cert_signature(&RSA_CERT, &wrong_type),
            Err(CertError::BadSignature)
        );
    }

    // ── ECDSA P-384 (secp384r1 / SHA-384) certificate support ────────────────
    //
    // Real openssl-issued P-384 PKI (external oracle — generated with
    // `openssl ecparam -name secp384r1` + `openssl x509 -req ... -sha384`):
    //   P384_ROOT_CERT — self-signed CA "CN=AthNet P384 Test Root", ecdsa-with-SHA384.
    //   P384_LEAF_CERT — "CN=raenet-p384.example.com" (SAN dNSName), signed by root.
    //   P384_CV_SIG    — an ecdsa_secp384r1_sha384 signature by the LEAF key over the
    //                    RFC 8446 §4.4.3 CertificateVerify content for the fixed
    //                    transcript hash `(0u8..32u8)`.
    // Both certs verify with `openssl verify -CAfile root.pem leaf.pem` → OK.
    // These close the gap where AthNet could not validate P-384 chains (enterprise
    // / government / some CDN leaf+intermediate certs).

    // Self-signed P-384 CA (ecdsa-with-SHA384). Its own key verifies its signature.
    const P384_ROOT_CERT: [u8; 471] = [
        0x30, 0x82, 0x01, 0xd3, 0x30, 0x82, 0x01, 0x58, 0xa0, 0x03, 0x02, 0x01, 0x02, 0x02, 0x14,
        0x51, 0x93, 0xf6, 0x52, 0xcb, 0x44, 0x40, 0x9c, 0xcb, 0x1e, 0x78, 0x82, 0x77, 0xaf, 0xec,
        0x1c, 0xa4, 0x7f, 0x40, 0x43, 0x30, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04,
        0x03, 0x03, 0x30, 0x20, 0x31, 0x1e, 0x30, 0x1c, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x15,
        0x52, 0x61, 0x65, 0x4e, 0x65, 0x74, 0x20, 0x50, 0x33, 0x38, 0x34, 0x20, 0x54, 0x65, 0x73,
        0x74, 0x20, 0x52, 0x6f, 0x6f, 0x74, 0x30, 0x1e, 0x17, 0x0d, 0x32, 0x35, 0x30, 0x31, 0x30,
        0x31, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a, 0x17, 0x0d, 0x33, 0x35, 0x30, 0x31, 0x30,
        0x31, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x5a, 0x30, 0x20, 0x31, 0x1e, 0x30, 0x1c, 0x06,
        0x03, 0x55, 0x04, 0x03, 0x0c, 0x15, 0x52, 0x61, 0x65, 0x4e, 0x65, 0x74, 0x20, 0x50, 0x33,
        0x38, 0x34, 0x20, 0x54, 0x65, 0x73, 0x74, 0x20, 0x52, 0x6f, 0x6f, 0x74, 0x30, 0x76, 0x30,
        0x10, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x05, 0x2b, 0x81, 0x04,
        0x00, 0x22, 0x03, 0x62, 0x00, 0x04, 0x3d, 0x5f, 0xa2, 0x75, 0xf2, 0xf3, 0xb5, 0x2b, 0xd7,
        0xc7, 0x28, 0x8e, 0x08, 0x85, 0x6d, 0x1c, 0x28, 0xa0, 0xd5, 0x64, 0x51, 0x8b, 0x05, 0x56,
        0xb1, 0x7a, 0x58, 0xc0, 0x0b, 0x7a, 0x8e, 0x30, 0x9e, 0x6d, 0x7d, 0x54, 0x66, 0x4f, 0xb4,
        0x0e, 0xd4, 0x3e, 0x13, 0xf2, 0x2a, 0x4c, 0x24, 0xad, 0x7b, 0x44, 0x0b, 0x8d, 0xf4, 0x07,
        0x83, 0x9f, 0xa2, 0xcf, 0x95, 0x70, 0x6a, 0xdb, 0x9e, 0x8c, 0x7c, 0x86, 0x35, 0x75, 0x72,
        0x62, 0x32, 0x8e, 0xf1, 0x31, 0xc0, 0x53, 0x2b, 0x56, 0x44, 0xa9, 0xc2, 0xd0, 0x05, 0x32,
        0x1e, 0xf3, 0x14, 0x35, 0x50, 0x82, 0xd8, 0x8d, 0x7d, 0x3b, 0xf9, 0xc0, 0xa3, 0x53, 0x30,
        0x51, 0x30, 0x1d, 0x06, 0x03, 0x55, 0x1d, 0x0e, 0x04, 0x16, 0x04, 0x14, 0xe6, 0x0d, 0x3d,
        0xcc, 0x06, 0x00, 0xed, 0x8b, 0x04, 0xc4, 0xc0, 0x36, 0x11, 0x99, 0x34, 0x1d, 0xcc, 0x27,
        0x63, 0xa2, 0x30, 0x1f, 0x06, 0x03, 0x55, 0x1d, 0x23, 0x04, 0x18, 0x30, 0x16, 0x80, 0x14,
        0xe6, 0x0d, 0x3d, 0xcc, 0x06, 0x00, 0xed, 0x8b, 0x04, 0xc4, 0xc0, 0x36, 0x11, 0x99, 0x34,
        0x1d, 0xcc, 0x27, 0x63, 0xa2, 0x30, 0x0f, 0x06, 0x03, 0x55, 0x1d, 0x13, 0x01, 0x01, 0xff,
        0x04, 0x05, 0x30, 0x03, 0x01, 0x01, 0xff, 0x30, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce,
        0x3d, 0x04, 0x03, 0x03, 0x03, 0x69, 0x00, 0x30, 0x66, 0x02, 0x31, 0x00, 0x89, 0xd4, 0xd4,
        0xa9, 0x8a, 0x2d, 0x83, 0x32, 0xb6, 0xb8, 0x81, 0x5b, 0x17, 0x4a, 0xc7, 0xbd, 0xc1, 0xe4,
        0xcf, 0x52, 0x30, 0xc0, 0x5c, 0xea, 0xd0, 0x63, 0xde, 0x26, 0x62, 0x03, 0x73, 0xb1, 0xd7,
        0xe0, 0xe4, 0xaa, 0x06, 0x46, 0x22, 0xb8, 0x7b, 0x43, 0x93, 0xe4, 0x27, 0x08, 0x86, 0xc4,
        0x02, 0x31, 0x00, 0xf5, 0xe2, 0x35, 0x32, 0x56, 0x7d, 0x5a, 0xa0, 0x1d, 0x58, 0x14, 0xdb,
        0xe6, 0x8b, 0x3c, 0xd4, 0xa2, 0x90, 0x0b, 0x7f, 0xf7, 0x6a, 0x98, 0x5f, 0x81, 0xdd, 0x32,
        0xcb, 0x02, 0x6e, 0xff, 0x83, 0xa4, 0xbf, 0x94, 0x40, 0x11, 0x7f, 0x9f, 0x2c, 0xb7, 0xed,
        0x75, 0xe8, 0x4c, 0x12, 0x22, 0x3d,
    ];
    // P-384 leaf, SAN=raenet-p384.example.com, signed by P384_ROOT_CERT.
    const P384_LEAF_CERT: [u8; 483] = [
        0x30, 0x82, 0x01, 0xdf, 0x30, 0x82, 0x01, 0x65, 0xa0, 0x03, 0x02, 0x01, 0x02, 0x02, 0x01,
        0x33, 0x30, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03, 0x30, 0x20,
        0x31, 0x1e, 0x30, 0x1c, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x15, 0x52, 0x61, 0x65, 0x4e,
        0x65, 0x74, 0x20, 0x50, 0x33, 0x38, 0x34, 0x20, 0x54, 0x65, 0x73, 0x74, 0x20, 0x52, 0x6f,
        0x6f, 0x74, 0x30, 0x1e, 0x17, 0x0d, 0x32, 0x35, 0x30, 0x31, 0x30, 0x31, 0x30, 0x30, 0x30,
        0x30, 0x30, 0x30, 0x5a, 0x17, 0x0d, 0x33, 0x35, 0x30, 0x31, 0x30, 0x31, 0x30, 0x30, 0x30,
        0x30, 0x30, 0x30, 0x5a, 0x30, 0x22, 0x31, 0x20, 0x30, 0x1e, 0x06, 0x03, 0x55, 0x04, 0x03,
        0x0c, 0x17, 0x72, 0x61, 0x65, 0x6e, 0x65, 0x74, 0x2d, 0x70, 0x33, 0x38, 0x34, 0x2e, 0x65,
        0x78, 0x61, 0x6d, 0x70, 0x6c, 0x65, 0x2e, 0x63, 0x6f, 0x6d, 0x30, 0x76, 0x30, 0x10, 0x06,
        0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x22,
        0x03, 0x62, 0x00, 0x04, 0x36, 0x60, 0xab, 0xc7, 0x1b, 0x95, 0x04, 0x56, 0xa7, 0xf1, 0xdd,
        0x62, 0x50, 0x4a, 0x6a, 0x9b, 0x9c, 0x72, 0xdf, 0x4b, 0xfe, 0x67, 0xb4, 0x2c, 0x49, 0x03,
        0xc5, 0x41, 0xbb, 0x28, 0xf8, 0x66, 0x79, 0x3c, 0xce, 0xb3, 0x31, 0x53, 0x5b, 0x56, 0xe0,
        0xc0, 0xfe, 0x79, 0x67, 0x42, 0x5b, 0xbe, 0x40, 0xbc, 0x46, 0xdc, 0x70, 0x25, 0x61, 0xb3,
        0x0a, 0x8e, 0xa5, 0xfa, 0x58, 0xdd, 0x3a, 0xfe, 0xae, 0x61, 0x44, 0xc5, 0x7c, 0x93, 0x60,
        0x44, 0x84, 0xf2, 0xa2, 0x84, 0x2c, 0xcf, 0x79, 0xe6, 0x19, 0x0c, 0x16, 0x7e, 0xc4, 0x38,
        0x40, 0xc7, 0x5e, 0x4e, 0x8a, 0xd3, 0x8d, 0x7a, 0x72, 0xe1, 0xa3, 0x71, 0x30, 0x6f, 0x30,
        0x22, 0x06, 0x03, 0x55, 0x1d, 0x11, 0x04, 0x1b, 0x30, 0x19, 0x82, 0x17, 0x72, 0x61, 0x65,
        0x6e, 0x65, 0x74, 0x2d, 0x70, 0x33, 0x38, 0x34, 0x2e, 0x65, 0x78, 0x61, 0x6d, 0x70, 0x6c,
        0x65, 0x2e, 0x63, 0x6f, 0x6d, 0x30, 0x09, 0x06, 0x03, 0x55, 0x1d, 0x13, 0x04, 0x02, 0x30,
        0x00, 0x30, 0x1d, 0x06, 0x03, 0x55, 0x1d, 0x0e, 0x04, 0x16, 0x04, 0x14, 0xff, 0xa6, 0xd9,
        0x54, 0xd9, 0x4f, 0xb0, 0x7c, 0x10, 0x95, 0x27, 0x1f, 0xe3, 0xf5, 0x19, 0xac, 0xdc, 0x0c,
        0x10, 0xb2, 0x30, 0x1f, 0x06, 0x03, 0x55, 0x1d, 0x23, 0x04, 0x18, 0x30, 0x16, 0x80, 0x14,
        0xe6, 0x0d, 0x3d, 0xcc, 0x06, 0x00, 0xed, 0x8b, 0x04, 0xc4, 0xc0, 0x36, 0x11, 0x99, 0x34,
        0x1d, 0xcc, 0x27, 0x63, 0xa2, 0x30, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04,
        0x03, 0x03, 0x03, 0x68, 0x00, 0x30, 0x65, 0x02, 0x31, 0x00, 0x9c, 0xdb, 0x4a, 0x5c, 0xc6,
        0x54, 0x40, 0xa8, 0x10, 0x85, 0xbd, 0xd4, 0xbe, 0x38, 0xb1, 0x7e, 0x37, 0x46, 0x68, 0x76,
        0xdb, 0x4d, 0x4f, 0xf2, 0xfb, 0x2c, 0xa1, 0xb0, 0x24, 0x91, 0x47, 0x06, 0xca, 0x9f, 0x6f,
        0x81, 0x46, 0xe4, 0xde, 0x2c, 0x18, 0x59, 0x4f, 0x66, 0xcc, 0x3c, 0x17, 0xba, 0x02, 0x30,
        0x75, 0xa0, 0x1c, 0xe0, 0x7a, 0xf2, 0xab, 0x90, 0x71, 0xba, 0xed, 0x7b, 0xc6, 0x8a, 0x48,
        0x19, 0xc2, 0x18, 0xc9, 0xd2, 0x2a, 0x66, 0x38, 0x46, 0x17, 0xf7, 0x80, 0x5d, 0xd4, 0xa7,
        0x76, 0x3e, 0x42, 0xc2, 0x36, 0x26, 0x71, 0x41, 0xd6, 0xa6, 0x9e, 0x51, 0xc8, 0x40, 0x62,
        0x5a, 0xa1, 0xbf,
    ];
    // A leaf-key ecdsa_secp384r1_sha384 signature over the §4.4.3 CertificateVerify
    // content for transcript hash `(0u8..32u8)` — the CertificateVerify (0x0503) oracle.
    const P384_CV_SIG: [u8; 103] = [
        0x30, 0x65, 0x02, 0x30, 0x4d, 0xeb, 0x28, 0x56, 0x7b, 0xbe, 0x12, 0x41, 0x53, 0xe7, 0xce,
        0x9b, 0xbe, 0x52, 0xa8, 0xb0, 0xda, 0x50, 0x03, 0xc7, 0xc4, 0x36, 0xce, 0xb5, 0xfa, 0xf3,
        0x4b, 0x59, 0x9f, 0x43, 0xaf, 0xed, 0x35, 0xd3, 0x20, 0xec, 0x0e, 0xd3, 0xb5, 0x29, 0x2d,
        0xe0, 0x2a, 0xc3, 0xf8, 0x46, 0x6a, 0xff, 0x02, 0x31, 0x00, 0x9b, 0xa3, 0xa0, 0x7e, 0x2f,
        0xd8, 0x0f, 0x3f, 0xc6, 0xff, 0xc5, 0x85, 0x39, 0x25, 0x70, 0xec, 0x83, 0x42, 0xf4, 0x7b,
        0xed, 0x51, 0x9a, 0xe6, 0x1c, 0x2d, 0x2a, 0x35, 0x3d, 0xa5, 0x1b, 0x81, 0x81, 0xe0, 0x80,
        0x82, 0x79, 0x77, 0xa1, 0xb0, 0x7d, 0xb1, 0xa5, 0x10, 0x41, 0x7f, 0xbb, 0xe6,
    ];

    #[test]
    fn cert_public_key_extracts_p384() {
        // The exact SEC1 point openssl put in the P-384 leaf cert's SPKI.
        const P384_LEAF_POINT: [u8; 97] = [
            0x04, 0x36, 0x60, 0xab, 0xc7, 0x1b, 0x95, 0x04, 0x56, 0xa7, 0xf1, 0xdd, 0x62, 0x50,
            0x4a, 0x6a, 0x9b, 0x9c, 0x72, 0xdf, 0x4b, 0xfe, 0x67, 0xb4, 0x2c, 0x49, 0x03, 0xc5,
            0x41, 0xbb, 0x28, 0xf8, 0x66, 0x79, 0x3c, 0xce, 0xb3, 0x31, 0x53, 0x5b, 0x56, 0xe0,
            0xc0, 0xfe, 0x79, 0x67, 0x42, 0x5b, 0xbe, 0x40, 0xbc, 0x46, 0xdc, 0x70, 0x25, 0x61,
            0xb3, 0x0a, 0x8e, 0xa5, 0xfa, 0x58, 0xdd, 0x3a, 0xfe, 0xae, 0x61, 0x44, 0xc5, 0x7c,
            0x93, 0x60, 0x44, 0x84, 0xf2, 0xa2, 0x84, 0x2c, 0xcf, 0x79, 0xe6, 0x19, 0x0c, 0x16,
            0x7e, 0xc4, 0x38, 0x40, 0xc7, 0x5e, 0x4e, 0x8a, 0xd3, 0x8d, 0x7a, 0x72, 0xe1,
        ];
        // The SPKI algorithm parameter is namedCurve secp384r1, so the parser must
        // route it to the P384 variant (NOT P256 — a P-256 verifier cannot consume a
        // 97-byte P-384 point, so a misroute would silently break every P-384 chain).
        match cert_public_key(&P384_LEAF_CERT) {
            Some(CertPublicKey::P384(pt)) => assert_eq!(pt, P384_LEAF_POINT.to_vec()),
            other => panic!(
                "expected a P-384 key from the real cert, got {:?}",
                other.is_some()
            ),
        }
        // The root is also P-384.
        assert!(matches!(
            cert_public_key(&P384_ROOT_CERT),
            Some(CertPublicKey::P384(_))
        ));
    }

    #[test]
    fn verify_cert_signature_p384_accepts_genuine_rejects_tampered() {
        // Issuer key = the self-signed root's own P-384 key.
        let root_key = cert_public_key(&P384_ROOT_CERT).expect("root P-384 key");
        assert!(matches!(root_key, CertPublicKey::P384(_)));

        // (a) The genuine leaf, signed by the root, verifies.
        assert_eq!(verify_cert_signature(&P384_LEAF_CERT, &root_key), Ok(()));
        // The self-signed root verifies under its own key.
        assert_eq!(verify_cert_signature(&P384_ROOT_CERT, &root_key), Ok(()));

        // (b) A single flipped bit inside the leaf's TBS must be rejected. Offset
        // 174 lands inside the SPKI EC-point coordinate bytes (the BIT STRING value
        // starts at 154 and is a fixed 0x62 bytes, so a flip here keeps the DER
        // structurally valid) — the re-encoded TBS changes and the root signature
        // no longer matches, so this must be BadSignature, not a parse error.
        let mut tampered = P384_LEAF_CERT.to_vec();
        tampered[174] ^= 0x01;
        assert_eq!(
            verify_cert_signature(&tampered, &root_key),
            Err(CertError::BadSignature),
            "a tampered P-384 cert body must NOT verify"
        );

        // (c) Presenting a P-256 issuer key for a SHA-384 signature is a type
        // mismatch → BadSignature (fail-closed, never a silent accept).
        let wrong_type = CertPublicKey::P256(vec![0x04; 65]);
        assert_eq!(
            verify_cert_signature(&P384_LEAF_CERT, &wrong_type),
            Err(CertError::BadSignature)
        );
    }

    #[test]
    fn validate_chain_p384_reaches_leaf_key() {
        // Trust store holds the P-384 root; the leaf chains to it. validate_chain
        // must anchor the leaf via the root's P-384 signature and hand back the
        // leaf's P-384 key. now=2027-ish (both certs valid 2025..2035).
        let mut store = TrustStore::new();
        assert!(store.add_root_der(&P384_ROOT_CERT));
        let leaf_key = validate_chain(&[P384_LEAF_CERT.to_vec()], &store, Some(1_800_000_000))
            .expect("P-384 leaf must anchor to the P-384 root");
        assert!(matches!(leaf_key, CertPublicKey::P384(_)));

        // Negative control: an empty trust store cannot anchor the P-384 leaf.
        let empty = TrustStore::new();
        assert!(matches!(
            validate_chain(&[P384_LEAF_CERT.to_vec()], &empty, Some(1_800_000_000)),
            Err(CertError::UntrustedRoot)
        ));
    }

    #[test]
    fn verify_certificate_verify_p384_scheme_0x0503() {
        // The leaf's verified P-384 key from the cert.
        let leaf_key = cert_public_key(&P384_LEAF_CERT).expect("leaf P-384 key");
        // The fixed transcript hash the P384_CV_SIG oracle was signed over.
        let transcript_hash: Vec<u8> = (0u8..32).collect();

        // (a) Genuine ecdsa_secp384r1_sha384 (0x0503) signature verifies.
        assert_eq!(
            verify_certificate_verify(&leaf_key, 0x0503, &P384_CV_SIG, &transcript_hash),
            Ok(())
        );

        // (b) A tampered transcript hash must be rejected (the whole point of
        // CertificateVerify — the signature is bound to the exact transcript).
        let mut bad_th = transcript_hash.clone();
        bad_th[0] ^= 0xFF;
        assert_eq!(
            verify_certificate_verify(&leaf_key, 0x0503, &P384_CV_SIG, &bad_th),
            Err(CertError::BadCertificateVerify)
        );

        // (c) A flipped signature byte must be rejected.
        let mut bad_sig = P384_CV_SIG.to_vec();
        bad_sig[60] ^= 0x01;
        assert_eq!(
            verify_certificate_verify(&leaf_key, 0x0503, &bad_sig, &transcript_hash),
            Err(CertError::BadCertificateVerify)
        );

        // (d) Advertising 0x0503 but presenting a P-256 key → mismatch, fail-closed.
        let p256_key = CertPublicKey::P256(vec![0x04; 65]);
        assert_eq!(
            verify_certificate_verify(&p256_key, 0x0503, &P384_CV_SIG, &transcript_hash),
            Err(CertError::BadCertificateVerify)
        );
    }

    #[test]
    fn p384_verify_is_fail_closed_on_garbage() {
        // Empty / malformed point / signature must all return false, never panic.
        assert!(!p384_verify(&[], b"msg", &P384_CV_SIG));
        assert!(!p384_verify(&[0x04, 0x00], b"msg", &P384_CV_SIG));
        let leaf_pt = match cert_public_key(&P384_LEAF_CERT) {
            Some(CertPublicKey::P384(pt)) => pt,
            _ => unreachable!(),
        };
        assert!(!p384_verify(&leaf_pt, b"msg", &[0x30, 0x00]));
    }

    #[test]
    fn client_hello_advertises_p384_signature_scheme() {
        // The ClientHello signature_algorithms list must include
        // ecdsa_secp384r1_sha384 (0x0503) so servers may offer P-384 certs, while
        // keeping ecdsa_secp256r1_sha256 (0x0403).
        let ch = build_client_hello(
            &[0u8; 32],
            &[],
            &[CipherSuite::Aes128GcmSha256],
            &[1u8; 32],
            "p384.example.com",
        );
        // sig_algs are two adjacent bytes; scan for the 0x0503 and 0x0403 pairs.
        let has_pair = |a: u8, b: u8| ch.windows(2).any(|w| w[0] == a && w[1] == b);
        assert!(has_pair(0x05, 0x03), "ClientHello must advertise 0x0503");
        assert!(
            has_pair(0x04, 0x03),
            "ClientHello must still advertise 0x0403"
        );
    }
}
