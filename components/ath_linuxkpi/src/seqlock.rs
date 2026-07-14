//! Linux sequence locks (`seqlock_t` / `seqcount_t`).
//!
//! The optimistic-read primitive DRM uses for vblank timestamps and other
//! fast-path state reads (`drm_vblank_count_and_time`, `drm_crtc_vblank_*`):
//! readers never block writers and never take the lock, they snapshot a
//! sequence counter, read the protected data, and retry if a write intervened.
//!
//! Layout: the sequence counter is the FIRST `u32` of the driver's `seqcount_t`
//! storage; `seqlock_t` is `{ seqcount_t seqcount; spinlock_t lock; }`, so its
//! first word is *also* the sequence counter and its second word is the writer
//! spinlock. We address both through the driver-owned pointer — we never
//! allocate seqlock state ourselves (same model as `sync.rs`).
//!
//! Convention (matches Linux): the sequence is EVEN when stable and ODD while a
//! writer holds it. A reader that begins on an odd value, or sees the value
//! change across its critical section, retries.

use crate::sync;
use core::sync::atomic::{AtomicU32, Ordering};

/// `seqcount_t` — a bare sequence counter (no embedded writer lock). Drivers
/// that already serialize writers another way use this directly.
#[repr(C)]
pub struct SeqCount {
    pub sequence: u32,
}

/// `seqlock_t` — sequence counter plus a writer spinlock word. `write_seqlock`
/// acquires `lock` before bumping `sequence`; readers ignore `lock` entirely.
#[repr(C)]
pub struct SeqLock {
    pub seqcount: SeqCount,
    pub lock: u32,
}

// The C ABI shapes drivers compile against. A mismatch here would silently
// corrupt the writer lock word that follows the counter.
const _: () = assert!(core::mem::size_of::<SeqCount>() == 4);
const _: () = assert!(core::mem::align_of::<SeqCount>() == 4);
const _: () = assert!(core::mem::size_of::<SeqLock>() == 8);
const _: () = assert!(core::mem::align_of::<SeqLock>() == 4);
const _: () = assert!(core::mem::offset_of!(SeqLock, seqcount) == 0);
const _: () = assert!(core::mem::offset_of!(SeqLock, lock) == 4);

#[inline]
unsafe fn seqword<'a>(p: *mut u32) -> &'a AtomicU32 {
    &*(p as *const AtomicU32)
}

// ── seqcount_t ────────────────────────────────────────────────────────────────

/// `seqcount_init(s)` — zero the counter (even == stable).
#[no_mangle]
pub extern "C" fn seqcount_init(s: *mut SeqCount) {
    if !s.is_null() {
        unsafe { seqword(s as *mut u32) }.store(0, Ordering::Relaxed);
    }
}

/// `__read_seqcount_begin` / `raw_read_seqcount_begin` — return the current
/// sequence. A reader that observes an ODD value must keep spinning until it is
/// even (a writer is mid-update); we yield the cooperative CPU while it spins.
#[no_mangle]
pub extern "C" fn read_seqcount_begin(s: *mut SeqCount) -> u32 {
    if s.is_null() {
        return 0;
    }
    let c = unsafe { seqword(s as *mut u32) };
    loop {
        let v = c.load(Ordering::Acquire);
        if v & 1 == 0 {
            return v;
        }
        // Writer in progress — let it finish (cooperative yield, no busy spin).
        unsafe { crate::host::sys_linuxkpi_msleep(0) };
    }
}

/// `read_seqcount_retry(s, start)` → 1 if the protected read must be retried
/// (the counter moved since `start`, i.e. a writer ran), 0 if the snapshot was
/// stable. The `Acquire` load pairs with the writer's `Release` increments so
/// the reader cannot reorder protected loads after this check.
#[no_mangle]
pub extern "C" fn read_seqcount_retry(s: *mut SeqCount, start: u32) -> i32 {
    if s.is_null() {
        return 0;
    }
    (unsafe { seqword(s as *mut u32) }.load(Ordering::Acquire) != start) as i32
}

/// `write_seqcount_begin(s)` — make the count odd (writer entered). Callers must
/// already hold whatever serializes writers (a bare `seqcount_t` has no lock).
#[no_mangle]
pub extern "C" fn write_seqcount_begin(s: *mut SeqCount) {
    if !s.is_null() {
        // Increment with Release so readers' Acquire snapshot ordering holds.
        unsafe { seqword(s as *mut u32) }.fetch_add(1, Ordering::Release);
    }
}

/// `write_seqcount_end(s)` — make the count even again (writer left).
#[no_mangle]
pub extern "C" fn write_seqcount_end(s: *mut SeqCount) {
    if !s.is_null() {
        unsafe { seqword(s as *mut u32) }.fetch_add(1, Ordering::Release);
    }
}

// ── seqlock_t ─────────────────────────────────────────────────────────────────

/// `seqlock_init(sl)` — zero both the counter and the writer lock word.
#[no_mangle]
pub extern "C" fn seqlock_init(sl: *mut SeqLock) {
    if sl.is_null() {
        return;
    }
    unsafe {
        seqcount_init(&mut (*sl).seqcount as *mut SeqCount);
        seqword((&mut (*sl).lock) as *mut u32).store(0, Ordering::Relaxed);
    }
}

/// `read_seqbegin(sl)` — reader entry: snapshot the sequence (spin past an
/// in-progress writer). Readers do NOT touch the writer lock.
#[no_mangle]
pub extern "C" fn read_seqbegin(sl: *mut SeqLock) -> u32 {
    if sl.is_null() {
        return 0;
    }
    read_seqcount_begin(unsafe { &mut (*sl).seqcount as *mut SeqCount })
}

/// `read_seqretry(sl, start)` → 1 if a writer intervened (retry the read), 0 if
/// the snapshot is consistent.
#[no_mangle]
pub extern "C" fn read_seqretry(sl: *mut SeqLock, start: u32) -> i32 {
    if sl.is_null() {
        return 0;
    }
    read_seqcount_retry(unsafe { &mut (*sl).seqcount as *mut SeqCount }, start)
}

/// `write_seqlock(sl)` — acquire the writer spinlock, then bump the counter odd.
/// The lock serializes writers against each other; the odd counter signals an
/// in-progress write to readers.
#[no_mangle]
pub extern "C" fn write_seqlock(sl: *mut SeqLock) {
    if sl.is_null() {
        return;
    }
    sync::acquire(unsafe { (&mut (*sl).lock) as *mut u32 });
    write_seqcount_begin(unsafe { &mut (*sl).seqcount as *mut SeqCount });
}

/// `write_sequnlock(sl)` — bump the counter even, then release the writer lock.
#[no_mangle]
pub extern "C" fn write_sequnlock(sl: *mut SeqLock) {
    if sl.is_null() {
        return;
    }
    write_seqcount_end(unsafe { &mut (*sl).seqcount as *mut SeqCount });
    sync::release(unsafe { (&mut (*sl).lock) as *mut u32 });
}

/// `write_seqlock_irqsave(sl)` — the daemon owns no maskable IRQ state of its
/// own (the host delivers interrupts), so the saved-flags value is a sentinel;
/// the lock + counter behaviour is identical to `write_seqlock`.
#[no_mangle]
pub extern "C" fn write_seqlock_irqsave(sl: *mut SeqLock) -> u64 {
    write_seqlock(sl);
    0
}

/// `write_sequnlock_irqrestore(sl, flags)` — restore (no-op flags) and unlock.
#[no_mangle]
pub extern "C" fn write_sequnlock_irqrestore(sl: *mut SeqLock, _flags: u64) {
    write_sequnlock(sl);
}

/// `write_seqlock_irq(sl)` — IRQ-disabling variant; same behaviour here.
#[no_mangle]
pub extern "C" fn write_seqlock_irq(sl: *mut SeqLock) {
    write_seqlock(sl);
}
/// `write_sequnlock_irq(sl)`.
#[no_mangle]
pub extern "C" fn write_sequnlock_irq(sl: *mut SeqLock) {
    write_sequnlock(sl);
}

/// `write_seqlock_bh(sl)` — bottom-half-disabling variant; same behaviour.
#[no_mangle]
pub extern "C" fn write_seqlock_bh(sl: *mut SeqLock) {
    write_seqlock(sl);
}
/// `write_sequnlock_bh(sl)`.
#[no_mangle]
pub extern "C" fn write_sequnlock_bh(sl: *mut SeqLock) {
    write_sequnlock(sl);
}

#[cfg(test)]
mod tests {
    //! Pure host KATs (no syscall path — safe on Windows-native per project
    //! memory). Every assert is a concrete expected-vs-actual comparison and can
    //! FAIL. None of these paths call `read_seqcount_begin` (which yields via the
    //! msleep syscall) — they drive the counter directly so the test stays pure.
    use super::*;

    #[test]
    fn write_makes_sequence_odd_then_even() {
        let mut sl = SeqLock {
            seqcount: SeqCount { sequence: 0 },
            lock: 0,
        };
        let p = &mut sl as *mut SeqLock;
        let cnt = || sl_seq(&sl);

        seqlock_init(p);
        assert_eq!(cnt(), 0, "fresh seqlock must start even (stable)");

        write_seqlock(p);
        // Odd while the writer holds it; writer lock word taken.
        assert_eq!(sl_seq(&sl) & 1, 1, "sequence must be ODD during a write");
        assert_eq!(sl.lock, 1, "writer spinlock must be held during a write");

        write_sequnlock(p);
        assert_eq!(sl_seq(&sl) & 1, 0, "sequence must be EVEN after the write");
        assert_eq!(sl_seq(&sl), 2, "one write advances the counter by 2");
        assert_eq!(
            sl.lock, 0,
            "writer spinlock must be released after the write"
        );
    }

    #[test]
    fn read_seqretry_detects_intervening_write() {
        let mut sl = SeqLock {
            seqcount: SeqCount { sequence: 0 },
            lock: 0,
        };
        let p = &mut sl as *mut SeqLock;
        seqlock_init(p);

        // Stable read: no write between begin and retry → retry == 0.
        let start = read_begin_pure(&sl);
        assert_eq!(
            read_seqretry(p, start),
            0,
            "no intervening write → read_seqretry must be 0 (snapshot valid)"
        );

        // A full write between begin and retry → retry == 1.
        let start = read_begin_pure(&sl);
        write_seqlock(p);
        write_sequnlock(p);
        assert_eq!(
            read_seqretry(p, start),
            1,
            "an intervening write → read_seqretry must be 1 (retry the read)"
        );

        // After re-snapshotting post-write, a subsequent stable read is valid.
        let start = read_begin_pure(&sl);
        assert_eq!(
            read_seqretry(p, start),
            0,
            "re-snapshot after the write must be stable again"
        );
    }

    #[test]
    fn reader_started_mid_write_must_retry() {
        // Models the race read_seqcount_begin spins on: a reader that captured an
        // ODD start (writer in progress) must always retry, even if the writer
        // then finishes. (We snapshot the odd value directly to keep the test
        // pure — no msleep yield.)
        let mut sl = SeqLock {
            seqcount: SeqCount { sequence: 0 },
            lock: 0,
        };
        let p = &mut sl as *mut SeqLock;
        seqlock_init(p);

        write_seqlock(p);
        let mid = sl_seq(&sl); // odd snapshot taken mid-write
        assert_eq!(mid & 1, 1);
        write_sequnlock(p);
        assert_eq!(
            read_seqretry(p, mid),
            1,
            "a reader that saw the mid-write odd count must retry"
        );
    }

    #[test]
    fn seqcount_standalone_increments_by_two_per_write() {
        let mut s = SeqCount { sequence: 7 };
        let p = &mut s as *mut SeqCount;
        seqcount_init(p);
        assert_eq!(unsafe { (*p).sequence }, 0, "init zeroes the counter");

        write_seqcount_begin(p);
        assert_eq!(unsafe { (*p).sequence } & 1, 1, "begin makes it odd");
        let start_after_begin = 1u32;
        assert_eq!(
            read_seqcount_retry(p, start_after_begin),
            0,
            "retry against the in-progress value itself is 0 (same value)"
        );
        write_seqcount_end(p);
        assert_eq!(unsafe { (*p).sequence }, 2, "end makes it even, +2 total");
        assert_eq!(
            read_seqcount_retry(p, 0),
            1,
            "the original stable snapshot (0) is now stale → retry 1"
        );
    }

    // Helpers (pure — read the counter word without the spinning begin path).
    fn sl_seq(sl: &SeqLock) -> u32 {
        sl.seqcount.sequence
    }
    fn read_begin_pure(sl: &SeqLock) -> u32 {
        // Equivalent to read_seqbegin on an even (stable) counter, without the
        // cooperative-yield spin that would hit the msleep syscall.
        let v = sl.seqcount.sequence;
        debug_assert_eq!(v & 1, 0, "test helper assumes a stable (even) counter");
        v
    }
}
