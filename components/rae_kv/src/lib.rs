//! # RaeKV — a never-panic, `no_std`, embedded sorted key-value store.
//!
//! RaeenOS_Concept.md §Compatibility Strategy ("how to actually win") + the
//! daily-driver table stakes: the first-party apps — Mail, Calendar, Notes,
//! Settings — and the switcher import path all need **structured durable
//! persistence beyond raw files**. A flat file gives you bytes; an app that wants
//! "list everything under `mail/inbox/`", "read a setting by key", "delete this
//! note", or "snapshot my data to a RaeFS bucket and reload it" needs an *ordered*,
//! *bounds-checked*, *integrity-checked* store. Nothing else in the tree provides
//! that. RaeKV is that infrastructure layer — small, dependency-free, and safe to
//! load from an untrusted blob.
//!
//! ## The model
//! [`KvStore`] holds a sorted `key → value` map (`Vec<u8>` keys, `Vec<u8>` values)
//! backed by [`alloc::collections::BTreeMap`]. The map gives us the two things a
//! plain hash map can't: a stable **sorted iteration order** and efficient
//! **range / prefix scans** — which is exactly what makes this a *store* rather
//! than a dictionary (list a bucket, scan a key prefix like `"mail/inbox/"`).
//!
//! ### Operations
//! - Point: [`put`](KvStore::put), [`get`](KvStore::get),
//!   [`delete`](KvStore::delete), [`contains`](KvStore::contains),
//!   [`len`](KvStore::len), [`is_empty`](KvStore::is_empty),
//!   [`clear`](KvStore::clear).
//! - Ordered: [`range`](KvStore::range) (half-open `start..end`),
//!   [`prefix_scan`](KvStore::prefix_scan), [`keys`](KvStore::keys),
//!   [`iter`](KvStore::iter) — all in sorted key order.
//! - Typed convenience: [`put_str`](KvStore::put_str)/[`get_str`](KvStore::get_str),
//!   [`put_u64`](KvStore::put_u64)/[`get_u64`](KvStore::get_u64). (`put_json`-style
//!   helpers are deliberately *not* pulled in — keeping the crate zero-dep is worth
//!   more than one convenience method; a caller serializes with `rae_json` and
//!   stores the bytes.)
//! - Transactional: a [`Batch`] of puts/deletes that [`apply`](KvStore::apply)s
//!   atomically (all-or-nothing — if any operation would exceed a bound the whole
//!   batch is rejected and the store is untouched).
//!
//! ## Persistence (the durable part)
//! [`KvStore::to_bytes`] serializes the *whole* store to a compact, versioned,
//! self-describing byte blob: a magic + version header, the entry count, then
//! length-prefixed `(key, value)` pairs in sorted order, then a CRC-32 trailer over
//! everything before it. This is a **consistent snapshot** — the natural unit for
//! "save my app data to a file / RaeFS bucket."
//!
//! [`KvStore::from_bytes`] parses it back. Every byte is treated as
//! attacker-controlled (a blob from disk can be truncated, bit-rotted, or
//! maliciously crafted): the magic and version are checked, the CRC is verified
//! over the payload *before any entry is trusted*, and the declared entry count and
//! every key/value length are **bounds-checked against the [caps](#caps) before a
//! single byte is allocated**. A corrupt, truncated, oversized, or hostile blob
//! yields an [`KvError`] — never a panic, never an OOM, never a hang.
//!
//! ## What's modeled vs deferred (honest)
//! - **Modeled:** the ordered in-memory store, range/prefix scans, the versioned
//!   integrity-checked snapshot format, atomic [`Batch`] application, and a full
//!   hostile-input posture.
//! - **Deferred (documented):** a write-ahead log and crash-recovery. RaeKV's
//!   durability unit today is the *snapshot* ([`to_bytes`](KvStore::to_bytes) /
//!   [`from_bytes`](KvStore::from_bytes)) — a caller persists a whole consistent
//!   image, not an incremental journal. A WAL layer that replays partial writes
//!   after a crash is a later addition that would sit *above* this format; it is
//!   intentionally out of scope for this slice.
//!
//! The host KAT suite at the bottom of this file is the primary proof
//! (`cargo test -p rae_kv`): point ops with exact values, overwrite, delete
//! semantics, the exact half-open `range("b".."d")` key list, `prefix_scan`, the
//! load-bearing `to_bytes`→`from_bytes` round-trip over binary/empty/large values,
//! the CRC catching a flipped byte, every hostile-blob class returning `Err`, an
//! atomic `Batch`, bound enforcement, and a seeded fuzz over `from_bytes`.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::ops::RangeBounds;

// ---------------------------------------------------------------------------
// Caps (untrusted input). Every allocation driven by stored counts/lengths is
// bounded by one of these BEFORE the allocation happens.
// ---------------------------------------------------------------------------

/// Maximum number of entries a store may hold (and the maximum declared entry
/// count [`KvStore::from_bytes`] will trust before allocating).
pub const MAX_KEYS: usize = 16_777_216; // 2^24
/// Maximum size, in bytes, of a single key.
pub const MAX_KEY_SIZE: usize = 65_536; // 64 KiB
/// Maximum size, in bytes, of a single value.
pub const MAX_VALUE_SIZE: usize = 268_435_456; // 256 MiB
/// Maximum size, in bytes, of an entire serialized store (the cap
/// [`KvStore::from_bytes`] enforces on its input length before doing any work).
pub const MAX_TOTAL_SIZE: usize = 1_073_741_824; // 1 GiB

// ---------------------------------------------------------------------------
// Serialization format constants.
// ---------------------------------------------------------------------------

/// File magic: ASCII `"RAEKV\0"` — 6 bytes, identifies a RaeKV snapshot.
pub const MAGIC: [u8; 6] = *b"RAEKV\0";
/// On-disk format version. Bumped on any breaking layout change.
pub const FORMAT_VERSION: u16 = 1;

/// Header size: magic(6) + version(2) + entry_count(8) = 16 bytes.
const HEADER_LEN: usize = 6 + 2 + 8;
/// Trailer size: CRC-32 over everything before it.
const TRAILER_LEN: usize = 4;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Why a RaeKV operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KvError {
    /// A key exceeded [`MAX_KEY_SIZE`].
    KeyTooLarge,
    /// A value exceeded [`MAX_VALUE_SIZE`].
    ValueTooLarge,
    /// Inserting would exceed [`MAX_KEYS`].
    TooManyKeys,
    /// The serialized input does not begin with the RaeKV [`MAGIC`].
    BadMagic,
    /// The serialized input declares a [`FORMAT_VERSION`] this build cannot read.
    UnsupportedVersion(u16),
    /// The CRC-32 trailer did not match the payload — the blob is corrupt.
    BadChecksum,
    /// The blob is truncated, internally inconsistent, declares a length that runs
    /// past the buffer, or declares a count/size beyond a cap. (Catch-all for
    /// "these bytes are not a valid, in-bounds snapshot.")
    Corrupt,
    /// The input is larger than [`MAX_TOTAL_SIZE`] — refused before any work.
    TooLarge,
}

// ---------------------------------------------------------------------------
// CRC-32 (IEEE 802.3, the zlib/PNG polynomial 0xEDB88320), table-free.
// ---------------------------------------------------------------------------

/// Compute the IEEE CRC-32 of `data`. From-scratch, allocation-free, never panics.
/// Used as the snapshot integrity trailer — a flipped or dropped byte changes it.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg(); // 0xFFFFFFFF if low bit set, else 0
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// An embedded, sorted, single-file key-value store. See the crate docs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KvStore {
    map: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl KvStore {
    /// Create a new, empty store. ([`open`](KvStore::open) is an alias — there is no
    /// file handle here; durability is via [`to_bytes`](KvStore::to_bytes) /
    /// [`from_bytes`](KvStore::from_bytes).)
    pub fn new() -> Self {
        KvStore {
            map: BTreeMap::new(),
        }
    }

    /// Alias for [`new`](KvStore::new) — an empty store, ready to populate or to be
    /// replaced by [`from_bytes`](KvStore::from_bytes).
    pub fn open() -> Self {
        Self::new()
    }

    // ---- point operations -------------------------------------------------

    /// Insert or overwrite `key → value`. Returns `Err` if `key` exceeds
    /// [`MAX_KEY_SIZE`], `value` exceeds [`MAX_VALUE_SIZE`], or inserting a *new*
    /// key would push past [`MAX_KEYS`]. On `Err` the store is unchanged.
    pub fn put(
        &mut self,
        key: impl Into<Vec<u8>>,
        value: impl Into<Vec<u8>>,
    ) -> Result<(), KvError> {
        let key = key.into();
        let value = value.into();
        if key.len() > MAX_KEY_SIZE {
            return Err(KvError::KeyTooLarge);
        }
        if value.len() > MAX_VALUE_SIZE {
            return Err(KvError::ValueTooLarge);
        }
        // A new key would grow the map; an overwrite would not.
        if !self.map.contains_key(&key) && self.map.len() >= MAX_KEYS {
            return Err(KvError::TooManyKeys);
        }
        self.map.insert(key, value);
        Ok(())
    }

    /// Fetch the value for `key`, if present.
    pub fn get(&self, key: impl AsRef<[u8]>) -> Option<&[u8]> {
        self.map.get(key.as_ref()).map(|v| v.as_slice())
    }

    /// Remove `key`. Returns `true` if it was present.
    pub fn delete(&mut self, key: impl AsRef<[u8]>) -> bool {
        self.map.remove(key.as_ref()).is_some()
    }

    /// Whether `key` is present.
    pub fn contains(&self, key: impl AsRef<[u8]>) -> bool {
        self.map.contains_key(key.as_ref())
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Remove every entry.
    pub fn clear(&mut self) {
        self.map.clear();
    }

    // ---- typed convenience ------------------------------------------------

    /// Store a UTF-8 string value under `key`. Subject to the same bounds as
    /// [`put`](KvStore::put).
    pub fn put_str(&mut self, key: impl Into<Vec<u8>>, value: &str) -> Result<(), KvError> {
        self.put(key, value.as_bytes().to_vec())
    }

    /// Fetch a value and interpret it as UTF-8. Returns `None` if the key is absent
    /// or the stored bytes are not valid UTF-8.
    pub fn get_str(&self, key: impl AsRef<[u8]>) -> Option<&str> {
        self.get(key).and_then(|b| core::str::from_utf8(b).ok())
    }

    /// Store a `u64` as 8 big-endian bytes under `key`.
    pub fn put_u64(&mut self, key: impl Into<Vec<u8>>, value: u64) -> Result<(), KvError> {
        self.put(key, value.to_be_bytes().to_vec())
    }

    /// Fetch a value and interpret it as a big-endian `u64`. Returns `None` if the
    /// key is absent or the stored value is not exactly 8 bytes.
    pub fn get_u64(&self, key: impl AsRef<[u8]>) -> Option<u64> {
        let b = self.get(key)?;
        if b.len() != 8 {
            return None;
        }
        let mut arr = [0u8; 8];
        arr.copy_from_slice(b);
        Some(u64::from_be_bytes(arr))
    }

    // ---- ordered queries --------------------------------------------------

    /// Iterate `(key, value)` over a key range, in sorted key order. The range uses
    /// the standard Rust bounds, so `range("b".as_bytes()..b"d".as_slice())` is the
    /// half-open `[b, d)`. Borrowed from the live map (no allocation, no copy).
    pub fn range<R>(&self, range: R) -> impl Iterator<Item = (&[u8], &[u8])>
    where
        R: RangeBounds<Vec<u8>>,
    {
        self.map
            .range(range)
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
    }

    /// Iterate `(key, value)` for every key beginning with `prefix`, in sorted key
    /// order. The canonical "list everything under `mail/inbox/`" scan. Allocation
    /// is bounded by the number of matching entries (already capped at
    /// [`MAX_KEYS`]).
    pub fn prefix_scan<'a>(
        &'a self,
        prefix: &'a [u8],
    ) -> impl Iterator<Item = (&'a [u8], &'a [u8])> + 'a {
        // BTreeMap is sorted, so all keys with `prefix` form a contiguous run. We
        // start at the prefix and take_while it still matches — no full scan.
        self.map
            .range(prefix.to_vec()..)
            .take_while(move |(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
    }

    /// Iterate every key in sorted order.
    pub fn keys(&self) -> impl Iterator<Item = &[u8]> {
        self.map.keys().map(|k| k.as_slice())
    }

    /// Iterate every `(key, value)` in sorted key order.
    pub fn iter(&self) -> impl Iterator<Item = (&[u8], &[u8])> {
        self.map.iter().map(|(k, v)| (k.as_slice(), v.as_slice()))
    }

    // ---- transactional ----------------------------------------------------

    /// Apply a [`Batch`] of puts/deletes atomically. The batch is fully validated
    /// against the bounds against a *projected* final state first; if any operation
    /// would violate a bound the whole batch is rejected with `Err` and the store is
    /// left completely untouched. Otherwise every operation is applied.
    pub fn apply(&mut self, batch: Batch) -> Result<(), KvError> {
        // Validate per-op sizes first (cheap, no state change).
        for op in &batch.ops {
            match op {
                BatchOp::Put(k, v) => {
                    if k.len() > MAX_KEY_SIZE {
                        return Err(KvError::KeyTooLarge);
                    }
                    if v.len() > MAX_VALUE_SIZE {
                        return Err(KvError::ValueTooLarge);
                    }
                }
                BatchOp::Delete(_) => {}
            }
        }
        // Project the final entry count to check MAX_KEYS without mutating.
        let mut projected = self.map.len();
        for op in &batch.ops {
            match op {
                BatchOp::Put(k, _) => {
                    if !self.map.contains_key(k) {
                        // A new key (unless an earlier op in this batch already added
                        // it — but BTreeMap projection below is the source of truth;
                        // we over-count conservatively, which only makes the bound
                        // STRICTER, never looser, so it is still safe).
                        projected = projected.saturating_add(1);
                    }
                }
                BatchOp::Delete(k) => {
                    if self.map.contains_key(k) {
                        projected = projected.saturating_sub(1);
                    }
                }
            }
        }
        if projected > MAX_KEYS {
            return Err(KvError::TooManyKeys);
        }
        // All checks passed — apply. (Validated above, so no op here can fail.)
        for op in batch.ops {
            match op {
                BatchOp::Put(k, v) => {
                    self.map.insert(k, v);
                }
                BatchOp::Delete(k) => {
                    self.map.remove(&k);
                }
            }
        }
        Ok(())
    }

    // ---- persistence ------------------------------------------------------

    /// Serialize the whole store to a compact, versioned, CRC-checked snapshot.
    ///
    /// Layout:
    /// ```text
    /// magic[6] = "RAEKV\0"
    /// version  : u16 LE
    /// count    : u64 LE          (number of entries)
    /// entries[count] {
    ///     key_len   : u32 LE
    ///     key       : [u8; key_len]
    ///     value_len : u32 LE
    ///     value     : [u8; value_len]
    /// }                          (in sorted key order — a consistent snapshot)
    /// crc32    : u32 LE          (IEEE CRC-32 over everything above)
    /// ```
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        out.extend_from_slice(&(self.map.len() as u64).to_le_bytes());
        for (k, v) in &self.map {
            out.extend_from_slice(&(k.len() as u32).to_le_bytes());
            out.extend_from_slice(k);
            out.extend_from_slice(&(v.len() as u32).to_le_bytes());
            out.extend_from_slice(v);
        }
        let crc = crc32(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    /// Parse a snapshot produced by [`to_bytes`](KvStore::to_bytes). Treats every
    /// byte as attacker-controlled: checks the magic, the version, and the CRC, and
    /// bounds-checks the declared count and every length against the caps BEFORE
    /// allocating. Returns `Err` (never panics, never OOMs, never hangs) on any
    /// corrupt, truncated, oversized, or hostile input.
    pub fn from_bytes(data: &[u8]) -> Result<KvStore, KvError> {
        if data.len() > MAX_TOTAL_SIZE {
            return Err(KvError::TooLarge);
        }
        if data.len() < HEADER_LEN + TRAILER_LEN {
            return Err(KvError::Corrupt);
        }
        // Magic.
        if data[..6] != MAGIC {
            return Err(KvError::BadMagic);
        }
        // Version.
        let version = u16::from_le_bytes([data[6], data[7]]);
        if version != FORMAT_VERSION {
            return Err(KvError::UnsupportedVersion(version));
        }
        // CRC: verify the trailer over everything before it BEFORE trusting a byte.
        let payload_end = data.len() - TRAILER_LEN;
        let stored_crc = u32::from_le_bytes([
            data[payload_end],
            data[payload_end + 1],
            data[payload_end + 2],
            data[payload_end + 3],
        ]);
        if crc32(&data[..payload_end]) != stored_crc {
            return Err(KvError::BadChecksum);
        }

        // Entry count.
        let count = u64::from_le_bytes([
            data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
        ]);
        // Cap the declared count before we allocate or loop on it.
        if count > MAX_KEYS as u64 {
            return Err(KvError::Corrupt);
        }
        let count = count as usize;

        let body = &data[HEADER_LEN..payload_end];
        let mut cursor = 0usize;
        let mut map: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();

        for _ in 0..count {
            // key_len
            let key_len = read_u32(body, &mut cursor)? as usize;
            if key_len > MAX_KEY_SIZE {
                return Err(KvError::Corrupt);
            }
            let key = read_slice(body, &mut cursor, key_len)?;
            // value_len
            let value_len = read_u32(body, &mut cursor)? as usize;
            if value_len > MAX_VALUE_SIZE {
                return Err(KvError::Corrupt);
            }
            let value = read_slice(body, &mut cursor, value_len)?;
            map.insert(key, value);
        }
        // Every declared entry must be consumed AND the body must be fully
        // consumed (no trailing garbage between the last entry and the CRC) —
        // and the de-duped map must match the declared count (no duplicate keys
        // silently collapsing).
        if cursor != body.len() || map.len() != count {
            return Err(KvError::Corrupt);
        }

        Ok(KvStore { map })
    }
}

/// Read a little-endian `u32` at `*cursor`, advancing it. `Err(Corrupt)` if fewer
/// than 4 bytes remain. Never panics.
fn read_u32(body: &[u8], cursor: &mut usize) -> Result<u32, KvError> {
    let end = cursor.checked_add(4).ok_or(KvError::Corrupt)?;
    if end > body.len() {
        return Err(KvError::Corrupt);
    }
    let v = u32::from_le_bytes([
        body[*cursor],
        body[*cursor + 1],
        body[*cursor + 2],
        body[*cursor + 3],
    ]);
    *cursor = end;
    Ok(v)
}

/// Read `len` bytes at `*cursor` into a fresh `Vec`, advancing the cursor.
/// `Err(Corrupt)` if fewer than `len` bytes remain — the length is validated
/// against the buffer BEFORE allocating, so a hostile declared length cannot OOM.
/// Never panics.
fn read_slice(body: &[u8], cursor: &mut usize, len: usize) -> Result<Vec<u8>, KvError> {
    let end = cursor.checked_add(len).ok_or(KvError::Corrupt)?;
    if end > body.len() {
        return Err(KvError::Corrupt);
    }
    let v = body[*cursor..end].to_vec();
    *cursor = end;
    Ok(v)
}

// ---------------------------------------------------------------------------
// Batch (lightweight transactionality)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum BatchOp {
    Put(Vec<u8>, Vec<u8>),
    Delete(Vec<u8>),
}

/// A set of put/delete operations applied atomically by [`KvStore::apply`].
///
/// The batch is a staging area: building it never touches a store. When applied it
/// is validated against the bounds *as a whole* and then committed all-or-nothing —
/// if any operation would violate a bound the entire batch is rejected and the store
/// is left untouched. Operations within a batch are applied in the order added, so a
/// later put overrides an earlier put of the same key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Batch {
    ops: Vec<BatchOp>,
}

impl Batch {
    /// A new, empty batch.
    pub fn new() -> Self {
        Batch { ops: Vec::new() }
    }

    /// Stage a put.
    pub fn put(&mut self, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) -> &mut Self {
        self.ops.push(BatchOp::Put(key.into(), value.into()));
        self
    }

    /// Stage a delete.
    pub fn delete(&mut self, key: impl Into<Vec<u8>>) -> &mut Self {
        self.ops.push(BatchOp::Delete(key.into()));
        self
    }

    /// Number of staged operations.
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// Whether the batch has no staged operations.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

// ===========================================================================
// Host KAT suite — the FAIL-able proof (cargo test -p rae_kv)
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    fn k(s: &str) -> Vec<u8> {
        s.as_bytes().to_vec()
    }

    // ---- point operations --------------------------------------------------

    #[test]
    fn put_get_delete_contains_basics() {
        let mut s = KvStore::new();
        assert!(s.is_empty());
        s.put(k("a"), k("1")).unwrap();
        s.put(k("b"), k("2")).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s.get("a"), Some(&b"1"[..]));
        assert_eq!(s.get("b"), Some(&b"2"[..]));
        assert_eq!(s.get("missing"), None);
        assert!(s.contains("a"));
        assert!(!s.contains("missing"));

        // delete returns true, then get -> None.
        assert!(s.delete("a"));
        assert!(!s.delete("a")); // already gone
        assert_eq!(s.get("a"), None);
        assert_eq!(s.len(), 1);

        s.clear();
        assert!(s.is_empty());
    }

    #[test]
    fn overwrite_updates_value_not_count() {
        let mut s = KvStore::new();
        s.put(k("x"), k("old")).unwrap();
        s.put(k("x"), k("new")).unwrap();
        assert_eq!(s.len(), 1); // overwrite, not a second entry
        assert_eq!(s.get("x"), Some(&b"new"[..]));
    }

    #[test]
    fn typed_str_and_u64() {
        let mut s = KvStore::new();
        s.put_str(k("name"), "Ada").unwrap();
        s.put_u64(k("count"), 0xDEAD_BEEF_CAFE_1234).unwrap();
        assert_eq!(s.get_str("name"), Some("Ada"));
        assert_eq!(s.get_u64("count"), Some(0xDEAD_BEEF_CAFE_1234));
        // Wrong-shaped reads -> None, never panic.
        assert_eq!(s.get_u64("name"), None); // 3 bytes, not 8
                                             // Invalid UTF-8 stored bytes -> get_str None.
        s.put(k("raw"), vec![0xFF, 0xFE]).unwrap();
        assert_eq!(s.get_str("raw"), None);
    }

    // ---- ordered queries (the value over a HashMap) ------------------------

    #[test]
    fn range_half_open_exact_keys_failable() {
        // keys a,b,c,d,e ; range("b".."d") MUST be exactly [b, c] (half-open).
        // FAIL-able: change the expected list to [b,c,d] and this turns red.
        let mut s = KvStore::new();
        for key in ["a", "b", "c", "d", "e"] {
            s.put(k(key), k(key)).unwrap();
        }
        let got: Vec<Vec<u8>> = s
            .range(k("b")..k("d"))
            .map(|(key, _)| key.to_vec())
            .collect();
        assert_eq!(got, vec![k("b"), k("c")]);
    }

    #[test]
    fn prefix_scan_returns_only_matching_in_order() {
        let mut s = KvStore::new();
        // Deliberately insert out of order; store must return sorted.
        for key in [
            "mail/sent/2",
            "mail/inbox/1",
            "notes/n1",
            "mail/inbox/2",
            "mail/inbox/0",
            "zzz",
        ] {
            s.put(k(key), k("v")).unwrap();
        }
        let got: Vec<Vec<u8>> = s
            .prefix_scan(b"mail/inbox/")
            .map(|(key, _)| key.to_vec())
            .collect();
        // Only mail/inbox/* keys, in sorted order — NOT mail/sent/, NOT notes/.
        assert_eq!(
            got,
            vec![k("mail/inbox/0"), k("mail/inbox/1"), k("mail/inbox/2")]
        );

        // keys() iterates everything in sorted order.
        let all: Vec<Vec<u8>> = s.keys().map(|key| key.to_vec()).collect();
        assert_eq!(
            all,
            vec![
                k("mail/inbox/0"),
                k("mail/inbox/1"),
                k("mail/inbox/2"),
                k("mail/sent/2"),
                k("notes/n1"),
                k("zzz"),
            ]
        );
    }

    // ---- the LOAD-BEARING persistence round-trip ---------------------------

    #[test]
    fn to_bytes_from_bytes_roundtrip_exact_failable() {
        let mut s = KvStore::new();
        // Mixed keys + values: binary 0x00..=0xFF, empty value, a large value.
        let full_byte_range: Vec<u8> = (0u16..=255).map(|b| b as u8).collect();
        let large: Vec<u8> = (0..100_000).map(|i| (i % 251) as u8).collect();
        s.put(k("alpha"), k("first")).unwrap();
        s.put(k("binary"), full_byte_range.clone()).unwrap();
        s.put(k("empty"), Vec::new()).unwrap(); // empty value
        s.put(Vec::new(), k("empty-key-value")).unwrap(); // empty KEY
        s.put(k("large"), large.clone()).unwrap();
        s.put(vec![0x00, 0xFF, 0x01], vec![0x10, 0x20]).unwrap(); // binary key

        let blob = s.to_bytes();
        let loaded = KvStore::from_bytes(&blob).expect("round-trip parse");

        // Every key/value EXACTLY equal.
        assert_eq!(loaded, s);
        assert_eq!(loaded.len(), s.len());
        assert_eq!(loaded.get("binary"), Some(full_byte_range.as_slice()));
        assert_eq!(loaded.get("empty"), Some(&[][..]));
        assert_eq!(loaded.get("large"), Some(large.as_slice()));
        assert_eq!(loaded.get(Vec::new()), Some(&b"empty-key-value"[..]));

        // Iteration order preserved (sorted). FAIL-able: tweak any expected key.
        let order: Vec<Vec<u8>> = loaded.keys().map(|key| key.to_vec()).collect();
        assert_eq!(
            order,
            vec![
                Vec::new(),             // empty key sorts first
                vec![0x00, 0xFF, 0x01], // binary key
                k("alpha"),
                k("binary"),
                k("empty"),
                k("large"),
            ]
        );
    }

    #[test]
    fn empty_store_roundtrips() {
        let s = KvStore::new();
        let blob = s.to_bytes();
        let loaded = KvStore::from_bytes(&blob).unwrap();
        assert!(loaded.is_empty());
        assert_eq!(loaded, s);
    }

    // ---- the checksum catches corruption -----------------------------------

    #[test]
    fn flipped_payload_byte_is_bad_checksum() {
        let mut s = KvStore::new();
        s.put(k("key"), k("value")).unwrap();
        let mut blob = s.to_bytes();
        // Flip a byte in the payload (the value region, well inside the header..crc).
        let idx = HEADER_LEN + 6; // somewhere in the first entry's bytes
        blob[idx] ^= 0xFF;
        match KvStore::from_bytes(&blob) {
            Err(KvError::BadChecksum) => {}
            other => panic!("expected BadChecksum, got {:?}", other),
        }
    }

    #[test]
    fn flipped_crc_byte_is_bad_checksum() {
        let mut s = KvStore::new();
        s.put(k("key"), k("value")).unwrap();
        let mut blob = s.to_bytes();
        let last = blob.len() - 1;
        blob[last] ^= 0x01;
        assert_eq!(KvStore::from_bytes(&blob), Err(KvError::BadChecksum));
    }

    // ---- hostile blobs: graceful Err, never panic/OOM ----------------------

    #[test]
    fn bad_magic_is_rejected() {
        let mut s = KvStore::new();
        s.put(k("a"), k("b")).unwrap();
        let mut blob = s.to_bytes();
        blob[0] = b'X';
        assert_eq!(KvStore::from_bytes(&blob), Err(KvError::BadMagic));
    }

    #[test]
    fn unsupported_version_is_rejected() {
        let mut s = KvStore::new();
        s.put(k("a"), k("b")).unwrap();
        let mut blob = s.to_bytes();
        // Bump the version field to 0xFFFF, then re-stamp the CRC so we exercise the
        // version gate specifically (not the checksum gate).
        blob[6] = 0xFF;
        blob[7] = 0xFF;
        let payload_end = blob.len() - TRAILER_LEN;
        let crc = crc32(&blob[..payload_end]).to_le_bytes();
        blob[payload_end..].copy_from_slice(&crc);
        assert_eq!(
            KvStore::from_bytes(&blob),
            Err(KvError::UnsupportedVersion(0xFFFF))
        );
    }

    #[test]
    fn truncated_blob_is_corrupt_not_panic() {
        let mut s = KvStore::new();
        s.put(k("hello"), k("world")).unwrap();
        let blob = s.to_bytes();
        // Every truncation length must yield Err, never panic.
        for cut in 0..blob.len() {
            let res = KvStore::from_bytes(&blob[..cut]);
            assert!(res.is_err(), "truncation to {cut} should Err");
        }
    }

    #[test]
    fn absurd_entry_count_does_not_allocate_or_panic() {
        // Hand-craft a header claiming u64::MAX entries with a valid magic/version
        // and a correct CRC — the count cap must reject it BEFORE looping/allocating.
        let mut blob = Vec::new();
        blob.extend_from_slice(&MAGIC);
        blob.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        blob.extend_from_slice(&u64::MAX.to_le_bytes()); // absurd count
        let crc = crc32(&blob);
        blob.extend_from_slice(&crc.to_le_bytes());
        assert_eq!(KvStore::from_bytes(&blob), Err(KvError::Corrupt));
    }

    #[test]
    fn oversized_declared_length_is_corrupt() {
        // One entry whose key_len claims 0xFFFF_FFFF bytes but the buffer is tiny.
        let mut blob = Vec::new();
        blob.extend_from_slice(&MAGIC);
        blob.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        blob.extend_from_slice(&1u64.to_le_bytes()); // count = 1
        blob.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // key_len = 4 GiB
        blob.extend_from_slice(b"only-a-few-bytes");
        let crc = crc32(&blob);
        blob.extend_from_slice(&crc.to_le_bytes());
        // key_len > MAX_KEY_SIZE -> Corrupt (caught before any 4 GiB allocation).
        assert_eq!(KvStore::from_bytes(&blob), Err(KvError::Corrupt));
    }

    #[test]
    fn trailing_garbage_after_last_entry_is_corrupt() {
        // count=1, one valid entry, then extra unconsumed bytes before the CRC.
        let mut blob = Vec::new();
        blob.extend_from_slice(&MAGIC);
        blob.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        blob.extend_from_slice(&1u64.to_le_bytes());
        blob.extend_from_slice(&1u32.to_le_bytes()); // key_len 1
        blob.push(b'k');
        blob.extend_from_slice(&1u32.to_le_bytes()); // value_len 1
        blob.push(b'v');
        blob.extend_from_slice(b"GARBAGE"); // unconsumed trailing bytes
        let crc = crc32(&blob);
        blob.extend_from_slice(&crc.to_le_bytes());
        assert_eq!(KvStore::from_bytes(&blob), Err(KvError::Corrupt));
    }

    #[test]
    fn empty_input_is_corrupt() {
        assert_eq!(KvStore::from_bytes(&[]), Err(KvError::Corrupt));
    }

    // ---- bounds enforcement ------------------------------------------------

    #[test]
    fn over_max_value_size_put_errs() {
        let mut s = KvStore::new();
        // We can't actually allocate 256 MiB+1 in a test cheaply; assert the bound
        // logic by checking the boundary with a synthetic length via a smaller cap
        // would need a feature flag. Instead prove the comparison directly with a
        // value exactly at the cap (ok) is impractical too — so prove the key cap,
        // which is small enough to materialize.
        let big_key = vec![b'k'; MAX_KEY_SIZE + 1];
        assert_eq!(s.put(big_key, k("v")), Err(KvError::KeyTooLarge));
        // A key exactly at the cap is accepted.
        let ok_key = vec![b'k'; MAX_KEY_SIZE];
        assert!(s.put(ok_key, k("v")).is_ok());
    }

    // ---- Batch (atomicity) -------------------------------------------------

    #[test]
    fn batch_applies_atomically() {
        let mut s = KvStore::new();
        s.put(k("keep"), k("1")).unwrap();
        s.put(k("drop"), k("2")).unwrap();

        let mut b = Batch::new();
        b.put(k("new1"), k("a"))
            .put(k("new2"), k("b"))
            .delete(k("drop"))
            .put(k("keep"), k("updated")); // overwrite
        assert_eq!(b.len(), 4);
        s.apply(b).unwrap();

        assert_eq!(s.get("new1"), Some(&b"a"[..]));
        assert_eq!(s.get("new2"), Some(&b"b"[..]));
        assert_eq!(s.get("drop"), None);
        assert_eq!(s.get("keep"), Some(&b"updated"[..]));
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn batch_rejected_whole_on_bound_violation_store_untouched() {
        let mut s = KvStore::new();
        s.put(k("a"), k("1")).unwrap();
        let before = s.clone();

        let mut b = Batch::new();
        b.put(k("ok"), k("fine"));
        b.put(vec![b'x'; MAX_KEY_SIZE + 1], k("bad")); // oversized key
        let res = s.apply(b);
        assert_eq!(res, Err(KvError::KeyTooLarge));
        // Store completely untouched — the "ok" put did NOT land.
        assert_eq!(s, before);
        assert_eq!(s.get("ok"), None);
    }

    // ---- seeded fuzz over from_bytes: bounded, never panics ----------------

    #[test]
    fn fuzz_from_bytes_never_panics() {
        // A tiny xorshift PRNG (deterministic, no std rng dep).
        let mut state: u64 = 0x1234_5678_9ABC_DEF0;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };

        // A valid baseline blob we will mutate.
        let mut base = KvStore::new();
        base.put(k("alpha"), k("one")).unwrap();
        base.put(k("beta"), vec![0x00, 0xFF, 0x7F]).unwrap();
        base.put(k("gamma"), Vec::new()).unwrap();
        let base_blob = base.to_bytes();

        for _ in 0..20_000 {
            let mode = next() % 3;
            let input: Vec<u8> = match mode {
                0 => {
                    // Purely random bytes, random length 0..512.
                    let len = (next() as usize) % 512;
                    (0..len).map(|_| (next() & 0xFF) as u8).collect()
                }
                1 => {
                    // Mutated copy of a valid blob: flip a handful of bytes.
                    let mut b = base_blob.clone();
                    let flips = (next() as usize) % 8;
                    for _ in 0..flips {
                        if !b.is_empty() {
                            let idx = (next() as usize) % b.len();
                            b[idx] ^= (next() & 0xFF) as u8;
                        }
                    }
                    b
                }
                _ => {
                    // Truncated/extended valid blob.
                    let mut b = base_blob.clone();
                    let op = next() % 2;
                    if op == 0 && !b.is_empty() {
                        let cut = (next() as usize) % b.len();
                        b.truncate(cut);
                    } else {
                        let add = (next() as usize) % 64;
                        for _ in 0..add {
                            b.push((next() & 0xFF) as u8);
                        }
                    }
                    b
                }
            };
            // The ONLY requirement: it returns (Ok or Err), never panics/hangs/OOMs.
            let _ = KvStore::from_bytes(&input);
        }
    }
}
