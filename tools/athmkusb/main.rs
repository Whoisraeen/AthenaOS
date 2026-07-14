// athmkusb — build a AthenaOS install-media image.
//
// Default (inject) mode: copy the real bootable UEFI image (kernel.uefi.img,
// produced by `xtask build`) and inject the `INSTALL.NOW` marker into its ESP
// root via the `fatfs` crate (auto-detects FAT16/32, LFN). The result is
// firmware-bootable (the proven boot tree EFI/BOOT/BOOTX64.EFI + kernel-x86_64
// is already inside) AND fires the marker-gated installer — which then sources
// the REAL BOOTX64.EFI + kernel-x86_64 from this very stick onto the target
// disk, so the installed system is itself firmware-bootable. This is the stick
// to flash to real hardware.
//
// --scratch mode: a minimal hand-rolled GPT + FAT32 ESP carrying only
// INSTALL.NOW, byte-compatible with the kernel's parse_gpt/parse_vbr. Fires the
// marker-gated installer in QEMU without rebuilding the bootloader (QEMU VVFAT
// can't — it only presents raw FAT32 the strict parser rejects). The installer
// then writes placeholder payloads (no real boot tree on this stick).
//
//   cargo run --manifest-path tools/athmkusb/Cargo.toml -- --out target/install-usb.img
//   cargo run --manifest-path tools/athmkusb/Cargo.toml -- --scratch --out target/install.img

use std::io::{Cursor, Read, Write};
use std::process::exit;

const SECTOR: usize = 512;

// EFI System Partition type GUID (mixed-endian on-disk form).
const EFI_GUID: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];

// The marker the kernel's maybe_run_triggered_install() looks for at the ESP
// root: read_esp_file(&[], "INSTALL", "NOW"). Presence is what matters; the
// content is informational.
const INSTALL_MARKER: &[u8] = b"athenaos-install-trigger-v1\n";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut out: Option<String> = None;
    let mut boot_image = String::from("target/x86_64-unknown-none/release/kernel.uefi.img");
    let mut scratch = false;
    let mut size_mib: u64 = 96;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--out" if i + 1 < args.len() => {
                out = Some(args[i + 1].clone());
                i += 2;
            }
            "--boot-image" if i + 1 < args.len() => {
                boot_image = args[i + 1].clone();
                i += 2;
            }
            "--scratch" => {
                scratch = true;
                i += 1;
            }
            "--inspect" if i + 1 < args.len() => {
                inspect_image(&args[i + 1]);
                return;
            }
            "--size-mib" if i + 1 < args.len() => {
                size_mib = args[i + 1].parse().unwrap_or_else(|_| {
                    eprintln!("athmkusb: bad --size-mib");
                    exit(2)
                });
                i += 2;
            }
            other => {
                eprintln!("usage: athmkusb [--scratch] [--boot-image <kernel.uefi.img>] --out <path.img> [--size-mib <N>]");
                eprintln!("  (unknown arg: {other})");
                exit(2);
            }
        }
    }

    if scratch {
        let out = out.unwrap_or_else(|| "target/install.img".to_string());
        build_scratch(&out, size_mib);
    } else {
        let out = out.unwrap_or_else(|| "target/install-usb.img".to_string());
        build_inject(&boot_image, &out);
    }
}

// ─── inspect mode: dump a disk image's ESP directory tree ────────────────────

fn inspect_image(image: &str) {
    let mut img = std::fs::read(image).unwrap_or_else(|e| {
        eprintln!("athmkusb: read {image}: {e}");
        exit(2);
    });
    let (s0, s1) = find_esp_range(&img).unwrap_or_else(|| {
        eprintln!("athmkusb: no ESP in {image}");
        exit(2);
    });
    println!("ESP bytes {s0}..{s1} ({} MiB)", (s1 - s0) / (1024 * 1024));
    let slice = fscommon::StreamSlice::new(Cursor::new(&mut img[..]), s0, s1).expect("slice");
    let fs = fatfs::FileSystem::new(slice, fatfs::FsOptions::new()).unwrap_or_else(|e| {
        eprintln!("athmkusb: open ESP FAT failed: {e}");
        exit(2);
    });
    println!("FAT type: {:?}", fs.fat_type());
    fn walk<IO: fatfs::ReadWriteSeek>(dir: &fatfs::Dir<IO>, prefix: &str, depth: usize) {
        if depth > 6 {
            return;
        }
        for entry in dir.iter() {
            let e = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let name = e.file_name();
            if name == "." || name == ".." {
                continue;
            }
            if e.is_dir() {
                println!("{prefix}{name}/");
                walk(&e.to_dir(), &format!("{prefix}{name}/"), depth + 1);
            } else {
                println!("{prefix}{name}  ({} B)", e.len());
            }
        }
    }
    walk(&fs.root_dir(), "/", 0);
}

// ─── inject mode: real bootable image + INSTALL.NOW ──────────────────────────

fn build_inject(boot_image: &str, out: &str) {
    // 1. Read the proven-bootable source image (xtask's kernel.uefi.img) and
    //    extract its boot tree. The `bootloader` crate builds that ESP as
    //    FAT16, which `fatfs` reads fine — but the AthenaOS kernel's installer
    //    reader is FAT32-only, so we can't just inject a marker and ship the
    //    FAT16 stick (the kernel couldn't read INSTALL.NOW or source payloads
    //    from it). Instead we re-house the IDENTICAL boot-tree bytes into a
    //    fresh FAT32 ESP that BOTH the UEFI firmware AND the kernel can read.
    let mut src = std::fs::read(boot_image).unwrap_or_else(|e| {
        eprintln!("athmkusb: read boot image {boot_image}: {e}");
        eprintln!("  (build it first: cargo run -p xtask --release -- build --release)");
        exit(2);
    });
    if src.len() < SECTOR * 3 {
        eprintln!("athmkusb: boot image too small to be a GPT disk");
        exit(2);
    }
    let (s0, s1) = find_esp_range(&src).unwrap_or_else(|| {
        eprintln!("athmkusb: no EFI System Partition found in {boot_image}'s GPT");
        exit(2);
    });

    let (bootx64, kernel, bootlog) = {
        let slice = fscommon::StreamSlice::new(Cursor::new(&mut src[..]), s0, s1)
            .expect("source ESP slice");
        let fs = fatfs::FileSystem::new(slice, fatfs::FsOptions::new()).unwrap_or_else(|e| {
            eprintln!("athmkusb: open source ESP FAT failed: {e}");
            exit(2);
        });
        let root = fs.root_dir();
        let bootx64 = read_fatfs_file(&root, &["EFI", "BOOT"], "BOOTX64.EFI").unwrap_or_else(|| {
            eprintln!("athmkusb: EFI/BOOT/BOOTX64.EFI not found in source image");
            exit(2);
        });
        let kernel = read_fatfs_file(&root, &[], "kernel-x86_64").unwrap_or_else(|| {
            eprintln!("athmkusb: kernel-x86_64 not found in source image");
            exit(2);
        });
        let bootlog = read_fatfs_file(&root, &[], "BOOTLOG.TXT");
        (bootx64, kernel, bootlog)
    };
    eprintln!(
        "athmkusb: extracted boot tree — BOOTX64.EFI {} B, kernel-x86_64 {} B{}",
        bootx64.len(),
        kernel.len(),
        match &bootlog {
            Some(b) => format!(", BOOTLOG.TXT {} B", b.len()),
            None => String::new(),
        }
    );

    // 2. Build a fresh GPT + FAT32 ESP large enough for the boot tree + slack,
    //    then copy the extracted bytes in (plus INSTALL.NOW at the root).
    // Size the ESP just big enough for the payload + slack while staying a valid
    // FAT32 (>= ~33 MiB / 65525 clusters at 512 B/cluster). Kept tight on purpose:
    // the installer raw-copies this whole ESP onto the target, so a smaller ESP
    // means a faster install. 12 MiB slack, 40 MiB floor.
    let payload_bytes = bootx64.len() + kernel.len() + bootlog.as_ref().map_or(0, |b| b.len());
    let total_mib = (((payload_bytes as u64) / (1024 * 1024)) + 12).max(40);
    let total_sectors = total_mib * 1024 * 1024 / SECTOR as u64;
    let mut img = vec![0u8; total_sectors as usize * SECTOR];
    let esp_start = 2048u64;
    let esp_end = total_sectors - 34;
    write_gpt(&mut img, total_sectors, esp_start, esp_end);

    let esp_b0 = esp_start * SECTOR as u64;
    let esp_b1 = (esp_end + 1) * SECTOR as u64;
    {
        let slice = fscommon::StreamSlice::new(Cursor::new(&mut img[..]), esp_b0, esp_b1)
            .expect("dest ESP slice");
        fatfs::format_volume(
            slice,
            fatfs::FormatVolumeOptions::new()
                .fat_type(fatfs::FatType::Fat32)
                .volume_label(*b"ATHENA USB "),
        )
        .unwrap_or_else(|e| {
            eprintln!("athmkusb: format dest FAT32 failed: {e}");
            exit(2);
        });
    }
    {
        let slice = fscommon::StreamSlice::new(Cursor::new(&mut img[..]), esp_b0, esp_b1)
            .expect("dest ESP slice 2");
        let fs = fatfs::FileSystem::new(slice, fatfs::FsOptions::new()).unwrap_or_else(|e| {
            eprintln!("athmkusb: open dest ESP FAT failed: {e}");
            exit(2);
        });
        let root = fs.root_dir();
        let efi = root.create_dir("EFI").expect("mkdir EFI");
        let boot = efi.create_dir("BOOT").expect("mkdir EFI/BOOT");
        write_fatfs_file(&boot, "BOOTX64.EFI", &bootx64);
        write_fatfs_file(&root, "kernel-x86_64", &kernel);
        write_fatfs_file(&root, "INSTALL.NOW", INSTALL_MARKER);
        if let Some(b) = &bootlog {
            write_fatfs_file(&root, "BOOTLOG.TXT", b);
        }
    }

    // 3. Verify the marker + kernel read back via a fresh FAT view (the same
    //    open-root → find path the kernel takes).
    let ok = {
        let slice = fscommon::StreamSlice::new(Cursor::new(&mut img[..]), esp_b0, esp_b1)
            .expect("verify slice");
        let fs = fatfs::FileSystem::new(slice, fatfs::FsOptions::new()).expect("verify FAT");
        let root = fs.root_dir();
        let marker_ok = read_fatfs_file(&root, &[], "INSTALL.NOW").as_deref() == Some(INSTALL_MARKER);
        let kernel_ok = read_fatfs_file(&root, &[], "kernel-x86_64").map_or(false, |k| k == kernel);
        let bootx64_ok =
            read_fatfs_file(&root, &["EFI", "BOOT"], "BOOTX64.EFI").map_or(false, |b| b == bootx64);
        marker_ok && kernel_ok && bootx64_ok
    };
    if !ok {
        eprintln!("athmkusb: FAT32 readback verification FAILED");
        exit(2);
    }

    std::fs::write(out, &img).unwrap_or_else(|e| {
        eprintln!("athmkusb: write {out}: {e}");
        exit(2);
    });

    println!(
        "athmkusb: wrote {} ({} MiB) — FAT32 ESP with EFI/BOOT/BOOTX64.EFI + kernel-x86_64 + INSTALL.NOW (readback OK)\n\
         UEFI-bootable AND kernel-readable; flash raw to a USB device, then boot the target to install.",
        out, total_mib,
    );
}

/// Read a file at `path`/`name` from a fatfs volume, or `None` if absent.
fn read_fatfs_file<IO: fatfs::ReadWriteSeek>(
    root: &fatfs::Dir<IO>,
    path: &[&str],
    name: &str,
) -> Option<Vec<u8>> {
    let mut dir = root.clone();
    for seg in path {
        dir = dir.open_dir(seg).ok()?;
    }
    let mut f = dir.open_file(name).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Create+write a file (overwriting) in a fatfs directory.
fn write_fatfs_file<IO: fatfs::ReadWriteSeek>(dir: &fatfs::Dir<IO>, name: &str, data: &[u8]) {
    let mut f = dir
        .create_file(name)
        .unwrap_or_else(|e| panic!("athmkusb: create {name}: {e}"));
    f.truncate().ok();
    f.write_all(data)
        .unwrap_or_else(|e| panic!("athmkusb: write {name}: {e}"));
    f.flush().ok();
}

/// Parse the GPT in `img` and return the ESP partition's (start_byte, end_byte).
fn find_esp_range(img: &[u8]) -> Option<(u64, u64)> {
    // GPT header at LBA 1; signature "EFI PART".
    let hdr = &img[SECTOR..SECTOR * 2];
    if &hdr[0..8] != b"EFI PART" {
        return None;
    }
    let entry_lba = u64::from_le_bytes(hdr[72..80].try_into().ok()?);
    let num_entries = u32::from_le_bytes(hdr[80..84].try_into().ok()?) as usize;
    let entry_size = u32::from_le_bytes(hdr[84..88].try_into().ok()?) as usize;
    let base = entry_lba as usize * SECTOR;
    for i in 0..num_entries.min(128) {
        let ofs = base + i * entry_size;
        if ofs + entry_size > img.len() {
            break;
        }
        let e = &img[ofs..ofs + entry_size];
        if e[0..16] == EFI_GUID {
            let first = u64::from_le_bytes(e[32..40].try_into().ok()?);
            let last = u64::from_le_bytes(e[40..48].try_into().ok()?);
            if last >= first {
                return Some((first * SECTOR as u64, (last + 1) * SECTOR as u64));
            }
        }
    }
    None
}

// ─── scratch mode: minimal hand-rolled GPT + FAT32 ESP + INSTALL.NOW ─────────
//
// Byte-compatible with the kernel's parse_gpt (no CRC check, ESP type GUID
// only) + parse_vbr. Fires the marker-gated installer in QEMU; carries NO real
// boot tree (the installer writes placeholder payloads).

fn crc32_ieee(data: &[u8]) -> u32 {
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

fn read_sector(img: &[u8], lba: u64) -> [u8; SECTOR] {
    let off = lba as usize * SECTOR;
    let mut s = [0u8; SECTOR];
    s.copy_from_slice(&img[off..off + SECTOR]);
    s
}
fn write_sector(img: &mut [u8], lba: u64, data: &[u8]) {
    let off = lba as usize * SECTOR;
    img[off..off + SECTOR].copy_from_slice(&data[..SECTOR]);
}

fn write_gpt(img: &mut [u8], total_sectors: u64, esp_start: u64, esp_end: u64) {
    let disk_last = total_sectors - 1;
    let first_usable = 34u64;
    let last_usable = disk_last - 33;

    let mut mbr = [0u8; SECTOR];
    mbr[446 + 4] = 0xEE;
    mbr[446 + 8..446 + 12].copy_from_slice(&1u32.to_le_bytes());
    mbr[446 + 12..446 + 16].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    write_sector(img, 0, &mbr);

    let mut entries = vec![0u8; 128 * 128];
    entries[0..16].copy_from_slice(&EFI_GUID);
    entries[16..32].copy_from_slice(&[
        0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc, 0xfe, 0x10, 0x32, 0x54, 0x76, 0x98, 0xba, 0xdc,
        0xfe,
    ]);
    entries[32..40].copy_from_slice(&esp_start.to_le_bytes());
    entries[40..48].copy_from_slice(&esp_end.to_le_bytes());
    for (i, ch) in [b'E' as u16, b'S' as u16, b'P' as u16, 0].iter().enumerate() {
        let o = 56 + i * 2;
        entries[o..o + 2].copy_from_slice(&ch.to_le_bytes());
    }
    let entries_crc = crc32_ieee(&entries);

    let make_header = |my_lba: u64, alt_lba: u64, entries_lba: u64| -> [u8; SECTOR] {
        let mut h = [0u8; SECTOR];
        h[0..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&92u32.to_le_bytes());
        h[24..32].copy_from_slice(&my_lba.to_le_bytes());
        h[32..40].copy_from_slice(&alt_lba.to_le_bytes());
        h[40..48].copy_from_slice(&first_usable.to_le_bytes());
        h[48..56].copy_from_slice(&last_usable.to_le_bytes());
        h[56..72].copy_from_slice(b"ATHENAOS-USBGUID");
        h[72..80].copy_from_slice(&entries_lba.to_le_bytes());
        h[80..84].copy_from_slice(&128u32.to_le_bytes());
        h[84..88].copy_from_slice(&128u32.to_le_bytes());
        h[88..92].copy_from_slice(&entries_crc.to_le_bytes());
        h[16..20].copy_from_slice(&0u32.to_le_bytes());
        let hc = crc32_ieee(&h[..92]);
        h[16..20].copy_from_slice(&hc.to_le_bytes());
        h
    };

    let primary_entries_lba = 2u64;
    let backup_entries_lba = disk_last - 32;
    write_sector(img, 1, &make_header(1, disk_last, primary_entries_lba));
    for s in 0..32u64 {
        let chunk = &entries[(s as usize) * SECTOR..(s as usize) * SECTOR + SECTOR];
        write_sector(img, primary_entries_lba + s, chunk);
        write_sector(img, backup_entries_lba + s, chunk);
    }
    write_sector(img, disk_last, &make_header(disk_last, 1, backup_entries_lba));
}

struct Fat {
    esp_start_lba: u64,
    sectors_per_cluster: u8,
    reserved_sectors: u16,
    num_fats: u8,
    fat_size_sectors: u32,
    data_start_lba: u64,
    next_cluster: u32,
}

impl Fat {
    fn cluster_first_lba(&self, c: u32) -> u64 {
        self.data_start_lba + (c as u64 - 2) * self.sectors_per_cluster as u64
    }
    fn fat_entry_loc(&self, c: u32) -> (u64, usize) {
        let fo = c as u64 * 4;
        (
            self.esp_start_lba + self.reserved_sectors as u64 + fo / 512,
            (fo % 512) as usize,
        )
    }
    fn set_fat_entry(&self, img: &mut [u8], c: u32, value: u32) {
        for fat in 0..self.num_fats as u64 {
            let (base, off) = self.fat_entry_loc(c);
            let lba = base + fat * self.fat_size_sectors as u64;
            let mut sec = read_sector(img, lba);
            sec[off..off + 4].copy_from_slice(&(value & 0x0FFF_FFFF).to_le_bytes());
            write_sector(img, lba, &sec);
        }
    }
    fn alloc_cluster(&mut self, img: &mut [u8]) -> u32 {
        let c = self.next_cluster;
        self.next_cluster += 1;
        let zero = [0u8; SECTOR];
        for s in 0..self.sectors_per_cluster as u64 {
            write_sector(img, self.cluster_first_lba(c) + s, &zero);
        }
        self.set_fat_entry(img, c, 0x0FFF_FFFF);
        c
    }
}

fn fat32_format(img: &mut [u8], esp_start_lba: u64, esp_sectors: u32) -> Fat {
    let reserved_sectors: u16 = 32;
    let num_fats: u8 = 2;
    let sectors_per_cluster: u8 = if esp_sectors < 0x20000 { 1 } else { 8 };
    let tmp1 = esp_sectors - reserved_sectors as u32;
    let tmp2 = (256 * sectors_per_cluster as u32) + num_fats as u32;
    let fat_size_sectors = (tmp1 + (tmp2 - 1)) / tmp2;
    let data_start_lba =
        esp_start_lba + reserved_sectors as u64 + num_fats as u64 * fat_size_sectors as u64;

    let mut vbr = [0u8; SECTOR];
    vbr[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    vbr[3..11].copy_from_slice(b"MSWIN4.1");
    vbr[11..13].copy_from_slice(&512u16.to_le_bytes());
    vbr[13] = sectors_per_cluster;
    vbr[14..16].copy_from_slice(&reserved_sectors.to_le_bytes());
    vbr[16] = num_fats;
    vbr[21] = 0xF8;
    vbr[24..26].copy_from_slice(&63u16.to_le_bytes());
    vbr[26..28].copy_from_slice(&255u16.to_le_bytes());
    vbr[28..32].copy_from_slice(&(esp_start_lba as u32).to_le_bytes());
    vbr[32..36].copy_from_slice(&esp_sectors.to_le_bytes());
    vbr[36..40].copy_from_slice(&fat_size_sectors.to_le_bytes());
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
    write_sector(img, esp_start_lba, &vbr);
    write_sector(img, esp_start_lba + 6, &vbr);

    let mut fsinfo = [0u8; SECTOR];
    fsinfo[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    fsinfo[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    fsinfo[488..492].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    fsinfo[492..496].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    fsinfo[508..512].copy_from_slice(&0xAA55_0000u32.to_le_bytes());
    write_sector(img, esp_start_lba + 1, &fsinfo);

    let w = Fat {
        esp_start_lba,
        sectors_per_cluster,
        reserved_sectors,
        num_fats,
        fat_size_sectors,
        data_start_lba,
        next_cluster: 3,
    };
    w.set_fat_entry(img, 0, 0x0FFF_FFF8);
    w.set_fat_entry(img, 1, 0x0FFF_FFFF);
    w.set_fat_entry(img, 2, 0x0FFF_FFFF);
    w
}

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

fn add_dirent(img: &mut [u8], w: &Fat, dir_cluster: u32, entry: &[u8; 32]) -> bool {
    let cluster_bytes = w.sectors_per_cluster as usize * SECTOR;
    let slots = cluster_bytes / 32;
    for slot in 0..slots {
        let byte_off = slot * 32;
        let lba = w.cluster_first_lba(dir_cluster) + (byte_off / SECTOR) as u64;
        let in_sec = byte_off % SECTOR;
        let mut sec = read_sector(img, lba);
        if sec[in_sec] == 0x00 || sec[in_sec] == 0xE5 {
            sec[in_sec..in_sec + 32].copy_from_slice(entry);
            write_sector(img, lba, &sec);
            return true;
        }
    }
    false
}

fn write_file_83(img: &mut [u8], w: &mut Fat, dir_cluster: u32, base: &str, ext: &str, data: &[u8]) {
    let cluster_bytes = w.sectors_per_cluster as usize * SECTOR;
    let clusters_needed = ((data.len() + cluster_bytes - 1) / cluster_bytes.max(1)).max(1);
    let mut chain = Vec::new();
    for _ in 0..clusters_needed {
        chain.push(w.alloc_cluster(img));
    }
    for i in 0..chain.len().saturating_sub(1) {
        w.set_fat_entry(img, chain[i], chain[i + 1]);
    }
    let mut written = 0usize;
    'outer: for &c in &chain {
        for s in 0..w.sectors_per_cluster as u64 {
            let mut sec = [0u8; SECTOR];
            let remaining = data.len() - written;
            if remaining > 0 {
                let n = remaining.min(SECTOR);
                sec[..n].copy_from_slice(&data[written..written + n]);
                written += n;
            }
            write_sector(img, w.cluster_first_lba(c) + s, &sec);
            if written >= data.len() {
                break 'outer;
            }
        }
    }
    let dent = fat_dirent(&name83(base, ext), 0x20, chain[0], data.len() as u32);
    add_dirent(img, w, dir_cluster, &dent);
}

fn build_scratch(out: &str, size_mib: u64) {
    let total_sectors = size_mib * 1024 * 1024 / SECTOR as u64;
    if total_sectors < 4096 {
        eprintln!("athmkusb: --size-mib too small");
        exit(2);
    }
    let mut img = vec![0u8; total_sectors as usize * SECTOR];

    let esp_start = 2048u64;
    let esp_end = total_sectors - 34;
    let esp_sectors = (esp_end - esp_start + 1) as u32;

    write_gpt(&mut img, total_sectors, esp_start, esp_end);
    let mut fat = fat32_format(&mut img, esp_start, esp_sectors);
    write_file_83(&mut img, &mut fat, 2, "INSTALL", "NOW", INSTALL_MARKER);

    std::fs::write(out, &img).unwrap_or_else(|e| {
        eprintln!("athmkusb: write {out}: {e}");
        exit(2);
    });

    println!(
        "athmkusb: wrote {} ({} MiB, --scratch): GPT + FAT32 ESP (LBA {}..{}, {} sectors) + INSTALL.NOW",
        out, size_mib, esp_start, esp_end, esp_sectors,
    );
}
