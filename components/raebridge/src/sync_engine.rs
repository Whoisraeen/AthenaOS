//! `sync_engine` — the cross-process wait/signal DRIVER + syscall accounting
//! (broker §6.1, Slice 2b host half).
//!
//! Concept §Compatibility ("apps run naturally"): a multi-process Windows app —
//! Steam included — lives or dies on cheap `WaitForSingleObject`/`SetEvent`. Wine
//! learned this the hard way: the wineserver round-trip on every wait was the
//! perf cliff, and `fsync` fixed it by moving the *uncontended* op to a raw futex
//! word with **zero** syscalls. RaeBridge holds that same contract.
//!
//! [`crate::broker`] supplies the DECISIONS — [`SharedSyncState::wait_prepare`]
//! and [`SharedSyncState::wake_count`] decide *whether* to cross the kernel
//! boundary. This module is the DRIVER that turns those decisions into (at most)
//! one `SYS_FUTEX` call and **counts every crossing**. The fsync-parity contract
//! (`docs/components/raebridge-process-model.md` §"fast-path") becomes a
//! FAIL-able number here: [`SyncAccount::uncontended_op_syscalls`] MUST stay 0
//! across any number of uncontended ops (Invariant 1/4). Nonzero = a hot-path
//! syscall leak — the precise regression the spec exists to prevent.
//!
//! The kernel boundary — `SYS_FUTEX` (258), re-keyed to the shared-page physical
//! identity (Invariant 2) and with the `expected`-compare fix (Invariant 3) — is
//! abstracted behind [`FutexOps`] so the FULL wait→park→wake→resume rendezvous is
//! host-provable today: the boot smoketest drives it deterministically, and the
//! host test drives it with two real OS threads through a `Condvar` parking lot
//! (which exercises the lost-wakeup race Invariant 3 guards against). The kernel
//! half — `SYS_FUTEX` (258) re-keyed to shared-frame physical identity + a real
//! blocking `futex_wait` (`BlockedOnFutex`) — LANDED (MasterChecklist item 1828,
//! kernel `sync.rs` `FUTEX_MANAGER`) under explicit owner authorization lifting
//! the `raebridge-wine-strategy.md` §8 gate. Wiring `SyscallFutexOps` to issue
//! the syscall from the guest is the remaining userspace step.

use crate::broker::{SharedSyncState, WaitPrep, WAKE_ALL};
use crate::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use alloc::string::String;
use core::sync::atomic::{AtomicU64, Ordering};

/// The kernel boundary a contended wait/signal crosses. The real implementation
/// is `SYS_FUTEX` (258) on `&SharedSyncState::futex` (offset 0 of the shared
/// page); the test implementations are an in-memory parking lot. Abstracting it
/// is what makes the whole rendezvous host-provable without the kernel half.
pub trait FutexOps {
    /// `SYS_FUTEX(WAIT, &word, expected)`: block until `*word != expected` or a
    /// wake targets `word`. Returns when the caller should re-check object state.
    /// Counts as exactly one syscall. The `expected`-compare under the wait-queue
    /// lock (Invariant 3) is what makes a signal landing between the decision and
    /// the block race-free — a stale `expected` returns immediately, no lost wake.
    fn futex_wait(&self, word: &core::sync::atomic::AtomicU32, expected: u32);

    /// `SYS_FUTEX(WAKE, &word, count)`: wake up to `count` parked waiters
    /// (`WAKE_ALL` = all). Counts as exactly one syscall.
    fn futex_wake(&self, word: &core::sync::atomic::AtomicU32, count: u32);
}

/// Bound on the contended wait loop so a logic regression (a wake that never
/// lands, an `expected`-compare bug) returns `WAIT_TIMEOUT` — a FAIL the test/
/// boot can see — instead of deadlocking the caller. Generous: a correct wake
/// resolves on the first re-check, so the loop body normally runs once.
pub const MAX_WAIT_SPINS: u32 = 1024;

/// Live syscall accounting for the sync fast path — the proof that "fsync-parity"
/// is real, not vibes (CLAUDE.md rule 8: a perf claim needs a counter). One
/// instance backs `/proc/raeen/raebridge_syncbroker`; the smoketest asserts the
/// headline invariant against it.
#[derive(Default)]
pub struct SyncAccount {
    /// Every wait/signal op driven through this engine.
    ops_total: AtomicU64,
    /// Ops that completed on the fast path (no waiter parked / already signaled).
    uncontended_ops: AtomicU64,
    /// **The headline FAIL-able number.** `SYS_FUTEX` syscalls charged to an op
    /// the engine classified as uncontended. MUST stay 0 — a fast-path op that
    /// touches the kernel is an Invariant 1/4 violation (the fsync perf cliff).
    uncontended_op_syscalls: AtomicU64,
    /// `SYS_FUTEX(WAIT)` calls issued (contended waits only).
    futex_waits: AtomicU64,
    /// `SYS_FUTEX(WAKE)` calls issued (contended signals only).
    futex_wakes: AtomicU64,
    /// Broker IPC round-trips on a wait/signal op. MUST stay 0 — the broker is
    /// consulted only on create/open/close, never on the steady-state hot path
    /// (Invariant 1). Kept for symmetry with the acceptance criteria; the engine
    /// never touches the broker, so this proves the absence structurally.
    broker_ipc_on_sync_op: AtomicU64,
}

impl SyncAccount {
    pub fn new() -> Self {
        Self::default()
    }

    fn futex_calls(&self) -> u64 {
        self.futex_waits.load(Ordering::Relaxed) + self.futex_wakes.load(Ordering::Relaxed)
    }

    /// Mark an op that took the fast path, and — the load-bearing check — charge
    /// `uncontended_op_syscalls` if it nonetheless crossed the kernel boundary
    /// (it must not). `before` is [`Self::futex_calls`] captured at op entry.
    fn record_uncontended(&self, before: u64) {
        self.uncontended_ops.fetch_add(1, Ordering::Relaxed);
        let after = self.futex_calls();
        if after > before {
            self.uncontended_op_syscalls
                .fetch_add(after - before, Ordering::Relaxed);
        }
    }

    pub fn ops_total(&self) -> u64 {
        self.ops_total.load(Ordering::Relaxed)
    }
    pub fn uncontended_ops(&self) -> u64 {
        self.uncontended_ops.load(Ordering::Relaxed)
    }
    /// The headline number (must be 0). See the field docstring.
    pub fn uncontended_op_syscalls(&self) -> u64 {
        self.uncontended_op_syscalls.load(Ordering::Relaxed)
    }
    pub fn futex_waits(&self) -> u64 {
        self.futex_waits.load(Ordering::Relaxed)
    }
    pub fn futex_wakes(&self) -> u64 {
        self.futex_wakes.load(Ordering::Relaxed)
    }
    pub fn broker_ipc_on_sync_op(&self) -> u64 {
        self.broker_ipc_on_sync_op.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// The driver — wait + the three signal ops, each charging the account exactly
// for the kernel crossings it makes (zero on the fast path).
// ---------------------------------------------------------------------------

/// `WaitForSingleObject` on a cross-process object. Returns `WAIT_OBJECT_0` when
/// the object is acquired, `WAIT_TIMEOUT` if the bounded retry loop is exhausted
/// (hang-detect). The uncontended case returns with ZERO syscalls; only a true
/// block touches `futex_wait`.
pub fn wait<F: FutexOps>(state: &SharedSyncState, tid: u32, f: &F, acct: &SyncAccount) -> u32 {
    acct.ops_total.fetch_add(1, Ordering::Relaxed);
    let before = acct.futex_calls();
    match state.wait_prepare(tid) {
        WaitPrep::Acquired => {
            acct.record_uncontended(before);
            WAIT_OBJECT_0
        }
        WaitPrep::Block { mut expected } => {
            // A waiter is already published (wait_prepare's Block arm). Park until
            // the word changes, re-checking each wake. wait_finish retracts the
            // single published waiter on every exit path.
            let mut spins = 0u32;
            loop {
                f.futex_wait(&state.futex, expected);
                acct.futex_waits.fetch_add(1, Ordering::Relaxed);
                if state.try_wait(tid) {
                    state.wait_finish();
                    return WAIT_OBJECT_0;
                }
                spins += 1;
                if spins >= MAX_WAIT_SPINS {
                    state.wait_finish();
                    return WAIT_TIMEOUT;
                }
                expected = state.futex.load(Ordering::Acquire);
            }
        }
    }
}

/// `SetEvent`. Returns the number of waiters actually woken (0 = wake elided —
/// nobody was parked, so no syscall). Auto-reset wakes one; manual-reset wakes
/// all parked.
pub fn set_event<F: FutexOps>(state: &SharedSyncState, f: &F, acct: &SyncAccount) -> u32 {
    signal(state, f, acct, state.event_set())
}

/// `ResetEvent` — pure state, never wakes, never syscalls. Always uncontended.
pub fn reset_event(state: &SharedSyncState, acct: &SyncAccount) {
    acct.ops_total.fetch_add(1, Ordering::Relaxed);
    let before = acct.futex_calls();
    state.event_reset();
    acct.record_uncontended(before);
}

/// `ReleaseMutex` by `tid`. Wakes one parked waiter on full release (0 while the
/// owner still holds it recursively). `Err` if `tid` is not the owner.
pub fn release_mutex<F: FutexOps>(
    state: &SharedSyncState,
    tid: u32,
    f: &F,
    acct: &SyncAccount,
) -> Result<u32, ()> {
    let intended = state.mutex_release(tid)?;
    Ok(signal(state, f, acct, intended))
}

/// `ReleaseSemaphore(n)`. Wakes up to `n` parked waiters. `Err` if it would
/// exceed `max_count` (`ERROR_TOO_MANY_POSTS`).
pub fn release_semaphore<F: FutexOps>(
    state: &SharedSyncState,
    n: i32,
    f: &F,
    acct: &SyncAccount,
) -> Result<u32, ()> {
    let intended = state.sem_release(n)?;
    Ok(signal(state, f, acct, intended))
}

/// Shared signal tail: given the `intended` wake count a transition produced,
/// apply wake-elision and issue the `futex_wake` only if a waiter is actually
/// parked. The state transition has already happened (its `futex` Release store
/// precedes the `SeqCst` `parked()` read inside `wake_count`).
fn signal<F: FutexOps>(state: &SharedSyncState, f: &F, acct: &SyncAccount, intended: u32) -> u32 {
    acct.ops_total.fetch_add(1, Ordering::Relaxed);
    let before = acct.futex_calls();
    let n = state.wake_count(intended);
    if n == 0 {
        // No waiter parked — the signal stays entirely in userspace.
        acct.record_uncontended(before);
        0
    } else {
        f.futex_wake(&state.futex, n);
        acct.futex_wakes.fetch_add(1, Ordering::Relaxed);
        if n == WAKE_ALL {
            WAKE_ALL
        } else {
            n
        }
    }
}

// ---------------------------------------------------------------------------
// Deterministic, no_std-safe self-test (boot smoketest + host KAT)
// ---------------------------------------------------------------------------

/// Single-thread [`FutexOps`] for the deterministic self-test. `futex_wait`
/// models "the other actor runs while we are parked": on the first park it stores
/// `1` into the futex word (an auto-reset event becoming signaled), so the
/// driver's re-check succeeds on the next iteration. Counts every call. No real
/// blocking — this runs on the single boot CPU and on the host.
struct ScriptedFutex {
    waits: core::cell::Cell<u64>,
    wakes: core::cell::Cell<u64>,
    /// When true, the first `futex_wait` signals the word (models a concurrent
    /// `SetEvent` landing on the parked waiter).
    signal_on_first_park: core::cell::Cell<bool>,
}

impl ScriptedFutex {
    fn new(signal_on_first_park: bool) -> Self {
        Self {
            waits: core::cell::Cell::new(0),
            wakes: core::cell::Cell::new(0),
            signal_on_first_park: core::cell::Cell::new(signal_on_first_park),
        }
    }
}

impl FutexOps for ScriptedFutex {
    fn futex_wait(&self, word: &core::sync::atomic::AtomicU32, _expected: u32) {
        self.waits.set(self.waits.get() + 1);
        if self.signal_on_first_park.replace(false) {
            word.store(1, Ordering::Release);
        }
    }
    fn futex_wake(&self, _word: &core::sync::atomic::AtomicU32, _count: u32) {
        self.wakes.set(self.wakes.get() + 1);
    }
}

/// FAIL-able self-test of the sync engine (broker §6.1, Slice 2b). Proves the
/// fsync-parity acceptance criteria deterministically:
///   * a batch of uncontended ops issues ZERO syscalls (`uncontended_op_syscalls
///     == 0`, `broker_ipc_on_sync_op == 0`) — the headline number;
///   * wake-elision: `SetEvent` with nobody parked makes no `futex_wake`;
///   * an end-to-end rendezvous through the driver loop — a waiter parks on an
///     unsignaled auto-event and resumes with `WAIT_OBJECT_0` when the signal
///     lands while it is parked (bounded, so a regression returns `WAIT_TIMEOUT`
///     not a hang);
///   * a parked waiter draws a real `futex_wake` (not elided), charged to a
///     contended op, never to `uncontended_op_syscalls`.
/// Returns false on any wrong result.
pub fn run_sync_engine_self_test() -> bool {
    let mut ok = true;

    // --- 1. Uncontended batch: zero syscalls across every op kind. ----------
    {
        let f = ScriptedFutex::new(false);
        let acct = SyncAccount::new();

        // Event: set (no waiter) then a satisfied wait, then reset.
        let ev = SharedSyncState::init_event(false, false);
        ok &= set_event(&ev, &f, &acct) == 0; // nobody parked -> wake elided
        ok &= wait(&ev, 0, &f, &acct) == WAIT_OBJECT_0; // signaled -> fast acquire
        reset_event(&ev, &acct);

        // Mutex: acquire (fast), release (no waiter), re-acquire.
        let mx = SharedSyncState::init_mutex(0);
        ok &= wait(&mx, 7, &f, &acct) == WAIT_OBJECT_0;
        ok &= release_mutex(&mx, 7, &f, &acct) == Ok(0); // no waiter -> elided
        ok &= wait(&mx, 7, &f, &acct) == WAIT_OBJECT_0;

        // Semaphore: acquire one unit (fast), release it back.
        let sm = SharedSyncState::init_semaphore(1, 4);
        ok &= wait(&sm, 0, &f, &acct) == WAIT_OBJECT_0;
        ok &= release_semaphore(&sm, 1, &f, &acct) == Ok(0); // no waiter -> elided

        // The headline assertions: NOT ONE syscall, NOT ONE broker IPC.
        ok &= acct.uncontended_op_syscalls() == 0;
        ok &= acct.broker_ipc_on_sync_op() == 0;
        ok &= acct.futex_waits() == 0 && acct.futex_wakes() == 0;
        ok &= f.waits.get() == 0 && f.wakes.get() == 0;
        ok &= acct.uncontended_ops() == acct.ops_total(); // every op was fast
    }

    // --- 2. Wake-elision on a signal with no parked waiter. -----------------
    {
        let f = ScriptedFutex::new(false);
        let acct = SyncAccount::new();
        let ev = SharedSyncState::init_event(false, false);
        ok &= set_event(&ev, &f, &acct) == 0;
        ok &= f.wakes.get() == 0; // elided: NO SYS_FUTEX(WAKE)
        ok &= acct.uncontended_op_syscalls() == 0;
    }

    // --- 3. End-to-end rendezvous through the driver loop. ------------------
    //     A waits on an unsignaled auto-event -> parks -> the scripted futex
    //     models B's SetEvent landing while A is parked -> A resumes WAIT_OBJECT_0
    //     after exactly one futex_wait, and the slow path is NOT charged to the
    //     uncontended counter.
    {
        let f = ScriptedFutex::new(true); // signal lands on the first park
        let acct = SyncAccount::new();
        let ev = SharedSyncState::init_event(false, false);
        ok &= wait(&ev, 0, &f, &acct) == WAIT_OBJECT_0; // parks, then wakes
        ok &= f.waits.get() == 1; // exactly one block
        ok &= acct.futex_waits() == 1;
        ok &= ev.parked() == 0; // waiter retracted
        ok &= acct.uncontended_op_syscalls() == 0; // contended op not miscounted
        ok &= acct.uncontended_ops() == 0; // it was a true block, not fast
    }

    // --- 4. A parked waiter draws a REAL wake (not elided). -----------------
    {
        let f = ScriptedFutex::new(false);
        let acct = SyncAccount::new();
        let ev = SharedSyncState::init_event(false, false);
        // Park a waiter by hand (wait_prepare publishes it), then signal.
        ok &= matches!(ev.wait_prepare(0), WaitPrep::Block { .. });
        ok &= ev.parked() == 1;
        ok &= set_event(&ev, &f, &acct) == 1; // a waiter is parked -> wake fires
        ok &= f.wakes.get() == 1; // a real SYS_FUTEX(WAKE)
        ok &= acct.futex_wakes() == 1;
        ok &= acct.uncontended_op_syscalls() == 0; // the wake belongs to a contended op
        ev.wait_finish();
    }

    // --- 5. Manual-reset wakes ALL parked; auto-reset wakes one. ------------
    {
        let f = ScriptedFutex::new(false);
        let acct = SyncAccount::new();
        let mev = SharedSyncState::init_event(true, false); // manual-reset
        ok &= matches!(mev.wait_prepare(0), WaitPrep::Block { .. });
        ok &= set_event(&mev, &f, &acct) == WAKE_ALL;
        ok &= f.wakes.get() == 1;
        mev.wait_finish();
    }

    ok
}

/// `/proc/raeen/raebridge_syncbroker` — the cross-process sync engine's capability
/// surface and the LIVE headline counter from a fresh self-test run. Concept
/// §Compatibility: the fsync-parity claim has to be measurable in procfs, not
/// just asserted. The counters printed here are produced by re-running the
/// uncontended batch, so a regression shows a nonzero `uncontended_op_syscalls`.
pub fn sync_engine_self_test_text() -> String {
    use core::fmt::Write;

    // Re-run the uncontended batch on a fresh account so the printed counters are
    // live, not stale — exactly the headline measurement.
    let f = ScriptedFutex::new(false);
    let acct = SyncAccount::new();
    let ev = SharedSyncState::init_event(false, false);
    let _ = set_event(&ev, &f, &acct);
    let _ = wait(&ev, 0, &f, &acct);
    reset_event(&ev, &acct);
    let mx = SharedSyncState::init_mutex(0);
    let _ = wait(&mx, 1, &f, &acct);
    let _ = release_mutex(&mx, 1, &f, &acct);
    let sm = SharedSyncState::init_semaphore(2, 4);
    let _ = wait(&sm, 0, &f, &acct);
    let _ = release_semaphore(&sm, 1, &f, &acct);

    let ok = run_sync_engine_self_test();

    let mut s = String::new();
    let _ = writeln!(
        s,
        "RaeBridge cross-process sync engine (broker §6.1, Slice 2b host half)"
    );
    let _ = writeln!(s, "self_test: {}", if ok { "PASS" } else { "FAIL" });
    let _ = writeln!(
        s,
        "model: SharedSyncState atomics + wake-elision; SYS_FUTEX only when parked"
    );
    let _ = writeln!(
        s,
        "fsync-parity counters (live, from an uncontended batch of {} ops):",
        acct.ops_total()
    );
    let _ = writeln!(
        s,
        "  uncontended_op_syscalls = {}  (headline; MUST be 0)",
        acct.uncontended_op_syscalls()
    );
    let _ = writeln!(
        s,
        "  broker_ipc_on_sync_op   = {}  (MUST be 0 on wait/signal)",
        acct.broker_ipc_on_sync_op()
    );
    let _ = writeln!(
        s,
        "  futex_waits = {}  futex_wakes = {}  uncontended_ops = {}/{}",
        acct.futex_waits(),
        acct.futex_wakes(),
        acct.uncontended_ops(),
        acct.ops_total()
    );
    let _ = writeln!(
        s,
        "invariants: 1=uncontended-zero-syscall 2=phys-page-key 3=expected-compare 4=wake-elision"
    );
    let _ = writeln!(
        s,
        "kernel half (LANDED, item 1828): SYS_FUTEX 258 phys-frame re-key + real blocking futex_wait"
    );
    s
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;

    #[test]
    fn engine_self_test_passes() {
        assert!(
            run_sync_engine_self_test(),
            "sync engine self-test (the boot/host proof) regressed"
        );
    }

    #[test]
    fn uncontended_batch_issues_zero_syscalls() {
        let f = ScriptedFutex::new(false);
        let acct = SyncAccount::new();
        let ev = SharedSyncState::init_event(false, true); // signaled
        for _ in 0..1000 {
            assert_eq!(wait(&ev, 0, &f, &acct), WAIT_OBJECT_0);
            // auto-reset consumed the signal; re-arm without a parked waiter.
            assert_eq!(set_event(&ev, &f, &acct), 0, "no waiter -> wake elided");
        }
        assert_eq!(
            acct.uncontended_op_syscalls(),
            0,
            "1000 uncontended ops must not touch the kernel (fsync parity)"
        );
        assert_eq!(acct.futex_waits() + acct.futex_wakes(), 0);
    }

    #[test]
    fn timeout_is_bounded_not_a_hang() {
        // A futex that never signals: the driver must give up at MAX_WAIT_SPINS
        // and return WAIT_TIMEOUT (a visible FAIL), never deadlock.
        struct DeadFutex;
        impl FutexOps for DeadFutex {
            fn futex_wait(&self, _w: &core::sync::atomic::AtomicU32, _e: u32) {}
            fn futex_wake(&self, _w: &core::sync::atomic::AtomicU32, _n: u32) {}
        }
        let acct = SyncAccount::new();
        let ev = SharedSyncState::init_event(false, false); // unsignaled forever
        assert_eq!(wait(&ev, 0, &DeadFutex, &acct), WAIT_TIMEOUT);
        assert_eq!(ev.parked(), 0, "waiter retracted on timeout");
        assert_eq!(acct.futex_waits(), MAX_WAIT_SPINS as u64);
    }

    // ---- The real proof: a genuine two-thread cross-process rendezvous -----
    //
    // A `Condvar`-backed parking lot stands in for the kernel's futex queue. One
    // thread blocks in `wait`, another `SetEvent`s it; the blocked thread must
    // resume with WAIT_OBJECT_0. The `expected`-compare under the lock is what
    // makes this lost-wakeup-safe (Invariant 3) — the exact race the live kernel
    // half must preserve.

    struct ParkingLotFutex {
        m: std::sync::Mutex<()>,
        cv: std::sync::Condvar,
        waits: AtomicU64,
        wakes: AtomicU64,
    }
    impl ParkingLotFutex {
        fn new() -> Self {
            Self {
                m: std::sync::Mutex::new(()),
                cv: std::sync::Condvar::new(),
                waits: AtomicU64::new(0),
                wakes: AtomicU64::new(0),
            }
        }
    }
    impl FutexOps for ParkingLotFutex {
        fn futex_wait(&self, word: &core::sync::atomic::AtomicU32, expected: u32) {
            self.waits.fetch_add(1, Ordering::Relaxed);
            let guard = self.m.lock().unwrap();
            // Invariant 3: re-check under the lock. A signal that stored a new
            // value (and notified) before we took the lock makes this false, so
            // we return immediately rather than sleeping on a stale value.
            if word.load(Ordering::Acquire) == expected {
                let _g = self
                    .cv
                    .wait_timeout(guard, std::time::Duration::from_secs(5))
                    .unwrap();
            }
        }
        fn futex_wake(&self, _word: &core::sync::atomic::AtomicU32, _count: u32) {
            self.wakes.fetch_add(1, Ordering::Relaxed);
            let _guard = self.m.lock().unwrap();
            self.cv.notify_all();
        }
    }

    #[test]
    fn cross_process_rendezvous_two_real_threads() {
        use std::sync::Arc;
        let ev = Arc::new(SharedSyncState::init_event(false, false)); // auto, unsignaled
        let f = Arc::new(ParkingLotFutex::new());
        let acct = Arc::new(SyncAccount::new());

        let (ev_a, f_a, acct_a) = (ev.clone(), f.clone(), acct.clone());
        let waiter = std::thread::spawn(move || wait(&ev_a, 0, &*f_a, &acct_a));

        // Wait until A has published its parked waiter, so B's wake is NOT elided
        // — this makes the woken-count deterministic. Bounded so a regression
        // fails the assert instead of spinning forever.
        let mut guard = 0u64;
        while ev.parked() == 0 {
            std::thread::yield_now();
            guard += 1;
            assert!(guard < 50_000_000, "the waiter never parked (decision bug)");
        }

        let woke = set_event(&ev, &*f, &acct);
        assert_eq!(
            woke, 1,
            "B must wake exactly the one parked auto-event waiter"
        );

        let result = waiter.join().unwrap();
        assert_eq!(result, WAIT_OBJECT_0, "A must wake with WAIT_OBJECT_0");
        assert_eq!(
            acct.uncontended_op_syscalls(),
            0,
            "the rendezvous must not leak a fast-path syscall"
        );
        assert_eq!(acct.futex_wakes(), 1, "exactly one real SYS_FUTEX(WAKE)");
        assert_eq!(ev.parked(), 0, "the waiter retracted after waking");
    }

    #[test]
    fn manual_reset_wakes_all_real_threads() {
        use std::sync::Arc;
        let ev = Arc::new(SharedSyncState::init_event(true, false)); // manual-reset
        let f = Arc::new(ParkingLotFutex::new());
        let acct = Arc::new(SyncAccount::new());

        let mut handles = alloc::vec::Vec::new();
        for _ in 0..4 {
            let (e, ff, a) = (ev.clone(), f.clone(), acct.clone());
            handles.push(std::thread::spawn(move || wait(&e, 0, &*ff, &a)));
        }
        // Wait for all four to park.
        let mut guard = 0u64;
        while ev.parked() < 4 {
            std::thread::yield_now();
            guard += 1;
            assert!(guard < 50_000_000, "not all waiters parked");
        }
        let woke = set_event(&ev, &*f, &acct);
        assert_eq!(woke, WAKE_ALL, "manual-reset must wake all parked");
        for h in handles {
            assert_eq!(h.join().unwrap(), WAIT_OBJECT_0, "every waiter wakes");
        }
        assert_eq!(acct.uncontended_op_syscalls(), 0);
    }
}
