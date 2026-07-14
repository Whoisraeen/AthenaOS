//! DeviceTree (DTB / Flattened DeviceTree) walk — the aarch64 firmware-discovery
//! seam that replaces ACPI (ADR 0009 §4).
//!
//! On `-M virt`, QEMU builds the DTB at runtime and hands its physical base in
//! `x0` per the arm64 Linux boot protocol. This module parses that blob with the
//! `fdt` crate (`#![no_std]`) and extracts the handful of values the early boot
//! path needs: CPU count, RAM base, the PL011 UART base, and the GICv2
//! distributor/CPU bases. Pure parsing — it returns values; the kernel's
//! `arch/aarch64/firmware.rs` (spec slice A8) feeds it the real blob.
//!
//! ## Grounding
//! - DTB binary format: Devicetree Specification v0.4 (the `fdt` crate
//!   implements it).
//! - The expected QEMU-virt bases are the documented `hw/arm/virt.c`
//!   `base_memmap[]` values, mirrored in the bring-up spec's memory-map table:
//!   RAM `0x4000_0000`, PL011 UART `0x0900_0000`, GICv2 distributor
//!   `0x0800_0000`, GICv2 CPU interface `0x0801_0000`.
//!
//! NOTE: `qemu-system-aarch64` is not installed on the build host that
//! authored this crate, so the test fixture (`tests/virt.dtb`) is a
//! HAND-AUTHORED minimal DeviceTree encoding those documented QEMU-virt values
//! (see `examples/gen_fixture.rs` for the byte-exact generator). It is a real,
//! spec-valid DTB the `fdt` crate parses; it is NOT a capture from a live QEMU
//! run. When `qemu-system-aarch64` is available, replace it with
//! `qemu-system-aarch64 -M virt -cpu cortex-a72 -smp 4 -machine
//! dumpdtb=tests/virt.dtb -nographic` and the same assertions hold.

use fdt::Fdt;

/// The platform topology the early aarch64 boot path discovers from the DTB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Topology {
    /// Number of `cpu@N` nodes under `/cpus`.
    pub cpu_count: usize,
    /// Base physical address of main RAM (`/memory` reg).
    pub ram_base: u64,
    /// Size of main RAM in bytes.
    pub ram_size: u64,
    /// PL011 UART MMIO base.
    pub uart_base: u64,
    /// GICv2 distributor MMIO base.
    pub gic_dist_base: u64,
    /// GICv2 CPU-interface MMIO base.
    pub gic_cpu_base: u64,
}

/// Documented QEMU-virt expectations (for the cross-check the spec's A8 slice
/// asserts against the hardcoded A3 bases).
pub mod virt_expected {
    /// Main RAM base on QEMU `-M virt`.
    pub const RAM_BASE: u64 = 0x4000_0000;
    /// PL011 UART0 base.
    pub const UART_BASE: u64 = 0x0900_0000;
    /// GICv2 distributor base.
    pub const GIC_DIST_BASE: u64 = 0x0800_0000;
    /// GICv2 CPU-interface base.
    pub const GIC_CPU_BASE: u64 = 0x0801_0000;
}

/// Errors that can arise walking the DTB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DtbError {
    /// The blob failed `fdt` parsing (bad magic, truncated, ...).
    ParseFailed,
    /// `/memory` node missing or had no `reg`.
    NoMemory,
    /// No PL011 UART node found.
    NoUart,
    /// No GICv2 node, or it lacked the two expected `reg` regions.
    NoGic,
}

/// Parse a flattened DeviceTree blob and extract the boot topology.
pub fn parse(blob: &[u8]) -> Result<Topology, DtbError> {
    let fdt = Fdt::new(blob).map_err(|_| DtbError::ParseFailed)?;

    let cpu_count = fdt.cpus().count();

    // /memory: first reg region = (base, size).
    let mem = fdt.memory();
    let region = mem.regions().next().ok_or(DtbError::NoMemory)?;
    let ram_base = region.starting_address as u64;
    let ram_size = region.size.ok_or(DtbError::NoMemory)? as u64;

    // PL011 UART — match by compatible string "arm,pl011".
    let uart = fdt
        .find_compatible(&["arm,pl011"])
        .ok_or(DtbError::NoUart)?;
    let uart_reg = uart
        .reg()
        .and_then(|mut r| r.next())
        .ok_or(DtbError::NoUart)?;
    let uart_base = uart_reg.starting_address as u64;

    // GICv2 — "arm,cortex-a15-gic" is the standard GICv2 compatible QEMU emits.
    // Its reg has TWO regions: [0] distributor, [1] CPU interface.
    let gic = fdt
        .find_compatible(&["arm,cortex-a15-gic", "arm,gic-400"])
        .ok_or(DtbError::NoGic)?;
    let mut gregs = gic.reg().ok_or(DtbError::NoGic)?;
    let dist = gregs.next().ok_or(DtbError::NoGic)?;
    let cpuif = gregs.next().ok_or(DtbError::NoGic)?;

    Ok(Topology {
        cpu_count,
        ram_base,
        ram_size,
        uart_base,
        gic_dist_base: dist.starting_address as u64,
        gic_cpu_base: cpuif.starting_address as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // The hand-authored QEMU-virt fixture (see tests/gen_fixture.rs).
    const VIRT_DTB: &[u8] = include_bytes!("../tests/virt.dtb");

    #[test]
    fn parses_qemu_virt_documented_values() {
        let topo = parse(VIRT_DTB).expect("fixture must parse");
        assert_eq!(topo.cpu_count, 4, "fixture encodes -smp 4");
        assert_eq!(topo.ram_base, virt_expected::RAM_BASE);
        assert_eq!(topo.uart_base, virt_expected::UART_BASE);
        assert_eq!(topo.gic_dist_base, virt_expected::GIC_DIST_BASE);
        assert_eq!(topo.gic_cpu_base, virt_expected::GIC_CPU_BASE);
        assert!(topo.ram_size >= 256 * 1024 * 1024, "fixture has >=256 MiB");
    }

    #[test]
    fn rejects_garbage_blob() {
        // Not a DTB (bad magic) -> ParseFailed, not a panic.
        let junk = [0u8; 64];
        assert_eq!(parse(&junk), Err(DtbError::ParseFailed));
    }

    // ---- FAIL-DEMONSTRATION ----
    #[test]
    fn faildemo_wrong_expected_base_fails() {
        // Prove the assertion is FAIL-able: if we expected the WRONG UART base
        // (e.g. the x86-ish 0x3F8 or a typo'd 0x0800_0000) the equality check
        // against the parsed value would fail.
        let topo = parse(VIRT_DTB).unwrap();
        let wrong_expected: u64 = 0x0800_0000; // GIC dist base, not the UART
        assert_ne!(
            topo.uart_base, wrong_expected,
            "the UART base must NOT equal a wrong expected value"
        );
        assert_eq!(topo.uart_base, virt_expected::UART_BASE);
    }

    #[test]
    fn faildemo_wrong_cpu_count_fails() {
        let topo = parse(VIRT_DTB).unwrap();
        assert_ne!(topo.cpu_count, 1, "a -smp 4 DTB must NOT report 1 CPU");
        assert_ne!(topo.cpu_count, 8);
    }
}
