//! PCIe ECAM Quirk Engine.
//!
//! Handles motherboard anomalies where the ACPI `MCFG` table is either
//! malformed or omitted. We hardcode known ECAM bases for critical host bridges.

#[derive(Debug)]
pub struct PciHostQuirk {
    pub vendor_id: u16,
    pub device_id: u16,
    pub corrected_ecam_base: u64,
    pub start_bus: u8,
    pub end_bus: u8,
}

/// Known hardware targets where MCFG is broken or missing.
pub static ECAM_QUIRK_TABLE: &[PciHostQuirk] = &[
    // AMD Family 19h (Zen 4 - Hawk Point/Phoenix) Root Complex.
    // Modern Beelink/Athena systems often require this when MCFG is malformed.
    PciHostQuirk {
        vendor_id: 0x1022,
        device_id: 0x14e8,
        corrected_ecam_base: 0xE000_0000,
        start_bus: 0x00,
        end_bus: 0xFF,
    },
    // AMD Family 17h (Zen 2) Root Complex hidden ECAM placement.
    // Some consumer boards hide this from ACPI to "protect" Windows.
    PciHostQuirk {
        vendor_id: 0x1022,
        device_id: 0x1480,
        corrected_ecam_base: 0xE000_0000,
        start_bus: 0x00,
        end_bus: 0xFF,
    },
    // Intel Gemini Lake / Apollo Lake SoC ECAM defaults
    PciHostQuirk {
        vendor_id: 0x8086,
        device_id: 0x3180,
        corrected_ecam_base: 0xE000_0000,
        start_bus: 0x00,
        end_bus: 0xFF,
    },
];

pub fn lookup_ecam_override(vendor: u16, device: u16) -> Option<&'static PciHostQuirk> {
    ECAM_QUIRK_TABLE
        .iter()
        .find(|q| q.vendor_id == vendor && q.device_id == device)
}
