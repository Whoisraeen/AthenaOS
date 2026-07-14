//! FAT32 read-only ESP locator — native no_std implementation.
//!
//! REDOX_EXTRACTION_MAP R09 / Concept §Install lane (Phase 3.3 + Phase 16).
//! The OS installer needs to read the EFI System Partition to drop
//! `BOOTX64.EFI` into `/EFI/BOOT/` and verify it landed. The obvious
//! third-party path — fatfs 0.3.6 — depends on the abandoned `core_io`
//! crate for its no_std mode and will not build on current nightly.
//!
//! This module is a tight native FAT32 parser: BPB at sector 0, root-dir
//! cluster walk, 8.3 + LFN entry decoding. Today it ships the read side
//! (locate + list root entries). Write support lands when the install lane
//! actually copies files. Per Concept §"working code only, no stubs."
//!
//! What we read on every boot (R10 smoketest):
//!   1. Sector 0 of `block_io::ACTIVE_BLOCK_DEVICE` (the device chosen by
//!      the NVMe / virtio-blk boot-disk picker).
//!   2. If it's a FAT32 VBR (signature 0x55AA, BPB bytes_per_sector 512,
//!      sectors_per_cluster pow2, root_cluster ≥ 2) we walk the root
//!      directory and log the first ~16 entries.
//!   3. Otherwise we report what we did see — usually GPT, since
//!      ACTIVE_BLOCK_DEVICE is the whole disk, not a partition view yet.
//!
//! Per Concept §"the user owns the machine": no panics on bad data. Every
//! parse step returns `Result` and `dump_text` reports honestly.

#![allow(dead_code)]

extern crate alloc;
use crate::block_io::BlockDevice;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// ── BPB / VBR layout (Microsoft FAT spec, §3.x BPB structure) ──────────

#[derive(Debug, Clone, Copy)]
pub struct Fat32Bpb {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub num_fats: u8,
    pub total_sectors_32: u32,
    pub fat_size_32: u32,
    pub root_cluster: u32,
    pub volume_label: [u8; 11],
    pub fs_type: [u8; 8],
}

#[derive(Debug)]
pub enum FatError {
    DeviceRead,
    BadSignature,
    NotFat32,
    BadBpb,
    ClusterOutOfRange,
}

impl core::fmt::Display for FatError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DeviceRead => write!(f, "block device read failed"),
            Self::BadSignature => write!(f, "VBR missing 0x55AA signature"),
            Self::NotFat32 => write!(f, "VBR is not FAT32 (likely GPT/MBR/other)"),
            Self::BadBpb => write!(f, "BPB values out of spec range"),
            Self::ClusterOutOfRange => write!(f, "cluster index past end of data region"),
        }
    }
}

/// Parse a 512-byte VBR. Returns Err if it doesn't look like FAT32.
pub fn parse_vbr(vbr: &[u8]) -> Result<Fat32Bpb, FatError> {
    if vbr.len() < 512 {
        return Err(FatError::BadBpb);
    }
    // Boot signature at 510..512 must be 0x55 0xAA.
    if vbr[510] != 0x55 || vbr[511] != 0xAA {
        return Err(FatError::BadSignature);
    }

    let bytes_per_sector = u16::from_le_bytes([vbr[11], vbr[12]]);
    let sectors_per_cluster = vbr[13];
    let reserved_sectors = u16::from_le_bytes([vbr[14], vbr[15]]);
    let num_fats = vbr[16];
    let total_sectors_16 = u16::from_le_bytes([vbr[19], vbr[20]]);
    let fat_size_16 = u16::from_le_bytes([vbr[22], vbr[23]]);
    let total_sectors_32 = u32::from_le_bytes([vbr[32], vbr[33], vbr[34], vbr[35]]);
    let fat_size_32 = u32::from_le_bytes([vbr[36], vbr[37], vbr[38], vbr[39]]);
    let root_cluster = u32::from_le_bytes([vbr[44], vbr[45], vbr[46], vbr[47]]);

    // FAT32 signature: FATSz16 == 0, TotSec16 == 0, FATSz32 > 0.
    if fat_size_16 != 0 || total_sectors_16 != 0 || fat_size_32 == 0 {
        return Err(FatError::NotFat32);
    }
    if bytes_per_sector != 512
        || sectors_per_cluster == 0
        || !sectors_per_cluster.is_power_of_two()
        || num_fats == 0
        || reserved_sectors == 0
        || root_cluster < 2
        || total_sectors_32 == 0
    {
        return Err(FatError::BadBpb);
    }

    let mut volume_label = [0u8; 11];
    volume_label.copy_from_slice(&vbr[71..82]);
    let mut fs_type = [0u8; 8];
    fs_type.copy_from_slice(&vbr[82..90]);
    // FileSystem field should read "FAT32   " for a genuine FAT32 VBR.
    if &fs_type[..5] != b"FAT32" {
        return Err(FatError::NotFat32);
    }

    Ok(Fat32Bpb {
        bytes_per_sector,
        sectors_per_cluster,
        reserved_sectors,
        num_fats,
        total_sectors_32,
        fat_size_32,
        root_cluster,
        volume_label,
        fs_type,
    })
}

// ── FAT-type-agnostic VBR parsing (FAT16 + FAT32) ──────────────────────
//
// The `bootloader` crate builds the boot image's ESP with the `fatfs`
// crate, which picks the FAT width from the partition size — and a ~27 MiB
// ESP comes out FAT16, not FAT32. The bootlog-persistence path must
// therefore understand both, or it can never find BOOTLOG.TXT on the very
// USB stick the kernel booted from. `parse_vbr` above stays FAT32-only for
// the installer paths; this section is the generalized view.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatKind {
    Fat16,
    Fat32,
}

/// Geometry of a FAT16 or FAT32 volume, normalized so callers can compute
/// the FAT location, root-directory location, and data-region start
/// without caring which width they're on.
#[derive(Debug, Clone, Copy)]
pub struct FatVolume {
    pub kind: FatKind,
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub num_fats: u8,
    /// FAT size in sectors (`fat_size_16` or `fat_size_32`, whichever applies).
    pub fat_size_sectors: u32,
    /// FAT16 only: number of 32-byte root-directory entries (0 on FAT32).
    pub root_entries: u16,
    /// FAT32 only: first cluster of the root directory (0 on FAT16).
    pub root_cluster: u32,
    pub total_sectors: u32,
}

impl FatVolume {
    /// Sectors occupied by the FAT16 fixed root-directory region (0 on FAT32).
    pub fn root_dir_sectors(&self) -> u64 {
        (self.root_entries as u64 * 32).div_ceil(512)
    }

    /// First data sector, relative to the partition start. On FAT16 the
    /// fixed root directory sits between the FATs and the data region.
    pub fn data_start_offset(&self) -> u64 {
        self.reserved_sectors as u64
            + self.num_fats as u64 * self.fat_size_sectors as u64
            + self.root_dir_sectors()
    }
}

/// Parse a 512-byte VBR as either FAT16 or FAT32. FAT12 (cluster count
/// < 4085) is rejected honestly — its 1.5-byte FAT entries are not worth
/// supporting for a boot ESP.
pub fn parse_vbr_any(vbr: &[u8]) -> Result<FatVolume, FatError> {
    if vbr.len() < 512 {
        return Err(FatError::BadBpb);
    }
    if vbr[510] != 0x55 || vbr[511] != 0xAA {
        return Err(FatError::BadSignature);
    }

    let bytes_per_sector = u16::from_le_bytes([vbr[11], vbr[12]]);
    let sectors_per_cluster = vbr[13];
    let reserved_sectors = u16::from_le_bytes([vbr[14], vbr[15]]);
    let num_fats = vbr[16];
    let root_entries = u16::from_le_bytes([vbr[17], vbr[18]]);
    let total_sectors_16 = u16::from_le_bytes([vbr[19], vbr[20]]) as u32;
    let fat_size_16 = u16::from_le_bytes([vbr[22], vbr[23]]) as u32;
    let total_sectors_32 = u32::from_le_bytes([vbr[32], vbr[33], vbr[34], vbr[35]]);
    let fat_size_32 = u32::from_le_bytes([vbr[36], vbr[37], vbr[38], vbr[39]]);
    let root_cluster = u32::from_le_bytes([vbr[44], vbr[45], vbr[46], vbr[47]]);

    if bytes_per_sector != 512
        || sectors_per_cluster == 0
        || !sectors_per_cluster.is_power_of_two()
        || num_fats == 0
        || reserved_sectors == 0
    {
        return Err(FatError::BadBpb);
    }

    let total_sectors = if total_sectors_16 != 0 {
        total_sectors_16
    } else {
        total_sectors_32
    };
    if total_sectors == 0 {
        return Err(FatError::BadBpb);
    }

    if fat_size_16 == 0 && fat_size_32 != 0 {
        // FAT32: FATSz16 == 0, FATSz32 > 0, root is a cluster chain.
        if root_cluster < 2 {
            return Err(FatError::BadBpb);
        }
        return Ok(FatVolume {
            kind: FatKind::Fat32,
            bytes_per_sector,
            sectors_per_cluster,
            reserved_sectors,
            num_fats,
            fat_size_sectors: fat_size_32,
            root_entries: 0,
            root_cluster,
            total_sectors,
        });
    }

    if fat_size_16 == 0 || root_entries == 0 {
        return Err(FatError::BadBpb);
    }

    // FAT16 vs FAT12 is defined by the data-region cluster count alone
    // (Microsoft fatgen103 §"FAT Type Determination").
    let root_dir_sectors = (root_entries as u64 * 32).div_ceil(512);
    let meta = reserved_sectors as u64 + num_fats as u64 * fat_size_16 as u64 + root_dir_sectors;
    let data_sectors = (total_sectors as u64).saturating_sub(meta);
    let cluster_count = data_sectors / sectors_per_cluster as u64;
    if cluster_count < 4085 {
        return Err(FatError::NotFat32); // FAT12 — unsupported
    }
    if cluster_count >= 65525 {
        return Err(FatError::BadBpb); // claims FAT16 geometry but is FAT32-sized
    }

    Ok(FatVolume {
        kind: FatKind::Fat16,
        bytes_per_sector,
        sectors_per_cluster,
        reserved_sectors,
        num_fats,
        fat_size_sectors: fat_size_16,
        root_entries,
        root_cluster: 0,
        total_sectors,
    })
}

/// First sector of a given cluster (LBA within the partition).
pub fn cluster_to_lba(bpb: &Fat32Bpb, cluster: u32) -> Result<u64, FatError> {
    if cluster < 2 {
        return Err(FatError::ClusterOutOfRange);
    }
    let first_data_sector =
        bpb.reserved_sectors as u64 + bpb.num_fats as u64 * bpb.fat_size_32 as u64;
    Ok(first_data_sector + (cluster as u64 - 2) * bpb.sectors_per_cluster as u64)
}

/// FAT entry kind for one 32-byte directory record.
#[derive(Debug, Clone)]
pub enum DirEntryKind {
    EndOfDir,
    Free,
    VolumeLabel(String),
    File {
        name: String,
        ext: String,
        size: u32,
        first_cluster: u32,
    },
    Subdir {
        name: String,
        first_cluster: u32,
    },
    LfnFragment,
}

fn decode_short_name(raw: &[u8]) -> (String, String) {
    let mut name = String::new();
    for &b in &raw[..8] {
        if b == b' ' || b == 0 {
            break;
        }
        // Special case: 0x05 means leading 0xE5 (Japanese FAT quirk).
        let c = if b == 0x05 { 0xE5 } else { b };
        name.push(c as char);
    }
    let mut ext = String::new();
    for &b in &raw[8..11] {
        if b == b' ' || b == 0 {
            break;
        }
        ext.push(b as char);
    }
    (name, ext)
}

fn parse_dir_entry(rec: &[u8; 32]) -> DirEntryKind {
    let first = rec[0];
    if first == 0x00 {
        return DirEntryKind::EndOfDir;
    }
    if first == 0xE5 {
        return DirEntryKind::Free;
    }
    let attr = rec[11];
    if attr == 0x0F {
        return DirEntryKind::LfnFragment;
    }
    let (name, ext) = decode_short_name(rec);
    let cluster_hi = u16::from_le_bytes([rec[20], rec[21]]) as u32;
    let cluster_lo = u16::from_le_bytes([rec[26], rec[27]]) as u32;
    let first_cluster = (cluster_hi << 16) | cluster_lo;
    let size = u32::from_le_bytes([rec[28], rec[29], rec[30], rec[31]]);
    if attr & 0x08 != 0 {
        // Volume label
        let mut label = name.clone();
        if !ext.is_empty() {
            label.push_str(&ext);
        }
        return DirEntryKind::VolumeLabel(label);
    }
    if attr & 0x10 != 0 {
        return DirEntryKind::Subdir {
            name,
            first_cluster,
        };
    }
    DirEntryKind::File {
        name,
        ext,
        size,
        first_cluster,
    }
}

/// Read a whole cluster from the active block device, with an optional
/// `partition_base_lba` offset for partition-relative addressing.
/// Installer source-media override for the READ side of this module. When
/// set, every internal sector read (cluster walks, FAT lookups, VBR/GPT
/// probes, ESP locate) targets this device instead of ACTIVE_BLOCK_DEVICE.
/// The installer uses it to source the real bootloader/kernel payloads from
/// the USB boot stick while the ACTIVE device — the install TARGET (Athena:
/// the internal NVMe) — is being written. The WRITE side (fat32_format,
/// fat32_write_file*, seed GPT, install-seed artifact) never consults it.
/// `None` means every read goes to ACTIVE exactly as before.
static SOURCE_DEVICE: Mutex<Option<alloc::boxed::Box<dyn crate::block_io::BlockDevice>>> =
    Mutex::new(None);

/// Run `f` with all fatfs READ-side sector I/O redirected to `dev` (e.g. the
/// boot USB stick), restoring normal ACTIVE-device reads afterwards.
pub fn with_source_device<R>(
    dev: alloc::boxed::Box<dyn crate::block_io::BlockDevice>,
    f: impl FnOnce() -> R,
) -> R {
    *SOURCE_DEVICE.lock() = Some(dev);
    let r = f();
    *SOURCE_DEVICE.lock() = None;
    r
}

/// Sector size of the current read-side device (override, else ACTIVE).
fn read_side_sector_size() -> Option<usize> {
    {
        let g = SOURCE_DEVICE.lock();
        if let Some(d) = g.as_ref() {
            return Some(d.sector_size());
        }
    }
    let g = crate::block_io::ACTIVE_BLOCK_DEVICE.lock();
    g.as_ref().map(|d| d.sector_size())
}

/// Read one sector from the read-side device (override, else ACTIVE). Every
/// read path in this module funnels through here so the installer's
/// source-media redirection covers the whole read API in one place.
fn read_side_sector(lba: u64, buf: &mut [u8]) -> Result<(), ()> {
    {
        let g = SOURCE_DEVICE.lock();
        if let Some(d) = g.as_ref() {
            return d.read_sector(lba, buf).map_err(|_| ());
        }
    }
    let g = crate::block_io::ACTIVE_BLOCK_DEVICE.lock();
    match g.as_ref() {
        Some(d) => d.read_sector(lba, buf).map_err(|_| ()),
        None => Err(()),
    }
}

fn read_cluster_with_base(
    bpb: &Fat32Bpb,
    cluster: u32,
    partition_base_lba: u64,
) -> Result<Vec<u8>, FatError> {
    let cluster_in_partition = cluster_to_lba(bpb, cluster)?;
    let abs_lba = partition_base_lba.saturating_add(cluster_in_partition);
    let bytes_per_cluster = bpb.sectors_per_cluster as usize * bpb.bytes_per_sector as usize;
    let mut buf = vec![0u8; bytes_per_cluster];

    let sector_size = read_side_sector_size().ok_or(FatError::DeviceRead)?;
    if sector_size != bpb.bytes_per_sector as usize {
        return Err(FatError::BadBpb);
    }

    for i in 0..bpb.sectors_per_cluster as u64 {
        let slice_start = i as usize * sector_size;
        let slice_end = slice_start + sector_size;
        read_side_sector(abs_lba + i, &mut buf[slice_start..slice_end])
            .map_err(|_| FatError::DeviceRead)?;
    }
    Ok(buf)
}

/// Backwards-compatible whole-disk reader (no partition offset).
fn read_cluster(bpb: &Fat32Bpb, cluster: u32) -> Result<Vec<u8>, FatError> {
    read_cluster_with_base(bpb, cluster, 0)
}

/// Walk the first cluster of the root directory and decode entries.
/// Honest scope: doesn't follow the FAT chain to subsequent clusters yet
/// — one cluster of the root is enough to find /EFI/ on a real ESP.
pub fn list_root_first_cluster(bpb: &Fat32Bpb) -> Result<Vec<DirEntryKind>, FatError> {
    list_root_first_cluster_at(bpb, 0)
}

/// Same as `list_root_first_cluster`, but for FAT32 living inside a
/// partition that starts at `partition_base_lba`.
pub fn list_root_first_cluster_at(
    bpb: &Fat32Bpb,
    partition_base_lba: u64,
) -> Result<Vec<DirEntryKind>, FatError> {
    let data = read_cluster_with_base(bpb, bpb.root_cluster, partition_base_lba)?;
    let mut out = Vec::new();
    for chunk in data.chunks_exact(32) {
        let mut rec = [0u8; 32];
        rec.copy_from_slice(chunk);
        let kind = parse_dir_entry(&rec);
        if matches!(kind, DirEntryKind::EndOfDir) {
            out.push(kind);
            break;
        }
        out.push(kind);
    }
    Ok(out)
}

// ── Source-media file reader (installer payload sourcing, Phase 3.1) ────
//
// The installer reads the real kernel/bootloader from the live boot media's
// ESP instead of fabricating placeholders. These helpers descend the FAT32
// directory tree on `ACTIVE_BLOCK_DEVICE` (which on QEMU IS the boot ESP) and
// return a file's bytes by following its cluster chain.

/// Resolve the source ESP's base LBA and parse its FAT32 BPB. Returns
/// `(base_lba, bpb)` or `None` if no readable FAT32 ESP is present.
fn esp_base_and_bpb() -> Option<(u64, Fat32Bpb)> {
    let base = match find_esp_partition() {
        PartitionScan::GptEspFound { start_lba, .. } => start_lba,
        PartitionScan::MbrFat32Found { start_lba, .. } => start_lba,
        // NoPartitionTable, OR a superfloppy / raw-FAT32 device whose FAT32 VBR
        // sits at sector 0 with no partition table (partitionless USB media,
        // QEMU VVFAT). Such a VBR's 0x55AA makes detect_partition_table report
        // a bogus MBR-with-no-FAT32, so fall through to sector 0 here. The
        // `parse_vbr` below is self-validating: a sector 0 that is NOT a real
        // FAT32 VBR (e.g. a protective MBR) simply yields `None`, so this never
        // false-positives on a genuinely partitioned disk.
        _ => 0,
    };
    let mut vbr = [0u8; 512];
    read_side_sector(base, &mut vbr).ok()?;
    let bpb = parse_vbr(&vbr).ok()?;
    Some((base, bpb))
}

/// Read the FAT entry (next cluster, masked to 28 bits) for `cluster`.
fn fat_next_cluster(bpb: &Fat32Bpb, base: u64, cluster: u32) -> Option<u32> {
    let fat_offset = cluster as u64 * 4;
    let lba = base + bpb.reserved_sectors as u64 + fat_offset / 512;
    let off = (fat_offset % 512) as usize;
    let mut sec = [0u8; 512];
    read_side_sector(lba, &mut sec).ok()?;
    Some(u32::from_le_bytes([sec[off], sec[off + 1], sec[off + 2], sec[off + 3]]) & 0x0FFF_FFFF)
}

/// List every directory entry in the directory whose data begins at
/// `first_cluster` (use `0` for the root dir), following the FAT chain.
fn list_dir_chain(bpb: &Fat32Bpb, base: u64, first_cluster: u32) -> Vec<DirEntryKind> {
    let mut out = Vec::new();
    let mut cluster = if first_cluster == 0 {
        bpb.root_cluster
    } else {
        first_cluster
    };
    let mut guard = 0;
    while cluster >= 2 && cluster < 0x0FFF_FFF8 && guard < 4096 {
        guard += 1;
        let data = match read_cluster_with_base(bpb, cluster, base) {
            Ok(d) => d,
            Err(_) => break,
        };
        for chunk in data.chunks_exact(32) {
            let mut rec = [0u8; 32];
            rec.copy_from_slice(chunk);
            let kind = parse_dir_entry(&rec);
            if matches!(kind, DirEntryKind::EndOfDir) {
                return out;
            }
            out.push(kind);
        }
        match fat_next_cluster(bpb, base, cluster) {
            Some(n) => cluster = n,
            None => break,
        }
    }
    out
}

/// Read a file from the source ESP by descending `path` (dir names) then
/// matching `name`.`ext` (8.3, case-insensitive). Returns the file's bytes
/// (truncated to its directory-entry size), or `None` if absent/unreadable.
/// This is the installer's "read source payload from the live media" path.
pub fn read_esp_file(path: &[&str], name: &str, ext: &str) -> Option<Vec<u8>> {
    let (base, bpb) = esp_base_and_bpb()?;

    // Descend the directory chain.
    let mut dir_cluster = 0u32; // 0 = root
    for seg in path {
        let entries = list_dir_chain(&bpb, base, dir_cluster);
        let mut next = None;
        for e in entries {
            if let DirEntryKind::Subdir {
                name: n,
                first_cluster,
            } = e
            {
                if n.eq_ignore_ascii_case(seg) {
                    next = Some(first_cluster);
                    break;
                }
            }
        }
        dir_cluster = next?;
    }

    // Find the file entry in the final directory.
    let (mut cluster, size) = {
        let entries = list_dir_chain(&bpb, base, dir_cluster);
        let mut r = None;
        for e in entries {
            if let DirEntryKind::File {
                name: n,
                ext: x,
                size,
                first_cluster,
            } = e
            {
                if n.eq_ignore_ascii_case(name) && x.eq_ignore_ascii_case(ext) {
                    r = Some((first_cluster, size));
                    break;
                }
            }
        }
        r?
    };

    // Follow the cluster chain reading data until we have `size` bytes.
    let mut out = Vec::with_capacity(size as usize);
    let mut guard = 0;
    while cluster >= 2 && cluster < 0x0FFF_FFF8 && out.len() < size as usize && guard < 1_000_000 {
        guard += 1;
        let data = read_cluster_with_base(&bpb, cluster, base).ok()?;
        out.extend_from_slice(&data);
        cluster = fat_next_cluster(&bpb, base, cluster)?;
    }
    out.truncate(size as usize);
    Some(out)
}

/// Locate a file by LONG name: descend `path` (subdirs matched by short name),
/// then raw-walk the final directory reconstructing LFN runs. Returns the
/// file's `(first_cluster, size)` without reading any data — cheap enough for a
/// boot-time existence check. Shared by [`read_esp_file_long`] and
/// [`stat_esp_file_long`].
fn locate_esp_file_long(path: &[&str], long_name: &str) -> Option<(u64, Fat32Bpb, u32, u32)> {
    let (base, bpb) = esp_base_and_bpb()?;

    let mut dir_cluster = 0u32; // 0 = root
    for seg in path {
        let entries = list_dir_chain(&bpb, base, dir_cluster);
        let mut next = None;
        for e in entries {
            if let DirEntryKind::Subdir {
                name: n,
                first_cluster,
            } = e
            {
                if n.eq_ignore_ascii_case(seg) {
                    next = Some(first_cluster);
                    break;
                }
            }
        }
        dir_cluster = next?;
    }

    let start = if dir_cluster == 0 {
        bpb.root_cluster
    } else {
        dir_cluster
    };
    let mut cluster = start;
    let mut guard = 0;
    let mut name_buf = [0u16; 260];
    let mut max_len = 0usize;
    'walk: while cluster >= 2 && cluster < 0x0FFF_FFF8 && guard < 4096 {
        guard += 1;
        let data = read_cluster_with_base(&bpb, cluster, base).ok()?;
        for rec in data.chunks_exact(32) {
            let first = rec[0];
            if first == 0x00 {
                break 'walk;
            }
            if first == 0xE5 {
                max_len = 0;
                continue;
            }
            let attr = rec[11];
            if attr == 0x0F {
                let seq = (rec[0] & 0x1F) as usize;
                if seq == 0 || seq > 20 {
                    max_len = 0;
                    continue;
                }
                for (i, &off) in LFN_CHAR_OFFSETS.iter().enumerate() {
                    let v = u16::from_le_bytes([rec[off], rec[off + 1]]);
                    let pos = (seq - 1) * 13 + i;
                    if v != 0x0000 && v != 0xFFFF && pos < name_buf.len() {
                        name_buf[pos] = v;
                        max_len = max_len.max(pos + 1);
                    }
                }
                continue;
            }
            // Regular file (not dir 0x10, not volume 0x08): match the LFN run.
            if attr & 0x18 == 0 && max_len > 0 {
                let mut s = String::new();
                for &c in name_buf.iter().take(max_len) {
                    s.push((c as u8) as char);
                }
                if s.eq_ignore_ascii_case(long_name) {
                    let chi = u16::from_le_bytes([rec[20], rec[21]]) as u32;
                    let clo = u16::from_le_bytes([rec[26], rec[27]]) as u32;
                    let size = u32::from_le_bytes([rec[28], rec[29], rec[30], rec[31]]);
                    return Some((base, bpb, (chi << 16) | clo, size));
                }
            }
            max_len = 0;
        }
        match fat_next_cluster(&bpb, base, cluster) {
            Some(n) => cluster = n,
            None => break,
        }
    }
    None
}

/// Existence + size of a long-named ESP file, via the directory walk only (no
/// data read). Used by the install smoketest to prove the bootable kernel
/// (`kernel-x86_64`) is locatable on a real bootloader-crate ESP without the
/// cost of streaming all ~26 MiB at boot. MasterChecklist Phase 3.5.
pub fn stat_esp_file_long(path: &[&str], long_name: &str) -> Option<u32> {
    locate_esp_file_long(path, long_name).map(|(_, _, _, size)| size)
}

/// Like [`read_esp_file`], but matches the FINAL file by its reconstructed LONG
/// name (LFN). The bootloader's `kernel-x86_64` has no stable 8.3 alias, so the
/// installer sources it by long name. MasterChecklist Phase 3.5.
pub fn read_esp_file_long(path: &[&str], long_name: &str) -> Option<Vec<u8>> {
    let (base, bpb, first_cluster, size) = locate_esp_file_long(path, long_name)?;
    let mut cluster = first_cluster;
    let mut out = Vec::with_capacity(size as usize);
    let mut guard = 0;
    while cluster >= 2 && cluster < 0x0FFF_FFF8 && out.len() < size as usize && guard < 1_000_000 {
        guard += 1;
        let data = read_cluster_with_base(&bpb, cluster, base).ok()?;
        out.extend_from_slice(&data);
        cluster = fat_next_cluster(&bpb, base, cluster)?;
    }
    out.truncate(size as usize);
    Some(out)
}

// ── Partition-table walk to locate the ESP ─────────────────────────────

/// Outcome of the partition scan on the active block device.
#[derive(Debug, Clone)]
pub enum PartitionScan {
    /// GPT detected, ESP-type partition found — start LBA + sector count.
    GptEspFound {
        start_lba: u64,
        sector_count: u64,
        name: String,
    },
    /// GPT detected but no partition with the EFI System type GUID.
    GptNoEsp { partition_count: usize },
    /// MBR detected, FAT partition found (FAT32 0x0B/0x0C, FAT16
    /// 0x04/0x06/0x0E, or ESP 0xEF).
    MbrFat32Found { start_lba: u64, sector_count: u64 },
    /// MBR detected, no FAT32 entry.
    MbrNoFat32 { partition_count: usize },
    /// Sector 0 has no recognized partition table — disk-as-VBR or junk.
    NoPartitionTable,
    /// Couldn't even read the sectors needed.
    ReadError(&'static str),
}

/// Read sector 0 (+ GPT header + first GPT entry array sector when
/// applicable) and try to locate an ESP partition. Today we only need
/// the first sector of the entry array — enough to cover the first
/// `512 / 128 = 4` partitions, which is where the ESP lives on every
/// sane install.
pub fn find_esp_partition() -> PartitionScan {
    // Honor the installer's source-media override so the SOURCE ESP (the
    // boot USB stick) is the one located while payloads are being read.
    {
        let g = SOURCE_DEVICE.lock();
        if let Some(d) = g.as_ref() {
            return find_esp_partition_on(d.as_ref());
        }
    }
    let dev_guard = crate::block_io::ACTIVE_BLOCK_DEVICE.lock();
    match dev_guard.as_ref() {
        None => PartitionScan::ReadError("no active block device"),
        Some(d) => find_esp_partition_on(d.as_ref()),
    }
}

/// Device-parameterized ESP scan (see [`find_esp_partition`]). Lets callers
/// probe a NON-active block device — e.g. the bootlog persistence path looking
/// for a `BOOTLOG.TXT`-bearing ESP on a USB stick while the NVMe stays active.
pub fn find_esp_partition_on(dev: &dyn crate::block_io::BlockDevice) -> PartitionScan {
    let sector_size = dev.sector_size();
    if sector_size != 512 {
        return PartitionScan::ReadError("unsupported sector size != 512");
    }

    // Read sector 0 (MBR + protective MBR) and sector 1 (GPT header on a
    // GPT disk). We assemble them into a flat buffer the block_io
    // parser can consume — it expects (sector_size * N + entries) bytes.
    let mut head = vec![0u8; sector_size * 2];
    if dev.read_sector(0, &mut head[0..sector_size]).is_err() {
        return PartitionScan::ReadError("sector 0 read failed");
    }

    let table = crate::block_io::detect_partition_table(&head[0..sector_size]);
    match table {
        crate::block_io::PartitionTableType::Gpt => {
            // Need to read the GPT header (sector 1) AND the entry array
            // sector (whose LBA the header points to). Read sector 1 first.
            if dev
                .read_sector(1, &mut head[sector_size..2 * sector_size])
                .is_err()
            {
                return PartitionScan::ReadError("GPT header read failed");
            }
            let hdr = &head[sector_size..sector_size * 2];
            let partition_entry_lba = u64::from_le_bytes([
                hdr[72], hdr[73], hdr[74], hdr[75], hdr[76], hdr[77], hdr[78], hdr[79],
            ]);
            // Grow head to include the entry array sector at the right offset.
            let entry_offset_bytes = partition_entry_lba as usize * sector_size;
            let needed = entry_offset_bytes + sector_size;
            if needed > head.len() {
                head.resize(needed, 0);
            }
            if dev
                .read_sector(
                    partition_entry_lba,
                    &mut head[entry_offset_bytes..entry_offset_bytes + sector_size],
                )
                .is_err()
            {
                return PartitionScan::ReadError("GPT entry array read failed");
            }
            match crate::block_io::parse_gpt(&head, sector_size as u32) {
                Err(_) => PartitionScan::NoPartitionTable,
                Ok(parts) => {
                    for p in &parts {
                        if matches!(p.partition_type, crate::block_io::PartitionType::Efi) {
                            return PartitionScan::GptEspFound {
                                start_lba: p.start_sector,
                                sector_count: p.sector_count,
                                name: p.name.clone(),
                            };
                        }
                    }
                    PartitionScan::GptNoEsp {
                        partition_count: parts.len(),
                    }
                }
            }
        }
        crate::block_io::PartitionTableType::Mbr => {
            match crate::block_io::parse_mbr(&head[0..sector_size]) {
                Err(_) => PartitionScan::NoPartitionTable,
                Ok(parts) => {
                    for p in &parts {
                        if matches!(
                            p.partition_type,
                            crate::block_io::PartitionType::WindowsFat32
                                | crate::block_io::PartitionType::WindowsFat16
                                | crate::block_io::PartitionType::Efi
                        ) {
                            return PartitionScan::MbrFat32Found {
                                start_lba: p.start_sector,
                                sector_count: p.sector_count,
                            };
                        }
                    }
                    PartitionScan::MbrNoFat32 {
                        partition_count: parts.len(),
                    }
                }
            }
        }
        _ => PartitionScan::NoPartitionTable,
    }
}

/// Read the FAT32 VBR at a given partition start LBA.
fn read_vbr_at(start_lba: u64) -> Result<Fat32Bpb, FatError> {
    let mut sector = vec![0u8; 512];
    read_side_sector(start_lba, &mut sector).map_err(|_| FatError::DeviceRead)?;
    parse_vbr(&sector)
}

// ── R10 smoketest + dump + counters ────────────────────────────────────

static MOUNT_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
static MOUNT_OK: AtomicU64 = AtomicU64::new(0);
static INSTALL_SEED_OK: AtomicU64 = AtomicU64::new(0);
static INSTALL_SEED_LBA: AtomicU64 = AtomicU64::new(0);
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);
static LAST_BPB: Mutex<Option<Fat32Bpb>> = Mutex::new(None);
static LAST_ENTRIES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static LAST_SCAN: Mutex<Option<String>> = Mutex::new(None);

pub fn init() {
    crate::serial_println!("[ OK ] FAT32 ESP locator initialized (native no_std parser)");
}

pub fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg() & 0xEDB8_8320;
            crc = (crc >> 1) ^ mask;
        }
    }
    !crc
}

/// Seed a spec-correct GPT with an ESP and (when the disk has room) a AthFS
/// root partition. Returns `(esp_start_lba, athfs_start_lba)`; `athfs_start`
/// is 0 when the disk is too small for a AthFS root (QEMU smoke disk).
pub fn seed_minimal_gpt_with_esp() -> Option<(u64, u64)> {
    const EFI_GUID: [u8; 16] = [
        0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9,
        0x3B,
    ];
    let dev_guard = crate::block_io::ACTIVE_BLOCK_DEVICE.lock();
    let dev = dev_guard.as_ref()?;
    if dev.sector_size() != 512 {
        return None;
    }
    let disk_last_lba = dev.total_sectors().saturating_sub(1);
    let first_usable_lba = 34u64;
    let last_usable_lba = disk_last_lba.saturating_sub(33);
    // Prefer standard 1 MiB alignment (LBA 2048). Very small test disks
    // (like our 1 MiB virtio smoke image) cannot fit that, so fall back.
    let preferred_start = 2048u64.max(first_usable_lba);
    let fallback_start = 64u64.max(first_usable_lba);
    let start_lba = if preferred_start < last_usable_lba {
        preferred_start
    } else {
        fallback_start
    };
    // ESP size: a REAL install must hold BOOTX64.EFI + the ~26 MiB
    // kernel-x86_64 (initramfs baked in) with headroom — 128 MiB. The old
    // fixed 2048 sectors (1 MiB) was sized for the QEMU smoke disk and
    // could never fit the kernel; keep that as the small-disk fallback so
    // the QEMU path is unchanged.
    const ESP_SECTORS_REAL: u64 = 262_144; // 128 MiB
    const ESP_SECTORS_SMALL: u64 = 2048; // 1 MiB (QEMU scratch disk)
    let esp_len = if start_lba.saturating_add(ESP_SECTORS_REAL) <= last_usable_lba {
        ESP_SECTORS_REAL
    } else {
        ESP_SECTORS_SMALL
    };
    let end_lba = start_lba
        .saturating_add(esp_len.saturating_sub(1))
        .min(last_usable_lba);
    if end_lba <= start_lba {
        return None;
    }
    crate::serial_println!(
        "[fatfs] install seed alignment: start_lba={} mode={}",
        start_lba,
        if start_lba == preferred_start {
            "1MiB"
        } else {
            "fallback"
        }
    );

    let mut mbr = [0u8; 512];
    mbr[446 + 4] = 0xEE; // Protective MBR
    mbr[446 + 8..446 + 12].copy_from_slice(&(1u32).to_le_bytes());
    mbr[446 + 12..446 + 16].copy_from_slice(&(0xFFFF_FFFFu32).to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;

    // ── Partition entry array: 128 entries × 128 bytes = 16 384 bytes (32
    // sectors). The header declares 128 entries, so the array CRC MUST cover
    // the full 16 KiB. The old code CRC'd only the first 512 bytes while the
    // header still claimed 128 entries — a spec violation that makes UEFI
    // firmware reject the table and refuse to boot the installed disk.
    const ENTRY_COUNT: u32 = 128;
    const ENTRY_SIZE: u32 = 128;
    const ARRAY_BYTES: usize = (ENTRY_COUNT * ENTRY_SIZE) as usize; // 16384
    const ARRAY_SECTORS: u64 = (ARRAY_BYTES / 512) as u64; // 32
    let mut entries = vec![0u8; ARRAY_BYTES];

    // Entry 0 — EFI System Partition.
    entries[0..16].copy_from_slice(&EFI_GUID);
    entries[16..32].copy_from_slice(&[
        0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe, 0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc,
        0xfe,
    ]);
    entries[32..40].copy_from_slice(&start_lba.to_le_bytes());
    entries[40..48].copy_from_slice(&end_lba.to_le_bytes());
    {
        let esp_name = [b'E' as u16, b'S' as u16, b'P' as u16, 0];
        for (i, ch) in esp_name.iter().enumerate() {
            let o = 56 + i * 2;
            entries[o..o + 2].copy_from_slice(&ch.to_le_bytes());
        }
    }

    // Entry 1 — AthFS root, when the disk is large enough to hold one. AthFS
    // begins at the next 1 MiB boundary after the ESP and runs to the last
    // usable LBA. Require ≥ ~2 MiB of root (4096 sectors) before bothering —
    // the tiny QEMU smoke disk gets ESP-only and reports athfs_start = 0.
    // GUID must match `block_io::PartitionType::RaeFs` (52414546-534F-2147-…).
    const ATHFS_GUID: [u8; 16] = [
        0x46, 0x45, 0x41, 0x52, 0x4F, 0x53, 0x47, 0x21, 0x41, 0x52, 0x45, 0x45, 0x4E, 0x4F, 0x53,
        0x21,
    ];
    let athfs_start = {
        let after_esp = end_lba.saturating_add(1);
        (after_esp + 2047) & !2047u64 // round up to the next 1 MiB boundary
    };
    let have_athfs = last_usable_lba > athfs_start.saturating_add(4096);
    if have_athfs {
        let off = ENTRY_SIZE as usize; // entry 1 begins at byte 128
        entries[off..off + 16].copy_from_slice(&ATHFS_GUID);
        // Unique partition GUID (arbitrary but stable across re-seeds).
        entries[off + 16..off + 32].copy_from_slice(&[
            0x52, 0x41, 0x45, 0x46, 0x53, 0x21, 0x52, 0x4F, 0x4F, 0x54, 0x00, 0x01, 0x02, 0x03,
            0x04, 0x05,
        ]);
        entries[off + 32..off + 40].copy_from_slice(&athfs_start.to_le_bytes());
        entries[off + 40..off + 48].copy_from_slice(&last_usable_lba.to_le_bytes());
        let root_name = [
            b'R' as u16,
            b'a' as u16,
            b'e' as u16,
            b'e' as u16,
            b'n' as u16,
            b'O' as u16,
            b'S' as u16,
            0,
        ];
        for (i, ch) in root_name.iter().enumerate() {
            let o = off + 56 + i * 2;
            entries[o..o + 2].copy_from_slice(&ch.to_le_bytes());
        }
    }

    // GPT requires CRC32 over the entire partition-entry array.
    let entries_crc = crc32_ieee(&entries);

    // Build a primary or backup GPT header that shares the same entry CRC.
    let make_header = |my_lba: u64, alt_lba: u64, entries_lba: u64| -> [u8; 512] {
        let mut h = [0u8; 512];
        h[0..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes()); // revision 1.0
        h[12..16].copy_from_slice(&92u32.to_le_bytes()); // header size
        h[24..32].copy_from_slice(&my_lba.to_le_bytes()); // this header's LBA
        h[32..40].copy_from_slice(&alt_lba.to_le_bytes()); // alternate header LBA
        h[40..48].copy_from_slice(&first_usable_lba.to_le_bytes());
        h[48..56].copy_from_slice(&last_usable_lba.to_le_bytes());
        // Disk GUID (bytes 56..72) — stable, arbitrary.
        h[56..72].copy_from_slice(&[
            0x52, 0x41, 0x45, 0x45, 0x4E, 0x4F, 0x53, 0x2D, 0x44, 0x49, 0x53, 0x4B, 0x47, 0x55,
            0x49, 0x44,
        ]);
        h[72..80].copy_from_slice(&entries_lba.to_le_bytes());
        h[80..84].copy_from_slice(&ENTRY_COUNT.to_le_bytes());
        h[84..88].copy_from_slice(&ENTRY_SIZE.to_le_bytes());
        h[88..92].copy_from_slice(&entries_crc.to_le_bytes());
        h[16..20].copy_from_slice(&0u32.to_le_bytes()); // zero CRC field first
        let hc = crc32_ieee(&h[..92]);
        h[16..20].copy_from_slice(&hc.to_le_bytes());
        h
    };

    // Backup entry array occupies the 32 sectors immediately before the
    // backup header (which lives at the very last LBA). last_usable_lba was
    // computed as disk_last_lba - 33, so this never overlaps usable space.
    let primary_entries_lba = 2u64;
    let backup_entries_lba = disk_last_lba.saturating_sub(ARRAY_SECTORS);
    let primary_hdr = make_header(1, disk_last_lba, primary_entries_lba);
    let backup_hdr = make_header(disk_last_lba, 1, backup_entries_lba);

    // ── Write everything: protective MBR, primary header, primary + backup
    // entry arrays, backup header. A failure at any step aborts the seed.
    if dev.write_sector(0, &mbr).is_err() {
        return None;
    }
    if dev.write_sector(1, &primary_hdr).is_err() {
        return None;
    }
    for s in 0..ARRAY_SECTORS {
        let mut sec = [0u8; 512];
        let base = (s as usize) * 512;
        sec.copy_from_slice(&entries[base..base + 512]);
        if dev.write_sector(primary_entries_lba + s, &sec).is_err() {
            return None;
        }
        if dev.write_sector(backup_entries_lba + s, &sec).is_err() {
            return None;
        }
    }
    if dev.write_sector(disk_last_lba, &backup_hdr).is_err() {
        return None;
    }

    crate::serial_println!(
        "[fatfs] install seed GPT: esp={}..{} athfs_start={}{} entries_crc=0x{:08x}",
        start_lba,
        end_lba,
        athfs_start,
        if have_athfs { "" } else { " (none)" },
        entries_crc
    );

    Some((start_lba, if have_athfs { athfs_start } else { 0 }))
}

/// The AthFS partition type GUID (matches `block_io::PartitionType::RaeFs`).
pub const ATHFS_TYPE_GUID: [u8; 16] = [
    0x46, 0x45, 0x41, 0x52, 0x4F, 0x53, 0x47, 0x21, 0x41, 0x52, 0x45, 0x45, 0x4E, 0x4F, 0x53, 0x21,
];

/// Non-destructively add a AthFS partition to the EXISTING GPT on `dev`
/// (dual-boot "install alongside"). Reads the current primary GPT, fills the
/// first FREE entry slot with a AthFS partition spanning `[start_lba, end_lba]`,
/// recomputes the partition-array + header CRC32s, and rewrites BOTH the primary
/// and backup GPT — leaving every existing partition entry byte-for-byte intact.
///
/// Returns the slot index used, or `None` on ANY validation failure. All
/// validation (header magic, sane geometry, no overlap with existing
/// partitions, a free slot exists) happens BEFORE the first write, so a refused
/// plan never leaves a half-written / corrupt table. Every write routes through
/// `safe_mode_guard_write`, so on a `--safe` image this is a logged dry run.
pub fn add_gpt_partition_on(
    dev: &dyn crate::block_io::BlockDevice,
    start_lba: u64,
    end_lba: u64,
) -> Option<usize> {
    if dev.sector_size() != 512 || end_lba <= start_lba {
        return None;
    }

    // ── Read + validate the primary header (LBA 1) ──────────────────────────
    let mut phdr = [0u8; 512];
    dev.read_sector(1, &mut phdr).ok()?;
    if &phdr[0..8] != b"EFI PART" {
        return None;
    }
    let hdr_size = u32::from_le_bytes(phdr[12..16].try_into().ok()?) as usize;
    if !(92..=512).contains(&hdr_size) {
        return None;
    }
    let alt_lba = u64::from_le_bytes(phdr[32..40].try_into().ok()?);
    let first_usable = u64::from_le_bytes(phdr[40..48].try_into().ok()?);
    let last_usable = u64::from_le_bytes(phdr[48..56].try_into().ok()?);
    let entry_lba = u64::from_le_bytes(phdr[72..80].try_into().ok()?);
    let entry_count = u32::from_le_bytes(phdr[80..84].try_into().ok()?) as usize;
    let entry_size = u32::from_le_bytes(phdr[84..88].try_into().ok()?) as usize;
    if entry_count == 0 || entry_count > 256 || entry_size < 128 || entry_size % 8 != 0 {
        return None;
    }
    if start_lba < first_usable || end_lba > last_usable || entry_lba < 2 {
        return None;
    }

    // ── Read the full primary entry array ───────────────────────────────────
    let array_bytes = entry_count * entry_size;
    let array_sectors = array_bytes.div_ceil(512);
    let mut entries = vec![0u8; array_sectors * 512];
    for s in 0..array_sectors {
        let mut sec = [0u8; 512];
        dev.read_sector(entry_lba + s as u64, &mut sec).ok()?;
        entries[s * 512..s * 512 + 512].copy_from_slice(&sec);
    }

    // ── Find the first free slot; refuse if the new range overlaps any
    //    existing partition (defence in depth — plan_layout already gapped it) ─
    let mut free_slot: Option<usize> = None;
    for i in 0..entry_count {
        let off = i * entry_size;
        if entries[off..off + 16].iter().all(|&b| b == 0) {
            if free_slot.is_none() {
                free_slot = Some(i);
            }
            continue;
        }
        let s = u64::from_le_bytes(entries[off + 32..off + 40].try_into().ok()?);
        let e = u64::from_le_bytes(entries[off + 40..off + 48].try_into().ok()?);
        if start_lba <= e && s <= end_lba {
            return None; // overlaps an existing partition — never clobber
        }
    }
    let slot = free_slot?;

    // ── Write the AthFS entry into the free slot ────────────────────────────
    let off = slot * entry_size;
    entries[off..off + 16].copy_from_slice(&ATHFS_TYPE_GUID);
    entries[off + 16..off + 32].copy_from_slice(&[
        0x52, 0x41, 0x45, 0x46, 0x53, 0x21, 0x52, 0x4F, 0x4F, 0x54, 0x00, 0x0D, 0x0A, 0x0B, 0x0C,
        0x0D,
    ]);
    entries[off + 32..off + 40].copy_from_slice(&start_lba.to_le_bytes());
    entries[off + 40..off + 48].copy_from_slice(&end_lba.to_le_bytes());
    for (i, ch) in [b'R', b'a', b'e', b'e', b'n', b'O', b'S']
        .iter()
        .map(|&c| c as u16)
        .chain(core::iter::once(0u16))
        .enumerate()
    {
        let o = off + 56 + i * 2;
        if o + 2 <= off + entry_size {
            entries[o..o + 2].copy_from_slice(&ch.to_le_bytes());
        }
    }

    // GPT array CRC is over exactly entry_count * entry_size bytes.
    let crc = crc32_ieee(&entries[..array_bytes]);

    // Patch a header in place: stamp the new array CRC, then recompute the
    // header self-CRC over its declared size.
    let patch_header = |h: &mut [u8; 512]| {
        h[88..92].copy_from_slice(&crc.to_le_bytes());
        h[16..20].copy_from_slice(&0u32.to_le_bytes());
        let hc = crc32_ieee(&h[..hdr_size]);
        h[16..20].copy_from_slice(&hc.to_le_bytes());
    };

    // ── Write primary: entry array, then header ─────────────────────────────
    for s in 0..array_sectors {
        let mut sec = [0u8; 512];
        sec.copy_from_slice(&entries[s * 512..s * 512 + 512]);
        if dev.write_sector(entry_lba + s as u64, &sec).is_err() {
            return None;
        }
    }
    patch_header(&mut phdr);
    if dev.write_sector(1, &phdr).is_err() {
        return None;
    }

    // ── Mirror to the backup GPT (alt header LBA), if present ────────────────
    if alt_lba > 1 {
        let mut bhdr = [0u8; 512];
        if dev.read_sector(alt_lba, &mut bhdr).is_ok() && &bhdr[0..8] == b"EFI PART" {
            if let Ok(bytes) = bhdr[72..80].try_into() {
                let b_entry_lba = u64::from_le_bytes(bytes);
                if b_entry_lba >= 2 {
                    for s in 0..array_sectors {
                        let mut sec = [0u8; 512];
                        sec.copy_from_slice(&entries[s * 512..s * 512 + 512]);
                        let _ = dev.write_sector(b_entry_lba + s as u64, &sec);
                    }
                    patch_header(&mut bhdr);
                    let _ = dev.write_sector(alt_lba, &bhdr);
                }
            }
        }
    }

    crate::serial_println!(
        "[fatfs] add_gpt_partition: AthFS in slot {} at LBA {}..{} (array_crc=0x{:08x}, {} existing entries preserved)",
        slot,
        start_lba,
        end_lba,
        crc,
        (0..entry_count)
            .filter(|&i| !entries[i * entry_size..i * entry_size + 16].iter().all(|&b| b == 0))
            .count()
            .saturating_sub(1),
    );
    Some(slot)
}

/// Add a AthFS partition to the ACTIVE block device's existing GPT (dual-boot).
/// Thin wrapper over `add_gpt_partition_on` whose writes route through the
/// active device's `safe_mode_guard_write`.
pub fn add_gpt_partition(start_lba: u64, end_lba: u64) -> Option<usize> {
    let dev_guard = crate::block_io::ACTIVE_BLOCK_DEVICE.lock();
    let dev = dev_guard.as_ref()?;
    add_gpt_partition_on(dev.as_ref(), start_lba, end_lba)
}

fn write_install_seed_artifact(esp_start_lba: u64) -> bool {
    let dev_guard = crate::block_io::ACTIVE_BLOCK_DEVICE.lock();
    let Some(dev) = dev_guard.as_ref() else {
        return false;
    };
    if dev.sector_size() != 512 {
        return false;
    }
    let artifact_lba = esp_start_lba.saturating_add(1);
    let mut sector = [0u8; 512];
    if dev.read_sector(artifact_lba, &mut sector).is_err() {
        return false;
    }
    let original = sector;
    let marker = b"ATHENAOS-INSTALL-SEED-BOOT-ARTIFACT";
    sector[..marker.len()].copy_from_slice(marker);
    if dev.write_sector(artifact_lba, &sector).is_err() {
        return false;
    }
    let mut verify = [0u8; 512];
    if dev.read_sector(artifact_lba, &mut verify).is_err() {
        return false;
    }
    let ok = &verify[..marker.len()] == marker;
    let _ = dev.write_sector(artifact_lba, &original);
    if ok {
        INSTALL_SEED_OK.fetch_add(1, Ordering::Relaxed);
        INSTALL_SEED_LBA.store(artifact_lba, Ordering::Relaxed);
    }
    ok
}

/// Honest smoketest: scan the active block device's partition table for
/// an ESP / FAT32 partition; if found, mount FAT32 at that partition
/// base. If the disk has no partition table, fall back to treating sector
/// 0 as a FAT VBR (lets us mount a raw FAT32 image). Every step reports
/// what it found instead of silently succeeding.
pub fn run_boot_smoketest() {
    MOUNT_ATTEMPTS.fetch_add(1, Ordering::Relaxed);

    // Step 1: partition scan.
    let scan = find_esp_partition();
    let mut install_seed_esp_lba = None;
    let (scan_label, partition_base_lba) = match &scan {
        PartitionScan::GptEspFound {
            start_lba,
            sector_count,
            name,
        } => {
            let l = format!(
                "GPT ESP: start_lba={} sectors={} name=\"{}\"",
                start_lba, sector_count, name
            );
            crate::serial_println!("[fatfs] {}", l);
            // No install-seed artifact write here: this is a REAL discovered
            // GPT ESP (on Athena: the Windows NVMe's boot partition) and the
            // artifact lands at ESP_start+1 — inside host data. Only --safe
            // blocked it ("[safe-mode] BLOCKED nvme write lba=2049"). The
            // artifact test belongs to the blank-disk seed path only.
            (l, *start_lba)
        }
        PartitionScan::GptNoEsp { partition_count } => {
            let l = format!(
                "GPT detected, {} partitions, no ESP-type entry — \
                             falling back to sector 0 probe",
                partition_count
            );
            crate::serial_println!("[fatfs] {}", l);
            (l, 0)
        }
        PartitionScan::MbrFat32Found {
            start_lba,
            sector_count,
        } => {
            let l = format!(
                "MBR FAT32: start_lba={} sectors={}",
                start_lba, sector_count
            );
            crate::serial_println!("[fatfs] {}", l);
            // Same as the GPT arm above: a real discovered partition is host
            // data — no artifact write.
            (l, *start_lba)
        }
        PartitionScan::MbrNoFat32 { partition_count } => {
            let l = format!(
                "MBR detected, {} partitions, no FAT32 entry — \
                             falling back to sector 0 probe",
                partition_count
            );
            crate::serial_println!("[fatfs] {}", l);
            (l, 0)
        }
        PartitionScan::NoPartitionTable => {
            let l = String::from("no partition table — trying sector 0 as raw FAT32 VBR");
            crate::serial_println!("[fatfs] {}", l);
            install_seed_esp_lba = seed_minimal_gpt_with_esp().map(|(esp, _athfs)| esp);
            if let Some(lba) = install_seed_esp_lba {
                crate::serial_println!(
                    "[fatfs] install seed: wrote minimal GPT + ESP entry at LBA {}",
                    lba
                );
            }
            (l, 0)
        }
        PartitionScan::ReadError(msg) => {
            let l = format!("partition scan read error: {}", msg);
            *LAST_ERROR.lock() = Some(l.clone());
            crate::serial_println!("[fatfs] {}", l);
            *LAST_SCAN.lock() = Some(l);
            return;
        }
    };
    *LAST_SCAN.lock() = Some(scan_label);
    if let Some(lba) = install_seed_esp_lba {
        let ok = write_install_seed_artifact(lba);
        crate::serial_println!(
            "[fatfs] install seed artifact write/readback at LBA {} -> {}",
            lba.saturating_add(1),
            if ok { "PASS" } else { "FAIL" }
        );
    }

    // Step 2: read the VBR at the chosen partition base.
    let bpb = match read_vbr_at(partition_base_lba) {
        Ok(b) => b,
        Err(e) => {
            *LAST_ERROR.lock() = Some(format!("{}", e));
            if partition_base_lba == 0 {
                crate::serial_println!(
                    "[fatfs] sector 0 is not FAT32 ({}). \
                     Expected on whole-disk view — ESP lives inside a GPT partition.",
                    e,
                );
            } else {
                crate::serial_println!(
                    "[fatfs] partition VBR at LBA {} is not FAT32 ({}).",
                    partition_base_lba,
                    e,
                );
            }
            return;
        }
    };

    MOUNT_OK.fetch_add(1, Ordering::Relaxed);
    let label = core::str::from_utf8(&bpb.volume_label)
        .unwrap_or("?")
        .trim_end();
    crate::serial_println!(
        "[fatfs] FAT32 VBR @ LBA {}: label=\"{}\" bps={} spc={} root_cluster={} total_sectors={}",
        partition_base_lba,
        label,
        bpb.bytes_per_sector,
        bpb.sectors_per_cluster,
        bpb.root_cluster,
        bpb.total_sectors_32,
    );
    *LAST_BPB.lock() = Some(bpb);

    // Step 3: list the first cluster of the root directory.
    match list_root_first_cluster_at(&bpb, partition_base_lba) {
        Ok(entries) => {
            let mut summary = Vec::new();
            for (i, e) in entries.iter().take(16).enumerate() {
                let s = match e {
                    DirEntryKind::EndOfDir => format!("[{}] <end>", i),
                    DirEntryKind::Free => format!("[{}] <free>", i),
                    DirEntryKind::LfnFragment => format!("[{}] <lfn>", i),
                    DirEntryKind::VolumeLabel(l) => format!("[{}] label=\"{}\"", i, l),
                    DirEntryKind::Subdir {
                        name,
                        first_cluster,
                    } => format!("[{}] DIR  {} (cl={})", i, name, first_cluster),
                    DirEntryKind::File {
                        name,
                        ext,
                        size,
                        first_cluster,
                    } => format!(
                        "[{}] FILE {}.{} ({} B, cl={})",
                        i, name, ext, size, first_cluster
                    ),
                };
                crate::serial_println!("[fatfs]   {}", s);
                summary.push(s);
            }
            *LAST_ENTRIES.lock() = summary;
        }
        Err(e) => {
            crate::serial_println!("[fatfs] root cluster walk failed: {}", e);
            *LAST_ERROR.lock() = Some(format!("root walk: {}", e));
        }
    }
}

pub fn dump_text() -> String {
    let mut s = String::new();
    s.push_str("# FAT32 ESP locator — native no_std parser\n");
    s.push_str(&format!(
        "mount_attempts: {}\n",
        MOUNT_ATTEMPTS.load(Ordering::Relaxed)
    ));
    s.push_str(&format!(
        "mount_ok:       {}\n",
        MOUNT_OK.load(Ordering::Relaxed)
    ));
    s.push_str(&format!(
        "install_seed_ok: {}\n",
        INSTALL_SEED_OK.load(Ordering::Relaxed)
    ));
    let lba = INSTALL_SEED_LBA.load(Ordering::Relaxed);
    if lba != 0 {
        s.push_str(&format!("install_seed_lba: {}\n", lba));
    }
    if let Some(scan) = LAST_SCAN.lock().as_ref() {
        s.push_str(&format!("partition_scan: {}\n", scan));
    } else {
        s.push_str("partition_scan: <not run>\n");
    }
    if let Some(err) = LAST_ERROR.lock().as_ref() {
        s.push_str(&format!("last_error:     {}\n", err));
    } else {
        s.push_str("last_error:     none\n");
    }
    if let Some(bpb) = LAST_BPB.lock().as_ref() {
        let label = core::str::from_utf8(&bpb.volume_label)
            .unwrap_or("?")
            .trim_end();
        s.push_str(&format!(
            "bpb: label=\"{}\" bytes_per_sector={} sectors_per_cluster={} \
             reserved_sectors={} num_fats={} fat_size_32={} root_cluster={} \
             total_sectors={}\n",
            label,
            bpb.bytes_per_sector,
            bpb.sectors_per_cluster,
            bpb.reserved_sectors,
            bpb.num_fats,
            bpb.fat_size_32,
            bpb.root_cluster,
            bpb.total_sectors_32,
        ));
    } else {
        s.push_str("bpb: <none — sector 0 is not a FAT32 VBR>\n");
    }
    s.push_str("# Root directory (first cluster, up to 16 entries):\n");
    let entries = LAST_ENTRIES.lock();
    if entries.is_empty() {
        s.push_str("  <no entries — VBR not parsed or root empty>\n");
    } else {
        for e in entries.iter() {
            s.push_str("  ");
            s.push_str(e);
            s.push('\n');
        }
    }
    s
}

// ===========================================================================
// FAT32 formatter + file writer — MasterChecklist Phase 3.3
//
// Writes a spec-compliant FAT32 filesystem into an ESP partition and populates
// the EFI boot tree (/EFI/BOOT/BOOTX64.EFI + /EFI/athenaos/). Used by the
// installer (athinstaller) to make a freshly-partitioned NVMe bootable.
// Microsoft FAT spec (fatgen103) section 3 (BPB) + section 6 (directory entries).
// ===========================================================================

/// In-progress FAT32 install context: tracks the BPB geometry + a bump cluster
/// allocator so the installer can lay down directories and files sequentially.
pub struct Fat32Writer {
    pub esp_start_lba: u64,
    pub total_sectors: u32,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub num_fats: u8,
    pub fat_size_sectors: u32,
    pub data_start_lba: u64,
    next_cluster: u32,
    max_cluster: u32,
}

impl Fat32Writer {
    fn cluster_first_lba(&self, cluster: u32) -> u64 {
        self.data_start_lba + (cluster as u64 - 2) * self.sectors_per_cluster as u64
    }

    fn fat_entry_lba_offset(&self, cluster: u32) -> (u64, usize) {
        let fat_offset = cluster as u64 * 4;
        let lba = self.esp_start_lba + self.reserved_sectors as u64 + (fat_offset / 512);
        let off = (fat_offset % 512) as usize;
        (lba, off)
    }

    fn set_fat_entry(
        &self,
        dev: &dyn crate::block_io::BlockDevice,
        cluster: u32,
        value: u32,
    ) -> bool {
        for fat in 0..self.num_fats as u64 {
            let (base_lba, off) = self.fat_entry_lba_offset(cluster);
            let lba = base_lba + fat * self.fat_size_sectors as u64;
            let mut sec = [0u8; 512];
            if dev.read_sector(lba, &mut sec).is_err() {
                return false;
            }
            sec[off..off + 4].copy_from_slice(&(value & 0x0FFF_FFFF).to_le_bytes());
            if dev.write_sector(lba, &sec).is_err() {
                return false;
            }
        }
        true
    }

    fn alloc_cluster(&mut self, dev: &dyn crate::block_io::BlockDevice) -> Option<u32> {
        if self.next_cluster > self.max_cluster {
            return None;
        }
        let c = self.next_cluster;
        self.next_cluster += 1;
        let zero = [0u8; 512];
        for s in 0..self.sectors_per_cluster as u64 {
            if dev
                .write_sector(self.cluster_first_lba(c) + s, &zero)
                .is_err()
            {
                return None;
            }
        }
        if !self.set_fat_entry(dev, c, 0x0FFF_FFFF) {
            return None;
        }
        Some(c)
    }
}

/// Build an 8.3 short-name directory entry (32 bytes).
fn fat_dirent(name83: &[u8; 11], attr: u8, first_cluster: u32, size: u32) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[0..11].copy_from_slice(name83);
    e[11] = attr;
    e[20..22].copy_from_slice(&(((first_cluster >> 16) & 0xFFFF) as u16).to_le_bytes());
    e[26..28].copy_from_slice(&((first_cluster & 0xFFFF) as u16).to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
    e[16..18].copy_from_slice(&0x5821u16.to_le_bytes());
    e[24..26].copy_from_slice(&0x5821u16.to_le_bytes());
    e
}

/// Convert a base + ext to a space-padded uppercase 8.3 name.
fn name83(base: &str, ext: &str) -> [u8; 11] {
    let mut n = [b' '; 11];
    for (i, b) in base.bytes().take(8).enumerate() {
        n[i] = b.to_ascii_uppercase();
    }
    for (i, b) in ext.bytes().take(3).enumerate() {
        n[8 + i] = b.to_ascii_uppercase();
    }
    n
}

/// Format a FAT32 filesystem into the ESP partition at esp_start_lba.
/// MasterChecklist Phase 3.3: "FAT32 formatter."
pub fn fat32_format(
    dev: &dyn crate::block_io::BlockDevice,
    esp_start_lba: u64,
    esp_sectors: u32,
) -> Option<Fat32Writer> {
    if esp_sectors < 512 {
        crate::serial_println!(
            "[fatfs] fat32_format: ESP too small ({} sectors)",
            esp_sectors
        );
        return None;
    }
    let reserved_sectors: u16 = 32;
    let num_fats: u8 = 2;
    // FAT type determination (Microsoft fatgen103 §3.5): a volume is FAT32 ONLY
    // if it has >= 65525 data clusters. UEFI firmware (OVMF/EDK2) enforces this
    // and REFUSES TO MOUNT a "FAT32"-labeled volume with fewer — so the
    // installed ESP then has no firmware-mountable filesystem and the machine
    // won't boot from its own disk (observed on a real install: OVMF "failed to
    // load ... Not Found", and the UEFI shell showed the ESP as a raw BLK with
    // NO FSx: mapping). Our own reader ignores cluster count, which hid this:
    // the old `spc = if < 128 MiB {1} else {8}` made a 128 MiB ESP 32732
    // clusters — an invalid FAT32. Pick the LARGEST power-of-two cluster size
    // (smallest FAT) that still yields a comfortably valid count, down to 1.
    // FAT32 FAT-size estimate (Microsoft fatgen103 §"FAT32 FAT Size"):
    //   TmpVal2 = (256 * SecPerClus + NumFATs) / 2   <-- the `/2` is FAT32-ONLY
    //   FATSz   = (TmpVal1 + (TmpVal2 - 1)) / TmpVal2
    // The previous code OMITTED the `/2`, so it declared a FAT ~half the size
    // a FAT32 with this many clusters needs (e.g. 510 sectors where 1021 were
    // required for 130 546 clusters). Our lenient reader never followed a chain
    // far enough to hit the truncation, but the resulting geometry is internally
    // inconsistent: UEFI/OVMF (and the `fatfs` crate) mount the volume yet read
    // the root directory as garbage, so the firmware finds no \EFI\BOOT\BOOTX64
    // and won't boot the installed disk.
    let fat_size_for = |spc: u32| -> u32 {
        let t1 = esp_sectors - reserved_sectors as u32;
        let t2 = ((256 * spc) + num_fats as u32) / 2;
        (t1 + (t2 - 1)) / t2
    };
    let clusters_for = |spc: u32| -> u64 {
        let fsz = fat_size_for(spc);
        let data = esp_sectors as u64 - (reserved_sectors as u64 + num_fats as u64 * fsz as u64);
        data / spc as u64
    };
    let sectors_per_cluster: u8 = {
        let mut spc = 8u32;
        while spc > 1 && clusters_for(spc) < 66_000 {
            spc /= 2;
        }
        spc as u8
    };

    let fat_size_sectors = fat_size_for(sectors_per_cluster as u32);

    let data_start_lba =
        esp_start_lba + reserved_sectors as u64 + (num_fats as u64 * fat_size_sectors as u64);
    let data_sectors =
        esp_sectors as u64 - (reserved_sectors as u64 + num_fats as u64 * fat_size_sectors as u64);
    let cluster_count = data_sectors / sectors_per_cluster as u64;
    let max_cluster = (cluster_count + 1) as u32;

    let mut vbr = [0u8; 512];
    vbr[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    vbr[3..11].copy_from_slice(b"MSWIN4.1");
    vbr[11..13].copy_from_slice(&512u16.to_le_bytes());
    vbr[13] = sectors_per_cluster;
    vbr[14..16].copy_from_slice(&reserved_sectors.to_le_bytes());
    vbr[16] = num_fats;
    vbr[17..19].copy_from_slice(&0u16.to_le_bytes());
    vbr[19..21].copy_from_slice(&0u16.to_le_bytes());
    vbr[21] = 0xF8;
    vbr[22..24].copy_from_slice(&0u16.to_le_bytes());
    vbr[24..26].copy_from_slice(&63u16.to_le_bytes());
    vbr[26..28].copy_from_slice(&255u16.to_le_bytes());
    vbr[28..32].copy_from_slice(&(esp_start_lba as u32).to_le_bytes());
    vbr[32..36].copy_from_slice(&esp_sectors.to_le_bytes());
    vbr[36..40].copy_from_slice(&fat_size_sectors.to_le_bytes());
    vbr[40..42].copy_from_slice(&0u16.to_le_bytes());
    vbr[42..44].copy_from_slice(&0u16.to_le_bytes());
    vbr[44..48].copy_from_slice(&2u32.to_le_bytes());
    vbr[48..50].copy_from_slice(&1u16.to_le_bytes());
    vbr[50..52].copy_from_slice(&6u16.to_le_bytes());
    vbr[64] = 0x80;
    vbr[66] = 0x29;
    vbr[67..71].copy_from_slice(&0x5241_4545u32.to_le_bytes());
    vbr[71..82].copy_from_slice(b"ATHENA ESP ");
    vbr[82..90].copy_from_slice(b"FAT32   ");
    vbr[510] = 0x55;
    vbr[511] = 0xAA;
    if dev.write_sector(esp_start_lba, &vbr).is_err() {
        return None;
    }
    let _ = dev.write_sector(esp_start_lba + 6, &vbr);

    let mut fsinfo = [0u8; 512];
    fsinfo[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    fsinfo[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    fsinfo[488..492].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    fsinfo[492..496].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    fsinfo[508..512].copy_from_slice(&0xAA55_0000u32.to_le_bytes());
    let _ = dev.write_sector(esp_start_lba + 1, &fsinfo);

    let zero = [0u8; 512];
    for fat in 0..num_fats as u64 {
        let fat_base = esp_start_lba + reserved_sectors as u64 + fat * fat_size_sectors as u64;
        for s in 0..fat_size_sectors as u64 {
            if dev.write_sector(fat_base + s, &zero).is_err() {
                return None;
            }
        }
    }

    let w = Fat32Writer {
        esp_start_lba,
        total_sectors: esp_sectors,
        sectors_per_cluster,
        reserved_sectors,
        num_fats,
        fat_size_sectors,
        data_start_lba,
        next_cluster: 3,
        max_cluster,
    };

    w.set_fat_entry(dev, 0, 0x0FFF_FFF8);
    w.set_fat_entry(dev, 1, 0x0FFF_FFFF);
    w.set_fat_entry(dev, 2, 0x0FFF_FFFF);
    for s in 0..w.sectors_per_cluster as u64 {
        let _ = dev.write_sector(w.cluster_first_lba(2) + s, &zero);
    }

    crate::serial_println!(
        "[fatfs] fat32_format: ESP at LBA{} sectors={} spc={} fat_size={} clusters={} -> OK",
        esp_start_lba,
        esp_sectors,
        sectors_per_cluster,
        fat_size_sectors,
        cluster_count,
    );
    Some(w)
}

/// Append a directory entry into a single-cluster directory.
fn fat32_add_dirent(
    dev: &dyn crate::block_io::BlockDevice,
    w: &Fat32Writer,
    dir_cluster: u32,
    entry: &[u8; 32],
) -> bool {
    let cluster_bytes = w.sectors_per_cluster as usize * 512;
    let slots = cluster_bytes / 32;
    for slot in 0..slots {
        let byte_off = slot * 32;
        let lba = w.cluster_first_lba(dir_cluster) + (byte_off / 512) as u64;
        let in_sec = byte_off % 512;
        let mut sec = [0u8; 512];
        if dev.read_sector(lba, &mut sec).is_err() {
            return false;
        }
        let first = sec[in_sec];
        if first == 0x00 || first == 0xE5 {
            sec[in_sec..in_sec + 32].copy_from_slice(entry);
            return dev.write_sector(lba, &sec).is_ok();
        }
    }
    false
}

/// Create a subdirectory inside parent_cluster. Returns its cluster.
pub fn fat32_mkdir(
    dev: &dyn crate::block_io::BlockDevice,
    w: &mut Fat32Writer,
    parent_cluster: u32,
    name: &str,
) -> Option<u32> {
    let c = w.alloc_cluster(dev)?;
    let dent = fat_dirent(&name83(name, ""), 0x10, c, 0);
    if !fat32_add_dirent(dev, w, parent_cluster, &dent) {
        return None;
    }
    let dot = fat_dirent(&name83(".", ""), 0x10, c, 0);
    let dotdot_cluster = if parent_cluster == 2 {
        0
    } else {
        parent_cluster
    };
    let dotdot = fat_dirent(&name83("..", ""), 0x10, dotdot_cluster, 0);
    let lba = w.cluster_first_lba(c);
    let mut sec = [0u8; 512];
    sec[0..32].copy_from_slice(&dot);
    sec[32..64].copy_from_slice(&dotdot);
    dev.write_sector(lba, &sec).ok()?;
    Some(c)
}

/// Write a file base.ext with data into directory dir_cluster.
/// MasterChecklist Phase 3.3: "Write EFI/BOOT/BOOTX64.EFI to ESP."
/// Allocate a cluster chain for `data`, link it in the FAT, and write the
/// bytes. Returns the chain (`chain[0]` = first cluster). Shared by the 8.3
/// and long-name file writers so both lay data down identically.
fn alloc_and_write_data(
    dev: &dyn crate::block_io::BlockDevice,
    w: &mut Fat32Writer,
    data: &[u8],
) -> Option<Vec<u32>> {
    let cluster_bytes = w.sectors_per_cluster as usize * 512;
    let clusters_needed = ((data.len() + cluster_bytes - 1) / cluster_bytes.max(1)).max(1);

    let mut chain: Vec<u32> = Vec::new();
    for _ in 0..clusters_needed {
        match w.alloc_cluster(dev) {
            Some(c) => chain.push(c),
            None => {
                crate::serial_println!("[fatfs] write: out of clusters");
                return None;
            }
        }
    }
    for i in 0..chain.len().saturating_sub(1) {
        w.set_fat_entry(dev, chain[i], chain[i + 1]);
    }

    let mut written = 0usize;
    'outer: for &c in &chain {
        for s in 0..w.sectors_per_cluster as u64 {
            let mut sec = [0u8; 512];
            let remaining = data.len() - written;
            if remaining > 0 {
                let n = remaining.min(512);
                sec[..n].copy_from_slice(&data[written..written + n]);
                written += n;
            }
            if dev.write_sector(w.cluster_first_lba(c) + s, &sec).is_err() {
                return None;
            }
            if written >= data.len() {
                break 'outer;
            }
        }
    }
    Some(chain)
}

pub fn fat32_write_file(
    dev: &dyn crate::block_io::BlockDevice,
    w: &mut Fat32Writer,
    dir_cluster: u32,
    base: &str,
    ext: &str,
    data: &[u8],
) -> bool {
    let Some(chain) = alloc_and_write_data(dev, w, data) else {
        return false;
    };
    let dent = fat_dirent(&name83(base, ext), 0x20, chain[0], data.len() as u32);
    if !fat32_add_dirent(dev, w, dir_cluster, &dent) {
        return false;
    }
    crate::serial_println!(
        "[fatfs] fat32_write_file: {}.{} {} bytes -> {} cluster(s) at {} -> OK",
        base,
        ext,
        data.len(),
        chain.len(),
        chain[0],
    );
    true
}

/// Microsoft LFN checksum of an 11-byte 8.3 short name (binds each long-name
/// fragment to its alias; a UEFI FAT driver rejects fragments whose checksum
/// doesn't match the following 8.3 entry).
fn fat_lfn_checksum(name83: &[u8; 11]) -> u8 {
    let mut sum: u8 = 0;
    for &b in name83.iter() {
        sum = ((sum & 1) << 7).wrapping_add(sum >> 1).wrapping_add(b);
    }
    sum
}

/// The 13 UCS-2 char slots within a 32-byte LFN entry (offsets 1..11, 14..26,
/// 28..32). Used by both the builder and the readback verifier.
const LFN_CHAR_OFFSETS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];

/// Build the on-disk directory-entry run for a long-named file: the LFN
/// entries (physically highest-sequence-first, per the spec) followed by the
/// 8.3 entry. `long_name` is ASCII (stored as UCS-2). Supports up to 20
/// fragments (260 chars) — far more than any ESP boot file needs.
fn build_lfn_entries(long_name: &str, short83: &[u8; 11], short_entry: [u8; 32]) -> Vec<[u8; 32]> {
    let chars: Vec<u16> = long_name.chars().map(|c| c as u16).collect();
    let n_frags = (chars.len() + 12) / 13; // ceil(len/13), >= 1
    let checksum = fat_lfn_checksum(short83);
    let mut out: Vec<[u8; 32]> = Vec::new();
    for frag in (1..=n_frags).rev() {
        let mut e = [0u8; 32];
        e[0] = frag as u8 | if frag == n_frags { 0x40 } else { 0 }; // 0x40 = LAST
        e[11] = 0x0F; // LFN attribute (RO|HID|SYS|VOL)
        e[13] = checksum;
        // first-cluster field (26..28) stays 0 for LFN entries.
        let base = (frag - 1) * 13;
        for (i, &off) in LFN_CHAR_OFFSETS.iter().enumerate() {
            let pos = base + i;
            let val: u16 = if pos < chars.len() {
                chars[pos]
            } else if pos == chars.len() {
                0x0000 // NUL terminator
            } else {
                0xFFFF // padding past the terminator
            };
            e[off..off + 2].copy_from_slice(&val.to_le_bytes());
        }
        out.push(e);
    }
    out.push(short_entry);
    out
}

/// Add a contiguous run of directory entries (an LFN set + its 8.3 entry) to
/// `dir_cluster`. LFN entries MUST be physically contiguous and immediately
/// precede their 8.3 entry, so this finds one free run of the needed length.
/// Single-cluster directories only (sufficient for the ESP boot tree, whose
/// freshly-formatted dirs have all free slots contiguous at the end).
fn fat32_add_dirents(
    dev: &dyn crate::block_io::BlockDevice,
    w: &Fat32Writer,
    dir_cluster: u32,
    entries: &[[u8; 32]],
) -> bool {
    let cluster_bytes = w.sectors_per_cluster as usize * 512;
    let slots = cluster_bytes / 32;
    let sectors_in = cluster_bytes / 512;
    if entries.is_empty() || entries.len() > slots {
        return false;
    }
    let mut buf = alloc::vec![0u8; cluster_bytes];
    for s in 0..sectors_in {
        if dev
            .read_sector(
                w.cluster_first_lba(dir_cluster) + s as u64,
                &mut buf[s * 512..s * 512 + 512],
            )
            .is_err()
        {
            return false;
        }
    }
    // First run of `entries.len()` consecutive free (0x00 / 0xE5) slots.
    let mut run_start: Option<usize> = None;
    let mut found: Option<usize> = None;
    for slot in 0..slots {
        let first = buf[slot * 32];
        if first == 0x00 || first == 0xE5 {
            let start = *run_start.get_or_insert(slot);
            if slot - start + 1 == entries.len() {
                found = Some(start);
                break;
            }
        } else {
            run_start = None;
        }
    }
    let Some(start) = found else {
        crate::serial_println!(
            "[fatfs] add_dirents: no contiguous run for {} entries",
            entries.len()
        );
        return false;
    };
    for (i, e) in entries.iter().enumerate() {
        let off = (start + i) * 32;
        buf[off..off + 32].copy_from_slice(e);
    }
    for s in 0..sectors_in {
        if dev
            .write_sector(
                w.cluster_first_lba(dir_cluster) + s as u64,
                &buf[s * 512..s * 512 + 512],
            )
            .is_err()
        {
            return false;
        }
    }
    true
}

/// Write a file whose name needs an LFN (`> 8.3`), e.g. the bootloader's
/// `kernel-x86_64`. `long_name` is what a UEFI FAT driver opens; `short_base`/
/// `short_ext` is the unique 8.3 alias the checksum binds to (caller ensures
/// no collision in `dir_cluster`). MasterChecklist Phase 3.5.
pub fn fat32_write_file_long(
    dev: &dyn crate::block_io::BlockDevice,
    w: &mut Fat32Writer,
    dir_cluster: u32,
    long_name: &str,
    short_base: &str,
    short_ext: &str,
    data: &[u8],
) -> bool {
    let Some(chain) = alloc_and_write_data(dev, w, data) else {
        return false;
    };
    let short83 = name83(short_base, short_ext);
    let short_entry = fat_dirent(&short83, 0x20, chain[0], data.len() as u32);
    let entries = build_lfn_entries(long_name, &short83, short_entry);
    if !fat32_add_dirents(dev, w, dir_cluster, &entries) {
        return false;
    }
    crate::serial_println!(
        "[fatfs] fat32_write_file_long: \"{}\" ({} LFN + 8.3 {}) {} bytes -> {} cluster(s) at {} -> OK",
        long_name,
        entries.len() - 1,
        short_base,
        data.len(),
        chain.len(),
        chain[0],
    );
    true
}

/// Format the ESP and lay down the EFI boot tree (bootloader + kernel slot A).
/// MasterChecklist Phase 3.3: full ESP population for the installer.
pub fn fat32_install_boot_tree(
    esp_start_lba: u64,
    esp_sectors: u32,
    bootx64: &[u8],
    kernel: &[u8],
    ramdisk: &[u8],
) -> bool {
    let dev_guard = crate::block_io::ACTIVE_BLOCK_DEVICE.lock();
    let Some(dev) = dev_guard.as_ref() else {
        crate::serial_println!("[fatfs] install: no active block device");
        return false;
    };
    let dev = dev.as_ref();

    let mut w = match fat32_format(dev, esp_start_lba, esp_sectors) {
        Some(w) => w,
        None => return false,
    };
    let efi = match fat32_mkdir(dev, &mut w, 2, "EFI") {
        Some(c) => c,
        None => return false,
    };
    let boot = match fat32_mkdir(dev, &mut w, efi, "BOOT") {
        Some(c) => c,
        None => return false,
    };
    if !fat32_write_file(dev, &mut w, boot, "BOOTX64", "EFI", bootx64) {
        return false;
    }
    // The bootable kernel: the bootloader (bootloader crate 0.11) opens
    // "kernel-x86_64" at the ESP ROOT. Without this exact long-named file the
    // installed disk cannot boot, regardless of the slot-A copy below.
    // MasterChecklist Phase 3.5.
    if !fat32_write_file_long(dev, &mut w, 2, "kernel-x86_64", "KERNEL~1", "", kernel) {
        return false;
    }
    let athenaos = match fat32_mkdir(dev, &mut w, efi, "ATHENAOS") {
        Some(c) => c,
        None => return false,
    };
    // Slot-A copy (Phase 3.6 atomic updates will swap kernel-x86_64 between
    // KERNEL-A.BIN / KERNEL-B.BIN); kept alongside the bootable root copy.
    if !fat32_write_file(dev, &mut w, athenaos, "KERNEL-A", "BIN", kernel) {
        return false;
    }
    // Phase 3.1: write the real ramdisk (initramfs) the installer sourced.
    if !ramdisk.is_empty() && !fat32_write_file(dev, &mut w, athenaos, "INITRD", "IMG", ramdisk) {
        return false;
    }
    crate::serial_println!(
        "[fatfs] install: EFI boot tree written (BOOTX64.EFI + kernel-x86_64 root + slot A + initrd {} B)",
        ramdisk.len()
    );
    true
}

/// RAM-backed block device for the formatter smoketest — proves the FAT32
/// writer deterministically without touching the live virtio/NVMe disk (whose
/// boot-time descriptor ring can't absorb the install's write volume yet).
struct RamDisk {
    sectors: spin::Mutex<Vec<[u8; 512]>>,
    count: usize,
}

impl RamDisk {
    fn new(count: usize) -> Self {
        Self {
            sectors: spin::Mutex::new(alloc::vec![[0u8; 512]; count]),
            count,
        }
    }
}

impl crate::block_io::BlockDevice for RamDisk {
    fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        let g = self.sectors.lock();
        let s = g.get(lba as usize).ok_or("ramdisk: lba oob")?;
        let n = buf.len().min(512);
        buf[..n].copy_from_slice(&s[..n]);
        Ok(())
    }
    fn write_sector(&self, lba: u64, buf: &[u8]) -> Result<(), &'static str> {
        let mut g = self.sectors.lock();
        let s = g.get_mut(lba as usize).ok_or("ramdisk: lba oob")?;
        let n = buf.len().min(512);
        s[..n].copy_from_slice(&buf[..n]);
        Ok(())
    }
    fn sector_size(&self) -> usize {
        512
    }
    fn total_sectors(&self) -> u64 {
        self.count as u64
    }
}

/// Boot smoketest: format a RAM disk as FAT32, write the EFI boot tree, re-parse
/// the VBR + re-list the root directory to prove the format is mountable.
/// MasterChecklist Phase 3.3.
pub fn run_format_smoketest() {
    let dev = RamDisk::new(4096); // 2 MiB scratch
    let esp_start: u64 = 64;
    let esp_sectors: u32 = 4000;

    let Some(mut w) = fat32_format(&dev, esp_start, esp_sectors) else {
        crate::serial_println!("[fatfs] format smoketest: fat32_format FAILED");
        return;
    };
    let Some(efi) = fat32_mkdir(&dev, &mut w, 2, "EFI") else {
        crate::serial_println!("[fatfs] format smoketest: mkdir EFI FAILED");
        return;
    };
    let Some(boot) = fat32_mkdir(&dev, &mut w, efi, "BOOT") else {
        crate::serial_println!("[fatfs] format smoketest: mkdir BOOT FAILED");
        return;
    };
    let payload = b"ATHENAOS-BOOTX64-PLACEHOLDER";
    let wrote = fat32_write_file(&dev, &mut w, boot, "BOOTX64", "EFI", payload);

    // Verify: re-parse the VBR we just wrote.
    let mut vbr = [0u8; 512];
    let reparse = dev.read_sector(esp_start, &mut vbr).is_ok() && parse_vbr(&vbr).is_ok();

    // Verify: the root directory's first cluster now lists "EFI".
    let mut root = [0u8; 512];
    let root_lba = w.data_start_lba; // cluster 2 first sector
    let root_has_efi = dev.read_sector(root_lba, &mut root).is_ok() && &root[0..3] == b"EFI";

    // LFN write proof (MasterChecklist 3.5): the bootloader opens the kernel by
    // its long name "kernel-x86_64" (13 chars → needs an LFN entry). Write it at
    // the ESP root (cluster 2) and reconstruct the long name from the on-disk
    // LFN run, checking the checksum binds to the 8.3 alias — exactly what a
    // UEFI FAT driver validates before opening the file.
    let kdata = b"\x7fELF fake-kernel payload for the lfn write test";
    let lfn_wrote = fat32_write_file_long(&dev, &mut w, 2, "kernel-x86_64", "KERNEL~1", "", kdata);
    let lfn_readback = lfn_name_present(&dev, &w, 2, "kernel-x86_64");

    let pass = wrote && reparse && root_has_efi && lfn_wrote && lfn_readback;
    crate::serial_println!(
        "[fatfs] format smoketest: format=OK mkdir=OK write={} vbr_reparse={} root_lists_EFI={} lfn_write={} lfn_readback={} -> {}",
        wrote, reparse, root_has_efi, lfn_wrote, lfn_readback,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// Scan a single-cluster directory and return true iff a long name `target`
/// is present and well-formed: its LFN fragments reconstruct to `target` AND
/// their stored checksum matches the following 8.3 entry's short name. This is
/// the same validation a UEFI FAT driver performs, so a PASS means the file is
/// openable by `BOOTX64.EFI` via its long name. Verifier for `fat32_write_file_long`.
fn lfn_name_present(
    dev: &dyn crate::block_io::BlockDevice,
    w: &Fat32Writer,
    dir_cluster: u32,
    target: &str,
) -> bool {
    let cluster_bytes = w.sectors_per_cluster as usize * 512;
    let slots = cluster_bytes / 32;
    let sectors_in = cluster_bytes / 512;
    let mut buf = alloc::vec![0u8; cluster_bytes];
    for s in 0..sectors_in {
        if dev
            .read_sector(
                w.cluster_first_lba(dir_cluster) + s as u64,
                &mut buf[s * 512..s * 512 + 512],
            )
            .is_err()
        {
            return false;
        }
    }
    let mut name_buf = [0u16; 260];
    let mut max_len = 0usize;
    let mut frag_chk = 0u8;
    for slot in 0..slots {
        let e = &buf[slot * 32..slot * 32 + 32];
        if e[0] == 0x00 {
            break;
        }
        if e[0] == 0xE5 {
            max_len = 0;
            continue;
        }
        if e[11] == 0x0F {
            // LFN fragment: place its 13 chars at (seq-1)*13.
            let seq = (e[0] & 0x1F) as usize;
            if seq == 0 || seq > 20 {
                max_len = 0;
                continue;
            }
            frag_chk = e[13];
            for (i, &off) in LFN_CHAR_OFFSETS.iter().enumerate() {
                let v = u16::from_le_bytes([e[off], e[off + 1]]);
                let pos = (seq - 1) * 13 + i;
                if v != 0x0000 && v != 0xFFFF && pos < name_buf.len() {
                    name_buf[pos] = v;
                    max_len = max_len.max(pos + 1);
                }
            }
        } else {
            // 8.3 entry: if preceded by an LFN set, reconstruct + check.
            if max_len > 0 {
                let mut reconstructed = String::new();
                for &c in name_buf.iter().take(max_len) {
                    reconstructed.push((c as u8) as char);
                }
                let mut n83 = [0u8; 11];
                n83.copy_from_slice(&e[0..11]);
                if reconstructed == target && fat_lfn_checksum(&n83) == frag_chk {
                    return true;
                }
            }
            name_buf = [0u16; 260];
            max_len = 0;
        }
    }
    false
}
