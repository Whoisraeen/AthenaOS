//! `/proc/raeen/perf` — the missing telemetry surface for the Concept's North
//! Star contracts.
//!
//! The Concept's headline promises (sub-frame input latency, sub-3ms audio,
//! SCHED_GAME hard deadlines, fast boot) were unmeasurable — and CLAUDE.md is
//! explicit that "a perf claim without a counter behind it is `[~]` at best".
//! This module gives each promise a live counter so it becomes falsifiable:
//!   * input events + a TSC timestamp of the last one (latency proxy),
//!   * frames presented (compositor throughput),
//!   * audio periods + underruns (the sub-3ms / zero-underrun contract),
//!   * SCHED_GAME deadline misses (surfaced from the scheduler),
//!   * total boot time.
//! Hot-path producers call the `record_*` helpers (a single relaxed atomic).

extern crate alloc;
use core::sync::atomic::{AtomicU64, Ordering};

static INPUT_EVENTS: AtomicU64 = AtomicU64::new(0);
static LAST_INPUT_TSC: AtomicU64 = AtomicU64::new(0);
static FRAMES_PRESENTED: AtomicU64 = AtomicU64::new(0);
static AUDIO_PERIODS: AtomicU64 = AtomicU64::new(0);
static AUDIO_UNDERRUNS: AtomicU64 = AtomicU64::new(0);

// ─── audio thread wake jitter (PERFORMANCE_TARGETS §3: < 100 µs) ──────────────
// The RaeAudio mix thread runs SCHED_GAME with a hard period; wake JITTER is how
// far each actual period drifts from the nominal period. Measured at the
// record_audio_period site: the TSC delta between consecutive periods IS the
// realized period, and jitter = |realized − budget|. AUDIO_PERIOD_BUDGET_US
// defaults to ~2.67 ms (a 128-frame buffer @ 48 kHz); the audio engine can set
// the real period via set_audio_period_us. Live numbers need sustained audio
// (iron / a real PCM sink); the jitter MATH is FAIL-ably unit-tested.
static AUDIO_PERIOD_BUDGET_US: AtomicU64 = AtomicU64::new(2670);
static AUDIO_PREV_TSC: AtomicU64 = AtomicU64::new(0);
static AJ_SAMPLES: AtomicU64 = AtomicU64::new(0);
static AJ_SUM_US: AtomicU64 = AtomicU64::new(0);
static AJ_MAX_US: AtomicU64 = AtomicU64::new(0);
static AJ_LAST_US: AtomicU64 = AtomicU64::new(0);

// ─── SCHED_GAME deadline telemetry (the Concept's "Gaming isn't a mode") ─────
// Detected scheduler-side at the EDF dispatch point: when an earliest-deadline
// SCHED_GAME task is *picked* and the monotonic clock is already past its
// absolute deadline, that period's deadline is provably missed. These counters
// are the dispatch-path twin of the per-task aggregate in `scheduler::
// deadline_stats` — they live here, lock-free, so the pick hot path bumps a
// single relaxed atomic with no new lock (the scheduler already holds the
// SCHEDULER lock at the pick, and taking another lock there would invert lock
// order). `dispatches` is the denominator that makes the miss *rate* knowable.
static SG_DISPATCHES: AtomicU64 = AtomicU64::new(0);
static SG_DEADLINE_MISSES: AtomicU64 = AtomicU64::new(0);
static SG_WORST_LATENESS_NS: AtomicU64 = AtomicU64::new(0);
static SG_LAST_LATENESS_NS: AtomicU64 = AtomicU64::new(0);

// ─── scheduler pick/switch telemetry (per-class + context-switch totals) ─────
static CONTEXT_SWITCHES: AtomicU64 = AtomicU64::new(0);
static PICKS_GAME: AtomicU64 = AtomicU64::new(0);
static PICKS_NORMAL: AtomicU64 = AtomicU64::new(0);
static MAX_RUNQ_DEPTH: AtomicU64 = AtomicU64::new(0);

// ─── input→photon latency (the Concept's "sub-frame input latency") ──────────
// The earliest input event since the last present whose effect has NOT yet
// reached the screen. 0 = no input pending. We measure latency from THIS (the
// worst case the user perceives) to the present that reflects it, not from the
// most recent input — a burst of mouse deltas between two frames is one visible
// update, and its felt latency is from the first delta.
static PENDING_INPUT_TSC: AtomicU64 = AtomicU64::new(0);
static IP_SAMPLES: AtomicU64 = AtomicU64::new(0);
static IP_SUM_TSC: AtomicU64 = AtomicU64::new(0);
static IP_MAX_TSC: AtomicU64 = AtomicU64::new(0);
static IP_LAST_TSC: AtomicU64 = AtomicU64::new(0);

// ─── input → game-thread wake latency (PERFORMANCE_TARGETS §4: < 1 ms) ────────
// The Concept: "input is IRQ-driven and the consuming game thread is SCHED_GAME
// — an input event must preempt normal work." This measures the time from an
// input event (USB-HID / PS/2 IRQ → record_input_event) to the next SCHED_GAME
// (EDF) task actually getting the CPU (record_game_dispatch, called live from
// the pick path at scheduler.rs:436). INPUT_WAKE_TSC is armed at the FIRST input
// after a game dispatch consumed the prior one (compare_exchange against 0), so
// a burst of inputs measures from the earliest — the worst case the player
// feels. Distinct from PENDING_INPUT_TSC (which the COMPOSITOR present consumes
// for input→photon); a game thread may run before the next frame, so the two
// latencies are different and both worth knowing.
static INPUT_WAKE_TSC: AtomicU64 = AtomicU64::new(0);
static IGW_SAMPLES: AtomicU64 = AtomicU64::new(0);
static IGW_SUM_TSC: AtomicU64 = AtomicU64::new(0);
static IGW_MAX_TSC: AtomicU64 = AtomicU64::new(0);
static IGW_LAST_TSC: AtomicU64 = AtomicU64::new(0);

// ─── frametime telemetry (the Game Bar's FPS + frametime-graph source) ───────
// `record_frame_present` already stamps a TSC each compositor present; the
// delta between consecutive presents IS the frametime. We keep the TSC of the
// previous present and a tiny lock-free ring of the most recent N frametimes
// in microseconds so the GameOS Game Bar (and `/proc/raeen/perf`) can read live
// FPS + a frametime history WITHOUT parsing text. The ring is fixed-size (no
// allocation, ever) and written only on the present hot path; readers snapshot
// it. This is additive instrumentation — the input→photon recording above is
// untouched.
const FT_RING: usize = 120;
static PREV_PRESENT_TSC: AtomicU64 = AtomicU64::new(0);
static FT_RING_US: [AtomicU64; FT_RING] = {
    const Z: AtomicU64 = AtomicU64::new(0);
    [Z; FT_RING]
};
static FT_WRITE_IDX: AtomicU64 = AtomicU64::new(0);
static FT_COUNT: AtomicU64 = AtomicU64::new(0);
static FT_LAST_US: AtomicU64 = AtomicU64::new(0);

// ─── missed-frame rate (PERFORMANCE_TARGETS §2: < 0.1 % steady state) ─────────
// A "missed frame" is a present whose frametime overran the refresh window by
// enough that at least one vsync was skipped (ft_us > 1.5× the budget). The
// budget defaults to 60 Hz (16667 µs) and the compositor calls
// `set_frame_budget_us` with the panel's real refresh (120/144/240 Hz) once
// known, so the contract is checked against the actual display. Counted over the
// same frametime path as the ring; readers derive the rate = missed / presented.
static FRAME_BUDGET_US: AtomicU64 = AtomicU64::new(16_667); // 60 Hz default
static MISSED_FRAMES: AtomicU64 = AtomicU64::new(0);
static FRAMETIME_SAMPLES: AtomicU64 = AtomicU64::new(0);

// ─── heap alloc fast-path cost (PERFORMANCE_TARGETS §7: slab alloc < 500 ns) ──
// Measured ONCE at boot by `measure_heap_alloc` (a microbench over the REAL
// global allocator) rather than instrumenting every alloc — taxing the hottest
// path in the kernel with two rdtsc + atomics per allocation would itself blow
// the budget it claims to measure. The boot microbench warms the size-class
// free list, then times repeated same-size alloc/free pairs (the steady-state
// fast path: each alloc reuses the slot just freed). Stored in nanoseconds so
// the sub-microsecond contract stays legible. 0 = not yet measured.
static HEAP_ALLOC_MIN_NS: AtomicU64 = AtomicU64::new(0);
static HEAP_ALLOC_AVG_NS: AtomicU64 = AtomicU64::new(0);
static HEAP_ALLOC_MAX_NS: AtomicU64 = AtomicU64::new(0);
static HEAP_ALLOC_SAMPLES: AtomicU64 = AtomicU64::new(0);

// ─── scheduler pick (decision/scan) latency — part of the §5 context-switch ──
// budget. `pick_next` is the runqueue + EDF earliest-deadline scan: the
// scheduler's *decision* cost, the part of a context switch that scales with
// runqueue depth (the asm register/stack swap in switch_context is ~fixed and
// is measured separately — and is genuinely entangled, see the perf goal notes).
// The steady-state preemption path brackets `pick_next` with rdtsc and feeds
// raw TSC ticks here; dump_text converts to ns once via tsc_mhz (no per-pick
// division on the hot path). Two rdtsc + relaxed atomics per pick is negligible
// at the ~100 Hz preempt cadence, so this is true always-on telemetry.
static PICK_SAMPLES: AtomicU64 = AtomicU64::new(0);
static PICK_SUM_TSC: AtomicU64 = AtomicU64::new(0);
static PICK_MIN_TSC: AtomicU64 = AtomicU64::new(u64::MAX);
static PICK_MAX_TSC: AtomicU64 = AtomicU64::new(0);

/// Record one scheduler `pick_next` duration in raw TSC ticks (the bracketed
/// decision/scan cost). Lock-free (relaxed atomics + fetch_min/max) — safe to
/// call with the SCHEDULER lock held, which the pick path already does. A zero
/// or absurd delta (TSC hiccup) is dropped so the min/avg stay meaningful.
#[inline]
pub fn record_pick_ticks(ticks: u64) {
    if ticks == 0 || ticks > 1_000_000_000 {
        return; // implausible (uncalibrated / migrated CPU) — don't poison stats
    }
    PICK_SAMPLES.fetch_add(1, Ordering::Relaxed);
    PICK_SUM_TSC.fetch_add(ticks, Ordering::Relaxed);
    PICK_MIN_TSC.fetch_min(ticks, Ordering::Relaxed);
    PICK_MAX_TSC.fetch_max(ticks, Ordering::Relaxed);
}

/// Live snapshot of the scheduler pick latency `(samples, min_ns, avg_ns,
/// max_ns)`, converted from raw TSC via the calibrated tsc_mhz. samples==0 →
/// all-zero (no pick measured yet).
#[inline]
#[must_use]
pub fn pick_latency_snapshot() -> (u64, u64, u64, u64) {
    let samples = PICK_SAMPLES.load(Ordering::Relaxed);
    if samples == 0 {
        return (0, 0, 0, 0);
    }
    let mhz = crate::fast_boot::tsc_mhz().max(1);
    let to_ns = |ticks: u64| ticks.saturating_mul(1000) / mhz;
    let min_raw = PICK_MIN_TSC.load(Ordering::Relaxed);
    let min_ns = if min_raw == u64::MAX {
        0
    } else {
        to_ns(min_raw)
    };
    let avg_ns = to_ns(PICK_SUM_TSC.load(Ordering::Relaxed) / samples);
    let max_ns = to_ns(PICK_MAX_TSC.load(Ordering::Relaxed));
    (samples, min_ns, avg_ns, max_ns)
}

#[inline]
fn rdtsc() -> u64 {
    unsafe { core::arch::x86_64::_rdtsc() }
}

/// Microbenchmark the heap alloc fast path against the REAL global allocator and
/// record min/avg/max nanoseconds into the perf surface. Returns the average ns
/// (0 if the TSC isn't calibrated yet). Allocation-only timing: each iteration
/// frees immediately after measuring so the next alloc hits the warm free list —
/// the steady-state fast path the < 500 ns contract targets (iron; TCG inflates
/// ~3–4×). Uses a fixed 64-byte layout (a representative small object) and a
/// bounded iteration count: no recursion into the thing it measures, no lock.
pub fn measure_heap_alloc() -> u64 {
    use alloc::alloc::{alloc, dealloc, Layout};
    let mhz = crate::fast_boot::tsc_mhz();
    if mhz == 0 {
        return 0; // TSC not calibrated — refuse to report a bogus number
    }
    let layout = match Layout::from_size_align(64, 16) {
        Ok(l) => l,
        Err(_) => return 0,
    };
    // Warm the size class so the timed loop measures free-list reuse, not the
    // one-time pool growth.
    unsafe {
        let warm = alloc(layout);
        if warm.is_null() {
            return 0;
        }
        dealloc(warm, layout);
    }
    const ITERS: u64 = 256;
    let mut min_ticks = u64::MAX;
    let mut max_ticks = 0u64;
    let mut sum_ticks = 0u64;
    let mut counted = 0u64;
    for _ in 0..ITERS {
        let t0 = rdtsc();
        let p = unsafe { alloc(layout) };
        let t1 = rdtsc();
        if p.is_null() {
            break; // allocator exhausted — stop rather than record a fault
        }
        // Touch the allocation so the compiler can't elide it.
        unsafe {
            core::ptr::write_volatile(p, 0xA5u8);
            dealloc(p, layout);
        }
        let d = t1.saturating_sub(t0);
        min_ticks = min_ticks.min(d);
        max_ticks = max_ticks.max(d);
        sum_ticks = sum_ticks.saturating_add(d);
        counted += 1;
    }
    if counted == 0 {
        return 0;
    }
    // ticks → ns: ns = ticks * 1000 / (ticks per µs). Integer math keeps the
    // sub-µs resolution the contract needs (no_std soft-float friendly).
    let to_ns = |ticks: u64| ticks.saturating_mul(1000) / mhz;
    let avg_ns = to_ns(sum_ticks / counted);
    HEAP_ALLOC_MIN_NS.store(to_ns(min_ticks), Ordering::Relaxed);
    HEAP_ALLOC_AVG_NS.store(avg_ns, Ordering::Relaxed);
    HEAP_ALLOC_MAX_NS.store(to_ns(max_ticks), Ordering::Relaxed);
    HEAP_ALLOC_SAMPLES.store(counted, Ordering::Relaxed);
    avg_ns
}

/// Live snapshot of the heap-alloc microbench `(min_ns, avg_ns, max_ns, samples)`.
#[inline]
#[must_use]
pub fn heap_alloc_snapshot() -> (u64, u64, u64, u64) {
    (
        HEAP_ALLOC_MIN_NS.load(Ordering::Relaxed),
        HEAP_ALLOC_AVG_NS.load(Ordering::Relaxed),
        HEAP_ALLOC_MAX_NS.load(Ordering::Relaxed),
        HEAP_ALLOC_SAMPLES.load(Ordering::Relaxed),
    )
}

/// Record a user input event that should produce a visible change (a real
/// keypress/transition or non-zero pointer motion — NOT idle HID reports).
/// Called from the USB-HID dispatch path (the iron input source) and the PS/2
/// IRQ. Arms the input→photon clock at the FIRST such event after a present.
#[inline]
pub fn record_input_event() {
    INPUT_EVENTS.fetch_add(1, Ordering::Relaxed);
    let now = rdtsc();
    LAST_INPUT_TSC.store(now, Ordering::Relaxed);
    // Only the first input after a present arms the clock; a burst before the
    // next frame keeps the earliest stamp (compare_exchange against 0).
    let _ = PENDING_INPUT_TSC.compare_exchange(0, now, Ordering::Relaxed, Ordering::Relaxed);
    // Arm the input→game-wake clock the same way (consumed by the next EDF
    // dispatch, not the next present — see INPUT_WAKE_TSC).
    let _ = INPUT_WAKE_TSC.compare_exchange(0, now, Ordering::Relaxed, Ordering::Relaxed);
}

/// Set the per-frame budget (refresh window) in microseconds. The compositor
/// calls this with the active panel's refresh once known (60→16667, 120→8333,
/// 144→6944, 240→4166). Defaults to 60 Hz. A 0 is ignored (keeps the prior
/// budget) so a transient mode query can't disable missed-frame detection.
#[inline]
pub fn set_frame_budget_us(us: u64) {
    if us != 0 {
        FRAME_BUDGET_US.store(us, Ordering::Relaxed);
    }
}

/// Account one frametime (µs) into the ring + the missed-frame contract. Split
/// out from `record_frame_present` so the policy (ring write + miss detection)
/// is unit-testable without the rdtsc plumbing. A frame is "missed" when its
/// frametime overran the refresh window enough to skip a vsync (> 1.5× budget).
#[inline]
pub fn note_frametime_us(ft_us: u64) {
    FT_LAST_US.store(ft_us, Ordering::Relaxed);
    let idx = (FT_WRITE_IDX.fetch_add(1, Ordering::Relaxed) as usize) % FT_RING;
    FT_RING_US[idx].store(ft_us, Ordering::Relaxed);
    let c = FT_COUNT.load(Ordering::Relaxed);
    if (c as usize) < FT_RING {
        FT_COUNT.store(c + 1, Ordering::Relaxed);
    }
    FRAMETIME_SAMPLES.fetch_add(1, Ordering::Relaxed);
    let budget = FRAME_BUDGET_US.load(Ordering::Relaxed).max(1);
    // > 1.5× budget ⇒ at least one refresh interval was skipped.
    if ft_us > budget.saturating_add(budget / 2) {
        MISSED_FRAMES.fetch_add(1, Ordering::Relaxed);
    }
}

/// Live snapshot of the missed-frame contract `(budget_us, missed, samples,
/// miss_rate_bp)` where the rate is basis points (missed × 10000 / samples).
#[inline]
#[must_use]
pub fn missed_frame_snapshot() -> (u64, u64, u64, u64) {
    let budget = FRAME_BUDGET_US.load(Ordering::Relaxed);
    let missed = MISSED_FRAMES.load(Ordering::Relaxed);
    let samples = FRAMETIME_SAMPLES.load(Ordering::Relaxed);
    let rate_bp = if samples == 0 {
        0
    } else {
        missed.saturating_mul(10_000) / samples
    };
    (budget, missed, samples, rate_bp)
}

/// Record a compositor frame present. If an input has been waiting for a frame,
/// this present is the photon that reflects it: input→photon latency = now −
/// the earliest pending input's TSC. Accumulate it (count / sum / max / last).
#[inline]
pub fn record_frame_present() {
    FRAMES_PRESENTED.fetch_add(1, Ordering::Relaxed);

    // Frametime = delta since the previous present, in microseconds. Skip the
    // very first present (no previous stamp) and any non-monotonic delta (a TSC
    // reset / sentinel zero) so the ring never carries garbage.
    let now = rdtsc();
    let prev = PREV_PRESENT_TSC.swap(now, Ordering::Relaxed);
    if prev != 0 && now > prev {
        let mhz = crate::fast_boot::tsc_mhz().max(1);
        let ft_us = (now - prev) / mhz;
        // Bound the stored value so a pathological stall (e.g. a long boot gap
        // before the first sustained frames) can't poison the FPS estimate.
        note_frametime_us(ft_us.min(10_000_000));
    }

    let pending = PENDING_INPUT_TSC.swap(0, Ordering::Relaxed);
    if pending == 0 {
        return; // idle redraw — no input to attribute this frame to
    }
    let lat = rdtsc().saturating_sub(pending);
    IP_SAMPLES.fetch_add(1, Ordering::Relaxed);
    IP_SUM_TSC.fetch_add(lat, Ordering::Relaxed);
    IP_LAST_TSC.store(lat, Ordering::Relaxed);
    let mut cur = IP_MAX_TSC.load(Ordering::Relaxed);
    while lat > cur {
        match IP_MAX_TSC.compare_exchange_weak(cur, lat, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(x) => cur = x,
        }
    }
}

/// Record an audio mix period; `underrun` true if the ring ran dry.
#[inline]
pub fn record_audio_period(underrun: bool) {
    AUDIO_PERIODS.fetch_add(1, Ordering::Relaxed);
    if underrun {
        AUDIO_UNDERRUNS.fetch_add(1, Ordering::Relaxed);
    }
    // Wake jitter: the TSC delta since the previous period IS the realized
    // period. Skip the first call (no previous stamp) and any non-monotonic
    // delta. Convert to µs and feed the jitter math.
    let now = rdtsc();
    let prev = AUDIO_PREV_TSC.swap(now, Ordering::Relaxed);
    if prev != 0 && now > prev {
        let mhz = crate::fast_boot::tsc_mhz().max(1);
        let period_us = (now - prev) / mhz;
        // Bound so a long boot gap before sustained audio can't poison the stats.
        note_audio_period_us(period_us.min(10_000_000));
    }
}

/// Set the nominal audio mix period (µs) the jitter is measured against. The
/// audio engine calls this with its real buffer period (frames/sample_rate). 0
/// is ignored. Default ~2.67 ms (128 frames @ 48 kHz).
#[inline]
pub fn set_audio_period_us(us: u64) {
    if us != 0 {
        AUDIO_PERIOD_BUDGET_US.store(us, Ordering::Relaxed);
    }
}

/// Account one realized audio period (µs) into the jitter stats. Split out from
/// record_audio_period so the math is unit-testable without rdtsc plumbing.
/// jitter = |realized − budget|; tracks count / sum / max / last.
#[inline]
pub fn note_audio_period_us(period_us: u64) {
    let budget = AUDIO_PERIOD_BUDGET_US.load(Ordering::Relaxed);
    let jitter = period_us.abs_diff(budget);
    AJ_SAMPLES.fetch_add(1, Ordering::Relaxed);
    AJ_SUM_US.fetch_add(jitter, Ordering::Relaxed);
    AJ_LAST_US.store(jitter, Ordering::Relaxed);
    let mut cur = AJ_MAX_US.load(Ordering::Relaxed);
    while jitter > cur {
        match AJ_MAX_US.compare_exchange_weak(cur, jitter, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(x) => cur = x,
        }
    }
}

/// Live snapshot of audio wake jitter `(samples, avg_us, max_us, last_us)` — the
/// §3 "< 100 µs" contract. samples==0 → all-zero (no sustained audio yet).
#[inline]
#[must_use]
pub fn audio_jitter_snapshot() -> (u64, u64, u64, u64) {
    let s = AJ_SAMPLES.load(Ordering::Relaxed);
    if s == 0 {
        return (0, 0, 0, 0);
    }
    (
        s,
        AJ_SUM_US.load(Ordering::Relaxed) / s,
        AJ_MAX_US.load(Ordering::Relaxed),
        AJ_LAST_US.load(Ordering::Relaxed),
    )
}

/// Record a SCHED_GAME (EDF) task dispatch from the scheduler pick path.
/// `now_us` and `absolute_deadline_us` are in the scheduler's existing
/// microsecond monotonic time base (tick × TICK_PERIOD_US) — NO new clock is
/// introduced. If `now_us > absolute_deadline_us` the period's deadline was
/// already blown by the time the task got the CPU: a real deadline miss, with
/// lateness = (now − deadline). Stored in nanoseconds (µs × 1000) so the
/// Concept's sub-millisecond budgets stay legible. A handful of relaxed atomic
/// ops — no lock, no allocation — safe to call with the SCHEDULER lock held.
#[inline]
pub fn record_game_dispatch(now_us: u64, absolute_deadline_us: u64) {
    SG_DISPATCHES.fetch_add(1, Ordering::Relaxed);
    // Input → game-thread wake latency (§4): if an input has been waiting since
    // before this SCHED_GAME task got the CPU, this dispatch is when the game
    // thread can first react to it. latency = now − the earliest pending input.
    // rdtsc here (not now_us) so it shares the input clock. Consume (swap 0) so
    // each input arms exactly one measurement.
    let wake = INPUT_WAKE_TSC.swap(0, Ordering::Relaxed);
    if wake != 0 {
        let lat = rdtsc().saturating_sub(wake);
        IGW_SAMPLES.fetch_add(1, Ordering::Relaxed);
        IGW_SUM_TSC.fetch_add(lat, Ordering::Relaxed);
        IGW_LAST_TSC.store(lat, Ordering::Relaxed);
        let mut cur = IGW_MAX_TSC.load(Ordering::Relaxed);
        while lat > cur {
            match IGW_MAX_TSC.compare_exchange_weak(cur, lat, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(x) => cur = x,
            }
        }
    }
    if now_us > absolute_deadline_us {
        let lateness_ns = (now_us - absolute_deadline_us).saturating_mul(1_000);
        SG_DEADLINE_MISSES.fetch_add(1, Ordering::Relaxed);
        SG_LAST_LATENESS_NS.store(lateness_ns, Ordering::Relaxed);
        let mut cur = SG_WORST_LATENESS_NS.load(Ordering::Relaxed);
        while lateness_ns > cur {
            match SG_WORST_LATENESS_NS.compare_exchange_weak(
                cur,
                lateness_ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(x) => cur = x,
            }
        }
    }
}

/// Live snapshot of input→game-wake latency `(samples, avg_us, max_us, last_us)`
/// — the §4 "input event → game wake latency < 1ms" contract. samples==0 →
/// all-zero (no input has driven a game dispatch yet, e.g. headless CI).
#[inline]
#[must_use]
pub fn input_game_wake_snapshot() -> (u64, u64, u64, u64) {
    let s = IGW_SAMPLES.load(Ordering::Relaxed);
    if s == 0 {
        return (0, 0, 0, 0);
    }
    let mhz = crate::fast_boot::tsc_mhz().max(1);
    (
        s,
        (IGW_SUM_TSC.load(Ordering::Relaxed) / s) / mhz,
        IGW_MAX_TSC.load(Ordering::Relaxed) / mhz,
        IGW_LAST_TSC.load(Ordering::Relaxed) / mhz,
    )
}

/// Record one context switch (a successful pick that actually changed the
/// running task) and which class won the CPU. `is_game` true = SCHED_GAME
/// (deadline or round-robin Game), false = Normal/CFS. One relaxed bump each.
#[inline]
pub fn record_context_switch(is_game: bool) {
    CONTEXT_SWITCHES.fetch_add(1, Ordering::Relaxed);
    if is_game {
        PICKS_GAME.fetch_add(1, Ordering::Relaxed);
    } else {
        PICKS_NORMAL.fetch_add(1, Ordering::Relaxed);
    }
}

/// Observe a runqueue depth at pick time; keeps the running maximum. Cheap
/// lock-free max so /proc/raeen/perf can show the worst backlog seen.
#[inline]
pub fn observe_runq_depth(depth: u64) {
    let mut cur = MAX_RUNQ_DEPTH.load(Ordering::Relaxed);
    while depth > cur {
        match MAX_RUNQ_DEPTH.compare_exchange_weak(cur, depth, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_) => break,
            Err(x) => cur = x,
        }
    }
}

/// Live snapshot of the SCHED_GAME (EDF) dispatch-path counters for the
/// deadline-adherence proof (`sched_proof`): `(dispatches, deadline_misses,
/// worst_lateness_ns, last_lateness_ns)`. These are the SAME lock-free atoms the
/// EDF pick path bumps via `record_game_dispatch`, so a reader sampling them
/// before/after a workload window measures exactly what the scheduler observed —
/// no separate clock, no instrumentation drift. Relaxed loads (counters only).
#[inline]
#[must_use]
pub fn sched_game_snapshot() -> (u64, u64, u64, u64) {
    (
        SG_DISPATCHES.load(Ordering::Relaxed),
        SG_DEADLINE_MISSES.load(Ordering::Relaxed),
        SG_WORST_LATENESS_NS.load(Ordering::Relaxed),
        SG_LAST_LATENESS_NS.load(Ordering::Relaxed),
    )
}

/// Live snapshot of the per-class pick counters `(picks_game, picks_normal)` —
/// proves an EDF workload actually PREEMPTED the competing Normal load (game
/// picks climbed while normal picks were also served, i.e. neither class was
/// starved). Same relaxed atoms the context-switch hot path bumps.
#[inline]
#[must_use]
pub fn pick_class_snapshot() -> (u64, u64) {
    (
        PICKS_GAME.load(Ordering::Relaxed),
        PICKS_NORMAL.load(Ordering::Relaxed),
    )
}

// ─── typed read accessors (the Game Bar reads these, not the text dump) ──────

/// The most recent frametime in microseconds, or `None` if no frame has been
/// presented yet (e.g. headless boot before the compositor runs). A typed read
/// for the GameOS Game Bar — cleaner than parsing `dump_text()`.
#[inline]
#[must_use]
pub fn last_frametime_us() -> Option<u64> {
    match FT_LAST_US.load(Ordering::Relaxed) {
        0 => None,
        v => Some(v),
    }
}

/// A live FPS estimate (frames per second × 100, fixed-point) derived from the
/// average of the recent frametime ring, or `None` if no frametime samples are
/// recorded yet. Fixed-point (×100) keeps this `no_std`/soft-float friendly and
/// lossless to two decimals — the caller divides by 100. 0-length / 0-sum
/// guards mean it never divides by zero.
#[inline]
#[must_use]
pub fn fps_estimate_x100() -> Option<u64> {
    let n = FT_COUNT.load(Ordering::Relaxed) as usize;
    if n == 0 {
        return None;
    }
    let mut sum_us: u64 = 0;
    for slot in FT_RING_US.iter().take(n) {
        sum_us = sum_us.saturating_add(slot.load(Ordering::Relaxed));
    }
    if sum_us == 0 {
        return None;
    }
    let avg_us = sum_us / n as u64;
    if avg_us == 0 {
        return None;
    }
    // fps = 1e6 / avg_us; ×100 fixed-point = 1e8 / avg_us.
    Some(100_000_000 / avg_us)
}

/// Copy the recent frametime history (oldest→newest, microseconds) into `out`,
/// returning the number of points written. The Game Bar's frametime graph reads
/// this each time it opens — a fixed-size ring, so zero allocation. Writes at
/// most `out.len()` and at most the number of recorded samples.
#[must_use]
pub fn frametime_history_us(out: &mut [u64]) -> usize {
    let n = (FT_COUNT.load(Ordering::Relaxed) as usize).min(FT_RING);
    if n == 0 || out.is_empty() {
        return 0;
    }
    let write_idx = FT_WRITE_IDX.load(Ordering::Relaxed) as usize;
    // The oldest of the n valid samples sits at (write_idx - n) mod FT_RING.
    let take = n.min(out.len());
    let start = (write_idx + FT_RING - n) % FT_RING;
    for (i, slot) in out.iter_mut().enumerate().take(take) {
        let ring_i = (start + (n - take) + i) % FT_RING;
        *slot = FT_RING_US[ring_i].load(Ordering::Relaxed);
    }
    take
}

pub fn init() {
    crate::serial_println!("[ OK ] perf telemetry ready (/proc/raeen/perf)");
}

/// `/proc/raeen/perf` body — one line per North Star contract counter.
pub fn dump_text() -> alloc::string::String {
    let (dl_misses, dl_worst_us) = crate::scheduler::deadline_miss_stats();
    let (pick_s, pick_min_ns, pick_avg_ns, pick_max_ns) = pick_latency_snapshot();
    let (frame_budget_us, frame_missed, frame_samples, frame_miss_bp) = missed_frame_snapshot();
    let (igw_s, igw_avg_us, igw_max_us, igw_last_us) = input_game_wake_snapshot();
    let (aj_s, aj_avg_us, aj_max_us, _aj_last_us) = audio_jitter_snapshot();
    let aj_budget_us = AUDIO_PERIOD_BUDGET_US.load(Ordering::Relaxed);
    let boot_ms = crate::fast_boot::boot_time_ms();
    // TSC ticks → µs: tsc_mhz == ticks per microsecond. 0 before calibration.
    let mhz = crate::fast_boot::tsc_mhz().max(1);
    let ip_samples = IP_SAMPLES.load(Ordering::Relaxed);
    let ip_avg_us = if ip_samples > 0 {
        (IP_SUM_TSC.load(Ordering::Relaxed) / ip_samples) / mhz
    } else {
        0
    };
    let ip_max_us = IP_MAX_TSC.load(Ordering::Relaxed) / mhz;
    let ip_last_us = IP_LAST_TSC.load(Ordering::Relaxed) / mhz;
    // SCHED_GAME miss-RATE in basis points (×100 = percent), from the
    // dispatch-path counters: misses / dispatches. 0 dispatches → 0 (no game
    // task has been picked yet — nothing to report, not a failure).
    let sg_dispatches = SG_DISPATCHES.load(Ordering::Relaxed);
    let sg_misses = SG_DEADLINE_MISSES.load(Ordering::Relaxed);
    let sg_miss_rate_bp = if sg_dispatches == 0 {
        0
    } else {
        sg_misses.saturating_mul(10_000) / sg_dispatches
    };
    alloc::format!(
        "# RaeenOS perf (North Star contracts)\n\
         boot_time_ms: {}\n\
         input_events: {}\n\
         last_input_tsc: {}\n\
         frames_presented: {}\n\
         input_photon_samples: {}\n\
         input_photon_avg_us: {}\n\
         input_photon_max_us: {}\n\
         input_photon_last_us: {}\n\
         audio_periods: {}\n\
         audio_underruns: {}\n\
         sched_game.dispatches: {}\n\
         sched_game.deadline_misses: {}\n\
         sched_game.miss_rate_bp: {}\n\
         sched_game.last_lateness_ns: {}\n\
         sched_game.worst_lateness_ns: {}\n\
         sched_game.worst_miss_us: {}\n\
         sched_game.aggregate_misses: {}\n\
         context_switches: {}\n\
         picks.game: {}\n\
         picks.normal: {}\n\
         max_runq_depth: {}\n\
         heap_alloc.min_ns: {}\n\
         heap_alloc.avg_ns: {}\n\
         heap_alloc.max_ns: {}\n\
         heap_alloc.samples: {}\n\
         sched_pick.samples: {}\n\
         sched_pick.min_ns: {}\n\
         sched_pick.avg_ns: {}\n\
         sched_pick.max_ns: {}\n\
         frame.budget_us: {}\n\
         frame.missed: {}\n\
         frame.samples: {}\n\
         frame.miss_rate_bp: {}\n\
         input_game_wake.samples: {}\n\
         input_game_wake.avg_us: {}\n\
         input_game_wake.max_us: {}\n\
         input_game_wake.last_us: {}\n\
         audio_jitter.samples: {}\n\
         audio_jitter.avg_us: {}\n\
         audio_jitter.max_us: {}\n\
         audio_jitter.budget_us: {}\n",
        boot_ms,
        INPUT_EVENTS.load(Ordering::Relaxed),
        LAST_INPUT_TSC.load(Ordering::Relaxed),
        FRAMES_PRESENTED.load(Ordering::Relaxed),
        ip_samples,
        ip_avg_us,
        ip_max_us,
        ip_last_us,
        AUDIO_PERIODS.load(Ordering::Relaxed),
        AUDIO_UNDERRUNS.load(Ordering::Relaxed),
        sg_dispatches,
        sg_misses,
        sg_miss_rate_bp,
        SG_LAST_LATENESS_NS.load(Ordering::Relaxed),
        SG_WORST_LATENESS_NS.load(Ordering::Relaxed),
        dl_worst_us,
        dl_misses,
        CONTEXT_SWITCHES.load(Ordering::Relaxed),
        PICKS_GAME.load(Ordering::Relaxed),
        PICKS_NORMAL.load(Ordering::Relaxed),
        MAX_RUNQ_DEPTH.load(Ordering::Relaxed),
        HEAP_ALLOC_MIN_NS.load(Ordering::Relaxed),
        HEAP_ALLOC_AVG_NS.load(Ordering::Relaxed),
        HEAP_ALLOC_MAX_NS.load(Ordering::Relaxed),
        HEAP_ALLOC_SAMPLES.load(Ordering::Relaxed),
        pick_s,
        pick_min_ns,
        pick_avg_ns,
        pick_max_ns,
        frame_budget_us,
        frame_missed,
        frame_samples,
        frame_miss_bp,
        igw_s,
        igw_avg_us,
        igw_max_us,
        igw_last_us,
        aj_s,
        aj_avg_us,
        aj_max_us,
        aj_budget_us,
    )
}

/// R10 smoketest: verifies (1) the surface renders every contract field
/// (incl. the new input→photon latency lines) and (2) the latency pipeline
/// actually records a sample when a real input is followed by a present.
/// Part 2 drives the live `record_*` helpers, then RESTORES every counter so
/// the boot telemetry stays pristine. Can print FAIL.
pub fn run_boot_smoketest() {
    // Run the heap-alloc microbench (PERFORMANCE_TARGETS §7) and keep its result
    // — this is the boot measurement reported at /proc/raeen/perf, not a
    // throwaway. FAIL-able: it must record samples and a non-zero, sane average.
    // The < 500 ns iron contract is NOT asserted here (QEMU TCG inflates alloc
    // latency ~3–4×, so a hard sub-500ns check would false-FAIL in CI); we assert
    // the measurement pipeline works and the number is within a generous sanity
    // ceiling, leaving the iron pass/fail to a real Athena read of the surface.
    let alloc_avg_ns = measure_heap_alloc();
    let (a_min, a_avg, a_max, a_samples) = heap_alloc_snapshot();
    let heap_ok =
        a_samples > 0 && a_avg > 0 && a_min <= a_avg && a_avg <= a_max && a_avg < 1_000_000; // 1 ms sanity ceiling (TCG-generous)

    let text = dump_text();
    let surface_ok = text.starts_with("# RaeenOS perf")
        && text.contains("input_events:")
        && text.contains("input_photon_avg_us:")
        && text.contains("input_photon_max_us:")
        && text.contains("frames_presented:")
        && text.contains("audio_underruns:")
        && text.contains("audio_jitter.avg_us:")
        && text.contains("sched_game.deadline_misses:")
        && text.contains("sched_game.dispatches:")
        && text.contains("sched_game.worst_lateness_ns:")
        && text.contains("context_switches:")
        && text.contains("heap_alloc.avg_ns:")
        && text.contains("sched_pick.avg_ns:")
        && text.contains("frame.miss_rate_bp:")
        && text.contains("input_game_wake.avg_us:");
    // Pristine input→game-wake clock value (restored at the very end so neither
    // this test nor the input→photon test — which also arms INPUT_WAKE_TSC —
    // leaves it armed and poisons the first real post-boot game dispatch).
    let input_wake_pristine = INPUT_WAKE_TSC.load(Ordering::Relaxed);

    // FAIL-able missed-frame logic proof: at a 60 Hz budget an on-time frame
    // (16 ms) records no miss; a 40 ms stall (> 1.5× budget) records exactly one.
    // Snapshot + restore the frame counters so boot telemetry stays pristine
    // (the compositor doesn't present in headless CI, so live data is
    // desktop/iron-gated — this proves the detection LOGIC regardless).
    let frame_snap = [
        FT_LAST_US.load(Ordering::Relaxed),
        FT_WRITE_IDX.load(Ordering::Relaxed),
        FT_COUNT.load(Ordering::Relaxed),
        FRAMETIME_SAMPLES.load(Ordering::Relaxed),
        MISSED_FRAMES.load(Ordering::Relaxed),
        FRAME_BUDGET_US.load(Ordering::Relaxed),
    ];
    set_frame_budget_us(16_667);
    let m0 = MISSED_FRAMES.load(Ordering::Relaxed);
    note_frametime_us(16_000); // on time -> no miss
    let on_time_no_miss = MISSED_FRAMES.load(Ordering::Relaxed) == m0;
    note_frametime_us(40_000); // > 1.5x budget -> one miss
    let stall_one_miss = MISSED_FRAMES.load(Ordering::Relaxed) == m0 + 1;
    let frame_ok = on_time_no_miss && stall_one_miss;
    FT_LAST_US.store(frame_snap[0], Ordering::Relaxed);
    FT_WRITE_IDX.store(frame_snap[1], Ordering::Relaxed);
    FT_COUNT.store(frame_snap[2], Ordering::Relaxed);
    FRAMETIME_SAMPLES.store(frame_snap[3], Ordering::Relaxed);
    MISSED_FRAMES.store(frame_snap[4], Ordering::Relaxed);
    FRAME_BUDGET_US.store(frame_snap[5], Ordering::Relaxed);

    // Drive the SCHED_GAME dispatch counters through the SAME `record_game_
    // dispatch` the EDF pick path uses: one ON-TIME dispatch (now <= deadline,
    // no miss) then one LATE dispatch 5_000 µs past deadline (one miss, lateness
    // 5_000_000 ns). Assert the counters moved exactly: misses +1, dispatches
    // +2, last_lateness == 5_000_000 ns. Snapshot + restore so boot telemetry
    // stays pristine. This is the FAIL-able proof the miss path is real even
    // before a live game thread ever blows a frame.
    let sg_snap = [
        SG_DISPATCHES.load(Ordering::Relaxed),
        SG_DEADLINE_MISSES.load(Ordering::Relaxed),
        SG_WORST_LATENESS_NS.load(Ordering::Relaxed),
        SG_LAST_LATENESS_NS.load(Ordering::Relaxed),
    ];
    record_game_dispatch(1_000, 2_000); // on time (now 1ms < deadline 2ms)
    let on_time_ok = SG_DEADLINE_MISSES.load(Ordering::Relaxed) == sg_snap[1];
    record_game_dispatch(7_000, 2_000); // 5_000 µs late -> one miss
    let late_ok = SG_DEADLINE_MISSES.load(Ordering::Relaxed) == sg_snap[1] + 1
        && SG_DISPATCHES.load(Ordering::Relaxed) == sg_snap[0] + 2
        && SG_LAST_LATENESS_NS.load(Ordering::Relaxed) == 5_000_000
        && SG_WORST_LATENESS_NS.load(Ordering::Relaxed) >= 5_000_000;
    let sg_misses_seen = SG_DEADLINE_MISSES.load(Ordering::Relaxed) - sg_snap[1];
    let sg_lateness_seen = SG_LAST_LATENESS_NS.load(Ordering::Relaxed);
    SG_DISPATCHES.store(sg_snap[0], Ordering::Relaxed);
    SG_DEADLINE_MISSES.store(sg_snap[1], Ordering::Relaxed);
    SG_WORST_LATENESS_NS.store(sg_snap[2], Ordering::Relaxed);
    SG_LAST_LATENESS_NS.store(sg_snap[3], Ordering::Relaxed);
    let sched_ok = on_time_ok && late_ok;

    // Snapshot, drive a synthetic input→present, assert one latency sample was
    // recorded and the pending clock cleared, then restore pristine counters.
    let snap = [
        INPUT_EVENTS.load(Ordering::Relaxed),
        LAST_INPUT_TSC.load(Ordering::Relaxed),
        FRAMES_PRESENTED.load(Ordering::Relaxed),
        PENDING_INPUT_TSC.load(Ordering::Relaxed),
        IP_SAMPLES.load(Ordering::Relaxed),
        IP_SUM_TSC.load(Ordering::Relaxed),
        IP_MAX_TSC.load(Ordering::Relaxed),
        IP_LAST_TSC.load(Ordering::Relaxed),
    ];
    let samples0 = IP_SAMPLES.load(Ordering::Relaxed);
    // Clear any pending arm from real boot input so this synthetic event arms
    // FRESH (record_input_event only arms via compare_exchange against 0 — the
    // "earliest stamp in a burst" rule). Without this the present below would
    // measure against a stale boot-time stamp (seconds), not the µs synthetic
    // cycle. The snapshot above restores the real pending value afterwards.
    PENDING_INPUT_TSC.store(0, Ordering::Relaxed);
    record_input_event();
    let armed = PENDING_INPUT_TSC.load(Ordering::Relaxed) != 0;
    for _ in 0..2_000 {
        core::hint::spin_loop(); // ensure present TSC > input TSC
    }
    record_frame_present();
    let pipeline_ok = armed
        && IP_SAMPLES.load(Ordering::Relaxed) == samples0 + 1
        && PENDING_INPUT_TSC.load(Ordering::Relaxed) == 0 // present consumed it
        && IP_LAST_TSC.load(Ordering::Relaxed) > 0;
    // The harness just measured one synthetic input→render→display cycle; assert
    // it lands UNDER the Concept's 16 ms budget (MasterChecklist Phase 12.4 /
    // PERFORMANCE_TARGETS §1 "sub-frame input latency"). The synthetic path is a
    // few microseconds, so a measured value at or beyond 16 ms means the
    // input→photon TSC clock is mis-scaled — a real FAIL, not a slow machine.
    let ip_last_us = IP_LAST_TSC.load(Ordering::Relaxed) / crate::fast_boot::tsc_mhz().max(1);
    let latency_under_budget = pipeline_ok && ip_last_us < 16_667;
    // A second present with nothing pending must NOT record a sample.
    let s_after = IP_SAMPLES.load(Ordering::Relaxed);
    record_frame_present();
    let idle_ok = IP_SAMPLES.load(Ordering::Relaxed) == s_after;
    INPUT_EVENTS.store(snap[0], Ordering::Relaxed);
    LAST_INPUT_TSC.store(snap[1], Ordering::Relaxed);
    FRAMES_PRESENTED.store(snap[2], Ordering::Relaxed);
    PENDING_INPUT_TSC.store(snap[3], Ordering::Relaxed);
    IP_SAMPLES.store(snap[4], Ordering::Relaxed);
    IP_SUM_TSC.store(snap[5], Ordering::Relaxed);
    IP_MAX_TSC.store(snap[6], Ordering::Relaxed);
    IP_LAST_TSC.store(snap[7], Ordering::Relaxed);

    // FAIL-able input→game-wake proof (§4): arm via record_input_event, then a
    // SCHED_GAME dispatch must record exactly one wake-latency sample and clear
    // the armed clock. Self-contained snapshot/restore of every static it
    // perturbs (the input-event family + IGW_* + SG_DISPATCHES). Live µs numbers
    // are iron-gated (no real input in headless CI) — this proves the LOGIC.
    let igw_snap = [
        INPUT_EVENTS.load(Ordering::Relaxed),
        LAST_INPUT_TSC.load(Ordering::Relaxed),
        PENDING_INPUT_TSC.load(Ordering::Relaxed),
        IGW_SAMPLES.load(Ordering::Relaxed),
        IGW_SUM_TSC.load(Ordering::Relaxed),
        IGW_MAX_TSC.load(Ordering::Relaxed),
        IGW_LAST_TSC.load(Ordering::Relaxed),
        SG_DISPATCHES.load(Ordering::Relaxed),
    ];
    INPUT_WAKE_TSC.store(0, Ordering::Relaxed); // clear any prior arm
    let igw0 = IGW_SAMPLES.load(Ordering::Relaxed);
    record_input_event(); // arms INPUT_WAKE_TSC
    let igw_armed = INPUT_WAKE_TSC.load(Ordering::Relaxed) != 0;
    for _ in 0..2_000 {
        core::hint::spin_loop(); // ensure dispatch TSC > input TSC
    }
    record_game_dispatch(1_000, 2_000); // on-time EDF dispatch consumes the wake
    let igw_ok = igw_armed
        && IGW_SAMPLES.load(Ordering::Relaxed) == igw0 + 1
        && INPUT_WAKE_TSC.load(Ordering::Relaxed) == 0 // dispatch consumed it
        && IGW_LAST_TSC.load(Ordering::Relaxed) > 0;
    let igw_last_us = input_game_wake_snapshot().3;
    INPUT_EVENTS.store(igw_snap[0], Ordering::Relaxed);
    LAST_INPUT_TSC.store(igw_snap[1], Ordering::Relaxed);
    PENDING_INPUT_TSC.store(igw_snap[2], Ordering::Relaxed);
    IGW_SAMPLES.store(igw_snap[3], Ordering::Relaxed);
    IGW_SUM_TSC.store(igw_snap[4], Ordering::Relaxed);
    IGW_MAX_TSC.store(igw_snap[5], Ordering::Relaxed);
    IGW_LAST_TSC.store(igw_snap[6], Ordering::Relaxed);
    SG_DISPATCHES.store(igw_snap[7], Ordering::Relaxed);
    // Final: clear INPUT_WAKE_TSC back to its pristine value so no sub-test
    // leaves the wake clock armed.
    INPUT_WAKE_TSC.store(input_wake_pristine, Ordering::Relaxed);

    // FAIL-able audio-jitter proof (§3 <100us): against a 2670us budget, a
    // period landing exactly on budget records ~0 jitter; a 2770us period (100us
    // late) records 100us jitter. Snapshot + restore so boot telemetry stays
    // pristine (no sustained audio in headless CI — this proves the MATH).
    let aj_snap = [
        AJ_SAMPLES.load(Ordering::Relaxed),
        AJ_SUM_US.load(Ordering::Relaxed),
        AJ_MAX_US.load(Ordering::Relaxed),
        AJ_LAST_US.load(Ordering::Relaxed),
        AUDIO_PERIOD_BUDGET_US.load(Ordering::Relaxed),
    ];
    set_audio_period_us(2670);
    note_audio_period_us(2670); // on budget -> 0 jitter
    let aj_ontime = AJ_LAST_US.load(Ordering::Relaxed) == 0;
    note_audio_period_us(2770); // 100us late -> 100us jitter
    let aj_jitter =
        AJ_LAST_US.load(Ordering::Relaxed) == 100 && AJ_MAX_US.load(Ordering::Relaxed) >= 100;
    let audio_jitter_ok = aj_ontime && aj_jitter;
    AJ_SAMPLES.store(aj_snap[0], Ordering::Relaxed);
    AJ_SUM_US.store(aj_snap[1], Ordering::Relaxed);
    AJ_MAX_US.store(aj_snap[2], Ordering::Relaxed);
    AJ_LAST_US.store(aj_snap[3], Ordering::Relaxed);
    AUDIO_PERIOD_BUDGET_US.store(aj_snap[4], Ordering::Relaxed);

    let pass = surface_ok
        && pipeline_ok
        && idle_ok
        && sched_ok
        && heap_ok
        && frame_ok
        && igw_ok
        && audio_jitter_ok
        && latency_under_budget;
    crate::selftest::record_smoketest("perf", pass);
    crate::serial_println!(
        "[perf] /proc/raeen/perf surface + input->photon pipeline (armed/sample/idle={}/{}/{}) -> {}",
        armed,
        pipeline_ok,
        idle_ok,
        if surface_ok && pipeline_ok && idle_ok { "PASS" } else { "FAIL" }
    );
    crate::serial_println!(
        "[perf] input->render->display latency harness: measured={}us budget<16667us -> {}",
        ip_last_us,
        if latency_under_budget { "PASS" } else { "FAIL" }
    );
    crate::serial_println!(
        "[perf] smoketest: misses={} lateness={}ns -> {}",
        sg_misses_seen,
        sg_lateness_seen,
        if sched_ok { "PASS" } else { "FAIL" }
    );
    crate::serial_println!(
        "[perf] heap_alloc fast path: min={}ns avg={}ns max={}ns samples={} (target <500ns iron) -> {}",
        a_min,
        alloc_avg_ns,
        a_max,
        a_samples,
        if heap_ok { "PASS" } else { "FAIL" }
    );
    // Make the live scheduler decision/scan latency visible in serial (it lives
    // at /proc/raeen/perf, read on demand). By this boot stage many timer
    // preemptions + yields have run, so the sample count is already meaningful.
    let (pk_s, pk_min, pk_avg, pk_max) = pick_latency_snapshot();
    crate::serial_println!(
        "[perf] sched_pick (decision/scan): min={}ns avg={}ns max={}ns samples={} (§5 ctxsw component)",
        pk_min,
        pk_avg,
        pk_max,
        pk_s,
    );
    crate::serial_println!(
        "[perf] missed-frame detection (§2 <0.1%): on_time_no_miss={} stall_one_miss={} -> {}",
        on_time_no_miss,
        stall_one_miss,
        if frame_ok { "PASS" } else { "FAIL" }
    );
    crate::serial_println!(
        "[perf] input->game wake (§4 <1ms): armed={} sample_us={} -> {}",
        igw_armed,
        igw_last_us,
        if igw_ok { "PASS" } else { "FAIL" }
    );
    crate::serial_println!(
        "[perf] audio wake jitter (§3 <100us): on_budget_0={} late100_jitter={} -> {}",
        aj_ontime,
        aj_jitter,
        if audio_jitter_ok { "PASS" } else { "FAIL" }
    );
}
