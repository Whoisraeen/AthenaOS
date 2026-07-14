//! WebAuthn (FIDO2) relying-party ceremony core for AthID.
//!
//! The Concept's account pillar: *"AthID — passkeys first, optional, never
//! required for local use."* This module is the **relying-party** half of the
//! WebAuthn/FIDO2 ceremonies (registration/attestation + authentication/
//! assertion) made real: it parses `authenticatorData`, `clientDataJSON`, and
//! the COSE public key, and it **actually verifies the assertion signature**
//! against the stored credential public key (the legacy `PasskeyCeremony` in
//! `lib.rs` only checked structure — this does the crypto).
//!
//! ## "Never required for local use"
//!
//! This is OPT-IN. Local login on a AthenaOS box does **not** touch this module:
//! guest mode, local password, and PIN (all in `lib.rs`) provide complete
//! offline functionality with no account and no passkey. WebAuthn here exists so
//! apps and web origins that *want* phishing-resistant passkeys can use one, and
//! so AthID can act as an authenticator-backed relying party — never as a gate on
//! using your own machine.
//!
//! ## Credential algorithms
//!
//! - **EdDSA / Ed25519** (COSE alg `-8`, the `OKP`/Ed25519 key type) is fully
//!   implemented: registration extracts the key, authentication verifies the
//!   signature via `ath_crypto::ed25519`. This is the algorithm AthID requests.
//! - **ES256 / P-256** (COSE alg `-7`) is fully implemented: registration
//!   extracts the EC2 public key, authentication verifies the assertion
//!   signature via `ath_crypto::p256_ecdsa::verify`. This is the algorithm
//!   virtually every hardware security key (YubiKey, Titan) and platform
//!   authenticator (Touch ID, Android) emits.
//! - **ES384 / P-384** (COSE alg `-35`) is fully implemented: EC2 key, crv=2,
//!   verified via `ath_crypto::p384_ecdsa::verify` (SHA-384 prehash) — the
//!   high-assurance tier some enterprise authenticators emit.
//! - **RS256 / RSA** (COSE alg `-257`) is fully implemented: registration
//!   extracts the RSA public key (`n` at COSE label -1, `e` at -2), assertions
//!   verify RSASSA-PKCS1-v1_5 over SHA-256 via `ath_crypto::rsa`. This is what
//!   **Windows Hello's TPM platform authenticator** emits — the last mainstream
//!   gap. With these four, passwordless login accepts every mainstream
//!   authenticator — see `MasterChecklist` Phase 15.
//!
//! ## Crypto reuse
//!
//! All crypto comes from the workspace `ath_crypto` crate (`ed25519::verify`,
//! `p256_ecdsa::verify`, `sha256::sha256`). No crypto is reimplemented here.
//! CBOR/COSE parsing is a minimal hand-rolled, bounds-checked, never-panic
//! reader (no new dependency).
//!
//! ## Never panics on untrusted input
//!
//! Every parser returns `Result<_, WebAuthnError>`; truncated/garbage bytes
//! yield an `Err`, never a panic or out-of-bounds index.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use ath_crypto::ed25519;
use ath_crypto::p256_ecdsa;
use ath_crypto::p384_ecdsa;
use ath_crypto::rsa;
use ath_crypto::sha256::sha256;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Failure modes of the WebAuthn ceremonies. Distinct variants so the OS layer
/// (and tests) can assert exactly *why* a ceremony was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebAuthnError {
    /// `authenticatorData` shorter than the 37-byte fixed prefix.
    AuthDataTooShort,
    /// The AT (attested-credential-data) flag was set but the trailing bytes
    /// were missing/truncated.
    AttestedCredentialTruncated,
    /// `clientDataJSON` was not valid UTF-8 / not parseable JSON.
    BadClientDataJson,
    /// `clientDataJSON.type` did not match the expected ceremony type.
    WrongClientDataType,
    /// `clientDataJSON.challenge` did not match the server-issued challenge.
    ChallengeMismatch,
    /// `clientDataJSON.origin` did not match the expected origin.
    OriginMismatch,
    /// `rpIdHash` in `authenticatorData` != SHA-256(rpId).
    RpIdHashMismatch,
    /// User-Presence (UP) flag was required but not set.
    UserPresenceMissing,
    /// User-Verification (UV) flag was required but not set.
    UserVerificationMissing,
    /// The COSE public key could not be parsed.
    BadCoseKey,
    /// The credential algorithm is parsed but not implemented (e.g. ES256).
    UnsupportedAlgorithm,
    /// The assertion signature did not verify against the stored public key.
    BadSignature,
    /// `signCount` regressed (stored >= received with both non-zero): the
    /// authenticator may have been cloned.
    SignCountCloned,
    /// No stored credential matched the asserted credential id.
    CredentialNotFound,
    /// Attestation statement format was understood but its signature failed.
    BadAttestation,
}

// ---------------------------------------------------------------------------
// Flags
// ---------------------------------------------------------------------------

/// `authenticatorData` flag byte (offset 32), per WebAuthn §6.1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthDataFlags(pub u8);

impl AuthDataFlags {
    pub const UP: u8 = 1 << 0; // User Present
    pub const UV: u8 = 1 << 2; // User Verified
    pub const BE: u8 = 1 << 3; // Backup Eligible
    pub const BS: u8 = 1 << 4; // Backup State
    pub const AT: u8 = 1 << 6; // Attested credential data included
    pub const ED: u8 = 1 << 7; // Extension data included

    pub fn user_present(&self) -> bool {
        self.0 & Self::UP != 0
    }
    pub fn user_verified(&self) -> bool {
        self.0 & Self::UV != 0
    }
    pub fn attested_credential_data(&self) -> bool {
        self.0 & Self::AT != 0
    }
    pub fn extension_data(&self) -> bool {
        self.0 & Self::ED != 0
    }
}

// ---------------------------------------------------------------------------
// COSE public key
// ---------------------------------------------------------------------------

/// A parsed COSE_Key public key. Every mainstream WebAuthn authenticator
/// algorithm is verifiable here: Ed25519, ES256, ES384, and RS256.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoseKey {
    /// OKP / Ed25519 (kty=1, crv=6, alg=-8). 32-byte public key (the `x` param).
    Ed25519([u8; 32]),
    /// EC2 / P-256 (kty=2, crv=1, alg=-7). 32-byte `x` and `y` affine coords.
    /// Verified via `ath_crypto::p256_ecdsa` (the `x||y` bare COSE form).
    Es256 { x: [u8; 32], y: [u8; 32] },
    /// EC2 / P-384 (kty=2, crv=2, alg=-35). 48-byte `x` and `y` affine coords.
    /// Verified via `ath_crypto::p384_ecdsa` (the `x||y` bare COSE form,
    /// SHA-384 prehash). The high-assurance ES384 tier.
    Es384 { x: [u8; 48], y: [u8; 48] },
    /// RSA (kty=3, alg=-257). Modulus `n` (COSE label -1) and public exponent
    /// `e` (label -2), both big-endian. RSASSA-PKCS1-v1_5 over SHA-256, verified
    /// via `ath_crypto::rsa`. This is what Windows Hello's TPM platform
    /// authenticator emits — the highest-impact coverage gap.
    Rs256 { n: Vec<u8>, e: Vec<u8> },
}

impl CoseKey {
    /// COSE `alg` identifier of this key.
    pub fn alg(&self) -> i64 {
        match self {
            CoseKey::Ed25519(_) => -8,
            CoseKey::Es256 { .. } => -7,
            CoseKey::Es384 { .. } => -35,
            CoseKey::Rs256 { .. } => -257,
        }
    }

    /// Whether this crate can verify signatures for this key type. All four
    /// supported COSE algorithms are verifiable, so this is always `true`.
    pub fn is_verifiable(&self) -> bool {
        matches!(
            self,
            CoseKey::Ed25519(_)
                | CoseKey::Es256 { .. }
                | CoseKey::Es384 { .. }
                | CoseKey::Rs256 { .. }
        )
    }
}

// ---------------------------------------------------------------------------
// Attested credential data + parsed authenticator data
// ---------------------------------------------------------------------------

/// The attested-credential-data section of `authenticatorData` (present when the
/// AT flag is set — i.e. registration responses).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttestedCredentialData {
    pub aaguid: [u8; 16],
    pub credential_id: Vec<u8>,
    pub public_key: CoseKey,
}

/// Parsed `authenticatorData` (WebAuthn §6.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthenticatorData {
    pub rp_id_hash: [u8; 32],
    pub flags: AuthDataFlags,
    pub sign_count: u32,
    /// Present iff the AT flag is set.
    pub attested_credential: Option<AttestedCredentialData>,
}

impl AuthenticatorData {
    /// Parse the fixed 37-byte prefix and, if the AT flag is set, the attested
    /// credential data (aaguid || L || credentialId || COSE key). Bounds-checked;
    /// never panics on truncated/garbage input.
    pub fn parse(bytes: &[u8]) -> Result<Self, WebAuthnError> {
        if bytes.len() < 37 {
            return Err(WebAuthnError::AuthDataTooShort);
        }
        let mut rp_id_hash = [0u8; 32];
        rp_id_hash.copy_from_slice(&bytes[0..32]);
        let flags = AuthDataFlags(bytes[32]);
        let sign_count = u32::from_be_bytes([bytes[33], bytes[34], bytes[35], bytes[36]]);

        let attested_credential = if flags.attested_credential_data() {
            // aaguid(16) credIdLen(2 BE) credId(L) cosePublicKey(CBOR)
            if bytes.len() < 37 + 16 + 2 {
                return Err(WebAuthnError::AttestedCredentialTruncated);
            }
            let mut aaguid = [0u8; 16];
            aaguid.copy_from_slice(&bytes[37..53]);
            let cred_id_len = u16::from_be_bytes([bytes[53], bytes[54]]) as usize;
            let id_start: usize = 55;
            let id_end = id_start
                .checked_add(cred_id_len)
                .ok_or(WebAuthnError::AttestedCredentialTruncated)?;
            if id_end > bytes.len() {
                return Err(WebAuthnError::AttestedCredentialTruncated);
            }
            let credential_id = bytes[id_start..id_end].to_vec();
            let public_key = parse_cose_key(&bytes[id_end..])?;
            Some(AttestedCredentialData {
                aaguid,
                credential_id,
                public_key,
            })
        } else {
            None
        };

        Ok(AuthenticatorData {
            rp_id_hash,
            flags,
            sign_count,
            attested_credential,
        })
    }
}

// ---------------------------------------------------------------------------
// clientDataJSON
// ---------------------------------------------------------------------------

/// The three fields of `clientDataJSON` the relying party validates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectedClientData {
    pub ceremony_type: String,
    /// The challenge as the raw base64url string the client echoed back.
    pub challenge_b64url: String,
    pub origin: String,
}

impl CollectedClientData {
    /// Parse `clientDataJSON`. This is a minimal extractor for the three
    /// security-relevant string fields (`type`, `challenge`, `origin`); it does
    /// not build a full JSON DOM. Never panics on malformed input.
    pub fn parse(json: &[u8]) -> Result<Self, WebAuthnError> {
        let s = core::str::from_utf8(json).map_err(|_| WebAuthnError::BadClientDataJson)?;
        let ceremony_type =
            extract_json_string(s, "type").ok_or(WebAuthnError::BadClientDataJson)?;
        let challenge_b64url =
            extract_json_string(s, "challenge").ok_or(WebAuthnError::BadClientDataJson)?;
        let origin = extract_json_string(s, "origin").ok_or(WebAuthnError::BadClientDataJson)?;
        Ok(CollectedClientData {
            ceremony_type,
            challenge_b64url,
            origin,
        })
    }
}

// ---------------------------------------------------------------------------
// Ceremony options (the data the relying party hands the client)
// ---------------------------------------------------------------------------

/// User-verification requirement for a ceremony.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserVerification {
    Required,
    Preferred,
    Discouraged,
}

/// `PublicKeyCredentialCreationOptions` (registration), the subset AthID uses.
#[derive(Clone, Debug)]
pub struct CredentialCreationOptions {
    pub rp_id: String,
    pub rp_name: String,
    pub user_handle: Vec<u8>,
    pub user_name: String,
    pub challenge: [u8; 32],
    /// COSE alg identifiers the RP accepts, in preference order. AthID requests
    /// EdDSA (`-8`) first; ES256 (`-7`) may be listed but is verification-deferred.
    pub pub_key_cred_params: Vec<i64>,
    pub user_verification: UserVerification,
}

impl CredentialCreationOptions {
    /// The default AthID registration request: EdDSA-only, UV preferred.
    pub fn new(
        rp_id: String,
        rp_name: String,
        user_handle: Vec<u8>,
        user_name: String,
        challenge: [u8; 32],
    ) -> Self {
        let mut params = Vec::new();
        params.push(-8i64); // EdDSA / Ed25519 — implemented + preferred
        Self {
            rp_id,
            rp_name,
            user_handle,
            user_name,
            challenge,
            pub_key_cred_params: params,
            user_verification: UserVerification::Preferred,
        }
    }
}

/// `PublicKeyCredentialRequestOptions` (authentication), the subset AthID uses.
#[derive(Clone, Debug)]
pub struct CredentialRequestOptions {
    pub rp_id: String,
    pub challenge: [u8; 32],
    /// Credential ids the RP will accept (empty = discoverable/resident).
    pub allow_credentials: Vec<Vec<u8>>,
    pub user_verification: UserVerification,
}

impl CredentialRequestOptions {
    pub fn new(rp_id: String, challenge: [u8; 32]) -> Self {
        Self {
            rp_id,
            challenge,
            allow_credentials: Vec::new(),
            user_verification: UserVerification::Required,
        }
    }
}

// ---------------------------------------------------------------------------
// Stored credential + in-memory store
// ---------------------------------------------------------------------------

/// A registered credential as the OS layer would persist it. Minimal by design:
/// the credential id (lookup key), the verifying public key, the last seen
/// `signCount` (clone detection), and the user handle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredCredential {
    pub credential_id: Vec<u8>,
    pub public_key: CoseKey,
    pub sign_count: u32,
    pub user_handle: Vec<u8>,
    pub aaguid: [u8; 16],
}

/// In-memory credential store keyed by credential id. The OS layer can snapshot
/// these to AthFS for persistence; this type holds no I/O.
#[derive(Clone, Debug, Default)]
pub struct CredentialStore {
    creds: Vec<StoredCredential>,
}

impl CredentialStore {
    pub fn new() -> Self {
        Self { creds: Vec::new() }
    }

    pub fn insert(&mut self, cred: StoredCredential) {
        if let Some(existing) = self
            .creds
            .iter_mut()
            .find(|c| c.credential_id == cred.credential_id)
        {
            *existing = cred;
        } else {
            self.creds.push(cred);
        }
    }

    pub fn get(&self, credential_id: &[u8]) -> Option<&StoredCredential> {
        self.creds.iter().find(|c| c.credential_id == credential_id)
    }

    pub fn get_mut(&mut self, credential_id: &[u8]) -> Option<&mut StoredCredential> {
        self.creds
            .iter_mut()
            .find(|c| c.credential_id == credential_id)
    }

    pub fn len(&self) -> usize {
        self.creds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.creds.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Registration (attestation) verification
// ---------------------------------------------------------------------------

/// Attestation conveyance the relying party supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttestationFormat {
    /// `none` — no attestation statement (the common passkey case).
    None,
    /// `packed` self-attestation: signature over (authData || clientDataHash)
    /// by the credential's own key.
    PackedSelf,
}

/// The inputs to a registration (attestation) verification.
pub struct RegistrationInput<'a> {
    pub authenticator_data: &'a [u8],
    pub client_data_json: &'a [u8],
    pub format: AttestationFormat,
    /// For `PackedSelf`: the attestation signature. Ignored for `None`.
    pub attestation_signature: &'a [u8],
}

/// Verify a registration response and return the credential to store.
///
/// Steps (WebAuthn §7.1, the parts a pure RP core can check):
/// 1. `clientDataJSON.type == "webauthn.create"`.
/// 2. challenge matches the server-issued challenge.
/// 3. origin matches the expected origin.
/// 4. `authData.rpIdHash == SHA-256(rpId)`.
/// 5. UP set; UV set if required.
/// 6. Attested credential data present → extract the COSE public key.
/// 7. For `PackedSelf`, verify the attestation signature with that key.
pub fn verify_registration(
    input: &RegistrationInput,
    options: &CredentialCreationOptions,
    expected_origin: &str,
) -> Result<StoredCredential, WebAuthnError> {
    let client = CollectedClientData::parse(input.client_data_json)?;
    if client.ceremony_type != "webauthn.create" {
        return Err(WebAuthnError::WrongClientDataType);
    }
    if !challenge_matches(&client.challenge_b64url, &options.challenge) {
        return Err(WebAuthnError::ChallengeMismatch);
    }
    if client.origin != expected_origin {
        return Err(WebAuthnError::OriginMismatch);
    }

    let auth = AuthenticatorData::parse(input.authenticator_data)?;
    let expected_rp_hash = sha256(options.rp_id.as_bytes());
    if auth.rp_id_hash != expected_rp_hash {
        return Err(WebAuthnError::RpIdHashMismatch);
    }
    if !auth.flags.user_present() {
        return Err(WebAuthnError::UserPresenceMissing);
    }
    if options.user_verification == UserVerification::Required && !auth.flags.user_verified() {
        return Err(WebAuthnError::UserVerificationMissing);
    }

    let attested = auth
        .attested_credential
        .ok_or(WebAuthnError::AttestedCredentialTruncated)?;

    // For self-attestation, verify the signature over authData || hash(clientData)
    // with the freshly minted credential key.
    if input.format == AttestationFormat::PackedSelf {
        let client_data_hash = sha256(input.client_data_json);
        let mut signed = Vec::with_capacity(input.authenticator_data.len() + 32);
        signed.extend_from_slice(input.authenticator_data);
        signed.extend_from_slice(&client_data_hash);
        verify_with_cose(&attested.public_key, &signed, input.attestation_signature)
            .map_err(|_| WebAuthnError::BadAttestation)?;
    }

    Ok(StoredCredential {
        credential_id: attested.credential_id,
        public_key: attested.public_key,
        sign_count: auth.sign_count,
        user_handle: options.user_handle.clone(),
        aaguid: attested.aaguid,
    })
}

// ---------------------------------------------------------------------------
// Authentication (assertion) verification
// ---------------------------------------------------------------------------

/// Outcome of a successful assertion: the new sign count and whether the
/// authenticator looks cloned (sign-count anomaly that did NOT hard-fail because
/// one side was zero — surfaced as advisory).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssertionOutcome {
    pub new_sign_count: u32,
    pub clone_warning: bool,
}

/// The inputs to an authentication (assertion) verification.
pub struct AssertionInput<'a> {
    pub credential_id: &'a [u8],
    pub authenticator_data: &'a [u8],
    pub client_data_json: &'a [u8],
    pub signature: &'a [u8],
}

/// Verify an authentication assertion against a stored credential, mutating its
/// `sign_count` on success.
///
/// Steps (WebAuthn §7.2):
/// 1. `clientDataJSON.type == "webauthn.get"`.
/// 2. challenge matches; origin matches.
/// 3. `authData.rpIdHash == SHA-256(rpId)`.
/// 4. UP set; UV set if required.
/// 5. `signCount` monotonicity (anti-clone).
/// 6. **Verify the signature over `authData || SHA-256(clientDataJSON)` with the
///    stored credential public key** — the core of the whole protocol.
pub fn verify_assertion(
    input: &AssertionInput,
    options: &CredentialRequestOptions,
    expected_origin: &str,
    stored: &mut StoredCredential,
) -> Result<AssertionOutcome, WebAuthnError> {
    if input.credential_id != stored.credential_id.as_slice() {
        return Err(WebAuthnError::CredentialNotFound);
    }

    let client = CollectedClientData::parse(input.client_data_json)?;
    if client.ceremony_type != "webauthn.get" {
        return Err(WebAuthnError::WrongClientDataType);
    }
    if !challenge_matches(&client.challenge_b64url, &options.challenge) {
        return Err(WebAuthnError::ChallengeMismatch);
    }
    if client.origin != expected_origin {
        return Err(WebAuthnError::OriginMismatch);
    }

    let auth = AuthenticatorData::parse(input.authenticator_data)?;
    let expected_rp_hash = sha256(options.rp_id.as_bytes());
    if auth.rp_id_hash != expected_rp_hash {
        return Err(WebAuthnError::RpIdHashMismatch);
    }
    if !auth.flags.user_present() {
        return Err(WebAuthnError::UserPresenceMissing);
    }
    if options.user_verification == UserVerification::Required && !auth.flags.user_verified() {
        return Err(WebAuthnError::UserVerificationMissing);
    }

    // Anti-clone sign-count handling (WebAuthn §7.2 step 21):
    //  - both non-zero and received <= stored => cloned authenticator (FAIL).
    //  - received == 0 while stored non-zero   => counter regressed to zero,
    //    suspicious; advisory warning (some authenticators legitimately stop
    //    counting, so this is surfaced, not hard-failed).
    //  - stored == 0 (fresh credential) advancing to any received value is the
    //    normal first-use path — no signal.
    let received = auth.sign_count;
    let mut clone_warning = false;
    if received != 0 && stored.sign_count != 0 {
        if received <= stored.sign_count {
            return Err(WebAuthnError::SignCountCloned);
        }
    } else if received == 0 && stored.sign_count != 0 {
        clone_warning = true;
    }

    // The signed message: authenticatorData || SHA-256(clientDataJSON).
    let client_data_hash = sha256(input.client_data_json);
    let mut signed = Vec::with_capacity(input.authenticator_data.len() + 32);
    signed.extend_from_slice(input.authenticator_data);
    signed.extend_from_slice(&client_data_hash);

    verify_with_cose(&stored.public_key, &signed, input.signature)?;

    // Keep the high-water mark so a (warned) regression-to-zero doesn't reset
    // the anti-clone baseline.
    stored.sign_count = stored.sign_count.max(received);
    Ok(AssertionOutcome {
        new_sign_count: received,
        clone_warning,
    })
}

// ---------------------------------------------------------------------------
// Signature verification dispatch
// ---------------------------------------------------------------------------

/// Verify `signature` over `message` with a COSE public key. All four supported
/// algorithms are real here: Ed25519 (-8), ES256 (-7), ES384 (-35), RS256
/// (-257).
///
/// - **Ed25519** signs the raw `message` bytes (no prehash); `ed25519::verify`
///   takes a fixed 64-byte signature.
/// - **ES256** is ECDSA-P256/SHA-256: the authenticator signs
///   `SHA-256(message)` and emits a DER (X9.62) signature. `p256_ecdsa::verify`
///   hashes `message` with SHA-256 *internally* and accepts the DER (or fixed
///   `r||s`) signature, so we pass the SAME `message` we pass to Ed25519 — the
///   WebAuthn signed bytes `authenticatorData || SHA-256(clientDataJSON)`. The
///   bare COSE EC2 public key is the 64-byte `x || y` concatenation.
///
/// Fail-closed: a malformed key/signature or a forged signature yields
/// `BadSignature`; never panics on attacker-controlled bytes.
fn verify_with_cose(key: &CoseKey, message: &[u8], signature: &[u8]) -> Result<(), WebAuthnError> {
    match key {
        CoseKey::Ed25519(pk) => {
            let sig: [u8; 64] = signature
                .try_into()
                .map_err(|_| WebAuthnError::BadSignature)?;
            if ed25519::verify(pk, message, &sig) {
                Ok(())
            } else {
                Err(WebAuthnError::BadSignature)
            }
        }
        CoseKey::Es256 { x, y } => {
            // Bare COSE EC2 public key: x (32) || y (32) = 64 bytes. The arrays
            // are statically 32 bytes each, so this is exactly 64 by construction.
            let mut pubkey = [0u8; 64];
            pubkey[..32].copy_from_slice(x);
            pubkey[32..].copy_from_slice(y);
            // `verify` SHA-256-hashes `message` internally (ES256 = ECDSA-P256/
            // SHA-256) and accepts the DER assertion signature as-is.
            if p256_ecdsa::verify(&pubkey, message, signature) {
                Ok(())
            } else {
                Err(WebAuthnError::BadSignature)
            }
        }
        CoseKey::Es384 { x, y } => {
            // Bare COSE EC2 P-384 public key: x (48) || y (48) = 96 bytes,
            // exactly what `p384_ecdsa::verify` accepts. It SHA-384-hashes
            // `message` internally (ES384 = ECDSA-P384/SHA-384) and accepts the
            // DER (or fixed r||s) assertion signature — the same signed bytes
            // (`authData || SHA-256(clientDataJSON)`) ES256 uses.
            let mut pubkey = [0u8; 96];
            pubkey[..48].copy_from_slice(x);
            pubkey[48..].copy_from_slice(y);
            if p384_ecdsa::verify(&pubkey, message, signature) {
                Ok(())
            } else {
                Err(WebAuthnError::BadSignature)
            }
        }
        CoseKey::Rs256 { n, e } => {
            // RS256 = RSASSA-PKCS1-v1_5 over SHA-256. `verify_pkcs1_sha256`
            // SHA-256-hashes `message` internally and runs the fail-closed EM
            // compare against `sig^e mod n`. A malformed/short/forged signature
            // returns false → BadSignature; never panics on hostile bytes.
            if rsa::verify_pkcs1_sha256(n, e, message, signature) {
                Ok(())
            } else {
                Err(WebAuthnError::BadSignature)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Minimal CBOR / COSE_Key parsing (no_std, never-panic, no new dep)
// ---------------------------------------------------------------------------

/// A bounds-checked CBOR byte reader. Supports exactly the subset a COSE_Key
/// map needs: unsigned/negative integers, byte strings, and maps thereof.
struct CborReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> CborReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn byte(&mut self) -> Result<u8, WebAuthnError> {
        let b = *self.buf.get(self.pos).ok_or(WebAuthnError::BadCoseKey)?;
        self.pos += 1;
        Ok(b)
    }

    /// Peek the CBOR major type of the next item WITHOUT consuming it. Used to
    /// disambiguate COSE label -1, which is an integer (`crv`) for EC2/OKP keys
    /// but a byte string (`n`, the modulus) for RSA keys.
    fn peek_major(&self) -> Result<u8, WebAuthnError> {
        let b = *self.buf.get(self.pos).ok_or(WebAuthnError::BadCoseKey)?;
        Ok(b >> 5)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], WebAuthnError> {
        let end = self.pos.checked_add(n).ok_or(WebAuthnError::BadCoseKey)?;
        let s = self
            .buf
            .get(self.pos..end)
            .ok_or(WebAuthnError::BadCoseKey)?;
        self.pos = end;
        Ok(s)
    }

    /// Read the (major-type, argument) header. `argument` carries the small/
    /// extended count per RFC 8949 §3.
    fn header(&mut self) -> Result<(u8, u64), WebAuthnError> {
        let ib = self.byte()?;
        let major = ib >> 5;
        let info = ib & 0x1f;
        let arg = match info {
            0..=23 => info as u64,
            24 => self.byte()? as u64,
            25 => {
                let b = self.take(2)?;
                u16::from_be_bytes([b[0], b[1]]) as u64
            }
            26 => {
                let b = self.take(4)?;
                u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64
            }
            27 => {
                let b = self.take(8)?;
                u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
            }
            _ => return Err(WebAuthnError::BadCoseKey),
        };
        Ok((major, arg))
    }

    /// Read a CBOR signed integer (major 0 unsigned, major 1 negative).
    fn read_int(&mut self) -> Result<i64, WebAuthnError> {
        let (major, arg) = self.header()?;
        match major {
            0 => i64::try_from(arg).map_err(|_| WebAuthnError::BadCoseKey),
            1 => {
                // negative integer encodes -1 - arg
                let v = i64::try_from(arg).map_err(|_| WebAuthnError::BadCoseKey)?;
                Ok(-1 - v)
            }
            _ => Err(WebAuthnError::BadCoseKey),
        }
    }

    /// Read a CBOR byte string (major 2).
    fn read_bytes(&mut self) -> Result<&'a [u8], WebAuthnError> {
        let (major, arg) = self.header()?;
        if major != 2 {
            return Err(WebAuthnError::BadCoseKey);
        }
        let n = usize::try_from(arg).map_err(|_| WebAuthnError::BadCoseKey)?;
        self.take(n)
    }

    /// Skip one complete CBOR data item (used for unknown map values). Recursion
    /// is bounded by the input length so adversarial nesting cannot loop forever.
    fn skip_item(&mut self, depth: u32) -> Result<(), WebAuthnError> {
        if depth > 32 {
            return Err(WebAuthnError::BadCoseKey);
        }
        let (major, arg) = self.header()?;
        match major {
            0 | 1 | 7 => Ok(()),
            2 | 3 => {
                let n = usize::try_from(arg).map_err(|_| WebAuthnError::BadCoseKey)?;
                self.take(n).map(|_| ())
            }
            4 => {
                for _ in 0..arg {
                    self.skip_item(depth + 1)?;
                }
                Ok(())
            }
            5 => {
                for _ in 0..arg {
                    self.skip_item(depth + 1)?;
                    self.skip_item(depth + 1)?;
                }
                Ok(())
            }
            6 => self.skip_item(depth + 1),
            _ => Err(WebAuthnError::BadCoseKey),
        }
    }
}

/// Parse a COSE_Key (RFC 9052) public key from the front of `bytes`.
///
/// Reads the integer-keyed map: kty(1), alg(3), then the type-specific params.
/// Supports OKP/Ed25519 (kty=1, crv=6, alg=-8), EC2/P-256 (kty=2, crv=1,
/// alg=-7), EC2/P-384 (kty=2, crv=2, alg=-35), and RSA (kty=3, alg=-257, with
/// `n` at label -1 and `e` at label -2). Never panics on hostile input.
///
/// Note the label overloading: for EC2/OKP, label -1 = `crv` (integer), -2 =
/// `x` (bstr), -3 = `y` (bstr). For RSA, label -1 = `n` (bstr) and -2 = `e`
/// (bstr). Label -1 is therefore disambiguated by peeking its CBOR major type.
pub fn parse_cose_key(bytes: &[u8]) -> Result<CoseKey, WebAuthnError> {
    let mut r = CborReader::new(bytes);
    let (major, n) = r.header()?;
    if major != 5 {
        return Err(WebAuthnError::BadCoseKey);
    }

    let mut kty: Option<i64> = None;
    let mut alg: Option<i64> = None;
    let mut crv: Option<i64> = None;
    // For EC2/OKP these are x/y; for RSA, label -2 (`param2`) is the exponent e
    // and `modulus` (label -1, byte string) is n.
    let mut param2: Option<Vec<u8>> = None;
    let mut y: Option<Vec<u8>> = None;
    let mut modulus: Option<Vec<u8>> = None;

    for _ in 0..n {
        let label = r.read_int()?;
        match label {
            1 => kty = Some(r.read_int()?),
            3 => alg = Some(r.read_int()?),
            -1 => {
                // EC2/OKP: crv (integer). RSA: n (byte string). Disambiguate by
                // the CBOR major type so an RSA `n` is never mis-read as an int.
                if r.peek_major()? == 2 {
                    modulus = Some(r.read_bytes()?.to_vec());
                } else {
                    crv = Some(r.read_int()?);
                }
            }
            -2 => param2 = Some(r.read_bytes()?.to_vec()),
            -3 => y = Some(r.read_bytes()?.to_vec()),
            _ => r.skip_item(0)?,
        }
    }

    let kty = kty.ok_or(WebAuthnError::BadCoseKey)?;
    match kty {
        // OKP / Ed25519
        1 => {
            // crv must be Ed25519 (6); alg, if present, must be EdDSA (-8).
            if crv != Some(6) {
                return Err(WebAuthnError::BadCoseKey);
            }
            if let Some(a) = alg {
                if a != -8 {
                    return Err(WebAuthnError::UnsupportedAlgorithm);
                }
            }
            let xv = param2.ok_or(WebAuthnError::BadCoseKey)?;
            let pk: [u8; 32] = xv
                .as_slice()
                .try_into()
                .map_err(|_| WebAuthnError::BadCoseKey)?;
            Ok(CoseKey::Ed25519(pk))
        }
        // EC2: P-256 (ES256, crv=1) or P-384 (ES384, crv=2), verified via
        // ath_crypto::p256_ecdsa / p384_ecdsa respectively.
        2 => {
            let xv = param2.ok_or(WebAuthnError::BadCoseKey)?;
            let yv = y.ok_or(WebAuthnError::BadCoseKey)?;
            match crv {
                Some(1) => {
                    if let Some(a) = alg {
                        if a != -7 {
                            return Err(WebAuthnError::UnsupportedAlgorithm);
                        }
                    }
                    let xa: [u8; 32] = xv
                        .as_slice()
                        .try_into()
                        .map_err(|_| WebAuthnError::BadCoseKey)?;
                    let ya: [u8; 32] = yv
                        .as_slice()
                        .try_into()
                        .map_err(|_| WebAuthnError::BadCoseKey)?;
                    Ok(CoseKey::Es256 { x: xa, y: ya })
                }
                Some(2) => {
                    if let Some(a) = alg {
                        if a != -35 {
                            return Err(WebAuthnError::UnsupportedAlgorithm);
                        }
                    }
                    let xa: [u8; 48] = xv
                        .as_slice()
                        .try_into()
                        .map_err(|_| WebAuthnError::BadCoseKey)?;
                    let ya: [u8; 48] = yv
                        .as_slice()
                        .try_into()
                        .map_err(|_| WebAuthnError::BadCoseKey)?;
                    Ok(CoseKey::Es384 { x: xa, y: ya })
                }
                _ => Err(WebAuthnError::BadCoseKey),
            }
        }
        // RSA (RS256 — verified via ath_crypto::rsa, RSASSA-PKCS1-v1_5/SHA-256)
        3 => {
            if let Some(a) = alg {
                if a != -257 {
                    return Err(WebAuthnError::UnsupportedAlgorithm);
                }
            }
            let nv = modulus.ok_or(WebAuthnError::BadCoseKey)?;
            let ev = param2.ok_or(WebAuthnError::BadCoseKey)?;
            // Reject empty key material outright; a real RSA modulus is hundreds
            // of bytes and a real exponent is non-empty. The verifier itself is
            // fail-closed on any malformed n/e, but this rejects garbage early.
            if nv.is_empty() || ev.is_empty() {
                return Err(WebAuthnError::BadCoseKey);
            }
            Ok(CoseKey::Rs256 { n: nv, e: ev })
        }
        _ => Err(WebAuthnError::BadCoseKey),
    }
}

/// Build a `none`-attestation registration `authenticatorData` for an Ed25519
/// credential. Useful to the OS layer (a software/platform authenticator) and to
/// the host KAT. Layout: rpIdHash(32) flags(1) signCount(4) aaguid(16)
/// credIdLen(2 BE) credId cose_key.
pub fn build_authenticator_data_ed25519(
    rp_id: &str,
    flags: u8,
    sign_count: u32,
    aaguid: &[u8; 16],
    credential_id: &[u8],
    public_key: &[u8; 32],
) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&sha256(rp_id.as_bytes()));
    out.push(flags);
    out.extend_from_slice(&sign_count.to_be_bytes());
    if flags & AuthDataFlags::AT != 0 {
        out.extend_from_slice(aaguid);
        out.extend_from_slice(&(credential_id.len() as u16).to_be_bytes());
        out.extend_from_slice(credential_id);
        out.extend_from_slice(&encode_cose_ed25519(public_key));
    }
    out
}

/// Build an assertion `authenticatorData` (no attested credential data): just the
/// rpIdHash(32) flags(1) signCount(4) prefix.
pub fn build_assertion_authenticator_data(rp_id: &str, flags: u8, sign_count: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(37);
    out.extend_from_slice(&sha256(rp_id.as_bytes()));
    out.push(flags & !AuthDataFlags::AT); // assertion carries no attested data
    out.extend_from_slice(&sign_count.to_be_bytes());
    out
}

/// CBOR-encode an Ed25519 public key as a COSE_Key map:
/// {1: 1 (OKP), 3: -8 (EdDSA), -1: 6 (Ed25519), -2: x}.
pub fn encode_cose_ed25519(public_key: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(48);
    out.push(0xA4); // map of 4 pairs
                    // 1 (kty) : 1 (OKP)
    out.push(0x01);
    out.push(0x01);
    // 3 (alg) : -8 (EdDSA)  → major 1, value 7
    out.push(0x03);
    out.push(0x27);
    // -1 (crv) : 6 (Ed25519) → key major 1 value 0 (=0x20), value 6
    out.push(0x20);
    out.push(0x06);
    // -2 (x) : bstr(32)
    out.push(0x21);
    out.push(0x58); // byte string, 1-byte length follows
    out.push(0x20); // length 32
    out.extend_from_slice(public_key);
    out
}

/// CBOR-encode an ES256 / P-256 public key as a COSE_Key map:
/// {1: 2 (EC2), 3: -7 (ES256), -1: 1 (P-256), -2: x(32), -3: y(32)}.
pub fn encode_cose_es256(x: &[u8; 32], y: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(80);
    out.push(0xA5); // map of 5 pairs
                    // 1 (kty) : 2 (EC2)
    out.push(0x01);
    out.push(0x02);
    // 3 (alg) : -7 (ES256)  → major 1, value 6
    out.push(0x03);
    out.push(0x26);
    // -1 (crv) : 1 (P-256) → key major 1 value 0 (=0x20), value 1
    out.push(0x20);
    out.push(0x01);
    // -2 (x) : bstr(32)
    out.push(0x21);
    out.push(0x58);
    out.push(0x20);
    out.extend_from_slice(x);
    // -3 (y) : bstr(32)
    out.push(0x22);
    out.push(0x58);
    out.push(0x20);
    out.extend_from_slice(y);
    out
}

/// CBOR-encode an ES384 / P-384 public key as a COSE_Key map:
/// {1: 2 (EC2), 3: -35 (ES384), -1: 2 (P-384), -2: x(48), -3: y(48)}.
pub fn encode_cose_es384(x: &[u8; 48], y: &[u8; 48]) -> Vec<u8> {
    let mut out = Vec::with_capacity(112);
    out.push(0xA5); // map of 5 pairs
                    // 1 (kty) : 2 (EC2)
    out.push(0x01);
    out.push(0x02);
    // 3 (alg) : -35 (ES384) → major 1, arg 34 (1-byte extension): 0x38 0x22
    out.push(0x03);
    out.push(0x38);
    out.push(0x22);
    // -1 (crv) : 2 (P-384) → key 0x20, value 2
    out.push(0x20);
    out.push(0x02);
    // -2 (x) : bstr(48)  → 0x21, 0x58, 0x30, <48 bytes>
    out.push(0x21);
    out.push(0x58);
    out.push(0x30);
    out.extend_from_slice(x);
    // -3 (y) : bstr(48)
    out.push(0x22);
    out.push(0x58);
    out.push(0x30);
    out.extend_from_slice(y);
    out
}

/// CBOR-encode an RSA (RS256) public key as a COSE_Key map:
/// {1: 3 (RSA), 3: -257 (RS256), -1: n (bstr), -2: e (bstr)}. `n`/`e` are the
/// big-endian modulus / public exponent exactly as stored in `CoseKey::Rs256`.
pub fn encode_cose_rsa(n: &[u8], e: &[u8]) -> Vec<u8> {
    /// Append a CBOR byte string header (major 2) for `len`, minimal-length form.
    fn push_bstr_header(out: &mut Vec<u8>, len: usize) {
        if len <= 23 {
            out.push(0x40 | len as u8);
        } else if len <= 0xFF {
            out.push(0x58);
            out.push(len as u8);
        } else if len <= 0xFFFF {
            out.push(0x59);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            out.push(0x5A);
            out.extend_from_slice(&(len as u32).to_be_bytes());
        }
    }
    let mut out = Vec::with_capacity(n.len() + e.len() + 16);
    out.push(0xA4); // map of 4 pairs
                    // 1 (kty) : 3 (RSA)
    out.push(0x01);
    out.push(0x03);
    // 3 (alg) : -257 (RS256) → major 1, arg 256 (2-byte extension): 0x39 0x01 0x00
    out.push(0x03);
    out.push(0x39);
    out.extend_from_slice(&256u16.to_be_bytes());
    // -1 (n) : bstr(modulus)
    out.push(0x20);
    push_bstr_header(&mut out, n.len());
    out.extend_from_slice(n);
    // -2 (e) : bstr(exponent)
    out.push(0x21);
    push_bstr_header(&mut out, e.len());
    out.extend_from_slice(e);
    out
}

// ---------------------------------------------------------------------------
// JSON + base64url helpers (minimal, never-panic)
// ---------------------------------------------------------------------------

/// Extract a top-level string value for `"key"` from a flat JSON object. Handles
/// the simple, escape-free values WebAuthn `clientDataJSON` uses for `type`,
/// `challenge`, and `origin`. Returns None if absent or not a plain string.
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let bytes = json.as_bytes();
    // Find `"key"` then the following `:` then the opening quote.
    let needle = alloc::format!("\"{}\"", key);
    let nbytes = needle.as_bytes();
    let mut i = 0usize;
    while i + nbytes.len() <= bytes.len() {
        if &bytes[i..i + nbytes.len()] == nbytes {
            let mut j = i + nbytes.len();
            // skip whitespace
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            if j >= bytes.len() || bytes[j] != b':' {
                i += 1;
                continue;
            }
            j += 1;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                j += 1;
            }
            if j >= bytes.len() || bytes[j] != b'"' {
                return None;
            }
            j += 1;
            let start = j;
            let mut value = String::new();
            while j < bytes.len() {
                let c = bytes[j];
                if c == b'\\' {
                    // minimal escape handling: keep the escaped char verbatim
                    if j + 1 < bytes.len() {
                        value.push(bytes[j + 1] as char);
                        j += 2;
                        continue;
                    } else {
                        return None;
                    }
                }
                if c == b'"' {
                    let _ = start;
                    return Some(value);
                }
                value.push(c as char);
                j += 1;
            }
            return None;
        }
        i += 1;
    }
    None
}

/// Does the client-echoed base64url challenge decode to exactly `expected`?
fn challenge_matches(b64url: &str, expected: &[u8; 32]) -> bool {
    match base64url_decode(b64url) {
        Some(v) => v.as_slice() == expected.as_slice(),
        None => false,
    }
}

/// base64url (no padding) encode — what `clientDataJSON.challenge` carries.
pub fn base64url_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut chunks = data.chunks_exact(3);
    for c in &mut chunks {
        let n = ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | (c[2] as u32);
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        out.push(ALPHABET[(n & 0x3f) as usize] as char);
    }
    let rem = chunks.remainder();
    match rem.len() {
        1 => {
            let n = (rem[0] as u32) << 16;
            out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        }
        2 => {
            let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
            out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
            out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        }
        _ => {}
    }
    out
}

/// base64url (padding-tolerant) decode. Returns None on any invalid character —
/// never panics.
fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }
    let mut bits = 0u32;
    let mut nbits = 0u32;
    let mut out = Vec::new();
    for &c in s.as_bytes() {
        if c == b'=' {
            break;
        }
        let v = val(c)?;
        bits = (bits << 6) | v as u32;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((bits >> nbits) as u8);
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Host KATs — FAIL-able, concrete. The R10 proof for a component crate.
// (No `use std::` — `cargo test` builds these with std available but the
// architecture-gate forbids the std-ism, so tests stay on core + alloc.)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const AAGUID: [u8; 16] = [0xAA; 16];
    const RP_ID: &str = "athenaos.local";
    const ORIGIN: &str = "https://athenaos.local";

    // A deterministic Ed25519 test credential: seed -> public key. The signature
    // vectors are computed in-test by signing with this key (so the KAT is
    // self-consistent and FAIL-able, not a pasted opaque blob).
    fn test_seed() -> [u8; 32] {
        let mut s = [0u8; 32];
        for (i, b) in s.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(7).wrapping_add(3);
        }
        s
    }

    fn client_data(ceremony: &str, challenge: &[u8; 32], origin: &str) -> Vec<u8> {
        let c = base64url_encode(challenge);
        let json = alloc::format!(
            "{{\"type\":\"{}\",\"challenge\":\"{}\",\"origin\":\"{}\",\"crossOrigin\":false}}",
            ceremony,
            c,
            origin
        );
        json.into_bytes()
    }

    fn challenge() -> [u8; 32] {
        let mut c = [0u8; 32];
        for (i, b) in c.iter_mut().enumerate() {
            *b = (i as u8) ^ 0x5A;
        }
        c
    }

    fn cred_id() -> Vec<u8> {
        vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]
    }

    /// Register a fresh Ed25519 passkey via the real `verify_registration` path
    /// and return the StoredCredential + the seed, for assertion tests.
    fn register() -> (StoredCredential, [u8; 32]) {
        let seed = test_seed();
        let pk = ed25519::derive_public_key(&seed);
        let challenge = challenge();
        let opts = CredentialCreationOptions::new(
            String::from(RP_ID),
            String::from("AthenaOS"),
            vec![0xDE, 0xAD],
            String::from("alice"),
            challenge,
        );
        let flags = AuthDataFlags::UP | AuthDataFlags::UV | AuthDataFlags::AT;
        let authdata = build_authenticator_data_ed25519(RP_ID, flags, 0, &AAGUID, &cred_id(), &pk);
        let cdj = client_data("webauthn.create", &challenge, ORIGIN);
        let input = RegistrationInput {
            authenticator_data: &authdata,
            client_data_json: &cdj,
            format: AttestationFormat::None,
            attestation_signature: &[],
        };
        let stored = verify_registration(&input, &opts, ORIGIN).expect("registration verifies");
        (stored, seed)
    }

    /// Produce a valid assertion for a stored Ed25519 credential at a given
    /// signCount, signing authData||SHA256(clientData) with `seed`.
    fn make_assertion(
        seed: &[u8; 32],
        sign_count: u32,
        flags: u8,
        challenge: &[u8; 32],
        origin: &str,
        rp_id: &str,
    ) -> (Vec<u8>, Vec<u8>, [u8; 64]) {
        let authdata = build_assertion_authenticator_data(rp_id, flags, sign_count);
        let cdj = client_data("webauthn.get", challenge, origin);
        let hash = sha256(&cdj);
        let mut signed = authdata.clone();
        signed.extend_from_slice(&hash);
        let sig = ed25519::sign(seed, &signed);
        (authdata, cdj, sig)
    }

    #[test]
    fn registration_extracts_correct_cose_key() {
        let (stored, seed) = register();
        let pk = ed25519::derive_public_key(&seed);
        assert_eq!(stored.public_key, CoseKey::Ed25519(pk));
        assert_eq!(stored.credential_id, cred_id());
        assert_eq!(stored.aaguid, AAGUID);
        assert_eq!(stored.public_key.alg(), -8);
        assert!(stored.public_key.is_verifiable());
    }

    #[test]
    fn good_assertion_verifies() {
        let (mut stored, seed) = register();
        let challenge = challenge();
        let opts = CredentialRequestOptions::new(String::from(RP_ID), challenge);
        let flags = AuthDataFlags::UP | AuthDataFlags::UV;
        let (authdata, cdj, sig) = make_assertion(&seed, 5, flags, &challenge, ORIGIN, RP_ID);
        let input = AssertionInput {
            credential_id: &cred_id(),
            authenticator_data: &authdata,
            client_data_json: &cdj,
            signature: &sig,
        };
        let outcome = verify_assertion(&input, &opts, ORIGIN, &mut stored).expect("verifies");
        assert_eq!(outcome.new_sign_count, 5);
        assert!(!outcome.clone_warning);
        assert_eq!(stored.sign_count, 5);
    }

    #[test]
    fn tampered_signature_fails() {
        let (mut stored, seed) = register();
        let challenge = challenge();
        let opts = CredentialRequestOptions::new(String::from(RP_ID), challenge);
        let flags = AuthDataFlags::UP | AuthDataFlags::UV;
        let (authdata, cdj, mut sig) = make_assertion(&seed, 5, flags, &challenge, ORIGIN, RP_ID);
        sig[0] ^= 0xFF; // flip a bit
        let input = AssertionInput {
            credential_id: &cred_id(),
            authenticator_data: &authdata,
            client_data_json: &cdj,
            signature: &sig,
        };
        assert_eq!(
            verify_assertion(&input, &opts, ORIGIN, &mut stored),
            Err(WebAuthnError::BadSignature)
        );
    }

    #[test]
    fn tampered_challenge_fails() {
        let (mut stored, seed) = register();
        let challenge = challenge();
        let opts = CredentialRequestOptions::new(String::from(RP_ID), challenge);
        let flags = AuthDataFlags::UP | AuthDataFlags::UV;
        // Sign with a DIFFERENT challenge than the server issued.
        let mut other = challenge;
        other[0] ^= 0x01;
        let (authdata, cdj, sig) = make_assertion(&seed, 5, flags, &other, ORIGIN, RP_ID);
        let input = AssertionInput {
            credential_id: &cred_id(),
            authenticator_data: &authdata,
            client_data_json: &cdj,
            signature: &sig,
        };
        assert_eq!(
            verify_assertion(&input, &opts, ORIGIN, &mut stored),
            Err(WebAuthnError::ChallengeMismatch)
        );
    }

    #[test]
    fn tampered_origin_fails() {
        let (mut stored, seed) = register();
        let challenge = challenge();
        let opts = CredentialRequestOptions::new(String::from(RP_ID), challenge);
        let flags = AuthDataFlags::UP | AuthDataFlags::UV;
        let (authdata, cdj, sig) =
            make_assertion(&seed, 5, flags, &challenge, "https://evil.example", RP_ID);
        let input = AssertionInput {
            credential_id: &cred_id(),
            authenticator_data: &authdata,
            client_data_json: &cdj,
            signature: &sig,
        };
        assert_eq!(
            verify_assertion(&input, &opts, ORIGIN, &mut stored),
            Err(WebAuthnError::OriginMismatch)
        );
    }

    #[test]
    fn wrong_rp_id_hash_fails() {
        let (mut stored, seed) = register();
        let challenge = challenge();
        let opts = CredentialRequestOptions::new(String::from(RP_ID), challenge);
        let flags = AuthDataFlags::UP | AuthDataFlags::UV;
        // authData built for a DIFFERENT rpId → rpIdHash mismatch.
        let (authdata, cdj, sig) =
            make_assertion(&seed, 5, flags, &challenge, ORIGIN, "other.rp.example");
        let input = AssertionInput {
            credential_id: &cred_id(),
            authenticator_data: &authdata,
            client_data_json: &cdj,
            signature: &sig,
        };
        assert_eq!(
            verify_assertion(&input, &opts, ORIGIN, &mut stored),
            Err(WebAuthnError::RpIdHashMismatch)
        );
    }

    #[test]
    fn sign_count_regression_is_flagged_as_cloned() {
        let (mut stored, seed) = register();
        stored.sign_count = 10; // last seen value
        let challenge = challenge();
        let opts = CredentialRequestOptions::new(String::from(RP_ID), challenge);
        let flags = AuthDataFlags::UP | AuthDataFlags::UV;
        // Authenticator reports a LOWER count than we last stored → clone.
        let (authdata, cdj, sig) = make_assertion(&seed, 4, flags, &challenge, ORIGIN, RP_ID);
        let input = AssertionInput {
            credential_id: &cred_id(),
            authenticator_data: &authdata,
            client_data_json: &cdj,
            signature: &sig,
        };
        assert_eq!(
            verify_assertion(&input, &opts, ORIGIN, &mut stored),
            Err(WebAuthnError::SignCountCloned)
        );
        // stored count must NOT advance on a rejected (cloned) assertion.
        assert_eq!(stored.sign_count, 10);
    }

    #[test]
    fn uv_required_but_absent_fails() {
        let (mut stored, seed) = register();
        let challenge = challenge();
        let mut opts = CredentialRequestOptions::new(String::from(RP_ID), challenge);
        opts.user_verification = UserVerification::Required;
        // UP set but UV NOT set.
        let flags = AuthDataFlags::UP;
        let (authdata, cdj, sig) = make_assertion(&seed, 5, flags, &challenge, ORIGIN, RP_ID);
        let input = AssertionInput {
            credential_id: &cred_id(),
            authenticator_data: &authdata,
            client_data_json: &cdj,
            signature: &sig,
        };
        assert_eq!(
            verify_assertion(&input, &opts, ORIGIN, &mut stored),
            Err(WebAuthnError::UserVerificationMissing)
        );
    }

    #[test]
    fn wrong_ceremony_type_fails() {
        let (mut stored, seed) = register();
        let challenge = challenge();
        let opts = CredentialRequestOptions::new(String::from(RP_ID), challenge);
        let flags = AuthDataFlags::UP | AuthDataFlags::UV;
        let authdata = build_assertion_authenticator_data(RP_ID, flags, 5);
        // clientData says "webauthn.create" during a get ceremony.
        let cdj = client_data("webauthn.create", &challenge, ORIGIN);
        let hash = sha256(&cdj);
        let mut signed = authdata.clone();
        signed.extend_from_slice(&hash);
        let sig = ed25519::sign(&seed, &signed);
        let input = AssertionInput {
            credential_id: &cred_id(),
            authenticator_data: &authdata,
            client_data_json: &cdj,
            signature: &sig,
        };
        assert_eq!(
            verify_assertion(&input, &opts, ORIGIN, &mut stored),
            Err(WebAuthnError::WrongClientDataType)
        );
    }

    #[test]
    fn unknown_credential_id_fails() {
        let (mut stored, seed) = register();
        let challenge = challenge();
        let opts = CredentialRequestOptions::new(String::from(RP_ID), challenge);
        let flags = AuthDataFlags::UP | AuthDataFlags::UV;
        let (authdata, cdj, sig) = make_assertion(&seed, 5, flags, &challenge, ORIGIN, RP_ID);
        let input = AssertionInput {
            credential_id: &[0xFF, 0xFF], // not the stored id
            authenticator_data: &authdata,
            client_data_json: &cdj,
            signature: &sig,
        };
        assert_eq!(
            verify_assertion(&input, &opts, ORIGIN, &mut stored),
            Err(WebAuthnError::CredentialNotFound)
        );
    }

    #[test]
    fn truncated_authdata_never_panics() {
        // Every truncation length must yield an Err, never a panic.
        let full = build_assertion_authenticator_data(RP_ID, AuthDataFlags::UP, 1);
        for len in 0..full.len() {
            let r = AuthenticatorData::parse(&full[..len]);
            assert!(r.is_err(), "len {} should fail", len);
        }
        // Full prefix parses.
        assert!(AuthenticatorData::parse(&full).is_ok());
    }

    #[test]
    fn garbage_cose_and_clientdata_never_panic() {
        // Random-ish garbage must not panic in any parser.
        let garbage: Vec<u8> = (0..200u32)
            .map(|i| (i.wrapping_mul(31) & 0xff) as u8)
            .collect();
        let _ = parse_cose_key(&garbage);
        let _ = CollectedClientData::parse(&garbage);
        let _ = AuthenticatorData::parse(&garbage);
        // Truncate the AT region of a valid registration authData at every point.
        let seed = test_seed();
        let pk = ed25519::derive_public_key(&seed);
        let flags = AuthDataFlags::UP | AuthDataFlags::AT;
        let full = build_authenticator_data_ed25519(RP_ID, flags, 0, &AAGUID, &cred_id(), &pk);
        for len in 0..full.len() {
            let _ = AuthenticatorData::parse(&full[..len]);
        }
    }

    #[test]
    fn cose_roundtrip_encode_parse() {
        let seed = test_seed();
        let pk = ed25519::derive_public_key(&seed);
        let encoded = encode_cose_ed25519(&pk);
        let parsed = parse_cose_key(&encoded).expect("parses");
        assert_eq!(parsed, CoseKey::Ed25519(pk));
    }

    // ── RFC 6979 §A.2.5 — ECDSA, curve P-256 (secp256r1), hash SHA-256. ──
    //
    // The canonical published deterministic-ECDSA vector (same one
    // `ath_crypto::p256_ecdsa` validates against). Public key (affine x,y) and
    // the signature over message "sample" with SHA-256. We use it to prove the
    // ES256 WebAuthn wiring end-to-end: a *real* P-256 signature verifies
    // through `verify_with_cose` (the exact path `verify_assertion` calls), and
    // any tamper of pubkey / signature / message is rejected — all FAIL-able.
    const ES256_QX: [u8; 32] = [
        0x60, 0xFE, 0xD4, 0xBA, 0x25, 0x5A, 0x9D, 0x31, 0xC9, 0x61, 0xEB, 0x74, 0xC6, 0x35, 0x6D,
        0x68, 0xC0, 0x49, 0xB8, 0x92, 0x3B, 0x61, 0xFA, 0x6C, 0xE6, 0x69, 0x62, 0x2E, 0x60, 0xF2,
        0x9F, 0xB6,
    ];
    const ES256_QY: [u8; 32] = [
        0x79, 0x03, 0xFE, 0x10, 0x08, 0xB8, 0xBC, 0x99, 0xA4, 0x1A, 0xE9, 0xE9, 0x56, 0x28, 0xBC,
        0x64, 0xF2, 0xF1, 0xB2, 0x0C, 0x2D, 0x7E, 0x9F, 0x51, 0x77, 0xA3, 0xC2, 0x94, 0xD4, 0x46,
        0x22, 0x99,
    ];
    const ES256_MSG: &[u8] = b"sample";
    const ES256_SIG_R: [u8; 32] = [
        0xEF, 0xD4, 0x8B, 0x2A, 0xAC, 0xB6, 0xA8, 0xFD, 0x11, 0x40, 0xDD, 0x9C, 0xD4, 0x5E, 0x81,
        0xD6, 0x9D, 0x2C, 0x87, 0x7B, 0x56, 0xAA, 0xF9, 0x91, 0xC3, 0x4D, 0x0E, 0xA8, 0x4E, 0xAF,
        0x37, 0x16,
    ];
    const ES256_SIG_S: [u8; 32] = [
        0xF7, 0xCB, 0x1C, 0x94, 0x2D, 0x65, 0x7C, 0x41, 0xD4, 0x36, 0xC7, 0xA1, 0xB6, 0xE2, 0x9F,
        0x65, 0xF3, 0xE9, 0x00, 0xDB, 0xB9, 0xAF, 0xF4, 0x06, 0x4D, 0xC4, 0xAB, 0x2F, 0x84, 0x3A,
        0xCD, 0xA8,
    ];

    /// DER (X9.62) encode SEQUENCE { INTEGER r, INTEGER s } — the form a
    /// WebAuthn assertion signature carries.
    fn es256_sig_der() -> Vec<u8> {
        fn der_int(x: &[u8]) -> Vec<u8> {
            let mut i = 0;
            while i < x.len() - 1 && x[i] == 0 {
                i += 1;
            }
            let mut body = x[i..].to_vec();
            if body[0] & 0x80 != 0 {
                body.insert(0, 0x00);
            }
            let mut out = Vec::new();
            out.push(0x02);
            out.push(body.len() as u8);
            out.extend_from_slice(&body);
            out
        }
        let mut inner = der_int(&ES256_SIG_R);
        inner.extend_from_slice(&der_int(&ES256_SIG_S));
        let mut out = Vec::new();
        out.push(0x30);
        out.push(inner.len() as u8);
        out.extend_from_slice(&inner);
        out
    }

    #[test]
    fn es256_cose_key_parses_and_is_verifiable() {
        // Hand-build / round-trip a real EC2/P-256 COSE key from the RFC 6979
        // public coordinates: {1:2, 3:-7, -1:1, -2:x, -3:y}.
        let encoded = encode_cose_es256(&ES256_QX, &ES256_QY);
        let parsed = parse_cose_key(&encoded).expect("ES256 key parses");
        assert_eq!(parsed.alg(), -7);
        // ES256 is now verifiable — no longer deferred.
        assert!(parsed.is_verifiable());
        assert_eq!(
            parsed,
            CoseKey::Es256 {
                x: ES256_QX,
                y: ES256_QY
            }
        );
    }

    #[test]
    fn es256_signature_verifies_via_cose_dispatch() {
        // The load-bearing assert: a genuine P-256/SHA-256 signature verifies
        // through the SAME `verify_with_cose` path that `verify_assertion`
        // invokes. `verify_with_cose` passes `message` straight to
        // `p256_ecdsa::verify`, which SHA-256-hashes it internally (ES256) and
        // accepts the DER signature — exactly the WebAuthn contract.
        let key = CoseKey::Es256 {
            x: ES256_QX,
            y: ES256_QY,
        };
        let der = es256_sig_der();
        assert_eq!(verify_with_cose(&key, ES256_MSG, &der), Ok(()));
    }

    #[test]
    fn es256_tampered_signature_fails() {
        let key = CoseKey::Es256 {
            x: ES256_QX,
            y: ES256_QY,
        };
        let mut der = es256_sig_der();
        let last = der.len() - 1;
        der[last] ^= 0x01; // flip a bit of s
        assert_eq!(
            verify_with_cose(&key, ES256_MSG, &der),
            Err(WebAuthnError::BadSignature)
        );
    }

    #[test]
    fn es256_tampered_message_fails() {
        let key = CoseKey::Es256 {
            x: ES256_QX,
            y: ES256_QY,
        };
        let der = es256_sig_der();
        let mut msg = ES256_MSG.to_vec();
        msg[0] ^= 0x01; // a different signed message must not verify
        assert_eq!(
            verify_with_cose(&key, &msg, &der),
            Err(WebAuthnError::BadSignature)
        );
    }

    #[test]
    fn es256_wrong_key_fails() {
        // A different (here corrupted/off-curve) key must not verify.
        let mut x = ES256_QX;
        x[0] ^= 0x01;
        let key = CoseKey::Es256 { x, y: ES256_QY };
        let der = es256_sig_der();
        assert_eq!(
            verify_with_cose(&key, ES256_MSG, &der),
            Err(WebAuthnError::BadSignature)
        );
    }

    #[test]
    fn es256_assertion_path_rejects_forged_signature() {
        // End-to-end through `verify_assertion`: an ES256 credential with a
        // bogus signature over the real WebAuthn message must fail closed with
        // BadSignature (not UnsupportedAlgorithm — the deferral is gone). This
        // exercises the full assertion message assembly
        // (authData || SHA-256(clientDataJSON)) feeding the ES256 verifier.
        let mut stored = StoredCredential {
            credential_id: cred_id(),
            public_key: CoseKey::Es256 {
                x: ES256_QX,
                y: ES256_QY,
            },
            sign_count: 0,
            user_handle: vec![],
            aaguid: AAGUID,
        };
        let challenge = challenge();
        let opts = CredentialRequestOptions::new(String::from(RP_ID), challenge);
        let flags = AuthDataFlags::UP | AuthDataFlags::UV;
        let authdata = build_assertion_authenticator_data(RP_ID, flags, 1);
        let cdj = client_data("webauthn.get", &challenge, ORIGIN);
        // A syntactically valid DER signature that is not over THIS message.
        let der = es256_sig_der();
        let input = AssertionInput {
            credential_id: &cred_id(),
            authenticator_data: &authdata,
            client_data_json: &cdj,
            signature: &der,
        };
        assert_eq!(
            verify_assertion(&input, &opts, ORIGIN, &mut stored),
            Err(WebAuthnError::BadSignature)
        );
    }

    #[test]
    fn es256_registration_extracts_verifiable_key() {
        // Register an ES256 credential and confirm the EC2 key is extracted and
        // marked verifiable (the registration ES256 on-ramp).
        let challenge = challenge();
        let opts = CredentialCreationOptions::new(
            String::from(RP_ID),
            String::from("AthenaOS"),
            vec![0xDE, 0xAD],
            String::from("alice"),
            challenge,
        );
        let flags = AuthDataFlags::UP | AuthDataFlags::UV | AuthDataFlags::AT;
        // Build registration authData with an ES256 COSE key appended.
        let mut authdata = Vec::new();
        authdata.extend_from_slice(&sha256(RP_ID.as_bytes()));
        authdata.push(flags);
        authdata.extend_from_slice(&0u32.to_be_bytes());
        authdata.extend_from_slice(&AAGUID);
        authdata.extend_from_slice(&(cred_id().len() as u16).to_be_bytes());
        authdata.extend_from_slice(&cred_id());
        authdata.extend_from_slice(&encode_cose_es256(&ES256_QX, &ES256_QY));
        let cdj = client_data("webauthn.create", &challenge, ORIGIN);
        let input = RegistrationInput {
            authenticator_data: &authdata,
            client_data_json: &cdj,
            format: AttestationFormat::None,
            attestation_signature: &[],
        };
        let stored =
            verify_registration(&input, &opts, ORIGIN).expect("ES256 registration verifies");
        assert_eq!(
            stored.public_key,
            CoseKey::Es256 {
                x: ES256_QX,
                y: ES256_QY
            }
        );
        assert_eq!(stored.public_key.alg(), -7);
        assert!(stored.public_key.is_verifiable());
    }

    #[test]
    fn es256_malformed_signature_fails_gracefully() {
        // Empty / truncated signatures must fail closed, never panic.
        let key = CoseKey::Es256 {
            x: ES256_QX,
            y: ES256_QY,
        };
        assert_eq!(
            verify_with_cose(&key, ES256_MSG, &[]),
            Err(WebAuthnError::BadSignature)
        );
        assert_eq!(
            verify_with_cose(&key, ES256_MSG, &[0x30, 0x06, 0x02, 0x01, 0x00]),
            Err(WebAuthnError::BadSignature)
        );
    }

    // ── ES384 (P-384 / SHA-384, COSE alg -35) and RS256 (RSA-2048 PKCS#1 v1.5
    //    / SHA-256, COSE alg -257) genuine WebAuthn assertion vectors. ──
    //
    // These are REAL signatures produced by the Python `cryptography` oracle
    // (openssl-equivalent) over the EXACT WebAuthn signed data this test suite
    // reconstructs: authData(`SHA-256("athenaos.local") || 0x05 || signCount=5`)
    // || SHA-256(clientDataJSON) for `webauthn.get`, challenge `challenge()`,
    // origin `ORIGIN`. The oracle self-verified each signature with the same
    // library before the bytes were embedded (cross-checked, not a blind blob).
    // Because the message is reconstructed here, a genuine registration →
    // assertion → `verify_assertion == Ok` runs end-to-end, and any tamper flips
    // the passing assertion to `BadSignature`.

    const ES384_QX: [u8; 48] = [
        0x34, 0xb0, 0x6f, 0x4b, 0x5d, 0x7b, 0x90, 0x14, 0x9f, 0xaf, 0x66, 0xa3, 0xdd, 0xd8, 0x5f,
        0xb4, 0x1e, 0x80, 0x21, 0x0b, 0x46, 0x72, 0xe7, 0x8d, 0xfb, 0x92, 0xaf, 0x95, 0xb7, 0x68,
        0xc9, 0x09, 0xf7, 0xa6, 0x4f, 0x89, 0xd9, 0x3c, 0xa2, 0xe6, 0xd7, 0xfb, 0x13, 0x14, 0x37,
        0x10, 0xdb, 0x34,
    ];
    const ES384_QY: [u8; 48] = [
        0x88, 0xf6, 0x77, 0x1c, 0x70, 0x58, 0x10, 0x67, 0x47, 0x34, 0xc8, 0x4d, 0x18, 0x09, 0x42,
        0xf7, 0xa0, 0xee, 0x75, 0x7d, 0xad, 0x13, 0x22, 0x3d, 0x50, 0xc4, 0x04, 0x17, 0x06, 0x96,
        0xb3, 0x02, 0xc6, 0xf5, 0x3c, 0xf3, 0x75, 0xaa, 0x0f, 0x1c, 0xe0, 0x58, 0x45, 0xb7, 0xba,
        0x54, 0x1a, 0x72,
    ];
    const ES384_SIG_DER: [u8; 102] = [
        0x30, 0x64, 0x02, 0x30, 0x2b, 0x98, 0xea, 0x57, 0x67, 0x55, 0x7d, 0x22, 0xbf, 0x42, 0x24,
        0x77, 0xdc, 0x03, 0xcb, 0xc9, 0xfd, 0x54, 0xa1, 0x92, 0xe4, 0x5f, 0xc0, 0x34, 0x25, 0x3f,
        0x24, 0x92, 0x93, 0x29, 0xe5, 0x06, 0xe2, 0x41, 0x3b, 0x57, 0xa9, 0x6f, 0x69, 0x9f, 0xdd,
        0xaf, 0x76, 0xf0, 0x0a, 0x3d, 0xdc, 0x00, 0x02, 0x30, 0x4c, 0x4d, 0x42, 0x05, 0x5a, 0x1a,
        0x3f, 0x92, 0x52, 0xc6, 0x40, 0xf0, 0x95, 0x15, 0x38, 0x0f, 0xf6, 0x55, 0xd3, 0x22, 0xcd,
        0x92, 0x88, 0xcc, 0x89, 0x05, 0xc2, 0x0e, 0x5d, 0x45, 0xb7, 0x88, 0xb3, 0x0e, 0xc9, 0xc4,
        0x96, 0xf6, 0x06, 0xd3, 0xff, 0x7d, 0x05, 0x1f, 0x74, 0xc2, 0x03, 0x66,
    ];

    const RS256_N: [u8; 256] = [
        0xb5, 0x59, 0x11, 0xb4, 0xe4, 0x51, 0x79, 0xb8, 0x57, 0x20, 0xd5, 0x6a, 0xe9, 0x12, 0x51,
        0x51, 0xf5, 0x8c, 0x7e, 0x0d, 0xa3, 0x16, 0x05, 0x84, 0xa3, 0x2d, 0x2b, 0x00, 0x05, 0xa0,
        0x0e, 0x61, 0x94, 0x6f, 0xa4, 0x67, 0x3e, 0xc7, 0x67, 0x9b, 0x51, 0x5e, 0xfb, 0xc6, 0xb1,
        0x4a, 0x6e, 0xf8, 0x2c, 0x31, 0xd0, 0x1a, 0x13, 0xe7, 0x0c, 0x09, 0xec, 0x6e, 0x5b, 0xbe,
        0x02, 0x1c, 0x11, 0x3b, 0xd2, 0x88, 0x1e, 0x49, 0x8d, 0x42, 0x50, 0xa8, 0x88, 0x86, 0x41,
        0x5c, 0x26, 0x23, 0x9d, 0x97, 0xaf, 0x10, 0x8f, 0xd1, 0x25, 0x26, 0xfa, 0x6f, 0xae, 0x1c,
        0xa7, 0x0b, 0x03, 0x2c, 0x0e, 0xc6, 0xe6, 0x6a, 0x86, 0xd6, 0x83, 0x9d, 0x89, 0x4a, 0x70,
        0x30, 0x28, 0x86, 0xee, 0xb7, 0x68, 0x67, 0xee, 0x50, 0x36, 0xc5, 0xfa, 0xab, 0x84, 0x46,
        0x57, 0x77, 0x2b, 0x54, 0x94, 0x97, 0x9d, 0x20, 0x65, 0x46, 0xe4, 0xe5, 0x1b, 0x17, 0xc0,
        0x3f, 0xf5, 0xb5, 0xed, 0x1d, 0xca, 0x1e, 0x92, 0xf0, 0x4f, 0x51, 0x53, 0x26, 0xa1, 0x72,
        0x13, 0x3a, 0xcb, 0xba, 0x0e, 0x38, 0x2a, 0xa0, 0x63, 0xa3, 0xf3, 0x85, 0xfe, 0xa9, 0x39,
        0x45, 0x0d, 0xe7, 0xed, 0x8c, 0xad, 0x4d, 0xae, 0xd5, 0xfa, 0xd3, 0xa2, 0x95, 0x06, 0x51,
        0x04, 0x61, 0x1e, 0x51, 0xd0, 0x39, 0xa7, 0x28, 0xff, 0x53, 0x16, 0xf3, 0x43, 0x2f, 0xc3,
        0x3e, 0xb6, 0x1d, 0x76, 0x9b, 0xee, 0x31, 0x6d, 0x06, 0x6a, 0xd7, 0xba, 0x5b, 0x82, 0xaf,
        0xa8, 0x9b, 0x60, 0xf6, 0x24, 0x40, 0xcc, 0x0b, 0x34, 0x9a, 0x60, 0xe2, 0x15, 0x99, 0xd2,
        0xbd, 0xf1, 0x77, 0x8a, 0x11, 0xf8, 0xb2, 0xe7, 0x6d, 0x46, 0xbe, 0x8a, 0x5a, 0xa8, 0xda,
        0x18, 0x42, 0xbd, 0x7c, 0x08, 0x9d, 0xd5, 0x92, 0xed, 0x06, 0x48, 0x61, 0x60, 0xea, 0x7f,
        0xed,
    ];
    const RS256_E: [u8; 3] = [0x01, 0x00, 0x01];
    const RS256_SIG: [u8; 256] = [
        0x1d, 0x7b, 0xaf, 0x78, 0xe7, 0x2c, 0x98, 0x3d, 0x88, 0x0f, 0x82, 0x45, 0x06, 0xb4, 0xf6,
        0x02, 0xaf, 0x99, 0xf1, 0x35, 0x2a, 0xbc, 0x48, 0x85, 0xb3, 0xf9, 0x63, 0x8d, 0x42, 0x9d,
        0x22, 0xb0, 0x8d, 0x3e, 0x73, 0xb1, 0xf3, 0xd1, 0xe9, 0xe6, 0x3e, 0x0a, 0x7d, 0x7b, 0x21,
        0x7a, 0xd4, 0x4f, 0xdd, 0xdb, 0x9d, 0xc3, 0x42, 0xf5, 0x83, 0xe0, 0xde, 0x53, 0x39, 0x2a,
        0xaf, 0xfb, 0xad, 0xa4, 0xed, 0x74, 0x2e, 0x64, 0x40, 0x6d, 0xdd, 0x7d, 0x48, 0x80, 0xfa,
        0xb1, 0x29, 0x68, 0x26, 0x93, 0x0a, 0x0d, 0x03, 0xfb, 0xa3, 0xcb, 0xea, 0x34, 0x5e, 0x19,
        0x71, 0xbf, 0x6b, 0xae, 0x22, 0x84, 0x95, 0x31, 0x31, 0x60, 0x64, 0x0a, 0x84, 0xa8, 0xd0,
        0xf5, 0x04, 0xc5, 0x6e, 0x4a, 0x40, 0xc6, 0x65, 0x8e, 0x59, 0x8c, 0xe6, 0x21, 0xf2, 0xea,
        0x59, 0x29, 0x50, 0x69, 0xfb, 0x74, 0x89, 0x57, 0x43, 0xd6, 0x2e, 0x8c, 0xda, 0xeb, 0x1b,
        0x36, 0x02, 0x05, 0x02, 0xc9, 0xda, 0xb0, 0xfb, 0x01, 0x89, 0x95, 0xb5, 0x02, 0x33, 0x56,
        0x39, 0x5e, 0xb8, 0x5c, 0xf1, 0xad, 0xf4, 0x1e, 0x37, 0x0e, 0x3b, 0x56, 0xfa, 0x5f, 0x1e,
        0xbc, 0xc2, 0x15, 0x7b, 0xd8, 0x82, 0x60, 0xf5, 0x48, 0xbb, 0xc7, 0x42, 0x19, 0x79, 0x80,
        0xd3, 0x34, 0x94, 0x68, 0x2d, 0xe4, 0x89, 0x15, 0x95, 0x4a, 0xa5, 0x80, 0x36, 0x37, 0xe5,
        0x0e, 0x35, 0x24, 0xe8, 0xeb, 0x0f, 0xb5, 0x75, 0x5b, 0x41, 0xbb, 0x37, 0x66, 0x8c, 0xfc,
        0x9f, 0xfd, 0x48, 0x16, 0xd3, 0x99, 0xe1, 0x7d, 0x79, 0xc8, 0x5f, 0xeb, 0xc7, 0x31, 0x27,
        0x9c, 0xa8, 0x39, 0x50, 0x41, 0xe4, 0x66, 0x4e, 0xe2, 0xad, 0xf9, 0x48, 0x98, 0x9a, 0x72,
        0x63, 0xee, 0xdd, 0xc3, 0x20, 0x00, 0x9f, 0xf8, 0x35, 0xa8, 0x10, 0x2a, 0x28, 0xed, 0xdc,
        0x56,
    ];

    /// Register a credential whose COSE public key is the pre-encoded `cose`
    /// blob (the real `verify_registration` path, `none` attestation, signCount
    /// 0), returning the StoredCredential — the on-ramp for the ES384/RS256
    /// genuine-assertion tests.
    fn register_cose(cose: &[u8]) -> StoredCredential {
        let challenge = challenge();
        let opts = CredentialCreationOptions::new(
            String::from(RP_ID),
            String::from("AthenaOS"),
            vec![0xDE, 0xAD],
            String::from("alice"),
            challenge,
        );
        let flags = AuthDataFlags::UP | AuthDataFlags::UV | AuthDataFlags::AT;
        let mut authdata = Vec::new();
        authdata.extend_from_slice(&sha256(RP_ID.as_bytes()));
        authdata.push(flags);
        authdata.extend_from_slice(&0u32.to_be_bytes());
        authdata.extend_from_slice(&AAGUID);
        authdata.extend_from_slice(&(cred_id().len() as u16).to_be_bytes());
        authdata.extend_from_slice(&cred_id());
        authdata.extend_from_slice(cose);
        let cdj = client_data("webauthn.create", &challenge, ORIGIN);
        let input = RegistrationInput {
            authenticator_data: &authdata,
            client_data_json: &cdj,
            format: AttestationFormat::None,
            attestation_signature: &[],
        };
        verify_registration(&input, &opts, ORIGIN).expect("registration verifies")
    }

    /// Run the genuine assertion (authData flags UP|UV, signCount 5, over the
    /// same message the oracle signed) against `stored` with `sig`.
    fn assert_genuine(
        stored: &mut StoredCredential,
        sig: &[u8],
    ) -> Result<AssertionOutcome, WebAuthnError> {
        let challenge = challenge();
        let opts = CredentialRequestOptions::new(String::from(RP_ID), challenge);
        let flags = AuthDataFlags::UP | AuthDataFlags::UV;
        let authdata = build_assertion_authenticator_data(RP_ID, flags, 5);
        let cdj = client_data("webauthn.get", &challenge, ORIGIN);
        let input = AssertionInput {
            credential_id: &cred_id(),
            authenticator_data: &authdata,
            client_data_json: &cdj,
            signature: sig,
        };
        verify_assertion(&input, &opts, ORIGIN, stored)
    }

    // ── ES384 ──────────────────────────────────────────────────────────────

    #[test]
    fn es384_cose_key_parses_and_is_verifiable() {
        let encoded = encode_cose_es384(&ES384_QX, &ES384_QY);
        let parsed = parse_cose_key(&encoded).expect("ES384 key parses");
        assert_eq!(parsed.alg(), -35);
        assert!(parsed.is_verifiable());
        assert_eq!(
            parsed,
            CoseKey::Es384 {
                x: ES384_QX,
                y: ES384_QY
            }
        );
    }

    #[test]
    fn es384_genuine_assertion_verifies_end_to_end() {
        // registration → assertion → verify == Ok, with a REAL P-384 signature.
        let mut stored = register_cose(&encode_cose_es384(&ES384_QX, &ES384_QY));
        assert_eq!(
            stored.public_key,
            CoseKey::Es384 {
                x: ES384_QX,
                y: ES384_QY
            }
        );
        let outcome =
            assert_genuine(&mut stored, &ES384_SIG_DER).expect("ES384 assertion verifies");
        assert_eq!(outcome.new_sign_count, 5);
        assert!(!outcome.clone_warning);
    }

    #[test]
    fn es384_tampered_signature_fails() {
        let mut stored = register_cose(&encode_cose_es384(&ES384_QX, &ES384_QY));
        let mut sig = ES384_SIG_DER.to_vec();
        let last = sig.len() - 1;
        sig[last] ^= 0x01; // flip a bit of s
        assert_eq!(
            assert_genuine(&mut stored, &sig),
            Err(WebAuthnError::BadSignature)
        );
    }

    #[test]
    fn es384_wrong_key_fails() {
        // A different (corrupted) public key must not verify the real signature.
        let mut x = ES384_QX;
        x[0] ^= 0x01;
        let mut stored = register_cose(&encode_cose_es384(&x, &ES384_QY));
        assert_eq!(
            assert_genuine(&mut stored, &ES384_SIG_DER),
            Err(WebAuthnError::BadSignature)
        );
    }

    #[test]
    fn es384_malformed_signature_fails_gracefully() {
        let key = CoseKey::Es384 {
            x: ES384_QX,
            y: ES384_QY,
        };
        assert_eq!(
            verify_with_cose(&key, b"anything", &[]),
            Err(WebAuthnError::BadSignature)
        );
        assert_eq!(
            verify_with_cose(&key, b"anything", &[0x30, 0x06, 0x02, 0x01, 0x00]),
            Err(WebAuthnError::BadSignature)
        );
    }

    // ── RS256 ──────────────────────────────────────────────────────────────

    #[test]
    fn rs256_cose_key_parses_and_is_verifiable() {
        let encoded = encode_cose_rsa(&RS256_N, &RS256_E);
        let parsed = parse_cose_key(&encoded).expect("RS256 key parses");
        assert_eq!(parsed.alg(), -257);
        assert!(parsed.is_verifiable());
        assert_eq!(
            parsed,
            CoseKey::Rs256 {
                n: RS256_N.to_vec(),
                e: RS256_E.to_vec(),
            }
        );
    }

    #[test]
    fn rs256_genuine_assertion_verifies_end_to_end() {
        // registration → assertion → verify == Ok, with a REAL RSA-2048
        // PKCS#1 v1.5 / SHA-256 signature (the Windows Hello TPM case).
        let mut stored = register_cose(&encode_cose_rsa(&RS256_N, &RS256_E));
        assert_eq!(
            stored.public_key,
            CoseKey::Rs256 {
                n: RS256_N.to_vec(),
                e: RS256_E.to_vec(),
            }
        );
        let outcome = assert_genuine(&mut stored, &RS256_SIG).expect("RS256 assertion verifies");
        assert_eq!(outcome.new_sign_count, 5);
        assert!(!outcome.clone_warning);
    }

    #[test]
    fn rs256_tampered_signature_fails() {
        let mut stored = register_cose(&encode_cose_rsa(&RS256_N, &RS256_E));
        let mut sig = RS256_SIG.to_vec();
        sig[128] ^= 0x01; // flip a bit mid-signature
        assert_eq!(
            assert_genuine(&mut stored, &sig),
            Err(WebAuthnError::BadSignature)
        );
    }

    #[test]
    fn rs256_wrong_key_fails() {
        // Corrupt the modulus: the real signature must no longer verify.
        let mut n = RS256_N;
        n[0] ^= 0x01;
        let mut stored = register_cose(&encode_cose_rsa(&n, &RS256_E));
        assert_eq!(
            assert_genuine(&mut stored, &RS256_SIG),
            Err(WebAuthnError::BadSignature)
        );
    }

    #[test]
    fn rs256_short_signature_fails_gracefully() {
        // A signature that is not exactly one modulus wide fails closed.
        let key = CoseKey::Rs256 {
            n: RS256_N.to_vec(),
            e: RS256_E.to_vec(),
        };
        assert_eq!(
            verify_with_cose(&key, b"anything", &RS256_SIG[..255]),
            Err(WebAuthnError::BadSignature)
        );
        assert_eq!(
            verify_with_cose(&key, b"anything", &[]),
            Err(WebAuthnError::BadSignature)
        );
    }

    #[test]
    fn rs256_malformed_cose_key_rejected() {
        // kty=3 (RSA) with an empty modulus must be rejected at parse
        // (fail-closed), never yielding a verifiable key.
        let empty_n: [u8; 0] = [];
        let cose = encode_cose_rsa(&empty_n, &RS256_E);
        assert_eq!(parse_cose_key(&cose), Err(WebAuthnError::BadCoseKey));

        // An RSA key advertising a mismatched alg (-7 instead of -257) is
        // rejected as UnsupportedAlgorithm. Hand-build {1:3, 3:-7, -1:n, -2:e}.
        let mut mism = Vec::new();
        mism.push(0xA4); // map(4)
        mism.push(0x01);
        mism.push(0x03); // kty: 3 (RSA)
        mism.push(0x03);
        mism.push(0x26); // alg: -7 (ES256) — WRONG for RSA
        mism.push(0x20);
        mism.push(0x59);
        mism.extend_from_slice(&(RS256_N.len() as u16).to_be_bytes());
        mism.extend_from_slice(&RS256_N); // -1: n
        mism.push(0x21);
        mism.push(0x43);
        mism.extend_from_slice(&RS256_E); // -2: e (3 bytes)
        assert_eq!(
            parse_cose_key(&mism),
            Err(WebAuthnError::UnsupportedAlgorithm)
        );

        // Truncated RSA COSE (drop the trailing bytes) must not panic.
        let short = &cose[..cose.len().min(6)];
        let _ = parse_cose_key(short);
    }

    #[test]
    fn packed_self_attestation_verifies_and_tamper_fails() {
        let seed = test_seed();
        let pk = ed25519::derive_public_key(&seed);
        let challenge = challenge();
        let opts = CredentialCreationOptions::new(
            String::from(RP_ID),
            String::from("AthenaOS"),
            vec![0xDE],
            String::from("alice"),
            challenge,
        );
        let flags = AuthDataFlags::UP | AuthDataFlags::UV | AuthDataFlags::AT;
        let authdata = build_authenticator_data_ed25519(RP_ID, flags, 0, &AAGUID, &cred_id(), &pk);
        let cdj = client_data("webauthn.create", &challenge, ORIGIN);
        // Self-attestation: sign authData || SHA256(clientData) with the cred key.
        let hash = sha256(&cdj);
        let mut signed = authdata.clone();
        signed.extend_from_slice(&hash);
        let sig = ed25519::sign(&seed, &signed);
        let input = RegistrationInput {
            authenticator_data: &authdata,
            client_data_json: &cdj,
            format: AttestationFormat::PackedSelf,
            attestation_signature: &sig,
        };
        assert!(verify_registration(&input, &opts, ORIGIN).is_ok());

        // Tampered attestation signature → BadAttestation.
        let mut bad = sig;
        bad[10] ^= 0xFF;
        let input_bad = RegistrationInput {
            authenticator_data: &authdata,
            client_data_json: &cdj,
            format: AttestationFormat::PackedSelf,
            attestation_signature: &bad,
        };
        assert_eq!(
            verify_registration(&input_bad, &opts, ORIGIN),
            Err(WebAuthnError::BadAttestation)
        );
    }

    #[test]
    fn credential_store_insert_get_update() {
        let (stored, _seed) = register();
        let mut store = CredentialStore::new();
        assert!(store.is_empty());
        store.insert(stored.clone());
        assert_eq!(store.len(), 1);
        assert!(store.get(&cred_id()).is_some());
        // Re-insert with the same id updates rather than duplicating.
        let mut updated = stored.clone();
        updated.sign_count = 42;
        store.insert(updated);
        assert_eq!(store.len(), 1);
        assert_eq!(store.get(&cred_id()).unwrap().sign_count, 42);
        assert!(store.get(&[0x99]).is_none());
    }

    #[test]
    fn base64url_roundtrip() {
        for len in 0..40usize {
            let data: Vec<u8> = (0..len as u32).map(|i| (i * 13 + 7) as u8).collect();
            let enc = base64url_encode(&data);
            let dec = base64url_decode(&enc).expect("decodes");
            assert_eq!(dec, data, "roundtrip len {}", len);
        }
        // Invalid char → None, no panic.
        assert!(base64url_decode("abc$def").is_none());
    }
}
