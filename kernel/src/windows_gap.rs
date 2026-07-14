//! Windows pain-point gap tracker — Concept §Windows Pain Points → AthenaOS Solutions.
//!
//! Maps each row in `LEGACY_GAMING_CONCEPT.md` "Windows Pain Points → AthenaOS Solutions"
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
    athena_answer: &'static str,
    status: GapStatus,
    kernel_module: &'static str,
}

fn rows() -> &'static [Row] {
    &[
        Row {
            windows: "Forced restarts for updates",
            athena_answer: "User controls update timing",
            status: GapStatus::Userspace,
            kernel_module: "athupdate (userspace)",
        },
        Row {
            windows: "Update breaks your machine",
            athena_answer: "Atomic CoW updates + rollback",
            status: GapStatus::Stub,
            kernel_module: "athfs.rs (snapshot API stub)",
        },
        Row {
            windows: "Bloatware preinstalled",
            athena_answer: "Zero — OS only default install",
            status: GapStatus::Userspace,
            kernel_module: "athstore manifest policy",
        },
        Row {
            windows: "Ads in Start / Explorer / lock screen",
            athena_answer: "Forbidden by design",
            status: GapStatus::Userspace,
            kernel_module: "athshell policy",
        },
        Row {
            windows: "Telemetry hard to disable",
            athena_answer: "Off by default, opt-in",
            status: GapStatus::Done,
            kernel_module: "config_registry (/system/telemetry_enabled=false)",
        },
        Row {
            windows: "Registry graveyard",
            athena_answer: "Versioned hierarchical config + snapshots",
            status: GapStatus::Done,
            kernel_module: "config_registry.rs (syscalls 50-53)",
        },
        Row {
            windows: "DLL hell",
            athena_answer: "App bundles with hashed deps",
            status: GapStatus::Partial,
            kernel_module: "app_bundle.rs (verify syscall 66-67)",
        },
        Row {
            windows: "Settings vs Control Panel split",
            athena_answer: "Single unified Settings",
            status: GapStatus::Userspace,
            kernel_module: "athsettings app",
        },
        Row {
            windows: "Search is broken",
            athena_answer: "Local-first indexed sub-100ms",
            status: GapStatus::Done,
            kernel_module: "search_index.rs (syscalls 54-57)",
        },
        Row {
            windows: "Random reboots",
            athena_answer: "Never without explicit consent",
            status: GapStatus::Partial,
            kernel_module: "perm_prompt.rs + suspend.rs stub",
        },
        Row {
            windows: "Slow boot",
            athena_answer: "<6s NVMe, target 3s",
            status: GapStatus::Partial,
            kernel_module: "procfs /proc/athena/boot + fast_boot.rs",
        },
        Row {
            windows: "Driver Wild West",
            athena_answer: "Signed drivers + IOMMU sandbox",
            status: GapStatus::Partial,
            kernel_module: "capability.rs + iommu.rs stub + storage_irq.rs",
        },
        Row {
            windows: "Pushed Copilot / Cortana / Edge",
            athena_answer: "AI optional, off by default",
            status: GapStatus::Userspace,
            kernel_module: "athshell install policy",
        },
        Row {
            windows: "File Explorer from 2007",
            athena_answer: "Modern file manager",
            status: GapStatus::Partial,
            kernel_module: "vfs.rs hierarchical paths + mkdir/unlink/rename syscalls",
        },
        Row {
            windows: "WSL friction",
            athena_answer: "Linux subsystem first-class",
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
    let mut out = String::from("# AthenaOS Windows pain-point kernel map\n");
    out.push_str(
        "# Source: LEGACY_GAMING_CONCEPT.md §Windows Pain Points → AthenaOS Solutions\n\n",
    );
    for (i, r) in rows().iter().enumerate() {
        out.push_str(&alloc::format!(
            "[{:02}] status={:<10} kernel={}\n     windows: {}\n     athena:   {}\n",
            i + 1,
            r.status.tag(),
            r.kernel_module,
            r.windows,
            r.athena_answer,
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
