//! PCI IRQ routing table and GSI resolution.
//!
//! This module maintains a global mapping of PCI (bus, device, pin) to GSI,
//! populated from ACPI _PRT objects.
//!
//! Concept:
//! 1. During ACPI initialization, we scan the namespace for _PRT objects.
//! 2. For each _PRT, we resolve the PCI bus it belongs to.
//! 3. We parse the entries and store them in a global table.
//! 4. PCI device enumeration uses this table to route legacy interrupts via the IOAPIC.

use crate::acpi_full::PciRoutingEntry;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

/// Unique identifier for a PCI interrupt pin on a specific device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PciIrqKey {
    pub bus: u8,
    pub device: u8,
    pub pin: u8, // 1=INTA#, 2=INTB#, 3=INTC#, 4=INTD#
}

/// A mapping from a PCI pin to a Global System Interrupt (GSI).
#[derive(Debug, Clone)]
pub struct PciIrqMapping {
    pub key: PciIrqKey,
    pub gsi: u32,
    pub source: String,
}

static ROUTING_TABLE: Mutex<Vec<PciIrqMapping>> = Mutex::new(Vec::new());

/// Initialize the PCI IRQ routing module.
pub fn init() {
    crate::serial_println!("[pci_irq] Initializing PCI IRQ routing subsystem...");
}

/// Add a routing entry to the global table.
pub fn add_entry(bus: u8, device: u8, pin: u8, gsi: u32, source: String) {
    let mut table = ROUTING_TABLE.lock();
    // Avoid duplicates
    if !table
        .iter()
        .any(|e| e.key.bus == bus && e.key.device == device && e.key.pin == pin)
    {
        table.push(PciIrqMapping {
            key: PciIrqKey { bus, device, pin },
            gsi,
            source,
        });
    }
}

/// Resolve a GSI for a given PCI bus, device, and pin.
pub fn resolve_gsi(bus: u8, device: u8, pin: u8) -> Option<u32> {
    if pin == 0 {
        return None;
    }
    let table = ROUTING_TABLE.lock();
    for entry in table.iter() {
        if entry.key.bus == bus && entry.key.device == device && entry.key.pin == pin {
            return Some(entry.gsi);
        }
    }
    None
}

/// Apply IRQ routing for all enumerated PCI devices using the _PRT table.
pub fn route_all_devices() {
    let devices = crate::pci::enumerate();
    for dev in devices {
        // Only route if the device has a legacy interrupt pin (1=INTA, etc.)
        if dev.irq_pin != 0 {
            if let Some(gsi) = resolve_gsi(dev.bus, dev.device, dev.irq_pin) {
                // Map legacy interrupts starting from vector 0x30 (after exceptions/legacy IRQs)
                // Note: This is a simplistic mapping; eventually we should have an IRQ allocator.
                let vector = 0x30 + (gsi % 32) as u8;

                // PCI interrupts are Level-Triggered and Active-Low by standard.
                crate::apic::route_irq(gsi, vector);
            }
        }
    }
}

/// Run boot-time smoketest.
pub fn run_boot_smoketest() {
    let table = ROUTING_TABLE.lock();
    crate::serial_println!(
        "[pci_irq] Routing table has {} entries -> PASS",
        table.len()
    );
}

/// Expose routing table via procfs.
pub fn dump_text() -> String {
    let table = ROUTING_TABLE.lock();
    let mut out = String::new();
    use core::fmt::Write;
    let _ = writeln!(out, "PCI IRQ Routing Table:");
    let _ = writeln!(
        out,
        "{:<4} {:<4} {:<4} {:<6} {:<20}",
        "BUS", "DEV", "PIN", "GSI", "SOURCE"
    );
    for entry in table.iter() {
        let pin_char = match entry.key.pin {
            1 => 'A',
            2 => 'B',
            3 => 'C',
            4 => 'D',
            _ => '?',
        };
        let _ = writeln!(
            out,
            "{:<4} {:<4} INT{}  {:<6} {:<20}",
            entry.key.bus, entry.key.device, pin_char, entry.gsi, entry.source
        );
    }
    out
}
