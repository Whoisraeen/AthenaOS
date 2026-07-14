//! SDMA 6.0 (sdma_v6 — Phoenix SDMA 6.0.1) command-packet builders.
//!
//! The SDMA (System DMA) engine is the GPU's asynchronous copy/fill engine: it
//! clears and blits memory with the CPU out of the copy path (VRAM clears,
//! scanout uploads, BO migration). These are the exact dword encodings the SDMA
//! microcode consumes, taken verbatim from the upstream `sdma_v6_0_0_pkt_open.h`
//! packet header and the `sdma_v6_0.c` emit functions — reproduced as pure
//! builders so every encoding is validated by `cargo test -p raeen_amdgpu` on the
//! host (the same host-KAT discipline as [`crate::pm4`]).
//!
//! Header dword layout: the opcode is in bits `[7:0]`
//! (`SDMA_PKT_*_HEADER_op_shift = 0`), the sub-op in `[15:8]`, plus per-packet
//! fields (e.g. CONSTANT_FILL's `fillsize` in `[31:30]`, FENCE's `mtype` in
//! `[18:16]`).

use alloc::vec;
use alloc::vec::Vec;

// ── SDMA opcodes (sdma_v6_0_0_pkt_open.h) ─────────────────────────────────────
pub const SDMA_OP_NOP: u32 = 0;
pub const SDMA_OP_COPY: u32 = 1;
pub const SDMA_OP_FENCE: u32 = 5;
pub const SDMA_OP_CONST_FILL: u32 = 11;

/// `SDMA_PKT_FENCE_HEADER_MTYPE(0x3)` — memory type Uncached(UC), at bit 16
/// (mask 0x7). `sdma_v6_0_ring_emit_fence` sets this so the fence write is
/// immediately visible to the polling driver.
const FENCE_MTYPE_UC: u32 = 0x3 << 16;

/// `SDMA_OP_CONST_FILL` — fill `byte_count` bytes at `dst_gpu_addr` with the
/// 32-bit `src_data` pattern. Byte fill (`fillsize = 0`), exactly as
/// `sdma_v6_0_emit_fill_buffer` — the path `amdgpu_fill_buffer` uses to clear
/// VRAM with the SDMA engine. 5 dwords; the COUNT field is `byte_count - 1`.
pub fn constant_fill(dst_gpu_addr: u64, src_data: u32, byte_count: u32) -> Vec<u32> {
    vec![
        SDMA_OP_CONST_FILL,                  // header: op=11, sub_op=0, fillsize=0 (byte)
        (dst_gpu_addr & 0xffff_ffff) as u32, // dst lo
        (dst_gpu_addr >> 32) as u32,         // dst hi
        src_data,                            // fill pattern
        byte_count.saturating_sub(1),        // count = byte_count - 1 (no underflow at 0)
    ]
}

/// `SDMA_OP_FENCE` — when the SDMA engine drains to this packet it writes `seq`
/// to `addr` (which must be 4-byte aligned). The driver polls that memory to
/// learn the SDMA job completed. 4 dwords; mirrors `sdma_v6_0_ring_emit_fence`
/// (the 32-bit fence — a 64-bit fence is two of these).
pub fn fence(addr: u64, seq: u32) -> Vec<u32> {
    debug_assert!(addr & 0x3 == 0, "SDMA fence addr must be 4-byte aligned");
    vec![
        SDMA_OP_FENCE | FENCE_MTYPE_UC, // header: op=5, mtype=UC
        (addr & 0xffff_ffff) as u32,    // addr lo
        (addr >> 32) as u32,            // addr hi
        seq,                            // data
    ]
}

/// A complete SDMA job: a CONSTANT_FILL followed by a FENCE the engine posts on
/// completion — the SDMA equivalent of the GFX "submit + fence" pattern
/// ([`crate::bringup::submit_and_wait_fence`]). Returns the dword stream to write
/// into the SDMA ring; the caller rings the SDMA doorbell and polls `fence_addr`.
pub fn constant_fill_with_fence(
    dst_gpu_addr: u64,
    src_data: u32,
    byte_count: u32,
    fence_addr: u64,
    fence_value: u32,
) -> Vec<u32> {
    let mut s = constant_fill(dst_gpu_addr, src_data, byte_count);
    s.extend_from_slice(&fence(fence_addr, fence_value));
    s
}

/// `SDMA_SUBOP_COPY_LINEAR` (sub-op 0 of `SDMA_OP_COPY`) — the plain
/// buffer-to-buffer copy sub-opcode.
pub const SDMA_SUBOP_COPY_LINEAR: u32 = 0;

/// `SDMA_OP_COPY` / `SUB_OP=LINEAR` — copy `byte_count` bytes from `src_gpu_addr`
/// to `dst_gpu_addr`. 7 dwords, mirroring `sdma_v6_0_emit_copy_buffer` (the path
/// `amdgpu_copy_buffer` uses for BO uploads/blits/evictions). Both addresses are
/// GPU VAs (GART/VMID0). COUNT is `byte_count - 1`; DW2 (parameters: src/dst swap
/// + endian + cache policy) is 0 for a plain coherent copy, exactly as amdgpu.
pub fn linear_copy(src_gpu_addr: u64, dst_gpu_addr: u64, byte_count: u32) -> Vec<u32> {
    vec![
        SDMA_OP_COPY | (SDMA_SUBOP_COPY_LINEAR << 8), // header: op=1, sub_op=0 (linear)
        byte_count.saturating_sub(1),                 // count = byte_count - 1
        0,                                            // parameters (default sw/endian/cache)
        (src_gpu_addr & 0xffff_ffff) as u32,          // src lo
        (src_gpu_addr >> 32) as u32,                  // src hi
        (dst_gpu_addr & 0xffff_ffff) as u32,          // dst lo
        (dst_gpu_addr >> 32) as u32,                  // dst hi
    ]
}

/// A complete SDMA copy job: a LINEAR_COPY followed by a FENCE the engine posts on
/// completion. Returns the dword stream to write into the SDMA ring; the caller
/// rings the doorbell and polls `fence_addr`.
pub fn linear_copy_with_fence(
    src_gpu_addr: u64,
    dst_gpu_addr: u64,
    byte_count: u32,
    fence_addr: u64,
    fence_value: u32,
) -> Vec<u32> {
    let mut s = linear_copy(src_gpu_addr, dst_gpu_addr, byte_count);
    s.extend_from_slice(&fence(fence_addr, fence_value));
    s
}

// ── SDMA0_QUEUE0_RB_CNTL fields (gc_11_0_0_sh_mask.h) ─────────────────────────
/// `RB_ENABLE` (bit 0) — enables the ring-buffer queue.
pub const SDMA_RB_CNTL_RB_ENABLE: u32 = 1 << 0;
/// `RB_SIZE` field shift (bit 1). The field value is `log2(ring_size / 4)` — the
/// ring size expressed in DWORDS (`order_base_2(ring->ring_size / 4)` in
/// `sdma_v6_0_gfx_resume_instance`), NOT bytes like the CP `RB_BUFSZ`.
pub const SDMA_RB_CNTL_RB_SIZE_SHIFT: u32 = 1;
/// `F32_WPTR_POLL_ENABLE` (bit 11): the SDMA F32 firmware polls the write pointer
/// from `RB_WPTR_POLL_ADDR` memory instead of reading the `RB_WPTR` register. The
/// live Athena amdgpu sets this (umr: RB_CNTL=0x841817) AND the RLC autoload leaves
/// it set by default (0x40800) — so a queue programmed WITHOUT it (register-WPTR
/// mode) is never serviced. THE rung-1 fix.
pub const SDMA_RB_CNTL_F32_WPTR_POLL_ENABLE: u32 = 1 << 11;
/// `RB_PRIV` (bit 23): the ring runs privileged. The live amdgpu kernel ring sets
/// it; the CONSTANT_FILL/FENCE packets are privileged ops.
pub const SDMA_RB_CNTL_RB_PRIV: u32 = 1 << 23;
/// `SDMA0_QUEUE0_DOORBELL.ENABLE` (bit 28): route the queue to the doorbell aperture
/// so a write to its doorbell slot wakes the engine. The live Athena amdgpu sets
/// DOORBELL=0x10000000; without it (boot 170257) the engine stayed asleep (RB_RPTR=0).
pub const SDMA_DOORBELL_ENABLE: u32 = 1 << 28;

/// `SDMA0_F32_CNTL.HALT` (bit 0, field `HALT 0 0` from the Athena umr db
/// `gc_11_0_0.reg`). `sdma_v6_0_enable(true)` clears this to take the SDMA engine
/// OUT of halt; the RLC autoload loads the SDMA ucode but leaves the engine halted,
/// so without clearing HALT the ring is programmed but the engine never drains it
/// (iron boot 220442: "SDMA submitted; fence not posted"). RMW preserves the
/// TH0/TH1 reset/enable/priority fields (bits 9..31).
pub const SDMA_F32_CNTL_HALT: u32 = 1 << 0;
/// `SDMA0_F32_CNTL.TH1_RESET` (bit 13, umr db field `TH1_RESET 13 13`). amdgpu's
/// sdma_v6_0 start clears BOTH HALT and TH1_RESET to run the dual-thread RS64 engine.
pub const SDMA_F32_CNTL_TH1_RESET: u32 = 1 << 13;
/// `SDMA0_F32_CNTL` RS64 dual-thread run controls (umr db `gc_11_0_0.reg`):
/// `TH0_RESET` (bit 9), `TH0_ENABLE` (bit 10), `TH1_ENABLE` (bit 14). The GOP
/// leaves the engine with TH0_ENABLE=0 (iron readback F32_CNTL=0x08084000), so
/// clearing HALT alone leaves thread 0 — the main thread that DRAINS the ring —
/// disabled, and the engine never advances RB_RPTR. Starting the engine must
/// enable BOTH threads (and clear both resets), not just clear HALT.
pub const SDMA_F32_CNTL_TH0_RESET: u32 = 1 << 9;
pub const SDMA_F32_CNTL_TH0_ENABLE: u32 = 1 << 10;
pub const SDMA_F32_CNTL_TH1_ENABLE: u32 = 1 << 14;

// ── SDMA0_UTCL1_CNTL fields (gc_11_0_0.reg: regSDMA0_UTCL1_CNTL 0 0x3c) ───────
/// `REDO_DELAY` (bits 0-4) — retry delay for the UTC L1 (the engine's address-
/// translation cache).
pub const SDMA_UTCL1_CNTL_REDO_DELAY_MASK: u32 = 0x1F;
/// `RESP_MODE` (bits 9-10) — how the UTC L1 responds to translation requests.
pub const SDMA_UTCL1_CNTL_RESP_MODE_MASK: u32 = 0x3 << 9;
/// `sdma_v6_0_gfx_resume_instance` programs RESP_MODE=3 + REDO_DELAY=9 so the
/// engine's UTC L1 resolves the ring/WPTR/fence GPU addresses through VMID0.
/// UNPROGRAMMED, the engine cannot translate those addresses and never fetches
/// the ring — RB_RPTR stays 0 even with the ring armed and the engine unhalted.
pub const SDMA_UTCL1_CNTL_VALUE: u32 = 9 | (0x3 << 9);

/// Broadcast-load ADDR for thread 0 (context) and thread 1 (control) — the two
/// windows amdgpu's sdma_v6_0_load_microcode writes the RS64 ucode into.
pub const SDMA_UCODE_ADDR_TH0: u32 = 0;
pub const SDMA_UCODE_ADDR_TH1: u32 = 0x8000;

/// The two RS64 ucode images from an `sdma_firmware_header_v2_0` blob, EXACTLY as
/// `sdma_v6_0_load_microcode` reads them: TH0 (context) at `ucode_array_offset_bytes`
/// for `ctx_jt_offset + ctx_jt_size` bytes; TH1 (control) at `ctl_ucode_offset` for
/// `ctl_jt_offset + ctl_jt_size` bytes. (Verified off-target against the real
/// sdma_6_0_1.bin: ucode_array_offset=256, ctx_jt_offset=17408/ctx_jt_size=4096 →
/// TH0=21504 B @256; ctl_ucode_offset=128, ctl_jt_offset=16896/ctl_jt_size=4096 →
/// TH1=20992 B @128. The two threads load overlapping images — normal for dual-thread
/// RS64.) Returns `None` if any slice is out of bounds. Header field offsets:
/// ucode_array_offset_bytes@24, ctx_jt_offset@36, ctx_jt_size@40, ctl_ucode_offset@44,
/// ctl_jt_offset@52, ctl_jt_size@56.
pub fn sdma_ucode_slices(blob: &[u8]) -> Option<(&[u8], &[u8])> {
    let rd = |o: usize| -> Option<usize> {
        blob.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    let ucode_off = rd(24)?;
    let th0_len = rd(36)?.checked_add(rd(40)?)?;
    let ctl_off = rd(44)?;
    let th1_len = rd(52)?.checked_add(rd(56)?)?;
    let th0 = blob.get(ucode_off..ucode_off.checked_add(th0_len)?)?;
    let th1 = blob.get(ctl_off..ctl_off.checked_add(th1_len)?)?;
    Some((th0, th1))
}

/// log2 of the ring size in DWORDS — the value the `RB_SIZE` field wants.
/// `None` for a non-power-of-two or sub-dword size (never program a bad RB_SIZE).
pub fn ring_size_log2_dwords(ring_bytes: u32) -> Option<u32> {
    if ring_bytes < 4 || !ring_bytes.is_power_of_two() {
        return None;
    }
    Some((ring_bytes / 4).trailing_zeros())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_fill_encoding() {
        let p = constant_fill(0x1234_5678_9abc_def0, 0xCAFE_BABE, 4096);
        assert_eq!(p[0], 0x0000_000B); // op = SDMA_OP_CONST_FILL (11), fillsize 0
        assert_eq!(p[1], 0x9abc_def0); // dst lo
        assert_eq!(p[2], 0x1234_5678); // dst hi
        assert_eq!(p[3], 0xCAFE_BABE); // fill pattern
        assert_eq!(p[4], 4095); // byte_count - 1
        assert_eq!(p.len(), 5);
    }

    #[test]
    fn constant_fill_count_saturates() {
        // A zero byte_count must NOT underflow the COUNT field to 0xFFFFFFFF.
        assert_eq!(constant_fill(0, 0, 0)[4], 0);
    }

    #[test]
    fn fence_encoding() {
        let p = fence(0x8000_0000_0000_1000, 0xABCD);
        // op=FENCE(5) | MTYPE(3)<<16.
        assert_eq!(p[0], 0x0003_0005);
        assert_eq!(p[1], 0x0000_1000); // addr lo
        assert_eq!(p[2], 0x8000_0000); // addr hi
        assert_eq!(p[3], 0xABCD); // seq
        assert_eq!(p.len(), 4);
    }

    #[test]
    fn fill_with_fence_composes() {
        let s = constant_fill_with_fence(0x1000, 0, 256, 0x2000, 1);
        assert_eq!(s.len(), 9); // 5 (fill) + 4 (fence)
        assert_eq!(s[0], SDMA_OP_CONST_FILL); // fill packet first
        assert_eq!(s[5], 0x0003_0005); // fence packet starts at dword 5
        assert_eq!(s[8], 1); // fence seq last
    }

    #[test]
    fn linear_copy_encoding() {
        let p = linear_copy(0x1111_2222_3333_4444, 0xAAAA_BBBB_CCCC_DDDD, 4096);
        assert_eq!(p[0], SDMA_OP_COPY); // op=COPY(1), sub_op=LINEAR(0) => header == 1
        assert_eq!(p[1], 4095); // count = byte_count - 1
        assert_eq!(p[2], 0); // parameters
        assert_eq!(p[3], 0x3333_4444); // src lo
        assert_eq!(p[4], 0x1111_2222); // src hi
        assert_eq!(p[5], 0xCCCC_DDDD); // dst lo
        assert_eq!(p[6], 0xAAAA_BBBB); // dst hi
        assert_eq!(p.len(), 7);
    }

    #[test]
    fn linear_copy_count_saturates() {
        // A zero byte_count must NOT underflow the COUNT field to 0xFFFFFFFF.
        assert_eq!(linear_copy(0, 0, 0)[1], 0);
    }

    #[test]
    fn copy_with_fence_composes() {
        let s = linear_copy_with_fence(0x1000, 0x5000, 256, 0x2000, 7);
        assert_eq!(s.len(), 11); // 7 (copy) + 4 (fence)
        assert_eq!(s[0], SDMA_OP_COPY); // copy packet first
        assert_eq!(s[7], 0x0003_0005); // fence packet starts at dword 7
        assert_eq!(s[10], 7); // fence seq last
    }

    #[test]
    fn ring_size_log2_is_dwords() {
        // 64 KiB ring = 16384 dwords -> log2 = 14 (NOT 16, which is log2 bytes).
        assert_eq!(ring_size_log2_dwords(64 * 1024), Some(14));
        assert_eq!(ring_size_log2_dwords(4096), Some(10)); // 1024 dwords
        assert_eq!(ring_size_log2_dwords(3000), None); // not a power of two
        assert_eq!(ring_size_log2_dwords(0), None);
    }

    #[test]
    fn sdma_ucode_slices_matches_amdgpu_read() {
        // Synthetic sdma_firmware_header_v2_0 with the REAL Athena sdma_6_0_1.bin field
        // values (verified off-target), so the slices match sdma_v6_0_load_microcode.
        let mut b = vec![0u8; 34560];
        let put = |b: &mut [u8], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut b, 24, 256); // ucode_array_offset_bytes
        put(&mut b, 36, 17408); // ctx_jt_offset
        put(&mut b, 40, 4096); // ctx_jt_size
        put(&mut b, 44, 128); // ctl_ucode_offset
        put(&mut b, 52, 16896); // ctl_jt_offset
        put(&mut b, 56, 4096); // ctl_jt_size
        let (th0, th1) = sdma_ucode_slices(&b).expect("valid header");
        assert_eq!(th0.len(), 17408 + 4096, "TH0 = ctx_jt_offset + ctx_jt_size");
        assert_eq!(th1.len(), 16896 + 4096, "TH1 = ctl_jt_offset + ctl_jt_size");
        // Both slices are dword-aligned in length (streamed as u32s).
        assert_eq!(th0.len() % 4, 0);
        assert_eq!(th1.len() % 4, 0);
        // A truncated blob (slice past EOF) returns None, never a panic.
        assert!(sdma_ucode_slices(&b[..1000]).is_none());
    }
}
