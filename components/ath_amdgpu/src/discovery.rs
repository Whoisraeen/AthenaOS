//! AMD GPU IP discovery table parser.
//!
//! Every register's absolute MMIO/SMN offset on a SOC15+ ASIC (gfx11/RDNA3
//! included) is `per-IP-block base + (register index << 2)`. Those per-block
//! bases are NOT fixed constants — they are published by the GPU itself in an
//! "IP discovery" binary blob (at the top of VRAM), which `amdgpu` reads at
//! probe time to build `reg_offset[ip][inst][seg]`. This is exactly why
//! `bringup.rs` hardcodes NO absolute offsets and gates `smu_mailbox`/`ih_ring`/
//! `rlc_safe_mode` on the daemon: the authoritative bases must come from the
//! hardware, not a guess.
//!
//! This module parses that blob and returns each block's `(hw_id, instance,
//! bases[])`, so a caller can resolve `ip_base(GC_HWID, 0, seg) + reg_index*4`
//! — authoritative because the bases originate on the device. The register
//! INDICES (e.g. SMU C2PMSG 66/82/90, `RLC_SAFE_MODE`, `IH_RB_*`,
//! `CONFIG_MEMSIZE`) are stable and supplied by the caller.
//!
//! Binary format is taken verbatim from the upstream `discovery.h`
//! (`binary_header` → `table_list` → `ip_discovery_header` → `die_header` →
//! `ip_v4`). Parsed with bounds-checked little-endian byte reads — no `unsafe`,
//! honoring the crate's `#![forbid(unsafe_code)]`. The blob is untrusted device
//! memory, so every access is bounds-checked and a malformed table yields an
//! empty result rather than a panic.

use alloc::vec::Vec;

/// `binary_header.binary_signature` — start of a valid discovery blob.
pub const BINARY_SIGNATURE: u32 = 0x2821_1407;
/// `ip_discovery_header.signature` — marks the IP-discovery table within the blob.
pub const DISCOVERY_TABLE_SIGNATURE: u32 = 0x5344_5049;

/// One IP block as published by the discovery table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpBlock {
    /// Hardware-block id (e.g. GC/MP1/OSSSYS/NBIF — the `*_HWID` values).
    pub hw_id: u16,
    /// Instance number (0 for the single-instance blocks we resolve).
    pub instance: u8,
    /// Per-segment register bases for this block (`base_address[]`), truncated
    /// to 32 bits — register MMIO/SMN offsets within the BAR are 32-bit.
    pub bases: Vec<u32>,
}

#[inline]
fn rd_u16(b: &[u8], off: usize) -> Option<u16> {
    b.get(off..off + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
}
#[inline]
fn rd_u32(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Locate the IP-discovery table inside the blob by scanning `table_list` for the
/// entry whose data begins with [`DISCOVERY_TABLE_SIGNATURE`] (robust without
/// depending on the `table` enum's index of IP_DISCOVERY).
fn find_ip_discovery(blob: &[u8]) -> Option<usize> {
    // binary_header: signature u32 @0, version_major/minor u16 @4/6,
    // binary_checksum u16 @8, binary_size u16 @10, then table_list[] @12
    // (each table_info is 8 bytes: offset u16, checksum u16, size u16, pad u16).
    let mut t = 12usize;
    let mut guard = 0;
    while t + 2 <= blob.len() && guard < 64 {
        if let Some(off) = rd_u16(blob, t) {
            let off = off as usize;
            if off != 0 && rd_u32(blob, off) == Some(DISCOVERY_TABLE_SIGNATURE) {
                return Some(off);
            }
        }
        t += 8;
        guard += 1;
    }
    None
}

/// Parse the discovery blob into its IP blocks. Returns an empty vec on any
/// malformed/short input (the blob is untrusted device memory).
pub fn parse(blob: &[u8]) -> Vec<IpBlock> {
    let mut out = Vec::new();
    if rd_u32(blob, 0) != Some(BINARY_SIGNATURE) {
        return out;
    }
    let Some(ipd) = find_ip_discovery(blob) else {
        return out;
    };
    // ip_discovery_header: signature u32 @0, version u16 @4, size u16 @6,
    // id u32 @8, num_dies u16 @12, die_info[16] @14 (die_id u16, die_offset u16),
    // then a base_addr flag byte at @78 (bit0 = 64-bit bases).
    let Some(num_dies) = rd_u16(blob, ipd + 12) else {
        return out;
    };
    let base64 = blob.get(ipd + 78).map(|b| b & 1 != 0).unwrap_or(false);
    let base_stride = if base64 { 8 } else { 4 };

    for d in 0..(num_dies as usize).min(16) {
        let Some(die_off) = rd_u16(blob, ipd + 14 + d * 4 + 2) else {
            continue;
        };
        let die_off = die_off as usize;
        // die_header: die_id u16 @0, num_ips u16 @2; ip_v4 entries follow.
        let Some(num_ips) = rd_u16(blob, die_off + 2) else {
            continue;
        };
        let mut p = die_off + 4;
        for _ in 0..num_ips {
            // ip_v4: hw_id u16 @0, instance u8 @2, num_base_address u8 @3,
            // major/minor/revision u8 @4/5/6, sub_revision:4|variant:4 @7,
            // then base_address[num_base_address].
            let (Some(hw_id), Some(instance), Some(num_base)) = (
                rd_u16(blob, p),
                blob.get(p + 2).copied(),
                blob.get(p + 3).copied(),
            ) else {
                break;
            };
            let mut bases = Vec::new();
            for i in 0..num_base as usize {
                if let Some(v) = rd_u32(blob, p + 8 + i * base_stride) {
                    bases.push(v);
                }
            }
            out.push(IpBlock {
                hw_id,
                instance,
                bases,
            });
            p += 8 + (num_base as usize) * base_stride;
            if p > blob.len() {
                break;
            }
        }
    }
    out
}

/// Resolve `block.bases[seg]` for the first block matching `(hw_id, instance)`.
/// `None` if the block (or that segment) isn't present — so a caller never
/// fabricates an offset for a block the hardware didn't publish.
pub fn ip_base(blocks: &[IpBlock], hw_id: u16, instance: u8, seg: usize) -> Option<u32> {
    blocks
        .iter()
        .find(|b| b.hw_id == hw_id && b.instance == instance)
        .and_then(|b| b.bases.get(seg).copied())
}

/// Validate + parse a discovery blob from an UNTRUSTED source (a vendored
/// firmware file OR the VRAM read): require the binary signature, parse, and
/// return the blocks only if at least one parsed. `None` on a bad signature or
/// an unparseable/empty blob — so a caller never adopts garbage as register
/// bases. This is the firmware-file counterpart to amdgpu's
/// `amdgpu_discovery_verify_binary_signature` + parse, and the safe gate the
/// `amdgpu/ip_discovery.bin` path runs through (see
/// `bringup::discovery_from_firmware`).
pub fn parse_checked(blob: &[u8]) -> Option<Vec<IpBlock>> {
    if rd_u32(blob, 0) != Some(BINARY_SIGNATURE) {
        return None;
    }
    let blocks = parse(blob);
    if blocks.is_empty() {
        None
    } else {
        Some(blocks)
    }
}

/// Build a minimal but format-faithful discovery blob: binary_header with a
/// table_list pointing at one ip_discovery table that has one die with two IP
/// blocks (hw_id 0x46 with two bases, hw_id 0x01 with one). Shared by this
/// module's tests and `bringup`'s firmware-discovery test.
#[cfg(test)]
pub(crate) fn synthetic_blob() -> Vec<u8> {
    let mut b = alloc::vec![0u8; 256];
    // binary_header
    b[0..4].copy_from_slice(&BINARY_SIGNATURE.to_le_bytes());
    b[10..12].copy_from_slice(&256u16.to_le_bytes()); // binary_size
                                                      // table_list[0] at @12: offset -> 64 (where the ip_discovery table lives)
    let ipd = 64usize;
    b[12..14].copy_from_slice(&(ipd as u16).to_le_bytes());
    // ip_discovery_header @ipd
    b[ipd..ipd + 4].copy_from_slice(&DISCOVERY_TABLE_SIGNATURE.to_le_bytes());
    b[ipd + 12..ipd + 14].copy_from_slice(&1u16.to_le_bytes()); // num_dies = 1
    let die = 160usize;
    // die_info[0].die_offset at ipd+14+2
    b[ipd + 14 + 2..ipd + 14 + 4].copy_from_slice(&(die as u16).to_le_bytes());
    // base_addr flag @ipd+78 = 0 (32-bit bases)
    // die_header @die: die_id, num_ips = 2
    b[die + 2..die + 4].copy_from_slice(&2u16.to_le_bytes());
    // ip_v4[0] @die+4: hw_id 0x46, instance 0, num_base 2, bases [0x1234, 0x5678]
    let ip0 = die + 4;
    b[ip0..ip0 + 2].copy_from_slice(&0x0046u16.to_le_bytes());
    b[ip0 + 2] = 0; // instance
    b[ip0 + 3] = 2; // num_base_address
    b[ip0 + 8..ip0 + 12].copy_from_slice(&0x0000_1234u32.to_le_bytes());
    b[ip0 + 12..ip0 + 16].copy_from_slice(&0x0000_5678u32.to_le_bytes());
    // ip_v4[1] @ip0 + 8 + 2*4 = ip0+16: hw_id 0x01, instance 0, num_base 1, base [0xABCD]
    let ip1 = ip0 + 16;
    b[ip1..ip1 + 2].copy_from_slice(&0x0001u16.to_le_bytes());
    b[ip1 + 2] = 0;
    b[ip1 + 3] = 1;
    b[ip1 + 8..ip1 + 12].copy_from_slice(&0x0000_ABCDu32.to_le_bytes());
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn parse_checked_validates_signature() {
        // A well-formed blob parses to its blocks.
        let good = synthetic_blob();
        assert_eq!(parse_checked(&good).unwrap().len(), 2);
        // A corrupted signature is rejected — never adopt garbage as reg bases.
        let mut bad = synthetic_blob();
        bad[0] ^= 0xFF;
        assert!(parse_checked(&bad).is_none());
        // Too-short / empty inputs are rejected, not panicked on.
        assert!(parse_checked(&[]).is_none());
        assert!(parse_checked(&[0u8; 3]).is_none());
    }

    #[test]
    fn parses_blocks_and_bases() {
        let blob = synthetic_blob();
        let blocks = parse(&blob);
        assert_eq!(blocks.len(), 2, "two IP blocks");
        assert_eq!(blocks[0].hw_id, 0x46);
        assert_eq!(blocks[0].bases, vec![0x1234, 0x5678]);
        assert_eq!(blocks[1].hw_id, 0x01);
        assert_eq!(blocks[1].bases, vec![0xABCD]);
    }

    #[test]
    fn ip_base_resolves_segments() {
        let blocks = parse(&synthetic_blob());
        assert_eq!(ip_base(&blocks, 0x46, 0, 0), Some(0x1234));
        assert_eq!(ip_base(&blocks, 0x46, 0, 1), Some(0x5678));
        assert_eq!(ip_base(&blocks, 0x01, 0, 0), Some(0xABCD));
        // absent block / segment -> None (never fabricate an offset)
        assert_eq!(ip_base(&blocks, 0x99, 0, 0), None);
        assert_eq!(ip_base(&blocks, 0x46, 0, 5), None);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse(&[0u8; 4]).is_empty());
        assert!(parse(&[0xFF; 256]).is_empty());
        assert!(parse(&[]).is_empty());
    }
}
