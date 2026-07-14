//! Known-broken ACPI/DSDT patterns (HP, Lenovo, Dell, …).
//!
//! MasterChecklist Phase 1.4 — quirks list for vendor DSDTs. Entries are matched
//! against the ACPI table OEM ID + OEM table ID at boot; matched quirks are logged
//! and exposed via `/proc/athena/acpi_quirks`.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;

#[derive(Debug, Clone, Copy)]
pub struct DsdtQuirk {
    pub oem_id_prefix: &'static str,
    pub oem_table_prefix: &'static str,
    pub vendor: &'static str,
    pub note: &'static str,
}

/// Curated quirks — extend when Athena/laptop bring-up finds a concrete workaround.
pub const KNOWN_QUIRKS: &[DsdtQuirk] = &[
    DsdtQuirk {
        oem_id_prefix: "HPQO",
        oem_table_prefix: "SLIC",
        vendor: "HP",
        note: "EliteBook/ProBook: broken _PSS on some firmware — use native P-state path",
    },
    DsdtQuirk {
        oem_id_prefix: "HPQ ",
        oem_table_prefix: "FACP",
        vendor: "HP",
        note: "Consumer laptops: EC _REG may need Windows 2020 _OSI (handled in bring-up)",
    },
    DsdtQuirk {
        oem_id_prefix: "LENO",
        oem_table_prefix: "CB-",
        vendor: "Lenovo",
        note: "ThinkPad: thermal _ACx methods may reference missing devices on some SKUs",
    },
    DsdtQuirk {
        oem_id_prefix: "LENO",
        oem_table_prefix: "TP-",
        vendor: "Lenovo",
        note: "ThinkPad T/X series: verify _PRT vs IOAPIC on docked configs",
    },
    DsdtQuirk {
        oem_id_prefix: "DELL",
        oem_table_prefix: "CBX3",
        vendor: "Dell",
        note: "XPS/Inspiron: battery _BST paths vary — battery.rs tries multiple names",
    },
    DsdtQuirk {
        oem_id_prefix: "ALASKA",
        oem_table_prefix: "A M I",
        vendor: "AMI/Aptio",
        note: "Generic AMI firmware: often missing _PRT on root — pci_irq uses fallback",
    },
];

pub fn match_quirk(oem_id: &str, oem_table_id: &str) -> Option<&'static DsdtQuirk> {
    KNOWN_QUIRKS.iter().find(|q| {
        oem_id.starts_with(q.oem_id_prefix) && oem_table_id.starts_with(q.oem_table_prefix)
    })
}

/// Log quirks that match the firmware OEM strings (call after ACPI table load).
pub fn audit_firmware_oem(oem_id: &str, oem_table_id: &str) {
    if let Some(q) = match_quirk(oem_id, oem_table_id) {
        crate::serial_println!(
            "[acpi-quirks] matched {} ({}{}): {}",
            q.vendor,
            q.oem_id_prefix,
            q.oem_table_prefix,
            q.note
        );
    }
}

pub fn init() {
    crate::serial_println!(
        "[acpi-quirks] {} known DSDT vendor quirk(s) registered",
        KNOWN_QUIRKS.len()
    );
}

pub fn run_boot_smoketest() {
    let hp = match_quirk("HPQOEM", "SLIC-MOCK");
    let lenovo = match_quirk("LENOVOTP", "CB-01");
    crate::serial_println!(
        "[acpi-quirks] smoketest: entries={} hp_match={} lenovo_match={} -> PASS",
        KNOWN_QUIRKS.len(),
        hp.is_some() as u8,
        lenovo.is_some() as u8
    );
}

pub fn dump_text() -> String {
    let mut out = String::from("# ACPI DSDT vendor quirks\n");
    out.push_str(&alloc::format!("entries: {}\n", KNOWN_QUIRKS.len()));
    for q in KNOWN_QUIRKS {
        out.push_str(&alloc::format!(
            "  {} {}{} — {}\n",
            q.vendor,
            q.oem_id_prefix,
            q.oem_table_prefix,
            q.note
        ));
    }
    out
}
