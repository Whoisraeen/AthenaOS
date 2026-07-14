//! discovery_probe — validate a captured amdgpu IP-discovery blob OFF-TARGET.
//!
//! Reads `firmware/amdgpu/ip_discovery.bin` (or an argv path), runs the kernel's
//! exact parser (`raeen_amdgpu::discovery::parse_checked`) + SOC15 offset
//! resolvers (`raeen_amdgpu::regs::*`) on the host, and prints the parsed IP
//! blocks + the absolute register offsets the driver will use on iron. Proves the
//! firmware-file discovery path will go ACTIVE before flashing the Athena.

use raeen_amdgpu::{discovery, regs};
use std::fs;

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "firmware/amdgpu/ip_discovery.bin".to_string());

    let blob = match fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("[discovery-probe] cannot read {path}: {e}");
            std::process::exit(2);
        }
    };
    println!("[discovery-probe] {} ({} bytes)", path, blob.len());

    if blob.len() < 4 {
        eprintln!("[discovery-probe] too short");
        std::process::exit(1);
    }
    let sig = u32::from_le_bytes([blob[0], blob[1], blob[2], blob[3]]);
    println!(
        "[discovery-probe] signature {:#010x} (want {:#010x}) -> {}",
        sig,
        discovery::BINARY_SIGNATURE,
        if sig == discovery::BINARY_SIGNATURE {
            "OK"
        } else {
            "BAD"
        }
    );

    let Some(blocks) = discovery::parse_checked(&blob) else {
        eprintln!("[discovery-probe] parse_checked FAILED (bad signature or 0 blocks)");
        std::process::exit(1);
    };
    println!("[discovery-probe] parsed {} IP blocks:", blocks.len());
    for b in &blocks {
        println!(
            "  hw_id={:3} inst={} bases={:#x?}",
            b.hw_id, b.instance, b.bases
        );
    }

    println!("[discovery-probe] resolved SOC15 register offsets:");
    println!("  gfx_regs        {:#x?}", regs::gfx_regs(&blocks));
    println!("  sdma_regs       {:#x?}", regs::sdma_regs(&blocks));
    println!("  smu_mailbox     {:#x?}", regs::smu_mailbox(&blocks));
    println!("  ih_ring         {:#x?}", regs::ih_ring(&blocks));
    println!("  rlc_safe_mode   {:#x?}", regs::rlc_safe_mode(&blocks));
    println!("  config_memsize  {:#x?}", regs::config_memsize_reg(&blocks));

    // The moment of truth: are the four blocks the bring-up needs all present,
    // and do the load-bearing resolvers return Some?
    let need = [
        ("GC", regs::GC_HWID),
        ("MP1", regs::MP1_HWID),
        ("OSSSYS", regs::OSSSYS_HWID),
        ("NBIF", regs::NBIF_HWID),
    ];
    println!("[discovery-probe] required blocks:");
    let mut all_blocks = true;
    for (name, hwid) in need {
        let present = blocks.iter().any(|b| b.hw_id == hwid);
        println!(
            "  {:7} (hwid {:3}): {}",
            name,
            hwid,
            if present { "PRESENT" } else { "MISSING" }
        );
        all_blocks &= present;
    }

    let resolvers_ok = regs::gfx_regs(&blocks).is_some()
        && regs::sdma_regs(&blocks).is_some()
        && regs::smu_mailbox(&blocks).is_some()
        && regs::ih_ring(&blocks).is_some()
        && regs::config_memsize_reg(&blocks).is_some();

    if all_blocks && resolvers_ok {
        println!(
            "[discovery-probe] RESULT: PASS — discovery parses + every SOC15 offset resolves. \
             The firmware-file path will go ACTIVE on the next Athena flash."
        );
    } else {
        println!(
            "[discovery-probe] RESULT: INCOMPLETE — some required block/resolver is missing \
             (blocks_ok={all_blocks} resolvers_ok={resolvers_ok})."
        );
        std::process::exit(1);
    }
}
