//! FAT boot-sector validation for installer ESP (R09).
//!
//! Patterns from Redox `redox-fatfs` recipe — on-disk layout only; no full FS yet.

#![no_std]

/// Parsed fields from a FAT BIOS Parameter Block (FAT12/16 BPB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FatBpb {
    pub bytes_per_sector: u16,
    pub sectors_per_cluster: u8,
    pub reserved_sectors: u16,
    pub num_fats: u8,
    pub root_entry_count: u16,
    pub total_sectors_16: u16,
    pub media: u8,
    pub sectors_per_fat: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatError {
    TooSmall,
    BadJump,
    BadSignature,
    InvalidBpb,
}

/// Validate a 512-byte boot sector looks like FAT (not exFAT/NTFS).
pub fn parse_boot_sector(sector: &[u8]) -> Result<FatBpb, FatError> {
    if sector.len() < 512 {
        return Err(FatError::TooSmall);
    }
    if sector[0] != 0xEB && sector[0] != 0xE9 {
        return Err(FatError::BadJump);
    }
    if sector[510] != 0x55 || sector[511] != 0xAA {
        return Err(FatError::BadSignature);
    }
    let bytes_per_sector = u16::from_le_bytes([sector[11], sector[12]]);
    if bytes_per_sector == 0 || (bytes_per_sector & (bytes_per_sector - 1)) != 0 {
        return Err(FatError::InvalidBpb);
    }
    Ok(FatBpb {
        bytes_per_sector,
        sectors_per_cluster: sector[13],
        reserved_sectors: u16::from_le_bytes([sector[14], sector[15]]),
        num_fats: sector[16],
        root_entry_count: u16::from_le_bytes([sector[17], sector[18]]),
        total_sectors_16: u16::from_le_bytes([sector[19], sector[20]]),
        media: sector[21],
        sectors_per_fat: u16::from_le_bytes([sector[22], sector[23]]),
    })
}

/// Heuristic: FAT32 BPB has extended fields; FAT12 ESP often has small total sector count.
pub fn is_probably_fat12(bpb: &FatBpb) -> bool {
    bpb.sectors_per_cluster > 0
        && bpb.num_fats > 0
        && bpb.root_entry_count > 0
        && bpb.total_sectors_16 > 0
        && bpb.total_sectors_16 < 32680
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_short_buffer() {
        assert!(matches!(
            parse_boot_sector(&[0u8; 64]),
            Err(FatError::TooSmall)
        ));
    }

    #[test]
    fn accepts_minimal_fat_signature() {
        let mut sector = [0u8; 512];
        sector[0] = 0xEB;
        sector[510] = 0x55;
        sector[511] = 0xAA;
        sector[11..13].copy_from_slice(&512u16.to_le_bytes());
        sector[13] = 1;
        sector[16] = 2;
        sector[17..19].copy_from_slice(&512u16.to_le_bytes());
        sector[19..21].copy_from_slice(&2880u16.to_le_bytes());
        sector[22..24].copy_from_slice(&9u16.to_le_bytes());
        let bpb = parse_boot_sector(&sector).expect("bpb");
        assert!(is_probably_fat12(&bpb));
    }
}
