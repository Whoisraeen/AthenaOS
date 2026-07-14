//! NVIDIA GPU BAR layout + the `NV_PMC_*` register offsets the earliest bring-up
//! touches. Offsets are absolute byte offsets into BAR0 (the MMIO register
//! aperture), matching the numbering used by nouveau/envytools — the PMC master
//! control block lives at the very bottom of the register space on every NVIDIA
//! GPU since NV04, which is why identification can happen before anything else
//! is mapped or powered.

/// PCI vendor id for NVIDIA Corporation.
pub const NVIDIA_VENDOR: u16 = 0x10DE;

/// PCI base-class for a display controller (VGA / 3D). NVIDIA GPUs enumerate as
/// base-class `0x03`, same as AMD/Intel display devices.
pub const PCI_CLASS_DISPLAY: u8 = 0x03;

// ── BAR layout (stable across the whole modern NVIDIA line) ──────────────────
// BAR0: MMIO register aperture (16 MiB). Every NV_* register offset below is an
//       offset into this BAR.
// BAR1: the VRAM aperture / framebuffer window (the CPU-visible portion of VRAM;
//       resizable-BAR widens it to all of VRAM).
// BAR3: instance memory (RAMIN) window — used for the page tables / engine
//       contexts on pre-GSP parts.
/// BAR index of the MMIO register aperture.
pub const BAR0_MMIO: u8 = 0;
/// BAR index of the VRAM aperture (framebuffer window).
pub const BAR1_VRAM: u8 = 1;
/// BAR index of the instance-memory (RAMIN) window.
pub const BAR3_RAMIN: u8 = 3;

// ── PMC — master control (the identification + top-level enable block) ───────
/// `NV_PMC_BOOT_0` — the chip identification register. Its architecture,
/// implementation and revision fields are decoded by [`crate::chip::decode_boot0`].
/// Readable immediately after BAR0 is mapped, before any power/clock setup.
pub const NV_PMC_BOOT_0: u32 = 0x000000;

/// `NV_PMC_BOOT_1` — endianness/strap register (bit 0 selects big vs little
/// endian MMIO). The bring-up asserts little-endian access before trusting
/// `NV_PMC_BOOT_0`.
pub const NV_PMC_BOOT_1: u32 = 0x000004;

/// `NV_PMC_INTR_0` — top-level interrupt status.
pub const NV_PMC_INTR_0: u32 = 0x000100;

/// `NV_PMC_INTR_EN_0` — top-level interrupt enable.
pub const NV_PMC_INTR_EN_0: u32 = 0x000140;

/// `NV_PMC_ENABLE` — per-engine master enable/reset gate.
pub const NV_PMC_ENABLE: u32 = 0x000200;
