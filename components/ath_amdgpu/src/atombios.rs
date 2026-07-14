//! ATOMBIOS ROM parsing — the first real step of `amdgpu_get_bios` /
//! `amdgpu_atombios_init` on iron. The VBIOS is an AMD "ATOMBIOS" image whose
//! header points at the master command + data tables (firmware-info,
//! integrated-system-info, display-object, …). This module finds and validates
//! the header and surfaces the master-table offsets; per-table decoders build
//! on top.
//!
//! Layout (little-endian throughout):
//! ```text
//!   ROM[0x00..0x02]  = 0xAA55             (PCI expansion-ROM signature)
//!   ROM[0x48..0x4A]  = u16 ptr to ATOM_ROM_HEADER
//!   ATOM_ROM_HEADER:
//!     +0   ATOM_COMMON_TABLE_HEADER (size u16, fmt_rev u8, content_rev u8)
//!     +4   "ATOM"  firmware signature
//!     +30  u16 master COMMAND table offset
//!     +32  u16 master DATA table offset
//! ```

/// Offset in the ROM image of the u16 pointer to the ATOM_ROM_HEADER.
pub const OFFSET_TO_ATOM_ROM_HEADER_PTR: usize = 0x48;
/// PCI expansion-ROM signature (LE) at ROM offset 0.
pub const ROM_SIGNATURE: u16 = 0xAA55;
/// "ATOM" firmware signature at ATOM_ROM_HEADER + 4.
pub const ATOM_SIGNATURE: &[u8; 4] = b"ATOM";

const ROM_HEADER_SIG_OFF: usize = 4;
const ROM_HEADER_MASTER_CMD_OFF: usize = 30;
const ROM_HEADER_MASTER_DATA_OFF: usize = 32;

/// The common 4-byte header every ATOM table starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommonTableHeader {
    pub structure_size: u16,
    pub format_revision: u8,
    pub content_revision: u8,
}

/// Parsed result of [`parse_rom`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtomRom {
    pub header_ptr: u16,
    pub rom_header: CommonTableHeader,
    pub master_command_table_offset: u16,
    pub master_data_table_offset: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomError {
    TooShort,
    BadRomSignature,
    HeaderPtrOutOfRange,
    BadAtomSignature,
    TableOffsetOutOfRange,
}

#[inline]
fn rd_u16(rom: &[u8], off: usize) -> Option<u16> {
    let b = rom.get(off..off + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

/// Validate and parse an ATOMBIOS ROM image, returning the header + master
/// table offsets. Bounds-checks every read so a truncated or junk ROM yields a
/// typed error rather than reading out of range (the kernel hands us a raw BAR
/// mirror that may be short or absent).
pub fn parse_rom(rom: &[u8]) -> Result<AtomRom, AtomError> {
    if rom.len() < OFFSET_TO_ATOM_ROM_HEADER_PTR + 2 {
        return Err(AtomError::TooShort);
    }
    if rd_u16(rom, 0).ok_or(AtomError::TooShort)? != ROM_SIGNATURE {
        return Err(AtomError::BadRomSignature);
    }
    let header_ptr = rd_u16(rom, OFFSET_TO_ATOM_ROM_HEADER_PTR).ok_or(AtomError::TooShort)?;
    let hp = header_ptr as usize;
    // The header needs at least 34 bytes (through the master-data offset).
    if hp + ROM_HEADER_MASTER_DATA_OFF + 2 > rom.len() {
        return Err(AtomError::HeaderPtrOutOfRange);
    }
    if &rom[hp + ROM_HEADER_SIG_OFF..hp + ROM_HEADER_SIG_OFF + 4] != ATOM_SIGNATURE {
        return Err(AtomError::BadAtomSignature);
    }
    let rom_header = CommonTableHeader {
        structure_size: rd_u16(rom, hp).ok_or(AtomError::TooShort)?,
        format_revision: rom[hp + 2],
        content_revision: rom[hp + 3],
    };
    let master_command_table_offset =
        rd_u16(rom, hp + ROM_HEADER_MASTER_CMD_OFF).ok_or(AtomError::TooShort)?;
    let master_data_table_offset =
        rd_u16(rom, hp + ROM_HEADER_MASTER_DATA_OFF).ok_or(AtomError::TooShort)?;
    // A zero offset for either table would be nonsensical (it would alias the
    // ROM signature at offset 0, not a real table); both must land in-image.
    if master_data_table_offset == 0
        || master_command_table_offset == 0
        || master_data_table_offset as usize >= rom.len()
        || master_command_table_offset as usize >= rom.len()
    {
        return Err(AtomError::TableOffsetOutOfRange);
    }
    Ok(AtomRom {
        header_ptr,
        rom_header,
        master_command_table_offset,
        master_data_table_offset,
    })
}

// ── ACPI VFCT (VBIOS Firmware Content Table) ─────────────────────────────────
//
// APUs have no PCI expansion ROM: firmware publishes the VBIOS image(s) inside
// this ACPI table instead, and Linux's `amdgpu_acpi_vfct_bios` walks it exactly
// like this. Athena's real table is captured at
// `firmware/acpi/athena-beelink-elitemini/VFCT.dat` (one image, 1002:15bf,
// 16896 bytes) and the host KAT below parses those exact bytes.
//
// Layout (little-endian):
// ```text
//   VFCT[0..4]    = "VFCT"             (ACPI signature)
//   VFCT[52..56]  = u32 offset of the first image entry (from table start)
//   image entry:  28-byte VFCT_IMAGE_HEADER, then `image_length` VBIOS bytes
//     +12 u16 vendor_id   +14 u16 device_id   +24 u32 image_length
//   next entry directly follows the previous image's bytes.
// ```

/// "VFCT" ACPI table signature.
pub const VFCT_SIGNATURE: &[u8; 4] = b"VFCT";
/// Offset of the u32 first-image offset in the VFCT header.
const VFCT_IMAGE_LIST_OFFSET_OFF: usize = 52;
/// Size of one VFCT_IMAGE_HEADER.
const VFCT_IMAGE_HEADER_LEN: usize = 28;
const IMG_VENDOR_OFF: usize = 12;
const IMG_DEVICE_OFF: usize = 14;
const IMG_LENGTH_OFF: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfctError {
    TooShort,
    BadSignature,
    /// An image entry's length runs past the end of the table buffer.
    ImageOutOfRange,
    /// Table walked clean but held no image for the requested vendor/device.
    NoMatchingImage,
}

#[inline]
fn rd_u32(buf: &[u8], off: usize) -> Option<u32> {
    let b = buf.get(off..off + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Walk an ACPI VFCT table and return the VBIOS image bytes for `vendor:device`.
/// Bounds-checked throughout; a zero-length image entry terminates the walk
/// (real tables, including Athena's, pad the tail with zeroes).
pub fn parse_vfct(table: &[u8], vendor: u16, device: u16) -> Result<&[u8], VfctError> {
    if table.len() < VFCT_IMAGE_LIST_OFFSET_OFF + 4 {
        return Err(VfctError::TooShort);
    }
    if &table[0..4] != VFCT_SIGNATURE {
        return Err(VfctError::BadSignature);
    }
    let mut off = rd_u32(table, VFCT_IMAGE_LIST_OFFSET_OFF).ok_or(VfctError::TooShort)? as usize;
    while off + VFCT_IMAGE_HEADER_LEN <= table.len() {
        let img_vendor = rd_u16(table, off + IMG_VENDOR_OFF).ok_or(VfctError::TooShort)?;
        let img_device = rd_u16(table, off + IMG_DEVICE_OFF).ok_or(VfctError::TooShort)?;
        let img_len = rd_u32(table, off + IMG_LENGTH_OFF).ok_or(VfctError::TooShort)? as usize;
        if img_len == 0 {
            break; // tail padding / terminator
        }
        let img_start = off + VFCT_IMAGE_HEADER_LEN;
        let img_end = img_start
            .checked_add(img_len)
            .ok_or(VfctError::ImageOutOfRange)?;
        if img_end > table.len() {
            return Err(VfctError::ImageOutOfRange);
        }
        if img_vendor == vendor && img_device == device {
            return Ok(&table[img_start..img_end]);
        }
        off = img_end;
    }
    Err(VfctError::NoMatchingImage)
}

/// Read the `CommonTableHeader` of a table at `offset` within the ROM (e.g. the
/// master data table itself). Bounds-checked.
pub fn table_header(rom: &[u8], offset: u16) -> Option<CommonTableHeader> {
    let o = offset as usize;
    let _ = rom.get(o..o + 4)?;
    Some(CommonTableHeader {
        structure_size: u16::from_le_bytes([rom[o], rom[o + 1]]),
        format_revision: rom[o + 2],
        content_revision: rom[o + 3],
    })
}

// ── ATOM master data-table list ──────────────────────────────────────────────
//
// `master_data_table_offset` (from [`parse_rom`]) points at the master DATA
// table: a 4-byte common header, then an array of u16 pointers — one per data
// table, indexed by position. Linux reaches each table via
// `GetIndexIntoMasterDataTable(name)`; the index→table mapping is fixed by the
// ATOM spec. A zero pointer means "this VBIOS has no such table". The number of
// entries is bounded by the master table's own `structure_size`, NOT the ROM
// length — bytes past the header span belong to other tables (Athena's image
// has only 35 entries; reading further yields garbage).

/// Data-table list index of the firmware-info table. This has been ATOM
/// data-table index 4 ("FirmwareInfo") across every ATOMBIOS generation; on
/// GC 11.0.1 (Phoenix / Radeon 780M) it points at an `atom_firmware_info_v3_4`
/// (`format_revision` 3, `content_revision` 4) — the table amdgpu reads for
/// bootup engine/memory clocks and firmware-capability flags.
pub const DATA_TABLE_FIRMWARE_INFO: usize = 4;

/// Data-table list index of the integrated-system-info table — APU memory
/// configuration (DRAM type, channel count, bootup display/memory clocks).
/// Unlike firmware-info, this index is NOT constant across ATOM generations;
/// `30` is the slot in `atom_master_list_of_data_tables_v2_1` (the modern
/// atomfirmware layout GC 11.0.1 uses). Athena's real VBIOS corroborates it:
/// index 30 is the only `format_revision 2` entry of integrated-system-info
/// size — a 1 KiB `atom_integrated_system_info_v2_2` at 0x3148 (see the KAT).
/// Note the VRAM carve-out *size* is NOT here — that is the CONFIG_MEMSIZE
/// register; this table supplies memory type/channels/clocks.
pub const DATA_TABLE_INTEGRATED_SYSTEM_INFO: usize = 30;

/// A located ATOM data table: its absolute offset in the ROM image plus the
/// common header found there. Field decoders build on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataTable {
    pub offset: u16,
    pub header: CommonTableHeader,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataTableError {
    /// The master data table's header is truncated, zero-sized, or runs past
    /// the image.
    MasterTableOutOfRange,
    /// The requested list index is beyond the master table's entry count.
    IndexOutOfRange,
    /// The list slot is a zero pointer — that table is absent in this VBIOS.
    NotPresent,
    /// The pointer lands outside the ROM image (no room for a table header).
    EntryOutOfRange,
    /// The located table is shorter than the fields a decoder needs to read.
    TableTooShort,
}

/// Resolve data-table `index` in the ATOM master data-table list, returning the
/// pointed-at table's offset + common header. Bounds-checked against the master
/// table's `structure_size` (entry count) and against the image for the target.
pub fn data_table(
    rom_bytes: &[u8],
    rom: &AtomRom,
    index: usize,
) -> Result<DataTable, DataTableError> {
    let base = rom.master_data_table_offset as usize;
    let size = rd_u16(rom_bytes, base).ok_or(DataTableError::MasterTableOutOfRange)? as usize;
    // Need the 4-byte header plus the whole declared span inside the image.
    if size < 4
        || base
            .checked_add(size)
            .is_none_or(|end| end > rom_bytes.len())
    {
        return Err(DataTableError::MasterTableOutOfRange);
    }
    let entries = (size - 4) / 2;
    if index >= entries {
        return Err(DataTableError::IndexOutOfRange);
    }
    let ptr =
        rd_u16(rom_bytes, base + 4 + index * 2).ok_or(DataTableError::MasterTableOutOfRange)?;
    if ptr == 0 {
        return Err(DataTableError::NotPresent);
    }
    let header = table_header(rom_bytes, ptr).ok_or(DataTableError::EntryOutOfRange)?;
    Ok(DataTable {
        offset: ptr,
        header,
    })
}

/// Locate the firmware-info table (data-table index 4) — the first table
/// amdgpu reads after `amdgpu_get_bios`.
pub fn firmware_info(rom_bytes: &[u8], rom: &AtomRom) -> Result<DataTable, DataTableError> {
    data_table(rom_bytes, rom, DATA_TABLE_FIRMWARE_INFO)
}

/// Locate the integrated-system-info table (data-table index 30) — present on
/// APUs, absent on dGPUs/QEMU. Its `format_revision` is 2 (the `v2_x` family).
pub fn integrated_system_info(
    rom_bytes: &[u8],
    rom: &AtomRom,
) -> Result<DataTable, DataTableError> {
    data_table(rom_bytes, rom, DATA_TABLE_INTEGRATED_SYSTEM_INFO)
}

/// The raw bytes of a located data table — its own `structure_size` span,
/// bounds-checked against the image. Per-table field decoders slice into this.
pub fn data_table_bytes<'a>(rom_bytes: &'a [u8], table: &DataTable) -> Option<&'a [u8]> {
    let start = table.offset as usize;
    let end = start.checked_add(table.header.structure_size as usize)?;
    rom_bytes.get(start..end)
}

// ── Firmware-info table (atom_firmware_info_v3_x) ─────────────────────────────
//
// amdgpu reads the firmware-info table for the bootup engine/memory clocks and
// the firmware-capability bitfield. The leading fields below are identical
// across v3_1..v3_4 (the driver reads them through a union), so this decodes
// only that shared prefix. GC 11.0.1 (Phoenix) ships v3_4 (format 3/content 4).
//
//   +0   ATOM_COMMON_TABLE_HEADER (4 bytes)
//   +4   u32 firmware_revision
//   +8   u32 bootup_sclk_in10khz   (engine/GFX clock, 10 kHz units)
//   +12  u32 bootup_mclk_in10khz   (memory/UMC clock, 10 kHz units)
//   +16  u32 firmware_capability   (bitfield)

const FW_INFO_REVISION_OFF: usize = 4;
const FW_INFO_SCLK_OFF: usize = 8;
const FW_INFO_MCLK_OFF: usize = 12;
const FW_INFO_CAP_OFF: usize = 16;
/// Shortest table that still carries every field we decode (through capability).
const FW_INFO_MIN_LEN: usize = FW_INFO_CAP_OFF + 4;

/// The bring-up–relevant prefix of `atom_firmware_info_v3_x`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirmwareInfo {
    pub firmware_revision: u32,
    /// Bootup engine (GFX) clock, in 10 kHz units.
    pub bootup_sclk_10khz: u32,
    /// Bootup memory (UMC) clock, in 10 kHz units.
    pub bootup_mclk_10khz: u32,
    pub firmware_capability: u32,
}

impl FirmwareInfo {
    /// Bootup engine clock in MHz (10 kHz units / 100).
    pub fn bootup_sclk_mhz(&self) -> u32 {
        self.bootup_sclk_10khz / 100
    }
    /// Bootup memory clock in MHz (10 kHz units / 100).
    pub fn bootup_mclk_mhz(&self) -> u32 {
        self.bootup_mclk_10khz / 100
    }
}

/// Decode the firmware-info table (data-table index 4). Reads only the shared
/// `v3_1..v3_4` prefix, so it is revision-agnostic; a table shorter than that
/// prefix is rejected as `TableTooShort`.
pub fn parse_firmware_info(
    rom_bytes: &[u8],
    rom: &AtomRom,
) -> Result<FirmwareInfo, DataTableError> {
    let table = firmware_info(rom_bytes, rom)?;
    let body = data_table_bytes(rom_bytes, &table).ok_or(DataTableError::EntryOutOfRange)?;
    if body.len() < FW_INFO_MIN_LEN {
        return Err(DataTableError::TableTooShort);
    }
    Ok(FirmwareInfo {
        firmware_revision: rd_u32(body, FW_INFO_REVISION_OFF)
            .ok_or(DataTableError::TableTooShort)?,
        bootup_sclk_10khz: rd_u32(body, FW_INFO_SCLK_OFF).ok_or(DataTableError::TableTooShort)?,
        bootup_mclk_10khz: rd_u32(body, FW_INFO_MCLK_OFF).ok_or(DataTableError::TableTooShort)?,
        firmware_capability: rd_u32(body, FW_INFO_CAP_OFF).ok_or(DataTableError::TableTooShort)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate alloc;
    use alloc::vec;

    /// Build a minimal but structurally valid ATOMBIOS ROM with the header at
    /// `hp` and the given master command/data offsets.
    fn synth_rom(hp: u16, cmd_off: u16, data_off: u16, len: usize) -> alloc::vec::Vec<u8> {
        let mut rom = vec![0u8; len];
        rom[0..2].copy_from_slice(&ROM_SIGNATURE.to_le_bytes());
        rom[OFFSET_TO_ATOM_ROM_HEADER_PTR..OFFSET_TO_ATOM_ROM_HEADER_PTR + 2]
            .copy_from_slice(&hp.to_le_bytes());
        let h = hp as usize;
        rom[h..h + 2].copy_from_slice(&36u16.to_le_bytes()); // structure_size
        rom[h + 2] = 1; // format_revision
        rom[h + 3] = 2; // content_revision
        rom[h + 4..h + 8].copy_from_slice(ATOM_SIGNATURE);
        rom[h + 30..h + 32].copy_from_slice(&cmd_off.to_le_bytes());
        rom[h + 32..h + 34].copy_from_slice(&data_off.to_le_bytes());
        rom
    }

    #[test]
    fn parses_valid_rom() {
        let rom = synth_rom(0x80, 0x100, 0x140, 0x400);
        let r = parse_rom(&rom).unwrap();
        assert_eq!(r.header_ptr, 0x80);
        assert_eq!(r.master_command_table_offset, 0x100);
        assert_eq!(r.master_data_table_offset, 0x140);
        assert_eq!(r.rom_header.format_revision, 1);
        assert_eq!(r.rom_header.content_revision, 2);
    }

    #[test]
    fn rejects_bad_signatures() {
        let mut rom = synth_rom(0x80, 0x100, 0x140, 0x400);
        rom[0] = 0; // clobber 0xAA55
        assert_eq!(parse_rom(&rom), Err(AtomError::BadRomSignature));

        let mut rom = synth_rom(0x80, 0x100, 0x140, 0x400);
        rom[0x80 + 4] = b'X'; // clobber "ATOM"
        assert_eq!(parse_rom(&rom), Err(AtomError::BadAtomSignature));
    }

    #[test]
    fn rejects_out_of_range() {
        // header pointer past the end: a valid ROM signature + a header_ptr that
        // leaves no room for the 34-byte header. (Built by hand, not synth_rom,
        // which would itself write out of bounds.)
        let mut rom = vec![0u8; 0x400];
        rom[0..2].copy_from_slice(&ROM_SIGNATURE.to_le_bytes());
        rom[OFFSET_TO_ATOM_ROM_HEADER_PTR..OFFSET_TO_ATOM_ROM_HEADER_PTR + 2]
            .copy_from_slice(&0x3F0u16.to_le_bytes());
        assert_eq!(parse_rom(&rom), Err(AtomError::HeaderPtrOutOfRange));
        // data table offset out of image.
        let rom = synth_rom(0x80, 0x100, 0x500, 0x400);
        assert_eq!(parse_rom(&rom), Err(AtomError::TableOffsetOutOfRange));
    }

    #[test]
    fn rejects_truncated() {
        assert_eq!(parse_rom(&[0x55, 0xAA]), Err(AtomError::TooShort));
    }

    #[test]
    fn reads_table_header() {
        let rom = synth_rom(0x80, 0x100, 0x140, 0x400);
        let th = table_header(&rom, 0x80).unwrap();
        assert_eq!(th.structure_size, 36);
    }

    /// Build a synthetic VFCT table holding the given (vendor, device, image)
    /// entries back-to-back, with the standard 76-byte header.
    fn synth_vfct(entries: &[(u16, u16, &[u8])]) -> alloc::vec::Vec<u8> {
        let mut t = vec![0u8; 76];
        t[0..4].copy_from_slice(VFCT_SIGNATURE);
        t[52..56].copy_from_slice(&76u32.to_le_bytes());
        for &(ven, dev, img) in entries {
            let mut hdr = vec![0u8; 28];
            hdr[12..14].copy_from_slice(&ven.to_le_bytes());
            hdr[14..16].copy_from_slice(&dev.to_le_bytes());
            hdr[24..28].copy_from_slice(&(img.len() as u32).to_le_bytes());
            t.extend_from_slice(&hdr);
            t.extend_from_slice(img);
        }
        t
    }

    #[test]
    fn vfct_finds_matching_image_among_several() {
        let wrong = synth_rom(0x80, 0x100, 0x140, 0x400);
        let right = synth_rom(0x90, 0x110, 0x150, 0x600);
        let t = synth_vfct(&[(0x1002, 0x0000, &wrong), (0x1002, 0x15BF, &right)]);
        let img = parse_vfct(&t, 0x1002, 0x15BF).unwrap();
        assert_eq!(img.len(), 0x600);
        // The extracted image must itself parse as ATOMBIOS.
        let r = parse_rom(img).unwrap();
        assert_eq!(r.header_ptr, 0x90);
    }

    #[test]
    fn vfct_rejects_bad_inputs() {
        assert_eq!(
            parse_vfct(b"VFCT", 0x1002, 0x15BF),
            Err(VfctError::TooShort)
        );
        let mut t = synth_vfct(&[(0x1002, 0x15BF, &[0u8; 64])]);
        t[0] = b'X';
        assert_eq!(parse_vfct(&t, 0x1002, 0x15BF), Err(VfctError::BadSignature));
        // Image length running past the table end.
        let mut t = synth_vfct(&[(0x1002, 0x15BF, &[0u8; 64])]);
        let n = t.len();
        t[n - 64 - 4..n - 64].copy_from_slice(&9999u32.to_le_bytes());
        assert_eq!(
            parse_vfct(&t, 0x1002, 0x15BF),
            Err(VfctError::ImageOutOfRange)
        );
        // No image for the requested device.
        let t = synth_vfct(&[(0x1002, 0x0000, &[0u8; 64])]);
        assert_eq!(
            parse_vfct(&t, 0x1002, 0x15BF),
            Err(VfctError::NoMatchingImage)
        );
    }

    #[test]
    fn vfct_zero_length_entry_terminates() {
        // A zero-length entry (tail padding) must end the walk, not loop forever.
        let mut t = synth_vfct(&[(0x1002, 0x0000, &[0u8; 64])]);
        t.extend_from_slice(&[0u8; 28]); // all-zero header: len == 0
        assert_eq!(
            parse_vfct(&t, 0x1002, 0x15BF),
            Err(VfctError::NoMatchingImage)
        );
    }

    /// Real-data KAT (the aml_probe pattern): parse Athena's actual VFCT dump
    /// and verify it yields the exact VBIOS image that is vendored at
    /// `firmware/vbios/1002-15bf.bin` — 16896 bytes of valid ATOMBIOS for the
    /// Radeon 760M. Collected from iron 2026-06-12 (docs/ATHENA_GROUND_TRUTH.md).
    #[test]
    fn vfct_parses_real_athena_table() {
        let table: &[u8] =
            include_bytes!("../../../firmware/acpi/athena-beelink-elitemini/VFCT.dat");
        let img = parse_vfct(table, 0x1002, 0x15BF).unwrap();
        assert_eq!(img.len(), 16896);
        let rom = parse_rom(img).expect("real VBIOS must parse as ATOMBIOS");
        assert_eq!(rom.header_ptr, 0x194);
        // And it must byte-match the vendored fallback blob amdgpud loads at
        // runtime via request_firmware("vbios/1002-15bf.bin").
        let vendored: &[u8] = include_bytes!("../../../firmware/vbios/1002-15bf.bin");
        assert_eq!(img, vendored);
    }

    /// Build on `synth_rom` a master data-table list with the given u16 entry
    /// pointers (0 = absent). Header at 0x80, master data table at 0x140.
    fn synth_rom_with_dtl(entries: &[u16]) -> alloc::vec::Vec<u8> {
        let mut rom = synth_rom(0x80, 0x100, 0x140, 0x400);
        let base = 0x140usize;
        let size = (4 + entries.len() * 2) as u16;
        rom[base..base + 2].copy_from_slice(&size.to_le_bytes());
        rom[base + 2] = 2; // format_revision
        rom[base + 3] = 1; // content_revision
        for (i, &e) in entries.iter().enumerate() {
            let o = base + 4 + i * 2;
            rom[o..o + 2].copy_from_slice(&e.to_le_bytes());
        }
        rom
    }

    #[test]
    fn data_table_bounds_presence_and_range() {
        // 5 entries: only index 4 present, pointing at 0x200 (in-image header).
        let bytes = synth_rom_with_dtl(&[0, 0, 0, 0, 0x200]);
        let rom = parse_rom(&bytes).unwrap();
        let fw = data_table(&bytes, &rom, 4).unwrap();
        assert_eq!(fw.offset, 0x200);
        // A zero slot is "table absent", distinct from out-of-range.
        assert_eq!(data_table(&bytes, &rom, 0), Err(DataTableError::NotPresent));
        // One past the 5-entry list.
        assert_eq!(
            data_table(&bytes, &rom, 5),
            Err(DataTableError::IndexOutOfRange)
        );
        // A pointer past the image end.
        let bytes = synth_rom_with_dtl(&[0x9000]);
        let rom = parse_rom(&bytes).unwrap();
        assert_eq!(
            data_table(&bytes, &rom, 0),
            Err(DataTableError::EntryOutOfRange)
        );
    }

    #[test]
    fn data_table_rejects_bad_master_table() {
        // Zero-sized master table (synth_rom leaves the data table zero-filled).
        let bytes = synth_rom(0x80, 0x100, 0x140, 0x400);
        let rom = parse_rom(&bytes).unwrap();
        assert_eq!(
            data_table(&bytes, &rom, 4),
            Err(DataTableError::MasterTableOutOfRange)
        );
        // A structure_size that runs past the image.
        let mut bytes = synth_rom_with_dtl(&[0, 0, 0, 0, 0x200]);
        bytes[0x140..0x142].copy_from_slice(&0xFFFFu16.to_le_bytes());
        let rom = parse_rom(&bytes).unwrap();
        assert_eq!(
            data_table(&bytes, &rom, 4),
            Err(DataTableError::MasterTableOutOfRange)
        );
    }

    /// Real-data KAT: walk Athena's actual VBIOS master data-table list and
    /// confirm firmware-info (index 4) resolves to the captured offset with the
    /// GC 11.0.1 header revision (`atom_firmware_info_v3_4`).
    #[test]
    fn data_table_locates_firmware_info_on_real_athena_vbios() {
        let table: &[u8] =
            include_bytes!("../../../firmware/acpi/athena-beelink-elitemini/VFCT.dat");
        let img = parse_vfct(table, 0x1002, 0x15BF).unwrap();
        let rom = parse_rom(img).unwrap();
        let fw = firmware_info(img, &rom).unwrap();
        assert_eq!(fw.offset, 0x3894);
        assert_eq!(fw.header.format_revision, 3); // v3
        assert_eq!(fw.header.content_revision, 4); // .4 -> atom_firmware_info_v3_4
                                                   // The table's own structure_size span must be readable in-image.
        let body = data_table_bytes(img, &fw).expect("firmware_info bytes in range");
        assert_eq!(body.len(), fw.header.structure_size as usize);
        assert_eq!(body.len(), 0x6c);
        // The master table declares 35 entries ((0x4a - 4) / 2); index 35 is out.
        assert_eq!(
            data_table(img, &rom, 35),
            Err(DataTableError::IndexOutOfRange)
        );
    }

    /// Real-data KAT: decode the firmware-info fields from Athena's actual VBIOS.
    /// Values captured from the table at 0x3894 (atom_firmware_info_v3_4): the
    /// 200 MHz bootup clocks are the APU's safe floor — the SMU ramps from there.
    #[test]
    fn parse_firmware_info_on_real_athena_vbios() {
        let table: &[u8] =
            include_bytes!("../../../firmware/acpi/athena-beelink-elitemini/VFCT.dat");
        let img = parse_vfct(table, 0x1002, 0x15BF).unwrap();
        let rom = parse_rom(img).unwrap();
        let fi = parse_firmware_info(img, &rom).unwrap();
        assert_eq!(fi.firmware_revision, 0x160c_001b);
        assert_eq!(fi.bootup_sclk_10khz, 20000);
        assert_eq!(fi.bootup_mclk_10khz, 20000);
        assert_eq!(fi.bootup_sclk_mhz(), 200);
        assert_eq!(fi.bootup_mclk_mhz(), 200);
        assert_eq!(fi.firmware_capability, 0x0000_0001);
    }

    /// Real-data KAT: locate integrated-system-info (index 30) in Athena's
    /// actual VBIOS — the 1 KiB `atom_integrated_system_info_v2_2` at 0x3148.
    /// This is the APU-only table; index 30's revision (v2.2) guards the
    /// from-memory index claim against a different table sitting there.
    #[test]
    fn data_table_locates_integrated_system_info_on_real_athena_vbios() {
        let table: &[u8] =
            include_bytes!("../../../firmware/acpi/athena-beelink-elitemini/VFCT.dat");
        let img = parse_vfct(table, 0x1002, 0x15BF).unwrap();
        let rom = parse_rom(img).unwrap();
        let isi = integrated_system_info(img, &rom).unwrap();
        assert_eq!(isi.offset, 0x3148);
        assert_eq!(isi.header.structure_size, 0x400); // 1 KiB
        assert_eq!(isi.header.format_revision, 2); // v2 family
        assert_eq!(isi.header.content_revision, 2); // v2_2
                                                    // The whole 1 KiB body is readable in-image.
        let body = data_table_bytes(img, &isi).expect("ISI body in range");
        assert_eq!(body.len(), 0x400);
    }

    #[test]
    fn parse_firmware_info_rejects_short_table() {
        // firmware-info (index 4) present, but its table header declares a
        // structure_size too small to hold the decoded fields.
        let mut bytes = synth_rom_with_dtl(&[0, 0, 0, 0, 0x200]);
        bytes[0x200..0x202].copy_from_slice(&8u16.to_le_bytes());
        let rom = parse_rom(&bytes).unwrap();
        assert_eq!(
            parse_firmware_info(&bytes, &rom),
            Err(DataTableError::TableTooShort)
        );
        // Absent firmware-info propagates NotPresent.
        let bytes = synth_rom_with_dtl(&[0, 0, 0, 0, 0]);
        let rom = parse_rom(&bytes).unwrap();
        assert_eq!(
            parse_firmware_info(&bytes, &rom),
            Err(DataTableError::NotPresent)
        );
    }
}
