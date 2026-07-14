//! AER Client — Userspace receiver for Asynchronous Event Ring.

use core::sync::atomic::{AtomicUsize, Ordering};

const AER_RING_SIZE: usize = 1024;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct AerEvent {
    pub vector: u32,
    pub payload: u32,
    pub timestamp_tsc: u64,
}

#[repr(C)]
pub struct AerRingClient {
    head: AtomicUsize,
    tail: AtomicUsize,
    events: [AerEvent; AER_RING_SIZE],
}

pub struct AerClient {
    ring_ptr: *const AerRingClient,
}

impl AerClient {
    pub fn new() -> Result<Self, crate::DriverError> {
        // The driver supervisor provides the shared memory ring mapping.
        // For scaffold, we assume a pointer.
        Ok(Self {
            ring_ptr: core::ptr::null(), // Stub
        })
    }

    /// Pops the next event from the AER locklessly.
    pub fn pop_event(&mut self) -> Option<AerEvent> {
        if self.ring_ptr.is_null() {
            return None;
        }

        unsafe {
            let ring = &*self.ring_ptr;
            let tail = ring.tail.load(Ordering::Relaxed);
            let head = ring.head.load(Ordering::Acquire);

            if tail == head {
                return None; // Empty
            }

            let idx = tail & (AER_RING_SIZE - 1);
            let event = ring.events[idx];

            ring.tail.store(tail.wrapping_add(1), Ordering::Release);
            Some(event)
        }
    }
}
