//! Linux kernel ABI symbol registry — name resolution scaffold for
//! vendor `.ko` / out-of-tree module imports.
//!
//! > **LEGACY_GAMING_CONCEPT.md §Architecture:** *"User-space: … drivers
//! > (IOMMU-sandboxed) … Anything that can fail without taking the system
//! > down."* *"Driver isolation: Every driver runs in its own protection
//! > domain with IOMMU enforcement."*
//!
//! This module is **not** GPL Linux kernel code. It is an MPL-2.0–compatible
//! metadata table (symbol name → category + implementation status) modeled
//! after `components/athbridge/src/pe_dll_registry.rs`: loaders resolve
//! names to stub dispatch slots; real behavior lives in future AthenaOS
//! shims or userspace driver services.
//!
//! Complements [`crate::linux_compat`] (userspace ELF hosting) and
//! [`crate::linux_syscall`] (syscall translation). Does **not** implement
//! Linux struct layouts, epoll/kqueue, or a monolithic Linux clone (R7).

#![allow(dead_code)]

extern crate alloc;

use alloc::format;
use alloc::string::String;

/// Errors returned when a resolved stub is invoked — never silent success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxKabiError {
    /// Symbol is listed but dispatches to a stub that always fails.
    StubNotImplemented,
    /// Symbol is in the registry with `KabiStatus::Unimplemented`.
    Unimplemented,
    /// Symbol is planned for a future phase; not yet wired.
    Planned,
    /// Name is not present in the registry.
    UnknownSymbol,
}

impl LinuxKabiError {
    pub const fn as_i32(self) -> i32 {
        match self {
            Self::StubNotImplemented => -38, // ENOSYS
            Self::Unimplemented => -38,
            Self::Planned => -38,
            Self::UnknownSymbol => -2, // ENOENT
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KabiCategory {
    Memory,
    Pci,
    Irq,
    Device,
    Dma,
    Misc,
}

impl KabiCategory {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Pci => "pci",
            Self::Irq => "irq",
            Self::Device => "device",
            Self::Dma => "dma",
            Self::Misc => "misc",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KabiStatus {
    /// Listed; stub handler exists and returns [`LinuxKabiError::StubNotImplemented`].
    Stub,
    /// Listed; no dispatch yet.
    Unimplemented,
    /// Tracked for a future phase; not callable.
    Planned,
}

impl KabiStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stub => "stub",
            Self::Unimplemented => "unimplemented",
            Self::Planned => "planned",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct KabiSymbol {
    pub name: &'static str,
    pub category: KabiCategory,
    pub status: KabiStatus,
}

/// Static registry — ~40 canonical `EXPORT_SYMBOL` names drivers probe first.
static KABI_TABLE: &[KabiSymbol] = &[
    // Memory
    KabiSymbol {
        name: "kmalloc",
        category: KabiCategory::Memory,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "kfree",
        category: KabiCategory::Memory,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "kzalloc",
        category: KabiCategory::Memory,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "krealloc",
        category: KabiCategory::Memory,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "kcalloc",
        category: KabiCategory::Memory,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "kmemdup",
        category: KabiCategory::Memory,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "vmalloc",
        category: KabiCategory::Memory,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "vfree",
        category: KabiCategory::Memory,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "devm_kzalloc",
        category: KabiCategory::Memory,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "devm_kfree",
        category: KabiCategory::Memory,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "devm_kmalloc",
        category: KabiCategory::Memory,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "get_free_pages",
        category: KabiCategory::Memory,
        status: KabiStatus::Planned,
    },
    KabiSymbol {
        name: "free_pages",
        category: KabiCategory::Memory,
        status: KabiStatus::Planned,
    },
    KabiSymbol {
        name: "copy_from_user",
        category: KabiCategory::Memory,
        status: KabiStatus::Planned,
    },
    KabiSymbol {
        name: "copy_to_user",
        category: KabiCategory::Memory,
        status: KabiStatus::Planned,
    },
    // PCI
    KabiSymbol {
        name: "pci_register_driver",
        category: KabiCategory::Pci,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "pci_unregister_driver",
        category: KabiCategory::Pci,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "__pci_register_driver",
        category: KabiCategory::Pci,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "pci_enable_device",
        category: KabiCategory::Pci,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "pci_disable_device",
        category: KabiCategory::Pci,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "pci_set_master",
        category: KabiCategory::Pci,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "pci_request_regions",
        category: KabiCategory::Pci,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "pci_release_regions",
        category: KabiCategory::Pci,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "pci_iomap",
        category: KabiCategory::Pci,
        status: KabiStatus::Planned,
    },
    KabiSymbol {
        name: "pci_iounmap",
        category: KabiCategory::Pci,
        status: KabiStatus::Planned,
    },
    KabiSymbol {
        name: "pci_alloc_irq_vectors",
        category: KabiCategory::Pci,
        status: KabiStatus::Planned,
    },
    KabiSymbol {
        name: "pci_free_irq_vectors",
        category: KabiCategory::Pci,
        status: KabiStatus::Planned,
    },
    // IRQ
    KabiSymbol {
        name: "request_irq",
        category: KabiCategory::Irq,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "free_irq",
        category: KabiCategory::Irq,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "devm_request_irq",
        category: KabiCategory::Irq,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "devm_free_irq",
        category: KabiCategory::Irq,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "request_threaded_irq",
        category: KabiCategory::Irq,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "enable_irq",
        category: KabiCategory::Irq,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "disable_irq",
        category: KabiCategory::Irq,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "irq_set_affinity_hint",
        category: KabiCategory::Irq,
        status: KabiStatus::Planned,
    },
    // Device model
    KabiSymbol {
        name: "dev_printk",
        category: KabiCategory::Device,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "dev_err",
        category: KabiCategory::Device,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "dev_warn",
        category: KabiCategory::Device,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "dev_info",
        category: KabiCategory::Device,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "device_register",
        category: KabiCategory::Device,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "device_unregister",
        category: KabiCategory::Device,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "platform_driver_register",
        category: KabiCategory::Device,
        status: KabiStatus::Planned,
    },
    KabiSymbol {
        name: "platform_driver_unregister",
        category: KabiCategory::Device,
        status: KabiStatus::Planned,
    },
    // DMA
    KabiSymbol {
        name: "dma_alloc_coherent",
        category: KabiCategory::Dma,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "dma_free_coherent",
        category: KabiCategory::Dma,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "dma_map_single",
        category: KabiCategory::Dma,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "dma_unmap_single",
        category: KabiCategory::Dma,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "dma_set_mask",
        category: KabiCategory::Dma,
        status: KabiStatus::Planned,
    },
    KabiSymbol {
        name: "dmam_alloc_coherent",
        category: KabiCategory::Dma,
        status: KabiStatus::Planned,
    },
    // Misc (printk, module, workqueue, time)
    KabiSymbol {
        name: "printk",
        category: KabiCategory::Misc,
        status: KabiStatus::Stub,
    },
    KabiSymbol {
        name: "_printk",
        category: KabiCategory::Misc,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "module_put",
        category: KabiCategory::Misc,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "__module_get",
        category: KabiCategory::Misc,
        status: KabiStatus::Unimplemented,
    },
    KabiSymbol {
        name: "alloc_workqueue",
        category: KabiCategory::Misc,
        status: KabiStatus::Planned,
    },
    KabiSymbol {
        name: "queue_work",
        category: KabiCategory::Misc,
        status: KabiStatus::Planned,
    },
    KabiSymbol {
        name: "msleep",
        category: KabiCategory::Misc,
        status: KabiStatus::Planned,
    },
    KabiSymbol {
        name: "usleep_range",
        category: KabiCategory::Misc,
        status: KabiStatus::Planned,
    },
    KabiSymbol {
        name: "jiffies",
        category: KabiCategory::Misc,
        status: KabiStatus::Planned,
    },
];

/// Look up a symbol by exact name (linear scan — table is small).
pub fn lookup(name: &str) -> Option<&'static KabiSymbol> {
    KABI_TABLE.iter().find(|s| s.name == name)
}

/// Number of symbols in the static registry.
pub fn symbol_count() -> usize {
    KABI_TABLE.len()
}

/// Stub dispatch address for a registered symbol (monotonic fake addresses).
/// Real `.ko` relocation would use these until native shims land.
pub fn stub_address(name: &str) -> Option<u64> {
    let idx = KABI_TABLE.iter().position(|s| s.name == name)?;
    Some(0xC0DE_0000_0000_0000 + (idx as u64) * 16)
}

/// Invoke path for a stub symbol — always returns an error (no silent success).
pub fn dispatch_stub(name: &str) -> Result<(), LinuxKabiError> {
    match lookup(name) {
        Some(s) => match s.status {
            KabiStatus::Stub => Err(LinuxKabiError::StubNotImplemented),
            KabiStatus::Unimplemented => Err(LinuxKabiError::Unimplemented),
            KabiStatus::Planned => Err(LinuxKabiError::Planned),
        },
        None => Err(LinuxKabiError::UnknownSymbol),
    }
}

fn count_by_category(cat: KabiCategory) -> usize {
    KABI_TABLE.iter().filter(|s| s.category == cat).count()
}

fn count_by_status(st: KabiStatus) -> usize {
    KABI_TABLE.iter().filter(|s| s.status == st).count()
}

pub fn init() {
    crate::serial_println!(
        "[ OK ] Linux kABI symbol registry (scaffold, not GPL): {} names",
        KABI_TABLE.len(),
    );
}

/// Resolve canonical probe names; reports how many appear in the registry.
pub fn run_boot_smoketest() {
    const PROBES: &[&str] = &[
        "printk",
        "kmalloc",
        "kfree",
        "request_irq",
        "free_irq",
        "pci_register_driver",
        "pci_unregister_driver",
        "devm_kzalloc",
        "dma_alloc_coherent",
        "dev_err",
    ];
    let mut hits = 0usize;
    for name in PROBES {
        if lookup(name).is_some() {
            hits += 1;
        }
    }
    crate::serial_println!(
        "[linux_kabi] smoketest: {}/{} symbols registered",
        hits,
        PROBES.len(),
    );
}

/// Text dump for `/proc/athena/linux_kabi`.
pub fn dump_text() -> String {
    let total = KABI_TABLE.len();
    let mut out = String::new();
    out.push_str("# AthenaOS Linux kABI symbol registry (scaffold)\n");
    out.push_str("# NOT GPL Linux code — MPL-2.0 metadata + stub dispatch only.\n");
    out.push_str(&format!("total_symbols: {total}\n\n"));

    out.push_str("## By category\n");
    for cat in [
        KabiCategory::Memory,
        KabiCategory::Pci,
        KabiCategory::Irq,
        KabiCategory::Device,
        KabiCategory::Dma,
        KabiCategory::Misc,
    ] {
        out.push_str(&format!(
            "  {:<8} {}\n",
            cat.as_str(),
            count_by_category(cat)
        ));
    }

    out.push_str("\n## By status\n");
    for st in [
        KabiStatus::Stub,
        KabiStatus::Unimplemented,
        KabiStatus::Planned,
    ] {
        out.push_str(&format!("  {:<14} {}\n", st.as_str(), count_by_status(st)));
    }

    out.push_str("\n## Sample entries (name category status)\n");
    for sym in KABI_TABLE.iter().take(12) {
        out.push_str(&format!(
            "  {} {} {}\n",
            sym.name,
            sym.category.as_str(),
            sym.status.as_str(),
        ));
    }
    if total > 12 {
        out.push_str(&format!(
            "  ... and {} more (see docs/LINUX_DRIVER_STRATEGY.md)\n",
            total - 12
        ));
    }

    out.push_str("\n# Real handlers must return LinuxKabiError — stubs never succeed.\n");
    out.push_str("# Supported path: IOMMU-sandboxed userspace drivers (Concept §Architecture).\n");
    out
}
