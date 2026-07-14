//! dma-fence — the GPU/CPU synchronization primitive. Every GPU job produces a
//! `dma_fence` that signals on completion; the CPU (or another engine) waits on
//! it. amdgpu uses fences for command-submission completion and for KMS
//! page-flip vsync.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// `struct dma_fence` — a one-shot completion signal with a monotonic seqno.
pub struct DmaFence {
    pub context: u64,
    pub seqno: u64,
    signaled: AtomicBool,
}

static FENCE_CONTEXT: AtomicU64 = AtomicU64::new(1);

impl DmaFence {
    /// `dma_fence_context_alloc` + `dma_fence_init`.
    pub fn new(seqno: u64) -> Self {
        Self {
            context: FENCE_CONTEXT.fetch_add(1, Ordering::Relaxed),
            seqno,
            signaled: AtomicBool::new(false),
        }
    }

    /// `dma_fence_signal` — mark the fence complete (called from the IRQ handler
    /// when the GPU ring's completion interrupt fires).
    pub fn signal(&self) {
        self.signaled.store(true, Ordering::Release);
    }

    /// `dma_fence_is_signaled`.
    pub fn is_signaled(&self) -> bool {
        self.signaled.load(Ordering::Acquire)
    }

    /// `dma_fence_wait_timeout` — block until signaled or `timeout_ms` elapses.
    /// Returns true if signaled, false on timeout. Backed by the LinuxKPI host
    /// jiffies clock + cooperative yield.
    pub fn wait_timeout(&self, timeout_ms: u64) -> bool {
        let start = ath_linuxkpi::get_jiffies_64();
        loop {
            if self.is_signaled() {
                return true;
            }
            let now = ath_linuxkpi::get_jiffies_64();
            if now.saturating_sub(start) >= timeout_ms {
                return false;
            }
            ath_linuxkpi::msleep(0); // yield to the scheduler / IRQ thread
        }
    }
}
