//! raeen_nvidia — pure NVIDIA GPU bring-up logic for the `nvidiad` daemon.
//!
//! AthenaOS's native NVIDIA driver, built to the same discipline as
//! [`raeen_amdgpu`](../raeen_amdgpu): the parts of the bring-up that are *pure
//! functions of register values* live here with NO dependency on the LinuxKPI
//! syscall shim, so they build on the dev box and every decode is validated by
//! `cargo test -p raeen_nvidia` (the host-KAT pattern) rather than only being
//! exercisable on real NVIDIA silicon — which the AthenaOS lab does not yet have.
//!
//! What lives here so far:
//! * [`regs`] — the NVIDIA BAR layout (BAR0 = MMIO registers, BAR1 = VRAM
//!   aperture, BAR3 = instance memory) and the handful of `NV_PMC_*` register
//!   offsets the earliest bring-up touches.
//! * [`chip`] — the [`chip::identify`] decode of `NV_PMC_BOOT_0` into an
//!   architecture family, chipset id, and revision, plus the honest
//!   *firmware-requirement tier* ([`chip::FwRequirement`]) that states exactly
//!   where each generation needs external firmware — and where GSP-RM (Turing
//!   and later) walls a from-scratch driver. This mirrors the identification
//!   `nvkm_device_ctor` performs first on every NVIDIA GPU.
//!
//! ## Scope honesty (the GSP wall)
//! Display modeset is reachable natively on every pre-GSP generation. Hardware
//! acceleration is not free: Kepler through Volta require NVIDIA-signed falcon
//! microcode, and Turing and later route full initialisation through the GSP-RM
//! firmware coprocessor. [`chip::FwRequirement`] classifies each part so the
//! daemon reports the wall up front instead of discovering it three stages deep.
//!
//! `nvidiad` (the `no_std` daemon) implements [`chip::GpuOps`] over the LinuxKPI
//! shim (real MMIO reads) and runs [`chip::identify`] as its first stage.

#![no_std]
#![forbid(unsafe_code)]

pub mod chip;
pub mod regs;
