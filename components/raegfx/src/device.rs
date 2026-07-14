use crate::shared_queue::{GpuCommandPacket, GpuRingControl};
use core::ptr::write_volatile;
use core::sync::atomic::Ordering;

/// A user-space graphics device abstraction that acts as the control layer for
/// memory allocation and command queue management.
pub struct Device {
    queue: RaeGfxQueue,
}

impl Device {
    /// Creates a new `Device` instance by mapping the kernel-provided
    /// memory structures. In a real environment, this would call the kernel
    /// (e.g., via a capability-based RPC or syscall) to request a mapped
    /// command ring and doorbell.
    pub unsafe fn new(
        control: *mut GpuRingControl,
        packet_ring: *mut GpuCommandPacket,
        doorbell: *mut u32,
    ) -> Self {
        Self {
            queue: RaeGfxQueue::new(control, packet_ring, doorbell),
        }
    }

    /// Access the underlying zero-syscall queue.
    pub fn queue(&self) -> &RaeGfxQueue {
        &self.queue
    }

    // Future methods: alloc_texture(), alloc_buffer(), etc.
    // These will use syscalls/RPC to request physical VRAM chunks from the kernel.
}

/// The ultra-low-latency, zero-syscall command submission queue.
#[derive(Debug)]
pub struct RaeGfxQueue {
    /// Memory-mapped pointer to the lockless ring control indices
    control: *mut GpuRingControl,

    /// Memory-mapped pointer to the actual ring packet slots
    packet_ring: *mut GpuCommandPacket,

    /// Memory-mapped address of the physical hardware doorbell register
    doorbell: *mut u32,
}

unsafe impl Send for RaeGfxQueue {}
unsafe impl Sync for RaeGfxQueue {}

impl RaeGfxQueue {
    pub unsafe fn new(
        control: *mut GpuRingControl,
        packet_ring: *mut GpuCommandPacket,
        doorbell: *mut u32,
    ) -> Self {
        Self {
            control,
            packet_ring,
            doorbell,
        }
    }

    /// Submits a slice of `GpuCommandPacket`s to the GPU.
    /// This method avoids syscalls completely. It writes directly to the
    /// memory-mapped packet ring, updates the lockless head index, and
    /// executes a volatile write to the hardware doorbell MMIO register.
    pub unsafe fn submit(&self, commands: &[GpuCommandPacket]) -> Result<u32, &'static str> {
        let control = &*self.control;
        let ring_depth = control.ring_depth;

        let mut current_head = control.head.load(Ordering::Relaxed);
        let current_tail = control.tail.load(Ordering::Acquire);

        for packet in commands {
            // Check for ring saturation/overflow conditions
            if (current_head + 1) % ring_depth == current_tail {
                return Err("AthGFX Error: Direct GPU Command Ring Buffer Saturated.");
            }

            // Calculate slot offset and perform a volatile copy into the mapped PCIe/VirtIO space
            let slot_offset = (current_head % ring_depth) as usize;
            let target_slot = self.packet_ring.add(slot_offset);
            core::ptr::write_volatile(target_slot, *packet);

            current_head = (current_head + 1) % ring_depth;
        }

        // Atomically update the head index to expose the packets to the hardware thread safely
        control.head.store(current_head, Ordering::Release);

        // POKE THE DOORBELL: Write the current head index straight to the MMIO target register.
        // This memory-mapped write bypasses Ring 0 completely and rings the silicon/VM directly.
        write_volatile(self.doorbell, current_head);

        Ok(current_head) // Return tracking token/fence index
    }
}
