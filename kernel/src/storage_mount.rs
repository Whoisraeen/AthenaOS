//! Root filesystem discovery — GPT/MBR partition scan + AthFS mount.
//!
//! Tier-2 deliverable: mount AthFS from a real NVMe partition when present,
//! fall back to virtio-blk whole-disk / in-memory format.
//!
//! Also provides `discover_boot_disk()` / `get_boot_disk()` — a one-time scan
//! that records device type, partition table type, sector count, and whether an
//! ESP and/or a AthFS partition are present. The result is cached in
//! `BOOT_DISK_INFO` and surfaced in `/proc/athena/storage`.

#![allow(dead_code)]

extern crate alloc;

use crate::block_io::{
    self, detect_partition_table, parse_gpt, parse_mbr, PartitionTableType, PartitionType,
};
use alloc::string::String;

// ─── BootDiskInfo ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct BootDiskInfo {
    /// Human-readable driver name: "nvme", "ahci", "virtio-blk", or "unknown".
    pub device_type: &'static str,
    /// Partition layout: "gpt", "mbr", "athfs-raw", or "unknown".
    pub table_type: &'static str,
    /// Total 512-byte sector count reported by the device.
    pub sector_count: u64,
    /// Whether an EFI System Partition (GUID C12A7328-...) was found.
    pub has_esp: bool,
    /// Whether a AthFS partition (type 0xDA / ATHFS GUID) was found.
    pub has_athfs: bool,
}

/// Cached result of the most recent `discover_boot_disk()` call.
static BOOT_DISK_INFO: spin::Mutex<Option<BootDiskInfo>> = spin::Mutex::new(None);

// ─── Device-type inference ────────────────────────────────────────────────────

/// Map a `BlockDeviceInfo.name` prefix to a short driver tag.
///
/// Convention (from drivers that call `set_active_block_device`):
///   nvme0nX  -> "nvme"       (nvme.rs)
///   sdX      -> "ahci"       (ahci.rs)
///   vdX      -> "virtio-blk" (virtio.rs some builds register "vda")
///   default  -> "virtio-blk" (virtio-blk sets ACTIVE_BLOCK_DEVICE directly)
fn infer_device_type() -> &'static str {
    let layer = block_io::BLOCK_LAYER.lock();
    if let Some(bl) = layer.as_ref() {
        for dev in bl.list_devices() {
            let name = dev.name.as_str();
            if name.starts_with("nvme") {
                return "nvme";
            }
            if name.starts_with("sd") {
                return "ahci";
            }
            if name.starts_with("vd") || name.starts_with("virtio") {
                return "virtio-blk";
            }
        }
    }
    // ACTIVE_BLOCK_DEVICE is set by virtio-blk when no BLOCK_LAYER entry
    // exists — the common QEMU path.
    if block_io::ACTIVE_BLOCK_DEVICE.lock().is_some() {
        return "virtio-blk";
    }
    "unknown"
}

// ─── AthFS magic probe ────────────────────────────────────────────────────────

/// Check whether the first 8 bytes of a sector carry the AthFS magic
/// 0x526165465321 ("AthFS!"), stored as a little-endian u64.
fn has_athfs_magic(sector_buf: &[u8]) -> bool {
    if sector_buf.len() < 8 {
        return false;
    }
    const ATHFS_MAGIC: u64 = 0x526165465321;
    let v = u64::from_le_bytes([
        sector_buf[0],
        sector_buf[1],
        sector_buf[2],
        sector_buf[3],
        sector_buf[4],
        sector_buf[5],
        sector_buf[6],
        sector_buf[7],
    ]);
    v == ATHFS_MAGIC
}

// ─── discover_boot_disk ───────────────────────────────────────────────────────

/// Probe `ACTIVE_BLOCK_DEVICE`, read sector 0 (and LBA 1+ for GPT), then
/// classify the disk layout and note whether an ESP / AthFS partition exists.
///
/// The result is stored in `BOOT_DISK_INFO` and also returned.
pub fn discover_boot_disk() -> Option<BootDiskInfo> {
    let device_type = infer_device_type();

    // ── Read sector 0 ────────────────────────────────────────────────────────
    let sector_count;
    let mut lba0 = [0u8; 512];
    {
        let lock = block_io::ACTIVE_BLOCK_DEVICE.lock();
        let dev = lock.as_ref()?;
        sector_count = dev.total_sectors();
        if dev.read_sector(0, &mut lba0).is_err() {
            crate::serial_println!("[storage] discover_boot_disk: failed to read LBA0");
            return None;
        }
    }

    // ── Determine partition table type ───────────────────────────────────────
    let pt = detect_partition_table(&lba0);

    // ── Check for a bare AthFS volume (no partition table) ──────────────────
    if matches!(pt, PartitionTableType::None | PartitionTableType::Unknown) {
        if has_athfs_magic(&lba0) {
            let info = BootDiskInfo {
                device_type,
                table_type: "athfs-raw",
                sector_count,
                has_esp: false,
                has_athfs: true,
            };
            crate::serial_println!(
                "[storage] boot disk: {} {} sectors={} esp=false athfs=true",
                device_type,
                "athfs-raw",
                sector_count
            );
            *BOOT_DISK_INFO.lock() = Some(info);
            return Some(info);
        }
    }

    // ── Parse partitions ─────────────────────────────────────────────────────
    let table_type: &'static str;
    let mut has_esp = false;
    let mut has_athfs = false;

    match pt {
        PartitionTableType::Gpt => {
            table_type = "gpt";

            // Read 34 sectors (standard minimum GPT area: MBR + header + entries).
            let buf_sectors: usize = 34;
            let buf_len = buf_sectors * 512;
            let mut gpt_buf = alloc::vec![0u8; buf_len];

            // Copy already-read LBA0 into position.
            gpt_buf[..512].copy_from_slice(&lba0);

            let read_ok = {
                let lock = block_io::ACTIVE_BLOCK_DEVICE.lock();
                let dev = match lock.as_ref() {
                    Some(d) => d,
                    None => {
                        crate::serial_println!("[storage] discover_boot_disk: device disappeared");
                        return None;
                    }
                };
                let mut ok = true;
                for s in 1..buf_sectors as u64 {
                    let off = s as usize * 512;
                    if dev.read_sector(s, &mut gpt_buf[off..off + 512]).is_err() {
                        ok = false;
                        break;
                    }
                }
                ok
            };

            if read_ok {
                if let Ok(parts) = parse_gpt(&gpt_buf, 512u32) {
                    for p in &parts {
                        match p.partition_type {
                            PartitionType::Efi => has_esp = true,
                            PartitionType::RaeFs => has_athfs = true,
                            _ => {}
                        }
                    }
                }
            } else {
                crate::serial_println!("[storage] discover_boot_disk: GPT sector read incomplete");
            }
        }

        PartitionTableType::Mbr => {
            table_type = "mbr";
            if let Ok(parts) = parse_mbr(&lba0) {
                for p in &parts {
                    match p.partition_type {
                        PartitionType::Efi => has_esp = true,
                        PartitionType::RaeFs => has_athfs = true,
                        _ => {}
                    }
                }
            }
        }

        _ => {
            table_type = "unknown";
        }
    }

    let info = BootDiskInfo {
        device_type,
        table_type,
        sector_count,
        has_esp,
        has_athfs,
    };

    crate::serial_println!(
        "[storage] boot disk: {} {} sectors={} esp={} athfs={}",
        device_type,
        table_type,
        sector_count,
        has_esp,
        has_athfs
    );

    *BOOT_DISK_INFO.lock() = Some(info);
    Some(info)
}

/// Return the cached `BootDiskInfo`, if `discover_boot_disk()` has been called.
pub fn get_boot_disk() -> Option<BootDiskInfo> {
    *BOOT_DISK_INFO.lock()
}

// ─── try_mount_athfs_root ─────────────────────────────────────────────────────

/// Scan sector 0 of the active block device, find a AthFS partition, and mount.
/// Returns `true` if AthFS mounted from a partition.
pub fn try_mount_athfs_root() -> bool {
    crate::serial_println!("[storage] scanning active block device for AthFS partition...");
    let active = block_io::ACTIVE_BLOCK_DEVICE.lock();
    let dev = match active.as_ref() {
        Some(d) => d,
        None => {
            crate::serial_println!("[storage] no block device for partition scan");
            return false;
        }
    };

    let mut lba0 = [0u8; 512];
    if dev.read_sector(0, &mut lba0).is_err() {
        crate::serial_println!("[storage] failed to read LBA0");
        return false;
    }

    let table_type = detect_partition_table(&lba0);
    crate::serial_println!("[storage] partition table: {:?}", table_type);

    let partitions = match table_type {
        PartitionTableType::Gpt => {
            let ss = dev.sector_size() as usize;
            let buf_sectors = 34usize;
            let mut buf = alloc::vec![0u8; ss * buf_sectors];
            // The GPT header lives at LBA 1 and the partition entry array at
            // LBA 2+, so the parser needs LBAs 0..34 in the buffer. The old
            // code read ONLY LBA 0 (the protective MBR), leaving the header
            // and entries zeroed — parse_gpt then saw no "EFI PART" signature
            // and rejected every freshly-installed disk, so the on-disk AthFS
            // root could never mount. Read all 34 sectors like discover does.
            let mut read_ok = true;
            for s in 0..buf_sectors as u64 {
                let off = s as usize * ss;
                if dev.read_sector(s, &mut buf[off..off + ss]).is_err() {
                    read_ok = false;
                    break;
                }
            }
            if !read_ok {
                crate::serial_println!("[storage] GPT sector read incomplete");
                return false;
            }
            match parse_gpt(&buf, ss as u32) {
                Ok(p) => p,
                Err(e) => {
                    crate::serial_println!("[storage] GPT parse failed: {:?}", e);
                    return false;
                }
            }
        }
        PartitionTableType::Mbr => match parse_mbr(&lba0) {
            Ok(p) => p,
            Err(e) => {
                crate::serial_println!("[storage] MBR parse failed: {:?}", e);
                return false;
            }
        },
        _ => {
            crate::serial_println!("[storage] no partition table on active device");
            return false;
        }
    };

    // Release the ACTIVE_BLOCK_DEVICE lock BEFORE the mount loop. AthFS::mount()
    // -> read_block() re-acquires this same lock, and spin::Mutex is NOT
    // reentrant, so holding it here deadlocked the mount of every freshly
    // installed on-disk AthFS volume — the boot froze permanently right after
    // "[storage] candidate part N LBA ... type RaeFs" (the candidate printed,
    // then mount() spun forever waiting on a lock this function still held).
    // The partition table is already parsed into the owned `partitions` Vec, so
    // the device guard is no longer needed for the scan/mount loop below.
    drop(active);

    for part in &partitions {
        let is_athfs = part.partition_type == PartitionType::RaeFs;
        if !is_athfs {
            continue;
        }
        crate::serial_println!(
            "[storage] candidate part {} LBA {} len {} type {:?}",
            part.number,
            part.start_sector,
            part.sector_count,
            part.partition_type,
        );

        *block_io::ROOT_PARTITION_LBA.lock() = part.start_sector;

        if let Some(fs) = crate::athfs::AthFS::mount() {
            crate::serial_println!(
                "[storage] AthFS mounted from partition {} @ LBA {}",
                part.number,
                part.start_sector,
            );
            let _ = fs;
            return true;
        }

        *block_io::ROOT_PARTITION_LBA.lock() = 0;
        crate::serial_println!(
            "[storage] partition {} is not a valid AthFS volume",
            part.number
        );
    }

    false
}

// ─── init / smoketest / dump_text ────────────────────────────────────────────

pub fn init() {
    // Discover and cache boot disk info during subsystem init.
    discover_boot_disk();
    crate::serial_println!("[ OK ] Storage mount helper initialized");
}

/// Log the cached boot disk info. Called from the boot smoketest sequence.
pub fn run_boot_smoketest() {
    match get_boot_disk() {
        Some(info) => {
            crate::serial_println!(
                "[storage] smoketest: device={} table={} sectors={} esp={} athfs={}",
                info.device_type,
                info.table_type,
                info.sector_count,
                info.has_esp,
                info.has_athfs
            );
        }
        None => {
            crate::serial_println!("[storage] smoketest: no boot disk info available");
        }
    }
}

/// Render boot-disk info as text for `/proc/athena/storage`.
pub fn dump_text() -> String {
    let mut out = String::new();
    match get_boot_disk() {
        Some(info) => {
            out.push_str("Boot Disk Info:\n");
            out.push_str(&alloc::format!("  Device Type:  {}\n", info.device_type));
            out.push_str(&alloc::format!("  Table Type:   {}\n", info.table_type));
            out.push_str(&alloc::format!("  Sector Count: {}\n", info.sector_count));
            let size_mb = info.sector_count / 2048;
            out.push_str(&alloc::format!("  Size:         {} MiB\n", size_mb));
            out.push_str(&alloc::format!("  Has ESP:      {}\n", info.has_esp));
            out.push_str(&alloc::format!("  Has AthFS:    {}\n", info.has_athfs));
        }
        None => {
            out.push_str("Boot Disk Info: <not yet discovered>\n");
        }
    }
    out
}
