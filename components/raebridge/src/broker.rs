//! `raebridge_server` — cross-process synchronization-object namespace (broker §6.1).
//!
//! Concept §Compatibility ("apps run naturally"): a multi-process Windows app —
//! and Steam itself — needs a `Global\Name` mutex/event/semaphore to be *shared
//! across processes*. The per-process [`crate::SyncObject`] store (lib.rs) handles
//! one `.exe`'s own threads; this is the cross-process half. See
//! `docs/components/raebridge-server-design.md`.
//!
//! This module is the AUTHORITATIVE cross-process *namespace*: it maps a named
//! object (`Global\Foo`) to a broker object id + the shared-state-page id that
//! backs it, and owns the object's lifetime (refcounted across every process
//! that opened it). Unnamed / process-local objects never come here — they stay
//! in the per-process store. The daemon consults this namespace only on
//! open/create; the steady-state wait/signal is a direct `SYS_FUTEX` on the
//! shared page (Slice 2+), with no broker round-trip.
//!
//! ## Slice 1 scope (this file)
//! The namespace state machine as PURE LOGIC — `create`/`open`/`close` with a
//! refcount, a kind type-check, and the cross-process sharing invariant (two
//! processes naming the same object get the SAME page id). Host-KAT-provable
//! now, independent of the daemon process and the `SYS_CHANNEL_SHMEM_MAP` /
//! `SYS_FUTEX` plumbing (Slice 2+). The `page_id` here is a logical id; the
//! daemon binds it to a real shared region when it allocates the page.

use alloc::collections::BTreeMap;
use alloc::string::String;

/// Kind of waitable object a broker entry represents. A name cannot be reused
/// across kinds — `CreateEventW("Global\Foo")` then `CreateMutexW("Global\Foo")`
/// is a Win32 `ERROR_INVALID_HANDLE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokerKind {
    Mutex,
    Event,
    Semaphore,
}

/// One cross-process object whose lifetime the broker owns.
struct BrokerEntry {
    kind: BrokerKind,
    /// Open handles across ALL processes. The entry (and its name) is freed when
    /// this reaches zero, and the daemon releases the shared page.
    refs: u32,
    /// Logical id of the shared state page backing this object. The daemon binds
    /// it to a real `SYS_CHANNEL_SHMEM_MAP` region (Slice 2); the futex word for
    /// every waiter lives at offset 0 of that page.
    page_id: u64,
}

/// Result of a named create — mirrors Win32 `CreateMutex/Event/Semaphore`.
#[derive(Debug, PartialEq, Eq)]
pub enum BrokerCreate {
    /// Brand-new object. `page_id` is the freshly-allocated shared page.
    Created { object_id: u64, page_id: u64 },
    /// A same-name, same-kind object already existed — a new handle to it, on
    /// the SAME page (the cross-process sharing invariant). Win32 sets
    /// `ERROR_ALREADY_EXISTS` but still returns the handle.
    OpenedExisting { object_id: u64, page_id: u64 },
    /// The name exists with a *different* kind. Win32 fails with
    /// `ERROR_INVALID_HANDLE` and returns no handle.
    TypeMismatch,
}

/// The broker's global named-object namespace. One instance lives in the
/// `raebridge_server` daemon; every AthBridge process talks to it on
/// open/create only.
pub struct BrokerNamespace {
    by_name: BTreeMap<String, u64>,
    objects: BTreeMap<u64, BrokerEntry>,
    next_object_id: u64,
    next_page_id: u64,
}

impl Default for BrokerNamespace {
    fn default() -> Self {
        Self::new()
    }
}

impl BrokerNamespace {
    pub fn new() -> Self {
        Self {
            by_name: BTreeMap::new(),
            objects: BTreeMap::new(),
            // 0 is reserved as the invalid id for both spaces.
            next_object_id: 1,
            next_page_id: 1,
        }
    }

    /// `CreateMutex/Event/SemaphoreW` on a NAMED object. Creates a fresh entry
    /// (allocating a shared page) or, if a same-name same-kind object already
    /// exists, hands back a new reference to it on its existing page. A name
    /// collision across kinds is rejected without side effects.
    pub fn create(&mut self, name: &str, kind: BrokerKind) -> BrokerCreate {
        if let Some(&object_id) = self.by_name.get(name) {
            // Existing name — must match kind, else reject untouched.
            let entry = match self.objects.get_mut(&object_id) {
                Some(e) if e.kind == kind => e,
                _ => return BrokerCreate::TypeMismatch,
            };
            entry.refs += 1;
            return BrokerCreate::OpenedExisting {
                object_id,
                page_id: entry.page_id,
            };
        }

        let object_id = self.next_object_id;
        self.next_object_id += 1;
        let page_id = self.next_page_id;
        self.next_page_id += 1;
        self.objects.insert(
            object_id,
            BrokerEntry {
                kind,
                refs: 1,
                page_id,
            },
        );
        self.by_name.insert(String::from(name), object_id);
        BrokerCreate::Created { object_id, page_id }
    }

    /// `OpenMutex/Event/SemaphoreW`: an EXISTING named object of `kind`. Returns
    /// `(object_id, page_id)` and bumps the refcount, or `None` if no such named
    /// object of that kind exists (Win32 `ERROR_FILE_NOT_FOUND`).
    pub fn open(&mut self, name: &str, kind: BrokerKind) -> Option<(u64, u64)> {
        let &object_id = self.by_name.get(name)?;
        let entry = self.objects.get_mut(&object_id)?;
        if entry.kind != kind {
            return None;
        }
        entry.refs += 1;
        Some((object_id, entry.page_id))
    }

    /// `CloseHandle` on a broker object. Decrements the refcount; when the last
    /// handle across all processes closes, frees the entry + its name and returns
    /// `true` so the daemon releases the shared page. Returns `false` while other
    /// references remain, or if `object_id` is unknown.
    pub fn close(&mut self, object_id: u64) -> bool {
        let drop_it = match self.objects.get_mut(&object_id) {
            Some(e) => {
                e.refs = e.refs.saturating_sub(1);
                e.refs == 0
            }
            None => return false,
        };
        if drop_it {
            if let Some(e) = self.objects.remove(&object_id) {
                // Free the name that pointed at this object (if still mapped here).
                let _ = e;
                self.by_name.retain(|_, &mut id| id != object_id);
            }
        }
        drop_it
    }

    /// Number of live cross-process objects (test/`/proc` introspection).
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    /// The shared page backing an object, if it is live.
    pub fn page_id(&self, object_id: u64) -> Option<u64> {
        self.objects.get(&object_id).map(|e| e.page_id)
    }
}

/// FAIL-able self-test of the cross-process namespace (broker §6.1, Slice 1).
/// Drives the load-bearing invariants — two processes naming the same object
/// share ONE page, kind-collision is rejected, refcounted free — returning
/// `false` on any wrong result. Boot-wireable and host-KAT'd.
pub fn run_namespace_self_test() -> bool {
    let mut ns = BrokerNamespace::new();
    let mut ok = true;

    // Process A creates Global\Evt (event).
    let (a_id, a_page) = match ns.create("Global\\Evt", BrokerKind::Event) {
        BrokerCreate::Created { object_id, page_id } => (object_id, page_id),
        _ => return false,
    };
    ok &= a_id != 0 && a_page != 0;

    // Process B creates the SAME name+kind -> opens the existing object on the
    // SAME page. This is the whole point: cross-process sharing.
    match ns.create("Global\\Evt", BrokerKind::Event) {
        BrokerCreate::OpenedExisting { object_id, page_id } => {
            ok &= object_id == a_id && page_id == a_page;
        }
        _ => ok = false,
    }

    // Process C opens it by name -> same object, same page.
    match ns.open("Global\\Evt", BrokerKind::Event) {
        Some((id, page)) => ok &= id == a_id && page == a_page,
        None => ok = false,
    }

    // Same name, wrong kind -> TypeMismatch, no side effects.
    ok &= ns.create("Global\\Evt", BrokerKind::Mutex) == BrokerCreate::TypeMismatch;
    // Opening a never-created name -> None.
    ok &= ns.open("Global\\Nope", BrokerKind::Event).is_none();

    // Three open handles (A create, B create, C open) -> only the last close
    // frees the page.
    ok &= !ns.close(a_id); // 3 -> 2
    ok &= !ns.close(a_id); // 2 -> 1
    ok &= ns.close(a_id); //  1 -> 0, release page
                          // After the last close the name is gone and reopen fails.
    ok &= ns.open("Global\\Evt", BrokerKind::Event).is_none();
    ok &= ns.object_count() == 0;

    ok
}

/// One-line `/proc/raeen/*` snapshot of the broker namespace (Slice 1).
pub fn namespace_self_test_text() -> String {
    let ok = run_namespace_self_test();
    let mut s = String::new();
    let _ = core::fmt::Write::write_fmt(
        &mut s,
        format_args!(
            "AthBridge cross-process sync broker (§6.1, Slice 1: namespace)\n\
             self_test: {}\n\
             model: name -> object-id -> shared page; refcounted across processes\n\
             invariants: same-name share ONE page, kind-collision rejected, free@0\n\
             next: SYS_CHANNEL_SHMEM_MAP page + SYS_FUTEX wait/signal (Slice 2)\n",
            if ok { "PASS" } else { "FAIL" }
        ),
    );
    s
}

// ===========================================================================
// Slice 2a: the shared object-state page (the cross-process state machine)
// ===========================================================================
//
// For a cross-process object, the state lives in a page mapped into EVERY
// process that opened it (SYS_CHANNEL_SHMEM_MAP=119, which exists). The primary
// wait word is at offset 0 — the futex address every waiter blocks on.
//
// CROSS-PROCESS KEYING (Slice 2b kernel task — LANDED 2026-07-07, item 1828):
// SYS_FUTEX (258 → linux_syscall::futex_wait/wake) is now keyed by the shared
// FRAME's PHYSICAL identity (kernel `sync::phys_key` → `virt_to_phys`), so two
// processes mapping this page at DIFFERENT VAs resolve to ONE wait queue and a
// wait/wake DOES rendezvous across processes; `futex_wait` is a real block
// (`TaskState::BlockedOnFutex`), not a cooperative yield. See
// docs/components/raebridge-process-model.md §"Cross-process sync broker"
// Invariant 2. This struct is the cross-process ABI; bump `SHARED_SYNC_VERSION`
// on any layout change.
//
// This file is the host-provable half: the atomic transitions (Slice 2a) PLUS
// the userspace fast-path contract (Slice 2b host half) — the parked-waiter
// count, wake-elision, and the lost-wakeup-safe `wait_prepare` loop. The only
// pieces left to the kernel half are the physical futex key (above) and a real
// blocking `futex_wait` (today it is a single cooperative yield, not a block).
// Everything here is proven now by driving a `SharedSyncState` in ordinary
// memory; orderings are written for the real concurrent path.

use core::sync::atomic::{AtomicU32, Ordering};

/// Layout version of [`SharedSyncState`] — the cross-process contract. Bumped to
/// 2 when the `waiters` parked-count word was added (Slice 2b host half).
pub const SHARED_SYNC_VERSION: u32 = 2;

/// Sentinel wake count meaning "wake every blocked waiter" (manual-reset event).
pub const WAKE_ALL: u32 = u32::MAX;

/// State of one cross-process sync object, living in a shared page. `#[repr(C)]`
/// with the futex word FIRST (offset 0) so `&state.futex` is the SYS_FUTEX key.
#[repr(C)]
pub struct SharedSyncState {
    /// Offset 0 — the SYS_FUTEX wait word. Encoding depends on `kind`:
    ///   Event     → 1 signaled / 0 unsignaled
    ///   Mutex     → 0 free / else owning thread id
    ///   Semaphore → current count (>0 acquirable)
    pub futex: AtomicU32,
    pub version: u32,
    /// `BrokerKind` as u32 (0=Mutex, 1=Event, 2=Semaphore).
    pub kind: u32,
    /// Event only: 1 = manual-reset, 0 = auto-reset.
    pub manual_reset: u32,
    /// Mutex only: recursive-acquire depth held by the current owner.
    pub recursion: AtomicU32,
    /// Semaphore only: ceiling for `ReleaseSemaphore`.
    pub max_count: i32,
    /// Count of waiters currently parked (blocked in `SYS_FUTEX(WAIT)`), published
    /// by [`SharedSyncState::wait_prepare`] before it blocks. The signal side
    /// issues a `futex_wake` ONLY when this is non-zero (wake-elision) — so an
    /// uncontended `SetEvent`/`ReleaseMutex` stays entirely in userspace, the
    /// fsync-parity contract (docs/.../raebridge-process-model.md Invariant 4).
    /// Not at offset 0, so it never aliases the futex word.
    pub waiters: AtomicU32,
}

/// Outcome of the userspace wait fast path ([`SharedSyncState::wait_prepare`]).
/// `Acquired` is the uncontended common case — returned with ZERO syscalls and
/// no waiter published. `Block { expected }` means the wait could not be
/// satisfied in userspace and a waiter has already been published; the caller
/// must issue `SYS_FUTEX(WAIT, &self.futex, expected)` and call
/// [`SharedSyncState::wait_finish`] when it returns.
#[derive(Debug, PartialEq, Eq)]
pub enum WaitPrep {
    /// Wait satisfied without blocking — `WAIT_OBJECT_0`, no syscall.
    Acquired,
    /// Must block; `expected` is the futex word value to pass to `SYS_FUTEX(WAIT)`
    /// (the syscall returns immediately if the word already changed).
    Block { expected: u32 },
}

impl SharedSyncState {
    pub fn init_event(manual_reset: bool, initial: bool) -> Self {
        Self {
            futex: AtomicU32::new(initial as u32),
            version: SHARED_SYNC_VERSION,
            kind: 1,
            manual_reset: manual_reset as u32,
            recursion: AtomicU32::new(0),
            max_count: 0,
            waiters: AtomicU32::new(0),
        }
    }

    pub fn init_mutex(initial_owner: u32) -> Self {
        Self {
            futex: AtomicU32::new(initial_owner),
            version: SHARED_SYNC_VERSION,
            kind: 0,
            manual_reset: 0,
            recursion: AtomicU32::new(if initial_owner != 0 { 1 } else { 0 }),
            max_count: 0,
            waiters: AtomicU32::new(0),
        }
    }

    pub fn init_semaphore(initial: i32, maximum: i32) -> Self {
        Self {
            futex: AtomicU32::new(initial.max(0) as u32),
            version: SHARED_SYNC_VERSION,
            kind: 2,
            manual_reset: 0,
            recursion: AtomicU32::new(0),
            max_count: maximum,
            waiters: AtomicU32::new(0),
        }
    }

    // ---- Event -----------------------------------------------------------

    /// `SetEvent`. Returns how many waiters to `futex_wake`: 1 for an auto-reset
    /// event (one waiter consumes the signal), [`WAKE_ALL`] for manual-reset.
    pub fn event_set(&self) -> u32 {
        self.futex.store(1, Ordering::Release);
        if self.manual_reset == 1 {
            WAKE_ALL
        } else {
            1
        }
    }

    /// `ResetEvent`.
    pub fn event_reset(&self) {
        self.futex.store(0, Ordering::Release);
    }

    /// Non-blocking wait. Auto-reset consumes the signal (1→0); manual-reset
    /// leaves it set. Returns true if the event was signaled (wait satisfied);
    /// false means the caller must `futex_wait` (Slice 2b).
    pub fn event_try_wait(&self) -> bool {
        if self.manual_reset == 1 {
            self.futex.load(Ordering::Acquire) == 1
        } else {
            self.futex
                .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        }
    }

    // ---- Mutex -----------------------------------------------------------

    /// Non-blocking acquire by `tid`. Returns true if acquired (free→owned, or a
    /// recursive re-acquire by the current owner); false means contended.
    pub fn mutex_try_acquire(&self, tid: u32) -> bool {
        if self
            .futex
            .compare_exchange(0, tid, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            self.recursion.store(1, Ordering::Release);
            return true;
        }
        if self.futex.load(Ordering::Acquire) == tid {
            self.recursion.fetch_add(1, Ordering::AcqRel);
            return true;
        }
        false
    }

    /// `ReleaseMutex` by `tid`. `Ok(wake)` where `wake` is the number of waiters
    /// to `futex_wake` (1 on full release, 0 while still recursively held).
    /// `Err(())` if `tid` is not the owner.
    pub fn mutex_release(&self, tid: u32) -> Result<u32, ()> {
        if self.futex.load(Ordering::Acquire) != tid || tid == 0 {
            return Err(());
        }
        if self.recursion.load(Ordering::Acquire) > 1 {
            self.recursion.fetch_sub(1, Ordering::AcqRel);
            return Ok(0);
        }
        self.recursion.store(0, Ordering::Release);
        self.futex.store(0, Ordering::Release);
        Ok(1)
    }

    // ---- Semaphore -------------------------------------------------------

    /// Non-blocking acquire (decrement if positive). True if a unit was taken.
    pub fn sem_try_acquire(&self) -> bool {
        loop {
            let cur = self.futex.load(Ordering::Acquire);
            if cur == 0 {
                return false;
            }
            if self
                .futex
                .compare_exchange(cur, cur - 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return true;
            }
        }
    }

    /// `ReleaseSemaphore(n)`. `Ok(wake)` = waiters to `futex_wake` (= `n`), or
    /// `Err(())` if it would exceed `max_count` (Win32 `ERROR_TOO_MANY_POSTS`).
    pub fn sem_release(&self, n: i32) -> Result<u32, ()> {
        if n <= 0 {
            return Err(());
        }
        loop {
            let cur = self.futex.load(Ordering::Acquire) as i32;
            if cur + n > self.max_count {
                return Err(());
            }
            if self
                .futex
                .compare_exchange(
                    cur as u32,
                    (cur + n) as u32,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return Ok(n as u32);
            }
        }
    }

    // ---- Wait fast path + wake-elision (Slice 2b host half) --------------
    //
    // These encode the fsync-parity contract from
    // docs/components/raebridge-process-model.md §"Cross-process sync broker":
    // an uncontended wait/signal touches ZERO syscalls (Invariant 1/4), and the
    // `wait_prepare` ordering is lost-wakeup-safe (Invariant 3). The SYS_FUTEX
    // WAIT/WAKE on `&self.futex` is the kernel boundary the caller crosses; this
    // logic decides *whether* to cross it.

    /// Non-blocking wait try, keyed by object kind: mutex needs the caller `tid`
    /// (event/semaphore ignore it). True ⇒ the wait is satisfied in userspace.
    pub fn try_wait(&self, tid: u32) -> bool {
        match self.kind {
            0 => self.mutex_try_acquire(tid),
            1 => self.event_try_wait(),
            2 => self.sem_try_acquire(),
            _ => false,
        }
    }

    /// Waiters currently parked in `SYS_FUTEX(WAIT)`. The signal side wakes only
    /// when this is non-zero. `SeqCst` so it participates in the same total order
    /// as `wait_prepare`'s publish — the pairing that makes wake-elision safe.
    pub fn parked(&self) -> u32 {
        self.waiters.load(Ordering::SeqCst)
    }

    /// The canonical wait fast path. Returns with ZERO syscalls in the
    /// uncontended case; otherwise publishes a parked waiter and returns the
    /// futex word to block on. The ordering is the standard futex/fsync recipe
    /// and is lost-wakeup-safe:
    ///   1. try once — uncontended ⇒ `Acquired`, no waiter published, no syscall.
    ///   2. publish intent (`waiters += 1`, `SeqCst`) BEFORE the final check.
    ///   3. re-check — a signal that landed between (1) and (2) is observed here
    ///      (its `futex` Release store is acquired by `try_wait`), so we retract
    ///      and return `Acquired` instead of blocking on a stale value.
    ///   4. otherwise `Block { expected }`; caller does `SYS_FUTEX(WAIT)` then
    ///      [`wait_finish`]. The `SeqCst` publish + the signaler's `SeqCst`
    ///      `parked()` read share a total order, so the signaler either sees this
    ///      waiter (and wakes it) or its state store is seen by the re-check.
    pub fn wait_prepare(&self, tid: u32) -> WaitPrep {
        if self.try_wait(tid) {
            return WaitPrep::Acquired;
        }
        self.waiters.fetch_add(1, Ordering::SeqCst);
        if self.try_wait(tid) {
            self.waiters.fetch_sub(1, Ordering::SeqCst);
            return WaitPrep::Acquired;
        }
        WaitPrep::Block {
            expected: self.futex.load(Ordering::Acquire),
        }
    }

    /// Retract a parked waiter after its `SYS_FUTEX(WAIT)` returns (woken or
    /// EAGAIN). Paired 1:1 with the publish in [`wait_prepare`]'s `Block` arm;
    /// guarded so a stray call can never underflow the count.
    pub fn wait_finish(&self) {
        let _ = self
            .waiters
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |w| w.checked_sub(1));
    }

    /// Wake-elision: given the `intended` wake count a transition returned
    /// (`event_set`/`mutex_release`/`sem_release`), the count to actually pass to
    /// `SYS_FUTEX(WAKE)`. Zero when no waiter is parked — so an uncontended
    /// signal issues NO syscall (Invariant 4). `WAKE_ALL` is preserved when a
    /// waiter is parked. Call AFTER the state transition (its `futex` Release
    /// store must precede this `SeqCst` `parked()` read).
    pub fn wake_count(&self, intended: u32) -> u32 {
        if self.parked() == 0 {
            0
        } else {
            intended
        }
    }
}

/// FAIL-able self-test of the shared-page state machine (broker §6.1, Slice 2a).
/// Drives event (auto+manual), mutex (owner/recursion/non-owner), and semaphore
/// (drain/refill/overflow) through their transitions on a real `SharedSyncState`
/// — the same struct a shared page will hold — returning false on any wrong
/// result. The blocking `futex_wait/wake` is Slice 2b; this proves the logic.
pub fn run_shared_state_self_test() -> bool {
    let mut ok = true;

    // Auto-reset event: set wakes 1, one wait consumes, second wait must block.
    let ev = SharedSyncState::init_event(false, false);
    ok &= ev.event_set() == 1;
    ok &= ev.event_try_wait(); // consumes
    ok &= !ev.event_try_wait(); // empty -> would block

    // Manual-reset event: set wakes ALL, stays signaled across waits.
    let mev = SharedSyncState::init_event(true, false);
    ok &= mev.event_set() == WAKE_ALL;
    ok &= mev.event_try_wait();
    ok &= mev.event_try_wait(); // still signaled
    mev.event_reset();
    ok &= !mev.event_try_wait();

    // Mutex: t1 acquires + recurses, t2 contends, t1 releases twice, t2 takes.
    let mx = SharedSyncState::init_mutex(0);
    ok &= mx.mutex_try_acquire(1);
    ok &= mx.mutex_try_acquire(1); // recursive
    ok &= !mx.mutex_try_acquire(2); // contended
    ok &= mx.mutex_release(2).is_err(); // non-owner
    ok &= mx.mutex_release(1) == Ok(0); // still held (recursion)
    ok &= mx.mutex_release(1) == Ok(1); // freed -> wake 1
    ok &= mx.mutex_try_acquire(2); // t2 now takes it

    // Semaphore(1,2): drain, empty blocks, release refills, overflow rejected.
    let sm = SharedSyncState::init_semaphore(1, 2);
    ok &= sm.sem_try_acquire();
    ok &= !sm.sem_try_acquire(); // empty
    ok &= sm.sem_release(1) == Ok(1);
    ok &= sm.sem_release(2).is_err(); // would exceed max=2 (count already 1)
    ok &= sm.sem_try_acquire();

    // Fast path (Slice 2b host half): uncontended wait acquires with NO waiter
    // published; contended wait publishes exactly one; the signal wakes only
    // when a waiter is parked (wake-elision); wait_finish retracts the publish.
    let fe = SharedSyncState::init_event(false, true); // auto-reset, signaled
    ok &= fe.wait_prepare(0) == WaitPrep::Acquired; // signaled -> no block, no syscall
    ok &= fe.parked() == 0; // uncontended -> nothing published
    ok &= matches!(fe.wait_prepare(0), WaitPrep::Block { .. }); // now empty -> must block
    ok &= fe.parked() == 1; // one waiter published
    ok &= fe.wake_count(fe.event_set()) == 1; // a waiter is parked -> real wake
    fe.wait_finish(); // waiter returned from futex
    ok &= fe.parked() == 0;
    fe.event_reset();
    ok &= fe.wake_count(fe.event_set()) == 0; // no waiter parked -> wake elided (0 syscalls)
    fe.wait_finish(); // stray finish with no matching prepare...
    ok &= fe.parked() == 0; // ...saturates, never underflows below 0

    ok
}

/// Slice 5: the piece that makes a NAMED Win32 sync object CROSS-process. It
/// composes the [`BrokerNamespace`] (name → object → shared page id, refcounted
/// across every process) with a page store holding each object's live
/// [`SharedSyncState`] (the futex word + event/mutex/semaphore fields). Two
/// processes that `CreateMutexW("Global\\Foo")` resolve — via the broker — to
/// ONE page id and therefore ONE `SharedSyncState`, so a wait in one process and
/// a signal in another rendezvous through the (now physical-frame-keyed, item
/// 1828) kernel futex. UNNAMED objects never reach here — the caller keeps those
/// in its per-process in-process store.
///
/// In the real daemon the page store is a set of `SYS_CHANNEL_SHMEM_MAP`
/// regions; here (and in host tests) it is an in-memory map so the routing +
/// rendezvous are host-provable without a live guest. Wiring a live guest's IAT
/// `CreateMutexW` to route here, binding page ids to real shared frames, and the
/// real `SyscallFutexOps` are the gated guest-execution step.
pub struct NamedSyncRouter {
    ns: BrokerNamespace,
    pages: BTreeMap<u64, SharedSyncState>,
}

impl Default for NamedSyncRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl NamedSyncRouter {
    pub fn new() -> Self {
        Self {
            ns: BrokerNamespace::new(),
            pages: BTreeMap::new(),
        }
    }

    /// Route a NAMED `Create{Mutex,Event,Semaphore}W`. On a brand-new object the
    /// caller's freshly-built `init` state is installed on the new page; on an
    /// existing same-name/same-kind object the existing page's state is reused
    /// and `init` is DROPPED (Win32: opening an existing object ignores your init
    /// args). A cross-kind name collision is rejected untouched.
    pub fn create(&mut self, name: &str, kind: BrokerKind, init: SharedSyncState) -> BrokerCreate {
        let r = self.ns.create(name, kind);
        if let BrokerCreate::Created { page_id, .. } = r {
            self.pages.insert(page_id, init);
        }
        r
    }

    /// Route `Open{Mutex,Event,Semaphore}W`: an EXISTING named object of `kind`.
    pub fn open(&mut self, name: &str, kind: BrokerKind) -> Option<(u64, u64)> {
        self.ns.open(name, kind)
    }

    /// Route `CloseHandle`. Frees the shared page when the last cross-process
    /// reference closes. Returns true iff this was the last reference.
    pub fn close(&mut self, object_id: u64) -> bool {
        let page = self.ns.page_id(object_id); // capture before the entry is freed
        let last = self.ns.close(object_id);
        if last {
            if let Some(p) = page {
                self.pages.remove(&p);
            }
        }
        last
    }

    /// The live shared state for a page id — drive it via `sync_engine`.
    pub fn state(&self, page_id: u64) -> Option<&SharedSyncState> {
        self.pages.get(&page_id)
    }

    /// Live cross-process object / page counts (test + `/proc` introspection).
    pub fn object_count(&self) -> usize {
        self.ns.object_count()
    }
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }
}

/// FAIL-able self-test (Slice 5): a NAMED object routes through the broker so two
/// "processes" share ONE page + one `SharedSyncState`, cross-kind collision is
/// rejected, the routed page's state is a real drivable object, and the page is
/// freed on last close. Deterministic + no_std (boot-wireable); the genuine
/// two-thread cross-process rendezvous is the host-KAT below. Returns false on
/// any wrong result.
pub fn run_named_routing_self_test() -> bool {
    use crate::sync_engine::{self, FutexOps, SyncAccount};
    use crate::WAIT_OBJECT_0;

    // No-op futex: this deterministic single-threaded test never truly parks.
    struct NoopFutex;
    impl FutexOps for NoopFutex {
        fn futex_wait(&self, _w: &core::sync::atomic::AtomicU32, _e: u32) {}
        fn futex_wake(&self, _w: &core::sync::atomic::AtomicU32, _n: u32) {}
    }
    let f = NoopFutex;
    let acct = SyncAccount::new();
    let mut r = NamedSyncRouter::new();
    let mut ok = true;

    // Process A: CreateMutexW("Global\Mtx") -> fresh page.
    let (a_id, a_page) = match r.create(
        "Global\\Mtx",
        BrokerKind::Mutex,
        SharedSyncState::init_mutex(0),
    ) {
        BrokerCreate::Created { object_id, page_id } => (object_id, page_id),
        _ => return false,
    };
    ok &= a_id != 0 && a_page != 0 && r.page_count() == 1;

    // Process B: SAME name+kind -> the SAME page (the whole point of Slice 5).
    let (b_id, b_page) = match r.create(
        "Global\\Mtx",
        BrokerKind::Mutex,
        SharedSyncState::init_mutex(0),
    ) {
        BrokerCreate::OpenedExisting { object_id, page_id } => (object_id, page_id),
        _ => return false,
    };
    ok &= b_id == a_id && b_page == a_page && r.page_count() == 1; // still ONE page

    // Cross-kind collision on the same name is rejected untouched.
    ok &= matches!(
        r.create(
            "Global\\Mtx",
            BrokerKind::Event,
            SharedSyncState::init_event(false, false)
        ),
        BrokerCreate::TypeMismatch
    );

    // Drive the SHARED mutex via sync_engine: A (tid 1) acquires uncontended then
    // releases — proves the routed page's state is a real, drivable object.
    if let Some(st) = r.state(a_page) {
        ok &= sync_engine::wait(st, 1, &f, &acct) == WAIT_OBJECT_0;
        ok &= sync_engine::release_mutex(st, 1, &f, &acct).is_ok();
    } else {
        ok = false;
    }

    // Refcount: A closes (B still holds) -> page survives; B closes -> freed.
    ok &= !r.close(a_id);
    ok &= r.state(a_page).is_some(); // still alive on B's ref
    ok &= r.close(b_id);
    ok &= r.state(a_page).is_none() && r.page_count() == 0; // page released

    ok
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;

    // ---- Slice 5: named-object routing through the cross-process broker ------

    #[test]
    fn slice5_named_routing_self_test_passes() {
        assert!(run_named_routing_self_test());
    }

    #[test]
    fn slice5_two_names_share_one_page_unnamed_never_routes() {
        let mut r = NamedSyncRouter::new();
        let a = match r.create(
            "Global\\E",
            BrokerKind::Event,
            SharedSyncState::init_event(false, false),
        ) {
            BrokerCreate::Created { page_id, .. } => page_id,
            _ => panic!(),
        };
        // Second create of the same name+kind shares the page; page_count stays 1.
        match r.create(
            "Global\\E",
            BrokerKind::Event,
            SharedSyncState::init_event(false, false),
        ) {
            BrokerCreate::OpenedExisting { page_id, .. } => assert_eq!(page_id, a),
            _ => panic!("second create must share the page"),
        }
        assert_eq!(r.page_count(), 1);
        // A DIFFERENT name gets a DIFFERENT page.
        let b = match r.create(
            "Global\\E2",
            BrokerKind::Event,
            SharedSyncState::init_event(false, false),
        ) {
            BrokerCreate::Created { page_id, .. } => page_id,
            _ => panic!(),
        };
        assert_ne!(a, b);
        assert_eq!(r.page_count(), 2);
    }

    // A Condvar parking lot standing in for the kernel futex queue (same model as
    // sync_engine's rendezvous test) so we can prove a REAL cross-process wait/wake
    // through a broker-shared page with two OS threads.
    struct ParkingLotFutex {
        m: std::sync::Mutex<()>,
        cv: std::sync::Condvar,
    }
    impl ParkingLotFutex {
        fn new() -> Self {
            Self {
                m: std::sync::Mutex::new(()),
                cv: std::sync::Condvar::new(),
            }
        }
    }
    impl crate::sync_engine::FutexOps for ParkingLotFutex {
        fn futex_wait(&self, word: &core::sync::atomic::AtomicU32, expected: u32) {
            let guard = self.m.lock().unwrap();
            if word.load(core::sync::atomic::Ordering::Acquire) == expected {
                let _g = self
                    .cv
                    .wait_timeout(guard, std::time::Duration::from_secs(5))
                    .unwrap();
            }
        }
        fn futex_wake(&self, _word: &core::sync::atomic::AtomicU32, _count: u32) {
            let _guard = self.m.lock().unwrap();
            self.cv.notify_all();
        }
    }

    #[test]
    fn slice5_cross_process_rendezvous_two_threads() {
        use crate::sync_engine::{self, SyncAccount};
        use crate::WAIT_OBJECT_0;

        // One daemon (router); two processes both CreateEventW the same name and
        // so resolve to ONE page + one SharedSyncState.
        let mut r = NamedSyncRouter::new();
        let page = match r.create(
            "Global\\Evt",
            BrokerKind::Event,
            SharedSyncState::init_event(false, false),
        ) {
            BrokerCreate::Created { page_id, .. } => page_id,
            _ => panic!("A create failed"),
        };
        match r.create(
            "Global\\Evt",
            BrokerKind::Event,
            SharedSyncState::init_event(false, false),
        ) {
            BrokerCreate::OpenedExisting { page_id, .. } => {
                assert_eq!(page_id, page, "B must share A's page")
            }
            _ => panic!("B did not share the page"),
        }

        let st = r.state(page).unwrap();
        let f = ParkingLotFutex::new();
        let acct = SyncAccount::new();

        std::thread::scope(|s| {
            // Process A: waiter parks on the shared event.
            let waiter = s.spawn(|| sync_engine::wait(st, 0, &f, &acct));
            // Wait until A has published a parked waiter (bounded — a decision bug
            // fails the assert instead of spinning forever).
            let mut guard = 0u64;
            while st.parked() == 0 {
                std::thread::yield_now();
                guard += 1;
                assert!(
                    guard < 50_000_000,
                    "waiter never parked through the routed page"
                );
            }
            // Process B: SetEvent on the SAME shared page wakes A cross-"process".
            let woke = sync_engine::set_event(st, &f, &acct);
            assert_eq!(woke, 1, "the one parked waiter must be woken");
            assert_eq!(
                waiter.join().unwrap(),
                WAIT_OBJECT_0,
                "A wakes with WAIT_OBJECT_0"
            );
        });
        assert_eq!(st.parked(), 0, "waiter retracted after waking");
    }

    #[test]
    fn create_then_create_same_name_shares_one_page() {
        let mut ns = BrokerNamespace::new();
        let first = ns.create("Global\\M", BrokerKind::Mutex);
        let (id1, page1) = match first {
            BrokerCreate::Created { object_id, page_id } => (object_id, page_id),
            other => panic!("first create must be Created, got {other:?}"),
        };
        // A second process naming the same object must land on the SAME page.
        match ns.create("Global\\M", BrokerKind::Mutex) {
            BrokerCreate::OpenedExisting { object_id, page_id } => {
                assert_eq!(object_id, id1);
                assert_eq!(page_id, page1, "cross-process share must reuse the page");
            }
            other => panic!("second create must OpenExisting, got {other:?}"),
        }
        assert_eq!(ns.object_count(), 1, "one object, not two");
    }

    #[test]
    fn distinct_names_get_distinct_pages() {
        let mut ns = BrokerNamespace::new();
        let a = ns.create("Global\\A", BrokerKind::Event);
        let b = ns.create("Global\\B", BrokerKind::Event);
        let pa = match a {
            BrokerCreate::Created { page_id, .. } => page_id,
            _ => panic!(),
        };
        let pb = match b {
            BrokerCreate::Created { page_id, .. } => page_id,
            _ => panic!(),
        };
        assert_ne!(pa, pb, "different objects must not alias one page");
    }

    #[test]
    fn name_reuse_across_kinds_is_rejected() {
        let mut ns = BrokerNamespace::new();
        let _ = ns.create("Global\\X", BrokerKind::Event);
        assert_eq!(
            ns.create("Global\\X", BrokerKind::Semaphore),
            BrokerCreate::TypeMismatch,
            "a name held by an event can't be recreated as a semaphore"
        );
        // The rejection must not have bumped the refcount or added an object.
        assert_eq!(ns.object_count(), 1);
        assert!(ns.open("Global\\X", BrokerKind::Semaphore).is_none());
    }

    #[test]
    fn refcount_frees_only_on_last_close() {
        let mut ns = BrokerNamespace::new();
        let id = match ns.create("Global\\R", BrokerKind::Event) {
            BrokerCreate::Created { object_id, .. } => object_id,
            _ => panic!(),
        };
        let _ = ns.open("Global\\R", BrokerKind::Event); // refs = 2
        assert!(!ns.close(id), "still one ref open -> no free");
        assert!(ns.close(id), "last ref -> free + page release");
        assert_eq!(ns.object_count(), 0);
        assert!(!ns.close(id), "closing an unknown id is a no-op false");
    }

    #[test]
    fn open_nonexistent_is_none() {
        let mut ns = BrokerNamespace::new();
        assert!(ns.open("Global\\ghost", BrokerKind::Mutex).is_none());
    }

    #[test]
    fn namespace_self_test_passes() {
        assert!(
            run_namespace_self_test(),
            "broker namespace self-test (the boot/iron proof) regressed"
        );
    }

    // ---- Slice 2a: shared-page state machine (FAIL-able) -----------------

    #[test]
    fn auto_event_consumes_one_signal() {
        let ev = SharedSyncState::init_event(false, false);
        assert_eq!(ev.event_set(), 1, "auto-reset wakes exactly one waiter");
        assert!(ev.event_try_wait(), "the one waiter consumes the signal");
        assert!(
            !ev.event_try_wait(),
            "auto-reset must be empty after consume"
        );
    }

    #[test]
    fn manual_event_stays_signaled_and_wakes_all() {
        let ev = SharedSyncState::init_event(true, false);
        assert_eq!(ev.event_set(), WAKE_ALL, "manual-reset wakes all waiters");
        assert!(ev.event_try_wait());
        assert!(
            ev.event_try_wait(),
            "manual-reset stays signaled until reset"
        );
        ev.event_reset();
        assert!(!ev.event_try_wait());
    }

    #[test]
    fn mutex_recursion_and_owner_enforcement() {
        let mx = SharedSyncState::init_mutex(0);
        assert!(mx.mutex_try_acquire(1));
        assert!(mx.mutex_try_acquire(1), "owner may recurse");
        assert!(!mx.mutex_try_acquire(2), "another thread is contended");
        assert_eq!(mx.mutex_release(2), Err(()), "non-owner release rejected");
        assert_eq!(mx.mutex_release(1), Ok(0), "recursive release, no wake yet");
        assert_eq!(mx.mutex_release(1), Ok(1), "final release wakes one");
        assert!(mx.mutex_try_acquire(2), "now free for the contender");
    }

    #[test]
    fn semaphore_count_and_overflow() {
        let sm = SharedSyncState::init_semaphore(1, 2);
        assert!(sm.sem_try_acquire());
        assert!(!sm.sem_try_acquire(), "drained");
        assert_eq!(sm.sem_release(1), Ok(1));
        assert_eq!(sm.sem_release(2), Err(()), "release past max is rejected");
        assert!(sm.sem_try_acquire());
    }

    #[test]
    fn futex_word_is_at_offset_zero() {
        // The cross-process contract: SYS_FUTEX keys on &state.futex, which MUST
        // be offset 0 of the shared page. A field reorder would silently break
        // cross-process blocking — catch it here.
        let st = SharedSyncState::init_event(false, false);
        let base = &st as *const _ as usize;
        let futex = &st.futex as *const _ as usize;
        assert_eq!(futex, base, "futex word must be at offset 0 of the page");
    }

    #[test]
    fn shared_state_self_test_passes() {
        assert!(
            run_shared_state_self_test(),
            "shared-page state machine self-test (the boot/iron proof) regressed"
        );
    }

    // ---- Slice 2b host half: fast path + wake-elision (FAIL-able) ---------

    #[test]
    fn uncontended_wait_publishes_no_waiter() {
        // A signaled object satisfies the wait in userspace: Acquired, and NO
        // waiter is published — the zero-syscall path (Invariant 1).
        let ev = SharedSyncState::init_event(false, true); // signaled
        assert_eq!(ev.wait_prepare(0), WaitPrep::Acquired);
        assert_eq!(ev.parked(), 0, "uncontended wait must not publish a waiter");
    }

    #[test]
    fn contended_wait_publishes_exactly_one_waiter() {
        let ev = SharedSyncState::init_event(false, false); // unsignaled
        match ev.wait_prepare(0) {
            WaitPrep::Block { expected } => assert_eq!(expected, 0, "block on the unset word"),
            WaitPrep::Acquired => panic!("an unsignaled auto-event must block"),
        }
        assert_eq!(
            ev.parked(),
            1,
            "the blocked waiter must be published exactly once"
        );
    }

    #[test]
    fn wake_is_elided_when_no_waiter_parked() {
        // SetEvent with nobody blocked must yield wake_count 0 -> NO SYS_FUTEX
        // WAKE syscall (Invariant 4 / fsync parity).
        let ev = SharedSyncState::init_event(false, false);
        assert_eq!(
            ev.wake_count(ev.event_set()),
            0,
            "uncontended signal must elide the wake"
        );
    }

    #[test]
    fn wake_fires_when_a_waiter_is_parked() {
        let ev = SharedSyncState::init_event(false, false);
        assert!(matches!(ev.wait_prepare(0), WaitPrep::Block { .. }));
        assert_eq!(ev.parked(), 1);
        assert_eq!(
            ev.wake_count(ev.event_set()),
            1,
            "a parked waiter must be woken"
        );
    }

    #[test]
    fn manual_event_wake_all_preserved_when_parked() {
        let ev = SharedSyncState::init_event(true, false); // manual-reset
        assert!(matches!(ev.wait_prepare(0), WaitPrep::Block { .. }));
        assert_eq!(
            ev.wake_count(ev.event_set()),
            WAKE_ALL,
            "manual-reset wakes all parked"
        );
    }

    #[test]
    fn wait_finish_retracts_one_and_saturates() {
        let ev = SharedSyncState::init_event(false, false);
        let _ = ev.wait_prepare(0); // Block -> parked == 1
        assert_eq!(ev.parked(), 1);
        ev.wait_finish(); // matched retract
        assert_eq!(ev.parked(), 0);
        ev.wait_finish(); // stray finish
        assert_eq!(ev.parked(), 0, "wait_finish must saturate, never underflow");
    }

    #[test]
    fn mutex_wait_prepare_honors_owner() {
        let mx = SharedSyncState::init_mutex(0);
        assert_eq!(
            mx.wait_prepare(1),
            WaitPrep::Acquired,
            "free mutex acquired by t1"
        );
        // t2 contends -> must block and publish a waiter.
        assert!(matches!(mx.wait_prepare(2), WaitPrep::Block { .. }));
        assert_eq!(mx.parked(), 1);
        // t1 releases; the wake must fire because t2 is parked.
        assert_eq!(mx.wake_count(mx.mutex_release(1).unwrap()), 1);
        mx.wait_finish();
        // t2 now acquires without blocking.
        assert_eq!(mx.wait_prepare(2), WaitPrep::Acquired);
    }

    #[test]
    fn version_bumped_for_waiters_word() {
        // The `waiters` field grew the cross-process page layout; the version
        // MUST advance so a stale mapper can detect the mismatch.
        assert_eq!(SHARED_SYNC_VERSION, 2);
    }
}
