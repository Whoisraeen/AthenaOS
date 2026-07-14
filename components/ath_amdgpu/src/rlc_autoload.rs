//! gfx11 RLC backdoor-autoload buffer — TOC parsing.
//!
//! Concept (AthGFX): on the Athena's Phoenix APU the PSP leaves the GFX engine
//! cold (iron boot 002046: `RLC_BOOTLOAD_STATUS=0`), so AthGFX DIRECT-loads the
//! GFX firmware. The IMU bring-up needs a single contiguous VRAM "autoload
//! buffer" holding every GFX engine's ucode (RLC, PFP, ME, MEC, MES, SDMA, TOC)
//! laid out at fixed offsets; the IMU's RLC-bootloader reads the RLC_G ucode from
//! that buffer and pulls in the rest. The layout is described by an
//! `RLC_TABLE_OF_CONTENT` (TOC) shipped in the PSP TOC firmware
//! (`psp_13_0_4_toc.bin`). This module parses that TOC into a per-firmware-ID
//! {offset,size} table — pure logic, host-KAT'd, so the byte math is proven
//! without hardware.
//!
//! Off-target ground truth (parsed from the real Phoenix `psp_13_0_4_toc.bin`):
//! 31 entries, the buffer is ~6.4 MiB (`max(offset+size) = 6_680_576`), and the
//! TOC entries begin at a 256-byte-signature-prefixed offset after the firmware
//! header's `ucode_array_offset_bytes` (the linux-firmware blob wraps the TOC in
//! a `$PS1` signature block the PSP would otherwise strip).

extern crate alloc;
use alloc::vec::Vec;

/// One `RLC_TABLE_OF_CONTENT` entry: where firmware `id`'s ucode lives in the
/// autoload buffer. `offset`/`size` are already in BYTES (the on-disk fields are
/// in dwords; the parser multiplies by 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AutoloadEntry {
    pub id: u8,
    pub offset: u32,
    pub size: u32,
}

fn rd_u32_le(b: &[u8], off: usize) -> Option<u32> {
    let s = b.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

const MAX_FW_ID: u32 = 0x40; // SOC21_FIRMWARE_ID_MAX is < 64 (7-bit id field)

/// Decode one 16-byte `RLC_TABLE_OF_CONTENT` (V1/V2 share DW0/DW1 here): DW0 =
/// `offset[0:24] | id[25:31]`, DW1 `size` = bits [14:31]. Offsets/sizes are in
/// dwords on disk → ×4 to bytes. Returns `None` for an out-of-range id (the TOC
/// terminator / past-end).
fn decode_entry(blob: &[u8], off: usize) -> Option<AutoloadEntry> {
    let dw0 = rd_u32_le(blob, off)?;
    let dw1 = rd_u32_le(blob, off + 4)?;
    let id = (dw0 >> 25) & 0x7F;
    if id == 0 || id >= MAX_FW_ID {
        return None;
    }
    let offset = (dw0 & 0x01FF_FFFF).checked_mul(4)?;
    let size = ((dw1 >> 14) & 0x0003_FFFF).checked_mul(4)?;
    Some(AutoloadEntry {
        id: id as u8,
        offset,
        size,
    })
}

/// Find the first byte position (4-byte aligned, at/after `from`) where the TOC
/// entries actually begin. The TOC's first entry is always `RLC_G_UCODE` (id 1)
/// at buffer offset 0 (the IMU bootloader source), immediately followed by another
/// valid entry — a strong, specific anchor that robustly skips the leading `$PS1`
/// signature block regardless of its size.
fn find_toc_start(blob: &[u8], from: usize) -> Option<usize> {
    let mut pos = from;
    while pos + 32 <= blob.len() {
        if let Some(a) = decode_entry(blob, pos) {
            if a.id == FW_ID_RLC_G_UCODE && a.offset == 0 && decode_entry(blob, pos + 16).is_some()
            {
                return Some(pos);
            }
        }
        pos += 4;
    }
    None
}

/// Parse the `RLC_TABLE_OF_CONTENT` from a PSP TOC firmware blob
/// (`psp_*_toc.bin`) into the per-firmware autoload layout. Reads
/// `ucode_array_offset_bytes` @24 of the firmware header, scans past the signature
/// block to the entry table, and decodes 16-byte entries until the terminator.
/// Bounds-checked end to end; returns `None` if no entry table is found.
pub fn parse_psp_toc(blob: &[u8]) -> Option<Vec<AutoloadEntry>> {
    let ucode_off = rd_u32_le(blob, 24)? as usize;
    let start = find_toc_start(blob, ucode_off)?;
    let mut entries = Vec::new();
    let mut off = start;
    while let Some(e) = decode_entry(blob, off) {
        entries.push(e);
        off += 16;
    }
    if entries.is_empty() {
        None
    } else {
        Some(entries)
    }
}

/// Total size of the autoload buffer = the highest `offset + size` over all
/// entries (the BO that holds every firmware, 64 KiB-aligned by the allocator).
pub fn autoload_bo_size(entries: &[AutoloadEntry]) -> u32 {
    entries
        .iter()
        .map(|e| e.offset.saturating_add(e.size))
        .max()
        .unwrap_or(0)
}

/// Look up an entry by firmware id (e.g. RLC_G to program the IMU bootloader).
pub fn entry_for(entries: &[AutoloadEntry], id: u8) -> Option<AutoloadEntry> {
    entries.iter().copied().find(|e| e.id == id)
}

// ── SOC21_FIRMWARE_ID values (amdgpu_rlc.h) ───────────────────────────────────
// The firmwares the IMU autoload buffer holds. On the F32 Phoenix APU only a
// SUBSET is filled — the RS64 CP slots (RS64_PFP=18, RS64_ME=19, RS64_MEC=20 and
// their P0..P3 stacks 23..30) stay EMPTY, because Phoenix's CP is F32 and uses
// CP_PFP/CP_ME/CP_MEC (13/14/15). Confirmed off-target: the real psp_13_0_4_toc
// fills 13/14/15 with ~263 KB each (matching the v1.0 pfp/me/mec blobs).
pub const FW_ID_RLC_G_UCODE: u8 = 1;
pub const FW_ID_RLC_TOC: u8 = 2;
pub const FW_ID_RLCG_SCRATCH: u8 = 3;
pub const FW_ID_RLC_SRM_ARAM: u8 = 4;
pub const FW_ID_RLC_P_UCODE: u8 = 5;
pub const FW_ID_RLC_V_UCODE: u8 = 6;
pub const FW_ID_RLX6_UCODE: u8 = 7;
pub const FW_ID_RLX6_DRAM_BOOT: u8 = 9;
pub const FW_ID_SDMA_UCODE_TH0: u8 = 11;
pub const FW_ID_SDMA_UCODE_TH1: u8 = 12;
pub const FW_ID_CP_PFP: u8 = 13; // F32 PFP (Phoenix) — NOT RS64_PFP (18)
pub const FW_ID_CP_ME: u8 = 14; // F32 ME
pub const FW_ID_CP_MEC: u8 = 15; // F32 MEC
pub const FW_ID_RS64_MES_P0: u8 = 16;
pub const FW_ID_RS64_MES_P1: u8 = 17;
pub const FW_ID_RS64_MES_P0_STACK: u8 = 21;
pub const FW_ID_RS64_MES_P1_STACK: u8 = 22;

/// One firmware payload to place in the autoload buffer: the SOC21 firmware `id`
/// and the raw ucode bytes (already extracted from its `*.bin` blob).
#[derive(Clone, Copy)]
pub struct FwSource<'a> {
    pub id: u8,
    pub data: &'a [u8],
}

/// Assemble the contiguous RLC autoload buffer: a zero-filled BO of
/// [`autoload_bo_size`], with each firmware's ucode copied into its TOC slot
/// (`entry.offset`), clamped to the slot size (`entry.size`) and zero-padded — the
/// `gfx_v11_0_rlc_backdoor_autoload_copy_ucode` contract. Sources whose id isn't in
/// the TOC are skipped. The IMU reads this buffer (pointed at the RLC_G slot) to
/// bring GFX out of reset. Pure logic, host-KAT'd; the daemon then DMA-copies the
/// result into a real VRAM/GTT BO.
pub fn assemble_autoload_buffer(entries: &[AutoloadEntry], sources: &[FwSource<'_>]) -> Vec<u8> {
    let total = autoload_bo_size(entries) as usize;
    let mut buf = alloc::vec![0u8; total];
    for src in sources {
        if let Some(e) = entry_for(entries, src.id) {
            let off = e.offset as usize;
            let cap = e.size as usize;
            let n = src.data.len().min(cap);
            if off.checked_add(n).map_or(false, |end| end <= buf.len()) {
                buf[off..off + n].copy_from_slice(&src.data[..n]);
            }
        }
    }
    buf
}

// ── per-blob ucode extraction (step 3) ───────────────────────────────────────
// Each `*.bin` blob carries its ucode behind a firmware header; the autoload
// buffer needs the raw ucode bytes per SOC21 firmware id. These extractors pull
// the right section out of each header type. All bounds-checked: a short/corrupt
// blob yields `None` rather than an out-of-range slice.

/// Single-section ucode = `common_firmware_header.ucode_array_offset_bytes[24] ..
/// + ucode_size_bytes[20]`. Covers RLC_G (the IMU bootloader source), the F32 CP
/// PFP/ME/MEC (`gfx_firmware_header_v1_0`), and the MES main ucode.
pub fn extract_common_ucode(blob: &[u8]) -> Option<&[u8]> {
    let ucode_size = rd_u32_le(blob, 20)? as usize;
    let ucode_off = rd_u32_le(blob, 24)? as usize;
    blob.get(ucode_off..ucode_off.checked_add(ucode_size)?)
}

/// The two SDMA threads from an `sdma_firmware_header_v2_0` blob for the PSP
/// autoload path: TH0 (context) and TH1 (control) are CONSECUTIVE slices of the
/// ucode array at `ucode_array_offset_bytes[24]` — TH0 the first `ctx_jt_offset[36]`
/// bytes, TH1 the next `ctl_jt_offset[52]` bytes (matching the working-amdgpu PSP
/// mmiotrace: fw type 71 = 17408 B, 72 = 16896 B, summing to the 34304 B array).
/// Returns `(th0, th1)` for `FW_ID_SDMA_UCODE_TH0` / `FW_ID_SDMA_UCODE_TH1`; see the
/// inline note for why the old `ctx_ucode_size_bytes[32]`/`ctl_ucode_offset[44]` read
/// was wrong. NOTE: this differs from `sdma::sdma_ucode_slices` (DIRECT-load), which
/// needs each thread's jump table appended — a different consumer, a different slice.
pub fn extract_sdma_threads(blob: &[u8]) -> Option<(&[u8], &[u8])> {
    // CORRECTED 2026-06-28 against the real sdma_6_0_1.bin header (off-target hexdump) +
    // the working-amdgpu mmiotrace (ftype 71 sz=17408, ftype 72 sz=16896). The two
    // threads are CONSECUTIVE slices of the ucode array (which starts at
    // ucode_array_offset_bytes@24=256 and is 34304 B): TH0 (context) is the first
    // ctx_jt_offset@36 (=17408) bytes, TH1 (control) the next ctl_jt_offset@52 (=16896)
    // bytes. The OLD code read ctx_ucode_size_bytes@32 (=60!) as the TH0 size and
    // ctl_ucode_offset@44 (=128) as the TH1 start, producing a 60-byte TH0 the PSP
    // rejected with 0xffff0010. (ctx/ctl_jt_offset are the per-thread image lengths here;
    // their JTs overlap into the next thread's region — normal for dual-thread RS64.)
    let ucode_off = rd_u32_le(blob, 24)? as usize; // ucode_array_offset_bytes (256)
    let th0_len = rd_u32_le(blob, 36)? as usize; // ctx_jt_offset (17408) = TH0 length
    let th1_len = rd_u32_le(blob, 52)? as usize; // ctl_jt_offset (16896) = TH1 length
    let th0_end = ucode_off.checked_add(th0_len)?;
    let th0 = blob.get(ucode_off..th0_end)?;
    let th1_end = th0_end.checked_add(th1_len)?;
    let th1 = blob.get(th0_end..th1_end)?;
    Some((th0, th1))
}

/// The MES ucode + data regions from a `mes_firmware_header_v1_0` blob (gfx11
/// `gc_11_0_1_mes_2.bin` / `mes1.bin`). After the 32-byte common header the v1_0
/// fields are: `mes_ucode_version`@32, `mes_ucode_size_bytes`@36,
/// `mes_ucode_offset_bytes`@40, `mes_ucode_data_version`@44,
/// `mes_ucode_data_size_bytes`@48, `mes_ucode_data_offset_bytes`@52 (verified
/// off-target against the real Athena blobs: mes_2 ucode 127040B@256 / data
/// 131072B@127296; mes1 ucode 104016B@256 / data 131072B@104272). Returns
/// `(ucode, data)` for `RS64_MES`+`RS64_MES_STACK` (pipe0) or
/// `RS64_KIQ`+`RS64_KIQ_STACK` (pipe1/KIQ). `None` if any slice is out of bounds.
pub fn extract_mes_ucode_data(blob: &[u8]) -> Option<(&[u8], &[u8])> {
    let ucode_size = rd_u32_le(blob, 36)? as usize;
    let ucode_off = rd_u32_le(blob, 40)? as usize;
    let data_size = rd_u32_le(blob, 48)? as usize;
    let data_off = rd_u32_le(blob, 52)? as usize;
    let ucode = blob.get(ucode_off..ucode_off.checked_add(ucode_size)?)?;
    let data = blob.get(data_off..data_off.checked_add(data_size)?)?;
    Some((ucode, data))
}

/// The RLX6 (RLC microcontroller) ucode regions from an `rlc_firmware_header_v2_2+`
/// blob: IRAM (`FW_ID_RLX6_UCODE`) and DRAM boot (`FW_ID_RLX6_DRAM_BOOT`). The
/// `rlc_iram/rlc_dram` size+offset fields sit at a fixed position after the v2_0
/// (18 u32, ends @104) + v2_1 (13 u32, ends @156) prefix: iram_size@156,
/// iram_off@160, dram_size@164, dram_off@168 (verified against the real
/// gc_11_0_1_rlc.bin, a v2.3 header). Returns `None` unless the header is v2.2+.
pub fn extract_rlc_rlx6(blob: &[u8]) -> Option<(&[u8], &[u8])> {
    let ver_major = rd_u32_le(blob, 8)? & 0xFFFF; // header_version_major (u16 @8)
    let ver_minor = (rd_u32_le(blob, 8)? >> 16) & 0xFFFF; // header_version_minor (u16 @10)
    if ver_major != 2 || ver_minor < 2 {
        return None;
    }
    let iram_size = rd_u32_le(blob, 156)? as usize;
    let iram_off = rd_u32_le(blob, 160)? as usize;
    let dram_size = rd_u32_le(blob, 164)? as usize;
    let dram_off = rd_u32_le(blob, 168)? as usize;
    let iram = blob.get(iram_off..iram_off.checked_add(iram_size)?)?;
    let dram = blob.get(dram_off..dram_off.checked_add(dram_size)?)?;
    Some((iram, dram))
}

/// RLC_P ucode from an `rlc_firmware_header_v2_3+` blob: rlcp_ucode_size_bytes @180,
/// rlcp_ucode_offset_bytes @184 (after the v2_2 iram/dram block @156..172). amdgpu
/// loads this as a separate `GFX_FW_TYPE_RLC_P` LOAD_IP_FW — the Athena
/// amdgpu_firmware_info shows RLCP (feature 1, ver 0x0f) loaded, and AthenaOS was NOT
/// loading it. `None` for a pre-v2.3 header.
pub fn extract_rlcp(blob: &[u8]) -> Option<&[u8]> {
    let ver_major = rd_u32_le(blob, 8)? & 0xFFFF;
    let ver_minor = (rd_u32_le(blob, 8)? >> 16) & 0xFFFF;
    if ver_major != 2 || ver_minor < 3 {
        return None;
    }
    let size = rd_u32_le(blob, 180)? as usize;
    let off = rd_u32_le(blob, 184)? as usize;
    blob.get(off..off.checked_add(size)?)
}

/// RLC GPM restore list (`save_restore_list_gpm` in rlc_firmware_header_v2_1 —
/// size @132, offset @136). PSP fw_type RLC_RESTORE_LIST_GPM_MEM (20). amdgpu
/// loads this in its autoload batch (iron trace); AthenaOS omitted it. Requires
/// header v2.1+ (Athena's rlc.bin is v2.3). Iron-verified size = 2560.
pub fn extract_rlc_gpm(blob: &[u8]) -> Option<&[u8]> {
    let ver = rd_u32_le(blob, 8)?;
    if ver & 0xFFFF != 2 || (ver >> 16) & 0xFFFF < 1 {
        return None;
    }
    let size = rd_u32_le(blob, 132)? as usize;
    let off = rd_u32_le(blob, 136)? as usize;
    if size == 0 {
        return None;
    }
    blob.get(off..off.checked_add(size)?)
}

/// RLC SRM (save/restore machine) list (`save_restore_list_srm` — size @148,
/// offset @152). PSP fw_type RLC_RESTORE_LIST_SRM_MEM (21). The RLC uses this to
/// save/restore the GFX pipeline state; without it the state the MES per-pipe
/// context reads is never fully established. Iron-verified size = 21104.
pub fn extract_rlc_srm(blob: &[u8]) -> Option<&[u8]> {
    let ver = rd_u32_le(blob, 8)?;
    if ver & 0xFFFF != 2 || (ver >> 16) & 0xFFFF < 1 {
        return None;
    }
    let size = rd_u32_le(blob, 148)? as usize;
    let off = rd_u32_le(blob, 152)? as usize;
    if size == 0 {
        return None;
    }
    blob.get(off..off.checked_add(size)?)
}

/// MES ucode + data from a `mes_firmware_header_v1_0` blob: ucode @
/// mes_ucode_offset_bytes[40] size[36]; data @ mes_ucode_data_offset_bytes[52]
/// size[48]. Returns `(ucode, data)` for a pipe's MES_Pn / MES_Pn_STACK ids.
pub fn extract_mes(blob: &[u8]) -> Option<(&[u8], &[u8])> {
    let ucode_size = rd_u32_le(blob, 36)? as usize;
    let ucode_off = rd_u32_le(blob, 40)? as usize;
    let data_size = rd_u32_le(blob, 48)? as usize;
    let data_off = rd_u32_le(blob, 52)? as usize;
    let ucode = blob.get(ucode_off..ucode_off.checked_add(ucode_size)?)?;
    let data = blob.get(data_off..data_off.checked_add(data_size)?)?;
    Some((ucode, data))
}

/// The firmware blobs (as loaded by `request_firmware_bytes`) feeding the autoload
/// buffer. `mes_p0` is the scheduler pipe (gc_*_mes_2.bin), `mes_p1` the KIQ pipe
/// (gc_*_mes1.bin) — the pipe→blob mapping is the one open assumption to confirm
/// on iron; everything else is offset-exact.
pub struct AutoloadBlobs<'a> {
    pub toc: &'a [u8],
    pub rlc: &'a [u8],
    pub sdma: &'a [u8],
    pub pfp: &'a [u8],
    pub me: &'a [u8],
    pub mec: &'a [u8],
    pub mes_p0: &'a [u8],
    pub mes_p1: &'a [u8],
}

/// Build the complete 6.4 MiB RLC autoload buffer from the firmware blobs: parse
/// the TOC, extract each firmware's ucode, and place it at its TOC slot. This is
/// the whole DIRECT-load buffer in pure logic (`gfx_v11_0_rlc_backdoor_autoload_*`);
/// the daemon then DMA-copies the result into a VRAM/GTT BO and points the IMU
/// bootloader at the RLC_G slot. On F32 Phoenix the RS64 CP slots stay zero.
/// Returns `None` only if the TOC itself won't parse.
pub fn build_autoload_buffer(blobs: &AutoloadBlobs<'_>) -> Option<Vec<u8>> {
    let entries = parse_psp_toc(blobs.toc)?;
    let mut sources: Vec<FwSource<'_>> = Vec::new();
    if let Some(u) = extract_common_ucode(blobs.rlc) {
        sources.push(FwSource {
            id: FW_ID_RLC_G_UCODE,
            data: u,
        });
    }
    if let Some((iram, dram)) = extract_rlc_rlx6(blobs.rlc) {
        sources.push(FwSource {
            id: FW_ID_RLX6_UCODE,
            data: iram,
        });
        sources.push(FwSource {
            id: FW_ID_RLX6_DRAM_BOOT,
            data: dram,
        });
    }
    if let Some((th0, th1)) = extract_sdma_threads(blobs.sdma) {
        sources.push(FwSource {
            id: FW_ID_SDMA_UCODE_TH0,
            data: th0,
        });
        sources.push(FwSource {
            id: FW_ID_SDMA_UCODE_TH1,
            data: th1,
        });
    }
    if let Some(u) = extract_common_ucode(blobs.pfp) {
        sources.push(FwSource {
            id: FW_ID_CP_PFP,
            data: u,
        });
    }
    if let Some(u) = extract_common_ucode(blobs.me) {
        sources.push(FwSource {
            id: FW_ID_CP_ME,
            data: u,
        });
    }
    if let Some(u) = extract_common_ucode(blobs.mec) {
        sources.push(FwSource {
            id: FW_ID_CP_MEC,
            data: u,
        });
    }
    if let Some((uc, dt)) = extract_mes(blobs.mes_p0) {
        sources.push(FwSource {
            id: FW_ID_RS64_MES_P0,
            data: uc,
        });
        sources.push(FwSource {
            id: FW_ID_RS64_MES_P0_STACK,
            data: dt,
        });
    }
    if let Some((uc, dt)) = extract_mes(blobs.mes_p1) {
        sources.push(FwSource {
            id: FW_ID_RS64_MES_P1,
            data: uc,
        });
        sources.push(FwSource {
            id: FW_ID_RS64_MES_P1_STACK,
            data: dt,
        });
    }
    Some(assemble_autoload_buffer(&entries, &sources))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// Encode a 16-byte TOC entry the way the on-disk format does (offset/size in
    /// dwords). Mirrors the real `psp_13_0_4_toc.bin` field packing.
    fn enc_entry(id: u8, offset_bytes: u32, size_bytes: u32) -> [u8; 16] {
        let dw0 = (offset_bytes / 4) | ((id as u32) << 25);
        let dw1 = (size_bytes / 4) << 14;
        let mut e = [0u8; 16];
        e[0..4].copy_from_slice(&dw0.to_le_bytes());
        e[4..8].copy_from_slice(&dw1.to_le_bytes());
        e
    }

    /// Build a synthetic `psp_*_toc.bin`: header (ucode_array_offset=256), a
    /// 256-byte `$PS1` signature block (garbage ids), then the sequential entry
    /// table — exactly the real Phoenix shape (entries at 512).
    fn synth_toc(entries: &[(u8, u32, u32)]) -> Vec<u8> {
        let mut b = vec![0u8; 256];
        b[24..28].copy_from_slice(&256u32.to_le_bytes()); // ucode_array_offset_bytes
                                                          // 256-byte signature block (offset 256..512), seeded so ids look invalid.
        let mut sig = vec![0xFFu8; 256];
        sig[16..20].copy_from_slice(b"$PS1");
        b.extend_from_slice(&sig);
        for &(id, off, sz) in entries {
            b.extend_from_slice(&enc_entry(id, off, sz));
        }
        b.extend_from_slice(&[0u8; 16]); // terminator (id 0)
        b
    }

    #[test]
    fn parses_sequential_toc_past_signature() {
        // Mirror the real Phoenix layout's first entries.
        let real = [
            (1u8, 0u32, 24576u32),
            (2, 24576, 1792),
            (3, 26368, 2048),
            (4, 28416, 28672),
            (5, 57088, 8192),
        ];
        let blob = synth_toc(&real);
        let parsed = parse_psp_toc(&blob).expect("valid TOC parses");
        assert_eq!(parsed.len(), real.len());
        for (p, (id, off, sz)) in parsed.iter().zip(real.iter()) {
            assert_eq!(p.id, *id);
            assert_eq!(p.offset, *off);
            assert_eq!(p.size, *sz);
        }
        // BO size = max(offset+size) = entry 5: 57088 + 8192 = 65280.
        assert_eq!(autoload_bo_size(&parsed), 65280);
        // RLC_G lookup (id 1) for the bootloader programming.
        let rlc_g = entry_for(&parsed, 1).unwrap();
        assert_eq!((rlc_g.offset, rlc_g.size), (0, 24576));
    }

    #[test]
    fn extract_mes_matches_real_header() {
        // Synthetic mes_firmware_header_v1_0 with the REAL Athena gc_11_0_1_mes_2.bin
        // field values (verified off-target): ucode 127040B@256, data 131072B@127296.
        let total = 127296 + 131072;
        let mut b = vec![0u8; total];
        let put = |b: &mut [u8], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut b, 36, 127040); // mes_ucode_size_bytes
        put(&mut b, 40, 256); // mes_ucode_offset_bytes
        put(&mut b, 48, 131072); // mes_ucode_data_size_bytes
        put(&mut b, 52, 127296); // mes_ucode_data_offset_bytes
        let (ucode, data) = extract_mes_ucode_data(&b).expect("valid mes header");
        assert_eq!(ucode.len(), 127040);
        assert_eq!(data.len(), 131072);
        // A truncated blob (slice past EOF) returns None, never a panic.
        assert!(extract_mes_ucode_data(&b[..1000]).is_none());
    }

    #[test]
    fn assembles_buffer_places_each_fw_at_its_slot() {
        // TOC: RLC_G at 0 (cap 32), CP_PFP at 64 (cap 16). Gap 32..64 stays zero.
        let blob = synth_toc(&[(FW_ID_RLC_G_UCODE, 0, 32), (FW_ID_CP_PFP, 64, 16)]);
        let entries = parse_psp_toc(&blob).unwrap();
        assert_eq!(autoload_bo_size(&entries), 80);
        let rlc = [0xAAu8; 32];
        let pfp = [0xBBu8; 24]; // bigger than the 16-byte slot -> clamped
        let buf = assemble_autoload_buffer(
            &entries,
            &[
                FwSource {
                    id: FW_ID_RLC_G_UCODE,
                    data: &rlc,
                },
                FwSource {
                    id: FW_ID_CP_PFP,
                    data: &pfp,
                },
                FwSource {
                    id: 99,
                    data: &[0xCC; 8],
                }, // id not in TOC -> ignored
            ],
        );
        assert_eq!(buf.len(), 80);
        assert!(buf[0..32].iter().all(|&b| b == 0xAA), "RLC_G filled");
        assert!(buf[32..64].iter().all(|&b| b == 0x00), "gap stays zero");
        assert!(
            buf[64..80].iter().all(|&b| b == 0xBB),
            "CP_PFP clamped to slot"
        );
    }

    #[test]
    fn extract_common_ucode_slices_the_payload() {
        // common header: ucode_size@20, ucode_array_offset@24.
        let mut b = [0u8; 64].to_vec();
        b[20..24].copy_from_slice(&16u32.to_le_bytes()); // ucode_size_bytes
        b[24..28].copy_from_slice(&32u32.to_le_bytes()); // ucode_array_offset_bytes
        for (i, byte) in b.iter_mut().enumerate().skip(32).take(16) {
            *byte = 0xC0u8.wrapping_add(i as u8);
        }
        let u = extract_common_ucode(&b).expect("valid");
        assert_eq!(u.len(), 16);
        assert_eq!(u[0], 0xC0u8.wrapping_add(32));
        // A blob too short for its declared ucode -> None.
        let mut bad = b.clone();
        bad.truncate(40);
        assert!(extract_common_ucode(&bad).is_none());
    }

    #[test]
    fn extract_sdma_splits_two_threads() {
        // Synthetic sdma_firmware_header_v2_0 carrying the REAL Athena sdma_6_0_1.bin
        // field values (verified off-target, same header the sdma.rs KAT uses). The PSP
        // autoload path takes the two threads as CONSECUTIVE slices of the ucode array:
        // TH0 = the first ctx_jt_offset@36 bytes, TH1 = the next ctl_jt_offset@52 bytes —
        // matching the working-amdgpu mmiotrace (fw type 71 = 17408 B, 72 = 16896 B,
        // together filling the 34304 B ucode array at ucode_array_offset@24 = 256).
        let ucode_off = 256usize;
        let th0_len = 17408usize; // ctx_jt_offset = TH0 length
        let th1_len = 16896usize; // ctl_jt_offset = TH1 length
        let mut b = vec![0u8; ucode_off + th0_len + th1_len]; // 34560 B (256 header + 34304 array)
        let put = |b: &mut [u8], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut b, 24, ucode_off as u32); // ucode_array_offset_bytes
        put(&mut b, 36, th0_len as u32); // ctx_jt_offset  -> TH0 length (what the fn reads)
        put(&mut b, 52, th1_len as u32); // ctl_jt_offset  -> TH1 length (what the fn reads)
                                         // Decoy fields the REVERTED bug read (@32/@44/@48) — seeded with the real header's
                                         // values so this KAT FAILs loudly if anyone re-reads them: the old code took
                                         // ctx_ucode_size_bytes@32 (=60) as TH0 size and sent the PSP a 60-byte TH0 it
                                         // rejected with 0xffff0010.
        put(&mut b, 32, 60); // ctx_ucode_size_bytes (the 60-byte TH0 trap)
        put(&mut b, 44, 128); // ctl_ucode_offset
        put(&mut b, 48, 16896); // ctl_ucode_size_bytes
                                // Distinct fills for the two back-to-back thread regions.
        for byte in b[ucode_off..ucode_off + th0_len].iter_mut() {
            *byte = 0x11;
        }
        for byte in b[ucode_off + th0_len..ucode_off + th0_len + th1_len].iter_mut() {
            *byte = 0x22;
        }
        let (th0, th1) = extract_sdma_threads(&b).expect("valid sdma");
        assert_eq!(th0.len(), 17408, "TH0 = ctx_jt_offset");
        assert_eq!(th1.len(), 16896, "TH1 = ctl_jt_offset");
        assert_eq!(
            th0.len() + th1.len(),
            34304,
            "the two threads fill the ucode array"
        );
        assert!(th0.iter().all(|&x| x == 0x11), "TH0 is the first slice");
        assert!(
            th1.iter().all(|&x| x == 0x22),
            "TH1 is the consecutive next slice"
        );
        // A truncated blob (thread slice past EOF) returns None, never a panic.
        assert!(extract_sdma_threads(&b[..1000]).is_none());
    }

    #[test]
    fn extract_rlc_rlx6_reads_v22_fields() {
        // v2.2+ header (major=2,minor=3); iram@200 size 8, dram@208 size 12.
        let mut b = [0u8; 220].to_vec();
        b[8..10].copy_from_slice(&2u16.to_le_bytes()); // header_version_major
        b[10..12].copy_from_slice(&3u16.to_le_bytes()); // header_version_minor
        b[156..160].copy_from_slice(&8u32.to_le_bytes()); // rlc_iram_ucode_size_bytes
        b[160..164].copy_from_slice(&200u32.to_le_bytes()); // rlc_iram_ucode_offset_bytes
        b[164..168].copy_from_slice(&12u32.to_le_bytes()); // rlc_dram_ucode_size_bytes
        b[168..172].copy_from_slice(&208u32.to_le_bytes()); // rlc_dram_ucode_offset_bytes
        for byte in b.iter_mut().skip(200).take(8) {
            *byte = 0x66;
        }
        for byte in b.iter_mut().skip(208).take(12) {
            *byte = 0x99;
        }
        let (iram, dram) = extract_rlc_rlx6(&b).expect("v2.2 parses");
        assert_eq!(iram.len(), 8);
        assert_eq!(dram.len(), 12);
        assert!(iram.iter().all(|&x| x == 0x66));
        assert!(dram.iter().all(|&x| x == 0x99));
        // A v2.0 header (no RLX6 fields) -> None.
        let mut old = b.clone();
        old[10..12].copy_from_slice(&0u16.to_le_bytes()); // minor 0
        assert!(extract_rlc_rlx6(&old).is_none());
    }

    #[test]
    fn extract_rlc_gpm_srm_read_v21_fields() {
        // v2.1+ header; GPM size@132 off@136, SRM size@148 off@152 (matches the
        // real Athena rlc.bin: GPM 2560B, SRM 21104B — sizes shrunk here for the KAT).
        let mut b = [0u8; 300].to_vec();
        b[8..10].copy_from_slice(&2u16.to_le_bytes()); // major
        b[10..12].copy_from_slice(&1u16.to_le_bytes()); // minor 1
        b[132..136].copy_from_slice(&6u32.to_le_bytes()); // gpm size
        b[136..140].copy_from_slice(&200u32.to_le_bytes()); // gpm offset
        b[148..152].copy_from_slice(&10u32.to_le_bytes()); // srm size
        b[152..156].copy_from_slice(&220u32.to_le_bytes()); // srm offset
        for byte in b.iter_mut().skip(200).take(6) {
            *byte = 0x20;
        }
        for byte in b.iter_mut().skip(220).take(10) {
            *byte = 0x21;
        }
        let gpm = extract_rlc_gpm(&b).expect("gpm parses");
        let srm = extract_rlc_srm(&b).expect("srm parses");
        assert_eq!(gpm.len(), 6);
        assert!(gpm.iter().all(|&x| x == 0x20));
        assert_eq!(srm.len(), 10);
        assert!(srm.iter().all(|&x| x == 0x21));
        // A v2.0 header (no restore-list fields) -> None (FAIL-able).
        let mut old = b.clone();
        old[10..12].copy_from_slice(&0u16.to_le_bytes());
        assert!(extract_rlc_gpm(&old).is_none());
        assert!(extract_rlc_srm(&old).is_none());
        // A zero-size list (absent, like CNTL) -> None, not an empty slice.
        let mut nogpm = b.clone();
        nogpm[132..136].copy_from_slice(&0u32.to_le_bytes());
        assert!(extract_rlc_gpm(&nogpm).is_none());
    }

    #[test]
    fn extract_mes_reads_ucode_and_data() {
        let mut b = [0u8; 96].to_vec();
        b[36..40].copy_from_slice(&8u32.to_le_bytes()); // mes_ucode_size_bytes
        b[40..44].copy_from_slice(&64u32.to_le_bytes()); // mes_ucode_offset_bytes
        b[48..52].copy_from_slice(&12u32.to_le_bytes()); // mes_ucode_data_size_bytes
        b[52..56].copy_from_slice(&72u32.to_le_bytes()); // mes_ucode_data_offset_bytes
        for byte in b.iter_mut().skip(64).take(8) {
            *byte = 0x55;
        }
        for byte in b.iter_mut().skip(72).take(12) {
            *byte = 0x77;
        }
        let (uc, dt) = extract_mes(&b).expect("valid mes");
        assert_eq!(uc.len(), 8);
        assert_eq!(dt.len(), 12);
        assert!(uc.iter().all(|&x| x == 0x55));
        assert!(dt.iter().all(|&x| x == 0x77));
    }

    #[test]
    fn build_autoload_buffer_places_rlc_and_cp() {
        // TOC: RLC_G@0 cap16, CP_PFP@64 cap16.
        let toc = synth_toc(&[(FW_ID_RLC_G_UCODE, 0, 16), (FW_ID_CP_PFP, 64, 16)]);
        // rlc blob: common ucode (RLC_G) of 16 bytes 0xAA at offset 32. v2.0 header
        // (minor 0) so extract_rlc_rlx6 returns None (no RLX6 slots in this TOC).
        let mut rlc = [0u8; 48].to_vec();
        rlc[20..24].copy_from_slice(&16u32.to_le_bytes());
        rlc[24..28].copy_from_slice(&32u32.to_le_bytes());
        for byte in rlc.iter_mut().skip(32).take(16) {
            *byte = 0xAA;
        }
        // pfp blob: common ucode (CP_PFP) of 16 bytes 0xBB.
        let mut pfp = [0u8; 48].to_vec();
        pfp[20..24].copy_from_slice(&16u32.to_le_bytes());
        pfp[24..28].copy_from_slice(&32u32.to_le_bytes());
        for byte in pfp.iter_mut().skip(32).take(16) {
            *byte = 0xBB;
        }
        let empty = [0u8; 64];
        let blobs = AutoloadBlobs {
            toc: &toc,
            rlc: &rlc,
            sdma: &empty,
            pfp: &pfp,
            me: &empty,
            mec: &empty,
            mes_p0: &empty,
            mes_p1: &empty,
        };
        let buf = build_autoload_buffer(&blobs).expect("builds");
        assert_eq!(buf.len(), 80);
        assert!(buf[0..16].iter().all(|&x| x == 0xAA), "RLC_G placed");
        assert!(buf[64..80].iter().all(|&x| x == 0xBB), "CP_PFP placed");
        assert!(
            buf[16..64].iter().all(|&x| x == 0x00),
            "unfilled stays zero"
        );
    }

    #[test]
    fn rejects_blob_with_no_table() {
        // Header present but the "table" never has two sequential valid ids.
        let mut b = vec![0u8; 256];
        b[24..28].copy_from_slice(&256u32.to_le_bytes());
        b.extend_from_slice(&[0xFFu8; 256]); // all-0xFF: ids out of range
        assert!(parse_psp_toc(&b).is_none());
    }

    #[test]
    fn dword_scaling_is_applied() {
        // On-disk offset/size are dwords; the parser must return BYTES (×4).
        let blob = synth_toc(&[(1, 0, 16), (2, 16, 32)]);
        let p = parse_psp_toc(&blob).unwrap();
        assert_eq!((p[0].offset, p[0].size), (0, 16));
        assert_eq!((p[1].offset, p[1].size), (16, 32));
        assert_eq!(autoload_bo_size(&p), 48);
    }
}
