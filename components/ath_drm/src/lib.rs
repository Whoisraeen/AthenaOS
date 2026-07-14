//! AthenaOS DRM/KMS API island.
//!
//! This crate is the **DRM subsystem surface** that Linux GPU drivers (amdgpu,
//! i915, nouveau) link against instead of `vmlinux`. It provides the
//! `drm_*` / `ttm_*` / `dma_fence_*` / `drm_sched_*` types and entry points the
//! driver expects, implemented on top of `ath_linuxkpi` (the hardware bridge:
//! ioremap, dma_alloc_coherent, request_irq) and AthGFX (the compositor scanout).
//!
//! The driver believes it is talking to the in-kernel DRM core. In reality every
//! object lives in the driver's sandboxed userspace daemon, and modeset / scanout
//! requests are forwarded to the AthenaOS compositor via IPC.
//!
//! Per `docs/LINUX_DRIVER_STRATEGY.md` (Path C) + `docs/LINUXKPI_PHASE2.md`. This
//! is the DRM half of the "full amdgpu" port — the driver-facing API contract.
//! It is a license-island: MPL-2.0 AthenaOS code providing a compatible *interface*,
//! not a copy of GPL Linux DRM source.

#![no_std]
#![allow(dead_code)]
#![allow(clippy::missing_safety_doc)]

extern crate alloc;

pub mod fence;
pub mod gem;
pub mod kms;
pub mod sched;
pub mod ttm;

use alloc::string::String;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Linux `irqreturn_t`.
pub const IRQ_NONE: i32 = 0;
pub const IRQ_HANDLED: i32 = 1;
pub const IRQ_WAKE_THREAD: i32 = 2;

/// Linux error codes (negated errno) the driver checks against.
pub const ENOMEM: i32 = -12;
pub const EINVAL: i32 = -22;
pub const ENODEV: i32 = -19;
pub const EIO: i32 = -5;
pub const ETIMEDOUT: i32 = -110;
pub const EBUSY: i32 = -16;

/// DRM driver feature flags (`DRIVER_*`).
pub const DRIVER_GEM: u32 = 1 << 0;
pub const DRIVER_MODESET: u32 = 1 << 1;
pub const DRIVER_RENDER: u32 = 1 << 2;
pub const DRIVER_ATOMIC: u32 = 1 << 3;
pub const DRIVER_SYNCOBJ: u32 = 1 << 4;
pub const DRIVER_GEM_GPUVA: u32 = 1 << 5;

/// `struct drm_driver` — the driver's registration vtable + identity.
#[derive(Clone)]
pub struct DrmDriver {
    pub name: String,
    pub desc: String,
    pub major: u32,
    pub minor: u32,
    pub patchlevel: u32,
    pub driver_features: u32,
}

/// `struct drm_device` — the central DRM object. amdgpu embeds its
/// `struct amdgpu_device` and back-references this.
pub struct DrmDevice {
    pub driver: DrmDriver,
    /// LinuxKPI device handle (from `pci_enable_device`) for hardware access.
    pub lkpi_dev: u64,
    pub primary_minor: u32,
    pub render_minor: u32,
    pub registered: bool,
    /// Number of CRTCs / connectors / planes registered via KMS.
    pub num_crtcs: u32,
    pub num_connectors: u32,
    pub num_planes: u32,
    /// Unique object-id counter for GEM/fence handles.
    next_handle: AtomicU64,
}

impl DrmDevice {
    /// `drm_dev_alloc` — allocate a DRM device bound to a LinuxKPI hardware handle.
    pub fn alloc(driver: DrmDriver, lkpi_dev: u64) -> Self {
        Self {
            driver,
            lkpi_dev,
            primary_minor: 0,
            render_minor: 128,
            registered: false,
            num_crtcs: 0,
            num_connectors: 0,
            num_planes: 0,
            next_handle: AtomicU64::new(1),
        }
    }

    /// `drm_dev_register` — publish the device to userspace (`/dev/dri/cardN`).
    /// On AthenaOS this registers the device with the compositor instead.
    pub fn register(&mut self) -> i32 {
        self.registered = true;
        log(&alloc::format!(
            "[drm] {} registered: card{} render{} features={:#x}",
            self.driver.name,
            self.primary_minor,
            self.render_minor,
            self.driver.driver_features
        ));
        0
    }

    pub fn alloc_handle(&self) -> u64 {
        self.next_handle.fetch_add(1, Ordering::Relaxed)
    }
}

/// Global DRM minor counter (`card0`, `card1`, …).
static NEXT_MINOR: AtomicU32 = AtomicU32::new(0);

pub fn alloc_minor() -> u32 {
    NEXT_MINOR.fetch_add(1, Ordering::Relaxed)
}

/// Route DRM log messages through the LinuxKPI host printk.
pub fn log(msg: &str) {
    // ath_linuxkpi::athena_printk takes a NUL-terminated C string.
    let mut buf = alloc::vec::Vec::with_capacity(msg.len() + 1);
    buf.extend_from_slice(msg.as_bytes());
    buf.push(0);
    ath_linuxkpi::athena_printk(buf.as_ptr());
}
