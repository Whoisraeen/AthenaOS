//! Userspace Driver Framework (RaeDriver) — Tiers of Trust and IPC bindings.
//!
//! Concept doc §Architecture: Driver isolation via IOMMU and Capabilities.
//! This module orchestrates the Split-Driver strategy and registers userspace
//! daemons to handle hardware devices.

#![allow(dead_code)]

use crate::capability::Rights;
use crate::task::TaskId;
use alloc::sync::Arc;
use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustTier {
    /// Tier 1: Hardware IOMMU present. Fully sandboxed userspace driver.
    Tier1Iommu,
    /// Tier 2: No IOMMU. Monolithic Fallback. Requires signed driver or dev flag.
    Tier2MonolithicFallback,
}

pub struct DriverRegistration {
    pub pci_vendor_id: u16,
    pub pci_device_id: u16,
    pub tier: TrustTier,
    /// The userspace process/thread handling this driver
    pub target_thread: TaskId,
    /// Associated AER ring for fast-path interrupt delivery
    pub aer: Option<Arc<crate::aer::AerRing>>,
}

static mut DRIVER_REGISTRY: Option<Vec<DriverRegistration>> = None;
static REGISTRY_LOCK: spin::Mutex<()> = spin::Mutex::new(());

pub fn init() {
    unsafe {
        DRIVER_REGISTRY = Some(Vec::new());
    }
    crate::serial_println!("[ OK ] Userspace Driver Framework (RaeDriver) initialized");
}

/// Syscall: `sys_driver_register`
///
/// Authorized userspace `driver_supervisor` calls this to attach a driver process
/// to a specific PCI device. It establishes the TrustTier based on IOMMU presence.
pub fn sys_driver_register(
    vendor_id: u16,
    device_id: u16,
    pci_bus: u8,
    pci_dev: u8,
    pci_func: u8,
    target_thread: TaskId,
    _signature_valid: bool,
) -> Result<Arc<crate::aer::AerRing>, &'static str> {
    let _guard = REGISTRY_LOCK.lock();

    // Query the IOMMU subsystem
    let (tier, domain_id) = if crate::iommu::is_enabled() {
        let did = crate::iommu::create_domain().ok_or("Failed to create IOMMU domain")?;
        if !crate::iommu::assign_device(did, pci_bus, pci_dev, pci_func) {
            crate::iommu::destroy_domain(did);
            return Err("Failed to assign PCI device to IOMMU domain");
        }
        (TrustTier::Tier1Iommu, Some(did))
    } else {
        (TrustTier::Tier2MonolithicFallback, None)
    };

    if tier == TrustTier::Tier2MonolithicFallback && !_signature_valid {
        // Enforce Tiers of Trust security policy
        return Err("EPERM: Tier 2 (No IOMMU) requires cryptographically signed drivers");
    }

    // Register an AER for this driver
    let aer = crate::aer::register_aer(target_thread, true);

    let reg = DriverRegistration {
        pci_vendor_id: vendor_id,
        pci_device_id: device_id,
        tier,
        target_thread,
        aer: Some(aer.clone()),
    };

    unsafe {
        if let Some(registry) = &mut DRIVER_REGISTRY {
            registry.push(reg);
        }
    }

    Ok(aer)
}
