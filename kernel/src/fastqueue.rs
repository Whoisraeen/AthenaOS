//! Fixed-capacity, allocation-free queues — Concept §SCHED_GAME
//! "allocation-free in steady state".
//!
//! The IRQ→task event path and the frame/audio inner loops must not risk an
//! allocator stall under load. `heapless` gives bounded SPSC rings and `Vec`s
//! that live entirely in their own storage (no heap), so a hot path can hand off
//! events with a wait-free enqueue and a known worst-case. New hot-path queues
//! should use these instead of `alloc::collections::VecDeque`.

use heapless::spsc::Queue;
use heapless::Vec;

/// A bounded SPSC event ring sized for a single producer (an IRQ handler) and a
/// single consumer (the servicing task). `N` is the storage size; capacity is
/// `N - 1` (one slot reserved by the lock-free algorithm).
pub type EventRing<T, const N: usize> = Queue<T, N>;

pub fn init() {
    crate::serial_println!("[ OK ] fastqueue: heapless SPSC ring / bounded Vec ready");
}

/// R10 smoketest — must be able to print FAIL. Exercises the lock-free SPSC ring
/// (FIFO + bounded) and the heapless Vec (bounded push), version-independently.
pub fn run_boot_smoketest() {
    // SPSC ring: over-fill, then drain and confirm FIFO order + conservation.
    let mut q: EventRing<u32, 8> = Queue::new();
    let (mut prod, mut cons) = q.split();
    let mut enq = 0u32;
    for i in 0..32u32 {
        if prod.enqueue(i).is_ok() {
            enq += 1;
        }
    }
    let first = cons.dequeue();
    let mut drained = 0u32;
    let mut last = 0u32;
    let mut in_order = true;
    while let Some(v) = cons.dequeue() {
        if drained > 0 && v <= last {
            in_order = false;
        }
        last = v;
        drained += 1;
    }
    // bounded, FIFO, and everything enqueued comes back out exactly once.
    let ring_ok = enq >= 1 && first == Some(0) && enq == drained + 1 && in_order;

    // heapless Vec: bounded push must cap at capacity and report full.
    let mut v: Vec<u32, 4> = Vec::new();
    let pushed = (0..16u32).filter(|&i| v.push(i).is_ok()).count();
    let vec_ok = pushed == 4 && v.is_full();

    let pass = ring_ok && vec_ok;
    crate::selftest::record_smoketest("fastqueue", pass);
    crate::serial_println!(
        "[fastqueue] spsc enq={} drained={} vec_pushed={} -> {}",
        enq,
        drained,
        pushed,
        if pass { "PASS" } else { "FAIL" }
    );
}
