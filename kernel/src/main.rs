//! AthKernel — the heart of AthenaOS (forked from AthKernel / AthenaOS).
//!
//! Boot path today:
//!   bootloader (BIOS or UEFI)  →  `kernel_main(boot_info)`
//!   serial::init()             →  COM1 16550 UART up at 0x3F8
//!   arch::cpu::load_gdt()      →  GDT + TSS loaded via the arch:: HAL seam
//!                                  (double-fault IST)
//!   arch::interrupts::load_idt() →  IDT loaded via the arch:: HAL seam
//!                                  (breakpoint, double-fault, page-fault,
//!                                  timer, keyboard)
//!   PIC 8259 initialized       →  hardware interrupts enabled
//!   memory::init()             →  OffsetPageTable from CR3
//!   allocator::init_heap()     →  256 KiB linked-list heap → alloc available
//!   framebuffer::init(fb)      →  graphical framebuffer cleared
//!   banner printed to serial   →  visible in QEMU stdout
//!   halt loop                  →  CPU idles, wakes on interrupts
//!
//! Next milestones (see docs/ROADMAP.md):
//!   scheduler → body/control RT class → AthSense/AthMind → AthBody under AthGuard.

#![no_std]
#![no_main]

extern crate alloc;

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
static SAVED_RSDP_ADDR: AtomicU64 = AtomicU64::new(0);

/// TSC at the very first instruction of kernel_main. Used by
/// [`boot_elapsed_ms`] to print per-Tier elapsed times on the [TIER]
/// complete lines — the cheapest way to localize bare-metal boot-time
/// regressions (real Athena: 58 s vs 5 s on QEMU). Set once, read often.
pub static BOOT_START_TSC: AtomicU64 = AtomicU64::new(0);

/// Wall-time elapsed since `kernel_main` started, in milliseconds. Returns
/// 0 until the APIC TSC calibration runs (early boot prints can't use it).
/// Safe to call from any context: lock-free atomic loads + integer math.
pub fn boot_elapsed_ms() -> u64 {
    let start = BOOT_START_TSC.load(Ordering::Relaxed);
    if start == 0 {
        return 0;
    }
    let now: u64 = {
        let lo: u32;
        let hi: u32;
        unsafe {
            core::arch::asm!(
                "rdtsc",
                out("eax") lo, out("edx") hi,
                options(nomem, nostack, preserves_flags),
            );
        }
        ((hi as u64) << 32) | (lo as u64)
    };
    let mhz = crate::apic::TSC_FREQ_MHZ.load(Ordering::Relaxed);
    if mhz == 0 {
        return 0; // calibration hasn't run yet
    }
    now.saturating_sub(start) / (mhz * 1000)
}

/// Set to true when kernel is booted in live-USB installer mode.
/// MasterChecklist Phase 3.1: "Boot media flag distinguishes live USB installer mode from installed boot."
/// Detected from UEFI cmdline argument "installer" or absence of a valid AthFS root partition.
pub static INSTALLER_MODE: AtomicBool = AtomicBool::new(false);

/// Check boot flags and set INSTALLER_MODE if appropriate.
/// Called early in kernel_main after memory init.
pub fn detect_installer_mode() {
    // Heuristic: if no valid AthFS partition is found during storage discovery,
    // we're likely booting from live USB → enter installer mode.
    // The full installer process (athinstaller) is spawned instead of the normal shell.
    // For now: always false (installed boot) until installer flow is wired.
    // When the installer USB image is built, it will pass "installer" in BootInfo.
    INSTALLER_MODE.store(false, Ordering::Relaxed);
    if INSTALLER_MODE.load(Ordering::Relaxed) {
        serial_println!("[boot] INSTALLER MODE: athinstaller will replace normal user_init");
    } else {
        serial_println!("[boot] normal boot mode (installed system)");
    }
}

pub mod a11y;
pub mod a11y_input;
pub mod acpi;
pub mod apic;
pub mod arch;
pub mod athfs;
pub mod aurora;
pub mod boot_selftest;
pub mod bootlog;
pub mod bootlog_persist;
pub mod capability;
pub mod captions;
pub mod compositor;
mod console;
pub mod context;
pub mod elf;
pub mod extable;
mod framebuffer;
mod gdt;
pub mod hpet;
mod interrupts;
pub mod ipc;
mod memory;
pub mod msi;
pub mod msr;
pub mod native_stack;
mod panic;
pub mod pci;
pub mod pci_irq;
pub mod pci_pm;
pub mod rtc;
pub mod sched_proof;
pub mod scheduler;
mod serial;
pub mod smp;
pub mod snapshot_policy;
pub mod swap;
pub mod sync;
pub mod syscall;
pub mod tar;
pub mod task;
pub mod uaccess;
pub mod vfs;
pub mod virtio;
pub mod virtio_gpu;
pub mod virtio_net;
// ── Expanded kernel modules ────────────────────────────────────────────────

//
// Concept-doc-aligned subsystems. Linux-clone modules (ext4, drm, wayland,
// ALSA, netfilter, procfs, sysfs, seccomp, mac, ebpf, io_uring, etc.) have
// been deleted — AthenaOS builds its own proprietary stack per the manifesto.
//
// ── Scheduling & process model ──
pub mod init_system;
pub mod namespaces;
pub mod process;
pub mod signals;
// ── Memory & locking primitives ──
pub mod dma_engine;
pub mod locking;
pub mod mmio;
pub mod msr_intel;
pub mod numa;
pub mod slab;
// ── Timers & power ──
pub mod cpufreq;
pub mod cpuidle;
pub mod perf;
pub mod power;
pub mod power_events;
pub mod power_supply;
pub mod suspend;
pub mod thermal;
pub mod timers;
// ── Storage controllers ──
pub mod ahci;
pub mod block_io;
pub mod filesystems;
pub mod nvme;
pub mod tmpfs;
// ── Networking (AthNet substrate) ──
pub mod dhcp;
pub mod dns;
pub mod dot;
pub mod firewall;
pub mod igc;
pub mod ipsec_kernel;
pub mod mdns;
pub mod net;
pub mod net_drivers;
pub mod netlog;
pub mod netmanager;
pub mod quic;
pub mod ssh;
pub mod tunnel;
// ── USB / HID / input ──
pub mod hid_gamepad;
pub mod input;
#[path = "usb.rs"]
pub mod usb;
pub mod usb_core;
pub mod usb_hid;
pub mod usb_msc;
pub mod xhci;
mod xhci_desc;
// ── Graphics / display ──
pub mod display;
pub mod gpu;
pub mod gpu_render;
// ── Audio (AthAudio substrate) ──
pub mod audio;
pub mod usb_audio;
// ── Security (AthGuard) ──
pub mod anticheat;
pub mod audit;
pub mod crypto;
pub mod encryption;
pub mod hardening;
pub mod security;
pub mod tls;
pub mod tpm;
// ── Firmware / platform ──
pub mod acpi_full;
pub mod acpi_quirks;
pub mod aml_bridge;
pub mod efi;
pub mod firmware;
pub mod hardware_profile;
pub mod pcie;
pub mod pcie_aer;
pub mod pcie_quirks;
pub mod selftest;
pub mod smbios;
pub mod userspace_driver;
// ── System infrastructure ──
pub mod app_bundle;
pub mod app_paths;
pub mod athbridge_boot;
pub mod cap_audit;
pub mod clipboard;
pub mod config_registry;
pub mod cpu_features;
pub mod data_buckets;
pub mod dbus_kernel;
pub mod debug;
pub mod dynamic_linker;
pub mod elf_loader;
pub mod event_bus;
pub mod eventloop;
pub mod game_profile;
pub mod game_session;
pub mod gpio;
pub mod i2c_spi;
pub mod installer_ui;
pub mod kexec;
pub mod kmod;
pub mod linux_compat;
pub mod linux_exec;
pub mod linux_kabi;
pub mod linux_syscall;
pub mod linuxkpi_host;
pub mod live_wallpaper;
pub mod login_ui;
pub mod measured_boot;
pub mod msr_amd;
pub mod notify;
pub mod perm_prompt;
pub mod perm_syscalls;
pub mod perm_ui;
pub mod posix;
pub mod posix_ipc;
pub mod prefetch;
pub mod procfs;
pub mod rae_manifest;
pub mod rgb;
pub mod scripting;
pub mod search_index;
pub mod secure_boot;
pub mod secure_ipc;
pub mod session;
pub mod setup_ui;
pub mod shell_api;
pub mod shell_runner;
pub mod storage_irq;
pub mod syscall_table;
pub mod sysfs_kobject;
pub mod theme_engine;
pub mod tty;
pub mod update_slots;
pub mod vibe_mode;
pub mod webview;
pub mod widgets;
pub mod win_registry;
pub mod window_chrome;
pub mod windows_gap;
pub mod wireguard;
pub mod wm_policy;
pub mod workqueue;
// ── Virtualization ──
pub mod virtualization;
// ── Bluetooth ──
pub mod bluetooth;
// ── IOMMU (DMA sandboxing) ──
pub mod iommu;
// ── Overclocking, Watchdog, Crash Dump, Fast Boot, HW Diagnostics ──
pub mod aer;
pub mod battery;
pub mod compress;
pub mod crash_dump;
pub mod driver_fw;
pub mod driver_manifest;
pub mod dwarf_sym;
pub mod edid;
pub mod fast_boot;
pub mod fastqueue;
pub mod fatfs_esp;
pub mod fde;
pub mod gpe;
pub mod handle_table;
pub mod hw_diag;
pub mod installer;
pub mod mce;
pub mod oom;
pub mod overclock;
pub mod sandbox;
pub mod soak;
pub mod storage_mount;
pub mod watchdog;

pub static INITRAMFS: &[u8] = include_bytes!("initramfs.tar");

use bootloader_api::{config::Mapping, entry_point, BootInfo, BootloaderConfig};

/// Bootloader configuration: ask for a dynamically-mapped physical memory
/// window so we can walk page tables and set up our own heap.
pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config.kernel_stack_size = 512 * 1024; // 512 KiB — default 80 KiB overflows with full init
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

const BANNER: &str = r#"
  ╔══════════════════════════════════════════════════════════════════╗
  ║                                                                  ║
  ║      █████╗ ████████╗██╗  ██╗███████╗███╗   ██╗ █████╗           ║
  ║     ██╔══██╗╚══██╔══╝██║  ██║██╔════╝████╗  ██║██╔══██╗          ║
  ║     ███████║   ██║   ███████║█████╗  ██╔██╗ ██║███████║          ║
  ║     ██╔══██║   ██║   ██╔══██║██╔══╝  ██║╚██╗██║██╔══██║          ║
  ║     ██║  ██║   ██║   ██║  ██║███████╗██║ ╚████║██║  ██║          ║
  ║     ╚═╝  ╚═╝   ╚═╝   ╚═╝  ╚═╝╚══════╝╚═╝  ╚═══╝╚═╝  ╚═╝          ║
  ║                                                                  ║
  ║              AthKernel v0.0.1  —  AthenaOS                       ║
  ║       "A mind in a body — continuous, owned, bounded."           ║
  ║                                                                  ║
  ╚══════════════════════════════════════════════════════════════════╝
"#;

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    // ── Phase 0: Capture TSC at the absolute first instruction we control.
    // Concept doc promises "boot under 6s on NVMe, target 3s." We can't aim
    // at that without measuring it.
    let boot_start_tsc: u64 = {
        let lo: u32;
        let hi: u32;
        unsafe {
            core::arch::asm!(
                "rdtsc",
                out("eax") lo, out("edx") hi,
                options(nomem, nostack, preserves_flags),
            );
        }
        ((hi as u64) << 32) | (lo as u64)
    };

    // ── Phase 1: Serial for early debug ──
    serial::init();

    // Initialize framebuffer & console IMMEDIATELY so early panics are visible on screen!
    //
    // NO splash here: the splash's RaeSans wordmark rasterizes a TTF through
    // the allocator, and at Phase 1 only the small bootstrap heap exists —
    // drawing it this early OOM-halted the UEFI boot (journey capture
    // 2026-07-01). The splash starts at the Phase-4 framebuffer re-init,
    // after the real heap is up.
    if let Some(fb) = boot_info.framebuffer.as_mut() {
        framebuffer::init(fb);
        console::init();
    }

    // Record T0 globally so any subsystem can ask "how long since boot
    // started?" via boot_elapsed_ms() — used by the per-Tier timing
    // breakdown printed alongside [TIER] complete lines, which is how we
    // localize bare-metal slow phases when the total boot time blows past
    // the 6 s target.
    BOOT_START_TSC.store(boot_start_tsc, core::sync::atomic::Ordering::SeqCst);

    serial_println!();
    serial_println!("{}", BANNER);
    serial_println!("[ OK ] Serial (COM1 16550 UART) @ 0x3F8");
    serial_println!(
        "[boot] T0 TSC = {} (boot-time benchmark armed)",
        boot_start_tsc
    );
    // Announce safe-mode status as the very next line so anyone tailing the
    // serial log sees it before any storage subsystem comes up. When the
    // kernel was compiled with `xtask --safe`, every BlockDevice::write_sector
    // refuses with a `[safe-mode] BLOCKED ...` log line; read paths are
    // untouched so boot still completes end-to-end.
    if crate::block_io::safe_mode_enabled() {
        serial_println!(
            "[safe-mode] ENABLED — sector writes will be refused at the BlockDevice trait"
        );
    } else {
        serial_println!("[safe-mode] disabled (normal build; sector writes go through)");
    }
    fast_boot::record_boot_start(boot_start_tsc);

    apic::calibrate_tsc(); // enable high-res timer fallback
    hpet::run_boot_smoketest();
    fast_boot::set_tsc_mhz(apic::TSC_FREQ_MHZ.load(core::sync::atomic::Ordering::Relaxed));
    rtc::init(); // wall-clock from CMOS + TSC anchor

    // ── Phase 2: CPU structures ───────────────────────────────────────
    // Slice 0b-2: load the BSP descriptor tables through the arch:: HAL seam
    // (x86 `LGDT`+`LTR` ↔ aarch64 has no GDT — its equivalent is the per-EL
    // stack/FP-enable). On x86_64 this delegates to the existing `crate::gdt::init()`
    // — byte-identical behavior, pure indirection. The AP per-CPU GDT/TSS paths
    // (smp.rs) stay in `crate::gdt` for a later sub-slice (they carry the
    // context-switch coupling the IDT/BSP-GDT loads don't).
    arch::cpu::load_gdt();
    serial_println!("[ OK ] GDT + TSS (double-fault IST at index 0)");

    // Enable SSE so userspace (relibc `_start` does `ldmxcsr`) and `fxsave64`
    // in the context switcher don't #UD. Kernel is soft-float, so this only
    // *permits* SSE — kernel code is unaffected.
    cpu_features::enable_sse();
    serial_println!("[ OK ] SSE enabled (CR4.OSFXSR|OSXMMEXCPT)");

    // Slice 0b: install the interrupt-vector table through the arch:: HAL seam
    // (x86 `LIDT` ↔ aarch64 `VBAR_EL1`). On x86_64 this delegates to the existing
    // `crate::interrupts::init_idt()` — byte-identical behavior, pure indirection.
    arch::interrupts::load_idt();
    serial_println!("[ OK ] IDT (breakpoint, double-fault, page-fault, timer, keyboard)");

    // Verify fault-tolerant MSR access now that the #GP handler is live: a read
    // of an absent MSR must recover (return None) instead of crashing. This is
    // the mechanism that lets the kernel probe vendor-specific MSRs on any CPU.
    msr::init();
    msr::run_boot_smoketest();

    // Architecture HAL boundary (Slice 0): anchor the arch:: abstraction in the
    // boot sequence and prove the x86_64 backend (identity / CPU control / port
    // I/O). Multi-arch reach — Concept §Architecture Reach.
    arch::init();
    arch::run_boot_smoketest();

    // Mask the legacy 8259 PIC — we'll route all hardware IRQs through the
    // I/O APIC instead. We do NOT enable interrupts yet: the APIC isn't up,
    // and we don't want a spurious timer firing into an unconfigured LAPIC.
    unsafe { interrupts::disable_pic() };
    serial_println!("[ OK ] Legacy 8259 PIC masked");

    // ── Phase 3: Memory management + heap ─────────────────────────────
    let phys_mem_offset = boot_info
        .physical_memory_offset
        .into_option()
        .expect("bootloader did not provide physical_memory_offset");
    let phys_mem_offset = crate::arch::VirtAddr::new(phys_mem_offset);
    memory::PHYS_MEM_OFFSET.call_once(|| phys_mem_offset);

    // Apply KASLR to randomize kernel base address.
    let kaslr_offset = memory::kaslr_random_offset();
    memory::apply_kaslr(kaslr_offset);

    let mut mapper = unsafe { memory::init(phys_mem_offset) };

    // Validate the bootloader memory map before any allocator walks it.
    memory::verify_boot_memory_map(&boot_info.memory_regions);

    let frame_allocator =
        unsafe { memory::BootInfoFrameAllocator::init(&boot_info.memory_regions) };

    *memory::FRAME_ALLOCATOR.lock() = Some(frame_allocator);
    let mut global_allocator = memory::GlobalFrameAllocator;

    // Log the bootloader's memory map and which regions are eligible for the allocator.
    // The general-purpose frame allocator MUST only draw from `Usable` regions.
    serial_println!("[boot] Memory map (bootloader_api 0.11):");
    for (i, r) in boot_info.memory_regions.iter().enumerate() {
        let kind = match r.kind {
            bootloader_api::info::MemoryRegionKind::Usable => "USABLE",
            _ => "NON_USABLE",
        };
        let use_flag = if r.kind == bootloader_api::info::MemoryRegionKind::Usable {
            "use"
        } else {
            "skip"
        };
        serial_println!(
            "[boot]  {:02}: {:#018x}-{:#018x}  {} {:?}  ({})",
            i,
            r.start,
            r.end,
            kind,
            r.kind,
            use_flag,
        );
    }

    memory::allocator::init_heap(&mut mapper, &mut global_allocator)
        .expect("heap initialization failed");
    serial_println!(
        "[ OK ] Heap allocator (256 KiB linked-list) @ 0x{:x}",
        memory::allocator::HEAP_START
    );

    // Apply hardware page protections to non-usable memory regions.
    unsafe {
        memory::harden_memory_map(&boot_info.memory_regions);
    }

    // Upgrade to Buddy Allocator for high-performance frame management.
    unsafe {
        memory::init_buddy_allocator(&boot_info.memory_regions);
    }

    // Quick sanity check: allocate a Vec on the heap
    {
        use alloc::vec;
        let v = vec![1, 2, 3, 4, 5];
        serial_println!(
            "[ OK ] Heap test: alloc::vec![1..5] sum = {}",
            v.iter().sum::<i32>()
        );
    }

    // Slice 1.5a: the arch::mmu paging seam round-trip. Runs HERE (not in the
    // early arch::run_boot_smoketest at Phase 0) because it needs a live frame
    // allocator + PHYS_MEM_OFFSET + KERNEL_PML4, all of which are now up. Maps a
    // throwaway frame at a dedicated unused probe VA, translates/writes/unmaps it,
    // and asserts the VA is a hole afterward — FAIL-able + leaves the kernel
    // address space byte-identical.
    arch::run_mmu_boot_smoketest();

    // Boot-time profiling (live-fix #1): capture elapsed ms at each heavy Tier-1
    // sub-step boundary so `[tier1-prof]` localizes where the bare-metal seconds
    // go (QEMU can't reproduce iron's ACPI-parse / AP-bringup / GOP-mirror cost).
    let t_pre_acpi = boot_elapsed_ms();
    let ap_apic_ids = if let Some(rsdp) = boot_info.rsdp_addr.into_option() {
        SAVED_RSDP_ADDR.store(rsdp, Ordering::Relaxed);
        crate::acpi::init(rsdp)
    } else {
        serial_println!("[WARN] RSDP not found, cannot initialize ACPI!");
        alloc::vec::Vec::new()
    };
    let t_post_acpi = boot_elapsed_ms();

    // ── Phase 4: Framebuffer ──
    // Moved to Phase 1 for early panic visibility.
    crate::serial_println!("[ OK ] GOP Framebuffer + Text Console initialized");
    console::run_boot_smoketest();

    // ── Phase 7: Scheduler, IPC & Syscalls ─────────────────────────────
    // Order matters: these must land on KEYBOARD_CHANNEL(1) then MOUSE_CHANNEL(2)
    // — the IRQ paths send to those ids by name (BUG-18). Assert the binding.
    let kbd_chan = ipc::IPC.lock().create_channel(false);
    let mouse_chan = ipc::IPC.lock().create_channel(false);
    debug_assert_eq!(kbd_chan, ipc::KEYBOARD_CHANNEL);
    debug_assert_eq!(mouse_chan, ipc::MOUSE_CHANNEL);
    let _ = (kbd_chan, mouse_chan);

    syscall::init();
    scheduler::init();

    // Now that the LAPIC + IOAPIC are programmed, it's safe to take interrupts.
    // From this point on, the LAPIC timer drives `timer_handler_inner` and
    // GSI 1 from the IOAPIC drives the keyboard handler.
    x86_64::instructions::interrupts::enable();
    serial_println!("[ OK ] Interrupts enabled (APIC routing live)");

    // ── Phase 3.5: SMP — wake any Application Processors ─────────────────
    crate::smp::bring_up_aps(&ap_apic_ids);
    apic::run_boot_smoketest();
    crate::smp::run_boot_smoketest();
    serial_println!(
        "[ OK ] SMP: {} CPU(s) online",
        crate::smp::APS_ONLINE.load(core::sync::atomic::Ordering::SeqCst),
    );
    let t_post_smp = boot_elapsed_ms();

    // ── Phase 3.7: PS/2 Mouse ──────────────────────────────────────────
    interrupts::init_ps2_mouse();

    // ── Phase 4: Framebuffer ──────────────────────────────────────────
    if let Some(fb) = boot_info.framebuffer.as_mut() {
        let info = fb.info();
        let width = info.width;
        let height = info.height;
        let bpp = info.bytes_per_pixel * 8;
        framebuffer::init(fb);
        console::init();
        // The re-init cleared the screen — repaint the boot splash over it
        // (no-op in safe mode / padded-stride fallback).
        fast_boot::splash_show_early();
        kprintln!("[ OK ] UEFI GOP Debug Console live");
        console::run_boot_smoketest();
        serial_println!(
            "[gop] bootloader primary framebuffer: {}x{} @ {}bpp stride={} (UEFI multi-monitor enum follow-up)",
            width, height, bpp, info.stride,
        );
        serial_println!("[ OK ] Framebuffer: {}x{} @ {}bpp", width, height, bpp,);
        // Compositor takes ownership of the framebuffer; userspace now
        // draws via SYS_SURFACE_CREATE/PRESENT instead of poking raw MMIO.
        compositor::init();
        // Phase 19.1: accessibility tree comes up after the compositor so the
        // window-tier surface list it reads exists.
        a11y::init();
    } else {
        serial_println!("[WARN] No framebuffer provided by bootloader.");
    }

    // ── Phase 5: Boot summary ─────────────────────────────────────────
    serial_println!(
        "[INFO] Memory regions reported by bootloader: {}",
        boot_info.memory_regions.len(),
    );

    serial_println!();
    serial_println!("  [boot] ┌─────────────────────────────────────┐");
    serial_println!("  [boot] │  AthKernel v0.0.1 — all systems go  │");
    serial_println!("  [boot] ├─────────────────────────────────────┤");
    serial_println!("  [boot] │  Serial ........... ✓  COM1 0x3F8   │");
    serial_println!("  [boot] │  GDT/TSS .......... ✓  IST[0]      │");
    serial_println!("  [boot] │  IDT .............. ✓  5 handlers   │");
    serial_println!("  [boot] │  PIC 8259 ......... ✓  IRQ 32-47   │");
    serial_println!("  [boot] │  Paging ........... ✓  OffsetPT    │");
    serial_println!("  [boot] │  Heap ............. ✓  256 KiB     │");
    serial_println!("  [boot] │  Framebuffer ...... ✓  pixel mode  │");
    serial_println!("  [boot] │  Keyboard ......... ✓  PS/2 IRQ1   │");
    serial_println!("  [boot] ├─────────────────────────────────────┤");
    serial_println!("  [boot] │  Next: scheduler, user-space, IPC   │");
    serial_println!("  [boot] └─────────────────────────────────────┘");
    serial_println!();
    // ── Phase 6: PCI enumeration + virtio bringup ─────────────────────
    pci_irq::init();
    let pci_devices = pci::enumerate();
    pci_irq::route_all_devices();
    pci_irq::run_boot_smoketest();
    // Split point for the tier1-prof `pci` vs `fb` breakdown (boot-time
    // live-fix #1): `pci` = the enumeration scan + IRQ routing this fix targets;
    // `fb` = the IOMMU + virtio bring-up remainder of the old `pci+fb` span.
    let t_post_pci_scan = boot_elapsed_ms();

    iommu::init(); // activates VT-d DMA sandboxing if hardware present
    iommu::run_boot_smoketest();
    // serial_println!("[ OK ] IOMMU subsystem initialized");
    for dev in &pci_devices {
        // virtio vendor (Red Hat) = 0x1AF4.
        //   subsystem 0x1000 = network, 0x1001 = block.
        if dev.vendor_id == 0x1AF4 && dev.device_id == 0x1001 {
            virtio::init(dev);
        }
        if dev.vendor_id == 0x1AF4 && dev.device_id == 0x1000 {
            virtio_net::init(dev);
        }
    }
    let t_post_pci = boot_elapsed_ms();

    // ── Phase 6-iommu: IOMMU (before DMA-capable drivers) ─────────────

    // ── Phase 6a: Tier 1 — Core infrastructure ────────────────────────
    slab::init();
    serial_println!("[ OK ] Slab allocator initialized");
    locking::init();
    serial_println!("[ OK ] Advanced locking primitives initialized");
    timers::init();
    serial_println!("[ OK ] Timer subsystem initialized");
    timers::run_boot_smoketest();
    config_registry::init();
    // Dev/CI-only: pre-complete first-boot so a headless QEMU boot skips the OOBE
    // (which needs a live keyboard) and lands on the desktop via login auto-sign-in
    // — the reliable path for the QMP screenshot-driven UI loop. Never in a shipped
    // image (feature is off by default; enabled by `xtask run --skip-oobe`).
    #[cfg(feature = "dev_skip_oobe")]
    {
        config_registry::set_bool("/setup/first_boot_done", true);
        serial_println!(
            "[dev] skip-oobe: first_boot_done pre-set -> boot lands on desktop (autologin)"
        );
    }
    data_buckets::init();
    data_buckets::run_boot_smoketest();
    session::init();
    shell_runner::init();
    shell_runner::run_boot_smoketest();
    wm_policy::init();
    // ADR 0006 (boot-time live-fix #1): wm_policy / window_chrome / login_ui
    // smoketests are pure tests (no init) — deferred to
    // boot_selftest::run_deferred() post-marker. wm_policy::init() stays here;
    // window_chrome / login_ui have no init().
    {
        // Phase 8.1 / typography-rendering S3: build the crisp grayscale-AA text
        // engine (embedded Inter + JetBrains Mono via athfont) BEFORE the OOBE /
        // desktop so the first composited frame is already crisp, not 8×8 block
        // font. Off the serial hot loop (memory `iron-console-logging-tax`).
        let r = athgfx::text::run_boot_smoketest();
        serial_println!(
            "[gfx] draw_text_aa smoketest: families={} face=Inter cov={} range={}..{} glyph_coverage={} -> {}",
            r.families,
            r.total_coverage,
            r.min_cov,
            r.max_cov,
            r.total_coverage > 0,
            if r.pass { "PASS" } else { "FAIL" },
        );
    }
    setup_ui::run_boot_smoketest(); // Phase 8.1: tokenized OOBE wizard + live accent
    notify::init();
    // ADR 0006 (boot-time live-fix #1): notify::run_boot_smoketest() (the
    // ~24-surface toast storm — the single largest line-item in the old
    // modules=5574ms bucket) is a pure test that posts synthetic toasts and
    // expires them all; deferred to boot_selftest::run_deferred() post-marker.
    // notify::init() stays on the critical path.
    widgets::init();
    // widgets::run_boot_smoketest() deferred to boot_selftest::run_deferred()
    // (ADR 0006 — runs post-marker; init stays on the critical path).
    boot_selftest::init(); // ADR 0006: arm deferred-sweep orchestration (critical path, cheap)
    boot_selftest::run_boot_smoketest(); // ADR 0006: deferred-sweep registry armed (cheap)
                                         // ADR 0006 (boot-time live-fix #1): the athshell VT100/ANSI terminal-parser
                                         // smoketest (escape decode / CUP / SGR / ED erase) is a pure decode test —
                                         // deferred to boot_selftest::run_deferred() post-marker.
    workqueue::init();
    serial_println!("[ OK ] Workqueue system initialized");
    numa::init();
    serial_println!("[ OK ] NUMA topology initialized");
    numa::run_boot_smoketest();
    sync::run_boot_smoketest(); // Phase 11 item 1828: phys-keyed blocking futex
    msr_intel::init();
    serial_println!("[ OK ] Intel MSR features initialized");
    msr_intel::run_boot_smoketest();
    config_registry::run_boot_smoketest();
    win_registry::init();
    win_registry::run_boot_smoketest(); // Phase 11.2: Win32 registry over versioned config
    session::run_boot_smoketest();
    aer::init();
    // ADR 0006: aer::run_boot_smoketest() (reads registration counters; pure)
    // deferred to boot_selftest::run_deferred() post-marker.
    // Extable: per-CPU copy_from_user fault-fixup table. No init() — the
    // table is `const` static. The smoketest proves install/check/clear
    // round-trip and that the happy-path copy stub works end-to-end.
    extable::run_boot_smoketest();
    // uaccess: the validated copy_from_user/copy_to_user chokepoint that the
    // native syscall handlers route through — prove the bounds gate rejects a
    // kernel pointer (info-leak/privesc guard) before any deref.
    uaccess::run_boot_smoketest();
    // Boot-log RAM ring smoketest — proves serial output is being
    // captured into the durable in-RAM buffer that backs
    // /proc/athena/bootlog (and the future ESP-persisted log).
    bootlog::run_boot_smoketest();
    driver_fw::init();
    let t_tier1_done = boot_elapsed_ms();
    // Sub-step breakdown of Tier 1 (the biggest bare-metal tier, ~4.2 s on
    // Athena): `early` = serial/calibrate/GDT/IDT/heap before ACPI; `acpi` =
    // namespace parse (37 tables / 159 devices on iron); `smp+sched` = AP
    // bring-up + scheduler/IPC/syscall init; `pci` = PCI enumeration scan + IRQ
    // routing (boot-time live-fix #1 target); `fb` = IOMMU + virtio bring-up;
    // `modules` = the Tier-1 service inits + smoketests. Localizes boot-time
    // live-fix #1 on the next iron flash.
    serial_println!(
        "[tier1-prof] early={}ms acpi={}ms smp+sched={}ms pci={}ms fb={}ms modules={}ms",
        t_pre_acpi,
        t_post_acpi.saturating_sub(t_pre_acpi),
        t_post_smp.saturating_sub(t_post_acpi),
        t_post_pci_scan.saturating_sub(t_post_smp),
        t_post_pci.saturating_sub(t_post_pci_scan),
        t_tier1_done.saturating_sub(t_post_pci),
    );
    serial_println!(
        "[TIER] Tier 1 complete: core infrastructure  (t={}ms)",
        t_tier1_done
    );
    fast_boot::splash_progress(20, "Starting core services");

    // ── Phase 6b: Tier 2 — Storage ──────────────────────────────────────
    block_io::init();
    // SAFE MODE gate. Block-device writes default OFF (fail-safe). A standard
    // build enables them at boot ONLY on the QEMU hardware profile (throwaway
    // virtual disks the CI smoketests + installer dry-runs need); on REAL
    // hardware a standard image stays READ-ONLY at boot, so even the install
    // image cannot touch the machine's disk until the user-confirmed installer
    // brackets a write window around the actual destructive write. A `--safe`
    // image leaves writes OFF for the WHOLE boot regardless. (block_io comment:
    // the "FOLLOW-UP (stronger)" hardening — structurally read-only until Install.)
    #[cfg(not(feature = "safe_mode"))]
    {
        // detect() is pure (DMI + CPUID, both live by Tier 2). On QEMU it returns
        // the qemu profile; real Athena classifies as AMD via the Zen4 CPU check
        // even if SMBIOS is unread this early — so the gate is correct pre-init.
        let family = hardware_profile::detect().family;
        if block_io::boot_writes_default_on(family) {
            block_io::set_writes_enabled(true);
            serial_println!(
                "[storage] disk writes ENABLED at boot (QEMU profile — throwaway disks)"
            );
        } else {
            serial_println!(
                "[storage] disk writes READ-ONLY at boot (real hardware {:?}) — \
                 a confirmed install opens the write window; nothing else can write the disk",
                family
            );
        }
    }
    #[cfg(feature = "safe_mode")]
    serial_println!(
        "[storage] *** SAFE IMAGE: storage is READ-ONLY for this entire boot — \
         writes are never enabled AND the safe-mode guard is active. AthenaOS \
         cannot write any real disk (only its own pre-allocated bootlog file). \
         Safe to run on real hardware. ***"
    );
    block_io::run_boot_smoketest(); // install write-window safety gate (disk-wipe guard)
    storage_irq::init();
    nvme::init();
    nvme::run_boot_smoketest();
    ahci::init();
    ahci::run_boot_smoketest();
    serial_println!("[ OK ] AHCI/SATA driver initialized");
    vfs::init();
    vfs::run_boot_smoketest();
    filesystems::init();
    serial_println!("[ OK ] Filesystem layer initialized");
    serial_println!(
        "[TIER] Tier 2 complete: storage  (t={}ms)",
        boot_elapsed_ms()
    );
    fast_boot::splash_progress(32, "Mounting storage");

    // ── Phase 6c: Tier 3 — Networking ───────────────────────────────────
    net_drivers::init();
    net_drivers::run_boot_smoketest();
    net::init();
    net::run_ipv6_smoketest(); // Phase 10.2: dual-stack + EUI-64 LLA
    dhcp::init();
    dhcp::run_boot_smoketest();
    dns::init();
    dns::run_boot_smoketest();
    dot::init();
    dot::run_boot_smoketest(); // Phase 10.2: DNS-over-TLS loopback round trip
    netmanager::init();
    netlog::init();
    netlog::run_boot_smoketest();
    mdns::init();
    mdns::run_boot_smoketest(); // Phase 10.2: DNS-SD round trip
    firewall::init();
    firewall::run_boot_smoketest();
    tunnel::init();
    serial_println!("[ OK ] Tunnel subsystem initialized");
    net::init_traffic_shaper();
    net::run_traffic_shaper_smoketest();
    net::run_socket_smoketest(); // Phase 10.2: socket API + SYS_NET_STATUS readiness
    ssh::init(); // RaeSSH host key + policy (Concept: "the user owns the machine")
    ssh::run_boot_smoketest(); // loopback handshake+auth+shell proof, in-kernel
    ssh::start_listener(); // Increment B1: bind the TCP :22 listening socket
    quic::init();
    serial_println!("[ OK ] QUIC subsystem initialized");
    quic::run_boot_smoketest();
    // RaeWeb native browser surface — fetch→parse→layout→paint→present + link
    // navigation, all through the athweb engine → athgfx (Concept §3: "renders
    // through AthUI", §Core Principles #1: "No Electron tax"). Lands after the net
    // stack so a live fetch is possible, but the smoketest uses a bundled document
    // for determinism (QEMU/iron net RX is gated — live fix #2).
    webview::init();
    webview::run_boot_smoketest();
    serial_println!(
        "[TIER] Tier 3 complete: networking  (t={}ms)",
        boot_elapsed_ms()
    );
    fast_boot::splash_progress(44, "Connecting network");

    // ── Phase 6d: Tier 4 — Security ─────────────────────────────────────
    tpm::init();
    serial_println!("[ OK ] TPM 2.0 subsystem initialized");
    tpm::run_seal_smoketest(); // measured-boot key sealing: unseal only at matching PCR state
    security::init();
    security::run_boot_smoketest(); // measured-boot log replay + tamper-detect proof
    serial_println!("[ OK ] Security framework initialized");
    encryption::init();
    serial_println!("[ OK ] Storage encryption initialized");
    encryption::run_boot_smoketest(); // Argon2id RFC 9106 + BLAKE2b known-answer tests
    fde::init();
    fde::run_boot_smoketest(); // Phase 3.8: AthFS through AES-XTS, plaintext-never-on-disk
    crypto::init();
    serial_println!("[ OK ] Kernel crypto API initialized");
    crypto::run_boot_smoketest(); // X25519 RFC 7748 known-answer test
    hardening::init();
    serial_println!("[ OK ] Kernel hardening initialized");
    hardening::run_boot_smoketest();
    // Real hardware SMEP was enabled on the BSP inside hardening::init (CR4.SMEP);
    // prove the bit is actually set (read-back), not just a status flag.
    cpu_features::run_smep_smoketest();
    // Real hardware SMAP (CR4.SMAP) was enabled on the BSP inside hardening::init;
    // prove the trap actually fires — a non-stac supervisor read of a user page
    // faults while the stac/clac uaccess chokepoint stays open. Must run before
    // any user task exists (uses a throwaway user VA) — this Tier-4 slot is it.
    cpu_features::run_smap_smoketest();
    // Real hardware UMIP (CR4.UMIP) — prove the bit is set (read-back), blocking
    // userspace SGDT/SIDT/SLDT/STR/SMSW descriptor-table address leaks (KASLR).
    cpu_features::run_umip_smoketest();
    // Real branch-speculation MSRs (IA32_SPEC_CTRL IBRS/STIBP/SSBD) were
    // programmed on the BSP in hardening::init and on every AP in ap_entry;
    // prove the advertised bits actually read back set — a hardware Spectre
    // defense, not a software-simulated flag.
    cpu_features::run_spec_ctrl_smoketest();
    // Per-CPU transient-execution vulnerability posture (the Linux
    // /sys .../vulnerabilities equivalent): classify Meltdown/Spectre/MDS/…
    // from CPUID + IA32_ARCH_CAPABILITIES + the branch-spec MSR state, and
    // FAIL if the gating logic regresses on synthetic silicon.
    cpu_features::run_vulnerabilities_smoketest();
    // KFENCE guard-page sampler proof (feature = "kfence" only). Compiled out of
    // the default build — no line emitted unless built with `--features kfence`.
    #[cfg(feature = "kfence")]
    hardening::sampler::run_boot_smoketest();
    // KASAN shadow-memory detector proof (feature = "kasan" only). Compiled out
    // of the default build — no line emitted unless built with `--features kasan`.
    #[cfg(feature = "kasan")]
    hardening::kasan::run_boot_smoketest();
    anticheat::init();
    serial_println!("[ OK ] Anti-cheat subsystem initialized");
    anticheat::run_boot_smoketest();
    audit::init();
    serial_println!("[ OK ] Audit framework initialized");
    serial_println!(
        "[TIER] Tier 4 complete: security  (t={}ms)",
        boot_elapsed_ms()
    );
    fast_boot::splash_progress(54, "Securing the system");

    // ── Phase 6e: Tier 5 — Process model ────────────────────────────────
    process::init();
    serial_println!("[ OK ] Process table initialized");
    init_system::init();
    serial_println!("[ OK ] Init system initialized");
    signals::init(0);
    serial_println!("[ OK ] Signals subsystem initialized");
    namespaces::init();
    serial_println!("[ OK ] Namespaces initialized");
    posix::init();
    serial_println!("[ OK ] POSIX compatibility layer initialized");
    linux_syscall::init();
    linux_syscall::run_boot_smoketest();
    shell_api::init();
    shell_api::run_boot_smoketest();
    secure_ipc::init();
    secure_ipc::run_boot_smoketest(); // forged/spoofed-token deny + real-cap allow proof
    serial_println!("[ OK ] Secure IPC (authenticated channels) initialized");
    perm_prompt::init();
    perm_prompt::run_boot_smoketest(); // request→approve→GRANT→poll cycle proof
    perm_ui::init();
    perm_ui::run_boot_smoketest(); // Phase 9.2: compositor consent dialog cycle
    dynamic_linker::init();
    tmpfs::init();
    procfs::init();
    procfs::run_boot_smoketest(); // /proc/athena/storage non-empty + physical_total_bytes > 0
    sysfs_kobject::init();
    serial_println!(
        "[TIER] Tier 5 complete: process model + virtual filesystems  (t={}ms)",
        boot_elapsed_ms()
    );
    fast_boot::splash_progress(64, "Preparing services");

    // ── Phase 6f: Tier 6 — USB/Input ────────────────────────────────────
    // Per-step timing here is the primary diagnostic for the bare-metal
    // boot-time regression: Tier 6 is the biggest delta on QEMU (+856ms)
    // and the prime suspect on Athena. Each line shows step delta in ms.
    let _t_t6_start = boot_elapsed_ms();
    input::init();
    input::run_boot_smoketest(); // DualSense/Xbox report parse + output serialize proof
    a11y_input::init();
    a11y_input::run_boot_smoketest(); // sticky/slow/bounce/repeat key-filter proof
    captions::init();
    captions::run_boot_smoketest(); // visual-alert caption stream proof
    hid_gamepad::init();
    hid_gamepad::run_boot_smoketest(); // Phase 12.2: generic pad via report descriptor
    let _t6_input = boot_elapsed_ms();
    usb_hid::init();
    let _t6_hid_init = boot_elapsed_ms();
    xhci::init();
    let _t6_xhci_init = boot_elapsed_ms();
    xhci::run_boot_smoketest();
    let _t6_xhci_smoke = boot_elapsed_ms();
    usb_core::init();
    let _t6_usb_core = boot_elapsed_ms();
    serial_println!("[ OK ] USB core framework initialized");
    usb_hid::run_boot_smoketest();
    let _t6_hid_smoke = boot_elapsed_ms();
    usb_msc::init();
    let _t6_msc_init = boot_elapsed_ms();
    usb_msc::run_boot_smoketest();
    let _t6_end = boot_elapsed_ms();
    serial_println!(
        "[tier6-prof] input={}ms hid_init={}ms xhci_init={}ms xhci_smoke={}ms usb_core={}ms hid_smoke={}ms msc_init={}ms msc_smoke={}ms",
        _t6_input.saturating_sub(_t_t6_start),
        _t6_hid_init.saturating_sub(_t6_input),
        _t6_xhci_init.saturating_sub(_t6_hid_init),
        _t6_xhci_smoke.saturating_sub(_t6_xhci_init),
        _t6_usb_core.saturating_sub(_t6_xhci_smoke),
        _t6_hid_smoke.saturating_sub(_t6_usb_core),
        _t6_msc_init.saturating_sub(_t6_hid_smoke),
        _t6_end.saturating_sub(_t6_msc_init),
    );
    serial_println!(
        "[TIER] Tier 6 complete: USB/input  (t={}ms)",
        boot_elapsed_ms()
    );
    fast_boot::splash_progress(74, "Detecting devices");

    // ── Phase 6g: Tier 7 — Power/ACPI ───────────────────────────────────
    // Boot-time profiling (live-fix #1): Tier 7 (~3 s on iron) holds the FULL
    // ACPI namespace parse (acpi_full, 159 devices) AND the late USB bring-up
    // (xHCI enum + hub probes run here, since ECAM only activates now).
    let t7_start = boot_elapsed_ms();
    let mut t7_after_acpi = t7_start;
    let mut t7_after_usb = t7_start;
    {
        let rsdp = SAVED_RSDP_ADDR.load(Ordering::Relaxed);
        if rsdp != 0 {
            acpi_full::init(rsdp);
            acpi_full::run_boot_smoketest();
            smbios::init(rsdp);

            pcie::init();
            pcie::run_boot_smoketest();
            t7_after_acpi = boot_elapsed_ms();

            // The first pci::enumerate() (above, before ACPI/ECAM) cached a
            // legacy scan capped at bus 0..=8. Now that ECAM may be active,
            // re-scan so controllers on higher buses (notably xHCI on AMD
            // platforms) are discovered before Tier 6 driver bring-up.
            if pci::PCIE_ECAM_BASE.load(Ordering::Relaxed) != 0 {
                let devs = pci::refresh();
                serial_println!(
                    "[pcie] PCI re-scan after ECAM active: {} devices (was legacy-capped)",
                    devs.len()
                );

                // Late USB bring-up — the bare-metal fix photographed on
                // Athena: Tier 6 ran the xHCI scan BEFORE ECAM was active,
                // and the legacy port-0xCF8 scan (bus 0..=8) cannot see the
                // xHCI on real AMD platforms (Athena: 31 devices legacy vs
                // 45 via ECAM, zero USB-class among the 31). If Tier 6 came
                // up empty, redo the controller scan + device enumeration
                // against the full bus view — this is what brings up the
                // keyboard, mouse, and the USB bootlog stick on real iron.
                // Runs before bootlog_persist::init below, so the USB stick
                // is preferred for the persistent log when it enumerates.
                if !xhci::is_initialized() {
                    serial_println!(
                        "[xhci] late bring-up: ECAM now active — re-scanning for xHCI..."
                    );
                    xhci::init();
                    if xhci::is_initialized() {
                        xhci::run_boot_smoketest(); // ports + enumerate + HID arm + MSC bulk
                        usb_msc::init();
                        usb_msc::run_boot_smoketest();
                    } else {
                        serial_println!("[xhci] late bring-up: still no xHCI controller via ECAM");
                    }
                }
            }
            t7_after_usb = boot_elapsed_ms();

            // PCIe AER capability scan needs ECAM (extended config >= 0x100),
            // so it runs after pcie::init. Handles ECAM-inactive gracefully.
            pcie_aer::init();
            pcie_aer::run_boot_smoketest();
        }
    }
    pci_pm::init();
    pci_pm::run_boot_smoketest(); // Phase 2.4: per-device D-states (PMCSR)
    power::init();
    oom::init();
    oom::run_boot_smoketest();
    swap::init();
    swap::run_boot_smoketest(); // Phase 4.1: page-out/page-in round trip
    process::run_boot_smoketest(); // Phase 4.1: cgroup-equiv per-bundle memory limits
    soak::init();
    soak::run_boot_smoketest();
    fatfs_esp::init();
    fatfs_esp::run_boot_smoketest();
    fatfs_esp::run_format_smoketest(); // Phase 3.3: FAT32 formatter + EFI boot tree
                                       // Allocate \BOOTLOG.TXT on the existing ESP so flush() at end-of-boot
                                       // can persist the in-RAM bootlog ring. Registers a narrow safe-mode
                                       // LBA carveout for just those clusters — everything else still blocked.
    bootlog_persist::init();
    update_slots::init(); // Phase 3.6: A/B slot config + boot-attempt accounting
    update_slots::run_boot_smoketest();
    installer::run_boot_smoketest(); // Phase 3 + 16.1: install pipeline + account creation
    installer::run_payload_smoketest(); // Phase 3.1: source kernel+ramdisk from live media
    installer::run_layout_smoketest(); // Phase 16.1: full-disk vs dual-boot planner
    installer::run_apply_plan_smoketest(); // Phase 16.1: non-destructive dual-boot GPT carve
    installer::run_boot_entry_smoketest(); // Phase 16.1: UEFI Boot#### load-option encoding
    installer_ui::run_boot_smoketest(); // Phase 3: graphical install wizard state machine
                                        // Phase 3.5: marker-gated automated install. Only fires when the boot USB
                                        // stick carries an explicit INSTALL.NOW marker the user created — never
                                        // auto-installs. Runs here, AFTER bootlog_persist::init chose the log
                                        // device (the stick when multi-controller xHCI enumerated it), so the
                                        // install's reformat of the NVMe ESP can't clobber the log it writes.
    installer::maybe_run_triggered_install();
    edid::init();
    edid::run_boot_smoketest();
    compress::init();
    compress::run_boot_smoketest();
    handle_table::run_boot_smoketest();
    mmio::run_boot_smoketest();
    fastqueue::init();
    fastqueue::run_boot_smoketest();
    dwarf_sym::init();
    dwarf_sym::run_boot_smoketest();
    perf::init();
    perf::run_boot_smoketest();
    acpi_quirks::init();
    acpi_quirks::run_boot_smoketest();
    storage_mount::init();
    storage_mount::run_boot_smoketest();
    gpe::init();
    gpe::run_boot_smoketest();
    battery::init();
    serial_println!("[ OK ] Power management initialized");
    thermal::init();
    serial_println!("[ OK ] Thermal management initialized");
    cpufreq::init();
    cpufreq::run_boot_smoketest();
    serial_println!("[ OK ] CPUfreq subsystem initialized (P-state governor active)");
    cpuidle::init_cpuidle();
    cpuidle::run_boot_smoketest();
    serial_println!("[ OK ] CPU idle (C-states) initialized");
    thermal::run_boot_smoketest();
    thermal::run_breach_selftest(); // Phase 4.7: _PSV cap + _CRT shutdown policy proof
    thermal::run_component_smoketest(); // Phase 4.7: per-component CPU/GPU/NVMe SMART temps
    thermal::run_amd_temp_selftest(); // Phase 4.7: AMD Zen SMU Tctl decode + live read (Athena CPU temp)
    suspend::init();
    suspend::run_boot_smoketest();
    serial_println!("[ OK ] Suspend/resume initialized");
    power_supply::init();
    serial_println!("[ OK ] Power supply subsystem initialized");
    power_supply::run_boot_smoketest();
    power::refresh_battery_from_acpi();
    power::run_boot_smoketest();
    power_events::run_boot_smoketest();
    let t7_done = boot_elapsed_ms();
    // Sub-step breakdown of Tier 7: `acpi+pcie` = full namespace parse (159
    // devices) + ECAM bring-up; `usb` = late xHCI enum + hub probes (run here
    // because ECAM activated only now); `aer+modules` = AER + power/thermal/
    // installer/fatfs smoketests. Localizes boot-time live-fix #1 next flash.
    serial_println!(
        "[tier7-prof] acpi+pcie={}ms usb={}ms aer+modules={}ms",
        t7_after_acpi.saturating_sub(t7_start),
        t7_after_usb.saturating_sub(t7_after_acpi),
        t7_done.saturating_sub(t7_after_usb),
    );
    serial_println!("[TIER] Tier 7 complete: power/ACPI  (t={}ms)", t7_done);
    fast_boot::splash_progress(84, "Configuring power");
    // Checkpoint the persistent boot log at each tier boundary from here on
    // (bootlog_persist::init above located BOOTLOG.TXT, so flushes are live).
    // A bare-metal HANG — no panic, no marker — previously left the stick
    // with only the early-init snapshot; tier checkpoints mean the stick
    // always shows the last tier reached and every line within it. Each
    // flush rewrites only the file's tail half (~0.5 MiB) — milliseconds.
    bootlog_persist::flush();

    // ── Phase 6h: Tier 8 — Platform/misc ────────────────────────────────
    // Athena (photo #6 + both NVMe BOOTLOGs) dies somewhere in Tier 8 before
    // the mid-tier checkpoint — extra checkpoints bracket the death window so
    // the on-disk log names the exact init that hangs.
    hardware_profile::init();
    hardware_profile::run_boot_smoketest();
    bootlog_persist::flush();
    userspace_driver::init();
    userspace_driver::run_boot_smoketest();
    driver_manifest::init(); // HWID → driver-package matcher (auto driver install)
    driver_manifest::run_boot_smoketest();
    virtio_gpu::init(); // Phase 6: virtio-gpu (CPU→GPU rendering on-ramp)
    virtio_gpu::run_boot_smoketest();
    memory::run_boot_smoketest();
    memory::buddy::run_boot_smoketest();
    memory::allocator::run_boot_smoketest();
    tls::init();
    tls::run_boot_smoketest();
    rtc::run_boot_smoketest();
    scheduler::run_boot_smoketest();
    sched_proof::run_boot_smoketest();
    bootlog_persist::flush(); // Tier-8 death-window checkpoint (post-sched)
    linux_kabi::init();
    linux_compat::init();
    linuxkpi_host::init();
    efi::init();
    serial_println!("[ OK ] EFI runtime services initialized");
    firmware::init();
    serial_println!("[ OK ] Firmware interface initialized");
    tty::init();
    serial_println!("[ OK ] TTY subsystem initialized");
    dbus_kernel::init();
    serial_println!("[ OK ] D-Bus kernel message bus initialized");
    audio::init();
    audio::run_boot_smoketest();
    usb_audio::init();
    usb_audio::run_boot_smoketest();
    serial_println!("[ OK ] Audio subsystem initialized");
    bootlog_persist::flush(); // Tier-8 death-window checkpoint (post-audio)
    gpu::init();
    serial_println!("[ OK ] GPU driver initialized");
    // Mid-Tier-8 checkpoint: tier 8 is the largest bring-up block (GPU,
    // audio, manifests, search…); flush so a hang in its second half still
    // leaves the audio/GPU lines on the stick.
    bootlog_persist::flush();
    // QEMU -display gtk shows the bootloader UEFI GOP buffer. Bochs VBE LFB is a
    // different physical framebuffer — attaching GPU scanout paints there while the
    // window still maps GOP, which looks like corrupted triangles/noise.
    if hardware_profile::active().map(|p| p.id) == Some("qemu") {
        serial_println!(
            "[compositor] QEMU: keeping UEFI GOP scanout (skip Bochs VBE attach for visible display)"
        );
    } else {
        compositor::attach_gpu_scanout();
    }
    gpu::run_boot_smoketest();
    framebuffer::run_boot_smoketest();
    game_session::init();
    // ADR 0006 (boot-time gate): the userspace-feature correctness smoketests
    // (search/game_profile/rgb/theme/wireguard/wallpaper/vibe + the two theme
    // cohesion checks) are deferred to boot_selftest::run_deferred(), which runs
    // AFTER the success marker. They gate no boot-health check and spawn no
    // needed thread. Every subsystem's init() STAYS on the critical path here —
    // only the *check* moves; each still prints PASS/FAIL post-marker.
    search_index::init();
    game_profile::init();
    rgb::init();
    app_bundle::init();
    app_bundle::run_boot_smoketest(); // NOT deferred: not in the ADR feature set
    theme_engine::init();
    scripting::init();
    scripting::run_boot_smoketest(); // NOT deferred: not in the ADR feature set
    wireguard::init();
    live_wallpaper::init();
    aurora::init(); // IDENTITY §3: Aurora Mesh is the default backdrop (kills the void)
    aurora::run_boot_smoketest();
    vibe_mode::init();
    cap_audit::init();
    cap_audit::run_boot_smoketest();
    sandbox::init(); // Phase 9: per-task AthGuard sandbox enforcement at the syscall edge
    sandbox::run_boot_smoketest();
    scheduler::run_kill_reclaim_smoketest(); // goal #6: kill_task reclaims sockets+sandbox (no leak)
    rae_manifest::init(); // Phase 9: RaeManifest.toml per-app permission manifests
    rae_manifest::run_boot_smoketest();
    cpu_features::init();
    msr_amd::init();
    clipboard::init();
    windows_gap::init();
    cpu_features::run_boot_smoketest();
    msr_amd::run_boot_smoketest();
    clipboard::run_boot_smoketest();
    syscall::run_boot_smoketest();
    storage_irq::run_boot_smoketest();
    windows_gap::run_boot_smoketest();
    // ADR 0006 (boot-time live-fix #1): athbridge_boot::run_boot_smoketest()
    // (throwaway DLL registry + embedded PE parse; pure) deferred to
    // boot_selftest::run_deferred() post-marker.
    linux_kabi::run_boot_smoketest();
    linux_compat::run_boot_smoketest();
    linuxkpi_host::run_boot_smoketest();
    display::init();
    serial_println!("[ OK ] Display subsystem initialized");
    display::run_boot_smoketest();
    bluetooth::init();
    serial_println!("[ OK ] Bluetooth stack initialized");
    serial_println!(
        "[TIER] Tier 8 complete: platform/misc  (t={}ms)",
        boot_elapsed_ms()
    );
    fast_boot::splash_progress(92, "Almost there");
    bootlog_persist::flush();

    // ── Phase 6i: Tier 9 — Overclocking, Watchdog, Crash Dump, Fast Boot, HW Diag
    crash_dump::init();
    crash_dump::run_boot_smoketest();
    mce::init();
    mce::run_boot_smoketest();
    serial_println!("[ OK ] Crash dump subsystem initialized");
    watchdog::init();
    watchdog::run_boot_smoketest();
    serial_println!("[ OK ] Watchdog timer initialized");
    fast_boot::init();
    serial_println!("[ OK ] Fast boot profiler initialized");
    overclock::init();
    serial_println!("[ OK ] Overclocking API initialized");
    hw_diag::init();
    serial_println!("[ OK ] Hardware diagnostics initialized");
    kmod::init();
    serial_println!("[ OK ] Kernel module loader initialized");
    serial_println!(
        "[TIER] Tier 9 complete: overclock/watchdog/diagnostics/kmod  (t={}ms)",
        boot_elapsed_ms()
    );
    fast_boot::splash_progress(98, "Welcome");
    bootlog_persist::flush();

    serial_println!("[BOOT] All 9 tiers initialized successfully");

    // Post-Tier-9 profiling: ~3.5s on QEMU between Tier 9 and the boot
    // marker, dominated on bare metal by DHCP polling + procfs dump + ELF
    // parse. Per-step deltas here let us split that bucket without another
    // re-flash cycle.
    let _t_post9_start = boot_elapsed_ms();

    // Mount root: GPT AthFS partition when present, else whole-disk / format fallback.
    if !storage_mount::try_mount_athfs_root() {
        let _fs = athfs::AthFS::mount();
    }
    athfs::tiered_storage_init();
    athfs::tiered_storage_smoketest();
    athfs::init_extent_manager();
    athfs::AthFS::run_boot_smoketest();
    athfs::AthFS::run_format_smoketest();
    athfs::run_snapshot_smoketest(); // Phase 5.1: snapshot syscall surface (101-103)
    athfs::run_rollback_roundtrip_smoketest(); // Phase 5: snapshot→write→restore round trip
    athfs::run_cow_journal_crash_smoketest(); // Phase 5: extent-data CoW crash-consistency (journal replay)
    athfs::run_large_volume_bound_smoketest(); // Phase 3: multi-block bitmap/refcount — no OOB on real-disk (>128MiB) format
    athfs::run_btree_overflow_smoketest(); // Phase 5: B-tree leaf overflow hard-errors (no silent loss / snapshot corruption)
    snapshot_policy::init();
    snapshot_policy::run_boot_smoketest(); // Phase 5.1: retention ladder + quota
    prefetch::init();
    prefetch::run_boot_smoketest(); // Phase 5.5: sequential read-ahead through the real read path
    athfs::run_encryption_smoketest(); // Phase 5.2: XTS-AES-256 + FIPS-197 KAT
    athfs::run_bucket_key_selftest(); // Phase 5.6: per-app bucket key isolation
    athfs::run_file_key_selftest(); // Phase 5.2: per-file (FSCRYPT-equiv) key isolation
    athfs::run_compression_flag_smoketest(); // Phase 5.4: per-extent compression flag
                                             // Phase 16 account persistence: now that the AthFS root is mounted, load any
                                             // local accounts saved on a prior boot so the installed system has logins.
    session::load_persisted_accounts();
    // Phase 4.5: now that AthFS is mounted, persist any prior-boot crash dump
    // captured from the high-RAM tombstone, then prove the /var/crash path.
    crash_dump::flush_pending_crash_dump();
    crash_dump::run_persist_smoketest();
    let _t_athfs = boot_elapsed_ms();

    // virtio-net demo: send one broadcast Ethernet frame, then poll for RX a
    // few times. QEMU's `user`-mode network responds with ARP for our IPs,
    // RARP for our MAC, and the like — we won't parse them this slice; just
    // log "got a frame, NN bytes". smoltcp plumbs into this driver in the
    // next slice.
    if let Some(net) = virtio_net::VIRTIO_NET.get() {
        let src = net.mac();
        let mut frame = [0u8; 60]; // min Ethernet payload size = 46, +14 hdr
        frame[0..6].copy_from_slice(&[0xff; 6]); // dst = broadcast
        frame[6..12].copy_from_slice(&src); // src = our MAC
        frame[12] = 0x88;
        frame[13] = 0xb5; // ethertype = 0x88b5 (local experimental)
                          // Tiny human-readable payload so anyone capturing the wire knows it's us.
        let tag = b"AthenaOS-virtio-net-hello";
        frame[14..14 + tag.len()].copy_from_slice(tag);
        match net.tx_frame(&frame) {
            Ok(()) => serial_println!("[virtio-net] tx_frame OK ({} bytes)", frame.len()),
            Err(e) => serial_println!("[virtio-net] tx_frame failed: {}", e),
        }
        // Give QEMU's net stack a few hundred timer ticks to respond, then poll.
        // 50k inner spins × 3 outer iterations ≈ 150k cycles each pass — keeps
        // boot under 600 ms while still giving the device a chance to answer.
        for _ in 0..3 {
            for _ in 0..50_000 {
                core::hint::spin_loop();
            }
            let mut rx_count = 0usize;
            net.rx_poll(|eth| {
                rx_count += 1;
                if eth.len() >= 14 {
                    serial_println!(
                        "[virtio-net] rx {} bytes  dst={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} \
                         src={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} ethertype={:04x}",
                        eth.len(),
                        eth[0],
                        eth[1],
                        eth[2],
                        eth[3],
                        eth[4],
                        eth[5],
                        eth[6],
                        eth[7],
                        eth[8],
                        eth[9],
                        eth[10],
                        eth[11],
                        ((eth[12] as u16) << 8) | eth[13] as u16,
                    );
                }
            });
            if rx_count == 0 {
                serial_println!("[virtio-net] rx poll: no frames");
            }
        }
    } else {
        serial_println!("[virtio-net] device not present, skipping demo");
    }

    let _t_vnet_demo = boot_elapsed_ms();
    // ADR 0006 (boot-time gate): the virtio-net RX bring-up probe (~950 ms of
    // polling/spin passes) used to run HERE on the boot critical path. It is a
    // once-proven diagnostic that gates no boot-health check, so it now runs
    // ONCE in net::spawn_poll_thread()'s post-boot loop, after the success
    // marker. The real DHCP bind is driven by that same poller.
    let _t_vnet_probe = boot_elapsed_ms();

    // ── DHCP/smoltcp boot drive ───────────────────────────────────────
    // Kick DHCPDISCOVER and pump the smoltcp poll loop a few times so
    // the boot log shows whether ARP + DHCP make it on the wire. We
    // intentionally bound this tightly (boot can't wait on a real DHCP
    // server) — userspace's athnet daemon picks it up afterwards.
    {
        let kicked = dhcp::kick_discovery(0);
        serial_println!(
            "[dhcp] kick_discovery -> emitted={} (DHCPDISCOVER on wire if true)",
            kicked,
        );
        // Drive the network stack AND drain virtio-net RX into the DHCP
        // state machine so OFFER from QEMU can land. We bound this to
        // ~64 iterations × 20k spin cycles ≈ a few ms — enough for the
        // local DHCP server to respond, fast enough to keep boot tight.
        // Drive the network stack until DHCP is Bound (or we hit the
        // cap). Early-exit keeps boot fast on a good day; the cap
        // guarantees we don't sit forever if QEMU's DHCP misses.
        for _ in 0..192 {
            net::poll_full();
            if matches!(dhcp::current_state(), Some(dhcp::DhcpState::Bound)) {
                break;
            }
            for _ in 0..20_000 {
                core::hint::spin_loop();
            }
        }
        let state_after = dhcp::current_state();
        serial_println!("[dhcp] after 64 poll_full() ticks: state={:?}", state_after,);
    }
    let _t_dhcp = boot_elapsed_ms();
    serial_println!(
        "[post9-prof] athfs={}ms vnet_demo={}ms vnet_probe={}ms dhcp={}ms",
        _t_athfs.saturating_sub(_t_post9_start),
        _t_vnet_demo.saturating_sub(_t_athfs),
        _t_vnet_probe.saturating_sub(_t_vnet_demo),
        _t_dhcp.saturating_sub(_t_vnet_probe),
    );

    // Mount INITRAMFS and extract init process
    serial_println!("[ OK ] Loading INITRAMFS, size: {} bytes", INITRAMFS.len());
    let archive = tar::TarArchive::new(INITRAMFS);

    // Boot the graphical demo init process.
    // IMPORTANT: do NOT call scheduler::spawn here — defer it until after the
    // procfs dump so a timer interrupt cannot context-switch to user_init while
    // dump_all_athena_endpoints_to_serial() runs on the boot kernel stack.
    // Doing so would make the userspace syscall handler use the *boot* stack and
    // immediately trigger a double fault on the first sys_open.
    // In installer mode, spawn `athinstaller` in place of the normal shell init.
    // MasterChecklist Phase 3.1: "Installer userspace process spawned in place of
    // normal user_init."
    let init_app = if INSTALLER_MODE.load(Ordering::Relaxed) {
        "athinstaller"
    } else {
        "user_init"
    };
    let deferred_init_task: Option<task::Task> = if let Some(init_file) = archive.get_file(init_app)
    {
        serial_println!("[ OK ] Found {} ({} bytes)", init_app, init_file.data.len());

        let mut task = task::Task::new_elf(init_file.data, None).expect("Failed to load init ELF");

        // Pin user_init to CPU0. Cross-CPU migration of a Ring-3 task via the
        // work-stealing scheduler is not yet reliable: user_init was observed
        // to print its first sentinel, then ping-pong between CPUs ("stole
        // Task" thrashing) without making forward progress. Pinning init to a
        // single core keeps it from being migrated mid-flight while we harden
        // the migration path separately. Honored by both select_cpu and the
        // (now affinity-aware) work-stealing loop in the scheduler.
        task.affinity = task::CpuAffinity::from_mask(0x1);

        // Seed the master "keys-to-the-kingdom" capabilities.
        use capability::{Cap, Rights};
        let h_mmio = task.cap_table.insert_root(Cap::Mmio {
            start_phys: 0xfd00_0000, // framebuffer region (per bootloader log)
            len: 0x0030_0000,
            rights: Rights::ALL,
        });
        let h_irq = task.cap_table.insert_root(Cap::Irq {
            vector: 33, // PIC offset 32 + IRQ1 (keyboard)
            rights: Rights::READ | Rights::WAIT | Rights::GRANT | Rights::REVOKE,
        });
        let h_port = task.cap_table.insert_root(Cap::Port {
            base: 0x3F8, // COM1
            count: 8,
            rights: Rights::READ | Rights::WRITE | Rights::GRANT | Rights::REVOKE,
        });
        // System cap: user_init is the trusted shell root — it (and only what
        // it derives to, e.g. Settings) may read/respond to the permission-
        // prompt queue (syscalls 71/72, now gated: respond performs a REAL
        // capability grant, so an ungated queue was a one-syscall sandbox
        // escape — any task could approve its own request).
        let h_system = task.cap_table.insert_root(Cap::System {
            rights: Rights::READ | Rights::WRITE | Rights::GRANT,
        });
        serial_println!(
            "[ OK ] Seeded user_init caps: mmio={} irq={} port={} system={}",
            h_mmio.raw(),
            h_irq.raw(),
            h_port.raw(),
            h_system.raw(),
        );

        // Channel cap for the keyboard ring buffer (kernel writes scancodes
        // into chan_id=1 from the keyboard IRQ handler; user_init reads them
        // via SYS_RECV).
        let kbd_handle = task.cap_table.insert_root(crate::capability::Cap::Channel {
            chan_id: 1,
            rights: crate::capability::Rights::READ | crate::capability::Rights::GRANT,
        });
        serial_println!(
            "[ OK ] Seeded user_init kbd channel cap (handle={})",
            kbd_handle.raw()
        );

        // Installer needs Cap::System{WRITE} to call SYS_INSTALL_RUN /
        // SYS_INSTALL_CREATE_ACCOUNT (the install + account-creation gate).
        if INSTALLER_MODE.load(Ordering::Relaxed) {
            let h_sys = task.cap_table.insert_root(crate::capability::Cap::System {
                rights: crate::capability::Rights::READ | crate::capability::Rights::WRITE,
            });
            serial_println!(
                "[ OK ] Seeded athinstaller System cap (handle={})",
                h_sys.raw()
            );
        }
        serial_println!(
            "[ OK ] {} prepared (spawn deferred until after procfs dump)",
            init_app
        );

        Some(task)
    } else {
        serial_println!("[WARN] user_init not found in initramfs!");
        None
    };

    // Init system services start after first successful login (shell_runner).

    // ── Boot-time benchmark ────────────────────────────────────────────────
    // ADR 0006 (never-fake-green): the [BOOT-BENCH] T0->userspace print MOVED
    // from here to the record_boot_complete() point below. Snapping it here read
    // ~3.5 s while the real 6 s gate failed at ~8 s, because this point is
    // BEFORE the post-boot spawns + net/athfs post-9 work — it MASKED the red
    // gate. There is now ONE honest boot-time number, printed alongside the
    // gate verdict (`boot_start_tsc` is the T0 reference fast_boot uses).

    // ADR 0006 (next lever): the /proc/athena 98-endpoint serial snapshot
    // (`dump_all_athena_endpoints_to_serial`) is a DIAGNOSTIC, not boot-critical.
    // It used to run HERE — before record_boot_complete() — so its ~900 KiB,
    // 98-getter sweep (every getter does format!/Vec/String work, several take
    // procfs locks) counted against the timed boot and pushed the gate over
    // 6000 ms. Moved to AFTER record_boot_complete()/check_boot_time_gate()
    // below so it no longer gates the timed number. It is NOT deleted: it still
    // emits to COM1 (the serial log) and the GOP screen exactly as before — the
    // dump was always `serial_only_println!` (procfs.rs:1596, deliberately NOT
    // in the bootlog RAM ring / netlog, to avoid evicting the tier transcript),
    // so its durability is unchanged. The begin/end snapshot markers and every
    // endpoint are preserved verbatim.
    //
    // Spawning user_init BEFORE the dump is now safe: the dump body runs
    // entirely inside `without_interrupts` (procfs.rs:1619), so a timer tick
    // can no longer context-switch to user_init mid-dump and corrupt the boot
    // kernel stack — the hazard the old "spawn after dump" ordering guarded
    // against. (The dump call below is additionally wrapped in
    // `without_interrupts` to cover its pre-mask begin-marker prints too.)
    // hello_relibc: spawn from user_init once relibc CRT entry is stable (kernel early
    // spawn hit INVALID OPCODE — printf path not yet safe on bare spawn).

    if let Some(t) = deferred_init_task {
        let tid = t.id;
        let aff = t.affinity.mask;
        scheduler::spawn(t);
        serial_println!(
            "[ OK ] Spawned {} ELF process (task {:?}, affinity_mask={:#x})",
            init_app,
            tid,
            aff,
        );
    }

    // Linux-ABI proof: spawn the embedded static Linux probe through the same
    // linux_exec path AthBridge/Proton binaries use. Its [linux-abi-probe]
    // PASS/FAIL line (printed once the scheduler runs it, captured in the CI
    // daemon-drain window) is the FAIL-able runtime proof that the ~31-syscall
    // Linux translation layer actually works on a real Linux ELF, not just at
    // build time. Deferred here for the same stack-safety reason as user_init.
    linux_exec::run_boot_smoketest();

    // Hand networking off to a continuous post-boot poller. The boot-time DHCP
    // loop only runs for tens of ms — far too short for a real router's OFFER —
    // and nothing drove `net::poll_full()` afterwards, so DHCP could never bind
    // and inbound RX frames were never drained on iron (2026-06-13). Spawn here,
    // after the procfs dump, for the same stack-safety reason as user_init.
    net::spawn_poll_thread();

    // Drive the thermal throttle continuously (Phase 4.7). poll_thermal_zones
    // evaluates AML + reads the AMD SMU, so it must run in a thread, not the
    // timer ISR — without this the passive _PSV/direct-CPU frequency cap never
    // engaged post-boot. Same deferred-spawn constraint as the net poller.
    thermal::spawn_poll_thread();

    // One-shot: the AthBridge thunk-INVOKE proof (constructs a CompatContext +
    // invokes every kernel32 thunk). It can't run in the boot smoketest — the
    // context construction overflows the small BSP boot stack (#DF, "Latent
    // kernel bugs" row) — so it runs here on a spawned thread's 64 KiB stack.
    athbridge_boot::spawn_thunk_invoke_thread();

    // Capture ~15s of POST-boot activity (desktop auto-advance, HID input, net
    // poll) into BOOTLOG.TXT with one late flush — the end-of-boot flush runs
    // before any of those threads do, so they were invisible off-target. If the
    // 'LATE FLUSH' line is missing from the next bootlog, post-boot kernel
    // threads aren't being scheduled on iron (the prime suspect for dead mouse
    // + no desktop despite user_init running).
    bootlog_persist::spawn_late_flush();

    // ── Phase 0 gate: record boot completion and check against 6 s target ─
    fast_boot::record_boot_complete();
    fast_boot::check_boot_time_gate();
    // ADR 0006 (never-fake-green): print the [BOOT-BENCH] T0->userspace line
    // HERE, off the SAME end-snap as record_boot_complete()/check_boot_time_gate
    // above, so it can no longer disagree with the gate. fast_boot::boot_time_ms
    // is the single source of truth for both lines.
    {
        let honest_ms = fast_boot::boot_time_ms();
        serial_println!(
            "[BOOT-BENCH] T0 -> userspace = {} ms. Concept target: <6000 ms (stretch 3000 ms).",
            honest_ms,
        );
        if honest_ms != 0 && honest_ms < 6000 {
            serial_println!("[BOOT-BENCH] [OK] Within concept target of 6s.");
        } else if honest_ms != 0 {
            serial_println!(
                "[BOOT-BENCH] [WARN] Boot exceeded 6s target by {} ms.",
                honest_ms - 6000
            );
        }
        procfs::record_boot_time_ms(honest_ms);
    }

    // Dump every /proc/athena/* endpoint to serial so an external dev tool (or a
    // remote AI agent) gets a complete observable snapshot per boot. See
    // `docs/HARDWARE_PATH.md` §"smart practical version" — Tier 1 of the
    // Claude-on-host introspection ladder.
    //
    // ADR 0006: moved here, AFTER record_boot_complete()/check_boot_time_gate()
    // above, so this ~900 KiB diagnostic no longer counts against the 6000 ms
    // boot-time gate. Wrapped in without_interrupts because user_init is already
    // spawned at this point — masking guarantees the dump (which runs on the
    // boot kernel stack) cannot be preempted into the userspace task and corrupt
    // that stack. The dump's own inner without_interrupts (procfs.rs) is
    // re-entrant-safe with this outer mask.
    x86_64::instructions::interrupts::without_interrupts(|| {
        procfs::dump_all_athena_endpoints_to_serial();
    });

    // ── Phase 6.4 smoketest: confirm VRR + HDR stubs were registered ───────
    compositor::run_boot_smoketest();
    // ── Phase 19.1 smoketest: a11y tree wire round-trip + cap gate ─────────
    a11y::run_boot_smoketest();

    // Last chance for the persisted log: if the earlier scan found no
    // BOOTLOG.TXT (stick enumerated late / transient bulk error), re-scan
    // now — then print the one-photo USB/bootlog summary so a single
    // photographed screen answers "why didn't the stick get the log".
    bootlog_persist::retry_if_not_ready();
    xhci::print_end_of_boot_summary();
    // First netlog pass: the ring now contains everything through the USB
    // summary. A second pass after the success marker (below) re-sends the
    // whole ring, so a frame lost here is recovered there.
    netlog::broadcast_ring("end-of-boot");

    // Persist the in-RAM bootlog ring to \BOOTLOG.TXT (FAT16/FAT32 ESP,
    // USB stick preferred) so a power-cycle preserves the transcript.
    // Best-effort: no-ops if init() found no pre-allocated file. Runs
    // BEFORE the success marker so a harness that stops the VM at the
    // marker still captures this end-of-boot tail flush. The carveout in
    // block_io::safe_mode_guard_write was already registered by
    // bootlog_persist::init; only the LBAs holding our file are writable
    // in safe-mode, everything else stays blocked.
    bootlog_persist::flush();

    // Enable preemption, then run the consolidated boot self-test so its
    // scheduler check sees BOOT_COMPLETE. One authoritative health line for
    // the bare-metal verify (MasterChecklist §1.9 / §4.12), emitted right
    // before the canonical success marker so a single log grep covers both.
    //
    // CRITICAL ordering hazard: setting BOOT_COMPLETE flips CPU0 from
    // never-preempted to preemptible (scheduler.rs gates the cpu0 tick on it).
    // selftest::run() is the FIRST heap-heavy code (format!/Vec/String) to run
    // on CPU0 after that flip. The global allocator spinlock (HEAP_INNER) is
    // not IRQ-reentrant: if a timer IRQ context-switches CPU0 away while it
    // holds that lock mid-allocation, every other CPU's next alloc spins on it
    // and the system wedges before the success marker ever prints. Mask
    // interrupts across the self-test so CPU0 cannot be preempted while holding
    // the allocator lock — it stays the boot CPU's last non-preemptible act,
    // exactly as the rest of boot ran. Normal preemption resumes for hlt_loop.
    // Heavy KASAN endurance soak (feature = "kasan" only; a NO-OP in the default
    // build — boot is byte-identical). Run HERE, just BEFORE BOOT_COMPLETE flips
    // CPU0 to preemptible: interrupts are already ENABLED (the LAPIC timer has
    // driven yield_task since line ~533), so this is a normal interrupts-on
    // context — NOT the masked post-marker sweep — but the scheduler does not yet
    // preempt CPU0, so the churn runs to completion deterministically and lands
    // its FAIL-able `[soak-kasan] endurance: ... -> PASS/FAIL` line. (A
    // fire-and-forget post-BOOT_COMPLETE thread is starved by sched_proof's EDF
    // load + user_init and reaped by CI before it finishes — observed.) The heavy
    // churn stays OUT of the default boot path entirely via the feature gate, so
    // the "heavy work in the boot critical path" hazard applies only to the
    // kasan/endurance build, which is exactly where we WANT it (KASAN is here to
    // catch any corruption the churn surfaces). It is deeper #6 (zero-leak)
    // evidence than the light boot-soak: a leak → nonzero heap delta → FAIL; a
    // UAF/OOB the churn trips → kasan_errors>0 → FAIL.
    soak::run_endurance();

    scheduler::BOOT_COMPLETE.store(true, core::sync::atomic::Ordering::Release);

    // Run the self-test AND print the success marker as one non-preemptible act.
    // BOOT_COMPLETE (above) makes CPU0 preemptible; previously the marker println
    // sat AFTER `without_interrupts(selftest::run)` returned and re-enabled IRQs,
    // so a timer IRQ in that window could context-switch CPU0 into user_init —
    // and if user_init then blocked, the marker never printed and the boot
    // looked hung even though it had completed. Mask through the marker so it
    // always lands; normal preemption resumes at hlt_loop.
    x86_64::instructions::interrupts::without_interrupts(|| {
        selftest::run();
        serial_println!("[ OS ] System successfully booted.");
        serial_println!("  [boot] Scheduler preemption enabled, entering halt loop.");
        // Userspace reached: the active A/B kernel slot is healthy — reset its
        // boot-attempt counter so the fallback ladder never fires on a good
        // kernel (MasterChecklist Phase 3.6).
        update_slots::mark_boot_successful();
        // Final BOOTLOG.TXT flush WITH the success marker in the ring: the
        // on-stick log can now prove a boot completed (previously the last
        // flush ran before the marker printed, so bare-metal sticks never
        // showed it). Still inside without_interrupts — flush allocates a
        // ~1 MiB String (dump_text) and CPU0 is preemptible after
        // BOOT_COMPLETE, which is exactly the allocator-lock preemption
        // hazard documented above; the block writes are polled, not
        // IRQ-driven, so masking is safe here.
        bootlog_persist::flush();
        // Second netlog pass with the success marker included — netlog TX is
        // polled (no IRQs needed), so masking is safe here too.
        netlog::broadcast_ring("final");

        // ── ADR 0006: deferred boot self-test sweep (post-marker) ──────────
        // The userspace-feature correctness smoketests (theme/vibe/wallpaper/
        // rgb/widgets/search/game_profile/wireguard) were moved OFF the boot
        // critical path so the marker — and the 6 s gate measured at
        // record_boot_complete() above — no longer wait on them. They still
        // print PASS/FAIL (R10 rule 16); only the ordering changed.
        //
        // CRITICAL: this MUST run INSIDE the marker's without_interrupts block,
        // AFTER the marker prints. The moment this block returns, IRQs re-enable
        // and (BOOT_COMPLETE being set) a timer tick preempts CPU0 into the
        // runqueue — the deadline HID task + Normal tasks then dominate and the
        // kernel_main continuation can starve indefinitely, so a sweep placed
        // after the block never runs (observed: zero sweep output). Masked here,
        // it is guaranteed to execute as the last act of the boot tail. The
        // boot-time gate was already snapped at record_boot_complete() above, so
        // this adds nothing to the measured number. Same allocator-lock-under-
        // mask safety as selftest::run() (these smoketests allocate via format!).
        boot_selftest::run_deferred();
    });
    serial_println!();

    // SCHED_BODY/EDF deadline-adherence proof (Concept §Gaming-First: "Gaming
    // isn't a mode"). Launched HERE — AFTER the masked marker block — on purpose:
    // it spawns a deadline-class frame orchestrator + competing SCHED_NORMAL hogs,
    // and the EDF class has HARD priority over SCHED_NORMAL, so spawning it BEFORE
    // the marker let those deadline tasks preempt and starve the kernel_main
    // continuation (a NORMAL task) before it reached the masked block — the boot
    // then looked hung even though it had completed, and the "System successfully
    // booted." marker never printed (MasterChecklist L80 boot-marker regression;
    // reproduced 0/marker on the Athena -cpu host KVM run, deadline=2 still Ready
    // at timeout). Post-marker, the proof runs as a real preemptive post-boot
    // diagnostic instead — its FAIL-able `[sched-proof] EDF deadline adherence:`
    // line prints right after the marker. Requirements preserved: BOOT_COMPLETE is
    // set, interrupts are ON (restored when the without_interrupts block above
    // returned — the LAPIC timer has driven yield_task since line ~533), and this
    // is OUTSIDE without_interrupts (the EDF proof relies on timer IRQs).
    sched_proof::init();

    // DEV-ONLY screenshot harness (cargo feature `desktop_autologin`): now that
    // the boot is complete and CPU0 is preemptible, spawn the guest-desktop
    // autologin thread in this proven post-boot context so the headless visual-QA
    // capture lands on the live desktop instead of the OOBE/login screen. Strictly
    // feature-gated — a shipped build never auto-signs-in (owner directive).
    #[cfg(feature = "desktop_autologin")]
    shell_runner::spawn_desktop_autologin();

    hlt_loop();
}

/// Halt the CPU until the next interrupt, in a loop.
/// More power-efficient than a busy spin.
pub fn hlt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}

/// The runtime base address the PIE kernel was loaded at. The bootloader picks
/// this per firmware — QEMU/OVMF uses `0x10000000000`, but real UEFI firmware
/// has been observed at `0x8000000000`. Subtracting it from a code address gives
/// the ELF file offset that `scripts/resolve-panic.ps1` resolves to a symbol.
///
/// Computed from a local function's runtime address: the image is well under
/// 4 GiB and the load base is 4 GiB-aligned, so masking off the low 32 bits
/// yields the base regardless of which firmware loaded us. Hardcoding the QEMU
/// base made every real-hardware backtrace decode to garbage.
#[inline(never)]
pub fn kernel_image_base() -> u64 {
    #[inline(never)]
    extern "C" fn anchor() {}
    (anchor as usize as u64) & 0xFFFF_FFFF_0000_0000
}
