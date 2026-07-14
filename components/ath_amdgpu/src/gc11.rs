//! GC 11.0.1 (Phoenix / Radeon 780M) graphics-core register map — the MMIO
//! byte offsets `amdgpud` programs during GFX/CP ring bring-up. Values are the
//! GFX11 `mmCP_*` / `mmGRBM_*` register addresses (the `nbio`/`gc` block bases
//! plus the per-register dword index). Only the registers the ring-init path
//! touches are listed; this is a map, not a clone of the vendor headers.
//!
//! These are byte addresses into the BAR5 register aperture. A register at
//! address `A` is accessed as a 32-bit MMIO read/write at `mmio_base + A`.

/// GRBM status — bit 31 (GUI_ACTIVE) is set while the GPU is busy; the CP
/// init waits for it to clear.
pub const MM_GRBM_STATUS: u32 = 0x0000_8010;
pub const GRBM_STATUS_GUI_ACTIVE: u32 = 1 << 31;

/// Soft-reset register for the GFX blocks (write a mask, poll, clear).
pub const MM_GRBM_SOFT_RESET: u32 = 0x0000_8020;

/// `RLC_SAFE_MODE` command/message bits. The driver writes `CMD | MESSAGE` to
/// ask the RLC to hold the GFX block POWERED + CLOCKED (so CP/GFX registers are
/// writable even when GFXOFF would otherwise gate them —
/// `amdgpu_gfx_rlc_enter_safe_mode`); the RLC clears `CMD` once it has entered,
/// which the driver polls. Exit writes `MESSAGE` alone (CMD=0). These bit
/// positions are stable across gfx generations; the register's ABSOLUTE offset
/// is ASIC-specific and stays iron-pending (`GpuOps::rlc_safe_mode`) — we never
/// write a guessed offset on real hardware.
pub const RLC_SAFE_MODE_CMD: u32 = 1 << 0;
pub const RLC_SAFE_MODE_MESSAGE: u32 = 1 << 1;

/// gfx11 `CP_ME_CNTL` halt bits (from `gc_11_0_0_sh_mask.h`). Unlike the register
/// OFFSET (legacy in `MM_CP_ME_CNTL`, resolved correctly via IP discovery as
/// `GfxRegs::cp_me_cntl`), these bit POSITIONS are architectural constants.
/// `gfx_v11_0_cp_gfx_enable` clears the CP halt bits to release the GFX CP.
pub const CP_ME_CNTL_PFP_HALT: u32 = 0x0400_0000; // bit 26
pub const CP_ME_CNTL_ME_HALT: u32 = 0x1000_0000; // bit 28
/// Bit 24 — set by the RLC autoload (CP_ME_CNTL=0x15000000 = bits 28|26|24) and
/// MUST be cleared too: the LIVE working amdgpu reads CP_ME_CNTL=0x00000000 (all
/// clear) + CP_STAT=0 (idle), but AthenaOS's unhalt cleared only ME|PFP and left
/// bit 24 set (0x01000000) — and the CP then never fetched the ring (CP EXEC
/// RPTR=0). Clearing it makes our value match the working driver exactly (iron
/// umr 2026-06-27). (Nominally CE_HALT, but gfx11's F32 CP keeps this bit
/// meaningful — the autoload sets it and the working driver clears it.)
pub const CP_ME_CNTL_CE_HALT: u32 = 0x0100_0000; // bit 24
/// The exact mask cleared to UNHALT the gfx11 GFX CP so CP_ME_CNTL reaches 0
/// (the working-driver value). Supplied to [`crate::bringup::cp_gfx_enable`] via
/// `GpuOps::cp_me_cntl_halt_mask`.
pub const CP_ME_CNTL_GFX11_HALT_MASK: u32 =
    CP_ME_CNTL_PFP_HALT | CP_ME_CNTL_ME_HALT | CP_ME_CNTL_CE_HALT;

// ── CP (command processor) GFX ring registers ───────────────────────────────
/// Ring buffer base address, low/high 32 bits (the GPU address of the ring).
pub const MM_CP_RB0_BASE: u32 = 0x0000_3040;
pub const MM_CP_RB0_BASE_HI: u32 = 0x0000_307C;
/// Ring buffer control: log2(size) and a few enable/rptr-writeback bits.
pub const MM_CP_RB0_CNTL: u32 = 0x0000_3041;
/// Read/write pointers (dword indices into the ring).
pub const MM_CP_RB0_RPTR: u32 = 0x0000_3060;
pub const MM_CP_RB0_WPTR: u32 = 0x0000_3048;
pub const MM_CP_RB0_WPTR_HI: u32 = 0x0000_3049;
/// Where the CP writes back the read pointer (a host memory address).
pub const MM_CP_RB0_RPTR_ADDR: u32 = 0x0000_3043;
pub const MM_CP_RB0_RPTR_ADDR_HI: u32 = 0x0000_3044;

/// CP master enable / halt register. **LEGACY offset** — same pre-SOC15 GCN trap
/// as the `MM_CP_RB0_*` values: on real gfx11 this addresses the wrong register.
/// The CP enable path resolves `CP_ME_CNTL` via IP discovery instead
/// (`regs::REG_CP_ME_CNTL` → `bringup::GfxRegs::cp_me_cntl`, used by
/// `bringup::cp_gfx_enable`). Kept only for the QEMU/no-discovery reg-probe dump.
pub const MM_CP_ME_CNTL: u32 = 0x0000_303A;
/// **LEGACY, DO NOT USE for gfx11.** The PFP/ME microcode-port load
/// (`CP_*_UCODE_ADDR`/`DATA` write loop) is a GFX6–9 mechanism that does NOT
/// exist on gfx11: gfx11 CP microcode is RS64, loaded via the instruction-cache
/// base registers (`CP_PFP_IC_BASE_*`) on the DIRECT path, or by the PSP on
/// PSP-load ASICs. Phoenix (the Athena 760M) is PSP-load — so the driver never
/// uploads CP ucode; it only enables/resumes the CP (`bringup::cp_gfx_enable`).
/// These constants are retained only as historical map entries.
pub const MM_CP_PFP_UCODE_ADDR: u32 = 0x0000_5814;
pub const MM_CP_PFP_UCODE_DATA: u32 = 0x0000_5815;
pub const MM_CP_ME_RAM_WADDR: u32 = 0x0000_5816;
pub const MM_CP_ME_RAM_DATA: u32 = 0x0000_5817;

/// Compute log2 of a power-of-two ring size in BYTES, the form `CP_RB_CNTL`'s
/// `RB_BUFSZ` field wants (it stores log2(qwords) actually, but the helper here
/// returns log2(bytes) so callers can apply the field shift). Returns `None`
/// for non-power-of-two or sub-dword sizes.
pub fn ring_buf_log2(size_bytes: u32) -> Option<u32> {
    if size_bytes < 4 || !size_bytes.is_power_of_two() {
        return None;
    }
    Some(size_bytes.trailing_zeros())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_register_offsets() {
        // Sanity: the ring base/cntl/ptr registers don't alias each other.
        let regs = [
            MM_CP_RB0_BASE,
            MM_CP_RB0_BASE_HI,
            MM_CP_RB0_CNTL,
            MM_CP_RB0_RPTR,
            MM_CP_RB0_WPTR,
            MM_CP_RB0_RPTR_ADDR,
        ];
        for i in 0..regs.len() {
            for j in (i + 1)..regs.len() {
                assert_ne!(regs[i], regs[j], "register offsets {i},{j} alias");
            }
        }
    }

    #[test]
    fn ring_log2() {
        assert_eq!(ring_buf_log2(4096), Some(12));
        assert_eq!(ring_buf_log2(64 * 1024), Some(16));
        assert_eq!(ring_buf_log2(0), None);
        assert_eq!(ring_buf_log2(3000), None); // not power of two
    }

    #[test]
    fn gui_active_bit() {
        assert_eq!(GRBM_STATUS_GUI_ACTIVE, 0x8000_0000);
    }
}
