//! RaeCloud — cloud storage and sync for AthenaOS.
//!
//! Multi-provider cloud storage with bidirectional sync, client-side encryption,
//! chunked transfers, and virtual filesystem mount points.
//!
//! See `docs/components/athcloud.md` for the design.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// 1. Core types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudError {
    NotFound,
    AlreadyExists,
    PermissionDenied,
    QuotaExceeded,
    NetworkError,
    Timeout,
    AuthenticationFailed,
    InvalidPath,
    ProviderUnavailable,
    EncryptionError,
    ChecksumMismatch,
    TransferAborted,
    ConflictDetected,
    RateLimited,
    InvalidConfig,
    UnsupportedOperation,
    CorruptedData,
    LockFailed,
    InternalError,
}

pub type Result<T> = core::result::Result<T, CloudError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransferId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShareId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MountId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    RegularFile,
    Directory,
    Symlink,
}

#[derive(Debug, Clone)]
pub struct FileStat {
    pub file_id: FileId,
    pub name: String,
    pub kind: FileKind,
    pub size: u64,
    pub created: u64,
    pub modified: u64,
    pub accessed: u64,
    pub content_hash: [u8; 32],
    pub is_encrypted: bool,
    pub version: u64,
    pub etag: String,
}

// ---------------------------------------------------------------------------
// 2. Storage providers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderType {
    Local,
    S3Compatible,
    WebDav,
    Ftp,
    Sftp,
    OneDrive,
    GoogleDrive,
    Dropbox,
}

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub id: ProviderId,
    pub provider_type: ProviderType,
    pub display_name: String,
    pub endpoint_url: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    pub bucket: String,
    pub root_path: String,
    pub use_ssl: bool,
    pub use_path_style: bool,
    pub max_connections: u32,
    pub timeout_ms: u64,
    pub enabled: bool,
}

impl ProviderConfig {
    pub fn new_s3(id: ProviderId, endpoint: String, bucket: String) -> Self {
        Self {
            id,
            provider_type: ProviderType::S3Compatible,
            display_name: String::new(),
            endpoint_url: endpoint,
            region: String::new(),
            access_key: String::new(),
            secret_key: String::new(),
            bucket,
            root_path: String::new(),
            use_ssl: true,
            use_path_style: false,
            max_connections: 8,
            timeout_ms: 30_000,
            enabled: true,
        }
    }

    pub fn new_webdav(id: ProviderId, endpoint: String) -> Self {
        Self {
            id,
            provider_type: ProviderType::WebDav,
            display_name: String::new(),
            endpoint_url: endpoint,
            region: String::new(),
            access_key: String::new(),
            secret_key: String::new(),
            bucket: String::new(),
            root_path: String::new(),
            use_ssl: true,
            use_path_style: false,
            max_connections: 4,
            timeout_ms: 30_000,
            enabled: true,
        }
    }

    pub fn new_local(id: ProviderId, root: String) -> Self {
        Self {
            id,
            provider_type: ProviderType::Local,
            display_name: String::new(),
            endpoint_url: String::new(),
            region: String::new(),
            access_key: String::new(),
            secret_key: String::new(),
            bucket: String::new(),
            root_path: root,
            use_ssl: false,
            use_path_style: false,
            max_connections: 16,
            timeout_ms: 5_000,
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderOp {
    List,
    Get,
    Put,
    Delete,
    Copy,
    Move,
    Mkdir,
    Stat,
    Exists,
}

#[derive(Debug, Clone)]
pub struct ProviderCapabilities {
    pub supported_ops: [bool; 9],
    pub supports_versioning: bool,
    pub supports_presigned_urls: bool,
    pub supports_multipart: bool,
    pub supports_locking: bool,
    pub max_file_size: u64,
    pub max_path_length: usize,
}

impl ProviderCapabilities {
    pub fn full() -> Self {
        Self {
            supported_ops: [true; 9],
            supports_versioning: true,
            supports_presigned_urls: true,
            supports_multipart: true,
            supports_locking: true,
            max_file_size: u64::MAX,
            max_path_length: 4096,
        }
    }

    pub fn supports(&self, op: ProviderOp) -> bool {
        self.supported_ops[op as usize]
    }
}

#[derive(Debug, Clone)]
pub struct ProviderState {
    pub config: ProviderConfig,
    pub capabilities: ProviderCapabilities,
    pub connected: bool,
    pub last_error: Option<CloudError>,
    pub bytes_used: u64,
    pub bytes_quota: u64,
    pub file_count: u64,
}

impl ProviderState {
    pub fn new(config: ProviderConfig) -> Self {
        let capabilities = match config.provider_type {
            ProviderType::S3Compatible => ProviderCapabilities::full(),
            ProviderType::WebDav => ProviderCapabilities {
                supported_ops: [true; 9],
                supports_versioning: false,
                supports_presigned_urls: false,
                supports_multipart: false,
                supports_locking: true,
                max_file_size: u64::MAX,
                max_path_length: 4096,
            },
            ProviderType::Ftp | ProviderType::Sftp => ProviderCapabilities {
                supported_ops: [true, true, true, true, false, true, true, true, true],
                supports_versioning: false,
                supports_presigned_urls: false,
                supports_multipart: false,
                supports_locking: false,
                max_file_size: u64::MAX,
                max_path_length: 4096,
            },
            _ => ProviderCapabilities::full(),
        };
        Self {
            config,
            capabilities,
            connected: false,
            last_error: None,
            bytes_used: 0,
            bytes_quota: 0,
            file_count: 0,
        }
    }

    pub fn quota_remaining(&self) -> u64 {
        self.bytes_quota.saturating_sub(self.bytes_used)
    }

    pub fn usage_percent(&self) -> u8 {
        if self.bytes_quota == 0 {
            return 0;
        }
        ((self.bytes_used as u128 * 100) / self.bytes_quota as u128) as u8
    }
}

// ---------------------------------------------------------------------------
// 3. S3 protocol
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct S3Bucket {
    pub name: String,
    pub creation_date: u64,
    pub region: String,
    pub versioning_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct S3Object {
    pub key: String,
    pub size: u64,
    pub last_modified: u64,
    pub etag: String,
    pub storage_class: S3StorageClass,
    pub content_type: String,
    pub version_id: String,
    pub is_delete_marker: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S3StorageClass {
    Standard,
    InfrequentAccess,
    OneZoneIA,
    Glacier,
    GlacierDeepArchive,
    Intelligent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S3Acl {
    Private,
    PublicRead,
    PublicReadWrite,
    AuthenticatedRead,
    BucketOwnerRead,
    BucketOwnerFullControl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S3SseMode {
    None,
    SseS3,
    SseKms,
    SseC,
}

#[derive(Debug, Clone)]
pub struct S3LifecycleRule {
    pub id: String,
    pub prefix: String,
    pub enabled: bool,
    pub expiration_days: u32,
    pub transition_days: u32,
    pub transition_class: S3StorageClass,
    pub abort_incomplete_multipart_days: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MultipartUploadId(pub u64);

#[derive(Debug, Clone)]
pub struct MultipartUpload {
    pub upload_id: MultipartUploadId,
    pub key: String,
    pub bucket: String,
    pub initiated: u64,
    pub parts: Vec<MultipartPart>,
    pub next_part_number: u32,
    pub total_size: u64,
    pub completed: bool,
    pub aborted: bool,
}

#[derive(Debug, Clone)]
pub struct MultipartPart {
    pub part_number: u32,
    pub size: u64,
    pub etag: String,
    pub checksum: [u8; 32],
    pub uploaded: bool,
}

impl MultipartUpload {
    pub fn new(upload_id: MultipartUploadId, key: String, bucket: String, now: u64) -> Self {
        Self {
            upload_id,
            key,
            bucket,
            initiated: now,
            parts: Vec::new(),
            next_part_number: 1,
            total_size: 0,
            completed: false,
            aborted: false,
        }
    }

    pub fn add_part(&mut self, size: u64, etag: String, checksum: [u8; 32]) -> u32 {
        let pn = self.next_part_number;
        self.parts.push(MultipartPart {
            part_number: pn,
            size,
            etag,
            checksum,
            uploaded: true,
        });
        self.next_part_number += 1;
        self.total_size += size;
        pn
    }

    pub fn complete(&mut self) -> Result<u64> {
        if self.aborted {
            return Err(CloudError::TransferAborted);
        }
        if self.parts.is_empty() {
            return Err(CloudError::InvalidConfig);
        }
        self.completed = true;
        Ok(self.total_size)
    }

    pub fn abort(&mut self) {
        self.aborted = true;
        self.parts.clear();
        self.total_size = 0;
    }

    pub fn uploaded_parts(&self) -> usize {
        self.parts.iter().filter(|p| p.uploaded).count()
    }
}

#[derive(Debug, Clone)]
pub struct PresignedUrl {
    pub url: String,
    pub expires_at: u64,
    pub method: String,
    pub bucket: String,
    pub key: String,
}

pub struct S3Client {
    pub provider_id: ProviderId,
    pub buckets: BTreeMap<String, S3Bucket>,
    pub objects: BTreeMap<String, Vec<S3Object>>,
    pub multipart_uploads: BTreeMap<MultipartUploadId, MultipartUpload>,
    pub lifecycle_rules: Vec<S3LifecycleRule>,
    pub next_upload_id: u64,
    pub default_acl: S3Acl,
    pub default_sse: S3SseMode,
}

impl S3Client {
    pub fn new(provider_id: ProviderId) -> Self {
        Self {
            provider_id,
            buckets: BTreeMap::new(),
            objects: BTreeMap::new(),
            multipart_uploads: BTreeMap::new(),
            lifecycle_rules: Vec::new(),
            next_upload_id: 1,
            default_acl: S3Acl::Private,
            default_sse: S3SseMode::None,
        }
    }

    pub fn create_bucket(&mut self, name: String, region: String, now: u64) -> Result<()> {
        if self.buckets.contains_key(&name) {
            return Err(CloudError::AlreadyExists);
        }
        self.buckets.insert(
            name.clone(),
            S3Bucket {
                name,
                creation_date: now,
                region,
                versioning_enabled: false,
            },
        );
        Ok(())
    }

    pub fn delete_bucket(&mut self, name: &str) -> Result<()> {
        if self.objects.get(name).map_or(false, |v| !v.is_empty()) {
            return Err(CloudError::InvalidConfig);
        }
        self.buckets.remove(name).ok_or(CloudError::NotFound)?;
        self.objects.remove(name);
        Ok(())
    }

    pub fn list_buckets(&self) -> Vec<&S3Bucket> {
        self.buckets.values().collect()
    }

    pub fn put_object(
        &mut self,
        bucket: &str,
        key: String,
        size: u64,
        content_type: String,
        now: u64,
    ) -> Result<()> {
        if !self.buckets.contains_key(bucket) {
            return Err(CloudError::NotFound);
        }
        let obj = S3Object {
            key: key.clone(),
            size,
            last_modified: now,
            etag: String::new(),
            storage_class: S3StorageClass::Standard,
            content_type,
            version_id: String::new(),
            is_delete_marker: false,
        };
        self.objects
            .entry(bucket.into())
            .or_insert_with(Vec::new)
            .push(obj);
        Ok(())
    }

    pub fn get_object(&self, bucket: &str, key: &str) -> Result<&S3Object> {
        let objs = self.objects.get(bucket).ok_or(CloudError::NotFound)?;
        objs.iter()
            .rev()
            .find(|o| o.key == key && !o.is_delete_marker)
            .ok_or(CloudError::NotFound)
    }

    pub fn delete_object(&mut self, bucket: &str, key: &str) -> Result<()> {
        let objs = self.objects.get_mut(bucket).ok_or(CloudError::NotFound)?;
        let versioning = self
            .buckets
            .get(bucket)
            .map_or(false, |b| b.versioning_enabled);
        if versioning {
            objs.push(S3Object {
                key: key.into(),
                size: 0,
                last_modified: 0,
                etag: String::new(),
                storage_class: S3StorageClass::Standard,
                content_type: String::new(),
                version_id: String::new(),
                is_delete_marker: true,
            });
        } else {
            objs.retain(|o| o.key != key);
        }
        Ok(())
    }

    pub fn head_object(&self, bucket: &str, key: &str) -> Result<(u64, u64)> {
        let obj = self.get_object(bucket, key)?;
        Ok((obj.size, obj.last_modified))
    }

    pub fn copy_object(
        &mut self,
        src_bucket: &str,
        src_key: &str,
        dst_bucket: &str,
        dst_key: String,
        now: u64,
    ) -> Result<()> {
        let obj = self.get_object(src_bucket, src_key)?.clone();
        let new_obj = S3Object {
            key: dst_key,
            last_modified: now,
            ..obj
        };
        self.objects
            .entry(dst_bucket.into())
            .or_insert_with(Vec::new)
            .push(new_obj);
        Ok(())
    }

    pub fn list_objects(
        &self,
        bucket: &str,
        prefix: &str,
        max_keys: usize,
    ) -> Result<Vec<&S3Object>> {
        let objs = self.objects.get(bucket).ok_or(CloudError::NotFound)?;
        Ok(objs
            .iter()
            .filter(|o| o.key.starts_with(prefix) && !o.is_delete_marker)
            .take(max_keys)
            .collect())
    }

    pub fn initiate_multipart(
        &mut self,
        bucket: &str,
        key: String,
        now: u64,
    ) -> Result<MultipartUploadId> {
        if !self.buckets.contains_key(bucket) {
            return Err(CloudError::NotFound);
        }
        let id = MultipartUploadId(self.next_upload_id);
        self.next_upload_id += 1;
        let upload = MultipartUpload::new(id, key, bucket.into(), now);
        self.multipart_uploads.insert(id, upload);
        Ok(id)
    }

    pub fn upload_part(
        &mut self,
        upload_id: MultipartUploadId,
        size: u64,
        etag: String,
        checksum: [u8; 32],
    ) -> Result<u32> {
        let upload = self
            .multipart_uploads
            .get_mut(&upload_id)
            .ok_or(CloudError::NotFound)?;
        if upload.aborted || upload.completed {
            return Err(CloudError::TransferAborted);
        }
        Ok(upload.add_part(size, etag, checksum))
    }

    pub fn complete_multipart(&mut self, upload_id: MultipartUploadId, now: u64) -> Result<u64> {
        let upload = self
            .multipart_uploads
            .get_mut(&upload_id)
            .ok_or(CloudError::NotFound)?;
        let total = upload.complete()?;
        let key = upload.key.clone();
        let bucket = upload.bucket.clone();
        self.put_object(&bucket, key, total, String::new(), now)?;
        Ok(total)
    }

    pub fn abort_multipart(&mut self, upload_id: MultipartUploadId) -> Result<()> {
        let upload = self
            .multipart_uploads
            .get_mut(&upload_id)
            .ok_or(CloudError::NotFound)?;
        upload.abort();
        Ok(())
    }

    pub fn enable_versioning(&mut self, bucket: &str) -> Result<()> {
        let b = self.buckets.get_mut(bucket).ok_or(CloudError::NotFound)?;
        b.versioning_enabled = true;
        Ok(())
    }

    pub fn add_lifecycle_rule(&mut self, rule: S3LifecycleRule) {
        self.lifecycle_rules.push(rule);
    }

    pub fn generate_presigned_url(
        &self,
        bucket: &str,
        key: &str,
        method: &str,
        expires_at: u64,
    ) -> Result<PresignedUrl> {
        if !self.buckets.contains_key(bucket) {
            return Err(CloudError::NotFound);
        }
        Ok(PresignedUrl {
            url: String::new(),
            expires_at,
            method: method.into(),
            bucket: bucket.into(),
            key: key.into(),
        })
    }
}

// ---------------------------------------------------------------------------
// 4. WebDAV protocol
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebDavMethod {
    Propfind,
    Proppatch,
    Mkcol,
    Copy,
    Move,
    Delete,
    Put,
    Get,
    Lock,
    Unlock,
}

#[derive(Debug, Clone)]
pub struct WebDavProperty {
    pub namespace: String,
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct WebDavResource {
    pub href: String,
    pub display_name: String,
    pub content_type: String,
    pub content_length: u64,
    pub last_modified: u64,
    pub etag: String,
    pub is_collection: bool,
    pub properties: Vec<WebDavProperty>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WebDavLockToken(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockScope {
    Exclusive,
    Shared,
}

#[derive(Debug, Clone)]
pub struct WebDavLock {
    pub token: WebDavLockToken,
    pub href: String,
    pub scope: LockScope,
    pub owner: String,
    pub timeout_seconds: u64,
    pub created: u64,
    pub depth_infinity: bool,
}

pub struct WebDavClient {
    pub provider_id: ProviderId,
    pub resources: BTreeMap<String, WebDavResource>,
    pub locks: BTreeMap<WebDavLockToken, WebDavLock>,
    pub next_lock_token: u64,
}

impl WebDavClient {
    pub fn new(provider_id: ProviderId) -> Self {
        Self {
            provider_id,
            resources: BTreeMap::new(),
            locks: BTreeMap::new(),
            next_lock_token: 1,
        }
    }

    pub fn propfind(&self, href: &str, depth: u32) -> Vec<&WebDavResource> {
        if depth == 0 {
            return self.resources.get(href).into_iter().collect();
        }
        let prefix = if href.ends_with('/') {
            alloc::format!("{}", href)
        } else {
            alloc::format!("{}/", href)
        };
        self.resources
            .values()
            .filter(|r| r.href == href || r.href.starts_with(prefix.as_str()))
            .collect()
    }

    fn to_string(href: &str) -> String {
        let mut s = String::new();
        for b in href.bytes() {
            s.push(b as char);
        }
        s
    }

    pub fn proppatch(
        &mut self,
        href: &str,
        set_props: Vec<WebDavProperty>,
        remove_props: Vec<String>,
    ) -> Result<()> {
        let resource = self.resources.get_mut(href).ok_or(CloudError::NotFound)?;
        for prop in set_props {
            if let Some(existing) = resource
                .properties
                .iter_mut()
                .find(|p| p.name == prop.name && p.namespace == prop.namespace)
            {
                existing.value = prop.value;
            } else {
                resource.properties.push(prop);
            }
        }
        for name in &remove_props {
            resource.properties.retain(|p| &p.name != name);
        }
        Ok(())
    }

    pub fn mkcol(&mut self, href: &str) -> Result<()> {
        if self.resources.contains_key(href) {
            return Err(CloudError::AlreadyExists);
        }
        let name = href.rsplit('/').next().unwrap_or(href);
        self.resources.insert(
            href.into(),
            WebDavResource {
                href: href.into(),
                display_name: Self::to_string(name),
                content_type: String::new(),
                content_length: 0,
                last_modified: 0,
                etag: String::new(),
                is_collection: true,
                properties: Vec::new(),
            },
        );
        Ok(())
    }

    pub fn put(&mut self, href: &str, size: u64, content_type: String, now: u64) -> Result<()> {
        let name = href.rsplit('/').next().unwrap_or(href);
        let resource = WebDavResource {
            href: href.into(),
            display_name: Self::to_string(name),
            content_type,
            content_length: size,
            last_modified: now,
            etag: String::new(),
            is_collection: false,
            properties: Vec::new(),
        };
        self.resources.insert(href.into(), resource);
        Ok(())
    }

    pub fn delete(&mut self, href: &str) -> Result<()> {
        self.check_lock(href)?;
        let prefix = alloc::format!("{}/", href);
        let to_remove: Vec<String> = self
            .resources
            .keys()
            .filter(|k| *k == href || k.starts_with(prefix.as_str()))
            .cloned()
            .collect();
        if to_remove.is_empty() {
            return Err(CloudError::NotFound);
        }
        for key in to_remove {
            self.resources.remove(&key);
        }
        Ok(())
    }

    pub fn copy(&mut self, src: &str, dst: &str, overwrite: bool) -> Result<()> {
        let source = self.resources.get(src).ok_or(CloudError::NotFound)?.clone();
        if !overwrite && self.resources.contains_key(dst) {
            return Err(CloudError::AlreadyExists);
        }
        let mut copied = source;
        copied.href = dst.into();
        self.resources.insert(dst.into(), copied);
        Ok(())
    }

    pub fn move_resource(&mut self, src: &str, dst: &str, overwrite: bool) -> Result<()> {
        self.check_lock(src)?;
        self.copy(src, dst, overwrite)?;
        self.resources.remove(src);
        Ok(())
    }

    pub fn lock(
        &mut self,
        href: &str,
        scope: LockScope,
        owner: String,
        timeout: u64,
        now: u64,
    ) -> Result<WebDavLockToken> {
        if scope == LockScope::Exclusive {
            let already_locked = self.locks.values().any(|l| l.href == href);
            if already_locked {
                return Err(CloudError::LockFailed);
            }
        }
        let token = WebDavLockToken(self.next_lock_token);
        self.next_lock_token += 1;
        self.locks.insert(
            token,
            WebDavLock {
                token,
                href: href.into(),
                scope,
                owner,
                timeout_seconds: timeout,
                created: now,
                depth_infinity: false,
            },
        );
        Ok(token)
    }

    pub fn unlock(&mut self, token: WebDavLockToken) -> Result<()> {
        self.locks.remove(&token).ok_or(CloudError::NotFound)?;
        Ok(())
    }

    pub fn refresh_lock(
        &mut self,
        token: WebDavLockToken,
        new_timeout: u64,
        now: u64,
    ) -> Result<()> {
        let lock = self.locks.get_mut(&token).ok_or(CloudError::NotFound)?;
        lock.timeout_seconds = new_timeout;
        lock.created = now;
        Ok(())
    }

    fn check_lock(&self, href: &str) -> Result<()> {
        let locked = self
            .locks
            .values()
            .any(|l| l.href == href && l.scope == LockScope::Exclusive);
        if locked {
            return Err(CloudError::LockFailed);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 5. Sync engine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    NewestWins,
    OldestWins,
    Manual,
    BothKeep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeDetection {
    HashBased,
    TimestampBased,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDirection {
    Upload,
    Download,
    Bidirectional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncItemState {
    Unchanged,
    LocalModified,
    RemoteModified,
    BothModified,
    LocalAdded,
    RemoteAdded,
    LocalDeleted,
    RemoteDeleted,
    Conflict,
    Syncing,
    Error,
}

#[derive(Debug, Clone)]
pub struct SyncPair {
    pub local_path: String,
    pub remote_path: String,
    pub provider_id: ProviderId,
    pub direction: SyncDirection,
    pub conflict_policy: ConflictPolicy,
    pub detection: ChangeDetection,
    pub enabled: bool,
    pub filter_patterns: Vec<String>,
    pub exclude_patterns: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SyncEntry {
    pub path: String,
    pub state: SyncItemState,
    pub local_hash: [u8; 32],
    pub remote_hash: [u8; 32],
    pub local_modified: u64,
    pub remote_modified: u64,
    pub local_size: u64,
    pub remote_size: u64,
    pub sync_version: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncAction {
    UploadFile,
    DownloadFile,
    DeleteLocal,
    DeleteRemote,
    ResolveConflict,
    Skip,
}

#[derive(Debug, Clone)]
pub struct SyncJournalEntry {
    pub timestamp: u64,
    pub path: String,
    pub action: SyncAction,
    pub old_hash: [u8; 32],
    pub new_hash: [u8; 32],
    pub success: bool,
    pub error: Option<CloudError>,
}

pub struct SyncEngine {
    pub pairs: Vec<SyncPair>,
    pub state: BTreeMap<String, SyncEntry>,
    pub journal: Vec<SyncJournalEntry>,
    pub delta_enabled: bool,
    pub last_full_scan: u64,
    pub total_synced_bytes: u64,
    pub total_synced_files: u64,
}

impl SyncEngine {
    pub fn new() -> Self {
        Self {
            pairs: Vec::new(),
            state: BTreeMap::new(),
            journal: Vec::new(),
            delta_enabled: true,
            last_full_scan: 0,
            total_synced_bytes: 0,
            total_synced_files: 0,
        }
    }

    pub fn add_pair(&mut self, pair: SyncPair) {
        self.pairs.push(pair);
    }

    pub fn remove_pair(&mut self, index: usize) -> bool {
        if index < self.pairs.len() {
            self.pairs.remove(index);
            true
        } else {
            false
        }
    }

    pub fn detect_changes(&self, pair_index: usize) -> Vec<(&str, SyncAction)> {
        if pair_index >= self.pairs.len() {
            return Vec::new();
        }
        let pair = &self.pairs[pair_index];
        let mut actions = Vec::new();

        for (path, entry) in &self.state {
            if !path.starts_with(pair.local_path.as_str()) {
                continue;
            }
            let action = match entry.state {
                SyncItemState::LocalModified => match pair.direction {
                    SyncDirection::Upload | SyncDirection::Bidirectional => SyncAction::UploadFile,
                    SyncDirection::Download => SyncAction::Skip,
                },
                SyncItemState::RemoteModified => match pair.direction {
                    SyncDirection::Download | SyncDirection::Bidirectional => {
                        SyncAction::DownloadFile
                    }
                    SyncDirection::Upload => SyncAction::Skip,
                },
                SyncItemState::LocalAdded => SyncAction::UploadFile,
                SyncItemState::RemoteAdded => SyncAction::DownloadFile,
                SyncItemState::LocalDeleted => SyncAction::DeleteRemote,
                SyncItemState::RemoteDeleted => SyncAction::DeleteLocal,
                SyncItemState::BothModified | SyncItemState::Conflict => {
                    SyncAction::ResolveConflict
                }
                _ => SyncAction::Skip,
            };
            actions.push((path.as_str(), action));
        }
        actions
    }

    pub fn resolve_conflict(&self, entry: &SyncEntry, policy: ConflictPolicy) -> SyncAction {
        match policy {
            ConflictPolicy::NewestWins => {
                if entry.local_modified >= entry.remote_modified {
                    SyncAction::UploadFile
                } else {
                    SyncAction::DownloadFile
                }
            }
            ConflictPolicy::OldestWins => {
                if entry.local_modified <= entry.remote_modified {
                    SyncAction::UploadFile
                } else {
                    SyncAction::DownloadFile
                }
            }
            ConflictPolicy::BothKeep => SyncAction::UploadFile,
            ConflictPolicy::Manual => SyncAction::ResolveConflict,
        }
    }

    pub fn record_action(
        &mut self,
        path: String,
        action: SyncAction,
        old_hash: [u8; 32],
        new_hash: [u8; 32],
        success: bool,
        error: Option<CloudError>,
        now: u64,
    ) {
        if success {
            self.total_synced_files += 1;
        }
        self.journal.push(SyncJournalEntry {
            timestamp: now,
            path,
            action,
            old_hash,
            new_hash,
            success,
            error,
        });
    }

    pub fn update_state(
        &mut self,
        path: String,
        local_hash: [u8; 32],
        remote_hash: [u8; 32],
        local_mod: u64,
        remote_mod: u64,
        local_size: u64,
        remote_size: u64,
    ) {
        let state = if local_hash == remote_hash {
            SyncItemState::Unchanged
        } else if local_mod > remote_mod {
            SyncItemState::LocalModified
        } else if remote_mod > local_mod {
            SyncItemState::RemoteModified
        } else {
            SyncItemState::BothModified
        };

        let entry = self.state.entry(path).or_insert_with(|| SyncEntry {
            path: String::new(),
            state: SyncItemState::Unchanged,
            local_hash: [0; 32],
            remote_hash: [0; 32],
            local_modified: 0,
            remote_modified: 0,
            local_size: 0,
            remote_size: 0,
            sync_version: 0,
        });
        entry.state = state;
        entry.local_hash = local_hash;
        entry.remote_hash = remote_hash;
        entry.local_modified = local_mod;
        entry.remote_modified = remote_mod;
        entry.local_size = local_size;
        entry.remote_size = remote_size;
        entry.sync_version += 1;
    }

    pub fn pending_count(&self) -> usize {
        self.state
            .values()
            .filter(|e| !matches!(e.state, SyncItemState::Unchanged))
            .count()
    }

    pub fn journal_since(&self, since: u64) -> Vec<&SyncJournalEntry> {
        self.journal
            .iter()
            .filter(|j| j.timestamp >= since)
            .collect()
    }

    pub fn clear_journal(&mut self) {
        self.journal.clear();
    }
}

// ---------------------------------------------------------------------------
// 6. File watcher
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchEvent {
    Created,
    Modified,
    Deleted,
    Renamed,
    MetadataChanged,
}

#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: String,
    pub event: WatchEvent,
    pub timestamp: u64,
    pub is_directory: bool,
}

pub struct FileWatcher {
    pub watched_paths: Vec<String>,
    pub pending_changes: Vec<FileChange>,
    pub debounce_ms: u64,
    pub last_event_time: u64,
    pub recursive: bool,
    pub ignore_patterns: Vec<String>,
}

impl FileWatcher {
    pub fn new(debounce_ms: u64) -> Self {
        Self {
            watched_paths: Vec::new(),
            pending_changes: Vec::new(),
            debounce_ms,
            last_event_time: 0,
            recursive: true,
            ignore_patterns: Vec::new(),
        }
    }

    pub fn watch(&mut self, path: String) {
        if !self.watched_paths.contains(&path) {
            self.watched_paths.push(path);
        }
    }

    pub fn unwatch(&mut self, path: &str) -> bool {
        let len = self.watched_paths.len();
        self.watched_paths.retain(|p| p != path);
        self.watched_paths.len() != len
    }

    pub fn push_event(&mut self, change: FileChange) {
        for pattern in &self.ignore_patterns {
            if change.path.contains(pattern.as_str()) {
                return;
            }
        }
        self.last_event_time = change.timestamp;
        if let Some(existing) = self
            .pending_changes
            .iter_mut()
            .find(|c| c.path == change.path)
        {
            existing.event = change.event;
            existing.timestamp = change.timestamp;
        } else {
            self.pending_changes.push(change);
        }
    }

    pub fn drain_debounced(&mut self, now: u64) -> Vec<FileChange> {
        if now.saturating_sub(self.last_event_time) < self.debounce_ms {
            return Vec::new();
        }
        let mut drained = Vec::new();
        core::mem::swap(&mut drained, &mut self.pending_changes);
        drained
    }

    pub fn add_ignore(&mut self, pattern: String) {
        self.ignore_patterns.push(pattern);
    }

    pub fn is_watching(&self, path: &str) -> bool {
        self.watched_paths
            .iter()
            .any(|w| path.starts_with(w.as_str()))
    }
}

// ---------------------------------------------------------------------------
// 7. Transfer manager
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferState {
    Queued,
    Active,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct TransferItem {
    pub id: TransferId,
    pub provider_id: ProviderId,
    pub direction: TransferDirection,
    pub local_path: String,
    pub remote_path: String,
    pub total_bytes: u64,
    pub transferred_bytes: u64,
    pub state: TransferState,
    pub retries: u32,
    pub max_retries: u32,
    pub retry_delay_ms: u64,
    pub priority: u32,
    pub created: u64,
    pub started: u64,
    pub completed: u64,
    pub error: Option<CloudError>,
    pub checksum: [u8; 32],
    pub chunk_size: u64,
    pub current_chunk: u32,
    pub total_chunks: u32,
}

impl TransferItem {
    pub fn progress_percent(&self) -> u8 {
        if self.total_bytes == 0 {
            return 100;
        }
        ((self.transferred_bytes as u128 * 100) / self.total_bytes as u128) as u8
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state, TransferState::Active)
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            TransferState::Completed | TransferState::Failed | TransferState::Cancelled
        )
    }

    pub fn can_retry(&self) -> bool {
        self.retries < self.max_retries && matches!(self.state, TransferState::Failed)
    }

    pub fn elapsed(&self) -> u64 {
        if self.started == 0 {
            return 0;
        }
        if self.completed > 0 {
            self.completed - self.started
        } else {
            0
        }
    }

    pub fn speed_bps(&self, now: u64) -> u64 {
        let elapsed = if self.started == 0 {
            return 0;
        } else {
            now.saturating_sub(self.started)
        };
        if elapsed == 0 {
            return 0;
        }
        (self.transferred_bytes as u128 * 1000 / elapsed as u128) as u64
    }
}

pub struct TransferManager {
    pub queue: Vec<TransferItem>,
    pub next_id: u64,
    pub max_concurrent: usize,
    pub bandwidth_limit_bps: u64,
    pub default_chunk_size: u64,
    pub default_max_retries: u32,
    pub paused_globally: bool,
    pub total_uploaded: u64,
    pub total_downloaded: u64,
}

impl TransferManager {
    pub fn new() -> Self {
        Self {
            queue: Vec::new(),
            next_id: 1,
            max_concurrent: 4,
            bandwidth_limit_bps: 0,
            default_chunk_size: 8 * 1024 * 1024,
            default_max_retries: 3,
            paused_globally: false,
            total_uploaded: 0,
            total_downloaded: 0,
        }
    }

    pub fn enqueue(
        &mut self,
        provider_id: ProviderId,
        direction: TransferDirection,
        local_path: String,
        remote_path: String,
        total_bytes: u64,
        now: u64,
    ) -> TransferId {
        let id = TransferId(self.next_id);
        self.next_id += 1;
        let total_chunks = if self.default_chunk_size > 0 {
            ((total_bytes + self.default_chunk_size - 1) / self.default_chunk_size) as u32
        } else {
            1
        };
        self.queue.push(TransferItem {
            id,
            provider_id,
            direction,
            local_path,
            remote_path,
            total_bytes,
            transferred_bytes: 0,
            state: TransferState::Queued,
            retries: 0,
            max_retries: self.default_max_retries,
            retry_delay_ms: 1000,
            priority: 0,
            created: now,
            started: 0,
            completed: 0,
            error: None,
            checksum: [0; 32],
            chunk_size: self.default_chunk_size,
            current_chunk: 0,
            total_chunks,
        });
        id
    }

    pub fn start_next(&mut self, now: u64) -> Option<TransferId> {
        if self.paused_globally {
            return None;
        }
        let active = self.queue.iter().filter(|t| t.is_active()).count();
        if active >= self.max_concurrent {
            return None;
        }
        if let Some(item) = self
            .queue
            .iter_mut()
            .filter(|t| t.state == TransferState::Queued)
            .min_by_key(|t| (core::cmp::Reverse(t.priority), t.created))
        {
            item.state = TransferState::Active;
            item.started = now;
            Some(item.id)
        } else {
            None
        }
    }

    pub fn report_progress(&mut self, id: TransferId, bytes: u64, chunk: u32) {
        if let Some(item) = self.queue.iter_mut().find(|t| t.id == id) {
            item.transferred_bytes = bytes;
            item.current_chunk = chunk;
        }
    }

    pub fn complete_transfer(&mut self, id: TransferId, now: u64) {
        if let Some(item) = self.queue.iter_mut().find(|t| t.id == id) {
            item.state = TransferState::Completed;
            item.completed = now;
            item.transferred_bytes = item.total_bytes;
            match item.direction {
                TransferDirection::Upload => self.total_uploaded += item.total_bytes,
                TransferDirection::Download => self.total_downloaded += item.total_bytes,
            }
        }
    }

    pub fn fail_transfer(&mut self, id: TransferId, error: CloudError) {
        if let Some(item) = self.queue.iter_mut().find(|t| t.id == id) {
            item.state = TransferState::Failed;
            item.error = Some(error);
        }
    }

    pub fn retry_transfer(&mut self, id: TransferId, now: u64) -> bool {
        if let Some(item) = self.queue.iter_mut().find(|t| t.id == id) {
            if item.can_retry() {
                item.retries += 1;
                item.retry_delay_ms *= 2;
                item.state = TransferState::Queued;
                item.error = None;
                item.started = now;
                return true;
            }
        }
        false
    }

    pub fn pause_transfer(&mut self, id: TransferId) -> bool {
        if let Some(item) = self.queue.iter_mut().find(|t| t.id == id && t.is_active()) {
            item.state = TransferState::Paused;
            return true;
        }
        false
    }

    pub fn resume_transfer(&mut self, id: TransferId) -> bool {
        if let Some(item) = self
            .queue
            .iter_mut()
            .find(|t| t.id == id && t.state == TransferState::Paused)
        {
            item.state = TransferState::Active;
            return true;
        }
        false
    }

    pub fn cancel_transfer(&mut self, id: TransferId) -> bool {
        if let Some(item) = self
            .queue
            .iter_mut()
            .find(|t| t.id == id && !t.is_terminal())
        {
            item.state = TransferState::Cancelled;
            return true;
        }
        false
    }

    pub fn pause_all(&mut self) {
        self.paused_globally = true;
        for item in &mut self.queue {
            if item.is_active() {
                item.state = TransferState::Paused;
            }
        }
    }

    pub fn resume_all(&mut self) {
        self.paused_globally = false;
    }

    pub fn set_bandwidth_limit(&mut self, bps: u64) {
        self.bandwidth_limit_bps = bps;
    }

    pub fn active_transfers(&self) -> Vec<&TransferItem> {
        self.queue.iter().filter(|t| t.is_active()).collect()
    }

    pub fn pending_transfers(&self) -> Vec<&TransferItem> {
        self.queue
            .iter()
            .filter(|t| t.state == TransferState::Queued)
            .collect()
    }

    pub fn completed_transfers(&self) -> Vec<&TransferItem> {
        self.queue
            .iter()
            .filter(|t| t.state == TransferState::Completed)
            .collect()
    }

    pub fn cleanup_completed(&mut self) {
        self.queue.retain(|t| !t.is_terminal());
    }
}

// ---------------------------------------------------------------------------
// 8. Encryption
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionAlgorithm {
    Aes256Gcm,
    ChaCha20Poly1305,
    XChaCha20Poly1305,
}

#[derive(Debug, Clone)]
pub struct EncryptionKey {
    pub id: u64,
    pub algorithm: EncryptionAlgorithm,
    pub key_data: [u8; 32],
    pub created: u64,
    pub active: bool,
    pub description: String,
}

pub struct KeyManager {
    pub keys: BTreeMap<u64, EncryptionKey>,
    pub active_key_id: Option<u64>,
    pub master_key_hash: [u8; 32],
    pub next_key_id: u64,
}

impl KeyManager {
    pub fn new() -> Self {
        Self {
            keys: BTreeMap::new(),
            active_key_id: None,
            master_key_hash: [0; 32],
            next_key_id: 1,
        }
    }

    pub fn add_key(
        &mut self,
        algorithm: EncryptionAlgorithm,
        key_data: [u8; 32],
        description: String,
        now: u64,
    ) -> u64 {
        let id = self.next_key_id;
        self.next_key_id += 1;
        let key = EncryptionKey {
            id,
            algorithm,
            key_data,
            created: now,
            active: true,
            description,
        };
        self.keys.insert(id, key);
        if self.active_key_id.is_none() {
            self.active_key_id = Some(id);
        }
        id
    }

    pub fn rotate_key(
        &mut self,
        algorithm: EncryptionAlgorithm,
        key_data: [u8; 32],
        now: u64,
    ) -> u64 {
        if let Some(old_id) = self.active_key_id {
            if let Some(old_key) = self.keys.get_mut(&old_id) {
                old_key.active = false;
            }
        }
        let new_id = self.add_key(algorithm, key_data, String::new(), now);
        self.active_key_id = Some(new_id);
        new_id
    }

    pub fn active_key(&self) -> Option<&EncryptionKey> {
        self.active_key_id.and_then(|id| self.keys.get(&id))
    }

    pub fn get_key(&self, id: u64) -> Option<&EncryptionKey> {
        self.keys.get(&id)
    }

    pub fn revoke_key(&mut self, id: u64) -> bool {
        if let Some(key) = self.keys.get_mut(&id) {
            key.active = false;
            if self.active_key_id == Some(id) {
                self.active_key_id = None;
            }
            true
        } else {
            false
        }
    }

    /// Encrypt a blob with the active key using ChaCha20-Poly1305 AEAD. The blob
    /// layout is `nonce(12) || ciphertext || tag(16)`. The caller MUST supply a
    /// `nonce` that is unique for this key (a 96-bit value from a real entropy
    /// source); nonce reuse under one key is catastrophic for AEAD confidentiality
    /// and forgery resistance. Only ChaCha20-Poly1305 is implemented — an
    /// AES-256-GCM / XChaCha key returns `EncryptionError` rather than emitting
    /// fake (unencrypted) output.
    pub fn encrypt_blob(&self, plaintext: &[u8], nonce: [u8; 12]) -> Result<Vec<u8>> {
        let key = self.active_key().ok_or(CloudError::EncryptionError)?;
        if key.algorithm != EncryptionAlgorithm::ChaCha20Poly1305 {
            return Err(CloudError::EncryptionError);
        }
        let sealed = ath_crypto::chacha20poly1305::seal(&key.key_data, &nonce, &[], plaintext);
        let mut out = Vec::with_capacity(12 + sealed.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// Decrypt a `nonce(12) || ciphertext || tag(16)` blob produced by
    /// [`encrypt_blob`] under `key_id`. The Poly1305 tag is verified: a tampered
    /// ciphertext, tag, or nonce, or the wrong key, fails closed with
    /// `CorruptedData` (never returns unauthenticated plaintext).
    pub fn decrypt_blob(&self, key_id: u64, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let key = self.get_key(key_id).ok_or(CloudError::EncryptionError)?;
        if key.algorithm != EncryptionAlgorithm::ChaCha20Poly1305 {
            return Err(CloudError::EncryptionError);
        }
        if ciphertext.len() < 28 {
            return Err(CloudError::CorruptedData);
        }
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&ciphertext[..12]);
        ath_crypto::chacha20poly1305::open(&key.key_data, &nonce, &[], &ciphertext[12..])
            .ok_or(CloudError::CorruptedData)
    }
}

// ---------------------------------------------------------------------------
// 9. Cache
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheEntryState {
    Fresh,
    Stale,
    Downloading,
    Evicted,
}

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub path: String,
    pub provider_id: ProviderId,
    pub remote_hash: [u8; 32],
    pub local_cache_path: String,
    pub size: u64,
    pub state: CacheEntryState,
    pub last_accessed: u64,
    pub last_validated: u64,
    pub access_count: u64,
    pub pinned: bool,
}

pub struct MetadataCache {
    pub entries: BTreeMap<String, FileStat>,
    pub max_entries: usize,
    pub ttl_ms: u64,
    pub last_pruned: u64,
}

impl MetadataCache {
    pub fn new(max_entries: usize, ttl_ms: u64) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_entries,
            ttl_ms,
            last_pruned: 0,
        }
    }

    pub fn get(&self, path: &str, now: u64) -> Option<&FileStat> {
        let stat = self.entries.get(path)?;
        if now.saturating_sub(stat.modified) > self.ttl_ms {
            return None;
        }
        Some(stat)
    }

    pub fn put(&mut self, path: String, stat: FileStat) {
        if self.entries.len() >= self.max_entries {
            self.evict_oldest();
        }
        self.entries.insert(path, stat);
    }

    pub fn invalidate(&mut self, path: &str) {
        self.entries.remove(path);
    }

    pub fn invalidate_prefix(&mut self, prefix: &str) {
        let keys: Vec<String> = self
            .entries
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        for k in keys {
            self.entries.remove(&k);
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    fn evict_oldest(&mut self) {
        if let Some(oldest_key) = self
            .entries
            .iter()
            .min_by_key(|(_, v)| v.accessed)
            .map(|(k, _)| k.clone())
        {
            self.entries.remove(&oldest_key);
        }
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}

pub struct OfflineCache {
    pub entries: BTreeMap<String, CacheEntry>,
    pub max_size_bytes: u64,
    pub current_size_bytes: u64,
    pub eviction_target_percent: u8,
}

impl OfflineCache {
    pub fn new(max_size: u64) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_size_bytes: max_size,
            current_size_bytes: 0,
            eviction_target_percent: 80,
        }
    }

    pub fn add(&mut self, entry: CacheEntry) -> bool {
        if self.current_size_bytes + entry.size > self.max_size_bytes {
            self.evict_lru(entry.size);
        }
        if self.current_size_bytes + entry.size > self.max_size_bytes {
            return false;
        }
        self.current_size_bytes += entry.size;
        self.entries.insert(entry.path.clone(), entry);
        true
    }

    pub fn get(&self, path: &str) -> Option<&CacheEntry> {
        self.entries.get(path)
    }

    pub fn touch(&mut self, path: &str, now: u64) {
        if let Some(entry) = self.entries.get_mut(path) {
            entry.last_accessed = now;
            entry.access_count += 1;
        }
    }

    pub fn remove(&mut self, path: &str) -> bool {
        if let Some(entry) = self.entries.remove(path) {
            self.current_size_bytes = self.current_size_bytes.saturating_sub(entry.size);
            true
        } else {
            false
        }
    }

    pub fn pin(&mut self, path: &str) -> bool {
        if let Some(entry) = self.entries.get_mut(path) {
            entry.pinned = true;
            true
        } else {
            false
        }
    }

    pub fn unpin(&mut self, path: &str) -> bool {
        if let Some(entry) = self.entries.get_mut(path) {
            entry.pinned = false;
            true
        } else {
            false
        }
    }

    fn evict_lru(&mut self, needed: u64) {
        let target =
            (self.max_size_bytes as u128 * self.eviction_target_percent as u128 / 100) as u64;
        while self.current_size_bytes + needed > target {
            let victim = self
                .entries
                .iter()
                .filter(|(_, e)| !e.pinned)
                .min_by_key(|(_, e)| (e.last_accessed, e.access_count))
                .map(|(k, _)| k.clone());
            match victim {
                Some(key) => {
                    self.remove(&key);
                }
                None => break,
            }
        }
    }

    pub fn usage_percent(&self) -> u8 {
        if self.max_size_bytes == 0 {
            return 0;
        }
        ((self.current_size_bytes as u128 * 100) / self.max_size_bytes as u128) as u8
    }
}

// ---------------------------------------------------------------------------
// 10. Mount points
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountPolicy {
    LazyLoad,
    EagerSync,
    OnDemand,
    CacheFirst,
}

#[derive(Debug, Clone)]
pub struct MountPoint {
    pub id: MountId,
    pub provider_id: ProviderId,
    pub local_mount: String,
    pub remote_root: String,
    pub policy: MountPolicy,
    pub read_only: bool,
    pub active: bool,
    pub offline_available: bool,
    pub max_cache_size: u64,
}

pub struct MountManager {
    pub mounts: BTreeMap<MountId, MountPoint>,
    pub next_id: u32,
}

impl MountManager {
    pub fn new() -> Self {
        Self {
            mounts: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn mount(
        &mut self,
        provider_id: ProviderId,
        local_mount: String,
        remote_root: String,
        policy: MountPolicy,
    ) -> MountId {
        let id = MountId(self.next_id);
        self.next_id += 1;
        self.mounts.insert(
            id,
            MountPoint {
                id,
                provider_id,
                local_mount,
                remote_root,
                policy,
                read_only: false,
                active: true,
                offline_available: false,
                max_cache_size: 1024 * 1024 * 1024,
            },
        );
        id
    }

    pub fn unmount(&mut self, id: MountId) -> bool {
        if let Some(mp) = self.mounts.get_mut(&id) {
            mp.active = false;
            true
        } else {
            false
        }
    }

    pub fn remove(&mut self, id: MountId) -> bool {
        self.mounts.remove(&id).is_some()
    }

    pub fn find_mount(&self, local_path: &str) -> Option<&MountPoint> {
        self.mounts
            .values()
            .find(|m| m.active && local_path.starts_with(m.local_mount.as_str()))
    }

    pub fn list_active(&self) -> Vec<&MountPoint> {
        self.mounts.values().filter(|m| m.active).collect()
    }

    pub fn set_read_only(&mut self, id: MountId, read_only: bool) -> bool {
        if let Some(mp) = self.mounts.get_mut(&id) {
            mp.read_only = read_only;
            true
        } else {
            false
        }
    }

    pub fn set_offline_available(&mut self, id: MountId, offline: bool) -> bool {
        if let Some(mp) = self.mounts.get_mut(&id) {
            mp.offline_available = offline;
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// 11. Share links
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharePermission {
    View,
    Edit,
    Upload,
}

#[derive(Debug, Clone)]
pub struct ShareLink {
    pub id: ShareId,
    pub provider_id: ProviderId,
    pub remote_path: String,
    pub permission: SharePermission,
    pub token: String,
    pub password_hash: Option<[u8; 32]>,
    pub expires_at: Option<u64>,
    pub created: u64,
    pub access_count: u64,
    pub max_accesses: Option<u64>,
    pub active: bool,
}

pub struct ShareManager {
    pub shares: BTreeMap<ShareId, ShareLink>,
    pub next_id: u64,
}

impl ShareManager {
    pub fn new() -> Self {
        Self {
            shares: BTreeMap::new(),
            next_id: 1,
        }
    }

    pub fn create_share(
        &mut self,
        provider_id: ProviderId,
        remote_path: String,
        permission: SharePermission,
        token: String,
        now: u64,
    ) -> ShareId {
        let id = ShareId(self.next_id);
        self.next_id += 1;
        self.shares.insert(
            id,
            ShareLink {
                id,
                provider_id,
                remote_path,
                permission,
                token,
                password_hash: None,
                expires_at: None,
                created: now,
                access_count: 0,
                max_accesses: None,
                active: true,
            },
        );
        id
    }

    pub fn set_password(&mut self, id: ShareId, hash: [u8; 32]) -> bool {
        if let Some(share) = self.shares.get_mut(&id) {
            share.password_hash = Some(hash);
            true
        } else {
            false
        }
    }

    pub fn set_expiration(&mut self, id: ShareId, expires_at: u64) -> bool {
        if let Some(share) = self.shares.get_mut(&id) {
            share.expires_at = Some(expires_at);
            true
        } else {
            false
        }
    }

    pub fn set_max_accesses(&mut self, id: ShareId, max: u64) -> bool {
        if let Some(share) = self.shares.get_mut(&id) {
            share.max_accesses = Some(max);
            true
        } else {
            false
        }
    }

    pub fn access_share(&mut self, id: ShareId, now: u64) -> Result<&ShareLink> {
        let share = self.shares.get_mut(&id).ok_or(CloudError::NotFound)?;
        if !share.active {
            return Err(CloudError::PermissionDenied);
        }
        if let Some(exp) = share.expires_at {
            if now > exp {
                share.active = false;
                return Err(CloudError::PermissionDenied);
            }
        }
        if let Some(max) = share.max_accesses {
            if share.access_count >= max {
                share.active = false;
                return Err(CloudError::PermissionDenied);
            }
        }
        share.access_count += 1;
        Ok(share)
    }

    pub fn revoke_share(&mut self, id: ShareId) -> bool {
        if let Some(share) = self.shares.get_mut(&id) {
            share.active = false;
            true
        } else {
            false
        }
    }

    pub fn list_active(&self) -> Vec<&ShareLink> {
        self.shares.values().filter(|s| s.active).collect()
    }

    pub fn find_by_token(&self, token: &str) -> Option<&ShareLink> {
        self.shares.values().find(|s| s.token == token && s.active)
    }

    pub fn cleanup_expired(&mut self, now: u64) {
        for share in self.shares.values_mut() {
            if let Some(exp) = share.expires_at {
                if now > exp {
                    share.active = false;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 12. Quota management
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct QuotaInfo {
    pub provider_id: ProviderId,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub file_count: u64,
    pub file_count_limit: Option<u64>,
    pub last_updated: u64,
}

impl QuotaInfo {
    pub fn available(&self) -> u64 {
        self.total_bytes.saturating_sub(self.used_bytes)
    }

    pub fn usage_percent(&self) -> u8 {
        if self.total_bytes == 0 {
            return 0;
        }
        ((self.used_bytes as u128 * 100) / self.total_bytes as u128) as u8
    }

    pub fn can_store(&self, size: u64) -> bool {
        self.available() >= size
    }

    pub fn file_count_remaining(&self) -> Option<u64> {
        self.file_count_limit
            .map(|limit| limit.saturating_sub(self.file_count))
    }
}

pub struct QuotaManager {
    pub quotas: BTreeMap<ProviderId, QuotaInfo>,
    pub warning_threshold_percent: u8,
}

impl QuotaManager {
    pub fn new() -> Self {
        Self {
            quotas: BTreeMap::new(),
            warning_threshold_percent: 90,
        }
    }

    pub fn set_quota(
        &mut self,
        provider_id: ProviderId,
        total: u64,
        used: u64,
        files: u64,
        now: u64,
    ) {
        self.quotas.insert(
            provider_id,
            QuotaInfo {
                provider_id,
                total_bytes: total,
                used_bytes: used,
                file_count: files,
                file_count_limit: None,
                last_updated: now,
            },
        );
    }

    pub fn record_usage(&mut self, provider_id: ProviderId, bytes: u64) {
        if let Some(q) = self.quotas.get_mut(&provider_id) {
            q.used_bytes += bytes;
            q.file_count += 1;
        }
    }

    pub fn release_usage(&mut self, provider_id: ProviderId, bytes: u64) {
        if let Some(q) = self.quotas.get_mut(&provider_id) {
            q.used_bytes = q.used_bytes.saturating_sub(bytes);
            q.file_count = q.file_count.saturating_sub(1);
        }
    }

    pub fn check_quota(&self, provider_id: ProviderId, needed: u64) -> Result<()> {
        let q = self
            .quotas
            .get(&provider_id)
            .ok_or(CloudError::InvalidConfig)?;
        if !q.can_store(needed) {
            return Err(CloudError::QuotaExceeded);
        }
        Ok(())
    }

    pub fn providers_near_limit(&self) -> Vec<ProviderId> {
        self.quotas
            .values()
            .filter(|q| q.usage_percent() >= self.warning_threshold_percent)
            .map(|q| q.provider_id)
            .collect()
    }

    pub fn get_quota(&self, provider_id: ProviderId) -> Option<&QuotaInfo> {
        self.quotas.get(&provider_id)
    }

    pub fn total_across_providers(&self) -> (u64, u64) {
        let total: u64 = self.quotas.values().map(|q| q.total_bytes).sum();
        let used: u64 = self.quotas.values().map(|q| q.used_bytes).sum();
        (total, used)
    }
}

// ---------------------------------------------------------------------------
// 13. Cloud manager (top-level)
// ---------------------------------------------------------------------------

pub struct CloudManager {
    pub providers: BTreeMap<ProviderId, ProviderState>,
    pub sync_engine: SyncEngine,
    pub transfer_manager: TransferManager,
    pub key_manager: KeyManager,
    pub metadata_cache: MetadataCache,
    pub offline_cache: OfflineCache,
    pub mount_manager: MountManager,
    pub share_manager: ShareManager,
    pub quota_manager: QuotaManager,
    pub file_watcher: FileWatcher,
    pub next_provider_id: u32,
    pub initialized: bool,
}

impl CloudManager {
    pub fn new() -> Self {
        Self {
            providers: BTreeMap::new(),
            sync_engine: SyncEngine::new(),
            transfer_manager: TransferManager::new(),
            key_manager: KeyManager::new(),
            metadata_cache: MetadataCache::new(10_000, 300_000),
            offline_cache: OfflineCache::new(10 * 1024 * 1024 * 1024),
            mount_manager: MountManager::new(),
            share_manager: ShareManager::new(),
            quota_manager: QuotaManager::new(),
            file_watcher: FileWatcher::new(500),
            next_provider_id: 1,
            initialized: false,
        }
    }

    pub fn add_provider(&mut self, config: ProviderConfig) -> ProviderId {
        let id = config.id;
        self.providers.insert(id, ProviderState::new(config));
        id
    }

    pub fn remove_provider(&mut self, id: ProviderId) -> bool {
        self.providers.remove(&id).is_some()
    }

    pub fn get_provider(&self, id: ProviderId) -> Option<&ProviderState> {
        self.providers.get(&id)
    }

    pub fn connect_provider(&mut self, id: ProviderId) -> Result<()> {
        let provider = self.providers.get_mut(&id).ok_or(CloudError::NotFound)?;
        if !provider.config.enabled {
            return Err(CloudError::ProviderUnavailable);
        }
        provider.connected = true;
        provider.last_error = None;
        Ok(())
    }

    pub fn disconnect_provider(&mut self, id: ProviderId) -> Result<()> {
        let provider = self.providers.get_mut(&id).ok_or(CloudError::NotFound)?;
        provider.connected = false;
        Ok(())
    }

    pub fn list_providers(&self) -> Vec<&ProviderState> {
        self.providers.values().collect()
    }

    pub fn connected_providers(&self) -> Vec<ProviderId> {
        self.providers
            .iter()
            .filter(|(_, p)| p.connected)
            .map(|(id, _)| *id)
            .collect()
    }

    pub fn upload_file(
        &mut self,
        provider_id: ProviderId,
        local_path: String,
        remote_path: String,
        size: u64,
        now: u64,
    ) -> Result<TransferId> {
        self.quota_manager.check_quota(provider_id, size)?;
        let id = self.transfer_manager.enqueue(
            provider_id,
            TransferDirection::Upload,
            local_path,
            remote_path,
            size,
            now,
        );
        Ok(id)
    }

    pub fn download_file(
        &mut self,
        provider_id: ProviderId,
        local_path: String,
        remote_path: String,
        size: u64,
        now: u64,
    ) -> TransferId {
        self.transfer_manager.enqueue(
            provider_id,
            TransferDirection::Download,
            local_path,
            remote_path,
            size,
            now,
        )
    }

    pub fn create_share(
        &mut self,
        provider_id: ProviderId,
        path: String,
        permission: SharePermission,
        token: String,
        now: u64,
    ) -> ShareId {
        self.share_manager
            .create_share(provider_id, path, permission, token, now)
    }

    pub fn mount_provider(
        &mut self,
        provider_id: ProviderId,
        local_path: String,
        remote_root: String,
        policy: MountPolicy,
    ) -> Result<MountId> {
        if !self.providers.contains_key(&provider_id) {
            return Err(CloudError::NotFound);
        }
        Ok(self
            .mount_manager
            .mount(provider_id, local_path, remote_root, policy))
    }

    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    pub fn total_storage(&self) -> (u64, u64) {
        self.quota_manager.total_across_providers()
    }
}

pub static CLOUD_MANAGER: spin::Mutex<Option<CloudManager>> = spin::Mutex::new(None);

pub fn init() {
    let mut mgr = CloudManager::new();
    mgr.initialized = true;
    *CLOUD_MANAGER.lock() = Some(mgr);
}

#[cfg(test)]
mod crypto_tests {
    use super::*;
    use alloc::vec::Vec;

    fn mgr_with_chacha_key() -> (KeyManager, u64) {
        let mut m = KeyManager::new();
        let id = m.add_key(
            EncryptionAlgorithm::ChaCha20Poly1305,
            [0x42u8; 32],
            String::new(),
            0,
        );
        (m, id)
    }

    /// encrypt_blob produces real AEAD ciphertext (NOT the old plaintext-in-the-
    /// clear stub) and decrypt_blob round-trips it.
    #[test]
    fn encrypt_blob_is_real_aead_round_trip() {
        let (m, id) = mgr_with_chacha_key();
        let plaintext = b"my private cloud document contents";
        let nonce = [1u8; 12];
        let blob = m.encrypt_blob(plaintext, nonce).unwrap();
        assert_eq!(blob.len(), plaintext.len() + 28);
        assert_eq!(&blob[..12], &nonce);
        let ct: &[u8] = &blob[12..blob.len() - 16];
        // The old stub embedded the plaintext here verbatim; real ciphertext must differ.
        assert_ne!(ct, &plaintext[..], "ciphertext must not equal plaintext");
        let back = m.decrypt_blob(id, &blob).unwrap();
        assert_eq!(back, plaintext);
    }

    /// A tampered ciphertext byte, tag byte, or nonce all fail closed.
    #[test]
    fn tampered_blob_fails_closed() {
        let (m, id) = mgr_with_chacha_key();
        let blob = m.encrypt_blob(b"secret", [2u8; 12]).unwrap();

        let mut ct = blob.clone();
        ct[13] ^= 0xFF;
        assert!(
            m.decrypt_blob(id, &ct).is_err(),
            "tampered ciphertext must fail"
        );

        let mut tag = blob.clone();
        let last = tag.len() - 1;
        tag[last] ^= 0xFF;
        assert!(m.decrypt_blob(id, &tag).is_err(), "tampered tag must fail");

        let mut non = blob.clone();
        non[0] ^= 0xFF;
        assert!(
            m.decrypt_blob(id, &non).is_err(),
            "tampered nonce must fail"
        );
    }

    /// Different nonces over the same plaintext give different ciphertext (the
    /// nonce is actually fed into the cipher).
    #[test]
    fn distinct_nonces_give_distinct_ciphertext() {
        let (m, _id) = mgr_with_chacha_key();
        let pt = b"same plaintext both times";
        let a = m.encrypt_blob(pt, [3u8; 12]).unwrap();
        let b = m.encrypt_blob(pt, [4u8; 12]).unwrap();
        assert_ne!(&a[12..], &b[12..], "distinct nonces -> distinct ciphertext");
    }

    /// Decrypting under the wrong key fails closed; the right key succeeds.
    #[test]
    fn wrong_key_fails_closed() {
        let mut m = KeyManager::new();
        let id1 = m.add_key(
            EncryptionAlgorithm::ChaCha20Poly1305,
            [0x11u8; 32],
            String::new(),
            0,
        );
        let id2 = m.add_key(
            EncryptionAlgorithm::ChaCha20Poly1305,
            [0x22u8; 32],
            String::new(),
            0,
        );
        let blob = m.encrypt_blob(b"data", [5u8; 12]).unwrap(); // active = id1
        assert!(m.decrypt_blob(id2, &blob).is_err(), "wrong key must fail");
        assert!(m.decrypt_blob(id1, &blob).is_ok(), "right key must succeed");
    }

    /// An unimplemented algorithm errors rather than emitting fake ciphertext.
    #[test]
    fn unsupported_algorithm_is_error_not_fake() {
        let mut m = KeyManager::new();
        m.add_key(EncryptionAlgorithm::Aes256Gcm, [0u8; 32], String::new(), 0);
        assert!(m.encrypt_blob(b"data", [6u8; 12]).is_err());
    }

    /// A too-short blob is CorruptedData, never an out-of-bounds slice.
    #[test]
    fn short_ciphertext_is_corrupt() {
        let (m, id) = mgr_with_chacha_key();
        assert!(m.decrypt_blob(id, &[0u8; 10]).is_err());
    }
}
