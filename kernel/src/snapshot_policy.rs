//! Snapshot retention + quota policy (Concept §RaeFS: "Time-machine snapshots
//! you never have to think about" — automatic thinning so snapshots stay
//! useful without eating the drive).
//!
//! MasterChecklist Phase 5.1: "Time-machine UX: hourly + daily + weekly
//! retention policy" and "Disk-quota for snapshots so they can't fill the
//! drive".
//!
//! The policy is the classic backup-thinning ladder over snapshot timestamps:
//!   * keep EVERY snapshot younger than one hour;
//!   * keep the newest snapshot of each hour for the last 24 hours;
//!   * keep the newest snapshot of each day for the last 7 days;
//!   * keep the newest snapshot of each week for the last 4 weeks;
//!   * everything older is deleted.
//! On top of that, a hard COUNT quota (default 12 of RaeFS's 16 slots) deletes
//! oldest-first so snapshot creation can never run the table full — the
//! remaining 4 slots stay free for explicit user snapshots.
//!
//! `enforce()` applies both rules to the live mount through the normal
//! `snapshot_delete` path (CoW blocks are reclaimed there). The smoketest is
//! deterministic: it runs the PURE policy over a synthetic timeline, so it
//! proves the algorithm on QEMU and iron identically.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const HOUR_MS: u64 = 3_600_000;
const DAY_MS: u64 = 24 * HOUR_MS;
const WEEK_MS: u64 = 7 * DAY_MS;

/// Hard cap applied by `enforce` (oldest deleted first beyond this).
const SNAPSHOT_COUNT_QUOTA: usize = 12;

static RETENTION_RUNS: AtomicU32 = AtomicU32::new(0);
static SNAPSHOTS_THINNED: AtomicU64 = AtomicU64::new(0);

/// Pure retention decision: given `(id, created_ms)` pairs and `now_ms`,
/// return the ids to DELETE under the hourly/daily/weekly ladder + count
/// quota. Newest-per-bucket wins; unbucketed (too old) snapshots go.
pub fn retention_victims(snapshots: &[(u32, u64)], now_ms: u64) -> Vec<u32> {
    let mut sorted: Vec<(u32, u64)> = snapshots.to_vec();
    // Newest first.
    sorted.sort_by(|a, b| b.1.cmp(&a.1));

    let mut keep: Vec<u32> = Vec::new();
    let mut hour_buckets: Vec<u64> = Vec::new();
    let mut day_buckets: Vec<u64> = Vec::new();
    let mut week_buckets: Vec<u64> = Vec::new();

    for &(id, ts) in &sorted {
        let age = now_ms.saturating_sub(ts);
        let keep_this = if age < HOUR_MS {
            true
        } else if age < DAY_MS {
            // One per hour bucket for the last 24h.
            let bucket = ts / HOUR_MS;
            if hour_buckets.contains(&bucket) {
                false
            } else {
                hour_buckets.push(bucket);
                true
            }
        } else if age < 7 * DAY_MS {
            let bucket = ts / DAY_MS;
            if day_buckets.contains(&bucket) {
                false
            } else {
                day_buckets.push(bucket);
                true
            }
        } else if age < 4 * WEEK_MS {
            let bucket = ts / WEEK_MS;
            if week_buckets.contains(&bucket) {
                false
            } else {
                week_buckets.push(bucket);
                true
            }
        } else {
            false
        };
        if keep_this {
            keep.push(id);
        }
    }

    // Count quota on the keepers: oldest beyond the cap go too.
    while keep.len() > SNAPSHOT_COUNT_QUOTA {
        // `keep` is in newest-first insertion order; pop the oldest.
        keep.pop();
    }

    sorted
        .iter()
        .map(|&(id, _)| id)
        .filter(|id| !keep.contains(id))
        .collect()
}

/// Apply the policy to the live mount: list snapshots, compute victims,
/// delete them through the normal CoW-reclaiming path. Returns the number
/// thinned. No-ops cleanly when nothing is mounted.
pub fn enforce() -> usize {
    let snapshots: Vec<(u32, u64)> = {
        let guard = crate::raefs::RAEFS.lock();
        match guard.as_ref() {
            Some(fs) => fs
                .list_snapshots()
                .iter()
                .map(|s| (s.id, s.timestamp))
                .collect(),
            None => return 0,
        }
    }; // RAEFS released before snapshot_delete re-locks it.

    let now_ms = crate::hpet::read_millis().unwrap_or(0) as u64;
    let victims = retention_victims(&snapshots, now_ms);
    let mut thinned = 0usize;
    for id in victims {
        if crate::raefs::snapshot_delete(id) == 0 {
            thinned += 1;
        }
    }
    RETENTION_RUNS.fetch_add(1, Ordering::Relaxed);
    SNAPSHOTS_THINNED.fetch_add(thinned as u64, Ordering::Relaxed);
    thinned
}

pub fn init() {
    crate::serial_println!(
        "[snap-policy] retention ladder armed (1h all / 24h hourly / 7d daily / 4w weekly, quota {})",
        SNAPSHOT_COUNT_QUOTA,
    );
}

/// Deterministic proof of the retention ladder + quota over a synthetic
/// timeline (no FS access — algorithm-only, identical on QEMU and iron).
pub fn run_boot_smoketest() {
    let now: u64 = 100 * WEEK_MS; // arbitrary "now" far from zero

    // Timeline: 3 fresh (<1h), pairs sharing an hour/day bucket, distinct old
    // buckets, 2 ancient. Timestamps sit MID-bucket (boundary minus half a
    // bucket): a timestamp exactly ON a boundary belongs to the next absolute
    // bucket, so "same bucket" pairs anchored there would be legitimately
    // distinct and nothing would thin.
    let hb = |k: u64| now - k * HOUR_MS - HOUR_MS / 2; // mid of an hour bucket
    let db = |k: u64| now - k * DAY_MS - DAY_MS / 2; // mid of a day bucket
    let wb = |k: u64| now - k * WEEK_MS - WEEK_MS / 2; // mid of a week bucket
    let snaps: Vec<(u32, u64)> = alloc::vec![
        (1, now - 10),            // fresh
        (2, now - HOUR_MS / 2),   // fresh
        (3, now - HOUR_MS + 1),   // fresh (just under 1h)
        (4, hb(2)),               // hour bucket A (newest wins)
        (5, hb(2) - 1000),        // hour bucket A (loses)
        (6, hb(5)),               // hour bucket B
        (7, hb(5) - 2000),        // hour bucket B (loses)
        (8, hb(22)),              // hour bucket C
        (9, db(2)),               // day bucket A
        (10, db(2) - 5000),       // day bucket A (loses)
        (11, db(6)),              // day bucket B
        (12, wb(1)),              // week bucket A
        (13, wb(2)),              // week bucket B
        (14, now - 6 * WEEK_MS),  // ancient (goes)
        (15, now - 10 * WEEK_MS), // ancient (goes)
    ];
    let victims = retention_victims(&snaps, now);

    let dup_hours_thinned = victims.contains(&5) && victims.contains(&7);
    let dup_day_thinned = victims.contains(&10);
    let ancient_thinned = victims.contains(&14) && victims.contains(&15);
    let fresh_kept = !victims.contains(&1) && !victims.contains(&2) && !victims.contains(&3);
    let buckets_kept = !victims.contains(&4)
        && !victims.contains(&6)
        && !victims.contains(&8)
        && !victims.contains(&9)
        && !victims.contains(&11)
        && !victims.contains(&12)
        && !victims.contains(&13);

    // Quota check: 20 fresh snapshots must thin to the cap.
    let many: Vec<(u32, u64)> = (0..20u32)
        .map(|i| (100 + i, now - i as u64 * 60_000))
        .collect();
    let quota_victims = retention_victims(&many, now);
    let quota_ok = many.len() - quota_victims.len() == SNAPSHOT_COUNT_QUOTA;

    let pass = dup_hours_thinned
        && dup_day_thinned
        && ancient_thinned
        && fresh_kept
        && buckets_kept
        && quota_ok;
    crate::serial_println!(
        "[snap-policy] retention selftest: fresh_kept={} buckets_kept={} dup_thinned={} ancient_thinned={} quota_ok={} -> {}",
        fresh_kept,
        buckets_kept,
        dup_hours_thinned && dup_day_thinned,
        ancient_thinned,
        quota_ok,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// `/proc/raeen/snap_policy` — retention/quota counters.
pub fn dump_text() -> String {
    alloc::format!(
        "# snapshot retention policy (1h all / 24h hourly / 7d daily / 4w weekly)\ncount_quota: {}\nretention_runs: {}\nsnapshots_thinned: {}\n",
        SNAPSHOT_COUNT_QUOTA,
        RETENTION_RUNS.load(Ordering::Relaxed),
        SNAPSHOTS_THINNED.load(Ordering::Relaxed),
    )
}
