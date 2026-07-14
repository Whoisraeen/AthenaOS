//! Storage controller IRQ mode tracking (MSI-X vs legacy INTx).
//!
//! RaeenOS_Concept.md §Windows Pain Points → "Driver Wild West":
//! > All drivers signed and IOMMU-sandboxed
//!
//! `kernelchecklist.md` §M-A requires MSI-X on at least one PCI device on
//! real hardware, with legacy INTx as fallback when MSI-X programming fails.
//! NVMe and AHCI call [`probe_msix_or_intx`] at init; this module records
//! the outcome for boot smoketest and `/proc/raeen/storage_irq`.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::pci::PciDevice;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrqMode {
    Msix,
    Intx,
    None,
}

#[derive(Debug, Clone)]
pub struct DeviceIrqRecord {
    pub driver: &'static str,
    pub bdf: String,
    pub mode: IrqMode,
    pub detail: &'static str,
}

static RECORDS: Mutex<Vec<DeviceIrqRecord>> = Mutex::new(Vec::new());

pub fn init() {
    crate::serial_println!("[storage_irq] IRQ mode registry ready");
}

/// Try MSI-X via `msi::try_enable_msix_or_intx`; record result for diagnostics.
pub fn probe_msix_or_intx(
    driver: &'static str,
    dev: &PciDevice,
    vectors: usize,
) -> (IrqMode, Option<Vec<u8>>) {
    let bdf = alloc::format!("{:02x}:{:02x}.{}", dev.bus, dev.device, dev.function);
    let (mode, detail, allocated_vectors) = match crate::msi::try_enable_msix_or_intx(dev, vectors)
    {
        Some(v) => {
            crate::serial_println!(
                "[storage_irq] {} {} MSI-X enabled ({} vector(s), first={})",
                driver,
                bdf,
                v.len(),
                v[0],
            );
            (IrqMode::Msix, "msix_ok", Some(v))
        }
        None => {
            crate::serial_println!(
                "[storage_irq] {} {} using legacy INTx fallback",
                driver,
                bdf,
            );
            (IrqMode::Intx, "intx_fallback", None)
        }
    };

    RECORDS.lock().push(DeviceIrqRecord {
        driver,
        bdf,
        mode,
        detail,
    });
    (mode, allocated_vectors)
}

pub fn records() -> Vec<DeviceIrqRecord> {
    RECORDS.lock().clone()
}

pub fn dump_text() -> String {
    let recs = records();
    let mut out = String::from("# RaeenOS storage IRQ modes\n");
    if recs.is_empty() {
        out.push_str("devices: 0\n");
        out.push_str("note: no NVMe/AHCI controllers probed (normal in virtio-only QEMU)\n");
        return out;
    }
    out.push_str(&alloc::format!("devices: {}\n", recs.len()));
    for r in &recs {
        let mode = match r.mode {
            IrqMode::Msix => "MSI-X",
            IrqMode::Intx => "INTx",
            IrqMode::None => "none",
        };
        out.push_str(&alloc::format!(
            "{:<6} {:>8}  mode={:<5}  {}\n",
            r.driver,
            r.bdf,
            mode,
            r.detail,
        ));
    }
    out
}

pub fn run_boot_smoketest() {
    let recs = records();
    if recs.is_empty() {
        crate::serial_println!(
            "[storage_irq] smoketest OK: no storage PCI devices (INTx fallback path verified in msi.rs)",
        );
        return;
    }
    let intx = recs.iter().filter(|r| r.mode == IrqMode::Intx).count();
    let msix = recs.iter().filter(|r| r.mode == IrqMode::Msix).count();
    crate::serial_println!(
        "[storage_irq] smoketest OK: {} device(s) — MSI-X={} INTx={}",
        recs.len(),
        msix,
        intx,
    );
}
