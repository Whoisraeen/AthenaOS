//! Windows pain-point gap tracker — Concept §Windows Pain Points → RaeenOS Solutions.
//!
//! Maps each row in `RaeenOS_Concept.md` "Windows Pain Points → RaeenOS Solutions"
//! to kernel-layer status (DONE / PARTIAL / STUB / USERSPACE / MISSING).
//! Userspace-only answers are labelled honestly; the kernel row is never
//! silently marked DONE.

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapStatus {
    Done,
    Partial,
    Stub,
    Userspace,
    Missing,
}

impl GapStatus {
    fn tag(self) -> &'static str {
        match self {
            GapStatus::Done => "DONE",
            GapStatus::Partial => "PARTIAL",
            GapStatus::Stub => "STUB",
            GapStatus::Userspace => "USERSPACE",
            GapStatus::Missing => "MISSING",
        }
    }
}

struct Row {
    windows: &'static str,
    raeen_answer: &'static str,
    status: GapStatus,
    kernel_module: &'static str,
}

fn rows() -> &'static [Row] {
    &[
        Row {
            windows: "Forced restarts for updates",
            raeen_answer: "User controls update timing",
            status: GapStatus::Userspace,
            kernel_module: "raeupdate (userspace)",
        },
        Row {
            windows: "Update breaks your machine",
            raeen_answer: "Atomic CoW updates + rollback",
            status: GapStatus::Stub,
            kernel_module: "raefs.rs (snapshot API stub)",
        },
        Row {
            windows: "Bloatware preinstalled",
            raeen_answer: "Zero — OS only default install",
            status: GapStatus::Userspace,
            kernel_module: "raestore manifest policy",
        },
        Row {
            windows: "Ads in Start / Explorer / lock screen",
            raeen_answer: "Forbidden by design",
            status: GapStatus::Userspace,
            kernel_module: "raeshell policy",
        },
        Row {
            windows: "Telemetry hard to disable",
            raeen_answer: "Off by default, opt-in",
            status: GapStatus::Done,
            kernel_module: "config_registry (/system/telemetry_enabled=false)",
        },
        Row {
            windows: "Registry graveyard",
            raeen_answer: "Versioned hierarchical config + snapshots",
            status: GapStatus::Done,
            kernel_module: "config_registry.rs (syscalls 50-53)",
        },
        Row {
            windows: "DLL hell",
            raeen_answer: "App bundles with hashed deps",
            status: GapStatus::Partial,
            kernel_module: "app_bundle.rs (verify syscall 66-67)",
        },
        Row {
            windows: "Settings vs Control Panel split",
            raeen_answer: "Single unified Settings",
            status: GapStatus::Userspace,
            kernel_module: "raesettings app",
        },
        Row {
            windows: "Search is broken",
            raeen_answer: "Local-first indexed sub-100ms",
            status: GapStatus::Done,
            kernel_module: "search_index.rs (syscalls 54-57)",
        },
        Row {
            windows: "Random reboots",
            raeen_answer: "Never without explicit consent",
            status: GapStatus::Partial,
            kernel_module: "perm_prompt.rs + suspend.rs stub",
        },
        Row {
            windows: "Slow boot",
            raeen_answer: "<6s NVMe, target 3s",
            status: GapStatus::Partial,
            kernel_module: "procfs /proc/raeen/boot + fast_boot.rs",
        },
        Row {
            windows: "Driver Wild West",
            raeen_answer: "Signed drivers + IOMMU sandbox",
            status: GapStatus::Partial,
            kernel_module: "capability.rs + iommu.rs stub + storage_irq.rs",
        },
        Row {
            windows: "Pushed Copilot / Cortana / Edge",
            raeen_answer: "AI optional, off by default",
            status: GapStatus::Userspace,
            kernel_module: "raeshell install policy",
        },
        Row {
            windows: "File Explorer from 2007",
            raeen_answer: "Modern file manager",
            status: GapStatus::Partial,
            kernel_module: "vfs.rs hierarchical paths + mkdir/unlink/rename syscalls",
        },
        Row {
            windows: "WSL friction",
            raeen_answer: "Linux subsystem first-class",
            status: GapStatus::Partial,
            kernel_module: "linux_syscall.rs + linux_exec.rs (SYS_LINUX_EXEC=5000)",
        },
    ]
}

pub fn init() {
    let done = rows()
        .iter()
        .filter(|r| r.status == GapStatus::Done)
        .count();
    let partial = rows()
        .iter()
        .filter(|r| r.status == GapStatus::Partial)
        .count();
    crate::serial_println!(
        "[windows_gap] {} pain-point rows mapped (DONE={} PARTIAL={})",
        rows().len(),
        done,
        partial,
    );
}

pub fn dump_text() -> String {
    let mut out = String::from("# RaeenOS Windows pain-point kernel map\n");
    out.push_str("# Source: RaeenOS_Concept.md §Windows Pain Points → RaeenOS Solutions\n\n");
    for (i, r) in rows().iter().enumerate() {
        out.push_str(&alloc::format!(
            "[{:02}] status={:<10} kernel={}\n     windows: {}\n     raeen:   {}\n",
            i + 1,
            r.status.tag(),
            r.kernel_module,
            r.windows,
            r.raeen_answer,
        ));
    }
    let done = rows()
        .iter()
        .filter(|r| r.status == GapStatus::Done)
        .count();
    out.push_str(&alloc::format!(
        "\nsummary: total={} done={} partial={}\n",
        rows().len(),
        done,
        rows()
            .iter()
            .filter(|r| r.status == GapStatus::Partial)
            .count(),
    ));
    out
}

pub fn run_boot_smoketest() {
    let n = rows().len();
    if n >= 14 {
        crate::serial_println!("[windows_gap] smoketest OK: {} Concept-doc rows mapped", n);
    } else {
        crate::serial_println!(
            "[windows_gap] smoketest FAIL: expected >=14 rows, got {}",
            n
        );
    }
}
