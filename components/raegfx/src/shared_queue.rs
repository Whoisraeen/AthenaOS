use core::sync::atomic::AtomicU32;

/// Memory-mapped shared control structure for the zero-syscall GPU command ring.
///
/// This struct is mapped directly between user-space (AthGFX) and the kernel's
/// GPU driver (or the hardware directly if supported). It allows user-space
/// to push command packets without executing a costly ring 0 context switch.
#[repr(C, align(64))]
pub struct GpuRingControl {
    /// Written by User Space, read by Kernel/Hardware.
    /// Indicates the next free slot to write a command packet.
    pub head: AtomicU32,

    /// Written by Kernel/Hardware, read by User Space.
    /// Indicates the last slot processed by the GPU.
    pub tail: AtomicU32,

    /// The total capacity (number of slots) in the packet ring.
    pub ring_depth: u32,

    /// Reserved for alignment and future expansion.
    pub reserved: u32,
}

/// A single tokenized command packet sent to the GPU.
/// For VirtIO-GPU Virgl, this will encapsulate a Virgl 3D command stream payload.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct GpuCommandPacket {
    /// Type of command (e.g., Virgl Submit 3D, Page Flip, etc.)
    pub cmd_type: u32,

    /// Execution flags (e.g., generate interrupt on completion)
    pub flags: u32,

    /// Physical offset / memory handle to the payload (e.g., Virgl command stream buffer)
    pub payload_addr: u64,

    /// Size of the payload in bytes
    pub payload_size: u32,

    /// Monotonically increasing sync token to track completion
    pub fence_id: u32,
}
