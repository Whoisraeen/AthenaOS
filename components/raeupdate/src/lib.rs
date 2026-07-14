//! RaeUpdate — system update manager for RaeenOS.
//!
//! Full-featured package management with dependency resolution, A/B partitioning,
//! rollback, delta updates, and automatic update scheduling.
//!
//! See `docs/components/raeupdate.md` for the design.
#![no_std]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Transactional, signature-gated, rollback-able delta update application —
/// the atomic A/B + one-click-rollback core of the RaeUpdate Concept pillar.
pub mod transactional;
pub use transactional::{
    deserialize_delta, serialize_delta, HealthCheck, SignedDeltaPayload, StagedImage,
    UpdateSession, UpdateState, UpdateTrustKey, VerifyError,
};

// ---------------------------------------------------------------------------
// 1. Core types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateError {
    NotFound,
    AlreadyInstalled,
    DependencyConflict,
    VersionConflict,
    DownloadFailed,
    ChecksumMismatch,
    SignatureInvalid,
    ExtractionFailed,
    ScriptFailed,
    FileConflict,
    InsufficientSpace,
    TransactionAborted,
    RollbackFailed,
    PartitionError,
    NetworkError,
    Unauthorized,
    CorruptedPackage,
    UnsatisfiedDependency,
    CyclicDependency,
    LowBattery,
    MeteredConnection,
}

pub type Result<T> = core::result::Result<T, UpdateError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransactionId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RepoId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SnapshotId(pub u64);

// ---------------------------------------------------------------------------
// 2. Versioning
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    pub epoch: u32,
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
    pub release: u16,
}

impl Version {
    pub const fn new(major: u16, minor: u16, patch: u16) -> Self {
        Self {
            epoch: 0,
            major,
            minor,
            patch,
            release: 0,
        }
    }

    pub const fn with_epoch(epoch: u32, major: u16, minor: u16, patch: u16, release: u16) -> Self {
        Self {
            epoch,
            major,
            minor,
            patch,
            release,
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.epoch
            .cmp(&other.epoch)
            .then(self.major.cmp(&other.major))
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
            .then(self.release.cmp(&other.release))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionConstraint {
    Exact(Version),
    GreaterEqual(Version),
    Greater(Version),
    LessEqual(Version),
    Less(Version),
    Range { min: Version, max: Version },
    Any,
}

impl VersionConstraint {
    pub fn satisfies(&self, version: &Version) -> bool {
        match self {
            Self::Exact(v) => version == v,
            Self::GreaterEqual(v) => version >= v,
            Self::Greater(v) => version > v,
            Self::LessEqual(v) => version <= v,
            Self::Less(v) => version < v,
            Self::Range { min, max } => version >= min && version <= max,
            Self::Any => true,
        }
    }
}

// ---------------------------------------------------------------------------
// 3. Repository management
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoType {
    Official,
    Community,
    ThirdParty,
    Local,
}

#[derive(Debug, Clone)]
pub struct Repository {
    pub id: RepoId,
    pub name: String,
    pub url: String,
    pub repo_type: RepoType,
    pub gpg_key_fingerprint: [u8; 20],
    pub priority: u32,
    pub enabled: bool,
    pub last_refreshed: u64,
    pub package_count: u64,
}

impl Repository {
    pub fn new(id: RepoId, name: String, url: String, repo_type: RepoType) -> Self {
        Self {
            id,
            name,
            url,
            repo_type,
            gpg_key_fingerprint: [0; 20],
            priority: 100,
            enabled: true,
            last_refreshed: 0,
            package_count: 0,
        }
    }
}

pub struct RepoManager {
    pub repos: BTreeMap<RepoId, Repository>,
    pub next_id: u32,
}

impl RepoManager {
    pub fn new() -> Self {
        Self {
            repos: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn add_repo(&mut self, name: String, url: String, repo_type: RepoType) -> RepoId {
        let id = RepoId(self.next_id);
        self.next_id += 1;
        self.repos
            .insert(id, Repository::new(id, name, url, repo_type));
        id
    }

    pub fn remove_repo(&mut self, id: RepoId) -> bool {
        self.repos.remove(&id).is_some()
    }

    pub fn enable_repo(&mut self, id: RepoId) -> bool {
        self.repos
            .get_mut(&id)
            .map(|r| {
                r.enabled = true;
            })
            .is_some()
    }

    pub fn disable_repo(&mut self, id: RepoId) -> bool {
        self.repos
            .get_mut(&id)
            .map(|r| {
                r.enabled = false;
            })
            .is_some()
    }

    pub fn set_priority(&mut self, id: RepoId, priority: u32) -> bool {
        self.repos
            .get_mut(&id)
            .map(|r| {
                r.priority = priority;
            })
            .is_some()
    }

    pub fn set_gpg_key(&mut self, id: RepoId, fingerprint: [u8; 20]) -> bool {
        self.repos
            .get_mut(&id)
            .map(|r| {
                r.gpg_key_fingerprint = fingerprint;
            })
            .is_some()
    }

    pub fn enabled_repos(&self) -> Vec<&Repository> {
        let mut repos: Vec<_> = self.repos.values().filter(|r| r.enabled).collect();
        repos.sort_by_key(|r| r.priority);
        repos
    }

    pub fn mark_refreshed(&mut self, id: RepoId, now: u64, pkg_count: u64) {
        if let Some(repo) = self.repos.get_mut(&id) {
            repo.last_refreshed = now;
            repo.package_count = pkg_count;
        }
    }
}

// ---------------------------------------------------------------------------
// 4. Package format
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    X86_64,
    Aarch64,
    Riscv64,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionType {
    None,
    Zstd,
    Lz4,
    Xz,
    Gzip,
}

#[derive(Debug, Clone)]
pub struct Dependency {
    pub package_name: String,
    pub constraint: VersionConstraint,
}

#[derive(Debug, Clone)]
pub struct PackageHeader {
    pub id: PackageId,
    pub name: String,
    pub version: Version,
    pub arch: Architecture,
    pub description: String,
    pub dependencies: Vec<Dependency>,
    pub conflicts: Vec<String>,
    pub provides: Vec<String>,
    pub replaces: Vec<String>,
    pub suggests: Vec<String>,
    pub recommends: Vec<String>,
    pub installed_size: u64,
    pub download_size: u64,
    pub sha256: [u8; 32],
    pub sha512: [u8; 64],
    pub gpg_signature: Vec<u8>,
    pub compression: CompressionType,
    pub pre_install_script: Option<String>,
    pub post_install_script: Option<String>,
    pub pre_remove_script: Option<String>,
    pub post_remove_script: Option<String>,
    pub repo_id: RepoId,
    pub source_url: String,
}

#[derive(Debug, Clone)]
pub struct DeltaPackage {
    pub from_version: Version,
    pub to_version: Version,
    pub package_name: String,
    pub delta_size: u64,
    pub full_size: u64,
    pub sha256: [u8; 32],
    pub patch_data: Vec<u8>,
}

impl DeltaPackage {
    pub fn savings_percent(&self) -> u8 {
        if self.full_size == 0 {
            return 0;
        }
        ((self.full_size.saturating_sub(self.delta_size) as u128 * 100) / self.full_size as u128)
            as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageState {
    Available,
    Downloading,
    Downloaded,
    Installing,
    Installed,
    Upgrading,
    Removing,
    Removed,
    Broken,
    OnHold,
}

#[derive(Debug, Clone)]
pub struct InstalledPackage {
    pub header: PackageHeader,
    pub state: PackageState,
    pub install_path: String,
    pub installed_at: u64,
    pub updated_at: u64,
    pub auto_installed: bool,
    pub config_files: Vec<String>,
    pub installed_files: Vec<String>,
}

// ---------------------------------------------------------------------------
// 5. Dependency resolver
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
// `name`/`version` carry the node's identity for diagnostics; the topological sort
// itself keys on `deps`/`visited`/`in_stack`/`order`.
#[allow(dead_code)]
struct DepNode {
    name: String,
    version: Version,
    deps: Vec<String>,
    visited: bool,
    in_stack: bool,
    order: u32,
}

pub struct DependencyResolver {
    pub available: BTreeMap<String, Vec<PackageHeader>>,
    pub installed: BTreeMap<String, InstalledPackage>,
    pub virtual_packages: BTreeMap<String, String>,
}

impl DependencyResolver {
    pub fn new() -> Self {
        Self {
            available: BTreeMap::new(),
            installed: BTreeMap::new(),
            virtual_packages: BTreeMap::new(),
        }
    }

    pub fn add_available(&mut self, header: PackageHeader) {
        self.available
            .entry(header.name.clone())
            .or_insert_with(Vec::new)
            .push(header);
    }

    pub fn register_installed(&mut self, pkg: InstalledPackage) {
        for provided in &pkg.header.provides {
            self.virtual_packages
                .insert(provided.clone(), pkg.header.name.clone());
        }
        self.installed.insert(pkg.header.name.clone(), pkg);
    }

    pub fn resolve_name<'a>(&'a self, name: &'a str) -> &'a str {
        self.virtual_packages
            .get(name)
            .map(|s| s.as_str())
            .unwrap_or(name)
    }

    pub fn best_candidate(&self, name: &str) -> Option<&PackageHeader> {
        let real_name = self.resolve_name(name);
        self.available
            .get(real_name)
            .and_then(|versions| versions.iter().max_by(|a, b| a.version.cmp(&b.version)))
    }

    pub fn find_version(
        &self,
        name: &str,
        constraint: &VersionConstraint,
    ) -> Option<&PackageHeader> {
        let real_name = self.resolve_name(name);
        self.available.get(real_name).and_then(|versions| {
            versions
                .iter()
                .filter(|h| constraint.satisfies(&h.version))
                .max_by(|a, b| a.version.cmp(&b.version))
        })
    }

    pub fn check_conflicts(&self, name: &str) -> Vec<String> {
        let mut conflicts = Vec::new();
        if let Some(header) = self.best_candidate(name) {
            for conflict in &header.conflicts {
                if self.installed.contains_key(conflict) {
                    conflicts.push(conflict.clone());
                }
            }
        }
        conflicts
    }

    pub fn is_satisfied(&self, dep: &Dependency) -> bool {
        let real_name = self.resolve_name(&dep.package_name);
        self.installed
            .get(real_name)
            .map_or(false, |pkg| dep.constraint.satisfies(&pkg.header.version))
    }

    pub fn compute_install_order(&self, names: &[&str]) -> Result<Vec<String>> {
        let mut nodes: BTreeMap<String, DepNode> = BTreeMap::new();
        let mut order_counter = 0u32;

        for name in names {
            self.collect_deps(*name, &mut nodes)?;
        }

        let mut result = Vec::new();
        let node_names: Vec<String> = nodes.keys().cloned().collect();

        for name in &node_names {
            if !nodes[name].visited {
                self.topo_visit(name, &mut nodes, &mut result, &mut order_counter)?;
            }
        }

        Ok(result)
    }

    fn collect_deps(&self, name: &str, nodes: &mut BTreeMap<String, DepNode>) -> Result<()> {
        let real_name = self.resolve_name(name);
        if nodes.contains_key(real_name) {
            return Ok(());
        }

        let header = self
            .best_candidate(real_name)
            .ok_or(UpdateError::NotFound)?;
        let dep_names: Vec<String> = header
            .dependencies
            .iter()
            .map(|d| self.resolve_name(&d.package_name).into())
            .collect();

        nodes.insert(
            real_name.into(),
            DepNode {
                name: real_name.into(),
                version: header.version,
                deps: dep_names.clone(),
                visited: false,
                in_stack: false,
                order: 0,
            },
        );

        for dep_name in &dep_names {
            if !self.installed.contains_key(dep_name.as_str()) {
                self.collect_deps(dep_name, nodes)?;
            }
        }

        Ok(())
    }

    fn topo_visit(
        &self,
        name: &str,
        nodes: &mut BTreeMap<String, DepNode>,
        result: &mut Vec<String>,
        counter: &mut u32,
    ) -> Result<()> {
        if let Some(node) = nodes.get(name) {
            if node.in_stack {
                return Err(UpdateError::CyclicDependency);
            }
            if node.visited {
                return Ok(());
            }
        } else {
            return Ok(());
        }

        nodes.get_mut(name).unwrap().in_stack = true;
        let deps: Vec<String> = nodes.get(name).unwrap().deps.clone();

        for dep in &deps {
            self.topo_visit(dep, nodes, result, counter)?;
        }

        let node = nodes.get_mut(name).unwrap();
        node.in_stack = false;
        node.visited = true;
        node.order = *counter;
        *counter += 1;
        result.push(name.into());
        Ok(())
    }

    pub fn find_unused(&self) -> Vec<String> {
        let mut needed: Vec<String> = Vec::new();
        for pkg in self.installed.values() {
            if !pkg.auto_installed {
                needed.push(pkg.header.name.clone());
                for dep in &pkg.header.dependencies {
                    let real = self.resolve_name(&dep.package_name);
                    if !needed.contains(&real.into()) {
                        needed.push(real.into());
                    }
                }
            }
        }

        self.installed
            .keys()
            .filter(|name| {
                self.installed
                    .get(name.as_str())
                    .map_or(false, |p| p.auto_installed)
                    && !needed.contains(name)
            })
            .cloned()
            .collect()
    }
}

// ---------------------------------------------------------------------------
// 6. Transaction system
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionOp {
    Install,
    Upgrade,
    Downgrade,
    Remove,
    Reinstall,
    AutoRemove,
}

#[derive(Debug, Clone)]
pub struct TransactionStep {
    pub op: TransactionOp,
    pub package_name: String,
    pub from_version: Option<Version>,
    pub to_version: Option<Version>,
    pub download_size: u64,
    pub installed_size_delta: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionState {
    Planning,
    Downloading,
    Applying,
    Completed,
    Failed,
    RolledBack,
}

#[derive(Debug, Clone)]
pub struct Transaction {
    pub id: TransactionId,
    pub steps: Vec<TransactionStep>,
    pub state: TransactionState,
    pub created: u64,
    pub started: u64,
    pub completed: u64,
    pub snapshot_id: Option<SnapshotId>,
    pub error: Option<UpdateError>,
    pub current_step: usize,
}

impl Transaction {
    pub fn new(id: TransactionId, now: u64) -> Self {
        Self {
            id,
            steps: Vec::new(),
            state: TransactionState::Planning,
            created: now,
            started: 0,
            completed: 0,
            snapshot_id: None,
            error: None,
            current_step: 0,
        }
    }

    pub fn add_step(&mut self, step: TransactionStep) {
        self.steps.push(step);
    }

    pub fn total_download_size(&self) -> u64 {
        self.steps.iter().map(|s| s.download_size).sum()
    }

    pub fn total_size_delta(&self) -> i64 {
        self.steps.iter().map(|s| s.installed_size_delta).sum()
    }

    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    pub fn progress_percent(&self) -> u8 {
        if self.steps.is_empty() {
            return 100;
        }
        ((self.current_step as u32 * 100) / self.steps.len() as u32) as u8
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            TransactionState::Completed | TransactionState::Failed | TransactionState::RolledBack
        )
    }

    pub fn advance(&mut self) -> bool {
        if self.current_step < self.steps.len() {
            self.current_step += 1;
            true
        } else {
            false
        }
    }
}

pub struct TransactionLog {
    pub transactions: Vec<Transaction>,
    pub max_history: usize,
}

impl TransactionLog {
    pub fn new(max_history: usize) -> Self {
        Self {
            transactions: Vec::new(),
            max_history,
        }
    }

    pub fn record(&mut self, txn: Transaction) {
        self.transactions.push(txn);
        if self.transactions.len() > self.max_history {
            self.transactions.remove(0);
        }
    }

    pub fn last_n(&self, n: usize) -> &[Transaction] {
        let start = self.transactions.len().saturating_sub(n);
        &self.transactions[start..]
    }

    pub fn find(&self, id: TransactionId) -> Option<&Transaction> {
        self.transactions.iter().find(|t| t.id == id)
    }

    pub fn failed_transactions(&self) -> Vec<&Transaction> {
        self.transactions
            .iter()
            .filter(|t| t.state == TransactionState::Failed)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// 7. Download manager
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    Queued,
    Active,
    Paused,
    Completed,
    Failed,
    Verifying,
}

#[derive(Debug, Clone)]
pub struct DownloadItem {
    pub package_name: String,
    pub url: String,
    pub mirror_urls: Vec<String>,
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    pub state: DownloadState,
    pub sha256_expected: [u8; 32],
    pub sha512_expected: [u8; 64],
    pub sha256_computed: [u8; 32],
    pub retries: u32,
    pub max_retries: u32,
    pub current_mirror: usize,
}

impl DownloadItem {
    pub fn progress_percent(&self) -> u8 {
        if self.total_bytes == 0 {
            return 100;
        }
        ((self.downloaded_bytes as u128 * 100) / self.total_bytes as u128) as u8
    }

    pub fn verify_checksum(&self) -> bool {
        self.sha256_computed == self.sha256_expected
    }

    pub fn next_mirror(&mut self) -> bool {
        if self.current_mirror + 1 < self.mirror_urls.len() {
            self.current_mirror += 1;
            true
        } else {
            false
        }
    }

    pub fn can_retry(&self) -> bool {
        self.retries < self.max_retries
    }
}

pub struct DownloadManager {
    pub queue: Vec<DownloadItem>,
    pub max_parallel: usize,
    pub total_downloaded: u64,
}

impl DownloadManager {
    pub fn new(max_parallel: usize) -> Self {
        Self {
            queue: Vec::new(),
            max_parallel,
            total_downloaded: 0,
        }
    }

    pub fn enqueue(
        &mut self,
        name: String,
        url: String,
        mirrors: Vec<String>,
        size: u64,
        sha256: [u8; 32],
        sha512: [u8; 64],
    ) {
        self.queue.push(DownloadItem {
            package_name: name,
            url,
            mirror_urls: mirrors,
            total_bytes: size,
            downloaded_bytes: 0,
            state: DownloadState::Queued,
            sha256_expected: sha256,
            sha512_expected: sha512,
            sha256_computed: [0; 32],
            retries: 0,
            max_retries: 3,
            current_mirror: 0,
        });
    }

    pub fn start_next(&mut self) -> Option<&str> {
        let active = self
            .queue
            .iter()
            .filter(|d| d.state == DownloadState::Active)
            .count();
        if active >= self.max_parallel {
            return None;
        }
        self.queue
            .iter_mut()
            .find(|d| d.state == DownloadState::Queued)
            .map(|d| {
                d.state = DownloadState::Active;
                d.package_name.as_str()
            })
    }

    pub fn report_progress(&mut self, name: &str, bytes: u64) {
        if let Some(item) = self.queue.iter_mut().find(|d| d.package_name == name) {
            item.downloaded_bytes = bytes;
        }
    }

    pub fn complete_download(&mut self, name: &str, computed_sha256: [u8; 32]) -> Result<()> {
        let item = self
            .queue
            .iter_mut()
            .find(|d| d.package_name == name)
            .ok_or(UpdateError::NotFound)?;
        item.sha256_computed = computed_sha256;
        item.state = DownloadState::Verifying;
        if item.verify_checksum() {
            item.state = DownloadState::Completed;
            self.total_downloaded += item.total_bytes;
            Ok(())
        } else {
            item.state = DownloadState::Failed;
            Err(UpdateError::ChecksumMismatch)
        }
    }

    pub fn retry_failed(&mut self, name: &str) -> bool {
        if let Some(item) = self
            .queue
            .iter_mut()
            .find(|d| d.package_name == name && d.state == DownloadState::Failed)
        {
            if item.can_retry() {
                item.retries += 1;
                item.downloaded_bytes = 0;
                item.next_mirror();
                item.state = DownloadState::Queued;
                return true;
            }
        }
        false
    }

    pub fn overall_progress(&self) -> u8 {
        let total: u64 = self.queue.iter().map(|d| d.total_bytes).sum();
        let done: u64 = self.queue.iter().map(|d| d.downloaded_bytes).sum();
        if total == 0 {
            return 100;
        }
        ((done as u128 * 100) / total as u128) as u8
    }

    pub fn pending_count(&self) -> usize {
        self.queue
            .iter()
            .filter(|d| !matches!(d.state, DownloadState::Completed | DownloadState::Failed))
            .count()
    }
}

// ---------------------------------------------------------------------------
// 8. Installation engine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigPolicy {
    KeepOld,
    ReplaceWithNew,
    MergePreferOld,
    MergePreferNew,
    Prompt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookPhase {
    PreInstall,
    PostInstall,
    PreRemove,
    PostRemove,
    PreUpgrade,
    PostUpgrade,
    Trigger,
}

#[derive(Debug, Clone)]
pub struct HookResult {
    pub phase: HookPhase,
    pub package_name: String,
    pub exit_code: i32,
    pub output: String,
    pub timestamp: u64,
}

pub struct InstallEngine {
    pub config_policy: ConfigPolicy,
    pub hook_results: Vec<HookResult>,
    pub pending_triggers: Vec<String>,
    pub file_conflicts: Vec<(String, String, String)>,
    pub protected_paths: Vec<String>,
}

impl InstallEngine {
    pub fn new() -> Self {
        Self {
            config_policy: ConfigPolicy::MergePreferOld,
            hook_results: Vec::new(),
            pending_triggers: Vec::new(),
            file_conflicts: Vec::new(),
            protected_paths: Vec::new(),
        }
    }

    pub fn run_hook(
        &mut self,
        phase: HookPhase,
        package_name: &str,
        _script: &str,
        now: u64,
    ) -> Result<()> {
        let result = HookResult {
            phase,
            package_name: package_name.into(),
            exit_code: 0,
            output: String::new(),
            timestamp: now,
        };
        let success = result.exit_code == 0;
        self.hook_results.push(result);
        if success {
            Ok(())
        } else {
            Err(UpdateError::ScriptFailed)
        }
    }

    pub fn check_file_conflicts(
        &mut self,
        package_name: &str,
        files: &[String],
        installed: &BTreeMap<String, InstalledPackage>,
    ) -> Vec<String> {
        let mut conflicts = Vec::new();
        for file in files {
            for (other_name, other_pkg) in installed {
                if other_name == package_name {
                    continue;
                }
                if other_pkg.installed_files.contains(file) {
                    conflicts.push(file.clone());
                    self.file_conflicts.push((
                        file.clone(),
                        package_name.into(),
                        other_name.clone(),
                    ));
                }
            }
        }
        conflicts
    }

    pub fn handle_config_file(&self, existing_modified: bool, new_differs: bool) -> ConfigPolicy {
        if !existing_modified {
            return ConfigPolicy::ReplaceWithNew;
        }
        if !new_differs {
            return ConfigPolicy::KeepOld;
        }
        self.config_policy
    }

    pub fn add_trigger(&mut self, trigger: String) {
        if !self.pending_triggers.contains(&trigger) {
            self.pending_triggers.push(trigger);
        }
    }

    pub fn process_triggers(&mut self, now: u64) -> Vec<HookResult> {
        let triggers: Vec<String> = self.pending_triggers.drain(..).collect();
        let mut results = Vec::new();
        for trigger in triggers {
            let result = HookResult {
                phase: HookPhase::Trigger,
                package_name: trigger,
                exit_code: 0,
                output: String::new(),
                timestamp: now,
            };
            results.push(result);
        }
        results
    }

    pub fn add_protected_path(&mut self, path: String) {
        if !self.protected_paths.contains(&path) {
            self.protected_paths.push(path);
        }
    }

    pub fn is_protected(&self, path: &str) -> bool {
        self.protected_paths
            .iter()
            .any(|p| path.starts_with(p.as_str()))
    }

    pub fn last_hook_results(&self, n: usize) -> &[HookResult] {
        let start = self.hook_results.len().saturating_sub(n);
        &self.hook_results[start..]
    }
}

// ---------------------------------------------------------------------------
// 9. Rollback / Snapshots
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub id: SnapshotId,
    pub transaction_id: TransactionId,
    pub timestamp: u64,
    pub description: String,
    pub package_states: BTreeMap<String, Version>,
    pub size_bytes: u64,
    pub bootable: bool,
}

pub struct SnapshotManager {
    pub snapshots: Vec<Snapshot>,
    pub max_snapshots: usize,
    pub next_id: u64,
}

impl SnapshotManager {
    pub fn new(max_snapshots: usize) -> Self {
        Self {
            snapshots: Vec::new(),
            max_snapshots,
            next_id: 1,
        }
    }

    pub fn create_snapshot(
        &mut self,
        txn_id: TransactionId,
        installed: &BTreeMap<String, InstalledPackage>,
        desc: String,
        now: u64,
    ) -> SnapshotId {
        let id = SnapshotId(self.next_id);
        self.next_id += 1;
        let states: BTreeMap<String, Version> = installed
            .iter()
            .map(|(name, pkg)| (name.clone(), pkg.header.version))
            .collect();
        self.snapshots.push(Snapshot {
            id,
            transaction_id: txn_id,
            timestamp: now,
            description: desc,
            package_states: states,
            size_bytes: 0,
            bootable: true,
        });
        if self.snapshots.len() > self.max_snapshots {
            self.snapshots.remove(0);
        }
        id
    }

    pub fn get_snapshot(&self, id: SnapshotId) -> Option<&Snapshot> {
        self.snapshots.iter().find(|s| s.id == id)
    }

    pub fn latest(&self) -> Option<&Snapshot> {
        self.snapshots.last()
    }

    pub fn list_snapshots(&self) -> &[Snapshot] {
        &self.snapshots
    }

    pub fn diff_packages(
        &self,
        from: SnapshotId,
        to: SnapshotId,
    ) -> Vec<(String, Option<Version>, Option<Version>)> {
        let from_snap = self.get_snapshot(from);
        let to_snap = self.get_snapshot(to);
        let (from_pkgs, to_pkgs) = match (from_snap, to_snap) {
            (Some(f), Some(t)) => (&f.package_states, &t.package_states),
            _ => return Vec::new(),
        };

        let mut diffs = Vec::new();
        for (name, from_ver) in from_pkgs {
            match to_pkgs.get(name) {
                Some(to_ver) if to_ver != from_ver => {
                    diffs.push((name.clone(), Some(*from_ver), Some(*to_ver)))
                }
                None => diffs.push((name.clone(), Some(*from_ver), None)),
                _ => {}
            }
        }
        for (name, to_ver) in to_pkgs {
            if !from_pkgs.contains_key(name) {
                diffs.push((name.clone(), None, Some(*to_ver)));
            }
        }
        diffs
    }

    pub fn delete_snapshot(&mut self, id: SnapshotId) -> bool {
        let len = self.snapshots.len();
        self.snapshots.retain(|s| s.id != id);
        self.snapshots.len() != len
    }
}

// ---------------------------------------------------------------------------
// 10. A/B partition scheme
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionSlot {
    A,
    B,
}

impl PartitionSlot {
    pub fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PartitionInfo {
    pub slot: PartitionSlot,
    pub version: Version,
    pub bootable: bool,
    pub successful: bool,
    pub boot_count: u32,
    pub max_boot_retries: u32,
    pub last_updated: u64,
}

pub struct AbPartitionManager {
    pub active: PartitionSlot,
    pub slot_a: PartitionInfo,
    pub slot_b: PartitionInfo,
    pub update_in_progress: bool,
}

impl AbPartitionManager {
    pub fn new() -> Self {
        let default_version = Version::new(0, 0, 1);
        Self {
            active: PartitionSlot::A,
            slot_a: PartitionInfo {
                slot: PartitionSlot::A,
                version: default_version,
                bootable: true,
                successful: true,
                boot_count: 0,
                max_boot_retries: 3,
                last_updated: 0,
            },
            slot_b: PartitionInfo {
                slot: PartitionSlot::B,
                version: default_version,
                bootable: false,
                successful: false,
                boot_count: 0,
                max_boot_retries: 3,
                last_updated: 0,
            },
            update_in_progress: false,
        }
    }

    pub fn active_slot(&self) -> &PartitionInfo {
        match self.active {
            PartitionSlot::A => &self.slot_a,
            PartitionSlot::B => &self.slot_b,
        }
    }

    pub fn standby_slot(&self) -> &PartitionInfo {
        match self.active {
            PartitionSlot::A => &self.slot_b,
            PartitionSlot::B => &self.slot_a,
        }
    }

    fn standby_slot_mut(&mut self) -> &mut PartitionInfo {
        match self.active {
            PartitionSlot::A => &mut self.slot_b,
            PartitionSlot::B => &mut self.slot_a,
        }
    }

    pub fn stage_update(&mut self, version: Version, now: u64) -> Result<()> {
        if self.update_in_progress {
            return Err(UpdateError::TransactionAborted);
        }
        let standby = self.standby_slot_mut();
        standby.version = version;
        standby.bootable = true;
        standby.successful = false;
        standby.boot_count = 0;
        standby.last_updated = now;
        self.update_in_progress = true;
        Ok(())
    }

    pub fn switch_active(&mut self) -> Result<()> {
        if !self.update_in_progress {
            return Err(UpdateError::PartitionError);
        }
        self.active = self.active.other();
        self.update_in_progress = false;
        Ok(())
    }

    pub fn mark_successful(&mut self) {
        match self.active {
            PartitionSlot::A => self.slot_a.successful = true,
            PartitionSlot::B => self.slot_b.successful = true,
        }
    }

    pub fn check_boot_fallback(&mut self) -> bool {
        let active = match self.active {
            PartitionSlot::A => &mut self.slot_a,
            PartitionSlot::B => &mut self.slot_b,
        };
        active.boot_count += 1;
        if !active.successful && active.boot_count > active.max_boot_retries {
            self.active = self.active.other();
            return true;
        }
        false
    }

    pub fn rollback(&mut self) -> Result<()> {
        let other = self.active.other();
        let other_info = match other {
            PartitionSlot::A => &self.slot_a,
            PartitionSlot::B => &self.slot_b,
        };
        if !other_info.bootable || !other_info.successful {
            return Err(UpdateError::RollbackFailed);
        }
        self.active = other;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 11. Automatic updates
// ---------------------------------------------------------------------------

/// Release/update channel. The channels are **nested** (Stable ⊆ Beta ⊆ Nightly):
/// a subscriber on a less-stable channel also receives everything from the more
/// stable ones, so a Nightly user gets Stable + Beta + Nightly builds, a Beta user
/// gets Stable + Beta, and a Stable user gets only Stable. `Ord` encodes the
/// stability order (Stable < Beta < Nightly).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UpdateChannel {
    Stable,
    Beta,
    Nightly,
}

impl UpdateChannel {
    /// Should a subscriber on `self` be offered an update published on `update`?
    /// True iff the update's channel is at least as stable as the subscription —
    /// i.e. `update <= self` in the stability order.
    pub fn accepts(self, update: UpdateChannel) -> bool {
        update <= self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoUpdatePolicy {
    Disabled,
    SecurityOnly,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateSeverity {
    Critical,
    Security,
    Important,
    Normal,
    Optional,
}

#[derive(Debug, Clone)]
pub struct AutoUpdateConfig {
    pub policy: AutoUpdatePolicy,
    pub check_interval_hours: u32,
    pub install_hour: u8,
    pub install_day: Option<u8>,
    pub allow_metered: bool,
    pub min_battery_percent: u8,
    pub auto_reboot: bool,
    pub reboot_delay_minutes: u32,
    pub defer_count: u32,
    pub max_defer: u32,
    /// Which release channel this machine is subscribed to (default `Stable`).
    pub channel: UpdateChannel,
}

impl AutoUpdateConfig {
    pub fn default_config() -> Self {
        Self {
            policy: AutoUpdatePolicy::SecurityOnly,
            check_interval_hours: 12,
            install_hour: 3,
            install_day: None,
            allow_metered: false,
            min_battery_percent: 30,
            auto_reboot: false,
            reboot_delay_minutes: 15,
            defer_count: 0,
            max_defer: 5,
            channel: UpdateChannel::Stable,
        }
    }

    pub fn can_defer(&self) -> bool {
        self.defer_count < self.max_defer
    }

    pub fn should_install(&self, severity: UpdateSeverity) -> bool {
        match self.policy {
            AutoUpdatePolicy::Disabled => false,
            AutoUpdatePolicy::SecurityOnly => matches!(
                severity,
                UpdateSeverity::Critical | UpdateSeverity::Security
            ),
            AutoUpdatePolicy::All => true,
        }
    }
}

// ---------------------------------------------------------------------------
// 12. Update notifications
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct UpdateNotification {
    pub package_name: String,
    pub from_version: Version,
    pub to_version: Version,
    pub severity: UpdateSeverity,
    pub changelog: String,
    pub download_size: u64,
    pub timestamp: u64,
    pub seen: bool,
}

pub struct NotificationManager {
    pub notifications: Vec<UpdateNotification>,
}

impl NotificationManager {
    pub fn new() -> Self {
        Self {
            notifications: Vec::new(),
        }
    }

    pub fn add(&mut self, notif: UpdateNotification) {
        self.notifications.push(notif);
    }

    pub fn unseen_count(&self) -> usize {
        self.notifications.iter().filter(|n| !n.seen).count()
    }

    pub fn security_count(&self) -> usize {
        self.notifications
            .iter()
            .filter(|n| {
                matches!(
                    n.severity,
                    UpdateSeverity::Critical | UpdateSeverity::Security
                )
            })
            .count()
    }

    pub fn mark_seen(&mut self, package: &str) {
        for n in &mut self.notifications {
            if n.package_name == package {
                n.seen = true;
            }
        }
    }

    pub fn mark_all_seen(&mut self) {
        for n in &mut self.notifications {
            n.seen = true;
        }
    }

    pub fn by_severity(&self, severity: UpdateSeverity) -> Vec<&UpdateNotification> {
        self.notifications
            .iter()
            .filter(|n| n.severity == severity)
            .collect()
    }

    pub fn clear(&mut self) {
        self.notifications.clear();
    }

    pub fn total_download_size(&self) -> u64 {
        self.notifications.iter().map(|n| n.download_size).sum()
    }
}

// ---------------------------------------------------------------------------
// 13. Kernel updates
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct KernelVersion {
    pub version: Version,
    pub install_path: String,
    pub initrd_path: String,
    pub bootloader_entry: String,
    pub installed_at: u64,
    pub is_running: bool,
    pub is_default: bool,
}

pub struct KernelManager {
    pub installed_kernels: Vec<KernelVersion>,
    pub max_kernels: usize,
}

impl KernelManager {
    pub fn new(max_kernels: usize) -> Self {
        Self {
            installed_kernels: Vec::new(),
            max_kernels,
        }
    }

    pub fn install_kernel(
        &mut self,
        version: Version,
        install_path: String,
        initrd_path: String,
        entry: String,
        now: u64,
    ) {
        self.installed_kernels.push(KernelVersion {
            version,
            install_path,
            initrd_path,
            bootloader_entry: entry,
            installed_at: now,
            is_running: false,
            is_default: false,
        });
        self.installed_kernels
            .sort_by(|a, b| b.version.cmp(&a.version));
        while self.installed_kernels.len() > self.max_kernels {
            if let Some(pos) = self
                .installed_kernels
                .iter()
                .rposition(|k| !k.is_running && !k.is_default)
            {
                self.installed_kernels.remove(pos);
            } else {
                break;
            }
        }
    }

    pub fn set_default(&mut self, version: &Version) -> bool {
        let mut found = false;
        for k in &mut self.installed_kernels {
            k.is_default = k.version == *version;
            if k.is_default {
                found = true;
            }
        }
        found
    }

    pub fn running_kernel(&self) -> Option<&KernelVersion> {
        self.installed_kernels.iter().find(|k| k.is_running)
    }

    pub fn default_kernel(&self) -> Option<&KernelVersion> {
        self.installed_kernels.iter().find(|k| k.is_default)
    }

    pub fn list_kernels(&self) -> &[KernelVersion] {
        &self.installed_kernels
    }

    pub fn remove_kernel(&mut self, version: &Version) -> Result<()> {
        let pos = self
            .installed_kernels
            .iter()
            .position(|k| k.version == *version)
            .ok_or(UpdateError::NotFound)?;
        let kernel = &self.installed_kernels[pos];
        if kernel.is_running {
            return Err(UpdateError::TransactionAborted);
        }
        self.installed_kernels.remove(pos);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 14. Delta updates
// ---------------------------------------------------------------------------

pub struct DeltaEngine {
    pub available_deltas: Vec<DeltaPackage>,
    pub min_savings_percent: u8,
}

impl DeltaEngine {
    pub fn new() -> Self {
        Self {
            available_deltas: Vec::new(),
            min_savings_percent: 20,
        }
    }

    pub fn add_delta(&mut self, delta: DeltaPackage) {
        self.available_deltas.push(delta);
    }

    pub fn find_delta(&self, package: &str, from: &Version, to: &Version) -> Option<&DeltaPackage> {
        self.available_deltas.iter().find(|d| {
            d.package_name == package
                && d.from_version == *from
                && d.to_version == *to
                && d.savings_percent() >= self.min_savings_percent
        })
    }

    pub fn best_delta_chain(
        &self,
        package: &str,
        from: &Version,
        to: &Version,
    ) -> Vec<&DeltaPackage> {
        let mut chain = Vec::new();
        let mut current = *from;
        while current < *to {
            let next = self
                .available_deltas
                .iter()
                .filter(|d| {
                    d.package_name == package && d.from_version == current && d.to_version <= *to
                })
                .max_by_key(|d| d.to_version);
            match next {
                Some(delta) => {
                    current = delta.to_version;
                    chain.push(delta);
                }
                None => break,
            }
        }
        if current == *to {
            chain
        } else {
            Vec::new()
        }
    }

    /// Apply a binary delta package to `base_data` to reconstruct the new image.
    ///
    /// `patch_data` is the serialized [`DeltaOp`](rae_diff::DeltaOp) stream (see
    /// [`crate::serialize_delta`]). This routes through the real `rae_diff`
    /// apply path — a `Copy` referencing bytes outside `base_data` (a forged or
    /// mismatched delta) is refused, never read out of bounds. The reconstructed
    /// image's SHA-256 is checked against the package's declared `sha256` so a
    /// delta that applies cleanly against the wrong base is still rejected.
    ///
    /// NOTE: this does NOT verify a publisher signature — that gate lives in
    /// [`crate::SignedDeltaPayload::reconstruct`], which callers MUST use for
    /// any untrusted/over-the-wire payload. This method is the post-verify
    /// reconstruction step only.
    pub fn apply_patch(&self, base_data: &[u8], delta: &DeltaPackage) -> Result<Vec<u8>> {
        if delta.patch_data.is_empty() {
            return Err(UpdateError::CorruptedPackage);
        }
        let ops = crate::transactional::deserialize_delta(&delta.patch_data)
            .ok_or(UpdateError::CorruptedPackage)?;
        let new_image =
            rae_diff::apply_delta(base_data, &ops).map_err(|_| UpdateError::CorruptedPackage)?;
        let actual = rae_hash::sha256(&new_image);
        if actual != delta.sha256 {
            return Err(UpdateError::ChecksumMismatch);
        }
        Ok(new_image)
    }

    pub fn total_savings(&self) -> u64 {
        self.available_deltas
            .iter()
            .map(|d| d.full_size.saturating_sub(d.delta_size))
            .sum()
    }
}

// ---------------------------------------------------------------------------
// 15. System upgrade
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradePhase {
    CompatibilityCheck,
    BackupConfig,
    DownloadPackages,
    ApplyUpdates,
    MigrateConfig,
    VerifySystem,
    Completed,
    Failed,
}

#[derive(Debug, Clone)]
pub struct SystemUpgrade {
    pub from_release: Version,
    pub to_release: Version,
    pub phase: UpgradePhase,
    pub packages_to_update: usize,
    pub packages_updated: usize,
    pub compatibility_issues: Vec<String>,
    pub started: u64,
}

impl SystemUpgrade {
    pub fn new(from: Version, to: Version, now: u64) -> Self {
        Self {
            from_release: from,
            to_release: to,
            phase: UpgradePhase::CompatibilityCheck,
            packages_to_update: 0,
            packages_updated: 0,
            compatibility_issues: Vec::new(),
            started: now,
        }
    }

    pub fn add_compat_issue(&mut self, issue: String) {
        self.compatibility_issues.push(issue);
    }

    pub fn can_proceed(&self) -> bool {
        self.compatibility_issues.is_empty()
    }

    pub fn advance_phase(&mut self) -> bool {
        self.phase = match self.phase {
            UpgradePhase::CompatibilityCheck => UpgradePhase::BackupConfig,
            UpgradePhase::BackupConfig => UpgradePhase::DownloadPackages,
            UpgradePhase::DownloadPackages => UpgradePhase::ApplyUpdates,
            UpgradePhase::ApplyUpdates => UpgradePhase::MigrateConfig,
            UpgradePhase::MigrateConfig => UpgradePhase::VerifySystem,
            UpgradePhase::VerifySystem => UpgradePhase::Completed,
            UpgradePhase::Completed | UpgradePhase::Failed => return false,
        };
        true
    }

    pub fn progress_percent(&self) -> u8 {
        if self.packages_to_update == 0 {
            return 0;
        }
        ((self.packages_updated as u32 * 100) / self.packages_to_update as u32) as u8
    }
}

// ---------------------------------------------------------------------------
// 16. Update manager (top-level)
// ---------------------------------------------------------------------------

pub struct UpdateManager {
    pub repo_manager: RepoManager,
    pub resolver: DependencyResolver,
    pub download_manager: DownloadManager,
    pub install_engine: InstallEngine,
    pub snapshot_manager: SnapshotManager,
    pub ab_partitions: AbPartitionManager,
    pub transaction_log: TransactionLog,
    pub auto_config: AutoUpdateConfig,
    pub notifications: NotificationManager,
    pub kernel_manager: KernelManager,
    pub delta_engine: DeltaEngine,
    pub current_upgrade: Option<SystemUpgrade>,
    pub next_txn_id: u64,
    pub initialized: bool,
}

impl UpdateManager {
    pub fn new() -> Self {
        Self {
            repo_manager: RepoManager::new(),
            resolver: DependencyResolver::new(),
            download_manager: DownloadManager::new(4),
            install_engine: InstallEngine::new(),
            snapshot_manager: SnapshotManager::new(10),
            ab_partitions: AbPartitionManager::new(),
            transaction_log: TransactionLog::new(100),
            auto_config: AutoUpdateConfig::default_config(),
            notifications: NotificationManager::new(),
            kernel_manager: KernelManager::new(3),
            delta_engine: DeltaEngine::new(),
            current_upgrade: None,
            next_txn_id: 1,
            initialized: false,
        }
    }

    pub fn begin_transaction(&mut self, now: u64) -> TransactionId {
        let id = TransactionId(self.next_txn_id);
        self.next_txn_id += 1;
        let txn = Transaction::new(id, now);
        self.transaction_log.record(txn);
        id
    }

    pub fn add_install(&mut self, txn_id: TransactionId, name: &str) -> Result<()> {
        let header = self
            .resolver
            .best_candidate(name)
            .ok_or(UpdateError::NotFound)?
            .clone();
        let conflicts = self.resolver.check_conflicts(name);
        if !conflicts.is_empty() {
            return Err(UpdateError::DependencyConflict);
        }
        if let Some(txn) = self
            .transaction_log
            .transactions
            .iter_mut()
            .find(|t| t.id == txn_id)
        {
            txn.add_step(TransactionStep {
                op: TransactionOp::Install,
                package_name: name.into(),
                from_version: None,
                to_version: Some(header.version),
                download_size: header.download_size,
                installed_size_delta: header.installed_size as i64,
            });
        }
        Ok(())
    }

    pub fn add_upgrade(&mut self, txn_id: TransactionId, name: &str) -> Result<()> {
        let installed = self
            .resolver
            .installed
            .get(name)
            .ok_or(UpdateError::NotFound)?;
        let from = installed.header.version;
        let candidate = self
            .resolver
            .best_candidate(name)
            .ok_or(UpdateError::NotFound)?;
        if candidate.version <= from {
            return Err(UpdateError::AlreadyInstalled);
        }
        let to = candidate.version;
        let download_size = candidate.download_size;
        let size_delta = candidate.installed_size as i64 - installed.header.installed_size as i64;
        if let Some(txn) = self
            .transaction_log
            .transactions
            .iter_mut()
            .find(|t| t.id == txn_id)
        {
            txn.add_step(TransactionStep {
                op: TransactionOp::Upgrade,
                package_name: name.into(),
                from_version: Some(from),
                to_version: Some(to),
                download_size,
                installed_size_delta: size_delta,
            });
        }
        Ok(())
    }

    pub fn add_remove(&mut self, txn_id: TransactionId, name: &str) -> Result<()> {
        let installed = self
            .resolver
            .installed
            .get(name)
            .ok_or(UpdateError::NotFound)?;
        let version = installed.header.version;
        let size = installed.header.installed_size;
        if let Some(txn) = self
            .transaction_log
            .transactions
            .iter_mut()
            .find(|t| t.id == txn_id)
        {
            txn.add_step(TransactionStep {
                op: TransactionOp::Remove,
                package_name: name.into(),
                from_version: Some(version),
                to_version: None,
                download_size: 0,
                installed_size_delta: -(size as i64),
            });
        }
        Ok(())
    }

    pub fn check_updates(&self) -> Vec<(&str, Version, Version)> {
        let mut updates = Vec::new();
        for (name, installed) in &self.resolver.installed {
            if let Some(candidate) = self.resolver.best_candidate(name) {
                if candidate.version > installed.header.version {
                    updates.push((name.as_str(), installed.header.version, candidate.version));
                }
            }
        }
        updates
    }

    pub fn available_update_count(&self) -> usize {
        self.check_updates().len()
    }

    pub fn security_update_count(&self) -> usize {
        self.notifications.security_count()
    }

    pub fn begin_system_upgrade(&mut self, to_release: Version, now: u64) -> Result<()> {
        if self.current_upgrade.is_some() {
            return Err(UpdateError::TransactionAborted);
        }
        let from = self.ab_partitions.active_slot().version;
        self.current_upgrade = Some(SystemUpgrade::new(from, to_release, now));
        Ok(())
    }
}

pub static UPDATE_MANAGER: spin::Mutex<Option<UpdateManager>> = spin::Mutex::new(None);

pub fn init() {
    let mut mgr = UpdateManager::new();
    mgr.initialized = true;
    *UPDATE_MANAGER.lock() = Some(mgr);
}

#[cfg(test)]
mod auto_update_policy_tests {
    use super::*;

    #[test]
    fn auto_update_is_user_consent_gated() {
        let mut cfg = AutoUpdateConfig::default_config();
        // Disabled: nothing auto-installs — the Concept "no forced updates" guarantee.
        cfg.policy = AutoUpdatePolicy::Disabled;
        for sev in [
            UpdateSeverity::Critical,
            UpdateSeverity::Security,
            UpdateSeverity::Important,
            UpdateSeverity::Normal,
            UpdateSeverity::Optional,
        ] {
            assert!(!cfg.should_install(sev));
        }
        // SecurityOnly: only Critical/Security auto-install (the safe default).
        cfg.policy = AutoUpdatePolicy::SecurityOnly;
        assert!(cfg.should_install(UpdateSeverity::Critical));
        assert!(cfg.should_install(UpdateSeverity::Security));
        assert!(!cfg.should_install(UpdateSeverity::Important));
        assert!(!cfg.should_install(UpdateSeverity::Normal));
        assert!(!cfg.should_install(UpdateSeverity::Optional));
        // All: the user opted into everything.
        cfg.policy = AutoUpdatePolicy::All;
        assert!(cfg.should_install(UpdateSeverity::Optional));
        // The shipped DEFAULT is SecurityOnly, never All — the OS does not silently
        // install non-security updates without the user opting in.
        assert_eq!(
            AutoUpdateConfig::default_config().policy,
            AutoUpdatePolicy::SecurityOnly
        );
    }

    #[test]
    fn update_channels_are_nested() {
        use UpdateChannel::*;
        // Stable subscriber: only Stable builds.
        assert!(Stable.accepts(Stable));
        assert!(!Stable.accepts(Beta));
        assert!(!Stable.accepts(Nightly));
        // Beta subscriber: Stable + Beta.
        assert!(Beta.accepts(Stable));
        assert!(Beta.accepts(Beta));
        assert!(!Beta.accepts(Nightly));
        // Nightly subscriber: everything.
        assert!(Nightly.accepts(Stable));
        assert!(Nightly.accepts(Beta));
        assert!(Nightly.accepts(Nightly));
        // The shipped default is the most conservative channel.
        assert_eq!(
            AutoUpdateConfig::default_config().channel,
            UpdateChannel::Stable
        );
    }
}
