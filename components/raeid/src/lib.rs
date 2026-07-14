//! RaeID — account system.
//!
//! Passkeys first, optional, never required for local use.
//! Supports multiple authentication methods with session management.
//! Everything works offline. Account is optional — "guest mode" is full-featured.
//! Sync is additive, never required.
#![no_std]

extern crate alloc;

/// WebAuthn (FIDO2) relying-party ceremony core — the real attestation/assertion
/// signature-verification path behind the "passkeys first" promise. Pure logic,
/// host-KAT'd. See [`webauthn`] module docs: this is OPT-IN for app/web auth and
/// is NEVER required for local login (guest mode + local password/PIN stay full).
pub mod webauthn;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// 1. Core identity types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UserId(pub u64);

/// Monotonic counter for generating unique user IDs.
static NEXT_USER_ID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

pub fn allocate_user_id() -> UserId {
    UserId(NEXT_USER_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed))
}

/// Guest user always has ID 0 — present on every system, never requires auth.
pub const GUEST_USER_ID: UserId = UserId(0);

// ---------------------------------------------------------------------------
// 2. User preferences
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserPreferences {
    pub theme: ThemePreference,
    pub locale: LocaleId,
    pub timezone_offset_minutes: i16,
    pub shell_variant: ShellVariant,
    pub input_method: InputMethod,
    pub reduce_motion: bool,
    pub high_contrast: bool,
    pub font_scale_percent: u8,
    pub custom_kvs: BTreeMap<String, String>,
}

impl Default for UserPreferences {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            locale: LocaleId::EnUs,
            timezone_offset_minutes: 0,
            shell_variant: ShellVariant::Default,
            input_method: InputMethod::Default,
            reduce_motion: false,
            high_contrast: false,
            font_scale_percent: 100,
            custom_kvs: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemePreference {
    Light,
    Dark,
    System,
    Custom(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocaleId {
    EnUs,
    EnGb,
    FrFr,
    DeDe,
    JaJp,
    ZhCn,
    KoKr,
    EsEs,
    PtBr,
    ArSa,
    Other(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellVariant {
    Default,
    Tiling,
    Floating,
    GameOS,
    Minimal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMethod {
    Default,
    CJK,
    Arabic,
    Custom(u16),
}

// ---------------------------------------------------------------------------
// 3. User profile
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct UserProfile {
    pub id: UserId,
    pub username: String,
    pub display_name: String,
    pub avatar_hash: Option<[u8; 32]>,
    pub preferences: UserPreferences,
    pub created_at: u64,
    pub last_login: u64,
    pub is_local: bool,
    pub email: Option<String>,
    pub locked: bool,
    pub failed_attempts: u32,
    pub lockout_until: u64,
}

impl UserProfile {
    pub fn new_local(id: UserId, username: String, display_name: String, now: u64) -> Self {
        Self {
            id,
            username,
            display_name,
            avatar_hash: None,
            preferences: UserPreferences::default(),
            created_at: now,
            last_login: now,
            is_local: true,
            email: None,
            locked: false,
            failed_attempts: 0,
            lockout_until: 0,
        }
    }

    pub fn guest(now: u64) -> Self {
        Self::new_local(
            GUEST_USER_ID,
            String::from("guest"),
            String::from("Guest"),
            now,
        )
    }

    pub fn is_guest(&self) -> bool {
        self.id == GUEST_USER_ID
    }

    pub fn is_locked_out(&self, now: u64) -> bool {
        self.locked || now < self.lockout_until
    }

    pub fn record_failed_attempt(&mut self, now: u64) {
        self.failed_attempts += 1;
        if self.failed_attempts >= MAX_FAILED_ATTEMPTS {
            self.lockout_until = now + LOCKOUT_DURATION_SECS;
        }
    }

    pub fn clear_failed_attempts(&mut self) {
        self.failed_attempts = 0;
        self.lockout_until = 0;
    }
}

const MAX_FAILED_ATTEMPTS: u32 = 5;
const LOCKOUT_DURATION_SECS: u64 = 300;

// ---------------------------------------------------------------------------
// 4. Passkey / WebAuthn-style authentication
// ---------------------------------------------------------------------------

/// Relying Party identifier — the domain or system name that issued the credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelyingParty {
    pub id: String,
    pub name: String,
}

impl RelyingParty {
    pub fn local() -> Self {
        Self {
            id: String::from("local.raeenos"),
            name: String::from("RaeenOS Local"),
        }
    }
}

/// A stored passkey credential following WebAuthn conventions.
#[derive(Clone, Debug)]
pub struct PasskeyCredential {
    pub credential_id: [u8; 32],
    pub public_key: [u8; 64],
    pub rp: RelyingParty,
    pub sign_count: u32,
    pub created_at: u64,
    pub last_used: u64,
    pub transports: Vec<PasskeyTransport>,
    pub user_verified: bool,
    pub aaguid: [u8; 16],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasskeyTransport {
    Usb,
    Nfc,
    Ble,
    Internal,
    Hybrid,
}

/// Challenge issued by the server side for a passkey ceremony.
#[derive(Clone, Debug)]
pub struct PasskeyChallenge {
    pub challenge: [u8; 32],
    pub rp: RelyingParty,
    pub user_id: UserId,
    pub timeout_secs: u32,
    pub created_at: u64,
    pub user_verification: UserVerificationRequirement,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UserVerificationRequirement {
    Required,
    Preferred,
    Discouraged,
}

impl PasskeyChallenge {
    pub fn is_expired(&self, now: u64) -> bool {
        now > self.created_at + self.timeout_secs as u64
    }
}

/// The response from the authenticator during a passkey ceremony.
#[derive(Clone, Debug)]
pub struct PasskeyResponse {
    pub credential_id: [u8; 32],
    pub authenticator_data: Vec<u8>,
    pub signature: Vec<u8>,
    pub client_data_hash: [u8; 32],
    pub user_handle: Option<Vec<u8>>,
}

/// Registration response for creating a new passkey.
#[derive(Clone, Debug)]
pub struct PasskeyRegistration {
    pub credential_id: [u8; 32],
    pub public_key: [u8; 64],
    pub authenticator_data: Vec<u8>,
    pub attestation: Vec<u8>,
    pub transports: Vec<PasskeyTransport>,
    pub aaguid: [u8; 16],
}

// ---------------------------------------------------------------------------
// 5. PIN / password fallback (Argon2-style hashing)
// ---------------------------------------------------------------------------

/// Which key-derivation function produced a stored password hash. Tagged into
/// the credential so a hash made by an older scheme can never be silently
/// re-interpreted under a newer one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasswordAlgorithm {
    /// Real memory-hard Argon2id (RFC 9106) via `rae_crypto`. The default and
    /// the only algorithm produced for new credentials.
    Argon2id,
    /// DEPRECATED homebrew "iterative mixing" (pre-2026-06): NOT cryptographic
    /// — no memory-hardness, ad-hoc `wrapping_mul` rounds. Retained ONLY so a
    /// credential created with it still verifies; never produced for new ones.
    LegacyMixV1,
}

/// Parameters for password hashing. For `Argon2id` these are the standard
/// Argon2 cost parameters; `memory_cost_kb` is bounded by the kernel heap
/// (~32 MiB), so the default sits well under that.
#[derive(Clone, Debug)]
pub struct PasswordParams {
    pub algorithm: PasswordAlgorithm,
    pub salt: [u8; 16],
    pub iterations: u32,
    pub memory_cost_kb: u32,
    pub parallelism: u8,
}

impl Default for PasswordParams {
    fn default() -> Self {
        Self {
            algorithm: PasswordAlgorithm::Argon2id,
            // t=3, m=8 MiB, p=1: strongly memory-hard yet safe on the ~32 MiB
            // kernel heap and fast enough for interactive login. (The former
            // 64 MiB cost would OOM the real Argon2id allocator.)
            salt: [0u8; 16],
            iterations: 3,
            memory_cost_kb: 8_192,
            parallelism: 1,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PasswordCredential {
    pub hash: [u8; 32],
    pub params: PasswordParams,
    pub created_at: u64,
    pub last_changed: u64,
    pub requires_change: bool,
}

/// Hash a password to a 32-byte tag using the algorithm named in `params`.
/// New credentials always use [`PasswordAlgorithm::Argon2id`] (a real,
/// RFC 9106 memory-hard KDF in `rae_crypto`); the legacy path exists only so
/// pre-existing `LegacyMixV1` credentials can still be verified.
pub fn hash_password(password: &[u8], params: &PasswordParams) -> [u8; 32] {
    match params.algorithm {
        PasswordAlgorithm::Argon2id => {
            let mut out = [0u8; 32];
            rae_crypto::argon2id_derive(
                password,
                &params.salt,
                params.iterations,
                params.memory_cost_kb,
                params.parallelism,
                &mut out,
            );
            out
        }
        PasswordAlgorithm::LegacyMixV1 => legacy_mix_v1(password, params),
    }
}

/// DEPRECATED. The former homebrew "iterative mixing" hash — NOT cryptographic
/// (no memory-hardness, ad-hoc `wrapping_mul` rounds). Kept private and used
/// ONLY to verify credentials that were created with it. Never call this for a
/// new credential; use [`hash_password`] with `PasswordAlgorithm::Argon2id`.
fn legacy_mix_v1(password: &[u8], params: &PasswordParams) -> [u8; 32] {
    let mut state = [0u8; 32];

    // Initial mix: XOR salt into state
    for i in 0..16 {
        state[i] = params.salt[i];
        state[i + 16] = params.salt[i] ^ 0xFF;
    }

    // Mix in password bytes
    for (i, &b) in password.iter().enumerate() {
        state[i % 32] ^= b;
        state[(i + 13) % 32] = state[(i + 13) % 32].wrapping_add(b);
    }

    // Iterative mixing rounds
    for round in 0..params.iterations {
        for j in 0..32 {
            let prev = state[(j + 31) % 32];
            let next = state[(j + 1) % 32];
            state[j] = state[j]
                .wrapping_add(prev)
                .wrapping_mul(37)
                .wrapping_add(next)
                .wrapping_add(round as u8)
                .wrapping_add(j as u8);
        }
        // Reverse pass for diffusion
        for j in (0..32).rev() {
            let prev = state[(j + 1) % 32];
            state[j] ^= prev.wrapping_mul(7).wrapping_add(state[(j + 17) % 32]);
        }
    }

    state
}

pub fn verify_password(password: &[u8], credential: &PasswordCredential) -> bool {
    let computed = hash_password(password, &credential.params);
    constant_time_eq(&computed, &credential.hash)
}

/// Constant-time comparison to prevent timing attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Estimated strength of a password, for account-creation feedback (Win11/macOS
/// parity). Ordered so `>=` comparisons express a policy floor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PasswordStrength {
    VeryWeak = 0,
    Weak = 1,
    Fair = 2,
    Strong = 3,
    VeryStrong = 4,
}

/// A short list of the most-abused passwords — always rated `VeryWeak` no matter
/// how long. A signal, NOT a full breach dictionary (that is a services-layer
/// concern with a real HIBP-style set).
const COMMON_PASSWORDS: &[&[u8]] = &[
    b"password",
    b"123456",
    b"123456789",
    b"12345678",
    b"qwerty",
    b"abc123",
    b"password1",
    b"111111",
    b"letmein",
    b"welcome",
    b"admin",
    b"iloveyou",
    b"monkey",
    b"dragon",
    b"qwerty123",
    b"changeme",
];

/// Whether an ascending run of ≥4 consecutive-by-byte characters (`abcd`,
/// `1234`) appears — a common weak pattern.
fn has_sequential_run(pw: &[u8]) -> bool {
    if pw.len() < 4 {
        return false;
    }
    let mut run = 1u32;
    for w in pw.windows(2) {
        if w[1] == w[0].wrapping_add(1) {
            run += 1;
            if run >= 4 {
                return true;
            }
        } else {
            run = 1;
        }
    }
    false
}

/// Estimate password strength for account-creation UI feedback and a policy
/// floor. Deterministic and dictionary-light: rewards length + character-class
/// diversity, and forces `VeryWeak` for empty / all-identical / common-listed
/// passwords. This is GUIDANCE, never a substitute for the Argon2id hashing that
/// actually protects the stored credential ([`hash_password`]).
pub fn estimate_password_strength(password: &[u8]) -> PasswordStrength {
    let len = password.len();
    if len == 0 {
        return PasswordStrength::VeryWeak;
    }
    // All-identical characters (aaaa, 1111).
    if password.iter().all(|&b| b == password[0]) {
        return PasswordStrength::VeryWeak;
    }
    // Case-insensitive common-password match.
    let lower: Vec<u8> = password.iter().map(|b| b.to_ascii_lowercase()).collect();
    if COMMON_PASSWORDS.iter().any(|c| *c == lower.as_slice()) {
        return PasswordStrength::VeryWeak;
    }

    let mut classes = 0i32;
    if password.iter().any(|b| b.is_ascii_lowercase()) {
        classes += 1;
    }
    if password.iter().any(|b| b.is_ascii_uppercase()) {
        classes += 1;
    }
    if password.iter().any(|b| b.is_ascii_digit()) {
        classes += 1;
    }
    if password.iter().any(|b| !b.is_ascii_alphanumeric()) {
        classes += 1;
    }

    let mut score: i32 = (len as i32).min(16); // length, capped
    score += (classes - 1).max(0) * 3; // class diversity bonus
    if has_sequential_run(password) {
        score -= 4;
    }
    if len < 8 {
        score = score.min(6); // short passwords cannot rate above Weak
    }

    match score {
        s if s >= 22 => PasswordStrength::VeryStrong,
        s if s >= 17 => PasswordStrength::Strong,
        s if s >= 11 => PasswordStrength::Fair,
        s if s >= 6 => PasswordStrength::Weak,
        _ => PasswordStrength::VeryWeak,
    }
}

// ---------------------------------------------------------------------------
// 5b. Account persistence (serialize to / from the on-disk accounts file)
// ---------------------------------------------------------------------------

/// A persisted local account: identity + its password credential. The kernel
/// writes a sequence of these to the RaeFS root so the installed system has
/// working logins across reboots (only the Argon2id hash is stored, never the
/// plaintext password).
#[derive(Clone, Debug)]
pub struct AccountRecord {
    pub username: String,
    pub display_name: String,
    pub credential: PasswordCredential,
}

const ACCOUNTS_MAGIC: &[u8; 8] = b"RAEACCT\x01";
// Fixed credential tail: algo(1) salt(16) hash(32) iter(4) mem(4) par(1)
// created(8) changed(8) requires_change(1).
const CRED_TAIL_LEN: usize = 1 + 16 + 32 + 4 + 4 + 1 + 8 + 8 + 1;

fn algo_to_u8(a: PasswordAlgorithm) -> u8 {
    match a {
        PasswordAlgorithm::Argon2id => 0,
        PasswordAlgorithm::LegacyMixV1 => 1,
    }
}
fn algo_from_u8(b: u8) -> PasswordAlgorithm {
    match b {
        0 => PasswordAlgorithm::Argon2id,
        _ => PasswordAlgorithm::LegacyMixV1,
    }
}

/// Serialize accounts to the on-disk format: magic + LE u32 count + records.
/// Each record is `LE16(username) username LE16(display) display` then a fixed
/// credential tail.
pub fn serialize_accounts(accounts: &[AccountRecord]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(ACCOUNTS_MAGIC);
    out.extend_from_slice(&(accounts.len() as u32).to_le_bytes());
    for a in accounts {
        let u = a.username.as_bytes();
        let d = a.display_name.as_bytes();
        out.extend_from_slice(&(u.len() as u16).to_le_bytes());
        out.extend_from_slice(u);
        out.extend_from_slice(&(d.len() as u16).to_le_bytes());
        out.extend_from_slice(d);
        let c = &a.credential;
        out.push(algo_to_u8(c.params.algorithm));
        out.extend_from_slice(&c.params.salt);
        out.extend_from_slice(&c.hash);
        out.extend_from_slice(&c.params.iterations.to_le_bytes());
        out.extend_from_slice(&c.params.memory_cost_kb.to_le_bytes());
        out.push(c.params.parallelism);
        out.extend_from_slice(&c.created_at.to_le_bytes());
        out.extend_from_slice(&c.last_changed.to_le_bytes());
        out.push(c.requires_change as u8);
    }
    out
}

/// Parse accounts written by [`serialize_accounts`]. Bounds-checked: malformed
/// or truncated input yields the records parsed so far (fail-safe, never panics).
pub fn deserialize_accounts(bytes: &[u8]) -> Vec<AccountRecord> {
    let mut accounts = Vec::new();
    if bytes.len() < 12 || &bytes[0..8] != ACCOUNTS_MAGIC {
        return accounts;
    }
    let count = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let mut p = 12usize;
    let take_str = |bytes: &[u8], p: &mut usize| -> Option<String> {
        if *p + 2 > bytes.len() {
            return None;
        }
        let len = u16::from_le_bytes([bytes[*p], bytes[*p + 1]]) as usize;
        *p += 2;
        if *p + len > bytes.len() {
            return None;
        }
        let s = core::str::from_utf8(&bytes[*p..*p + len])
            .ok()
            .map(String::from);
        *p += len;
        s
    };
    for _ in 0..count {
        let username = match take_str(bytes, &mut p) {
            Some(s) => s,
            None => break,
        };
        let display_name = match take_str(bytes, &mut p) {
            Some(s) => s,
            None => break,
        };
        if p + CRED_TAIL_LEN > bytes.len() {
            break;
        }
        let algorithm = algo_from_u8(bytes[p]);
        p += 1;
        let mut salt = [0u8; 16];
        salt.copy_from_slice(&bytes[p..p + 16]);
        p += 16;
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&bytes[p..p + 32]);
        p += 32;
        let iterations = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
        p += 4;
        let memory_cost_kb = u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap());
        p += 4;
        let parallelism = bytes[p];
        p += 1;
        let created_at = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
        p += 8;
        let last_changed = u64::from_le_bytes(bytes[p..p + 8].try_into().unwrap());
        p += 8;
        let requires_change = bytes[p] != 0;
        p += 1;
        accounts.push(AccountRecord {
            username,
            display_name,
            credential: PasswordCredential {
                hash,
                params: PasswordParams {
                    algorithm,
                    salt,
                    iterations,
                    memory_cost_kb,
                    parallelism,
                },
                created_at,
                last_changed,
                requires_change,
            },
        });
    }
    accounts
}

// ---------------------------------------------------------------------------
// 6. Unified auth method enum
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum AuthMethod {
    Passkey(PasskeyCredential),
    Password(PasswordCredential),
    Pin(PasswordCredential),
    Biometric { template_hash: [u8; 32] },
    LocalOnly,
}

impl AuthMethod {
    pub fn kind(&self) -> AuthMethodKind {
        match self {
            Self::Passkey(_) => AuthMethodKind::Passkey,
            Self::Password(_) => AuthMethodKind::Password,
            Self::Pin(_) => AuthMethodKind::Pin,
            Self::Biometric { .. } => AuthMethodKind::Biometric,
            Self::LocalOnly => AuthMethodKind::LocalOnly,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMethodKind {
    Passkey,
    Password,
    Pin,
    Biometric,
    LocalOnly,
}

// ---------------------------------------------------------------------------
// 7. Session management
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionToken(pub [u8; 32]);

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub device_id: [u8; 16],
    pub device_name: String,
    pub os_version: String,
    pub last_ip_hash: Option<[u8; 32]>,
}

/// Permissions attached to a session — subset of what the user is allowed to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionPermissions {
    bits: u32,
}

impl SessionPermissions {
    pub const NONE: Self = Self { bits: 0 };
    pub const FULL: Self = Self { bits: u32::MAX };

    pub const READ_PROFILE: u32 = 1 << 0;
    pub const WRITE_PROFILE: u32 = 1 << 1;
    pub const MANAGE_AUTH: u32 = 1 << 2;
    pub const MANAGE_SESSIONS: u32 = 1 << 3;
    pub const SYNC_DATA: u32 = 1 << 4;
    pub const ADMIN: u32 = 1 << 5;
    pub const MANAGE_USERS: u32 = 1 << 6;
    pub const INSTALL_APPS: u32 = 1 << 7;

    pub const fn new(bits: u32) -> Self {
        Self { bits }
    }

    pub const fn has(&self, perm: u32) -> bool {
        self.bits & perm != 0
    }

    pub const fn with(self, perm: u32) -> Self {
        Self {
            bits: self.bits | perm,
        }
    }

    pub fn default_user() -> Self {
        Self::new(
            Self::READ_PROFILE
                | Self::WRITE_PROFILE
                | Self::MANAGE_AUTH
                | Self::SYNC_DATA
                | Self::INSTALL_APPS,
        )
    }

    pub fn admin() -> Self {
        Self::FULL
    }

    pub fn guest() -> Self {
        Self::new(Self::READ_PROFILE | Self::INSTALL_APPS)
    }
}

#[derive(Clone, Debug)]
pub struct Session {
    pub token: SessionToken,
    pub user_id: UserId,
    pub created_at: u64,
    pub expires_at: u64,
    pub last_activity: u64,
    pub is_active: bool,
    pub device: DeviceInfo,
    pub permissions: SessionPermissions,
    pub refresh_count: u32,
    pub max_refreshes: u32,
    pub auth_method_used: AuthMethodKind,
}

impl Session {
    pub fn is_valid(&self, now: u64) -> bool {
        self.is_active && now < self.expires_at
    }

    pub fn is_expired(&self, now: u64) -> bool {
        now >= self.expires_at
    }

    pub fn can_refresh(&self) -> bool {
        self.refresh_count < self.max_refreshes
    }

    pub fn refresh(&mut self, now: u64, new_token: SessionToken, duration: u64) {
        self.token = new_token;
        self.last_activity = now;
        self.expires_at = now + duration;
        self.refresh_count += 1;
    }

    pub fn touch(&mut self, now: u64) {
        self.last_activity = now;
    }

    pub fn revoke(&mut self) {
        self.is_active = false;
    }

    pub fn has_permission(&self, perm: u32) -> bool {
        self.permissions.has(perm)
    }

    pub fn idle_duration(&self, now: u64) -> u64 {
        now.saturating_sub(self.last_activity)
    }
}

/// Default session duration: 24 hours.
pub const DEFAULT_SESSION_DURATION: u64 = 86400;
/// Extended session for "remember me": 30 days.
pub const EXTENDED_SESSION_DURATION: u64 = 86400 * 30;
/// Max idle time before session auto-expires: 1 hour.
pub const SESSION_IDLE_TIMEOUT: u64 = 3600;
/// Default max refresh count.
pub const DEFAULT_MAX_REFRESHES: u32 = 48;

// ---------------------------------------------------------------------------
// 8. Auth results and errors
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum AuthResult {
    Success {
        session: Session,
        is_new_device: bool,
    },
    Failed(AuthError),
    LockedOut {
        until: u64,
    },
    RequiresSecondFactor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthError {
    InvalidCredentials,
    AccountLocked,
    AccountDisabled,
    SessionExpired,
    SessionRevoked,
    ChallengeExpired,
    ChallengeMismatch,
    InvalidSignature,
    SignCountRegression,
    CredentialNotFound,
    UserNotFound,
    NotConfigured,
    TooManyAttempts,
    PermissionDenied,
    InternalError,
}

// ---------------------------------------------------------------------------
// 9. Passkey ceremony manager
// ---------------------------------------------------------------------------

pub struct PasskeyCeremony {
    pending_challenges: BTreeMap<UserId, PasskeyChallenge>,
    pending_registrations: BTreeMap<UserId, PasskeyChallenge>,
}

impl PasskeyCeremony {
    pub fn new() -> Self {
        Self {
            pending_challenges: BTreeMap::new(),
            pending_registrations: BTreeMap::new(),
        }
    }

    /// Create a registration challenge for adding a new passkey.
    pub fn create_registration_challenge(
        &mut self,
        user_id: UserId,
        rp: RelyingParty,
        challenge_bytes: [u8; 32],
        now: u64,
    ) -> &PasskeyChallenge {
        let challenge = PasskeyChallenge {
            challenge: challenge_bytes,
            rp,
            user_id,
            timeout_secs: 120,
            created_at: now,
            user_verification: UserVerificationRequirement::Preferred,
        };
        self.pending_registrations.insert(user_id, challenge);
        self.pending_registrations.get(&user_id).unwrap()
    }

    /// Verify a registration response and produce a credential.
    pub fn complete_registration(
        &mut self,
        user_id: UserId,
        registration: &PasskeyRegistration,
        now: u64,
    ) -> Result<PasskeyCredential, AuthError> {
        let challenge = self
            .pending_registrations
            .remove(&user_id)
            .ok_or(AuthError::ChallengeExpired)?;

        if challenge.is_expired(now) {
            return Err(AuthError::ChallengeExpired);
        }

        Ok(PasskeyCredential {
            credential_id: registration.credential_id,
            public_key: registration.public_key,
            rp: challenge.rp,
            sign_count: 0,
            created_at: now,
            last_used: now,
            transports: registration.transports.clone(),
            user_verified: true,
            aaguid: registration.aaguid,
        })
    }

    /// Create an authentication challenge for login.
    pub fn create_auth_challenge(
        &mut self,
        user_id: UserId,
        rp: RelyingParty,
        challenge_bytes: [u8; 32],
        now: u64,
    ) -> &PasskeyChallenge {
        let challenge = PasskeyChallenge {
            challenge: challenge_bytes,
            rp,
            user_id,
            timeout_secs: 60,
            created_at: now,
            user_verification: UserVerificationRequirement::Required,
        };
        self.pending_challenges.insert(user_id, challenge);
        self.pending_challenges.get(&user_id).unwrap()
    }

    /// Verify an authentication response against stored credentials.
    pub fn verify_auth_response(
        &mut self,
        user_id: UserId,
        response: &PasskeyResponse,
        stored: &mut PasskeyCredential,
        now: u64,
    ) -> Result<(), AuthError> {
        let challenge = self
            .pending_challenges
            .remove(&user_id)
            .ok_or(AuthError::ChallengeExpired)?;

        if challenge.is_expired(now) {
            return Err(AuthError::ChallengeExpired);
        }

        if response.credential_id != stored.credential_id {
            return Err(AuthError::CredentialNotFound);
        }

        // Extract sign count from authenticator_data (bytes 33..37 in WebAuthn spec)
        let response_sign_count = if response.authenticator_data.len() >= 37 {
            u32::from_be_bytes([
                response.authenticator_data[33],
                response.authenticator_data[34],
                response.authenticator_data[35],
                response.authenticator_data[36],
            ])
        } else {
            return Err(AuthError::InvalidSignature);
        };

        // Detect cloned authenticator via sign count regression
        if response_sign_count != 0
            && stored.sign_count != 0
            && response_sign_count <= stored.sign_count
        {
            return Err(AuthError::SignCountRegression);
        }

        // Verify the WebAuthn assertion signature over
        // `authenticator_data || client_data_hash` against the stored ES256
        // (ECDSA P-256 + SHA-256, COSE alg -7) public key. This is the actual
        // authentication gate — without it ANY non-empty signature would pass.
        // Fail-closed: a forged, tampered, wrong-key, or empty signature is
        // rejected. `rae_crypto::p256_ecdsa::verify` hashes with SHA-256
        // internally, accepts the bare 64-byte COSE `X||Y` key + a DER (or raw
        // r||s) signature, and never panics on attacker-controlled bytes.
        let mut signed = Vec::with_capacity(response.authenticator_data.len() + 32);
        signed.extend_from_slice(&response.authenticator_data);
        signed.extend_from_slice(&response.client_data_hash);
        if !rae_crypto::p256_ecdsa::verify(&stored.public_key, &signed, &response.signature) {
            return Err(AuthError::InvalidSignature);
        }

        stored.sign_count = response_sign_count;
        stored.last_used = now;

        Ok(())
    }

    /// Expire old challenges that have timed out.
    pub fn cleanup_expired(&mut self, now: u64) {
        self.pending_challenges.retain(|_, c| !c.is_expired(now));
        self.pending_registrations.retain(|_, c| !c.is_expired(now));
    }
}

// ---------------------------------------------------------------------------
// 10. User switching
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchPolicy {
    /// Lock the current session, require auth for previous user to return.
    LockPrevious,
    /// Keep previous session active (fast switching).
    KeepActive,
    /// End previous session.
    EndPrevious,
}

pub struct UserSwitcher {
    active_user: UserId,
    user_stack: Vec<UserId>,
    policy: SwitchPolicy,
}

impl UserSwitcher {
    pub fn new(initial_user: UserId, policy: SwitchPolicy) -> Self {
        Self {
            active_user: initial_user,
            user_stack: vec![initial_user],
            policy,
        }
    }

    pub fn active_user(&self) -> UserId {
        self.active_user
    }

    pub fn switch_to(&mut self, user_id: UserId) -> SwitchEvent {
        let previous = self.active_user;
        self.active_user = user_id;

        match self.policy {
            SwitchPolicy::LockPrevious => {
                self.user_stack.push(user_id);
                SwitchEvent::Switched {
                    from: previous,
                    to: user_id,
                    previous_locked: true,
                }
            }
            SwitchPolicy::KeepActive => {
                if !self.user_stack.contains(&user_id) {
                    self.user_stack.push(user_id);
                }
                SwitchEvent::Switched {
                    from: previous,
                    to: user_id,
                    previous_locked: false,
                }
            }
            SwitchPolicy::EndPrevious => {
                self.user_stack.retain(|&id| id != previous);
                self.user_stack.push(user_id);
                SwitchEvent::SwitchedWithLogout {
                    from: previous,
                    to: user_id,
                }
            }
        }
    }

    pub fn switch_back(&mut self) -> Option<SwitchEvent> {
        if self.user_stack.len() <= 1 {
            return None;
        }
        // Remove current user from stack and switch to the one below
        self.user_stack.pop();
        let previous = self.active_user;
        self.active_user = *self.user_stack.last().unwrap();
        Some(SwitchEvent::Switched {
            from: previous,
            to: self.active_user,
            previous_locked: false,
        })
    }

    pub fn active_users(&self) -> &[UserId] {
        &self.user_stack
    }

    pub fn set_policy(&mut self, policy: SwitchPolicy) {
        self.policy = policy;
    }
}

#[derive(Clone, Copy, Debug)]
pub enum SwitchEvent {
    Switched {
        from: UserId,
        to: UserId,
        previous_locked: bool,
    },
    SwitchedWithLogout {
        from: UserId,
        to: UserId,
    },
}

// ---------------------------------------------------------------------------
// 11. Account manager — the top-level orchestrator
// ---------------------------------------------------------------------------

pub struct AccountManager {
    users: BTreeMap<UserId, UserProfile>,
    auth_methods: BTreeMap<UserId, Vec<AuthMethod>>,
    sessions: BTreeMap<SessionToken, Session>,
    ceremony: PasskeyCeremony,
    switcher: UserSwitcher,
    guest_enabled: bool,
}

impl AccountManager {
    pub fn new(now: u64) -> Self {
        let mut users = BTreeMap::new();
        let guest = UserProfile::guest(now);
        users.insert(GUEST_USER_ID, guest);

        let mut auth_methods = BTreeMap::new();
        auth_methods.insert(GUEST_USER_ID, vec![AuthMethod::LocalOnly]);

        Self {
            users,
            auth_methods,
            sessions: BTreeMap::new(),
            ceremony: PasskeyCeremony::new(),
            switcher: UserSwitcher::new(GUEST_USER_ID, SwitchPolicy::KeepActive),
            guest_enabled: true,
        }
    }

    // --- User management ---

    pub fn create_user(
        &mut self,
        username: String,
        display_name: String,
        now: u64,
    ) -> &UserProfile {
        let id = allocate_user_id();
        let profile = UserProfile::new_local(id, username, display_name, now);
        self.users.insert(id, profile);
        self.auth_methods.insert(id, Vec::new());
        self.users.get(&id).unwrap()
    }

    pub fn get_user(&self, user_id: UserId) -> Option<&UserProfile> {
        self.users.get(&user_id)
    }

    pub fn get_user_mut(&mut self, user_id: UserId) -> Option<&mut UserProfile> {
        self.users.get_mut(&user_id)
    }

    pub fn find_user_by_username(&self, username: &str) -> Option<&UserProfile> {
        self.users.values().find(|u| u.username == username)
    }

    pub fn list_users(&self) -> Vec<&UserProfile> {
        self.users.values().collect()
    }

    pub fn list_non_guest_users(&self) -> Vec<&UserProfile> {
        self.users.values().filter(|u| !u.is_guest()).collect()
    }

    pub fn delete_user(&mut self, user_id: UserId) -> Result<(), AuthError> {
        if user_id == GUEST_USER_ID {
            return Err(AuthError::PermissionDenied);
        }
        if self.users.remove(&user_id).is_none() {
            return Err(AuthError::UserNotFound);
        }
        self.auth_methods.remove(&user_id);
        self.sessions.retain(|_, s| s.user_id != user_id);
        Ok(())
    }

    pub fn update_profile(
        &mut self,
        user_id: UserId,
        display_name: Option<String>,
        avatar_hash: Option<[u8; 32]>,
        email: Option<String>,
    ) -> Result<(), AuthError> {
        let user = self
            .users
            .get_mut(&user_id)
            .ok_or(AuthError::UserNotFound)?;
        if let Some(name) = display_name {
            user.display_name = name;
        }
        if let Some(hash) = avatar_hash {
            user.avatar_hash = Some(hash);
        }
        if let Some(mail) = email {
            user.email = Some(mail);
        }
        Ok(())
    }

    pub fn update_preferences(
        &mut self,
        user_id: UserId,
        prefs: UserPreferences,
    ) -> Result<(), AuthError> {
        let user = self
            .users
            .get_mut(&user_id)
            .ok_or(AuthError::UserNotFound)?;
        user.preferences = prefs;
        Ok(())
    }

    pub fn user_count(&self) -> usize {
        self.users.len()
    }

    // --- Auth method management ---

    pub fn add_passkey(
        &mut self,
        user_id: UserId,
        credential: PasskeyCredential,
    ) -> Result<(), AuthError> {
        let methods = self
            .auth_methods
            .get_mut(&user_id)
            .ok_or(AuthError::UserNotFound)?;
        methods.push(AuthMethod::Passkey(credential));
        Ok(())
    }

    pub fn add_password(
        &mut self,
        user_id: UserId,
        password: &[u8],
        salt: [u8; 16],
        now: u64,
    ) -> Result<(), AuthError> {
        let methods = self
            .auth_methods
            .get_mut(&user_id)
            .ok_or(AuthError::UserNotFound)?;

        let params = PasswordParams {
            salt,
            ..PasswordParams::default()
        };
        let hash = hash_password(password, &params);
        let cred = PasswordCredential {
            hash,
            params,
            created_at: now,
            last_changed: now,
            requires_change: false,
        };
        methods.push(AuthMethod::Password(cred));
        Ok(())
    }

    /// The stored password credential for `user_id`, if any. Used to serialize
    /// accounts for on-disk persistence.
    pub fn password_credential(&self, user_id: UserId) -> Option<&PasswordCredential> {
        self.auth_methods
            .get(&user_id)?
            .iter()
            .find_map(|m| match m {
                AuthMethod::Password(c) => Some(c),
                _ => None,
            })
    }

    /// Install a PRE-COMPUTED password credential (loaded from disk — the
    /// plaintext password is not available to re-hash). Replaces any existing
    /// password credential for the user.
    pub fn set_password_credential(
        &mut self,
        user_id: UserId,
        credential: PasswordCredential,
    ) -> Result<(), AuthError> {
        let methods = self
            .auth_methods
            .get_mut(&user_id)
            .ok_or(AuthError::UserNotFound)?;
        methods.retain(|m| !matches!(m, AuthMethod::Password(_)));
        methods.push(AuthMethod::Password(credential));
        Ok(())
    }

    pub fn add_pin(
        &mut self,
        user_id: UserId,
        pin: &[u8],
        salt: [u8; 16],
        now: u64,
    ) -> Result<(), AuthError> {
        let methods = self
            .auth_methods
            .get_mut(&user_id)
            .ok_or(AuthError::UserNotFound)?;

        // PINs are low-entropy, so lean on a few extra Argon2id passes. Memory
        // stays at the 8 MiB house default to fit the kernel heap.
        let params = PasswordParams {
            algorithm: PasswordAlgorithm::Argon2id,
            salt,
            iterations: 6,
            memory_cost_kb: 8_192,
            parallelism: 1,
        };
        let hash = hash_password(pin, &params);
        let cred = PasswordCredential {
            hash,
            params,
            created_at: now,
            last_changed: now,
            requires_change: false,
        };
        methods.push(AuthMethod::Pin(cred));
        Ok(())
    }

    pub fn list_auth_methods(&self, user_id: UserId) -> Option<Vec<AuthMethodKind>> {
        self.auth_methods
            .get(&user_id)
            .map(|methods| methods.iter().map(|m| m.kind()).collect())
    }

    pub fn remove_auth_method(
        &mut self,
        user_id: UserId,
        kind: AuthMethodKind,
    ) -> Result<bool, AuthError> {
        let methods = self
            .auth_methods
            .get_mut(&user_id)
            .ok_or(AuthError::UserNotFound)?;
        let before = methods.len();
        methods.retain(|m| m.kind() != kind);
        Ok(methods.len() != before)
    }

    // --- Passkey ceremony ---

    pub fn begin_passkey_registration(
        &mut self,
        user_id: UserId,
        challenge_bytes: [u8; 32],
        now: u64,
    ) -> Result<&PasskeyChallenge, AuthError> {
        if !self.users.contains_key(&user_id) {
            return Err(AuthError::UserNotFound);
        }
        let rp = RelyingParty::local();
        Ok(self
            .ceremony
            .create_registration_challenge(user_id, rp, challenge_bytes, now))
    }

    pub fn finish_passkey_registration(
        &mut self,
        user_id: UserId,
        registration: &PasskeyRegistration,
        now: u64,
    ) -> Result<(), AuthError> {
        let credential = self
            .ceremony
            .complete_registration(user_id, registration, now)?;
        self.add_passkey(user_id, credential)
    }

    pub fn begin_passkey_auth(
        &mut self,
        user_id: UserId,
        challenge_bytes: [u8; 32],
        now: u64,
    ) -> Result<&PasskeyChallenge, AuthError> {
        if !self.users.contains_key(&user_id) {
            return Err(AuthError::UserNotFound);
        }
        let rp = RelyingParty::local();
        Ok(self
            .ceremony
            .create_auth_challenge(user_id, rp, challenge_bytes, now))
    }

    pub fn finish_passkey_auth(
        &mut self,
        user_id: UserId,
        response: &PasskeyResponse,
        session_token: SessionToken,
        device: DeviceInfo,
        now: u64,
    ) -> AuthResult {
        let user = match self.users.get(&user_id) {
            Some(u) => u,
            None => return AuthResult::Failed(AuthError::UserNotFound),
        };

        if user.is_locked_out(now) {
            return AuthResult::LockedOut {
                until: user.lockout_until,
            };
        }

        let methods = match self.auth_methods.get_mut(&user_id) {
            Some(m) => m,
            None => return AuthResult::Failed(AuthError::NotConfigured),
        };

        // Find the matching passkey credential
        let passkey_cred = methods.iter_mut().find_map(|m| match m {
            AuthMethod::Passkey(ref mut pk) if pk.credential_id == response.credential_id => {
                Some(pk)
            }
            _ => None,
        });

        let cred = match passkey_cred {
            Some(c) => c,
            None => {
                if let Some(u) = self.users.get_mut(&user_id) {
                    u.record_failed_attempt(now);
                }
                return AuthResult::Failed(AuthError::CredentialNotFound);
            }
        };

        match self
            .ceremony
            .verify_auth_response(user_id, response, cred, now)
        {
            Ok(()) => {
                if let Some(u) = self.users.get_mut(&user_id) {
                    u.clear_failed_attempts();
                    u.last_login = now;
                }

                let is_new_device = !self
                    .sessions
                    .values()
                    .any(|s| s.user_id == user_id && s.device.device_id == device.device_id);

                let session = Session {
                    token: session_token,
                    user_id,
                    created_at: now,
                    expires_at: now + DEFAULT_SESSION_DURATION,
                    last_activity: now,
                    is_active: true,
                    device,
                    permissions: SessionPermissions::default_user(),
                    refresh_count: 0,
                    max_refreshes: DEFAULT_MAX_REFRESHES,
                    auth_method_used: AuthMethodKind::Passkey,
                };

                self.sessions.insert(session_token, session.clone());
                AuthResult::Success {
                    session,
                    is_new_device,
                }
            }
            Err(e) => {
                if let Some(u) = self.users.get_mut(&user_id) {
                    u.record_failed_attempt(now);
                }
                AuthResult::Failed(e)
            }
        }
    }

    // --- Password / PIN authentication ---

    pub fn authenticate_password(
        &mut self,
        user_id: UserId,
        password: &[u8],
        session_token: SessionToken,
        device: DeviceInfo,
        now: u64,
    ) -> AuthResult {
        self.authenticate_credential(
            user_id,
            password,
            AuthMethodKind::Password,
            session_token,
            device,
            now,
        )
    }

    pub fn authenticate_pin(
        &mut self,
        user_id: UserId,
        pin: &[u8],
        session_token: SessionToken,
        device: DeviceInfo,
        now: u64,
    ) -> AuthResult {
        self.authenticate_credential(
            user_id,
            pin,
            AuthMethodKind::Pin,
            session_token,
            device,
            now,
        )
    }

    fn authenticate_credential(
        &mut self,
        user_id: UserId,
        secret: &[u8],
        kind: AuthMethodKind,
        session_token: SessionToken,
        device: DeviceInfo,
        now: u64,
    ) -> AuthResult {
        let user = match self.users.get(&user_id) {
            Some(u) => u,
            None => return AuthResult::Failed(AuthError::UserNotFound),
        };

        if user.is_locked_out(now) {
            return AuthResult::LockedOut {
                until: user.lockout_until,
            };
        }

        let methods = match self.auth_methods.get(&user_id) {
            Some(m) => m,
            None => return AuthResult::Failed(AuthError::NotConfigured),
        };

        let verified = methods.iter().any(|m| match (kind, m) {
            (AuthMethodKind::Password, AuthMethod::Password(cred)) => verify_password(secret, cred),
            (AuthMethodKind::Pin, AuthMethod::Pin(cred)) => verify_password(secret, cred),
            _ => false,
        });

        if !verified {
            if let Some(u) = self.users.get_mut(&user_id) {
                u.record_failed_attempt(now);
            }
            return AuthResult::Failed(AuthError::InvalidCredentials);
        }

        if let Some(u) = self.users.get_mut(&user_id) {
            u.clear_failed_attempts();
            u.last_login = now;
        }

        let is_new_device = !self
            .sessions
            .values()
            .any(|s| s.user_id == user_id && s.device.device_id == device.device_id);

        let session = Session {
            token: session_token,
            user_id,
            created_at: now,
            expires_at: now + DEFAULT_SESSION_DURATION,
            last_activity: now,
            is_active: true,
            device,
            permissions: SessionPermissions::default_user(),
            refresh_count: 0,
            max_refreshes: DEFAULT_MAX_REFRESHES,
            auth_method_used: kind,
        };

        self.sessions.insert(session_token, session.clone());
        AuthResult::Success {
            session,
            is_new_device,
        }
    }

    // --- Guest authentication (no credentials needed) ---

    pub fn login_as_guest(
        &mut self,
        session_token: SessionToken,
        device: DeviceInfo,
        now: u64,
    ) -> AuthResult {
        if !self.guest_enabled {
            return AuthResult::Failed(AuthError::AccountDisabled);
        }

        if let Some(u) = self.users.get_mut(&GUEST_USER_ID) {
            u.last_login = now;
        }

        let session = Session {
            token: session_token,
            user_id: GUEST_USER_ID,
            created_at: now,
            expires_at: now + EXTENDED_SESSION_DURATION,
            last_activity: now,
            is_active: true,
            device,
            permissions: SessionPermissions::guest(),
            refresh_count: 0,
            max_refreshes: u32::MAX,
            auth_method_used: AuthMethodKind::LocalOnly,
        };

        self.sessions.insert(session_token, session.clone());
        AuthResult::Success {
            session,
            is_new_device: false,
        }
    }

    pub fn set_guest_enabled(&mut self, enabled: bool) {
        self.guest_enabled = enabled;
    }

    pub fn is_guest_enabled(&self) -> bool {
        self.guest_enabled
    }

    // --- Session management ---

    pub fn validate_session(&self, token: &SessionToken, now: u64) -> Option<&Session> {
        self.sessions.get(token).filter(|s| s.is_valid(now))
    }

    pub fn get_session_user(&self, token: &SessionToken, now: u64) -> Option<&UserProfile> {
        self.validate_session(token, now)
            .and_then(|s| self.users.get(&s.user_id))
    }

    pub fn refresh_session(
        &mut self,
        old_token: &SessionToken,
        new_token: SessionToken,
        now: u64,
    ) -> Result<&Session, AuthError> {
        let session = self
            .sessions
            .get(old_token)
            .ok_or(AuthError::SessionExpired)?;

        if !session.is_active {
            return Err(AuthError::SessionRevoked);
        }
        if !session.can_refresh() {
            return Err(AuthError::SessionExpired);
        }

        // Clone and update
        let mut refreshed = session.clone();
        refreshed.refresh(now, new_token, DEFAULT_SESSION_DURATION);

        self.sessions.remove(old_token);
        self.sessions.insert(new_token, refreshed);
        Ok(self.sessions.get(&new_token).unwrap())
    }

    pub fn revoke_session(&mut self, token: &SessionToken) -> bool {
        if let Some(session) = self.sessions.get_mut(token) {
            session.revoke();
            true
        } else {
            false
        }
    }

    pub fn revoke_all_sessions(&mut self, user_id: UserId) -> usize {
        let mut count = 0;
        for session in self.sessions.values_mut() {
            if session.user_id == user_id && session.is_active {
                session.revoke();
                count += 1;
            }
        }
        count
    }

    pub fn revoke_device_sessions(&mut self, user_id: UserId, device_id: &[u8; 16]) -> usize {
        let mut count = 0;
        for session in self.sessions.values_mut() {
            if session.user_id == user_id
                && session.is_active
                && &session.device.device_id == device_id
            {
                session.revoke();
                count += 1;
            }
        }
        count
    }

    pub fn list_active_sessions(&self, user_id: UserId, now: u64) -> Vec<&Session> {
        self.sessions
            .values()
            .filter(|s| s.user_id == user_id && s.is_valid(now))
            .collect()
    }

    pub fn list_all_sessions(&self, user_id: UserId) -> Vec<&Session> {
        self.sessions
            .values()
            .filter(|s| s.user_id == user_id)
            .collect()
    }

    pub fn list_devices(&self, user_id: UserId) -> Vec<&DeviceInfo> {
        let mut seen = Vec::new();
        let mut devices = Vec::new();
        for session in self.sessions.values() {
            if session.user_id == user_id && !seen.contains(&session.device.device_id) {
                seen.push(session.device.device_id);
                devices.push(&session.device);
            }
        }
        devices
    }

    pub fn cleanup_expired_sessions(&mut self, now: u64) -> usize {
        let before = self.sessions.len();
        self.sessions
            .retain(|_, s| !s.is_expired(now) || s.is_active);
        // Also revoke idle sessions
        for session in self.sessions.values_mut() {
            if session.is_active && session.idle_duration(now) > SESSION_IDLE_TIMEOUT {
                session.revoke();
            }
        }
        before - self.sessions.len()
    }

    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    pub fn active_session_count(&self, now: u64) -> usize {
        self.sessions.values().filter(|s| s.is_valid(now)).count()
    }

    // --- User switching ---

    pub fn switch_user(&mut self, user_id: UserId) -> Result<SwitchEvent, AuthError> {
        if !self.users.contains_key(&user_id) {
            return Err(AuthError::UserNotFound);
        }
        Ok(self.switcher.switch_to(user_id))
    }

    pub fn switch_back(&mut self) -> Option<SwitchEvent> {
        self.switcher.switch_back()
    }

    pub fn active_user(&self) -> UserId {
        self.switcher.active_user()
    }

    pub fn active_user_profile(&self) -> Option<&UserProfile> {
        self.users.get(&self.switcher.active_user())
    }

    pub fn set_switch_policy(&mut self, policy: SwitchPolicy) {
        self.switcher.set_policy(policy);
    }

    // --- Ceremony cleanup ---

    pub fn cleanup_expired_challenges(&mut self, now: u64) {
        self.ceremony.cleanup_expired(now);
    }

    // --- Lock/unlock user ---

    pub fn lock_user(&mut self, user_id: UserId) -> Result<(), AuthError> {
        let user = self
            .users
            .get_mut(&user_id)
            .ok_or(AuthError::UserNotFound)?;
        if user.is_guest() {
            return Err(AuthError::PermissionDenied);
        }
        user.locked = true;
        self.revoke_all_sessions(user_id);
        Ok(())
    }

    pub fn unlock_user(&mut self, user_id: UserId) -> Result<(), AuthError> {
        let user = self
            .users
            .get_mut(&user_id)
            .ok_or(AuthError::UserNotFound)?;
        user.locked = false;
        user.clear_failed_attempts();
        Ok(())
    }

    // --- Change password ---

    pub fn change_password(
        &mut self,
        user_id: UserId,
        old_password: &[u8],
        new_password: &[u8],
        new_salt: [u8; 16],
        now: u64,
    ) -> Result<(), AuthError> {
        // Brute-force protection: change_password re-verifies `old_password`, so
        // it MUST honor the same lockout the authenticate path enforces —
        // otherwise it is a password-guessing oracle that bypasses the lockout
        // entirely (refuse while locked; count a wrong old password as a failed
        // attempt; a correct one clears the counter).
        match self.users.get(&user_id) {
            Some(u) if u.is_locked_out(now) => return Err(AuthError::AccountLocked),
            Some(_) => {}
            None => return Err(AuthError::UserNotFound),
        }

        let methods = self
            .auth_methods
            .get(&user_id)
            .ok_or(AuthError::UserNotFound)?;

        let verified = methods.iter().any(|m| match m {
            AuthMethod::Password(cred) => verify_password(old_password, cred),
            _ => false,
        });

        if !verified {
            if let Some(u) = self.users.get_mut(&user_id) {
                u.record_failed_attempt(now);
            }
            return Err(AuthError::InvalidCredentials);
        }

        if let Some(u) = self.users.get_mut(&user_id) {
            u.clear_failed_attempts();
        }

        let methods = self.auth_methods.get_mut(&user_id).unwrap();
        methods.retain(|m| !matches!(m, AuthMethod::Password(_)));

        let params = PasswordParams {
            salt: new_salt,
            ..PasswordParams::default()
        };
        let hash = hash_password(new_password, &params);
        methods.push(AuthMethod::Password(PasswordCredential {
            hash,
            params,
            created_at: now,
            last_changed: now,
            requires_change: false,
        }));

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 12. Serialization helpers (for persistence)
// ---------------------------------------------------------------------------

/// Minimal serialized form of a user profile for storage/sync.
#[derive(Clone, Debug)]
pub struct SerializedProfile {
    pub data: Vec<u8>,
}

impl SerializedProfile {
    /// Serialize a profile into a compact binary format.
    pub fn from_profile(profile: &UserProfile) -> Self {
        let mut data = Vec::with_capacity(256);

        // User ID (8 bytes)
        data.extend_from_slice(&profile.id.0.to_le_bytes());
        // Timestamps (16 bytes)
        data.extend_from_slice(&profile.created_at.to_le_bytes());
        data.extend_from_slice(&profile.last_login.to_le_bytes());
        // Flags (1 byte)
        let flags = (profile.is_local as u8)
            | ((profile.locked as u8) << 1)
            | ((profile.avatar_hash.is_some() as u8) << 2)
            | ((profile.email.is_some() as u8) << 3);
        data.push(flags);
        // Avatar hash (32 bytes, optional)
        if let Some(hash) = &profile.avatar_hash {
            data.extend_from_slice(hash);
        }
        // Failed attempts + lockout (12 bytes)
        data.extend_from_slice(&profile.failed_attempts.to_le_bytes());
        data.extend_from_slice(&profile.lockout_until.to_le_bytes());
        // Username (length-prefixed)
        let username_bytes = profile.username.as_bytes();
        data.extend_from_slice(&(username_bytes.len() as u16).to_le_bytes());
        data.extend_from_slice(username_bytes);
        // Display name (length-prefixed)
        let display_bytes = profile.display_name.as_bytes();
        data.extend_from_slice(&(display_bytes.len() as u16).to_le_bytes());
        data.extend_from_slice(display_bytes);
        // Email (length-prefixed, optional)
        if let Some(email) = &profile.email {
            let email_bytes = email.as_bytes();
            data.extend_from_slice(&(email_bytes.len() as u16).to_le_bytes());
            data.extend_from_slice(email_bytes);
        }

        Self { data }
    }

    /// Deserialize a profile from binary data. Returns None on malformed input.
    pub fn to_profile(&self) -> Option<UserProfile> {
        let d = &self.data;
        if d.len() < 33 {
            return None;
        }

        let id = UserId(u64::from_le_bytes(d[0..8].try_into().ok()?));
        let created_at = u64::from_le_bytes(d[8..16].try_into().ok()?);
        let last_login = u64::from_le_bytes(d[16..24].try_into().ok()?);
        let flags = d[24];
        let is_local = flags & 1 != 0;
        let locked = flags & 2 != 0;
        let has_avatar = flags & 4 != 0;
        let has_email = flags & 8 != 0;

        let mut pos = 25;

        let avatar_hash = if has_avatar {
            if pos + 32 > d.len() {
                return None;
            }
            let mut hash = [0u8; 32];
            hash.copy_from_slice(&d[pos..pos + 32]);
            pos += 32;
            Some(hash)
        } else {
            None
        };

        if pos + 12 > d.len() {
            return None;
        }
        let failed_attempts = u32::from_le_bytes(d[pos..pos + 4].try_into().ok()?);
        pos += 4;
        let lockout_until = u64::from_le_bytes(d[pos..pos + 8].try_into().ok()?);
        pos += 8;

        // Read username
        if pos + 2 > d.len() {
            return None;
        }
        let username_len = u16::from_le_bytes(d[pos..pos + 2].try_into().ok()?) as usize;
        pos += 2;
        if pos + username_len > d.len() {
            return None;
        }
        let username = String::from_utf8(d[pos..pos + username_len].to_vec()).ok()?;
        pos += username_len;

        // Read display name
        if pos + 2 > d.len() {
            return None;
        }
        let display_len = u16::from_le_bytes(d[pos..pos + 2].try_into().ok()?) as usize;
        pos += 2;
        if pos + display_len > d.len() {
            return None;
        }
        let display_name = String::from_utf8(d[pos..pos + display_len].to_vec()).ok()?;
        pos += display_len;

        let email = if has_email && pos + 2 <= d.len() {
            let email_len = u16::from_le_bytes(d[pos..pos + 2].try_into().ok()?) as usize;
            pos += 2;
            if pos + email_len <= d.len() {
                Some(String::from_utf8(d[pos..pos + email_len].to_vec()).ok()?)
            } else {
                None
            }
        } else {
            None
        };

        Some(UserProfile {
            id,
            username,
            display_name,
            avatar_hash,
            preferences: UserPreferences::default(),
            created_at,
            last_login,
            is_local,
            email,
            locked,
            failed_attempts,
            lockout_until,
        })
    }
}

// ---------------------------------------------------------------------------
// 13. Audit log
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct AuditEntry {
    pub timestamp: u64,
    pub user_id: UserId,
    pub event: AuditEvent,
    pub device_id: Option<[u8; 16]>,
    pub success: bool,
}

#[derive(Clone, Debug)]
pub enum AuditEvent {
    Login(AuthMethodKind),
    Logout,
    FailedLogin(AuthError),
    SessionRefresh,
    SessionRevoke,
    UserCreated,
    UserDeleted,
    UserLocked,
    UserUnlocked,
    PasswordChanged,
    PasskeyAdded,
    PasskeyRemoved,
    UserSwitched { to: UserId },
    ProfileUpdated,
    PreferencesUpdated,
}

pub struct AuditLog {
    entries: Vec<AuditEntry>,
    max_entries: usize,
}

impl AuditLog {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries,
        }
    }

    pub fn record(&mut self, entry: AuditEntry) {
        if self.entries.len() >= self.max_entries {
            // Ring buffer behavior — drop oldest
            self.entries.remove(0);
        }
        self.entries.push(entry);
    }

    pub fn entries_for_user(&self, user_id: UserId) -> Vec<&AuditEntry> {
        self.entries
            .iter()
            .filter(|e| e.user_id == user_id)
            .collect()
    }

    pub fn recent(&self, count: usize) -> &[AuditEntry] {
        let start = self.entries.len().saturating_sub(count);
        &self.entries[start..]
    }

    pub fn failed_logins_since(&self, user_id: UserId, since: u64) -> usize {
        self.entries
            .iter()
            .filter(|e| {
                e.user_id == user_id
                    && e.timestamp >= since
                    && matches!(e.event, AuditEvent::FailedLogin(_))
            })
            .count()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// 14. Token generation helpers
// ---------------------------------------------------------------------------

/// Simple PRNG for generating session tokens and challenges in `no_std`.
/// Uses xorshift128+ — not cryptographically secure, but sufficient for
/// non-adversarial session token generation in a local OS context.
/// A real deployment would use a hardware RNG or CSPRNG.
pub struct TokenGenerator {
    state: [u64; 2],
}

impl TokenGenerator {
    pub fn new(seed0: u64, seed1: u64) -> Self {
        let s0 = if seed0 == 0 {
            0xDEAD_BEEF_CAFE_BABE
        } else {
            seed0
        };
        let s1 = if seed1 == 0 {
            0x1234_5678_9ABC_DEF0
        } else {
            seed1
        };
        Self { state: [s0, s1] }
    }

    fn next_u64(&mut self) -> u64 {
        let mut s0 = self.state[0];
        let s1 = self.state[1];
        let result = s0.wrapping_add(s1);
        self.state[0] = s1;
        s0 ^= s0 << 23;
        self.state[1] = s0 ^ s1 ^ (s0 >> 17) ^ (s1 >> 26);
        result
    }

    pub fn generate_token(&mut self) -> SessionToken {
        let mut bytes = [0u8; 32];
        for chunk in bytes.chunks_exact_mut(8) {
            let val = self.next_u64();
            chunk.copy_from_slice(&val.to_le_bytes());
        }
        SessionToken(bytes)
    }

    pub fn generate_challenge(&mut self) -> [u8; 32] {
        let mut bytes = [0u8; 32];
        for chunk in bytes.chunks_exact_mut(8) {
            let val = self.next_u64();
            chunk.copy_from_slice(&val.to_le_bytes());
        }
        bytes
    }

    pub fn generate_salt(&mut self) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        let a = self.next_u64();
        let b = self.next_u64();
        bytes[..8].copy_from_slice(&a.to_le_bytes());
        bytes[8..].copy_from_slice(&b.to_le_bytes());
        bytes
    }

    pub fn generate_device_id(&mut self) -> [u8; 16] {
        self.generate_salt()
    }
}

// ---------------------------------------------------------------------------
// 15. Init
// ---------------------------------------------------------------------------

/// Initialize the RaeID subsystem. Creates the guest user and returns a
/// manager ready for use. Account is never required — guest mode is the
/// default and provides full local functionality.
pub fn init(now: u64) -> AccountManager {
    AccountManager::new(now)
}

#[cfg(test)]
mod password_tests {
    use super::*;

    fn make_cred(password: &[u8]) -> PasswordCredential {
        let params = PasswordParams {
            salt: *b"raeid-test-salt!",
            ..PasswordParams::default()
        };
        PasswordCredential {
            hash: hash_password(password, &params),
            params,
            created_at: 0,
            last_changed: 0,
            requires_change: false,
        }
    }

    #[test]
    fn argon2id_is_the_default() {
        assert_eq!(
            PasswordParams::default().algorithm,
            PasswordAlgorithm::Argon2id
        );
    }

    #[test]
    fn correct_password_verifies_wrong_rejected() {
        let cred = make_cred(b"correct horse battery staple");
        assert!(verify_password(b"correct horse battery staple", &cred));
        assert!(!verify_password(b"correct horse battery stapl", &cred));
        assert!(!verify_password(b"", &cred));
    }

    #[test]
    fn argon2id_differs_from_legacy_homebrew() {
        // Same password+salt under the two schemes must produce different
        // hashes — proves new credentials really use Argon2id, not the old mix.
        let argon = PasswordParams {
            salt: *b"raeid-test-salt!",
            ..PasswordParams::default()
        };
        let legacy = PasswordParams {
            algorithm: PasswordAlgorithm::LegacyMixV1,
            ..argon.clone()
        };
        assert_ne!(hash_password(b"pw", &argon), hash_password(b"pw", &legacy));
    }

    #[test]
    fn legacy_credentials_still_verify() {
        // A credential created under the deprecated scheme must keep working
        // (no silent lockout) until migration.
        let params = PasswordParams {
            algorithm: PasswordAlgorithm::LegacyMixV1,
            salt: *b"raeid-test-salt!",
            ..PasswordParams::default()
        };
        let cred = PasswordCredential {
            hash: hash_password(b"legacy-pw", &params),
            params,
            created_at: 0,
            last_changed: 0,
            requires_change: false,
        };
        assert!(verify_password(b"legacy-pw", &cred));
        assert!(!verify_password(b"nope", &cred));
    }

    fn record(user: &str, pw: &[u8]) -> AccountRecord {
        let params = PasswordParams {
            salt: *b"raeid-test-salt!",
            ..PasswordParams::default()
        };
        AccountRecord {
            username: String::from(user),
            display_name: String::from("Test User"),
            credential: PasswordCredential {
                hash: hash_password(pw, &params),
                params,
                created_at: 7,
                last_changed: 9,
                requires_change: false,
            },
        }
    }

    #[test]
    fn accounts_serialize_roundtrip_and_auth() {
        let accts = alloc::vec![record("alice", b"hunter2"), record("bob", b"swordfish")];
        let bytes = serialize_accounts(&accts);
        let back = deserialize_accounts(&bytes);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].username, "alice");
        assert_eq!(back[1].username, "bob");
        assert_eq!(back[0].credential.hash, accts[0].credential.hash);
        assert_eq!(
            back[0].credential.params.salt,
            accts[0].credential.params.salt
        );
        assert_eq!(back[0].credential.created_at, 7);
        // The DESERIALIZED credential authenticates the original password.
        assert!(verify_password(b"hunter2", &back[0].credential));
        assert!(!verify_password(b"wrong", &back[0].credential));
        assert!(verify_password(b"swordfish", &back[1].credential));
    }

    #[test]
    fn accounts_deserialize_is_failsafe() {
        assert!(deserialize_accounts(b"").is_empty());
        assert!(deserialize_accounts(b"NOTMAGIC!....").is_empty());
        // A truncated record is dropped, not panicked on.
        let mut bytes = serialize_accounts(&[record("x", b"p")]);
        bytes.truncate(bytes.len() - 5);
        assert!(deserialize_accounts(&bytes).is_empty());
    }

    #[test]
    fn manager_credential_get_set_roundtrip() {
        // The kernel persistence path: read a user's credential out, then load
        // it back into a FRESH manager via set_password_credential (no re-hash).
        let mut mgr = AccountManager::new(0);
        let uid = mgr
            .create_user(String::from("carol"), String::from("Carol"), 0)
            .id;
        mgr.add_password(uid, b"correct-horse", *b"raeid-test-salt!", 0)
            .unwrap();
        let cred = mgr
            .password_credential(uid)
            .cloned()
            .expect("has credential");

        let mut fresh = AccountManager::new(0);
        let uid2 = fresh
            .create_user(String::from("carol"), String::from("Carol"), 0)
            .id;
        fresh.set_password_credential(uid2, cred).unwrap();
        let loaded = fresh.password_credential(uid2).expect("loaded credential");
        assert!(verify_password(b"correct-horse", loaded));
        assert!(!verify_password(b"nope", loaded));
    }

    /// Real ES256 (ECDSA P-256 + SHA-256) WebAuthn assertion verification.
    /// Fixture generated with openssl prime256v1: the signature is ECDSA over
    /// SHA-256(authenticator_data || client_data_hash). This is the gate the old
    /// stub skipped (it only checked the signature was non-empty).
    #[test]
    fn passkey_es256_assertion_signature_verified() {
        const PUBKEY: [u8; 64] = [
            122, 121, 59, 37, 41, 14, 50, 118, 51, 230, 37, 213, 244, 61, 19, 177, 206, 114, 77,
            139, 26, 80, 205, 157, 237, 20, 166, 63, 59, 170, 78, 49, 208, 59, 161, 75, 223, 198,
            175, 189, 31, 128, 40, 53, 85, 57, 56, 50, 35, 72, 176, 191, 53, 65, 218, 170, 123,
            252, 149, 8, 216, 122, 172, 107,
        ];
        const AUTH_DATA: [u8; 37] = [
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
            24, 25, 26, 27, 28, 29, 30, 31, 5, 0, 0, 0, 1,
        ];
        const CLIENT_DATA_HASH: [u8; 32] = [
            32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53,
            54, 55, 56, 57, 58, 59, 60, 61, 62, 63,
        ];
        const SIG: &[u8] = &[
            48, 69, 2, 33, 0, 129, 6, 159, 102, 120, 147, 136, 142, 23, 203, 111, 44, 55, 231, 89,
            133, 223, 118, 2, 129, 16, 116, 135, 126, 120, 108, 115, 199, 130, 220, 119, 94, 2, 32,
            14, 179, 31, 140, 39, 189, 115, 3, 21, 71, 41, 162, 196, 89, 94, 109, 106, 55, 229,
            199, 140, 117, 129, 128, 116, 20, 206, 221, 86, 60, 47, 102,
        ];

        let cred_id = [7u8; 32];
        let base_cred = || PasskeyCredential {
            credential_id: cred_id,
            public_key: PUBKEY,
            rp: RelyingParty::local(),
            sign_count: 0,
            created_at: 0,
            last_used: 0,
            transports: Vec::new(),
            user_verified: true,
            aaguid: [0u8; 16],
        };
        let make_resp = |ad: Vec<u8>, sig: Vec<u8>| PasskeyResponse {
            credential_id: cred_id,
            authenticator_data: ad,
            signature: sig,
            client_data_hash: CLIENT_DATA_HASH,
            user_handle: None,
        };
        let uid = UserId(42);
        let issue = || {
            let mut c = PasskeyCeremony::new();
            c.create_auth_challenge(uid, RelyingParty::local(), [0u8; 32], 100);
            c
        };

        // Valid signature -> Ok, and sign_count advances to the response's count.
        let mut cred = base_cred();
        let resp = make_resp(AUTH_DATA.to_vec(), SIG.to_vec());
        assert!(issue()
            .verify_auth_response(uid, &resp, &mut cred, 101)
            .is_ok());
        assert_eq!(cred.sign_count, 1);

        // Tampered signature byte -> InvalidSignature (fail-closed).
        let mut cred = base_cred();
        let mut bad_sig = SIG.to_vec();
        bad_sig[40] ^= 0xFF;
        let resp = make_resp(AUTH_DATA.to_vec(), bad_sig);
        assert_eq!(
            issue().verify_auth_response(uid, &resp, &mut cred, 101),
            Err(AuthError::InvalidSignature)
        );

        // Tampered authenticator_data (signed message changed) -> InvalidSignature.
        let mut cred = base_cred();
        let mut bad_ad = AUTH_DATA.to_vec();
        bad_ad[0] ^= 0x01;
        let resp = make_resp(bad_ad, SIG.to_vec());
        assert_eq!(
            issue().verify_auth_response(uid, &resp, &mut cred, 101),
            Err(AuthError::InvalidSignature)
        );

        // Wrong public key -> InvalidSignature.
        let mut cred = base_cred();
        cred.public_key[0] ^= 0x01;
        let resp = make_resp(AUTH_DATA.to_vec(), SIG.to_vec());
        assert_eq!(
            issue().verify_auth_response(uid, &resp, &mut cred, 101),
            Err(AuthError::InvalidSignature)
        );

        // Empty signature -> InvalidSignature (the old stub passed any non-empty;
        // this now also fails because verification, not length, is the gate).
        let mut cred = base_cred();
        let resp = make_resp(AUTH_DATA.to_vec(), Vec::new());
        assert_eq!(
            issue().verify_auth_response(uid, &resp, &mut cred, 101),
            Err(AuthError::InvalidSignature)
        );
    }

    /// FAIL-able proof of brute-force login protection (Win11/macOS parity):
    /// after MAX_FAILED_ATTEMPTS wrong passwords the account locks and rejects
    /// even the CORRECT password until the lockout window elapses; a success
    /// below the threshold clears the counter. If the lockout were removed,
    /// step 4 would return Success instead of LockedOut and this FAILs.
    #[test]
    fn brute_force_lockout_blocks_then_recovers() {
        fn dev() -> DeviceInfo {
            DeviceInfo {
                device_id: [7u8; 16],
                device_name: String::from("test"),
                os_version: String::from("raeenos"),
                last_ip_hash: None,
            }
        }
        let tok = SessionToken([0u8; 32]);
        let mut mgr = AccountManager::new(0);
        let uid = mgr
            .create_user(String::from("dave"), String::from("Dave"), 0)
            .id;
        mgr.add_password(uid, b"correct-horse", *b"raeid-test-salt!", 0)
            .unwrap();
        let now = 1000u64;

        // 1. Below the threshold, wrong passwords return Failed (not LockedOut).
        for _ in 0..(MAX_FAILED_ATTEMPTS - 1) {
            assert!(matches!(
                mgr.authenticate_password(uid, b"wrong", tok, dev(), now),
                AuthResult::Failed(_)
            ));
        }
        // 2. A correct login below the threshold succeeds AND clears the counter.
        assert!(matches!(
            mgr.authenticate_password(uid, b"correct-horse", tok, dev(), now),
            AuthResult::Success { .. }
        ));

        // 3. Exhaust the threshold with wrong passwords.
        for _ in 0..MAX_FAILED_ATTEMPTS {
            let _ = mgr.authenticate_password(uid, b"wrong", tok, dev(), now);
        }
        // 4. The CORRECT password is now BLOCKED — the anti-brute-force property.
        assert!(matches!(
            mgr.authenticate_password(uid, b"correct-horse", tok, dev(), now),
            AuthResult::LockedOut { .. }
        ));

        // 5. After the lockout window elapses, the correct password works again.
        let later = now + LOCKOUT_DURATION_SECS + 1;
        assert!(matches!(
            mgr.authenticate_password(uid, b"correct-horse", tok, dev(), later),
            AuthResult::Success { .. }
        ));
    }

    /// change_password re-verifies the old password, so it must honor the same
    /// brute-force lockout as the login path — otherwise it is a guessing oracle
    /// that bypasses the lockout. FAIL-able: without the gate, step 3 would
    /// change the password (Ok) instead of returning AccountLocked.
    #[test]
    fn change_password_honors_brute_force_lockout() {
        let mut mgr = AccountManager::new(0);
        let uid = mgr
            .create_user(String::from("erin"), String::from("Erin"), 0)
            .id;
        mgr.add_password(uid, b"orig-pass", *b"raeid-test-salt!", 0)
            .unwrap();
        let now = 2000u64;

        // 1. Wrong old password is rejected AND counts toward the lockout.
        for _ in 0..MAX_FAILED_ATTEMPTS {
            assert_eq!(
                mgr.change_password(uid, b"wrong", b"new-pass", *b"raeid-test-salt2", now),
                Err(AuthError::InvalidCredentials)
            );
        }
        // 2. The account is now locked; even the CORRECT old password is refused.
        assert_eq!(
            mgr.change_password(uid, b"orig-pass", b"new-pass", *b"raeid-test-salt2", now),
            Err(AuthError::AccountLocked)
        );
        // 3. The password was NOT changed — the original still authenticates once
        //    the lockout elapses (proving no partial change slipped through).
        let later = now + LOCKOUT_DURATION_SECS + 1;
        assert!(mgr
            .change_password(uid, b"orig-pass", b"new-pass", *b"raeid-test-salt2", later)
            .is_ok());
    }

    /// Categorical (non-arbitrary) invariants of the password-strength meter:
    /// the buckets are heuristic, but these RELATIONSHIPS must always hold.
    #[test]
    fn password_strength_categorical_invariants() {
        use PasswordStrength::*;
        // Trivial/known-bad passwords are always the floor.
        assert_eq!(estimate_password_strength(b""), VeryWeak);
        assert_eq!(estimate_password_strength(b"password"), VeryWeak);
        assert_eq!(estimate_password_strength(b"PASSWORD"), VeryWeak); // case-insensitive
        assert_eq!(estimate_password_strength(b"12345678"), VeryWeak); // common
        assert_eq!(estimate_password_strength(b"aaaaaaaaaaaa"), VeryWeak); // all-same
                                                                           // A short simple password can never rate above Weak.
        assert!(estimate_password_strength(b"ab1") <= Weak);
        // A long, 4-class password is strong.
        assert!(estimate_password_strength(b"Tr0ub4dor&3xK") >= Strong);
        // Monotonicity: adding classes/length never LOWERS the rating.
        assert!(
            estimate_password_strength(b"abcdefghijklmnop9Q!")
                >= estimate_password_strength(b"abcdefghijklmnop")
        );
        // A long diverse passphrase clears the Fair floor (usable accounts).
        assert!(estimate_password_strength(b"correct-horse-Battery-9") >= Fair);
    }
}
