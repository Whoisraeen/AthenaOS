//! PM4 (Type-3 / PKT3) command-packet builders for the AMD GFX command
//! processor (CP). These are the exact dword encodings amdgpu writes into a
//! GFX ring; the CP's microcode (ME/PFP/MEC) parses them. Encodings follow
//! `drivers/gpu/drm/amd/include/...` PM4 headers — reproduced here as pure
//! builders so they can be unit-tested on the host.
//!
//! Packet header layout (32-bit, little-endian dword):
//! ```text
//!   bits 31:30  type   (3 = Type-3 / PKT3)
//!   bits 29:16  count  (number of DATA dwords that follow, minus 1)
//!   bits 15:8   opcode (IT_*)
//!   bits 7:0    predicate / shader_type (0 for the cases here)
//! ```

use alloc::vec;
use alloc::vec::Vec;

// ── Packet types ────────────────────────────────────────────────────────────
/// Type-2 NOP filler dword (used to pad a ring to alignment).
pub const PACKET2_NOP: u32 = 0x8000_0000;

// ── IT_* opcodes (a working subset of the GFX9+/GFX11 PM4 set) ───────────────
pub const IT_NOP: u8 = 0x10;
pub const IT_SET_BASE: u8 = 0x11;
pub const IT_DISPATCH_DIRECT: u8 = 0x15;
pub const IT_INDIRECT_BUFFER: u8 = 0x3F;
pub const IT_EVENT_WRITE: u8 = 0x46;
pub const IT_EVENT_WRITE_EOP: u8 = 0x47;
pub const IT_RELEASE_MEM: u8 = 0x49;
pub const IT_WRITE_DATA: u8 = 0x37;
pub const IT_DRAW_INDEX_AUTO: u8 = 0x2D;
pub const IT_SET_CONTEXT_REG: u8 = 0x69;
pub const IT_SET_SH_REG: u8 = 0x76;
pub const IT_SET_UCONFIG_REG: u8 = 0x79;

// Register-window bases: the SET_*_REG packets carry a register offset relative
// to these (offset = (mmREG_addr - BASE) is the dword index written).
pub const PACKET3_SET_CONTEXT_REG_START: u32 = 0x0000_a000;
pub const PACKET3_SET_SH_REG_START: u32 = 0x0000_2c00;
pub const PACKET3_SET_UCONFIG_REG_START: u32 = 0x0000_c000;

// WRITE_DATA control (dword 1) fields.
pub const WR_CONFIRM: u32 = 1 << 20;
pub const WRITE_DATA_DST_SEL_MEM: u32 = 5 << 8; // dst_sel = 5 (TC/L2 memory)
pub const WRITE_DATA_ENGINE_ME: u32 = 0 << 30;
pub const WRITE_DATA_ENGINE_PFP: u32 = 1 << 30;

/// Build a Type-3 packet header. `count` is the number of DATA dwords that
/// follow the header (the on-wire field is `count - 1`).
#[inline]
pub fn pkt3_header(opcode: u8, count: u16) -> u32 {
    debug_assert!(count >= 1, "PKT3 must carry at least one data dword");
    (3u32 << 30) | (((count as u32 - 1) & 0x3fff) << 16) | ((opcode as u32) << 8)
}

/// `IT_NOP` carrying `count` payload dwords (all zero). amdgpu emits a 1-dword
/// NOP (`count = 1`, one zero payload word) as ring padding / a sync point.
pub fn nop(count: u16) -> Vec<u32> {
    let mut p = Vec::with_capacity(1 + count as usize);
    p.push(pkt3_header(IT_NOP, count));
    p.resize(1 + count as usize, 0); // `count` zero payload dwords
    p
}

/// `IT_WRITE_DATA` to a 64-bit GPU memory address. The CP writes `data` dwords
/// to `dst_gpu_addr`; with `WR_CONFIRM` it waits for the write to land. This is
/// the fence-style "CP wrote a known value to memory" primitive.
pub fn write_data_mem(dst_gpu_addr: u64, data: &[u32]) -> Vec<u32> {
    // payload dwords = control(1) + addr_lo(1) + addr_hi(1) + data
    let count = (3 + data.len()) as u16;
    let mut p = Vec::with_capacity(1 + count as usize);
    p.push(pkt3_header(IT_WRITE_DATA, count));
    p.push(WRITE_DATA_ENGINE_ME | WRITE_DATA_DST_SEL_MEM | WR_CONFIRM);
    p.push((dst_gpu_addr & 0xffff_ffff) as u32);
    p.push((dst_gpu_addr >> 32) as u32);
    p.extend_from_slice(data);
    p
}

/// `IT_SET_SH_REG` — write `values` to consecutive SH (shader) registers
/// starting at MMIO byte address `reg`. `reg` must lie in the SH window.
pub fn set_sh_reg(reg: u32, values: &[u32]) -> Vec<u32> {
    let reg_index = (reg - PACKET3_SET_SH_REG_START) >> 2;
    let count = (1 + values.len()) as u16; // reg_index(1) + values
    let mut p = Vec::with_capacity(1 + count as usize);
    p.push(pkt3_header(IT_SET_SH_REG, count));
    p.push(reg_index);
    p.extend_from_slice(values);
    p
}

/// `IT_SET_UCONFIG_REG` — write `values` to consecutive UCONFIG registers
/// starting at MMIO byte address `reg`.
pub fn set_uconfig_reg(reg: u32, values: &[u32]) -> Vec<u32> {
    let reg_index = (reg - PACKET3_SET_UCONFIG_REG_START) >> 2;
    let count = (1 + values.len()) as u16;
    let mut p = Vec::with_capacity(1 + count as usize);
    p.push(pkt3_header(IT_SET_UCONFIG_REG, count));
    p.push(reg_index);
    p.extend_from_slice(values);
    p
}

/// `IT_INDIRECT_BUFFER` — chain into a secondary command buffer at
/// `ib_gpu_addr` of `ib_size_dw` dwords. `vmid` selects the address space.
pub fn indirect_buffer(ib_gpu_addr: u64, ib_size_dw: u32, vmid: u8) -> Vec<u32> {
    vec![
        pkt3_header(IT_INDIRECT_BUFFER, 3),
        (ib_gpu_addr & 0xffff_ffff) as u32,
        (ib_gpu_addr >> 32) as u32,
        // dword 3: [23:0] size in dwords, [31:24] vmid (chain bit etc. left 0).
        (ib_size_dw & 0x00ff_ffff) | ((vmid as u32) << 24),
    ]
}

/// `IT_RELEASE_MEM` — signal an end-of-pipe fence: when the GPU pipeline drains
/// to the chosen event, write `fence_value` to `dst_gpu_addr`. This is how the
/// driver knows a submitted GFX job has completed. `event_type` is e.g.
/// CACHE_FLUSH_AND_INV_TS_EVENT (0x14) on GFX11.
pub fn release_mem(event_type: u8, dst_gpu_addr: u64, fence_value: u64) -> Vec<u32> {
    vec![
        pkt3_header(IT_RELEASE_MEM, 6),
        // dword 1: event_type[5:0], event_index[11:8]=5 (EOP), cache flush bits.
        (event_type as u32 & 0x3f) | (5 << 8),
        // dword 2: dst_sel=mem(2)<<16, int_sel=send-when-confirmed(2)<<24,
        //          data_sel=64-bit(2)<<29.
        (2 << 16) | (2 << 24) | (2 << 29),
        (dst_gpu_addr & 0xffff_ffff) as u32,
        (dst_gpu_addr >> 32) as u32,
        (fence_value & 0xffff_ffff) as u32,
        (fence_value >> 32) as u32,
    ]
}

/// `IT_DISPATCH_DIRECT` — launch a compute grid of `x*y*z` workgroups. The
/// trailing dword is the DISPATCH_INITIATOR (kept caller-supplied so the SH
/// program/resource regs set up beforehand decide the shader).
pub fn dispatch_direct(x: u32, y: u32, z: u32, initiator: u32) -> Vec<u32> {
    vec![pkt3_header(IT_DISPATCH_DIRECT, 4), x, y, z, initiator]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_encoding() {
        // PKT3(NOP, count=1): type=3, count-1=0, op=0x10.
        assert_eq!(pkt3_header(IT_NOP, 1), 0xc000_1000);
        // count=4 -> (4-1)<<16 = 0x3<<16.
        assert_eq!(pkt3_header(IT_DISPATCH_DIRECT, 4), 0xc003_1500);
        // count=14 (0xe-1=0xd) op=WRITE_DATA.
        assert_eq!(pkt3_header(IT_WRITE_DATA, 14), 0xc00d_3700);
    }

    #[test]
    fn nop_shape() {
        let p = nop(1);
        assert_eq!(p, alloc::vec![0xc000_1000, 0]);
    }

    #[test]
    fn write_data_lands_addr_and_payload() {
        let p = write_data_mem(0x1234_5678_9abc_def0, &[0xdead_beef]);
        // header: count = 3 control/addr + 1 data = 4 -> (4-1)<<16, op 0x37.
        assert_eq!(p[0], 0xc003_3700);
        assert_eq!(
            p[1],
            WRITE_DATA_ENGINE_ME | WRITE_DATA_DST_SEL_MEM | WR_CONFIRM
        );
        assert_eq!(p[2], 0x9abc_def0); // addr lo
        assert_eq!(p[3], 0x1234_5678); // addr hi
        assert_eq!(p[4], 0xdead_beef); // data
        assert_eq!(p.len(), 5);
    }

    #[test]
    fn set_sh_reg_index_is_relative() {
        // A register one dword above the SH base -> index 1.
        let reg = PACKET3_SET_SH_REG_START + 4;
        let p = set_sh_reg(reg, &[0x55]);
        assert_eq!(p[0], pkt3_header(IT_SET_SH_REG, 2));
        assert_eq!(p[1], 1);
        assert_eq!(p[2], 0x55);
    }

    #[test]
    fn indirect_buffer_packs_size_and_vmid() {
        let p = indirect_buffer(0xAABB_CCDD_1122_3344, 0x40, 7);
        assert_eq!(p[0], pkt3_header(IT_INDIRECT_BUFFER, 3));
        assert_eq!(p[1], 0x1122_3344);
        assert_eq!(p[2], 0xAABB_CCDD);
        assert_eq!(p[3], 0x40 | (7 << 24));
    }

    #[test]
    fn release_mem_writes_64bit_fence() {
        let p = release_mem(0x14, 0x8000_0000_0000_1000, 0x1);
        assert_eq!(p[0], pkt3_header(IT_RELEASE_MEM, 6));
        assert_eq!(p[1] & 0x3f, 0x14);
        assert_eq!(p[3], 0x0000_1000);
        assert_eq!(p[4], 0x8000_0000);
        assert_eq!(p[5], 1);
        assert_eq!(p[6], 0);
        assert_eq!(p.len(), 7);
    }

    #[test]
    fn dispatch_direct_grid() {
        let p = dispatch_direct(64, 1, 1, 0x1);
        assert_eq!(&p[1..], &[64, 1, 1, 0x1]);
    }
}
