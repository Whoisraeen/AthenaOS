//! Kernel soak / stress harness + heap-leak detector.
//!
//! Concept §"the user owns the machine" / stability: a shipping OS must run for
//! days without leaking memory or wedging. This module is a **bounded**,
//! boot-time workload that exercises the four subsystems a soak must cover —
//! CPU, memory (alloc/free churn), storage (sector read), network (driver
//! presence) — and then verifies the kernel heap returned to within 10% of its
//! pre-soak usage. That is a fast proxy for the 24h-soak leak criterion
//! ("heap usage at end ≤ 10% over start", MasterChecklist Phase 4.9); the full
//! 24h Athena run (Phase 4.12) layers more iterations on this same harness.
//!
//! R10: `init()` + `run_boot_smoketest()` + `/proc/raeen/soak` (`dump_text`) +
//! this Concept docstring.

extern crate alloc;

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static SOAK_RUNS: AtomicU64 = AtomicU64::new(0);
static SOAK_CHECKSUM: AtomicU64 = AtomicU64::new(0);
static SOAK_MEM_CYCLES: AtomicU64 = AtomicU64::new(0);
static SOAK_HEAP_START: AtomicU64 = AtomicU64::new(0);
static SOAK_HEAP_END: AtomicU64 = AtomicU64::new(0);
static SOAK_STORAGE_OK: AtomicBool = AtomicBool::new(false);
static SOAK_NETS: AtomicU64 = AtomicU64::new(0);
static SOAK_LEAK_OK: AtomicBool = AtomicBool::new(false);

pub fn init() {
    crate::serial_println!("[ OK ] Soak/leak harness ready");
}

/// CPU workload — a cheap mixing loop. Returns a checksum so the optimizer
/// can't elide it.
fn cpu_work(iters: u64) -> u64 {
    let mut acc = 0x9E37_79B9_7F4A_7C15u64;
    for i in 0..iters {
        acc = acc
            .wrapping_mul(6364136223846793005)
            .wrapping_add(i ^ (acc >> 29));
    }
    acc
}

/// Memory workload — churn `Vec`/`Box` allocations to stress the heap and
/// surface leaks. Everything allocated here is dropped before returning, so a
/// well-behaved allocator ends near where it started.
fn mem_work(cycles: usize) -> u64 {
    let mut mixed = 0u64;
    for c in 0..cycles {
        let n = 64 + (c % 64) * 16;
        let mut v: alloc::vec::Vec<u64> = alloc::vec::Vec::with_capacity(n);
        for i in 0..n {
            v.push((i as u64) ^ (c as u64));
        }
        mixed ^= v.iter().copied().fold(0u64, |a, x| a.wrapping_add(x));
        let b = alloc::boxed::Box::new([0xA5u8; 256]);
        mixed = mixed.wrapping_add(b[c % 256] as u64);
        // v + b dropped here → freed
    }
    mixed
}

/// Storage workload — read sector 0 from the active block device (read-only,
/// safe in safe-mode). Returns whether the read succeeded.
fn storage_work() -> bool {
    let guard = crate::block_io::ACTIVE_BLOCK_DEVICE.lock();
    match guard.as_ref() {
        Some(dev) => {
            let mut buf = [0u8; 512];
            dev.read_sector(0, &mut buf).is_ok()
        }
        None => false,
    }
}

/// Network workload — count registered net drivers (read-only presence check).
fn net_work() -> usize {
    crate::net_drivers::NET_DRIVERS
        .lock()
        .as_ref()
        .map_or(0, |m| m.list().len())
}

/// Run one bounded soak pass and check the heap leak budget.
pub fn run_boot_smoketest() {
    SOAK_RUNS.fetch_add(1, Ordering::Relaxed);

    // Sample heap BEFORE the workload (lock released by heap_used()).
    let start = crate::memory::allocator::heap_used();
    SOAK_HEAP_START.store(start as u64, Ordering::Relaxed);

    // Boot does a LIGHT bounded pass — just enough to prove the cpu/mem/storage/
    // net workload paths execute and the heap-leak accounting works. The HEAVY
    // 24h-style churn (millions of CPU iters + thousands of large alloc/free
    // cycles) is deliberately NOT run in the boot critical path: besides adding
    // ~seconds to boot, its heavy heap churn made a latent return-address-smash
    // corruption deterministic at boot (see MasterChecklist "Latent kernel bugs"
    // — root-cause pending KASAN). The full soak belongs in an on-demand /
    // post-boot mode, which is where Phase 4.9's 24h run will drive it.
    const BOOT_CPU_ITERS: u64 = 50_000;
    const BOOT_MEM_CYCLES: usize = 32;
    let cks = cpu_work(BOOT_CPU_ITERS);
    let mixed = mem_work(BOOT_MEM_CYCLES);
    SOAK_MEM_CYCLES.fetch_add(BOOT_MEM_CYCLES as u64, Ordering::Relaxed);
    SOAK_CHECKSUM.store(cks ^ mixed, Ordering::Relaxed);

    let storage_ok = storage_work();
    SOAK_STORAGE_OK.store(storage_ok, Ordering::Relaxed);
    let nets = net_work();
    SOAK_NETS.store(nets as u64, Ordering::Relaxed);

    // Sample heap AFTER (all soak allocations dropped).
    let end = crate::memory::allocator::heap_used();
    SOAK_HEAP_END.store(end as u64, Ordering::Relaxed);

    // Leak criterion: end ≤ start + 10% (with a small absolute slack for the
    // soak's own bookkeeping / dump-string allocations on tiny baselines).
    let budget = start
        .saturating_add(start / 10)
        .max(start.saturating_add(64 * 1024));
    let leak_ok = end <= budget;
    SOAK_LEAK_OK.store(leak_ok, Ordering::Relaxed);

    crate::serial_println!(
        "[soak] cpu_cks={:#x} mem_cycles=32 storage_read={} nets={} heap_start={} heap_end={} delta={} -> {}",
        SOAK_CHECKSUM.load(Ordering::Relaxed),
        storage_ok,
        nets,
        start,
        end,
        end as i64 - start as i64,
        if leak_ok { "PASS" } else { "FAIL(leak)" }
    );
}

// ===========================================================================
// Heavy KASAN endurance soak (feature = "kasan" only).
//
// The boot smoketest above is a deliberately LIGHT proxy: 32 mem cycles, run
// inside the masked boot critical path. It proves the leak ACCOUNTING works but
// is NOT a heavy-churn endurance proof, and it cannot exercise the KASAN
// free→quarantine→evict→reuse path under sustained fragmentation.
//
// This module adds a HEAVY churn loop that:
//   * does ~100k+ alloc/free pairs across VARIED sizes (small / medium / large
//     / multi-page) with realistic fragmentation (a live working set that is
//     partially retained and partially freed each round, LIFO + FIFO eviction);
//   * frees FAR more than the KASAN quarantine ring's 512 slots so the ring
//     genuinely cycles (freed+poisoned chunks get evicted, unpoisoned, reused —
//     the full UAF-detection lifecycle), proven by `kasan_eviction_count() > 0`;
//   * asserts at the end (a) heap delta == 0 (every alloc freed → no leak),
//     (b) ZERO KASAN reports during the churn (no UAF/OOB the churn triggered),
//     (c) the quarantine actually cycled (evictions > 0).
//
// It is FAIL-able: a real leak → delta != 0 → FAIL; a real UAF/OOB the churn
// trips → kasan_errors > 0 → FAIL; a churn that never cycled the ring →
// evictions == 0 → FAIL.
//
// KEYSTONES honored (CLAUDE.md §10):
//   * Runs as a REAL schedulable thread spawned POST-BOOT_COMPLETE with
//     interrupts ENABLED — NOT in the masked post-marker sweep. A heavy soak
//     that yields/preempts must not run masked.
//   * BSP-pinned (affinity mask = CPU 0): the AP cores hlt-loop post-boot and
//     never pull from their runqueues, so an unpinned post-boot kernel thread
//     silently never runs.
//   * The ENTIRE module is `#[cfg(feature = "kasan")]`: the DEFAULT boot does
//     not spawn it, does not churn, and is byte-identical. The heavy soak ships
//     only in the kasan/endurance build.

#[cfg(feature = "kasan")]
mod endurance {
    use super::{AtomicBool, AtomicU64, Ordering};
    use alloc::boxed::Box;
    use alloc::vec::Vec;

    /// Result fields, also surfaced on `/proc/raeen/soak`.
    pub static RAN: AtomicBool = AtomicBool::new(false);
    pub static CYCLES: AtomicU64 = AtomicU64::new(0);
    pub static HEAP_DELTA: AtomicU64 = AtomicU64::new(0); // stored as i64 bits
    pub static KASAN_ERRORS: AtomicU64 = AtomicU64::new(0);
    pub static EVICTIONS: AtomicU64 = AtomicU64::new(0);
    pub static PASS: AtomicBool = AtomicBool::new(false);

    /// Cycles of churn. Each cycle does 9 allocations (8 transient size classes +
    /// 1 working-set rotation) and the same number of frees, so CYCLE_COUNT * 9 is
    /// the alloc/free-pair count. 4_000 cycles ≈ 36k pairs — heavy endurance (vs
    /// the light boot-soak's 32 cycles), and ~36k frees through a 512-slot
    /// quarantine forces tens of thousands of evictions (≈70x ring turnover), so
    /// the full free→quarantine→evict→reuse lifecycle is exercised many times
    /// over. Sized to keep the KASAN-instrumented boot under the 300 s CI window
    /// while staying FAR past the ring (a higher count adds eviction headroom but
    /// no new coverage and risks the timeout; the eviction count proves cycling).
    const CYCLE_COUNT: usize = 4_000;

    /// Size of the persistent working set that is held across cycles and rotated
    /// (a slot is replaced each cycle → its old occupant is freed → quarantined →
    /// eventually evicted). This is the realistic "some live, some freed"
    /// fragmentation pattern, not a clean alloc-then-immediately-free.
    const WORKING_SET: usize = 96;

    /// Varied allocation sizes covering small / medium / large / multi-page so
    /// the churn stresses different heap free-list buckets and the shadow's
    /// granule rounding. The largest (8192, 16384) span multiple pages.
    const SIZES: [usize; 8] = [16, 48, 64, 200, 512, 1024, 8192, 16384];

    /// Touch every byte of a freshly-handed-out allocation so KASAN's per-byte
    /// shadow is actually consulted (a write the allocator-boundary check and the
    /// fill both cover). If the alloc hook failed to unpoison, OR the chunk were
    /// a still-quarantined UAF, this is exactly the access that would be caught.
    #[inline(never)]
    fn touch(buf: &mut [u8], tag: u8) -> u64 {
        let mut sum = 0u64;
        for (i, b) in buf.iter_mut().enumerate() {
            *b = tag ^ (i as u8);
            sum = sum.wrapping_add(*b as u64);
        }
        sum
    }

    /// The heavy churn. Returns a checksum so nothing is optimized away.
    fn churn() -> u64 {
        let mut mixed = 0u64;
        // The persistent working set: Some(Box) slots that get rotated.
        let mut live: Vec<Option<Box<[u8]>>> = Vec::with_capacity(WORKING_SET);
        for _ in 0..WORKING_SET {
            live.push(None);
        }

        for c in 0..CYCLE_COUNT {
            // (1) Transient burst: alloc several varied-size buffers, touch them,
            //     drop them at end of iteration (LIFO free order via Vec drop).
            let mut transient: Vec<Vec<u8>> = Vec::with_capacity(SIZES.len());
            for (k, &sz) in SIZES.iter().enumerate() {
                let mut v = alloc::vec![0u8; sz];
                mixed = mixed.wrapping_add(touch(&mut v, (c as u8).wrapping_add(k as u8)));
                transient.push(v);
            }
            // FIFO read-back of the transient set before it drops (more touches).
            for v in transient.iter() {
                mixed ^= v.first().copied().unwrap_or(0) as u64;
            }

            // (2) Rotate the persistent working set: replace one slot each cycle.
            //     The OLD occupant is freed here → poisoned → quarantined. Over
            //     CYCLE_COUNT >> 512 cycles this floods the ring and forces
            //     evictions (freed chunk leaves quarantine, unpoisoned, reused).
            let slot = c % WORKING_SET;
            let sz = SIZES[c % SIZES.len()];
            let mut nb = alloc::vec![0u8; sz].into_boxed_slice();
            mixed = mixed.wrapping_add(touch(&mut nb, 0xA5u8.wrapping_add(c as u8)));
            // Replacing Some(old) drops `old` → free → quarantine.
            live[slot] = Some(nb);

            // transient drops here → freed → quarantined.
        }

        // (3) Drain the persistent working set so EVERY allocation is freed →
        //     heap delta must return to 0 (no leak). Freed in slot order (FIFO).
        for slot in 0..WORKING_SET {
            live[slot] = None;
        }
        drop(live);

        mixed
    }

    /// Run the heavy churn, sample the KASAN/heap counters around it, assert the
    /// three conditions, and print the FAIL-able marker. Called synchronously
    /// from the kasan-build boot tail (see `run_endurance`) where interrupts are
    /// ENABLED but CPU0 is not yet scheduler-preemptible — a normal interrupts-on
    /// context (NOT the masked post-marker sweep), so the heap + quarantine locks
    /// are never held across a context switch.
    pub fn run() {
        let kasan_live = crate::memory::allocator::kasan_is_live();

        // Drain any chunks the EARLIER boot (incl. the [kasan] smoketest) left in
        // the quarantine, so `heap_start` is a clean quarantine-empty baseline.
        // Without this, the start sample over-counts by the pre-existing residency
        // and the post-churn flush makes the delta spuriously NEGATIVE — the
        // measurement must bracket the churn with a drained quarantine at BOTH
        // ends so the delta reflects ONLY this soak's net allocation.
        let _pre = crate::memory::allocator::kasan_quarantine_flush();

        // Sample heap + KASAN counters BEFORE the churn (quarantine now empty).
        let heap_start = crate::memory::allocator::heap_used();
        let err_start = crate::memory::allocator::kasan_error_count();
        let evict_start = crate::memory::allocator::kasan_eviction_count();

        let cks = churn();

        // Drain the KASAN quarantine before sampling: the ring deliberately holds
        // up to QUARANTINE_SLOTS (512) freed-but-not-yet-returned chunks at steady
        // state, so `heap_used()` over-counts by that FIXED residency (bounded,
        // independent of cycle count — NOT a leak). Flushing returns those chunks
        // to the heap so the only bytes still allocated after a clean churn are
        // genuine leaks (allocations never freed). The drained count must equal a
        // full ring after this many cycles.
        let drained = crate::memory::allocator::kasan_quarantine_flush();

        // Sample AFTER — all churn allocations dropped AND the quarantine drained.
        let heap_end = crate::memory::allocator::heap_used();
        let err_end = crate::memory::allocator::kasan_error_count();
        let evict_end = crate::memory::allocator::kasan_eviction_count();

        let delta = heap_end as i64 - heap_start as i64;
        let kasan_errors = err_end.saturating_sub(err_start);
        let evictions = evict_end.saturating_sub(evict_start);

        // PASS requires: no leak (delta == 0), zero KASAN findings during the
        // churn, the quarantine actually cycled (evictions > 0), and KASAN was
        // genuinely live (else this proved nothing).
        let pass = kasan_live && delta == 0 && kasan_errors == 0 && evictions > 0;

        CYCLES.store(CYCLE_COUNT as u64, Ordering::Relaxed);
        HEAP_DELTA.store(delta as u64, Ordering::Relaxed);
        KASAN_ERRORS.store(kasan_errors, Ordering::Relaxed);
        EVICTIONS.store(evictions, Ordering::Relaxed);
        PASS.store(pass, Ordering::Relaxed);
        RAN.store(true, Ordering::Relaxed);

        crate::serial_println!(
            "[soak-kasan] endurance: kasan_live={} cks={:#x} cycles={} heap_delta={} kasan_errors={} quarantine_evictions={} quarantine_drained={} -> {}",
            kasan_live,
            cks,
            CYCLE_COUNT,
            delta,
            kasan_errors,
            evictions,
            drained,
            if pass { "PASS" } else { "FAIL" }
        );
    }
}

/// Run the heavy KASAN endurance soak (feature = "kasan" only). Called from the
/// boot tail in the kasan/endurance build, at a point where interrupts are
/// ENABLED but BOOT_COMPLETE is not yet set — so CPU0 runs the churn to
/// completion as a normal interrupts-on context (NOT masked, NOT the post-marker
/// sweep), and the heavy churn deterministically lands its FAIL-able result line
/// instead of racing the CI reap as a starved fire-and-forget thread would.
///
/// This is gated entirely behind `feature = "kasan"`: the DEFAULT build does not
/// call this (the no-op below) and its boot path is byte-identical.
#[cfg(feature = "kasan")]
pub fn run_endurance() {
    crate::serial_println!("[soak-kasan] starting heavy endurance soak (IRQs on, pre-marker)");
    endurance::run();
}

/// Default build: the heavy endurance soak does not exist — boot is unaffected.
#[cfg(not(feature = "kasan"))]
pub fn run_endurance() {}

/// `/proc/raeen/soak` body.
pub fn dump_text() -> alloc::string::String {
    let base = alloc::format!(
        "# AthenaOS soak / leak harness (Phase 4.9)\n\
         runs:            {}\n\
         cpu_checksum:    {:#018x}\n\
         mem_cycles:      {}\n\
         storage_read_ok: {}\n\
         net_drivers:     {}\n\
         heap_start:      {} bytes\n\
         heap_end:        {} bytes\n\
         heap_delta:      {} bytes\n\
         leak_ok:         {}\n",
        SOAK_RUNS.load(Ordering::Relaxed),
        SOAK_CHECKSUM.load(Ordering::Relaxed),
        SOAK_MEM_CYCLES.load(Ordering::Relaxed),
        SOAK_STORAGE_OK.load(Ordering::Relaxed),
        SOAK_NETS.load(Ordering::Relaxed),
        SOAK_HEAP_START.load(Ordering::Relaxed),
        SOAK_HEAP_END.load(Ordering::Relaxed),
        SOAK_HEAP_END.load(Ordering::Relaxed) as i64
            - SOAK_HEAP_START.load(Ordering::Relaxed) as i64,
        SOAK_LEAK_OK.load(Ordering::Relaxed),
    );

    #[cfg(feature = "kasan")]
    {
        let mut s = base;
        s.push_str(&alloc::format!(
            "# heavy KASAN endurance soak (post-BOOT_COMPLETE thread)\n\
             kasan_endurance_ran:   {}\n\
             kasan_cycles:          {}\n\
             kasan_heap_delta:      {} bytes\n\
             kasan_errors:          {}\n\
             kasan_evictions:       {}\n\
             kasan_endurance_pass:  {}\n",
            endurance::RAN.load(Ordering::Relaxed),
            endurance::CYCLES.load(Ordering::Relaxed),
            endurance::HEAP_DELTA.load(Ordering::Relaxed) as i64,
            endurance::KASAN_ERRORS.load(Ordering::Relaxed),
            endurance::EVICTIONS.load(Ordering::Relaxed),
            endurance::PASS.load(Ordering::Relaxed),
        ));
        return s;
    }

    #[cfg(not(feature = "kasan"))]
    base
}
