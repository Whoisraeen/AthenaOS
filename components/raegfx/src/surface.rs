use crate::TextureHandle;

/// Represents the on-screen Framebuffer surface.
/// This acts similarly to a Vulkan Swapchain.
pub struct Surface {
    width: u32,
    height: u32,
    // Typically, a swapchain maintains multiple images (e.g. double/triple buffering).
    // For now, we simulate this with a single handle.
    current_texture: TextureHandle,
}

impl Surface {
    pub fn new(width: u32, height: u32, initial_texture: TextureHandle) -> Self {
        Self {
            width,
            height,
            current_texture: initial_texture,
        }
    }

    /// Acquires the next available texture from the swapchain.
    pub fn get_current_texture(&self) -> TextureHandle {
        self.current_texture
    }

    /// Presents the surface. This would encode a page-flip command (e.g. `GPU_CMD_FLIP`)
    /// and submit it to the `RaeGfxQueue` to be processed by the kernel/hardware.
    pub fn present(&self) {
        // In a complete implementation, this would:
        // 1. Construct a GpuCommandPacket with cmd_type = GPU_CMD_FLIP (or similar for Virgl).
        // 2. Submit it to the Device's queue.
        // For example:
        // device.queue().submit(&[flip_packet]).unwrap();
    }
}
