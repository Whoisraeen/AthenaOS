//! Persist the in-RAM bootlog ring (kernel/src/bootlog.rs) into a
//! pre-created file on the existing FAT16/FAT32 ESP.
//!
//! # Why this exists
//!
//! Bare-metal Athena has no serial cable, boots fast enough that the
//! framebuffer text mirror scrolls past anything interesting, and (today)
//! has no working keyboard — so there's no way to dump `/proc/athena/bootlog`
//! interactively. The RAM ring captures everything but dies on power-cycle.
//! We need the transcript on disk so it can be read after a reboot to
//! Windows.
//!
//! # The safe design: overwrite an existing file's DATA only
//!
//! An earlier version had the kernel *allocate* clusters and *create* a
//! directory entry on the Windows ESP. That was wrong on two counts:
//!   1. Safe-mode (correctly) blocked the kernel's own FAT/dirent writes,
//!      so init failed silently.
//!   2. Modifying the FAT and root directory of the partition Windows
//!      boots from risks corrupting it.
//!
//! This version never touches filesystem metadata. The file's clusters are
//! allocated ahead of time by a real FAT implementation:
//!
//!   * **USB stick (preferred):** xtask bakes a 1 MiB `BOOTLOG.TXT` into the
//!     boot image's ESP at build time (`DiskImageBuilder::set_file_contents`),
//!     so every flashed stick already carries it. NOTE: the `bootloader`
//!     crate's ESP is **FAT16** at its ~27 MiB size — the locator below
//!     handles FAT16 and FAT32.
//!   * **Internal NVMe ESP (fallback):** the user creates it once from
//!     Windows, where Windows' own FAT driver allocates the clusters safely:
//!
//! ```text
//!   (admin cmd)
//!   mountvol B: /S
//!   fsutil file createnew B:\BOOTLOG.TXT 1048576    REM 1 MiB, zero-filled
//! ```
//!
//! On boot the kernel scans **every** block device for the file, preferring
//! removable USB media so the log can be pulled on another machine without
//! ever writing to the internal NVMe (and USB writes aren't safe-mode-guarded,
//! so it works on a `--safe` image). For the chosen device it:
//!   1. Finds the ESP (read-only).
//!   2. Scans the root directory — following its cluster chain across
//!      multiple clusters — for the 8.3 entry `BOOTLOG TXT`.
//!   3. Follows that file's FAT cluster chain (read-only) to enumerate
//!      every data cluster it owns.
//!   4. On the active (NVMe/virtio) device only, registers the data-cluster
//!      LBA ranges as the safe-mode carveout — so ONLY those sectors become
//!      writable, and only the file's contents, never FAT or the directory.
//!      (USB devices need no carveout; their writes aren't guarded.)
//!   5. [`flush()`] writes the log text into the data clusters and
//!      zero-pads (no leftover disk garbage). The FIRST flush locks the
//!      early-boot transcript into the file's first half; later flushes
//!      rewrite only the second half with the newest ring tail — so a
//!      wrapped ring can never evict the early lines from disk.
//!
//! If `BOOTLOG.TXT` doesn't exist, the kernel prints a one-line
//! instruction telling the user how to create it, and disables — it never
//! tries to create the file itself.
//!
//! # Reader side
//!
//!   mountvol B: /S            (admin)
//!   notepad B:\BOOTLOG.TXT
//!   mountvol B: /D            (when done)

#![allow(dead_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::fatfs_esp;
use crate::fatfs_esp::FatKind;

/// EOC threshold: FAT32 entries >= this mark end-of-chain.
const FAT_EOC: u32 = 0x0FFF_FFF8;
/// EOC threshold for FAT16 chains. The boot image's ESP (built by the
/// `bootloader` crate) is FAT16 at its ~27 MiB size, so the USB-stick path
/// MUST handle 16-bit FAT entries or it never finds BOOTLOG.TXT.
const FAT16_EOC: u32 = 0xFFF8;

fn is_eoc(kind: FatKind, entry: u32) -> bool {
    match kind {
        FatKind::Fat16 => entry >= FAT16_EOC,
        FatKind::Fat32 => entry >= FAT_EOC,
    }
}
/// Bound the root-directory cluster walk so a corrupt chain can't loop forever.
const MAX_ROOT_CLUSTERS: u32 = 64;
/// Bound the file cluster-chain walk likewise.
const MAX_FILE_CLUSTERS: u32 = 4096;

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);
static FILE_FIRST_LBA: AtomicU64 = AtomicU64::new(0);
static FILE_SECTOR_COUNT: AtomicU64 = AtomicU64::new(0);
static FLUSH_COUNT: AtomicUsize = AtomicUsize::new(0);
static LAST_FLUSH_BYTES: AtomicUsize = AtomicUsize::new(0);
const SAFE_AUTORESET_SECONDS: u64 = 480;
const SAFE_HW_BACKSTOP_SECONDS: u32 = 540;
static SAFE_AUTORESET_DEADLINE: AtomicU64 = AtomicU64::new(0);
static SAFE_AUTORESET_FIRING: AtomicBool = AtomicBool::new(false);

/// Layout captured at init, consumed by flush. The cluster_lbas list is
/// the file's data clusters in order (each entry = first LBA of one
/// cluster). capacity_bytes is the file's full allocated size.
static LAYOUT: spin::Mutex<Option<FileLayout>> = spin::Mutex::new(None);

struct FileLayout {
    sectors_per_cluster: u8,
    cluster_lbas: Vec<u64>,
    capacity_bytes: usize,
}

/// The block device the bootlog writes to. `Some` when a non-active device
/// (e.g. a USB stick) was chosen; `None` means use `ACTIVE_BLOCK_DEVICE`.
static LOG_DEVICE: spin::Mutex<Option<alloc::boxed::Box<dyn crate::block_io::BlockDevice>>> =
    spin::Mutex::new(None);

/// Read-only probe: locate a pre-allocated `BOOTLOG.TXT` in `dev`'s ESP and
/// return its on-disk layout (data-cluster LBAs + capacity), or the exact
/// step that failed — the reason is logged per candidate device, which is
/// the only way to diagnose this path on bare metal (no debugger, no
/// serial). Safe to run against every candidate.
fn locate_on_device(dev: &dyn crate::block_io::BlockDevice) -> Result<FileLayout, &'static str> {
    let esp_start = match fatfs_esp::find_esp_partition_on(dev) {
        fatfs_esp::PartitionScan::GptEspFound { start_lba, .. } => start_lba,
        fatfs_esp::PartitionScan::MbrFat32Found { start_lba, .. } => start_lba,
        fatfs_esp::PartitionScan::GptNoEsp { .. } => return Err("GPT but no ESP partition"),
        fatfs_esp::PartitionScan::MbrNoFat32 { .. } => return Err("MBR but no FAT partition"),
        fatfs_esp::PartitionScan::NoPartitionTable => return Err("no partition table"),
        fatfs_esp::PartitionScan::ReadError(e) => return Err(e),
    };

    let mut vbr = [0u8; 512];
    if dev.read_sector(esp_start, &mut vbr).is_err() {
        return Err("VBR read failed");
    }
    let vol = fatfs_esp::parse_vbr_any(&vbr).map_err(|_| "VBR is not FAT16/FAT32")?;

    let fat_lba = esp_start + vol.reserved_sectors as u64;
    let data_start = esp_start + vol.data_start_offset();
    let spc = vol.sectors_per_cluster as u64;

    let entry = match vol.kind {
        // FAT32: the root directory is a cluster chain in the data region.
        FatKind::Fat32 => find_root_file_fat32(
            dev,
            fat_lba,
            vol.root_cluster,
            spc,
            data_start,
            b"BOOTLOG ",
            b"TXT",
        ),
        // FAT16: the root directory is a fixed sector run between the
        // FATs and the data region — no chain to follow.
        FatKind::Fat16 => {
            let root_lba = fat_lba + vol.num_fats as u64 * vol.fat_size_sectors as u64;
            find_root_file_fat16(dev, root_lba, vol.root_dir_sectors(), b"BOOTLOG ", b"TXT")
        }
    }
    .ok_or("BOOTLOG.TXT not in root directory")?;
    if entry.first_cluster < 2 || entry.size == 0 {
        return Err("BOOTLOG.TXT entry has no clusters/size");
    }

    // Follow the file's FAT chain to enumerate every data cluster. A 1 MiB
    // file has up to ~1024 chain entries but they live in only a handful of
    // FAT sectors — cache the last sector read so the walk costs ~8 device
    // reads instead of ~1024 (each USB read is a full CBW/data/CSW round
    // trip).
    let mut cluster_lbas: Vec<u64> = Vec::new();
    let mut cluster = entry.first_cluster;
    let mut hops = 0u32;
    let entry_width = match vol.kind {
        FatKind::Fat16 => 2u64,
        FatKind::Fat32 => 4u64,
    };
    let mut cached_lba = u64::MAX;
    let mut cached_sec = [0u8; 512];
    loop {
        if cluster < 2 || is_eoc(vol.kind, cluster) {
            break;
        }
        cluster_lbas.push(data_start + (cluster as u64 - 2) * spc);
        hops += 1;
        if hops >= MAX_FILE_CLUSTERS {
            break;
        }
        let byte_offset = cluster as u64 * entry_width;
        let lba = fat_lba + byte_offset / 512;
        let off = (byte_offset % 512) as usize;
        if lba != cached_lba {
            if dev.read_sector(lba, &mut cached_sec).is_err() {
                return Err("FAT chain read failed");
            }
            cached_lba = lba;
        }
        cluster = match vol.kind {
            FatKind::Fat16 => u16::from_le_bytes([cached_sec[off], cached_sec[off + 1]]) as u32,
            FatKind::Fat32 => {
                u32::from_le_bytes([
                    cached_sec[off],
                    cached_sec[off + 1],
                    cached_sec[off + 2],
                    cached_sec[off + 3],
                ]) & 0x0FFF_FFFF
            }
        };
    }
    if cluster_lbas.is_empty() {
        return Err("FAT chain empty");
    }

    let capacity_bytes = cluster_lbas.len() * spc as usize * 512;
    Ok(FileLayout {
        sectors_per_cluster: vol.sectors_per_cluster,
        cluster_lbas,
        capacity_bytes,
    })
}

/// Record a located layout + chosen device, then flush once immediately so a
/// hang right after init still leaves a log on disk.
fn finalize(
    layout: FileLayout,
    log_dev: Option<alloc::boxed::Box<dyn crate::block_io::BlockDevice>>,
    source: &str,
) {
    let n = layout.cluster_lbas.len();
    let cap = layout.capacity_bytes;
    FILE_FIRST_LBA.store(layout.cluster_lbas[0], Ordering::SeqCst);
    FILE_SECTOR_COUNT.store(
        (n as u64) * layout.sectors_per_cluster as u64,
        Ordering::SeqCst,
    );
    *LOG_DEVICE.lock() = log_dev;
    *LAYOUT.lock() = Some(layout);
    READY.store(true, Ordering::SeqCst);

    crate::serial_println!(
        "[bootlog-persist] ready: BOOTLOG.TXT on {} — {} clusters, {} KiB capacity, first LBA {}",
        source,
        n,
        cap / 1024,
        FILE_FIRST_LBA.load(Ordering::Relaxed),
    );
    flush();
}

/// Initialize. Scans candidate block devices for a pre-allocated `BOOTLOG.TXT`
/// and writes the kernel log there. **Removable USB media is preferred** — so
/// the log can be pulled on another machine without ever touching the internal
/// NVMe (and USB writes aren't safe-mode-guarded, so it works on a `--safe`
/// image). The active block device (NVMe/virtio) is the fallback, where the
/// file's data clusters are registered as the safe-mode write carveout.
/// Best-effort + idempotent.
pub fn init() {
    if INITIALIZED.swap(true, Ordering::SeqCst) {
        return;
    }
    scan_and_attach();
}

/// Late retry for the Athena flash cycle: if init() found no BOOTLOG.TXT
/// (e.g. the stick's MSC enumeration completed after the first scan, or an
/// earlier transient bulk error), re-scan once at end-of-boot so the log
/// still lands. No-op when already attached.
pub fn retry_if_not_ready() {
    if READY.load(Ordering::Relaxed) {
        return;
    }
    crate::serial_println!("[bootlog-persist] end-of-boot retry: re-scanning for BOOTLOG.TXT...");
    crate::usb_msc::reprobe_if_empty();
    scan_and_attach();
}

/// One-line state for the end-of-boot `[usb-summary]` block.
pub fn status_line() -> String {
    if READY.load(Ordering::Relaxed) {
        alloc::format!(
            "READY — {} flush(es), last {} bytes, {} sectors @ LBA {}",
            FLUSH_COUNT.load(Ordering::Relaxed),
            LAST_FLUSH_BYTES.load(Ordering::Relaxed),
            FILE_SECTOR_COUNT.load(Ordering::Relaxed),
            FILE_FIRST_LBA.load(Ordering::Relaxed),
        )
    } else {
        String::from("NOT READY — BOOTLOG.TXT not found on any device (log lives only in RAM)")
    }
}

fn scan_and_attach() {
    // 1. Prefer USB Mass Storage (removable). USB write_sector IS safe-mode
    //    guarded as of commit 4d228c8 (the USB-MSC hole was closed), so on a
    //    `--safe` image both gates (read-only + safe-mode) reject the flush
    //    unless the file's own data clusters are registered as the carveout —
    //    exactly like the active-device branch below. Without this, BOOTLOG.TXT
    //    on the stick stays empty after a safe boot (the primary iron
    //    diagnostic loop) even though the file was located.
    for (i, dev) in crate::usb_msc::msc_block_devices().into_iter().enumerate() {
        match locate_on_device(dev.as_ref()) {
            Ok(layout) => {
                let ranges =
                    coalesce_ranges(&layout.cluster_lbas, layout.sectors_per_cluster as u64);
                crate::block_io::set_log_lba_carveout(ranges);
                finalize(layout, Some(dev), "USB drive");
                return;
            }
            Err(why) => {
                crate::serial_println!("[bootlog-persist] usb{}: no BOOTLOG.TXT ({})", i, why);
            }
        }
    }

    // 2. Fall back to the active block device (NVMe/virtio). This path IS
    //    safe-mode guarded, so register the file's data clusters as the
    //    write carveout before flushing.
    let active_layout = {
        let guard = crate::block_io::ACTIVE_BLOCK_DEVICE.lock();
        guard.as_ref().map(|d| locate_on_device(d.as_ref()))
    };
    match &active_layout {
        Some(Err(why)) => {
            crate::serial_println!("[bootlog-persist] active device: no BOOTLOG.TXT ({})", why);
        }
        None => {
            crate::serial_println!("[bootlog-persist] no active block device to probe");
        }
        Some(Ok(_)) => {}
    }
    if let Some(Ok(layout)) = active_layout {
        let ranges = coalesce_ranges(&layout.cluster_lbas, layout.sectors_per_cluster as u64);
        crate::block_io::set_log_lba_carveout(ranges);
        finalize(layout, None, "active block device");
        return;
    }

    crate::serial_println!(
        "[bootlog-persist] BOOTLOG.TXT not found on any device — persistent log disabled."
    );
    crate::serial_println!(
        "[bootlog-persist]   Flash a current image (xtask bakes BOOTLOG.TXT into the ESP),"
    );
    crate::serial_println!(
        "[bootlog-persist]   or create a 1 MiB B:\\BOOTLOG.TXT on the NVMe ESP:"
    );
    crate::serial_println!("[bootlog-persist]     mountvol B: /S");
    crate::serial_println!("[bootlog-persist]     fsutil file createnew B:\\BOOTLOG.TXT 1048576");
}

/// Write the current bootlog ring into BOOTLOG.TXT's data clusters (see
/// [`flush_to`] for the split-region layout: first flush locks the early
/// transcript into the first half; later flushes rewrite only the tail
/// half). Zero-pads so the reader never sees leftover disk garbage.
/// Touches only data clusters — never FAT, never the directory entry.
/// Safe to call repeatedly.
pub fn flush() {
    if !READY.load(Ordering::Relaxed) {
        return;
    }
    // Write to the chosen log device (a USB stick when one had BOOTLOG.TXT),
    // else the active block device.
    let log_guard = LOG_DEVICE.lock();
    if let Some(d) = log_guard.as_ref() {
        flush_to(d.as_ref());
        return;
    }
    drop(log_guard);
    let active_guard = crate::block_io::ACTIVE_BLOCK_DEVICE.lock();
    if let Some(d) = active_guard.as_ref() {
        flush_to(d.as_ref());
    }
}

/// One-shot post-boot capture thread. The end-of-boot flush writes BOOTLOG.TXT
/// before any post-boot kernel thread (desktop auto-advance, HID input, net
/// poll) has run — so their output (and any failure to RUN at all) was
/// invisible off-target. This thread waits well past desktop bring-up (~15 s),
/// then flushes the ring ONCE. Being long after bring-up, the single block I/O
/// can't contend with it (the earlier poll-thread flush failed by firing
/// DURING bring-up). If this line itself is ABSENT from BOOTLOG.TXT, that alone
/// proves post-boot kernel threads aren't being scheduled on iron.
extern "C" fn late_flush_thread_entry() {
    // PERIODIC (bounded) post-boot capture. A single one-shot flush closed the
    // capture window before the user could test INPUT (mouse wiggle / keypresses
    // happen after the desktop appears), so live HID activity never landed in
    // BOOTLOG.TXT. Now we flush ~5 times spaced ~7s apart (covering ~7..35s):
    // each flush overwrites BOOTLOG.TXT with the current RAM ring, so whenever
    // the user powers off after testing input, the most recent flush captured it
    // (incl. the [hid-diag] heartbeats + any "HID report" lines). Bounded count +
    // spacing well past the ~2.5s desktop auto-advance so the block I/O never
    // contends with bring-up and never runs forever (the poll-thread flush hazard
    // was a flush DURING bring-up / in a hot loop — neither applies here).
    let mut flush_no = 0u32;
    loop {
        // ~2.5s between flushes (was ~7s): amdgpud's bring-up runs LATE in boot and
        // a sparse/short window let it run out the clock before finishing (boots
        // 050943/065153/072405 all truncated mid-bring-up). Denser flushes + a wider
        // count (below) keep capturing the post-boot ring as the daemon progresses.
        for _ in 0..250 {
            crate::scheduler::yield_task();
            x86_64::instructions::hlt();
        }
        flush_no += 1;
        crate::serial_println!(
            "[bootlog-persist] LATE FLUSH #{}: persisting post-boot ring (incl. live HID input) to BOOTLOG.TXT",
            flush_no
        );
        // NETLOG FIRST, before the flush: the UDP broadcast touches NO block I/O, so a
        // stalled NVMe BOOTLOG write below can never lose the capture (a live NVMe
        // controller blocking this flush bare-metal stranded the Athena 2026-06-28). A
        // netlog-listen session sees POST-BOOT output (amdgpud bring-up, the scratch
        // result, live HID) live with no stick round-trip. The two end-of-boot netlog
        // passes (kernel_main) fire BEFORE post-boot threads run, so this is the only
        // netlog that carries their lines. NIC TX is polled (no IRQs); no-ops cleanly
        // if there's no link (e.g. QEMU without a NIC).
        crate::netlog::broadcast_ring("late-flush");
        flush();
        // 80 passes @ ~2.5s = ~200s window: cold-boot GPU bring-up (PSP fw-load x15
        // + 200ms EnableGfxImu wait + 8MB DMA write + setup_imu + try_imu_core_start
        // + MES version heartbeat) was running right to the ~180s wall-clock limit
        // with the old 50-pass (125s) window. 80 passes gives 200s so the full
        // stage-6 sequence including CRESET release, GFX reset-done poll, GFXOFF
        // disable, RLC SRM, and MES fw-version all land before the auto-reboot.
        // 160 passes (was 80): Athena 2026-06-29 the GPU bring-up was CPU-starved by
        // the compositor on the single post-boot CPU and the IMU-start poll alone ran
        // past the old 200s window (bootlog stalled mid-poll). Doubling to ~400s gives
        // a slow-but-progressing bring-up room to reach the MES stage + be flushed to
        // BOOTLOG.TXT before the auto-reboot, so an SSH read after the return is complete.
        if flush_no >= 160 {
            break;
        }
    }
    // BARE-METAL AUTO-REBOOT (safe-mode only). The Athena bare-metal test image has
    // no SSH and won't return control on its own, so after the full capture window
    // (~125s: the GPU bring-up has run + been broadcast to the LAN netlog + flushed to
    // BOOTLOG.TXT, captured over SSH), reboot. A one-shot efibootmgr `BootNext` is
    // consumed by this boot, so the reset returns the Athena to Linux for the next
    // SSH-driven iteration — no human power-cycle. Gated on SAFE_MODE (= the
    // `safe_mode` feature, true only in the `--safe` image), so it NEVER fires in the
    // normal install image or the non-safe cold-vfio runs.
    if crate::block_io::SAFE_MODE.load(core::sync::atomic::Ordering::Relaxed) {
        crate::serial_println!(
            "[bootlog-persist] SAFE-MODE bare-metal: capture window done -> auto-reboot (one-shot BootNext returns the Athena to Linux)"
        );
        crate::netlog::broadcast_ring("pre-reboot");
        crate::installer_ui::reboot();
    }
    crate::scheduler::exit_current_task(0);
}

/// Spawn the late-flush capture thread. Call once, after `user_init` spawn.
/// SAFE-MODE bare-metal auto-return SAFETY NET — INDEPENDENT of the NVMe flush.
/// The late-flush thread's end-of-window auto-reboot only runs if its loop completes, but
/// a live NVMe controller can block `flush()` bare-metal and stall the loop forever — which
/// stranded the Athena 2026-06-28 (AthenaOS running + ping-able, but no auto-reboot, no SSH,
/// needing a physical power-cycle). This thread shares NOTHING with that loop: it sleeps a
/// fixed ~150s using only `yield`+`hlt` (no block I/O), re-broadcasts the ring over UDP
/// netlog (the flush-free capture path), then triple-faults via `reboot_no_flush` — a
/// guaranteed reset. A one-shot efibootmgr `BootNext` is already consumed by this boot, so
/// the reset returns the Athena to Linux for the next SSH iteration with no human action.
/// Gated on SAFE_MODE so it NEVER fires in the normal install image or non-safe runs.
extern "C" fn safe_autoreset_thread_entry() {
    if !crate::block_io::SAFE_MODE.load(core::sync::atomic::Ordering::Relaxed) {
        crate::scheduler::exit_current_task(0);
    }
    // WALL-CLOCK sleep (~180s), NOT an iteration count. yield+hlt iterations stretch badly
    // when this thread is descheduled (bare-metal 2026-06-28: a "150s" 15000-iter loop took
    // >18min and stranded the box, because the live GPU + compositor starved it). timers::
    // JIFFIES is bumped by the LAPIC IRQ (HZ=100, 1 jiffy=10ms) regardless of which task is
    // running, so this fires after REAL ~180s no matter how little CPU the thread gets.
    // 180s was past amdgpud's spawn + the OLD GPU bring-up, but the host RLC backdoor
    // autoload path (the games first-light sequence) overruns it: the 2026-06-28 cold
    // boot reached setup_imu exactly as 180s guillotined it, one step short of start_imu.
    // Give it 480s of runway, and re-broadcast the ring every ~10s so the bring-up's
    // progress (and where it wedges, if it does) is captured live at fine granularity,
    // not only at the end — a coarse 45s window missed a fast post-first-light wedge.
    let hz = crate::timers::HZ;
    let deadline = SAFE_AUTORESET_DEADLINE.load(Ordering::Acquire);
    let mut next_bcast =
        crate::timers::JIFFIES.load(core::sync::atomic::Ordering::Relaxed) + 10 * hz;
    while crate::timers::JIFFIES.load(core::sync::atomic::Ordering::Relaxed) < deadline {
        crate::scheduler::yield_task();
        x86_64::instructions::hlt();
        if crate::timers::JIFFIES.load(core::sync::atomic::Ordering::Relaxed) >= next_bcast {
            crate::netlog::broadcast_ring("safe-progress");
            next_bcast += 10 * hz;
        }
    }
    crate::serial_println!(
        "[bootlog-persist] SAFE-MODE bare-metal auto-return: ~480s WALL-CLOCK elapsed -> netlog + triple-fault reset (one-shot BootNext returns the Athena to Linux)"
    );
    crate::netlog::broadcast_ring("safe-autoreset");
    crate::installer_ui::reboot_no_flush_irq();
}

/// Lock-free IRQ-context deadline for the safe-image one-shot return.
///
/// The logging worker above is useful for progress broadcasts, but the
/// 2026-07-12 Athena run proved it can be starved after only a few passes.
/// Checking the same absolute deadline from the BSP LAPIC tick removes process
/// scheduling from the reset path. Do not add logging, allocation, or locks.
pub fn on_timer_tick() {
    let deadline = SAFE_AUTORESET_DEADLINE.load(Ordering::Acquire);
    if deadline == 0
        || crate::gdt::current_cpu_id() != 0
        || crate::timers::JIFFIES.load(Ordering::Relaxed) < deadline
        || SAFE_AUTORESET_FIRING.swap(true, Ordering::AcqRel)
    {
        return;
    }
    crate::installer_ui::reboot_no_flush_irq();
}

pub fn spawn_late_flush() {
    if crate::block_io::SAFE_MODE.load(Ordering::Relaxed) {
        let deadline = crate::timers::JIFFIES
            .load(Ordering::Relaxed)
            .saturating_add(SAFE_AUTORESET_SECONDS.saturating_mul(crate::timers::HZ));
        SAFE_AUTORESET_FIRING.store(false, Ordering::Release);
        SAFE_AUTORESET_DEADLINE.store(deadline, Ordering::Release);
    }
    let task = crate::task::Task::new(late_flush_thread_entry, None);
    // Pin to CPU 0 — the APs don't schedule post-boot (see scheduler::spawn_on_bsp).
    crate::scheduler::spawn_on_bsp(task);
    // SAFE-MODE bare-metal: a flush-INDEPENDENT auto-return safety net. The late-flush
    // auto-reboot above can be stalled forever by a blocked NVMe write (that stranded the
    // Athena); this guarantees the box returns to Linux on a timer regardless.
    let reset_task = crate::task::Task::new(safe_autoreset_thread_entry, None);
    crate::scheduler::spawn_on_bsp(reset_task);
    // SAFE-MODE: arm the HARDWARE watchdog as the RELIABLE backstop. The two sw threads
    // above both ride JIFFIES (LAPIC IRQ); a HARD hang (no IRQs — exactly what the GPU
    // bring-up wedges) freezes JIFFIES and strands the box, needing a human power-cycle
    // (happened 3x on 2026-06-29). The EFCH watchdog resets in hardware regardless.
    // Its 540s deadline trails the IRQ-driven 480s reset by 60s, preserving the
    // capture window while still returning from a total IRQ-off hard hang.
    if crate::block_io::SAFE_MODE.load(core::sync::atomic::Ordering::Relaxed) {
        crate::watchdog::arm_hw_safe_return(SAFE_HW_BACKSTOP_SECONDS);
    }
    crate::serial_println!(
        "[bootlog-persist] late-flush capture thread spawned (post-boot -> BOOTLOG.TXT in ~15s, BSP-pinned) + SAFE-MODE auto-return safety net + hw-watchdog backstop"
    );
}

/// Write the current bootlog ring into BOOTLOG.TXT's data clusters.
///
/// Split-region layout: the boot log volume far exceeds the RAM ring, so by
/// end-of-boot the ring has wrapped and a naive full rewrite would evict the
/// EARLY lines (ACPI/xHCI/USB bring-up) — the exact part needed to debug a
/// bare-metal input failure. Instead:
///   * **First flush** (at init, right after USB enumeration, before the
///     ring ever wraps) writes the full snapshot from sector 0 and zero-pads
///     — it permanently owns the first half of the file.
///   * **Later flushes** preserve the first half and rewrite only the second
///     half with a marker + the newest ring tail.
/// The reader sees: early transcript, zeros, marker, latest tail.
fn flush_to(dev: &dyn crate::block_io::BlockDevice) {
    let layout_guard = LAYOUT.lock();
    let Some(layout) = layout_guard.as_ref() else {
        return;
    };

    // dump_text = "# AthenaOS boot log …" header + transcript, so the file
    // self-describes (capacity, bytes logged, whether the ring wrapped).
    let log = crate::bootlog::dump_text();
    let bytes = log.as_bytes();

    let spc = layout.sectors_per_cluster as u64;
    let total_sectors = layout.cluster_lbas.len() as u64 * spc;
    let first_flush = FLUSH_COUNT.load(Ordering::Relaxed) == 0;
    let early_sectors = total_sectors / 2;

    let mut tail_buf: Vec<u8> = Vec::new();
    let (start_sector, payload): (u64, &[u8]) = if first_flush {
        let cap = layout.capacity_bytes.min(bytes.len());
        (0, &bytes[..cap])
    } else {
        let region_bytes = (total_sectors - early_sectors) as usize * 512;
        let marker: &[u8] =
            b"\n# === ring tail at last flush (early transcript preserved above) ===\n";
        let keep = region_bytes.saturating_sub(marker.len());
        let tail_start = bytes.len().saturating_sub(keep);
        tail_buf.reserve(marker.len() + (bytes.len() - tail_start));
        tail_buf.extend_from_slice(marker);
        tail_buf.extend_from_slice(&bytes[tail_start..]);
        (early_sectors, &tail_buf[..])
    };
    let write_len = payload.len();

    let mut written = 0usize;
    let mut sectors_done = 0usize;
    let mut failed = false;

    // Walk every sector of every cluster from `start_sector` on. Front
    // sectors of the region carry the payload; once it's exhausted we keep
    // going with all-zero sectors so there is no uninitialized garbage in
    // the tail. 1 MiB = 2048 sectors, cheap to overwrite each flush.
    let mut sector_idx = 0u64;
    'outer: for &cluster_first_lba in &layout.cluster_lbas {
        for s in 0..spc {
            let this_sector = sector_idx;
            sector_idx += 1;
            if this_sector < start_sector {
                continue; // preserved early-transcript region
            }
            let mut sec = [0u8; 512];
            let remaining = write_len.saturating_sub(written);
            if remaining > 0 {
                let n = remaining.min(512);
                sec[..n].copy_from_slice(&payload[written..written + n]);
                written += n;
            }
            let lba = cluster_first_lba + s;
            if dev.write_sector(lba, &sec).is_err() {
                crate::serial_println!(
                    "[bootlog-persist] flush: write failed at lba={} — partial",
                    lba
                );
                failed = true;
                break 'outer;
            }
            sectors_done += 1;
        }
    }

    // Commit the drive's volatile write cache to media. WITHOUT this, a
    // power-cycle (the normal bare-metal test cycle) loses everything we
    // just wrote — the sectors sit in the controller's DRAM and never
    // reach NAND. This is the fix for "BOOTLOG.TXT is still empty after
    // reboot": the writes were happening, but evaporating on power-off.
    let synced = dev.flush_cache();
    FLUSH_COUNT.fetch_add(1, Ordering::Relaxed);
    LAST_FLUSH_BYTES.store(write_len, Ordering::Relaxed);
    if !failed {
        crate::serial_println!(
            "[bootlog-persist] flush OK ({}): {} log bytes + zero-pad -> {} sectors ({} KiB file), cache_sync={}",
            if first_flush {
                "full, early region locked"
            } else {
                "tail region"
            },
            write_len,
            sectors_done,
            layout.capacity_bytes / 1024,
            match synced {
                Ok(()) => "OK",
                Err(e) => e,
            },
        );
    }
}

// ── FAT + directory helpers (all READ-ONLY except the carveout'd data) ──

/// Read the FAT entry for `cluster` — 16-bit on FAT16, 32-bit (masked to
/// 28 bits) on FAT32. None on I/O error.
fn read_fat_entry(
    dev: &dyn crate::block_io::BlockDevice,
    kind: FatKind,
    fat_lba: u64,
    cluster: u32,
) -> Option<u32> {
    let entry_width = match kind {
        FatKind::Fat16 => 2u64,
        FatKind::Fat32 => 4u64,
    };
    let byte_offset = cluster as u64 * entry_width;
    let lba = fat_lba + (byte_offset / 512);
    let off = (byte_offset % 512) as usize;
    let mut sec = [0u8; 512];
    if dev.read_sector(lba, &mut sec).is_err() {
        return None;
    }
    Some(match kind {
        FatKind::Fat16 => u16::from_le_bytes([sec[off], sec[off + 1]]) as u32,
        FatKind::Fat32 => {
            u32::from_le_bytes([sec[off], sec[off + 1], sec[off + 2], sec[off + 3]]) & 0x0FFF_FFFF
        }
    })
}

struct FoundEntry {
    first_cluster: u32,
    size: u32,
}

/// Outcome of scanning one directory sector for an 8.3 name.
enum DirScan {
    Found(FoundEntry),
    /// Hit the 0x00 end-of-directory marker — no more entries anywhere.
    End,
    NotHere,
}

/// Scan one 512-byte directory sector for an 8.3 entry matching
/// `name8`+`ext3`. Skips long-name (0x0F attr), deleted (0xE5), and
/// volume-label entries.
fn scan_dir_sector(sec: &[u8; 512], name8: &[u8; 8], ext3: &[u8; 3]) -> DirScan {
    for slot in 0..16 {
        let off = slot * 32;
        let first = sec[off];
        if first == 0x00 {
            return DirScan::End;
        }
        if first == 0xE5 {
            continue; // deleted
        }
        let attr = sec[off + 11];
        if attr & 0x0F == 0x0F {
            continue; // long-name component
        }
        if attr & 0x08 != 0 {
            continue; // volume label
        }
        if &sec[off..off + 8] == name8 && &sec[off + 8..off + 11] == ext3 {
            let hi = u16::from_le_bytes([sec[off + 20], sec[off + 21]]) as u32;
            let lo = u16::from_le_bytes([sec[off + 26], sec[off + 27]]) as u32;
            let first_cluster = (hi << 16) | lo;
            let size =
                u32::from_le_bytes([sec[off + 28], sec[off + 29], sec[off + 30], sec[off + 31]]);
            return DirScan::Found(FoundEntry {
                first_cluster,
                size,
            });
        }
    }
    DirScan::NotHere
}

/// FAT16: scan the fixed root-directory region (`root_sectors` sectors
/// starting at `root_lba`) for `name8`+`ext3`.
fn find_root_file_fat16(
    dev: &dyn crate::block_io::BlockDevice,
    root_lba: u64,
    root_sectors: u64,
    name8: &[u8; 8],
    ext3: &[u8; 3],
) -> Option<FoundEntry> {
    for s in 0..root_sectors {
        let mut sec = [0u8; 512];
        if dev.read_sector(root_lba + s, &mut sec).is_err() {
            return None;
        }
        match scan_dir_sector(&sec, name8, ext3) {
            DirScan::Found(e) => return Some(e),
            DirScan::End => return None,
            DirScan::NotHere => {}
        }
    }
    None
}

/// FAT32: scan the root directory — following its cluster chain across
/// multiple clusters — for an 8.3 entry matching `name8`+`ext3`.
fn find_root_file_fat32(
    dev: &dyn crate::block_io::BlockDevice,
    fat_lba: u64,
    root_cluster: u32,
    spc: u64,
    data_start: u64,
    name8: &[u8; 8],
    ext3: &[u8; 3],
) -> Option<FoundEntry> {
    let mut cluster = root_cluster;
    let mut root_clusters_walked = 0u32;

    loop {
        if cluster < 2 || cluster >= FAT_EOC {
            break;
        }
        let cluster_first_lba = data_start + (cluster as u64 - 2) * spc;
        for s in 0..spc {
            let mut sec = [0u8; 512];
            if dev.read_sector(cluster_first_lba + s, &mut sec).is_err() {
                return None;
            }
            match scan_dir_sector(&sec, name8, ext3) {
                DirScan::Found(e) => return Some(e),
                DirScan::End => return None,
                DirScan::NotHere => {}
            }
        }
        root_clusters_walked += 1;
        if root_clusters_walked >= MAX_ROOT_CLUSTERS {
            break;
        }
        match read_fat_entry(dev, FatKind::Fat32, fat_lba, cluster) {
            Some(next) => cluster = next,
            None => break,
        }
    }
    None
}

/// Coalesce a list of per-cluster first-LBAs (each `spc` sectors long)
/// into contiguous (start_lba, sector_count) ranges. A freshly created
/// file is usually one range; a fragmented one yields several.
fn coalesce_ranges(cluster_lbas: &[u64], spc: u64) -> Vec<(u64, u64)> {
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    for &lba in cluster_lbas {
        if let Some(last) = ranges.last_mut() {
            // If this cluster begins exactly where the previous range ends,
            // extend it instead of opening a new range.
            if last.0 + last.1 == lba {
                last.1 += spc;
                continue;
            }
        }
        ranges.push((lba, spc));
    }
    ranges
}

pub fn dump_text() -> String {
    alloc::format!(
        "# bootlog persistence (ESP \\BOOTLOG.TXT, data-only overwrite)\ninitialized: {}\nready: {}\nfirst_lba: {}\nsector_count: {}\nflushes: {}\nlast_flush_bytes: {}\ncarveout_writes: {}\n",
        INITIALIZED.load(Ordering::Relaxed),
        READY.load(Ordering::Relaxed),
        FILE_FIRST_LBA.load(Ordering::Relaxed),
        FILE_SECTOR_COUNT.load(Ordering::Relaxed),
        FLUSH_COUNT.load(Ordering::Relaxed),
        LAST_FLUSH_BYTES.load(Ordering::Relaxed),
        crate::block_io::SAFE_MODE_LOG_WRITES.load(Ordering::Relaxed),
    )
}
