//! gfx11 IMU microcode parsing (`imu_firmware_header_v1_0`).
//!
//! Concept (RaeGFX, "looks like Metal, performs like Vulkan"): on the Athena's
//! Phoenix APU the PSP leaves the GFX engine COLD — it loads the PMFW (SMU) but
//! NOT the GFX firmware (iron boot 233600: `RLC_BOOTLOAD_STATUS=0`,
//! `GFX_IMU_GFX_RESET_CTRL=0x10`). So RaeGFX's driver must DIRECT-load the GFX
//! firmware itself. Step 1 is loading the IMU ucode into the IMU's SRAM; the IMU
//! then brings GFX out of reset and autoloads RLC/CP. This module parses the
//! `gc_*_imu.bin` blob to find the I-RAM and D-RAM ucode regions — pure logic,
//! host-KAT'd, so the byte math is proven without hardware.

/// The I-RAM / D-RAM ucode regions inside a `gc_*_imu.bin` blob, parsed from
/// `imu_firmware_header_v1_0`. Byte offsets are from the start of the blob.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImuUcodeLayout {
    /// `common_firmware_header.ucode_version` — written to `GFX_IMU_*_RAM_ADDR`
    /// after streaming each region (the address the IMU expects post-load).
    pub fw_version: u32,
    /// Byte offset of the I-RAM ucode (= `ucode_array_offset_bytes`).
    pub iram_offset: usize,
    /// I-RAM ucode size in bytes (a multiple of 4).
    pub iram_size: usize,
    /// Byte offset of the D-RAM ucode (immediately follows the I-RAM region).
    pub dram_offset: usize,
    /// D-RAM ucode size in bytes (a multiple of 4).
    pub dram_size: usize,
}

impl ImuUcodeLayout {
    /// Number of 32-bit words to stream into `GFX_IMU_I_RAM_DATA`.
    pub fn iram_dwords(&self) -> usize {
        self.iram_size / 4
    }
    /// Number of 32-bit words to stream into `GFX_IMU_D_RAM_DATA`.
    pub fn dram_dwords(&self) -> usize {
        self.dram_size / 4
    }
}

fn rd_u32_le(b: &[u8], off: usize) -> Option<u32> {
    let s = b.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Parse `imu_firmware_header_v1_0` from a `gc_*_imu.bin` blob.
///
/// The 32-byte `common_firmware_header` holds `ucode_version` @16 and
/// `ucode_array_offset_bytes` @24; the v1_0 IMU header then adds
/// `imu_iram_ucode_size_bytes` @32 and `imu_dram_ucode_size_bytes` @40. Per
/// `imu_v11_0_load_microcode`, the I-RAM ucode is at `ucode_array_offset_bytes`
/// and the D-RAM ucode immediately follows it. Fully bounds-checked: returns
/// `None` on a truncated blob, a non-dword-aligned region, or an overflow, so a
/// corrupt/short firmware can never drive an out-of-bounds register stream.
pub fn parse_imu_ucode_layout(blob: &[u8]) -> Option<ImuUcodeLayout> {
    let fw_version = rd_u32_le(blob, 16)?;
    let array_off = rd_u32_le(blob, 24)? as usize;
    let iram_size = rd_u32_le(blob, 32)? as usize;
    let dram_size = rd_u32_le(blob, 40)? as usize;

    // Both regions must be whole dwords (the load streams 32 bits at a time).
    if iram_size % 4 != 0 || dram_size % 4 != 0 || iram_size == 0 {
        return None;
    }
    let iram_offset = array_off;
    let dram_offset = array_off.checked_add(iram_size)?;
    let end = dram_offset.checked_add(dram_size)?;
    if end > blob.len() {
        return None;
    }
    Some(ImuUcodeLayout {
        fw_version,
        iram_offset,
        iram_size,
        dram_offset,
        dram_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate alloc;
    use alloc::vec;
    use alloc::vec::Vec;

    /// Build a minimal valid `gc_*_imu.bin`: a 48-byte header (common 32 +
    /// v1_0 16) then `iram_size` + `dram_size` payload bytes.
    fn synth_imu_blob(fw_version: u32, iram_size: u32, dram_size: u32) -> Vec<u8> {
        let array_off: u32 = 48; // payload starts right after the v1_0 header
        let mut b = vec![0u8; (array_off + iram_size + dram_size) as usize];
        b[16..20].copy_from_slice(&fw_version.to_le_bytes()); // ucode_version
        b[24..28].copy_from_slice(&array_off.to_le_bytes()); // ucode_array_offset_bytes
        b[32..36].copy_from_slice(&iram_size.to_le_bytes()); // imu_iram_ucode_size_bytes
        b[40..44].copy_from_slice(&dram_size.to_le_bytes()); // imu_dram_ucode_size_bytes
                                                             // Tag the I-RAM region so a loader test can verify routing/order.
        for (i, byte) in b
            .iter_mut()
            .enumerate()
            .skip(array_off as usize)
            .take(iram_size as usize)
        {
            *byte = 0xA0u8.wrapping_add(i as u8);
        }
        b
    }

    #[test]
    fn parses_valid_header() {
        let b = synth_imu_blob(0x1234, 256, 128);
        let l = parse_imu_ucode_layout(&b).expect("valid blob parses");
        assert_eq!(l.fw_version, 0x1234);
        assert_eq!(l.iram_offset, 48);
        assert_eq!(l.iram_size, 256);
        assert_eq!(l.iram_dwords(), 64);
        assert_eq!(l.dram_offset, 48 + 256);
        assert_eq!(l.dram_size, 128);
        assert_eq!(l.dram_dwords(), 32);
    }

    #[test]
    fn rejects_truncated_blob() {
        let mut b = synth_imu_blob(1, 256, 128);
        b.truncate(48 + 256 + 64); // chop the D-RAM region short
        assert!(
            parse_imu_ucode_layout(&b).is_none(),
            "a blob that can't hold its declared ucode must be rejected"
        );
    }

    #[test]
    fn rejects_unaligned_and_empty() {
        // I-RAM size not a multiple of 4.
        assert!(parse_imu_ucode_layout(&synth_imu_blob(1, 255, 128)).is_none());
        // Zero I-RAM (no instruction ucode) is invalid.
        assert!(parse_imu_ucode_layout(&synth_imu_blob(1, 0, 128)).is_none());
        // Header too short to even read the fields.
        assert!(parse_imu_ucode_layout(&[0u8; 8]).is_none());
    }
}
