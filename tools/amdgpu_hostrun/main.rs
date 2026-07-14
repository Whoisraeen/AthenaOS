//! amdgpu_hostrun — execute the REAL upstream `amdgpu_device_init` off-target.
//!
//! Concept §RaeGFX ("looks like Metal, performs like Vulkan"): the Year-1
//! real-GPU-submit campaign's M1 iron run wedged the box somewhere at/after the
//! CLKA #2 IP-discovery enumeration (docs/gpu-oracle/M1-VERDICT-20260706.md).
//! Every probe on iron costs a manual power cycle; this runner executes the same
//! C init graph, against the same shim, with the same pristine discovery blob,
//! as a normal Linux process on the dev box (identical Phoenix1 silicon) — so a
//! wedge is a gdb session, not a flash. Pure-logic bugs (parse walks, printf
//! facade, allocator) reproduce here; only genuinely MMIO-dependent behavior
//! needs iron.
//!
//! Usage (from the repo root):
//!   bash linuxkpi-drm/m4c-link.sh   # HOST-mode object graph
//!   cargo run --release --manifest-path tools/amdgpu_hostrun/Cargo.toml \
//!       [-- --fw-dir firmware --cfg docs/gpu-oracle/pci-config-c4000-phoenix.bin \
//!           --timeout 180]
//!
//! Exit: 0 = init returned (any code — that is already past the iron wedge),
//! SIGABRT via watchdog = wedge reproduced (core-dump/gdb it).

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};

// The daemon entry seam (linuxkpi-drm/bringup_entry.c) + the module params the
// runner steers (bringup_params.c) — all inside the merged object graph.
extern "C" {
    fn rae_amdgpu_device_init(
        vendor: u16,
        device: u16,
        revision: u8,
        pci_bus: u8,
        pci_devfn: u8,
        bar0_phys: u64,
        bar0_size: u64,
        bar2_phys: u64,
        bar2_size: u64,
        bar5_phys: u64,
        bar5_size: u64,
    ) -> i32;
    static mut amdgpu_discovery: i32;
}

/// bringup_entry.c logs pointer triples through this hook; amdgpud implements it
/// over netlog. Here: stderr. (Overrides the merged object's weak stub.)
#[no_mangle]
pub extern "C" fn rae_dbg_ptrs(a: u64, b: u64, c: u64) {
    eprintln!("[hostrun] DBG a={a:#x} b={b:#x} c={c:#x}");
}

// The dev box IS the Athena reference silicon: Phoenix1 1002:15bf rev C3 at
// c4:00.0. BAR layout read from /sys/bus/pci/devices/0000:c4:00.0/resource.
const VENDOR: u16 = 0x1002;
const DEVICE: u16 = 0x15bf;
const REVISION: u8 = 0xc3;
const BAR0: (u64, u64) = (0x7c_0000_0000, 0x1000_0000); // VRAM aperture, 256 MiB
const BAR2: (u64, u64) = (0xdc00_0000, 0x20_0000); //      doorbells, 2 MiB
const BAR5: (u64, u64) = (0xdc50_0000, 0x8_0000); //       registers, 512 KiB
const PCI_BUS: u8 = 0xc4; // the VFCT VBIOS image is matched against this
const PCI_DEVFN: u8 = 0x00; // dev 0, fn 0

static INIT_RETURNED: AtomicBool = AtomicBool::new(false);

fn arg_after(args: &[String], key: &str) -> Option<String> {
    args.iter().position(|a| a == key).and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let fw_dir = arg_after(&args, "--fw-dir").unwrap_or_else(|| "firmware".into());
    let cfg_path = arg_after(&args, "--cfg")
        .unwrap_or_else(|| "docs/gpu-oracle/pci-config-c4000-phoenix.bin".into());
    let timeout_s: u64 = arg_after(&args, "--timeout").and_then(|v| v.parse().ok()).unwrap_or(180);

    // 1. Device + BAR registration — the same calls amdgpud makes before the C init.
    raeen_linuxkpi::device_map::lkpi_set_current_device(1);
    raeen_linuxkpi::device_map::lkpi_register_bar(0, BAR0.0, BAR0.1);
    raeen_linuxkpi::device_map::lkpi_register_bar(2, BAR2.0, BAR2.1);
    raeen_linuxkpi::device_map::lkpi_register_bar(5, BAR5.0, BAR5.1);

    // 2. Arm the functional host seam.
    raeen_linuxkpi::host::hostrun_install(&fw_dir);

    // 3. Seed the fake PCI config space from the REAL GPU's captured 4 KiB dump
    //    (oracle bytes, not guesses).
    match std::fs::File::open(&cfg_path) {
        Ok(mut f) => {
            let mut cfg = Vec::new();
            let _ = f.read_to_end(&mut cfg);
            for (i, chunk) in cfg.chunks_exact(4).enumerate() {
                let v = u32::from_le_bytes(chunk.try_into().unwrap());
                if v != 0 {
                    raeen_linuxkpi::host::hostrun_set_cfg_dword((i * 4) as u16, v);
                }
            }
            eprintln!("[hostrun] pci config seeded from {cfg_path} ({} bytes)", cfg.len());
        }
        Err(e) => eprintln!("[hostrun] WARN: no config seed ({cfg_path}: {e}) — cfg reads return 0"),
    }

    // 4. Pre-map the register BAR so oracle register values can be prefilled
    //    before the C init's first RREG32. (ioremap caches per-BAR, so the C
    //    side sees this same mapping.)
    let bar5_va = raeen_linuxkpi::device_map::ioremap_phys(BAR5.0, BAR5.1 as usize) as u64;
    assert!(bar5_va != 0, "BAR5 pre-map failed");
    // `--real-bar5`: umr-class READ-THROUGH to the live GPU. Register reads in
    // the BAR5 window return the REAL silicon's values (mmap of the device's
    // sysfs resource5, PROT_READ, needs root); writes still land in the fake
    // buffer — the live amdgpu keeps owning the hardware. Real RCC_CONFIG_
    // MEMSIZE/PSP/SMU state feeds the init; INDEX/DATA indirect reads are
    // garbage-but-harmless (the live driver's index selects them).
    if args.iter().any(|a| a == "--real-bar5") {
        let path = "/sys/bus/pci/devices/0000:c4:00.0/resource5";
        if raeen_linuxkpi::host::hostrun_set_real_bar(5, path) {
            eprintln!("[hostrun] BAR5 READ-THROUGH armed: reads come from live silicon ({path})");
        } else {
            eprintln!("[hostrun] WARN: --real-bar5 failed ({path}; need root) — staying fully fake");
        }
    }

    // 5. Steer discovery to the file path: no live VRAM TMR to read on host.
    unsafe { amdgpu_discovery = 2 };

    // 6. Watchdog — if the init wedges (the iron failure mode), SIGABRT so the
    //    core dump / gdb shows the exact spinning stack.
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(timeout_s));
        if !INIT_RETURNED.load(Ordering::SeqCst) {
            eprintln!("[hostrun] WATCHDOG: amdgpu_device_init still running after {timeout_s}s — aborting for backtrace (iron-wedge repro?)");
            std::process::abort();
        }
    });

    eprintln!("[hostrun] entering rae_amdgpu_device_init (real amdgpu C, host seam)");
    let r = unsafe {
        rae_amdgpu_device_init(
            VENDOR, DEVICE, REVISION, PCI_BUS, PCI_DEVFN, BAR0.0, BAR0.1, BAR2.0, BAR2.1, BAR5.0,
            BAR5.1,
        )
    };
    INIT_RETURNED.store(true, Ordering::SeqCst);
    eprintln!("[hostrun] rae_amdgpu_device_init returned {r} — no wedge on host up to this depth");
}
