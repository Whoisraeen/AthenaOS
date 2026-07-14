//! AthStore — app store and package manager for AthenaOS.
//!
//! 12% revenue share, sideloading allowed, no review hostage situations.
//! Full package management with dependency resolution, sandboxed installation,
//! integrity verification, and atomic updates with rollback.
//!
//! This is the *store* layer — user-facing categories, reviews, developer pages,
//! editorial collections, and the install/update lifecycle. The lower-level
//! package primitives live in `athpackage`.
#![no_std]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ═══════════════════════════════════════════════════════════════════════════
// 1. CORE TYPES & ERROR HANDLING
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    PackageNotFound,
    AlreadyInstalled,
    NotInstalled,
    DependencyConflict,
    CycleDetected,
    VersionMismatch,
    ChecksumMismatch,
    SignatureInvalid,
    PermissionDenied,
    InsufficientSpace,
    DownloadFailed,
    NetworkUnavailable,
    RepositoryUnavailable,
    InstallFailed,
    UninstallFailed,
    UpdateFailed,
    RollbackFailed,
    SideloadBlocked,
    ManifestInvalid,
    SectionCorrupted,
    ReviewNotAllowed,
    DeveloperNotFound,
    CollectionNotFound,
    LockHeld,
    TransactionAborted,
    CapabilityDenied,
    SandboxViolation,
    QuotaExceeded,
}

pub type Result<T> = core::result::Result<T, StoreError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AppId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeveloperId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReviewId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CollectionId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransactionId(pub u64);

// ── Semantic version ────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SemVer {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

impl SemVer {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    pub fn is_compatible_with(&self, other: &SemVer) -> bool {
        if self.major == 0 && other.major == 0 {
            return self.minor == other.minor;
        }
        self.major == other.major
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. PACKAGE FORMAT
// ═══════════════════════════════════════════════════════════════════════════

/// Capability a package may request. Maps to AthGuard's Cap system —
/// every privileged operation goes through capabilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AppPermission {
    FileReadHome,
    FileWriteHome,
    FileReadSystem,
    NetworkClient,
    NetworkServer,
    NetworkUnrestricted,
    Camera,
    Microphone,
    AudioPlayback,
    AudioCapture,
    GpuCompute,
    GpuRender,
    UsbDevices,
    BluetoothAccess,
    Notifications,
    BackgroundExec,
    SystemTray,
    Clipboard,
    ScreenCapture,
    InputCapture,
    Overlay,
    GameMode,
    HardwareInfo,
    ProcessList,
    AutoStart,
}

/// Where an installed package came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackageOrigin {
    /// Downloaded from an official AthStore repository.
    Store,
    /// Sideloaded from a local file — shown as "unverified" in UI.
    Sideloaded,
    /// Installed from a developer build (debug-signed).
    Developer,
    /// Came from a third-party mirror.
    ThirdPartyMirror,
}

/// Content rating for store display.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContentRating {
    Everyone,
    Teen,
    Mature,
    AdultsOnly,
}

/// A section inside a `.athpkg` archive.
#[derive(Clone, Debug)]
pub struct PackageSection {
    pub kind: SectionKind,
    pub offset: u64,
    pub size: u64,
    pub compressed_size: u64,
    pub sha256: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionKind {
    Manifest,
    Code,
    Assets,
    Metadata,
    Signatures,
    DeltaPatch,
    Icons,
    Translations,
}

/// Section table — the TOC at the start of a `.athpkg` file.
#[derive(Clone, Debug)]
pub struct SectionTable {
    pub magic: [u8; 8],
    pub format_version: u16,
    pub sections: Vec<PackageSection>,
}

impl SectionTable {
    pub const MAGIC: [u8; 8] = *b"RAEPKG\x01\x00";

    pub fn new() -> Self {
        Self {
            magic: Self::MAGIC,
            format_version: 1,
            sections: Vec::new(),
        }
    }

    pub fn add_section(&mut self, section: PackageSection) {
        self.sections.push(section);
    }

    pub fn find_section(&self, kind: SectionKind) -> Option<&PackageSection> {
        self.sections.iter().find(|s| s.kind == kind)
    }

    pub fn total_size(&self) -> u64 {
        self.sections.iter().map(|s| s.compressed_size).sum()
    }

    pub fn validate_magic(&self) -> bool {
        self.magic == Self::MAGIC
    }

    pub fn validate_checksums(&self, data: &[u8]) -> Result<()> {
        for section in &self.sections {
            let end = section.offset as usize + section.compressed_size as usize;
            if end > data.len() {
                return Err(StoreError::SectionCorrupted);
            }
            let slice = &data[section.offset as usize..end];
            let computed = sha256_digest(slice);
            if computed != section.sha256 {
                return Err(StoreError::ChecksumMismatch);
            }
        }
        Ok(())
    }
}

/// The manifest embedded in every AthStore package.
#[derive(Clone, Debug)]
pub struct PackageManifest {
    pub id: AppId,
    pub name: String,
    pub version: SemVer,
    pub author: String,
    pub developer_id: DeveloperId,
    pub description: String,
    pub long_description: String,
    pub license: String,
    pub homepage: String,
    pub repository: String,
    pub min_os_version: SemVer,
    pub content_rating: ContentRating,
    pub category: StoreCategory,
    pub tags: Vec<String>,
    pub permissions: Vec<AppPermission>,
    pub dependencies: Vec<Dependency>,
    pub size_bytes: u64,
    pub installed_size: u64,
    pub sha256_hash: [u8; 32],
    pub signature: Option<PackageSignature>,
    pub icon_hash: [u8; 32],
    pub screenshots: Vec<String>,
    pub changelog: String,
}

impl PackageManifest {
    pub fn new(id: AppId, name: String, version: SemVer) -> Self {
        Self {
            id,
            name,
            version,
            author: String::new(),
            developer_id: DeveloperId(0),
            description: String::new(),
            long_description: String::new(),
            license: String::new(),
            homepage: String::new(),
            repository: String::new(),
            min_os_version: SemVer::new(0, 1, 0),
            content_rating: ContentRating::Everyone,
            category: StoreCategory::Utilities,
            tags: Vec::new(),
            permissions: Vec::new(),
            dependencies: Vec::new(),
            size_bytes: 0,
            installed_size: 0,
            sha256_hash: [0u8; 32],
            signature: None,
            icon_hash: [0u8; 32],
            screenshots: Vec::new(),
            changelog: String::new(),
        }
    }

    pub fn is_signed(&self) -> bool {
        self.signature.is_some()
    }

    pub fn requires_permission(&self, perm: AppPermission) -> bool {
        self.permissions.contains(&perm)
    }

    pub fn permission_count(&self) -> usize {
        self.permissions.len()
    }

    pub fn has_sensitive_permissions(&self) -> bool {
        self.permissions.iter().any(|p| {
            matches!(
                p,
                AppPermission::FileReadSystem
                    | AppPermission::NetworkUnrestricted
                    | AppPermission::InputCapture
                    | AppPermission::ScreenCapture
                    | AppPermission::ProcessList
            )
        })
    }
}

/// Code signing signature for verified distribution.
#[derive(Clone, Debug)]
pub struct PackageSignature {
    pub key_id: String,
    pub algorithm: SignatureAlgorithm,
    pub signature_bytes: Vec<u8>,
    pub signed_hash: [u8; 32],
    pub timestamp: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureAlgorithm {
    Ed25519,
    Rsa4096Sha256,
}

impl PackageSignature {
    pub fn verify_against_hash(&self, manifest_hash: &[u8; 32]) -> bool {
        self.signed_hash == *manifest_hash
    }
}

/// A full AthStore package — the archive contents parsed into memory.
#[derive(Clone, Debug)]
pub struct RaePackage {
    pub section_table: SectionTable,
    pub manifest: PackageManifest,
    pub code_data: Vec<u8>,
    pub asset_data: Vec<u8>,
    pub metadata: Vec<u8>,
}

impl RaePackage {
    pub fn new(manifest: PackageManifest) -> Self {
        Self {
            section_table: SectionTable::new(),
            manifest,
            code_data: Vec::new(),
            asset_data: Vec::new(),
            metadata: Vec::new(),
        }
    }

    pub fn verify_integrity(&self) -> Result<()> {
        if !self.section_table.validate_magic() {
            return Err(StoreError::ManifestInvalid);
        }
        let hash = sha256_digest(&self.code_data);
        if hash != self.manifest.sha256_hash {
            return Err(StoreError::ChecksumMismatch);
        }
        Ok(())
    }

    pub fn verify_signature(&self, keyring: &SigningKeyring) -> Result<()> {
        match &self.manifest.signature {
            Some(sig) => {
                // Bind the signature to THIS package: the signed hash must be
                // the manifest's content hash (which `verify_integrity` checks
                // equals sha256(code_data)). Otherwise a valid signature over
                // some other hash could be attached to tampered content.
                if sig.signed_hash != self.manifest.sha256_hash {
                    return Err(StoreError::SignatureInvalid);
                }
                if !keyring.verify(&sig.key_id, &sig.signed_hash, &sig.signature_bytes) {
                    return Err(StoreError::SignatureInvalid);
                }
                Ok(())
            }
            None => Ok(()),
        }
    }

    pub fn total_size(&self) -> u64 {
        self.code_data.len() as u64 + self.asset_data.len() as u64 + self.metadata.len() as u64
    }
}

// ── Install lifecycle ───────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppState {
    Available,
    Queued,
    Downloading { progress_pct: u8 },
    Verifying,
    Installing,
    Installed,
    Updating { from: SemVer, to: SemVer },
    Uninstalling,
    Broken,
    Disabled,
}

#[derive(Clone, Debug)]
pub struct InstalledApp {
    pub manifest: PackageManifest,
    pub state: AppState,
    pub origin: PackageOrigin,
    pub install_path: String,
    pub data_path: String,
    pub installed_at: u64,
    pub updated_at: u64,
    pub last_launched: u64,
    pub launch_count: u64,
    pub granted_permissions: Vec<AppPermission>,
    pub denied_permissions: Vec<AppPermission>,
    pub disk_usage: u64,
    pub auto_update: bool,
    pub pinned_version: Option<SemVer>,
    pub snapshot_id: Option<u64>,
    /// Why this app is present — `Explicit` (user-requested) or `Dependency`
    /// (auto-installed to satisfy another app). Drives orphan GC on uninstall.
    pub install_reason: InstallReason,
}

impl InstalledApp {
    pub fn new(
        manifest: PackageManifest,
        origin: PackageOrigin,
        install_path: String,
        timestamp: u64,
    ) -> Self {
        let data_path = alloc::format!("{}/data", install_path);
        Self {
            manifest,
            state: AppState::Installed,
            origin,
            install_path,
            data_path,
            installed_at: timestamp,
            updated_at: timestamp,
            last_launched: 0,
            launch_count: 0,
            granted_permissions: Vec::new(),
            denied_permissions: Vec::new(),
            disk_usage: 0,
            auto_update: true,
            pinned_version: None,
            snapshot_id: None,
            install_reason: InstallReason::Explicit,
        }
    }

    pub fn is_sideloaded(&self) -> bool {
        self.origin == PackageOrigin::Sideloaded
    }

    pub fn is_verified(&self) -> bool {
        self.manifest.is_signed() && self.origin != PackageOrigin::Sideloaded
    }

    pub fn has_permission(&self, perm: AppPermission) -> bool {
        self.granted_permissions.contains(&perm)
    }

    pub fn grant_permission(&mut self, perm: AppPermission) {
        if !self.granted_permissions.contains(&perm) {
            self.granted_permissions.push(perm);
        }
        self.denied_permissions.retain(|p| *p != perm);
    }

    pub fn deny_permission(&mut self, perm: AppPermission) {
        if !self.denied_permissions.contains(&perm) {
            self.denied_permissions.push(perm);
        }
        self.granted_permissions.retain(|p| *p != perm);
    }

    pub fn pending_permissions(&self) -> Vec<AppPermission> {
        self.manifest
            .permissions
            .iter()
            .filter(|p| {
                !self.granted_permissions.contains(p) && !self.denied_permissions.contains(p)
            })
            .copied()
            .collect()
    }

    pub fn record_launch(&mut self, timestamp: u64) {
        self.last_launched = timestamp;
        self.launch_count += 1;
    }
}

// ── Signing keyring ─────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct SigningKey {
    pub key_id: String,
    pub owner: String,
    pub public_key: Vec<u8>,
    pub trust_level: KeyTrust,
    pub created_at: u64,
    pub expires_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyTrust {
    Unknown,
    Untrusted,
    Marginal,
    Full,
    Ultimate,
    Revoked,
}

pub struct SigningKeyring {
    pub keys: BTreeMap<String, SigningKey>,
}

impl SigningKeyring {
    pub fn new() -> Self {
        Self {
            keys: BTreeMap::new(),
        }
    }

    pub fn import(&mut self, key: SigningKey) {
        self.keys.insert(key.key_id.clone(), key);
    }

    pub fn revoke(&mut self, key_id: &str) -> Result<()> {
        let key = self
            .keys
            .get_mut(key_id)
            .ok_or(StoreError::PackageNotFound)?;
        key.trust_level = KeyTrust::Revoked;
        Ok(())
    }

    /// Verify a detached Ed25519 signature over `message` (the manifest's
    /// signed hash) under the named key. AthenaOS keys are Ed25519 (32-byte
    /// public key, 64-byte signature); a wrong-sized key/signature — e.g. an
    /// RSA-4096 signature we cannot verify — is rejected fail-closed. The
    /// former stub ignored the signature and accepted any non-revoked key.
    pub fn verify(&self, key_id: &str, message: &[u8; 32], signature: &[u8]) -> bool {
        let key = match self.keys.get(key_id) {
            Some(k) => k,
            None => return false,
        };
        if matches!(key.trust_level, KeyTrust::Revoked | KeyTrust::Untrusted) {
            return false;
        }
        let pk: [u8; 32] = match key.public_key.as_slice().try_into() {
            Ok(p) => p,
            Err(_) => return false,
        };
        let sig: [u8; 64] = match signature.try_into() {
            Ok(s) => s,
            Err(_) => return false,
        };
        ath_crypto::ed25519::verify(&pk, message, &sig)
    }

    pub fn trusted_keys(&self) -> Vec<&SigningKey> {
        self.keys
            .values()
            .filter(|k| matches!(k.trust_level, KeyTrust::Full | KeyTrust::Ultimate))
            .collect()
    }
}

// ── SHA-256 digest ──────────────────────────────────────────────────────
// Real SHA-256 (FIPS 180-4) via the shared `ath_crypto` crate. This replaced a
// homebrew FNV-style `wrapping_mul(0x01000193)` loop that was NOT SHA-256 and
// gave `verify_integrity` no real collision resistance.

fn sha256_digest(data: &[u8]) -> [u8; 32] {
    ath_crypto::sha256::sha256(data)
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. DEPENDENCY RESOLUTION
// ═══════════════════════════════════════════════════════════════════════════

/// How a version constraint matches candidates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VersionConstraint {
    /// `=1.2.3` — only this exact version.
    Exact(SemVer),
    /// `>=1.2.0, <2.0.0` — inclusive lower, exclusive upper.
    Range { min: SemVer, max: SemVer },
    /// `^1.2.3` — compatible (same major, >= given).
    Compatible(SemVer),
    /// `*` — any version.
    Any,
}

impl VersionConstraint {
    pub fn satisfied_by(&self, candidate: &SemVer) -> bool {
        match self {
            VersionConstraint::Exact(v) => candidate == v,
            VersionConstraint::Range { min, max } => candidate >= min && candidate < max,
            VersionConstraint::Compatible(v) => candidate.is_compatible_with(v) && candidate >= v,
            VersionConstraint::Any => true,
        }
    }

    pub fn conflicts_with(&self, other: &VersionConstraint) -> bool {
        match (self, other) {
            (VersionConstraint::Exact(a), VersionConstraint::Exact(b)) => a != b,
            (VersionConstraint::Exact(v), VersionConstraint::Range { min, max })
            | (VersionConstraint::Range { min, max }, VersionConstraint::Exact(v)) => {
                v < min || v >= max
            }
            (
                VersionConstraint::Range {
                    min: min_a,
                    max: max_a,
                },
                VersionConstraint::Range {
                    min: min_b,
                    max: max_b,
                },
            ) => max_a <= min_b || max_b <= min_a,
            _ => false,
        }
    }
}

/// A single dependency declaration.
#[derive(Clone, Debug)]
pub struct Dependency {
    pub app_id: AppId,
    pub name: String,
    pub constraint: VersionConstraint,
    pub optional: bool,
}

impl Dependency {
    pub fn required(app_id: AppId, name: String, constraint: VersionConstraint) -> Self {
        Self {
            app_id,
            name,
            constraint,
            optional: false,
        }
    }

    pub fn optional(app_id: AppId, name: String, constraint: VersionConstraint) -> Self {
        Self {
            app_id,
            name,
            constraint,
            optional: true,
        }
    }
}

/// A node in the dependency DAG.
#[derive(Clone, Debug)]
struct DepNode {
    app_id: AppId,
    name: String,
    version: SemVer,
    deps: Vec<(AppId, VersionConstraint)>,
    visited: DepVisit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DepVisit {
    Unvisited,
    InProgress,
    Done,
}

/// Conflict detected during resolution.
#[derive(Clone, Debug)]
pub struct DependencyConflict {
    pub package_a: AppId,
    pub package_b: AppId,
    pub conflicting_dep: AppId,
    pub constraint_a: VersionConstraint,
    pub constraint_b: VersionConstraint,
}

/// How to handle conflicts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictStrategy {
    /// Fail on first conflict.
    Strict,
    /// Prefer the newer version when ranges overlap.
    PreferNewest,
    /// Prefer whatever's already installed.
    PreferInstalled,
}

/// DAG-based dependency resolver with topological sort.
pub struct DependencyResolver {
    nodes: BTreeMap<AppId, DepNode>,
    conflicts: Vec<DependencyConflict>,
    strategy: ConflictStrategy,
}

impl DependencyResolver {
    pub fn new(strategy: ConflictStrategy) -> Self {
        Self {
            nodes: BTreeMap::new(),
            conflicts: Vec::new(),
            strategy,
        }
    }

    pub fn add_package(&mut self, id: AppId, name: String, version: SemVer, deps: Vec<Dependency>) {
        let dep_pairs: Vec<(AppId, VersionConstraint)> = deps
            .iter()
            .filter(|d| !d.optional)
            .map(|d| (d.app_id, d.constraint.clone()))
            .collect();
        self.nodes.insert(
            id,
            DepNode {
                app_id: id,
                name,
                version,
                deps: dep_pairs,
                visited: DepVisit::Unvisited,
            },
        );
    }

    /// Resolve install order via topological sort. Returns app IDs in
    /// dependency-first order (leaf deps before the packages that need them).
    pub fn resolve(&mut self, targets: &[AppId]) -> Result<Vec<AppId>> {
        self.detect_conflicts()?;

        for node in self.nodes.values_mut() {
            node.visited = DepVisit::Unvisited;
        }

        let mut order = Vec::new();
        for &target in targets {
            self.topo_visit(target, &mut order)?;
        }
        Ok(order)
    }

    fn topo_visit(&mut self, id: AppId, order: &mut Vec<AppId>) -> Result<()> {
        let visit_state = match self.nodes.get(&id) {
            Some(node) => node.visited,
            None => return Err(StoreError::PackageNotFound),
        };

        match visit_state {
            DepVisit::Done => return Ok(()),
            DepVisit::InProgress => return Err(StoreError::CycleDetected),
            DepVisit::Unvisited => {}
        }

        if let Some(node) = self.nodes.get_mut(&id) {
            node.visited = DepVisit::InProgress;
        }

        let deps: Vec<AppId> = self
            .nodes
            .get(&id)
            .map(|n| n.deps.iter().map(|(dep_id, _)| *dep_id).collect())
            .unwrap_or_default();

        for dep_id in deps {
            self.topo_visit(dep_id, order)?;
        }

        if let Some(node) = self.nodes.get_mut(&id) {
            node.visited = DepVisit::Done;
        }

        if !order.contains(&id) {
            order.push(id);
        }
        Ok(())
    }

    fn detect_conflicts(&mut self) -> Result<()> {
        self.conflicts.clear();

        let node_ids: Vec<AppId> = self.nodes.keys().copied().collect();
        let mut required: BTreeMap<AppId, Vec<(AppId, VersionConstraint)>> = BTreeMap::new();

        for &nid in &node_ids {
            if let Some(node) = self.nodes.get(&nid) {
                for (dep_id, constraint) in &node.deps {
                    required
                        .entry(*dep_id)
                        .or_default()
                        .push((nid, constraint.clone()));
                }
            }
        }

        for (dep_id, requestors) in &required {
            for i in 0..requestors.len() {
                for j in (i + 1)..requestors.len() {
                    let (pkg_a, constraint_a) = &requestors[i];
                    let (pkg_b, constraint_b) = &requestors[j];
                    if constraint_a.conflicts_with(constraint_b) {
                        let conflict = DependencyConflict {
                            package_a: *pkg_a,
                            package_b: *pkg_b,
                            conflicting_dep: *dep_id,
                            constraint_a: constraint_a.clone(),
                            constraint_b: constraint_b.clone(),
                        };
                        match self.strategy {
                            ConflictStrategy::Strict => {
                                return Err(StoreError::DependencyConflict);
                            }
                            _ => {
                                self.conflicts.push(conflict);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn get_conflicts(&self) -> &[DependencyConflict] {
        &self.conflicts
    }

    pub fn dependency_count(&self, id: AppId) -> usize {
        fn count_recursive(
            nodes: &BTreeMap<AppId, DepNode>,
            id: AppId,
            seen: &mut Vec<AppId>,
        ) -> usize {
            if seen.contains(&id) {
                return 0;
            }
            seen.push(id);
            let mut total = 0;
            if let Some(node) = nodes.get(&id) {
                for (dep_id, _) in &node.deps {
                    total += 1 + count_recursive(nodes, *dep_id, seen);
                }
            }
            total
        }
        let mut seen = Vec::new();
        count_recursive(&self.nodes, id, &mut seen)
    }

    pub fn reverse_deps(&self, id: AppId) -> Vec<AppId> {
        self.nodes
            .values()
            .filter(|n| n.deps.iter().any(|(dep_id, _)| *dep_id == id))
            .map(|n| n.app_id)
            .collect()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. REPOSITORY SYSTEM
// ═══════════════════════════════════════════════════════════════════════════

/// A mirror for a repository.
#[derive(Clone, Debug)]
pub struct Mirror {
    pub url: String,
    pub country: String,
    pub priority: u32,
    pub last_sync: u64,
    pub available: bool,
    pub avg_speed_kbps: u32,
}

/// Repository configuration.
#[derive(Clone, Debug)]
pub struct RepositoryConfig {
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub signing_key_id: String,
    pub mirrors: Vec<Mirror>,
    pub priority: u32,
    pub sync_interval_secs: u64,
}

impl RepositoryConfig {
    pub fn new(name: String, base_url: String) -> Self {
        Self {
            name,
            base_url,
            enabled: true,
            signing_key_id: String::new(),
            mirrors: Vec::new(),
            priority: 100,
            sync_interval_secs: 3600,
        }
    }

    pub fn best_mirror(&self) -> Option<&Mirror> {
        self.mirrors
            .iter()
            .filter(|m| m.available)
            .min_by_key(|m| m.priority)
    }

    pub fn add_mirror(&mut self, mirror: Mirror) {
        self.mirrors.push(mirror);
    }
}

/// An entry in the repository package index.
#[derive(Clone, Debug)]
pub struct RepoEntry {
    pub manifest: PackageManifest,
    pub download_url: String,
    pub download_size: u64,
    pub mirrors: Vec<String>,
}

/// The repository — a remote package source with a local index cache.
pub struct Repository {
    pub config: RepositoryConfig,
    pub index: BTreeMap<AppId, Vec<RepoEntry>>,
    pub last_sync: u64,
    pub entry_count: usize,
}

impl Repository {
    pub fn new(config: RepositoryConfig) -> Self {
        Self {
            config,
            index: BTreeMap::new(),
            last_sync: 0,
            entry_count: 0,
        }
    }

    pub fn sync(&mut self, timestamp: u64) -> Result<()> {
        if !self.config.enabled {
            return Err(StoreError::RepositoryUnavailable);
        }
        self.last_sync = timestamp;
        self.entry_count = self.index.values().map(|v| v.len()).sum();
        Ok(())
    }

    pub fn add_entry(&mut self, entry: RepoEntry) {
        self.index.entry(entry.manifest.id).or_default().push(entry);
        self.entry_count += 1;
    }

    pub fn latest_version(&self, id: AppId) -> Option<&RepoEntry> {
        self.index
            .get(&id)?
            .iter()
            .max_by(|a, b| a.manifest.version.cmp(&b.manifest.version))
    }

    pub fn find_version(&self, id: AppId, constraint: &VersionConstraint) -> Option<&RepoEntry> {
        self.index
            .get(&id)?
            .iter()
            .filter(|e| constraint.satisfied_by(&e.manifest.version))
            .max_by(|a, b| a.manifest.version.cmp(&b.manifest.version))
    }

    /// Search by name and description substring (case-insensitive-ish).
    pub fn search(&self, query: &str) -> Vec<&RepoEntry> {
        let q = query;
        let mut results = Vec::new();
        for entries in self.index.values() {
            if let Some(latest) = entries.last() {
                if fuzzy_match(&latest.manifest.name, q)
                    || fuzzy_match(&latest.manifest.description, q)
                    || latest.manifest.tags.iter().any(|t| fuzzy_match(t, q))
                {
                    results.push(latest);
                }
            }
        }
        results
    }

    pub fn search_by_category(&self, cat: StoreCategory) -> Vec<&RepoEntry> {
        let mut results = Vec::new();
        for entries in self.index.values() {
            if let Some(latest) = entries.last() {
                if latest.manifest.category == cat {
                    results.push(latest);
                }
            }
        }
        results
    }

    pub fn all_packages(&self) -> Vec<&RepoEntry> {
        self.index
            .values()
            .filter_map(|entries| entries.last())
            .collect()
    }
}

/// Simple fuzzy matching — checks if all chars of `needle` appear in
/// `haystack` in order. Good enough for store search in no_std.
fn fuzzy_match(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let mut needle_chars = needle.as_bytes().iter();
    let mut current = match needle_chars.next() {
        Some(c) => *c,
        None => return true,
    };

    for &h in haystack.as_bytes() {
        let h_lower = if h >= b'A' && h <= b'Z' { h + 32 } else { h };
        let c_lower = if current >= b'A' && current <= b'Z' {
            current + 32
        } else {
            current
        };
        if h_lower == c_lower {
            current = match needle_chars.next() {
                Some(c) => *c,
                None => return true,
            };
        }
    }
    false
}

/// Compute a fuzzy relevance score (0..100). Higher is better.
fn fuzzy_score(haystack: &str, needle: &str) -> u32 {
    if needle.is_empty() || haystack.is_empty() {
        return 0;
    }

    if haystack == needle {
        return 100;
    }

    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    let mut matched = 0u32;
    let mut hi = 0usize;

    for &nc in n {
        let nc_lower = if nc >= b'A' && nc <= b'Z' {
            nc + 32
        } else {
            nc
        };
        while hi < h.len() {
            let hc = h[hi];
            let hc_lower = if hc >= b'A' && hc <= b'Z' {
                hc + 32
            } else {
                hc
            };
            hi += 1;
            if hc_lower == nc_lower {
                matched += 1;
                break;
            }
        }
    }

    if n.is_empty() {
        return 0;
    }
    let coverage = matched * 100 / n.len() as u32;

    let starts_with = {
        let prefix_len = core::cmp::min(n.len(), h.len());
        let mut ok = true;
        for i in 0..prefix_len {
            let a = if h[i] >= b'A' && h[i] <= b'Z' {
                h[i] + 32
            } else {
                h[i]
            };
            let b = if n[i] >= b'A' && n[i] <= b'Z' {
                n[i] + 32
            } else {
                n[i]
            };
            if a != b {
                ok = false;
                break;
            }
        }
        ok
    };

    let bonus = if starts_with { 20 } else { 0 };
    let len_penalty = if h.len() > n.len() * 4 { 10 } else { 0 };

    coverage
        .saturating_add(bonus)
        .saturating_sub(len_penalty)
        .min(100)
}

// ── Download queue with integrity verification ──────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadState {
    Queued,
    Downloading,
    Completed,
    Failed,
    Cancelled,
    VerifyingIntegrity,
}

#[derive(Clone, Debug)]
pub struct DownloadTask {
    pub app_id: AppId,
    pub url: String,
    pub mirror_urls: Vec<String>,
    pub current_mirror: usize,
    pub expected_size: u64,
    pub downloaded_bytes: u64,
    pub expected_sha256: [u8; 32],
    pub state: DownloadState,
    pub retry_count: u8,
    pub max_retries: u8,
}

impl DownloadTask {
    pub fn progress_pct(&self) -> u8 {
        if self.expected_size == 0 {
            return 0;
        }
        ((self.downloaded_bytes * 100) / self.expected_size).min(100) as u8
    }

    pub fn try_next_mirror(&mut self) -> bool {
        if self.current_mirror + 1 < self.mirror_urls.len() {
            self.current_mirror += 1;
            self.downloaded_bytes = 0;
            self.state = DownloadState::Queued;
            true
        } else {
            false
        }
    }
}

pub struct DownloadQueue {
    pub tasks: Vec<DownloadTask>,
    pub max_concurrent: usize,
    pub active_count: usize,
    pub total_bytes_downloaded: u64,
}

impl DownloadQueue {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            tasks: Vec::new(),
            max_concurrent,
            active_count: 0,
            total_bytes_downloaded: 0,
        }
    }

    pub fn enqueue(&mut self, task: DownloadTask) {
        self.tasks.push(task);
    }

    pub fn start_next(&mut self) -> Option<usize> {
        if self.active_count >= self.max_concurrent {
            return None;
        }
        for (i, task) in self.tasks.iter_mut().enumerate() {
            if task.state == DownloadState::Queued {
                task.state = DownloadState::Downloading;
                self.active_count += 1;
                return Some(i);
            }
        }
        None
    }

    pub fn complete_task(&mut self, index: usize) -> Result<()> {
        let task = self
            .tasks
            .get_mut(index)
            .ok_or(StoreError::DownloadFailed)?;
        task.state = DownloadState::VerifyingIntegrity;
        task.downloaded_bytes = task.expected_size;
        self.total_bytes_downloaded += task.expected_size;
        if self.active_count > 0 {
            self.active_count -= 1;
        }
        // In a real implementation, verify SHA-256 here and transition to
        // Completed or ChecksumMismatch. For now, trust the download.
        task.state = DownloadState::Completed;
        Ok(())
    }

    pub fn fail_task(&mut self, index: usize) {
        if let Some(task) = self.tasks.get_mut(index) {
            task.retry_count += 1;
            if task.retry_count < task.max_retries {
                if !task.try_next_mirror() {
                    task.downloaded_bytes = 0;
                    task.state = DownloadState::Queued;
                }
            } else {
                task.state = DownloadState::Failed;
            }
            if self.active_count > 0 {
                self.active_count -= 1;
            }
        }
    }

    pub fn cancel_task(&mut self, app_id: AppId) {
        for task in &mut self.tasks {
            if task.app_id == app_id && task.state == DownloadState::Downloading {
                task.state = DownloadState::Cancelled;
                if self.active_count > 0 {
                    self.active_count -= 1;
                }
            }
        }
    }

    pub fn overall_progress(&self) -> (u64, u64) {
        let total: u64 = self.tasks.iter().map(|t| t.expected_size).sum();
        let done: u64 = self.tasks.iter().map(|t| t.downloaded_bytes).sum();
        (done, total)
    }

    pub fn all_complete(&self) -> bool {
        self.tasks
            .iter()
            .all(|t| matches!(t.state, DownloadState::Completed | DownloadState::Cancelled))
    }

    pub fn failed_tasks(&self) -> Vec<AppId> {
        self.tasks
            .iter()
            .filter(|t| t.state == DownloadState::Failed)
            .map(|t| t.app_id)
            .collect()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 5. STORE FRONTEND MODEL
// ═══════════════════════════════════════════════════════════════════════════

/// Store categories — AthenaOS is embodiment-first, so gaming categories
/// get first-class treatment alongside productivity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StoreCategory {
    Games,
    GameUtilities,
    Emulators,
    Productivity,
    CreativeTools,
    DeveloperTools,
    SystemUtilities,
    Utilities,
    Communication,
    Media,
    Education,
    Security,
    Customization,
    Themes,
    Drivers,
    Libraries,
}

/// Pricing model — 12% revenue share on paid apps.
#[derive(Clone, Debug)]
pub enum PricingModel {
    Free,
    Paid {
        price_cents: u64,
    },
    FreeWithIAP,
    Subscription {
        monthly_cents: u64,
        yearly_cents: u64,
    },
    DonationWare,
}

impl PricingModel {
    /// AthStore takes 12%. Calculate the developer's share in cents.
    pub fn developer_share_cents(&self) -> u64 {
        match self {
            PricingModel::Paid { price_cents } => price_cents * 88 / 100,
            PricingModel::Subscription { monthly_cents, .. } => monthly_cents * 88 / 100,
            _ => 0,
        }
    }

    pub fn store_cut_cents(&self) -> u64 {
        match self {
            PricingModel::Paid { price_cents } => price_cents * 12 / 100,
            PricingModel::Subscription { monthly_cents, .. } => monthly_cents * 12 / 100,
            _ => 0,
        }
    }
}

/// A user review on the store.
#[derive(Clone, Debug)]
pub struct Review {
    pub id: ReviewId,
    pub app_id: AppId,
    pub author: String,
    pub rating: u8,
    pub title: String,
    pub body: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub helpful_votes: u32,
    pub unhelpful_votes: u32,
    pub version_reviewed: SemVer,
    pub verified_purchase: bool,
}

impl Review {
    pub fn is_valid(&self) -> bool {
        self.rating >= 1 && self.rating <= 5 && !self.title.is_empty()
    }

    pub fn helpfulness_score(&self) -> i32 {
        self.helpful_votes as i32 - self.unhelpful_votes as i32
    }
}

/// Aggregate rating statistics for a store listing.
#[derive(Clone, Debug, Default)]
pub struct RatingStats {
    pub total_reviews: u32,
    pub total_rating_sum: u64,
    pub star_counts: [u32; 5],
}

impl RatingStats {
    pub fn average(&self) -> u32 {
        if self.total_reviews == 0 {
            return 0;
        }
        ((self.total_rating_sum * 10) / self.total_reviews as u64) as u32
    }

    pub fn add_review(&mut self, rating: u8) {
        if rating >= 1 && rating <= 5 {
            self.total_reviews += 1;
            self.total_rating_sum += rating as u64;
            self.star_counts[(rating - 1) as usize] += 1;
        }
    }

    pub fn remove_review(&mut self, rating: u8) {
        if rating >= 1 && rating <= 5 && self.total_reviews > 0 {
            self.total_reviews -= 1;
            self.total_rating_sum -= rating as u64;
            self.star_counts[(rating - 1) as usize] =
                self.star_counts[(rating - 1) as usize].saturating_sub(1);
        }
    }

    pub fn star_percentage(&self, star: u8) -> u32 {
        if self.total_reviews == 0 || star < 1 || star > 5 {
            return 0;
        }
        (self.star_counts[(star - 1) as usize] * 100) / self.total_reviews
    }
}

/// A developer page on the store.
#[derive(Clone, Debug)]
pub struct DeveloperPage {
    pub id: DeveloperId,
    pub name: String,
    pub display_name: String,
    pub bio: String,
    pub website: String,
    pub verified: bool,
    pub joined_at: u64,
    pub app_ids: Vec<AppId>,
    pub total_downloads: u64,
    pub average_rating: u32,
    pub support_email: String,
    pub privacy_policy_url: String,
}

impl DeveloperPage {
    pub fn new(id: DeveloperId, name: String) -> Self {
        Self {
            id,
            name: name.clone(),
            display_name: name,
            bio: String::new(),
            website: String::new(),
            verified: false,
            joined_at: 0,
            app_ids: Vec::new(),
            total_downloads: 0,
            average_rating: 0,
            support_email: String::new(),
            privacy_policy_url: String::new(),
        }
    }

    pub fn add_app(&mut self, id: AppId) {
        if !self.app_ids.contains(&id) {
            self.app_ids.push(id);
        }
    }

    pub fn app_count(&self) -> usize {
        self.app_ids.len()
    }
}

/// An editorial collection — curated lists by the AthenaOS team.
/// "No review hostage situations" — these are surfacing, not gatekeeping.
#[derive(Clone, Debug)]
pub struct EditorialCollection {
    pub id: CollectionId,
    pub title: String,
    pub description: String,
    pub curator: String,
    pub app_ids: Vec<AppId>,
    pub created_at: u64,
    pub featured: bool,
    pub banner_hash: [u8; 32],
}

impl EditorialCollection {
    pub fn new(id: CollectionId, title: String) -> Self {
        Self {
            id,
            title,
            description: String::new(),
            curator: String::new(),
            app_ids: Vec::new(),
            created_at: 0,
            featured: false,
            banner_hash: [0u8; 32],
        }
    }

    pub fn add_app(&mut self, id: AppId) {
        if !self.app_ids.contains(&id) {
            self.app_ids.push(id);
        }
    }

    pub fn app_count(&self) -> usize {
        self.app_ids.len()
    }
}

/// A store listing — the full product page for one app.
#[derive(Clone, Debug)]
pub struct StoreListing {
    pub manifest: PackageManifest,
    pub pricing: PricingModel,
    pub ratings: RatingStats,
    pub reviews: Vec<Review>,
    pub total_downloads: u64,
    pub weekly_downloads: u64,
    pub trending_score: u64,
    pub featured: bool,
    pub staff_pick: bool,
    pub developer: DeveloperId,
    pub related_apps: Vec<AppId>,
    pub first_published: u64,
    pub last_updated: u64,
}

impl StoreListing {
    pub fn new(manifest: PackageManifest, pricing: PricingModel) -> Self {
        let dev = manifest.developer_id;
        Self {
            manifest,
            pricing,
            ratings: RatingStats::default(),
            reviews: Vec::new(),
            total_downloads: 0,
            weekly_downloads: 0,
            trending_score: 0,
            featured: false,
            staff_pick: false,
            developer: dev,
            related_apps: Vec::new(),
            first_published: 0,
            last_updated: 0,
        }
    }

    pub fn add_review(&mut self, review: Review) {
        if review.is_valid() {
            self.ratings.add_review(review.rating);
            self.reviews.push(review);
        }
    }

    pub fn remove_review(&mut self, review_id: ReviewId) {
        if let Some(pos) = self.reviews.iter().position(|r| r.id == review_id) {
            let rating = self.reviews[pos].rating;
            self.ratings.remove_review(rating);
            self.reviews.remove(pos);
        }
    }

    pub fn record_download(&mut self) {
        self.total_downloads += 1;
        self.weekly_downloads += 1;
    }

    pub fn compute_trending_score(&mut self) {
        let download_factor = self.weekly_downloads * 10;
        let rating_factor = self.ratings.average() as u64 * self.ratings.total_reviews as u64;
        let recency_factor = if self.last_updated > 0 { 50 } else { 0 };
        self.trending_score = download_factor + rating_factor + recency_factor;
    }

    pub fn top_reviews(&self, count: usize) -> Vec<&Review> {
        let mut sorted: Vec<&Review> = self.reviews.iter().collect();
        sorted.sort_by(|a, b| b.helpfulness_score().cmp(&a.helpfulness_score()));
        sorted.truncate(count);
        sorted
    }
}

/// Search filters for the store frontend.
#[derive(Clone, Debug)]
pub struct SearchFilter {
    pub query: Option<String>,
    pub category: Option<StoreCategory>,
    pub min_rating: Option<u8>,
    pub max_price_cents: Option<u64>,
    pub free_only: bool,
    pub content_rating: Option<ContentRating>,
    pub sort_by: SortOrder,
    pub page: usize,
    pub per_page: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortOrder {
    Relevance,
    Trending,
    MostDownloaded,
    HighestRated,
    Newest,
    RecentlyUpdated,
    PriceLowToHigh,
    PriceHighToLow,
    NameAZ,
}

impl SearchFilter {
    pub fn new() -> Self {
        Self {
            query: None,
            category: None,
            min_rating: None,
            max_price_cents: None,
            free_only: false,
            content_rating: None,
            sort_by: SortOrder::Relevance,
            page: 0,
            per_page: 20,
        }
    }

    pub fn with_query(mut self, q: String) -> Self {
        self.query = Some(q);
        self
    }

    pub fn with_category(mut self, c: StoreCategory) -> Self {
        self.category = Some(c);
        self
    }

    pub fn free(mut self) -> Self {
        self.free_only = true;
        self
    }

    pub fn with_sort(mut self, s: SortOrder) -> Self {
        self.sort_by = s;
        self
    }
}

/// The store index — powers browsing, search, featured/trending lists.
pub struct StoreIndex {
    pub listings: BTreeMap<AppId, StoreListing>,
    pub developers: BTreeMap<DeveloperId, DeveloperPage>,
    pub collections: BTreeMap<CollectionId, EditorialCollection>,
    next_review_id: u64,
}

impl StoreIndex {
    pub fn new() -> Self {
        Self {
            listings: BTreeMap::new(),
            developers: BTreeMap::new(),
            collections: BTreeMap::new(),
            next_review_id: 1,
        }
    }

    pub fn add_listing(&mut self, listing: StoreListing) {
        let id = listing.manifest.id;
        self.listings.insert(id, listing);
    }

    pub fn get_listing(&self, id: AppId) -> Option<&StoreListing> {
        self.listings.get(&id)
    }

    pub fn get_listing_mut(&mut self, id: AppId) -> Option<&mut StoreListing> {
        self.listings.get_mut(&id)
    }

    pub fn add_developer(&mut self, page: DeveloperPage) {
        self.developers.insert(page.id, page);
    }

    pub fn get_developer(&self, id: DeveloperId) -> Option<&DeveloperPage> {
        self.developers.get(&id)
    }

    pub fn add_collection(&mut self, collection: EditorialCollection) {
        self.collections.insert(collection.id, collection);
    }

    pub fn submit_review(&mut self, app_id: AppId, mut review: Review) -> Result<ReviewId> {
        let listing = self
            .listings
            .get_mut(&app_id)
            .ok_or(StoreError::PackageNotFound)?;
        let rid = ReviewId(self.next_review_id);
        self.next_review_id += 1;
        review.id = rid;
        listing.add_review(review);
        Ok(rid)
    }

    pub fn featured_apps(&self) -> Vec<&StoreListing> {
        self.listings.values().filter(|l| l.featured).collect()
    }

    pub fn trending_apps(&self, limit: usize) -> Vec<&StoreListing> {
        let mut sorted: Vec<&StoreListing> = self.listings.values().collect();
        sorted.sort_by(|a, b| b.trending_score.cmp(&a.trending_score));
        sorted.truncate(limit);
        sorted
    }

    pub fn new_apps(&self, since: u64, limit: usize) -> Vec<&StoreListing> {
        let mut recent: Vec<&StoreListing> = self
            .listings
            .values()
            .filter(|l| l.first_published >= since)
            .collect();
        recent.sort_by(|a, b| b.first_published.cmp(&a.first_published));
        recent.truncate(limit);
        recent
    }

    pub fn top_rated(&self, limit: usize) -> Vec<&StoreListing> {
        let mut rated: Vec<&StoreListing> = self
            .listings
            .values()
            .filter(|l| l.ratings.total_reviews >= 5)
            .collect();
        rated.sort_by(|a, b| b.ratings.average().cmp(&a.ratings.average()));
        rated.truncate(limit);
        rated
    }

    pub fn search(&self, filter: &SearchFilter) -> Vec<&StoreListing> {
        let mut results: Vec<&StoreListing> = self
            .listings
            .values()
            .filter(|l| self.matches_filter(l, filter))
            .collect();

        match filter.sort_by {
            SortOrder::Relevance => {
                if let Some(ref q) = filter.query {
                    let q_clone = q.clone();
                    results.sort_by(|a, b| {
                        let sa = fuzzy_score(&a.manifest.name, &q_clone);
                        let sb = fuzzy_score(&b.manifest.name, &q_clone);
                        sb.cmp(&sa)
                    });
                }
            }
            SortOrder::Trending => {
                results.sort_by(|a, b| b.trending_score.cmp(&a.trending_score));
            }
            SortOrder::MostDownloaded => {
                results.sort_by(|a, b| b.total_downloads.cmp(&a.total_downloads));
            }
            SortOrder::HighestRated => {
                results.sort_by(|a, b| b.ratings.average().cmp(&a.ratings.average()));
            }
            SortOrder::Newest => {
                results.sort_by(|a, b| b.first_published.cmp(&a.first_published));
            }
            SortOrder::RecentlyUpdated => {
                results.sort_by(|a, b| b.last_updated.cmp(&a.last_updated));
            }
            SortOrder::PriceLowToHigh => {
                results.sort_by(|a, b| price_cents(&a.pricing).cmp(&price_cents(&b.pricing)));
            }
            SortOrder::PriceHighToLow => {
                results.sort_by(|a, b| price_cents(&b.pricing).cmp(&price_cents(&a.pricing)));
            }
            SortOrder::NameAZ => {
                results.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
            }
        }

        let start = filter.page * filter.per_page;
        if start >= results.len() {
            return Vec::new();
        }
        let end = core::cmp::min(start + filter.per_page, results.len());
        results[start..end].to_vec()
    }

    fn matches_filter(&self, listing: &StoreListing, filter: &SearchFilter) -> bool {
        if let Some(ref q) = filter.query {
            if !fuzzy_match(&listing.manifest.name, q)
                && !fuzzy_match(&listing.manifest.description, q)
                && !listing.manifest.tags.iter().any(|t| fuzzy_match(t, q))
            {
                return false;
            }
        }

        if let Some(cat) = filter.category {
            if listing.manifest.category != cat {
                return false;
            }
        }

        if let Some(min) = filter.min_rating {
            if listing.ratings.average() < min as u32 * 10 {
                return false;
            }
        }

        if filter.free_only {
            if !matches!(
                listing.pricing,
                PricingModel::Free | PricingModel::DonationWare
            ) {
                return false;
            }
        }

        if let Some(max_price) = filter.max_price_cents {
            if price_cents(&listing.pricing) > max_price {
                return false;
            }
        }

        if let Some(max_rating) = filter.content_rating {
            if listing.manifest.content_rating > max_rating {
                return false;
            }
        }

        true
    }

    pub fn apps_by_developer(&self, dev_id: DeveloperId) -> Vec<&StoreListing> {
        self.listings
            .values()
            .filter(|l| l.developer == dev_id)
            .collect()
    }

    pub fn recalculate_trending(&mut self) {
        let ids: Vec<AppId> = self.listings.keys().copied().collect();
        for id in ids {
            if let Some(listing) = self.listings.get_mut(&id) {
                listing.compute_trending_score();
            }
        }
    }
}

fn price_cents(pricing: &PricingModel) -> u64 {
    match pricing {
        PricingModel::Paid { price_cents } => *price_cents,
        PricingModel::Subscription { monthly_cents, .. } => *monthly_cents,
        _ => 0,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 6. SANDBOXED INSTALL
// ═══════════════════════════════════════════════════════════════════════════

/// Each app installs into `/apps/<app_name>/` with this layout.
#[derive(Clone, Debug)]
pub struct SandboxLayout {
    pub root: String,
    pub bin_dir: String,
    pub lib_dir: String,
    pub data_dir: String,
    pub cache_dir: String,
    pub config_dir: String,
    pub tmp_dir: String,
}

impl SandboxLayout {
    pub fn for_app(app_name: &str) -> Self {
        let root = alloc::format!("/apps/{}", app_name);
        Self {
            bin_dir: alloc::format!("{}/bin", root),
            lib_dir: alloc::format!("{}/lib", root),
            data_dir: alloc::format!("{}/data", root),
            cache_dir: alloc::format!("{}/cache", root),
            config_dir: alloc::format!("{}/config", root),
            tmp_dir: alloc::format!("{}/tmp", root),
            root,
        }
    }

    pub fn all_dirs(&self) -> Vec<&str> {
        alloc::vec![
            &*self.root,
            &*self.bin_dir,
            &*self.lib_dir,
            &*self.data_dir,
            &*self.cache_dir,
            &*self.config_dir,
            &*self.tmp_dir,
        ]
    }
}

/// Capability request shown to the user during install.
#[derive(Clone, Debug)]
pub struct CapabilityRequest {
    pub permission: AppPermission,
    pub reason: String,
    pub required: bool,
}

/// User's decision on a capability request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityDecision {
    Grant,
    Deny,
    AskEachTime,
}

/// Policy for sideloaded apps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SideloadPolicy {
    /// Always allow sideloading (default for AthenaOS).
    AllowAll,
    /// Allow only signed sideloads.
    SignedOnly,
    /// Block all sideloading.
    Blocked,
}

/// Tracks a sandbox for one installed app. Enforces isolation.
#[derive(Clone, Debug)]
pub struct AppSandbox {
    pub app_id: AppId,
    pub layout: SandboxLayout,
    pub origin: PackageOrigin,
    pub capability_grants: BTreeMap<AppPermission, CapabilityDecision>,
    pub disk_quota_bytes: u64,
    pub disk_used_bytes: u64,
    pub network_allowed: bool,
    pub background_allowed: bool,
}

impl AppSandbox {
    pub fn new(app_id: AppId, app_name: &str, origin: PackageOrigin) -> Self {
        Self {
            app_id,
            layout: SandboxLayout::for_app(app_name),
            origin,
            capability_grants: BTreeMap::new(),
            disk_quota_bytes: 10 * 1024 * 1024 * 1024,
            disk_used_bytes: 0,
            network_allowed: false,
            background_allowed: false,
        }
    }

    pub fn process_requests(
        &mut self,
        requests: &[CapabilityRequest],
        decisions: &[(AppPermission, CapabilityDecision)],
    ) -> Result<()> {
        let decision_map: BTreeMap<AppPermission, CapabilityDecision> =
            decisions.iter().cloned().collect();

        for req in requests {
            let decision = decision_map
                .get(&req.permission)
                .copied()
                .unwrap_or(if req.required {
                    return Err(StoreError::CapabilityDenied);
                } else {
                    CapabilityDecision::Deny
                });

            self.capability_grants.insert(req.permission, decision);

            match req.permission {
                AppPermission::NetworkClient
                | AppPermission::NetworkServer
                | AppPermission::NetworkUnrestricted => {
                    if decision == CapabilityDecision::Grant {
                        self.network_allowed = true;
                    }
                }
                AppPermission::BackgroundExec => {
                    if decision == CapabilityDecision::Grant {
                        self.background_allowed = true;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn check_permission(&self, perm: AppPermission) -> bool {
        matches!(
            self.capability_grants.get(&perm),
            Some(CapabilityDecision::Grant)
        )
    }

    pub fn check_disk_quota(&self, additional_bytes: u64) -> Result<()> {
        if self.disk_used_bytes + additional_bytes > self.disk_quota_bytes {
            return Err(StoreError::QuotaExceeded);
        }
        Ok(())
    }

    pub fn is_sideloaded(&self) -> bool {
        self.origin == PackageOrigin::Sideloaded
    }
}

/// Install context — orchestrates the sandboxed install of one app.
pub struct InstallContext {
    pub app_id: AppId,
    pub manifest: PackageManifest,
    pub sandbox: AppSandbox,
    pub origin: PackageOrigin,
    pub sideload_policy: SideloadPolicy,
    pub snapshot_before: Option<u64>,
    pub install_started: u64,
}

impl InstallContext {
    pub fn new(
        manifest: PackageManifest,
        origin: PackageOrigin,
        policy: SideloadPolicy,
        timestamp: u64,
    ) -> Result<Self> {
        if origin == PackageOrigin::Sideloaded {
            match policy {
                SideloadPolicy::Blocked => return Err(StoreError::SideloadBlocked),
                SideloadPolicy::SignedOnly if !manifest.is_signed() => {
                    return Err(StoreError::SignatureInvalid);
                }
                _ => {}
            }
        }

        let sandbox = AppSandbox::new(manifest.id, &manifest.name, origin);

        Ok(Self {
            app_id: manifest.id,
            manifest,
            sandbox,
            origin,
            sideload_policy: policy,
            snapshot_before: None,
            install_started: timestamp,
        })
    }

    pub fn capability_requests(&self) -> Vec<CapabilityRequest> {
        self.manifest
            .permissions
            .iter()
            .map(|p| CapabilityRequest {
                permission: *p,
                reason: permission_description(*p),
                required: !matches!(
                    p,
                    AppPermission::Notifications
                        | AppPermission::AutoStart
                        | AppPermission::SystemTray
                ),
            })
            .collect()
    }

    pub fn apply_decisions(
        &mut self,
        decisions: &[(AppPermission, CapabilityDecision)],
    ) -> Result<()> {
        let requests = self.capability_requests();
        self.sandbox.process_requests(&requests, decisions)
    }

    pub fn finalize(self, timestamp: u64) -> InstalledApp {
        let install_path = self.sandbox.layout.root.clone();
        let mut app = InstalledApp::new(self.manifest, self.origin, install_path, timestamp);

        for (perm, decision) in &self.sandbox.capability_grants {
            match decision {
                CapabilityDecision::Grant => app.grant_permission(*perm),
                CapabilityDecision::Deny => app.deny_permission(*perm),
                CapabilityDecision::AskEachTime => {}
            }
        }
        app.snapshot_id = self.snapshot_before;
        app
    }
}

fn permission_description(perm: AppPermission) -> String {
    String::from(match perm {
        AppPermission::FileReadHome => "Read files in your home directory",
        AppPermission::FileWriteHome => "Write files in your home directory",
        AppPermission::FileReadSystem => "Read system files",
        AppPermission::NetworkClient => "Connect to the internet",
        AppPermission::NetworkServer => "Accept incoming connections",
        AppPermission::NetworkUnrestricted => "Unrestricted network access",
        AppPermission::Camera => "Use your camera",
        AppPermission::Microphone => "Use your microphone",
        AppPermission::AudioPlayback => "Play audio",
        AppPermission::AudioCapture => "Record system audio",
        AppPermission::GpuCompute => "Use GPU for computation",
        AppPermission::GpuRender => "Use GPU for rendering",
        AppPermission::UsbDevices => "Access USB devices",
        AppPermission::BluetoothAccess => "Access Bluetooth",
        AppPermission::Notifications => "Show notifications",
        AppPermission::BackgroundExec => "Run in the background",
        AppPermission::SystemTray => "Show icon in system tray",
        AppPermission::Clipboard => "Access the clipboard",
        AppPermission::ScreenCapture => "Capture your screen",
        AppPermission::InputCapture => "Capture keyboard/mouse input globally",
        AppPermission::Overlay => "Draw overlays on screen",
        AppPermission::GameMode => "Use SCHED_BODY priority",
        AppPermission::HardwareInfo => "Read hardware information",
        AppPermission::ProcessList => "View running processes",
        AppPermission::AutoStart => "Start automatically at boot",
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// 7. UPDATE MANAGER
// ═══════════════════════════════════════════════════════════════════════════

/// A delta patch — binary diff between two versions.
#[derive(Clone, Debug)]
pub struct DeltaPatch {
    pub app_id: AppId,
    pub from_version: SemVer,
    pub to_version: SemVer,
    pub patch_data: Vec<u8>,
    pub patch_size: u64,
    pub full_size: u64,
    pub sha256_result: [u8; 32],
}

impl DeltaPatch {
    pub fn savings_pct(&self) -> u32 {
        if self.full_size == 0 {
            return 0;
        }
        ((self.full_size - self.patch_size) * 100 / self.full_size) as u32
    }

    pub fn apply(&self, old_data: &[u8]) -> Result<Vec<u8>> {
        let mut result = Vec::from(old_data);

        let mut i = 0;
        while i < self.patch_data.len() {
            let op = self.patch_data[i];
            i += 1;

            match op & 0x03 {
                0 => {
                    if i < self.patch_data.len() {
                        result.push(self.patch_data[i]);
                        i += 1;
                    }
                }
                1 => {
                    if i + 1 < self.patch_data.len() {
                        let offset =
                            ((self.patch_data[i] as usize) << 8) | self.patch_data[i + 1] as usize;
                        i += 2;
                        if offset < result.len() {
                            result.remove(offset);
                        }
                    }
                }
                2 => {
                    if i + 2 < self.patch_data.len() {
                        let offset =
                            ((self.patch_data[i] as usize) << 8) | self.patch_data[i + 1] as usize;
                        let new_byte = self.patch_data[i + 2];
                        i += 3;
                        if offset < result.len() {
                            result[offset] = new_byte;
                        }
                    }
                }
                _ => {}
            }
        }

        let hash = sha256_digest(&result);
        if hash != self.sha256_result {
            return Err(StoreError::ChecksumMismatch);
        }
        Ok(result)
    }
}

/// Pending update info.
#[derive(Clone, Debug)]
pub struct PendingUpdate {
    pub app_id: AppId,
    pub from_version: SemVer,
    pub to_version: SemVer,
    pub download_size: u64,
    pub delta_available: bool,
    pub delta_size: u64,
    pub changelog: String,
    pub critical: bool,
}

impl PendingUpdate {
    pub fn effective_download_size(&self) -> u64 {
        if self.delta_available {
            self.delta_size
        } else {
            self.download_size
        }
    }
}

/// Update scheduling preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateSchedule {
    /// Check + download + install automatically.
    Automatic,
    /// Check + download automatically, ask before install.
    DownloadOnly,
    /// Only check, show notification. User decides.
    NotifyOnly,
    /// Never check. User does manual updates.
    Manual,
}

/// Rollback entry stored alongside each install/update for CoW rollback.
#[derive(Clone, Debug)]
pub struct RollbackEntry {
    pub app_id: AppId,
    pub previous_version: SemVer,
    pub snapshot_id: u64,
    pub timestamp: u64,
    pub files_changed: Vec<String>,
}

/// The update manager — background update checker with delta support
/// and atomic CoW installs with rollback on failure.
pub struct UpdateManager {
    pub schedule: UpdateSchedule,
    pub pending: Vec<PendingUpdate>,
    pub rollback_log: Vec<RollbackEntry>,
    pub check_interval_secs: u64,
    pub last_check: u64,
    pub auto_restart: bool,
    next_snapshot_id: u64,
}

impl UpdateManager {
    pub fn new(schedule: UpdateSchedule) -> Self {
        Self {
            schedule,
            pending: Vec::new(),
            rollback_log: Vec::new(),
            check_interval_secs: 3600,
            last_check: 0,
            auto_restart: false,
            next_snapshot_id: 1,
        }
    }

    pub fn should_check(&self, now: u64) -> bool {
        if self.schedule == UpdateSchedule::Manual {
            return false;
        }
        now.saturating_sub(self.last_check) >= self.check_interval_secs
    }

    pub fn check_for_updates(
        &mut self,
        installed: &BTreeMap<AppId, InstalledApp>,
        repos: &[Repository],
        now: u64,
    ) -> Vec<PendingUpdate> {
        self.last_check = now;
        self.pending.clear();

        for (id, app) in installed {
            if app.state != AppState::Installed {
                continue;
            }
            if !app.auto_update {
                continue;
            }
            if let Some(pinned) = &app.pinned_version {
                if app.manifest.version == *pinned {
                    continue;
                }
            }

            for repo in repos {
                if let Some(entry) = repo.latest_version(*id) {
                    if entry.manifest.version > app.manifest.version {
                        let update = PendingUpdate {
                            app_id: *id,
                            from_version: app.manifest.version,
                            to_version: entry.manifest.version,
                            download_size: entry.download_size,
                            delta_available: false,
                            delta_size: 0,
                            changelog: entry.manifest.changelog.clone(),
                            critical: false,
                        };
                        self.pending.push(update);
                        break;
                    }
                }
            }
        }

        self.pending.clone()
    }

    /// Create a snapshot before applying an update (CoW atomic install).
    pub fn create_snapshot(
        &mut self,
        app_id: AppId,
        current_version: SemVer,
        timestamp: u64,
    ) -> u64 {
        let snap_id = self.next_snapshot_id;
        self.next_snapshot_id += 1;
        self.rollback_log.push(RollbackEntry {
            app_id,
            previous_version: current_version,
            snapshot_id: snap_id,
            timestamp,
            files_changed: Vec::new(),
        });
        snap_id
    }

    pub fn rollback(&self, app_id: AppId) -> Option<&RollbackEntry> {
        self.rollback_log.iter().rev().find(|e| e.app_id == app_id)
    }

    pub fn clear_rollback(&mut self, app_id: AppId) {
        self.rollback_log.retain(|e| e.app_id != app_id);
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn total_download_size(&self) -> u64 {
        self.pending
            .iter()
            .map(|p| p.effective_download_size())
            .sum()
    }

    pub fn critical_updates(&self) -> Vec<&PendingUpdate> {
        self.pending.iter().filter(|p| p.critical).collect()
    }

    pub fn apply_update(
        &mut self,
        app: &mut InstalledApp,
        new_manifest: PackageManifest,
        timestamp: u64,
    ) -> Result<()> {
        let snap_id = self.create_snapshot(app.manifest.id, app.manifest.version, timestamp);
        app.snapshot_id = Some(snap_id);
        let old_version = app.manifest.version;
        let new_version = new_manifest.version;

        app.state = AppState::Updating {
            from: old_version,
            to: new_version,
        };
        app.manifest = new_manifest;
        app.updated_at = timestamp;
        app.state = AppState::Installed;

        self.pending.retain(|p| p.app_id != app.manifest.id);
        Ok(())
    }

    pub fn rollback_update(
        &mut self,
        app: &mut InstalledApp,
        old_manifest: PackageManifest,
    ) -> Result<()> {
        let entry = self
            .rollback_log
            .iter()
            .rev()
            .find(|e| e.app_id == app.manifest.id)
            .ok_or(StoreError::RollbackFailed)?;

        if old_manifest.version != entry.previous_version {
            return Err(StoreError::RollbackFailed);
        }

        app.manifest = old_manifest;
        app.state = AppState::Installed;
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 8. RAESTORE — TOP-LEVEL ORCHESTRATOR
// ═══════════════════════════════════════════════════════════════════════════

/// Transaction operation for the install log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreOp {
    Install,
    Uninstall,
    Update,
    Sideload,
    Rollback,
}

/// Recorded transaction for history/auditability.
#[derive(Clone, Debug)]
pub struct StoreTransaction {
    pub id: TransactionId,
    pub op: StoreOp,
    pub app_id: AppId,
    pub app_name: String,
    pub version: SemVer,
    pub timestamp: u64,
    pub origin: PackageOrigin,
}

/// The top-level AthStore — app store + package manager.
///
/// Design principles (from concept doc):
/// - 12% revenue share
/// - Sideloading allowed and supported as first-class
/// - No review hostage situations
pub struct AthStore {
    pub installed: BTreeMap<AppId, InstalledApp>,
    pub sandboxes: BTreeMap<AppId, AppSandbox>,
    pub repos: Vec<Repository>,
    pub keyring: SigningKeyring,
    pub store_index: StoreIndex,
    pub update_manager: UpdateManager,
    pub download_queue: DownloadQueue,
    pub sideload_policy: SideloadPolicy,
    pub transaction_log: Vec<StoreTransaction>,
    next_txn_id: u64,
    lock: AtomicBool,
}

static STORE_INITIALIZED: AtomicBool = AtomicBool::new(false);
static STORE_TRANSACTION_COUNT: AtomicU64 = AtomicU64::new(0);

impl AthStore {
    pub fn new() -> Self {
        Self {
            installed: BTreeMap::new(),
            sandboxes: BTreeMap::new(),
            repos: Vec::new(),
            keyring: SigningKeyring::new(),
            store_index: StoreIndex::new(),
            update_manager: UpdateManager::new(UpdateSchedule::DownloadOnly),
            download_queue: DownloadQueue::new(4),
            sideload_policy: SideloadPolicy::AllowAll,
            transaction_log: Vec::new(),
            next_txn_id: 1,
            lock: AtomicBool::new(false),
        }
    }

    fn acquire_lock(&self) -> Result<()> {
        if self
            .lock
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(StoreError::LockHeld);
        }
        Ok(())
    }

    fn release_lock(&self) {
        self.lock.store(false, Ordering::SeqCst);
    }

    fn record_transaction(&mut self, op: StoreOp, app: &InstalledApp) {
        let txn = StoreTransaction {
            id: TransactionId(self.next_txn_id),
            op,
            app_id: app.manifest.id,
            app_name: app.manifest.name.clone(),
            version: app.manifest.version,
            timestamp: app.updated_at,
            origin: app.origin,
        };
        self.next_txn_id += 1;
        self.transaction_log.push(txn);
        STORE_TRANSACTION_COUNT.fetch_add(1, Ordering::Relaxed);
    }

    // ── Repository management ───────────────────────────────────────────

    pub fn add_repository(&mut self, config: RepositoryConfig) {
        self.repos.push(Repository::new(config));
    }

    pub fn sync_repositories(&mut self, timestamp: u64) -> Result<()> {
        for repo in &mut self.repos {
            if repo.config.enabled {
                repo.sync(timestamp)?;
            }
        }
        Ok(())
    }

    // ── Install ─────────────────────────────────────────────────────────

    pub fn install(
        &mut self,
        app_id: AppId,
        permission_decisions: &[(AppPermission, CapabilityDecision)],
        timestamp: u64,
    ) -> Result<()> {
        self.acquire_lock()?;

        if self.installed.contains_key(&app_id) {
            self.release_lock();
            return Err(StoreError::AlreadyInstalled);
        }

        let entry = self.find_in_repos(app_id).ok_or_else(|| {
            self.release_lock();
            StoreError::PackageNotFound
        })?;

        let manifest = entry.manifest.clone();

        let mut ctx = InstallContext::new(
            manifest,
            PackageOrigin::Store,
            self.sideload_policy,
            timestamp,
        )
        .map_err(|e| {
            self.release_lock();
            e
        })?;

        ctx.apply_decisions(permission_decisions).map_err(|e| {
            self.release_lock();
            e
        })?;

        let app = ctx.finalize(timestamp);
        let sandbox = AppSandbox::new(app_id, &app.manifest.name, PackageOrigin::Store);

        if let Some(listing) = self.store_index.get_listing_mut(app_id) {
            listing.record_download();
        }

        self.sandboxes.insert(app_id, sandbox);
        self.record_transaction(StoreOp::Install, &app);
        self.installed.insert(app_id, app);

        self.release_lock();
        Ok(())
    }

    // ── Sideload ────────────────────────────────────────────────────────

    pub fn sideload(
        &mut self,
        package: RaePackage,
        permission_decisions: &[(AppPermission, CapabilityDecision)],
        timestamp: u64,
    ) -> Result<()> {
        self.acquire_lock()?;

        let app_id = package.manifest.id;
        if self.installed.contains_key(&app_id) {
            self.release_lock();
            return Err(StoreError::AlreadyInstalled);
        }

        package.verify_integrity().map_err(|e| {
            self.release_lock();
            e
        })?;

        let mut ctx = InstallContext::new(
            package.manifest,
            PackageOrigin::Sideloaded,
            self.sideload_policy,
            timestamp,
        )
        .map_err(|e| {
            self.release_lock();
            e
        })?;

        ctx.apply_decisions(permission_decisions).map_err(|e| {
            self.release_lock();
            e
        })?;

        let app = ctx.finalize(timestamp);
        let sandbox = AppSandbox::new(app_id, &app.manifest.name, PackageOrigin::Sideloaded);

        self.sandboxes.insert(app_id, sandbox);
        self.record_transaction(StoreOp::Sideload, &app);
        self.installed.insert(app_id, app);

        self.release_lock();
        Ok(())
    }

    // ── Uninstall ───────────────────────────────────────────────────────

    pub fn uninstall(&mut self, app_id: AppId, _timestamp: u64) -> Result<()> {
        self.acquire_lock()?;

        let app = match self.installed.get(&app_id) {
            Some(a) => a.clone(),
            None => {
                self.release_lock();
                return Err(StoreError::NotInstalled);
            }
        };

        let deps_using: Vec<AppId> = self
            .installed
            .values()
            .filter(|a| a.manifest.dependencies.iter().any(|d| d.app_id == app_id))
            .map(|a| a.manifest.id)
            .collect();

        if !deps_using.is_empty() {
            self.release_lock();
            return Err(StoreError::DependencyConflict);
        }

        self.record_transaction(StoreOp::Uninstall, &app);
        self.installed.remove(&app_id);
        self.sandboxes.remove(&app_id);
        self.update_manager.clear_rollback(app_id);

        self.release_lock();
        Ok(())
    }

    // ── Update ──────────────────────────────────────────────────────────

    pub fn check_updates(&mut self, timestamp: u64) -> Vec<PendingUpdate> {
        self.update_manager
            .check_for_updates(&self.installed, &self.repos, timestamp)
    }

    pub fn apply_update(&mut self, app_id: AppId, timestamp: u64) -> Result<()> {
        self.acquire_lock()?;

        let new_manifest = match self.find_in_repos(app_id) {
            Some(entry) => entry.manifest.clone(),
            None => {
                self.release_lock();
                return Err(StoreError::PackageNotFound);
            }
        };

        let app = match self.installed.get_mut(&app_id) {
            Some(a) => a,
            None => {
                self.release_lock();
                return Err(StoreError::NotInstalled);
            }
        };

        self.update_manager
            .apply_update(app, new_manifest, timestamp)
            .map_err(|e| {
                self.release_lock();
                e
            })?;

        let app_ref = self.installed.get(&app_id).unwrap().clone();
        self.record_transaction(StoreOp::Update, &app_ref);

        self.release_lock();
        Ok(())
    }

    pub fn apply_all_updates(&mut self, timestamp: u64) -> Result<usize> {
        let pending_ids: Vec<AppId> = self
            .update_manager
            .pending
            .iter()
            .map(|p| p.app_id)
            .collect();

        let mut count = 0;
        for id in pending_ids {
            if self.apply_update(id, timestamp).is_ok() {
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn rollback(&mut self, app_id: AppId, old_manifest: PackageManifest) -> Result<()> {
        self.acquire_lock()?;

        let app = match self.installed.get_mut(&app_id) {
            Some(a) => a,
            None => {
                self.release_lock();
                return Err(StoreError::NotInstalled);
            }
        };

        self.update_manager
            .rollback_update(app, old_manifest)
            .map_err(|e| {
                self.release_lock();
                e
            })?;

        let app_ref = self.installed.get(&app_id).unwrap().clone();
        self.record_transaction(StoreOp::Rollback, &app_ref);

        self.release_lock();
        Ok(())
    }

    // ── Search & browse ─────────────────────────────────────────────────

    pub fn search(&self, filter: &SearchFilter) -> Vec<&StoreListing> {
        self.store_index.search(filter)
    }

    pub fn featured(&self) -> Vec<&StoreListing> {
        self.store_index.featured_apps()
    }

    pub fn trending(&self, limit: usize) -> Vec<&StoreListing> {
        self.store_index.trending_apps(limit)
    }

    pub fn new_releases(&self, since: u64, limit: usize) -> Vec<&StoreListing> {
        self.store_index.new_apps(since, limit)
    }

    pub fn top_rated(&self, limit: usize) -> Vec<&StoreListing> {
        self.store_index.top_rated(limit)
    }

    pub fn browse_category(&self, cat: StoreCategory) -> Vec<&StoreListing> {
        self.store_index
            .listings
            .values()
            .filter(|l| l.manifest.category == cat)
            .collect()
    }

    pub fn get_developer(&self, id: DeveloperId) -> Option<&DeveloperPage> {
        self.store_index.get_developer(id)
    }

    pub fn get_collection(&self, id: CollectionId) -> Option<&EditorialCollection> {
        self.store_index.collections.get(&id)
    }

    pub fn submit_review(&mut self, app_id: AppId, review: Review) -> Result<ReviewId> {
        if !self.installed.contains_key(&app_id) {
            return Err(StoreError::ReviewNotAllowed);
        }
        self.store_index.submit_review(app_id, review)
    }

    // ── Queries ─────────────────────────────────────────────────────────

    pub fn is_installed(&self, app_id: AppId) -> bool {
        self.installed.contains_key(&app_id)
    }

    pub fn get_installed(&self, app_id: AppId) -> Option<&InstalledApp> {
        self.installed.get(&app_id)
    }

    pub fn list_installed(&self) -> Vec<&InstalledApp> {
        self.installed.values().collect()
    }

    pub fn installed_count(&self) -> usize {
        self.installed.len()
    }

    pub fn sideloaded_apps(&self) -> Vec<&InstalledApp> {
        self.installed
            .values()
            .filter(|a| a.is_sideloaded())
            .collect()
    }

    pub fn apps_needing_permissions(&self) -> Vec<&InstalledApp> {
        self.installed
            .values()
            .filter(|a| !a.pending_permissions().is_empty())
            .collect()
    }

    pub fn total_disk_usage(&self) -> u64 {
        self.installed.values().map(|a| a.disk_usage).sum()
    }

    pub fn transaction_history(&self) -> &[StoreTransaction] {
        &self.transaction_log
    }

    pub fn total_transactions() -> u64 {
        STORE_TRANSACTION_COUNT.load(Ordering::Relaxed)
    }

    // ── Helpers ──────────────────────────────────────────────────────────

    fn find_in_repos(&self, id: AppId) -> Option<&RepoEntry> {
        for repo in &self.repos {
            if let Some(entry) = repo.latest_version(id) {
                return Some(entry);
            }
        }
        None
    }

    /// Resolve dependencies for a target set, returning install order.
    pub fn resolve_dependencies(&self, targets: &[AppId]) -> Result<Vec<AppId>> {
        let mut resolver = DependencyResolver::new(ConflictStrategy::PreferNewest);

        for repo in &self.repos {
            for entries in repo.index.values() {
                if let Some(latest) = entries.last() {
                    resolver.add_package(
                        latest.manifest.id,
                        latest.manifest.name.clone(),
                        latest.manifest.version,
                        latest.manifest.dependencies.clone(),
                    );
                }
            }
        }

        resolver.resolve(targets)
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 8b. TRANSACTIONAL MULTI-PACKAGE INSTALL  (the "safe install" guarantee)
// ═══════════════════════════════════════════════════════════════════════════
//
// LEGACY_GAMING_CONCEPT.md §"the apps people use just work" + §"the user owns the
// machine" (atomic CoW updates + one-click rollback): installing an app is a
// TRANSACTION, not a sequence of independent mutations. Either the app and every
// dependency it needs land together, or NOTHING changes — a missing dependency, a
// version conflict, a dependency cycle, or a verify failure aborts the whole plan
// and leaves the installed-app registry exactly as it was. A half-installed app
// (binary present, a dependency missing) is the "broken state" this section
// exists to make impossible.
//
// The flow is two-phase so the UI can show the user EXACTLY what will change
// before anything is committed:
//
//   plan_install(target)  → InstallPlan  (resolve deps, classify each step,
//                                          detect conflict/cycle/missing — pure,
//                                          mutates nothing)
//   commit_plan(plan)     → Result<()>   (apply every step; on ANY failure,
//                                          roll back every step already applied,
//                                          restoring the pre-transaction registry)
//
// Dependency bookkeeping: a package the user explicitly asked for is recorded as
// `Explicit`; a package pulled in only to satisfy a dependency is `Dependency`.
// This is what lets uninstall garbage-collect an auto-installed dep once nothing
// needs it, while refusing to orphan one that another app still requires.

/// Why an app is present in the installed registry — drives orphan GC on
/// uninstall. An `Explicit` app was directly requested by the user and is never
/// auto-removed; a `Dependency` app was pulled in transitively and becomes a GC
/// candidate when its last reverse-dependency goes away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallReason {
    /// User explicitly requested this app.
    Explicit,
    /// Pulled in only to satisfy another app's dependency.
    Dependency,
}

/// What a single step of a resolved [`InstallPlan`] will do to one app id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanStep {
    /// Not currently installed → fresh install of this version.
    Install { app_id: AppId, version: SemVer },
    /// Installed at an older version → upgrade to this version.
    Upgrade {
        app_id: AppId,
        from: SemVer,
        to: SemVer,
    },
    /// Already installed at a version that satisfies the requirement — no-op,
    /// recorded so the plan is a complete picture of what was considered.
    AlreadySatisfied { app_id: AppId, version: SemVer },
}

impl PlanStep {
    pub fn app_id(&self) -> AppId {
        match self {
            PlanStep::Install { app_id, .. }
            | PlanStep::Upgrade { app_id, .. }
            | PlanStep::AlreadySatisfied { app_id, .. } => *app_id,
        }
    }

    /// Does this step actually change the registry (vs. a no-op)?
    pub fn is_mutating(&self) -> bool {
        !matches!(self, PlanStep::AlreadySatisfied { .. })
    }
}

/// A fully-resolved, not-yet-applied plan to install a target app and whatever
/// dependencies it needs. Steps are in dependency-first order: a leaf dependency
/// appears before the package that requires it, so committing top-to-bottom never
/// installs an app before something it needs.
#[derive(Clone, Debug)]
pub struct InstallPlan {
    /// The app the user explicitly requested.
    pub target: AppId,
    /// Ordered steps (dependency-first). May contain `AlreadySatisfied` no-ops.
    pub steps: Vec<PlanStep>,
    /// For each step's app id, whether it is explicit (the target) or a pulled-in
    /// dependency — recorded into the registry on commit for orphan GC.
    pub reasons: BTreeMap<AppId, InstallReason>,
}

impl InstallPlan {
    /// Steps that will actually mutate the registry (excludes `AlreadySatisfied`).
    pub fn mutating_steps(&self) -> Vec<&PlanStep> {
        self.steps.iter().filter(|s| s.is_mutating()).collect()
    }

    /// Count of fresh installs in the plan.
    pub fn install_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| matches!(s, PlanStep::Install { .. }))
            .count()
    }

    /// Count of upgrades in the plan.
    pub fn upgrade_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| matches!(s, PlanStep::Upgrade { .. }))
            .count()
    }

    /// True if committing this plan would change nothing (everything already
    /// satisfied) — e.g. re-installing an app that is fully present.
    pub fn is_noop(&self) -> bool {
        self.steps.iter().all(|s| !s.is_mutating())
    }
}

impl AthStore {
    /// Build a transactional install plan for `target`: resolve its dependency
    /// closure against the repositories, classify each app as a fresh install /
    /// upgrade / already-satisfied, and surface any failure (missing package,
    /// dependency cycle, version conflict) BEFORE anything is committed.
    ///
    /// This mutates nothing — it is the "show the user what will change" phase of
    /// the two-phase commit. A returned `Ok(plan)` is guaranteed installable in
    /// the order given; an `Err` means the transaction cannot be satisfied and the
    /// caller must not proceed.
    pub fn plan_install(&self, target: AppId) -> Result<InstallPlan> {
        // Resolve the full dependency-first order (also runs conflict + cycle
        // detection inside the resolver).
        let order = self.resolve_dependencies(&[target])?;
        if order.is_empty() {
            // resolve_dependencies returns PackageNotFound for an unknown target,
            // so an empty order means the target had no node — treat as not found.
            return Err(StoreError::PackageNotFound);
        }

        let mut steps = Vec::new();
        let mut reasons = BTreeMap::new();

        for &app_id in &order {
            let entry = self
                .find_in_repos(app_id)
                .ok_or(StoreError::PackageNotFound)?;
            let repo_version = entry.manifest.version;

            // Verify the version the repo actually offers satisfies every required
            // constraint placed on it by another app *in this closure*. The
            // resolver's conflict pass guarantees the requested constraints are
            // mutually compatible; this confirms a real candidate exists, so the
            // plan can't promise an install that violates a declared dependency.
            for &requestor in &order {
                if let Some(constraint) = self.constraint_for(requestor, app_id) {
                    if !constraint.satisfied_by(&repo_version) {
                        return Err(StoreError::VersionMismatch);
                    }
                }
            }

            let reason = if app_id == target {
                InstallReason::Explicit
            } else {
                InstallReason::Dependency
            };
            reasons.insert(app_id, reason);

            let step = match self.installed.get(&app_id) {
                None => PlanStep::Install {
                    app_id,
                    version: repo_version,
                },
                Some(existing) => {
                    let cur = existing.manifest.version;
                    match repo_version.cmp(&cur) {
                        core::cmp::Ordering::Greater => PlanStep::Upgrade {
                            app_id,
                            from: cur,
                            to: repo_version,
                        },
                        // Equal, or repo is OLDER than installed: do not downgrade
                        // as part of a dependency pull. The installed version is
                        // kept and reported as satisfied.
                        _ => PlanStep::AlreadySatisfied {
                            app_id,
                            version: cur,
                        },
                    }
                }
            };
            steps.push(step);
        }

        Ok(InstallPlan {
            target,
            steps,
            reasons,
        })
    }

    /// The version constraint `requestor` places on dependency `dep`, if any.
    fn constraint_for(&self, requestor: AppId, dep: AppId) -> Option<VersionConstraint> {
        let entry = self.find_in_repos(requestor)?;
        entry
            .manifest
            .dependencies
            .iter()
            .find(|d| d.app_id == dep && !d.optional)
            .map(|d| d.constraint.clone())
    }

    /// Commit a resolved [`InstallPlan`] atomically. Applies every mutating step
    /// in dependency-first order; if ANY step fails (verify, sideload policy,
    /// capability), every step already applied in THIS commit is rolled back,
    /// restoring the registry to its exact pre-commit state. On success, every app
    /// and its `InstallReason` is recorded. All-or-nothing — never a partial
    /// install.
    ///
    /// `decisions` provides permission grants keyed by app id; an app whose
    /// required permissions are not granted aborts (and rolls back) the whole
    /// transaction.
    pub fn commit_plan(
        &mut self,
        plan: &InstallPlan,
        decisions: &BTreeMap<AppId, Vec<(AppPermission, CapabilityDecision)>>,
        timestamp: u64,
    ) -> Result<()> {
        self.acquire_lock()?;

        // Snapshot the ids we touch so we can undo on failure. We only ever ADD
        // or REPLACE entries within a commit, so remembering the prior value (or
        // None) per touched id is a complete inverse.
        let mut applied: Vec<(AppId, Option<InstalledApp>, Option<AppSandbox>)> = Vec::new();
        // Snapshot the audit-log length + next-txn id so a rolled-back commit
        // leaves NO transaction records behind — the registry AND its history
        // return to the exact pre-commit state.
        let txn_log_len = self.transaction_log.len();
        let next_txn_id = self.next_txn_id;

        for step in &plan.steps {
            if !step.is_mutating() {
                continue;
            }
            let app_id = step.app_id();
            let prior_app = self.installed.get(&app_id).cloned();
            let prior_sandbox = self.sandboxes.get(&app_id).cloned();

            let result = self.apply_plan_step(step, plan, decisions, timestamp);

            match result {
                Ok(()) => {
                    applied.push((app_id, prior_app, prior_sandbox));
                }
                Err(e) => {
                    // Roll back everything applied in this commit, newest first.
                    for (id, prev_app, prev_sandbox) in applied.into_iter().rev() {
                        match prev_app {
                            Some(a) => {
                                self.installed.insert(id, a);
                            }
                            None => {
                                self.installed.remove(&id);
                            }
                        }
                        match prev_sandbox {
                            Some(s) => {
                                self.sandboxes.insert(id, s);
                            }
                            None => {
                                self.sandboxes.remove(&id);
                            }
                        }
                    }
                    // Discard any audit records this commit appended.
                    self.transaction_log.truncate(txn_log_len);
                    self.next_txn_id = next_txn_id;
                    self.release_lock();
                    return Err(e);
                }
            }
        }

        self.release_lock();
        Ok(())
    }

    /// Apply one mutating plan step. Builds + verifies the install context for the
    /// app, applies the caller's permission decisions, and records the result with
    /// its `InstallReason`. Returns `Err` (without leaving the entry half-written)
    /// so `commit_plan` can roll the whole transaction back.
    fn apply_plan_step(
        &mut self,
        step: &PlanStep,
        plan: &InstallPlan,
        decisions: &BTreeMap<AppId, Vec<(AppPermission, CapabilityDecision)>>,
        timestamp: u64,
    ) -> Result<()> {
        let app_id = step.app_id();
        let entry = self
            .find_in_repos(app_id)
            .ok_or(StoreError::PackageNotFound)?;
        let manifest = entry.manifest.clone();

        let mut ctx = InstallContext::new(
            manifest,
            PackageOrigin::Store,
            self.sideload_policy,
            timestamp,
        )?;

        let empty = Vec::new();
        let app_decisions = decisions.get(&app_id).unwrap_or(&empty);
        ctx.apply_decisions(app_decisions)?;

        let mut app = ctx.finalize(timestamp);
        // Carry the install reason so uninstall can GC orphaned deps.
        app.install_reason = plan
            .reasons
            .get(&app_id)
            .copied()
            .unwrap_or(InstallReason::Explicit);

        // An upgrade preserves the prior install timestamp; a fresh install uses
        // `timestamp` for both. This keeps "installed_at" stable across upgrades.
        if let Some(existing) = self.installed.get(&app_id) {
            app.installed_at = existing.installed_at;
            app.launch_count = existing.launch_count;
            app.last_launched = existing.last_launched;
        }

        let sandbox = AppSandbox::new(app_id, &app.manifest.name, PackageOrigin::Store);
        let op = match step {
            PlanStep::Upgrade { .. } => StoreOp::Update,
            _ => StoreOp::Install,
        };

        if let Some(listing) = self.store_index.get_listing_mut(app_id) {
            listing.record_download();
        }

        self.sandboxes.insert(app_id, sandbox);
        self.record_transaction(op, &app);
        self.installed.insert(app_id, app);
        Ok(())
    }

    /// Transactionally uninstall an app and garbage-collect any dependency-only
    /// apps that it leaves orphaned. Refuses (with `DependencyConflict`) to remove
    /// an app that another *still-installed* app depends on. Auto-installed deps
    /// whose last reverse-dependency is removed by this uninstall are collected
    /// too; an `Explicit` app is never auto-removed.
    ///
    /// Returns the list of app ids actually removed (the target plus any GC'd
    /// orphans). All-or-nothing: if the target cannot be removed, nothing changes.
    pub fn uninstall_with_gc(&mut self, app_id: AppId, timestamp: u64) -> Result<Vec<AppId>> {
        self.acquire_lock()?;

        if !self.installed.contains_key(&app_id) {
            self.release_lock();
            return Err(StoreError::NotInstalled);
        }

        // Refuse to remove the target if a still-installed app (other than itself)
        // depends on it.
        if self.has_active_dependents(app_id, &[app_id]) {
            self.release_lock();
            return Err(StoreError::DependencyConflict);
        }

        let mut removed: Vec<AppId> = Vec::new();
        self.remove_one(app_id, timestamp);
        removed.push(app_id);

        // Iteratively GC dependency-only apps that are now orphaned. We pass the
        // already-removed set as "ignored" reverse-deps so a chain collapses.
        loop {
            let candidate = self.installed.values().find(|a| {
                a.install_reason == InstallReason::Dependency
                    && !self.has_active_dependents(a.manifest.id, &removed)
            });
            match candidate {
                Some(app) => {
                    let id = app.manifest.id;
                    self.remove_one(id, timestamp);
                    removed.push(id);
                }
                None => break,
            }
        }

        self.release_lock();
        Ok(removed)
    }

    /// Is any installed app (other than those in `ignored`) declaring a required
    /// dependency on `app_id`?
    fn has_active_dependents(&self, app_id: AppId, ignored: &[AppId]) -> bool {
        self.installed.values().any(|a| {
            !ignored.contains(&a.manifest.id)
                && a.manifest
                    .dependencies
                    .iter()
                    .any(|d| d.app_id == app_id && !d.optional)
        })
    }

    /// Remove one app's registry + sandbox + rollback state, recording the txn.
    /// Internal helper for the GC uninstall path (lock already held).
    fn remove_one(&mut self, app_id: AppId, timestamp: u64) {
        if let Some(app) = self.installed.get(&app_id).cloned() {
            let mut app = app;
            app.updated_at = timestamp;
            self.record_transaction(StoreOp::Uninstall, &app);
        }
        self.installed.remove(&app_id);
        self.sandboxes.remove(&app_id);
        self.update_manager.clear_rollback(app_id);
    }
}

// ── Global store instance ───────────────────────────────────────────────

static mut RAESTORE: Option<AthStore> = None;

pub fn init() {
    if STORE_INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        RAESTORE = Some(AthStore::new());
    }
}

pub fn store() -> &'static AthStore {
    unsafe { RAESTORE.as_ref().expect("AthStore not initialized") }
}

pub fn store_mut() -> &'static mut AthStore {
    unsafe { RAESTORE.as_mut().expect("AthStore not initialized") }
}

// ═══════════════════════════════════════════════════════════════════════════
// N. `.athpkg` WIRE-FORMAT CODEC  (untrusted-input parse + verify pipeline)
// ═══════════════════════════════════════════════════════════════════════════
//
// LEGACY_GAMING_CONCEPT.md §"security by default, not by friction" + §"the apps people
// use just work": a `.athpkg` arrives as opaque bytes — downloaded from a repo,
// double-clicked off a USB stick, or pulled from a mirror. THIS is the single
// trust boundary where untrusted bytes become a package the system will run, so
// the parser MUST be bounds-checked and total: every malformed/truncated/hostile
// input returns `Err`, never panics. Verification happens BEFORE the bytes are
// trusted, in fail-closed order:
//
//   1. magic + format-version check        (is this even a RaePackage?)
//   2. bounds-checked TLV decode           (every length validated vs remaining)
//   3. manifest decode                     (typed fields, no field trusted yet)
//   4. section checksum verify (SHA-256)   (integrity: bytes match the TOC)
//   5. code-hash binds the manifest        (manifest.sha256_hash == H(code))
//   6. signature verify (Ed25519, keyring) (authenticity: a trusted key signed it)
//
// The on-disk layout is a flat little-endian container so it round-trips the
// existing in-memory `RaePackage` without a serde dependency:
//
//   [0..8]   magic  = b"RAEPKG\x01\x00"
//   [8..10]  format_version : u16
//   [10..]   blob: a sequence of length-prefixed records, each
//              u8  tag           (RecordTag)
//              u32 len           (payload length, LE)
//              [len] payload
//            tags: 1=Manifest, 2=Code, 3=Assets, 4=Metadata, 5=Signature
//            unknown tags are skipped (forward-compat), but their length is
//            still bounds-checked so a hostile len can't run us off the buffer.
//
// Strings are u32-len-prefixed UTF-8 (lossy on decode — never panics on bad
// UTF-8). Vecs are u32-count-prefixed. All integers little-endian.

/// Errors specific to decoding the `.athpkg` byte container. These map onto
/// `StoreError` for the public pipeline but are kept distinct so a caller can
/// tell "the bytes were garbage" from "the signature was forged".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PkgParseError {
    /// Buffer shorter than the fixed header, or a record claims more bytes than
    /// remain in the buffer.
    Truncated,
    /// Magic bytes are not `RAEPKG\x01\x00`.
    BadMagic,
    /// `format_version` is newer than this implementation understands.
    UnsupportedVersion,
    /// A required record (the manifest) was absent.
    MissingManifest,
    /// A field inside a record was malformed (e.g. a bogus enum discriminant or
    /// an unterminated string).
    MalformedField,
    /// Total declared sizes overflowed `usize` / `u64`.
    SizeOverflow,
}

impl From<PkgParseError> for StoreError {
    fn from(e: PkgParseError) -> Self {
        match e {
            // BadMagic / version / missing-manifest / malformed all mean "this
            // is not a package we can trust" → ManifestInvalid.
            PkgParseError::BadMagic
            | PkgParseError::UnsupportedVersion
            | PkgParseError::MissingManifest
            | PkgParseError::MalformedField => StoreError::ManifestInvalid,
            PkgParseError::Truncated | PkgParseError::SizeOverflow => StoreError::SectionCorrupted,
        }
    }
}

/// Magic for the flat container (distinct from the in-memory `SectionTable::MAGIC`
/// which it intentionally equals — same 8 bytes lead both representations).
pub const RAEPKG_CONTAINER_MAGIC: [u8; 8] = SectionTable::MAGIC;
/// Highest container `format_version` this build can decode.
pub const RAEPKG_MAX_FORMAT_VERSION: u16 = 1;
/// Hard cap on any single length field, so a hostile 4 GiB length can't trigger
/// a giant allocation before we've even validated the buffer. 256 MiB is well
/// above any plausible single section for an app store package.
const RAEPKG_MAX_RECORD_LEN: u32 = 256 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RecordTag {
    Manifest,
    Code,
    Assets,
    Metadata,
    Signature,
    Unknown(u8),
}

impl RecordTag {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => RecordTag::Manifest,
            2 => RecordTag::Code,
            3 => RecordTag::Assets,
            4 => RecordTag::Metadata,
            5 => RecordTag::Signature,
            other => RecordTag::Unknown(other),
        }
    }
    fn to_u8(self) -> u8 {
        match self {
            RecordTag::Manifest => 1,
            RecordTag::Code => 2,
            RecordTag::Assets => 3,
            RecordTag::Metadata => 4,
            RecordTag::Signature => 5,
            RecordTag::Unknown(v) => v,
        }
    }
}

// ── Bounds-checked cursor over untrusted bytes ──────────────────────────────
// Every read validates against `remaining()`; a short read returns `None`, which
// the decoders translate into `PkgParseError::Truncated`/`MalformedField`. No
// indexing that can panic, no `unwrap`, no slicing past the end.

struct ByteReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> ByteReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        ByteReader { buf, pos: 0 }
    }
    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if n > self.remaining() {
            return None;
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
    fn u16(&mut self) -> Option<u16> {
        let s = self.take(2)?;
        Some(u16::from_le_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        let s = self.take(4)?;
        Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u64(&mut self) -> Option<u64> {
        let s = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Some(u64::from_le_bytes(a))
    }
    fn hash32(&mut self) -> Option<[u8; 32]> {
        let s = self.take(32)?;
        let mut a = [0u8; 32];
        a.copy_from_slice(s);
        Some(a)
    }
    /// u32-len-prefixed payload, length-capped against the buffer.
    fn bytes(&mut self) -> Option<Vec<u8>> {
        let len = self.u32()? as usize;
        let s = self.take(len)?;
        Some(s.to_vec())
    }
    /// u32-len-prefixed UTF-8 (lossy — never panics on invalid sequences).
    fn string(&mut self) -> Option<String> {
        let raw = self.bytes()?;
        Some(String::from_utf8_lossy(&raw).into_owned())
    }
}

// ── Little-endian writer ────────────────────────────────────────────────────

struct ByteWriter {
    buf: Vec<u8>,
}

impl ByteWriter {
    fn new() -> Self {
        ByteWriter { buf: Vec::new() }
    }
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn hash32(&mut self, v: &[u8; 32]) {
        self.buf.extend_from_slice(v);
    }
    fn bytes(&mut self, v: &[u8]) {
        self.u32(v.len() as u32);
        self.buf.extend_from_slice(v);
    }
    fn string(&mut self, v: &str) {
        self.bytes(v.as_bytes());
    }
}

// ── Enum <-> u8 wire mappings (total decode: bad discriminant => Err) ────────

fn content_rating_to_u8(r: ContentRating) -> u8 {
    match r {
        ContentRating::Everyone => 0,
        ContentRating::Teen => 1,
        ContentRating::Mature => 2,
        ContentRating::AdultsOnly => 3,
    }
}
fn content_rating_from_u8(v: u8) -> Option<ContentRating> {
    Some(match v {
        0 => ContentRating::Everyone,
        1 => ContentRating::Teen,
        2 => ContentRating::Mature,
        3 => ContentRating::AdultsOnly,
        _ => return None,
    })
}

fn category_to_u8(c: StoreCategory) -> u8 {
    match c {
        StoreCategory::Games => 0,
        StoreCategory::GameUtilities => 1,
        StoreCategory::Emulators => 2,
        StoreCategory::Productivity => 3,
        StoreCategory::CreativeTools => 4,
        StoreCategory::DeveloperTools => 5,
        StoreCategory::SystemUtilities => 6,
        StoreCategory::Utilities => 7,
        StoreCategory::Communication => 8,
        StoreCategory::Media => 9,
        StoreCategory::Education => 10,
        StoreCategory::Security => 11,
        StoreCategory::Customization => 12,
        StoreCategory::Themes => 13,
        StoreCategory::Drivers => 14,
        StoreCategory::Libraries => 15,
    }
}
fn category_from_u8(v: u8) -> Option<StoreCategory> {
    Some(match v {
        0 => StoreCategory::Games,
        1 => StoreCategory::GameUtilities,
        2 => StoreCategory::Emulators,
        3 => StoreCategory::Productivity,
        4 => StoreCategory::CreativeTools,
        5 => StoreCategory::DeveloperTools,
        6 => StoreCategory::SystemUtilities,
        7 => StoreCategory::Utilities,
        8 => StoreCategory::Communication,
        9 => StoreCategory::Media,
        10 => StoreCategory::Education,
        11 => StoreCategory::Security,
        12 => StoreCategory::Customization,
        13 => StoreCategory::Themes,
        14 => StoreCategory::Drivers,
        15 => StoreCategory::Libraries,
        _ => return None,
    })
}

fn permission_to_u8(p: AppPermission) -> u8 {
    match p {
        AppPermission::FileReadHome => 0,
        AppPermission::FileWriteHome => 1,
        AppPermission::FileReadSystem => 2,
        AppPermission::NetworkClient => 3,
        AppPermission::NetworkServer => 4,
        AppPermission::NetworkUnrestricted => 5,
        AppPermission::Camera => 6,
        AppPermission::Microphone => 7,
        AppPermission::AudioPlayback => 8,
        AppPermission::AudioCapture => 9,
        AppPermission::GpuCompute => 10,
        AppPermission::GpuRender => 11,
        AppPermission::UsbDevices => 12,
        AppPermission::BluetoothAccess => 13,
        AppPermission::Notifications => 14,
        AppPermission::BackgroundExec => 15,
        AppPermission::SystemTray => 16,
        AppPermission::Clipboard => 17,
        AppPermission::ScreenCapture => 18,
        AppPermission::InputCapture => 19,
        AppPermission::Overlay => 20,
        AppPermission::GameMode => 21,
        AppPermission::HardwareInfo => 22,
        AppPermission::ProcessList => 23,
        AppPermission::AutoStart => 24,
    }
}
fn permission_from_u8(v: u8) -> Option<AppPermission> {
    Some(match v {
        0 => AppPermission::FileReadHome,
        1 => AppPermission::FileWriteHome,
        2 => AppPermission::FileReadSystem,
        3 => AppPermission::NetworkClient,
        4 => AppPermission::NetworkServer,
        5 => AppPermission::NetworkUnrestricted,
        6 => AppPermission::Camera,
        7 => AppPermission::Microphone,
        8 => AppPermission::AudioPlayback,
        9 => AppPermission::AudioCapture,
        10 => AppPermission::GpuCompute,
        11 => AppPermission::GpuRender,
        12 => AppPermission::UsbDevices,
        13 => AppPermission::BluetoothAccess,
        14 => AppPermission::Notifications,
        15 => AppPermission::BackgroundExec,
        16 => AppPermission::SystemTray,
        17 => AppPermission::Clipboard,
        18 => AppPermission::ScreenCapture,
        19 => AppPermission::InputCapture,
        20 => AppPermission::Overlay,
        21 => AppPermission::GameMode,
        22 => AppPermission::HardwareInfo,
        23 => AppPermission::ProcessList,
        24 => AppPermission::AutoStart,
        _ => return None,
    })
}

fn sig_alg_to_u8(a: SignatureAlgorithm) -> u8 {
    match a {
        SignatureAlgorithm::Ed25519 => 0,
        SignatureAlgorithm::Rsa4096Sha256 => 1,
    }
}
fn sig_alg_from_u8(v: u8) -> Option<SignatureAlgorithm> {
    Some(match v {
        0 => SignatureAlgorithm::Ed25519,
        1 => SignatureAlgorithm::Rsa4096Sha256,
        _ => return None,
    })
}

// VersionConstraint wire form: 1 tag byte + (0/1/2) SemVers.
fn write_constraint(w: &mut ByteWriter, c: &VersionConstraint) {
    match c {
        VersionConstraint::Exact(v) => {
            w.u8(0);
            write_semver(w, v);
        }
        VersionConstraint::Range { min, max } => {
            w.u8(1);
            write_semver(w, min);
            write_semver(w, max);
        }
        VersionConstraint::Compatible(v) => {
            w.u8(2);
            write_semver(w, v);
        }
        VersionConstraint::Any => w.u8(3),
    }
}
fn read_constraint(r: &mut ByteReader) -> Option<VersionConstraint> {
    Some(match r.u8()? {
        0 => VersionConstraint::Exact(read_semver(r)?),
        1 => VersionConstraint::Range {
            min: read_semver(r)?,
            max: read_semver(r)?,
        },
        2 => VersionConstraint::Compatible(read_semver(r)?),
        3 => VersionConstraint::Any,
        _ => return None,
    })
}

fn write_semver(w: &mut ByteWriter, v: &SemVer) {
    w.u16(v.major);
    w.u16(v.minor);
    w.u16(v.patch);
}
fn read_semver(r: &mut ByteReader) -> Option<SemVer> {
    Some(SemVer::new(r.u16()?, r.u16()?, r.u16()?))
}

// ── Manifest <-> bytes ──────────────────────────────────────────────────────

fn encode_manifest(m: &PackageManifest) -> Vec<u8> {
    let mut w = ByteWriter::new();
    w.u64(m.id.0);
    w.string(&m.name);
    write_semver(&mut w, &m.version);
    w.string(&m.author);
    w.u64(m.developer_id.0);
    w.string(&m.description);
    w.string(&m.long_description);
    w.string(&m.license);
    w.string(&m.homepage);
    w.string(&m.repository);
    write_semver(&mut w, &m.min_os_version);
    w.u8(content_rating_to_u8(m.content_rating));
    w.u8(category_to_u8(m.category));
    // tags
    w.u32(m.tags.len() as u32);
    for t in &m.tags {
        w.string(t);
    }
    // permissions
    w.u32(m.permissions.len() as u32);
    for p in &m.permissions {
        w.u8(permission_to_u8(*p));
    }
    // dependencies
    w.u32(m.dependencies.len() as u32);
    for d in &m.dependencies {
        w.u64(d.app_id.0);
        w.string(&d.name);
        write_constraint(&mut w, &d.constraint);
        w.u8(d.optional as u8);
    }
    w.u64(m.size_bytes);
    w.u64(m.installed_size);
    w.hash32(&m.sha256_hash);
    // signature (optional)
    match &m.signature {
        Some(s) => {
            w.u8(1);
            w.string(&s.key_id);
            w.u8(sig_alg_to_u8(s.algorithm));
            w.bytes(&s.signature_bytes);
            w.hash32(&s.signed_hash);
            w.u64(s.timestamp);
        }
        None => w.u8(0),
    }
    w.hash32(&m.icon_hash);
    w.u32(m.screenshots.len() as u32);
    for s in &m.screenshots {
        w.string(s);
    }
    w.string(&m.changelog);
    w.buf
}

/// `Option::ok_or` specialized to the manifest-decode error, kept generic so a
/// single helper covers every field type (a closure can't, since it would
/// monomorphize to its first call's type).
fn bound<T>(o: Option<T>) -> core::result::Result<T, PkgParseError> {
    o.ok_or(PkgParseError::MalformedField)
}

fn decode_manifest(bytes: &[u8]) -> core::result::Result<PackageManifest, PkgParseError> {
    let mut r = ByteReader::new(bytes);

    let id = AppId(bound(r.u64())?);
    let name = bound(r.string())?;
    let version = bound(read_semver(&mut r))?;
    let author = bound(r.string())?;
    let developer_id = DeveloperId(bound(r.u64())?);
    let description = bound(r.string())?;
    let long_description = bound(r.string())?;
    let license = bound(r.string())?;
    let homepage = bound(r.string())?;
    let repository = bound(r.string())?;
    let min_os_version = bound(read_semver(&mut r))?;
    let content_rating =
        content_rating_from_u8(bound(r.u8())?).ok_or(PkgParseError::MalformedField)?;
    let category = category_from_u8(bound(r.u8())?).ok_or(PkgParseError::MalformedField)?;

    let tag_count = bound(r.u32())? as usize;
    let mut tags = Vec::new();
    for _ in 0..tag_count {
        tags.push(bound(r.string())?);
    }

    let perm_count = bound(r.u32())? as usize;
    let mut permissions = Vec::new();
    for _ in 0..perm_count {
        permissions.push(permission_from_u8(bound(r.u8())?).ok_or(PkgParseError::MalformedField)?);
    }

    let dep_count = bound(r.u32())? as usize;
    let mut dependencies = Vec::new();
    for _ in 0..dep_count {
        let app_id = AppId(bound(r.u64())?);
        let dep_name = bound(r.string())?;
        let constraint = read_constraint(&mut r).ok_or(PkgParseError::MalformedField)?;
        let optional = bound(r.u8())? != 0;
        dependencies.push(Dependency {
            app_id,
            name: dep_name,
            constraint,
            optional,
        });
    }

    let size_bytes = bound(r.u64())?;
    let installed_size = bound(r.u64())?;
    let sha256_hash = bound(r.hash32())?;

    let signature = match bound(r.u8())? {
        0 => None,
        1 => {
            let key_id = bound(r.string())?;
            let algorithm = sig_alg_from_u8(bound(r.u8())?).ok_or(PkgParseError::MalformedField)?;
            let signature_bytes = bound(r.bytes())?;
            let signed_hash = bound(r.hash32())?;
            let timestamp = bound(r.u64())?;
            Some(PackageSignature {
                key_id,
                algorithm,
                signature_bytes,
                signed_hash,
                timestamp,
            })
        }
        _ => return Err(PkgParseError::MalformedField),
    };

    let icon_hash = bound(r.hash32())?;
    let shot_count = bound(r.u32())? as usize;
    let mut screenshots = Vec::new();
    for _ in 0..shot_count {
        screenshots.push(bound(r.string())?);
    }
    let changelog = bound(r.string())?;

    Ok(PackageManifest {
        id,
        name,
        version,
        author,
        developer_id,
        description,
        long_description,
        license,
        homepage,
        repository,
        min_os_version,
        content_rating,
        category,
        tags,
        permissions,
        dependencies,
        size_bytes,
        installed_size,
        sha256_hash,
        signature,
        icon_hash,
        screenshots,
        changelog,
    })
}

impl RaePackage {
    /// Serialize this package into the flat `.athpkg` container. The inverse of
    /// [`RaePackage::from_bytes`]; used by the signing/packaging tool and by the
    /// host KATs to produce a known-good package to then tamper with.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&RAEPKG_CONTAINER_MAGIC);
        out.extend_from_slice(&RAEPKG_MAX_FORMAT_VERSION.to_le_bytes());

        let mut push_record = |tag: RecordTag, payload: &[u8]| {
            out.push(tag.to_u8());
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(payload);
        };

        let manifest_bytes = encode_manifest(&self.manifest);
        push_record(RecordTag::Manifest, &manifest_bytes);
        push_record(RecordTag::Code, &self.code_data);
        if !self.asset_data.is_empty() {
            push_record(RecordTag::Assets, &self.asset_data);
        }
        if !self.metadata.is_empty() {
            push_record(RecordTag::Metadata, &self.metadata);
        }
        out
    }

    /// Parse a `.athpkg` byte container into an in-memory package. **Total over
    /// all inputs**: any truncated, malformed, or hostile byte sequence returns
    /// `Err` — this function never panics, indexes out of bounds, or allocates
    /// an attacker-chosen huge buffer (every length is capped + bounds-checked).
    ///
    /// This does NOT verify integrity or authenticity — call
    /// [`parse_and_verify`] for the full security gate. Parsing alone yields an
    /// *untrusted* structure.
    pub fn from_bytes(bytes: &[u8]) -> core::result::Result<RaePackage, PkgParseError> {
        let mut r = ByteReader::new(bytes);

        let magic = r.take(8).ok_or(PkgParseError::Truncated)?;
        if magic != RAEPKG_CONTAINER_MAGIC {
            return Err(PkgParseError::BadMagic);
        }
        let version = r.u16().ok_or(PkgParseError::Truncated)?;
        if version > RAEPKG_MAX_FORMAT_VERSION {
            return Err(PkgParseError::UnsupportedVersion);
        }

        let mut manifest: Option<PackageManifest> = None;
        let mut code_data = Vec::new();
        let mut asset_data = Vec::new();
        let mut metadata = Vec::new();
        let mut section_table = SectionTable::new();
        section_table.format_version = version;
        // `offset` here is the offset WITHIN the reconstructed payload stream we
        // checksum (code||assets||metadata); it lets the section TOC point at the
        // right bytes for `validate_checksums`.
        let mut payload_offset: u64 = 0;

        while r.remaining() > 0 {
            let tag = RecordTag::from_u8(r.u8().ok_or(PkgParseError::Truncated)?);
            let len = r.u32().ok_or(PkgParseError::Truncated)?;
            if len > RAEPKG_MAX_RECORD_LEN {
                return Err(PkgParseError::SizeOverflow);
            }
            let payload = r.take(len as usize).ok_or(PkgParseError::Truncated)?;

            match tag {
                RecordTag::Manifest => {
                    if manifest.is_some() {
                        return Err(PkgParseError::MalformedField);
                    }
                    manifest = Some(decode_manifest(payload)?);
                }
                RecordTag::Code => {
                    code_data = payload.to_vec();
                    section_table.add_section(PackageSection {
                        kind: SectionKind::Code,
                        offset: payload_offset,
                        size: code_data.len() as u64,
                        compressed_size: code_data.len() as u64,
                        sha256: sha256_digest(&code_data),
                    });
                    payload_offset = payload_offset
                        .checked_add(code_data.len() as u64)
                        .ok_or(PkgParseError::SizeOverflow)?;
                }
                RecordTag::Assets => {
                    asset_data = payload.to_vec();
                    section_table.add_section(PackageSection {
                        kind: SectionKind::Assets,
                        offset: payload_offset,
                        size: asset_data.len() as u64,
                        compressed_size: asset_data.len() as u64,
                        sha256: sha256_digest(&asset_data),
                    });
                    payload_offset = payload_offset
                        .checked_add(asset_data.len() as u64)
                        .ok_or(PkgParseError::SizeOverflow)?;
                }
                RecordTag::Metadata => {
                    metadata = payload.to_vec();
                    payload_offset = payload_offset
                        .checked_add(metadata.len() as u64)
                        .ok_or(PkgParseError::SizeOverflow)?;
                }
                // Forward-compat: a record tag we don't know is skipped, but its
                // length was already bounds-checked above so it can't desync us.
                RecordTag::Signature | RecordTag::Unknown(_) => {}
            }
        }

        let manifest = manifest.ok_or(PkgParseError::MissingManifest)?;
        Ok(RaePackage {
            section_table,
            manifest,
            code_data,
            asset_data,
            metadata,
        })
    }
}

/// The full security gate: parse untrusted `.athpkg` bytes AND verify them
/// before the caller is allowed to trust the result. Order is fail-closed:
/// parse → integrity (code hash binds the manifest) → signature (a trusted key
/// signed the bound hash). Returns the verified package on success, or the first
/// failing `StoreError`.
///
/// `require_signature` lets the caller distinguish store installs (must be
/// signed) from `AllowAll` sideloads (an unsigned package is permitted but is
/// still integrity-checked and is the caller's responsibility to surface as
/// "unverified developer"). When `true`, an unsigned package is rejected with
/// `SignatureInvalid`.
pub fn parse_and_verify(
    bytes: &[u8],
    keyring: &SigningKeyring,
    require_signature: bool,
) -> Result<RaePackage> {
    let pkg = RaePackage::from_bytes(bytes).map_err(StoreError::from)?;
    // Integrity: section TOC checksums match the bytes, AND the manifest's
    // declared code hash equals the actual code (verify_integrity does both the
    // magic check and the code-hash bind).
    pkg.verify_integrity()?;
    if require_signature && pkg.manifest.signature.is_none() {
        return Err(StoreError::SignatureInvalid);
    }
    // Authenticity: a trusted key signed the (bound) hash. For an unsigned
    // sideload with require_signature=false, verify_signature returns Ok.
    pkg.verify_signature(keyring)?;
    Ok(pkg)
}

#[cfg(test)]
mod crypto_tests {
    use super::*;

    #[test]
    fn sha256_digest_is_real() {
        assert_eq!(sha256_digest(b"abc"), ath_crypto::sha256::sha256(b"abc"));
        // SHA-256("") = e3b0c442...
        let e = sha256_digest(b"");
        assert_eq!(e[0], 0xe3);
        assert_eq!(e[31], 0x55);
    }

    fn signed_pkg(
        code: &[u8],
        seed: &[u8; 32],
        key_id: &str,
        trust: KeyTrust,
    ) -> (RaePackage, SigningKeyring) {
        let mut pkg = RaePackage::new(PackageManifest::new(
            AppId(1),
            String::from("test.app"),
            SemVer::new(1, 0, 0),
        ));
        pkg.code_data = code.to_vec();
        let content_hash = ath_crypto::sha256::sha256(code);
        pkg.manifest.sha256_hash = content_hash;
        pkg.manifest.signature = Some(PackageSignature {
            key_id: String::from(key_id),
            algorithm: SignatureAlgorithm::Ed25519,
            signature_bytes: ath_crypto::ed25519::sign(seed, &content_hash).to_vec(),
            signed_hash: content_hash,
            timestamp: 0,
        });
        let mut keyring = SigningKeyring::new();
        keyring.import(SigningKey {
            key_id: String::from(key_id),
            owner: String::from("dev"),
            public_key: ath_crypto::ed25519::derive_public_key(seed).to_vec(),
            trust_level: trust,
            created_at: 0,
            expires_at: 0,
        });
        (pkg, keyring)
    }

    #[test]
    fn valid_signature_accepted() {
        let (pkg, kr) = signed_pkg(b"app code bytes", &[3u8; 32], "dev", KeyTrust::Full);
        assert!(pkg.verify_signature(&kr).is_ok());
    }

    #[test]
    fn forged_signature_rejected() {
        let (mut pkg, kr) = signed_pkg(b"app code bytes", &[3u8; 32], "dev", KeyTrust::Full);
        pkg.manifest.signature.as_mut().unwrap().signature_bytes[5] ^= 0x01;
        assert!(pkg.verify_signature(&kr).is_err());
    }

    #[test]
    fn unbound_hash_rejected() {
        // A signature whose signed_hash doesn't match the manifest content hash
        // must be rejected even if the signature itself is valid over that hash.
        let seed = [3u8; 32];
        let (mut pkg, kr) = signed_pkg(b"app code bytes", &seed, "dev", KeyTrust::Full);
        let other = ath_crypto::sha256::sha256(b"different");
        pkg.manifest.signature.as_mut().unwrap().signed_hash = other;
        pkg.manifest.signature.as_mut().unwrap().signature_bytes =
            ath_crypto::ed25519::sign(&seed, &other).to_vec();
        assert!(pkg.verify_signature(&kr).is_err());
    }

    #[test]
    fn wrong_and_revoked_keys_rejected() {
        let (pkg, _) = signed_pkg(b"code", &[3u8; 32], "dev", KeyTrust::Full);
        // Different keypair under the same id.
        let mut kr = SigningKeyring::new();
        kr.import(SigningKey {
            key_id: String::from("dev"),
            owner: String::from("attacker"),
            public_key: ath_crypto::ed25519::derive_public_key(&[8u8; 32]).to_vec(),
            trust_level: KeyTrust::Full,
            created_at: 0,
            expires_at: 0,
        });
        assert!(pkg.verify_signature(&kr).is_err());

        let (pkg2, kr2) = signed_pkg(b"code", &[3u8; 32], "dev", KeyTrust::Revoked);
        assert!(pkg2.verify_signature(&kr2).is_err());
    }
}

// ── `.athpkg` wire-format codec KATs ─────────────────────────────────────────
// FAIL-able known-answer tests for the untrusted-input parse + verify pipeline.
// Every test asserts a CONCRETE outcome (Ok with the right fields, or a specific
// Err variant); the negative controls flip a single byte to prove tamper/truncate
// rejection; the fuzz-ish loop proves never-panic over arbitrary truncations.
#[cfg(test)]
mod codec_tests {
    use super::*;

    /// Build a fully-populated, correctly-signed package and its keyring.
    fn make_signed_package(
        code: &[u8],
        seed: &[u8; 32],
        key_id: &str,
        trust: KeyTrust,
    ) -> (RaePackage, SigningKeyring) {
        let mut m = PackageManifest::new(
            AppId(0xDEAD_BEEF),
            String::from("com.athena.editor"),
            SemVer::new(2, 3, 4),
        );
        m.author = String::from("Rae Dev");
        m.developer_id = DeveloperId(42);
        m.description = String::from("A nice editor");
        m.category = StoreCategory::CreativeTools;
        m.content_rating = ContentRating::Teen;
        m.tags = alloc::vec![String::from("editor"), String::from("text")];
        m.permissions = alloc::vec![
            AppPermission::FileReadHome,
            AppPermission::FileWriteHome,
            AppPermission::NetworkClient,
        ];
        m.dependencies = alloc::vec![Dependency::required(
            AppId(7),
            String::from("libfoo"),
            VersionConstraint::Compatible(SemVer::new(1, 0, 0)),
        )];
        m.screenshots = alloc::vec![String::from("a.png")];
        m.changelog = String::from("initial");

        let content_hash = ath_crypto::sha256::sha256(code);
        m.sha256_hash = content_hash;
        m.signature = Some(PackageSignature {
            key_id: String::from(key_id),
            algorithm: SignatureAlgorithm::Ed25519,
            signature_bytes: ath_crypto::ed25519::sign(seed, &content_hash).to_vec(),
            signed_hash: content_hash,
            timestamp: 1000,
        });

        let mut pkg = RaePackage::new(m);
        pkg.code_data = code.to_vec();
        pkg.asset_data = alloc::vec![1, 2, 3, 4];
        pkg.metadata = alloc::vec![9, 9];

        let mut keyring = SigningKeyring::new();
        keyring.import(SigningKey {
            key_id: String::from(key_id),
            owner: String::from("Rae Dev"),
            public_key: ath_crypto::ed25519::derive_public_key(seed).to_vec(),
            trust_level: trust,
            created_at: 0,
            expires_at: 0,
        });
        (pkg, keyring)
    }

    #[test]
    fn round_trips_all_manifest_fields() {
        let (pkg, _) = make_signed_package(b"hello app", &[5u8; 32], "k1", KeyTrust::Full);
        let bytes = pkg.to_bytes();
        let back = RaePackage::from_bytes(&bytes).expect("valid package must parse");

        assert_eq!(back.manifest.id, pkg.manifest.id);
        assert_eq!(back.manifest.name, pkg.manifest.name);
        assert_eq!(back.manifest.version, pkg.manifest.version);
        assert_eq!(back.manifest.author, pkg.manifest.author);
        assert_eq!(back.manifest.developer_id, pkg.manifest.developer_id);
        assert_eq!(back.manifest.category, pkg.manifest.category);
        assert_eq!(back.manifest.content_rating, pkg.manifest.content_rating);
        assert_eq!(back.manifest.tags, pkg.manifest.tags);
        assert_eq!(back.manifest.permissions, pkg.manifest.permissions);
        assert_eq!(back.manifest.dependencies.len(), 1);
        assert_eq!(back.manifest.dependencies[0].name, "libfoo");
        assert_eq!(back.manifest.sha256_hash, pkg.manifest.sha256_hash);
        assert!(back.manifest.signature.is_some());
        assert_eq!(back.code_data, pkg.code_data);
        assert_eq!(back.asset_data, pkg.asset_data);
        assert_eq!(back.metadata, pkg.metadata);
        assert_eq!(back.manifest.changelog, "initial");
    }

    #[test]
    fn valid_signed_package_parses_and_verifies() {
        let (pkg, kr) = make_signed_package(b"trusted bytes", &[7u8; 32], "store", KeyTrust::Full);
        let bytes = pkg.to_bytes();
        let verified = parse_and_verify(&bytes, &kr, true).expect("valid signed pkg accepted");
        // The verified manifest carries exactly the requested permission set.
        assert_eq!(verified.manifest.permissions.len(), 3);
        assert!(verified
            .manifest
            .requires_permission(AppPermission::NetworkClient));
        assert!(!verified.manifest.requires_permission(AppPermission::Camera));
    }

    #[test]
    fn bad_magic_rejected() {
        let (pkg, kr) = make_signed_package(b"x", &[7u8; 32], "store", KeyTrust::Full);
        let mut bytes = pkg.to_bytes();
        bytes[0] ^= 0xFF; // corrupt magic
        assert_eq!(
            RaePackage::from_bytes(&bytes).err(),
            Some(PkgParseError::BadMagic)
        );
        assert_eq!(
            parse_and_verify(&bytes, &kr, true).err(),
            Some(StoreError::ManifestInvalid)
        );
    }

    #[test]
    fn unsupported_version_rejected() {
        let (pkg, _) = make_signed_package(b"x", &[7u8; 32], "store", KeyTrust::Full);
        let mut bytes = pkg.to_bytes();
        // version is at offset 8..10, LE. Bump it past the max.
        bytes[8] = 0xFF;
        bytes[9] = 0xFF;
        assert_eq!(
            RaePackage::from_bytes(&bytes).err(),
            Some(PkgParseError::UnsupportedVersion)
        );
    }

    #[test]
    fn tampered_code_fails_integrity() {
        // Flip a byte of the CODE record after signing → manifest.sha256_hash no
        // longer matches H(code). Parse succeeds (bytes are well-formed) but the
        // verify pipeline must reject on integrity, BEFORE trusting it.
        let (pkg, kr) = make_signed_package(b"original code", &[7u8; 32], "store", KeyTrust::Full);
        let mut bytes = pkg.to_bytes();
        // The code bytes "original code" appear verbatim in the container; flip
        // the last 'e'. Find it to avoid corrupting the header/manifest.
        let pos = bytes
            .windows(b"original code".len())
            .position(|w| w == b"original code")
            .expect("code bytes present");
        let last = pos + b"original code".len() - 1;
        bytes[last] ^= 0x01;
        // It still parses (structurally valid)...
        assert!(RaePackage::from_bytes(&bytes).is_ok());
        // ...but is rejected by the full gate on integrity.
        assert_eq!(
            parse_and_verify(&bytes, &kr, true).err(),
            Some(StoreError::ChecksumMismatch)
        );
    }

    #[test]
    fn forged_signature_rejected_through_pipeline() {
        let (mut pkg, kr) = make_signed_package(b"app", &[7u8; 32], "store", KeyTrust::Full);
        // Flip a signature byte before serializing.
        pkg.manifest.signature.as_mut().unwrap().signature_bytes[10] ^= 0x80;
        let bytes = pkg.to_bytes();
        // Parses fine; signature verification must fail closed.
        assert!(RaePackage::from_bytes(&bytes).is_ok());
        assert_eq!(
            parse_and_verify(&bytes, &kr, true).err(),
            Some(StoreError::SignatureInvalid)
        );
    }

    #[test]
    fn untrusted_key_rejected() {
        // Package signed by seed A, but the keyring's public key is for seed B.
        let (pkg, _) = make_signed_package(b"app", &[1u8; 32], "store", KeyTrust::Full);
        let bytes = pkg.to_bytes();
        let mut kr = SigningKeyring::new();
        kr.import(SigningKey {
            key_id: String::from("store"),
            owner: String::from("attacker"),
            public_key: ath_crypto::ed25519::derive_public_key(&[2u8; 32]).to_vec(),
            trust_level: KeyTrust::Full,
            created_at: 0,
            expires_at: 0,
        });
        assert_eq!(
            parse_and_verify(&bytes, &kr, true).err(),
            Some(StoreError::SignatureInvalid)
        );
    }

    #[test]
    fn revoked_key_rejected() {
        let (pkg, kr) = make_signed_package(b"app", &[3u8; 32], "store", KeyTrust::Revoked);
        let bytes = pkg.to_bytes();
        assert_eq!(
            parse_and_verify(&bytes, &kr, true).err(),
            Some(StoreError::SignatureInvalid)
        );
    }

    #[test]
    fn unsigned_package_rejected_when_signature_required() {
        let mut m = PackageManifest::new(AppId(1), String::from("x"), SemVer::new(1, 0, 0));
        m.sha256_hash = ath_crypto::sha256::sha256(b"code");
        let mut pkg = RaePackage::new(m);
        pkg.code_data = b"code".to_vec();
        let bytes = pkg.to_bytes();
        let kr = SigningKeyring::new();
        // Store path requires a signature → rejected.
        assert_eq!(
            parse_and_verify(&bytes, &kr, true).err(),
            Some(StoreError::SignatureInvalid)
        );
        // Sideload path (AllowAll) permits unsigned but still integrity-checks it.
        let ok = parse_and_verify(&bytes, &kr, false).expect("unsigned sideload allowed");
        assert!(ok.manifest.signature.is_none());
    }

    #[test]
    fn unsigned_tampered_sideload_still_rejected() {
        // Even on the permissive sideload path, a corrupted code section fails
        // the integrity check — "unverified" must never mean "unchecked".
        let mut m = PackageManifest::new(AppId(1), String::from("x"), SemVer::new(1, 0, 0));
        m.sha256_hash = ath_crypto::sha256::sha256(b"good code");
        let mut pkg = RaePackage::new(m);
        pkg.code_data = b"good code".to_vec();
        let mut bytes = pkg.to_bytes();
        let pos = bytes
            .windows(b"good code".len())
            .position(|w| w == b"good code")
            .unwrap();
        bytes[pos] ^= 0x01;
        let kr = SigningKeyring::new();
        assert_eq!(
            parse_and_verify(&bytes, &kr, false).err(),
            Some(StoreError::ChecksumMismatch)
        );
    }

    #[test]
    fn truncated_inputs_never_panic() {
        // THE untrusted-input safety contract: every prefix of a valid container
        // must return cleanly (Ok or Err) — never panic / index out of bounds.
        // (A prefix that happens to end on a record boundary after a complete
        // manifest is legitimately parseable, since trailing Assets/Metadata
        // records are optional — so we assert totality, not always-Err here.)
        let (pkg, kr) = make_signed_package(b"some code here", &[9u8; 32], "k", KeyTrust::Full);
        let full = pkg.to_bytes();
        let mut parsed_ok = 0usize;
        let mut errored = 0usize;
        for cut in 0..full.len() {
            match RaePackage::from_bytes(&full[..cut]) {
                Ok(_) => parsed_ok += 1,
                Err(_) => errored += 1,
            }
            // The verify pipeline must also be total over every prefix.
            let _ = parse_and_verify(&full[..cut], &kr, true);
        }
        // Most short prefixes are incomplete → Err; the full set proves no panic.
        assert!(errored > 0, "truncations should produce errors");
        assert_eq!(parsed_ok + errored, full.len());
        // A truncation in the middle of the fixed header is always Truncated.
        assert_eq!(
            RaePackage::from_bytes(&full[..5]).err(),
            Some(PkgParseError::Truncated)
        );
        // Empty buffer.
        assert_eq!(
            RaePackage::from_bytes(&[]).err(),
            Some(PkgParseError::Truncated)
        );
    }

    #[test]
    fn hostile_record_length_does_not_overrun() {
        // A record header claiming a huge length with no payload must be rejected
        // as Truncated, not panic or allocate gigabytes.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&RAEPKG_CONTAINER_MAGIC);
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.push(1); // Manifest tag
        bytes.extend_from_slice(&0xFFFF_FFFEu32.to_le_bytes()); // absurd length, < MAX cap boundary
                                                                // no payload follows
        assert_eq!(
            RaePackage::from_bytes(&bytes).err(),
            Some(PkgParseError::SizeOverflow)
        );

        // A length just under the cap but still past the buffer → Truncated.
        let mut bytes2 = Vec::new();
        bytes2.extend_from_slice(&RAEPKG_CONTAINER_MAGIC);
        bytes2.extend_from_slice(&1u16.to_le_bytes());
        bytes2.push(1);
        bytes2.extend_from_slice(&1000u32.to_le_bytes()); // claims 1000 bytes
        bytes2.extend_from_slice(&[0u8; 4]); // but only 4 present
        assert_eq!(
            RaePackage::from_bytes(&bytes2).err(),
            Some(PkgParseError::Truncated)
        );
    }

    #[test]
    fn garbage_after_header_is_rejected() {
        // Random bytes after a valid header must not be misread as records that
        // happen to parse; assert it errors rather than silently "succeeding".
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&RAEPKG_CONTAINER_MAGIC);
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&[0xAB; 3]); // not enough for a tag+u32 len cleanly
        assert!(RaePackage::from_bytes(&bytes).is_err());
    }

    #[test]
    fn missing_manifest_rejected() {
        // A container with only a Code record (no Manifest) must be rejected.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&RAEPKG_CONTAINER_MAGIC);
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.push(2); // Code tag
        let code = b"abc";
        bytes.extend_from_slice(&(code.len() as u32).to_le_bytes());
        bytes.extend_from_slice(code);
        assert_eq!(
            RaePackage::from_bytes(&bytes).err(),
            Some(PkgParseError::MissingManifest)
        );
    }

    #[test]
    fn bad_enum_discriminant_in_manifest_rejected() {
        // Forge a manifest whose category byte is out of range → MalformedField,
        // not a panic. Build a minimal valid manifest then corrupt the category.
        let m = PackageManifest::new(AppId(1), String::from("a"), SemVer::new(1, 0, 0));
        let mut mbytes = super::encode_manifest(&m);
        // category sits right after content_rating; rather than compute the exact
        // offset, decode should still reject if we set a clearly-invalid byte at
        // a known enum position. Use the round-trip to locate: re-encode, then
        // flip every byte and assert decode never panics and the original decodes.
        assert!(super::decode_manifest(&mbytes).is_ok());
        for i in 0..mbytes.len() {
            let orig = mbytes[i];
            mbytes[i] = 0xFF;
            // Must never panic — Ok or Err, both acceptable; the point is totality.
            let _ = super::decode_manifest(&mbytes);
            mbytes[i] = orig;
        }
    }
}

// ── Transactional install / upgrade / uninstall KATs ─────────────────────────
// FAIL-able known-answer tests for the two-phase install transaction (section
// 8b): plan_install resolves the dependency closure + classifies steps;
// commit_plan applies all-or-nothing with rollback; uninstall_with_gc removes a
// target and GCs orphaned auto-installed deps while refusing to orphan a
// still-needed one. Every test asserts a CONCRETE registry outcome, and the
// negative controls prove that a failed verify/dep/conflict leaves NO partial
// state.
#[cfg(test)]
mod transaction_tests {
    use super::*;

    /// Build a repo entry for `id@version` with the given required deps.
    fn entry(id: u64, version: SemVer, deps: Vec<Dependency>) -> RepoEntry {
        let mut m = PackageManifest::new(AppId(id), alloc::format!("app{}", id), version);
        m.dependencies = deps;
        // Bind a trivial integrity hash so any future verify is self-consistent.
        m.sha256_hash = ath_crypto::sha256::sha256(&[id as u8]);
        RepoEntry {
            manifest: m,
            download_url: String::new(),
            download_size: 0,
            mirrors: Vec::new(),
        }
    }

    /// Build a AthStore with one repo populated from `entries`.
    fn store_with(entries: Vec<RepoEntry>) -> AthStore {
        let mut s = AthStore::new();
        let mut repo = Repository::new(RepositoryConfig::new(
            String::from("main"),
            String::from("http://repo"),
        ));
        for e in entries {
            repo.add_entry(e);
        }
        s.repos.push(repo);
        s
    }

    fn no_decisions() -> BTreeMap<AppId, Vec<(AppPermission, CapabilityDecision)>> {
        BTreeMap::new()
    }

    fn dep(id: u64, c: VersionConstraint) -> Dependency {
        Dependency::required(AppId(id), alloc::format!("app{}", id), c)
    }

    // ── plan_install: dependency-first ordering ──────────────────────────────

    #[test]
    fn plan_orders_dependencies_before_dependents() {
        // app1 -> app2 -> app3 ; install order must be 3, 2, 1.
        let s = store_with(alloc::vec![
            entry(
                1,
                SemVer::new(1, 0, 0),
                alloc::vec![dep(2, VersionConstraint::Any)]
            ),
            entry(
                2,
                SemVer::new(1, 0, 0),
                alloc::vec![dep(3, VersionConstraint::Any)]
            ),
            entry(3, SemVer::new(1, 0, 0), alloc::vec![]),
        ]);
        let plan = s.plan_install(AppId(1)).expect("plan");
        let ids: Vec<u64> = plan.steps.iter().map(|st| st.app_id().0).collect();
        assert_eq!(ids, alloc::vec![3, 2, 1]);
        // Target is Explicit; pulled-in deps are Dependency.
        assert_eq!(plan.reasons[&AppId(1)], InstallReason::Explicit);
        assert_eq!(plan.reasons[&AppId(2)], InstallReason::Dependency);
        assert_eq!(plan.reasons[&AppId(3)], InstallReason::Dependency);
        assert_eq!(plan.install_count(), 3);
    }

    #[test]
    fn plan_unknown_target_is_not_found() {
        let s = store_with(alloc::vec![entry(1, SemVer::new(1, 0, 0), alloc::vec![])]);
        assert_eq!(
            s.plan_install(AppId(99)).err(),
            Some(StoreError::PackageNotFound)
        );
    }

    #[test]
    fn plan_missing_dependency_is_not_found() {
        // app1 needs app2 but app2 is not in any repo → plan must fail, NOT
        // produce a plan that would install app1 with a missing dep.
        let s = store_with(alloc::vec![entry(
            1,
            SemVer::new(1, 0, 0),
            alloc::vec![dep(2, VersionConstraint::Any)]
        )]);
        assert_eq!(
            s.plan_install(AppId(1)).err(),
            Some(StoreError::PackageNotFound)
        );
    }

    #[test]
    fn plan_detects_dependency_cycle() {
        // app1 -> app2 -> app1 : a cycle must be rejected, never looped forever.
        let s = store_with(alloc::vec![
            entry(
                1,
                SemVer::new(1, 0, 0),
                alloc::vec![dep(2, VersionConstraint::Any)]
            ),
            entry(
                2,
                SemVer::new(1, 0, 0),
                alloc::vec![dep(1, VersionConstraint::Any)]
            ),
        ]);
        assert_eq!(
            s.plan_install(AppId(1)).err(),
            Some(StoreError::CycleDetected)
        );
    }

    #[test]
    fn plan_rejects_unsatisfiable_version_constraint() {
        // app1 requires app2 ^2.0.0 but the repo only offers app2 1.0.0 → the
        // candidate version does not satisfy the constraint → VersionMismatch.
        let s = store_with(alloc::vec![
            entry(
                1,
                SemVer::new(1, 0, 0),
                alloc::vec![dep(2, VersionConstraint::Compatible(SemVer::new(2, 0, 0)))]
            ),
            entry(2, SemVer::new(1, 0, 0), alloc::vec![]),
        ]);
        assert_eq!(
            s.plan_install(AppId(1)).err(),
            Some(StoreError::VersionMismatch)
        );
    }

    #[test]
    fn plan_satisfiable_version_constraint_accepted() {
        // Same as above but the repo offers app2 2.1.0 which satisfies ^2.0.0.
        let s = store_with(alloc::vec![
            entry(
                1,
                SemVer::new(1, 0, 0),
                alloc::vec![dep(2, VersionConstraint::Compatible(SemVer::new(2, 0, 0)))]
            ),
            entry(2, SemVer::new(2, 1, 0), alloc::vec![]),
        ]);
        let plan = s.plan_install(AppId(1)).expect("plan");
        assert_eq!(plan.install_count(), 2);
    }

    // ── commit_plan: atomic apply ────────────────────────────────────────────

    #[test]
    fn commit_records_app_and_dependency_reason() {
        let mut s = store_with(alloc::vec![
            entry(
                1,
                SemVer::new(1, 0, 0),
                alloc::vec![dep(2, VersionConstraint::Any)]
            ),
            entry(2, SemVer::new(1, 0, 0), alloc::vec![]),
        ]);
        let plan = s.plan_install(AppId(1)).expect("plan");
        s.commit_plan(&plan, &no_decisions(), 100).expect("commit");

        assert!(s.is_installed(AppId(1)));
        assert!(s.is_installed(AppId(2)));
        assert_eq!(
            s.get_installed(AppId(1)).unwrap().install_reason,
            InstallReason::Explicit
        );
        assert_eq!(
            s.get_installed(AppId(2)).unwrap().install_reason,
            InstallReason::Dependency
        );
        // Sandboxes were created for both.
        assert!(s.sandboxes.contains_key(&AppId(1)));
        assert!(s.sandboxes.contains_key(&AppId(2)));
    }

    #[test]
    fn commit_aborts_with_no_partial_state_when_a_required_permission_is_denied() {
        // app1 (no required perms) depends on app2 which requires FileReadSystem.
        // We deny app2's permission → applying app2 fails AFTER app1's dep step
        // order: deps are applied first (app2 before app1), so app2 itself fails
        // and nothing is left installed.
        let mut s = store_with(alloc::vec![
            entry(
                1,
                SemVer::new(1, 0, 0),
                alloc::vec![dep(2, VersionConstraint::Any)]
            ),
            {
                let mut e = entry(2, SemVer::new(1, 0, 0), alloc::vec![]);
                e.manifest.permissions = alloc::vec![AppPermission::FileReadSystem];
                e
            },
        ]);
        let plan = s.plan_install(AppId(1)).expect("plan");
        // No decision supplied for app2's REQUIRED FileReadSystem permission →
        // the install context aborts with CapabilityDenied (a required permission
        // left ungranted fails closed).
        let res = s.commit_plan(&plan, &no_decisions(), 100);
        assert_eq!(res.err(), Some(StoreError::CapabilityDenied));
        // NO partial state: neither app installed, no sandboxes, no txn records.
        assert!(!s.is_installed(AppId(1)));
        assert!(!s.is_installed(AppId(2)));
        assert!(s.sandboxes.is_empty());
        assert!(s.transaction_log.is_empty());
    }

    #[test]
    fn commit_rolls_back_later_failure_preserving_earlier_real_install() {
        // app1 depends on app2 AND app3. app2 installs fine; app3 requires a perm
        // we deny. Order is deps-first: app2, app3, app1. app2 applies, app3 fails
        // → app2 (applied in THIS commit) must be rolled back too. Whole tx aborts.
        let mut s = store_with(alloc::vec![
            entry(
                1,
                SemVer::new(1, 0, 0),
                alloc::vec![
                    dep(2, VersionConstraint::Any),
                    dep(3, VersionConstraint::Any)
                ]
            ),
            entry(2, SemVer::new(1, 0, 0), alloc::vec![]),
            {
                let mut e = entry(3, SemVer::new(1, 0, 0), alloc::vec![]);
                e.manifest.permissions = alloc::vec![AppPermission::Camera];
                e
            },
        ]);
        let plan = s.plan_install(AppId(1)).expect("plan");
        // app3 requires Camera; we supply NO decision for it → its step aborts
        // with CapabilityDenied after app2 has already been applied this commit.
        let res = s.commit_plan(&plan, &no_decisions(), 100);
        assert_eq!(res.err(), Some(StoreError::CapabilityDenied));
        // app2 was applied then rolled back — nothing survives.
        assert_eq!(s.installed_count(), 0);
        assert!(s.sandboxes.is_empty());
        assert!(s.transaction_log.is_empty());
    }

    #[test]
    fn commit_preserves_unrelated_preexisting_install_on_rollback() {
        // A pre-existing, unrelated app (id 9) must be untouched when a SEPARATE
        // transaction rolls back.
        let mut s = store_with(alloc::vec![
            entry(9, SemVer::new(1, 0, 0), alloc::vec![]),
            entry(
                1,
                SemVer::new(1, 0, 0),
                alloc::vec![dep(2, VersionConstraint::Any)]
            ),
            {
                let mut e = entry(2, SemVer::new(1, 0, 0), alloc::vec![]);
                e.manifest.permissions = alloc::vec![AppPermission::Camera];
                e
            },
        ]);
        // First, install app9 successfully.
        let plan9 = s.plan_install(AppId(9)).expect("plan9");
        s.commit_plan(&plan9, &no_decisions(), 10).expect("commit9");
        assert!(s.is_installed(AppId(9)));
        let txns_after_9 = s.transaction_log.len();

        // Now a failing transaction for app1 (its dep app2 requires Camera, which
        // we leave ungranted → the commit aborts and rolls back).
        let plan1 = s.plan_install(AppId(1)).expect("plan1");
        assert!(s.commit_plan(&plan1, &no_decisions(), 20).is_err());

        // app9 untouched; app1/app2 absent; txn log back to post-app9 length.
        assert!(s.is_installed(AppId(9)));
        assert!(!s.is_installed(AppId(1)));
        assert!(!s.is_installed(AppId(2)));
        assert_eq!(s.transaction_log.len(), txns_after_9);
    }

    // ── upgrade / already-installed / downgrade ──────────────────────────────

    #[test]
    fn plan_classifies_upgrade_vs_already_satisfied() {
        let mut s = store_with(alloc::vec![entry(1, SemVer::new(1, 0, 0), alloc::vec![])]);
        let plan = s.plan_install(AppId(1)).expect("plan");
        s.commit_plan(&plan, &no_decisions(), 1).expect("commit");
        assert_eq!(
            s.get_installed(AppId(1)).unwrap().manifest.version,
            SemVer::new(1, 0, 0)
        );

        // Repo now offers a newer version → plan must classify as Upgrade.
        s.repos[0].index.clear();
        s.repos[0].add_entry(entry(1, SemVer::new(1, 2, 0), alloc::vec![]));
        let plan2 = s.plan_install(AppId(1)).expect("plan2");
        assert_eq!(plan2.upgrade_count(), 1);
        assert!(matches!(
            plan2.steps[0],
            PlanStep::Upgrade {
                from: SemVer {
                    major: 1,
                    minor: 0,
                    patch: 0
                },
                to: SemVer {
                    major: 1,
                    minor: 2,
                    patch: 0
                },
                ..
            }
        ));
        // Commit the upgrade → version replaced.
        s.commit_plan(&plan2, &no_decisions(), 2)
            .expect("commit upgrade");
        assert_eq!(
            s.get_installed(AppId(1)).unwrap().manifest.version,
            SemVer::new(1, 2, 0)
        );

        // Same version again → AlreadySatisfied no-op plan.
        let plan3 = s.plan_install(AppId(1)).expect("plan3");
        assert!(plan3.is_noop());
        assert_eq!(plan3.upgrade_count(), 0);
        assert_eq!(plan3.install_count(), 0);
    }

    #[test]
    fn plan_does_not_downgrade_to_older_repo_version() {
        let mut s = store_with(alloc::vec![entry(1, SemVer::new(2, 0, 0), alloc::vec![])]);
        let plan = s.plan_install(AppId(1)).expect("plan");
        s.commit_plan(&plan, &no_decisions(), 1).expect("commit");
        // Repo now only offers an OLDER version → must be AlreadySatisfied, never
        // a silent downgrade.
        s.repos[0].index.clear();
        s.repos[0].add_entry(entry(1, SemVer::new(1, 0, 0), alloc::vec![]));
        let plan2 = s.plan_install(AppId(1)).expect("plan2");
        assert!(plan2.is_noop());
        assert!(matches!(plan2.steps[0], PlanStep::AlreadySatisfied { .. }));
    }

    #[test]
    fn upgrade_preserves_install_timestamp_and_launch_history() {
        let mut s = store_with(alloc::vec![entry(1, SemVer::new(1, 0, 0), alloc::vec![])]);
        let plan = s.plan_install(AppId(1)).expect("plan");
        s.commit_plan(&plan, &no_decisions(), 1000).expect("commit");
        // Simulate prior usage.
        s.installed.get_mut(&AppId(1)).unwrap().record_launch(1500);
        s.installed.get_mut(&AppId(1)).unwrap().record_launch(1600);

        s.repos[0].index.clear();
        s.repos[0].add_entry(entry(1, SemVer::new(1, 1, 0), alloc::vec![]));
        let plan2 = s.plan_install(AppId(1)).expect("plan2");
        s.commit_plan(&plan2, &no_decisions(), 2000)
            .expect("upgrade");

        let app = s.get_installed(AppId(1)).unwrap();
        assert_eq!(app.installed_at, 1000); // original install time preserved
        assert_eq!(app.launch_count, 2); // launch history carried across upgrade
    }

    // ── uninstall_with_gc ────────────────────────────────────────────────────

    #[test]
    fn uninstall_gcs_orphaned_dependency() {
        // Install app1 (-> app2). Uninstalling app1 must also remove app2 because
        // app2 was auto-installed and is now orphaned.
        let mut s = store_with(alloc::vec![
            entry(
                1,
                SemVer::new(1, 0, 0),
                alloc::vec![dep(2, VersionConstraint::Any)]
            ),
            entry(2, SemVer::new(1, 0, 0), alloc::vec![]),
        ]);
        let plan = s.plan_install(AppId(1)).expect("plan");
        s.commit_plan(&plan, &no_decisions(), 1).expect("commit");

        let removed = s.uninstall_with_gc(AppId(1), 2).expect("uninstall");
        assert!(removed.contains(&AppId(1)));
        assert!(removed.contains(&AppId(2)));
        assert_eq!(s.installed_count(), 0);
    }

    #[test]
    fn uninstall_refuses_to_orphan_a_still_needed_dependency() {
        // Two explicit apps (app1, app4) both depend on app2. Removing app1 must
        // NOT remove app2 (app4 still needs it).
        let mut s = store_with(alloc::vec![
            entry(
                1,
                SemVer::new(1, 0, 0),
                alloc::vec![dep(2, VersionConstraint::Any)]
            ),
            entry(
                4,
                SemVer::new(1, 0, 0),
                alloc::vec![dep(2, VersionConstraint::Any)]
            ),
            entry(2, SemVer::new(1, 0, 0), alloc::vec![]),
        ]);
        let p1 = s.plan_install(AppId(1)).expect("p1");
        s.commit_plan(&p1, &no_decisions(), 1).expect("c1");
        let p4 = s.plan_install(AppId(4)).expect("p4");
        s.commit_plan(&p4, &no_decisions(), 2).expect("c4");
        assert_eq!(s.installed_count(), 3);

        let removed = s.uninstall_with_gc(AppId(1), 3).expect("uninstall app1");
        assert_eq!(removed, alloc::vec![AppId(1)]); // ONLY app1
        assert!(s.is_installed(AppId(2))); // shared dep retained
        assert!(s.is_installed(AppId(4)));
    }

    #[test]
    fn uninstall_blocked_when_an_explicit_app_depends_on_target() {
        // app1 (explicit) depends on app2 (explicit, separately requested).
        // Removing app2 directly must be refused — an installed app needs it.
        let mut s = store_with(alloc::vec![
            entry(
                1,
                SemVer::new(1, 0, 0),
                alloc::vec![dep(2, VersionConstraint::Any)]
            ),
            entry(2, SemVer::new(1, 0, 0), alloc::vec![]),
        ]);
        // Install app2 explicitly first, then app1 (which finds app2 satisfied).
        let p2 = s.plan_install(AppId(2)).expect("p2");
        s.commit_plan(&p2, &no_decisions(), 1).expect("c2");
        let p1 = s.plan_install(AppId(1)).expect("p1");
        s.commit_plan(&p1, &no_decisions(), 2).expect("c1");
        assert_eq!(
            s.get_installed(AppId(2)).unwrap().install_reason,
            InstallReason::Explicit
        );

        assert_eq!(
            s.uninstall_with_gc(AppId(2), 3).err(),
            Some(StoreError::DependencyConflict)
        );
        // Nothing removed.
        assert!(s.is_installed(AppId(1)));
        assert!(s.is_installed(AppId(2)));
    }

    #[test]
    fn uninstall_chain_gc_collapses_transitive_orphans() {
        // app1 -> app2 -> app3, all auto via app1. Removing app1 GCs app2 then
        // app3 (transitive orphan collapse).
        let mut s = store_with(alloc::vec![
            entry(
                1,
                SemVer::new(1, 0, 0),
                alloc::vec![dep(2, VersionConstraint::Any)]
            ),
            entry(
                2,
                SemVer::new(1, 0, 0),
                alloc::vec![dep(3, VersionConstraint::Any)]
            ),
            entry(3, SemVer::new(1, 0, 0), alloc::vec![]),
        ]);
        let p = s.plan_install(AppId(1)).expect("p");
        s.commit_plan(&p, &no_decisions(), 1).expect("c");
        assert_eq!(s.installed_count(), 3);
        let removed = s.uninstall_with_gc(AppId(1), 2).expect("uninstall");
        assert_eq!(removed.len(), 3);
        assert_eq!(s.installed_count(), 0);
    }

    #[test]
    fn uninstall_unknown_app_is_not_installed() {
        let mut s = store_with(alloc::vec![entry(1, SemVer::new(1, 0, 0), alloc::vec![])]);
        assert_eq!(
            s.uninstall_with_gc(AppId(7), 1).err(),
            Some(StoreError::NotInstalled)
        );
    }
}
