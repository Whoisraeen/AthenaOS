//! RaeFS — copy-on-write filesystem with snapshots, tiered storage, native encryption.
//!
//! Userspace library providing:
//! - Path manipulation and normalization
//! - File/directory metadata and permission model
//! - Copy-on-write snapshot management
//! - Tiered storage placement hints
//! - Transparent compression (Zstd) interface
//! - Per-app data bucket isolation
//! - Versioned config tracking

// no_std for real builds; std under `cargo test` so the ntfs_probe host KAT links.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod fsck;
#[cfg(feature = "ntfs_ro")]
pub mod ntfs_probe;
pub mod redoxfs_adapter;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use ruzstd::io_nostd::Read;

// ── Errors ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsError {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    NotADirectory,
    NotAFile,
    IsDirectory,
    DiskFull,
    SnapshotLimitReached,
    InvalidPath,
    CorruptedData,
    EncryptionError,
    IoError,
    BucketViolation,
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => write!(f, "not found"),
            Self::PermissionDenied => write!(f, "permission denied"),
            Self::AlreadyExists => write!(f, "already exists"),
            Self::NotADirectory => write!(f, "not a directory"),
            Self::NotAFile => write!(f, "not a file"),
            Self::IsDirectory => write!(f, "is a directory"),
            Self::DiskFull => write!(f, "disk full"),
            Self::SnapshotLimitReached => write!(f, "snapshot limit reached"),
            Self::InvalidPath => write!(f, "invalid path"),
            Self::CorruptedData => write!(f, "corrupted data"),
            Self::EncryptionError => write!(f, "encryption error"),
            Self::IoError => write!(f, "I/O error"),
            Self::BucketViolation => write!(f, "app bucket access violation"),
        }
    }
}

pub type FsResult<T> = Result<T, FsError>;

// ── Inode and block types ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InodeId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SnapshotId(pub u64);

// ── Path manipulation ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaePath {
    inner: String,
}

impl RaePath {
    pub fn new(s: &str) -> Self {
        Self {
            inner: String::from(s),
        }
    }

    pub fn root() -> Self {
        Self {
            inner: String::from("/"),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.inner
    }

    pub fn is_absolute(&self) -> bool {
        self.inner.starts_with('/')
    }

    pub fn is_root(&self) -> bool {
        self.inner == "/"
    }

    pub fn parent(&self) -> Option<RaePath> {
        if self.is_root() {
            return None;
        }
        let trimmed = self.inner.trim_end_matches('/');
        match trimmed.rfind('/') {
            Some(0) => Some(RaePath::root()),
            Some(pos) => Some(RaePath::new(&trimmed[..pos])),
            None => None,
        }
    }

    pub fn file_name(&self) -> Option<&str> {
        let trimmed = self.inner.trim_end_matches('/');
        if trimmed.is_empty() {
            return None;
        }
        match trimmed.rfind('/') {
            Some(pos) => {
                let name = &trimmed[pos + 1..];
                if name.is_empty() {
                    None
                } else {
                    Some(name)
                }
            }
            None => Some(trimmed),
        }
    }

    pub fn extension(&self) -> Option<&str> {
        self.file_name().and_then(|name| {
            let dot = name.rfind('.')?;
            if dot == 0 || dot == name.len() - 1 {
                None
            } else {
                Some(&name[dot + 1..])
            }
        })
    }

    pub fn join(&self, other: &str) -> RaePath {
        if other.starts_with('/') {
            return RaePath::new(other);
        }
        let mut s = self.inner.clone();
        if !s.ends_with('/') {
            s.push('/');
        }
        s.push_str(other);
        RaePath { inner: s }
    }

    pub fn components(&self) -> Vec<&str> {
        self.inner.split('/').filter(|c| !c.is_empty()).collect()
    }

    pub fn normalize(&self) -> RaePath {
        let is_abs = self.is_absolute();
        let mut parts: Vec<&str> = Vec::new();
        for c in self.components() {
            match c {
                "." => {}
                ".." => {
                    parts.pop();
                }
                other => parts.push(other),
            }
        }
        let mut result = String::new();
        if is_abs {
            result.push('/');
        }
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                result.push('/');
            }
            result.push_str(part);
        }
        if result.is_empty() {
            result.push('.');
        }
        RaePath { inner: result }
    }
}

impl fmt::Display for RaePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

// ── Permissions ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    pub bits: u16,
}

impl Permissions {
    pub const OWNER_READ: u16 = 0o400;
    pub const OWNER_WRITE: u16 = 0o200;
    pub const OWNER_EXEC: u16 = 0o100;
    pub const GROUP_READ: u16 = 0o040;
    pub const GROUP_WRITE: u16 = 0o020;
    pub const GROUP_EXEC: u16 = 0o010;
    pub const OTHER_READ: u16 = 0o004;
    pub const OTHER_WRITE: u16 = 0o002;
    pub const OTHER_EXEC: u16 = 0o001;

    pub const fn new(bits: u16) -> Self {
        Self { bits }
    }

    pub const fn default_file() -> Self {
        Self { bits: 0o644 }
    }

    pub const fn default_dir() -> Self {
        Self { bits: 0o755 }
    }

    pub fn is_readable(&self) -> bool {
        self.bits & Self::OWNER_READ != 0
    }

    pub fn is_writable(&self) -> bool {
        self.bits & Self::OWNER_WRITE != 0
    }

    pub fn is_executable(&self) -> bool {
        self.bits & Self::OWNER_EXEC != 0
    }
}

// ── File types and metadata ──────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    RegularFile,
    Directory,
    Symlink,
    Device,
    Pipe,
    Socket,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageTier {
    NVMe,
    Sata,
    Spinning,
    Archive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionAlgo {
    None,
    Zstd,
    Lz4,
}

pub struct ZstdDecompressor;

impl ZstdDecompressor {
    pub fn new() -> Self {
        Self
    }

    pub fn decompress(&self, input: &[u8], output: &mut [u8]) -> FsResult<usize> {
        let mut decoder =
            ruzstd::StreamingDecoder::new(input).map_err(|_| FsError::CorruptedData)?;
        let mut total_out = 0;
        loop {
            let n = decoder
                .read(&mut output[total_out..])
                .map_err(|_| FsError::CorruptedData)?;
            if n == 0 {
                break;
            }
            total_out += n;
        }
        Ok(total_out)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionMode {
    None,
    Aes256Gcm,
    ChaCha20Poly1305,
}

#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub inode: InodeId,
    pub file_type: FileType,
    pub size: u64,
    pub blocks_used: u64,
    pub permissions: Permissions,
    pub owner_uid: u32,
    pub owner_gid: u32,
    pub created_at: u64,
    pub modified_at: u64,
    pub accessed_at: u64,
    pub link_count: u32,
    pub storage_tier: StorageTier,
    pub compression: CompressionAlgo,
    pub encryption: EncryptionMode,
    pub cow_generation: u64,
}

impl FileMetadata {
    pub fn is_file(&self) -> bool {
        self.file_type == FileType::RegularFile
    }
    pub fn is_dir(&self) -> bool {
        self.file_type == FileType::Directory
    }
    pub fn is_symlink(&self) -> bool {
        self.file_type == FileType::Symlink
    }
}

// ── Directory entry ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub inode: InodeId,
    pub file_type: FileType,
}

// ── Copy-on-Write / Snapshots ────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotType {
    Manual,
    Automatic,
    PreUpdate,
    ConfigVersion,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub id: SnapshotId,
    pub parent: Option<SnapshotId>,
    pub snap_type: SnapshotType,
    pub label: String,
    pub created_at: u64,
    pub root_inode: InodeId,
    pub blocks_exclusive: u64,
    pub blocks_shared: u64,
}

#[derive(Debug)]
pub struct SnapshotManager {
    snapshots: BTreeMap<u64, Snapshot>,
    next_id: u64,
    max_snapshots: usize,
}

impl SnapshotManager {
    pub fn new(max_snapshots: usize) -> Self {
        Self {
            snapshots: BTreeMap::new(),
            next_id: 1,
            max_snapshots,
        }
    }

    pub fn create(
        &mut self,
        snap_type: SnapshotType,
        label: &str,
        root_inode: InodeId,
        parent: Option<SnapshotId>,
        now: u64,
    ) -> FsResult<SnapshotId> {
        if self.snapshots.len() >= self.max_snapshots {
            return Err(FsError::SnapshotLimitReached);
        }
        let id = SnapshotId(self.next_id);
        self.next_id += 1;
        let snap = Snapshot {
            id,
            parent,
            snap_type,
            label: String::from(label),
            created_at: now,
            root_inode,
            blocks_exclusive: 0,
            blocks_shared: 0,
        };
        self.snapshots.insert(id.0, snap);
        Ok(id)
    }

    pub fn delete(&mut self, id: SnapshotId) -> FsResult<()> {
        self.snapshots
            .remove(&id.0)
            .map(|_| ())
            .ok_or(FsError::NotFound)
    }

    pub fn get(&self, id: SnapshotId) -> Option<&Snapshot> {
        self.snapshots.get(&id.0)
    }

    pub fn list(&self) -> Vec<&Snapshot> {
        self.snapshots.values().collect()
    }

    pub fn list_by_type(&self, snap_type: SnapshotType) -> Vec<&Snapshot> {
        self.snapshots
            .values()
            .filter(|s| s.snap_type == snap_type)
            .collect()
    }

    pub fn rollback_target(&self, id: SnapshotId) -> FsResult<InodeId> {
        self.get(id).map(|s| s.root_inode).ok_or(FsError::NotFound)
    }
}

// ── Tiered storage placement ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementHint {
    Default,
    Hot,
    Cold,
    GameAsset,
    StreamingMedia,
}

#[derive(Debug, Clone)]
pub struct TierInfo {
    pub tier: StorageTier,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub read_bandwidth_mbps: u32,
    pub write_bandwidth_mbps: u32,
    pub latency_us: u32,
}

impl TierInfo {
    pub fn free_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.used_bytes)
    }

    pub fn usage_percent(&self) -> u8 {
        if self.total_bytes == 0 {
            return 100;
        }
        ((self.used_bytes * 100) / self.total_bytes) as u8
    }
}

#[derive(Debug)]
pub struct TierManager {
    tiers: Vec<TierInfo>,
    promotion_threshold: u32,
    demotion_threshold: u32,
}

impl TierManager {
    pub fn new() -> Self {
        Self {
            tiers: Vec::new(),
            promotion_threshold: 80,
            demotion_threshold: 30,
        }
    }

    pub fn add_tier(&mut self, tier: TierInfo) {
        self.tiers.push(tier);
        self.tiers
            .sort_by_key(|t| core::cmp::Reverse(t.read_bandwidth_mbps));
    }

    pub fn best_tier_for(&self, hint: PlacementHint) -> Option<StorageTier> {
        match hint {
            PlacementHint::Hot | PlacementHint::GameAsset => self.tiers.first().map(|t| t.tier),
            PlacementHint::Cold => self.tiers.last().map(|t| t.tier),
            PlacementHint::StreamingMedia => self
                .tiers
                .iter()
                .find(|t| t.read_bandwidth_mbps >= 500)
                .or(self.tiers.first())
                .map(|t| t.tier),
            PlacementHint::Default => self
                .tiers
                .iter()
                .find(|t| t.usage_percent() < self.promotion_threshold as u8)
                .or(self.tiers.first())
                .map(|t| t.tier),
        }
    }

    pub fn should_promote(&self, current: StorageTier, access_count: u64) -> bool {
        let threshold = match current {
            StorageTier::Spinning => 5,
            StorageTier::Sata => 20,
            StorageTier::NVMe => return false,
            StorageTier::Archive => 2,
        };
        access_count >= threshold
    }

    pub fn should_demote(&self, current: StorageTier, idle_secs: u64) -> bool {
        let threshold = match current {
            StorageTier::NVMe => 86400,  // 1 day
            StorageTier::Sata => 604800, // 1 week
            StorageTier::Spinning => return false,
            StorageTier::Archive => return false,
        };
        idle_secs >= threshold
    }

    pub fn total_capacity(&self) -> u64 {
        self.tiers.iter().map(|t| t.total_bytes).sum()
    }

    pub fn total_used(&self) -> u64 {
        self.tiers.iter().map(|t| t.used_bytes).sum()
    }
}

// ── Per-app data buckets ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AppId(pub u64);

#[derive(Debug, Clone)]
pub struct DataBucket {
    pub app_id: AppId,
    pub root_path: RaePath,
    pub quota_bytes: u64,
    pub used_bytes: u64,
    pub encryption: EncryptionMode,
    pub created_at: u64,
}

impl DataBucket {
    pub fn remaining_quota(&self) -> u64 {
        self.quota_bytes.saturating_sub(self.used_bytes)
    }

    pub fn is_over_quota(&self) -> bool {
        self.used_bytes > self.quota_bytes
    }
}

#[derive(Debug)]
pub struct BucketManager {
    buckets: BTreeMap<u64, DataBucket>,
}

impl BucketManager {
    pub fn new() -> Self {
        Self {
            buckets: BTreeMap::new(),
        }
    }

    pub fn create_bucket(
        &mut self,
        app_id: AppId,
        quota_bytes: u64,
        encryption: EncryptionMode,
        now: u64,
    ) -> FsResult<&DataBucket> {
        if self.buckets.contains_key(&app_id.0) {
            return Err(FsError::AlreadyExists);
        }
        let root = RaePath::new("/data/apps").join(&alloc::format!("{}", app_id.0));
        let bucket = DataBucket {
            app_id,
            root_path: root,
            quota_bytes,
            used_bytes: 0,
            encryption,
            created_at: now,
        };
        self.buckets.insert(app_id.0, bucket);
        Ok(self.buckets.get(&app_id.0).unwrap())
    }

    pub fn get_bucket(&self, app_id: AppId) -> Option<&DataBucket> {
        self.buckets.get(&app_id.0)
    }

    pub fn delete_bucket(&mut self, app_id: AppId) -> FsResult<()> {
        self.buckets
            .remove(&app_id.0)
            .map(|_| ())
            .ok_or(FsError::NotFound)
    }

    pub fn check_access(&self, app_id: AppId, path: &RaePath) -> FsResult<()> {
        let bucket = self.get_bucket(app_id).ok_or(FsError::NotFound)?;
        let bucket_prefix = bucket.root_path.as_str();
        if path.as_str().starts_with(bucket_prefix) {
            Ok(())
        } else {
            Err(FsError::BucketViolation)
        }
    }

    pub fn record_usage(&mut self, app_id: AppId, bytes: u64) -> FsResult<()> {
        let bucket = self.buckets.get_mut(&app_id.0).ok_or(FsError::NotFound)?;
        bucket.used_bytes = bucket.used_bytes.saturating_add(bytes);
        if bucket.is_over_quota() {
            Err(FsError::DiskFull)
        } else {
            Ok(())
        }
    }
}

// ── Versioned config ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ConfigVersion {
    pub version: u64,
    pub data: Vec<u8>,
    pub timestamp: u64,
    pub label: Option<String>,
}

#[derive(Debug)]
pub struct VersionedConfig {
    pub path: RaePath,
    pub versions: Vec<ConfigVersion>,
    pub max_versions: usize,
}

impl VersionedConfig {
    pub fn new(path: RaePath, max_versions: usize) -> Self {
        Self {
            path,
            versions: Vec::new(),
            max_versions,
        }
    }

    pub fn save(&mut self, data: Vec<u8>, timestamp: u64, label: Option<&str>) -> u64 {
        let version = self.versions.last().map(|v| v.version + 1).unwrap_or(1);
        let entry = ConfigVersion {
            version,
            data,
            timestamp,
            label: label.map(String::from),
        };
        self.versions.push(entry);
        while self.versions.len() > self.max_versions {
            self.versions.remove(0);
        }
        version
    }

    pub fn current(&self) -> Option<&ConfigVersion> {
        self.versions.last()
    }

    pub fn get_version(&self, version: u64) -> Option<&ConfigVersion> {
        self.versions.iter().find(|v| v.version == version)
    }

    pub fn rollback(&mut self, version: u64) -> FsResult<&ConfigVersion> {
        let idx = self
            .versions
            .iter()
            .position(|v| v.version == version)
            .ok_or(FsError::NotFound)?;
        self.versions.truncate(idx + 1);
        self.versions.last().ok_or(FsError::NotFound)
    }

    pub fn history(&self) -> &[ConfigVersion] {
        &self.versions
    }

    pub fn diff_versions(&self, a: u64, b: u64) -> FsResult<(Vec<u8>, Vec<u8>)> {
        let va = self.get_version(a).ok_or(FsError::NotFound)?;
        let vb = self.get_version(b).ok_or(FsError::NotFound)?;
        Ok((va.data.clone(), vb.data.clone()))
    }
}

// ── Game-aware extents ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ExtentHint {
    pub sequential: bool,
    pub expected_size: u64,
    pub contiguous: bool,
    pub read_ahead_bytes: u64,
}

impl ExtentHint {
    pub fn game_install(expected_size: u64) -> Self {
        Self {
            sequential: true,
            expected_size,
            contiguous: true,
            read_ahead_bytes: 4 * 1024 * 1024, // 4 MiB read-ahead
        }
    }

    pub fn streaming_asset() -> Self {
        Self {
            sequential: true,
            expected_size: 0,
            contiguous: false,
            read_ahead_bytes: 1024 * 1024, // 1 MiB
        }
    }

    pub fn database() -> Self {
        Self {
            sequential: false,
            expected_size: 0,
            contiguous: false,
            read_ahead_bytes: 0,
        }
    }
}

// ── Filesystem handle / operations ───────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileHandle(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    ReadOnly,
    WriteOnly,
    ReadWrite,
    Append,
    Create,
    CreateNew,
    Truncate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekFrom {
    Start(u64),
    End(i64),
    Current(i64),
}

pub trait FileSystem {
    fn open(&mut self, path: &RaePath, mode: OpenMode) -> FsResult<FileHandle>;
    fn close(&mut self, handle: FileHandle) -> FsResult<()>;
    fn read(&mut self, handle: FileHandle, buf: &mut [u8]) -> FsResult<usize>;
    fn write(&mut self, handle: FileHandle, buf: &[u8]) -> FsResult<usize>;
    fn seek(&mut self, handle: FileHandle, pos: SeekFrom) -> FsResult<u64>;
    fn stat(&self, path: &RaePath) -> FsResult<FileMetadata>;
    fn mkdir(&mut self, path: &RaePath, perms: Permissions) -> FsResult<()>;
    fn rmdir(&mut self, path: &RaePath) -> FsResult<()>;
    fn unlink(&mut self, path: &RaePath) -> FsResult<()>;
    fn rename(&mut self, from: &RaePath, to: &RaePath) -> FsResult<()>;
    fn readdir(&self, path: &RaePath) -> FsResult<Vec<DirEntry>>;
    fn symlink(&mut self, target: &RaePath, link: &RaePath) -> FsResult<()>;
    fn readlink(&self, path: &RaePath) -> FsResult<RaePath>;
    fn sync(&mut self, handle: FileHandle) -> FsResult<()>;

    fn create_snapshot(&mut self, snap_type: SnapshotType, label: &str) -> FsResult<SnapshotId>;

    fn restore_snapshot(&mut self, id: SnapshotId) -> FsResult<()>;

    fn set_extent_hint(&mut self, handle: FileHandle, hint: &ExtentHint) -> FsResult<()>;
}

// ── Filesystem statistics ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FsStats {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub total_inodes: u64,
    pub used_inodes: u64,
    pub snapshot_count: u32,
    pub block_size: u32,
    pub compression_ratio: u32, // percentage, e.g. 65 means data compressed to 65% of original
}

impl FsStats {
    pub fn usage_percent(&self) -> u8 {
        if self.total_bytes == 0 {
            return 100;
        }
        ((self.used_bytes * 100) / self.total_bytes) as u8
    }

    pub fn savings_from_compression(&self) -> u64 {
        if self.compression_ratio >= 100 {
            return 0;
        }
        let uncompressed_estimate = (self.used_bytes * 100) / self.compression_ratio.max(1) as u64;
        uncompressed_estimate.saturating_sub(self.used_bytes)
    }
}

// ── In-memory filesystem (testing / early boot) ──────────────────────────

#[derive(Debug, Clone)]
struct MemInode {
    metadata: FileMetadata,
    data: Vec<u8>,
    children: BTreeMap<String, InodeId>,
    symlink_target: Option<RaePath>,
}

pub struct MemFs {
    inodes: BTreeMap<u64, MemInode>,
    next_inode: u64,
    next_handle: u64,
    handles: BTreeMap<u64, (InodeId, u64, OpenMode)>, // handle -> (inode, offset, mode)
    snapshots: SnapshotManager,
}

impl MemFs {
    pub fn new() -> Self {
        let mut inodes = BTreeMap::new();
        let root = MemInode {
            metadata: FileMetadata {
                inode: InodeId(1),
                file_type: FileType::Directory,
                size: 0,
                blocks_used: 0,
                permissions: Permissions::default_dir(),
                owner_uid: 0,
                owner_gid: 0,
                created_at: 0,
                modified_at: 0,
                accessed_at: 0,
                link_count: 2,
                storage_tier: StorageTier::NVMe,
                compression: CompressionAlgo::None,
                encryption: EncryptionMode::None,
                cow_generation: 0,
            },
            data: Vec::new(),
            children: BTreeMap::new(),
            symlink_target: None,
        };
        inodes.insert(1, root);

        Self {
            inodes,
            next_inode: 2,
            next_handle: 1,
            handles: BTreeMap::new(),
            snapshots: SnapshotManager::new(256),
        }
    }

    fn resolve_inode(&self, path: &RaePath) -> FsResult<InodeId> {
        if path.is_root() {
            return Ok(InodeId(1));
        }
        let parts = path.components();
        let mut current = InodeId(1);
        for part in parts {
            let inode = self.inodes.get(&current.0).ok_or(FsError::NotFound)?;
            if inode.metadata.file_type != FileType::Directory {
                return Err(FsError::NotADirectory);
            }
            current = *inode.children.get(part).ok_or(FsError::NotFound)?;
        }
        Ok(current)
    }

    fn alloc_inode(&mut self) -> InodeId {
        let id = InodeId(self.next_inode);
        self.next_inode += 1;
        id
    }
}

impl FileSystem for MemFs {
    fn open(&mut self, path: &RaePath, mode: OpenMode) -> FsResult<FileHandle> {
        let inode_id = match self.resolve_inode(path) {
            Ok(id) => {
                if matches!(mode, OpenMode::CreateNew) {
                    return Err(FsError::AlreadyExists);
                }
                if matches!(mode, OpenMode::Truncate) {
                    let inode = self.inodes.get_mut(&id.0).ok_or(FsError::NotFound)?;
                    inode.data.clear();
                    inode.metadata.size = 0;
                }
                id
            }
            Err(FsError::NotFound) if matches!(mode, OpenMode::Create | OpenMode::CreateNew) => {
                let parent_path = path.parent().ok_or(FsError::InvalidPath)?;
                let name = path.file_name().ok_or(FsError::InvalidPath)?;
                let parent_id = self.resolve_inode(&parent_path)?;
                let new_id = self.alloc_inode();

                let inode = MemInode {
                    metadata: FileMetadata {
                        inode: new_id,
                        file_type: FileType::RegularFile,
                        size: 0,
                        blocks_used: 0,
                        permissions: Permissions::default_file(),
                        owner_uid: 0,
                        owner_gid: 0,
                        created_at: 0,
                        modified_at: 0,
                        accessed_at: 0,
                        link_count: 1,
                        storage_tier: StorageTier::NVMe,
                        compression: CompressionAlgo::None,
                        encryption: EncryptionMode::None,
                        cow_generation: 0,
                    },
                    data: Vec::new(),
                    children: BTreeMap::new(),
                    symlink_target: None,
                };
                self.inodes.insert(new_id.0, inode);

                let parent = self.inodes.get_mut(&parent_id.0).ok_or(FsError::NotFound)?;
                parent.children.insert(String::from(name), new_id);
                new_id
            }
            Err(e) => return Err(e),
        };

        let handle = FileHandle(self.next_handle);
        self.next_handle += 1;
        let offset = if matches!(mode, OpenMode::Append) {
            let inode = self.inodes.get(&inode_id.0).ok_or(FsError::NotFound)?;
            inode.data.len() as u64
        } else {
            0
        };
        self.handles.insert(handle.0, (inode_id, offset, mode));
        Ok(handle)
    }

    fn close(&mut self, handle: FileHandle) -> FsResult<()> {
        self.handles
            .remove(&handle.0)
            .map(|_| ())
            .ok_or(FsError::NotFound)
    }

    fn read(&mut self, handle: FileHandle, buf: &mut [u8]) -> FsResult<usize> {
        let (inode_id, offset, _mode) = self
            .handles
            .get(&handle.0)
            .copied()
            .ok_or(FsError::NotFound)?;
        let inode = self.inodes.get(&inode_id.0).ok_or(FsError::NotFound)?;
        let start = offset as usize;
        if start >= inode.data.len() {
            return Ok(0);
        }
        let available = inode.data.len() - start;
        let to_read = buf.len().min(available);
        buf[..to_read].copy_from_slice(&inode.data[start..start + to_read]);
        let h = self.handles.get_mut(&handle.0).unwrap();
        h.1 += to_read as u64;
        Ok(to_read)
    }

    fn write(&mut self, handle: FileHandle, buf: &[u8]) -> FsResult<usize> {
        let (inode_id, offset, mode) = self
            .handles
            .get(&handle.0)
            .copied()
            .ok_or(FsError::NotFound)?;
        if matches!(mode, OpenMode::ReadOnly) {
            return Err(FsError::PermissionDenied);
        }
        let inode = self.inodes.get_mut(&inode_id.0).ok_or(FsError::NotFound)?;
        let start = offset as usize;
        let end = start + buf.len();
        if end > inode.data.len() {
            inode.data.resize(end, 0);
        }
        inode.data[start..end].copy_from_slice(buf);
        inode.metadata.size = inode.data.len() as u64;
        inode.metadata.blocks_used = (inode.metadata.size + 4095) / 4096;

        let h = self.handles.get_mut(&handle.0).unwrap();
        h.1 = end as u64;
        Ok(buf.len())
    }

    fn seek(&mut self, handle: FileHandle, pos: SeekFrom) -> FsResult<u64> {
        let (inode_id, offset, _) = self
            .handles
            .get(&handle.0)
            .copied()
            .ok_or(FsError::NotFound)?;
        let inode = self.inodes.get(&inode_id.0).ok_or(FsError::NotFound)?;
        let new_offset = match pos {
            SeekFrom::Start(n) => n,
            SeekFrom::End(n) => (inode.data.len() as i64 + n).max(0) as u64,
            SeekFrom::Current(n) => (offset as i64 + n).max(0) as u64,
        };
        let h = self.handles.get_mut(&handle.0).unwrap();
        h.1 = new_offset;
        Ok(new_offset)
    }

    fn stat(&self, path: &RaePath) -> FsResult<FileMetadata> {
        let id = self.resolve_inode(path)?;
        let inode = self.inodes.get(&id.0).ok_or(FsError::NotFound)?;
        Ok(inode.metadata.clone())
    }

    fn mkdir(&mut self, path: &RaePath, perms: Permissions) -> FsResult<()> {
        if self.resolve_inode(path).is_ok() {
            return Err(FsError::AlreadyExists);
        }
        let parent_path = path.parent().ok_or(FsError::InvalidPath)?;
        let name = path.file_name().ok_or(FsError::InvalidPath)?;
        let parent_id = self.resolve_inode(&parent_path)?;
        let new_id = self.alloc_inode();

        let inode = MemInode {
            metadata: FileMetadata {
                inode: new_id,
                file_type: FileType::Directory,
                size: 0,
                blocks_used: 0,
                permissions: perms,
                owner_uid: 0,
                owner_gid: 0,
                created_at: 0,
                modified_at: 0,
                accessed_at: 0,
                link_count: 2,
                storage_tier: StorageTier::NVMe,
                compression: CompressionAlgo::None,
                encryption: EncryptionMode::None,
                cow_generation: 0,
            },
            data: Vec::new(),
            children: BTreeMap::new(),
            symlink_target: None,
        };
        self.inodes.insert(new_id.0, inode);

        let parent = self.inodes.get_mut(&parent_id.0).ok_or(FsError::NotFound)?;
        parent.children.insert(String::from(name), new_id);
        Ok(())
    }

    fn rmdir(&mut self, path: &RaePath) -> FsResult<()> {
        let id = self.resolve_inode(path)?;
        let inode = self.inodes.get(&id.0).ok_or(FsError::NotFound)?;
        if inode.metadata.file_type != FileType::Directory {
            return Err(FsError::NotADirectory);
        }
        if !inode.children.is_empty() {
            return Err(FsError::IoError); // directory not empty
        }
        let parent_path = path.parent().ok_or(FsError::InvalidPath)?;
        let name = path.file_name().ok_or(FsError::InvalidPath)?;
        let parent_id = self.resolve_inode(&parent_path)?;
        let parent = self.inodes.get_mut(&parent_id.0).ok_or(FsError::NotFound)?;
        parent.children.remove(name);
        self.inodes.remove(&id.0);
        Ok(())
    }

    fn unlink(&mut self, path: &RaePath) -> FsResult<()> {
        let id = self.resolve_inode(path)?;
        let inode = self.inodes.get(&id.0).ok_or(FsError::NotFound)?;
        if inode.metadata.file_type == FileType::Directory {
            return Err(FsError::IsDirectory);
        }
        let parent_path = path.parent().ok_or(FsError::InvalidPath)?;
        let name = path.file_name().ok_or(FsError::InvalidPath)?;
        let parent_id = self.resolve_inode(&parent_path)?;
        let parent = self.inodes.get_mut(&parent_id.0).ok_or(FsError::NotFound)?;
        parent.children.remove(name);
        self.inodes.remove(&id.0);
        Ok(())
    }

    fn rename(&mut self, from: &RaePath, to: &RaePath) -> FsResult<()> {
        let id = self.resolve_inode(from)?;
        let from_parent = from.parent().ok_or(FsError::InvalidPath)?;
        let from_name = from.file_name().ok_or(FsError::InvalidPath)?;
        let to_parent = to.parent().ok_or(FsError::InvalidPath)?;
        let to_name = to.file_name().ok_or(FsError::InvalidPath)?;

        let from_parent_id = self.resolve_inode(&from_parent)?;
        let to_parent_id = self.resolve_inode(&to_parent)?;

        let fp = self
            .inodes
            .get_mut(&from_parent_id.0)
            .ok_or(FsError::NotFound)?;
        fp.children.remove(from_name);

        let tp = self
            .inodes
            .get_mut(&to_parent_id.0)
            .ok_or(FsError::NotFound)?;
        tp.children.insert(String::from(to_name), id);
        Ok(())
    }

    fn readdir(&self, path: &RaePath) -> FsResult<Vec<DirEntry>> {
        let id = self.resolve_inode(path)?;
        let inode = self.inodes.get(&id.0).ok_or(FsError::NotFound)?;
        if inode.metadata.file_type != FileType::Directory {
            return Err(FsError::NotADirectory);
        }
        let entries = inode
            .children
            .iter()
            .map(|(name, &child_id)| {
                let child = self.inodes.get(&child_id.0);
                DirEntry {
                    name: name.clone(),
                    inode: child_id,
                    file_type: child
                        .map(|c| c.metadata.file_type)
                        .unwrap_or(FileType::RegularFile),
                }
            })
            .collect();
        Ok(entries)
    }

    fn symlink(&mut self, target: &RaePath, link: &RaePath) -> FsResult<()> {
        let parent_path = link.parent().ok_or(FsError::InvalidPath)?;
        let name = link.file_name().ok_or(FsError::InvalidPath)?;
        let parent_id = self.resolve_inode(&parent_path)?;
        let new_id = self.alloc_inode();

        let inode = MemInode {
            metadata: FileMetadata {
                inode: new_id,
                file_type: FileType::Symlink,
                size: target.as_str().len() as u64,
                blocks_used: 0,
                permissions: Permissions::new(0o777),
                owner_uid: 0,
                owner_gid: 0,
                created_at: 0,
                modified_at: 0,
                accessed_at: 0,
                link_count: 1,
                storage_tier: StorageTier::NVMe,
                compression: CompressionAlgo::None,
                encryption: EncryptionMode::None,
                cow_generation: 0,
            },
            data: Vec::new(),
            children: BTreeMap::new(),
            symlink_target: Some(target.clone()),
        };
        self.inodes.insert(new_id.0, inode);

        let parent = self.inodes.get_mut(&parent_id.0).ok_or(FsError::NotFound)?;
        parent.children.insert(String::from(name), new_id);
        Ok(())
    }

    fn readlink(&self, path: &RaePath) -> FsResult<RaePath> {
        let id = self.resolve_inode(path)?;
        let inode = self.inodes.get(&id.0).ok_or(FsError::NotFound)?;
        inode.symlink_target.clone().ok_or(FsError::NotAFile)
    }

    fn sync(&mut self, _handle: FileHandle) -> FsResult<()> {
        Ok(())
    }

    fn create_snapshot(&mut self, snap_type: SnapshotType, label: &str) -> FsResult<SnapshotId> {
        self.snapshots.create(snap_type, label, InodeId(1), None, 0)
    }

    fn restore_snapshot(&mut self, id: SnapshotId) -> FsResult<()> {
        let _root = self.snapshots.rollback_target(id)?;
        Ok(())
    }

    fn set_extent_hint(&mut self, _handle: FileHandle, _hint: &ExtentHint) -> FsResult<()> {
        Ok(())
    }
}

// ── R10 Artifacts ────────────────────────────────────────────────────────

/// Initialize the RaeFS userspace library.
pub fn init() {
    // No global state for now, but following the contract.
}

/// Prove behavioral correctness of the path and metadata logic.
pub fn run_boot_smoketest() -> bool {
    // 1. Test Path
    let root = RaePath::root();
    let home = root.join("home");
    let ok1 = home.as_str() == "/home";

    // 2. Test Metadata
    let meta = FileMetadata {
        inode: InodeId(1),
        file_type: FileType::Directory,
        size: 0,
        blocks_used: 0,
        owner_uid: 0,
        owner_gid: 0,
        permissions: Permissions { bits: 0o755 },
        created_at: 0,
        modified_at: 0,
        accessed_at: 0,
        link_count: 1,
        storage_tier: StorageTier::NVMe,
        encryption: EncryptionMode::None,
        compression: CompressionAlgo::None,
        cow_generation: 0,
    };
    let ok2 = meta.is_dir();

    // 3. Test fsck core logic
    let mut data = [0u8; 4096];
    data[0..8].copy_from_slice(&0x526165465321u64.to_le_bytes()); // magic
    let fsck_res = fsck::run_fsck_on_disk_image(&data);
    let ok3 = fsck_res.is_ok();

    ok1 && ok2 && ok3
}
