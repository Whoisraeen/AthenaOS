#![no_std]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

// ─── Error Types ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageError {
    NotFound,
    AlreadyInstalled,
    DependencyConflict,
    VersionConflict,
    BrokenDependency,
    CycleDetected,
    ChecksumMismatch,
    SignatureInvalid,
    DiskSpaceLow,
    DownloadFailed,
    ExtractFailed,
    ScriptFailed,
    LockHeld,
    DatabaseCorrupt,
    InvalidPackage,
    PermissionDenied,
    NetworkError,
    BuildFailed,
    IoError,
    InternalError,
}

pub type Result<T> = core::result::Result<T, PackageError>;

// ─── Version Types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub epoch: u32,
    pub version: String,
    pub release: String,
}

impl Version {
    pub fn new(version: String) -> Self {
        Self {
            epoch: 0,
            version,
            release: String::from("1"),
        }
    }

    pub fn with_epoch(mut self, epoch: u32) -> Self {
        self.epoch = epoch;
        self
    }

    pub fn with_release(mut self, release: String) -> Self {
        self.release = release;
        self
    }

    pub fn compare(&self, other: &Self) -> core::cmp::Ordering {
        match self.epoch.cmp(&other.epoch) {
            core::cmp::Ordering::Equal => {}
            ord => return ord,
        }
        match self.cmp_version_str(&self.version, &other.version) {
            core::cmp::Ordering::Equal => {}
            ord => return ord,
        }
        self.cmp_version_str(&self.release, &other.release)
    }

    fn cmp_version_str(&self, a: &str, b: &str) -> core::cmp::Ordering {
        let mut ai = a.as_bytes().iter().peekable();
        let mut bi = b.as_bytes().iter().peekable();
        loop {
            while ai.peek().map_or(false, |c| !c.is_ascii_alphanumeric()) {
                ai.next();
            }
            while bi.peek().map_or(false, |c| !c.is_ascii_alphanumeric()) {
                bi.next();
            }
            let a_done = ai.peek().is_none();
            let b_done = bi.peek().is_none();
            if a_done && b_done {
                return core::cmp::Ordering::Equal;
            }
            if a_done {
                return core::cmp::Ordering::Less;
            }
            if b_done {
                return core::cmp::Ordering::Greater;
            }
            let is_digit = ai.peek().map_or(false, |c| c.is_ascii_digit());
            if is_digit {
                let mut an = 0u64;
                while ai.peek().map_or(false, |c| c.is_ascii_digit()) {
                    an = an * 10 + (*ai.next().unwrap() - b'0') as u64;
                }
                let mut bn = 0u64;
                while bi.peek().map_or(false, |c| c.is_ascii_digit()) {
                    bn = bn * 10 + (*bi.next().unwrap() - b'0') as u64;
                }
                match an.cmp(&bn) {
                    core::cmp::Ordering::Equal => continue,
                    ord => return ord,
                }
            } else {
                let ac = ai.next().unwrap();
                let bc = bi.next().unwrap();
                match ac.cmp(bc) {
                    core::cmp::Ordering::Equal => continue,
                    ord => return ord,
                }
            }
        }
    }

    pub fn to_string(&self) -> String {
        let mut s = String::new();
        if self.epoch > 0 {
            s.push_str(&alloc::format!("{}:", self.epoch));
        }
        s.push_str(&self.version);
        if !self.release.is_empty() {
            s.push('-');
            s.push_str(&self.release);
        }
        s
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionOp {
    Equal,
    GreaterEqual,
    LessEqual,
    GreaterThan,
    LessThan,
    Any,
}

#[derive(Debug, Clone)]
pub struct VersionConstraint {
    pub op: VersionOp,
    pub version: Version,
}

impl VersionConstraint {
    pub fn any() -> Self {
        Self {
            op: VersionOp::Any,
            version: Version::new(String::new()),
        }
    }

    pub fn exact(version: Version) -> Self {
        Self {
            op: VersionOp::Equal,
            version,
        }
    }

    pub fn satisfied_by(&self, candidate: &Version) -> bool {
        match self.op {
            VersionOp::Any => true,
            VersionOp::Equal => candidate.compare(&self.version) == core::cmp::Ordering::Equal,
            VersionOp::GreaterEqual => {
                candidate.compare(&self.version) != core::cmp::Ordering::Less
            }
            VersionOp::LessEqual => {
                candidate.compare(&self.version) != core::cmp::Ordering::Greater
            }
            VersionOp::GreaterThan => {
                candidate.compare(&self.version) == core::cmp::Ordering::Greater
            }
            VersionOp::LessThan => candidate.compare(&self.version) == core::cmp::Ordering::Less,
        }
    }
}

// ─── Package Format ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    X86_64,
    Aarch64,
    Riscv64,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageReason {
    Explicit,
    Dependency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageHold {
    None,
    Hold,
}

#[derive(Debug, Clone)]
pub struct Dependency {
    pub name: String,
    pub constraint: VersionConstraint,
}

impl Dependency {
    pub fn new(name: String) -> Self {
        Self {
            name,
            constraint: VersionConstraint::any(),
        }
    }

    pub fn with_constraint(name: String, constraint: VersionConstraint) -> Self {
        Self { name, constraint }
    }
}

#[derive(Debug, Clone)]
pub struct PackageHeader {
    pub name: String,
    pub version: Version,
    pub arch: Architecture,
    pub description: String,
    pub url: String,
    pub license: String,
    pub groups: Vec<String>,
    pub provides: Vec<String>,
    pub conflicts: Vec<String>,
    pub replaces: Vec<String>,
    pub depends: Vec<Dependency>,
    pub optdepends: Vec<Dependency>,
    pub makedepends: Vec<Dependency>,
    pub checkdepends: Vec<Dependency>,
    pub size: u64,
    pub installed_size: u64,
    pub packager: String,
    pub build_date: u64,
    pub install_date: u64,
}

impl PackageHeader {
    pub fn new(name: String, version: Version) -> Self {
        Self {
            name,
            version,
            arch: Architecture::X86_64,
            description: String::new(),
            url: String::new(),
            license: String::new(),
            groups: Vec::new(),
            provides: Vec::new(),
            conflicts: Vec::new(),
            replaces: Vec::new(),
            depends: Vec::new(),
            optdepends: Vec::new(),
            makedepends: Vec::new(),
            checkdepends: Vec::new(),
            size: 0,
            installed_size: 0,
            packager: String::new(),
            build_date: 0,
            install_date: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PackageFile {
    pub path: String,
    pub size: u64,
    pub mode: u32,
    pub checksum: [u8; 32],
    pub is_config: bool,
    pub backup: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptPhase {
    PreInstall,
    PostInstall,
    PreUpgrade,
    PostUpgrade,
    PreRemove,
    PostRemove,
}

#[derive(Debug, Clone)]
pub struct PackageScript {
    pub phase: ScriptPhase,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumType {
    Sha256,
    Sha512,
    Blake2b,
}

#[derive(Debug, Clone)]
pub struct PackageChecksum {
    pub checksum_type: ChecksumType,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct PackageSignature {
    pub key_id: String,
    pub signature: Vec<u8>,
    pub signed_data_hash: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Package {
    pub header: PackageHeader,
    pub files: Vec<PackageFile>,
    pub scripts: Vec<PackageScript>,
    pub checksums: Vec<PackageChecksum>,
    pub signature: Option<PackageSignature>,
    pub data: Vec<u8>,
}

impl Package {
    pub fn new(header: PackageHeader) -> Self {
        Self {
            header,
            files: Vec::new(),
            scripts: Vec::new(),
            checksums: Vec::new(),
            signature: None,
            data: Vec::new(),
        }
    }

    pub fn verify_checksum(&self) -> Result<()> {
        for cs in &self.checksums {
            let computed = match cs.checksum_type {
                ChecksumType::Sha256 => compute_sha256(&self.data),
                ChecksumType::Sha512 => compute_sha512(&self.data),
                ChecksumType::Blake2b => compute_blake2b(&self.data),
            };
            if computed != cs.value {
                return Err(PackageError::ChecksumMismatch);
            }
        }
        Ok(())
    }

    pub fn verify_signature(&self, keyring: &GpgKeyring) -> Result<()> {
        if let Some(sig) = &self.signature {
            // Bind the signature to THIS payload: the signed hash must be the
            // real hash of self.data, otherwise a valid signature over some
            // benign hash could be shipped alongside malicious data. Hash
            // width selects the algorithm (32 = SHA-256, 64 = SHA-512).
            let expected = match sig.signed_data_hash.len() {
                32 => compute_sha256(&self.data),
                64 => compute_sha512(&self.data),
                _ => return Err(PackageError::SignatureInvalid),
            };
            if expected != sig.signed_data_hash {
                return Err(PackageError::SignatureInvalid);
            }
            if !keyring.verify(&sig.key_id, &sig.signed_data_hash, &sig.signature) {
                return Err(PackageError::SignatureInvalid);
            }
        }
        Ok(())
    }
}

// Real cryptographic hashes via the shared `rae_crypto` crate. These replaced
// homebrew `wrapping_mul` XOR loops that were trivially collidable and gave
// `verify_checksum` no actual integrity guarantee.
fn compute_sha256(data: &[u8]) -> Vec<u8> {
    rae_crypto::sha256::sha256(data).to_vec()
}

fn compute_sha512(data: &[u8]) -> Vec<u8> {
    rae_crypto::ed25519::sha512(data).to_vec()
}

fn compute_blake2b(data: &[u8]) -> Vec<u8> {
    // BLAKE2b-512 (64-byte digest), matching the SHA-512 width.
    rae_crypto::blake2b(64, data)
}

// ─── GPG Key Management ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyTrust {
    Unknown,
    Untrusted,
    Marginal,
    Full,
    Ultimate,
    Revoked,
}

#[derive(Debug, Clone)]
pub struct GpgKey {
    pub id: String,
    pub fingerprint: String,
    pub owner: String,
    pub trust: KeyTrust,
    pub public_key: Vec<u8>,
    pub created: u64,
    pub expires: u64,
}

pub struct GpgKeyring {
    pub keys: BTreeMap<String, GpgKey>,
}

impl GpgKeyring {
    pub fn new() -> Self {
        Self {
            keys: BTreeMap::new(),
        }
    }

    pub fn import_key(&mut self, key: GpgKey) -> Result<()> {
        self.keys.insert(key.id.clone(), key);
        Ok(())
    }

    pub fn revoke_key(&mut self, id: &str) -> Result<()> {
        let key = self.keys.get_mut(id).ok_or(PackageError::NotFound)?;
        key.trust = KeyTrust::Revoked;
        Ok(())
    }

    pub fn trust_key(&mut self, id: &str, trust: KeyTrust) -> Result<()> {
        let key = self.keys.get_mut(id).ok_or(PackageError::NotFound)?;
        key.trust = trust;
        Ok(())
    }

    pub fn get_key(&self, id: &str) -> Option<&GpgKey> {
        self.keys.get(id)
    }

    /// Verify a detached Ed25519 signature over `message` (the package's
    /// signed-data hash) under the named key. RaeenOS keys are Ed25519
    /// (32-byte public key, 64-byte signature) — `public_key` shorter/longer
    /// is rejected. Fail-closed: the former stub ignored the signature entirely
    /// and returned `true` for any non-revoked key, accepting forged packages.
    pub fn verify(&self, key_id: &str, message: &[u8], signature: &[u8]) -> bool {
        let key = match self.keys.get(key_id) {
            Some(k) => k,
            None => return false,
        };
        if key.trust == KeyTrust::Revoked || key.trust == KeyTrust::Untrusted {
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
        rae_crypto::ed25519::verify(&pk, message, &sig)
    }
}

// ─── Repository ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RepoConfig {
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub gpg_key: String,
    pub priority: u32,
    pub mirrorlist: Vec<String>,
}

impl RepoConfig {
    pub fn new(name: String, url: String) -> Self {
        Self {
            name,
            url,
            enabled: true,
            gpg_key: String::new(),
            priority: 100,
            mirrorlist: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RepoPackageEntry {
    pub name: String,
    pub version: Version,
    pub arch: Architecture,
    pub filename: String,
    pub size: u64,
    pub installed_size: u64,
    pub checksum: Vec<u8>,
    pub depends: Vec<Dependency>,
    pub provides: Vec<String>,
    pub conflicts: Vec<String>,
    pub description: String,
}

pub struct RepoDatabase {
    pub config: RepoConfig,
    pub packages: BTreeMap<String, Vec<RepoPackageEntry>>,
    pub last_sync: u64,
}

impl RepoDatabase {
    pub fn new(config: RepoConfig) -> Self {
        Self {
            config,
            packages: BTreeMap::new(),
            last_sync: 0,
        }
    }

    pub fn sync(&mut self, _timestamp: u64) -> Result<()> {
        self.last_sync = _timestamp;
        Ok(())
    }

    pub fn find_package(&self, name: &str) -> Option<&RepoPackageEntry> {
        self.packages.get(name).and_then(|entries| entries.last())
    }

    pub fn find_with_constraint(
        &self,
        name: &str,
        constraint: &VersionConstraint,
    ) -> Option<&RepoPackageEntry> {
        self.packages.get(name).and_then(|entries| {
            entries
                .iter()
                .rev()
                .find(|e| constraint.satisfied_by(&e.version))
        })
    }

    pub fn search(&self, query: &str) -> Vec<&RepoPackageEntry> {
        let mut results = Vec::new();
        for entries in self.packages.values() {
            for entry in entries {
                if entry.name.contains(query) || entry.description.contains(query) {
                    results.push(entry);
                }
            }
        }
        results
    }

    pub fn all_packages(&self) -> Vec<&RepoPackageEntry> {
        self.packages.values().flat_map(|v| v.iter()).collect()
    }
}

// ─── Dependency Resolution (SAT Solver) ──────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SatLiteral {
    Positive(u32),
    Negative(u32),
}

struct SatClause {
    literals: Vec<SatLiteral>,
}

pub struct DependencySolver {
    installed: BTreeMap<String, Version>,
    available: Vec<RepoPackageEntry>,
    clauses: Vec<SatClause>,
    assignments: BTreeMap<u32, bool>,
    package_ids: BTreeMap<String, u32>,
    next_var: u32,
}

impl DependencySolver {
    pub fn new() -> Self {
        Self {
            installed: BTreeMap::new(),
            available: Vec::new(),
            clauses: Vec::new(),
            assignments: BTreeMap::new(),
            package_ids: BTreeMap::new(),
            next_var: 1,
        }
    }

    pub fn add_installed(&mut self, name: String, version: Version) {
        self.installed.insert(name, version);
    }

    pub fn add_available(&mut self, entry: RepoPackageEntry) {
        self.available.push(entry);
    }

    fn get_var(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.package_ids.get(name) {
            id
        } else {
            let id = self.next_var;
            self.next_var += 1;
            self.package_ids.insert(String::from(name), id);
            id
        }
    }

    pub fn resolve(&mut self, targets: &[String]) -> Result<Vec<String>> {
        for target in targets {
            let var = self.get_var(target);
            self.clauses.push(SatClause {
                literals: alloc::vec![SatLiteral::Positive(var)],
            });
        }

        for pkg in &self.available.clone() {
            let pkg_var = self.get_var(&pkg.name);
            for dep in &pkg.depends {
                let dep_var = self.get_var(&dep.name);
                self.clauses.push(SatClause {
                    literals: alloc::vec![
                        SatLiteral::Negative(pkg_var),
                        SatLiteral::Positive(dep_var),
                    ],
                });
            }
            for conflict in &pkg.conflicts {
                let conflict_var = self.get_var(conflict);
                self.clauses.push(SatClause {
                    literals: alloc::vec![
                        SatLiteral::Negative(pkg_var),
                        SatLiteral::Negative(conflict_var),
                    ],
                });
            }
        }

        self.solve_dpll()?;

        let mut to_install = Vec::new();
        for (name, &var_id) in &self.package_ids {
            if self.assignments.get(&var_id).copied().unwrap_or(false) {
                if !self.installed.contains_key(name) {
                    to_install.push(name.clone());
                }
            }
        }
        Ok(to_install)
    }

    fn solve_dpll(&mut self) -> Result<()> {
        loop {
            let propagated = self.unit_propagate()?;
            if !propagated {
                break;
            }
        }

        if self.all_satisfied() {
            return Ok(());
        }

        let unassigned = self.find_unassigned();
        if let Some(var) = unassigned {
            self.assignments.insert(var, true);
            if self.solve_dpll().is_ok() {
                return Ok(());
            }
            self.assignments.insert(var, false);
            if self.solve_dpll().is_ok() {
                return Ok(());
            }
            self.assignments.remove(&var);
            return Err(PackageError::DependencyConflict);
        }

        Ok(())
    }

    fn unit_propagate(&mut self) -> Result<bool> {
        let mut propagated = false;
        for i in 0..self.clauses.len() {
            let clause = &self.clauses[i];
            let mut unset = None;
            let mut unset_count = 0;
            let mut satisfied = false;

            for lit in &clause.literals {
                match lit {
                    SatLiteral::Positive(v) => {
                        if let Some(&val) = self.assignments.get(v) {
                            if val {
                                satisfied = true;
                                break;
                            }
                        } else {
                            unset = Some((*v, true));
                            unset_count += 1;
                        }
                    }
                    SatLiteral::Negative(v) => {
                        if let Some(&val) = self.assignments.get(v) {
                            if !val {
                                satisfied = true;
                                break;
                            }
                        } else {
                            unset = Some((*v, false));
                            unset_count += 1;
                        }
                    }
                }
            }

            if !satisfied && unset_count == 0 {
                return Err(PackageError::DependencyConflict);
            }
            if !satisfied && unset_count == 1 {
                if let Some((var, val)) = unset {
                    self.assignments.insert(var, val);
                    propagated = true;
                }
            }
        }
        Ok(propagated)
    }

    fn all_satisfied(&self) -> bool {
        for clause in &self.clauses {
            let mut satisfied = false;
            for lit in &clause.literals {
                let ok = match lit {
                    SatLiteral::Positive(v) => self.assignments.get(v).copied().unwrap_or(false),
                    SatLiteral::Negative(v) => !self.assignments.get(v).copied().unwrap_or(true),
                };
                if ok {
                    satisfied = true;
                    break;
                }
            }
            if !satisfied {
                return false;
            }
        }
        true
    }

    fn find_unassigned(&self) -> Option<u32> {
        for (_, &id) in &self.package_ids {
            if !self.assignments.contains_key(&id) {
                return Some(id);
            }
        }
        None
    }

    pub fn detect_cycles(&self) -> Vec<Vec<String>> {
        let mut cycles = Vec::new();
        let mut visited = alloc::collections::BTreeSet::new();
        let mut stack = Vec::new();

        for pkg in &self.available {
            if !visited.contains(&pkg.name) {
                self.dfs_cycle(&pkg.name, &mut visited, &mut stack, &mut cycles);
            }
        }
        cycles
    }

    fn dfs_cycle(
        &self,
        name: &str,
        visited: &mut alloc::collections::BTreeSet<String>,
        stack: &mut Vec<String>,
        cycles: &mut Vec<Vec<String>>,
    ) {
        if stack.contains(&String::from(name)) {
            let start = stack.iter().position(|s| s == name).unwrap();
            cycles.push(stack[start..].to_vec());
            return;
        }
        if visited.contains(name) {
            return;
        }
        visited.insert(String::from(name));
        stack.push(String::from(name));

        for pkg in &self.available {
            if pkg.name == name {
                for dep in &pkg.depends {
                    self.dfs_cycle(&dep.name, visited, stack, cycles);
                }
            }
        }
        stack.pop();
    }
}

// ─── Transaction System ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionOp {
    Install,
    Remove,
    Upgrade,
    Downgrade,
    Reinstall,
}

#[derive(Debug, Clone)]
pub struct TransactionItem {
    pub op: TransactionOp,
    pub package_name: String,
    pub old_version: Option<Version>,
    pub new_version: Option<Version>,
    pub download_size: u64,
    pub install_size: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionState {
    Planning,
    Downloading,
    Installing,
    Committed,
    RolledBack,
    Failed,
}

pub struct Transaction {
    pub id: u64,
    pub state: TransactionState,
    pub items: Vec<TransactionItem>,
    pub timestamp: u64,
    pub total_download: u64,
    pub total_install_delta: i64,
    pub rollback_data: Vec<RollbackEntry>,
}

impl Transaction {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            state: TransactionState::Planning,
            items: Vec::new(),
            timestamp: 0,
            total_download: 0,
            total_install_delta: 0,
            rollback_data: Vec::new(),
        }
    }

    pub fn add_item(&mut self, item: TransactionItem) {
        self.total_download += item.download_size;
        self.total_install_delta += item.install_size;
        self.items.push(item);
    }

    pub fn check_disk_space(&self, available: u64) -> Result<()> {
        if self.total_install_delta > 0 && (self.total_install_delta as u64) > available {
            return Err(PackageError::DiskSpaceLow);
        }
        Ok(())
    }

    pub fn execute(&mut self, db: &mut InstalledDatabase) -> Result<()> {
        self.state = TransactionState::Downloading;

        self.state = TransactionState::Installing;
        for item in &self.items {
            let result = match item.op {
                TransactionOp::Install => {
                    db.mark_installed(item.package_name.clone(), item.new_version.clone().unwrap())
                }
                TransactionOp::Remove => db.mark_removed(&item.package_name),
                TransactionOp::Upgrade => {
                    db.mark_removed(&item.package_name)?;
                    db.mark_installed(item.package_name.clone(), item.new_version.clone().unwrap())
                }
                TransactionOp::Downgrade => {
                    db.mark_removed(&item.package_name)?;
                    db.mark_installed(item.package_name.clone(), item.new_version.clone().unwrap())
                }
                TransactionOp::Reinstall => {
                    db.mark_removed(&item.package_name)?;
                    db.mark_installed(item.package_name.clone(), item.new_version.clone().unwrap())
                }
            };

            if let Err(e) = result {
                self.state = TransactionState::Failed;
                self.rollback(db);
                return Err(e);
            }

            self.rollback_data.push(RollbackEntry {
                package_name: item.package_name.clone(),
                op: item.op,
                old_version: item.old_version.clone(),
            });
        }

        self.state = TransactionState::Committed;
        Ok(())
    }

    pub fn rollback(&mut self, db: &mut InstalledDatabase) {
        for entry in self.rollback_data.iter().rev() {
            match entry.op {
                TransactionOp::Install => {
                    let _ = db.mark_removed(&entry.package_name);
                }
                TransactionOp::Remove => {
                    if let Some(ver) = &entry.old_version {
                        let _ = db.mark_installed(entry.package_name.clone(), ver.clone());
                    }
                }
                TransactionOp::Upgrade | TransactionOp::Downgrade | TransactionOp::Reinstall => {
                    let _ = db.mark_removed(&entry.package_name);
                    if let Some(ver) = &entry.old_version {
                        let _ = db.mark_installed(entry.package_name.clone(), ver.clone());
                    }
                }
            }
        }
        self.state = TransactionState::RolledBack;
    }
}

#[derive(Debug, Clone)]
pub struct RollbackEntry {
    pub package_name: String,
    pub op: TransactionOp,
    pub old_version: Option<Version>,
}

// ─── Installed Database ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct InstalledPackage {
    pub name: String,
    pub version: Version,
    pub reason: PackageReason,
    pub hold: PackageHold,
    pub install_date: u64,
    pub files: Vec<String>,
    pub backup_files: Vec<String>,
}

pub struct InstalledDatabase {
    pub packages: BTreeMap<String, InstalledPackage>,
}

impl InstalledDatabase {
    pub fn new() -> Self {
        Self {
            packages: BTreeMap::new(),
        }
    }

    pub fn is_installed(&self, name: &str) -> bool {
        self.packages.contains_key(name)
    }

    pub fn get(&self, name: &str) -> Option<&InstalledPackage> {
        self.packages.get(name)
    }

    pub fn mark_installed(&mut self, name: String, version: Version) -> Result<()> {
        let pkg = InstalledPackage {
            name: name.clone(),
            version,
            reason: PackageReason::Explicit,
            hold: PackageHold::None,
            install_date: 0,
            files: Vec::new(),
            backup_files: Vec::new(),
        };
        self.packages.insert(name, pkg);
        Ok(())
    }

    pub fn mark_removed(&mut self, name: &str) -> Result<()> {
        self.packages.remove(name).ok_or(PackageError::NotFound)?;
        Ok(())
    }

    pub fn set_reason(&mut self, name: &str, reason: PackageReason) -> Result<()> {
        let pkg = self.packages.get_mut(name).ok_or(PackageError::NotFound)?;
        pkg.reason = reason;
        Ok(())
    }

    pub fn set_hold(&mut self, name: &str, hold: PackageHold) -> Result<()> {
        let pkg = self.packages.get_mut(name).ok_or(PackageError::NotFound)?;
        pkg.hold = hold;
        Ok(())
    }

    pub fn list_installed(&self) -> Vec<&InstalledPackage> {
        self.packages.values().collect()
    }

    pub fn list_orphans(&self) -> Vec<&InstalledPackage> {
        self.packages
            .values()
            .filter(|p| p.reason == PackageReason::Dependency)
            .collect()
    }

    pub fn list_held(&self) -> Vec<&InstalledPackage> {
        self.packages
            .values()
            .filter(|p| p.hold == PackageHold::Hold)
            .collect()
    }

    pub fn find_file_owner(&self, path: &str) -> Option<&InstalledPackage> {
        self.packages
            .values()
            .find(|p| p.files.iter().any(|f| f == path))
    }

    pub fn detect_file_conflicts(&self, files: &[String]) -> Vec<(String, String)> {
        let mut conflicts = Vec::new();
        for file in files {
            if let Some(owner) = self.find_file_owner(file) {
                conflicts.push((file.clone(), owner.name.clone()));
            }
        }
        conflicts
    }
}

// ─── Delta Packages ──────────────────────────────────────────────────────────

pub struct DeltaPackage {
    pub from_version: Version,
    pub to_version: Version,
    pub package_name: String,
    pub delta_data: Vec<u8>,
    pub delta_size: u64,
    pub full_size: u64,
    pub checksum: Vec<u8>,
}

impl DeltaPackage {
    pub fn savings_percent(&self) -> u32 {
        if self.full_size == 0 {
            return 0;
        }
        ((self.full_size - self.delta_size) * 100 / self.full_size) as u32
    }

    pub fn apply(&self, old_data: &[u8]) -> Result<Vec<u8>> {
        let mut result = Vec::from(old_data);
        for &b in &self.delta_data {
            match b & 0x03 {
                0 => result.push(b),
                1 => {
                    if !result.is_empty() {
                        let idx = (b >> 2) as usize % result.len();
                        result[idx] ^= b;
                    }
                }
                2 => result.extend_from_slice(&[b, b]),
                _ => {}
            }
        }
        Ok(result)
    }
}

// ─── Package Groups ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PackageGroup {
    pub name: String,
    pub description: String,
    pub packages: Vec<String>,
    pub optional_packages: Vec<String>,
}

impl PackageGroup {
    pub fn new(name: String) -> Self {
        Self {
            name,
            description: String::new(),
            packages: Vec::new(),
            optional_packages: Vec::new(),
        }
    }

    pub fn all_members(&self) -> Vec<&String> {
        self.packages
            .iter()
            .chain(self.optional_packages.iter())
            .collect()
    }
}

// ─── Build System ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BuildRecipe {
    pub pkg_name: String,
    pub pkg_version: Version,
    pub description: String,
    pub url: String,
    pub license: String,
    pub sources: Vec<SourceEntry>,
    pub depends: Vec<Dependency>,
    pub makedepends: Vec<Dependency>,
    pub checkdepends: Vec<Dependency>,
    pub build_steps: Vec<String>,
    pub package_steps: Vec<String>,
    pub install_script: String,
    pub options: BuildOptions,
}

#[derive(Debug, Clone)]
pub struct SourceEntry {
    pub url: String,
    pub filename: String,
    pub checksum: Vec<u8>,
    pub checksum_type: ChecksumType,
}

#[derive(Debug, Clone, Copy)]
pub struct BuildOptions {
    pub strip: bool,
    pub docs: bool,
    pub man_pages: bool,
    pub static_libs: bool,
    pub empty_dirs: bool,
    pub ccache: bool,
    pub distcc: bool,
    pub debug: bool,
    pub lto: bool,
}

impl BuildOptions {
    pub fn defaults() -> Self {
        Self {
            strip: true,
            docs: true,
            man_pages: true,
            static_libs: false,
            empty_dirs: false,
            ccache: false,
            distcc: false,
            debug: false,
            lto: false,
        }
    }
}

pub struct BuildEnvironment {
    pub root: String,
    pub recipe: BuildRecipe,
    pub source_dir: String,
    pub build_dir: String,
    pub package_dir: String,
    pub log: Vec<String>,
}

impl BuildEnvironment {
    pub fn new(recipe: BuildRecipe) -> Self {
        let name = recipe.pkg_name.clone();
        Self {
            root: alloc::format!("/var/build/{}", name),
            recipe,
            source_dir: String::from("src"),
            build_dir: String::from("build"),
            package_dir: String::from("pkg"),
            log: Vec::new(),
        }
    }

    pub fn fetch_sources(&mut self) -> Result<()> {
        for source in &self.recipe.sources {
            let _url = &source.url;
            self.log
                .push(alloc::format!("Fetching {}", source.filename));
        }
        Ok(())
    }

    pub fn verify_sources(&self) -> Result<()> {
        for source in &self.recipe.sources {
            let _ = &source.checksum;
        }
        Ok(())
    }

    pub fn build(&mut self) -> Result<()> {
        for step in &self.recipe.build_steps {
            self.log.push(alloc::format!("Build: {}", step));
        }
        Ok(())
    }

    pub fn package(&mut self) -> Result<Package> {
        for step in &self.recipe.package_steps {
            self.log.push(alloc::format!("Package: {}", step));
        }
        let header = PackageHeader::new(
            self.recipe.pkg_name.clone(),
            self.recipe.pkg_version.clone(),
        );
        Ok(Package::new(header))
    }
}

// ─── Transaction History ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub id: u64,
    pub timestamp: u64,
    pub items: Vec<TransactionItem>,
    pub user: String,
}

pub struct TransactionHistory {
    pub entries: Vec<HistoryEntry>,
    next_id: u64,
}

impl TransactionHistory {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_id: 1,
        }
    }

    pub fn record(&mut self, items: Vec<TransactionItem>, timestamp: u64) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(HistoryEntry {
            id,
            timestamp,
            items,
            user: String::from("root"),
        });
        id
    }

    pub fn get(&self, id: u64) -> Option<&HistoryEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn list(&self) -> &[HistoryEntry] {
        &self.entries
    }

    pub fn undo(&self, id: u64) -> Option<Vec<TransactionItem>> {
        let entry = self.get(id)?;
        let mut reversed = Vec::new();
        for item in entry.items.iter().rev() {
            let new_op = match item.op {
                TransactionOp::Install => TransactionOp::Remove,
                TransactionOp::Remove => TransactionOp::Install,
                TransactionOp::Upgrade => TransactionOp::Downgrade,
                TransactionOp::Downgrade => TransactionOp::Upgrade,
                TransactionOp::Reinstall => TransactionOp::Reinstall,
            };
            reversed.push(TransactionItem {
                op: new_op,
                package_name: item.package_name.clone(),
                old_version: item.new_version.clone(),
                new_version: item.old_version.clone(),
                download_size: item.download_size,
                install_size: -item.install_size,
            });
        }
        Some(reversed)
    }
}

// ─── Hooks (alpm-style) ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookTargetType {
    File,
    Package,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookOperation {
    Install,
    Upgrade,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookWhen {
    PreTransaction,
    PostTransaction,
}

#[derive(Debug, Clone)]
pub struct AlpmHook {
    pub name: String,
    pub description: String,
    pub when: HookWhen,
    pub target_type: HookTargetType,
    pub operation: HookOperation,
    pub targets: Vec<String>,
    pub exec: String,
    pub needs_targets: bool,
    pub abort_on_fail: bool,
}

impl AlpmHook {
    pub fn matches(&self, op: HookOperation, target: &str) -> bool {
        if self.operation != op {
            return false;
        }
        self.targets.iter().any(|t| {
            if t.ends_with('*') {
                target.starts_with(&t[..t.len() - 1])
            } else {
                target == t
            }
        })
    }
}

pub struct HookRunner {
    pub hooks: Vec<AlpmHook>,
}

impl HookRunner {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    pub fn add_hook(&mut self, hook: AlpmHook) {
        self.hooks.push(hook);
    }

    pub fn run_pre_hooks(&self, op: HookOperation, targets: &[String]) -> Result<()> {
        for hook in &self.hooks {
            if hook.when != HookWhen::PreTransaction {
                continue;
            }
            for target in targets {
                if hook.matches(op, target) {
                    let _ = &hook.exec;
                    break;
                }
            }
        }
        Ok(())
    }

    pub fn run_post_hooks(&self, op: HookOperation, targets: &[String]) -> Result<()> {
        for hook in &self.hooks {
            if hook.when != HookWhen::PostTransaction {
                continue;
            }
            for target in targets {
                if hook.matches(op, target) {
                    let _ = &hook.exec;
                    break;
                }
            }
        }
        Ok(())
    }
}

// ─── Lock File ───────────────────────────────────────────────────────────────

pub struct PackageLock {
    pub locked: AtomicBool,
    pub holder_pid: AtomicU64,
    pub lock_time: AtomicU64,
}

impl PackageLock {
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
            holder_pid: AtomicU64::new(0),
            lock_time: AtomicU64::new(0),
        }
    }

    pub fn acquire(&self, pid: u64, now: u64) -> Result<()> {
        if self
            .locked
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            let held_since = self.lock_time.load(Ordering::Relaxed);
            if now.saturating_sub(held_since) > 3600 {
                self.locked.store(true, Ordering::SeqCst);
                self.holder_pid.store(pid, Ordering::Relaxed);
                self.lock_time.store(now, Ordering::Relaxed);
                return Ok(());
            }
            return Err(PackageError::LockHeld);
        }
        self.holder_pid.store(pid, Ordering::Relaxed);
        self.lock_time.store(now, Ordering::Relaxed);
        Ok(())
    }

    pub fn release(&self, pid: u64) -> Result<()> {
        if self.holder_pid.load(Ordering::Relaxed) != pid {
            return Err(PackageError::PermissionDenied);
        }
        self.locked.store(false, Ordering::SeqCst);
        self.holder_pid.store(0, Ordering::Relaxed);
        self.lock_time.store(0, Ordering::Relaxed);
        Ok(())
    }

    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::SeqCst)
    }
}

// ─── Download Manager ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadState {
    Queued,
    Downloading,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct DownloadTask {
    pub url: String,
    pub destination: String,
    pub expected_size: u64,
    pub downloaded: u64,
    pub state: DownloadState,
    pub checksum: Vec<u8>,
    pub mirror_index: usize,
}

pub struct DownloadManager {
    pub tasks: Vec<DownloadTask>,
    pub max_concurrent: usize,
    pub active_count: usize,
    pub total_downloaded: u64,
}

impl DownloadManager {
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            tasks: Vec::new(),
            max_concurrent,
            active_count: 0,
            total_downloaded: 0,
        }
    }

    pub fn enqueue(&mut self, url: String, dest: String, size: u64, checksum: Vec<u8>) {
        self.tasks.push(DownloadTask {
            url,
            destination: dest,
            expected_size: size,
            downloaded: 0,
            state: DownloadState::Queued,
            checksum,
            mirror_index: 0,
        });
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

    pub fn complete_task(&mut self, index: usize) {
        if index < self.tasks.len() {
            self.tasks[index].state = DownloadState::Completed;
            self.tasks[index].downloaded = self.tasks[index].expected_size;
            self.total_downloaded += self.tasks[index].expected_size;
            if self.active_count > 0 {
                self.active_count -= 1;
            }
        }
    }

    pub fn fail_task(&mut self, index: usize) {
        if index < self.tasks.len() {
            self.tasks[index].state = DownloadState::Failed;
            if self.active_count > 0 {
                self.active_count -= 1;
            }
        }
    }

    pub fn progress(&self) -> (u64, u64) {
        let total: u64 = self.tasks.iter().map(|t| t.expected_size).sum();
        let done: u64 = self.tasks.iter().map(|t| t.downloaded).sum();
        (done, total)
    }

    pub fn all_complete(&self) -> bool {
        self.tasks
            .iter()
            .all(|t| t.state == DownloadState::Completed)
    }

    pub fn select_fastest_mirror(&self, mirrors: &[String]) -> Option<String> {
        mirrors.first().cloned()
    }
}

// ─── User Repository (AUR-like) ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct UserPackage {
    pub name: String,
    pub version: Version,
    pub description: String,
    pub maintainer: String,
    pub votes: u32,
    pub out_of_date: bool,
    pub submitted: u64,
    pub modified: u64,
    pub recipe: BuildRecipe,
    pub comments: Vec<UserComment>,
}

#[derive(Debug, Clone)]
pub struct UserComment {
    pub author: String,
    pub content: String,
    pub timestamp: u64,
}

pub struct UserRepository {
    pub packages: BTreeMap<String, UserPackage>,
}

impl UserRepository {
    pub fn new() -> Self {
        Self {
            packages: BTreeMap::new(),
        }
    }

    pub fn submit(&mut self, pkg: UserPackage) -> Result<()> {
        if self.packages.contains_key(&pkg.name) {
            return Err(PackageError::AlreadyInstalled);
        }
        self.packages.insert(pkg.name.clone(), pkg);
        Ok(())
    }

    pub fn update(&mut self, name: &str, recipe: BuildRecipe, version: Version) -> Result<()> {
        let pkg = self.packages.get_mut(name).ok_or(PackageError::NotFound)?;
        pkg.recipe = recipe;
        pkg.version = version;
        pkg.out_of_date = false;
        Ok(())
    }

    pub fn vote(&mut self, name: &str) -> Result<()> {
        let pkg = self.packages.get_mut(name).ok_or(PackageError::NotFound)?;
        pkg.votes += 1;
        Ok(())
    }

    pub fn flag_out_of_date(&mut self, name: &str) -> Result<()> {
        let pkg = self.packages.get_mut(name).ok_or(PackageError::NotFound)?;
        pkg.out_of_date = true;
        Ok(())
    }

    pub fn add_comment(&mut self, name: &str, comment: UserComment) -> Result<()> {
        let pkg = self.packages.get_mut(name).ok_or(PackageError::NotFound)?;
        pkg.comments.push(comment);
        Ok(())
    }

    pub fn search(&self, query: &str) -> Vec<&UserPackage> {
        self.packages
            .values()
            .filter(|p| p.name.contains(query) || p.description.contains(query))
            .collect()
    }

    pub fn get(&self, name: &str) -> Option<&UserPackage> {
        self.packages.get(name)
    }

    pub fn popular(&self, limit: usize) -> Vec<&UserPackage> {
        let mut sorted: Vec<&UserPackage> = self.packages.values().collect();
        sorted.sort_by(|a, b| b.votes.cmp(&a.votes));
        sorted.truncate(limit);
        sorted
    }
}

// ─── Package Cache ───────────────────────────────────────────────────────────

pub struct PackageCache {
    pub cache_dir: String,
    pub cached_packages: BTreeMap<String, CachedPackage>,
    pub max_size_bytes: u64,
    pub current_size: u64,
}

#[derive(Debug, Clone)]
pub struct CachedPackage {
    pub name: String,
    pub version: Version,
    pub filename: String,
    pub size: u64,
    pub accessed: u64,
}

impl PackageCache {
    pub fn new(cache_dir: String) -> Self {
        Self {
            cache_dir,
            cached_packages: BTreeMap::new(),
            max_size_bytes: 5 * 1024 * 1024 * 1024,
            current_size: 0,
        }
    }

    pub fn add(&mut self, name: String, version: Version, filename: String, size: u64) {
        let key = alloc::format!("{}-{}", name, version.to_string());
        self.current_size += size;
        self.cached_packages.insert(
            key,
            CachedPackage {
                name,
                version,
                filename,
                size,
                accessed: 0,
            },
        );
    }

    pub fn lookup(&self, name: &str, version: &Version) -> Option<&CachedPackage> {
        let key = alloc::format!("{}-{}", name, version.to_string());
        self.cached_packages.get(&key)
    }

    pub fn clean(&mut self) {
        self.cached_packages.clear();
        self.current_size = 0;
    }

    pub fn clean_old(&mut self, keep_versions: usize) {
        let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (key, pkg) in &self.cached_packages {
            by_name
                .entry(pkg.name.clone())
                .or_default()
                .push(key.clone());
        }
        for (_name, mut keys) in by_name {
            if keys.len() > keep_versions {
                keys.sort();
                let to_remove = keys.len() - keep_versions;
                for key in &keys[..to_remove] {
                    if let Some(pkg) = self.cached_packages.remove(key) {
                        self.current_size -= pkg.size;
                    }
                }
            }
        }
    }
}

// ─── Package Manager ─────────────────────────────────────────────────────────

pub struct PackageManager {
    pub installed_db: InstalledDatabase,
    pub repos: Vec<RepoDatabase>,
    pub keyring: GpgKeyring,
    pub history: TransactionHistory,
    pub hooks: HookRunner,
    pub lock: PackageLock,
    pub downloads: DownloadManager,
    pub cache: PackageCache,
    pub groups: Vec<PackageGroup>,
    pub user_repo: UserRepository,
    next_txn_id: u64,
    initialized: bool,
}

static PM_INITIALIZED: AtomicBool = AtomicBool::new(false);
static TOTAL_TRANSACTIONS: AtomicU64 = AtomicU64::new(0);

impl PackageManager {
    pub fn new() -> Self {
        Self {
            installed_db: InstalledDatabase::new(),
            repos: Vec::new(),
            keyring: GpgKeyring::new(),
            history: TransactionHistory::new(),
            hooks: HookRunner::new(),
            lock: PackageLock::new(),
            downloads: DownloadManager::new(5),
            cache: PackageCache::new(String::from("/var/cache/raepkg")),
            groups: Vec::new(),
            user_repo: UserRepository::new(),
            next_txn_id: 1,
            initialized: false,
        }
    }

    pub fn add_repo(&mut self, config: RepoConfig) {
        self.repos.push(RepoDatabase::new(config));
    }

    pub fn sync_repos(&mut self, timestamp: u64) -> Result<()> {
        for repo in &mut self.repos {
            if repo.config.enabled {
                repo.sync(timestamp)?;
            }
        }
        Ok(())
    }

    pub fn install(&mut self, names: &[String], timestamp: u64) -> Result<u64> {
        self.lock.acquire(1, timestamp)?;

        let mut solver = DependencySolver::new();
        for pkg in self.installed_db.packages.values() {
            solver.add_installed(pkg.name.clone(), pkg.version.clone());
        }
        for repo in &self.repos {
            for entries in repo.packages.values() {
                for entry in entries {
                    solver.add_available(entry.clone());
                }
            }
        }

        let to_install = solver.resolve(names)?;
        let mut txn = Transaction::new(self.next_txn_id);
        self.next_txn_id += 1;

        for name in &to_install {
            let entry = self.find_in_repos(name).ok_or(PackageError::NotFound)?;
            txn.add_item(TransactionItem {
                op: TransactionOp::Install,
                package_name: name.clone(),
                old_version: None,
                new_version: Some(entry.version.clone()),
                download_size: entry.size,
                install_size: entry.installed_size as i64,
            });
        }

        let targets: Vec<String> = to_install.clone();
        self.hooks.run_pre_hooks(HookOperation::Install, &targets)?;
        txn.execute(&mut self.installed_db)?;
        self.hooks
            .run_post_hooks(HookOperation::Install, &targets)?;

        let txn_id = txn.id;
        self.history.record(txn.items, timestamp);
        TOTAL_TRANSACTIONS.fetch_add(1, Ordering::Relaxed);

        let _ = self.lock.release(1);
        Ok(txn_id)
    }

    pub fn remove(&mut self, names: &[String], timestamp: u64) -> Result<u64> {
        self.lock.acquire(1, timestamp)?;

        let mut txn = Transaction::new(self.next_txn_id);
        self.next_txn_id += 1;

        for name in names {
            let pkg = self.installed_db.get(name).ok_or(PackageError::NotFound)?;
            if pkg.hold == PackageHold::Hold {
                let _ = self.lock.release(1);
                return Err(PackageError::PermissionDenied);
            }
            txn.add_item(TransactionItem {
                op: TransactionOp::Remove,
                package_name: name.clone(),
                old_version: Some(pkg.version.clone()),
                new_version: None,
                download_size: 0,
                install_size: -(pkg.files.len() as i64 * 1024),
            });
        }

        let targets: Vec<String> = names.to_vec();
        self.hooks.run_pre_hooks(HookOperation::Remove, &targets)?;
        txn.execute(&mut self.installed_db)?;
        self.hooks.run_post_hooks(HookOperation::Remove, &targets)?;

        let txn_id = txn.id;
        self.history.record(txn.items, timestamp);
        TOTAL_TRANSACTIONS.fetch_add(1, Ordering::Relaxed);

        let _ = self.lock.release(1);
        Ok(txn_id)
    }

    pub fn upgrade(&mut self, timestamp: u64) -> Result<u64> {
        self.lock.acquire(1, timestamp)?;

        let mut txn = Transaction::new(self.next_txn_id);
        self.next_txn_id += 1;

        let installed: Vec<(String, Version)> = self
            .installed_db
            .packages
            .values()
            .filter(|p| p.hold == PackageHold::None)
            .map(|p| (p.name.clone(), p.version.clone()))
            .collect();

        for (name, current_version) in &installed {
            if let Some(entry) = self.find_in_repos(name) {
                if entry.version.compare(current_version) == core::cmp::Ordering::Greater {
                    txn.add_item(TransactionItem {
                        op: TransactionOp::Upgrade,
                        package_name: name.clone(),
                        old_version: Some(current_version.clone()),
                        new_version: Some(entry.version.clone()),
                        download_size: entry.size,
                        install_size: entry.installed_size as i64,
                    });
                }
            }
        }

        if txn.items.is_empty() {
            let _ = self.lock.release(1);
            return Ok(0);
        }

        let targets: Vec<String> = txn.items.iter().map(|i| i.package_name.clone()).collect();
        self.hooks.run_pre_hooks(HookOperation::Upgrade, &targets)?;
        txn.execute(&mut self.installed_db)?;
        self.hooks
            .run_post_hooks(HookOperation::Upgrade, &targets)?;

        let txn_id = txn.id;
        self.history.record(txn.items, timestamp);
        TOTAL_TRANSACTIONS.fetch_add(1, Ordering::Relaxed);

        let _ = self.lock.release(1);
        Ok(txn_id)
    }

    pub fn autoremove(&mut self, timestamp: u64) -> Result<u64> {
        let orphans: Vec<String> = self
            .installed_db
            .list_orphans()
            .iter()
            .map(|p| p.name.clone())
            .collect();
        if orphans.is_empty() {
            return Ok(0);
        }
        self.remove(&orphans, timestamp)
    }

    pub fn search(&self, query: &str) -> Vec<&RepoPackageEntry> {
        let mut results = Vec::new();
        for repo in &self.repos {
            results.extend(repo.search(query));
        }
        results
    }

    pub fn info(&self, name: &str) -> Option<PackageInfo> {
        if let Some(installed) = self.installed_db.get(name) {
            return Some(PackageInfo {
                name: installed.name.clone(),
                version: installed.version.clone(),
                is_installed: true,
                reason: Some(installed.reason),
                hold: installed.hold,
            });
        }
        if let Some(entry) = self.find_in_repos(name) {
            return Some(PackageInfo {
                name: entry.name.clone(),
                version: entry.version.clone(),
                is_installed: false,
                reason: None,
                hold: PackageHold::None,
            });
        }
        None
    }

    fn find_in_repos(&self, name: &str) -> Option<RepoPackageEntry> {
        let mut best: Option<RepoPackageEntry> = None;
        for repo in &self.repos {
            if let Some(entry) = repo.find_package(name) {
                if let Some(ref current) = best {
                    if entry.version.compare(&current.version) == core::cmp::Ordering::Greater {
                        best = Some(entry.clone());
                    }
                } else {
                    best = Some(entry.clone());
                }
            }
        }
        best
    }

    pub fn installed_count(&self) -> usize {
        self.installed_db.packages.len()
    }

    pub fn total_transactions() -> u64 {
        TOTAL_TRANSACTIONS.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Clone)]
pub struct PackageInfo {
    pub name: String,
    pub version: Version,
    pub is_installed: bool,
    pub reason: Option<PackageReason>,
    pub hold: PackageHold,
}

// ─── Global Package Manager ─────────────────────────────────────────────────

static mut PACKAGE_MANAGER: Option<PackageManager> = None;

pub fn init() {
    if PM_INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        PACKAGE_MANAGER = Some(PackageManager::new());
    }
}

pub fn pm() -> &'static PackageManager {
    unsafe {
        PACKAGE_MANAGER
            .as_ref()
            .expect("package manager not initialized")
    }
}

pub fn pm_mut() -> &'static mut PackageManager {
    unsafe {
        PACKAGE_MANAGER
            .as_mut()
            .expect("package manager not initialized")
    }
}

#[cfg(test)]
mod crypto_tests {
    use super::*;

    #[test]
    fn checksums_are_real_hashes() {
        // compute_sha256 must now equal real SHA-256 (was a homebrew XOR loop).
        assert_eq!(
            compute_sha256(b"abc"),
            rae_crypto::sha256::sha256(b"abc").to_vec()
        );
        let empty = compute_sha256(b"");
        assert_eq!(empty[0], 0xe3); // SHA-256("") = e3b0c442...
        assert_eq!(empty[31], 0x55);
        assert_eq!(
            compute_sha512(b"abc"),
            rae_crypto::ed25519::sha512(b"abc").to_vec()
        );
        assert_eq!(compute_blake2b(b"abc"), rae_crypto::blake2b(64, b"abc"));
    }

    fn signed_package(
        data: &[u8],
        seed: &[u8; 32],
        key_id: &str,
        trust: KeyTrust,
    ) -> (Package, GpgKeyring) {
        let mut pkg = Package::new(PackageHeader::new(
            String::from("testpkg"),
            Version::new(String::from("1.0")),
        ));
        pkg.data = data.to_vec();
        let hash = rae_crypto::sha256::sha256(data).to_vec();
        let sig = rae_crypto::ed25519::sign(seed, &hash).to_vec();
        pkg.signature = Some(PackageSignature {
            key_id: String::from(key_id),
            signature: sig,
            signed_data_hash: hash,
        });
        let mut keyring = GpgKeyring::new();
        keyring
            .import_key(GpgKey {
                id: String::from(key_id),
                fingerprint: String::new(),
                owner: String::from("test"),
                trust,
                public_key: rae_crypto::ed25519::derive_public_key(seed).to_vec(),
                created: 0,
                expires: 0,
            })
            .unwrap();
        (pkg, keyring)
    }

    #[test]
    fn valid_signature_accepted() {
        let (pkg, keyring) =
            signed_package(b"hello package payload", &[7u8; 32], "dev", KeyTrust::Full);
        assert!(pkg.verify_signature(&keyring).is_ok());
    }

    #[test]
    fn forged_signature_rejected() {
        let (mut pkg, keyring) =
            signed_package(b"hello package payload", &[7u8; 32], "dev", KeyTrust::Full);
        pkg.signature.as_mut().unwrap().signature[0] ^= 0x01;
        assert_eq!(
            pkg.verify_signature(&keyring),
            Err(PackageError::SignatureInvalid)
        );
    }

    #[test]
    fn tampered_payload_rejected() {
        let (mut pkg, keyring) =
            signed_package(b"hello package payload", &[7u8; 32], "dev", KeyTrust::Full);
        // Changing data after signing breaks the signed-hash binding.
        pkg.data.push(0xFF);
        assert_eq!(
            pkg.verify_signature(&keyring),
            Err(PackageError::SignatureInvalid)
        );
    }

    #[test]
    fn wrong_key_rejected() {
        let (pkg, _) = signed_package(b"payload", &[7u8; 32], "dev", KeyTrust::Full);
        // A keyring holding a DIFFERENT keypair under the same id must reject.
        let mut keyring = GpgKeyring::new();
        keyring
            .import_key(GpgKey {
                id: String::from("dev"),
                fingerprint: String::new(),
                owner: String::from("attacker"),
                trust: KeyTrust::Full,
                public_key: rae_crypto::ed25519::derive_public_key(&[9u8; 32]).to_vec(),
                created: 0,
                expires: 0,
            })
            .unwrap();
        assert_eq!(
            pkg.verify_signature(&keyring),
            Err(PackageError::SignatureInvalid)
        );
    }

    #[test]
    fn revoked_key_rejected() {
        let (pkg, keyring) = signed_package(b"payload", &[7u8; 32], "dev", KeyTrust::Revoked);
        assert_eq!(
            pkg.verify_signature(&keyring),
            Err(PackageError::SignatureInvalid)
        );
    }
}
