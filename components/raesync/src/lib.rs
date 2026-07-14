//! RaeSync — optional cross-device sync.
//!
//! End-to-end encrypted synchronization across devices with
//! conflict resolution and peer trust management.
//! Optional and additive — never required for local use.
#![no_std]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// 1. Device identity
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(pub [u8; 16]);

impl DeviceId {
    pub const ZERO: Self = Self([0u8; 16]);

    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

// ---------------------------------------------------------------------------
// 2. Trust model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TrustLevel {
    /// Full access — can manage other devices, change sync settings.
    Owner = 3,
    /// Can sync all data types, cannot manage devices.
    Trusted = 2,
    /// Can sync a restricted subset of data types.
    Limited = 1,
    /// Access revoked — no sync, pending removal.
    Revoked = 0,
}

impl TrustLevel {
    pub fn can_sync(&self) -> bool {
        matches!(self, Self::Owner | Self::Trusted | Self::Limited)
    }

    pub fn can_manage_devices(&self) -> bool {
        matches!(self, Self::Owner)
    }

    pub fn can_sync_type(&self, item_type: SyncItemType) -> bool {
        match self {
            Self::Owner | Self::Trusted => true,
            Self::Limited => matches!(item_type, SyncItemType::Settings | SyncItemType::Bookmarks),
            Self::Revoked => false,
        }
    }
}

/// Information about a paired device.
#[derive(Clone, Debug)]
pub struct DeviceRecord {
    pub device_id: DeviceId,
    pub device_name: String,
    pub trust_level: TrustLevel,
    pub public_key: [u8; 32],
    pub paired_at: u64,
    pub last_seen: u64,
    pub last_sync: u64,
    pub sync_scope: SyncScope,
    pub pairing_method: PairingMethod,
    pub os_version: String,
}

impl DeviceRecord {
    pub fn is_active(&self, now: u64, stale_threshold: u64) -> bool {
        self.trust_level.can_sync() && (now - self.last_seen) < stale_threshold
    }
}

/// How the device was paired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairingMethod {
    QrCode,
    NumericPin,
    ProximityBle,
    ManualKeyExchange,
}

/// Which data types this device is allowed to sync.
#[derive(Clone, Debug)]
pub struct SyncScope {
    pub allowed_types: Vec<SyncItemType>,
    pub max_payload_bytes: u64,
    pub sync_frequency_secs: u32,
}

impl SyncScope {
    pub fn full() -> Self {
        Self {
            allowed_types: vec![
                SyncItemType::Settings,
                SyncItemType::Bookmarks,
                SyncItemType::Clipboard,
                SyncItemType::WifiPasswords,
                SyncItemType::AppData,
                SyncItemType::Files,
                SyncItemType::GameSaves,
                SyncItemType::Credentials,
            ],
            max_payload_bytes: 256 * 1024 * 1024, // 256 MiB
            sync_frequency_secs: 300,
        }
    }

    pub fn limited() -> Self {
        Self {
            allowed_types: vec![SyncItemType::Settings, SyncItemType::Bookmarks],
            max_payload_bytes: 1024 * 1024, // 1 MiB
            sync_frequency_secs: 3600,
        }
    }

    pub fn allows(&self, item_type: SyncItemType) -> bool {
        self.allowed_types.contains(&item_type)
    }
}

// ---------------------------------------------------------------------------
// 3. Device pairing ceremony
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct PairingChallenge {
    pub initiator: DeviceId,
    pub numeric_pin: u32,
    pub public_key: [u8; 32],
    pub created_at: u64,
    pub expires_at: u64,
    pub qr_payload: Vec<u8>,
}

impl PairingChallenge {
    pub fn is_expired(&self, now: u64) -> bool {
        now >= self.expires_at
    }
}

#[derive(Clone, Debug)]
pub struct PairingResponse {
    pub responder: DeviceId,
    pub responder_name: String,
    pub numeric_pin: u32,
    pub public_key: [u8; 32],
    pub os_version: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairingError {
    PinMismatch,
    Expired,
    AlreadyPaired,
    KeyExchangeFailed,
    TrustDenied,
    InvalidResponse,
}

pub struct PairingCeremony {
    pending: BTreeMap<DeviceId, PairingChallenge>,
}

impl PairingCeremony {
    pub fn new() -> Self {
        Self {
            pending: BTreeMap::new(),
        }
    }

    /// Initiate pairing. Returns a challenge containing QR data and numeric PIN.
    pub fn initiate(
        &mut self,
        local_device: DeviceId,
        local_pubkey: [u8; 32],
        pin: u32,
        now: u64,
    ) -> &PairingChallenge {
        let mut qr_payload = Vec::with_capacity(52);
        qr_payload.extend_from_slice(&local_device.0);
        qr_payload.extend_from_slice(&local_pubkey);
        qr_payload.extend_from_slice(&pin.to_le_bytes());

        let challenge = PairingChallenge {
            initiator: local_device,
            numeric_pin: pin,
            public_key: local_pubkey,
            created_at: now,
            expires_at: now + 300, // 5 minute window
            qr_payload,
        };

        self.pending.insert(local_device, challenge);
        self.pending.get(&local_device).unwrap()
    }

    /// Complete pairing after the remote device responds.
    pub fn complete(
        &mut self,
        local_device: DeviceId,
        response: &PairingResponse,
        now: u64,
    ) -> Result<DeviceRecord, PairingError> {
        let challenge = self
            .pending
            .remove(&local_device)
            .ok_or(PairingError::Expired)?;

        if challenge.is_expired(now) {
            return Err(PairingError::Expired);
        }

        if challenge.numeric_pin != response.numeric_pin {
            return Err(PairingError::PinMismatch);
        }

        Ok(DeviceRecord {
            device_id: response.responder,
            device_name: response.responder_name.clone(),
            trust_level: TrustLevel::Trusted,
            public_key: response.public_key,
            paired_at: now,
            last_seen: now,
            last_sync: 0,
            sync_scope: SyncScope::full(),
            pairing_method: PairingMethod::NumericPin,
            os_version: response.os_version.clone(),
        })
    }

    pub fn cancel(&mut self, device_id: &DeviceId) {
        self.pending.remove(device_id);
    }

    pub fn cleanup_expired(&mut self, now: u64) {
        self.pending.retain(|_, c| !c.is_expired(now));
    }
}

// ---------------------------------------------------------------------------
// 4. Sync item types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SyncItemType {
    Settings,
    Bookmarks,
    Clipboard,
    WifiPasswords,
    AppData,
    Files,
    GameSaves,
    Credentials,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SyncItemId(pub [u8; 16]);

#[derive(Clone, Debug)]
pub struct SyncItem {
    pub id: SyncItemId,
    pub item_type: SyncItemType,
    pub version: u64,
    pub data_hash: [u8; 32],
    pub created_at: u64,
    pub modified_at: u64,
    pub origin_device: DeviceId,
    pub size_bytes: u64,
    pub encrypted_payload: Vec<u8>,
    pub metadata: SyncItemMetadata,
}

#[derive(Clone, Debug)]
pub struct SyncItemMetadata {
    pub name: String,
    pub mime_type: Option<String>,
    pub tags: Vec<String>,
    pub parent_id: Option<SyncItemId>,
    pub is_deleted: bool,
    pub delete_after: Option<u64>,
}

impl SyncItemMetadata {
    pub fn new(name: String) -> Self {
        Self {
            name,
            mime_type: None,
            tags: Vec::new(),
            parent_id: None,
            is_deleted: false,
            delete_after: None,
        }
    }
}

// ---------------------------------------------------------------------------
// 5. Sync state machine
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncPhase {
    /// No sync in progress.
    Idle,
    /// Discovering peers on the network.
    Discovering,
    /// Exchanging manifests and computing deltas.
    Negotiating,
    /// Transferring data.
    Syncing,
    /// Handling conflicts.
    Resolving,
    /// Sync completed successfully.
    Complete,
    /// Sync failed.
    Failed,
}

#[derive(Clone, Debug)]
pub struct SyncProgress {
    pub phase: SyncPhase,
    pub total_items: usize,
    pub completed_items: usize,
    pub failed_items: usize,
    pub bytes_transferred: u64,
    pub bytes_total: u64,
    pub started_at: u64,
    pub peer_device: Option<DeviceId>,
    pub errors: Vec<SyncError>,
}

impl SyncProgress {
    pub fn new(now: u64) -> Self {
        Self {
            phase: SyncPhase::Idle,
            total_items: 0,
            completed_items: 0,
            failed_items: 0,
            bytes_transferred: 0,
            bytes_total: 0,
            started_at: now,
            peer_device: None,
            errors: Vec::new(),
        }
    }

    pub fn fraction_complete(&self) -> f32 {
        if self.total_items == 0 {
            return 0.0;
        }
        self.completed_items as f32 / self.total_items as f32
    }

    pub fn is_done(&self) -> bool {
        matches!(self.phase, SyncPhase::Complete | SyncPhase::Failed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncError {
    PeerUnreachable(DeviceId),
    TrustRevoked(DeviceId),
    EncryptionFailed,
    DecryptionFailed,
    IntegrityCheckFailed,
    PayloadTooLarge,
    VersionMismatch,
    ConflictUnresolved(SyncItemId),
    StorageFull,
    Timeout,
    ProtocolError,
}

// ---------------------------------------------------------------------------
// 6. Per-item sync status
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemSyncStatus {
    InSync,
    LocalAhead,
    RemoteAhead,
    Conflict,
    PendingUpload,
    PendingDownload,
    Uploading,
    Downloading,
    Error,
}

#[derive(Clone, Debug)]
pub struct ItemSyncState {
    pub item_id: SyncItemId,
    pub status: ItemSyncStatus,
    pub local_version: u64,
    pub remote_version: u64,
    pub last_synced_version: u64,
    pub last_sync_time: u64,
    pub retry_count: u32,
}

// ---------------------------------------------------------------------------
// 7. Conflict resolution
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictStrategy {
    KeepLocal,
    KeepRemote,
    KeepNewer,
    KeepBoth,
    Manual,
}

/// Per-type conflict resolution rules.
#[derive(Clone, Debug)]
pub struct ConflictPolicy {
    pub default_strategy: ConflictStrategy,
    pub type_overrides: BTreeMap<u8, ConflictStrategy>,
}

impl ConflictPolicy {
    pub fn new(default: ConflictStrategy) -> Self {
        Self {
            default_strategy: default,
            type_overrides: BTreeMap::new(),
        }
    }

    pub fn set_override(&mut self, item_type: SyncItemType, strategy: ConflictStrategy) {
        self.type_overrides.insert(item_type as u8, strategy);
    }

    pub fn strategy_for(&self, item_type: SyncItemType) -> ConflictStrategy {
        self.type_overrides
            .get(&(item_type as u8))
            .copied()
            .unwrap_or(self.default_strategy)
    }
}

/// A detected conflict between local and remote versions.
#[derive(Clone, Debug)]
pub struct Conflict {
    pub item_id: SyncItemId,
    pub item_type: SyncItemType,
    pub local_version: u64,
    pub remote_version: u64,
    pub local_modified: u64,
    pub remote_modified: u64,
    pub local_hash: [u8; 32],
    pub remote_hash: [u8; 32],
    pub resolution: Option<ConflictStrategy>,
}

impl Conflict {
    pub fn auto_resolve(&mut self, policy: &ConflictPolicy) -> ConflictStrategy {
        let strategy = policy.strategy_for(self.item_type);
        let resolved = match strategy {
            ConflictStrategy::KeepNewer => {
                if self.local_modified >= self.remote_modified {
                    ConflictStrategy::KeepLocal
                } else {
                    ConflictStrategy::KeepRemote
                }
            }
            other => other,
        };
        self.resolution = Some(resolved);
        resolved
    }

    pub fn is_resolved(&self) -> bool {
        self.resolution.is_some()
    }
}

/// Three-way merge result for text-like content.
#[derive(Clone, Debug)]
pub enum MergeResult {
    Clean(Vec<u8>),
    ConflictMarkers(Vec<u8>),
    CannotMerge,
}

/// Simple three-way merge for text files.
/// Compares local and remote against a common ancestor, line by line.
pub fn three_way_merge(ancestor: &[u8], local: &[u8], remote: &[u8]) -> MergeResult {
    let ancestor_lines = split_lines(ancestor);
    let local_lines = split_lines(local);
    let remote_lines = split_lines(remote);

    let mut result = Vec::new();
    let mut has_conflicts = false;
    let max_len = ancestor_lines
        .len()
        .max(local_lines.len())
        .max(remote_lines.len());

    for i in 0..max_len {
        let a = ancestor_lines.get(i).copied().unwrap_or(b"");
        let l = local_lines.get(i).copied().unwrap_or(b"");
        let r = remote_lines.get(i).copied().unwrap_or(b"");

        if l == r {
            // Both agree — use either.
            result.extend_from_slice(l);
            result.push(b'\n');
        } else if l == a {
            // Only remote changed.
            result.extend_from_slice(r);
            result.push(b'\n');
        } else if r == a {
            // Only local changed.
            result.extend_from_slice(l);
            result.push(b'\n');
        } else {
            // Both changed differently — conflict.
            has_conflicts = true;
            result.extend_from_slice(b"<<<<<<< LOCAL\n");
            result.extend_from_slice(l);
            result.push(b'\n');
            result.extend_from_slice(b"=======\n");
            result.extend_from_slice(r);
            result.push(b'\n');
            result.extend_from_slice(b">>>>>>> REMOTE\n");
        }
    }

    if has_conflicts {
        MergeResult::ConflictMarkers(result)
    } else {
        MergeResult::Clean(result)
    }
}

fn split_lines(data: &[u8]) -> Vec<&[u8]> {
    if data.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut start = 0;
    for i in 0..data.len() {
        if data[i] == b'\n' {
            lines.push(&data[start..i]);
            start = i + 1;
        }
    }
    if start < data.len() {
        lines.push(&data[start..]);
    }
    lines
}

// ---------------------------------------------------------------------------
// 8. Encrypted transport (X25519 + ChaCha20-Poly1305 + HKDF)
// ---------------------------------------------------------------------------

/// X25519 (RFC 7748) key pair for Diffie-Hellman key exchange, via the shared
/// `rae_crypto` curve25519 implementation. The `private_key` is the raw seed
/// (X25519 clamps it internally per RFC 7748).
#[derive(Clone, Debug)]
pub struct X25519KeyPair {
    pub private_key: [u8; 32],
    pub public_key: [u8; 32],
}

impl X25519KeyPair {
    /// Derive a key pair from a 32-byte secret seed: public = X25519(seed, 9).
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            private_key: seed,
            public_key: rae_crypto::x25519::public_key(&seed),
        }
    }

    /// X25519 shared secret from our private key and their public key. Both
    /// peers (with roles swapped) derive the same 32 bytes.
    pub fn diffie_hellman(&self, their_public: &[u8; 32]) -> [u8; 32] {
        rae_crypto::x25519::diffie_hellman(&self.private_key, their_public)
    }
}

/// HKDF-style key derivation (simplified for `no_std`).
/// Derives multiple sub-keys from a shared secret + context info.
pub struct Hkdf;

impl Hkdf {
    /// HKDF-Extract (RFC 5869 §2.2): PRK = HMAC-SHA256(salt, ikm). Real key
    /// derivation via the shared `rae_crypto` crate (was a homebrew
    /// `wrapping_mul` mixer with no security properties).
    pub fn extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
        rae_crypto::sha256::hkdf_extract(salt, ikm)
    }

    /// HKDF-Expand (RFC 5869 §2.3) into `length` bytes of output keying material.
    pub fn expand(prk: &[u8; 32], info: &[u8], length: usize) -> Vec<u8> {
        let mut okm = Vec::new();
        okm.resize(length, 0u8);
        rae_crypto::sha256::hkdf_expand(prk, info, &mut okm);
        okm
    }

    /// Derive an encryption key and nonce from a shared secret.
    pub fn derive_session_keys(
        shared_secret: &[u8; 32],
        context: &[u8],
    ) -> (EncryptionKey, [u8; 12]) {
        let prk = Self::extract(context, shared_secret);
        let expanded = Self::expand(&prk, b"raesync-session", 44);
        let mut key = [0u8; 32];
        let mut nonce = [0u8; 12];
        key.copy_from_slice(&expanded[..32]);
        nonce.copy_from_slice(&expanded[32..44]);
        (EncryptionKey(key), nonce)
    }
}

/// 256-bit encryption key for ChaCha20-Poly1305.
#[derive(Clone)]
pub struct EncryptionKey(pub [u8; 32]);

impl core::fmt::Debug for EncryptionKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EncryptionKey")
            .field("key", &"[REDACTED]")
            .finish()
    }
}

/// ChaCha20-Poly1305 AEAD (RFC 8439) for the E2E sync channel — now the REAL
/// authenticated cipher via the shared `rae_crypto` crate (was a placeholder
/// XOR keystream + homebrew tag). Both confidentiality and integrity are real.
pub struct ChaCha20Poly1305;

impl ChaCha20Poly1305 {
    /// Encrypt + authenticate with associated data. Returns ciphertext || tag.
    pub fn encrypt(key: &EncryptionKey, nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        rae_crypto::chacha20poly1305::seal(&key.0, nonce, aad, plaintext)
    }

    /// Verify + decrypt. Returns the plaintext, or `None` on authentication
    /// failure (wrong key/nonce/aad, tampered ciphertext or tag).
    pub fn decrypt(
        key: &EncryptionKey,
        nonce: &[u8; 12],
        aad: &[u8],
        ciphertext_with_tag: &[u8],
    ) -> Option<Vec<u8>> {
        rae_crypto::chacha20poly1305::open(&key.0, nonce, aad, ciphertext_with_tag)
    }
}

// ---------------------------------------------------------------------------
// 9. Sealed sync payload
// ---------------------------------------------------------------------------

/// An encrypted sync payload ready for transmission.
#[derive(Clone, Debug)]
pub struct SealedPayload {
    pub sender: DeviceId,
    pub recipient: DeviceId,
    pub sequence: u64,
    pub encrypted_data: Vec<u8>,
    pub nonce: [u8; 12],
    pub timestamp: u64,
}

impl SealedPayload {
    pub fn seal(
        sender: DeviceId,
        recipient: DeviceId,
        sequence: u64,
        key: &EncryptionKey,
        nonce: [u8; 12],
        plaintext: &[u8],
        now: u64,
    ) -> Self {
        let mut aad = Vec::with_capacity(40);
        aad.extend_from_slice(&sender.0);
        aad.extend_from_slice(&recipient.0);
        aad.extend_from_slice(&sequence.to_le_bytes());

        let encrypted_data = ChaCha20Poly1305::encrypt(key, &nonce, &aad, plaintext);

        Self {
            sender,
            recipient,
            sequence,
            encrypted_data,
            nonce,
            timestamp: now,
        }
    }

    pub fn unseal(&self, key: &EncryptionKey) -> Option<Vec<u8>> {
        let mut aad = Vec::with_capacity(40);
        aad.extend_from_slice(&self.sender.0);
        aad.extend_from_slice(&self.recipient.0);
        aad.extend_from_slice(&self.sequence.to_le_bytes());

        ChaCha20Poly1305::decrypt(key, &self.nonce, &aad, &self.encrypted_data)
    }
}

// ---------------------------------------------------------------------------
// 10. Sync manifest
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct SyncManifest {
    pub device_id: DeviceId,
    pub version: u64,
    pub last_sync: u64,
    pub items: BTreeMap<SyncItemId, SyncManifestEntry>,
}

#[derive(Clone, Debug)]
pub struct SyncManifestEntry {
    pub item_type: SyncItemType,
    pub version: u64,
    pub data_hash: [u8; 32],
    pub modified_at: u64,
    pub size_bytes: u64,
    pub is_deleted: bool,
}

impl SyncManifest {
    pub fn new(device_id: DeviceId) -> Self {
        Self {
            device_id,
            version: 0,
            last_sync: 0,
            items: BTreeMap::new(),
        }
    }

    pub fn add_entry(&mut self, id: SyncItemId, entry: SyncManifestEntry) {
        self.items.insert(id, entry);
        self.version += 1;
    }

    pub fn remove_entry(&mut self, id: &SyncItemId) -> bool {
        if self.items.remove(id).is_some() {
            self.version += 1;
            true
        } else {
            false
        }
    }

    pub fn mark_deleted(&mut self, id: &SyncItemId) -> bool {
        if let Some(entry) = self.items.get_mut(id) {
            entry.is_deleted = true;
            self.version += 1;
            true
        } else {
            false
        }
    }

    /// Compute the delta between this manifest and a remote one.
    pub fn compute_delta(&self, remote: &SyncManifest) -> SyncDelta {
        let mut to_upload = Vec::new();
        let mut to_download = Vec::new();
        let mut conflicts = Vec::new();

        for (id, local_entry) in &self.items {
            match remote.items.get(id) {
                None => {
                    if !local_entry.is_deleted {
                        to_upload.push(*id);
                    }
                }
                Some(remote_entry) => {
                    if local_entry.data_hash == remote_entry.data_hash {
                        continue;
                    }
                    if local_entry.version > remote_entry.version {
                        to_upload.push(*id);
                    } else if remote_entry.version > local_entry.version {
                        to_download.push(*id);
                    } else {
                        conflicts.push(Conflict {
                            item_id: *id,
                            item_type: local_entry.item_type,
                            local_version: local_entry.version,
                            remote_version: remote_entry.version,
                            local_modified: local_entry.modified_at,
                            remote_modified: remote_entry.modified_at,
                            local_hash: local_entry.data_hash,
                            remote_hash: remote_entry.data_hash,
                            resolution: None,
                        });
                    }
                }
            }
        }

        for (id, remote_entry) in &remote.items {
            if !self.items.contains_key(id) && !remote_entry.is_deleted {
                to_download.push(*id);
            }
        }

        SyncDelta {
            to_upload,
            to_download,
            conflicts,
        }
    }

    pub fn total_size(&self) -> u64 {
        self.items.values().map(|e| e.size_bytes).sum()
    }

    pub fn item_count(&self) -> usize {
        self.items.len()
    }
}

/// The computed difference between local and remote manifests.
#[derive(Clone, Debug)]
pub struct SyncDelta {
    pub to_upload: Vec<SyncItemId>,
    pub to_download: Vec<SyncItemId>,
    pub conflicts: Vec<Conflict>,
}

impl SyncDelta {
    pub fn is_empty(&self) -> bool {
        self.to_upload.is_empty() && self.to_download.is_empty() && self.conflicts.is_empty()
    }

    pub fn total_changes(&self) -> usize {
        self.to_upload.len() + self.to_download.len() + self.conflicts.len()
    }
}

// ---------------------------------------------------------------------------
// 11. Sync engine
// ---------------------------------------------------------------------------

pub struct SyncEngine {
    local_device: DeviceId,
    keypair: X25519KeyPair,
    devices: BTreeMap<DeviceId, DeviceRecord>,
    manifest: SyncManifest,
    items: BTreeMap<SyncItemId, SyncItem>,
    item_states: BTreeMap<SyncItemId, ItemSyncState>,
    conflict_policy: ConflictPolicy,
    pairing: PairingCeremony,
    progress: SyncProgress,
    sequence_counter: u64,
    enabled: bool,
}

impl SyncEngine {
    pub fn new(
        device_id: DeviceId,
        seed: [u8; 32],
        default_strategy: ConflictStrategy,
        now: u64,
    ) -> Self {
        Self {
            local_device: device_id,
            keypair: X25519KeyPair::from_seed(seed),
            devices: BTreeMap::new(),
            manifest: SyncManifest::new(device_id),
            items: BTreeMap::new(),
            item_states: BTreeMap::new(),
            conflict_policy: ConflictPolicy::new(default_strategy),
            pairing: PairingCeremony::new(),
            progress: SyncProgress::new(now),
            sequence_counter: 0,
            enabled: false,
        }
    }

    pub fn local_device_id(&self) -> DeviceId {
        self.local_device
    }

    pub fn public_key(&self) -> &[u8; 32] {
        &self.keypair.public_key
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn disable(&mut self) {
        self.enabled = false;
    }

    // --- Device management ---

    pub fn initiate_pairing(&mut self, pin: u32, now: u64) -> &PairingChallenge {
        self.pairing
            .initiate(self.local_device, self.keypair.public_key, pin, now)
    }

    pub fn complete_pairing(
        &mut self,
        response: &PairingResponse,
        now: u64,
    ) -> Result<DeviceId, PairingError> {
        if self.devices.contains_key(&response.responder) {
            return Err(PairingError::AlreadyPaired);
        }

        let record = self.pairing.complete(self.local_device, response, now)?;
        let id = record.device_id;
        self.devices.insert(id, record);
        Ok(id)
    }

    pub fn add_device(&mut self, record: DeviceRecord) -> Result<(), PairingError> {
        if self.devices.contains_key(&record.device_id) {
            return Err(PairingError::AlreadyPaired);
        }
        self.devices.insert(record.device_id, record);
        Ok(())
    }

    pub fn remove_device(&mut self, device_id: &DeviceId) -> bool {
        self.devices.remove(device_id).is_some()
    }

    pub fn set_trust_level(
        &mut self,
        device_id: &DeviceId,
        level: TrustLevel,
    ) -> Option<TrustLevel> {
        self.devices.get_mut(device_id).map(|d| {
            let old = d.trust_level;
            d.trust_level = level;
            old
        })
    }

    pub fn revoke_device(&mut self, device_id: &DeviceId) -> bool {
        if let Some(device) = self.devices.get_mut(device_id) {
            device.trust_level = TrustLevel::Revoked;
            true
        } else {
            false
        }
    }

    pub fn get_device(&self, device_id: &DeviceId) -> Option<&DeviceRecord> {
        self.devices.get(device_id)
    }

    pub fn list_devices(&self) -> Vec<&DeviceRecord> {
        self.devices.values().collect()
    }

    pub fn list_trusted_devices(&self) -> Vec<&DeviceRecord> {
        self.devices
            .values()
            .filter(|d| d.trust_level.can_sync())
            .collect()
    }

    pub fn device_count(&self) -> usize {
        self.devices.len()
    }

    pub fn update_device_sync_scope(&mut self, device_id: &DeviceId, scope: SyncScope) -> bool {
        if let Some(device) = self.devices.get_mut(device_id) {
            device.sync_scope = scope;
            true
        } else {
            false
        }
    }

    // --- Item management ---

    pub fn register_item(&mut self, item: SyncItem) {
        let id = item.id;
        let entry = SyncManifestEntry {
            item_type: item.item_type,
            version: item.version,
            data_hash: item.data_hash,
            modified_at: item.modified_at,
            size_bytes: item.size_bytes,
            is_deleted: false,
        };
        self.manifest.add_entry(id, entry);

        self.item_states.insert(
            id,
            ItemSyncState {
                item_id: id,
                status: ItemSyncStatus::PendingUpload,
                local_version: item.version,
                remote_version: 0,
                last_synced_version: 0,
                last_sync_time: 0,
                retry_count: 0,
            },
        );

        self.items.insert(id, item);
    }

    pub fn update_item(&mut self, id: &SyncItemId, new_data: Vec<u8>, hash: [u8; 32], now: u64) {
        if let Some(item) = self.items.get_mut(id) {
            item.version += 1;
            item.data_hash = hash;
            item.modified_at = now;
            item.encrypted_payload = new_data;
            item.size_bytes = item.encrypted_payload.len() as u64;

            if let Some(entry) = self.manifest.items.get_mut(id) {
                entry.version = item.version;
                entry.data_hash = hash;
                entry.modified_at = now;
                entry.size_bytes = item.size_bytes;
            }
            self.manifest.version += 1;

            if let Some(state) = self.item_states.get_mut(id) {
                state.local_version = item.version;
                state.status = ItemSyncStatus::PendingUpload;
            }
        }
    }

    pub fn delete_item(&mut self, id: &SyncItemId) -> bool {
        if self.items.remove(id).is_some() {
            self.manifest.mark_deleted(id);
            self.item_states.remove(id);
            true
        } else {
            false
        }
    }

    pub fn get_item(&self, id: &SyncItemId) -> Option<&SyncItem> {
        self.items.get(id)
    }

    pub fn list_items_by_type(&self, item_type: SyncItemType) -> Vec<&SyncItem> {
        self.items
            .values()
            .filter(|i| i.item_type == item_type)
            .collect()
    }

    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    pub fn get_item_status(&self, id: &SyncItemId) -> Option<ItemSyncStatus> {
        self.item_states.get(id).map(|s| s.status)
    }

    // --- Sync operations ---

    /// Begin a sync session with a specific peer.
    pub fn begin_sync(&mut self, peer_id: DeviceId, now: u64) -> Result<(), SyncError> {
        if !self.enabled {
            return Err(SyncError::ProtocolError);
        }

        let device = self
            .devices
            .get(&peer_id)
            .ok_or(SyncError::PeerUnreachable(peer_id))?;

        if !device.trust_level.can_sync() {
            return Err(SyncError::TrustRevoked(peer_id));
        }

        self.progress = SyncProgress::new(now);
        self.progress.phase = SyncPhase::Discovering;
        self.progress.peer_device = Some(peer_id);
        Ok(())
    }

    /// Negotiate sync by computing the delta against a remote manifest.
    pub fn negotiate(&mut self, remote_manifest: &SyncManifest) -> SyncDelta {
        self.progress.phase = SyncPhase::Negotiating;
        let delta = self.manifest.compute_delta(remote_manifest);
        self.progress.total_items = delta.total_changes();
        delta
    }

    /// Prepare encrypted payloads for items that need uploading.
    pub fn prepare_upload(
        &mut self,
        item_ids: &[SyncItemId],
        peer_id: &DeviceId,
        now: u64,
    ) -> Result<Vec<SealedPayload>, SyncError> {
        let device = self
            .devices
            .get(peer_id)
            .ok_or(SyncError::PeerUnreachable(*peer_id))?;

        if !device.trust_level.can_sync() {
            return Err(SyncError::TrustRevoked(*peer_id));
        }

        let shared_secret = self.keypair.diffie_hellman(&device.public_key);
        let (enc_key, base_nonce) = Hkdf::derive_session_keys(&shared_secret, b"raesync-upload");

        let mut payloads = Vec::with_capacity(item_ids.len());

        for id in item_ids {
            let item = match self.items.get(id) {
                Some(i) => i,
                None => continue,
            };

            if !device.sync_scope.allows(item.item_type) {
                continue;
            }
            if !device.trust_level.can_sync_type(item.item_type) {
                continue;
            }

            self.sequence_counter += 1;

            // Derive a unique nonce per message from the base nonce + sequence
            let mut nonce = base_nonce;
            let seq_bytes = self.sequence_counter.to_le_bytes();
            for i in 0..8 {
                nonce[i] ^= seq_bytes[i];
            }

            let payload = SealedPayload::seal(
                self.local_device,
                *peer_id,
                self.sequence_counter,
                &enc_key,
                nonce,
                &item.encrypted_payload,
                now,
            );
            payloads.push(payload);

            if let Some(state) = self.item_states.get_mut(id) {
                state.status = ItemSyncStatus::Uploading;
            }
        }

        self.progress.phase = SyncPhase::Syncing;
        Ok(payloads)
    }

    /// Apply downloaded items from sealed payloads.
    pub fn apply_download(
        &mut self,
        payloads: &[SealedPayload],
        _remote_manifest: &SyncManifest,
        now: u64,
    ) -> Result<usize, SyncError> {
        let mut applied = 0;

        for payload in payloads {
            let device = self
                .devices
                .get(&payload.sender)
                .ok_or(SyncError::PeerUnreachable(payload.sender))?;

            let shared_secret = self.keypair.diffie_hellman(&device.public_key);
            let (enc_key, _) = Hkdf::derive_session_keys(&shared_secret, b"raesync-upload");

            let plaintext = payload
                .unseal(&enc_key)
                .ok_or(SyncError::DecryptionFailed)?;

            // Find the corresponding remote manifest entry to get metadata
            // In a real protocol, item ID would be in the AAD or payload header
            // For now, apply sequentially
            self.progress.bytes_transferred += plaintext.len() as u64;
            self.progress.completed_items += 1;
            applied += 1;

            // Store the raw payload — in production, we'd reconstruct the SyncItem
            let _ = plaintext;
        }

        if let Some(peer_id) = self.progress.peer_device {
            if let Some(device) = self.devices.get_mut(&peer_id) {
                device.last_sync = now;
                device.last_seen = now;
            }
        }

        self.manifest.last_sync = now;
        Ok(applied)
    }

    /// Resolve all conflicts in a delta using the configured policy.
    pub fn resolve_conflicts(&mut self, delta: &mut SyncDelta) -> Vec<ConflictStrategy> {
        self.progress.phase = SyncPhase::Resolving;
        let mut resolutions = Vec::with_capacity(delta.conflicts.len());

        for conflict in &mut delta.conflicts {
            let strategy = conflict.auto_resolve(&self.conflict_policy);
            resolutions.push(strategy);
        }

        resolutions
    }

    /// Complete a sync session.
    pub fn complete_sync(&mut self, now: u64) {
        self.progress.phase = SyncPhase::Complete;
        self.manifest.last_sync = now;

        // Mark all uploading/downloading items as synced
        for state in self.item_states.values_mut() {
            match state.status {
                ItemSyncStatus::Uploading | ItemSyncStatus::Downloading => {
                    state.status = ItemSyncStatus::InSync;
                    state.last_synced_version = state.local_version;
                    state.last_sync_time = now;
                }
                _ => {}
            }
        }
    }

    /// Mark sync as failed.
    pub fn fail_sync(&mut self, error: SyncError) {
        self.progress.phase = SyncPhase::Failed;
        self.progress.errors.push(error);

        for state in self.item_states.values_mut() {
            match state.status {
                ItemSyncStatus::Uploading | ItemSyncStatus::Downloading => {
                    state.status = ItemSyncStatus::Error;
                    state.retry_count += 1;
                }
                _ => {}
            }
        }
    }

    pub fn progress(&self) -> &SyncProgress {
        &self.progress
    }

    // --- Conflict policy ---

    pub fn set_conflict_policy(&mut self, policy: ConflictPolicy) {
        self.conflict_policy = policy;
    }

    pub fn set_type_conflict_strategy(
        &mut self,
        item_type: SyncItemType,
        strategy: ConflictStrategy,
    ) {
        self.conflict_policy.set_override(item_type, strategy);
    }

    pub fn default_conflict_strategy(&self) -> ConflictStrategy {
        self.conflict_policy.default_strategy
    }

    // --- Cleanup ---

    pub fn cleanup_expired_pairings(&mut self, now: u64) {
        self.pairing.cleanup_expired(now);
    }

    pub fn prune_stale_devices(&mut self, now: u64, stale_threshold: u64) -> usize {
        let stale: Vec<DeviceId> = self
            .devices
            .values()
            .filter(|d| !d.is_active(now, stale_threshold))
            .map(|d| d.device_id)
            .collect();
        let count = stale.len();
        for id in stale {
            self.devices.remove(&id);
        }
        count
    }

    pub fn purge_deleted_items(&mut self) -> usize {
        let deleted: Vec<SyncItemId> = self
            .manifest
            .items
            .iter()
            .filter(|(_, e)| e.is_deleted)
            .map(|(id, _)| *id)
            .collect();
        let count = deleted.len();
        for id in &deleted {
            self.manifest.items.remove(id);
            self.items.remove(id);
            self.item_states.remove(id);
        }
        count
    }

    // --- Statistics ---

    pub fn sync_stats(&self) -> SyncStats {
        let mut pending_upload = 0;
        let mut pending_download = 0;
        let mut in_sync = 0;
        let mut conflicts = 0;
        let mut errors = 0;

        for state in self.item_states.values() {
            match state.status {
                ItemSyncStatus::PendingUpload | ItemSyncStatus::Uploading => pending_upload += 1,
                ItemSyncStatus::PendingDownload | ItemSyncStatus::Downloading => {
                    pending_download += 1
                }
                ItemSyncStatus::InSync => in_sync += 1,
                ItemSyncStatus::Conflict => conflicts += 1,
                ItemSyncStatus::Error => errors += 1,
                _ => {}
            }
        }

        SyncStats {
            total_items: self.items.len(),
            in_sync,
            pending_upload,
            pending_download,
            conflicts,
            errors,
            total_devices: self.devices.len(),
            trusted_devices: self
                .devices
                .values()
                .filter(|d| d.trust_level.can_sync())
                .count(),
            manifest_version: self.manifest.version,
            last_sync: self.manifest.last_sync,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SyncStats {
    pub total_items: usize,
    pub in_sync: usize,
    pub pending_upload: usize,
    pub pending_download: usize,
    pub conflicts: usize,
    pub errors: usize,
    pub total_devices: usize,
    pub trusted_devices: usize,
    pub manifest_version: u64,
    pub last_sync: u64,
}

// ---------------------------------------------------------------------------
// 12. Sync item builders for common types
// ---------------------------------------------------------------------------

pub fn make_settings_item(
    id: SyncItemId,
    name: String,
    data: Vec<u8>,
    hash: [u8; 32],
    device: DeviceId,
    now: u64,
) -> SyncItem {
    let size = data.len() as u64;
    SyncItem {
        id,
        item_type: SyncItemType::Settings,
        version: 1,
        data_hash: hash,
        created_at: now,
        modified_at: now,
        origin_device: device,
        size_bytes: size,
        encrypted_payload: data,
        metadata: SyncItemMetadata::new(name),
    }
}

pub fn make_bookmark_item(
    id: SyncItemId,
    name: String,
    url_data: Vec<u8>,
    hash: [u8; 32],
    device: DeviceId,
    now: u64,
) -> SyncItem {
    let size = url_data.len() as u64;
    SyncItem {
        id,
        item_type: SyncItemType::Bookmarks,
        version: 1,
        data_hash: hash,
        created_at: now,
        modified_at: now,
        origin_device: device,
        size_bytes: size,
        encrypted_payload: url_data,
        metadata: SyncItemMetadata::new(name),
    }
}

pub fn make_clipboard_item(
    id: SyncItemId,
    data: Vec<u8>,
    hash: [u8; 32],
    device: DeviceId,
    now: u64,
) -> SyncItem {
    let size = data.len() as u64;
    let mut meta = SyncItemMetadata::new(String::from("clipboard"));
    meta.delete_after = Some(now + 86400); // clipboard items expire after 24h
    SyncItem {
        id,
        item_type: SyncItemType::Clipboard,
        version: 1,
        data_hash: hash,
        created_at: now,
        modified_at: now,
        origin_device: device,
        size_bytes: size,
        encrypted_payload: data,
        metadata: meta,
    }
}

pub fn make_wifi_item(
    id: SyncItemId,
    ssid: String,
    encrypted_psk: Vec<u8>,
    hash: [u8; 32],
    device: DeviceId,
    now: u64,
) -> SyncItem {
    let size = encrypted_psk.len() as u64;
    SyncItem {
        id,
        item_type: SyncItemType::WifiPasswords,
        version: 1,
        data_hash: hash,
        created_at: now,
        modified_at: now,
        origin_device: device,
        size_bytes: size,
        encrypted_payload: encrypted_psk,
        metadata: SyncItemMetadata::new(ssid),
    }
}

pub fn make_file_item(
    id: SyncItemId,
    filename: String,
    mime: String,
    data: Vec<u8>,
    hash: [u8; 32],
    device: DeviceId,
    now: u64,
) -> SyncItem {
    let size = data.len() as u64;
    let mut meta = SyncItemMetadata::new(filename);
    meta.mime_type = Some(mime);
    SyncItem {
        id,
        item_type: SyncItemType::Files,
        version: 1,
        data_hash: hash,
        created_at: now,
        modified_at: now,
        origin_device: device,
        size_bytes: size,
        encrypted_payload: data,
        metadata: meta,
    }
}

pub fn make_game_save_item(
    id: SyncItemId,
    game_name: String,
    save_data: Vec<u8>,
    hash: [u8; 32],
    device: DeviceId,
    now: u64,
) -> SyncItem {
    let size = save_data.len() as u64;
    let mut meta = SyncItemMetadata::new(game_name);
    meta.tags.push(String::from("game-save"));
    SyncItem {
        id,
        item_type: SyncItemType::GameSaves,
        version: 1,
        data_hash: hash,
        created_at: now,
        modified_at: now,
        origin_device: device,
        size_bytes: size,
        encrypted_payload: save_data,
        metadata: meta,
    }
}

// ---------------------------------------------------------------------------
// 13. Init
// ---------------------------------------------------------------------------

/// Initialize the RaeSync subsystem. Sync is disabled by default —
/// the user must explicitly opt in. Local-only operation is the default.
pub fn init(device_id: DeviceId, seed: [u8; 32], now: u64) -> SyncEngine {
    SyncEngine::new(device_id, seed, ConflictStrategy::KeepNewer, now)
}

// ---------------------------------------------------------------------------
// 14. E2E sync core — zero-knowledge blob store + LWW-register CRDT
// ---------------------------------------------------------------------------
//
// "RaeSync: end-to-end-encrypted cross-device sync" — RaeenOS_Concept.md,
// §The Proprietary Stack. And the user-ownership pillar: "The user owns the
// machine" — sync is OPT-IN and the server is ZERO-KNOWLEDGE. The server only
// ever stores `SyncBlob`s: opaque AEAD ciphertext + a signature. It holds NO
// key material and CANNOT read any value. Only devices enrolled into the sync
// group (each holding the group key, wrapped to their X25519 public key) can
// decrypt. Every blob is Ed25519-signed by its originating device so a
// tampering or impersonating server is rejected even though the payload is
// already authenticated by the AEAD.
//
// Conflict model: a **last-writer-wins (LWW) register** per record key,
// ordered by a monotonic Lamport clock with the device id as the deterministic
// tiebreak. This is the right CRDT for settings / themes / bookmarks (the
// buckets RaeSync targets): every device that has seen the same set of blobs
// converges to the SAME value with no coordination. (An OR-set would layer on
// top for collection-typed buckets; LWW-register is the documented core.)

/// An Ed25519 identity keypair (signing) plus an X25519 key (encryption) for a
/// single enrolled device. `ed_seed` / `x_secret` are the secret halves and
/// never leave the device; only the public halves are shared.
#[derive(Clone)]
pub struct DeviceKeys {
    pub device_id: DeviceId,
    pub ed_seed: [u8; 32],
    pub ed_public: [u8; 32],
    pub x_secret: [u8; 32],
    pub x_public: [u8; 32],
}

impl core::fmt::Debug for DeviceKeys {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DeviceKeys")
            .field("device_id", &self.device_id)
            .field("ed_public", &self.ed_public)
            .field("x_public", &self.x_public)
            .field("secrets", &"[REDACTED]")
            .finish()
    }
}

impl DeviceKeys {
    /// Build a device's keypairs from two independent 32-byte secret seeds.
    pub fn from_seeds(device_id: DeviceId, ed_seed: [u8; 32], x_secret: [u8; 32]) -> Self {
        Self {
            device_id,
            ed_public: rae_crypto::ed25519::derive_public_key(&ed_seed),
            x_public: rae_crypto::x25519::public_key(&x_secret),
            ed_seed,
            x_secret,
        }
    }

    /// The public identity this device advertises for enrollment.
    pub fn identity(&self) -> DeviceIdentity {
        DeviceIdentity {
            device_id: self.device_id,
            ed_public: self.ed_public,
            x_public: self.x_public,
        }
    }
}

/// The public identity of an enrolled device, as held by every other member of
/// the sync group. `apply_remote` verifies incoming blob signatures against the
/// `ed_public` recorded here, so only enrolled devices can author accepted
/// records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub device_id: DeviceId,
    pub ed_public: [u8; 32],
    pub x_public: [u8; 32],
}

/// The symmetric group key shared by every device in a sync group. It is never
/// sent in the clear: the account holder wraps it per-device with
/// `wrap_group_key_for`. All record AEAD uses this key.
#[derive(Clone, PartialEq, Eq)]
pub struct GroupKey(pub [u8; 32]);

impl core::fmt::Debug for GroupKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GroupKey")
            .field("key", &"[REDACTED]")
            .finish()
    }
}

/// A group key wrapped (encrypted) for one specific device. Produced by the
/// account holder via ECDH(holder.x_secret, device.x_public) + AEAD; the device
/// unwraps it with ECDH(device.x_secret, holder.x_public). The server may store
/// and relay this blob without ever learning the group key.
#[derive(Clone, Debug)]
pub struct WrappedGroupKey {
    /// Device this wrap is destined for.
    pub recipient: DeviceId,
    /// X25519 public key of the wrapping account holder (so the recipient can
    /// recompute the ECDH shared secret).
    pub wrapper_x_public: [u8; 32],
    pub nonce: [u8; 12],
    /// AEAD(ciphertext || tag) of the 32-byte group key.
    pub ciphertext: Vec<u8>,
}

/// Errors from the E2E sync core. Every crypto failure is reported here rather
/// than panicking — untrusted bytes can never crash a device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum E2eError {
    /// AEAD authentication failed (wrong key, tampered ciphertext, wrong AAD).
    DecryptFailed,
    /// Ed25519 signature did not verify against an enrolled device identity.
    BadSignature,
    /// The originating device is not enrolled in this group.
    UnknownDevice,
    /// A wrapped/unwrapped group key was not exactly 32 bytes.
    MalformedKey,
    /// A blob field was truncated or otherwise structurally invalid.
    MalformedBlob,
}

/// Derive the per-pair key-wrapping key from an X25519 shared secret. HKDF binds
/// it to a fixed context so it can never be confused with a record AEAD key.
fn derive_wrap_key(shared: &[u8; 32]) -> [u8; 32] {
    let prk = rae_crypto::sha256::hkdf_extract(b"raesync-groupkey-wrap-v1", shared);
    let mut out = [0u8; 32];
    rae_crypto::sha256::hkdf_expand(&prk, b"raesync-groupkey-wrap", &mut out);
    out
}

/// Wrap the group key for one device: ECDH(holder ⇄ device) → HKDF → AEAD.
/// Only the holder of `device_x_public`'s matching secret can unwrap it.
pub fn wrap_group_key_for(
    group_key: &GroupKey,
    holder_x_secret: &[u8; 32],
    holder_x_public: &[u8; 32],
    recipient: DeviceId,
    recipient_x_public: &[u8; 32],
    nonce: [u8; 12],
) -> WrappedGroupKey {
    let shared = rae_crypto::x25519::diffie_hellman(holder_x_secret, recipient_x_public);
    let wrap_key = derive_wrap_key(&shared);
    // AAD binds the recipient id + the holder's public key so a wrap cannot be
    // replayed against a different device or attributed to a different holder.
    let mut aad = Vec::with_capacity(48);
    aad.extend_from_slice(&recipient.0);
    aad.extend_from_slice(holder_x_public);
    let ciphertext = rae_crypto::chacha20poly1305::seal(&wrap_key, &nonce, &aad, &group_key.0);
    WrappedGroupKey {
        recipient,
        wrapper_x_public: *holder_x_public,
        nonce,
        ciphertext,
    }
}

/// Unwrap a group key destined for `device`. Fails closed on a bad tag, a wrong
/// recipient, or a non-32-byte payload.
pub fn unwrap_group_key(
    wrapped: &WrappedGroupKey,
    device: &DeviceKeys,
) -> Result<GroupKey, E2eError> {
    if wrapped.recipient != device.device_id {
        return Err(E2eError::UnknownDevice);
    }
    let shared = rae_crypto::x25519::diffie_hellman(&device.x_secret, &wrapped.wrapper_x_public);
    let wrap_key = derive_wrap_key(&shared);
    let mut aad = Vec::with_capacity(48);
    aad.extend_from_slice(&device.device_id.0);
    aad.extend_from_slice(&wrapped.wrapper_x_public);
    let pt =
        rae_crypto::chacha20poly1305::open(&wrap_key, &wrapped.nonce, &aad, &wrapped.ciphertext)
            .ok_or(E2eError::DecryptFailed)?;
    if pt.len() != 32 {
        return Err(E2eError::MalformedKey);
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&pt);
    Ok(GroupKey(key))
}

/// A single encrypted, signed sync record — the ONLY thing the server ever
/// sees. `record_key` and the Lamport/device ordering are plaintext (the server
/// needs them to index and the CRDT needs them to merge); `value` is sealed.
#[derive(Clone, Debug)]
pub struct SyncBlob {
    /// Application-level key (e.g. "settings/theme"), plaintext for indexing.
    pub record_key: String,
    /// Lamport clock of this write.
    pub lamport: u64,
    /// Authoring device (also the CRDT tiebreak and the signature identity).
    pub device_id: DeviceId,
    /// AEAD nonce for the value.
    pub nonce: [u8; 12],
    /// AEAD(value) ciphertext || tag, under the group key.
    pub ciphertext: Vec<u8>,
    /// Ed25519 signature by `device_id` over the canonical signing transcript.
    pub signature: [u8; 64],
}

impl SyncBlob {
    /// The AEAD associated data binds the record key + Lamport + device id +
    /// nonce into the ciphertext so none can be swapped without detection.
    fn aad(record_key: &str, lamport: u64, device_id: DeviceId, nonce: &[u8; 12]) -> Vec<u8> {
        let mut aad = Vec::with_capacity(record_key.len() + 8 + 16 + 12 + 8);
        aad.extend_from_slice(b"raesync-record-v1");
        aad.extend_from_slice(&(record_key.len() as u64).to_le_bytes());
        aad.extend_from_slice(record_key.as_bytes());
        aad.extend_from_slice(&lamport.to_le_bytes());
        aad.extend_from_slice(&device_id.0);
        aad.extend_from_slice(nonce);
        aad
    }

    /// The Ed25519 signing transcript — every field except the signature, so a
    /// server cannot alter ANY plaintext field (key/clock/device/nonce/ct)
    /// without invalidating the signature.
    fn signing_transcript(
        record_key: &str,
        lamport: u64,
        device_id: DeviceId,
        nonce: &[u8; 12],
        ciphertext: &[u8],
    ) -> Vec<u8> {
        let mut t = Vec::with_capacity(record_key.len() + ciphertext.len() + 64);
        t.extend_from_slice(b"raesync-sig-v1");
        t.extend_from_slice(&(record_key.len() as u64).to_le_bytes());
        t.extend_from_slice(record_key.as_bytes());
        t.extend_from_slice(&lamport.to_le_bytes());
        t.extend_from_slice(&device_id.0);
        t.extend_from_slice(nonce);
        t.extend_from_slice(&(ciphertext.len() as u64).to_le_bytes());
        t.extend_from_slice(ciphertext);
        t
    }

    /// Verify the blob's Ed25519 signature against the authoring identity. Pure,
    /// never panics on attacker-chosen field contents.
    pub fn verify_signature(&self, ed_public: &[u8; 32]) -> bool {
        let t = Self::signing_transcript(
            &self.record_key,
            self.lamport,
            self.device_id,
            &self.nonce,
            &self.ciphertext,
        );
        rae_crypto::ed25519::verify(ed_public, &t, &self.signature)
    }
}

/// Encrypt + sign a record into a `SyncBlob`. The value is sealed under the
/// group key; the blob is signed by the authoring device's Ed25519 key.
pub fn encrypt_record(
    record_key: &str,
    value: &[u8],
    lamport: u64,
    device: &DeviceKeys,
    group_key: &GroupKey,
    nonce: [u8; 12],
) -> SyncBlob {
    let aad = SyncBlob::aad(record_key, lamport, device.device_id, &nonce);
    let ciphertext = rae_crypto::chacha20poly1305::seal(&group_key.0, &nonce, &aad, value);
    let transcript =
        SyncBlob::signing_transcript(record_key, lamport, device.device_id, &nonce, &ciphertext);
    let signature = rae_crypto::ed25519::sign(&device.ed_seed, &transcript);
    SyncBlob {
        record_key: String::from(record_key),
        lamport,
        device_id: device.device_id,
        nonce,
        ciphertext,
        signature,
    }
}

/// Decrypt a record's value with the group key. Fails closed on a wrong key, a
/// tampered ciphertext, or a swapped AAD field. Does NOT check the signature —
/// callers in the sync path use `SyncState::apply_remote`, which verifies first.
pub fn decrypt_record(blob: &SyncBlob, group_key: &GroupKey) -> Result<Vec<u8>, E2eError> {
    let aad = SyncBlob::aad(&blob.record_key, blob.lamport, blob.device_id, &blob.nonce);
    rae_crypto::chacha20poly1305::open(&group_key.0, &blob.nonce, &aad, &blob.ciphertext)
        .ok_or(E2eError::DecryptFailed)
}

/// One converged register cell: the winning (lamport, device_id) and the
/// decrypted value at that write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisterCell {
    pub lamport: u64,
    pub device_id: DeviceId,
    pub value: Vec<u8>,
}

impl RegisterCell {
    /// LWW ordering: higher Lamport wins; ties broken by larger device id. This
    /// total order is what makes every device converge to the SAME cell.
    fn dominates(&self, lamport: u64, device_id: DeviceId) -> bool {
        match self.lamport.cmp(&lamport) {
            core::cmp::Ordering::Greater => true,
            core::cmp::Ordering::Less => false,
            core::cmp::Ordering::Equal => self.device_id >= device_id,
        }
    }
}

/// A device's local view of the synced state: an LWW-register map plus the
/// enrolled-device roster, the group key, and this device's own keys. Two
/// devices that have observed the same set of `SyncBlob`s hold identical maps.
pub struct SyncState {
    device: DeviceKeys,
    group_key: GroupKey,
    enrolled: BTreeMap<DeviceId, DeviceIdentity>,
    records: BTreeMap<String, RegisterCell>,
    lamport: u64,
}

impl SyncState {
    /// Create local state. The device enrolls itself automatically so its own
    /// blobs verify on round-trip.
    pub fn new(device: DeviceKeys, group_key: GroupKey) -> Self {
        let mut enrolled = BTreeMap::new();
        enrolled.insert(device.device_id, device.identity());
        Self {
            device,
            group_key,
            enrolled,
            records: BTreeMap::new(),
            lamport: 0,
        }
    }

    /// Enroll another device's public identity so its authored blobs are
    /// accepted by `apply_remote`.
    pub fn enroll_device(&mut self, identity: DeviceIdentity) {
        self.enrolled.insert(identity.device_id, identity);
    }

    /// Remove (revoke) a device; its future and replayed blobs are rejected.
    pub fn revoke_device(&mut self, device_id: &DeviceId) -> bool {
        self.enrolled.remove(device_id).is_some()
    }

    pub fn is_enrolled(&self, device_id: &DeviceId) -> bool {
        self.enrolled.contains_key(device_id)
    }

    /// Current Lamport clock.
    pub fn lamport(&self) -> u64 {
        self.lamport
    }

    /// Read the current converged value for a record key.
    pub fn get(&self, record_key: &str) -> Option<&[u8]> {
        self.records.get(record_key).map(|c| c.value.as_slice())
    }

    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    /// Local write: bump the Lamport clock, update the local register, and
    /// produce a signed+encrypted blob to upload to the (dumb) server.
    pub fn local_set(&mut self, record_key: &str, value: Vec<u8>, nonce: [u8; 12]) -> SyncBlob {
        self.lamport += 1;
        let lamport = self.lamport;
        let device_id = self.device.device_id;
        let blob = encrypt_record(
            record_key,
            &value,
            lamport,
            &self.device,
            &self.group_key,
            nonce,
        );
        // Apply locally under the same LWW rule (we always dominate here since
        // we just produced the highest clock for this key).
        self.merge_cell(record_key, lamport, device_id, value);
        blob
    }

    /// Apply a remote blob: verify the signature against the enrolled author,
    /// decrypt under the group key, advance the Lamport clock, and merge under
    /// the deterministic LWW rule. Fails closed; never panics on garbage input.
    pub fn apply_remote(&mut self, blob: &SyncBlob) -> Result<(), E2eError> {
        let identity = self
            .enrolled
            .get(&blob.device_id)
            .ok_or(E2eError::UnknownDevice)?;
        if !blob.verify_signature(&identity.ed_public) {
            return Err(E2eError::BadSignature);
        }
        let value = decrypt_record(blob, &self.group_key)?;
        // Lamport receive rule: clock = max(local, received).
        if blob.lamport > self.lamport {
            self.lamport = blob.lamport;
        }
        self.merge_cell(&blob.record_key, blob.lamport, blob.device_id, value);
        Ok(())
    }

    /// The deterministic LWW merge. Identical on every device for the same
    /// inputs regardless of arrival order — this is the convergence guarantee.
    fn merge_cell(&mut self, record_key: &str, lamport: u64, device_id: DeviceId, value: Vec<u8>) {
        match self.records.get(record_key) {
            Some(existing) if existing.dominates(lamport, device_id) => {
                // Existing write wins — drop the incoming one.
            }
            _ => {
                self.records.insert(
                    String::from(record_key),
                    RegisterCell {
                        lamport,
                        device_id,
                        value,
                    },
                );
            }
        }
    }
}

#[cfg(test)]
mod e2e_tests {
    use super::*;

    fn dev(tag: u8) -> DeviceKeys {
        let mut id = [0u8; 16];
        id[0] = tag;
        DeviceKeys::from_seeds(DeviceId(id), [tag ^ 0xA1; 32], [tag ^ 0x5C; 32])
    }

    fn nonce(n: u8) -> [u8; 12] {
        [n; 12]
    }

    #[test]
    fn record_roundtrip_recovers_plaintext() {
        let a = dev(1);
        let gk = GroupKey([0x42; 32]);
        let blob = encrypt_record("settings/theme", b"glass-dark", 1, &a, &gk, nonce(7));
        let pt = decrypt_record(&blob, &gk).expect("decrypt");
        assert_eq!(pt.as_slice(), b"glass-dark");
        // Signature verifies against the author's public key.
        assert!(blob.verify_signature(&a.ed_public));
    }

    #[test]
    fn wrong_group_key_fails_closed() {
        let a = dev(1);
        let gk = GroupKey([0x42; 32]);
        let blob = encrypt_record("k", b"v", 1, &a, &gk, nonce(1));
        let wrong = GroupKey([0x43; 32]);
        assert_eq!(decrypt_record(&blob, &wrong), Err(E2eError::DecryptFailed));
    }

    #[test]
    fn flipped_ciphertext_byte_fails_closed() {
        let a = dev(1);
        let gk = GroupKey([0x42; 32]);
        let mut blob = encrypt_record("k", b"value-bytes", 1, &a, &gk, nonce(2));
        blob.ciphertext[0] ^= 0x01;
        assert_eq!(decrypt_record(&blob, &gk), Err(E2eError::DecryptFailed));
    }

    #[test]
    fn wrong_aad_field_fails_closed() {
        let a = dev(1);
        let gk = GroupKey([0x42; 32]);
        let mut blob = encrypt_record("k", b"value-bytes", 1, &a, &gk, nonce(3));
        // Swap a plaintext field that is bound into the AAD -> decrypt must fail.
        blob.lamport = 999;
        assert_eq!(decrypt_record(&blob, &gk), Err(E2eError::DecryptFailed));
    }

    #[test]
    fn bad_signature_is_rejected() {
        let a = dev(1);
        let b = dev(2);
        let gk = GroupKey([0x42; 32]);
        let mut state = SyncState::new(b.clone(), gk.clone());
        state.enroll_device(a.identity());
        let mut blob = encrypt_record("k", b"v", 1, &a, &gk, nonce(4));
        // Tamper the signature.
        blob.signature[0] ^= 0xFF;
        assert_eq!(state.apply_remote(&blob), Err(E2eError::BadSignature));
        // A forged author (signed by b but claiming a) is also rejected: the
        // device_id is bound into the signing transcript, so b's signature over
        // a's id won't verify against a's enrolled public key.
        let forged = encrypt_record_as(&a.device_id, "k", b"v", 1, &b, &gk, nonce(5));
        assert_eq!(state.apply_remote(&forged), Err(E2eError::BadSignature));
    }

    #[test]
    fn unenrolled_device_blob_is_rejected() {
        let a = dev(1);
        let stranger = dev(9);
        let gk = GroupKey([0x42; 32]);
        let mut state = SyncState::new(a.clone(), gk.clone());
        let blob = encrypt_record("k", b"v", 1, &stranger, &gk, nonce(6));
        assert_eq!(state.apply_remote(&blob), Err(E2eError::UnknownDevice));
    }

    #[test]
    fn higher_lamport_wins() {
        let a = dev(1);
        let b = dev(2);
        let gk = GroupKey([0x42; 32]);
        let mut state = SyncState::new(a.clone(), gk.clone());
        state.enroll_device(b.identity());
        let lo = encrypt_record("k", b"old", 1, &a, &gk, nonce(1));
        let hi = encrypt_record("k", b"new", 5, &b, &gk, nonce(2));
        state.apply_remote(&lo).unwrap();
        state.apply_remote(&hi).unwrap();
        assert_eq!(state.get("k"), Some(&b"new"[..]));
        // Reverse arrival order -> same winner (order-independence).
        let mut state2 = SyncState::new(a.clone(), gk.clone());
        state2.enroll_device(b.identity());
        state2.apply_remote(&hi).unwrap();
        state2.apply_remote(&lo).unwrap();
        assert_eq!(state2.get("k"), Some(&b"new"[..]));
    }

    #[test]
    fn same_lamport_tie_breaks_by_device_id() {
        let low = dev(1); // id[0]=1
        let high = dev(2); // id[0]=2 -> larger device id wins
        let gk = GroupKey([0x42; 32]);
        let from_low = encrypt_record("k", b"low", 3, &low, &gk, nonce(1));
        let from_high = encrypt_record("k", b"high", 3, &high, &gk, nonce(2));

        let mut s1 = SyncState::new(low.clone(), gk.clone());
        s1.enroll_device(high.identity());
        s1.apply_remote(&from_low).unwrap();
        s1.apply_remote(&from_high).unwrap();
        assert_eq!(s1.get("k"), Some(&b"high"[..]));

        // Opposite arrival order -> identical deterministic winner.
        let mut s2 = SyncState::new(low.clone(), gk.clone());
        s2.enroll_device(high.identity());
        s2.apply_remote(&from_high).unwrap();
        s2.apply_remote(&from_low).unwrap();
        assert_eq!(s2.get("k"), Some(&b"high"[..]));
    }

    #[test]
    fn concurrent_writes_converge_on_both_devices() {
        // THE CONVERGENCE PROOF. Two devices, both enrolled in each other's
        // roster, each write the SAME record key at the SAME Lamport clock
        // (concurrent), then exchange blobs. Both must end on the SAME value.
        let a = dev(1);
        let b = dev(2);
        let gk = GroupKey([0x42; 32]);

        let mut sa = SyncState::new(a.clone(), gk.clone());
        sa.enroll_device(b.identity());
        let mut sb = SyncState::new(b.clone(), gk.clone());
        sb.enroll_device(a.identity());

        // Each device sets the key locally (independent, concurrent writes).
        let blob_a = sa.local_set("settings/wallpaper", b"aurora".to_vec(), nonce(10));
        let blob_b = sb.local_set("settings/wallpaper", b"nebula".to_vec(), nonce(11));
        // Both produced lamport=1 locally — a genuine concurrent conflict.
        assert_eq!(blob_a.lamport, 1);
        assert_eq!(blob_b.lamport, 1);

        // Exchange: A receives B's blob, B receives A's blob.
        sa.apply_remote(&blob_b).unwrap();
        sb.apply_remote(&blob_a).unwrap();

        // Convergence: identical value on BOTH devices, decided by the device-id
        // tiebreak (b's id > a's id at lamport 1 -> "nebula").
        assert_eq!(sa.get("settings/wallpaper"), sb.get("settings/wallpaper"));
        assert_eq!(sa.get("settings/wallpaper"), Some(&b"nebula"[..]));
    }

    #[test]
    fn group_key_wrap_unwrap_roundtrips_for_enrolled_device() {
        // Account holder owns the group key + an X25519 keypair.
        let holder_x_secret = [0x77u8; 32];
        let holder_x_public = rae_crypto::x25519::public_key(&holder_x_secret);
        let gk = GroupKey([0xABu8; 32]);
        let device = dev(3);

        let wrapped = wrap_group_key_for(
            &gk,
            &holder_x_secret,
            &holder_x_public,
            device.device_id,
            &device.x_public,
            nonce(20),
        );
        let unwrapped = unwrap_group_key(&wrapped, &device).expect("unwrap");
        assert_eq!(unwrapped.0, gk.0);

        // A different (non-recipient) device cannot unwrap it.
        let other = dev(4);
        // Even re-addressed, the ECDH secret differs -> AEAD fails closed.
        let mut mis = wrapped.clone();
        mis.recipient = other.device_id;
        assert_eq!(unwrap_group_key(&mis, &other), Err(E2eError::DecryptFailed));
        // And addressed to the right id but unwrapped by the wrong device keys:
        assert_eq!(
            unwrap_group_key(&wrapped, &other),
            Err(E2eError::UnknownDevice)
        );
    }

    #[test]
    fn truncated_or_garbage_blob_never_panics() {
        let a = dev(1);
        let gk = GroupKey([0x42; 32]);
        // Empty ciphertext (shorter than a 16-byte tag) -> graceful error.
        let garbage = SyncBlob {
            record_key: String::from("k"),
            lamport: 1,
            device_id: a.device_id,
            nonce: nonce(1),
            ciphertext: Vec::new(),
            signature: [0u8; 64],
        };
        assert_eq!(decrypt_record(&garbage, &gk), Err(E2eError::DecryptFailed));
        assert!(!garbage.verify_signature(&a.ed_public));

        // A few bytes of random ciphertext, also no panic.
        let garbage2 = SyncBlob {
            record_key: String::from("k"),
            lamport: 1,
            device_id: a.device_id,
            nonce: nonce(1),
            ciphertext: vec![0xDE, 0xAD, 0xBE, 0xEF],
            signature: [0xFFu8; 64],
        };
        assert_eq!(decrypt_record(&garbage2, &gk), Err(E2eError::DecryptFailed));
        assert!(!garbage2.verify_signature(&a.ed_public));
    }

    /// Test-only helper: forge a blob that CLAIMS to be authored by
    /// `claimed_author` but is actually signed by `signer`. Used to prove the
    /// signature binds the device id.
    fn encrypt_record_as(
        claimed_author: &DeviceId,
        record_key: &str,
        value: &[u8],
        lamport: u64,
        signer: &DeviceKeys,
        group_key: &GroupKey,
        nonce: [u8; 12],
    ) -> SyncBlob {
        let aad = SyncBlob::aad(record_key, lamport, *claimed_author, &nonce);
        let ciphertext = rae_crypto::chacha20poly1305::seal(&group_key.0, &nonce, &aad, value);
        let transcript =
            SyncBlob::signing_transcript(record_key, lamport, *claimed_author, &nonce, &ciphertext);
        let signature = rae_crypto::ed25519::sign(&signer.ed_seed, &transcript);
        SyncBlob {
            record_key: String::from(record_key),
            lamport,
            device_id: *claimed_author,
            nonce,
            ciphertext,
            signature,
        }
    }
}

#[cfg(test)]
mod crypto_tests {
    use super::*;

    #[test]
    fn x25519_key_exchange_agrees() {
        // Two peers derive the same shared secret (real X25519, RFC 7748).
        let alice = X25519KeyPair::from_seed([0x11; 32]);
        let bob = X25519KeyPair::from_seed([0x22; 32]);
        let k_ab = alice.diffie_hellman(&bob.public_key);
        let k_ba = bob.diffie_hellman(&alice.public_key);
        assert_eq!(k_ab, k_ba);
        // A different peer must NOT land on the same secret.
        let mallory = X25519KeyPair::from_seed([0x33; 32]);
        assert_ne!(alice.diffie_hellman(&mallory.public_key), k_ab);
    }

    #[test]
    fn hkdf_is_real_rfc5869() {
        // RFC 5869 Appendix A.1 through the raesync wrappers.
        let ikm = [0x0bu8; 22];
        let salt: [u8; 13] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let info: [u8; 10] = [0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9];
        let prk = Hkdf::extract(&salt, &ikm);
        assert_eq!(prk, rae_crypto::sha256::hkdf_extract(&salt, &ikm));
        let okm = Hkdf::expand(&prk, &info, 42);
        assert_eq!(okm.len(), 42);
        assert_eq!(okm[0], 0x3c); // OKM = 3cb25f25...
        assert_eq!(okm[1], 0xb2);
    }

    #[test]
    fn aead_roundtrip_and_tamper_detected() {
        let key = EncryptionKey([7u8; 32]);
        let nonce = [9u8; 12];
        let aad = b"sync-aad";
        let pt = b"synchronized item payload";

        let ct = ChaCha20Poly1305::encrypt(&key, &nonce, aad, pt);
        // Correct key/nonce/aad -> plaintext recovered.
        assert_eq!(
            ChaCha20Poly1305::decrypt(&key, &nonce, aad, &ct).as_deref(),
            Some(&pt[..])
        );
        // Flipped ciphertext byte -> real HMAC tag mismatch -> rejected.
        let mut t1 = ct.clone();
        t1[0] ^= 0x01;
        assert!(ChaCha20Poly1305::decrypt(&key, &nonce, aad, &t1).is_none());
        // Flipped tag byte -> rejected.
        let mut t2 = ct.clone();
        let last = t2.len() - 1;
        t2[last] ^= 0x01;
        assert!(ChaCha20Poly1305::decrypt(&key, &nonce, aad, &t2).is_none());
        // Wrong key -> rejected.
        assert!(ChaCha20Poly1305::decrypt(&EncryptionKey([8u8; 32]), &nonce, aad, &ct).is_none());
        // Wrong AAD -> rejected (AAD is bound into the HMAC).
        assert!(ChaCha20Poly1305::decrypt(&key, &nonce, b"other-aad", &ct).is_none());
    }
}
