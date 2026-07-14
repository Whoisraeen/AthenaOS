//! AML parser probe: parse dumped ACPI tables with the vendored `aml` crate
//! on the host (MasterChecklist Phase 1.4 — Athena's empty namespace).
//!
//! Usage:
//!   cargo run -- <table.dat> [more tables...]      # parse in order
//!   RUST_LOG=trace cargo run -- DSDT.dat 2>trace   # full parse trail
//!
//! Tables are full SDTs (36-byte header + AML body); the header is stripped
//! here, same as kernel/src/acpi_full.rs. Parse order matters: the DSDT must
//! come first so SSDTs can resolve \_SB.PCI0 etc.

use aml::{AmlContext, AmlName, DebugVerbosity, Handler, LevelType};

/// No-op hardware handler: region reads return 0, writes are ignored. Table
/// PARSING only touches hardware for OpRegion accesses in executed code paths
/// (e.g. Load-time If predicates), where zeros are a safe default.
struct NullHandler;

impl Handler for NullHandler {
    fn read_u8(&self, _: usize) -> u8 {
        0
    }
    fn read_u16(&self, _: usize) -> u16 {
        0
    }
    fn read_u32(&self, _: usize) -> u32 {
        0
    }
    fn read_u64(&self, _: usize) -> u64 {
        0
    }
    fn write_u8(&mut self, _: usize, _: u8) {}
    fn write_u16(&mut self, _: usize, _: u16) {}
    fn write_u32(&mut self, _: usize, _: u32) {}
    fn write_u64(&mut self, _: usize, _: u64) {}
    fn read_io_u8(&self, _: u16) -> u8 {
        0
    }
    fn read_io_u16(&self, _: u16) -> u16 {
        0
    }
    fn read_io_u32(&self, _: u16) -> u32 {
        0
    }
    fn write_io_u8(&self, _: u16, _: u8) {}
    fn write_io_u16(&self, _: u16, _: u16) {}
    fn write_io_u32(&self, _: u16, _: u32) {}
    fn read_pci_u8(&self, _: u16, _: u8, _: u8, _: u8, _: u16) -> u8 {
        0
    }
    fn read_pci_u16(&self, _: u16, _: u8, _: u8, _: u8, _: u16) -> u16 {
        0
    }
    fn read_pci_u32(&self, _: u16, _: u8, _: u8, _: u8, _: u16) -> u32 {
        0
    }
    fn write_pci_u8(&self, _: u16, _: u8, _: u8, _: u8, _: u16, _: u8) {}
    fn write_pci_u16(&self, _: u16, _: u8, _: u8, _: u8, _: u16, _: u16) {}
    fn write_pci_u32(&self, _: u16, _: u8, _: u8, _: u8, _: u16, _: u32) {}
}

fn main() {
    env_logger::init();
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: aml_probe <table.dat> [more tables...]");
        std::process::exit(2);
    }

    let mut ctx = AmlContext::new(Box::new(NullHandler), DebugVerbosity::All);
    let mut failures = 0usize;

    for path in &paths {
        let bytes = std::fs::read(path).unwrap_or_else(|e| {
            eprintln!("read {path}: {e}");
            std::process::exit(2);
        });
        if bytes.len() <= 36 {
            println!("{path}: too short ({} bytes), skipping", bytes.len());
            continue;
        }
        let sig = String::from_utf8_lossy(&bytes[0..4]).into_owned();
        let oem_table = String::from_utf8_lossy(&bytes[16..24]).into_owned();
        let body = &bytes[36..];
        match ctx.parse_table(body) {
            Ok(()) => println!("{path} [{sig}/{oem_table}]: OK ({} bytes AML)", body.len()),
            Err(e) => {
                failures += 1;
                println!(
                    "{path} [{sig}/{oem_table}]: FAILED after parsing — {e:?} ({} bytes AML)",
                    body.len()
                );
            }
        }
    }

    // Namespace census: count devices the way the kernel's smoketest does.
    let mut devices = 0usize;
    let mut names = 0usize;
    let _ = ctx.namespace.traverse(|_name: &AmlName, level| {
        if level.typ == LevelType::Device {
            devices += 1;
        }
        names += level.values.len();
        Ok(true)
    });
    println!("---");
    println!(
        "namespace: {} device(s), {} value(s), {} table(s) failed",
        devices, names, failures
    );

    // Method-runtime probe: the boot path evaluates these on iron; failures
    // here reproduce Athena's "[acpi][warn] \_PIC failed: UnexpectedByte(..)"
    // class of bugs without a flash.
    println!("--- method evaluation ---");
    let mut method_failures = 0usize;
    match ctx.invoke_method(
        &AmlName::from_str("\\_PIC").unwrap(),
        aml::value::Args::from_list(vec![aml::AmlValue::Integer(1)]).unwrap(),
    ) {
        Ok(v) => println!("\\_PIC(1): OK -> {v:?}"),
        Err(e) => {
            method_failures += 1;
            println!("\\_PIC(1): FAILED -> {e:?}");
        }
    }

    // Find every _PRT in the namespace and evaluate it (they are Methods on
    // this firmware; the kernel needs the returned Package for IRQ routing).
    let mut prt_paths: Vec<AmlName> = Vec::new();
    let _ = ctx.namespace.traverse(|name: &AmlName, level| {
        for (seg, _) in level.values.iter() {
            if seg.as_str() == "_PRT" {
                if let Ok(full) = AmlName::from_str(&format!("{}._PRT", name.as_string())) {
                    prt_paths.push(full);
                }
            }
        }
        Ok(true)
    });
    println!("_PRT objects found: {}", prt_paths.len());
    for p in &prt_paths {
        match ctx.invoke_method(p, aml::value::Args::default()) {
            Ok(aml::AmlValue::Package(entries)) => {
                println!("{}: OK -> Package with {} entries", p.as_string(), entries.len())
            }
            Ok(v) => println!("{}: OK -> {v:?}", p.as_string()),
            Err(e) => {
                method_failures += 1;
                println!("{}: FAILED -> {e:?}", p.as_string());
            }
        }
    }

    if failures > 0 || method_failures > 0 {
        std::process::exit(1);
    }
}
