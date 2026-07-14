//! Asynchronous Event Ring (AER) — ultra-low latency IRQ delivery to userspace.
//!
//! Concept doc §Architecture & §Gaming-First Design mandates sub-3ms audio/input
//! latency. Traditional microkernel IPC (message passing through the scheduler)
//! introduces catastrophic jitter for hardware interrupts.
//!
//! AER solves this via:
//! 1. **Lockless Shared Memory:** A ring buffer shared directly between the kernel
//!    and the userspace driver.
//! 2. **Top-Half Execution:** Zero-allocation kernel interrupt handlers push events
//!    here and exit immediately.
//! 3. **Direct Thread Injection:** The kernel top-half directly wakes the driver's
//!    `SCHED_GAME` thread, bypassing the normal scheduling epoch for instant
//!    context switching.

#![allow(dead_code)]

use crate::task::TaskId;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// An individual event in the AER ring.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct AerEvent {
    /// The interrupt vector or event type.
    pub vector: u32,
    /// Hardware-provided payload (e.g., MSI-X message data or status register).
    pub payload: u32,
    /// TSC timestamp when the kernel top-half handled the interrupt.
    pub timestamp_tsc: u64,
}

const AER_RING_SIZE: usize = 1024; // Power of two for fast masking

/// The lockless ring buffer, mapped into both kernel and userspace driver memory.
#[repr(C)]
pub struct AerRing {
    /// Kernel writes to the head.
    head: AtomicUsize,
    /// Userspace reads from the tail.
    tail: AtomicUsize,
    /// The event ring buffer.
    events: [AerEvent; AER_RING_SIZE],
}

impl AerRing {
    pub const fn new() -> Self {
        Self {
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            // Safe initialization of array of defaults
            events: [AerEvent {
                vector: 0,
                payload: 0,
                timestamp_tsc: 0,
            }; AER_RING_SIZE],
        }
    }

    /// Called by the kernel top-half to push an event.
    /// Lockless, zero-allocation, wait-free.
    pub fn push(&self, event: AerEvent) -> Result<(), &'static str> {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);

        if head.wrapping_sub(tail) >= AER_RING_SIZE {
            return Err("AER ring buffer overflow");
        }

        let idx = head & (AER_RING_SIZE - 1);
        // We use UnsafeCell or standard array assignment here; in a strict lockless MPMC/SPSC
        // we'd use atomics, but since kernel top-half (producer) is the only writer and runs
        // with interrupts disabled on the local CPU, simple assignment is fine before bumping head.
        unsafe {
            let ptr = self.events.as_ptr().add(idx) as *mut AerEvent;
            *ptr = event;
        }

        // Publish the new event
        self.head.store(head.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    /// Called by the userspace driver to pop an event.
    pub fn pop(&self) -> Option<AerEvent> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);

        if tail == head {
            return None; // Empty
        }

        let idx = tail & (AER_RING_SIZE - 1);
        let event = unsafe { *self.events.as_ptr().add(idx) };

        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(event)
    }
}

/// Registry entry tying an AER to a specific userspace thread for Direct Thread Injection.
pub struct AerRegistration {
    pub ring: Arc<AerRing>,
    pub target_thread: TaskId,
    /// Is the target thread running with `SCHED_GAME` priority?
    pub is_sched_game: bool,
}

static mut AER_REGISTRY: Option<Vec<AerRegistration>> = None;
static AER_LOCK: spin::Mutex<()> = spin::Mutex::new(());
static AER_DISPATCHES: AtomicU64 = AtomicU64::new(0);
static AER_PUSH_ERRORS: AtomicU64 = AtomicU64::new(0);

pub fn init() {
    unsafe {
        AER_REGISTRY = Some(Vec::new());
    }
    crate::serial_println!("[ OK ] Asynchronous Event Ring (AER) framework initialized");
}

/// Registers a new AER for a driver thread.
pub fn register_aer(target_thread: TaskId, is_sched_game: bool) -> Arc<AerRing> {
    let _guard = AER_LOCK.lock();
    let ring = Arc::new(AerRing::new());
    unsafe {
        if let Some(registry) = &mut AER_REGISTRY {
            registry.push(AerRegistration {
                ring: ring.clone(),
                target_thread,
                is_sched_game,
            });
        }
    }
    ring
}

/// Dispatches a hardware interrupt to the appropriate AER and performs Direct Thread Injection.
/// Called from the IDT / top-half interrupt handler.
pub fn dispatch_irq(vector: u32, payload: u32) {
    let tsc = 0; // Hardware TSC read not implemented in scaffold
    let event = AerEvent {
        vector,
        payload,
        timestamp_tsc: tsc,
    };

    // IRQ context: try_lock to avoid the same re-entrant spin-deadlock that
    // bit virtio-blk (MasterChecklist §Latent kernel bugs). If the
    // interrupted code holds AER_LOCK (e.g. register_aer mid-push), we drop
    // this event rather than deadlock. AER_PUSH_ERRORS counts the drops.
    let _guard = match AER_LOCK.try_lock() {
        Some(g) => g,
        None => {
            AER_PUSH_ERRORS.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };
    unsafe {
        if let Some(registry) = &AER_REGISTRY {
            for reg in registry {
                // In a real implementation, we'd look up by IRQ vector mapping.
                // For scaffolding, we just broadcast or match some criteria.
                if reg.ring.push(event).is_err() {
                    AER_PUSH_ERRORS.fetch_add(1, Ordering::Relaxed);
                }

                // Direct Thread Injection
                if reg.is_sched_game {
                    // Bypass standard queue and immediately flag this thread as runnable
                    // and trigger a preemption check.
                    crate::scheduler::wake_thread_direct(reg.target_thread);
                } else {
                    // Standard wakeup
                    crate::scheduler::wake_thread(reg.target_thread);
                }
            }
            AER_DISPATCHES.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub fn run_boot_smoketest() {
    let reg_count = unsafe { AER_REGISTRY.as_ref().map(|r| r.len()).unwrap_or(0) };
    crate::serial_println!(
        "[aer] smoketest: registrations={} dispatches={} push_errors={}",
        reg_count,
        AER_DISPATCHES.load(Ordering::Relaxed),
        AER_PUSH_ERRORS.load(Ordering::Relaxed)
    );
}

pub fn dump_text() -> String {
    let reg_count = unsafe { AER_REGISTRY.as_ref().map(|r| r.len()).unwrap_or(0) };
    alloc::format!(
        "# AER\nregistrations: {}\ndispatches: {}\npush_errors: {}\n",
        reg_count,
        AER_DISPATCHES.load(Ordering::Relaxed),
        AER_PUSH_ERRORS.load(Ordering::Relaxed)
    )
}
