//! drm_sched — the DRM GPU scheduler. amdgpu submits work through per-ring
//! `drm_gpu_scheduler` instances; each `drm_sched_job` is queued, run on the
//! hardware ring, and produces a `dma_fence` on completion.

extern crate alloc;
use crate::fence::DmaFence;
use alloc::collections::VecDeque;
use alloc::vec::Vec;

/// `struct drm_sched_job` — one unit of GPU work (a command-buffer submission).
pub struct DrmSchedJob {
    pub job_id: u64,
    /// Indirect-buffer GPU address the ring will execute.
    pub ib_gpu_addr: u64,
    pub ib_size_dw: u32,
    /// Fence that signals when this job completes.
    pub fence_seqno: u64,
}

/// `struct drm_gpu_scheduler` — one hardware ring's job queue.
pub struct DrmGpuScheduler {
    pub name: alloc::string::String,
    pub ring_id: u32,
    queue: VecDeque<DrmSchedJob>,
    completed: Vec<u64>,
    next_seqno: u64,
}

impl DrmGpuScheduler {
    /// `drm_sched_init`.
    pub fn init(name: &str, ring_id: u32) -> Self {
        Self {
            name: alloc::string::String::from(name),
            ring_id,
            queue: VecDeque::new(),
            completed: Vec::new(),
            next_seqno: 1,
        }
    }

    /// `drm_sched_job_init` + `drm_sched_entity_push_job` — queue a job, return
    /// its completion fence.
    pub fn push_job(&mut self, ib_gpu_addr: u64, ib_size_dw: u32) -> DmaFence {
        let seqno = self.next_seqno;
        self.next_seqno += 1;
        self.queue.push_back(DrmSchedJob {
            job_id: seqno,
            ib_gpu_addr,
            ib_size_dw,
            fence_seqno: seqno,
        });
        DmaFence::new(seqno)
    }

    /// `drm_sched_main` step — pop the next job and emit it to the hardware ring.
    /// Returns the job's GPU address to write into the ring buffer, or None if
    /// the queue is empty. The caller (the amdgpu ring code) writes the ring
    /// PM4/SDMA packets and rings the doorbell.
    pub fn run_next(&mut self) -> Option<DrmSchedJob> {
        self.queue.pop_front()
    }

    /// Called from the ring completion IRQ — mark a seqno done.
    pub fn complete(&mut self, seqno: u64) {
        self.completed.push(seqno);
    }

    pub fn pending(&self) -> usize {
        self.queue.len()
    }
}
