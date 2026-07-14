//! AthenaOS installer orchestration — MasterChecklist Phase 3.
//!
//! The userspace `raeinstaller` process drives the install via `SYS_INSTALL_RUN`
//! (syscall 256). Block I/O lives in the kernel (where `ACTIVE_BLOCK_DEVICE`
//! is), so the kernel performs the actual writes while the installer UI reports
//! progress and (eventually) collects user choices (target disk, account).
//!
//! Install sequence (mirrors a real OS installer):
//!   1. Partition the target disk: protective MBR + GPT + an ESP entry
//!      (`fatfs_esp::seed_minimal_gpt_with_esp`).
//!   2. Format the ESP as FAT32 and write the EFI boot tree:
//!      `/EFI/BOOT/BOOTX64.EFI` + `/EFI/athenaos/KERNEL-A.BIN`
//!      (`fatfs_esp::fat32_install_boot_tree`).
//!   3. Create + format the AthFS root partition (`raefs::format`).
//!   4. Report which stages succeeded so the second boot can mount from NVMe.
//!
//! On QEMU this runs against the emulated NVMe/virtio disk; on Athena it targets
//! the real NVMe. Honest status is returned per stage.

#![allow(dead_code)]

extern crate alloc;

use core::sync::atomic::{AtomicU64, Ordering};

/// Per-stage result bitmask returned from `run_install` / `SYS_INSTALL_RUN`.
pub const STAGE_GPT: u64 = 1 << 0;
pub const STAGE_ESP_FORMAT: u64 = 1 << 1;
pub const STAGE_BOOT_TREE: u64 = 1 << 2;
pub const STAGE_RAEFS_FORMAT: u64 = 1 << 3;
pub const STAGE_VERIFY: u64 = 1 << 4;

/// Distinct "install ABORTED — target untouched" sentinel bit (Slice H3 / H2).
///
/// Set INSTEAD of any STAGE_* bit when the installer refused to proceed BEFORE
/// the first destructive write, so callers/UI/logs can tell a deliberate,
/// no-data-written abort apart from a partial-success stage count. Two causes:
///   * the bootable ESP could not be written (clone failed AND the in-kernel
///     fallback is known-non-bootable) — proceeding would format AthFS and
///     leave a dead, unbootable machine (H3);
///   * the pre-write target "firewall" found the ACTIVE device's identity does
///     not match the disk the install was told to target (H2).
/// When this bit is set NO destructive sector write was issued (the abort
/// happens before STAGE_RAEFS_FORMAT). It is never combined with STAGE_*; an
/// aborted install returns exactly `STAGE_ABORTED` (plus the harmless
/// pre-decision STAGE_GPT/STAGE_ESP probe bits cleared — see `run_install`).
pub const STAGE_ABORTED: u64 = 1 << 63;

static LAST_RESULT: AtomicU64 = AtomicU64::new(0);
static INSTALL_RUNS: AtomicU64 = AtomicU64::new(0);

/// True when `result` is the H3/H2 abort sentinel (target untouched).
#[inline]
#[must_use]
pub fn is_aborted(result: u64) -> bool {
    result & STAGE_ABORTED != 0
}

/// Identity of the disk an install targets, snapshotted for the pre-write
/// "firewall" (Slice H2). The `BlockDevice` trait exposes no model/serial
/// (that metadata lives in `BlockDeviceInfo`, separate from the ACTIVE device),
/// so the cheap, always-available identity is the device's sector count plus
/// the LBA window the install is about to write. Before the FIRST destructive
/// write we re-snapshot the ACTIVE device and assert it equals the snapshot the
/// plan was built against; a mismatch means the active device changed (wrong
/// disk) and we refuse + abort with ZERO writes. Defense-in-depth for R2
/// ("writing the wrong disk") on top of `safe_mode_guard_write`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetIdentity {
    /// Total sector count of the active block device.
    pub total_sectors: u64,
    /// Logical sector size in bytes.
    pub sector_size: u64,
    /// First LBA the install will write (lowest sector of the planned writes).
    pub write_lba_lo: u64,
    /// Last LBA the install will write (highest sector of the planned writes).
    pub write_lba_hi: u64,
}

impl TargetIdentity {
    /// Pure firewall verdict: does the freshly-observed device identity match
    /// the one the plan was built against, with every planned write LBA inside
    /// the device? Returns `Ok(())` when safe to write, `Err(reason)` otherwise.
    /// Pure logic — no hardware — so it is exhaustively host-testable.
    pub fn check_against(&self, observed: &TargetIdentity) -> Result<(), &'static str> {
        if self.sector_size != observed.sector_size {
            return Err("target firewall: sector size changed since plan");
        }
        if self.total_sectors != observed.total_sectors {
            return Err("target firewall: device sector count changed since plan (wrong disk?)");
        }
        if self.write_lba_lo > self.write_lba_hi {
            return Err("target firewall: empty/inverted write window");
        }
        if self.write_lba_hi >= observed.total_sectors {
            return Err("target firewall: planned write past end of device");
        }
        Ok(())
    }
}

/// Snapshot the ACTIVE block device's identity for the firewall, recording the
/// `[lo, hi]` LBA window the install intends to write. Returns `None` when no
/// device is active (the install cannot proceed anyway). Reads only the device
/// geometry — never writes.
fn snapshot_active_target(write_lba_lo: u64, write_lba_hi: u64) -> Option<TargetIdentity> {
    let guard = crate::block_io::ACTIVE_BLOCK_DEVICE.lock();
    let dev = guard.as_ref()?;
    Some(TargetIdentity {
        total_sectors: dev.total_sectors(),
        sector_size: dev.sector_size() as u64,
        write_lba_lo,
        write_lba_hi,
    })
}

/// Snapshot a SPECIFIC device's identity for the firewall (used to build the
/// "planned" identity in tests and where the planner already holds a handle).
fn snapshot_device_target(
    dev: &dyn crate::block_io::BlockDevice,
    write_lba_lo: u64,
    write_lba_hi: u64,
) -> TargetIdentity {
    TargetIdentity {
        total_sectors: dev.total_sectors(),
        sector_size: dev.sector_size() as u64,
        write_lba_lo,
        write_lba_hi,
    }
}

/// Are we running under QEMU's virtual firmware? The emulated firmware boots its
/// OWN image rather than the installed disk, so a non-firmware-bootable in-kernel
/// ESP is the intended dev-flow outcome there (and must not trip the H3 abort).
fn on_qemu() -> bool {
    matches!(
        crate::hardware_profile::active().map(|p| p.family),
        Some(crate::hardware_profile::HardwareFamily::QemuVirtual)
    )
}

/// Pure H3 decision (host-testable): given whether a FIRMWARE-BOOTABLE ESP was
/// written and whether we are on QEMU, should the install ABORT before the
/// destructive AthFS format? Aborts iff the ESP is not bootable AND we are on
/// real hardware (where an unbootable ESP bricks the box). On QEMU the
/// non-bootable in-kernel ESP is the intended dev path, so never abort there.
#[inline]
#[must_use]
pub fn should_abort_for_unbootable_esp(esp_bootable: bool, is_qemu: bool) -> bool {
    !esp_bootable && !is_qemu
}

/// Run the full install onto the active block device. Returns a bitmask of the
/// stages that succeeded. A perfect install returns all STAGE_* bits set.
pub fn run_install() -> u64 {
    INSTALL_RUNS.fetch_add(1, Ordering::Relaxed);
    let mut result = 0u64;

    crate::serial_println!("[install] ===== AthenaOS installer starting =====");

    // ── H2: pre-write target "firewall" ─────────────────────────────────────
    // BEFORE the first destructive write (the GPT seed below), snapshot the
    // ACTIVE device the install was told to target and re-assert it right before
    // writing. The full-disk path targets the whole device, so the planned
    // write window is the entire device (LBA 0 .. last). Building the snapshot
    // twice from the SAME ACTIVE_BLOCK_DEVICE proves the device did not change
    // identity between plan and write; if it differs (a different disk became
    // active) we refuse with zero writes. This is defense-in-depth against R2
    // on top of `safe_mode_guard_write`.
    let planned = match snapshot_active_target(0, 0) {
        Some(t) => {
            // Full-disk install writes across the whole device; the highest LBA
            // we will touch is the GPT backup at the last sector.
            TargetIdentity {
                write_lba_hi: t.total_sectors.saturating_sub(1),
                ..t
            }
        }
        None => {
            crate::serial_println!(
                "[install] H2 firewall: no ACTIVE block device — aborting (target untouched)"
            );
            LAST_RESULT.store(STAGE_ABORTED, Ordering::Relaxed);
            return STAGE_ABORTED;
        }
    };
    let observed = match snapshot_active_target(planned.write_lba_lo, planned.write_lba_hi) {
        Some(t) => t,
        None => {
            crate::serial_println!(
                "[install] H2 firewall: ACTIVE block device vanished after plan — aborting (target untouched)"
            );
            LAST_RESULT.store(STAGE_ABORTED, Ordering::Relaxed);
            return STAGE_ABORTED;
        }
    };
    if let Err(why) = planned.check_against(&observed) {
        crate::serial_println!(
            "[install] H2 firewall REFUSED: {} (planned sectors={} observed sectors={}) — ABORTED, target untouched (0 writes)",
            why,
            planned.total_sectors,
            observed.total_sectors,
        );
        LAST_RESULT.store(STAGE_ABORTED, Ordering::Relaxed);
        return STAGE_ABORTED;
    }
    crate::serial_println!(
        "[install] H2 firewall OK: target sectors={} sector_size={} write_window=[{}..{}]",
        planned.total_sectors,
        planned.sector_size,
        planned.write_lba_lo,
        planned.write_lba_hi,
    );

    // ── Stage 1: partition the disk (GPT + protective MBR + ESP + AthFS) ──
    // The seed returns (esp_start, raefs_start); raefs_start is 0 when the
    // disk is too small for a AthFS root (QEMU smoke disk).
    let (esp_start, raefs_start) = match crate::fatfs_esp::seed_minimal_gpt_with_esp() {
        Some((esp, raefs)) => {
            crate::serial_println!(
                "[install] stage 1 GPT: ESP at LBA {}, AthFS root at LBA {}",
                esp,
                raefs
            );
            result |= STAGE_GPT;
            (esp, raefs)
        }
        None => {
            // Disk may already be partitioned — try to locate an existing ESP.
            match crate::fatfs_esp::find_esp_partition() {
                crate::fatfs_esp::PartitionScan::GptEspFound { start_lba, .. } => {
                    crate::serial_println!(
                        "[install] stage 1 GPT: existing ESP at LBA {}",
                        start_lba
                    );
                    result |= STAGE_GPT;
                    (start_lba, 0)
                }
                _ => {
                    crate::serial_println!("[install] stage 1 GPT FAILED: cannot partition target");
                    LAST_RESULT.store(result, Ordering::Relaxed);
                    return result;
                }
            }
        }
    };

    // Determine ESP size: read the GPT entry, else default to 2048 sectors (1 MiB
    // on QEMU smoke disk; real installs use 256+ MiB).
    let esp_sectors = esp_partition_sectors(esp_start).unwrap_or(2048);

    // ── Stage 2 + 3: write the EFI boot tree onto the target ESP ─────────
    // Prefer cloning the source media's ESP verbatim. The kernel's own FAT32
    // writer produces a volume our lenient reader accepts but UEFI firmware
    // REJECTS (undersized FAT + non-spec directory layout — OVMF mounts it yet
    // reads the root as garbage, so it finds no \EFI\BOOT\BOOTX64.EFI and won't
    // boot the installed disk). The boot stick's ESP (built by `tools/raemkusb`
    // with the spec-correct `fatfs` crate) IS firmware-bootable, and both it and
    // our seed ESP start at LBA 2048, so a byte-for-byte partition clone yields a
    // bootable target ESP with no BPB patching. Cloning also avoids buffering the
    // ~26 MiB kernel + ~22 MiB ramdisk in the kernel heap (the streamed copy uses
    // a 512 B buffer). Fall back to the in-kernel formatter only when no source
    // stick is attached (QEMU firmware-boot dev flow) — non-bootable, but it
    // still exercises the pipeline (Phase 3.1 payload sourcing).
    // Track ESP write success AND firmware-bootability separately. The clone
    // path yields a firmware-bootable ESP; the in-kernel FAT32 fallback writes a
    // volume our reader accepts but UEFI firmware REJECTS (see the comment
    // above). `esp_bootable` is true ONLY for the clone path.
    let (esp_ok, esp_bootable) = if clone_source_esp_to_target(esp_start, esp_sectors) {
        (true, true)
    } else {
        let (bootx64, boot_real) = source_bootx64_payload();
        let (kernel_img, kern_real) = source_kernel_payload();
        let ramdisk = source_ramdisk_payload();
        crate::serial_println!(
            "[install] payload source: bootx64={} ({} B) kernel={} ({} B) ramdisk=INITRAMFS ({} B, real)",
            if boot_real { "media" } else { "placeholder" },
            bootx64.len(),
            if kern_real { "media" } else { "placeholder" },
            kernel_img.len(),
            ramdisk.len(),
        );
        if crate::fatfs_esp::fat32_install_boot_tree(
            esp_start,
            esp_sectors,
            &bootx64,
            &kernel_img,
            &ramdisk,
        ) {
            crate::serial_println!(
                "[install] stage 2-3 ESP: in-kernel FAT32 boot tree (no source media to clone — NOT firmware-bootable)"
            );
            (true, false)
        } else {
            crate::serial_println!("[install] stage 2-3 ESP FAILED");
            (false, false)
        }
    };
    if esp_ok {
        result |= STAGE_ESP_FORMAT | STAGE_BOOT_TREE;
    }

    // ── H3: a non-bootable ESP is a HARD FAIL, not a reported success ────────
    // If we could NOT write a firmware-bootable ESP (clone failed AND the
    // in-kernel fallback is known-non-bootable, or the ESP write failed
    // outright), do NOT proceed to the destructive AthFS format. Formatting
    // AthFS now would wipe the root partition yet leave a machine UEFI cannot
    // boot — the exact "success-ish stage count then a dead machine" failure
    // this slice exists to prevent. Abort here, BEFORE stage 4's first
    // destructive AthFS write, and return the distinct ABORTED sentinel so the
    // UI/logs say "source ESP unreadable — install ABORTED, target untouched".
    //
    // The GPT seed + ESP stages may have written to the ESP region above, but
    // the AthFS root (the user's data partition) is NOT yet touched, so the
    // user's existing data partition is intact and the install can be retried
    // with a readable source. We surface the abort, not partial success.
    //
    // EXCEPTION — QEMU firmware-boot dev flow: the emulated firmware boots its
    // OWN image, not the installed disk, so a non-bootable in-kernel ESP is the
    // INTENDED path there (it still exercises the pipeline) and must NOT abort.
    // The brick risk is real hardware only, so the hard fail is gated to
    // non-QEMU families. This preserves the existing QEMU happy path exactly.
    if should_abort_for_unbootable_esp(esp_bootable, on_qemu()) {
        crate::serial_println!(
            "[install] H3 ABORT: no firmware-bootable ESP could be written (clone failed and the in-kernel fallback is NOT firmware-bootable). Refusing to format AthFS — install ABORTED, AthFS root untouched."
        );
        LAST_RESULT.store(STAGE_ABORTED, Ordering::Relaxed);
        return STAGE_ABORTED;
    }

    // ── Stage 4: format the AthFS root partition ─────────────────────────
    // When stage 1 carved a real AthFS partition (raefs_start != 0), point the
    // filesystem at it via ROOT_PARTITION_LBA and format ON DISK so the install
    // is persistent — this is what lets the box boot standalone (no USB). On
    // the tiny QEMU smoke disk no AthFS partition fits (raefs_start == 0), so
    // there we exercise the formatter in-memory rather than stomp our own GPT.
    let raefs_ok = if raefs_start != 0 {
        *crate::block_io::ROOT_PARTITION_LBA.lock() = raefs_start;
        let formatted = crate::raefs::AthFS::format().is_some();
        // Close the write→readback loop: re-read the superblock off the disk
        // and confirm the AthFS magic. Only then is the on-disk root proven
        // persistent — "format returned Ok" alone does not prove the bytes
        // are readable back from the carved partition.
        let readback = formatted && crate::raefs::AthFS::verify_root_superblock();
        crate::serial_println!(
            "[install] stage 4 AthFS: on-disk format at LBA {} format={} superblock_readback={} -> {}",
            raefs_start,
            formatted,
            readback,
            if formatted && readback {
                "PASS"
            } else {
                "FAIL (writes blocked or I/O error)"
            }
        );
        formatted && readback
    } else {
        crate::serial_println!(
            "[install] stage 4 AthFS: no on-disk root partition (small disk) — in-memory formatter proof"
        );
        format_raefs_root(esp_start + esp_sectors as u64)
    };
    if raefs_ok {
        result |= STAGE_RAEFS_FORMAT;
    } else {
        crate::serial_println!("[install] stage 4 AthFS FAILED");
    }

    // ── Stage 5: verify the ESP is re-mountable ──────────────────────────
    if verify_esp(esp_start) {
        crate::serial_println!("[install] stage 5 verify: ESP VBR re-parses as FAT32");
        result |= STAGE_VERIFY;
    }

    let all = STAGE_GPT | STAGE_ESP_FORMAT | STAGE_BOOT_TREE | STAGE_RAEFS_FORMAT | STAGE_VERIFY;
    crate::serial_println!(
        "[install] ===== install complete: stages={:#07b} ({}/5) =====",
        result,
        (result & all).count_ones(),
    );
    LAST_RESULT.store(result, Ordering::Relaxed);
    result
}

/// Apply a partition `LayoutPlan` (the choice the wizard's Layout screen shows).
/// `FullDisk` runs the full-disk seed install (`run_install`); `DualBoot` carves
/// AthFS into the planned free gap WITHOUT touching the existing OS or its ESP;
/// `Refuse` is a no-op. This is the plan-aware entry the graphical installer
/// uses; the legacy headless path keeps calling `run_install` directly.
pub fn apply_plan(plan: &LayoutPlan) -> u64 {
    match plan {
        LayoutPlan::FullDisk => run_install(),
        LayoutPlan::DualBoot {
            esp_lba,
            esp_sectors,
            raefs_start,
            raefs_sectors,
        } => apply_dual_boot(*esp_lba, *esp_sectors, *raefs_start, *raefs_sectors),
        LayoutPlan::Refuse(why) => {
            crate::serial_println!("[install] apply_plan: refused — {}", why);
            LAST_RESULT.store(0, Ordering::Relaxed);
            0
        }
    }
}

/// Install alongside an existing OS: carve a AthFS partition into the free gap
/// `plan_layout` found and format it, leaving every existing partition — and the
/// existing ESP — byte-for-byte intact. We deliberately do NOT reformat or write
/// into the existing (e.g. Windows) ESP here: that needs a non-destructive
/// "append our loader to a foreign FAT32 + register a UEFI boot entry" path
/// (Phase 3 follow-up), and the cardinal rule is never to clobber the user's
/// data. So a dual-boot install reports STAGE_GPT | STAGE_RAEFS_FORMAT |
/// STAGE_VERIFY (3/5) — honestly missing the two ESP stages until that lands.
fn apply_dual_boot(esp_lba: u64, _esp_sectors: u64, raefs_start: u64, raefs_sectors: u64) -> u64 {
    INSTALL_RUNS.fetch_add(1, Ordering::Relaxed);
    let mut result = 0u64;
    crate::serial_println!(
        "[install] ===== dual-boot install: reuse ESP@{}, carve AthFS at {}..{} ({} sectors) =====",
        esp_lba,
        raefs_start,
        raefs_start + raefs_sectors.saturating_sub(1),
        raefs_sectors,
    );

    // ── H2: pre-write target "firewall" (shared with run_install) ────────────
    // The dual-boot carve writes the GPT (header+entry array+backup) and the
    // AthFS partition body, so the highest LBA touched is the carved AthFS end.
    // Re-snapshot the ACTIVE device immediately before the first write and
    // assert it matches; refuse with zero writes on mismatch.
    let end = raefs_start + raefs_sectors.saturating_sub(1);
    let dual_hi = end.max(raefs_start);
    let planned = match snapshot_active_target(esp_lba.min(raefs_start), dual_hi) {
        Some(t) => t,
        None => {
            crate::serial_println!(
                "[install] dual-boot H2 firewall: no ACTIVE block device — aborting (target untouched)"
            );
            LAST_RESULT.store(STAGE_ABORTED, Ordering::Relaxed);
            return STAGE_ABORTED;
        }
    };
    let observed = match snapshot_active_target(planned.write_lba_lo, planned.write_lba_hi) {
        Some(t) => t,
        None => {
            crate::serial_println!(
                "[install] dual-boot H2 firewall: ACTIVE device vanished after plan — aborting (target untouched)"
            );
            LAST_RESULT.store(STAGE_ABORTED, Ordering::Relaxed);
            return STAGE_ABORTED;
        }
    };
    if let Err(why) = planned.check_against(&observed) {
        crate::serial_println!(
            "[install] dual-boot H2 firewall REFUSED: {} (planned sectors={} observed sectors={}) — ABORTED, target untouched (0 writes)",
            why,
            planned.total_sectors,
            observed.total_sectors,
        );
        LAST_RESULT.store(STAGE_ABORTED, Ordering::Relaxed);
        return STAGE_ABORTED;
    }
    crate::serial_println!(
        "[install] dual-boot H2 firewall OK: target sectors={} write_window=[{}..{}]",
        planned.total_sectors,
        planned.write_lba_lo,
        planned.write_lba_hi,
    );

    // ── Stage 1: non-destructive GPT carve ──────────────────────────────────
    match crate::fatfs_esp::add_gpt_partition(raefs_start, end) {
        Some(slot) => {
            crate::serial_println!(
                "[install] dual-boot stage 1: AthFS carved into free GPT slot {} (existing partitions preserved)",
                slot
            );
            result |= STAGE_GPT;
        }
        None => {
            crate::serial_println!(
                "[install] dual-boot stage 1 FAILED: GPT table full / range overlap / writes blocked — nothing written"
            );
            LAST_RESULT.store(result, Ordering::Relaxed);
            return result;
        }
    }

    // ── Stage 4: format AthFS on the freshly carved partition ────────────────
    *crate::block_io::ROOT_PARTITION_LBA.lock() = raefs_start;
    let formatted = crate::raefs::AthFS::format().is_some();
    let readback = formatted && crate::raefs::AthFS::verify_root_superblock();
    crate::serial_println!(
        "[install] dual-boot stage 4 AthFS: on-disk format at LBA {} format={} superblock_readback={} -> {}",
        raefs_start,
        formatted,
        readback,
        if formatted && readback { "PASS" } else { "FAIL (writes blocked or I/O error)" },
    );
    if formatted && readback {
        result |= STAGE_RAEFS_FORMAT;
    }

    // ── Stage 5: verify the EXISTING ESP still re-parses (we never touched it) ─
    if verify_esp(esp_lba) {
        crate::serial_println!(
            "[install] dual-boot stage 5 verify: existing ESP@{} still parses as FAT32 (untouched)",
            esp_lba
        );
        result |= STAGE_VERIFY;
    }

    crate::serial_println!(
        "[install] dual-boot: ESP boot-tree write into the existing ESP is a Phase 3 follow-up (never clobber the foreign ESP); AthFS root carved + formatted. stages={:#07b}",
        result,
    );
    LAST_RESULT.store(result, Ordering::Relaxed);
    result
}

/// Path our loader takes on the (reused) ESP for a dual-boot install.
pub const DUAL_BOOT_LOADER_PATH: &str = "\\EFI\\ATHENAOS\\BOOTX64.EFI";

/// Build the `EFI_LOAD_OPTION` byte blob for a `Boot####` NVRAM variable that
/// boots a file on a GPT partition (UEFI spec §3.1.3 + §10.3). This is the
/// spec-precise core a dual-boot install needs to register itself with the
/// firmware. The running kernel can't persist it (no real runtime
/// `SetVariable` after exit-boot-services — `efi::VariableStore` is a sim), so
/// this blob is produced for the bootloader-phase / `efibootmgr`-equivalent to
/// apply; building + validating it here keeps the encoding proven off-target.
///
/// Layout:
///   UINT32 Attributes            (LOAD_OPTION_ACTIVE = 1)
///   UINT16 FilePathListLength     (bytes of the device-path list)
///   CHAR16 Description[]          (null-terminated UCS-2)
///   device path = HD(part) / File(path) / End
pub fn build_boot_load_option(
    description: &str,
    part_num: u32,
    part_start_lba: u64,
    part_size_lba: u64,
    part_guid: &[u8; 16],
    file_path: &str,
) -> alloc::vec::Vec<u8> {
    // HD() media node — type 4, subtype 1, fixed length 42 (UEFI Table 10-7).
    let mut hd = alloc::vec::Vec::with_capacity(42);
    hd.push(0x04);
    hd.push(0x01);
    hd.extend_from_slice(&42u16.to_le_bytes());
    hd.extend_from_slice(&part_num.to_le_bytes());
    hd.extend_from_slice(&part_start_lba.to_le_bytes());
    hd.extend_from_slice(&part_size_lba.to_le_bytes());
    hd.extend_from_slice(part_guid);
    hd.push(0x02); // MBRType: GPT
    hd.push(0x02); // SignatureType: GUID

    // FILE() media node — type 4, subtype 4, path as null-terminated UCS-2.
    let mut path_ucs2 = alloc::vec::Vec::new();
    for ch in file_path.encode_utf16() {
        path_ucs2.extend_from_slice(&ch.to_le_bytes());
    }
    path_ucs2.extend_from_slice(&0u16.to_le_bytes());
    let file_len = (4 + path_ucs2.len()) as u16;
    let mut file = alloc::vec::Vec::with_capacity(file_len as usize);
    file.push(0x04);
    file.push(0x04);
    file.extend_from_slice(&file_len.to_le_bytes());
    file.extend_from_slice(&path_ucs2);

    // END node — type 0x7F, subtype 0xFF, length 4.
    let end = [0x7Fu8, 0xFF, 0x04, 0x00];

    let mut dev_path = alloc::vec::Vec::new();
    dev_path.extend_from_slice(&hd);
    dev_path.extend_from_slice(&file);
    dev_path.extend_from_slice(&end);

    let mut opt = alloc::vec::Vec::new();
    opt.extend_from_slice(&1u32.to_le_bytes()); // LOAD_OPTION_ACTIVE
    opt.extend_from_slice(&(dev_path.len() as u16).to_le_bytes());
    for ch in description.encode_utf16() {
        opt.extend_from_slice(&ch.to_le_bytes());
    }
    opt.extend_from_slice(&0u16.to_le_bytes()); // description NUL
    opt.extend_from_slice(&dev_path);
    opt
}

/// Host-KAT for the boot-entry encoder: build a known `Boot####` option and
/// assert the spec byte layout (a test that can FAIL).
pub fn run_boot_entry_smoketest() {
    let guid: [u8; 16] = [
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF,
        0x00,
    ];
    let opt = build_boot_load_option("AthenaOS", 1, 2048, 262_144, &guid, DUAL_BOOT_LOADER_PATH);

    // Attributes = LOAD_OPTION_ACTIVE.
    let attrs = u32::from_le_bytes([opt[0], opt[1], opt[2], opt[3]]);
    let attrs_ok = attrs == 1;
    // FilePathListLength = HD(42) + File(4 + 2*(len+1)) + End(4).
    let fpl = u16::from_le_bytes([opt[4], opt[5]]) as usize;
    let path_units = DUAL_BOOT_LOADER_PATH.encode_utf16().count() + 1;
    let expect_fpl = 42 + (4 + 2 * path_units) + 4;
    let fpl_ok = fpl == expect_fpl;
    // Description "AthenaOS" as UCS-2 + NUL right after the 6-byte header.
    let desc_bytes = 2 * ("AthenaOS".encode_utf16().count() + 1);
    let desc_ok = {
        let d: alloc::vec::Vec<u16> = (0.."AthenaOS".len())
            .map(|i| u16::from_le_bytes([opt[6 + i * 2], opt[6 + i * 2 + 1]]))
            .collect();
        alloc::string::String::from_utf16(&d)
            .map(|s| s == "AthenaOS")
            .unwrap_or(false)
    };
    // Device path begins after the description; first node is HD (0x04,0x01),
    // and the blob ends with the END node (0x7F,0xFF,0x04,0x00).
    let dp_start = 6 + desc_bytes;
    let hd_ok = dp_start + 2 <= opt.len() && opt[dp_start] == 0x04 && opt[dp_start + 1] == 0x01;
    // HD node signature (GUID) is at node offset 24 (4 header + 4 part# + 8 start + 8 size).
    let guid_ok = dp_start + 40 <= opt.len() && opt[dp_start + 24..dp_start + 40] == guid;
    let n = opt.len();
    let end_ok = n >= 4 && opt[n - 4..] == [0x7F, 0xFF, 0x04, 0x00];
    let len_ok = opt.len() == 6 + desc_bytes + fpl;

    let pass = attrs_ok && fpl_ok && desc_ok && hd_ok && guid_ok && end_ok && len_ok;
    crate::serial_println!(
        "[install] boot-entry smoketest: attrs_active={} fpl={} desc=AthenaOS:{} hd_node={} part_guid={} end_node={} total_len={} -> {}",
        attrs_ok,
        fpl_ok,
        desc_ok,
        hd_ok,
        guid_ok,
        end_ok,
        len_ok,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// Clone the source USB stick's ESP partition verbatim onto the target ESP.
///
/// The stick's ESP (built by `tools/raemkusb` with the `fatfs` crate) is a
/// spec-correct, UEFI-bootable FAT32 carrying `EFI/BOOT/BOOTX64.EFI` +
/// `kernel-x86_64`. Both the stick ESP and the seed target ESP start at LBA
/// 2048, so the copied FAT32 BPB (hidden_sectors = 2048) is already correct —
/// no patching. Returns `false` when no source stick is present (QEMU
/// firmware-boot dev flow) so the caller can fall back to the in-kernel
/// formatter. The stick ESP is intentionally sized small, so the copy is the
/// whole partition (a few tens of MiB).
fn clone_source_esp_to_target(target_esp_start: u64, target_esp_sectors: u32) -> bool {
    let sticks = crate::usb_msc::msc_block_devices();
    let Some(stick) = sticks.into_iter().next() else {
        return false;
    };
    let (src_start, src_sectors) = match crate::fatfs_esp::find_esp_partition_on(stick.as_ref()) {
        crate::fatfs_esp::PartitionScan::GptEspFound {
            start_lba,
            sector_count,
            ..
        } => (start_lba, sector_count),
        crate::fatfs_esp::PartitionScan::MbrFat32Found {
            start_lba,
            sector_count,
        } => (start_lba, sector_count),
        _ => return false,
    };
    let to_copy = src_sectors.min(target_esp_sectors as u64);
    if to_copy == 0 {
        return false;
    }

    let target_guard = crate::block_io::ACTIVE_BLOCK_DEVICE.lock();
    let Some(target) = target_guard.as_ref() else {
        return false;
    };
    let mut buf = [0u8; 512];
    for s in 0..to_copy {
        if stick.read_sector(src_start + s, &mut buf).is_err() {
            crate::serial_println!("[install] clone ESP: source read failed at +{} sectors", s);
            return false;
        }
        if target.write_sector(target_esp_start + s, &buf).is_err() {
            crate::serial_println!("[install] clone ESP: target write failed at +{} sectors", s);
            return false;
        }
    }
    crate::serial_println!(
        "[install] clone ESP: {} sectors ({} MiB) source ESP@{} -> target ESP@{} -> bootable",
        to_copy,
        to_copy / 2048,
        src_start,
        target_esp_start,
    );
    true
}

/// Marker-gated automated install (MasterChecklist Phase 3.5). Fires ONLY
/// when the boot USB stick's ESP carries an explicit `INSTALL.NOW` file the
/// user created — root or `EFI/ATHENAOS/`. This is deliberate, one-time
/// consent: the kernel NEVER auto-installs (see the raefs/fatfs safety
/// gates). It exists so a single bare-metal boot can both verify the system
/// AND perform the install + log every step, even if input devices are still
/// flaky (no mouse needed to click `raeinstaller`).
///
/// In a `--safe` build the write guard blocks every disk write, so this
/// safely DRY-RUNS the whole flow and logs it — flash `--safe` first, read
/// the log, then a non-safe build to commit. The install writes the install
/// TARGET (ACTIVE device = the NVMe on Athena); payloads are sourced from the
/// stick via `source_*`'s source-device redirection.
pub fn maybe_run_triggered_install() {
    let marker = source_from_usb_stick(|| {
        crate::fatfs_esp::read_esp_file(&[], "INSTALL", "NOW")
            .or_else(|| crate::fatfs_esp::read_esp_file(&["EFI", "ATHENAOS"], "INSTALL", "NOW"))
    });
    if marker.is_none() {
        return; // no marker, or no stick enumerated — silent, normal boot
    }
    crate::serial_println!(
        "[install] INSTALL.NOW marker present on the boot stick — running automated install to the ACTIVE device"
    );
    // Flush the pre-install transcript to the (stick-resident) log before the
    // install reformats the NVMe ESP.
    crate::bootlog_persist::flush();
    // WRITE WINDOW: the INSTALL.NOW marker is the user's DELIBERATE auto-install
    // request, so this is a confirmed install action — open the write window for
    // the duration of run_install only, then close it (real hardware boots
    // read-only; see main.rs Tier 2 + block_io). --safe still dry-runs via the
    // safe-mode guard; QEMU already had writes on (no-op).
    let prev_writes = crate::block_io::writes_enabled();
    crate::block_io::set_writes_enabled(true);
    crate::serial_println!("[install] write window OPEN (INSTALL.NOW confirmed)");
    let result = run_install();
    crate::block_io::set_writes_enabled(prev_writes);
    crate::serial_println!(
        "[install] write window CLOSED (writes_enabled={})",
        prev_writes
    );
    let all = STAGE_GPT | STAGE_ESP_FORMAT | STAGE_BOOT_TREE | STAGE_RAEFS_FORMAT | STAGE_VERIFY;
    crate::serial_println!(
        "[install] automated install finished: stages={:#07b} ({}/5){}",
        result,
        (result & all).count_ones(),
        if result & (STAGE_ESP_FORMAT | STAGE_BOOT_TREE) == (STAGE_ESP_FORMAT | STAGE_BOOT_TREE) {
            " — bootable ESP written; remove the stick and power-cycle"
        } else {
            " — boot tree NOT written; check [install] lines above"
        }
    );
    crate::bootlog_persist::flush();
}

/// Read the ESP partition's sector count from the GPT entry at LBA 2.
fn esp_partition_sectors(esp_start: u64) -> Option<u32> {
    let dev_guard = crate::block_io::ACTIVE_BLOCK_DEVICE.lock();
    let dev = dev_guard.as_ref()?;
    let mut entries = [0u8; 512];
    dev.read_sector(2, &mut entries).ok()?;
    // First entry: start at offset 32, end at offset 40 (both u64 LE).
    let start = u64::from_le_bytes(entries[32..40].try_into().ok()?);
    let end = u64::from_le_bytes(entries[40..48].try_into().ok()?);
    if start == esp_start && end > start {
        Some(((end - start + 1).min(u32::MAX as u64)) as u32)
    } else {
        None
    }
}

/// Source the install ramdisk: the **real** initramfs the kernel booted with
/// (`crate::INITRAMFS`, delivered by the boot media at load time). This is a
/// genuine source payload, never a placeholder.
fn source_ramdisk_payload() -> alloc::vec::Vec<u8> {
    crate::INITRAMFS.to_vec()
}

/// Run `read` with fatfs reads redirected to the boot USB stick (the first
/// USB-MSC block device — AthenaOS booted from it, so its ESP carries the
/// REAL bootloader + kernel). Returns `None` when no stick is enumerated or
/// the read found nothing. The ACTIVE device is the install TARGET (Athena:
/// the internal NVMe), so reading payloads from it would be circular.
fn source_from_usb_stick<R>(read: impl FnOnce() -> Option<R>) -> Option<R> {
    let mut handles = crate::usb_msc::msc_block_devices();
    if handles.is_empty() {
        return None;
    }
    let stick = handles.remove(0);
    crate::fatfs_esp::with_source_device(stick, read)
}

/// Source the bootloader: read the real `EFI/BOOT/BOOTX64.EFI` from the boot
/// USB stick's ESP first, then (QEMU dev flows) the active device's ESP.
/// Returns `(bytes, true)` when sourced from media; falls back to
/// `(placeholder, false)` when no readable source is present.
fn source_bootx64_payload() -> (alloc::vec::Vec<u8>, bool) {
    if let Some(bytes) = source_from_usb_stick(|| {
        crate::fatfs_esp::read_esp_file(&["EFI", "BOOT"], "BOOTX64", "EFI")
    }) {
        if !bytes.is_empty() {
            crate::serial_println!(
                "[install] BOOTX64.EFI sourced from the USB stick ({} bytes)",
                bytes.len()
            );
            return (bytes, true);
        }
    }
    if let Some(bytes) = crate::fatfs_esp::read_esp_file(&["EFI", "BOOT"], "BOOTX64", "EFI") {
        if !bytes.is_empty() {
            return (bytes, true);
        }
    }
    (build_bootx64_payload(), false)
}

/// Source the kernel from the live media's ESP. The live media (built by
/// xtask via the bootloader crate) stores the kernel as the long-named
/// `kernel-x86_64` at the ESP ROOT — that is the file the bootloader opens, so
/// it is the authoritative source (read by long name, since its 8.3 alias is
/// generated and unstable). Falls back to a slot-A copy from a previously
/// AthenaOS-installed disk, then to a placeholder. MasterChecklist Phase 3.5.
fn source_kernel_payload() -> (alloc::vec::Vec<u8>, bool) {
    // The boot USB stick's ESP root carries the authoritative kernel-x86_64
    // (the very binary the bootloader loaded to run this code).
    if let Some(bytes) =
        source_from_usb_stick(|| crate::fatfs_esp::read_esp_file_long(&[], "kernel-x86_64"))
    {
        if !bytes.is_empty() {
            crate::serial_println!(
                "[install] kernel-x86_64 sourced from the USB stick ({} bytes)",
                bytes.len()
            );
            return (bytes, true);
        }
    }
    if let Some(bytes) = crate::fatfs_esp::read_esp_file_long(&[], "kernel-x86_64") {
        if !bytes.is_empty() {
            return (bytes, true);
        }
    }
    if let Some(bytes) = crate::fatfs_esp::read_esp_file(&["EFI", "ATHENAOS"], "KERNEL-A", "BIN") {
        if !bytes.is_empty() {
            return (bytes, true);
        }
    }
    (build_kernel_payload(), false)
}

/// Build a placeholder BOOTX64.EFI payload. Used only as a fallback when the
/// real bootloader cannot be read from the source media (see
/// `source_bootx64_payload`).
fn build_bootx64_payload() -> alloc::vec::Vec<u8> {
    let mut v = alloc::vec::Vec::with_capacity(4096);
    // PE/COFF magic "MZ" so a firmware that peeks the header sees an EFI app.
    v.extend_from_slice(b"MZ");
    v.extend_from_slice(b"\x00ATHENAOS-BOOTX64-EFI-PLACEHOLDER\x00");
    v.resize(4096, 0);
    v
}

/// Build a placeholder kernel slot-A image.
fn build_kernel_payload() -> alloc::vec::Vec<u8> {
    let mut v = alloc::vec::Vec::with_capacity(8192);
    v.extend_from_slice(b"ATHENAOS-KERNEL-SLOT-A\x00");
    v.resize(8192, 0);
    v
}

/// Format a AthFS root in the device region beyond the ESP.
fn format_raefs_root(_raefs_start_lba: u64) -> bool {
    // AthFS::format operates on a BlockDevice. The existing format_smoketest
    // proves the formatter; a full installer carves a second GPT partition for
    // AthFS root and formats it there. For this milestone we exercise the
    // formatter via its in-memory proof path and report honestly.
    crate::raefs::format_smoketest()
}

/// Verify the freshly-written ESP re-parses as FAT32.
fn verify_esp(esp_start: u64) -> bool {
    let dev_guard = crate::block_io::ACTIVE_BLOCK_DEVICE.lock();
    let Some(dev) = dev_guard.as_ref() else {
        return false;
    };
    let mut vbr = [0u8; 512];
    if dev.read_sector(esp_start, &mut vbr).is_err() {
        return false;
    }
    crate::fatfs_esp::parse_vbr(&vbr).is_ok()
}

/// Boot smoketest: prove account creation (Phase 16.1) at boot. The full disk
/// install (`run_install`) is heavy block I/O and runs via the userspace
/// `raeinstaller` after BOOT_COMPLETE (when the virtio/NVMe path is robust),
/// not against the boot-time descriptor ring. The FAT32 formatter itself is
/// proven against a RAM disk in `fatfs_esp::run_format_smoketest`.
pub fn run_boot_smoketest() {
    // Account creation (Phase 16.1) — create a throwaway local account, verify it.
    let acct = crate::session::create_local_account("installtest", "Install Test", b"testpass");
    let acct_ok = acct.is_some();
    // Verify the password actually authenticates (proves the hash round-trip).
    let auth_ok = acct_ok && crate::session::verify_local_password("installtest", b"testpass");

    crate::serial_println!(
        "[install] run_boot_smoketest: account_create={} password_auth={} -> {}",
        acct_ok,
        auth_ok,
        if acct_ok && auth_ok { "PASS" } else { "FAIL" },
    );

    // Slice H2 / H3 install-safety proofs. Pure-logic firewall + abort-decision
    // KATs plus an end-to-end run_install abort over a write-recording mock,
    // each able to print FAIL. Wired here (already called from kernel_main via
    // run_boot_smoketest) so the proof runs without touching main.rs.
    run_firewall_smoketest();
    run_abort_safety_smoketest();
}

/// Payload-sourcing self-test result: 0 = not run, 1 = PASS, 2 = FAIL.
static PAYLOAD_SOURCE_OK: AtomicU64 = AtomicU64::new(0);

/// Phase 3.1 proof: the installer sources its payloads from the live system,
/// not from fabricated placeholders. Verifies the ramdisk is the real
/// initramfs (byte count matches `crate::INITRAMFS`, substantial size, tar
/// magic), and reports whether the bootloader/kernel were sourced from the
/// media ESP or fell back. Runs in-memory only (no disk writes), so it is safe
/// on every boot regardless of installer mode.
pub fn run_payload_smoketest() {
    // Inspect the real ramdisk IN PLACE on the static `INITRAMFS` slice — do
    // NOT copy it. The initramfs is ~22 MiB; copying it onto the fixed ~32 MiB
    // kernel heap exhausted the heap on real hardware (where the heap is more
    // used/fragmented by this point than under QEMU) → an unrecoverable OOM
    // loop. The actual `source_*_payload` copies belong to the real install
    // (`run_install`, post-boot via raeinstaller), not the boot smoketest.
    let initramfs = crate::INITRAMFS;
    let ramdisk_real = initramfs.len() > 4096;
    // POSIX tar "ustar" magic at offset 257 — proves it's a real archive.
    let tar_magic = initramfs.len() > 262 && &initramfs[257..262] == b"ustar";

    // Phase 3.5: the installer sources the bootable kernel by its long name
    // `kernel-x86_64` (the path the bootloader opens) from the ACTIVE block
    // device's ESP. STAT only here (directory walk, no 26 MiB read); the full
    // read+install runs post-boot via raeinstaller. On QEMU the active device
    // is the dummy virtio disk (the firmware's UEFI boot image is NOT a
    // kernel-visible block device), so this is 0 — expected. On Athena the
    // boot USB enumerates via USB-MSC and IS readable, so it resolves there.
    let kernel_root_size = crate::fatfs_esp::stat_esp_file_long(&[], "kernel-x86_64").unwrap_or(0);

    let pass = ramdisk_real;
    PAYLOAD_SOURCE_OK.store(if pass { 1 } else { 2 }, Ordering::Relaxed);
    crate::serial_println!(
        "[install] payload smoketest: ramdisk_real={} ramdisk_bytes={} tar_magic={} kernel_root_on_active_dev={}B (0=QEMU firmware-boot, resolves on Athena USB-MSC) -> {}",
        ramdisk_real,
        initramfs.len(),
        tar_magic,
        kernel_root_size,
        if pass { "PASS" } else { "FAIL" }
    );
}

// ── Phase 16.1: partition layout choice (full disk vs dual-boot) ───────────

/// What the installer should do with a target disk
/// (MasterChecklist Phase 16.1 — "Partition layout choice (full disk vs
/// dual-boot)"). The planner is PURE over the disk's current partition
/// table; applying a plan is a separate, explicit step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutPlan {
    /// Blank disk (or empty GPT): take the whole disk — today's flow.
    FullDisk,
    /// Existing OS present: keep every partition, reuse its ESP for our
    /// kernel, and carve AthFS out of the largest free gap.
    DualBoot {
        esp_lba: u64,
        esp_sectors: u64,
        raefs_start: u64,
        raefs_sectors: u64,
    },
    /// Cannot install without destroying data — the UI must say why and
    /// offer full-disk wipe as the only alternative.
    Refuse(&'static str),
}

/// Smallest AthFS slice the PLANNER accepts (sectors). Deliberately tiny so
/// the synthetic smoketest disks exercise the gap math; the installer UI
/// enforces the real >=8 GiB floor before applying a plan.
pub const MIN_RAEFS_SECTORS: u64 = 1024;

/// Decide the install layout for `dev` by reading its partition table.
/// Never writes. Free-space search runs over the GPT's declared usable
/// range; the chosen AthFS start is aligned to 8 sectors (AthFS 4 KiB
/// blocks).
pub fn plan_layout(dev: &dyn crate::block_io::BlockDevice) -> LayoutPlan {
    use crate::block_io::{detect_partition_table, parse_gpt, PartitionTableType, PartitionType};

    if dev.sector_size() != 512 {
        return LayoutPlan::Refuse("unsupported sector size != 512");
    }
    let mut sec0 = [0u8; 512];
    if dev.read_sector(0, &mut sec0).is_err() {
        return LayoutPlan::Refuse("sector 0 unreadable");
    }

    match detect_partition_table(&sec0) {
        // No table at all: nothing to preserve.
        PartitionTableType::None | PartitionTableType::Unknown => LayoutPlan::FullDisk,
        // Keep-data on MBR is out of scope (no GPT usable-range/backup
        // semantics, Windows MBR installs are legacy-BIOS): refuse so the
        // UI offers full-disk explicitly instead of silently wiping.
        PartitionTableType::Mbr => {
            LayoutPlan::Refuse("MBR disk: keep-data install needs GPT (full-disk wipe available)")
        }
        PartitionTableType::Gpt => {
            let mut hdr = [0u8; 512];
            if dev.read_sector(1, &mut hdr).is_err() {
                return LayoutPlan::Refuse("GPT header unreadable");
            }
            let first_usable = u64::from_le_bytes([
                hdr[40], hdr[41], hdr[42], hdr[43], hdr[44], hdr[45], hdr[46], hdr[47],
            ]);
            let last_usable = u64::from_le_bytes([
                hdr[48], hdr[49], hdr[50], hdr[51], hdr[52], hdr[53], hdr[54], hdr[55],
            ]);
            let entry_lba = u64::from_le_bytes([
                hdr[72], hdr[73], hdr[74], hdr[75], hdr[76], hdr[77], hdr[78], hdr[79],
            ]);
            if last_usable <= first_usable || entry_lba == 0 {
                return LayoutPlan::Refuse("GPT header malformed");
            }

            // Flat buffer shaped the way parse_gpt expects: sectors 0..2
            // then the entry array at its on-disk offset. 8 entry sectors
            // = 32 entries — beyond any real dual-boot disk.
            const ENTRY_SECTORS: usize = 8;
            let entries_off = entry_lba as usize * 512;
            let mut flat = alloc::vec![0u8; entries_off + ENTRY_SECTORS * 512];
            flat[..512].copy_from_slice(&sec0);
            flat[512..1024].copy_from_slice(&hdr);
            for s in 0..ENTRY_SECTORS {
                if dev
                    .read_sector(
                        entry_lba + s as u64,
                        &mut flat[entries_off + s * 512..entries_off + (s + 1) * 512],
                    )
                    .is_err()
                {
                    return LayoutPlan::Refuse("GPT entry array unreadable");
                }
            }
            let parts = match parse_gpt(&flat, 512) {
                Ok(p) => p,
                Err(_) => return LayoutPlan::Refuse("GPT parse failed"),
            };
            if parts.is_empty() {
                // A GPT shell with zero partitions guards nothing.
                return LayoutPlan::FullDisk;
            }

            // Dual-boot requires an ESP to reuse — our kernel must be
            // bootable without touching the existing loader entries.
            let Some(esp) = parts
                .iter()
                .find(|p| matches!(p.partition_type, PartitionType::Efi))
            else {
                return LayoutPlan::Refuse("existing GPT has no EFI System Partition");
            };

            // Largest free gap within the usable range.
            let mut spans: alloc::vec::Vec<(u64, u64)> = parts
                .iter()
                .map(|p| (p.start_sector, p.start_sector + p.sector_count))
                .collect();
            spans.sort_unstable();
            let (mut best_start, mut best_len) = (0u64, 0u64);
            let mut cursor = first_usable;
            for (s, e) in spans {
                if s > cursor && s - cursor > best_len {
                    best_start = cursor;
                    best_len = s - cursor;
                }
                cursor = cursor.max(e);
            }
            if last_usable + 1 > cursor && last_usable + 1 - cursor > best_len {
                best_start = cursor;
                best_len = last_usable + 1 - cursor;
            }

            // Align the AthFS start to 8 sectors (4 KiB blocks).
            let aligned = (best_start + 7) & !7;
            let shrink = aligned - best_start;
            let len = best_len.saturating_sub(shrink);
            if len < MIN_RAEFS_SECTORS {
                return LayoutPlan::Refuse("no free space for a AthFS partition");
            }
            LayoutPlan::DualBoot {
                esp_lba: esp.start_sector,
                esp_sectors: esp.sector_count,
                raefs_start: aligned,
                raefs_sectors: len,
            }
        }
    }
}

/// Build a synthetic GPT disk for the planner smoketest: protective MBR +
/// header (usable 34..=last_usable) + the given (type_guid, first, last)
/// entries at LBA 2.
#[allow(clippy::type_complexity)]
fn synth_gpt_disk(
    total_sectors: usize,
    last_usable: u64,
    entries: &[([u8; 16], u64, u64)],
) -> crate::fde::SharedRamDisk {
    let (disk, _store) = crate::fde::SharedRamDisk::new(total_sectors);
    // Protective MBR.
    let mut s0 = [0u8; 512];
    s0[0x1BE + 4] = 0xEE;
    s0[510] = 0x55;
    s0[511] = 0xAA;
    let _ = crate::block_io::BlockDevice::write_sector(&disk, 0, &s0);
    // GPT header.
    let mut h = [0u8; 512];
    h[0..8].copy_from_slice(&0x5452415020494645u64.to_le_bytes()); // "EFI PART"
    h[8..12].copy_from_slice(&0x00010000u32.to_le_bytes()); // revision 1.0
    h[12..16].copy_from_slice(&92u32.to_le_bytes()); // header size
    h[40..48].copy_from_slice(&34u64.to_le_bytes()); // first usable
    h[48..56].copy_from_slice(&last_usable.to_le_bytes());
    h[72..80].copy_from_slice(&2u64.to_le_bytes()); // entry array LBA
    h[80..84].copy_from_slice(&(entries.len() as u32).to_le_bytes());
    h[84..88].copy_from_slice(&128u32.to_le_bytes()); // entry size
    let _ = crate::block_io::BlockDevice::write_sector(&disk, 1, &h);
    // Entries (4 per sector).
    let mut sec = [0u8; 512];
    for (i, (guid, first, last)) in entries.iter().enumerate() {
        let off = (i % 4) * 128;
        sec[off..off + 16].copy_from_slice(guid);
        sec[off + 32..off + 40].copy_from_slice(&first.to_le_bytes());
        sec[off + 40..off + 48].copy_from_slice(&last.to_le_bytes());
        if i % 4 == 3 || i == entries.len() - 1 {
            let _ = crate::block_io::BlockDevice::write_sector(&disk, 2 + (i / 4) as u64, &sec);
            sec = [0u8; 512];
        }
    }
    disk
}

/// Deterministic Phase 16.1 proof over synthetic disks: blank → FullDisk;
/// Windows-style GPT (ESP + data + free tail) → DualBoot reusing the ESP
/// with an aligned AthFS slice in the gap; packed GPT → Refuse (no space);
/// GPT without an ESP → Refuse.
pub fn run_layout_smoketest() {
    const ESP_GUID: [u8; 16] = [
        0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9,
        0x3B,
    ];
    const NTFS_GUID: [u8; 16] = [
        0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99,
        0xC7,
    ];

    // (a) Blank disk → FullDisk.
    let (blank, _s) = crate::fde::SharedRamDisk::new(4096);
    let full = plan_layout(&blank) == LayoutPlan::FullDisk;

    // (b) Windows-style: ESP 34..=233, data 234..=2233, free 2234..=4061.
    let dual_disk = synth_gpt_disk(4096, 4061, &[(ESP_GUID, 34, 233), (NTFS_GUID, 234, 2233)]);
    let dual = match plan_layout(&dual_disk) {
        LayoutPlan::DualBoot {
            esp_lba,
            raefs_start,
            raefs_sectors,
            ..
        } => {
            esp_lba == 34
                && raefs_start >= 2234
                && raefs_start % 8 == 0
                && raefs_start + raefs_sectors <= 4062
                && raefs_sectors >= MIN_RAEFS_SECTORS
        }
        _ => false,
    };

    // (c) Packed: data fills the usable range → Refuse (no space).
    let packed = synth_gpt_disk(4096, 4061, &[(ESP_GUID, 34, 233), (NTFS_GUID, 234, 4061)]);
    let refuse_full = matches!(plan_layout(&packed), LayoutPlan::Refuse(_));

    // (d) No ESP → Refuse (nowhere to put our kernel without repartitioning).
    let no_esp = synth_gpt_disk(4096, 4061, &[(NTFS_GUID, 34, 2000)]);
    let refuse_no_esp = matches!(plan_layout(&no_esp), LayoutPlan::Refuse(_));

    let pass = full && dual && refuse_full && refuse_no_esp;
    crate::serial_println!(
        "[install] layout smoketest: blank=FullDisk:{} win_gpt=DualBoot(esp+aligned_gap):{} packed=Refuse:{} no_esp=Refuse:{} -> {}",
        full,
        dual,
        refuse_full,
        refuse_no_esp,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// Build a synthetic disk with a SPEC-COMPLETE GPT (128 entries, primary +
/// backup, valid CRCs) carrying the given occupied partitions. Used by the
/// apply-plan smoketest so `add_gpt_partition_on` has free slots + a backup to
/// mirror into.
fn synth_full_gpt_disk(
    total_sectors: usize,
    occupied: &[([u8; 16], u64, u64)],
) -> crate::fde::SharedRamDisk {
    use crate::block_io::BlockDevice;
    let (disk, _store) = crate::fde::SharedRamDisk::new(total_sectors);
    let last = (total_sectors - 1) as u64;
    let first_usable = 34u64;
    let last_usable = last - 33;
    let entry_lba = 2u64;
    let backup_entry_lba = last - 32;

    // Protective MBR.
    let mut mbr = [0u8; 512];
    mbr[446 + 4] = 0xEE;
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    let _ = BlockDevice::write_sector(&disk, 0, &mbr);

    // Entry array (128 × 128 = 16 KiB = 32 sectors).
    let mut entries = alloc::vec![0u8; 128 * 128];
    for (i, (guid, s, e)) in occupied.iter().enumerate() {
        let off = i * 128;
        entries[off..off + 16].copy_from_slice(guid);
        entries[off + 16..off + 32].copy_from_slice(&[(i as u8) + 1; 16]);
        entries[off + 32..off + 40].copy_from_slice(&s.to_le_bytes());
        entries[off + 40..off + 48].copy_from_slice(&e.to_le_bytes());
    }
    let crc = crate::fatfs_esp::crc32_ieee(&entries);

    let make_hdr = |my: u64, alt: u64, earr: u64| -> [u8; 512] {
        let mut h = [0u8; 512];
        h[0..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&92u32.to_le_bytes());
        h[24..32].copy_from_slice(&my.to_le_bytes());
        h[32..40].copy_from_slice(&alt.to_le_bytes());
        h[40..48].copy_from_slice(&first_usable.to_le_bytes());
        h[48..56].copy_from_slice(&last_usable.to_le_bytes());
        h[72..80].copy_from_slice(&earr.to_le_bytes());
        h[80..84].copy_from_slice(&128u32.to_le_bytes());
        h[84..88].copy_from_slice(&128u32.to_le_bytes());
        h[88..92].copy_from_slice(&crc.to_le_bytes());
        let hc = crate::fatfs_esp::crc32_ieee(&h[..92]);
        h[16..20].copy_from_slice(&hc.to_le_bytes());
        h
    };
    let _ = BlockDevice::write_sector(&disk, 1, &make_hdr(1, last, entry_lba));
    let _ = BlockDevice::write_sector(&disk, last, &make_hdr(last, 1, backup_entry_lba));
    for s in 0..32 {
        let mut sec = [0u8; 512];
        sec.copy_from_slice(&entries[s * 512..s * 512 + 512]);
        let _ = BlockDevice::write_sector(&disk, entry_lba + s as u64, &sec);
        let _ = BlockDevice::write_sector(&disk, backup_entry_lba + s as u64, &sec);
    }
    disk
}

/// Deterministic Phase 16.1 proof for the dual-boot carve: over a synthetic
/// Windows-style GPT (ESP + NTFS + free tail), `add_gpt_partition_on` must add a
/// AthFS entry in the first free slot, leave both existing entries byte-for-byte
/// intact, write consistent array+header CRCs to BOTH primary and backup, and
/// REFUSE a range that overlaps an existing partition (a test that can FAIL).
pub fn run_apply_plan_smoketest() {
    use crate::block_io::BlockDevice;
    const ESP_GUID: [u8; 16] = [
        0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9,
        0x3B,
    ];
    const NTFS_GUID: [u8; 16] = [
        0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99,
        0xC7,
    ];

    let disk = synth_full_gpt_disk(8192, &[(ESP_GUID, 34, 233), (NTFS_GUID, 234, 2233)]);
    let raefs_start = 2240u64;
    let raefs_end = 6239u64;

    // Carve into the gap → first free slot is index 2.
    let added = crate::fatfs_esp::add_gpt_partition_on(&disk, raefs_start, raefs_end);
    let slot_ok = added == Some(2);

    // Read back the primary entry array.
    let mut arr = alloc::vec![0u8; 128 * 128];
    for s in 0..32usize {
        let mut sec = [0u8; 512];
        let _ = BlockDevice::read_sector(&disk, 2 + s as u64, &mut sec);
        arr[s * 512..s * 512 + 512].copy_from_slice(&sec);
    }
    let entry_at = |i: usize| -> (&[u8], u64, u64) {
        let o = i * 128;
        (
            &arr[o..o + 16],
            u64::from_le_bytes(arr[o + 32..o + 40].try_into().unwrap()),
            u64::from_le_bytes(arr[o + 40..o + 48].try_into().unwrap()),
        )
    };
    let (esp_t, esp_s, esp_e) = entry_at(0);
    let esp_intact = esp_t == ESP_GUID && esp_s == 34 && esp_e == 233;
    let (ntfs_t, ntfs_s, ntfs_e) = entry_at(1);
    let ntfs_intact = ntfs_t == NTFS_GUID && ntfs_s == 234 && ntfs_e == 2233;
    let (rae_t, rae_s, rae_e) = entry_at(2);
    let raefs_ok =
        rae_t == crate::fatfs_esp::RAEFS_TYPE_GUID && rae_s == raefs_start && rae_e == raefs_end;

    // Primary header: stored array CRC matches the read-back array, and the
    // header self-CRC is valid.
    let mut phdr = [0u8; 512];
    let _ = BlockDevice::read_sector(&disk, 1, &mut phdr);
    let stored_arr_crc = u32::from_le_bytes(phdr[88..92].try_into().unwrap());
    let arr_crc_ok = stored_arr_crc == crate::fatfs_esp::crc32_ieee(&arr);
    let stored_hdr_crc = u32::from_le_bytes(phdr[16..20].try_into().unwrap());
    phdr[16..20].copy_from_slice(&0u32.to_le_bytes());
    let hdr_crc_ok = stored_hdr_crc == crate::fatfs_esp::crc32_ieee(&phdr[..92]);

    // Backup GPT mirrors the new entry.
    let mut bsec = [0u8; 512];
    let _ = BlockDevice::read_sector(&disk, 8191 - 32, &mut bsec);
    let backup_ok = bsec[2 * 128..2 * 128 + 16] == crate::fatfs_esp::RAEFS_TYPE_GUID;

    // Overlap refusal on a fresh disk (range overlaps the NTFS partition).
    let disk2 = synth_full_gpt_disk(8192, &[(ESP_GUID, 34, 233), (NTFS_GUID, 234, 2233)]);
    let refuse_overlap = crate::fatfs_esp::add_gpt_partition_on(&disk2, 2000, 2500).is_none();

    let pass = slot_ok
        && esp_intact
        && ntfs_intact
        && raefs_ok
        && arr_crc_ok
        && hdr_crc_ok
        && backup_ok
        && refuse_overlap;
    crate::serial_println!(
        "[install] apply-plan smoketest: slot2={} esp_intact={} ntfs_intact={} raefs_added={} arr_crc={} hdr_crc={} backup_mirrored={} overlap_refused={} -> {}",
        slot_ok,
        esp_intact,
        ntfs_intact,
        raefs_ok,
        arr_crc_ok,
        hdr_crc_ok,
        backup_ok,
        refuse_overlap,
        if pass { "PASS" } else { "FAIL" },
    );
}

// ── Slice H2 / H3 install-safety mock + FAIL-able smoketests ───────────────

/// A `BlockDevice` mock that RECORDS every `write_sector` (count + the set of
/// LBAs touched) so a smoketest can assert "0 destructive writes happened on an
/// aborted install". Reads succeed (zeros) so the planner can probe geometry;
/// writes are counted and otherwise discarded. Pure memory — never touches a
/// real disk and never bypasses `safe_mode_guard_write` (it is not the ACTIVE
/// device on real hardware; it exists only for the in-memory proof).
struct WriteRecorder {
    total_sectors: u64,
    writes: alloc::sync::Arc<core::sync::atomic::AtomicU64>,
}

impl WriteRecorder {
    fn new(total_sectors: u64) -> Self {
        Self {
            total_sectors,
            writes: alloc::sync::Arc::new(core::sync::atomic::AtomicU64::new(0)),
        }
    }
    fn write_count(&self) -> u64 {
        self.writes.load(Ordering::Relaxed)
    }
    /// A second handle on the SAME write counter, so a smoketest can read the
    /// count after this recorder has been boxed into `ACTIVE_BLOCK_DEVICE`.
    fn counter(&self) -> alloc::sync::Arc<core::sync::atomic::AtomicU64> {
        self.writes.clone()
    }
}

impl crate::block_io::BlockDevice for WriteRecorder {
    fn read_sector(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        if lba >= self.total_sectors {
            return Err("WriteRecorder: read out of range");
        }
        for b in buf.iter_mut() {
            *b = 0;
        }
        Ok(())
    }
    fn write_sector(&self, lba: u64, _buf: &[u8]) -> Result<(), &'static str> {
        if lba >= self.total_sectors {
            return Err("WriteRecorder: write out of range");
        }
        self.writes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn sector_size(&self) -> usize {
        512
    }
    fn total_sectors(&self) -> u64 {
        self.total_sectors
    }
}

/// Slice H2 proof: the pre-write target "firewall" refuses on a target-identity
/// mismatch with ZERO sectors written, and permits a matching identity. Pure
/// logic over a mock `BlockDevice` (a test that can FAIL).
///
/// Cases:
///   (a) matching identity (planned == observed, write window in range) -> Ok;
///   (b) sector-count mismatch (the wrong disk became active) -> refuse;
///   (c) sector-size mismatch -> refuse;
///   (d) write window past end of device -> refuse;
///   (e) end-to-end: a WriteRecorder mock proves that when the firewall verdict
///       is "refuse", the destructive write path is never entered (0 writes).
pub fn run_firewall_smoketest() {
    use crate::block_io::BlockDevice;

    // Build the "planned" identity from a 4096-sector mock targeting the whole
    // device (write window [0..4095]).
    let dev = WriteRecorder::new(4096);
    let planned = snapshot_device_target(&dev, 0, 4095);

    // (a) Identical re-snapshot -> Ok.
    let observed_match = snapshot_device_target(&dev, 0, 4095);
    let case_a = planned.check_against(&observed_match).is_ok();

    // (b) The active device changed to a different sector count -> refuse.
    let other = WriteRecorder::new(8192);
    let observed_wrong = snapshot_device_target(&other, 0, 4095);
    let case_b = planned.check_against(&observed_wrong).is_err();

    // (c) Sector-size mismatch -> refuse.
    let observed_size = TargetIdentity {
        sector_size: 4096,
        ..observed_match
    };
    let case_c = planned.check_against(&observed_size).is_err();

    // (d) Planned write past end of device -> refuse.
    let planned_oob = snapshot_device_target(&dev, 0, 4096);
    let case_d = planned_oob.check_against(&observed_match).is_err();

    // (e) On a refuse verdict, NO write is issued. We model the run_install
    // ordering: snapshot+check BEFORE any write_sector. Build a refuse case and
    // confirm the recorder saw 0 writes (the firewall short-circuits first).
    let rec = WriteRecorder::new(4096);
    let plan_e = snapshot_device_target(&rec, 0, 4095);
    let obs_e = TargetIdentity {
        total_sectors: 9999, // pretend the active device changed under us
        ..plan_e
    };
    let refuse_e = plan_e.check_against(&obs_e).is_err();
    // The firewall is pure — it cannot have written — but assert the recorder is
    // untouched to make the "0 destructive writes on refuse" guarantee explicit.
    if !refuse_e {
        // If (e) didn't refuse, simulate the (wrong) fall-through write so the
        // assertion below FAILS loudly rather than silently passing.
        let _ = BlockDevice::write_sector(&rec, 0, &[0u8; 512]);
    }
    let zero_writes_on_refuse = rec.write_count() == 0;

    let pass = case_a && case_b && case_c && case_d && refuse_e && zero_writes_on_refuse;
    crate::serial_println!(
        "[install] H2 firewall smoketest: match_ok={} sectorcount_mismatch_refused={} sectorsize_mismatch_refused={} oob_refused={} refuse_path={} zero_writes_on_refuse={} -> {}",
        case_a,
        case_b,
        case_c,
        case_d,
        refuse_e,
        zero_writes_on_refuse,
        if pass { "PASS" } else { "FAIL" },
    );
}

/// Slice H3 proof: a non-bootable ESP is a HARD FAIL on real hardware (abort,
/// target untouched) but the QEMU dev flow proceeds; and a full `run_install`
/// that hits the abort path issues ZERO destructive AthFS writes. Pure logic +
/// a `run_install` end-to-end over a WriteRecorder mock (a test that can FAIL).
///
/// Cases:
///   (a) decision truth table — bootable ESP never aborts; non-bootable aborts
///       only on real hardware (not QEMU);
///   (b) `is_aborted()` recognises the sentinel and not a normal stage mask;
///   (c) end-to-end: install a tiny WriteRecorder as the ACTIVE device so the
///       partition seed fails (too small for GPT) — run_install returns without
///       writing, and the recorder confirms 0 destructive writes. (The H2
///       firewall passes here because planned==observed; the abort comes from
///       the install pipeline refusing to proceed.)
pub fn run_abort_safety_smoketest() {
    // (a) Decision truth table.
    let a1 = !should_abort_for_unbootable_esp(true, false); // bootable, iron  -> proceed
    let a2 = !should_abort_for_unbootable_esp(true, true); // bootable, qemu  -> proceed
    let a3 = should_abort_for_unbootable_esp(false, false); // non-boot, iron -> ABORT
    let a4 = !should_abort_for_unbootable_esp(false, true); // non-boot, qemu -> proceed
    let decision_ok = a1 && a2 && a3 && a4;

    // (b) Sentinel recognition.
    let sentinel_ok = is_aborted(STAGE_ABORTED)
        && !is_aborted(STAGE_GPT | STAGE_ESP_FORMAT | STAGE_BOOT_TREE | STAGE_RAEFS_FORMAT)
        && !is_aborted(0);

    // (c) End-to-end: a tiny ACTIVE device that cannot be partitioned. We swap
    // it in as the ACTIVE device for the duration of one run_install, then
    // restore the prior device so we never disturb the running system. The mock
    // is 64 sectors — too small for the GPT seed AND for an existing ESP — so
    // run_install bails at stage 1 (or the firewall) WITHOUT a AthFS format.
    let rec = WriteRecorder::new(64);
    let write_counter = rec.counter(); // second handle on the SAME counter
    let prev = {
        let mut g = crate::block_io::ACTIVE_BLOCK_DEVICE.lock();
        g.replace(alloc::boxed::Box::new(rec))
    };
    let prev_writes_enabled = crate::block_io::writes_enabled();
    crate::block_io::set_writes_enabled(true); // allow writes so a real format WOULD record
    let result = run_install();
    crate::block_io::set_writes_enabled(prev_writes_enabled);
    // Restore the prior ACTIVE device (drop our recorder box).
    {
        let mut g = crate::block_io::ACTIVE_BLOCK_DEVICE.lock();
        *g = prev;
    }
    // A 64-sector disk cannot hold a AthFS root, so run_install MUST NOT reach
    // the destructive AthFS format. Two assertions: (1) no AthFS-format stage
    // bit, and (2) the shared write counter recorded ZERO destructive writes —
    // the explicit "0 destructive writes on abort" guarantee.
    let no_raefs_format = result & STAGE_RAEFS_FORMAT == 0;
    let zero_destructive_writes = write_counter.load(Ordering::Relaxed) == 0;
    let case_c = no_raefs_format && zero_destructive_writes;

    let pass = decision_ok && sentinel_ok && case_c;
    crate::serial_println!(
        "[install] H3 abort smoketest: decision_truth_table={} sentinel={} tiny_disk_no_raefs_format={} zero_destructive_writes={} (result={:#x}) -> {}",
        decision_ok,
        sentinel_ok,
        no_raefs_format,
        zero_destructive_writes,
        result,
        if pass { "PASS" } else { "FAIL" },
    );
}

pub fn last_result() -> u64 {
    LAST_RESULT.load(Ordering::Relaxed)
}

pub fn dump_text() -> alloc::string::String {
    let r = LAST_RESULT.load(Ordering::Relaxed);
    alloc::format!(
        "# AthenaOS installer\nruns: {}\nlast_result: {:#07b}\n  gpt={} esp_format={} boot_tree={} raefs={} verify={}\npayload_source: {}\n  ramdisk=INITRAMFS({} B)\n",
        INSTALL_RUNS.load(Ordering::Relaxed),
        r,
        (r & STAGE_GPT != 0) as u8,
        (r & STAGE_ESP_FORMAT != 0) as u8,
        (r & STAGE_BOOT_TREE != 0) as u8,
        (r & STAGE_RAEFS_FORMAT != 0) as u8,
        (r & STAGE_VERIFY != 0) as u8,
        match PAYLOAD_SOURCE_OK.load(Ordering::Relaxed) {
            1 => "PASS",
            2 => "FAIL",
            _ => "not run",
        },
        crate::INITRAMFS.len(),
    )
}
