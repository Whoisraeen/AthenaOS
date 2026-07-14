//! DNS resolver — query building, response parsing, caching, and resolution.
//!
//! Supports A, AAAA, CNAME, MX, TXT, NS, PTR, SRV record types.
//! Maintains a TTL-aware cache with LRU eviction and negative caching
//! (NXDOMAIN). Queries are built as RFC 1035 wire-format packets for
//! transmission over UDP port 53.
//!
//! Also includes:
//!   - Static host overrides (`/etc/hosts` equivalent)
//!   - Multiple upstream servers with failover
//!   - DNS-over-HTTPS query construction stub
//!   - DNSSEC validation: single-zone chain-of-trust against a configured trust
//!     anchor (RFC 4035 §5.3) over RRs the wire parser retains, PLUS the
//!     recursive multi-zone delegation walk (root → TLD → owner zone) behind an
//!     injectable record fetcher; only the live-UDP `DnssecFetcher` impl remains.

#![allow(dead_code)]

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

// ── DNS Record Types ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum DnsRecord {
    A([u8; 4]),
    Aaaa([u8; 16]),
    Cname(String),
    Mx {
        priority: u16,
        exchange: String,
    },
    Txt(String),
    Ns(String),
    Ptr(String),
    Srv {
        priority: u16,
        weight: u16,
        port: u16,
        target: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum DnsQueryType {
    A = 1,
    Ns = 2,
    Cname = 5,
    Ptr = 12,
    Mx = 15,
    Txt = 16,
    Aaaa = 28,
    Srv = 33,
    Any = 255,
}

impl DnsQueryType {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            1 => Some(Self::A),
            2 => Some(Self::Ns),
            5 => Some(Self::Cname),
            12 => Some(Self::Ptr),
            15 => Some(Self::Mx),
            16 => Some(Self::Txt),
            28 => Some(Self::Aaaa),
            33 => Some(Self::Srv),
            255 => Some(Self::Any),
            _ => None,
        }
    }
}

// ── DNS Response Codes ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum DnsResponseCode {
    NoError = 0,
    FormError = 1,
    ServFail = 2,
    NxDomain = 3,
    NotImpl = 4,
    Refused = 5,
}

impl DnsResponseCode {
    pub fn from_u16(v: u16) -> Self {
        match v {
            0 => Self::NoError,
            1 => Self::FormError,
            2 => Self::ServFail,
            3 => Self::NxDomain,
            4 => Self::NotImpl,
            _ => Self::Refused,
        }
    }
}

// ── DNS Header ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DnsHeader {
    pub id: u16,
    pub flags: u16,
    pub qd_count: u16,
    pub an_count: u16,
    pub ns_count: u16,
    pub ar_count: u16,
}

impl DnsHeader {
    pub fn is_response(&self) -> bool {
        (self.flags & 0x8000) != 0
    }

    pub fn is_authoritative(&self) -> bool {
        (self.flags & 0x0400) != 0
    }

    pub fn is_truncated(&self) -> bool {
        (self.flags & 0x0200) != 0
    }

    pub fn recursion_available(&self) -> bool {
        (self.flags & 0x0080) != 0
    }

    pub fn rcode(&self) -> DnsResponseCode {
        DnsResponseCode::from_u16(self.flags & 0x000F)
    }

    pub fn serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.id.to_be_bytes());
        buf.extend_from_slice(&self.flags.to_be_bytes());
        buf.extend_from_slice(&self.qd_count.to_be_bytes());
        buf.extend_from_slice(&self.an_count.to_be_bytes());
        buf.extend_from_slice(&self.ns_count.to_be_bytes());
        buf.extend_from_slice(&self.ar_count.to_be_bytes());
    }

    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 12 {
            return None;
        }
        Some(Self {
            id: u16::from_be_bytes([data[0], data[1]]),
            flags: u16::from_be_bytes([data[2], data[3]]),
            qd_count: u16::from_be_bytes([data[4], data[5]]),
            an_count: u16::from_be_bytes([data[6], data[7]]),
            ns_count: u16::from_be_bytes([data[8], data[9]]),
            ar_count: u16::from_be_bytes([data[10], data[11]]),
        })
    }
}

// ── DNS Question ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DnsQuestion {
    pub name: String,
    pub qtype: u16,
    pub qclass: u16,
}

// ── DNS Resource Record ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DnsResourceRecord {
    pub name: String,
    pub rtype: u16,
    pub rclass: u16,
    pub ttl: u32,
    pub rdata: Vec<u8>,
}

// ── DNS Query ───────────────────────────────────────────────────────────────

pub struct DnsQuery {
    pub id: u16,
    pub name: String,
    pub qtype: DnsQueryType,
    pub qclass: u16,
}

// ── DNS Response ────────────────────────────────────────────────────────────

pub struct DnsResponse {
    pub id: u16,
    pub authoritative: bool,
    pub truncated: bool,
    pub recursion_available: bool,
    pub rcode: DnsResponseCode,
    pub answers: Vec<DnsRecord>,
    pub answer_ttls: Vec<u32>,
    pub authority: Vec<DnsRecord>,
    pub additional: Vec<DnsRecord>,
    pub query_name: String,
    pub query_type: DnsQueryType,
    /// Raw DNSSEC RRs retained verbatim from the wire (across ALL sections) so
    /// [`DnsResolver::validate_dnssec`] can feed [`validate_chain_of_trust`].
    /// Each entry is `(lowercased owner name, parsed struct)`. Malformed RRs are
    /// silently skipped by the parser (hostile-input-safe), never retained.
    pub rrsigs: Vec<(String, Rrsig)>,
    pub dnskeys: Vec<(String, Dnskey)>,
    pub dss: Vec<(String, Ds)>,
    /// Answer-section RRs in canonical form `(lowercased owner, CanonicalRr)` —
    /// the raw RDATA bytes an RRSIG signs over. Used to assemble the covered
    /// RRset for validation. (Names embedded in RDATA are canonical only for
    /// types without embedded names — A/AAAA/TXT — which is this slice's scope.)
    pub answer_canonical: Vec<(String, CanonicalRr)>,
}

// ── DNS Cache Entry ─────────────────────────────────────────────────────────

pub struct DnsCacheEntry {
    pub records: Vec<DnsRecord>,
    pub expires_at: u64,
    pub inserted_at: u64,
    pub is_negative: bool,
}

// ── Static Hosts ────────────────────────────────────────────────────────────

/// `/etc/hosts`-style static override: hostname → IPv4 address.
pub struct StaticHostEntry {
    pub hostname: String,
    pub address: [u8; 4],
}

// ── DNSSEC Stub Types ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DnssecAlgorithm {
    RsaSha256 = 8,
    RsaSha512 = 10,
    EcdsaP256Sha256 = 13,
    EcdsaP384Sha384 = 14,
    Ed25519 = 15,
}

impl DnssecAlgorithm {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            8 => Some(Self::RsaSha256),
            10 => Some(Self::RsaSha512),
            13 => Some(Self::EcdsaP256Sha256),
            14 => Some(Self::EcdsaP384Sha384),
            15 => Some(Self::Ed25519),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum DnssecRrType {
    Dnskey = 48,
    Rrsig = 46,
    Nsec = 47,
    Ds = 43,
    Nsec3 = 50,
}

/// A DNSSEC validation result (RFC 4035 §5): `Secure` = a valid signature by a
/// trusted key covers the data; `Bogus` = a signature is present but does not
/// verify (tampered / wrong key / expired); `Insecure` = a proven-unsigned
/// delegation; `Indeterminate` = no path to a trust anchor, or an algorithm we
/// do not verify yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnssecStatus {
    Secure,
    Insecure,
    Bogus,
    Indeterminate,
}

/// One resource record in the canonical form DNSSEC signs over (RFC 4034 §6.2).
/// `rdata` must ALREADY be canonical — for RDATA that embeds domain names (MX,
/// NS, CNAME, SOA, …) the response parser is responsible for decompressing and
/// lowercasing those names before verification. The owner name and the RRSIG's
/// original TTL are supplied alongside the RRset, not per-RR.
#[derive(Clone)]
pub struct CanonicalRr {
    pub rtype: u16,
    pub class: u16,
    pub rdata: Vec<u8>,
}

/// Parsed RRSIG RDATA (RFC 4034 §3.1) with the signature split out for verify.
#[derive(Clone)]
pub struct Rrsig {
    pub type_covered: u16,
    pub algorithm: u8,
    pub labels: u8,
    pub original_ttl: u32,
    pub sig_expiration: u32,
    pub sig_inception: u32,
    pub key_tag: u16,
    /// The signer's zone name, e.g. `"example.com"` (canonicalized internally).
    pub signer_name: String,
    pub signature: Vec<u8>,
}

/// Parsed DNSKEY RDATA (RFC 4034 §2.1).
#[derive(Clone)]
pub struct Dnskey {
    pub flags: u16,
    pub protocol: u8,
    pub algorithm: u8,
    pub public_key: Vec<u8>,
}

/// Encode a domain name into canonical wire form (RFC 4034 §6.2): each label
/// length-prefixed, ASCII-lowercased, terminated by the root label (0x00). A
/// trailing dot is optional; an empty string is the root.
pub fn encode_name_canonical(name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for label in name.trim_end_matches('.').split('.') {
        if label.is_empty() {
            continue;
        }
        out.push(label.len() as u8);
        for &b in label.as_bytes() {
            out.push(b.to_ascii_lowercase());
        }
    }
    out.push(0);
    out
}

/// Build the exact byte string an RRSIG signs (RFC 4034 §3.1.8.1):
///   `RRSIG_RDATA`(without the signature) ‖ for each RR, sorted by canonical
///   RDATA (§6.3): `owner ‖ type ‖ class ‖ original_ttl ‖ rdlength ‖ rdata`.
/// The RR TTL used is the RRSIG's `original_ttl`, NOT each RR's live TTL.
fn dnssec_signed_data(rrsig: &Rrsig, owner: &str, rrset: &[CanonicalRr]) -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(&rrsig.type_covered.to_be_bytes());
    d.push(rrsig.algorithm);
    d.push(rrsig.labels);
    d.extend_from_slice(&rrsig.original_ttl.to_be_bytes());
    d.extend_from_slice(&rrsig.sig_expiration.to_be_bytes());
    d.extend_from_slice(&rrsig.sig_inception.to_be_bytes());
    d.extend_from_slice(&rrsig.key_tag.to_be_bytes());
    d.extend_from_slice(&encode_name_canonical(&rrsig.signer_name));

    let owner_wire = encode_name_canonical(owner);
    let mut rrs: Vec<&CanonicalRr> = rrset.iter().collect();
    rrs.sort_by(|a, b| a.rdata.cmp(&b.rdata));
    for rr in rrs {
        d.extend_from_slice(&owner_wire);
        d.extend_from_slice(&rr.rtype.to_be_bytes());
        d.extend_from_slice(&rr.class.to_be_bytes());
        d.extend_from_slice(&rrsig.original_ttl.to_be_bytes());
        d.extend_from_slice(&(rr.rdata.len() as u16).to_be_bytes());
        d.extend_from_slice(&rr.rdata);
    }
    d
}

/// Cryptographically verify an RRSIG over an RRset with a DNSKEY (RFC 4035
/// §5.3): reconstruct the signed bytes and check the signature. Returns
/// `Secure` on a valid signature, `Bogus` on an invalid one (tampered / wrong
/// key / algorithm mismatch), and `Indeterminate` for an algorithm this build
/// does not verify yet. This is the cryptographic core; establishing that the
/// DNSKEY is itself trusted (the DS/DNSKEY chain to a configured trust anchor)
/// is the layer above.
pub fn verify_rrsig(
    rrsig: &Rrsig,
    dnskey: &Dnskey,
    owner: &str,
    rrset: &[CanonicalRr],
) -> DnssecStatus {
    if rrsig.algorithm != dnskey.algorithm {
        return DnssecStatus::Bogus;
    }
    let data = dnssec_signed_data(rrsig, owner, rrset);
    match DnssecAlgorithm::from_u8(rrsig.algorithm) {
        Some(DnssecAlgorithm::Ed25519) => {
            // RFC 8080: Ed25519 (PureEdDSA) signs the signed-data bytes directly.
            let pk: [u8; 32] = match dnskey.public_key.as_slice().try_into() {
                Ok(k) => k,
                Err(_) => return DnssecStatus::Bogus,
            };
            let sig: [u8; 64] = match rrsig.signature.as_slice().try_into() {
                Ok(s) => s,
                Err(_) => return DnssecStatus::Bogus,
            };
            if ath_crypto::ed25519::verify(&pk, &data, &sig) {
                DnssecStatus::Secure
            } else {
                DnssecStatus::Bogus
            }
        }
        Some(DnssecAlgorithm::EcdsaP256Sha256) => {
            // RFC 6605: the DNSKEY public key is the raw uncompressed point
            // x‖y (64 bytes, no 0x04 prefix) and the signature is r‖s (64
            // bytes). The signed data is SHA-256-hashed then ECDSA-P256
            // verified — `p256_ecdsa::verify` does the SHA-256 internally and
            // accepts both the raw 64-byte key and the raw 64-byte signature.
            if ath_crypto::p256_ecdsa::verify(&dnskey.public_key, &data, &rrsig.signature) {
                DnssecStatus::Secure
            } else {
                DnssecStatus::Bogus
            }
        }
        Some(DnssecAlgorithm::RsaSha256) => {
            // RFC 5702 (RSASHA256) + RFC 3110: the DNSKEY public key is
            // `exponent-length ‖ exponent ‖ modulus`. If the first byte is 0,
            // the following two bytes are the exponent length (big-endian);
            // otherwise the first byte itself is the exponent length. The signed
            // data is verified with RSASSA-PKCS1-v1_5 over SHA-256 (RFC 8017).
            let pk = dnskey.public_key.as_slice();
            let (exp_len, off) = if pk.is_empty() {
                return DnssecStatus::Bogus;
            } else if pk[0] == 0 {
                if pk.len() < 3 {
                    return DnssecStatus::Bogus;
                }
                (((pk[1] as usize) << 8) | pk[2] as usize, 3usize)
            } else {
                (pk[0] as usize, 1usize)
            };
            // Need at least one modulus byte after the exponent (off + exp_len
            // must leave a non-empty tail). Malformed ⇒ Bogus (fail-closed).
            if exp_len == 0 || off + exp_len >= pk.len() {
                return DnssecStatus::Bogus;
            }
            let exponent = &pk[off..off + exp_len];
            let modulus = &pk[off + exp_len..];
            if crate::crypto::rsa_pkcs1_sha256_verify(modulus, exponent, &data, &rrsig.signature) {
                DnssecStatus::Secure
            } else {
                DnssecStatus::Bogus
            }
        }
        Some(DnssecAlgorithm::RsaSha512) => {
            // RFC 5702 (RSASHA512) + RFC 3110: identical DNSKEY wire format to
            // RSASHA256 above (`exponent-length ‖ exponent ‖ modulus`); only the
            // signature hash differs. The signed data is verified with
            // RSASSA-PKCS1-v1_5 over SHA-512 (RFC 8017).
            let pk = dnskey.public_key.as_slice();
            let (exp_len, off) = if pk.is_empty() {
                return DnssecStatus::Bogus;
            } else if pk[0] == 0 {
                if pk.len() < 3 {
                    return DnssecStatus::Bogus;
                }
                (((pk[1] as usize) << 8) | pk[2] as usize, 3usize)
            } else {
                (pk[0] as usize, 1usize)
            };
            // Need at least one modulus byte after the exponent (off + exp_len
            // must leave a non-empty tail). Malformed ⇒ Bogus (fail-closed).
            if exp_len == 0 || off + exp_len >= pk.len() {
                return DnssecStatus::Bogus;
            }
            let exponent = &pk[off..off + exp_len];
            let modulus = &pk[off + exp_len..];
            if crate::crypto::rsa_pkcs1_sha512_verify(modulus, exponent, &data, &rrsig.signature) {
                DnssecStatus::Secure
            } else {
                DnssecStatus::Bogus
            }
        }
        Some(DnssecAlgorithm::EcdsaP384Sha384) => {
            // RFC 6605: the alg-14 DNSKEY public key is the raw uncompressed
            // point x‖y (96 bytes, no 0x04 prefix) and the RRSIG signature is
            // r‖s (96 bytes). The signed data is SHA-384-hashed then
            // ECDSA-P384 verified — `p384_ecdsa::verify` does the SHA-384
            // internally and accepts both the raw 96-byte key and the raw
            // 96-byte signature. This closes DNSSEC to algorithm-complete.
            if ath_crypto::p384_ecdsa::verify(&dnskey.public_key, &data, &rrsig.signature) {
                DnssecStatus::Secure
            } else {
                DnssecStatus::Bogus
            }
        }
        // A genuinely-unknown algorithm number: fail SAFE as Indeterminate
        // (never Secure) rather than claim a verification we did not perform.
        _ => DnssecStatus::Indeterminate,
    }
}

/// Serialize DNSKEY RDATA (RFC 4034 §2.1): flags ‖ protocol ‖ algorithm ‖ key.
fn dnskey_rdata(dnskey: &Dnskey) -> Vec<u8> {
    let mut r = Vec::with_capacity(4 + dnskey.public_key.len());
    r.extend_from_slice(&dnskey.flags.to_be_bytes());
    r.push(dnskey.protocol);
    r.push(dnskey.algorithm);
    r.extend_from_slice(&dnskey.public_key);
    r
}

/// Compute a DNSKEY's key tag (RFC 4034 Appendix B) — the 16-bit checksum that
/// links an RRSIG (its `key_tag` field) and a DS record to the DNSKEY that
/// signed / is delegated to. Defined for every algorithm except the historic
/// alg 1 (RSA/MD5), which AthenaOS does not support.
pub fn dnskey_key_tag(dnskey: &Dnskey) -> u16 {
    let rdata = dnskey_rdata(dnskey);
    let mut ac: u32 = 0;
    for (i, &b) in rdata.iter().enumerate() {
        ac += if i & 1 == 1 {
            b as u32
        } else {
            (b as u32) << 8
        };
    }
    ac += (ac >> 16) & 0xFFFF;
    (ac & 0xFFFF) as u16
}

/// A DS (Delegation Signer) record — the PARENT zone's commitment to a child
/// zone's DNSKEY (RFC 4034 §5.1). `digest_type` 2 = SHA-256 (RFC 4509).
#[derive(Clone)]
pub struct Ds {
    pub key_tag: u16,
    pub algorithm: u8,
    pub digest_type: u8,
    pub digest: Vec<u8>,
}

/// Verify a DS record correctly commits to `dnskey` at `owner` (RFC 4509): the
/// DS key tag + algorithm must match the DNSKEY, and the DS digest must equal
/// `SHA-256(canonical owner ‖ DNSKEY RDATA)`. This is the parent→child link of
/// the DNSSEC chain of trust (a secure delegation): a validator anchored at the
/// root follows verified DS→DNSKEY hops down to the answer's zone. Only digest
/// type 2 (SHA-256) is accepted; anything else returns `false` (never a false
/// accept).
pub fn verify_ds(ds: &Ds, dnskey: &Dnskey, owner: &str) -> bool {
    if ds.key_tag != dnskey_key_tag(dnskey) || ds.algorithm != dnskey.algorithm {
        return false;
    }
    if ds.digest_type != 2 || ds.digest.len() != 32 {
        return false;
    }
    let mut input = encode_name_canonical(owner);
    input.extend_from_slice(&dnskey_rdata(dnskey));
    let computed = ath_crypto::sha256::sha256(&input);
    crate::crypto::ct_eq(&computed, &ds.digest)
}

/// Read an UNcompressed domain name from `data[*off..]` (RFC 4034 §3.1.7: names
/// in DNSSEC RDATA MUST NOT be compressed). Advances `*off` past the root label
/// and returns the lowercased dotted name. Returns `None` on truncated input or
/// a compression/reserved label (`len & 0xC0`), which must not appear here — the
/// bounds checks make this safe on hostile wire data.
fn read_uncompressed_name(data: &[u8], off: &mut usize) -> Option<String> {
    let mut name = String::new();
    loop {
        let len = *data.get(*off)? as usize;
        *off += 1;
        if len == 0 {
            break;
        }
        if len & 0xC0 != 0 {
            return None;
        }
        let end = off.checked_add(len)?;
        let label = data.get(*off..end)?;
        if !name.is_empty() {
            name.push('.');
        }
        for &b in label {
            name.push(b.to_ascii_lowercase() as char);
        }
        *off = end;
    }
    Some(name)
}

/// Parse RRSIG RDATA (RFC 4034 §3.1) from untrusted wire bytes into an [`Rrsig`].
/// Every field is bounds-checked; a truncated or malformed record yields `None`
/// rather than a panic. The signer name is read uncompressed and the remaining
/// bytes are the signature.
pub fn parse_rrsig_rdata(rdata: &[u8]) -> Option<Rrsig> {
    if rdata.len() < 18 {
        return None;
    }
    let type_covered = u16::from_be_bytes([rdata[0], rdata[1]]);
    let algorithm = rdata[2];
    let labels = rdata[3];
    let original_ttl = u32::from_be_bytes([rdata[4], rdata[5], rdata[6], rdata[7]]);
    let sig_expiration = u32::from_be_bytes([rdata[8], rdata[9], rdata[10], rdata[11]]);
    let sig_inception = u32::from_be_bytes([rdata[12], rdata[13], rdata[14], rdata[15]]);
    let key_tag = u16::from_be_bytes([rdata[16], rdata[17]]);
    let mut off = 18usize;
    let signer_name = read_uncompressed_name(rdata, &mut off)?;
    let signature = rdata.get(off..)?.to_vec();
    if signature.is_empty() {
        return None;
    }
    Some(Rrsig {
        type_covered,
        algorithm,
        labels,
        original_ttl,
        sig_expiration,
        sig_inception,
        key_tag,
        signer_name,
        signature,
    })
}

/// Parse DNSKEY RDATA (RFC 4034 §2.1) from wire bytes: flags ‖ protocol ‖
/// algorithm ‖ public key. `None` if shorter than the 4-byte header + 1 key byte.
pub fn parse_dnskey_rdata(rdata: &[u8]) -> Option<Dnskey> {
    if rdata.len() < 5 {
        return None;
    }
    Some(Dnskey {
        flags: u16::from_be_bytes([rdata[0], rdata[1]]),
        protocol: rdata[2],
        algorithm: rdata[3],
        public_key: rdata[4..].to_vec(),
    })
}

/// Parse DS RDATA (RFC 4034 §5.1) from wire bytes: key tag ‖ algorithm ‖ digest
/// type ‖ digest. `None` if shorter than the 4-byte header + 1 digest byte.
pub fn parse_ds_rdata(rdata: &[u8]) -> Option<Ds> {
    if rdata.len() < 5 {
        return None;
    }
    Some(Ds {
        key_tag: u16::from_be_bytes([rdata[0], rdata[1]]),
        algorithm: rdata[2],
        digest_type: rdata[3],
        digest: rdata[4..].to_vec(),
    })
}

/// Second-pass wire scan that retains the raw DNSSEC RRs a validator needs,
/// WITHOUT disturbing the primary decoded-record path. Walks the header,
/// questions, then the answer / authority / additional sections, pulling out
/// RRSIG (type 46), DNSKEY (48) and DS (43) RRs (parsed via the bounds-checked
/// `parse_*_rdata` helpers) plus every answer-section RR's canonical RDATA.
///
/// This parses ATTACKER-CONTROLLED wire bytes: every field is bounds-checked and
/// a malformed record is SKIPPED (the RR is dropped, the scan stops at the first
/// unrecoverable framing error) — it never panics and never fabricates an RR.
/// Owner names are lowercased for case-insensitive matching (RFC 4034 §6.2).
/// Returns whatever was gathered before the first framing error, so a truncated
/// tail still yields the well-formed RRs that preceded it.
fn scan_dnssec_rrs(
    data: &[u8],
) -> (
    Vec<(String, Rrsig)>,
    Vec<(String, Dnskey)>,
    Vec<(String, Ds)>,
    Vec<(String, CanonicalRr)>,
) {
    let mut rrsigs: Vec<(String, Rrsig)> = Vec::new();
    let mut dnskeys: Vec<(String, Dnskey)> = Vec::new();
    let mut dss: Vec<(String, Ds)> = Vec::new();
    let mut answers: Vec<(String, CanonicalRr)> = Vec::new();

    let header = match DnsHeader::parse(data) {
        Some(h) => h,
        None => return (rrsigs, dnskeys, dss, answers),
    };
    let mut off = 12usize;

    // Skip the question section (names may be compressed → use decode_name).
    for _ in 0..header.qd_count {
        if decode_name(data, &mut off).is_none() {
            return (rrsigs, dnskeys, dss, answers);
        }
        match off.checked_add(4) {
            Some(n) if n <= data.len() => off = n,
            _ => return (rrsigs, dnskeys, dss, answers),
        }
    }

    // Resource-record sections. Only the answer section contributes to the
    // covered RRset; RRSIG/DNSKEY/DS are harvested from every section.
    let sections = [
        (header.an_count, true),
        (header.ns_count, false),
        (header.ar_count, false),
    ];
    for (count, is_answer) in sections {
        for _ in 0..count {
            let name = match decode_name(data, &mut off) {
                Some(n) => n,
                None => return (rrsigs, dnskeys, dss, answers),
            };
            // Fixed RR header: type(2) class(2) ttl(4) rdlength(2) = 10 bytes.
            if off + 10 > data.len() {
                return (rrsigs, dnskeys, dss, answers);
            }
            let rtype = u16::from_be_bytes([data[off], data[off + 1]]);
            let rclass = u16::from_be_bytes([data[off + 2], data[off + 3]]);
            let rdlength = u16::from_be_bytes([data[off + 8], data[off + 9]]) as usize;
            off += 10;
            let rd_end = match off.checked_add(rdlength) {
                Some(e) if e <= data.len() => e,
                _ => return (rrsigs, dnskeys, dss, answers),
            };
            let rdata = &data[off..rd_end];
            let owner = name.to_ascii_lowercase();
            match rtype {
                46 => {
                    // Skip-not-fail: a malformed RRSIG RDATA yields None.
                    if let Some(rs) = parse_rrsig_rdata(rdata) {
                        rrsigs.push((owner, rs));
                    }
                }
                48 => {
                    if let Some(dk) = parse_dnskey_rdata(rdata) {
                        dnskeys.push((owner, dk));
                    }
                }
                43 => {
                    if let Some(ds) = parse_ds_rdata(rdata) {
                        dss.push((owner, ds));
                    }
                }
                _ => {
                    if is_answer {
                        answers.push((
                            owner,
                            CanonicalRr {
                                rtype,
                                class: rclass,
                                rdata: rdata.to_vec(),
                            },
                        ));
                    }
                }
            }
            off = rd_end;
        }
    }

    (rrsigs, dnskeys, dss, answers)
}

/// True iff `now` falls within an RRSIG's validity window using RFC 1982 serial
/// arithmetic (RFC 4034 §3.1.5): `inception ≤ now ≤ expiration` measured on the
/// 2^32 circle, so the timestamps keep working past the 2038 wrap. An RRSIG
/// outside its window is treated as a forgery (never Secure).
fn rrsig_time_valid(rrsig: &Rrsig, now: u32) -> bool {
    // `a <= b` in serial arithmetic ⇔ (b - a) mod 2^32 < 2^31.
    let after_inception = now.wrapping_sub(rrsig.sig_inception) < 0x8000_0000;
    let before_expiration = rrsig.sig_expiration.wrapping_sub(now) < 0x8000_0000;
    after_inception && before_expiration
}

/// Validate a single zone's DNSSEC chain of trust from a configured trust anchor
/// (RFC 4035 §5.3). Given the anchor `DS` (the parent's commitment to this
/// zone's Key-Signing Key), the zone's DNSKEY RRset plus the RRSIG the KSK made
/// over it, and the answer RRset plus its RRSIG, this walks the one-hop chain:
///
/// ```text
///   anchor DS ──confirms──▶ KSK ──signs──▶ DNSKEY RRset ──contains──▶ ZSK
///   ZSK ──signs──▶ answer RRset  ⇒  Secure
/// ```
///
/// Fail-closed (RFC 4035 §5.5): `Secure` is returned ONLY when every link
/// verifies cryptographically and every RRSIG is inside its validity window.
/// `Bogus` = a link is present but tampered/expired (an active forgery);
/// `Indeterminate` = there is no path to the anchor at all, or an algorithm this
/// build does not verify yet (an honest "cannot decide", never a false accept).
///
/// The `owner` name is both the zone apex (owner of the DNSKEY RRset) and the
/// owner of the answer RRset — this validates data at the zone apex. It is the
/// single-hop, apex special case of [`validate_with_delegation`], which walks
/// the full root→owner-zone chain via an injectable [`DnssecFetcher`].
/// MasterChecklist Phase 10.
pub fn validate_chain_of_trust(
    anchor: &Ds,
    zone_dnskeys: &[Dnskey],
    dnskey_rrsig: &Rrsig,
    owner: &str,
    answer_rrset: &[CanonicalRr],
    answer_rrsig: &Rrsig,
    now_unix: u32,
) -> DnssecStatus {
    // A single-zone chain is a one-hop delegation whose answer sits at the zone
    // apex: exactly one trusted anchor DS, and the answer owner equals the zone.
    validate_hop(
        core::slice::from_ref(anchor),
        owner,
        zone_dnskeys,
        dnskey_rrsig,
        owner,
        answer_rrset,
        answer_rrsig,
        now_unix,
    )
}

/// Serialize DS RDATA (RFC 4034 §5.1) to canonical wire bytes: key tag ‖
/// algorithm ‖ digest type ‖ digest. The inverse of [`parse_ds_rdata`]; used to
/// assemble a child's DS RRset into the [`CanonicalRr`] form an RRSIG signs over
/// during a delegation walk.
fn ds_rdata(ds: &Ds) -> Vec<u8> {
    let mut r = Vec::with_capacity(4 + ds.digest.len());
    r.extend_from_slice(&ds.key_tag.to_be_bytes());
    r.push(ds.algorithm);
    r.push(ds.digest_type);
    r.extend_from_slice(&ds.digest);
    r
}

/// Verify ONE hop of a DNSSEC chain of trust (RFC 4035 §5.3), generalized so the
/// signed RRset need NOT sit at the zone apex. Given the already-trusted anchor
/// DS set for `zone`, this zone's DNSKEY RRset plus the RRSIG its KSK made over
/// that RRset, and an arbitrary RRset (owned by `answer_owner`, served by this
/// zone) plus its RRSIG, it walks:
///
/// ```text
///   trusted DS ─confirms→ KSK ─signs→ DNSKEY RRset ─contains→ ZSK
///   ZSK ─signs→ answer RRset (@ answer_owner)  ⇒  Secure
/// ```
///
/// `answer_owner` may differ from `zone`: for an intermediate delegation the
/// "answer" is the child zone's DS RRset (owned by the child, signed by this
/// zone's ZSK); for a leaf it is a record below the apex (e.g. `www.example.com`
/// in zone `example.com`). [`validate_chain_of_trust`] is the special case
/// `answer_owner == zone` with a single anchor.
///
/// Fail-closed (RFC 4035 §5.5): `Secure` ONLY when a trusted DS confirms a KSK,
/// the KSK's RRSIG covers the in-window DNSKEY RRset, and a ZSK's in-window RRSIG
/// covers the answer. `Bogus` = a link is present but tampered/expired (a
/// forgery); `Indeterminate` = no anchor reaches a KSK here, or an algorithm this
/// build does not verify.
fn validate_hop(
    anchors: &[Ds],
    zone: &str,
    zone_dnskeys: &[Dnskey],
    dnskey_rrsig: &Rrsig,
    answer_owner: &str,
    answer_rrset: &[CanonicalRr],
    answer_rrsig: &Rrsig,
    now_unix: u32,
) -> DnssecStatus {
    // (a) Some trusted anchor DS must confirm a KSK in this zone's DNSKEY RRset.
    //     A tag match whose SHA-256 digest fails is a tamper (Bogus); no tag
    //     match at all is simply no path to the anchor (Indeterminate).
    let mut ksk: Option<&Dnskey> = None;
    let mut tag_matched_but_bad = false;
    'outer: for anchor in anchors {
        for key in zone_dnskeys {
            if dnskey_key_tag(key) == anchor.key_tag {
                if verify_ds(anchor, key, zone) {
                    ksk = Some(key);
                    break 'outer;
                }
                tag_matched_but_bad = true;
            }
        }
    }
    let ksk = match ksk {
        Some(k) => k,
        None => {
            return if tag_matched_but_bad {
                DnssecStatus::Bogus
            } else {
                DnssecStatus::Indeterminate
            };
        }
    };

    // (b) The confirmed KSK must have signed this zone's whole DNSKEY RRset with
    //     an in-window RRSIG (a DNSKEY RRset is always owned by the apex `zone`).
    //     Reject an expired / not-yet-valid RRSIG before spending a verify.
    if !rrsig_time_valid(dnskey_rrsig, now_unix) {
        return DnssecStatus::Bogus;
    }
    let dnskey_canonical: Vec<CanonicalRr> = zone_dnskeys
        .iter()
        .map(|k| CanonicalRr {
            rtype: DnssecRrType::Dnskey as u16,
            class: 1,
            rdata: dnskey_rdata(k),
        })
        .collect();
    match verify_rrsig(dnskey_rrsig, ksk, zone, &dnskey_canonical) {
        DnssecStatus::Secure => {}
        DnssecStatus::Indeterminate => return DnssecStatus::Indeterminate,
        _ => return DnssecStatus::Bogus,
    }

    // (c) The whole DNSKEY RRset is now trusted. The answer RRSIG must verify
    //     under the ZSK it names (its `key_tag`) over `answer_owner`. No key with
    //     that tag ⇒ no path (Indeterminate); a matching key whose signature
    //     fails ⇒ tamper (Bogus).
    if !rrsig_time_valid(answer_rrsig, now_unix) {
        return DnssecStatus::Bogus;
    }
    let mut saw_zsk = false;
    let mut verdict = DnssecStatus::Indeterminate;
    for key in zone_dnskeys {
        if dnskey_key_tag(key) == answer_rrsig.key_tag {
            saw_zsk = true;
            match verify_rrsig(answer_rrsig, key, answer_owner, answer_rrset) {
                DnssecStatus::Secure => return DnssecStatus::Secure,
                DnssecStatus::Indeterminate => verdict = DnssecStatus::Indeterminate,
                _ => verdict = DnssecStatus::Bogus,
            }
        }
    }
    if saw_zsk {
        verdict
    } else {
        // The answer's RRSIG names a key that is not in the trusted DNSKEY
        // RRset — no path from the anchor to this signature.
        DnssecStatus::Indeterminate
    }
}

/// Abstraction over "fetch a zone's DNSSEC records" so the recursive delegation
/// walk ([`validate_with_delegation`]) is testable with ZERO network: the walk
/// calls the fetcher, never a socket. A future `impl DnssecFetcher for
/// LiveResolver` issuing a `net::udp_query` DNSKEY query per zone and a DS query
/// per delegation (needs a DHCP lease, hence iron-gated) drops in WITHOUT
/// touching the walk logic — that live impl is the one remaining DNSSEC tail.
/// MasterChecklist Phase 10.
///
/// Zone names are canonical: ASCII-lowercase, no trailing dot; the root is the
/// empty string `""`.
pub trait DnssecFetcher {
    /// This zone's DNSKEY RRset plus the RRSIG its KSK made over that RRset.
    /// `None` = the zone returned no DNSKEY / was unreachable (a missing hop).
    fn fetch_dnskeys(&self, zone: &str) -> Option<(Vec<Dnskey>, Rrsig)>;
    /// The DS RRset `parent_zone` publishes for `child_zone`, plus the RRSIG the
    /// parent's ZSK made over it. `None` = no secure delegation was returned.
    fn fetch_ds(&self, parent_zone: &str, child_zone: &str) -> Option<(Vec<Ds>, Rrsig)>;
}

/// Build the delegation chain of ZONES from the root down to `zone`, canonical
/// and root-first: `"example.com"` → `["", "com", "example.com"]`; `""` → `[""]`.
fn zone_chain(zone: &str) -> Vec<String> {
    let z = zone.trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = if z.is_empty() {
        Vec::new()
    } else {
        z.split('.').filter(|l| !l.is_empty()).collect()
    };
    let mut chain = Vec::with_capacity(labels.len() + 1);
    chain.push(String::new()); // the root zone "."
    for i in (0..labels.len()).rev() {
        chain.push(labels[i..].join("."));
    }
    chain
}

/// Recursive multi-zone DNSSEC delegation walk (RFC 4035 §5): start at the root
/// trust anchor and descend the zone cuts to the answer's owner zone, verifying
/// EVERY hop, then verify the answer itself. `fetcher` supplies each zone's
/// DNSKEY RRset and each parent's DS RRset for its child; the walk never touches
/// the network, so it is fully host/boot-provable. A live `impl DnssecFetcher`
/// issuing real UDP DNSKEY/DS queries is the ONLY remaining piece (iron-gated on
/// a DHCP lease). MasterChecklist Phase 10.
///
/// The owner zone is taken from `answer_rrsig.signer_name` (RFC 4035 §5.3.1: the
/// signer name of an RRSIG is the name of the zone that signed it). For each zone
/// from the root down: fetch its DNSKEY RRset + RRSIG, confirm the currently
/// trusted DS set authenticates a KSK here and that KSK signed the DNSKEY RRset
/// (via [`validate_hop`]); if this is not the owner zone, fetch the child's DS
/// RRset, verify it under THIS zone's ZSK, and carry it forward as the next
/// trusted anchor set. At the owner zone, verify the answer RRSIG.
///
/// Fail-closed: `Secure` ONLY if every hop AND the answer verify; a broken
/// DS/RRSIG at any hop ⇒ `Bogus`; a hop the fetcher cannot supply (`None`) ⇒
/// `Indeterminate` (no path). `qtype` guards that the answer is of the queried
/// type.
pub fn validate_with_delegation(
    root_anchor: &Ds,
    fetcher: &dyn DnssecFetcher,
    owner: &str,
    qtype: u16,
    answer_rrset: &[CanonicalRr],
    answer_rrsig: &Rrsig,
    now_unix: u32,
) -> DnssecStatus {
    // The answer must actually be of the queried type — the RRSIG that covers it
    // and every answer RR — or it is not the data that was asked for.
    if answer_rrsig.type_covered != qtype || answer_rrset.iter().any(|rr| rr.rtype != qtype) {
        return DnssecStatus::Indeterminate;
    }

    let owner_lower = owner.trim_end_matches('.').to_ascii_lowercase();
    // The zone that signed the answer (the RRSIG signer name is the zone name).
    let owner_zone = answer_rrsig
        .signer_name
        .trim_end_matches('.')
        .to_ascii_lowercase();
    // The signer must be at or above the answer owner, else it is not
    // authoritative for it (fail-closed, never a false accept).
    let in_zone = owner_zone.is_empty()
        || owner_lower == owner_zone
        || owner_lower.ends_with(&alloc::format!(".{}", owner_zone));
    if !in_zone {
        return DnssecStatus::Indeterminate;
    }

    let chain = zone_chain(&owner_zone);
    // The trusted DS set for the current zone, seeded with the root anchor.
    let mut trusted_ds: Vec<Ds> = alloc::vec![root_anchor.clone()];

    for idx in 0..chain.len() {
        let zone = chain[idx].as_str();
        let (dnskeys, dnskey_rrsig) = match fetcher.fetch_dnskeys(zone) {
            Some(v) => v,
            None => return DnssecStatus::Indeterminate, // missing hop → no path
        };

        if idx + 1 == chain.len() {
            // Owner zone: verify the real answer, owned by `owner`, under it.
            return validate_hop(
                &trusted_ds,
                zone,
                &dnskeys,
                &dnskey_rrsig,
                &owner_lower,
                answer_rrset,
                answer_rrsig,
                now_unix,
            );
        }

        // Intermediate zone: the "answer" is the child's DS RRset — owned by the
        // child, signed by THIS zone's ZSK. Verify it, then carry the trusted
        // child DS set forward as the anchor for the next hop.
        let child_zone = chain[idx + 1].as_str();
        let (child_ds, ds_rrsig) = match fetcher.fetch_ds(zone, child_zone) {
            Some(v) => v,
            None => return DnssecStatus::Indeterminate,
        };
        let ds_canonical: Vec<CanonicalRr> = child_ds
            .iter()
            .map(|d| CanonicalRr {
                rtype: DnssecRrType::Ds as u16,
                class: 1,
                rdata: ds_rdata(d),
            })
            .collect();
        match validate_hop(
            &trusted_ds,
            zone,
            &dnskeys,
            &dnskey_rrsig,
            child_zone,
            &ds_canonical,
            &ds_rrsig,
            now_unix,
        ) {
            DnssecStatus::Secure => trusted_ds = child_ds,
            other => return other, // Bogus / Indeterminate propagate (fail-closed)
        }
    }

    // Unreachable: the loop always returns at the owner zone. Fail-closed.
    DnssecStatus::Indeterminate
}

// ── Upstream Server State ───────────────────────────────────────────────────

struct UpstreamServer {
    addr: [u8; 4],
    failures: u32,
    last_success: u64,
    last_failure: u64,
    avg_rtt_ms: u64,
}

impl UpstreamServer {
    fn new(addr: [u8; 4]) -> Self {
        Self {
            addr,
            failures: 0,
            last_success: 0,
            last_failure: 0,
            avg_rtt_ms: 0,
        }
    }

    fn record_success(&mut self, now: u64, rtt_ms: u64) {
        self.failures = 0;
        self.last_success = now;
        self.avg_rtt_ms = (self.avg_rtt_ms * 7 + rtt_ms) / 8;
    }

    fn record_failure(&mut self, now: u64) {
        self.failures += 1;
        self.last_failure = now;
    }

    fn is_healthy(&self, now: u64) -> bool {
        if self.failures == 0 {
            return true;
        }
        let backoff = (1u64 << self.failures.min(6)) * 1000;
        now.saturating_sub(self.last_failure) >= backoff
    }
}

// ── Name Encoding / Decoding ────────────────────────────────────────────────

pub fn encode_name(name: &str, buf: &mut Vec<u8>) {
    for label in name.split('.') {
        if label.is_empty() {
            continue;
        }
        buf.push(label.len() as u8);
        buf.extend_from_slice(label.as_bytes());
    }
    buf.push(0);
}

pub fn decode_name(data: &[u8], offset: &mut usize) -> Option<String> {
    let mut name = String::new();
    let mut pos = *offset;
    let mut jumped = false;
    let mut jump_offset = 0usize;
    let mut safety = 0u32;

    loop {
        if pos >= data.len() || safety > 256 {
            return None;
        }
        safety += 1;

        let len = data[pos] as usize;

        if len == 0 {
            if !jumped {
                *offset = pos + 1;
            }
            break;
        }

        // Compression pointer (top 2 bits set)
        if (len & 0xC0) == 0xC0 {
            if pos + 1 >= data.len() {
                return None;
            }
            if !jumped {
                jump_offset = pos + 2;
            }
            pos = ((len & 0x3F) << 8) | (data[pos + 1] as usize);
            jumped = true;
            continue;
        }

        pos += 1;
        if pos + len > data.len() {
            return None;
        }

        if !name.is_empty() {
            name.push('.');
        }
        if let Ok(label) = core::str::from_utf8(&data[pos..pos + len]) {
            name.push_str(label);
        } else {
            return None;
        }
        pos += len;
    }

    if jumped {
        *offset = jump_offset;
    }

    Some(name)
}

// ── DNS Resolver ────────────────────────────────────────────────────────────

pub struct DnsResolver {
    servers: Vec<UpstreamServer>,
    cache: BTreeMap<String, DnsCacheEntry>,
    static_hosts: Vec<StaticHostEntry>,
    max_cache: usize,
    negative_ttl: u32,
    timeout_ms: u64,
    next_id: u16,
    /// Index of the server to try next (round-robin with failover).
    current_server: usize,
}

pub static DNS_RESOLVER: Mutex<Option<DnsResolver>> = Mutex::new(None);

impl DnsResolver {
    pub fn new() -> Self {
        Self {
            servers: Vec::new(),
            cache: BTreeMap::new(),
            static_hosts: Vec::new(),
            max_cache: 1024,
            negative_ttl: 60,
            timeout_ms: 5000,
            next_id: 1,
            current_server: 0,
        }
    }

    // ── Server management ───────────────────────────────────────────────

    pub fn set_servers(&mut self, addrs: Vec<[u8; 4]>) {
        self.servers = addrs.into_iter().map(UpstreamServer::new).collect();
        self.current_server = 0;
    }

    pub fn add_server(&mut self, addr: [u8; 4]) {
        if !self.servers.iter().any(|s| s.addr == addr) {
            self.servers.push(UpstreamServer::new(addr));
        }
    }

    pub fn servers(&self) -> Vec<[u8; 4]> {
        self.servers.iter().map(|s| s.addr).collect()
    }

    pub fn server_count(&self) -> usize {
        self.servers.len()
    }

    /// Pick the next healthy upstream server, falling back to any server
    /// if none are currently healthy.
    pub fn pick_server(&mut self, now: u64) -> Option<[u8; 4]> {
        if self.servers.is_empty() {
            return None;
        }

        let n = self.servers.len();
        for i in 0..n {
            let idx = (self.current_server + i) % n;
            if self.servers[idx].is_healthy(now) {
                self.current_server = (idx + 1) % n;
                return Some(self.servers[idx].addr);
            }
        }

        // All servers unhealthy — try the current one anyway
        let addr = self.servers[self.current_server].addr;
        self.current_server = (self.current_server + 1) % n;
        Some(addr)
    }

    /// Record a successful response from a server.
    pub fn record_server_success(&mut self, addr: [u8; 4], now: u64, rtt_ms: u64) {
        if let Some(s) = self.servers.iter_mut().find(|s| s.addr == addr) {
            s.record_success(now, rtt_ms);
        }
    }

    /// Record a failed query to a server.
    pub fn record_server_failure(&mut self, addr: [u8; 4], now: u64) {
        if let Some(s) = self.servers.iter_mut().find(|s| s.addr == addr) {
            s.record_failure(now);
        }
    }

    // ── Static hosts ────────────────────────────────────────────────────

    pub fn add_static_host(&mut self, hostname: String, address: [u8; 4]) {
        self.static_hosts.retain(|h| h.hostname != hostname);
        self.static_hosts
            .push(StaticHostEntry { hostname, address });
    }

    pub fn remove_static_host(&mut self, hostname: &str) -> bool {
        let before = self.static_hosts.len();
        self.static_hosts.retain(|h| h.hostname != hostname);
        self.static_hosts.len() < before
    }

    pub fn static_hosts(&self) -> &[StaticHostEntry] {
        &self.static_hosts
    }

    fn lookup_static(&self, name: &str) -> Option<Vec<DnsRecord>> {
        let lower = name.to_ascii_lowercase();
        let matches: Vec<DnsRecord> = self
            .static_hosts
            .iter()
            .filter(|h| h.hostname == lower || h.hostname == name)
            .map(|h| DnsRecord::A(h.address))
            .collect();

        if matches.is_empty() {
            None
        } else {
            Some(matches)
        }
    }

    // ── Resolution ──────────────────────────────────────────────────────

    /// Resolve a hostname to A records (checks static hosts, cache, then
    /// prepares a query for upstream).
    pub fn resolve(&mut self, name: &str, now: u64) -> ResolveResult {
        self.resolve_with_type(name, DnsQueryType::A, now)
    }

    /// Resolve with a specific query type.
    pub fn resolve_with_type(
        &mut self,
        name: &str,
        qtype: DnsQueryType,
        now: u64,
    ) -> ResolveResult {
        // 1. Static hosts (A queries only)
        if matches!(qtype, DnsQueryType::A) {
            if let Some(records) = self.lookup_static(name) {
                return ResolveResult::Cached(records);
            }
        }

        // 2. Cache lookup
        let cache_key = Self::cache_key(name, qtype);
        if let Some(entry) = self.cache.get(&cache_key) {
            if entry.expires_at > now {
                if entry.is_negative {
                    return ResolveResult::NxDomain;
                }
                return ResolveResult::Cached(entry.records.clone());
            }
        }

        // 3. Build query for upstream
        if self.servers.is_empty() {
            return ResolveResult::NoServers;
        }

        let server = match self.pick_server(now) {
            Some(s) => s,
            None => return ResolveResult::NoServers,
        };

        let query_data = self.build_query(name, qtype);
        ResolveResult::QueryReady {
            server,
            data: query_data,
        }
    }

    fn cache_key(name: &str, qtype: DnsQueryType) -> String {
        use core::fmt::Write;
        let mut key = String::new();
        let _ = write!(key, "{}:{}", qtype as u16, name);
        key
    }

    pub fn build_query(&mut self, name: &str, qtype: DnsQueryType) -> Vec<u8> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        let mut buf = Vec::with_capacity(64);

        let header = DnsHeader {
            id,
            flags: 0x0100, // RD (recursion desired)
            qd_count: 1,
            an_count: 0,
            ns_count: 0,
            ar_count: 0,
        };
        header.serialize(&mut buf);

        encode_name(name, &mut buf);
        buf.extend_from_slice(&(qtype as u16).to_be_bytes());
        buf.extend_from_slice(&1u16.to_be_bytes()); // IN class

        buf
    }

    // ── DNS-over-HTTPS stub ─────────────────────────────────────────────

    /// Build a DOH query URL (RFC 8484). Returns the URL and the
    /// base64url-encoded DNS wire-format query. Full TLS is not yet
    /// implemented; this constructs what the DOH request *would* look like.
    pub fn build_doh_query(&mut self, name: &str, qtype: DnsQueryType) -> DohQuery {
        let wire = self.build_query(name, qtype);

        // base64url encode (no padding) — simple manual encoder
        let encoded = base64url_encode(&wire);

        DohQuery {
            url: String::from("https://cloudflare-dns.com/dns-query"),
            wire_query: wire,
            encoded_query: encoded,
            accept: String::from("application/dns-message"),
        }
    }

    // ── Response handling ───────────────────────────────────────────────

    pub fn parse_response(&self, data: &[u8]) -> Option<DnsResponse> {
        let header = DnsHeader::parse(data)?;
        if !header.is_response() {
            return None;
        }

        let mut offset = 12;

        // Parse questions to extract query name/type
        let mut query_name = String::new();
        let mut query_type = DnsQueryType::A;

        for i in 0..header.qd_count {
            let name = decode_name(data, &mut offset)?;
            if offset + 4 > data.len() {
                return None;
            }
            let qt = u16::from_be_bytes([data[offset], data[offset + 1]]);
            offset += 4; // qtype + qclass

            if i == 0 {
                query_name = name;
                query_type = DnsQueryType::from_u16(qt).unwrap_or(DnsQueryType::A);
            }
        }

        let (answers, answer_ttls) =
            self.parse_records_with_ttl(data, &mut offset, header.an_count)?;
        let (authority, _) = self.parse_records_with_ttl(data, &mut offset, header.ns_count)?;
        let (additional, _) = self.parse_records_with_ttl(data, &mut offset, header.ar_count)?;

        // Second, independent pass to retain the raw DNSSEC RRs (RRSIG/DNSKEY/DS)
        // and the canonical answer RDATA the decoded `DnsRecord` path discards.
        // This never fails the whole parse — a malformed DNSSEC RR is skipped, so
        // an ordinary (unsigned) response still parses exactly as before.
        let (rrsigs, dnskeys, dss, answer_canonical) = scan_dnssec_rrs(data);

        Some(DnsResponse {
            id: header.id,
            authoritative: header.is_authoritative(),
            truncated: header.is_truncated(),
            recursion_available: header.recursion_available(),
            rcode: header.rcode(),
            answers,
            answer_ttls,
            authority,
            additional,
            query_name,
            query_type,
            rrsigs,
            dnskeys,
            dss,
            answer_canonical,
        })
    }

    /// Parse records and return their TTLs alongside them.
    fn parse_records_with_ttl(
        &self,
        data: &[u8],
        offset: &mut usize,
        count: u16,
    ) -> Option<(Vec<DnsRecord>, Vec<u32>)> {
        let mut records = Vec::new();
        let mut ttls = Vec::new();

        for _ in 0..count {
            let _name = decode_name(data, offset)?;
            if *offset + 10 > data.len() {
                return None;
            }

            let rtype = u16::from_be_bytes([data[*offset], data[*offset + 1]]);
            let _rclass = u16::from_be_bytes([data[*offset + 2], data[*offset + 3]]);
            let ttl = u32::from_be_bytes([
                data[*offset + 4],
                data[*offset + 5],
                data[*offset + 6],
                data[*offset + 7],
            ]);
            let rdlength = u16::from_be_bytes([data[*offset + 8], data[*offset + 9]]) as usize;
            *offset += 10;

            if *offset + rdlength > data.len() {
                return None;
            }

            let record = match DnsQueryType::from_u16(rtype) {
                Some(DnsQueryType::A) if rdlength == 4 => DnsRecord::A([
                    data[*offset],
                    data[*offset + 1],
                    data[*offset + 2],
                    data[*offset + 3],
                ]),
                Some(DnsQueryType::Aaaa) if rdlength == 16 => {
                    let mut addr = [0u8; 16];
                    addr.copy_from_slice(&data[*offset..*offset + 16]);
                    DnsRecord::Aaaa(addr)
                }
                Some(DnsQueryType::Cname) => {
                    let mut cname_offset = *offset;
                    let cname = decode_name(data, &mut cname_offset)?;
                    DnsRecord::Cname(cname)
                }
                Some(DnsQueryType::Mx) if rdlength >= 3 => {
                    let priority = u16::from_be_bytes([data[*offset], data[*offset + 1]]);
                    let mut mx_offset = *offset + 2;
                    let exchange = decode_name(data, &mut mx_offset)?;
                    DnsRecord::Mx { priority, exchange }
                }
                Some(DnsQueryType::Txt) => {
                    let txt = core::str::from_utf8(&data[*offset..*offset + rdlength])
                        .unwrap_or("")
                        .into();
                    DnsRecord::Txt(txt)
                }
                Some(DnsQueryType::Ns) => {
                    let mut ns_offset = *offset;
                    let ns = decode_name(data, &mut ns_offset)?;
                    DnsRecord::Ns(ns)
                }
                Some(DnsQueryType::Ptr) => {
                    let mut ptr_offset = *offset;
                    let ptr = decode_name(data, &mut ptr_offset)?;
                    DnsRecord::Ptr(ptr)
                }
                Some(DnsQueryType::Srv) if rdlength >= 7 => {
                    let priority = u16::from_be_bytes([data[*offset], data[*offset + 1]]);
                    let weight = u16::from_be_bytes([data[*offset + 2], data[*offset + 3]]);
                    let port = u16::from_be_bytes([data[*offset + 4], data[*offset + 5]]);
                    let mut srv_offset = *offset + 6;
                    let target = decode_name(data, &mut srv_offset)?;
                    DnsRecord::Srv {
                        priority,
                        weight,
                        port,
                        target,
                    }
                }
                _ => {
                    *offset += rdlength;
                    continue;
                }
            };

            records.push(record);
            ttls.push(ttl);
            *offset += rdlength;
        }

        Some((records, ttls))
    }

    /// Handle a raw DNS response: parse it, cache the results (with TTL),
    /// and return the records. Handles both positive and negative caching.
    pub fn handle_response(&mut self, data: &[u8], now: u64) -> Option<DnsResponse> {
        let resp = self.parse_response(data)?;

        match resp.rcode {
            DnsResponseCode::NoError => {
                if !resp.answers.is_empty() {
                    let min_ttl = resp
                        .answer_ttls
                        .iter()
                        .copied()
                        .min()
                        .unwrap_or(300)
                        .max(10)
                        .min(86400);

                    self.cache_insert(
                        &resp.query_name,
                        resp.query_type,
                        resp.answers.clone(),
                        min_ttl,
                        now,
                    );
                }
            }
            DnsResponseCode::NxDomain => {
                self.cache_insert_negative(&resp.query_name, resp.query_type, now);
            }
            _ => {}
        }

        Some(resp)
    }

    // ── Cache operations ────────────────────────────────────────────────

    pub fn cache_lookup(&self, key: &str, now: u64) -> Option<Vec<DnsRecord>> {
        self.cache.get(key).and_then(|entry| {
            if entry.expires_at > now && !entry.is_negative {
                Some(entry.records.clone())
            } else {
                None
            }
        })
    }

    /// Check if a name has a negative cache entry (NXDOMAIN).
    pub fn cache_is_negative(&self, name: &str, qtype: DnsQueryType, now: u64) -> bool {
        let key = Self::cache_key(name, qtype);
        self.cache
            .get(&key)
            .map(|e| e.is_negative && e.expires_at > now)
            .unwrap_or(false)
    }

    pub fn cache_insert(
        &mut self,
        name: &str,
        qtype: DnsQueryType,
        records: Vec<DnsRecord>,
        ttl: u32,
        now: u64,
    ) {
        if records.is_empty() {
            return;
        }

        if self.cache.len() >= self.max_cache {
            self.cache_evict(now);
        }

        let key = Self::cache_key(name, qtype);
        self.cache.insert(
            key,
            DnsCacheEntry {
                records,
                expires_at: now + ttl as u64,
                inserted_at: now,
                is_negative: false,
            },
        );
    }

    /// Insert a negative cache entry (NXDOMAIN). Uses the configured
    /// `negative_ttl` (default 60 seconds).
    pub fn cache_insert_negative(&mut self, name: &str, qtype: DnsQueryType, now: u64) {
        if self.cache.len() >= self.max_cache {
            self.cache_evict(now);
        }

        let key = Self::cache_key(name, qtype);
        self.cache.insert(
            key,
            DnsCacheEntry {
                records: Vec::new(),
                expires_at: now + self.negative_ttl as u64,
                inserted_at: now,
                is_negative: true,
            },
        );
    }

    pub fn cache_evict(&mut self, now: u64) {
        self.cache.retain(|_, entry| entry.expires_at > now);

        while self.cache.len() >= self.max_cache {
            let oldest = self
                .cache
                .iter()
                .min_by_key(|(_, e)| e.inserted_at)
                .map(|(k, _)| k.clone());

            if let Some(key) = oldest {
                self.cache.remove(&key);
            } else {
                break;
            }
        }
    }

    pub fn cache_clear(&mut self) {
        self.cache.clear();
    }

    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }

    pub fn set_negative_ttl(&mut self, ttl: u32) {
        self.negative_ttl = ttl;
    }

    // ── DNSSEC validation ────────────────────────────────────────────────

    /// Validate a response's DNSSEC signatures against a configured trust anchor,
    /// for a SINGLE signed zone (RFC 4035 §5.3). `trust_anchor` is the parent's
    /// `DS` committing to this zone's KSK (a DNSSEC configuration input — the root
    /// anchor for an apex zone, or a locally-configured anchor); `now_unix` is the
    /// wall clock in seconds for the RRSIG validity-window check.
    ///
    /// From the RRs the wire parser retained (`response.dnskeys`, `.rrsigs`,
    /// `.answer_canonical`) this assembles the zone's DNSKEY RRset + the RRSIG the
    /// KSK made over it, and the covered answer RRset (the `query_type` records at
    /// the zone apex) + the RRSIG the ZSK made over it, then calls
    /// [`validate_chain_of_trust`]:
    ///
    /// ```text
    ///   trust_anchor DS ⇒ KSK ⇒ DNSKEY RRset ⇒ ZSK ⇒ answer RRset  ⇒  Secure
    /// ```
    ///
    /// Fail-closed (RFC 4035 §5.5): returns `Secure` ONLY when every link verifies
    /// and every RRSIG is inside its validity window; `Bogus` when a link is
    /// present but tampered/expired (an active forgery); `Indeterminate` when the
    /// response lacks the RRs needed to decide — an unsigned response (no DNSKEY
    /// or no covering RRSIG), no answer of the queried type, or an algorithm this
    /// build does not verify. `Indeterminate` (never `Insecure`) is correct here:
    /// proving an *unsigned delegation* (`Insecure`) needs the authenticated NSEC
    /// denial-of-DS from the parent, which the single-zone scope does not walk.
    ///
    /// SCOPE: this validates one signed zone against a directly-configured anchor.
    /// The live recursive, multi-zone delegation walk (fetch each hop's DS+DNSKEY
    /// from the root down to `owner`, chaining `verify_ds`/`verify_rrsig`) needs
    /// network queries and remains the documented follow-up. MasterChecklist
    /// Phase 10.
    pub fn validate_dnssec(
        &self,
        response: &DnsResponse,
        trust_anchor: &Ds,
        now_unix: u32,
    ) -> DnssecStatus {
        // The zone apex owns the answer, its DNSKEY RRset, and (equals) the
        // anchor's zone — the single-zone scope validates data AT the apex.
        let owner = response.query_name.to_ascii_lowercase();

        // (1) The zone's DNSKEY RRset (all DNSKEYs owned by the apex).
        let dnskeys: Vec<Dnskey> = response
            .dnskeys
            .iter()
            .filter(|(n, _)| *n == owner)
            .map(|(_, k)| k.clone())
            .collect();
        if dnskeys.is_empty() {
            // No DNSKEY present — an unsigned response; cannot decide.
            return DnssecStatus::Indeterminate;
        }

        // (2) The RRSIG the KSK made over the DNSKEY RRset (type covered 48).
        let dnskey_rrsig = match response
            .rrsigs
            .iter()
            .find(|(n, rs)| *n == owner && rs.type_covered == DnssecRrType::Dnskey as u16)
        {
            Some((_, rs)) => rs,
            None => return DnssecStatus::Indeterminate,
        };

        // (3) The covered answer RRset — the queried-type records at the apex.
        let qtype = response.query_type as u16;
        let answer_rrset: Vec<CanonicalRr> = response
            .answer_canonical
            .iter()
            .filter(|(n, rr)| *n == owner && rr.rtype == qtype)
            .map(|(_, rr)| rr.clone())
            .collect();
        if answer_rrset.is_empty() {
            return DnssecStatus::Indeterminate;
        }

        // (4) The RRSIG the ZSK made over the answer RRset (type covered qtype).
        let answer_rrsig = match response
            .rrsigs
            .iter()
            .find(|(n, rs)| *n == owner && rs.type_covered == qtype)
        {
            Some((_, rs)) => rs,
            None => return DnssecStatus::Indeterminate,
        };

        validate_chain_of_trust(
            trust_anchor,
            &dnskeys,
            dnskey_rrsig,
            &owner,
            &answer_rrset,
            answer_rrsig,
            now_unix,
        )
    }
}

// ── DOH Query ───────────────────────────────────────────────────────────────

pub struct DohQuery {
    pub url: String,
    pub wire_query: Vec<u8>,
    pub encoded_query: String,
    pub accept: String,
}

/// base64url encode (RFC 4648 §5) without padding.
fn base64url_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((data.len() * 4 + 2) / 3);
    let mut i = 0;
    while i + 2 < data.len() {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | (data[i + 2] as u32);
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        out.push(TABLE[(n & 0x3F) as usize] as char);
        i += 3;
    }
    let remaining = data.len() - i;
    if remaining == 2 {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8);
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
    } else if remaining == 1 {
        let n = (data[i] as u32) << 16;
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
    }
    out
}

// ── Resolve Result ──────────────────────────────────────────────────────────

pub enum ResolveResult {
    Cached(Vec<DnsRecord>),
    QueryReady { server: [u8; 4], data: Vec<u8> },
    NxDomain,
    NoServers,
}

/// Initialize the DNS resolver with default upstream servers.
pub fn init() {
    let mut resolver = DnsResolver::new();

    // Default public DNS servers
    resolver.add_server([8, 8, 8, 8]); // Google
    resolver.add_server([1, 1, 1, 1]); // Cloudflare
    resolver.add_server([9, 9, 9, 9]); // Quad9

    // Default static hosts
    resolver.add_static_host(String::from("localhost"), [127, 0, 0, 1]);

    *DNS_RESOLVER.lock() = Some(resolver);

    crate::serial_println!(
        "[ OK ] DNS resolver initialized (3 upstream servers, cache max 1024, negative TTL 60s)",
    );
}

/// Handle a raw DNS response packet. Call from the network stack when
/// a UDP packet from port 53 arrives.
pub fn handle_response(data: &[u8], now: u64) -> Option<DnsResponse> {
    let mut guard = DNS_RESOLVER.lock();
    guard.as_mut()?.handle_response(data, now)
}

/// Resolve a hostname (convenience wrapper around the global resolver).
pub fn resolve(name: &str, now: u64) -> ResolveResult {
    let mut guard = DNS_RESOLVER.lock();
    match *guard {
        Some(ref mut resolver) => resolver.resolve(name, now),
        None => ResolveResult::NoServers,
    }
}

/// First A record in a record set, as raw IPv4 bytes.
fn first_a(records: &[DnsRecord]) -> Option<[u8; 4]> {
    records.iter().find_map(|r| match r {
        DnsRecord::A(ip) => Some(*ip),
        _ => None,
    })
}

/// Resolve `hostname` to an IPv4 address, performing the upstream UDP query if
/// it isn't already a static host or cached. This is the function `SYS_NET_DNS`
/// exposes to userspace — the missing glue between the (already-proven) DNS
/// codec and the live network stack. Static/cached lookups return with zero
/// network I/O (deterministic, smoketested); a cache miss does one UDP
/// round-trip to the chosen server via `net::udp_query` (needs a DHCP lease, so
/// it resolves on iron, not in headless CI). MasterChecklist Phase 10.2.
pub fn resolve_blocking(hostname: &str) -> Option<[u8; 4]> {
    let now = crate::game_session::sys_wall_clock() / 1_000_000_000;

    match resolve(hostname, now) {
        ResolveResult::Cached(records) => first_a(&records),
        ResolveResult::NxDomain => None,
        ResolveResult::NoServers => {
            crate::serial_println!(
                "[dns] resolve('{}'): no upstream servers configured",
                hostname
            );
            None
        }
        ResolveResult::QueryReady { server, data } => {
            crate::serial_println!(
                "[dns] resolve('{}'): querying {}.{}.{}.{}:53",
                hostname,
                server[0],
                server[1],
                server[2],
                server[3],
            );
            let resp = crate::net::udp_query(server, 53, &data, 3000)?;
            handle_response(&resp, now);
            // Re-resolve: the response just populated the cache.
            match resolve(hostname, now) {
                ResolveResult::Cached(records) => first_a(&records),
                _ => None,
            }
        }
    }
}

/// Add a static host entry to the global resolver.
pub fn add_static_host(hostname: String, addr: [u8; 4]) {
    let mut guard = DNS_RESOLVER.lock();
    if let Some(ref mut resolver) = *guard {
        resolver.add_static_host(hostname, addr);
    }
}

// ── Boot smoketest (R10) ──────────────────────────────────────────────────────

/// Deterministic proof of the DNS wire codec and resolver decision logic with
/// ZERO network I/O: name encode/decode round-trip, header serialize/parse,
/// compression-pointer decompression, query construction, and static-host
/// resolution. MasterChecklist Phase 10 (supports the `curl` acceptance);
/// Concept §AthNet name resolution.
pub fn run_boot_smoketest() {
    let mut pass = 0u32;
    let mut total = 0u32;
    let mut check = |cond: bool, name: &str| {
        total += 1;
        if cond {
            pass += 1;
        } else {
            crate::serial_println!("[dns-selftest] FAIL {}", name);
        }
    };

    // 1. Name encode → decode round-trip.
    let mut buf = Vec::new();
    encode_name("example.com", &mut buf);
    let mut off = 0usize;
    let decoded = decode_name(&buf, &mut off);
    check(decoded.as_deref() == Some("example.com"), "name-roundtrip");

    // 2. Header serialize → parse round-trip (response, qd=1, an=2).
    let h = DnsHeader {
        id: 0x1234,
        flags: 0x8180,
        qd_count: 1,
        an_count: 2,
        ns_count: 0,
        ar_count: 0,
    };
    let mut hbuf = Vec::new();
    h.serialize(&mut hbuf);
    let header_ok = DnsHeader::parse(&hbuf)
        .map(|p| p.id == 0x1234 && p.qd_count == 1 && p.an_count == 2 && p.is_response())
        .unwrap_or(false);
    check(header_ok, "header-roundtrip");

    // 3. Compression pointer: a label "a" followed by a 0xC0 pointer back to
    //    the "example.com" name at offset 0 must decompress to "a.example.com".
    let mut cbuf = buf.clone();
    let ptr_at = cbuf.len();
    cbuf.push(0x01);
    cbuf.push(b'a');
    cbuf.push(0xC0);
    cbuf.push(0x00);
    let mut coff = ptr_at;
    let cdec = decode_name(&cbuf, &mut coff);
    check(
        cdec.as_deref() == Some("a.example.com"),
        "compression-pointer",
    );

    // 4. Query construction: header + encoded name + qtype(A=1) + class(IN=1).
    let mut r = DnsResolver::new();
    let q = r.build_query("athena.dev", DnsQueryType::A);
    let query_ok = DnsHeader::parse(&q)
        .map(|x| x.qd_count == 1 && x.an_count == 0)
        .unwrap_or(false)
        && q.len() > 12
        && q[q.len() - 4..] == [0x00, 0x01, 0x00, 0x01];
    check(query_ok, "build-query");

    // 5. Static host resolves synchronously, no upstream server contacted.
    let mut r2 = DnsResolver::new();
    r2.add_static_host(String::from("athena.local"), [10, 0, 0, 7]);
    let static_ok = matches!(
        r2.resolve("athena.local", 1000),
        ResolveResult::Cached(ref v) if matches!(v.first(), Some(DnsRecord::A([10, 0, 0, 7])))
    );
    check(static_ok, "static-host-resolve");

    // 6. resolve_blocking() end-to-end glue via the GLOBAL resolver's static
    //    host (`localhost` → 127.0.0.1, seeded in init()): proves the
    //    SYS_NET_DNS path returns an A record with no network I/O. The upstream
    //    UDP path is exercised on iron (needs a DHCP lease + internet).
    let blocking_ok = resolve_blocking("localhost") == Some([127, 0, 0, 1]);
    check(blocking_ok, "resolve-blocking-static");

    // 7. DoS resilience: a DNS response is ATTACKER-CONTROLLED (spoofable). Every
    //    malformed input below must be rejected gracefully (return None) and,
    //    above all, must NOT panic — an out-of-bounds panic here would crash the
    //    kernel on a hostile packet (remote DoS). Reaching the tally at all
    //    proves no panic; each `check` proves graceful rejection.
    let resolver = DnsResolver::new();
    check(resolver.parse_response(&[]).is_none(), "resilience-empty");
    check(
        resolver.parse_response(&[0u8; 8]).is_none(),
        "resilience-trunc-header",
    );
    // Header claims 3 questions but carries no question bytes.
    let mut short = alloc::vec![0u8; 12];
    short[2] = 0x81;
    short[3] = 0x80; // response flags
    short[5] = 0x03; // qd_count = 3
    check(
        resolver.parse_response(&short).is_none(),
        "resilience-trunc-question",
    );
    // A single answer claiming rdlength = 65535 with no rdata → bounds reject.
    let mut rd = alloc::vec![0u8; 12];
    rd[2] = 0x81;
    rd[3] = 0x80; // response
    rd[7] = 0x01; // an_count = 1
    rd.push(0x00); // root name
    rd.extend_from_slice(&[0x00, 0x01, 0x00, 0x01]); // type A, class IN
    rd.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // ttl
    rd.extend_from_slice(&[0xFF, 0xFF]); // rdlength = 65535 (overflow)
    check(
        resolver.parse_response(&rd).is_none(),
        "resilience-rdlength-overflow",
    );
    // Compression-pointer self-loop must terminate via the safety guard.
    let mut loopbuf = alloc::vec![0u8; 14];
    loopbuf[12] = 0xC0;
    loopbuf[13] = 0x0C; // pointer to offset 12 (itself)
    let mut loff = 12usize;
    check(
        decode_name(&loopbuf, &mut loff).is_none(),
        "resilience-compression-loop",
    );
    // decode_name starting past the end of the buffer.
    let mut ooff = 999usize;
    check(
        decode_name(&loopbuf, &mut ooff).is_none(),
        "resilience-oob-offset",
    );

    // ── DNSSEC RRSIG verification against the RFC 8080 §A Ed25519 vector ──
    // A REAL published DNSSEC signature (example.com MX, algorithm 15). If our
    // signed-data reconstruction (RFC 4034 §3.1.8.1) or canonicalization is
    // wrong, the real signature will NOT verify and this FAILs — an external
    // oracle, not a self-signed round-trip.
    const DNSSEC_PUBKEY: [u8; 32] = [
        0x97, 0x4d, 0x96, 0xa2, 0x2d, 0x22, 0x4b, 0xc0, 0x1a, 0xdb, 0x91, 0x50, 0x91, 0x47, 0x7d,
        0x44, 0xcc, 0xd9, 0x1c, 0x9a, 0x41, 0xa1, 0x14, 0x30, 0x01, 0x01, 0x17, 0xd5, 0x2c, 0x59,
        0x24, 0x0e,
    ];
    const DNSSEC_SIG: [u8; 64] = [
        0xa0, 0xbf, 0x64, 0xac, 0x9b, 0xa7, 0xef, 0x17, 0xc1, 0x38, 0x85, 0x9c, 0x18, 0x78, 0xbb,
        0x99, 0xa8, 0x39, 0xfe, 0x17, 0x59, 0xac, 0xa5, 0xb0, 0xd7, 0x98, 0xcf, 0x1a, 0xb1, 0xe9,
        0x8d, 0x07, 0x91, 0x02, 0xf4, 0xdd, 0xb3, 0x36, 0x8f, 0x0f, 0xe4, 0x0b, 0xb3, 0x77, 0xf1,
        0xf0, 0x0e, 0x0c, 0xdd, 0xed, 0xb7, 0x99, 0x16, 0x7d, 0x56, 0xb6, 0xe9, 0x32, 0x78, 0x30,
        0x72, 0xba, 0x8d, 0x02,
    ];
    // MX 10 mail.example.com. → preference(2) ‖ canonical exchange name.
    let mut mx_rdata: Vec<u8> = alloc::vec![0x00, 0x0a];
    mx_rdata.extend_from_slice(&encode_name_canonical("mail.example.com"));
    let rrset = [CanonicalRr {
        rtype: 15,
        class: 1,
        rdata: mx_rdata,
    }];
    let mut rrsig = Rrsig {
        type_covered: 15,
        algorithm: 15,
        labels: 2,
        original_ttl: 3600,
        sig_expiration: 1_440_021_600,
        sig_inception: 1_438_207_200,
        key_tag: 3613,
        signer_name: String::from("example.com"),
        signature: DNSSEC_SIG.to_vec(),
    };
    let dnskey = Dnskey {
        flags: 257,
        protocol: 3,
        algorithm: 15,
        public_key: DNSSEC_PUBKEY.to_vec(),
    };
    check(
        verify_rrsig(&rrsig, &dnskey, "example.com", &rrset) == DnssecStatus::Secure,
        "dnssec-rfc8080-ed25519-secure",
    );
    // Tamper the signature → MUST be Bogus (never Secure).
    rrsig.signature[0] ^= 0xFF;
    check(
        verify_rrsig(&rrsig, &dnskey, "example.com", &rrset) == DnssecStatus::Bogus,
        "dnssec-tampered-sig-bogus",
    );
    rrsig.signature[0] ^= 0xFF;
    // Algorithm mismatch (DNSKEY says a different alg) → Bogus, not verified.
    let dnskey_wrong_alg = Dnskey {
        flags: 257,
        protocol: 3,
        algorithm: 13,
        public_key: DNSSEC_PUBKEY.to_vec(),
    };
    check(
        verify_rrsig(&rrsig, &dnskey_wrong_alg, "example.com", &rrset) == DnssecStatus::Bogus,
        "dnssec-alg-mismatch-bogus",
    );

    // ── DNSSEC ECDSA P-256 against the RFC 6605 §A.1 vector (algorithm 13) ──
    // A REAL published ECDSAP256SHA256 signature (example.net A, alg 13).
    const DNSSEC_P256_PK: [u8; 64] = [
        0x1a, 0x88, 0xc8, 0x86, 0x15, 0xd4, 0x37, 0xfb, 0xb8, 0xbf, 0x9e, 0x19, 0x42, 0xa1, 0x92,
        0x9f, 0x28, 0x56, 0x27, 0x06, 0xae, 0x6c, 0x2b, 0xd3, 0x99, 0xe7, 0xb1, 0xbf, 0xb6, 0xd1,
        0xe9, 0xe7, 0x5b, 0x92, 0xb4, 0xaa, 0x42, 0x91, 0x7a, 0xe1, 0xc6, 0x1b, 0x70, 0x1e, 0xf0,
        0x35, 0xc3, 0xfe, 0x7b, 0xe3, 0x00, 0x9c, 0xba, 0xfe, 0x5a, 0x2f, 0x71, 0x31, 0x6c, 0x90,
        0x2d, 0xcf, 0x0d, 0x00,
    ];
    const DNSSEC_P256_SIG: [u8; 64] = [
        0xab, 0x1e, 0xb0, 0x2d, 0x8a, 0xa6, 0x87, 0xe9, 0x7d, 0xa0, 0x22, 0x93, 0x37, 0xaa, 0x88,
        0x73, 0xe6, 0xf0, 0xeb, 0x26, 0xbe, 0x28, 0x9f, 0x28, 0x33, 0x3d, 0x18, 0x3f, 0x5d, 0x3b,
        0x7a, 0x95, 0xc0, 0xc8, 0x69, 0xad, 0xfb, 0x74, 0x8d, 0xae, 0xe3, 0xc5, 0x28, 0x6e, 0xed,
        0x66, 0x82, 0xc1, 0x2e, 0x55, 0x33, 0x18, 0x6b, 0xac, 0xed, 0x9c, 0x26, 0xc1, 0x67, 0xa9,
        0xeb, 0xae, 0x95, 0x0b,
    ];
    // A 192.0.2.1 → 4-byte rdata.
    let p256_rrset = [CanonicalRr {
        rtype: 1,
        class: 1,
        rdata: alloc::vec![192, 0, 2, 1],
    }];
    // RFC 6605 §6.1: the signed RRset is `www.example.net` A (3 labels), signed
    // by the `example.net` zone key.
    let mut p256_rrsig = Rrsig {
        type_covered: 1,
        algorithm: 13,
        labels: 3,
        original_ttl: 3600,
        sig_expiration: 1_284_026_679,
        sig_inception: 1_281_607_479,
        key_tag: 55648,
        signer_name: String::from("example.net"),
        signature: DNSSEC_P256_SIG.to_vec(),
    };
    let p256_dnskey = Dnskey {
        flags: 257,
        protocol: 3,
        algorithm: 13,
        public_key: DNSSEC_P256_PK.to_vec(),
    };
    check(
        verify_rrsig(&p256_rrsig, &p256_dnskey, "www.example.net", &p256_rrset)
            == DnssecStatus::Secure,
        "dnssec-rfc6605-p256-secure",
    );
    p256_rrsig.signature[0] ^= 0xFF;
    check(
        verify_rrsig(&p256_rrsig, &p256_dnskey, "www.example.net", &p256_rrset)
            == DnssecStatus::Bogus,
        "dnssec-p256-tampered-bogus",
    );

    // ── DNSSEC ECDSA P-384 against the RFC 6605 §6.2 vector (algorithm 14) ──
    // The REAL published ECDSAP384SHA384 signature: `www.example.net` A (3
    // labels), signed by the `example.net` zone key. The DNSKEY public key is
    // the raw 96-byte x‖y pair and the RRSIG is the raw 96-byte r‖s (RFC 6605
    // §4). This is the last DNSSEC algorithm the resolver could not verify;
    // wiring it closes DNSSEC to algorithm-complete. The vector was
    // cross-checked with the Python `cryptography` oracle
    // (`ec.ECDSA(hashes.SHA384())`) over the exact `dnssec_signed_data` bytes
    // before embedding.
    const DNSSEC_P384_PK: [u8; 96] = [
        0xc4, 0xa6, 0x1a, 0x36, 0x15, 0x9d, 0x18, 0xe7, 0xc9, 0xfa, 0x73, 0xeb, 0x2f, 0xcf, 0xda,
        0xae, 0x4c, 0x1f, 0xd8, 0x46, 0x37, 0x30, 0x32, 0x7e, 0x48, 0x4a, 0xca, 0x8a, 0xf0, 0x55,
        0x4a, 0xe9, 0xb5, 0xc3, 0xf7, 0xa0, 0xb1, 0x7b, 0xd2, 0x00, 0x3b, 0x4d, 0x26, 0x1c, 0x9e,
        0x9b, 0x94, 0x42, 0x3a, 0x98, 0x10, 0xe8, 0xaf, 0x17, 0xd4, 0x34, 0x52, 0x12, 0x4a, 0xdb,
        0x61, 0x0f, 0x8e, 0x07, 0xeb, 0xfc, 0xfe, 0xe5, 0xf8, 0xe4, 0xd0, 0x70, 0x63, 0xca, 0xe9,
        0xeb, 0x91, 0x7a, 0x1a, 0x5b, 0xab, 0xf0, 0x8f, 0xe6, 0x95, 0x53, 0x60, 0x17, 0xa5, 0xbf,
        0xa9, 0x32, 0x37, 0xee, 0x6e, 0x34,
    ];
    const DNSSEC_P384_SIG: [u8; 96] = [
        0xfc, 0xbe, 0x61, 0x0c, 0xa2, 0x2f, 0x18, 0x3c, 0x88, 0xd5, 0xf7, 0x00, 0x45, 0x7d, 0xf3,
        0xeb, 0x9a, 0xab, 0x98, 0xfb, 0x15, 0xcf, 0xbd, 0xd0, 0x0f, 0x53, 0x2b, 0xe4, 0x21, 0x2a,
        0x3a, 0x22, 0xcf, 0xf7, 0x98, 0x71, 0x42, 0x8b, 0xae, 0xae, 0x81, 0x82, 0x79, 0x93, 0xaf,
        0xcc, 0x56, 0xb1, 0xb1, 0x3f, 0x06, 0x96, 0xbe, 0xf8, 0x85, 0xb6, 0xaf, 0x44, 0xa6, 0xb2,
        0x24, 0xdb, 0xb2, 0x74, 0x2b, 0xb3, 0x59, 0x34, 0x92, 0x3d, 0xdc, 0xfb, 0xc2, 0x7a, 0x97,
        0x2f, 0x96, 0xdd, 0x70, 0x9c, 0xee, 0xb1, 0xd9, 0xc8, 0xd1, 0x14, 0x8c, 0x44, 0xec, 0x71,
        0xc0, 0x68, 0xa9, 0x59, 0xc2, 0x66,
    ];
    let p384_rrset = [CanonicalRr {
        rtype: 1,
        class: 1,
        rdata: alloc::vec![192, 0, 2, 1],
    }];
    let mut p384_rrsig = Rrsig {
        type_covered: 1,
        algorithm: 14,
        labels: 3,
        original_ttl: 3600,
        sig_expiration: 1_284_027_625,
        sig_inception: 1_281_608_425,
        key_tag: 10771,
        signer_name: String::from("example.net"),
        signature: DNSSEC_P384_SIG.to_vec(),
    };
    let p384_dnskey = Dnskey {
        flags: 257,
        protocol: 3,
        algorithm: 14,
        public_key: DNSSEC_P384_PK.to_vec(),
    };
    check(
        verify_rrsig(&p384_rrsig, &p384_dnskey, "www.example.net", &p384_rrset)
            == DnssecStatus::Secure,
        "dnssec-rfc6605-p384-secure",
    );
    p384_rrsig.signature[0] ^= 0xFF;
    check(
        verify_rrsig(&p384_rrsig, &p384_dnskey, "www.example.net", &p384_rrset)
            == DnssecStatus::Bogus,
        "dnssec-p384-tampered-bogus",
    );
    p384_rrsig.signature[0] ^= 0xFF;
    // A malformed alg-14 DNSKEY (wrong length for the raw x‖y pair) must
    // fail-closed to Bogus, never Secure and never a panic.
    let p384_dnskey_malformed = Dnskey {
        flags: 257,
        protocol: 3,
        algorithm: 14,
        public_key: alloc::vec![0xff; 40],
    };
    check(
        verify_rrsig(
            &p384_rrsig,
            &p384_dnskey_malformed,
            "www.example.net",
            &p384_rrset,
        ) == DnssecStatus::Bogus,
        "dnssec-p384-malformed-key-bogus",
    );

    // ── DNSSEC RSASHA256 (algorithm 8, RFC 5702 / RFC 3110), the most common
    //    real-world DNSSEC algorithm. The DNSKEY public key is RFC 3110
    //    `len(exp) ‖ exp ‖ modulus` (exponent 65537); the RRSIG is an
    //    RSASSA-PKCS1-v1_5-SHA256 signature over the RFC 4034 signed data for
    //    `www.example.com A 192.0.2.1`. The signature was produced OFFLINE with
    //    OpenSSL 3.5 over the exact bytes `dnssec_signed_data` builds and
    //    cross-checked with `openssl dgst -sha256 -verify` before embedding (an
    //    external oracle — the kernel cannot sign RSA). This exercises the
    //    alg-8 wiring end-to-end: RFC 3110 key parse → PKCS#1 v1.5 verify. ──
    const DNSSEC_RSA_PUBKEY: [u8; 260] = [
        0x03, 0x01, 0x00, 0x01, 0xca, 0xdf, 0x4e, 0x06, 0xae, 0x3c, 0x91, 0xef, 0x82, 0x67, 0x0c,
        0x26, 0x0f, 0xcf, 0xfd, 0xc7, 0xd9, 0x4f, 0xd0, 0xc8, 0x6f, 0x55, 0xb3, 0xc1, 0x5d, 0xd9,
        0x19, 0x8a, 0x79, 0x99, 0x04, 0x47, 0xa9, 0x08, 0xd6, 0xd6, 0x4a, 0x9a, 0xbe, 0xb0, 0x16,
        0x14, 0x4e, 0xdf, 0x6e, 0xda, 0x20, 0xee, 0x6b, 0xce, 0xf3, 0xd3, 0x93, 0x85, 0x74, 0x5d,
        0x99, 0x64, 0x0a, 0x05, 0xe4, 0xc6, 0x8e, 0xf8, 0xcf, 0x4f, 0x39, 0x33, 0xa7, 0x32, 0x55,
        0x77, 0xed, 0x7d, 0xec, 0x31, 0x54, 0x22, 0x7c, 0x8c, 0x73, 0x99, 0xf8, 0xd9, 0x1e, 0x26,
        0x93, 0x77, 0x1b, 0x76, 0x4f, 0xce, 0x29, 0xfc, 0xcb, 0xb3, 0xdf, 0x87, 0xef, 0xb9, 0x4b,
        0x90, 0x39, 0x11, 0xcb, 0x45, 0x9e, 0x8c, 0xa4, 0x37, 0x0e, 0x30, 0x0e, 0x2c, 0x6a, 0xde,
        0x3e, 0x4d, 0x37, 0x82, 0x67, 0x13, 0x31, 0xe6, 0x6c, 0xe9, 0x08, 0xcf, 0x0f, 0x56, 0x17,
        0x42, 0xe9, 0x59, 0x14, 0xc7, 0x17, 0x9d, 0xcf, 0x7a, 0x5c, 0x81, 0x9d, 0x48, 0xdf, 0xcf,
        0xcc, 0x5c, 0xa7, 0x1a, 0x7d, 0x93, 0x5c, 0x56, 0xd6, 0x0a, 0xf8, 0x5b, 0xf9, 0x01, 0x76,
        0x79, 0x42, 0x66, 0x79, 0xa3, 0x2f, 0x00, 0x42, 0x91, 0xd0, 0xb9, 0x52, 0xf1, 0xe4, 0xf4,
        0x88, 0xe0, 0x63, 0x91, 0x7d, 0x43, 0x1c, 0x5f, 0x5e, 0xdc, 0xb7, 0xad, 0xaa, 0xc6, 0xbb,
        0xc0, 0xc2, 0x30, 0x9e, 0x93, 0x0c, 0x0e, 0x1c, 0x4b, 0x2a, 0x90, 0x85, 0x6c, 0x4b, 0xa6,
        0x0e, 0x55, 0x60, 0x74, 0x37, 0xd5, 0x8a, 0x0e, 0x42, 0xb4, 0xd0, 0x35, 0x3a, 0x22, 0x69,
        0x3e, 0x7a, 0xfa, 0x5a, 0x7f, 0xb5, 0x6d, 0x78, 0x4e, 0x4b, 0x3b, 0x76, 0x31, 0x71, 0x36,
        0x6d, 0x29, 0xbf, 0x7a, 0xbc, 0x72, 0xe8, 0xa4, 0x47, 0x23, 0x0a, 0xcb, 0x1d, 0x8d, 0x85,
        0xf1, 0xca, 0x24, 0x5b, 0x57,
    ];
    const DNSSEC_RSA_SIG: [u8; 256] = [
        0x0e, 0x27, 0x61, 0x38, 0x8b, 0xa2, 0xb8, 0x07, 0xd6, 0x45, 0x86, 0xd6, 0x3e, 0x9e, 0xc6,
        0x18, 0x3a, 0x54, 0x44, 0x6e, 0xa6, 0x5c, 0x77, 0xb7, 0xe7, 0x16, 0xc4, 0x0c, 0x55, 0x9f,
        0x7a, 0xb3, 0xf9, 0xde, 0xd7, 0x13, 0xe3, 0x5b, 0x5e, 0xc2, 0x31, 0xe9, 0xa0, 0xaa, 0x76,
        0xf4, 0x91, 0x66, 0xe0, 0x79, 0x5d, 0x11, 0xbc, 0xd9, 0xad, 0xef, 0x5f, 0xf7, 0xa6, 0xc0,
        0xbb, 0xa1, 0x98, 0x0c, 0x96, 0x5a, 0x09, 0xce, 0x21, 0xab, 0x83, 0xbc, 0x30, 0xde, 0x8b,
        0xb4, 0xa3, 0xf9, 0xec, 0xda, 0xba, 0x32, 0x42, 0xd1, 0x47, 0xc4, 0x15, 0x12, 0x8c, 0xe6,
        0x40, 0x97, 0xdd, 0x6f, 0xab, 0x66, 0xc5, 0x7c, 0x98, 0xed, 0xa7, 0x20, 0xd9, 0x95, 0x17,
        0xb1, 0xc0, 0x70, 0xc2, 0xc8, 0x0e, 0x92, 0xd6, 0xe2, 0xc8, 0xf1, 0x9c, 0x33, 0xe1, 0xfb,
        0x02, 0x85, 0x17, 0x10, 0x03, 0x71, 0x83, 0x0c, 0x72, 0x91, 0xaf, 0x5f, 0xed, 0xa0, 0xd8,
        0xdb, 0xc3, 0xb0, 0x99, 0x88, 0x22, 0x11, 0x7c, 0xba, 0x28, 0x2a, 0x6b, 0xab, 0x88, 0xa9,
        0x2a, 0xe8, 0x68, 0x5f, 0xe2, 0x78, 0xda, 0x67, 0x6f, 0x5b, 0x8d, 0x53, 0x42, 0x30, 0x77,
        0x5a, 0x2d, 0x13, 0x85, 0xa1, 0xc6, 0x2b, 0xc2, 0x74, 0xed, 0x50, 0xe6, 0x21, 0x50, 0xeb,
        0xb4, 0x22, 0x54, 0x2a, 0x89, 0xd0, 0xb2, 0xbf, 0x39, 0x04, 0xe3, 0x20, 0xd3, 0x7c, 0x00,
        0x1f, 0x85, 0x40, 0x70, 0xbc, 0x9f, 0x5a, 0x51, 0x70, 0xed, 0xc0, 0x89, 0x54, 0xb7, 0xc3,
        0x6d, 0x89, 0x68, 0x12, 0xe0, 0x1c, 0x90, 0x80, 0xf9, 0x7e, 0x7a, 0x0c, 0x79, 0x6b, 0x8f,
        0xc0, 0xa3, 0x65, 0xa3, 0x75, 0x67, 0x9a, 0x54, 0x44, 0x31, 0x03, 0x03, 0x27, 0x8c, 0x3e,
        0x40, 0x5d, 0x0f, 0x17, 0xf8, 0x05, 0x00, 0xb1, 0x81, 0xac, 0x7f, 0xe5, 0x57, 0x8c, 0xb9,
        0x16,
    ];
    let rsa_rrset = [CanonicalRr {
        rtype: 1,
        class: 1,
        rdata: alloc::vec![192, 0, 2, 1],
    }];
    let mut rsa_rrsig = Rrsig {
        type_covered: 1,
        algorithm: 8,
        labels: 3,
        original_ttl: 3600,
        sig_expiration: 0x7fff_ffff,
        sig_inception: 0,
        key_tag: 36810,
        signer_name: String::from("example.com"),
        signature: DNSSEC_RSA_SIG.to_vec(),
    };
    let rsa_dnskey = Dnskey {
        flags: 256,
        protocol: 3,
        algorithm: 8,
        public_key: DNSSEC_RSA_PUBKEY.to_vec(),
    };
    check(
        verify_rrsig(&rsa_rrsig, &rsa_dnskey, "www.example.com", &rsa_rrset)
            == DnssecStatus::Secure,
        "dnssec-rfc5702-rsasha256-secure",
    );
    rsa_rrsig.signature[0] ^= 0xFF;
    check(
        verify_rrsig(&rsa_rrsig, &rsa_dnskey, "www.example.com", &rsa_rrset) == DnssecStatus::Bogus,
        "dnssec-rsasha256-tampered-bogus",
    );
    rsa_rrsig.signature[0] ^= 0xFF;
    // A malformed RFC 3110 key (exponent length runs past the buffer) must
    // fail-closed to Bogus, never Secure and never a panic.
    let rsa_dnskey_malformed = Dnskey {
        flags: 256,
        protocol: 3,
        algorithm: 8,
        public_key: alloc::vec![0xff, 0x01, 0x02, 0x03],
    };
    check(
        verify_rrsig(
            &rsa_rrsig,
            &rsa_dnskey_malformed,
            "www.example.com",
            &rsa_rrset,
        ) == DnssecStatus::Bogus,
        "dnssec-rsasha256-malformed-key-bogus",
    );

    // ── DNSSEC RSASHA512 (algorithm 10, RFC 5702 / RFC 3110). Identical DNSKEY
    //    wire format to alg 8 (`len(exp) ‖ exp ‖ modulus`, exponent 65537); only
    //    the signature hash is SHA-512. The RRSIG signs the RFC 4034 signed data
    //    for `www.example.com A 192.0.2.1`. The signature was produced OFFLINE
    //    with OpenSSL 3.5 (`openssl dgst -sha512 -sign` over the exact bytes
    //    `dnssec_signed_data` builds) and cross-checked with
    //    `openssl dgst -sha512 -verify` before embedding — an external oracle,
    //    not a self-signed round-trip. Exercises the alg-10 wiring end-to-end:
    //    RFC 3110 key parse → PKCS#1 v1.5 verify over the SHA-512 DigestInfo. ──
    const DNSSEC_RSA512_PUBKEY: [u8; 260] = [
        0x03, 0x01, 0x00, 0x01, 0xba, 0xa2, 0xb0, 0xfb, 0x82, 0xe1, 0x8e, 0x44, 0x83, 0x54, 0xdb,
        0x03, 0x92, 0x21, 0x18, 0xb9, 0x2e, 0xc7, 0x83, 0x4e, 0x81, 0x0d, 0xed, 0x90, 0x6d, 0x0d,
        0xdb, 0xf3, 0x47, 0xb6, 0xcd, 0x32, 0x2e, 0x7c, 0x1d, 0x4a, 0x9b, 0xf4, 0xed, 0x44, 0x29,
        0xb9, 0x6b, 0xca, 0x3e, 0xd3, 0x9d, 0x48, 0xcb, 0x76, 0xfb, 0x49, 0xf6, 0xc2, 0x1d, 0x75,
        0x4b, 0x07, 0x09, 0x9d, 0x1d, 0xd0, 0x87, 0x53, 0xd1, 0x91, 0x12, 0xbd, 0x0a, 0xe0, 0x8f,
        0x4a, 0x2b, 0x08, 0xfe, 0x9e, 0x10, 0x56, 0x1b, 0xe5, 0xf3, 0xc9, 0xb0, 0xf4, 0x9a, 0x21,
        0x02, 0x3b, 0x7c, 0x44, 0x1b, 0x7e, 0x35, 0x28, 0x04, 0x13, 0x31, 0x0a, 0x76, 0xc5, 0x9b,
        0x7f, 0xac, 0xc2, 0xef, 0xbf, 0x36, 0x6d, 0xf7, 0x50, 0xa8, 0xd5, 0x6e, 0x36, 0x96, 0x99,
        0x92, 0xf3, 0xa1, 0x29, 0x2b, 0x35, 0x72, 0x8c, 0xa4, 0x59, 0x5a, 0x91, 0x58, 0xec, 0xb2,
        0x5a, 0xa4, 0xd3, 0x85, 0x30, 0x10, 0x45, 0xbe, 0x78, 0xbf, 0xcb, 0x8a, 0xa0, 0x8b, 0x1d,
        0xff, 0x99, 0x02, 0x02, 0x1c, 0x08, 0x70, 0x3e, 0xc3, 0xa8, 0x1e, 0x21, 0x78, 0x8f, 0xf9,
        0x23, 0x4f, 0xfb, 0x23, 0xf0, 0xca, 0x36, 0x26, 0x13, 0xff, 0x4f, 0xb5, 0x23, 0x63, 0x88,
        0xdf, 0x13, 0x8c, 0x40, 0x97, 0x38, 0x49, 0xa3, 0xd1, 0x41, 0xfd, 0xbd, 0x42, 0x16, 0x0c,
        0x7c, 0xa0, 0xbb, 0x41, 0x38, 0x43, 0x1c, 0x17, 0x54, 0x75, 0x80, 0x21, 0x58, 0xc1, 0xed,
        0xdf, 0xa2, 0x7c, 0xb2, 0x49, 0x89, 0xbd, 0xaa, 0x03, 0xf9, 0xa6, 0xf3, 0xab, 0x59, 0x9b,
        0xd2, 0xb9, 0x44, 0xb3, 0x1b, 0x8f, 0x8c, 0x59, 0xec, 0x66, 0x84, 0x7b, 0x47, 0x0a, 0xf6,
        0x4a, 0xbe, 0xc4, 0x0c, 0xcb, 0xdd, 0x60, 0x4a, 0xfa, 0xb8, 0xc4, 0x25, 0x1b, 0xaf, 0x54,
        0x78, 0xf9, 0xae, 0xf5, 0x81,
    ];
    const DNSSEC_RSA512_SIG: [u8; 256] = [
        0xa6, 0x04, 0x68, 0xf4, 0x4d, 0x91, 0x8f, 0x69, 0x78, 0x4d, 0x54, 0x2f, 0xfd, 0x4c, 0xdc,
        0x29, 0xd6, 0x5d, 0x45, 0x47, 0x92, 0xe6, 0xb5, 0x79, 0x4e, 0x1f, 0x93, 0x28, 0xf1, 0xdb,
        0x56, 0x50, 0x41, 0x8b, 0xf3, 0x6e, 0xb8, 0x56, 0xc9, 0x2b, 0x7b, 0x5b, 0x07, 0x48, 0xb9,
        0x9b, 0x2f, 0x47, 0x20, 0x05, 0x7d, 0x00, 0xcd, 0x86, 0xe2, 0x06, 0xcc, 0x16, 0x7a, 0x85,
        0x7e, 0xf8, 0xb2, 0x6e, 0x3f, 0x62, 0x93, 0xba, 0xbe, 0x93, 0xc6, 0x4c, 0x17, 0xde, 0x54,
        0xeb, 0xff, 0xfa, 0x69, 0xff, 0xc1, 0x85, 0xe8, 0x88, 0x10, 0xd0, 0xd4, 0x09, 0x10, 0x0d,
        0x9e, 0x4b, 0xe0, 0xba, 0x4d, 0x90, 0x72, 0x19, 0xc3, 0xb5, 0xab, 0x88, 0x36, 0x41, 0x3b,
        0x83, 0xf0, 0x3a, 0xba, 0x95, 0x22, 0x93, 0x80, 0xb9, 0x77, 0xd4, 0x26, 0x59, 0x3a, 0xd4,
        0xbc, 0x12, 0x2a, 0xab, 0x78, 0x8f, 0xc2, 0xeb, 0x53, 0xe2, 0x29, 0x63, 0xe2, 0xa7, 0x08,
        0xcb, 0xe7, 0xfc, 0x0a, 0xcb, 0xa8, 0xcb, 0x12, 0xef, 0xf4, 0xa8, 0x9a, 0x31, 0x35, 0xab,
        0xf4, 0x87, 0x3d, 0xde, 0xe6, 0x80, 0xe6, 0xe4, 0x82, 0xf0, 0xbe, 0x11, 0x44, 0x10, 0x6a,
        0x48, 0x86, 0x1e, 0xb6, 0xc6, 0x45, 0xad, 0x49, 0x90, 0x1e, 0xdc, 0x6a, 0x18, 0x3b, 0x8a,
        0x30, 0xbe, 0x5b, 0x27, 0x77, 0xb0, 0xaa, 0x5a, 0x64, 0x6e, 0x5d, 0x02, 0x09, 0x87, 0x3e,
        0xd1, 0xfc, 0xeb, 0x38, 0x33, 0xed, 0xf5, 0x05, 0xe9, 0xf3, 0x50, 0xad, 0x04, 0x28, 0x0b,
        0x2d, 0x0b, 0x8a, 0x5c, 0xde, 0x70, 0x14, 0x0d, 0x0d, 0x03, 0xc8, 0x12, 0x1a, 0x68, 0x66,
        0x81, 0xf1, 0x56, 0xfa, 0xfe, 0xef, 0x0d, 0xcd, 0x3a, 0xde, 0x0f, 0x82, 0xea, 0x1e, 0xc4,
        0x52, 0x02, 0x5d, 0xe1, 0x28, 0x45, 0xb3, 0x97, 0xa2, 0x5b, 0xa4, 0x77, 0x05, 0xc2, 0xc8,
        0xdc,
    ];
    let rsa512_rrset = [CanonicalRr {
        rtype: 1,
        class: 1,
        rdata: alloc::vec![192, 0, 2, 1],
    }];
    let mut rsa512_rrsig = Rrsig {
        type_covered: 1,
        algorithm: 10,
        labels: 3,
        original_ttl: 3600,
        sig_expiration: 0x7fff_ffff,
        sig_inception: 0,
        key_tag: 36810,
        signer_name: String::from("example.com"),
        signature: DNSSEC_RSA512_SIG.to_vec(),
    };
    let rsa512_dnskey = Dnskey {
        flags: 256,
        protocol: 3,
        algorithm: 10,
        public_key: DNSSEC_RSA512_PUBKEY.to_vec(),
    };
    check(
        verify_rrsig(
            &rsa512_rrsig,
            &rsa512_dnskey,
            "www.example.com",
            &rsa512_rrset,
        ) == DnssecStatus::Secure,
        "dnssec-rfc5702-rsasha512-secure",
    );
    rsa512_rrsig.signature[0] ^= 0xFF;
    check(
        verify_rrsig(
            &rsa512_rrsig,
            &rsa512_dnskey,
            "www.example.com",
            &rsa512_rrset,
        ) == DnssecStatus::Bogus,
        "dnssec-rsasha512-tampered-bogus",
    );
    rsa512_rrsig.signature[0] ^= 0xFF;
    // A malformed RFC 3110 key (exponent length runs past the buffer) must
    // fail-closed to Bogus, never Secure and never a panic.
    let rsa512_dnskey_malformed = Dnskey {
        flags: 256,
        protocol: 3,
        algorithm: 10,
        public_key: alloc::vec![0xff, 0x01, 0x02, 0x03],
    };
    check(
        verify_rrsig(
            &rsa512_rrsig,
            &rsa512_dnskey_malformed,
            "www.example.com",
            &rsa512_rrset,
        ) == DnssecStatus::Bogus,
        "dnssec-rsasha512-malformed-key-bogus",
    );

    // ── DNSSEC chain-of-trust primitives: key tag (RFC 4034 App B) links an
    //    RRSIG/DS to its DNSKEY; DS digest (RFC 4509) is the parent→child link.
    //    The reference key tags are the RFC-published key IDs (3613, 55648). ──
    check(
        dnskey_key_tag(&dnskey) == 3613,
        "dnssec-keytag-ed25519-3613",
    );
    check(
        dnskey_key_tag(&p256_dnskey) == 55648,
        "dnssec-keytag-p256-55648",
    );
    // SHA-256 DS digest of the RFC 8080 Ed25519 DNSKEY at example.com. Oracle:
    // computed offline as SHA-256(canonical owner ‖ DNSKEY RDATA).
    const ED_DS_SHA256: [u8; 32] = [
        0x3a, 0xa5, 0xab, 0x37, 0xef, 0xce, 0x57, 0xf7, 0x37, 0xfc, 0x16, 0x27, 0x01, 0x3f, 0xee,
        0x07, 0xbd, 0xf2, 0x41, 0xbd, 0x10, 0xf3, 0xb1, 0x96, 0x4a, 0xb5, 0x5c, 0x78, 0xe7, 0x9a,
        0x30, 0x4b,
    ];
    let ds = Ds {
        key_tag: 3613,
        algorithm: 15,
        digest_type: 2,
        digest: ED_DS_SHA256.to_vec(),
    };
    check(
        verify_ds(&ds, &dnskey, "example.com"),
        "dnssec-ds-sha256-verify",
    );
    // A tampered DS digest must be rejected (the delegation link is broken).
    let mut ds_bad = Ds {
        key_tag: 3613,
        algorithm: 15,
        digest_type: 2,
        digest: ED_DS_SHA256.to_vec(),
    };
    ds_bad.digest[0] ^= 0xFF;
    check(
        !verify_ds(&ds_bad, &dnskey, "example.com"),
        "dnssec-ds-tamper-reject",
    );

    // ── DNSSEC wire-RDATA parsers (the layer validate_dnssec needs to pull
    //    RRSIG/DNSKEY/DS out of a response). Serialize the RFC 8080 RRSIG +
    //    DNSKEY to wire bytes, parse them back, and verify — proving the
    //    parsers feed the verifier correctly on the real signature. ──
    let mut rrsig_wire: Vec<u8> = Vec::new();
    rrsig_wire.extend_from_slice(&15u16.to_be_bytes()); // type covered = MX
    rrsig_wire.push(15); // algorithm = Ed25519
    rrsig_wire.push(2); // labels
    rrsig_wire.extend_from_slice(&3600u32.to_be_bytes());
    rrsig_wire.extend_from_slice(&1_440_021_600u32.to_be_bytes());
    rrsig_wire.extend_from_slice(&1_438_207_200u32.to_be_bytes());
    rrsig_wire.extend_from_slice(&3613u16.to_be_bytes());
    rrsig_wire.extend_from_slice(&encode_name_canonical("example.com"));
    rrsig_wire.extend_from_slice(&DNSSEC_SIG);
    let mut dnskey_wire: Vec<u8> = Vec::new();
    dnskey_wire.extend_from_slice(&257u16.to_be_bytes());
    dnskey_wire.push(3);
    dnskey_wire.push(15);
    dnskey_wire.extend_from_slice(&DNSSEC_PUBKEY);
    let wire_parse_ok = match (
        parse_rrsig_rdata(&rrsig_wire),
        parse_dnskey_rdata(&dnskey_wire),
    ) {
        (Some(rs), Some(dk)) => {
            verify_rrsig(&rs, &dk, "example.com", &rrset) == DnssecStatus::Secure
        }
        _ => false,
    };
    check(wire_parse_ok, "dnssec-wire-parse-verify");
    // Truncated RRSIG RDATA must parse to None (no panic on hostile input).
    check(
        parse_rrsig_rdata(&rrsig_wire[..10]).is_none(),
        "dnssec-rrsig-trunc-none",
    );
    // A DNSKEY RDATA with a compression byte in a place we never expect still
    // must not panic; a 4-byte-only record (no key) is None.
    check(
        parse_dnskey_rdata(&[0x01, 0x01, 0x03]).is_none(),
        "dnssec-dnskey-short-none",
    );
    // DS RDATA round-trips through the wire parser and verifies.
    let mut ds_wire: Vec<u8> = Vec::new();
    ds_wire.extend_from_slice(&3613u16.to_be_bytes());
    ds_wire.push(15);
    ds_wire.push(2);
    ds_wire.extend_from_slice(&ED_DS_SHA256);
    check(
        parse_ds_rdata(&ds_wire).map_or(false, |d| verify_ds(&d, &dnskey, "example.com")),
        "dnssec-ds-wire-parse-verify",
    );

    // ── DNSSEC chain of trust (RFC 4035 §5.3) — validate_chain_of_trust ──────
    //    The published RFC 8080/6605 vectors are single answer RRsets WITHOUT an
    //    RRSIG over their DNSKEY RRset (that would need the zone's private key,
    //    which the RFCs do not publish). So — exactly like the WireGuard
    //    loopback precedent — we build a minimal SELF-CONSISTENT chain here by
    //    generating a test KSK + ZSK with ath_crypto::ed25519 and signing the
    //    DNSKEY RRset (KSK) and an answer RRset (ZSK) ourselves. This is an
    //    INTERNAL-CONSISTENCY proof of the chain-walk logic (fail-closed
    //    verdicts, tamper detection); the cryptographic verify/DS primitives are
    //    separately proven above against the REAL published RFC signatures.
    let ct_owner = "athnet.test";
    let ksk_seed: [u8; 32] = [
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
        0x00, 0x0f, 0x1e, 0x2d, 0x3c, 0x4b, 0x5a, 0x69, 0x78, 0x87, 0x96, 0xa5, 0xb4, 0xc3, 0xd2,
        0xe1, 0xf0,
    ];
    let zsk_seed: [u8; 32] = [
        0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x07, 0x18, 0x29, 0x3a, 0x4b, 0x5c, 0x6d, 0x7e, 0x8f,
        0x90, 0x01, 0x12, 0x23, 0x34, 0x45, 0x56, 0x67, 0x78, 0x89, 0x9a, 0xab, 0xbc, 0xcd, 0xde,
        0xef, 0xf0,
    ];
    let ksk = Dnskey {
        flags: 257, // KSK: Zone Key + SEP
        protocol: 3,
        algorithm: 15, // Ed25519
        public_key: ath_crypto::ed25519::derive_public_key(&ksk_seed).to_vec(),
    };
    let zsk = Dnskey {
        flags: 256, // ZSK: Zone Key, no SEP
        protocol: 3,
        algorithm: 15,
        public_key: ath_crypto::ed25519::derive_public_key(&zsk_seed).to_vec(),
    };
    let ct_dnskeys = alloc::vec![ksk, zsk];
    let ksk_tag = dnskey_key_tag(&ct_dnskeys[0]);
    let zsk_tag = dnskey_key_tag(&ct_dnskeys[1]);

    // Trust anchor: the parent's DS committing to our KSK (RFC 4509 SHA-256).
    let mut ds_input = encode_name_canonical(ct_owner);
    ds_input.extend_from_slice(&dnskey_rdata(&ct_dnskeys[0]));
    let anchor = Ds {
        key_tag: ksk_tag,
        algorithm: 15,
        digest_type: 2,
        digest: ath_crypto::sha256::sha256(&ds_input).to_vec(),
    };

    // Validity window bracketing our test `now`.
    let ct_now: u32 = 1_700_000_000;
    let ct_inception: u32 = ct_now - 3600;
    let ct_expiration: u32 = ct_now + 3600;

    // RRSIG over the DNSKEY RRset, made by the KSK.
    let dnskey_canon: Vec<CanonicalRr> = ct_dnskeys
        .iter()
        .map(|k| CanonicalRr {
            rtype: 48,
            class: 1,
            rdata: dnskey_rdata(k),
        })
        .collect();
    let mut dnskey_sig = Rrsig {
        type_covered: 48,
        algorithm: 15,
        labels: 2,
        original_ttl: 3600,
        sig_expiration: ct_expiration,
        sig_inception: ct_inception,
        key_tag: ksk_tag,
        signer_name: String::from(ct_owner),
        signature: Vec::new(),
    };
    let dnskey_signed = dnssec_signed_data(&dnskey_sig, ct_owner, &dnskey_canon);
    dnskey_sig.signature = ath_crypto::ed25519::sign(&ksk_seed, &dnskey_signed).to_vec();

    // Answer RRset (A 203.0.113.5 at the apex) signed by the ZSK.
    let answer_rrset = [CanonicalRr {
        rtype: 1,
        class: 1,
        rdata: alloc::vec![203, 0, 113, 5],
    }];
    let mut answer_sig = Rrsig {
        type_covered: 1,
        algorithm: 15,
        labels: 2,
        original_ttl: 300,
        sig_expiration: ct_expiration,
        sig_inception: ct_inception,
        key_tag: zsk_tag,
        signer_name: String::from(ct_owner),
        signature: Vec::new(),
    };
    let answer_signed = dnssec_signed_data(&answer_sig, ct_owner, &answer_rrset);
    answer_sig.signature = ath_crypto::ed25519::sign(&zsk_seed, &answer_signed).to_vec();

    // 1. A genuine anchor-DS → DNSKEY-RRset → answer-RRset chain ⇒ Secure.
    check(
        validate_chain_of_trust(
            &anchor,
            &ct_dnskeys,
            &dnskey_sig,
            ct_owner,
            &answer_rrset,
            &answer_sig,
            ct_now,
        ) == DnssecStatus::Secure,
        "dnssec-chain-secure",
    );

    // 2. A tampered answer signature ⇒ Bogus (chain confirmed, leaf forged).
    let mut answer_sig_bad = Rrsig {
        type_covered: answer_sig.type_covered,
        algorithm: answer_sig.algorithm,
        labels: answer_sig.labels,
        original_ttl: answer_sig.original_ttl,
        sig_expiration: answer_sig.sig_expiration,
        sig_inception: answer_sig.sig_inception,
        key_tag: answer_sig.key_tag,
        signer_name: String::from(ct_owner),
        signature: answer_sig.signature.clone(),
    };
    answer_sig_bad.signature[0] ^= 0xFF;
    check(
        validate_chain_of_trust(
            &anchor,
            &ct_dnskeys,
            &dnskey_sig,
            ct_owner,
            &answer_rrset,
            &answer_sig_bad,
            ct_now,
        ) == DnssecStatus::Bogus,
        "dnssec-chain-tampered-answer-bogus",
    );

    // 3. A wrong/unmatched anchor DS (no KSK carries its tag) ⇒ Indeterminate
    //    (no path to the trust anchor — not a forgery, just undecidable).
    let anchor_nopath = Ds {
        key_tag: ksk_tag ^ 0x5A5A,
        algorithm: 15,
        digest_type: 2,
        digest: anchor.digest.clone(),
    };
    check(
        validate_chain_of_trust(
            &anchor_nopath,
            &ct_dnskeys,
            &dnskey_sig,
            ct_owner,
            &answer_rrset,
            &answer_sig,
            ct_now,
        ) == DnssecStatus::Indeterminate,
        "dnssec-chain-noanchor-indeterminate",
    );

    // 4a. An anchor DS whose tag matches the KSK but whose digest is corrupted
    //     (the DS no longer confirms the KSK) ⇒ Bogus (tamper on the link).
    let mut anchor_baddigest = Ds {
        key_tag: ksk_tag,
        algorithm: 15,
        digest_type: 2,
        digest: anchor.digest.clone(),
    };
    anchor_baddigest.digest[0] ^= 0xFF;
    check(
        validate_chain_of_trust(
            &anchor_baddigest,
            &ct_dnskeys,
            &dnskey_sig,
            ct_owner,
            &answer_rrset,
            &answer_sig,
            ct_now,
        ) == DnssecStatus::Bogus,
        "dnssec-chain-badds-bogus",
    );

    // 4b. A tampered DNSKEY RRset (flip a byte of the ZSK's public key): the KSK
    //     is still DS-confirmed, but the KSK's RRSIG no longer covers the
    //     modified DNSKEY RRset ⇒ Bogus (the trusted set has been forged).
    let mut ct_dnskeys_tampered = alloc::vec![
        Dnskey {
            flags: ct_dnskeys[0].flags,
            protocol: ct_dnskeys[0].protocol,
            algorithm: ct_dnskeys[0].algorithm,
            public_key: ct_dnskeys[0].public_key.clone(),
        },
        Dnskey {
            flags: ct_dnskeys[1].flags,
            protocol: ct_dnskeys[1].protocol,
            algorithm: ct_dnskeys[1].algorithm,
            public_key: ct_dnskeys[1].public_key.clone(),
        },
    ];
    ct_dnskeys_tampered[1].public_key[0] ^= 0xFF;
    check(
        validate_chain_of_trust(
            &anchor,
            &ct_dnskeys_tampered,
            &dnskey_sig,
            ct_owner,
            &answer_rrset,
            &answer_sig,
            ct_now,
        ) == DnssecStatus::Bogus,
        "dnssec-chain-tampered-dnskey-bogus",
    );

    // ── validate_dnssec end-to-end: WIRE BYTES → parser retains RRs → verdict ──
    //    The FULL path with zero network, the loopback precedent. Serialize the
    //    self-consistent chain above (anchor DS + signed DNSKEY RRset + signed A
    //    RRset) into a real DNS response packet, run it through the ACTUAL
    //    `parse_response` wire parser (which retains the raw RRSIG/DNSKEY RRs),
    //    then `validate_dnssec(&parsed, &anchor, ct_now)`.
    let owner_wire = {
        let mut b = Vec::new();
        encode_name(ct_owner, &mut b);
        b
    };
    // Serialize an RRSIG's RDATA (RFC 4034 §3.1) back to wire form.
    let rrsig_to_rdata = |rs: &Rrsig| -> Vec<u8> {
        let mut r = Vec::new();
        r.extend_from_slice(&rs.type_covered.to_be_bytes());
        r.push(rs.algorithm);
        r.push(rs.labels);
        r.extend_from_slice(&rs.original_ttl.to_be_bytes());
        r.extend_from_slice(&rs.sig_expiration.to_be_bytes());
        r.extend_from_slice(&rs.sig_inception.to_be_bytes());
        r.extend_from_slice(&rs.key_tag.to_be_bytes());
        r.extend_from_slice(&encode_name_canonical(&rs.signer_name));
        r.extend_from_slice(&rs.signature);
        r
    };
    // Append one resource record (owner ‖ type ‖ class IN ‖ ttl ‖ rdlen ‖ rdata).
    let push_rr = |buf: &mut Vec<u8>, owner: &[u8], rtype: u16, rdata: &[u8]| {
        buf.extend_from_slice(owner);
        buf.extend_from_slice(&rtype.to_be_bytes());
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.extend_from_slice(&300u32.to_be_bytes());
        buf.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        buf.extend_from_slice(rdata);
    };
    // Build a response: A + 2×DNSKEY, then the RRSIGs unless `strip_rrsig`, then
    // (optionally) an intentionally-truncated RRSIG RR. `tamper_sig` flips the
    // last byte of the answer RRSIG's signature in the wire.
    let build_signed_response = |strip_rrsig: bool, tamper_sig: bool, malformed: bool| -> Vec<u8> {
        let mut an: u16 = 3; // A + KSK + ZSK
        if !strip_rrsig {
            an += 2; // RRSIG(A) + RRSIG(DNSKEY)
        }
        if malformed {
            an += 1;
        }
        let hdr = DnsHeader {
            id: 0x4242,
            flags: 0x8180, // response, RD, RA, NoError
            qd_count: 1,
            an_count: an,
            ns_count: 0,
            ar_count: 0,
        };
        let mut msg = Vec::new();
        hdr.serialize(&mut msg);
        // Question: owner, qtype A, class IN.
        msg.extend_from_slice(&owner_wire);
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        // Answers.
        push_rr(&mut msg, &owner_wire, 1, &[203, 0, 113, 5]);
        push_rr(&mut msg, &owner_wire, 48, &dnskey_rdata(&ct_dnskeys[0]));
        push_rr(&mut msg, &owner_wire, 48, &dnskey_rdata(&ct_dnskeys[1]));
        if !strip_rrsig {
            let mut a_sig_rd = rrsig_to_rdata(&answer_sig);
            if tamper_sig {
                let n = a_sig_rd.len();
                a_sig_rd[n - 1] ^= 0xFF;
            }
            push_rr(&mut msg, &owner_wire, 46, &a_sig_rd);
            push_rr(&mut msg, &owner_wire, 46, &rrsig_to_rdata(&dnskey_sig));
        }
        if malformed {
            // rdlength=4 (< the 18-byte RRSIG minimum) → parser skips it safely.
            push_rr(&mut msg, &owner_wire, 46, &[0x00, 0x01, 0x02, 0x03]);
        }
        msg
    };

    let ct_resolver = DnsResolver::new();

    // 5a. Genuine signed response → Secure (full wire → parse → validate).
    let good_wire = build_signed_response(false, false, false);
    let good_parsed = ct_resolver.parse_response(&good_wire);
    check(
        good_parsed
            .as_ref()
            .map(|r| ct_resolver.validate_dnssec(r, &anchor, ct_now))
            == Some(DnssecStatus::Secure),
        "dnssec-validate-wire-secure",
    );
    // Parser must also have retained exactly the DNSSEC RRs (2 DNSKEY, 2 RRSIG).
    check(
        good_parsed
            .as_ref()
            .map(|r| r.dnskeys.len() == 2 && r.rrsigs.len() == 2 && r.answer_canonical.len() == 1)
            .unwrap_or(false),
        "dnssec-validate-wire-retained-rrs",
    );

    // 5b. One flipped signature byte → parses fine, verdict Bogus (never Secure).
    let bad_wire = build_signed_response(false, true, false);
    let bad_status = ct_resolver
        .parse_response(&bad_wire)
        .map(|r| ct_resolver.validate_dnssec(&r, &anchor, ct_now));
    check(
        bad_status == Some(DnssecStatus::Bogus),
        "dnssec-validate-wire-bogus",
    );

    // 5c. RRSIG RRs stripped (unsigned response) → Indeterminate (fail-closed).
    let unsigned_wire = build_signed_response(true, false, false);
    let unsigned_status = ct_resolver
        .parse_response(&unsigned_wire)
        .map(|r| ct_resolver.validate_dnssec(&r, &anchor, ct_now));
    check(
        unsigned_status == Some(DnssecStatus::Indeterminate),
        "dnssec-validate-wire-unsigned-indeterminate",
    );

    // 5d. A truncated/malformed DNSSEC RR in the buffer (with no valid RRSIGs):
    //     the parser must SKIP it without panicking, the whole response still
    //     parses, and the verdict is Indeterminate (never Secure, never a crash).
    let malformed_wire = build_signed_response(true, false, true);
    let malformed_parsed = ct_resolver.parse_response(&malformed_wire);
    check(
        malformed_parsed.is_some(),
        "dnssec-validate-wire-malformed-parses",
    );
    let malformed_status = malformed_parsed
        .as_ref()
        .map(|r| ct_resolver.validate_dnssec(r, &anchor, ct_now));
    check(
        malformed_status == Some(DnssecStatus::Indeterminate),
        "dnssec-validate-wire-malformed-indeterminate",
    );
    // And the malformed RRSIG must NOT have been retained (skip-not-fail).
    check(
        malformed_parsed
            .as_ref()
            .map(|r| r.rrsigs.is_empty())
            .unwrap_or(false),
        "dnssec-validate-wire-malformed-skipped",
    );

    // ── DNSSEC recursive multi-zone delegation walk — validate_with_delegation ──
    //    Build a 3-zone hierarchy IN THE TEST — root "." , "com.", "example.com."
    //    — each with its own KSK + ZSK (ath_crypto::ed25519). Each zone signs its
    //    DNSKEY RRset with its KSK; each PARENT publishes a signed DS committing
    //    to its child's KSK (ath_crypto::sha256), signed by the parent's ZSK; and
    //    example.com signs an A answer at www.example.com with its ZSK. A
    //    `MockFetcher` returns these signed records so the WALK logic is proven
    //    end-to-end with ZERO network (the live UDP `DnssecFetcher` is the tail).
    let dw_now: u32 = 1_700_000_000;
    let dw_inception: u32 = dw_now - 3600;
    let dw_expiration: u32 = dw_now + 3600;

    // Make an Ed25519 DNSKEY (algorithm 15) from a fixed test seed.
    let mk_key = |seed_byte: u8, flags: u16| -> ([u8; 32], Dnskey) {
        let seed = [seed_byte; 32];
        let dk = Dnskey {
            flags,
            protocol: 3,
            algorithm: 15,
            public_key: ath_crypto::ed25519::derive_public_key(&seed).to_vec(),
        };
        (seed, dk)
    };
    // Sign an RRset: reconstruct the RFC 4034 §3.1.8.1 signed data and Ed25519-sign
    // it, producing an RRSIG whose signer name is the zone `signer`.
    let sign = |seed: &[u8; 32],
                key_tag: u16,
                type_covered: u16,
                owner: &str,
                signer: &str,
                rrset: &[CanonicalRr]|
     -> Rrsig {
        let mut sig = Rrsig {
            type_covered,
            algorithm: 15,
            labels: owner
                .trim_end_matches('.')
                .split('.')
                .filter(|l| !l.is_empty())
                .count() as u8,
            original_ttl: 3600,
            sig_expiration: dw_expiration,
            sig_inception: dw_inception,
            key_tag,
            signer_name: String::from(signer),
            signature: Vec::new(),
        };
        let data = dnssec_signed_data(&sig, owner, rrset);
        sig.signature = ath_crypto::ed25519::sign(seed, &data).to_vec();
        sig
    };
    // Parent's DS for a child: SHA-256(canonical child zone ‖ child KSK RDATA).
    let make_ds = |child_zone: &str, child_ksk: &Dnskey| -> Ds {
        let mut input = encode_name_canonical(child_zone);
        input.extend_from_slice(&dnskey_rdata(child_ksk));
        Ds {
            key_tag: dnskey_key_tag(child_ksk),
            algorithm: 15,
            digest_type: 2,
            digest: ath_crypto::sha256::sha256(&input).to_vec(),
        }
    };
    let dnskey_canon = |keys: &[Dnskey]| -> Vec<CanonicalRr> {
        keys.iter()
            .map(|k| CanonicalRr {
                rtype: 48,
                class: 1,
                rdata: dnskey_rdata(k),
            })
            .collect()
    };

    let (root_ksk_seed, root_ksk) = mk_key(0x21, 257);
    let (root_zsk_seed, root_zsk) = mk_key(0x22, 256);
    let (com_ksk_seed, com_ksk) = mk_key(0x23, 257);
    let (com_zsk_seed, com_zsk) = mk_key(0x24, 256);
    let (ex_ksk_seed, ex_ksk) = mk_key(0x25, 257);
    let (ex_zsk_seed, ex_zsk) = mk_key(0x26, 256);

    // Per-zone DNSKEY RRsets + the RRSIG the KSK makes over each.
    let root_dnskeys = alloc::vec![root_ksk.clone(), root_zsk.clone()];
    let root_dnskey_rrsig = sign(
        &root_ksk_seed,
        dnskey_key_tag(&root_ksk),
        48,
        "",
        "",
        &dnskey_canon(&root_dnskeys),
    );
    let com_dnskeys = alloc::vec![com_ksk.clone(), com_zsk.clone()];
    let com_dnskey_rrsig = sign(
        &com_ksk_seed,
        dnskey_key_tag(&com_ksk),
        48,
        "com",
        "com",
        &dnskey_canon(&com_dnskeys),
    );
    let ex_dnskeys = alloc::vec![ex_ksk.clone(), ex_zsk.clone()];
    let ex_dnskey_rrsig = sign(
        &ex_ksk_seed,
        dnskey_key_tag(&ex_ksk),
        48,
        "example.com",
        "example.com",
        &dnskey_canon(&ex_dnskeys),
    );

    // Parent DS RRsets (owned by the child, signed by the parent's ZSK).
    let ds_com = alloc::vec![make_ds("com", &com_ksk)];
    let ds_com_canon: Vec<CanonicalRr> = ds_com
        .iter()
        .map(|d| CanonicalRr {
            rtype: 43,
            class: 1,
            rdata: ds_rdata(d),
        })
        .collect();
    let ds_com_rrsig = sign(
        &root_zsk_seed,
        dnskey_key_tag(&root_zsk),
        43,
        "com",
        "",
        &ds_com_canon,
    );
    let ds_ex = alloc::vec![make_ds("example.com", &ex_ksk)];
    let ds_ex_canon: Vec<CanonicalRr> = ds_ex
        .iter()
        .map(|d| CanonicalRr {
            rtype: 43,
            class: 1,
            rdata: ds_rdata(d),
        })
        .collect();
    let ds_ex_rrsig = sign(
        &com_zsk_seed,
        dnskey_key_tag(&com_zsk),
        43,
        "example.com",
        "com",
        &ds_ex_canon,
    );

    // The root trust anchor (a DS committing to the root KSK) and the leaf answer.
    let dw_root_anchor = make_ds("", &root_ksk);
    let dw_owner = "www.example.com";
    let dw_answer = alloc::vec![CanonicalRr {
        rtype: 1,
        class: 1,
        rdata: alloc::vec![203, 0, 113, 5],
    }];
    let dw_answer_rrsig = sign(
        &ex_zsk_seed,
        dnskey_key_tag(&ex_zsk),
        1,
        dw_owner,
        "example.com",
        &dw_answer,
    );

    // A fetcher over the built hierarchy — the network-abstracted seam.
    struct MockZone {
        zone: String,
        dnskeys: Vec<Dnskey>,
        rrsig: Rrsig,
    }
    struct MockDs {
        parent: String,
        child: String,
        ds: Vec<Ds>,
        rrsig: Rrsig,
    }
    struct MockFetcher {
        zones: Vec<MockZone>,
        dss: Vec<MockDs>,
        /// A zone whose DNSKEY fetch returns None (simulates a missing hop).
        missing: Option<String>,
    }
    impl DnssecFetcher for MockFetcher {
        fn fetch_dnskeys(&self, zone: &str) -> Option<(Vec<Dnskey>, Rrsig)> {
            if self.missing.as_deref() == Some(zone) {
                return None;
            }
            self.zones
                .iter()
                .find(|z| z.zone == zone)
                .map(|z| (z.dnskeys.clone(), z.rrsig.clone()))
        }
        fn fetch_ds(&self, parent: &str, child: &str) -> Option<(Vec<Ds>, Rrsig)> {
            self.dss
                .iter()
                .find(|d| d.parent == parent && d.child == child)
                .map(|d| (d.ds.clone(), d.rrsig.clone()))
        }
    }
    let build_mock = || MockFetcher {
        zones: alloc::vec![
            MockZone {
                zone: String::new(),
                dnskeys: root_dnskeys.clone(),
                rrsig: root_dnskey_rrsig.clone(),
            },
            MockZone {
                zone: String::from("com"),
                dnskeys: com_dnskeys.clone(),
                rrsig: com_dnskey_rrsig.clone(),
            },
            MockZone {
                zone: String::from("example.com"),
                dnskeys: ex_dnskeys.clone(),
                rrsig: ex_dnskey_rrsig.clone(),
            },
        ],
        dss: alloc::vec![
            MockDs {
                parent: String::new(),
                child: String::from("com"),
                ds: ds_com.clone(),
                rrsig: ds_com_rrsig.clone(),
            },
            MockDs {
                parent: String::from("com"),
                child: String::from("example.com"),
                ds: ds_ex.clone(),
                rrsig: ds_ex_rrsig.clone(),
            },
        ],
        missing: None,
    };

    // 1. Full genuine chain root → com → example.com → answer ⇒ Secure.
    let mock_good = build_mock();
    check(
        validate_with_delegation(
            &dw_root_anchor,
            &mock_good,
            dw_owner,
            1,
            &dw_answer,
            &dw_answer_rrsig,
            dw_now,
        ) == DnssecStatus::Secure,
        "dnssec-delegation-secure",
    );

    // 2. Break the com → example.com DS signature ⇒ Bogus (mid-chain forgery).
    let mut mock_ds = build_mock();
    mock_ds.dss[1].rrsig.signature[0] ^= 0xFF;
    check(
        validate_with_delegation(
            &dw_root_anchor,
            &mock_ds,
            dw_owner,
            1,
            &dw_answer,
            &dw_answer_rrsig,
            dw_now,
        ) == DnssecStatus::Bogus,
        "dnssec-delegation-broken-ds-bogus",
    );

    // 3. Break a mid-chain DNSKEY RRSIG (com's self-signature) ⇒ Bogus.
    let mut mock_dk = build_mock();
    mock_dk.zones[1].rrsig.signature[0] ^= 0xFF;
    check(
        validate_with_delegation(
            &dw_root_anchor,
            &mock_dk,
            dw_owner,
            1,
            &dw_answer,
            &dw_answer_rrsig,
            dw_now,
        ) == DnssecStatus::Bogus,
        "dnssec-delegation-broken-dnskey-bogus",
    );

    // 4. Fetcher returns None for one hop (com) ⇒ Indeterminate (no path).
    let mut mock_miss = build_mock();
    mock_miss.missing = Some(String::from("com"));
    check(
        validate_with_delegation(
            &dw_root_anchor,
            &mock_miss,
            dw_owner,
            1,
            &dw_answer,
            &dw_answer_rrsig,
            dw_now,
        ) == DnssecStatus::Indeterminate,
        "dnssec-delegation-missing-hop-indeterminate",
    );

    // 5. Tamper the final answer signature ⇒ Bogus (leaf forgery, chain intact).
    let mut dw_answer_rrsig_bad = dw_answer_rrsig.clone();
    dw_answer_rrsig_bad.signature[0] ^= 0xFF;
    let mock_ans = build_mock();
    check(
        validate_with_delegation(
            &dw_root_anchor,
            &mock_ans,
            dw_owner,
            1,
            &dw_answer,
            &dw_answer_rrsig_bad,
            dw_now,
        ) == DnssecStatus::Bogus,
        "dnssec-delegation-tampered-answer-bogus",
    );

    drop(check);
    crate::serial_println!(
        "[ OK ] DNS selftest: {}/{} checks passed (codec + resolver + resolve_blocking glue + DNSSEC delegation walk, no network)",
        pass,
        total
    );
    if pass != total {
        crate::serial_println!("[FAIL] DNS selftest: {} check(s) failed", total - pass);
    }
}

/// `/proc/athena/dns` — global resolver upstream servers and static hosts.
/// MasterChecklist Phase 10.
pub fn dump_text() -> String {
    let guard = DNS_RESOLVER.lock();
    let mut out = String::new();
    out.push_str("# AthNet DNS resolver\n");
    match *guard {
        Some(ref r) => {
            out.push_str(&alloc::format!("upstream_servers: {}\n", r.server_count()));
            for s in r.servers() {
                out.push_str(&alloc::format!(
                    "  server: {}.{}.{}.{}\n",
                    s[0],
                    s[1],
                    s[2],
                    s[3]
                ));
            }
            let hosts = r.static_hosts();
            out.push_str(&alloc::format!("static_hosts: {}\n", hosts.len()));
            for h in hosts {
                let a = h.address;
                out.push_str(&alloc::format!(
                    "  {} -> {}.{}.{}.{}\n",
                    h.hostname,
                    a[0],
                    a[1],
                    a[2],
                    a[3]
                ));
            }
        }
        None => out.push_str("status: not initialized\n"),
    }
    out
}
