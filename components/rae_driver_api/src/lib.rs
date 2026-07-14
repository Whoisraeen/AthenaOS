//! AthenaOS Driver API (RaeDriver)
//!
//! Standard library for userspace device drivers. Provides safe wrappers
//! for MMIO, port I/O, DMA, and Asynchronous Event Ring (AER) interrupts.

#![no_std]

pub mod aer_client;

#[derive(Debug)]
pub enum DriverError {
    RegistrationFailed,
    MmioMapFailed,
    DmaMapFailed,
    AerRingCorrupted,
}

/// Represents an established driver session with the supervisor.
pub struct DriverSession {
    pub vendor_id: u16,
    pub device_id: u16,
    pub tier: TrustTier,
    pub aer: aer_client::AerClient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustTier {
    Tier1Iommu,
    Tier2MonolithicFallback,
}

impl DriverSession {
    /// Initialize a new driver session.
    pub fn init(vendor_id: u16, device_id: u16) -> Result<Self, DriverError> {
        // In reality, this would communicate with driver_supervisor via IPC
        // or directly use SYS_DRIVER_REGISTER if permitted.

        let aer_client = aer_client::AerClient::new()?;

        Ok(Self {
            vendor_id,
            device_id,
            tier: TrustTier::Tier1Iommu,
            aer: aer_client,
        })
    }
}
