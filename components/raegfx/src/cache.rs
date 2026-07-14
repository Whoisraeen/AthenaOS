//! OS-Level Shader Cache — persistent compilation cache with LRU eviction.
//!
//! Shaders are identified by a SHA-256 hash of their source/SPIR-V. The cache
//! stores compiled IR so that subsequent loads skip compilation. This is the
//! "shader cache at the OS level, shared across Vulkan/RaeGFX" from the concept doc.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use crate::shader::{ShaderIR, ShaderStage};

// ═══════════════════════════════════════════════════════════════════════════
// Cache entry
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct CachedShader {
    pub hash: [u8; 32],
    pub compiled_ir: Vec<ShaderIR>,
    pub stage: ShaderStage,
    pub entry_point: String,
    pub compile_time_ms: u32,
    pub last_used: u64,
    pub hit_count: u32,
    pub source_size: u32,
}

impl CachedShader {
    pub fn estimated_size(&self) -> u64 {
        // Rough estimate: each IR instruction ~64 bytes + metadata overhead
        let ir_size = self.compiled_ir.len() as u64 * 64;
        let metadata = 32 + 4 + 4 + 8 + 4 + 4 + self.entry_point.len() as u64;
        ir_size + metadata
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Cache statistics
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct CacheStats {
    pub total_entries: usize,
    pub total_size: u64,
    pub max_size: u64,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub insertions: u64,
}

impl CacheStats {
    pub fn hit_rate(&self) -> f32 {
        let total = self.hits + self.misses;
        if total == 0 {
            return 0.0;
        }
        self.hits as f32 / total as f32
    }

    pub fn fill_ratio(&self) -> f32 {
        if self.max_size == 0 {
            return 0.0;
        }
        self.total_size as f32 / self.max_size as f32
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// SHA-256 (minimal inline implementation for no_std)
// ═══════════════════════════════════════════════════════════════════════════

pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (data.len() as u64) * 8;
    let mut padded = Vec::from(data);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for i in 0..8 {
        out[i * 4..i * 4 + 4].copy_from_slice(&h[i].to_be_bytes());
    }
    out
}

// ═══════════════════════════════════════════════════════════════════════════
// Shader Cache
// ═══════════════════════════════════════════════════════════════════════════

pub struct ShaderCache {
    entries: BTreeMap<[u8; 32], CachedShader>,
    max_entries: usize,
    total_size: u64,
    max_size: u64,
    lru_order: VecDeque<[u8; 32]>,
    hits: u64,
    misses: u64,
    evictions: u64,
    insertions: u64,
    timestamp: u64,
}

impl ShaderCache {
    pub fn new(max_entries: usize, max_size: u64) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_entries,
            total_size: 0,
            max_size,
            lru_order: VecDeque::new(),
            hits: 0,
            misses: 0,
            evictions: 0,
            insertions: 0,
            timestamp: 0,
        }
    }

    fn tick(&mut self) -> u64 {
        self.timestamp += 1;
        self.timestamp
    }

    /// Look up a shader by its source hash. Returns the cached compiled IR on hit.
    pub fn lookup(&mut self, source_hash: &[u8; 32]) -> Option<&CachedShader> {
        if self.entries.contains_key(source_hash) {
            let ts = self.tick();
            let entry = self.entries.get_mut(source_hash).unwrap();
            entry.hit_count += 1;
            entry.last_used = ts;
            self.hits += 1;
            self.promote_lru(source_hash);
            Some(self.entries.get(source_hash).unwrap())
        } else {
            self.misses += 1;
            None
        }
    }

    /// Insert a compiled shader into the cache. Evicts LRU entries if full.
    pub fn insert(&mut self, entry: CachedShader) {
        let hash = entry.hash;
        let entry_size = entry.estimated_size();

        // Evict until we have room
        while (self.entries.len() >= self.max_entries
            || self.total_size + entry_size > self.max_size)
            && !self.entries.is_empty()
        {
            self.evict_lru();
        }

        self.total_size += entry_size;
        self.entries.insert(hash, entry);
        self.lru_order.push_back(hash);
        self.insertions += 1;
    }

    /// Remove a specific entry from the cache.
    pub fn remove(&mut self, hash: &[u8; 32]) -> bool {
        if let Some(entry) = self.entries.remove(hash) {
            self.total_size = self.total_size.saturating_sub(entry.estimated_size());
            self.lru_order.retain(|h| h != hash);
            true
        } else {
            false
        }
    }

    /// Clear the entire cache.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.lru_order.clear();
        self.total_size = 0;
    }

    /// Get cache statistics.
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            total_entries: self.entries.len(),
            total_size: self.total_size,
            max_size: self.max_size,
            hits: self.hits,
            misses: self.misses,
            evictions: self.evictions,
            insertions: self.insertions,
        }
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub fn total_size(&self) -> u64 {
        self.total_size
    }

    pub fn hit_rate(&self) -> f32 {
        let total = self.hits + self.misses;
        if total == 0 {
            return 0.0;
        }
        self.hits as f32 / total as f32
    }

    /// Check if a hash exists in the cache without updating LRU.
    pub fn contains(&self, hash: &[u8; 32]) -> bool {
        self.entries.contains_key(hash)
    }

    /// Get the most frequently hit shaders (for diagnostics / preloading).
    pub fn hot_shaders(&self, count: usize) -> Vec<&CachedShader> {
        let mut entries: Vec<&CachedShader> = self.entries.values().collect();
        entries.sort_by(|a, b| b.hit_count.cmp(&a.hit_count));
        entries.truncate(count);
        entries
    }

    fn evict_lru(&mut self) {
        if let Some(hash) = self.lru_order.pop_front() {
            if let Some(entry) = self.entries.remove(&hash) {
                self.total_size = self.total_size.saturating_sub(entry.estimated_size());
                self.evictions += 1;
            }
        }
    }

    fn promote_lru(&mut self, hash: &[u8; 32]) {
        if let Some(pos) = self.lru_order.iter().position(|h| h == hash) {
            self.lru_order.remove(pos);
        }
        self.lru_order.push_back(*hash);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Cache-aware compilation pipeline
// ═══════════════════════════════════════════════════════════════════════════

/// Result of attempting to get a compiled shader from the cache.
pub enum CacheLookupResult<'a> {
    Hit(&'a CachedShader),
    Miss,
}

/// Compile a shader with caching. If the shader is already cached, return
/// the cached version. Otherwise compile it and insert into cache.
pub fn compile_with_cache(
    cache: &mut ShaderCache,
    source: &[u8],
    stage: ShaderStage,
    entry_point: &str,
    compile_fn: fn(&[u8]) -> Option<Vec<ShaderIR>>,
) -> Option<Vec<ShaderIR>> {
    let hash = sha256(source);

    if let Some(cached) = cache.lookup(&hash) {
        return Some(cached.compiled_ir.clone());
    }

    let compiled = compile_fn(source)?;

    let entry = CachedShader {
        hash,
        compiled_ir: compiled.clone(),
        stage,
        entry_point: String::from(entry_point),
        compile_time_ms: 0,
        last_used: 0,
        hit_count: 0,
        source_size: source.len() as u32,
    };

    cache.insert(entry);
    Some(compiled)
}

// ═══════════════════════════════════════════════════════════════════════════
// Persistent cache serialization format
// ═══════════════════════════════════════════════════════════════════════════

/// Magic bytes for the on-disk cache format.
pub const CACHE_MAGIC: [u8; 4] = [b'R', b'G', b'S', b'C'];
pub const CACHE_VERSION: u32 = 1;

/// Header for the persistent shader cache file.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct CacheFileHeader {
    pub magic: [u8; 4],
    pub version: u32,
    pub entry_count: u32,
    pub total_size: u64,
    pub gpu_vendor_id: u16,
    pub gpu_device_id: u16,
    pub driver_version: u32,
}

impl CacheFileHeader {
    pub fn new(entry_count: u32, total_size: u64, vendor: u16, device: u16, driver: u32) -> Self {
        Self {
            magic: CACHE_MAGIC,
            version: CACHE_VERSION,
            entry_count,
            total_size,
            gpu_vendor_id: vendor,
            gpu_device_id: device,
            driver_version: driver,
        }
    }

    pub fn validate(&self) -> bool {
        self.magic == CACHE_MAGIC && self.version == CACHE_VERSION
    }
}
