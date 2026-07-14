//! Build script: assemble the SMP trampoline with NASM into a raw .bin so
//! the kernel can `include_bytes!()` it and copy it to physical 0x8000 at boot.
//!
//! NASM is required (https://www.nasm.us/). Windows install paths we probe:
//!   * `nasm.exe` on PATH
//!   * `C:\Program Files\NASM\nasm.exe`
//!   * `C:\Program Files (x86)\NASM\nasm.exe`
//!   * `%LOCALAPPDATA%\bin\NASM\nasm.exe`  (winget default for NASM.NASM)

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn find_nasm() -> Option<PathBuf> {
    // Honor an explicit override first.
    if let Ok(p) = env::var("NASM") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    // PATH lookup.
    let probe = if cfg!(windows) {
        Command::new("where").arg("nasm").output()
    } else {
        Command::new("which").arg("nasm").output()
    };
    if let Ok(out) = probe {
        if out.status.success() {
            let first = String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if !first.is_empty() && Path::new(&first).exists() {
                return Some(PathBuf::from(first));
            }
        }
    }
    // Common Windows install locations.
    let mut candidates: Vec<PathBuf> = vec![
        PathBuf::from(r"C:\Program Files\NASM\nasm.exe"),
        PathBuf::from(r"C:\Program Files (x86)\NASM\nasm.exe"),
    ];
    if let Ok(local) = env::var("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(local)
                .join("bin")
                .join("NASM")
                .join("nasm.exe"),
        );
    }
    if let Ok(home) = env::var("USERPROFILE") {
        candidates.push(
            PathBuf::from(home)
                .join("AppData")
                .join("Local")
                .join("bin")
                .join("NASM")
                .join("nasm.exe"),
        );
    }
    candidates.into_iter().find(|p| p.exists())
}

fn main() {
    let asm_in = Path::new("src/smp/trampoline.asm");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let bin_out = out_dir.join("ap_trampoline.bin");

    println!("cargo:rerun-if-changed={}", asm_in.display());
    println!("cargo:rerun-if-env-changed=NASM");

    let nasm = find_nasm().unwrap_or_else(|| {
        panic!(
            "NASM not found. Install it (Windows: `winget install NASM.NASM`, \
             macOS: `brew install nasm`, Linux: `apt install nasm`) or set NASM=/path/to/nasm."
        )
    });

    let status = Command::new(&nasm)
        .arg("-f")
        .arg("bin")
        .arg(asm_in)
        .arg("-o")
        .arg(&bin_out)
        .status()
        .expect("failed to invoke nasm");
    if !status.success() {
        panic!(
            "nasm failed to assemble {} (exit {:?})",
            asm_in.display(),
            status.code()
        );
    }

    // Expose the path to the .bin file as an env var for include_bytes!.
    println!("cargo:rustc-env=AP_TRAMPOLINE_BIN={}", bin_out.display());

    generate_acpi_test_tables(&out_dir);
}

/// For the `embed_test_dsdt` debug feature: generate a Rust module that embeds
/// one board's ACPI tables (DSDT + every SSDT) so QEMU can parse real firmware.
/// The board directory is `RAEEN_DSDT_DIR` (set per-board by the corpus harness);
/// when unset, the tables are empty and the kernel falls back to the firmware's.
/// This handles an arbitrary SSDT count without hardcoding filenames.
fn generate_acpi_test_tables(out_dir: &Path) {
    println!("cargo:rerun-if-env-changed=RAEEN_DSDT_DIR");
    let gen = out_dir.join("acpi_test_tables.rs");
    let mut src = String::new();

    let dir = env::var("RAEEN_DSDT_DIR")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.is_dir());

    if let Some(dir) = dir {
        println!("cargo:rerun-if-changed={}", dir.display());
        let dsdt = dir.join("dsdt.dat");
        if dsdt.is_file() {
            src.push_str(&format!(
                "pub static TEST_DSDT: &[u8] = include_bytes!(r\"{}\");\n",
                dsdt.display()
            ));
        } else {
            src.push_str("pub static TEST_DSDT: &[u8] = &[];\n");
        }

        let mut ssdts: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .map(|n| n.starts_with("ssdt") && n.ends_with(".dat"))
                            .unwrap_or(false)
                    })
                    .collect()
            })
            .unwrap_or_default();
        ssdts.sort();
        src.push_str("pub static TEST_SSDTS: &[&[u8]] = &[\n");
        for s in &ssdts {
            src.push_str(&format!("    include_bytes!(r\"{}\"),\n", s.display()));
        }
        src.push_str("];\n");
    } else {
        src.push_str("pub static TEST_DSDT: &[u8] = &[];\n");
        src.push_str("pub static TEST_SSDTS: &[&[u8]] = &[];\n");
    }

    std::fs::write(&gen, src).expect("write acpi_test_tables.rs");
    println!("cargo:rustc-env=RAEEN_ACPI_TEST_TABLES={}", gen.display());
}
