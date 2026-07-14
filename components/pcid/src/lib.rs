//! PCI vendor/device name lookup (R03).
//!
//! Adapted from Redox `pciids` recipe patterns — embedded subset for boot/kernel
//! logging until full `pciids.git` database is vendored.

#![no_std]

/// Known PCI vendor + device pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciId {
    pub vendor: u16,
    pub device: u16,
    pub vendor_name: &'static str,
    pub device_name: &'static str,
}

/// Curated IDs for QEMU dev and common bring-up hardware.
pub const KNOWN_DEVICES: &[PciId] = &[
    PciId {
        vendor: 0x0627,
        device: 0x0001,
        vendor_name: "QEMU",
        device_name: "USB Tablet/Keyboard",
    },
    PciId {
        vendor: 0x8086,
        device: 0x10D3,
        vendor_name: "Intel",
        device_name: "82574L Gigabit Ethernet",
    },
    PciId {
        vendor: 0x8086,
        device: 0x15F3,
        vendor_name: "Intel",
        device_name: "I225-V Ethernet",
    },
    PciId {
        vendor: 0x8086,
        device: 0x15B8,
        vendor_name: "Intel",
        device_name: "I219-V Ethernet",
    },
    PciId {
        vendor: 0x10EC,
        device: 0x8125,
        vendor_name: "Realtek",
        device_name: "RTL8125 2.5GbE",
    },
    PciId {
        vendor: 0x10EC,
        device: 0x8168,
        vendor_name: "Realtek",
        device_name: "RTL8168 Gigabit Ethernet",
    },
    PciId {
        vendor: 0x8086,
        device: 0x7D55,
        vendor_name: "Intel",
        device_name: "Xe LPG Graphics",
    },
    PciId {
        vendor: 0x8086,
        device: 0x7A60,
        vendor_name: "Intel",
        device_name: "xHCI Host Controller",
    },
    PciId {
        vendor: 0x1022,
        device: 0x43F7,
        vendor_name: "AMD",
        device_name: "NVMe Controller",
    },
    PciId {
        vendor: 0x1B36,
        device: 0x000D,
        vendor_name: "Red Hat",
        device_name: "QEMU xHCI",
    },
];

/// Look up a human-readable device label for logging/UI.
pub fn describe(vendor: u16, device: u16) -> Option<&'static str> {
    KNOWN_DEVICES
        .iter()
        .find(|e| e.vendor == vendor && e.device == device)
        .map(|e| e.device_name)
}

/// Format `vendor:device` with optional names.
pub fn format_id(vendor: u16, device: u16) -> (&'static str, &'static str, Option<&'static str>) {
    let vname = KNOWN_DEVICES
        .iter()
        .find(|e| e.vendor == vendor)
        .map(|e| e.vendor_name)
        .unwrap_or("Unknown vendor");
    let dname = describe(vendor, device);
    (vname, "device", dname)
}
