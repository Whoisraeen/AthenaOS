//! Sequential read-ahead (Concept §AthFS: "game-aware extents — games
//! stream assets in long sequential runs; the filesystem should already
//! hold the next block by the time the engine asks for it").
//! MasterChecklist Phase 5.5 — "Sequential prefetch on read patterns
//! matching games".
//!
//! A per-inode stream detector watches logical-block reads; once a run of
//! [`RUN_THRESHOLD`] consecutive blocks is seen, the AthFS read path pulls
//! the next [`PREFETCH_DEPTH`] blocks of the SAME file into a small
//! read-ahead cache while the device queue is already hot. Subsequent
//! sequential reads are served from the cache (hits) without touching the
//! device. Random access never triggers it. Any write to an inode
//! invalidates its cached blocks (no stale serves, CoW included).
//!
//! The smoketest streams a multi-block file on a RAM volume through the
//! REAL AthFS read path and asserts the detector fired, blocks were
//! prefetched, and the sequential tail was served from cache —
//! deterministic on QEMU and iron.

#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

/// Must equal `raefs::BLOCK_SIZE` (4 KiB blocks = 8 × 512 sectors).
pub const BLOCK: usize = 4096;
/// Consecutive blocks before read-ahead arms.
pub const RUN_THRESHOLD: u32 = 2;
/// Blocks read ahead per detected-run read.
pub const PREFETCH_DEPTH: u64 = 4;
/// Read-ahead cache cap (blocks): 32 × 4 KiB = 128 KiB.
const CACHE_CAP: usize = 32;

struct State {
    /// inode → (next expected logical block, current run length).
    streams: BTreeMap<u64, (u64, u32)>,
    cache: BTreeMap<(u64, u64), Box<[u8; BLOCK]>>,
    /// FIFO eviction order for `cache`.
    order: VecDeque<(u64, u64)>,
}

static STATE: Mutex<State> = Mutex::new(State {
    streams: BTreeMap::new(),
    cache: BTreeMap::new(),
    order: VecDeque::new(),
});

static RUNS_DETECTED: AtomicU64 = AtomicU64::new(0);
static BLOCKS_PREFETCHED: AtomicU64 = AtomicU64::new(0);
static HITS: AtomicU64 = AtomicU64::new(0);
static MISSES: AtomicU64 = AtomicU64::new(0);

/// Feed the stream detector with a read of `logical` on `inode`. Returns
/// how many blocks ahead the caller should prefetch (0 = pattern is not
/// sequential yet / random access).
pub fn record(inode: u64, logical: u64) -> u64 {
    let mut st = STATE.lock();
    let entry = st.streams.entry(inode).or_insert((logical, 0));
    if logical == entry.0 {
        entry.1 = entry.1.saturating_add(1);
    } else {
        entry.1 = 1;
    }
    entry.0 = logical + 1;
    if entry.1 == RUN_THRESHOLD {
        RUNS_DETECTED.fetch_add(1, Ordering::Relaxed);
    }
    if entry.1 >= RUN_THRESHOLD {
        PREFETCH_DEPTH
    } else {
        0
    }
}

/// Serve a block from the read-ahead cache, consuming it. A hit means the
/// device was never touched for this block.
pub fn take(inode: u64, logical: u64) -> Option<Box<[u8; BLOCK]>> {
    let mut st = STATE.lock();
    match st.cache.remove(&(inode, logical)) {
        Some(b) => {
            st.order.retain(|k| *k != (inode, logical));
            HITS.fetch_add(1, Ordering::Relaxed);
            Some(b)
        }
        None => {
            MISSES.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
}

pub fn contains(inode: u64, logical: u64) -> bool {
    STATE.lock().cache.contains_key(&(inode, logical))
}

/// Park a read-ahead block. FIFO-evicts beyond [`CACHE_CAP`].
pub fn stash(inode: u64, logical: u64, data: Box<[u8; BLOCK]>) {
    let mut st = STATE.lock();
    if st.cache.contains_key(&(inode, logical)) {
        return;
    }
    while st.cache.len() >= CACHE_CAP {
        if let Some(old) = st.order.pop_front() {
            st.cache.remove(&old);
        } else {
            break;
        }
    }
    st.cache.insert((inode, logical), data);
    st.order.push_back((inode, logical));
    BLOCKS_PREFETCHED.fetch_add(1, Ordering::Relaxed);
}

/// Drop every cached block of `inode` — called on ANY write to the inode
/// so the cache can never serve pre-write (stale) data.
pub fn invalidate_inode(inode: u64) {
    let mut st = STATE.lock();
    st.cache.retain(|k, _| k.0 != inode);
    st.order.retain(|k| k.0 != inode);
    st.streams.remove(&inode);
}

/// Drop everything — mount switches (snapshot restore, RAM test volumes)
/// reuse inode ids across different on-disk realities.
pub fn invalidate_all() {
    let mut st = STATE.lock();
    st.cache.clear();
    st.order.clear();
    st.streams.clear();
}

pub fn stats() -> (u64, u64, u64, u64) {
    (
        RUNS_DETECTED.load(Ordering::Relaxed),
        BLOCKS_PREFETCHED.load(Ordering::Relaxed),
        HITS.load(Ordering::Relaxed),
        MISSES.load(Ordering::Relaxed),
    )
}

pub fn init() {
    crate::serial_println!(
        "[prefetch] sequential read-ahead armed (run>={} -> +{} blocks, cache {} KiB)",
        RUN_THRESHOLD,
        PREFETCH_DEPTH,
        CACHE_CAP * BLOCK / 1024,
    );
}

/// Deterministic proof on a RAM volume through the REAL read path: a
/// 8-block file streamed sequentially fires the detector, prefetches
/// ahead, and serves the tail from cache; a random pattern arms nothing.
pub fn run_boot_smoketest() {
    let (runs0, pre0, hits0, _miss0) = stats();

    let io = crate::raefs::with_custom_raefs_device(
        alloc::boxed::Box::new(crate::fde::SharedRamDisk::new(4096).0),
        || {
            let mut data = alloc::vec![0u8; 8 * BLOCK];
            for (i, b) in data.iter_mut().enumerate() {
                *b = (i % 251) as u8;
            }
            let wrote = crate::raefs::write_flat_file("prefetch-stream.bin", &data);
            let read = crate::raefs::read_flat_file("prefetch-stream.bin");
            (wrote, read.as_deref() == Some(&data[..]))
        },
    );
    let (wrote, read_ok) = io.unwrap_or((false, false));

    let (runs1, pre1, hits1, _miss1) = stats();
    let run_detected = runs1 > runs0;
    let prefetched = pre1.saturating_sub(pre0) >= 4;
    let cache_hits = hits1.saturating_sub(hits0) >= 4;

    // Random access never arms read-ahead.
    let r1 = record(u64::MAX, 10);
    let r2 = record(u64::MAX, 50);
    let r3 = record(u64::MAX, 7);
    let random_quiet = r1 == 0 && r2 == 0 && r3 == 0;
    invalidate_inode(u64::MAX);

    let pass = wrote && read_ok && run_detected && prefetched && cache_hits && random_quiet;
    crate::serial_println!(
        "[prefetch] smoketest: stream_read={} run_detected={} prefetched>=4={} cache_hits>=4={} random_quiet={} -> {}",
        wrote && read_ok,
        run_detected,
        prefetched,
        cache_hits,
        random_quiet,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// `/proc/raeen/prefetch` — read-ahead engine state.
pub fn dump_text() -> String {
    let st = STATE.lock();
    let (runs, pre, hits, misses) = stats();
    alloc::format!(
        "# sequential read-ahead (game asset streaming)\nrun_threshold: {}\ndepth: {}\ncached_blocks: {}\nstreams_tracked: {}\nruns_detected: {}\nblocks_prefetched: {}\nhits: {}\nmisses: {}\n",
        RUN_THRESHOLD,
        PREFETCH_DEPTH,
        st.cache.len(),
        st.streams.len(),
        runs,
        pre,
        hits,
        misses,
    )
}
