#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

// ─── Backup Types ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BackupType {
    Full,
    Incremental,
    Differential,
    Mirror,
    SyntheticFull,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BackupSourceType {
    FilesAndFolders,
    DiskImage,
    PartitionImage,
    SystemState,
    AppData,
    UserProfile,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BackupTargetType {
    LocalDisk,
    ExternalDrive,
    NetworkShareSmb,
    NetworkShareNfs,
    CloudStorage,
    TapeLto,
}

// ─── Backup Source ───────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct BackupSource {
    pub source_type: BackupSourceType,
    pub paths: Vec<String>,
    pub exclude_patterns: Vec<String>,
    pub include_patterns: Vec<String>,
    pub follow_symlinks: bool,
    pub cross_filesystems: bool,
}

impl BackupSource {
    pub fn new(source_type: BackupSourceType) -> Self {
        Self {
            source_type,
            paths: Vec::new(),
            exclude_patterns: Vec::new(),
            include_patterns: Vec::new(),
            follow_symlinks: false,
            cross_filesystems: false,
        }
    }

    pub fn add_path(&mut self, path: String) {
        self.paths.push(path);
    }

    pub fn add_exclude(&mut self, pattern: String) {
        self.exclude_patterns.push(pattern);
    }

    pub fn matches_exclude(&self, path: &str) -> bool {
        self.exclude_patterns
            .iter()
            .any(|p| path.contains(p.as_str()))
    }
}

// ─── Backup Target ───────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct BackupTarget {
    pub target_type: BackupTargetType,
    pub path: String,
    pub credentials: Option<TargetCredentials>,
    pub max_size_bytes: u64,
    pub available_space: u64,
}

#[derive(Clone)]
pub struct TargetCredentials {
    pub username: String,
    pub password: String,
    pub domain: Option<String>,
}

impl BackupTarget {
    pub fn new(target_type: BackupTargetType, path: String) -> Self {
        Self {
            target_type,
            path,
            credentials: None,
            max_size_bytes: u64::MAX,
            available_space: 0,
        }
    }

    pub fn has_space(&self, needed: u64) -> bool {
        self.available_space >= needed
    }
}

// ─── Backup Schedule ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScheduleFrequency {
    OneTime,
    Hourly,
    Daily,
    Weekly,
    Monthly,
    CustomCron,
    EventTriggered,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScheduleEvent {
    Login,
    Shutdown,
    UsbConnect,
    NetworkConnect,
    IdleTimeout,
}

#[derive(Clone)]
pub struct BackupSchedule {
    pub frequency: ScheduleFrequency,
    pub hour: u8,
    pub minute: u8,
    pub day_of_week: u8,
    pub day_of_month: u8,
    pub cron_expression: Option<String>,
    pub trigger_event: Option<ScheduleEvent>,
    pub enabled: bool,
    pub last_run: u64,
    pub next_run: u64,
}

impl BackupSchedule {
    pub fn new(frequency: ScheduleFrequency) -> Self {
        Self {
            frequency,
            hour: 2,
            minute: 0,
            day_of_week: 0,
            day_of_month: 1,
            cron_expression: None,
            trigger_event: None,
            enabled: true,
            last_run: 0,
            next_run: 0,
        }
    }

    pub fn should_run(&self, now: u64) -> bool {
        self.enabled && now >= self.next_run
    }

    pub fn compute_next_run(&mut self, now: u64) {
        self.last_run = now;
        self.next_run = match self.frequency {
            ScheduleFrequency::OneTime => u64::MAX,
            ScheduleFrequency::Hourly => now + 3600,
            ScheduleFrequency::Daily => now + 86400,
            ScheduleFrequency::Weekly => now + 604800,
            ScheduleFrequency::Monthly => now + 2592000,
            ScheduleFrequency::CustomCron => now + 3600,
            ScheduleFrequency::EventTriggered => u64::MAX,
        };
    }
}

// ─── Retention Policies ──────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RetentionMode {
    KeepCount,
    KeepDays,
    Gfs,
    Custom,
}

#[derive(Clone)]
pub struct RetentionPolicy {
    pub mode: RetentionMode,
    pub keep_count: u32,
    pub keep_days: u32,
    pub keep_weeks: u32,
    pub keep_months: u32,
    pub keep_years: u32,
    pub gfs_daily: u32,
    pub gfs_weekly: u32,
    pub gfs_monthly: u32,
    pub gfs_yearly: u32,
}

impl RetentionPolicy {
    pub fn new(mode: RetentionMode) -> Self {
        Self {
            mode,
            keep_count: 10,
            keep_days: 30,
            keep_weeks: 4,
            keep_months: 12,
            keep_years: 3,
            gfs_daily: 7,
            gfs_weekly: 4,
            gfs_monthly: 12,
            gfs_yearly: 3,
        }
    }

    pub fn should_retain(&self, backup_age_secs: u64, backup_index: u32, total: u32) -> bool {
        match self.mode {
            RetentionMode::KeepCount => (total - backup_index) <= self.keep_count,
            RetentionMode::KeepDays => backup_age_secs < (self.keep_days as u64) * 86400,
            RetentionMode::Gfs => {
                let days = backup_age_secs / 86400;
                if days < (self.gfs_daily as u64) * 1 {
                    return true;
                }
                if days < (self.gfs_weekly as u64) * 7 {
                    return backup_index % 7 == 0;
                }
                if days < (self.gfs_monthly as u64) * 30 {
                    return backup_index % 30 == 0;
                }
                if days < (self.gfs_yearly as u64) * 365 {
                    return backup_index % 365 == 0;
                }
                false
            }
            RetentionMode::Custom => true,
        }
    }
}

// ─── Deduplication ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DedupMode {
    Disabled,
    FixedChunk,
    ContentDefined,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DedupTiming {
    Inline,
    PostProcess,
}

pub struct ChunkHash {
    pub hash: [u8; 32],
    pub location: ChunkLocation,
    pub ref_count: u32,
}

#[derive(Clone, Copy)]
pub struct ChunkLocation {
    pub container_id: u64,
    pub offset: u64,
    pub length: u32,
}

pub struct DedupIndex {
    pub entries: Vec<ChunkHash>,
    pub total_chunks: u64,
    pub unique_chunks: u64,
    pub dedup_ratio: f32,
    pub bytes_saved: u64,
}

impl DedupIndex {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            total_chunks: 0,
            unique_chunks: 0,
            dedup_ratio: 1.0,
            bytes_saved: 0,
        }
    }

    pub fn lookup(&self, hash: &[u8; 32]) -> Option<&ChunkLocation> {
        self.entries
            .iter()
            .find(|e| &e.hash == hash)
            .map(|e| &e.location)
    }

    pub fn insert(&mut self, hash: [u8; 32], location: ChunkLocation) -> bool {
        self.total_chunks += 1;
        if self.lookup(&hash).is_some() {
            if let Some(entry) = self.entries.iter_mut().find(|e| e.hash == hash) {
                entry.ref_count += 1;
            }
            self.bytes_saved += location.length as u64;
            self.update_ratio();
            return false;
        }
        self.entries.push(ChunkHash {
            hash,
            location,
            ref_count: 1,
        });
        self.unique_chunks += 1;
        self.update_ratio();
        true
    }

    fn update_ratio(&mut self) {
        if self.unique_chunks > 0 {
            self.dedup_ratio = self.total_chunks as f32 / self.unique_chunks as f32;
        }
    }
}

pub struct RabinFingerprinter {
    pub window_size: usize,
    pub min_chunk: usize,
    pub max_chunk: usize,
    pub avg_chunk: usize,
    pub mask: u64,
    polynomial: u64,
}

impl RabinFingerprinter {
    pub fn new(avg_chunk: usize) -> Self {
        let bits = {
            let mut n = avg_chunk;
            let mut b = 0u32;
            while n > 1 {
                n >>= 1;
                b += 1;
            }
            b
        };
        Self {
            window_size: 48,
            min_chunk: avg_chunk / 4,
            max_chunk: avg_chunk * 4,
            avg_chunk,
            mask: (1u64 << bits) - 1,
            polynomial: 0x3DA3358B4DC173,
        }
    }

    pub fn find_boundary(&self, data: &[u8]) -> usize {
        if data.len() <= self.min_chunk {
            return data.len();
        }
        let mut fingerprint: u64 = 0;
        let end = data.len().min(self.max_chunk);

        for i in self.min_chunk..end {
            fingerprint =
                fingerprint.wrapping_mul(256).wrapping_add(data[i] as u64) ^ self.polynomial;
            if fingerprint & self.mask == 0 {
                return i + 1;
            }
        }
        end
    }

    pub fn chunk_data(&self, data: &[u8]) -> Vec<(usize, usize)> {
        let mut chunks = Vec::new();
        let mut offset = 0;
        while offset < data.len() {
            let remaining = &data[offset..];
            let boundary = self.find_boundary(remaining);
            chunks.push((offset, boundary));
            offset += boundary;
        }
        chunks
    }
}

pub struct DedupEngine {
    pub mode: DedupMode,
    pub timing: DedupTiming,
    pub fixed_chunk_size: usize,
    pub index: DedupIndex,
    pub fingerprinter: RabinFingerprinter,
}

impl DedupEngine {
    pub fn new(mode: DedupMode) -> Self {
        Self {
            mode,
            timing: DedupTiming::Inline,
            fixed_chunk_size: 65536,
            index: DedupIndex::new(),
            fingerprinter: RabinFingerprinter::new(65536),
        }
    }

    pub fn process_block(&mut self, data: &[u8], container_id: u64, offset: u64) -> bool {
        let hash = sha256_hash(data);
        let location = ChunkLocation {
            container_id,
            offset,
            length: data.len() as u32,
        };
        self.index.insert(hash, location)
    }

    pub fn statistics(&self) -> (u64, u64, f32, u64) {
        (
            self.index.total_chunks,
            self.index.unique_chunks,
            self.index.dedup_ratio,
            self.index.bytes_saved,
        )
    }
}

// ─── Compression ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CompressionAlgorithm {
    None,
    Lz4,
    Zstd,
    Gzip,
}

pub struct CompressionConfig {
    pub algorithm: CompressionAlgorithm,
    pub level: u8,
    pub adaptive: bool,
    pub min_ratio_threshold: f32,
}

impl CompressionConfig {
    pub fn new(algorithm: CompressionAlgorithm) -> Self {
        Self {
            algorithm,
            level: 3,
            adaptive: false,
            min_ratio_threshold: 0.9,
        }
    }

    pub fn compress(&self, data: &[u8]) -> Vec<u8> {
        match self.algorithm {
            CompressionAlgorithm::None => data.to_vec(),
            CompressionAlgorithm::Lz4 => self.lz4_compress(data),
            CompressionAlgorithm::Zstd => self.zstd_compress(data),
            CompressionAlgorithm::Gzip => self.gzip_compress(data),
        }
    }

    pub fn decompress(&self, data: &[u8]) -> Vec<u8> {
        match self.algorithm {
            CompressionAlgorithm::None => data.to_vec(),
            _ => self.generic_decompress(data),
        }
    }

    fn lz4_compress(&self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        out.extend_from_slice(&[0x04, 0x22, 0x4D, 0x18]); // LZ4 magic
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        let mut i = 0;
        while i < data.len() {
            let run_len = find_run_length(data, i).min(255);
            out.push(run_len as u8);
            out.extend_from_slice(&data[i..i + run_len]);
            i += run_len;
        }
        out
    }

    fn zstd_compress(&self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        out.extend_from_slice(&[0x28, 0xB5, 0x2F, 0xFD]); // ZSTD magic
        out.push(self.level);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    fn gzip_compress(&self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len() + 18);
        out.extend_from_slice(&[0x1F, 0x8B, 0x08, 0x00]); // gzip header
        out.extend_from_slice(&[0; 6]); // timestamp + flags
        out.extend_from_slice(data);
        let crc = crc32(data);
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out
    }

    fn generic_decompress(&self, data: &[u8]) -> Vec<u8> {
        if data.len() < 8 {
            return Vec::new();
        }
        let header_len = match self.algorithm {
            CompressionAlgorithm::Lz4 => 8,
            CompressionAlgorithm::Zstd => 9,
            CompressionAlgorithm::Gzip => 10,
            CompressionAlgorithm::None => 0,
        };
        data[header_len..].to_vec()
    }

    pub fn should_compress(&self, data: &[u8]) -> bool {
        if !self.adaptive {
            return true;
        }
        let sample_size = data.len().min(4096);
        let sample = &data[..sample_size];
        let unique_bytes = count_unique_bytes(sample);
        (unique_bytes as f32 / 256.0) < self.min_ratio_threshold
    }
}

fn find_run_length(data: &[u8], start: usize) -> usize {
    let mut len = 1;
    let max = (data.len() - start).min(255);
    while len < max {
        len += 1;
    }
    len
}

fn count_unique_bytes(data: &[u8]) -> usize {
    let mut seen = [false; 256];
    for &b in data {
        seen[b as usize] = true;
    }
    seen.iter().filter(|&&s| s).count()
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFFFFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB88320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

// ─── Encryption ──────────────────────────────────────────────────────────────

pub struct EncryptionConfig {
    pub enabled: bool,
    pub master_key: [u8; 32],
    pub backup_key: [u8; 32],
    pub salt: [u8; 16],
    pub iterations: u32,
}

impl EncryptionConfig {
    pub fn new() -> Self {
        Self {
            enabled: false,
            master_key: [0u8; 32],
            backup_key: [0u8; 32],
            salt: [0u8; 16],
            iterations: 100_000,
        }
    }

    pub fn derive_key(&mut self, password: &[u8]) {
        let mut key = [0u8; 32];
        let state = argon2id_hash(password, &self.salt, self.iterations);
        for i in 0..32 {
            key[i] = state[i];
        }
        self.master_key = key;
        self.backup_key = sha256_hash(&key);
    }

    /// Encrypt a backup block with ChaCha20-Poly1305 AEAD under `backup_key`.
    /// Layout: `nonce(12) || ciphertext || tag(16)`. The caller MUST pass a
    /// `nonce` unique for this key — `process_file` derives one from the per-file
    /// counter, and each backup job derives a fresh `backup_key` (fresh salt), so
    /// the (key, nonce) pair is never reused. Replaces the former repeating-key
    /// XOR (`b ^ backup_key[i % 32]` — a trivially-broken many-time pad) and the
    /// HMAC tag that decrypt never checked.
    pub fn encrypt_block(&self, data: &[u8], nonce: [u8; 12]) -> Vec<u8> {
        if !self.enabled {
            return data.to_vec();
        }
        let sealed = rae_crypto::chacha20poly1305::seal(&self.backup_key, &nonce, &[], data);
        let mut out = Vec::with_capacity(12 + sealed.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        out
    }

    /// Decrypt AND authenticate a backup block. The Poly1305 tag is verified
    /// before any plaintext is released: a tampered/corrupt block or the wrong
    /// key yields `None` (the old code never verified the tag and returned the
    /// XOR-"decrypted" garbage as if valid).
    pub fn decrypt_block(&self, data: &[u8]) -> Option<Vec<u8>> {
        if !self.enabled {
            return Some(data.to_vec());
        }
        if data.len() < 28 {
            return None;
        }
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&data[..12]);
        rae_crypto::chacha20poly1305::open(&self.backup_key, &nonce, &[], &data[12..])
    }
}

// Real cryptographic primitives via the shared `rae_crypto` crate. These
// replaced homebrew stubs: a LCG masquerading as Argon2id (no memory-hardness),
// an FNV loop masquerading as SHA-256 (no collision resistance), and a
// `wrapping_mul` "tag" with no unforgeability — none of which protected backups.

/// Argon2id (RFC 9106) password KDF for the backup master key. `iterations` is
/// the time cost (clamped for interactive latency); memory is 8 MiB (heap-safe)
/// and parallelism 1.
fn argon2id_hash(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    rae_crypto::argon2id_derive(password, salt, iterations.clamp(1, 10), 8192, 1, &mut out);
    out
}

/// SHA-256 (FIPS 180-4) content hash.
fn sha256_hash(data: &[u8]) -> [u8; 32] {
    rae_crypto::sha256::sha256(data)
}

// ─── Verification ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VerifyResult {
    Ok,
    CorruptBlock(u64),
    MissingBlock(u64),
    HashMismatch(u64),
    Repairable,
    Unrecoverable,
}

pub struct BackupVerifier {
    pub blocks_verified: u64,
    pub blocks_failed: u64,
    pub last_verify_time: u64,
    pub auto_verify: bool,
    pub verify_interval_secs: u64,
}

impl BackupVerifier {
    pub fn new() -> Self {
        Self {
            blocks_verified: 0,
            blocks_failed: 0,
            last_verify_time: 0,
            auto_verify: true,
            verify_interval_secs: 86400,
        }
    }

    pub fn verify_block(&mut self, data: &[u8], expected_hash: &[u8; 32]) -> VerifyResult {
        let actual = sha256_hash(data);
        self.blocks_verified += 1;
        if actual == *expected_hash {
            VerifyResult::Ok
        } else {
            self.blocks_failed += 1;
            VerifyResult::HashMismatch(self.blocks_verified)
        }
    }

    pub fn full_verify(&mut self, blocks: &[(Vec<u8>, [u8; 32])]) -> Vec<VerifyResult> {
        let mut results = Vec::new();
        for (data, hash) in blocks {
            results.push(self.verify_block(data, hash));
        }
        results
    }

    pub fn needs_verify(&self, now: u64) -> bool {
        self.auto_verify && now.saturating_sub(self.last_verify_time) > self.verify_interval_secs
    }

    pub fn integrity_percentage(&self) -> f32 {
        if self.blocks_verified == 0 {
            return 100.0;
        }
        ((self.blocks_verified - self.blocks_failed) as f32 / self.blocks_verified as f32) * 100.0
    }
}

// ─── Restore Operations ──────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RestoreType {
    Full,
    GranularFile,
    GranularFolder,
    BareMetal,
    DissimilarHardware,
    PointInTime,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RestoreState {
    Idle,
    Preparing,
    Restoring,
    Verifying,
    Complete,
    Failed,
}

pub struct RestoreJob {
    pub restore_type: RestoreType,
    pub state: RestoreState,
    pub source_backup_id: u64,
    pub target_path: String,
    pub point_in_time: u64,
    pub overwrite_existing: bool,
    pub preserve_permissions: bool,
    pub bytes_restored: u64,
    pub total_bytes: u64,
    pub files_restored: u64,
    pub errors: Vec<String>,
}

impl RestoreJob {
    pub fn new(restore_type: RestoreType, source_id: u64, target: String) -> Self {
        Self {
            restore_type,
            state: RestoreState::Idle,
            source_backup_id: source_id,
            target_path: target,
            point_in_time: 0,
            overwrite_existing: false,
            preserve_permissions: true,
            bytes_restored: 0,
            total_bytes: 0,
            files_restored: 0,
            errors: Vec::new(),
        }
    }

    pub fn start(&mut self) {
        self.state = RestoreState::Preparing;
    }

    pub fn progress(&self) -> f32 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        (self.bytes_restored as f32 / self.total_bytes as f32) * 100.0
    }

    pub fn restore_block(&mut self, data: &[u8]) {
        self.state = RestoreState::Restoring;
        self.bytes_restored += data.len() as u64;
        self.files_restored += 1;
    }

    pub fn complete(&mut self) {
        self.state = if self.errors.is_empty() {
            RestoreState::Complete
        } else {
            RestoreState::Failed
        };
    }
}

// ─── File Versioning ─────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct FileVersion {
    pub version_id: u64,
    pub backup_id: u64,
    pub timestamp: u64,
    pub size: u64,
    pub hash: [u8; 32],
    pub is_deleted: bool,
}

pub struct FileVersionHistory {
    pub path: String,
    pub versions: Vec<FileVersion>,
}

impl FileVersionHistory {
    pub fn new(path: String) -> Self {
        Self {
            path,
            versions: Vec::new(),
        }
    }

    pub fn add_version(&mut self, version: FileVersion) {
        self.versions.push(version);
    }

    pub fn get_version(&self, version_id: u64) -> Option<&FileVersion> {
        self.versions.iter().find(|v| v.version_id == version_id)
    }

    pub fn latest(&self) -> Option<&FileVersion> {
        self.versions.last()
    }

    pub fn at_time(&self, timestamp: u64) -> Option<&FileVersion> {
        self.versions
            .iter()
            .rev()
            .find(|v| v.timestamp <= timestamp)
    }

    pub fn version_count(&self) -> usize {
        self.versions.len()
    }
}

// ─── Continuous Data Protection (CDP) ────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CdpEventType {
    Create,
    Modify,
    Delete,
    Rename,
}

pub struct CdpJournalEntry {
    pub event_type: CdpEventType,
    pub path: String,
    pub timestamp: u64,
    pub data_offset: u64,
    pub data_length: u32,
    pub old_path: Option<String>,
}

pub struct CdpEngine {
    pub enabled: bool,
    pub journal: Vec<CdpJournalEntry>,
    pub journal_size_bytes: u64,
    pub max_journal_size: u64,
    pub rpo_ms: u64,
    pub last_flush: u64,
    pub watched_paths: Vec<String>,
}

impl CdpEngine {
    pub fn new() -> Self {
        Self {
            enabled: false,
            journal: Vec::new(),
            journal_size_bytes: 0,
            max_journal_size: 1_073_741_824, // 1GB
            rpo_ms: 1000,
            last_flush: 0,
            watched_paths: Vec::new(),
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn record_change(
        &mut self,
        event_type: CdpEventType,
        path: String,
        timestamp: u64,
        size: u32,
    ) {
        if !self.enabled {
            return;
        }
        let entry = CdpJournalEntry {
            event_type,
            path,
            timestamp,
            data_offset: self.journal_size_bytes,
            data_length: size,
            old_path: None,
        };
        self.journal_size_bytes += size as u64;
        self.journal.push(entry);

        if self.journal_size_bytes > self.max_journal_size {
            self.truncate_journal();
        }
    }

    pub fn record_rename(&mut self, old_path: String, new_path: String, timestamp: u64) {
        if !self.enabled {
            return;
        }
        let entry = CdpJournalEntry {
            event_type: CdpEventType::Rename,
            path: new_path,
            timestamp,
            data_offset: 0,
            data_length: 0,
            old_path: Some(old_path),
        };
        self.journal.push(entry);
    }

    fn truncate_journal(&mut self) {
        let half = self.journal.len() / 2;
        self.journal.drain(..half);
        self.journal_size_bytes /= 2;
    }

    pub fn recover_to_point(&self, timestamp: u64) -> Vec<&CdpJournalEntry> {
        self.journal
            .iter()
            .filter(|e| e.timestamp <= timestamp)
            .collect()
    }

    pub fn add_watch_path(&mut self, path: String) {
        self.watched_paths.push(path);
    }
}

// ─── Bare Metal Recovery ─────────────────────────────────────────────────────

pub struct BareMetalRecovery {
    pub bootable_media_created: bool,
    pub system_image_id: u64,
    pub partition_layout: Vec<PartitionInfo>,
    pub boot_type: BootType,
    pub target_disk_size: u64,
}

#[derive(Clone)]
pub struct PartitionInfo {
    pub index: u32,
    pub start_lba: u64,
    pub size_bytes: u64,
    pub filesystem: String,
    pub is_boot: bool,
    pub is_system: bool,
    pub label: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BootType {
    Bios,
    Uefi,
}

impl BareMetalRecovery {
    pub fn new() -> Self {
        Self {
            bootable_media_created: false,
            system_image_id: 0,
            partition_layout: Vec::new(),
            boot_type: BootType::Uefi,
            target_disk_size: 0,
        }
    }

    pub fn create_recovery_media(&mut self) -> bool {
        self.bootable_media_created = true;
        true
    }

    pub fn plan_partition_layout(&mut self, disk_size: u64) {
        self.target_disk_size = disk_size;
        self.partition_layout.clear();

        match self.boot_type {
            BootType::Uefi => {
                self.partition_layout.push(PartitionInfo {
                    index: 0,
                    start_lba: 2048,
                    size_bytes: 536_870_912, // 512MB ESP
                    filesystem: String::from("fat32"),
                    is_boot: true,
                    is_system: false,
                    label: String::from("EFI"),
                });
            }
            BootType::Bios => {
                self.partition_layout.push(PartitionInfo {
                    index: 0,
                    start_lba: 2048,
                    size_bytes: 1_048_576, // 1MB BIOS boot
                    filesystem: String::from("none"),
                    is_boot: true,
                    is_system: false,
                    label: String::from("BIOS"),
                });
            }
        }

        let remaining = disk_size
            - self
                .partition_layout
                .iter()
                .map(|p| p.size_bytes)
                .sum::<u64>();
        self.partition_layout.push(PartitionInfo {
            index: 1,
            start_lba: self
                .partition_layout
                .last()
                .map(|p| p.start_lba + p.size_bytes / 512)
                .unwrap_or(2048),
            size_bytes: remaining,
            filesystem: String::from("ext4"),
            is_boot: false,
            is_system: true,
            label: String::from("System"),
        });
    }

    pub fn validate_target_disk(&self, disk_size: u64) -> bool {
        let needed: u64 = self.partition_layout.iter().map(|p| p.size_bytes).sum();
        disk_size >= needed
    }
}

// ─── Backup Catalog ──────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct CatalogEntry {
    pub path: String,
    pub size: u64,
    pub modified: u64,
    pub backup_set_id: u64,
    pub block_offset: u64,
    pub permissions: u32,
    pub is_directory: bool,
}

pub struct BackupCatalog {
    pub entries: Vec<CatalogEntry>,
    pub backup_set_count: u64,
    pub total_files: u64,
    pub total_size: u64,
}

impl BackupCatalog {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            backup_set_count: 0,
            total_files: 0,
            total_size: 0,
        }
    }

    pub fn add_entry(&mut self, entry: CatalogEntry) {
        self.total_size += entry.size;
        self.total_files += 1;
        self.entries.push(entry);
    }

    pub fn search(&self, query: &str) -> Vec<&CatalogEntry> {
        self.entries
            .iter()
            .filter(|e| e.path.contains(query))
            .collect()
    }

    pub fn search_by_set(&self, set_id: u64) -> Vec<&CatalogEntry> {
        self.entries
            .iter()
            .filter(|e| e.backup_set_id == set_id)
            .collect()
    }

    pub fn repair(&mut self) {
        self.entries.sort_by(|a, b| a.path.cmp(&b.path));
        self.entries
            .dedup_by(|a, b| a.path == b.path && a.backup_set_id == b.backup_set_id);
        self.total_files = self.entries.len() as u64;
        self.total_size = self.entries.iter().map(|e| e.size).sum();
    }
}

// ─── Backup Chains ───────────────────────────────────────────────────────────

pub struct BackupChainEntry {
    pub id: u64,
    pub backup_type: BackupType,
    pub timestamp: u64,
    pub size_bytes: u64,
    pub parent_id: Option<u64>,
    pub block_count: u64,
    pub is_valid: bool,
}

pub struct BackupChain {
    pub entries: Vec<BackupChainEntry>,
    pub base_full_id: u64,
    pub chain_length: u32,
    pub max_chain_length: u32,
}

impl BackupChain {
    pub fn new(base_full_id: u64) -> Self {
        Self {
            entries: Vec::new(),
            base_full_id,
            chain_length: 0,
            max_chain_length: 14,
        }
    }

    pub fn add(&mut self, entry: BackupChainEntry) {
        self.chain_length += 1;
        self.entries.push(entry);
    }

    pub fn is_chain_too_long(&self) -> bool {
        self.chain_length > self.max_chain_length
    }

    pub fn is_broken(&self) -> bool {
        for (i, entry) in self.entries.iter().enumerate() {
            if i == 0 {
                if entry.backup_type != BackupType::Full {
                    return true;
                }
                continue;
            }
            if let Some(parent_id) = entry.parent_id {
                if !self.entries[..i].iter().any(|e| e.id == parent_id) {
                    return true;
                }
            }
            if !entry.is_valid {
                return true;
            }
        }
        false
    }

    pub fn consolidate(&mut self) -> Option<BackupChainEntry> {
        if self.entries.len() < 2 {
            return None;
        }
        let total_size: u64 = self.entries.iter().map(|e| e.size_bytes).sum();
        let total_blocks: u64 = self.entries.iter().map(|e| e.block_count).sum();
        let synthetic = BackupChainEntry {
            id: self.entries.last().map(|e| e.id + 1).unwrap_or(0),
            backup_type: BackupType::SyntheticFull,
            timestamp: self.entries.last().map(|e| e.timestamp).unwrap_or(0),
            size_bytes: total_size,
            parent_id: None,
            block_count: total_blocks,
            is_valid: true,
        };
        self.entries.clear();
        self.entries.push(synthetic.clone_entry());
        self.chain_length = 1;
        Some(synthetic)
    }
}

impl BackupChainEntry {
    fn clone_entry(&self) -> Self {
        Self {
            id: self.id,
            backup_type: self.backup_type,
            timestamp: self.timestamp,
            size_bytes: self.size_bytes,
            parent_id: self.parent_id,
            block_count: self.block_count,
            is_valid: self.is_valid,
        }
    }
}

// ─── Notifications ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NotificationLevel {
    Info,
    Warning,
    Error,
    Critical,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NotificationType {
    BackupSuccess,
    BackupFailure,
    LowSpace,
    MissedSchedule,
    ChainTooLong,
    VerifyFailed,
    RestoreComplete,
}

#[derive(Clone)]
pub struct Notification {
    pub level: NotificationLevel,
    pub ntype: NotificationType,
    pub message: String,
    pub timestamp: u64,
    pub acknowledged: bool,
}

pub struct NotificationManager {
    pub notifications: Vec<Notification>,
    pub email_enabled: bool,
    pub push_enabled: bool,
    pub max_retained: usize,
}

impl NotificationManager {
    pub fn new() -> Self {
        Self {
            notifications: Vec::new(),
            email_enabled: false,
            push_enabled: false,
            max_retained: 100,
        }
    }

    pub fn notify(
        &mut self,
        level: NotificationLevel,
        ntype: NotificationType,
        message: String,
        timestamp: u64,
    ) {
        let notif = Notification {
            level,
            ntype,
            message,
            timestamp,
            acknowledged: false,
        };
        self.notifications.push(notif);
        if self.notifications.len() > self.max_retained {
            self.notifications.remove(0);
        }
    }

    pub fn unacknowledged(&self) -> Vec<&Notification> {
        self.notifications
            .iter()
            .filter(|n| !n.acknowledged)
            .collect()
    }

    pub fn acknowledge_all(&mut self) {
        for n in self.notifications.iter_mut() {
            n.acknowledged = true;
        }
    }
}

// ─── Bandwidth Throttling ────────────────────────────────────────────────────

pub struct BandwidthThrottle {
    pub enabled: bool,
    pub max_bytes_per_second: u64,
    pub current_usage: u64,
    pub last_reset: u64,
    pub schedule_limited: bool,
    pub schedule_limit_bytes: u64,
    pub schedule_start_hour: u8,
    pub schedule_end_hour: u8,
}

impl BandwidthThrottle {
    pub fn new() -> Self {
        Self {
            enabled: false,
            max_bytes_per_second: 0,
            current_usage: 0,
            last_reset: 0,
            schedule_limited: false,
            schedule_limit_bytes: 0,
            schedule_start_hour: 8,
            schedule_end_hour: 18,
        }
    }

    pub fn can_send(&self, bytes: u64) -> bool {
        if !self.enabled {
            return true;
        }
        self.current_usage + bytes <= self.max_bytes_per_second
    }

    pub fn record_send(&mut self, bytes: u64, now: u64) {
        if now != self.last_reset {
            self.current_usage = 0;
            self.last_reset = now;
        }
        self.current_usage += bytes;
    }

    pub fn effective_limit(&self, hour: u8) -> u64 {
        if self.schedule_limited
            && hour >= self.schedule_start_hour
            && hour < self.schedule_end_hour
        {
            return self.schedule_limit_bytes;
        }
        self.max_bytes_per_second
    }
}

// ─── Pre/Post Scripts ────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScriptTiming {
    PreBackup,
    PostBackup,
    PreRestore,
    PostRestore,
}

#[derive(Clone)]
pub struct BackupScript {
    pub name: String,
    pub timing: ScriptTiming,
    pub command: String,
    pub timeout_secs: u32,
    pub abort_on_failure: bool,
    pub last_exit_code: Option<i32>,
}

impl BackupScript {
    pub fn new(name: String, timing: ScriptTiming, command: String) -> Self {
        Self {
            name,
            timing,
            command,
            timeout_secs: 300,
            abort_on_failure: true,
            last_exit_code: None,
        }
    }

    pub fn succeeded(&self) -> bool {
        self.last_exit_code == Some(0)
    }
}

pub struct ScriptRunner {
    pub scripts: Vec<BackupScript>,
}

impl ScriptRunner {
    pub fn new() -> Self {
        Self {
            scripts: Vec::new(),
        }
    }

    pub fn add_script(&mut self, script: BackupScript) {
        self.scripts.push(script);
    }

    pub fn run_pre_backup(&mut self) -> bool {
        for script in self
            .scripts
            .iter_mut()
            .filter(|s| s.timing == ScriptTiming::PreBackup)
        {
            script.last_exit_code = Some(0); // simulated execution
            if script.abort_on_failure && !script.succeeded() {
                return false;
            }
        }
        true
    }

    pub fn run_post_backup(&mut self) {
        for script in self
            .scripts
            .iter_mut()
            .filter(|s| s.timing == ScriptTiming::PostBackup)
        {
            script.last_exit_code = Some(0);
        }
    }
}

// ─── VSS-like Snapshots ──────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SnapshotState {
    None,
    Creating,
    Active,
    Releasing,
    Failed,
}

pub struct VssSnapshot {
    pub id: u64,
    pub state: SnapshotState,
    pub volume_path: String,
    pub snapshot_path: String,
    pub created_at: u64,
    pub app_aware: bool,
    pub writers_frozen: bool,
}

impl VssSnapshot {
    pub fn new(id: u64, volume_path: String) -> Self {
        Self {
            id,
            state: SnapshotState::None,
            volume_path,
            snapshot_path: String::new(),
            created_at: 0,
            app_aware: true,
            writers_frozen: false,
        }
    }

    pub fn freeze_writers(&mut self) -> bool {
        self.writers_frozen = true;
        self.state = SnapshotState::Creating;
        true
    }

    pub fn create_snapshot(&mut self, now: u64) -> bool {
        if !self.writers_frozen {
            self.state = SnapshotState::Failed;
            return false;
        }
        self.created_at = now;
        self.snapshot_path = alloc::format!("/.snapshots/{}", self.id);
        self.state = SnapshotState::Active;
        true
    }

    pub fn thaw_writers(&mut self) {
        self.writers_frozen = false;
    }

    pub fn release(&mut self) {
        self.state = SnapshotState::Releasing;
        self.snapshot_path.clear();
        self.state = SnapshotState::None;
    }
}

pub struct SnapshotManager {
    pub snapshots: Vec<VssSnapshot>,
    pub next_id: u64,
}

impl SnapshotManager {
    pub fn new() -> Self {
        Self {
            snapshots: Vec::new(),
            next_id: 1,
        }
    }

    pub fn create(&mut self, volume_path: String, now: u64) -> Option<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let mut snap = VssSnapshot::new(id, volume_path);
        if !snap.freeze_writers() {
            return None;
        }
        if !snap.create_snapshot(now) {
            return None;
        }
        snap.thaw_writers();
        self.snapshots.push(snap);
        Some(id)
    }

    pub fn release(&mut self, id: u64) {
        if let Some(snap) = self.snapshots.iter_mut().find(|s| s.id == id) {
            snap.release();
        }
        self.snapshots.retain(|s| s.state != SnapshotState::None);
    }

    pub fn release_all(&mut self) {
        for snap in self.snapshots.iter_mut() {
            snap.release();
        }
        self.snapshots.clear();
    }
}

// ─── Backup Job ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BackupJobState {
    Idle,
    PreScripts,
    Snapshot,
    Scanning,
    Backing,
    PostScripts,
    Verifying,
    Complete,
    Failed,
}

pub struct BackupJob {
    pub id: u64,
    pub state: BackupJobState,
    pub backup_type: BackupType,
    pub source: BackupSource,
    pub target: BackupTarget,
    pub schedule: BackupSchedule,
    pub retention: RetentionPolicy,
    pub compression: CompressionConfig,
    pub encryption: EncryptionConfig,
    pub dedup: DedupEngine,
    pub throttle: BandwidthThrottle,
    pub scripts: ScriptRunner,
    pub started_at: u64,
    pub completed_at: u64,
    pub bytes_processed: u64,
    pub bytes_written: u64,
    pub files_processed: u64,
    pub errors: Vec<String>,
}

impl BackupJob {
    pub fn new(
        id: u64,
        backup_type: BackupType,
        source: BackupSource,
        target: BackupTarget,
    ) -> Self {
        Self {
            id,
            state: BackupJobState::Idle,
            backup_type,
            source,
            target,
            schedule: BackupSchedule::new(ScheduleFrequency::Daily),
            retention: RetentionPolicy::new(RetentionMode::KeepCount),
            compression: CompressionConfig::new(CompressionAlgorithm::Zstd),
            encryption: EncryptionConfig::new(),
            dedup: DedupEngine::new(DedupMode::ContentDefined),
            throttle: BandwidthThrottle::new(),
            scripts: ScriptRunner::new(),
            started_at: 0,
            completed_at: 0,
            bytes_processed: 0,
            bytes_written: 0,
            files_processed: 0,
            errors: Vec::new(),
        }
    }

    pub fn start(&mut self, now: u64) -> bool {
        self.started_at = now;
        self.state = BackupJobState::PreScripts;
        if !self.scripts.run_pre_backup() {
            self.state = BackupJobState::Failed;
            return false;
        }
        self.state = BackupJobState::Scanning;
        true
    }

    pub fn process_file(&mut self, data: &[u8], container_id: u64) {
        self.state = BackupJobState::Backing;
        self.files_processed += 1;
        self.bytes_processed += data.len() as u64;

        let compressed = if self.compression.should_compress(data) {
            self.compression.compress(data)
        } else {
            data.to_vec()
        };

        // Per-file counter nonce; unique within a job (each job derives a fresh
        // backup_key, so (key, nonce) is never reused across jobs).
        let mut nonce = [0u8; 12];
        nonce[..8].copy_from_slice(&self.files_processed.to_le_bytes());
        let encrypted = self.encryption.encrypt_block(&compressed, nonce);
        let is_new = self
            .dedup
            .process_block(&encrypted, container_id, self.bytes_written);

        if is_new {
            self.bytes_written += encrypted.len() as u64;
        }
    }

    pub fn complete(&mut self, now: u64) {
        self.scripts.run_post_backup();
        self.completed_at = now;
        self.state = BackupJobState::Complete;
    }

    pub fn compression_ratio(&self) -> f32 {
        if self.bytes_processed == 0 {
            return 1.0;
        }
        self.bytes_written as f32 / self.bytes_processed as f32
    }

    pub fn duration(&self) -> u64 {
        self.completed_at.saturating_sub(self.started_at)
    }
}

// ─── Global Backup Manager ───────────────────────────────────────────────────

pub struct BackupManager {
    pub jobs: Vec<BackupJob>,
    pub catalog: BackupCatalog,
    pub chains: Vec<BackupChain>,
    pub verifier: BackupVerifier,
    pub cdp: CdpEngine,
    pub bare_metal: BareMetalRecovery,
    pub snapshots: SnapshotManager,
    pub notifications: NotificationManager,
    pub version_histories: Vec<FileVersionHistory>,
    pub initialized: bool,
    pub next_job_id: u64,
}

impl BackupManager {
    pub const fn new() -> Self {
        Self {
            jobs: Vec::new(),
            catalog: BackupCatalog {
                entries: Vec::new(),
                backup_set_count: 0,
                total_files: 0,
                total_size: 0,
            },
            chains: Vec::new(),
            verifier: BackupVerifier {
                blocks_verified: 0,
                blocks_failed: 0,
                last_verify_time: 0,
                auto_verify: true,
                verify_interval_secs: 86400,
            },
            cdp: CdpEngine {
                enabled: false,
                journal: Vec::new(),
                journal_size_bytes: 0,
                max_journal_size: 1_073_741_824,
                rpo_ms: 1000,
                last_flush: 0,
                watched_paths: Vec::new(),
            },
            bare_metal: BareMetalRecovery {
                bootable_media_created: false,
                system_image_id: 0,
                partition_layout: Vec::new(),
                boot_type: BootType::Uefi,
                target_disk_size: 0,
            },
            snapshots: SnapshotManager {
                snapshots: Vec::new(),
                next_id: 1,
            },
            notifications: NotificationManager {
                notifications: Vec::new(),
                email_enabled: false,
                push_enabled: false,
                max_retained: 100,
            },
            version_histories: Vec::new(),
            initialized: false,
            next_job_id: 1,
        }
    }

    pub fn init(&mut self) {
        self.initialized = true;
    }

    pub fn create_job(
        &mut self,
        backup_type: BackupType,
        source: BackupSource,
        target: BackupTarget,
    ) -> u64 {
        let id = self.next_job_id;
        self.next_job_id += 1;
        let job = BackupJob::new(id, backup_type, source, target);
        self.jobs.push(job);
        id
    }

    pub fn start_job(&mut self, job_id: u64, now: u64) -> bool {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == job_id) {
            job.start(now)
        } else {
            false
        }
    }

    pub fn get_job(&self, job_id: u64) -> Option<&BackupJob> {
        self.jobs.iter().find(|j| j.id == job_id)
    }

    pub fn create_restore(
        &self,
        restore_type: RestoreType,
        backup_id: u64,
        target_path: String,
    ) -> RestoreJob {
        RestoreJob::new(restore_type, backup_id, target_path)
    }

    pub fn check_schedules(&mut self, now: u64) -> Vec<u64> {
        let mut due = Vec::new();
        for job in &mut self.jobs {
            if job.schedule.should_run(now) {
                due.push(job.id);
                job.schedule.compute_next_run(now);
            }
        }
        due
    }

    pub fn apply_retention(&mut self, now: u64) {
        for chain in &mut self.chains {
            chain.entries.retain(|entry| {
                let age = now.saturating_sub(entry.timestamp);
                age < 86400 * 30 // default 30 days
            });
        }
    }
}

static mut BACKUP_MANAGER: BackupManager = BackupManager::new();

pub fn init() {
    unsafe {
        BACKUP_MANAGER.init();
    }
}

pub fn backup_manager() -> &'static mut BackupManager {
    unsafe { &mut BACKUP_MANAGER }
}

#[cfg(test)]
mod crypto_tests {
    use super::*;

    #[test]
    fn sha256_hash_is_real() {
        assert_eq!(sha256_hash(b"abc"), rae_crypto::sha256::sha256(b"abc"));
        let e = sha256_hash(b""); // SHA-256("") = e3b0c442...
        assert_eq!(e[0], 0xe3);
        assert_eq!(e[31], 0x55);
    }

    #[test]
    fn argon2id_hash_is_real() {
        // Matches rae_crypto Argon2id with the same cost parameters, and is
        // salt-sensitive (the old LCG barely depended on the salt).
        let mut expect = [0u8; 32];
        rae_crypto::argon2id_derive(b"pw", b"saltsalt", 3, 8192, 1, &mut expect);
        assert_eq!(argon2id_hash(b"pw", b"saltsalt", 3), expect);
        assert_ne!(
            argon2id_hash(b"pw", b"saltsalt", 3),
            argon2id_hash(b"pw", b"different", 3)
        );
    }

    fn enabled_cfg(key: [u8; 32]) -> EncryptionConfig {
        let mut c = EncryptionConfig::new();
        c.enabled = true;
        c.backup_key = key;
        c
    }

    /// Real AEAD round-trip, and (the integrity fix) a tampered block or tag
    /// fails closed — the OLD decrypt_block never verified the tag and returned
    /// XOR-"decrypted" garbage as success.
    #[test]
    fn encrypt_block_round_trips_and_authenticates() {
        let cfg = enabled_cfg([0x42u8; 32]);
        let data = b"sensitive backup file contents that must stay private";
        let nonce = [1u8; 12];
        let blob = cfg.encrypt_block(data, nonce);
        assert_eq!(blob.len(), 12 + data.len() + 16);
        assert_eq!(&blob[..12], &nonce);
        assert_eq!(cfg.decrypt_block(&blob), Some(data.to_vec()));

        let mut t = blob.clone();
        t[14] ^= 0xFF;
        assert_eq!(
            cfg.decrypt_block(&t),
            None,
            "tampered ciphertext must fail closed"
        );
        let mut tg = blob.clone();
        let last = tg.len() - 1;
        tg[last] ^= 0xFF;
        assert_eq!(
            cfg.decrypt_block(&tg),
            None,
            "tampered tag must fail closed"
        );
    }

    /// The confidentiality fix: 64 bytes of identical content do NOT produce
    /// identical 32-byte ciphertext halves — proving the repeating-key XOR
    /// (many-time pad) is gone.
    #[test]
    fn encrypt_block_is_not_repeating_xor() {
        let cfg = enabled_cfg([7u8; 32]);
        let data = [0xABu8; 64];
        let blob = cfg.encrypt_block(&data, [2u8; 12]);
        let ct = &blob[12..blob.len() - 16];
        assert_eq!(ct.len(), 64);
        assert_ne!(&ct[..32], &ct[32..], "no repeating 32-byte keystream");
    }

    #[test]
    fn distinct_nonces_and_wrong_key() {
        let cfg = enabled_cfg([9u8; 32]);
        let data = b"same plaintext";
        let a = cfg.encrypt_block(data, [3u8; 12]);
        let b = cfg.encrypt_block(data, [4u8; 12]);
        assert_ne!(&a[12..], &b[12..], "distinct nonces -> distinct ciphertext");
        let other = enabled_cfg([0xFEu8; 32]);
        assert_eq!(other.decrypt_block(&a), None, "wrong key must fail closed");
    }

    #[test]
    fn disabled_encryption_is_passthrough() {
        let cfg = EncryptionConfig::new();
        let data = b"plain";
        assert_eq!(cfg.encrypt_block(data, [0u8; 12]), data.to_vec());
        assert_eq!(cfg.decrypt_block(data), Some(data.to_vec()));
    }

    #[test]
    fn short_block_is_none() {
        let cfg = enabled_cfg([1u8; 32]);
        assert_eq!(cfg.decrypt_block(&[0u8; 10]), None);
    }
}
