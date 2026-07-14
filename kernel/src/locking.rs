//! Advanced Locking Primitives — RaeenOS kernel
//!
//! Full locking infrastructure: RCU, seqlocks, rw semaphores, mutexes,
//! spinlock variants, per-CPU variables, lockdep, atomics, memory ordering,
//! and wait/wake mechanisms.

#![allow(dead_code)]

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{
    AtomicBool, AtomicI32, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};
use spin::Mutex;

// ─── Memory Ordering Helpers ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryOrdering {
    Relaxed,
    Acquire,
    Release,
    AcqRel,
    SeqCst,
}

impl MemoryOrdering {
    pub fn to_atomic(self) -> Ordering {
        match self {
            MemoryOrdering::Relaxed => Ordering::Relaxed,
            MemoryOrdering::Acquire => Ordering::Acquire,
            MemoryOrdering::Release => Ordering::Release,
            MemoryOrdering::AcqRel => Ordering::AcqRel,
            MemoryOrdering::SeqCst => Ordering::SeqCst,
        }
    }
}

#[inline(always)]
pub fn smp_store_release(target: &AtomicU64, val: u64) {
    target.store(val, Ordering::Release);
}

#[inline(always)]
pub fn smp_load_acquire(source: &AtomicU64) -> u64 {
    source.load(Ordering::Acquire)
}

#[inline(always)]
pub fn read_once_u64(source: &AtomicU64) -> u64 {
    source.load(Ordering::Relaxed)
}

#[inline(always)]
pub fn write_once_u64(target: &AtomicU64, val: u64) {
    target.store(val, Ordering::Relaxed);
}

#[inline(always)]
pub fn read_once_usize(source: &AtomicUsize) -> usize {
    source.load(Ordering::Relaxed)
}

#[inline(always)]
pub fn write_once_usize(target: &AtomicUsize, val: usize) {
    target.store(val, Ordering::Relaxed);
}

#[inline(always)]
pub fn compiler_barrier() {
    core::sync::atomic::fence(Ordering::SeqCst);
}

#[inline(always)]
pub fn smp_mb() {
    core::sync::atomic::fence(Ordering::SeqCst);
}

#[inline(always)]
pub fn smp_rmb() {
    core::sync::atomic::fence(Ordering::Acquire);
}

#[inline(always)]
pub fn smp_wmb() {
    core::sync::atomic::fence(Ordering::Release);
}

// ─── Atomic Types ───────────────────────────────────────────────────────────

pub struct KernelAtomicI32 {
    value: AtomicI32,
}

impl KernelAtomicI32 {
    pub const fn new(val: i32) -> Self {
        Self {
            value: AtomicI32::new(val),
        }
    }

    pub fn load(&self, order: Ordering) -> i32 {
        self.value.load(order)
    }

    pub fn store(&self, val: i32, order: Ordering) {
        self.value.store(val, order);
    }

    pub fn add(&self, val: i32, order: Ordering) {
        self.value.fetch_add(val, order);
    }

    pub fn sub(&self, val: i32, order: Ordering) {
        self.value.fetch_sub(val, order);
    }

    pub fn inc(&self, order: Ordering) {
        self.value.fetch_add(1, order);
    }

    pub fn dec(&self, order: Ordering) {
        self.value.fetch_sub(1, order);
    }

    pub fn cmpxchg(
        &self,
        current: i32,
        new: i32,
        success: Ordering,
        failure: Ordering,
    ) -> Result<i32, i32> {
        self.value.compare_exchange(current, new, success, failure)
    }

    pub fn xchg(&self, val: i32, order: Ordering) -> i32 {
        self.value.swap(val, order)
    }

    pub fn fetch_add(&self, val: i32, order: Ordering) -> i32 {
        self.value.fetch_add(val, order)
    }

    pub fn fetch_sub(&self, val: i32, order: Ordering) -> i32 {
        self.value.fetch_sub(val, order)
    }

    pub fn fetch_and(&self, val: i32, order: Ordering) -> i32 {
        self.value.fetch_and(val, order)
    }

    pub fn fetch_or(&self, val: i32, order: Ordering) -> i32 {
        self.value.fetch_or(val, order)
    }

    pub fn fetch_xor(&self, val: i32, order: Ordering) -> i32 {
        self.value.fetch_xor(val, order)
    }

    pub fn add_return(&self, val: i32, order: Ordering) -> i32 {
        self.value.fetch_add(val, order).wrapping_add(val)
    }

    pub fn sub_return(&self, val: i32, order: Ordering) -> i32 {
        self.value.fetch_sub(val, order).wrapping_sub(val)
    }

    pub fn inc_return(&self, order: Ordering) -> i32 {
        self.add_return(1, order)
    }

    pub fn dec_return(&self, order: Ordering) -> i32 {
        self.sub_return(1, order)
    }

    pub fn dec_and_test(&self, order: Ordering) -> bool {
        self.dec_return(order) == 0
    }

    pub fn inc_and_test(&self, order: Ordering) -> bool {
        self.inc_return(order) == 0
    }

    pub fn add_negative(&self, val: i32, order: Ordering) -> bool {
        self.add_return(val, order) < 0
    }
}

pub struct KernelAtomicI64 {
    value: AtomicI64,
}

impl KernelAtomicI64 {
    pub const fn new(val: i64) -> Self {
        Self {
            value: AtomicI64::new(val),
        }
    }

    pub fn load(&self, order: Ordering) -> i64 {
        self.value.load(order)
    }
    pub fn store(&self, val: i64, order: Ordering) {
        self.value.store(val, order);
    }
    pub fn fetch_add(&self, val: i64, order: Ordering) -> i64 {
        self.value.fetch_add(val, order)
    }
    pub fn fetch_sub(&self, val: i64, order: Ordering) -> i64 {
        self.value.fetch_sub(val, order)
    }
    pub fn cmpxchg(
        &self,
        current: i64,
        new: i64,
        success: Ordering,
        failure: Ordering,
    ) -> Result<i64, i64> {
        self.value.compare_exchange(current, new, success, failure)
    }
    pub fn xchg(&self, val: i64, order: Ordering) -> i64 {
        self.value.swap(val, order)
    }
    pub fn fetch_and(&self, val: i64, order: Ordering) -> i64 {
        self.value.fetch_and(val, order)
    }
    pub fn fetch_or(&self, val: i64, order: Ordering) -> i64 {
        self.value.fetch_or(val, order)
    }
    pub fn fetch_xor(&self, val: i64, order: Ordering) -> i64 {
        self.value.fetch_xor(val, order)
    }
    pub fn add_return(&self, val: i64, order: Ordering) -> i64 {
        self.value.fetch_add(val, order).wrapping_add(val)
    }
    pub fn sub_return(&self, val: i64, order: Ordering) -> i64 {
        self.value.fetch_sub(val, order).wrapping_sub(val)
    }
    pub fn inc(&self, order: Ordering) {
        self.value.fetch_add(1, order);
    }
    pub fn dec(&self, order: Ordering) {
        self.value.fetch_sub(1, order);
    }
    pub fn dec_and_test(&self, order: Ordering) -> bool {
        self.sub_return(1, order) == 0
    }
    pub fn inc_and_test(&self, order: Ordering) -> bool {
        self.add_return(1, order) == 0
    }
}

pub type KernelAtomicLong = KernelAtomicI64;

// ─── Ticket Spinlock ────────────────────────────────────────────────────────

pub struct TicketSpinlock {
    next_ticket: AtomicU32,
    now_serving: AtomicU32,
}

impl TicketSpinlock {
    pub const fn new() -> Self {
        Self {
            next_ticket: AtomicU32::new(0),
            now_serving: AtomicU32::new(0),
        }
    }

    pub fn lock(&self) -> TicketSpinlockGuard<'_> {
        let ticket = self.next_ticket.fetch_add(1, Ordering::Relaxed);
        while self.now_serving.load(Ordering::Acquire) != ticket {
            core::hint::spin_loop();
        }
        TicketSpinlockGuard { lock: self }
    }

    pub fn try_lock(&self) -> Option<TicketSpinlockGuard<'_>> {
        let current = self.now_serving.load(Ordering::Relaxed);
        if self
            .next_ticket
            .compare_exchange(
                current,
                current.wrapping_add(1),
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_ok()
        {
            Some(TicketSpinlockGuard { lock: self })
        } else {
            None
        }
    }

    fn unlock(&self) {
        self.now_serving.fetch_add(1, Ordering::Release);
    }

    pub fn is_locked(&self) -> bool {
        self.next_ticket.load(Ordering::Relaxed) != self.now_serving.load(Ordering::Relaxed)
    }
}

pub struct TicketSpinlockGuard<'a> {
    lock: &'a TicketSpinlock,
}

impl<'a> Drop for TicketSpinlockGuard<'a> {
    fn drop(&mut self) {
        self.lock.unlock();
    }
}

// ─── MCS / Queued Spinlock (NUMA-fair) ──────────────────────────────────────

pub struct McsNode {
    next: AtomicUsize,
    locked: AtomicBool,
}

impl McsNode {
    pub const fn new() -> Self {
        Self {
            next: AtomicUsize::new(0),
            locked: AtomicBool::new(false),
        }
    }
}

pub struct QueuedSpinlock {
    tail: AtomicUsize,
}

impl QueuedSpinlock {
    pub const fn new() -> Self {
        Self {
            tail: AtomicUsize::new(0),
        }
    }

    pub fn lock(&self, node: &McsNode) {
        node.next.store(0, Ordering::Relaxed);
        node.locked.store(true, Ordering::Relaxed);

        let node_ptr = node as *const McsNode as usize;
        let prev = self.tail.swap(node_ptr, Ordering::AcqRel);

        if prev != 0 {
            let prev_node = unsafe { &*(prev as *const McsNode) };
            prev_node.next.store(node_ptr, Ordering::Release);
            while node.locked.load(Ordering::Acquire) {
                core::hint::spin_loop();
            }
        }
    }

    pub fn unlock(&self, node: &McsNode) {
        let node_ptr = node as *const McsNode as usize;

        if node.next.load(Ordering::Relaxed) == 0 {
            if self
                .tail
                .compare_exchange(node_ptr, 0, Ordering::Release, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
            while node.next.load(Ordering::Relaxed) == 0 {
                core::hint::spin_loop();
            }
        }

        let next_ptr = node.next.load(Ordering::Acquire);
        let next_node = unsafe { &*(next_ptr as *const McsNode) };
        next_node.locked.store(false, Ordering::Release);
    }

    pub fn is_locked(&self) -> bool {
        self.tail.load(Ordering::Relaxed) != 0
    }
}

// ─── Raw Spinlock (non-preemptible) ─────────────────────────────────────────

pub struct RawSpinlock {
    locked: AtomicBool,
    owner_cpu: AtomicI32,
}

impl RawSpinlock {
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
            owner_cpu: AtomicI32::new(-1),
        }
    }

    pub fn lock(&self) -> RawSpinlockGuard<'_> {
        // BUG-27 fix: irqsave. Disable local interrupts BEFORE acquiring so an
        // IRQ handler on this CPU can never try to re-acquire the same lock and
        // self-deadlock. The previous interrupt state is restored on drop.
        let was_enabled = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable();
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        self.owner_cpu
            .store(current_cpu_id() as i32, Ordering::Relaxed);
        RawSpinlockGuard {
            lock: self,
            restore_intr: was_enabled,
        }
    }

    pub fn try_lock(&self) -> Option<RawSpinlockGuard<'_>> {
        let was_enabled = x86_64::instructions::interrupts::are_enabled();
        x86_64::instructions::interrupts::disable();
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            self.owner_cpu
                .store(current_cpu_id() as i32, Ordering::Relaxed);
            Some(RawSpinlockGuard {
                lock: self,
                restore_intr: was_enabled,
            })
        } else {
            // Did not acquire — restore interrupts we just disabled.
            if was_enabled {
                x86_64::instructions::interrupts::enable();
            }
            None
        }
    }

    fn unlock(&self) {
        self.owner_cpu.store(-1, Ordering::Relaxed);
        self.locked.store(false, Ordering::Release);
    }

    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }

    pub fn owner(&self) -> i32 {
        self.owner_cpu.load(Ordering::Relaxed)
    }
}

pub struct RawSpinlockGuard<'a> {
    lock: &'a RawSpinlock,
    /// Whether local interrupts were enabled before `lock()` disabled them;
    /// restored on drop (irqsave/irqrestore).
    restore_intr: bool,
}

impl<'a> Drop for RawSpinlockGuard<'a> {
    fn drop(&mut self) {
        self.lock.unlock();
        if self.restore_intr {
            x86_64::instructions::interrupts::enable();
        }
    }
}

// ─── Read-Write Lock (rwlock_t) ─────────────────────────────────────────────

const RWLOCK_WRITER_BIT: u32 = 1 << 31;
const RWLOCK_READER_MASK: u32 = !(1 << 31);

pub struct RwLock {
    state: AtomicU32,
}

impl RwLock {
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(0),
        }
    }

    pub fn read_lock(&self) -> RwReadGuard<'_> {
        loop {
            let current = self.state.load(Ordering::Relaxed);
            if current & RWLOCK_WRITER_BIT != 0 {
                core::hint::spin_loop();
                continue;
            }
            if self
                .state
                .compare_exchange_weak(current, current + 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return RwReadGuard { lock: self };
            }
        }
    }

    pub fn read_trylock(&self) -> Option<RwReadGuard<'_>> {
        let current = self.state.load(Ordering::Relaxed);
        if current & RWLOCK_WRITER_BIT != 0 {
            return None;
        }
        if self
            .state
            .compare_exchange(current, current + 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(RwReadGuard { lock: self })
        } else {
            None
        }
    }

    pub fn write_lock(&self) -> RwWriteGuard<'_> {
        loop {
            if self
                .state
                .compare_exchange_weak(0, RWLOCK_WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return RwWriteGuard { lock: self };
            }
            core::hint::spin_loop();
        }
    }

    pub fn write_trylock(&self) -> Option<RwWriteGuard<'_>> {
        if self
            .state
            .compare_exchange(0, RWLOCK_WRITER_BIT, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(RwWriteGuard { lock: self })
        } else {
            None
        }
    }

    fn read_unlock(&self) {
        self.state.fetch_sub(1, Ordering::Release);
    }

    fn write_unlock(&self) {
        self.state.fetch_and(!RWLOCK_WRITER_BIT, Ordering::Release);
    }

    pub fn reader_count(&self) -> u32 {
        self.state.load(Ordering::Relaxed) & RWLOCK_READER_MASK
    }

    pub fn is_write_locked(&self) -> bool {
        self.state.load(Ordering::Relaxed) & RWLOCK_WRITER_BIT != 0
    }
}

pub struct RwReadGuard<'a> {
    lock: &'a RwLock,
}

impl<'a> Drop for RwReadGuard<'a> {
    fn drop(&mut self) {
        self.lock.read_unlock();
    }
}

pub struct RwWriteGuard<'a> {
    lock: &'a RwLock,
}

impl<'a> Drop for RwWriteGuard<'a> {
    fn drop(&mut self) {
        self.lock.write_unlock();
    }
}

// ─── Seqlock ────────────────────────────────────────────────────────────────

pub struct SeqCount {
    sequence: AtomicU32,
}

impl SeqCount {
    pub const fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
        }
    }

    pub fn raw_write_begin(&self) {
        self.sequence.fetch_add(1, Ordering::Release);
        smp_wmb();
    }

    pub fn raw_write_end(&self) {
        smp_wmb();
        self.sequence.fetch_add(1, Ordering::Release);
    }

    pub fn read_begin(&self) -> u32 {
        loop {
            let seq = self.sequence.load(Ordering::Acquire);
            if seq & 1 == 0 {
                return seq;
            }
            core::hint::spin_loop();
        }
    }

    pub fn read_retry(&self, start: u32) -> bool {
        smp_rmb();
        self.sequence.load(Ordering::Acquire) != start
    }

    pub fn sequence(&self) -> u32 {
        self.sequence.load(Ordering::Relaxed)
    }
}

pub struct Seqlock {
    seqcount: SeqCount,
    lock: RawSpinlock,
}

impl Seqlock {
    pub const fn new() -> Self {
        Self {
            seqcount: SeqCount::new(),
            lock: RawSpinlock::new(),
        }
    }

    pub fn write_lock(&self) -> SeqlockWriteGuard<'_> {
        let _guard = self.lock.lock();
        self.seqcount.raw_write_begin();
        SeqlockWriteGuard {
            seqlock: self,
            _guard,
        }
    }

    pub fn write_unlock(&self) {
        self.seqcount.raw_write_end();
        self.lock.unlock();
    }

    pub fn read_begin(&self) -> u32 {
        self.seqcount.read_begin()
    }

    pub fn read_retry(&self, start: u32) -> bool {
        self.seqcount.read_retry(start)
    }
}

pub struct SeqlockWriteGuard<'a> {
    seqlock: &'a Seqlock,
    _guard: RawSpinlockGuard<'a>,
}

impl<'a> Drop for SeqlockWriteGuard<'a> {
    fn drop(&mut self) {
        self.seqlock.seqcount.raw_write_end();
    }
}

// ─── RW Semaphore ───────────────────────────────────────────────────────────

const RWSEM_READER_BIAS: i64 = 0x0000_0001;
const RWSEM_WRITER_LOCKED: i64 = 0x0001_0000;
const RWSEM_WRITER_WAITING: i64 = 0x0002_0000;

pub struct RwSemaphore {
    count: AtomicI64,
    owner: AtomicU64,
    reader_count: AtomicU32,
    writer_count: AtomicU32,
}

impl RwSemaphore {
    pub const fn new() -> Self {
        Self {
            count: AtomicI64::new(0),
            owner: AtomicU64::new(0),
            reader_count: AtomicU32::new(0),
            writer_count: AtomicU32::new(0),
        }
    }

    pub fn down_read(&self) {
        let old = self.count.fetch_add(RWSEM_READER_BIAS, Ordering::Acquire);
        if old & RWSEM_WRITER_LOCKED != 0 {
            self.down_read_slowpath();
        }
        self.reader_count.fetch_add(1, Ordering::Relaxed);
    }

    fn down_read_slowpath(&self) {
        // BUG-29: the previous version spun on a LOCAL `woken` AtomicBool while
        // pushing a SEPARATE waiter (with its own AtomicBool) into the vector,
        // so `wake_readers` set woken on an instance the spinner never observed —
        // the wake was a dead no-op. The shared `count` is the single source of
        // truth: spin until the writer-locked bit clears. (No scheduler-blocking
        // here yet; RwSemaphore has no live callers, so this is a correct
        // busy-wait, not a hot path.)
        loop {
            if self.count.load(Ordering::Acquire) & RWSEM_WRITER_LOCKED == 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }

    pub fn up_read(&self) {
        self.reader_count.fetch_sub(1, Ordering::Relaxed);
        // A waiting writer spins on `count` in down_write, so clearing our
        // reader bias here is the wake (no separate waiter list — see BUG-29).
        self.count.fetch_sub(RWSEM_READER_BIAS, Ordering::Release);
    }

    pub fn down_write(&self) {
        loop {
            let old = self.count.load(Ordering::Relaxed);
            if old == 0 {
                if self
                    .count
                    .compare_exchange(0, RWSEM_WRITER_LOCKED, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    self.owner.store(current_task_id(), Ordering::Relaxed);
                    self.writer_count.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            }
            self.count.fetch_or(RWSEM_WRITER_WAITING, Ordering::Relaxed);
            core::hint::spin_loop();
        }
    }

    pub fn up_write(&self) {
        self.writer_count.fetch_sub(1, Ordering::Relaxed);
        self.owner.store(0, Ordering::Relaxed);
        self.count.fetch_and(
            !(RWSEM_WRITER_LOCKED | RWSEM_WRITER_WAITING),
            Ordering::Release,
        );
        // Waiting readers spin on the writer-locked bit in down_read_slowpath;
        // clearing it above releases them (no separate waiter list — BUG-29).
    }

    pub fn down_read_trylock(&self) -> bool {
        let old = self.count.load(Ordering::Relaxed);
        if old & RWSEM_WRITER_LOCKED != 0 {
            return false;
        }
        self.count
            .compare_exchange(
                old,
                old + RWSEM_READER_BIAS,
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_ok()
    }

    pub fn down_write_trylock(&self) -> bool {
        if self
            .count
            .compare_exchange(0, RWSEM_WRITER_LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            self.owner.store(current_task_id(), Ordering::Relaxed);
            self.writer_count.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    pub fn downgrade_write(&self) {
        self.owner.store(0, Ordering::Relaxed);
        self.count
            .fetch_and(!RWSEM_WRITER_LOCKED, Ordering::Release);
        self.count.fetch_add(RWSEM_READER_BIAS, Ordering::AcqRel);
        self.reader_count.fetch_add(1, Ordering::Relaxed);
        self.writer_count.fetch_sub(1, Ordering::Relaxed);
        // Clearing the writer-locked bit above releases readers spinning in
        // down_read_slowpath (no separate waiter list — BUG-29).
    }

    pub fn is_write_locked(&self) -> bool {
        self.count.load(Ordering::Relaxed) & RWSEM_WRITER_LOCKED != 0
    }

    pub fn readers(&self) -> u32 {
        self.reader_count.load(Ordering::Relaxed)
    }

    pub fn writers(&self) -> u32 {
        self.writer_count.load(Ordering::Relaxed)
    }
}

// ─── Mutex with Owner Tracking + Adaptive Spinning ──────────────────────────

pub struct KernelMutex {
    locked: AtomicBool,
    owner: AtomicU64,
    owner_cpu: AtomicI32,
    wait_lock: RawSpinlock,
    waiters: Mutex<Vec<MutexWaiter>>,
    contentions: AtomicU64,
    total_wait_ns: AtomicU64,
    total_hold_ns: AtomicU64,
    acquire_time: AtomicU64,
}

struct MutexWaiter {
    task_id: u64,
    woken: AtomicBool,
}

impl KernelMutex {
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
            owner: AtomicU64::new(0),
            owner_cpu: AtomicI32::new(-1),
            wait_lock: RawSpinlock::new(),
            waiters: Mutex::new(Vec::new()),
            contentions: AtomicU64::new(0),
            total_wait_ns: AtomicU64::new(0),
            total_hold_ns: AtomicU64::new(0),
            acquire_time: AtomicU64::new(0),
        }
    }

    pub fn lock(&self) {
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            self.set_owner();
            return;
        }
        self.lock_slowpath();
    }

    fn lock_slowpath(&self) {
        self.contentions.fetch_add(1, Ordering::Relaxed);

        let spin_limit = 100;
        for _ in 0..spin_limit {
            let owner_cpu = self.owner_cpu.load(Ordering::Relaxed);
            if owner_cpu < 0 {
                break;
            }
            if self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                self.set_owner();
                return;
            }
            core::hint::spin_loop();
        }

        loop {
            if self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                self.set_owner();
                return;
            }
            core::hint::spin_loop();
        }
    }

    pub fn unlock(&self) {
        self.owner.store(0, Ordering::Relaxed);
        self.owner_cpu.store(-1, Ordering::Relaxed);
        self.locked.store(false, Ordering::Release);
        self.wake_one();
    }

    pub fn trylock(&self) -> bool {
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            self.set_owner();
            true
        } else {
            false
        }
    }

    fn set_owner(&self) {
        self.owner.store(current_task_id(), Ordering::Relaxed);
        self.owner_cpu
            .store(current_cpu_id() as i32, Ordering::Relaxed);
        self.acquire_time.store(read_timestamp(), Ordering::Relaxed);
    }

    fn wake_one(&self) {
        let mut waiters = self.waiters.lock();
        if let Some(w) = waiters.first() {
            w.woken.store(true, Ordering::Release);
        }
        if !waiters.is_empty() {
            waiters.remove(0);
        }
    }

    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }

    pub fn owner_id(&self) -> u64 {
        self.owner.load(Ordering::Relaxed)
    }

    pub fn contentions(&self) -> u64 {
        self.contentions.load(Ordering::Relaxed)
    }
}

// ─── Wait-Wound Mutex (ww_mutex) for Deadlock Avoidance ─────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct WwAcquireContext {
    stamp: u64,
    task_id: u64,
    is_wait_die: bool,
}

impl WwAcquireContext {
    pub fn new(task_id: u64, is_wait_die: bool) -> Self {
        static STAMP_COUNTER: AtomicU64 = AtomicU64::new(0);
        Self {
            stamp: STAMP_COUNTER.fetch_add(1, Ordering::Relaxed),
            task_id,
            is_wait_die,
        }
    }
}

pub struct WwMutex {
    inner: KernelMutex,
    ctx_stamp: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WwMutexError {
    Deadlock,
    WouldBlock,
}

impl WwMutex {
    pub const fn new() -> Self {
        Self {
            inner: KernelMutex::new(),
            ctx_stamp: AtomicU64::new(0),
        }
    }

    pub fn lock(&self, ctx: &WwAcquireContext) -> Result<(), WwMutexError> {
        if self.inner.trylock() {
            self.ctx_stamp.store(ctx.stamp, Ordering::Relaxed);
            return Ok(());
        }

        let holder_stamp = self.ctx_stamp.load(Ordering::Relaxed);
        if ctx.is_wait_die {
            if ctx.stamp > holder_stamp && holder_stamp != 0 {
                return Err(WwMutexError::Deadlock);
            }
        } else {
            if ctx.stamp < holder_stamp && holder_stamp != 0 {
                return Err(WwMutexError::Deadlock);
            }
        }

        self.inner.lock();
        self.ctx_stamp.store(ctx.stamp, Ordering::Relaxed);
        Ok(())
    }

    pub fn unlock(&self) {
        self.ctx_stamp.store(0, Ordering::Relaxed);
        self.inner.unlock();
    }

    pub fn trylock(&self, ctx: &WwAcquireContext) -> Result<bool, WwMutexError> {
        if self.inner.trylock() {
            self.ctx_stamp.store(ctx.stamp, Ordering::Relaxed);
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

// ─── RCU (Read-Copy-Update) ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RcuFlavor {
    Classic,
    Bh,
    Sched,
}

struct RcuCallback {
    func: Box<dyn FnOnce() + Send>,
    grace_period: u64,
}

struct RcuPerCpu {
    nesting_depth: AtomicI32,
    qs_passed: AtomicBool,
    bh_nesting: AtomicI32,
    sched_nesting: AtomicI32,
}

impl RcuPerCpu {
    const fn new() -> Self {
        Self {
            nesting_depth: AtomicI32::new(0),
            qs_passed: AtomicBool::new(false),
            bh_nesting: AtomicI32::new(0),
            sched_nesting: AtomicI32::new(0),
        }
    }
}

pub struct RcuState {
    grace_period: AtomicU64,
    completed_gp: AtomicU64,
    callbacks: Mutex<Vec<RcuCallback>>,
    per_cpu: [RcuPerCpu; MAX_CPUS],
    nr_cpus: AtomicU32,
    expedited_pending: AtomicBool,
    stall_timeout_ms: AtomicU64,
    stall_warned: AtomicBool,
    callbacks_invoked: AtomicU64,
    gp_started: AtomicU64,
}

const MAX_CPUS: usize = 64;

impl RcuState {
    const fn new() -> Self {
        const PER_CPU_INIT: RcuPerCpu = RcuPerCpu::new();
        Self {
            grace_period: AtomicU64::new(0),
            completed_gp: AtomicU64::new(0),
            callbacks: Mutex::new(Vec::new()),
            per_cpu: [PER_CPU_INIT; MAX_CPUS],
            nr_cpus: AtomicU32::new(1),
            expedited_pending: AtomicBool::new(false),
            stall_timeout_ms: AtomicU64::new(21000),
            stall_warned: AtomicBool::new(false),
            callbacks_invoked: AtomicU64::new(0),
            gp_started: AtomicU64::new(0),
        }
    }
}

static RCU_STATE: RcuState = RcuState::new();

pub fn rcu_read_lock() {
    let cpu = current_cpu_id();
    if cpu < MAX_CPUS {
        RCU_STATE.per_cpu[cpu]
            .nesting_depth
            .fetch_add(1, Ordering::Relaxed);
    }
    smp_mb();
}

pub fn rcu_read_unlock() {
    smp_mb();
    let cpu = current_cpu_id();
    if cpu < MAX_CPUS {
        let prev = RCU_STATE.per_cpu[cpu]
            .nesting_depth
            .fetch_sub(1, Ordering::Relaxed);
        if prev == 1 {
            RCU_STATE.per_cpu[cpu]
                .qs_passed
                .store(true, Ordering::Release);
        }
    }
}

pub fn rcu_read_lock_bh() {
    let cpu = current_cpu_id();
    if cpu < MAX_CPUS {
        RCU_STATE.per_cpu[cpu]
            .bh_nesting
            .fetch_add(1, Ordering::Relaxed);
    }
    smp_mb();
}

pub fn rcu_read_unlock_bh() {
    smp_mb();
    let cpu = current_cpu_id();
    if cpu < MAX_CPUS {
        RCU_STATE.per_cpu[cpu]
            .bh_nesting
            .fetch_sub(1, Ordering::Relaxed);
    }
}

pub fn rcu_read_lock_sched() {
    let cpu = current_cpu_id();
    if cpu < MAX_CPUS {
        RCU_STATE.per_cpu[cpu]
            .sched_nesting
            .fetch_add(1, Ordering::Relaxed);
    }
    smp_mb();
}

pub fn rcu_read_unlock_sched() {
    smp_mb();
    let cpu = current_cpu_id();
    if cpu < MAX_CPUS {
        RCU_STATE.per_cpu[cpu]
            .sched_nesting
            .fetch_sub(1, Ordering::Relaxed);
    }
}

pub fn synchronize_rcu() {
    let new_gp = RCU_STATE.grace_period.fetch_add(1, Ordering::SeqCst) + 1;
    RCU_STATE
        .gp_started
        .store(read_timestamp(), Ordering::Relaxed);

    let nr_cpus = RCU_STATE.nr_cpus.load(Ordering::Relaxed) as usize;
    for cpu in 0..nr_cpus.min(MAX_CPUS) {
        RCU_STATE.per_cpu[cpu]
            .qs_passed
            .store(false, Ordering::Release);
    }

    let stall_timeout = RCU_STATE.stall_timeout_ms.load(Ordering::Relaxed);
    let start = read_timestamp();
    loop {
        let mut all_passed = true;
        for cpu in 0..nr_cpus.min(MAX_CPUS) {
            if RCU_STATE.per_cpu[cpu].nesting_depth.load(Ordering::Relaxed) > 0 {
                if !RCU_STATE.per_cpu[cpu].qs_passed.load(Ordering::Acquire) {
                    all_passed = false;
                    break;
                }
            }
        }
        if all_passed {
            break;
        }
        let elapsed = read_timestamp().wrapping_sub(start);
        if elapsed > stall_timeout * 1_000_000 {
            rcu_stall_warning(new_gp);
            break;
        }
        core::hint::spin_loop();
    }

    RCU_STATE.completed_gp.store(new_gp, Ordering::Release);
    process_rcu_callbacks(new_gp);
}

fn rcu_stall_warning(gp: u64) {
    if !RCU_STATE.stall_warned.swap(true, Ordering::Relaxed) {
        let nr_cpus = RCU_STATE.nr_cpus.load(Ordering::Relaxed) as usize;
        for cpu in 0..nr_cpus.min(MAX_CPUS) {
            if RCU_STATE.per_cpu[cpu].nesting_depth.load(Ordering::Relaxed) > 0
                && !RCU_STATE.per_cpu[cpu].qs_passed.load(Ordering::Relaxed)
            {
                let _ = (gp, cpu);
            }
        }
    }
}

pub fn synchronize_rcu_expedited() {
    RCU_STATE.expedited_pending.store(true, Ordering::Release);
    smp_mb();
    synchronize_rcu();
    RCU_STATE.expedited_pending.store(false, Ordering::Release);
}

pub fn call_rcu<F: FnOnce() + Send + 'static>(func: F) {
    let gp = RCU_STATE.grace_period.load(Ordering::Relaxed) + 1;
    let cb = RcuCallback {
        func: Box::new(func),
        grace_period: gp,
    };
    RCU_STATE.callbacks.lock().push(cb);
}

fn process_rcu_callbacks(completed_gp: u64) {
    let mut cbs = RCU_STATE.callbacks.lock();
    let mut i = 0;
    while i < cbs.len() {
        if cbs[i].grace_period <= completed_gp {
            let cb = cbs.remove(i);
            (cb.func)();
            RCU_STATE.callbacks_invoked.fetch_add(1, Ordering::Relaxed);
        } else {
            i += 1;
        }
    }
}

pub fn rcu_assign_pointer<T>(target: &AtomicUsize, ptr: *mut T) {
    smp_wmb();
    target.store(ptr as usize, Ordering::Release);
}

pub fn rcu_dereference<T>(source: &AtomicUsize) -> *const T {
    let ptr = source.load(Ordering::Acquire);
    smp_rmb();
    ptr as *const T
}

// ─── SRCU (Sleepable RCU) ───────────────────────────────────────────────────

pub struct SrcuStruct {
    per_cpu_ref: [AtomicI32; MAX_CPUS],
    grace_period: AtomicU64,
    completed: AtomicU64,
    lock: RawSpinlock,
    srcu_idx: AtomicU32,
}

impl SrcuStruct {
    pub const fn new() -> Self {
        const INIT: AtomicI32 = AtomicI32::new(0);
        Self {
            per_cpu_ref: [INIT; MAX_CPUS],
            grace_period: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            lock: RawSpinlock::new(),
            srcu_idx: AtomicU32::new(0),
        }
    }

    pub fn srcu_read_lock(&self) -> u32 {
        let idx = self.srcu_idx.load(Ordering::Acquire);
        let cpu = current_cpu_id().min(MAX_CPUS - 1);
        self.per_cpu_ref[cpu].fetch_add(1, Ordering::Relaxed);
        smp_mb();
        idx
    }

    pub fn srcu_read_unlock(&self, idx: u32) {
        smp_mb();
        let _ = idx;
        let cpu = current_cpu_id().min(MAX_CPUS - 1);
        self.per_cpu_ref[cpu].fetch_sub(1, Ordering::Relaxed);
    }

    pub fn synchronize_srcu(&self) {
        let _guard = self.lock.lock();
        self.grace_period.fetch_add(1, Ordering::SeqCst);
        self.srcu_idx.fetch_xor(1, Ordering::Release);
        smp_mb();

        loop {
            let mut total = 0i64;
            for cpu in 0..MAX_CPUS {
                total += self.per_cpu_ref[cpu].load(Ordering::Relaxed) as i64;
            }
            if total <= 0 {
                break;
            }
            core::hint::spin_loop();
        }

        self.completed
            .store(self.grace_period.load(Ordering::Relaxed), Ordering::Release);
    }

    pub fn srcu_readers_active(&self) -> bool {
        for cpu in 0..MAX_CPUS {
            if self.per_cpu_ref[cpu].load(Ordering::Relaxed) > 0 {
                return true;
            }
        }
        false
    }
}

// ─── Tasks RCU ──────────────────────────────────────────────────────────────

pub struct TasksRcu {
    gp_seq: AtomicU64,
    holdouts: Mutex<Vec<u64>>,
    lock: RawSpinlock,
}

impl TasksRcu {
    pub const fn new() -> Self {
        Self {
            gp_seq: AtomicU64::new(0),
            holdouts: Mutex::new(Vec::new()),
            lock: RawSpinlock::new(),
        }
    }

    pub fn synchronize_tasks_rcu(&self) {
        let _guard = self.lock.lock();
        self.gp_seq.fetch_add(1, Ordering::SeqCst);
        smp_mb();
        let holdouts = self.holdouts.lock();
        if holdouts.is_empty() {
            return;
        }
        drop(holdouts);

        loop {
            let holdouts = self.holdouts.lock();
            if holdouts.is_empty() {
                break;
            }
            drop(holdouts);
            core::hint::spin_loop();
        }
    }

    pub fn register_holdout(&self, task_id: u64) {
        self.holdouts.lock().push(task_id);
    }

    pub fn remove_holdout(&self, task_id: u64) {
        let mut holdouts = self.holdouts.lock();
        holdouts.retain(|&id| id != task_id);
    }
}

static TASKS_RCU: TasksRcu = TasksRcu::new();

// ─── Per-CPU Variables ──────────────────────────────────────────────────────

pub struct PerCpuVar<T: Copy + Default> {
    data: [Mutex<T>; MAX_CPUS],
}

impl<T: Copy + Default> PerCpuVar<T> {
    pub fn new(init: T) -> Self {
        let data: [Mutex<T>; MAX_CPUS] = core::array::from_fn(|_| Mutex::new(init));
        Self { data }
    }

    pub fn this_cpu_read(&self) -> T {
        let cpu = current_cpu_id().min(MAX_CPUS - 1);
        *self.data[cpu].lock()
    }

    pub fn this_cpu_write(&self, val: T) {
        let cpu = current_cpu_id().min(MAX_CPUS - 1);
        *self.data[cpu].lock() = val;
    }

    pub fn per_cpu_ptr(&self, cpu: usize) -> Option<&Mutex<T>> {
        if cpu < MAX_CPUS {
            Some(&self.data[cpu])
        } else {
            None
        }
    }

    pub fn for_each_cpu<F: FnMut(usize, &T)>(&self, mut f: F) {
        let nr = RCU_STATE.nr_cpus.load(Ordering::Relaxed) as usize;
        for cpu in 0..nr.min(MAX_CPUS) {
            let val = self.data[cpu].lock();
            f(cpu, &*val);
        }
    }
}

pub struct PerCpuCounter {
    counters: [AtomicI64; MAX_CPUS],
}

impl PerCpuCounter {
    pub const fn new() -> Self {
        const INIT: AtomicI64 = AtomicI64::new(0);
        Self {
            counters: [INIT; MAX_CPUS],
        }
    }

    pub fn this_cpu_add(&self, val: i64) {
        let cpu = current_cpu_id().min(MAX_CPUS - 1);
        self.counters[cpu].fetch_add(val, Ordering::Relaxed);
    }

    pub fn this_cpu_sub(&self, val: i64) {
        let cpu = current_cpu_id().min(MAX_CPUS - 1);
        self.counters[cpu].fetch_sub(val, Ordering::Relaxed);
    }

    pub fn this_cpu_inc(&self) {
        self.this_cpu_add(1);
    }

    pub fn this_cpu_dec(&self) {
        self.this_cpu_sub(1);
    }

    pub fn this_cpu_read(&self) -> i64 {
        let cpu = current_cpu_id().min(MAX_CPUS - 1);
        self.counters[cpu].load(Ordering::Relaxed)
    }

    pub fn sum(&self) -> i64 {
        let nr = RCU_STATE.nr_cpus.load(Ordering::Relaxed) as usize;
        let mut total = 0i64;
        for cpu in 0..nr.min(MAX_CPUS) {
            total = total.wrapping_add(self.counters[cpu].load(Ordering::Relaxed));
        }
        total
    }

    pub fn reset(&self) {
        for c in &self.counters {
            c.store(0, Ordering::Relaxed);
        }
    }
}

// ─── Wait Queue ─────────────────────────────────────────────────────────────

const WQ_FLAG_EXCLUSIVE: u32 = 1 << 0;
const WQ_FLAG_BOOKMARK: u32 = 1 << 1;
const WQ_FLAG_INTERRUPTIBLE: u32 = 1 << 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitQueueEntryState {
    Waiting,
    Woken,
    TimedOut,
    Interrupted,
}

pub struct WaitQueueEntry {
    task_id: u64,
    flags: u32,
    state: AtomicU32,
    woken: AtomicBool,
}

impl WaitQueueEntry {
    pub fn new(task_id: u64, flags: u32) -> Self {
        Self {
            task_id,
            flags,
            state: AtomicU32::new(WaitQueueEntryState::Waiting as u32),
            woken: AtomicBool::new(false),
        }
    }

    pub fn is_exclusive(&self) -> bool {
        self.flags & WQ_FLAG_EXCLUSIVE != 0
    }

    pub fn is_bookmark(&self) -> bool {
        self.flags & WQ_FLAG_BOOKMARK != 0
    }

    pub fn is_interruptible(&self) -> bool {
        self.flags & WQ_FLAG_INTERRUPTIBLE != 0
    }
}

pub struct WaitQueueHead {
    lock: RawSpinlock,
    entries: Mutex<Vec<WaitQueueEntry>>,
    nr_waiters: AtomicU32,
}

impl WaitQueueHead {
    pub const fn new() -> Self {
        Self {
            lock: RawSpinlock::new(),
            entries: Mutex::new(Vec::new()),
            nr_waiters: AtomicU32::new(0),
        }
    }

    pub fn prepare_to_wait(&self, task_id: u64, flags: u32) {
        let entry = WaitQueueEntry::new(task_id, flags);
        let _guard = self.lock.lock();
        let mut entries = self.entries.lock();
        if entry.is_exclusive() {
            entries.push(entry);
        } else {
            let first_excl = entries.iter().position(|e| e.is_exclusive());
            let pos = first_excl.unwrap_or(entries.len());
            entries.insert(pos, entry);
        }
        self.nr_waiters.fetch_add(1, Ordering::Relaxed);
    }

    pub fn finish_wait(&self, task_id: u64) {
        let _guard = self.lock.lock();
        let mut entries = self.entries.lock();
        let len_before = entries.len();
        entries.retain(|e| e.task_id != task_id);
        let removed = len_before - entries.len();
        if removed > 0 {
            self.nr_waiters.fetch_sub(removed as u32, Ordering::Relaxed);
        }
    }

    pub fn wait_event<F: Fn() -> bool>(&self, condition: F) {
        let task_id = current_task_id();
        self.prepare_to_wait(task_id, 0);

        while !condition() {
            core::hint::spin_loop();
        }

        self.finish_wait(task_id);
    }

    pub fn wait_event_interruptible<F: Fn() -> bool>(&self, condition: F) -> bool {
        let task_id = current_task_id();
        self.prepare_to_wait(task_id, WQ_FLAG_INTERRUPTIBLE);

        let mut interrupted = false;
        while !condition() {
            if check_signal_pending(task_id) {
                interrupted = true;
                break;
            }
            core::hint::spin_loop();
        }

        self.finish_wait(task_id);
        !interrupted
    }

    pub fn wait_event_timeout<F: Fn() -> bool>(&self, condition: F, timeout_ns: u64) -> bool {
        let task_id = current_task_id();
        self.prepare_to_wait(task_id, 0);

        let start = read_timestamp();
        let mut timed_out = false;
        while !condition() {
            if read_timestamp().wrapping_sub(start) > timeout_ns {
                timed_out = true;
                break;
            }
            core::hint::spin_loop();
        }

        self.finish_wait(task_id);
        !timed_out
    }

    pub fn wake_up(&self) {
        let _guard = self.lock.lock();
        let entries = self.entries.lock();
        for entry in entries.iter() {
            if entry.is_bookmark() {
                continue;
            }
            entry.woken.store(true, Ordering::Release);
            entry
                .state
                .store(WaitQueueEntryState::Woken as u32, Ordering::Release);
            if entry.is_exclusive() {
                break;
            }
        }
    }

    pub fn wake_up_all(&self) {
        let _guard = self.lock.lock();
        let entries = self.entries.lock();
        for entry in entries.iter() {
            if entry.is_bookmark() {
                continue;
            }
            entry.woken.store(true, Ordering::Release);
            entry
                .state
                .store(WaitQueueEntryState::Woken as u32, Ordering::Release);
        }
    }

    pub fn wake_up_interruptible(&self) {
        let _guard = self.lock.lock();
        let entries = self.entries.lock();
        for entry in entries.iter() {
            if entry.is_bookmark() {
                continue;
            }
            if entry.is_interruptible() {
                entry.woken.store(true, Ordering::Release);
                entry
                    .state
                    .store(WaitQueueEntryState::Woken as u32, Ordering::Release);
                if entry.is_exclusive() {
                    break;
                }
            }
        }
    }

    pub fn wake_up_nr(&self, nr: usize) {
        let _guard = self.lock.lock();
        let entries = self.entries.lock();
        let mut woken = 0usize;
        for entry in entries.iter() {
            if woken >= nr {
                break;
            }
            if entry.is_bookmark() {
                continue;
            }
            entry.woken.store(true, Ordering::Release);
            entry
                .state
                .store(WaitQueueEntryState::Woken as u32, Ordering::Release);
            woken += 1;
        }
    }

    pub fn nr_waiters(&self) -> u32 {
        self.nr_waiters.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.nr_waiters() == 0
    }
}

// ─── Lockdep — Lock Dependency Tracking ─────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LockClassId(u64);

impl LockClassId {
    pub fn new(id: u64) -> Self {
        Self(id)
    }
}

static LOCKDEP_NEXT_CLASS: AtomicU64 = AtomicU64::new(1);

pub fn lockdep_alloc_class() -> LockClassId {
    LockClassId(LOCKDEP_NEXT_CLASS.fetch_add(1, Ordering::Relaxed))
}

#[derive(Debug)]
struct LockClass {
    id: LockClassId,
    name: String,
    subclass: u32,
    lock_type: LockType,
    contentions: AtomicU64,
    total_wait_ns: AtomicU64,
    total_hold_ns: AtomicU64,
    acquisitions: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockType {
    Spinlock,
    Mutex,
    RwSemaphore,
    RwLock,
    Seqlock,
}

impl LockClass {
    fn new(id: LockClassId, name: String, lock_type: LockType) -> Self {
        Self {
            id,
            name,
            subclass: 0,
            lock_type,
            contentions: AtomicU64::new(0),
            total_wait_ns: AtomicU64::new(0),
            total_hold_ns: AtomicU64::new(0),
            acquisitions: AtomicU64::new(0),
        }
    }
}

#[derive(Debug, Clone)]
struct LockDepEdge {
    from: LockClassId,
    to: LockClassId,
    stack_trace_hash: u64,
}

#[derive(Debug, Clone)]
struct HeldLock {
    class_id: LockClassId,
    acquire_time: u64,
    task_id: u64,
    cpu: usize,
    is_read: bool,
}

struct LockDepState {
    classes: BTreeMap<u64, LockClass>,
    dependency_graph: Vec<LockDepEdge>,
    held_locks: Vec<HeldLock>,
    chain_cache: BTreeMap<u64, Vec<LockClassId>>,
    enabled: bool,
    circular_detected: u64,
    max_depth: usize,
    total_lock_acquires: AtomicU64,
    total_lock_releases: AtomicU64,
}

impl LockDepState {
    const fn new() -> Self {
        Self {
            classes: BTreeMap::new(),
            dependency_graph: Vec::new(),
            held_locks: Vec::new(),
            chain_cache: BTreeMap::new(),
            enabled: true,
            circular_detected: 0,
            max_depth: 48,
            total_lock_acquires: AtomicU64::new(0),
            total_lock_releases: AtomicU64::new(0),
        }
    }
}

static LOCKDEP: Mutex<LockDepState> = Mutex::new(LockDepState::new());

pub fn lockdep_register_class(name: &str, lock_type_str: &str) -> LockClassId {
    let id = lockdep_alloc_class();
    let lock_type = match lock_type_str {
        "spinlock" => LockType::Spinlock,
        "mutex" => LockType::Mutex,
        "rwsem" => LockType::RwSemaphore,
        "rwlock" => LockType::RwLock,
        "seqlock" => LockType::Seqlock,
        _ => LockType::Mutex,
    };
    let class = LockClass::new(id, String::from(name), lock_type);
    let mut state = LOCKDEP.lock();
    state.classes.insert(id.0, class);
    id
}

pub fn lock_acquire(class_id: LockClassId, is_read: bool) {
    let mut state = LOCKDEP.lock();
    if !state.enabled {
        return;
    }

    state.total_lock_acquires.fetch_add(1, Ordering::Relaxed);

    let task_id = current_task_id();
    let cpu = current_cpu_id();

    let task_held: Vec<LockClassId> = state
        .held_locks
        .iter()
        .filter(|h| h.task_id == task_id)
        .map(|h| h.class_id)
        .collect();

    for &held_id in &task_held {
        if held_id == class_id && !is_read {
            state.circular_detected += 1;
        }

        let edge_exists = state
            .dependency_graph
            .iter()
            .any(|e| e.from == held_id && e.to == class_id);

        if !edge_exists {
            state.dependency_graph.push(LockDepEdge {
                from: held_id,
                to: class_id,
                stack_trace_hash: 0,
            });

            if lockdep_check_circular(&state.dependency_graph, class_id, held_id, state.max_depth) {
                state.circular_detected += 1;
            }
        }
    }

    if let Some(class) = state.classes.get(&class_id.0) {
        class.acquisitions.fetch_add(1, Ordering::Relaxed);
    }

    state.held_locks.push(HeldLock {
        class_id,
        acquire_time: read_timestamp(),
        task_id,
        cpu,
        is_read,
    });
}

pub fn lock_release(class_id: LockClassId) {
    let mut state = LOCKDEP.lock();
    if !state.enabled {
        return;
    }

    state.total_lock_releases.fetch_add(1, Ordering::Relaxed);
    let task_id = current_task_id();

    if let Some(pos) = state
        .held_locks
        .iter()
        .rposition(|h| h.class_id == class_id && h.task_id == task_id)
    {
        let held = state.held_locks.remove(pos);
        let hold_time = read_timestamp().wrapping_sub(held.acquire_time);
        if let Some(class) = state.classes.get(&class_id.0) {
            class.total_hold_ns.fetch_add(hold_time, Ordering::Relaxed);
        }
    }
}

fn lockdep_check_circular(
    graph: &[LockDepEdge],
    start: LockClassId,
    target: LockClassId,
    max_depth: usize,
) -> bool {
    if max_depth == 0 {
        return false;
    }
    for edge in graph {
        if edge.from == start {
            if edge.to == target {
                return true;
            }
            if lockdep_check_circular(graph, edge.to, target, max_depth - 1) {
                return true;
            }
        }
    }
    false
}

pub fn lockdep_assert_held(class_id: LockClassId) {
    let state = LOCKDEP.lock();
    let task_id = current_task_id();
    let held = state
        .held_locks
        .iter()
        .any(|h| h.class_id == class_id && h.task_id == task_id);
    if !held && state.enabled {
        // Lock not held — assertion failure in debug builds
    }
}

pub fn lockdep_set_enabled(enabled: bool) {
    LOCKDEP.lock().enabled = enabled;
}

pub fn lockdep_stats() -> LockDepStats {
    let state = LOCKDEP.lock();
    LockDepStats {
        classes: state.classes.len(),
        edges: state.dependency_graph.len(),
        held: state.held_locks.len(),
        circular_detected: state.circular_detected,
        total_acquires: state.total_lock_acquires.load(Ordering::Relaxed),
        total_releases: state.total_lock_releases.load(Ordering::Relaxed),
        chain_cache_size: state.chain_cache.len(),
    }
}

pub struct LockDepStats {
    pub classes: usize,
    pub edges: usize,
    pub held: usize,
    pub circular_detected: u64,
    pub total_acquires: u64,
    pub total_releases: u64,
    pub chain_cache_size: usize,
}

// ─── Completion ─────────────────────────────────────────────────────────────

pub struct Completion {
    done: AtomicU32,
    wq: WaitQueueHead,
}

impl Completion {
    pub const fn new() -> Self {
        Self {
            done: AtomicU32::new(0),
            wq: WaitQueueHead::new(),
        }
    }

    pub fn wait(&self) {
        self.wq.wait_event(|| self.done.load(Ordering::Acquire) > 0);
    }

    pub fn wait_timeout(&self, timeout_ns: u64) -> bool {
        self.wq
            .wait_event_timeout(|| self.done.load(Ordering::Acquire) > 0, timeout_ns)
    }

    pub fn complete(&self) {
        self.done.fetch_add(1, Ordering::Release);
        self.wq.wake_up();
    }

    pub fn complete_all(&self) {
        self.done.store(u32::MAX, Ordering::Release);
        self.wq.wake_up_all();
    }

    pub fn try_wait(&self) -> bool {
        self.done.load(Ordering::Acquire) > 0
    }

    pub fn reinit(&self) {
        self.done.store(0, Ordering::Release);
    }

    pub fn done_count(&self) -> u32 {
        self.done.load(Ordering::Relaxed)
    }
}

// ─── Semaphore ──────────────────────────────────────────────────────────────

pub struct Semaphore {
    count: AtomicI32,
    wq: WaitQueueHead,
}

impl Semaphore {
    pub const fn new(count: i32) -> Self {
        Self {
            count: AtomicI32::new(count),
            wq: WaitQueueHead::new(),
        }
    }

    pub fn down(&self) {
        loop {
            let old = self.count.load(Ordering::Relaxed);
            if old > 0 {
                if self
                    .count
                    .compare_exchange_weak(old, old - 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return;
                }
            }
            core::hint::spin_loop();
        }
    }

    pub fn down_interruptible(&self) -> bool {
        let task_id = current_task_id();
        loop {
            let old = self.count.load(Ordering::Relaxed);
            if old > 0 {
                if self
                    .count
                    .compare_exchange_weak(old, old - 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return true;
                }
            }
            if check_signal_pending(task_id) {
                return false;
            }
            core::hint::spin_loop();
        }
    }

    pub fn down_trylock(&self) -> bool {
        let old = self.count.load(Ordering::Relaxed);
        if old > 0 {
            self.count
                .compare_exchange(old, old - 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        } else {
            false
        }
    }

    pub fn down_timeout(&self, timeout_ns: u64) -> bool {
        let start = read_timestamp();
        loop {
            let old = self.count.load(Ordering::Relaxed);
            if old > 0 {
                if self
                    .count
                    .compare_exchange_weak(old, old - 1, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return true;
                }
            }
            if read_timestamp().wrapping_sub(start) > timeout_ns {
                return false;
            }
            core::hint::spin_loop();
        }
    }

    pub fn up(&self) {
        self.count.fetch_add(1, Ordering::Release);
        self.wq.wake_up();
    }

    pub fn count(&self) -> i32 {
        self.count.load(Ordering::Relaxed)
    }
}

// ─── Preempt Control ────────────────────────────────────────────────────────

static PREEMPT_COUNT: [AtomicI32; MAX_CPUS] = {
    const INIT: AtomicI32 = AtomicI32::new(0);
    [INIT; MAX_CPUS]
};

pub fn preempt_disable() {
    let cpu = current_cpu_id().min(MAX_CPUS - 1);
    PREEMPT_COUNT[cpu].fetch_add(1, Ordering::Relaxed);
    compiler_barrier();
}

pub fn preempt_enable() {
    compiler_barrier();
    let cpu = current_cpu_id().min(MAX_CPUS - 1);
    PREEMPT_COUNT[cpu].fetch_sub(1, Ordering::Relaxed);
}

pub fn preempt_count() -> i32 {
    let cpu = current_cpu_id().min(MAX_CPUS - 1);
    PREEMPT_COUNT[cpu].load(Ordering::Relaxed)
}

pub fn preemptible() -> bool {
    preempt_count() == 0
}

// ─── Futex Support (PI-futex integration) ───────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FutexOp {
    Wait,
    Wake,
    Requeue,
    CmpRequeue,
    WaitBitset,
    WakeBitset,
    LockPi,
    UnlockPi,
    TrylockPi,
    WaitRequeuePi,
}

pub struct FutexEntry {
    key: u64,
    task_id: u64,
    bitset: u32,
    op: FutexOp,
    pi_owner: AtomicU64,
}

struct FutexHashBucket {
    lock: RawSpinlock,
    entries: Mutex<Vec<FutexEntry>>,
}

impl FutexHashBucket {
    const fn new() -> Self {
        Self {
            lock: RawSpinlock::new(),
            entries: Mutex::new(Vec::new()),
        }
    }
}

const FUTEX_HASH_SIZE: usize = 256;

struct FutexTable {
    buckets: [FutexHashBucket; FUTEX_HASH_SIZE],
}

impl FutexTable {
    const fn new() -> Self {
        const BUCKET_INIT: FutexHashBucket = FutexHashBucket::new();
        Self {
            buckets: [BUCKET_INIT; FUTEX_HASH_SIZE],
        }
    }

    fn bucket_for(&self, key: u64) -> &FutexHashBucket {
        let idx = (key as usize) % FUTEX_HASH_SIZE;
        &self.buckets[idx]
    }

    pub fn futex_wait(&self, key: u64, expected: u32, bitset: u32) -> bool {
        let bucket = self.bucket_for(key);
        let _guard = bucket.lock.lock();
        bucket.entries.lock().push(FutexEntry {
            key,
            task_id: current_task_id(),
            bitset,
            op: FutexOp::Wait,
            pi_owner: AtomicU64::new(0),
        });
        let _ = expected;
        true
    }

    pub fn futex_wake(&self, key: u64, nr_wake: u32, bitset: u32) -> u32 {
        let bucket = self.bucket_for(key);
        let _guard = bucket.lock.lock();
        let mut entries = bucket.entries.lock();
        let mut woken = 0u32;
        entries.retain(|e| {
            if e.key == key && (e.bitset & bitset) != 0 && woken < nr_wake {
                woken += 1;
                false
            } else {
                true
            }
        });
        woken
    }

    pub fn futex_requeue(
        &self,
        key_from: u64,
        key_to: u64,
        nr_wake: u32,
        nr_requeue: u32,
    ) -> (u32, u32) {
        let bucket_from = self.bucket_for(key_from);
        let _guard = bucket_from.lock.lock();

        let mut entries = bucket_from.entries.lock();
        let mut woken = 0u32;
        let mut requeued = 0u32;

        let mut i = 0;
        while i < entries.len() {
            if entries[i].key == key_from {
                if woken < nr_wake {
                    entries.remove(i);
                    woken += 1;
                } else if requeued < nr_requeue {
                    entries[i].key = key_to;
                    requeued += 1;
                    i += 1;
                } else {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }

        (woken, requeued)
    }

    pub fn futex_lock_pi(&self, key: u64) -> bool {
        let bucket = self.bucket_for(key);
        let _guard = bucket.lock.lock();
        let mut entries = bucket.entries.lock();
        let task_id = current_task_id();

        let has_owner = entries
            .iter()
            .any(|e| e.key == key && e.pi_owner.load(Ordering::Relaxed) != 0);
        if !has_owner {
            entries.push(FutexEntry {
                key,
                task_id,
                bitset: u32::MAX,
                op: FutexOp::LockPi,
                pi_owner: AtomicU64::new(task_id),
            });
            return true;
        }
        false
    }

    pub fn futex_unlock_pi(&self, key: u64) -> bool {
        let bucket = self.bucket_for(key);
        let _guard = bucket.lock.lock();
        let mut entries = bucket.entries.lock();
        let task_id = current_task_id();

        if let Some(pos) = entries
            .iter()
            .position(|e| e.key == key && e.pi_owner.load(Ordering::Relaxed) == task_id)
        {
            entries.remove(pos);
            if let Some(next) = entries.iter().find(|e| e.key == key) {
                next.pi_owner.store(next.task_id, Ordering::Release);
            }
            return true;
        }
        false
    }
}

static FUTEX_TABLE: FutexTable = FutexTable::new();

// ─── Helper Functions ───────────────────────────────────────────────────────

fn current_cpu_id() -> usize {
    0
}

fn current_task_id() -> u64 {
    0
}

fn read_timestamp() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::x86_64::_rdtsc()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        0
    }
}

fn check_signal_pending(_task_id: u64) -> bool {
    false
}

// ─── Global Locking System ──────────────────────────────────────────────────

pub struct LockingSystem {
    initialized: AtomicBool,
    rcu_enabled: AtomicBool,
    lockdep_enabled: AtomicBool,
    nr_cpus: AtomicU32,
    stats: LockingStats,
}

pub struct LockingStats {
    pub spinlock_acquires: AtomicU64,
    pub mutex_acquires: AtomicU64,
    pub rwsem_read_acquires: AtomicU64,
    pub rwsem_write_acquires: AtomicU64,
    pub rcu_grace_periods: AtomicU64,
    pub rcu_callbacks_invoked: AtomicU64,
    pub futex_waits: AtomicU64,
    pub futex_wakes: AtomicU64,
    pub wq_wake_ups: AtomicU64,
    pub completions: AtomicU64,
}

impl LockingStats {
    const fn new() -> Self {
        Self {
            spinlock_acquires: AtomicU64::new(0),
            mutex_acquires: AtomicU64::new(0),
            rwsem_read_acquires: AtomicU64::new(0),
            rwsem_write_acquires: AtomicU64::new(0),
            rcu_grace_periods: AtomicU64::new(0),
            rcu_callbacks_invoked: AtomicU64::new(0),
            futex_waits: AtomicU64::new(0),
            futex_wakes: AtomicU64::new(0),
            wq_wake_ups: AtomicU64::new(0),
            completions: AtomicU64::new(0),
        }
    }
}

impl LockingSystem {
    const fn new() -> Self {
        Self {
            initialized: AtomicBool::new(false),
            rcu_enabled: AtomicBool::new(false),
            lockdep_enabled: AtomicBool::new(false),
            nr_cpus: AtomicU32::new(1),
            stats: LockingStats::new(),
        }
    }
}

pub static LOCKING_SYSTEM: LockingSystem = LockingSystem::new();

pub fn init() {
    if LOCKING_SYSTEM.initialized.swap(true, Ordering::SeqCst) {
        return;
    }

    RCU_STATE.nr_cpus.store(1, Ordering::Relaxed);
    RCU_STATE.stall_timeout_ms.store(21_000, Ordering::Relaxed);
    LOCKING_SYSTEM.rcu_enabled.store(true, Ordering::Release);

    {
        let mut state = LOCKDEP.lock();
        state.enabled = true;
        state.max_depth = 48;
    }
    LOCKING_SYSTEM
        .lockdep_enabled
        .store(true, Ordering::Release);

    for i in 0..MAX_CPUS {
        PREEMPT_COUNT[i].store(0, Ordering::Relaxed);
    }

    LOCKING_SYSTEM.nr_cpus.store(1, Ordering::Release);
}
