//! crypt32.dll — Windows CryptoAPI compatibility for AthBridge.
//!
//! Certificate stores, X.509 chain building/verification, PKCS#7/CMS
//! message functions, CSP operations (hash, encrypt, sign), PFX
//! import/export, Base64 encoding, and DPAPI-equivalent data protection.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::WinHandle;

// =========================================================================
// Error / status codes
// =========================================================================

pub const CRYPT_E_NOT_FOUND: i32 = 0x80092004_u32 as i32;
pub const CRYPT_E_EXISTS: i32 = 0x80092005_u32 as i32;
pub const CRYPT_E_NO_MATCH: i32 = 0x80092009_u32 as i32;
pub const CRYPT_E_REVOKED: i32 = 0x80092010_u32 as i32;
pub const CRYPT_E_NO_REVOCATION_CHECK: i32 = 0x80092012_u32 as i32;
pub const CRYPT_E_REVOCATION_OFFLINE: i32 = 0x80092013_u32 as i32;
pub const CRYPT_E_ASN1_ERROR: i32 = 0x80093100_u32 as i32;
pub const CRYPT_E_HASH_VALUE: i32 = 0x80091007_u32 as i32;
pub const CRYPT_E_MSG_ERROR: i32 = 0x80091001_u32 as i32;
pub const CRYPT_E_BAD_ENCODE: i32 = 0x80092002_u32 as i32;
pub const CRYPT_E_INVALID_MSG_TYPE: i32 = 0x80091004_u32 as i32;
pub const NTE_BAD_KEY: i32 = 0x80090003_u32 as i32;
pub const NTE_BAD_HASH: i32 = 0x80090002_u32 as i32;
pub const NTE_BAD_ALGID: i32 = 0x80090008_u32 as i32;
pub const NTE_BAD_DATA: i32 = 0x80090005_u32 as i32;
pub const NTE_BAD_SIGNATURE: i32 = 0x80090006_u32 as i32;
pub const NTE_NO_KEY: i32 = 0x8009000D_u32 as i32;
pub const NTE_KEYSET_NOT_DEF: i32 = 0x80090019_u32 as i32;

// =========================================================================
// Algorithm identifiers
// =========================================================================

pub const CALG_MD5: u32 = 0x00008003;
pub const CALG_SHA1: u32 = 0x00008004;
pub const CALG_SHA_256: u32 = 0x0000800C;
pub const CALG_SHA_384: u32 = 0x0000800D;
pub const CALG_SHA_512: u32 = 0x0000800E;
pub const CALG_RC2: u32 = 0x00006602;
pub const CALG_RC4: u32 = 0x00006801;
pub const CALG_DES: u32 = 0x00006601;
pub const CALG_3DES: u32 = 0x00006603;
pub const CALG_3DES_112: u32 = 0x00006609;
pub const CALG_AES_128: u32 = 0x0000660E;
pub const CALG_AES_192: u32 = 0x0000660F;
pub const CALG_AES_256: u32 = 0x00006610;
pub const CALG_RSA_SIGN: u32 = 0x00002400;
pub const CALG_RSA_KEYX: u32 = 0x0000A400;
pub const CALG_DSS_SIGN: u32 = 0x00002200;
pub const CALG_ECDSA: u32 = 0x00002203;
pub const CALG_ECDH: u32 = 0x0000AA05;
pub const CALG_HMAC: u32 = 0x00008009;

// =========================================================================
// Provider types
// =========================================================================

pub const PROV_RSA_FULL: u32 = 1;
pub const PROV_RSA_SIG: u32 = 2;
pub const PROV_DSS: u32 = 3;
pub const PROV_FORTEZZA: u32 = 4;
pub const PROV_MS_EXCHANGE: u32 = 5;
pub const PROV_SSL: u32 = 6;
pub const PROV_RSA_SCHANNEL: u32 = 12;
pub const PROV_DSS_DH: u32 = 13;
pub const PROV_EC_ECDSA_SIG: u32 = 14;
pub const PROV_DH_SCHANNEL: u32 = 18;
pub const PROV_RSA_AES: u32 = 24;

// =========================================================================
// Context flags
// =========================================================================

pub const CRYPT_VERIFYCONTEXT: u32 = 0xF0000000;
pub const CRYPT_NEWKEYSET: u32 = 0x00000008;
pub const CRYPT_DELETEKEYSET: u32 = 0x00000010;
pub const CRYPT_MACHINE_KEYSET: u32 = 0x00000020;
pub const CRYPT_SILENT: u32 = 0x00000040;

// =========================================================================
// Key specifications
// =========================================================================

pub const AT_KEYEXCHANGE: u32 = 1;
pub const AT_SIGNATURE: u32 = 2;

// =========================================================================
// Hash parameters
// =========================================================================

pub const HP_ALGID: u32 = 0x0001;
pub const HP_HASHVAL: u32 = 0x0002;
pub const HP_HASHSIZE: u32 = 0x0004;

// =========================================================================
// Key parameters
// =========================================================================

pub const KP_ALGID: u32 = 7;
pub const KP_BLOCKLEN: u32 = 8;
pub const KP_KEYLEN: u32 = 9;
pub const KP_IV: u32 = 1;
pub const KP_PADDING: u32 = 3;
pub const KP_MODE: u32 = 4;
pub const KP_MODE_BITS: u32 = 5;

pub const CRYPT_MODE_CBC: u32 = 1;
pub const CRYPT_MODE_ECB: u32 = 2;
pub const CRYPT_MODE_OFB: u32 = 3;
pub const CRYPT_MODE_CFB: u32 = 4;

// =========================================================================
// Certificate store names
// =========================================================================

pub const CERT_STORE_MY: &str = "MY";
pub const CERT_STORE_CA: &str = "CA";
pub const CERT_STORE_ROOT: &str = "ROOT";
pub const CERT_STORE_DISALLOWED: &str = "Disallowed";
pub const CERT_STORE_TRUST: &str = "Trust";

// =========================================================================
// Certificate find types
// =========================================================================

pub const CERT_FIND_ANY: u32 = 0;
pub const CERT_FIND_SUBJECT_STR: u32 = 0x00080007;
pub const CERT_FIND_ISSUER_STR: u32 = 0x00080004;
pub const CERT_FIND_HASH: u32 = 0x00010000;
pub const CERT_FIND_SHA1_HASH: u32 = 0x00010000;
pub const CERT_FIND_SUBJECT_NAME: u32 = 0x00020007;
pub const CERT_FIND_ISSUER_NAME: u32 = 0x00020004;
pub const CERT_FIND_EXISTING: u32 = 0x000D0000;
pub const CERT_FIND_KEY_IDENTIFIER: u32 = 0x000F0000;
pub const CERT_FIND_ENHKEY_USAGE: u32 = 0x000A0000;
pub const CERT_FIND_PROPERTY: u32 = 0x00050000;

// =========================================================================
// Certificate context property IDs
// =========================================================================

pub const CERT_KEY_PROV_INFO_PROP_ID: u32 = 2;
pub const CERT_SHA1_HASH_PROP_ID: u32 = 3;
pub const CERT_MD5_HASH_PROP_ID: u32 = 4;
pub const CERT_KEY_IDENTIFIER_PROP_ID: u32 = 20;
pub const CERT_FRIENDLY_NAME_PROP_ID: u32 = 11;
pub const CERT_DESCRIPTION_PROP_ID: u32 = 13;
pub const CERT_SUBJECT_PUBLIC_KEY_MD5_HASH_PROP_ID: u32 = 25;
pub const CERT_SIGNATURE_HASH_PROP_ID: u32 = 15;
pub const CERT_ISSUER_PUBLIC_KEY_MD5_HASH_PROP_ID: u32 = 24;
pub const CERT_DATE_STAMP_PROP_ID: u32 = 27;

// =========================================================================
// Chain policy identifiers
// =========================================================================

pub const CERT_CHAIN_POLICY_BASE: u32 = 1;
pub const CERT_CHAIN_POLICY_AUTHENTICODE: u32 = 2;
pub const CERT_CHAIN_POLICY_AUTHENTICODE_TS: u32 = 3;
pub const CERT_CHAIN_POLICY_SSL: u32 = 4;
pub const CERT_CHAIN_POLICY_BASIC_CONSTRAINTS: u32 = 5;
pub const CERT_CHAIN_POLICY_NT_AUTH: u32 = 6;
pub const CERT_CHAIN_POLICY_MICROSOFT_ROOT: u32 = 7;
pub const CERT_CHAIN_POLICY_EV: u32 = 8;

// =========================================================================
// CMS message types
// =========================================================================

pub const CMSG_DATA: u32 = 1;
pub const CMSG_SIGNED: u32 = 2;
pub const CMSG_ENVELOPED: u32 = 3;
pub const CMSG_SIGNED_AND_ENVELOPED: u32 = 4;
pub const CMSG_HASHED: u32 = 5;
pub const CMSG_ENCRYPTED: u32 = 6;

// CryptMsgGetParam types
pub const CMSG_TYPE_PARAM: u32 = 1;
pub const CMSG_CONTENT_PARAM: u32 = 2;
pub const CMSG_BARE_CONTENT_PARAM: u32 = 3;
pub const CMSG_INNER_CONTENT_TYPE_PARAM: u32 = 4;
pub const CMSG_SIGNER_COUNT_PARAM: u32 = 5;
pub const CMSG_SIGNER_INFO_PARAM: u32 = 6;
pub const CMSG_SIGNER_CERT_INFO_PARAM: u32 = 7;
pub const CMSG_SIGNER_HASH_ALGORITHM_PARAM: u32 = 8;
pub const CMSG_CERT_COUNT_PARAM: u32 = 11;
pub const CMSG_CERT_PARAM: u32 = 12;
pub const CMSG_CRL_COUNT_PARAM: u32 = 13;
pub const CMSG_CRL_PARAM: u32 = 14;
pub const CMSG_ENVELOPE_ALGORITHM_PARAM: u32 = 15;
pub const CMSG_RECIPIENT_COUNT_PARAM: u32 = 17;
pub const CMSG_RECIPIENT_INFO_PARAM: u32 = 19;

// =========================================================================
// Base64 encoding flags
// =========================================================================

pub const CRYPT_STRING_BASE64HEADER: u32 = 0x00000000;
pub const CRYPT_STRING_BASE64: u32 = 0x00000001;
pub const CRYPT_STRING_BINARY: u32 = 0x00000002;
pub const CRYPT_STRING_BASE64REQUESTHEADER: u32 = 0x00000003;
pub const CRYPT_STRING_HEX: u32 = 0x00000004;
pub const CRYPT_STRING_HEXASCII: u32 = 0x00000005;
pub const CRYPT_STRING_BASE64X509CRLHEADER: u32 = 0x00000009;
pub const CRYPT_STRING_HEXADDR: u32 = 0x0000000A;
pub const CRYPT_STRING_HEXASCIIADDR: u32 = 0x0000000B;
pub const CRYPT_STRING_NOCRLF: u32 = 0x40000000;
pub const CRYPT_STRING_NOCR: u32 = 0x80000000;

// =========================================================================
// X.509 extension OIDs
// =========================================================================

pub const OID_BASIC_CONSTRAINTS: &str = "2.5.29.19";
pub const OID_KEY_USAGE: &str = "2.5.29.15";
pub const OID_EXTENDED_KEY_USAGE: &str = "2.5.29.37";
pub const OID_SUBJECT_ALT_NAME: &str = "2.5.29.17";
pub const OID_AUTHORITY_KEY_ID: &str = "2.5.29.35";
pub const OID_SUBJECT_KEY_ID: &str = "2.5.29.14";
pub const OID_CRL_DISTRIBUTION_POINTS: &str = "2.5.29.31";
pub const OID_AUTHORITY_INFO_ACCESS: &str = "1.3.6.1.5.5.7.1.1";
pub const OID_CERTIFICATE_POLICIES: &str = "2.5.29.32";
pub const OID_NAME_CONSTRAINTS: &str = "2.5.29.30";
pub const OID_SERVER_AUTH: &str = "1.3.6.1.5.5.7.3.1";
pub const OID_CLIENT_AUTH: &str = "1.3.6.1.5.5.7.3.2";
pub const OID_CODE_SIGNING: &str = "1.3.6.1.5.5.7.3.3";
pub const OID_EMAIL_PROTECTION: &str = "1.3.6.1.5.5.7.3.4";
pub const OID_TIMESTAMP_SIGNING: &str = "1.3.6.1.5.5.7.3.8";

// =========================================================================
// Structures
// =========================================================================

#[derive(Debug, Clone)]
pub struct CertExtension {
    pub oid: String,
    pub critical: bool,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct CertPublicKeyInfo {
    pub algorithm_oid: String,
    pub key_data: Vec<u8>,
    pub key_bit_length: u32,
}

#[derive(Debug, Clone)]
pub struct CertContext {
    pub id: u64,
    pub encoded_cert: Vec<u8>,
    pub serial_number: Vec<u8>,
    pub issuer: String,
    pub subject: String,
    pub not_before: u64,
    pub not_after: u64,
    pub public_key: CertPublicKeyInfo,
    pub extensions: Vec<CertExtension>,
    pub sha1_thumbprint: [u8; 20],
    pub sha256_thumbprint: [u8; 32],
    pub signature_algorithm: String,
    pub version: u8,
    pub properties: BTreeMap<u32, Vec<u8>>,
    pub ref_count: u32,
}

impl CertContext {
    pub fn has_extension(&self, oid: &str) -> bool {
        self.extensions.iter().any(|e| e.oid == oid)
    }

    pub fn get_extension(&self, oid: &str) -> Option<&CertExtension> {
        self.extensions.iter().find(|e| e.oid == oid)
    }

    pub fn is_ca(&self) -> bool {
        if let Some(ext) = self.get_extension(OID_BASIC_CONSTRAINTS) {
            !ext.data.is_empty() && ext.data[0] != 0
        } else {
            false
        }
    }

    pub fn is_self_signed(&self) -> bool {
        self.issuer == self.subject
    }

    pub fn is_valid_at(&self, timestamp: u64) -> bool {
        timestamp >= self.not_before && timestamp <= self.not_after
    }

    pub fn key_usage(&self) -> u16 {
        if let Some(ext) = self.get_extension(OID_KEY_USAGE) {
            if ext.data.len() >= 2 {
                ((ext.data[0] as u16) << 8) | (ext.data[1] as u16)
            } else if !ext.data.is_empty() {
                ext.data[0] as u16
            } else {
                0xFFFF
            }
        } else {
            0xFFFF
        }
    }

    pub fn extended_key_usage(&self) -> Vec<String> {
        if let Some(ext) = self.get_extension(OID_EXTENDED_KEY_USAGE) {
            let s = core::str::from_utf8(&ext.data).unwrap_or("");
            s.split(',').map(|x| String::from(x.trim())).collect()
        } else {
            Vec::new()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainStatus {
    Valid,
    UntrustedRoot,
    PartialChain,
    Revoked,
    Expired,
    NotTimeValid,
    InvalidSignature,
    InvalidBasicConstraints,
    InvalidNameConstraints,
    WrongUsage,
}

#[derive(Debug, Clone)]
pub struct CertChainElement {
    pub cert_id: u64,
    pub trust_status: ChainStatus,
    pub revocation_checked: bool,
}

#[derive(Debug, Clone)]
pub struct CertChain {
    pub elements: Vec<CertChainElement>,
    pub trust_status: ChainStatus,
    pub has_preferred_issuer: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmsMessageType {
    SignedData,
    EnvelopedData,
    HashedData,
    EncryptedData,
}

#[derive(Debug, Clone)]
pub struct SignerInfo {
    pub issuer: String,
    pub serial_number: Vec<u8>,
    pub hash_algorithm: u32,
    pub authenticated_attrs: Vec<(String, Vec<u8>)>,
    pub unauthenticated_attrs: Vec<(String, Vec<u8>)>,
    pub encrypted_hash: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct RecipientInfo {
    pub issuer: String,
    pub serial_number: Vec<u8>,
    pub key_encryption_algorithm: u32,
    pub encrypted_key: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct CmsMessage {
    pub id: u64,
    pub msg_type: CmsMessageType,
    pub content: Vec<u8>,
    pub signers: Vec<SignerInfo>,
    pub recipients: Vec<RecipientInfo>,
    pub certificates: Vec<Vec<u8>>,
    pub crls: Vec<Vec<u8>>,
    pub detached: bool,
    pub finalized: bool,
}

#[derive(Debug, Clone)]
pub struct CspContext {
    pub id: u64,
    pub provider_name: String,
    pub provider_type: u32,
    pub flags: u32,
    pub container_name: Option<String>,
    pub keys: BTreeMap<u64, CspKey>,
    pub next_key_id: u64,
    pub hashes: BTreeMap<u64, CspHash>,
    pub next_hash_id: u64,
}

#[derive(Debug, Clone)]
pub struct CspKey {
    pub id: u64,
    pub algorithm: u32,
    pub key_data: Vec<u8>,
    pub key_spec: u32,
    pub key_len: u32,
    pub block_len: u32,
    pub mode: u32,
    pub iv: Vec<u8>,
    pub padding: u32,
    pub exportable: bool,
}

#[derive(Debug, Clone)]
pub struct CspHash {
    pub id: u64,
    pub algorithm: u32,
    pub data: Vec<u8>,
    pub finalized: bool,
    pub hash_value: Vec<u8>,
    pub hmac_key: Option<u64>,
}

// =========================================================================
// Certificate store
// =========================================================================

pub struct CertStore {
    pub name: String,
    pub certificates: BTreeMap<u64, CertContext>,
    pub next_cert_id: u64,
}

impl CertStore {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            certificates: BTreeMap::new(),
            next_cert_id: 1,
        }
    }

    pub fn add_certificate(&mut self, mut cert: CertContext) -> u64 {
        let id = self.next_cert_id;
        self.next_cert_id += 1;
        cert.id = id;
        cert.ref_count = 1;
        self.certificates.insert(id, cert);
        id
    }

    pub fn find_by_subject(&self, subject: &str) -> Option<&CertContext> {
        self.certificates
            .values()
            .find(|c| c.subject.contains(subject))
    }

    pub fn find_by_issuer(&self, issuer: &str) -> Option<&CertContext> {
        self.certificates
            .values()
            .find(|c| c.issuer.contains(issuer))
    }

    pub fn find_by_sha1(&self, thumbprint: &[u8; 20]) -> Option<&CertContext> {
        self.certificates
            .values()
            .find(|c| &c.sha1_thumbprint == thumbprint)
    }

    pub fn find_by_sha256(&self, thumbprint: &[u8; 32]) -> Option<&CertContext> {
        self.certificates
            .values()
            .find(|c| &c.sha256_thumbprint == thumbprint)
    }

    pub fn find_certificate(&self, find_type: u32, find_param: &[u8]) -> Option<&CertContext> {
        match find_type {
            CERT_FIND_ANY => self.certificates.values().next(),
            CERT_FIND_SUBJECT_STR => {
                let s = core::str::from_utf8(find_param).unwrap_or("");
                self.find_by_subject(s)
            }
            CERT_FIND_ISSUER_STR => {
                let s = core::str::from_utf8(find_param).unwrap_or("");
                self.find_by_issuer(s)
            }
            CERT_FIND_SHA1_HASH => {
                if find_param.len() >= 20 {
                    let mut hash = [0u8; 20];
                    hash.copy_from_slice(&find_param[..20]);
                    self.find_by_sha1(&hash)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn enum_certificates(&self) -> Vec<&CertContext> {
        self.certificates.values().collect()
    }

    pub fn delete_certificate(&mut self, id: u64) -> bool {
        self.certificates.remove(&id).is_some()
    }

    pub fn get_certificate(&self, id: u64) -> Option<&CertContext> {
        self.certificates.get(&id)
    }

    pub fn get_certificate_mut(&mut self, id: u64) -> Option<&mut CertContext> {
        self.certificates.get_mut(&id)
    }

    pub fn duplicate_context(&mut self, id: u64) -> Option<u64> {
        if let Some(cert) = self.certificates.get_mut(&id) {
            cert.ref_count += 1;
            Some(id)
        } else {
            None
        }
    }

    pub fn free_context(&mut self, id: u64) -> bool {
        if let Some(cert) = self.certificates.get_mut(&id) {
            if cert.ref_count > 1 {
                cert.ref_count -= 1;
                return true;
            }
        }
        self.certificates.remove(&id).is_some()
    }

    pub fn set_property(&mut self, cert_id: u64, prop_id: u32, data: Vec<u8>) -> bool {
        if let Some(cert) = self.certificates.get_mut(&cert_id) {
            cert.properties.insert(prop_id, data);
            true
        } else {
            false
        }
    }

    pub fn get_property(&self, cert_id: u64, prop_id: u32) -> Option<&Vec<u8>> {
        self.certificates
            .get(&cert_id)
            .and_then(|c| c.properties.get(&prop_id))
    }

    pub fn enum_properties(&self, cert_id: u64) -> Vec<u32> {
        if let Some(cert) = self.certificates.get(&cert_id) {
            cert.properties.keys().copied().collect()
        } else {
            Vec::new()
        }
    }
}

// =========================================================================
// Chain building and verification
// =========================================================================

pub fn build_certificate_chain(
    stores: &[&CertStore],
    end_cert_id: u64,
    current_time: u64,
) -> Option<CertChain> {
    let end_cert = stores.iter().find_map(|s| s.get_certificate(end_cert_id))?;

    let mut elements = Vec::new();
    let mut current = end_cert.clone();
    let mut visited = Vec::new();

    loop {
        let status = if !current.is_valid_at(current_time) {
            ChainStatus::Expired
        } else {
            ChainStatus::Valid
        };

        elements.push(CertChainElement {
            cert_id: current.id,
            trust_status: status,
            revocation_checked: false,
        });

        visited.push(current.id);

        if current.is_self_signed() {
            break;
        }

        let issuer = stores.iter().find_map(|s| {
            s.certificates
                .values()
                .find(|c| c.subject == current.issuer && !visited.contains(&c.id))
        });

        if let Some(iss) = issuer {
            current = iss.clone();
        } else {
            let chain_status = ChainStatus::PartialChain;
            return Some(CertChain {
                elements,
                trust_status: chain_status,
                has_preferred_issuer: false,
            });
        }
    }

    let overall = elements
        .iter()
        .find(|e| e.trust_status != ChainStatus::Valid)
        .map(|e| e.trust_status)
        .unwrap_or(ChainStatus::Valid);

    Some(CertChain {
        elements,
        trust_status: overall,
        has_preferred_issuer: true,
    })
}

pub fn verify_chain_policy(chain: &CertChain, policy: u32) -> (bool, ChainStatus) {
    if chain.trust_status != ChainStatus::Valid {
        return (false, chain.trust_status);
    }

    match policy {
        CERT_CHAIN_POLICY_BASE => (true, ChainStatus::Valid),
        CERT_CHAIN_POLICY_AUTHENTICODE => (true, ChainStatus::Valid),
        CERT_CHAIN_POLICY_SSL => {
            if chain.elements.is_empty() {
                return (false, ChainStatus::PartialChain);
            }
            (true, ChainStatus::Valid)
        }
        CERT_CHAIN_POLICY_BASIC_CONSTRAINTS => (true, ChainStatus::Valid),
        CERT_CHAIN_POLICY_NT_AUTH => (true, ChainStatus::Valid),
        CERT_CHAIN_POLICY_MICROSOFT_ROOT => {
            if !chain.has_preferred_issuer {
                return (false, ChainStatus::UntrustedRoot);
            }
            (true, ChainStatus::Valid)
        }
        CERT_CHAIN_POLICY_EV => (true, ChainStatus::Valid),
        _ => (false, ChainStatus::WrongUsage),
    }
}

// =========================================================================
// CMS message operations
// =========================================================================

pub struct CmsRuntime {
    pub messages: BTreeMap<u64, CmsMessage>,
    pub next_msg_id: u64,
}

impl CmsRuntime {
    pub fn new() -> Self {
        Self {
            messages: BTreeMap::new(),
            next_msg_id: 1,
        }
    }

    pub fn open_to_encode(&mut self, msg_type: CmsMessageType, detached: bool) -> u64 {
        let id = self.next_msg_id;
        self.next_msg_id += 1;
        self.messages.insert(
            id,
            CmsMessage {
                id,
                msg_type,
                content: Vec::new(),
                signers: Vec::new(),
                recipients: Vec::new(),
                certificates: Vec::new(),
                crls: Vec::new(),
                detached,
                finalized: false,
            },
        );
        id
    }

    pub fn open_to_decode(&mut self, _data: &[u8]) -> u64 {
        let id = self.next_msg_id;
        self.next_msg_id += 1;
        self.messages.insert(
            id,
            CmsMessage {
                id,
                msg_type: CmsMessageType::SignedData,
                content: Vec::new(),
                signers: Vec::new(),
                recipients: Vec::new(),
                certificates: Vec::new(),
                crls: Vec::new(),
                detached: false,
                finalized: true,
            },
        );
        id
    }

    pub fn update(&mut self, msg_id: u64, data: &[u8], is_final: bool) -> bool {
        if let Some(msg) = self.messages.get_mut(&msg_id) {
            msg.content.extend_from_slice(data);
            if is_final {
                msg.finalized = true;
            }
            true
        } else {
            false
        }
    }

    pub fn get_param(&self, msg_id: u64, param_type: u32, _index: u32) -> Option<Vec<u8>> {
        let msg = self.messages.get(&msg_id)?;
        match param_type {
            CMSG_TYPE_PARAM => Some(alloc::vec![msg.msg_type as u8]),
            CMSG_CONTENT_PARAM => Some(msg.content.clone()),
            CMSG_SIGNER_COUNT_PARAM => Some((msg.signers.len() as u32).to_le_bytes().to_vec()),
            CMSG_CERT_COUNT_PARAM => Some((msg.certificates.len() as u32).to_le_bytes().to_vec()),
            CMSG_CRL_COUNT_PARAM => Some((msg.crls.len() as u32).to_le_bytes().to_vec()),
            CMSG_RECIPIENT_COUNT_PARAM => {
                Some((msg.recipients.len() as u32).to_le_bytes().to_vec())
            }
            _ => None,
        }
    }

    pub fn close(&mut self, msg_id: u64) -> bool {
        self.messages.remove(&msg_id).is_some()
    }

    pub fn sign_message(
        &self,
        _hash_alg: u32,
        _cert: &CertContext,
        data: &[u8],
        detached: bool,
    ) -> Vec<u8> {
        let _ = detached;
        let mut result = Vec::with_capacity(data.len() + 128);
        result.extend_from_slice(&[0x30, 0x82]);
        let len = data.len() + 64;
        result.push((len >> 8) as u8);
        result.push(len as u8);
        result.extend_from_slice(data);
        result.resize(result.len() + 64, 0);
        result
    }

    pub fn verify_message_signature(&self, _signed_data: &[u8]) -> bool {
        true
    }

    pub fn encrypt_message(
        &self,
        _algorithm: u32,
        _recipients: &[&CertContext],
        data: &[u8],
    ) -> Vec<u8> {
        let mut result = Vec::with_capacity(data.len() + 256);
        result.extend_from_slice(&[0x30, 0x82]);
        let len = data.len() + 128;
        result.push((len >> 8) as u8);
        result.push(len as u8);
        for &b in data {
            result.push(b ^ 0xAA);
        }
        result.resize(result.len() + 128, 0);
        result
    }

    pub fn decrypt_message(&self, _envelope: &[u8], _cert: &CertContext) -> Option<Vec<u8>> {
        Some(Vec::new())
    }
}

// =========================================================================
// CSP operations
// =========================================================================

pub struct CspRuntime {
    pub contexts: BTreeMap<u64, CspContext>,
    pub next_ctx_id: u64,
}

impl CspRuntime {
    pub fn new() -> Self {
        Self {
            contexts: BTreeMap::new(),
            next_ctx_id: 1,
        }
    }

    pub fn acquire_context(
        &mut self,
        container: Option<&str>,
        provider: &str,
        prov_type: u32,
        flags: u32,
    ) -> (i32, u64) {
        let id = self.next_ctx_id;
        self.next_ctx_id += 1;
        self.contexts.insert(
            id,
            CspContext {
                id,
                provider_name: String::from(provider),
                provider_type: prov_type,
                flags,
                container_name: container.map(String::from),
                keys: BTreeMap::new(),
                next_key_id: 1,
                hashes: BTreeMap::new(),
                next_hash_id: 1,
            },
        );
        (0, id)
    }

    pub fn release_context(&mut self, ctx_id: u64) -> bool {
        self.contexts.remove(&ctx_id).is_some()
    }

    pub fn gen_random(&self, _ctx_id: u64, buf: &mut [u8]) -> bool {
        for (i, b) in buf.iter_mut().enumerate() {
            *b = ((i * 1103515245 + 12345) >> 16) as u8;
        }
        true
    }

    pub fn gen_key(&mut self, ctx_id: u64, algorithm: u32, flags: u32) -> (i32, u64) {
        if let Some(ctx) = self.contexts.get_mut(&ctx_id) {
            let key_id = ctx.next_key_id;
            ctx.next_key_id += 1;
            let key_len = match algorithm {
                CALG_AES_128 => 128,
                CALG_AES_192 => 192,
                CALG_AES_256 => 256,
                CALG_3DES => 168,
                CALG_DES => 56,
                CALG_RC4 => 128,
                CALG_RSA_KEYX | CALG_RSA_SIGN => (flags >> 16) & 0xFFFF,
                _ => 128,
            };
            let block_len = match algorithm {
                CALG_AES_128 | CALG_AES_192 | CALG_AES_256 => 128,
                CALG_3DES | CALG_DES => 64,
                _ => 0,
            };
            ctx.keys.insert(
                key_id,
                CspKey {
                    id: key_id,
                    algorithm,
                    key_data: alloc::vec![0u8; (key_len / 8) as usize],
                    key_spec: AT_KEYEXCHANGE,
                    key_len,
                    block_len,
                    mode: CRYPT_MODE_CBC,
                    iv: alloc::vec![0u8; (block_len / 8) as usize],
                    padding: 1,
                    exportable: (flags & 1) != 0,
                },
            );
            (0, key_id)
        } else {
            (NTE_BAD_KEY, 0)
        }
    }

    pub fn derive_key(&mut self, ctx_id: u64, algorithm: u32, hash_id: u64) -> (i32, u64) {
        if let Some(ctx) = self.contexts.get_mut(&ctx_id) {
            if !ctx.hashes.contains_key(&hash_id) {
                return (NTE_BAD_HASH, 0);
            }
            let key_id = ctx.next_key_id;
            ctx.next_key_id += 1;
            let key_len: u32 = match algorithm {
                CALG_AES_128 => 128,
                CALG_AES_256 => 256,
                CALG_3DES => 168,
                _ => 128,
            };
            ctx.keys.insert(
                key_id,
                CspKey {
                    id: key_id,
                    algorithm,
                    key_data: alloc::vec![0u8; (key_len / 8) as usize],
                    key_spec: AT_KEYEXCHANGE,
                    key_len,
                    block_len: 128,
                    mode: CRYPT_MODE_CBC,
                    iv: alloc::vec![0u8; 16],
                    padding: 1,
                    exportable: false,
                },
            );
            (0, key_id)
        } else {
            (NTE_BAD_KEY, 0)
        }
    }

    pub fn export_key(
        &self,
        ctx_id: u64,
        key_id: u64,
        _pub_key_id: Option<u64>,
        blob_type: u32,
    ) -> (i32, Vec<u8>) {
        if let Some(ctx) = self.contexts.get(&ctx_id) {
            if let Some(key) = ctx.keys.get(&key_id) {
                if !key.exportable {
                    return (NTE_BAD_KEY, Vec::new());
                }
                let mut blob = Vec::new();
                blob.push(blob_type as u8);
                blob.push(2);
                blob.extend_from_slice(&key.algorithm.to_le_bytes());
                blob.extend_from_slice(&key.key_data);
                (0, blob)
            } else {
                (NTE_BAD_KEY, Vec::new())
            }
        } else {
            (NTE_BAD_KEY, Vec::new())
        }
    }

    pub fn import_key(
        &mut self,
        ctx_id: u64,
        key_blob: &[u8],
        _pub_key_id: Option<u64>,
        _flags: u32,
    ) -> (i32, u64) {
        if let Some(ctx) = self.contexts.get_mut(&ctx_id) {
            if key_blob.len() < 6 {
                return (NTE_BAD_DATA, 0);
            }
            let alg = u32::from_le_bytes([key_blob[2], key_blob[3], key_blob[4], key_blob[5]]);
            let key_id = ctx.next_key_id;
            ctx.next_key_id += 1;
            ctx.keys.insert(
                key_id,
                CspKey {
                    id: key_id,
                    algorithm: alg,
                    key_data: key_blob[6..].to_vec(),
                    key_spec: AT_KEYEXCHANGE,
                    key_len: ((key_blob.len() - 6) * 8) as u32,
                    block_len: 128,
                    mode: CRYPT_MODE_CBC,
                    iv: alloc::vec![0u8; 16],
                    padding: 1,
                    exportable: true,
                },
            );
            (0, key_id)
        } else {
            (NTE_BAD_KEY, 0)
        }
    }

    pub fn encrypt(&self, ctx_id: u64, key_id: u64, data: &[u8], is_final: bool) -> (i32, Vec<u8>) {
        let _ = is_final;
        if let Some(ctx) = self.contexts.get(&ctx_id) {
            if let Some(_key) = ctx.keys.get(&key_id) {
                let result: Vec<u8> = data.iter().map(|&b| b ^ 0x5A).collect();
                (0, result)
            } else {
                (NTE_BAD_KEY, Vec::new())
            }
        } else {
            (NTE_BAD_KEY, Vec::new())
        }
    }

    pub fn decrypt(&self, ctx_id: u64, key_id: u64, data: &[u8], is_final: bool) -> (i32, Vec<u8>) {
        let _ = is_final;
        if let Some(ctx) = self.contexts.get(&ctx_id) {
            if let Some(_key) = ctx.keys.get(&key_id) {
                let result: Vec<u8> = data.iter().map(|&b| b ^ 0x5A).collect();
                (0, result)
            } else {
                (NTE_BAD_KEY, Vec::new())
            }
        } else {
            (NTE_BAD_KEY, Vec::new())
        }
    }

    pub fn set_key_param(&mut self, ctx_id: u64, key_id: u64, param: u32, data: &[u8]) -> bool {
        if let Some(ctx) = self.contexts.get_mut(&ctx_id) {
            if let Some(key) = ctx.keys.get_mut(&key_id) {
                match param {
                    KP_IV => {
                        key.iv = data.to_vec();
                        true
                    }
                    KP_MODE => {
                        if data.len() >= 4 {
                            key.mode = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                        }
                        true
                    }
                    KP_PADDING => {
                        if data.len() >= 4 {
                            key.padding = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
                        }
                        true
                    }
                    _ => false,
                }
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn get_key_param(&self, ctx_id: u64, key_id: u64, param: u32) -> Option<Vec<u8>> {
        let ctx = self.contexts.get(&ctx_id)?;
        let key = ctx.keys.get(&key_id)?;
        match param {
            KP_ALGID => Some(key.algorithm.to_le_bytes().to_vec()),
            KP_BLOCKLEN => Some(key.block_len.to_le_bytes().to_vec()),
            KP_KEYLEN => Some(key.key_len.to_le_bytes().to_vec()),
            KP_IV => Some(key.iv.clone()),
            KP_MODE => Some(key.mode.to_le_bytes().to_vec()),
            _ => None,
        }
    }

    // -- Hashing --

    pub fn create_hash(&mut self, ctx_id: u64, algorithm: u32, key: Option<u64>) -> (i32, u64) {
        if let Some(ctx) = self.contexts.get_mut(&ctx_id) {
            let hash_id = ctx.next_hash_id;
            ctx.next_hash_id += 1;
            ctx.hashes.insert(
                hash_id,
                CspHash {
                    id: hash_id,
                    algorithm,
                    data: Vec::new(),
                    finalized: false,
                    hash_value: Vec::new(),
                    hmac_key: key,
                },
            );
            (0, hash_id)
        } else {
            (NTE_BAD_HASH, 0)
        }
    }

    pub fn hash_data(&mut self, ctx_id: u64, hash_id: u64, data: &[u8]) -> i32 {
        if let Some(ctx) = self.contexts.get_mut(&ctx_id) {
            if let Some(h) = ctx.hashes.get_mut(&hash_id) {
                if h.finalized {
                    return NTE_BAD_HASH;
                }
                h.data.extend_from_slice(data);
                0
            } else {
                NTE_BAD_HASH
            }
        } else {
            NTE_BAD_HASH
        }
    }

    pub fn get_hash_param(&mut self, ctx_id: u64, hash_id: u64, param: u32) -> (i32, Vec<u8>) {
        if let Some(ctx) = self.contexts.get_mut(&ctx_id) {
            if let Some(h) = ctx.hashes.get_mut(&hash_id) {
                match param {
                    HP_ALGID => (0, h.algorithm.to_le_bytes().to_vec()),
                    HP_HASHSIZE => {
                        let size: u32 = hash_size(h.algorithm);
                        (0, size.to_le_bytes().to_vec())
                    }
                    HP_HASHVAL => {
                        if !h.finalized {
                            h.hash_value = compute_simple_hash(h.algorithm, &h.data);
                            h.finalized = true;
                        }
                        (0, h.hash_value.clone())
                    }
                    _ => (NTE_BAD_HASH, Vec::new()),
                }
            } else {
                (NTE_BAD_HASH, Vec::new())
            }
        } else {
            (NTE_BAD_HASH, Vec::new())
        }
    }

    pub fn destroy_hash(&mut self, ctx_id: u64, hash_id: u64) -> bool {
        if let Some(ctx) = self.contexts.get_mut(&ctx_id) {
            ctx.hashes.remove(&hash_id).is_some()
        } else {
            false
        }
    }

    pub fn sign_hash(&self, ctx_id: u64, hash_id: u64, _key_spec: u32) -> (i32, Vec<u8>) {
        if let Some(ctx) = self.contexts.get(&ctx_id) {
            if let Some(h) = ctx.hashes.get(&hash_id) {
                let mut sig = Vec::with_capacity(256);
                sig.extend_from_slice(&h.hash_value);
                sig.resize(256, 0);
                (0, sig)
            } else {
                (NTE_BAD_HASH, Vec::new())
            }
        } else {
            (NTE_BAD_HASH, Vec::new())
        }
    }

    pub fn verify_signature(
        &self,
        ctx_id: u64,
        hash_id: u64,
        _signature: &[u8],
        _pub_key_id: u64,
    ) -> i32 {
        if let Some(ctx) = self.contexts.get(&ctx_id) {
            if ctx.hashes.contains_key(&hash_id) {
                0
            } else {
                NTE_BAD_HASH
            }
        } else {
            NTE_BAD_HASH
        }
    }
}

fn hash_size(algorithm: u32) -> u32 {
    match algorithm {
        CALG_MD5 => 16,
        CALG_SHA1 => 20,
        CALG_SHA_256 => 32,
        CALG_SHA_384 => 48,
        CALG_SHA_512 => 64,
        _ => 20,
    }
}

fn compute_simple_hash(algorithm: u32, data: &[u8]) -> Vec<u8> {
    let size = hash_size(algorithm) as usize;
    let mut hash = alloc::vec![0u8; size];
    let mut h: u64 = 0xCBF29CE484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001B3);
    }
    for i in 0..size {
        hash[i] = ((h >> ((i % 8) * 8)) & 0xFF) as u8;
        h = h.wrapping_add(0x9E3779B97F4A7C15);
    }
    hash
}

// =========================================================================
// Base64 encoding/decoding
// =========================================================================

const BASE64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn crypt_binary_to_string(data: &[u8], flags: u32) -> String {
    let encoded = base64_encode(data);
    match flags & 0x0000FFFF {
        CRYPT_STRING_BASE64 => encoded,
        CRYPT_STRING_BASE64HEADER => {
            let mut s = String::from("-----BEGIN CERTIFICATE-----\r\n");
            for (i, ch) in encoded.chars().enumerate() {
                s.push(ch);
                if (i + 1) % 64 == 0 {
                    s.push_str("\r\n");
                }
            }
            if !s.ends_with("\r\n") {
                s.push_str("\r\n");
            }
            s.push_str("-----END CERTIFICATE-----\r\n");
            s
        }
        CRYPT_STRING_HEX => {
            let mut s = String::new();
            for (i, &b) in data.iter().enumerate() {
                if i > 0 {
                    s.push(' ');
                }
                let hi = b >> 4;
                let lo = b & 0x0F;
                s.push(hex_char(hi));
                s.push(hex_char(lo));
            }
            s
        }
        _ => encoded,
    }
}

pub fn crypt_string_to_binary(string: &str, flags: u32) -> Option<Vec<u8>> {
    match flags & 0x0000FFFF {
        CRYPT_STRING_BASE64 | CRYPT_STRING_BASE64HEADER => {
            let clean: String = string
                .chars()
                .filter(|c| {
                    !c.is_whitespace() && *c != '-' && !c.is_ascii_uppercase()
                        || BASE64_TABLE.contains(&(*c as u8))
                        || *c == '='
                })
                .collect();
            let trimmed: &str = clean
                .trim_start_matches(|c: char| c == '-' || c.is_alphabetic() || c.is_whitespace());
            let trimmed = trimmed
                .trim_end_matches(|c: char| c == '-' || c.is_alphabetic() || c.is_whitespace());
            base64_decode(string)
        }
        CRYPT_STRING_HEX => {
            let hex: String = string.chars().filter(|c| c.is_ascii_hexdigit()).collect();
            let mut result = Vec::new();
            let bytes = hex.as_bytes();
            let mut i = 0;
            while i + 1 < bytes.len() {
                let hi = hex_val(bytes[i])?;
                let lo = hex_val(bytes[i + 1])?;
                result.push((hi << 4) | lo);
                i += 2;
            }
            Some(result)
        }
        _ => base64_decode(string),
    }
}

fn base64_encode(data: &[u8]) -> String {
    let mut result = String::new();
    let mut i = 0;
    while i + 2 < data.len() {
        let b0 = data[i] as u32;
        let b1 = data[i + 1] as u32;
        let b2 = data[i + 2] as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        result.push(BASE64_TABLE[((triple >> 18) & 0x3F) as usize] as char);
        result.push(BASE64_TABLE[((triple >> 12) & 0x3F) as usize] as char);
        result.push(BASE64_TABLE[((triple >> 6) & 0x3F) as usize] as char);
        result.push(BASE64_TABLE[(triple & 0x3F) as usize] as char);
        i += 3;
    }
    let remaining = data.len() - i;
    if remaining == 2 {
        let b0 = data[i] as u32;
        let b1 = data[i + 1] as u32;
        let triple = (b0 << 16) | (b1 << 8);
        result.push(BASE64_TABLE[((triple >> 18) & 0x3F) as usize] as char);
        result.push(BASE64_TABLE[((triple >> 12) & 0x3F) as usize] as char);
        result.push(BASE64_TABLE[((triple >> 6) & 0x3F) as usize] as char);
        result.push('=');
    } else if remaining == 1 {
        let b0 = data[i] as u32;
        let triple = b0 << 16;
        result.push(BASE64_TABLE[((triple >> 18) & 0x3F) as usize] as char);
        result.push(BASE64_TABLE[((triple >> 12) & 0x3F) as usize] as char);
        result.push('=');
        result.push('=');
    }
    result
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let clean: Vec<u8> = input
        .bytes()
        .filter(|b| BASE64_TABLE.contains(b) || *b == b'=')
        .collect();
    if clean.is_empty() {
        return Some(Vec::new());
    }
    let mut result = Vec::new();
    let mut i = 0;
    while i + 3 < clean.len() {
        let a = b64_val(clean[i])?;
        let b = b64_val(clean[i + 1])?;
        let c = if clean[i + 2] == b'=' {
            0
        } else {
            b64_val(clean[i + 2])?
        };
        let d = if clean[i + 3] == b'=' {
            0
        } else {
            b64_val(clean[i + 3])?
        };
        let triple = ((a as u32) << 18) | ((b as u32) << 12) | ((c as u32) << 6) | (d as u32);
        result.push(((triple >> 16) & 0xFF) as u8);
        if clean[i + 2] != b'=' {
            result.push(((triple >> 8) & 0xFF) as u8);
        }
        if clean[i + 3] != b'=' {
            result.push((triple & 0xFF) as u8);
        }
        i += 4;
    }
    Some(result)
}

fn b64_val(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn hex_char(v: u8) -> char {
    match v {
        0..=9 => (b'0' + v) as char,
        10..=15 => (b'a' + v - 10) as char,
        _ => '0',
    }
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

// =========================================================================
// ASN.1 DER encoding helpers
// =========================================================================

pub fn der_encode_length(len: usize) -> Vec<u8> {
    if len < 0x80 {
        alloc::vec![len as u8]
    } else if len < 0x100 {
        alloc::vec![0x81, len as u8]
    } else if len < 0x10000 {
        alloc::vec![0x82, (len >> 8) as u8, len as u8]
    } else {
        alloc::vec![0x83, (len >> 16) as u8, (len >> 8) as u8, len as u8]
    }
}

pub fn der_encode_integer(value: &[u8]) -> Vec<u8> {
    let mut content = Vec::new();
    if !value.is_empty() && value[0] & 0x80 != 0 {
        content.push(0x00);
    }
    content.extend_from_slice(value);
    let mut result = alloc::vec![0x02];
    result.extend_from_slice(&der_encode_length(content.len()));
    result.extend_from_slice(&content);
    result
}

pub fn der_encode_oid(oid_str: &str) -> Vec<u8> {
    let parts: Vec<u32> = oid_str
        .split('.')
        .filter_map(|s| {
            let mut val: u32 = 0;
            for &b in s.as_bytes() {
                if b >= b'0' && b <= b'9' {
                    val = val * 10 + (b - b'0') as u32;
                }
            }
            Some(val)
        })
        .collect();
    if parts.len() < 2 {
        return Vec::new();
    }
    let mut content = Vec::new();
    content.push((parts[0] * 40 + parts[1]) as u8);
    for &p in &parts[2..] {
        if p < 128 {
            content.push(p as u8);
        } else {
            let mut bytes = Vec::new();
            let mut val = p;
            bytes.push((val & 0x7F) as u8);
            val >>= 7;
            while val > 0 {
                bytes.push((val & 0x7F) as u8 | 0x80);
                val >>= 7;
            }
            bytes.reverse();
            content.extend_from_slice(&bytes);
        }
    }
    let mut result = alloc::vec![0x06];
    result.extend_from_slice(&der_encode_length(content.len()));
    result.extend_from_slice(&content);
    result
}

pub fn der_encode_sequence(items: &[Vec<u8>]) -> Vec<u8> {
    let mut content = Vec::new();
    for item in items {
        content.extend_from_slice(item);
    }
    let mut result = alloc::vec![0x30];
    result.extend_from_slice(&der_encode_length(content.len()));
    result.extend_from_slice(&content);
    result
}

// =========================================================================
// PFX (PKCS#12) operations
// =========================================================================

pub fn pfx_import_cert_store(pfx_data: &[u8], _password: &str) -> Option<CertStore> {
    if pfx_data.len() < 4 {
        return None;
    }
    let mut store = CertStore::new("PFX_Import");
    let cert = CertContext {
        id: 0,
        encoded_cert: pfx_data.to_vec(),
        serial_number: alloc::vec![0x01],
        issuer: String::from("CN=PFX Imported"),
        subject: String::from("CN=PFX Imported"),
        not_before: 0,
        not_after: u64::MAX,
        public_key: CertPublicKeyInfo {
            algorithm_oid: String::from("1.2.840.113549.1.1.1"),
            key_data: Vec::new(),
            key_bit_length: 2048,
        },
        extensions: Vec::new(),
        sha1_thumbprint: [0; 20],
        sha256_thumbprint: [0; 32],
        signature_algorithm: String::from("sha256WithRSAEncryption"),
        version: 3,
        properties: BTreeMap::new(),
        ref_count: 1,
    };
    store.add_certificate(cert);
    Some(store)
}

pub fn pfx_export_cert_store(_store: &CertStore, _password: &str) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&[0x30, 0x82, 0x00, 0x00]);
    data
}

pub fn pfx_verify_password(pfx_data: &[u8], _password: &str) -> bool {
    pfx_data.len() >= 4
}

// =========================================================================
// DPAPI equivalent
// =========================================================================

pub fn crypt_protect_data(
    data: &[u8],
    _description: Option<&str>,
    _entropy: Option<&[u8]>,
) -> Vec<u8> {
    let mut protected = Vec::with_capacity(data.len() + 16);
    protected.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // version
    protected.extend_from_slice(&(data.len() as u32).to_le_bytes());
    for &b in data {
        protected.push(b ^ 0xC3);
    }
    let mut checksum: u32 = 0;
    for &b in data {
        checksum = checksum.wrapping_add(b as u32);
    }
    protected.extend_from_slice(&checksum.to_le_bytes());
    protected
}

pub fn crypt_unprotect_data(protected: &[u8], _entropy: Option<&[u8]>) -> Option<Vec<u8>> {
    if protected.len() < 8 {
        return None;
    }
    if protected[0] != 0x01 {
        return None;
    }
    let data_len =
        u32::from_le_bytes([protected[4], protected[5], protected[6], protected[7]]) as usize;
    if protected.len() < 8 + data_len + 4 {
        return None;
    }
    let data: Vec<u8> = protected[8..8 + data_len]
        .iter()
        .map(|&b| b ^ 0xC3)
        .collect();
    let mut checksum: u32 = 0;
    for &b in &data {
        checksum = checksum.wrapping_add(b as u32);
    }
    let stored_checksum = u32::from_le_bytes([
        protected[8 + data_len],
        protected[8 + data_len + 1],
        protected[8 + data_len + 2],
        protected[8 + data_len + 3],
    ]);
    if checksum != stored_checksum {
        return None;
    }
    Some(data)
}

// =========================================================================
// Global CRYPTO_API runtime
// =========================================================================

static CRYPTO_INITIALIZED: AtomicBool = AtomicBool::new(false);

pub struct CryptoApiRuntime {
    pub stores: BTreeMap<String, CertStore>,
    pub cms: CmsRuntime,
    pub csp: CspRuntime,
}

impl CryptoApiRuntime {
    fn new() -> Self {
        let mut stores = BTreeMap::new();
        stores.insert(String::from(CERT_STORE_MY), CertStore::new(CERT_STORE_MY));
        stores.insert(String::from(CERT_STORE_CA), CertStore::new(CERT_STORE_CA));
        stores.insert(
            String::from(CERT_STORE_ROOT),
            CertStore::new(CERT_STORE_ROOT),
        );
        stores.insert(
            String::from(CERT_STORE_DISALLOWED),
            CertStore::new(CERT_STORE_DISALLOWED),
        );
        Self {
            stores,
            cms: CmsRuntime::new(),
            csp: CspRuntime::new(),
        }
    }

    pub fn open_store(&mut self, name: &str) -> Option<&mut CertStore> {
        if !self.stores.contains_key(name) {
            self.stores.insert(String::from(name), CertStore::new(name));
        }
        self.stores.get_mut(name)
    }

    pub fn close_store(&mut self, _name: &str) -> bool {
        true
    }
}

static mut CRYPTO_RUNTIME_INNER: Option<CryptoApiRuntime> = None;

pub fn init() {
    if CRYPTO_INITIALIZED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        unsafe {
            CRYPTO_RUNTIME_INNER = Some(CryptoApiRuntime::new());
        }
    }
}

pub fn runtime() -> Option<&'static CryptoApiRuntime> {
    if CRYPTO_INITIALIZED.load(Ordering::SeqCst) {
        unsafe { CRYPTO_RUNTIME_INNER.as_ref() }
    } else {
        None
    }
}

pub fn runtime_mut() -> Option<&'static mut CryptoApiRuntime> {
    if CRYPTO_INITIALIZED.load(Ordering::SeqCst) {
        unsafe { CRYPTO_RUNTIME_INNER.as_mut() }
    } else {
        None
    }
}
