//! Authoritative gfx11 (GC 11.0.x) + SMU 13.0.4 (Phoenix APU) SOC15 register map
//! and offset resolution.
//!
//! **SOC15 addressing.** A register's absolute MMIO byte offset is
//! `(ip_base(HWID, inst, BASE_IDX) + reg_dword) << 2`, where `ip_base` comes
//! from the IP discovery table ([`crate::discovery`]) — the per-IP base is
//! published by the hardware, not a constant. The `reg_dword` + `BASE_IDX`
//! values below are taken verbatim from the upstream register headers (cited
//! per constant). The HWIDs are from `soc15_hw_ip.h`. This is exactly the
//! resolution `amdgpu` does (`RREG32_SOC15`), so it is authoritative for the
//! Athena Radeon 760M (gfx 11.0.1 / SMU 13.0.4) once the bases are read off the
//! hardware's discovery blob.
//!
//! **Likely stage-6 root cause (flagged, not yet fixed here).** `gc11.rs` holds
//! hardcoded CP/GRBM *byte* offsets — `MM_GRBM_STATUS = 0x8010`,
//! `MM_CP_RB0_BASE = 0x3040` — that do NOT match the gfx11 SOC15 offsets
//! (`regGRBM_STATUS = 0x0da4`, `regCP_RB0_BASE = 0x1de0`); they match the
//! *legacy* pre-SOC15 GCN layout. On real gfx11 those legacy offsets address the
//! wrong registers, which is consistent with the CP-ring readback MISMATCH seen
//! on the Athena (the failure attributed to GFXOFF re-gating may be — or also
//! be — wrong offsets). The SOC15-correct CP/GRBM offsets are provided here so
//! the daemon can adopt them; rewiring stage 6 to use them is a deliberate
//! follow-up (gc11.rs is load-bearing and separately owned).

use crate::bringup::{
    CpGfxRingRegs, GfxRegs, GmcVmRegs, IhRing, PspRegs, RlcSafeMode, Rs64CpRegs, SdmaRegs,
    SmuMailbox,
};
use crate::discovery::{ip_base, IpBlock};
use crate::gart::GfxhubGartRegs;

// ── HWIDs (drivers/gpu/drm/amd/include/soc15_hw_ip.h) ────────────────────────
pub const GC_HWID: u16 = 11;
pub const MP1_HWID: u16 = 1;
pub const MP0_HWID: u16 = 255;
pub const OSSSYS_HWID: u16 = 40;
pub const MMHUB_HWID: u16 = 34;
pub const NBIF_HWID: u16 = 108;
/// Display Micro-controller Unit — the DCN/display block (soc15_hw_ip.h). This is a
/// SEPARATE power domain from GC: it is already lit by the firmware (it drives the GOP
/// boot framebuffer) even when the GFX block is power-gated, so the display/scanout path
/// is reachable on a WARM boot — unlike the GFX/CP games path which needs a cold GPU.
pub const DMU_HWID: u16 = 271;

// ── gfx11 GC register dword offsets + BASE_IDX (gc/gc_11_0_0_offset.h) ────────
pub const REG_GRBM_STATUS: (u32, usize) = (0x0da4, 0);
pub const REG_CP_RB0_BASE: (u32, usize) = (0x1de0, 0);
pub const REG_CP_RB0_BASE_HI: (u32, usize) = (0x1e51, 0);
pub const REG_CP_RB0_CNTL: (u32, usize) = (0x1de1, 0);
pub const REG_CP_RB0_RPTR: (u32, usize) = (0x0f60, 0);
pub const REG_CP_RB0_WPTR: (u32, usize) = (0x1df4, 0);
pub const REG_CP_ME_CNTL: (u32, usize) = (0x0803, 1);
pub const REG_RLC_SAFE_MODE: (u32, usize) = (0x0980, 1);

// Authoritative "is GFX up?" status (the stage-6 fork-resolver) + the PSP-load
// rlc_resume SRM enable. gc_11_0_1 (Phoenix) offsets, GC SEGMENT 1.
//   RLC_RLCS_BOOTLOAD_STATUS — PSP RLC-autoload completion (regRLC_RLCS_BOOTLOAD_STATUS_gc_11_0_1)
//   GFX_IMU_GFX_RESET_CTRL   — IMU "GFX out of reset" status (imu_v11_0_wait_for_reset_status)
//   RLC_SRM_CNTL             — RLC Save/Restore Machine enable (gfx_v11_0_rlc_enable_srm)
pub const REG_RLC_RLCS_BOOTLOAD_STATUS: (u32, usize) = (0x4e7e, 1);
pub const REG_GFX_IMU_GFX_RESET_CTRL: (u32, usize) = (0x40bc, 1);
pub const REG_RLC_SRM_CNTL: (u32, usize) = (0x4c80, 1);
//   RLC_PG_CNTL — RLC GFX power-gating enables. amdgpu clears this to 0 EARLY in
//   bring-up (Athena resume trace: 0xec43 <- 0x0) so the RLC hardware cannot gate
//   GFX; the SMU DisallowGfxOff alone does NOT hold. Offset verified via the umr db
//   (gc_11_0_0.reg: regRLC_PG_CNTL 0 0x4c43 ... 1; bit0=GFX_POWER_GATING_ENABLE).
pub const REG_RLC_PG_CNTL: (u32, usize) = (0x4c43, 1);
// RLC_CSIB_ADDR_LO/HI + LENGTH — the RLC Clear-State Buffer descriptor (init_csb).
// amdgpu writes these RIGHT BEFORE the MES enable (oracle trace 2026-06-27:
// ADDR_LO=0x1ec000 HI=0x80 LENGTH=0x3c0); AthenaOS skipped it, and the MES stalls one
// instruction into boot waiting for the RLC. GC seg1 0x0987/0x0988/0x0989.
pub const REG_RLC_CSIB_ADDR_LO: (u32, usize) = (0x0987, 1);
pub const REG_RLC_CSIB_ADDR_HI: (u32, usize) = (0x0988, 1);
pub const REG_RLC_CSIB_LENGTH: (u32, usize) = (0x0989, 1);
// GFX_IMU_CORE_CTRL — the IMU-core start register. `imu_v11_0_start` releases the
// IMU core by clearing bit0 (CRESET); the IMU then brings GFX out of reset. The
// DOWN-branch wake when the PSP did not start GFX (ucode is already PSP-loaded, so
// this is just the "release core" step, not a ucode reload). GC seg1.
pub const REG_GFX_IMU_CORE_CTRL: (u32, usize) = (0x40b6, 1);
// IMU I-RAM / D-RAM windows (imu_v11_0_load_microcode): write ADDR=0, stream the
// ucode dwords through DATA (auto-incrementing), then ADDR=fw_version. The
// DIRECT-load step that puts the IMU ucode into IMU SRAM (the PSP leaves GFX cold
// on Phoenix). gc_11_0_0_offset.h, GC seg1.
pub const REG_GFX_IMU_I_RAM_ADDR: (u32, usize) = (0x5f90, 1);
pub const REG_GFX_IMU_I_RAM_DATA: (u32, usize) = (0x5f91, 1);
pub const REG_GFX_IMU_D_RAM_ADDR: (u32, usize) = (0x40fc, 1);
pub const REG_GFX_IMU_D_RAM_DATA: (u32, usize) = (0x40fd, 1);
// RLC autoload bootloader regs (rlc_backdoor_autoload_enable): point the IMU at
// the RLC_G ucode inside the autoload buffer. GC seg1.
pub const REG_GFX_IMU_RLC_BOOTLOADER_ADDR_LO: (u32, usize) = (0x5f82, 1);
pub const REG_GFX_IMU_RLC_BOOTLOADER_ADDR_HI: (u32, usize) = (0x5f81, 1);
pub const REG_GFX_IMU_RLC_BOOTLOADER_SIZE: (u32, usize) = (0x5f83, 1);
// setup_imu (imu_v11_0_setup): enable IMU debug access + disable Rtavfs/etc.
pub const REG_GFX_IMU_ACCESS_CTRL0: (u32, usize) = (0x4040, 1);
pub const REG_GFX_IMU_ACCESS_CTRL1: (u32, usize) = (0x4041, 1);
pub const REG_GFX_IMU_SCRATCH_10: (u32, usize) = (0x4072, 1);

// CP gfx-ring completion regs (gc_11_0_0_offset.h) — the rest of cp_gfx_resume
// beyond BASE/CNTL/WPTR. All GC seg0. CP_RB_ACTIVE=1 ACTIVATES the ring.
const REG_CP_RB_ACTIVE: (u32, usize) = (0x1f40, 0);
const REG_CP_RB_VMID: (u32, usize) = (0x1df1, 0);
const REG_CP_RB0_RPTR_ADDR: (u32, usize) = (0x1de3, 0);
const REG_CP_RB0_RPTR_ADDR_HI: (u32, usize) = (0x1de4, 0);
const REG_CP_RB_WPTR_POLL_ADDR_LO: (u32, usize) = (0x1e8b, 0);
const REG_CP_RB_WPTR_POLL_ADDR_HI: (u32, usize) = (0x1e8c, 0);
// CP_RB_DOORBELL_CONTROL — gfx11 CP gfx ring wakes on a DOORBELL, not a bare
// WPTR-register write. Offset 0x1e8d derived from the live umr (CP_RB_VMID umr
// 0x03051 = our 0x1df1 + const 0x1260; CP_RB_DOORBELL_CONTROL umr 0x030ed -
// 0x1260 = 0x1e8d). `gfx_v11_0_cp_gfx_set_doorbell` sets DOORBELL_EN (bit 30) +
// DOORBELL_OFFSET (the ring's doorbell index, GFX_RING0 = 0).
const REG_CP_RB_DOORBELL_CONTROL: (u32, usize) = (0x1e8d, 0);
// CP_RB_DOORBELL_RANGE_LOWER/UPPER — the doorbell-aperture byte range the CP
// listens to. Without it the doorbell write is NOT routed to the CP, so the ring
// never wakes. amdgpu: LOWER=(gfx_ring0*2)<<2=0, UPPER=(userqueue_end*2)<<2.
// Offsets 0x1dfa/0x1dfb (umr 0x0305a/0x0305b - the 0x1260 const).
const REG_CP_RB_DOORBELL_RANGE_LOWER: (u32, usize) = (0x1dfa, 0);
const REG_CP_RB_DOORBELL_RANGE_UPPER: (u32, usize) = (0x1dfb, 0);
// CP_MEC_DOORBELL_RANGE_LOWER/UPPER — the COMPUTE/MES-class doorbell byte range the
// MEC/MES monitors to WAKE on a doorbell ring (distinct from the gfx CP_RB range above).
// The MES SCHED (byte 0x58) + KIQ (byte 0x60) doorbells live here. Iron 2026-06-28: the
// working amdgpu sets this to [0x0, 0x450]; AthenaOS omitted it entirely, so the KIQ
// doorbell HIT latched in the HQD but the MES microengine was NEVER woken (KIQ rptr=0,
// SCHED ring never mapped). Offsets 0x1dfc/0x1dfd (umr 0x0305c/0x0305d - the 0x1260 const).
const REG_CP_MEC_DOORBELL_RANGE_LOWER: (u32, usize) = (0x1dfc, 0);
const REG_CP_MEC_DOORBELL_RANGE_UPPER: (u32, usize) = (0x1dfd, 0);
const REG_CP_MAX_CONTEXT: (u32, usize) = (0x1e4e, 0);
const REG_CP_DEVICE_ID: (u32, usize) = (0x1deb, 0);

// RS64 CP startup regs (gfx_v11_0_config_gfx_rs64). GRBM_GFX_CNTL + MEC regs are
// GC seg1; the PFP/ME program-counter starts are seg0 (per gc_11_0_0_offset.h).
const REG_GRBM_GFX_CNTL: (u32, usize) = (0x0900, 1);
const REG_CP_PFP_PRGRM_CNTR_START: (u32, usize) = (0x1e44, 0);
const REG_CP_PFP_PRGRM_CNTR_START_HI: (u32, usize) = (0x1e59, 0);
const REG_CP_ME_PRGRM_CNTR_START: (u32, usize) = (0x1e45, 0);
const REG_CP_ME_PRGRM_CNTR_START_HI: (u32, usize) = (0x1e79, 0);
const REG_CP_MEC_RS64_PRGRM_CNTR_START: (u32, usize) = (0x2900, 1);
const REG_CP_MEC_RS64_PRGRM_CNTR_START_HI: (u32, usize) = (0x2938, 1);
const REG_CP_MEC_RS64_CNTL: (u32, usize) = (0x2904, 1);

// MES engine-enable regs (mes_v11_0_enable). All GC seg1 (per gc_11_0_0_offset.h).
const REG_CP_MES_CNTL: (u32, usize) = (0x2807, 1);
const REG_CP_MES_PRGRM_CNTR_START: (u32, usize) = (0x2800, 1);
const REG_CP_MES_PRGRM_CNTR_START_HI: (u32, usize) = (0x289d, 1);
const REG_CP_MES_GP3_LO: (u32, usize) = (0x2849, 1); // MES fw-version heartbeat
                                                     // MES instruction/data-cache base regs (mes_v11_0_load_microcode). All GC seg1.
const REG_CP_MES_IC_OP_CNTL: (u32, usize) = (0x2820, 1);
// CP_HQD regs (mes_v11_0_queue_init_register). All GC seg0.
const REG_CP_HQD_VMID: (u32, usize) = (0x1fac, 0);
const REG_CP_MQD_BASE_ADDR: (u32, usize) = (0x1fa9, 0);
const REG_CP_MQD_BASE_ADDR_HI: (u32, usize) = (0x1faa, 0);
const REG_CP_MQD_CONTROL: (u32, usize) = (0x1fcb, 0);
const REG_CP_HQD_PQ_BASE: (u32, usize) = (0x1fb1, 0);
const REG_CP_HQD_PQ_BASE_HI: (u32, usize) = (0x1fb2, 0);
const REG_CP_HQD_PQ_RPTR_REPORT_ADDR: (u32, usize) = (0x1fb4, 0);
const REG_CP_HQD_PQ_RPTR_REPORT_ADDR_HI: (u32, usize) = (0x1fb5, 0);
const REG_CP_HQD_PQ_WPTR_POLL_ADDR: (u32, usize) = (0x1fb6, 0);
const REG_CP_HQD_PQ_WPTR_POLL_ADDR_HI: (u32, usize) = (0x1fb7, 0);
const REG_CP_HQD_PQ_DOORBELL_CONTROL: (u32, usize) = (0x1fb8, 0);
const REG_CP_HQD_PQ_CONTROL: (u32, usize) = (0x1fba, 0);
const REG_CP_HQD_PERSISTENT_STATE: (u32, usize) = (0x1fad, 0);
const REG_CP_HQD_ACTIVE: (u32, usize) = (0x1fab, 0);
// mes_v11_0_kiq_setting ("tell RLC which is KIQ queue"). SEGMENT 1 — gc_11_0_0_offset.h
// regRLC_CP_SCHEDULERS_BASE_IDX=1; it sits with the CSIB regs in the seg-1 RLC bank. This
// was (0x098a, 0) until 2026-07-01: every kiq_setting write landed at GC[0]+0x98a (a dead
// address) so the REAL register never learned scheduler0/KIQ was active — and the RLC,
// which clock-manages the CP/MES pipes, saw no live scheduler. The working driver's cold
// trace (docs/gpu-oracle/cold_init_named-20260624.txt) shows it RMW 0x3038 -> 0x30e8
// ((me=3<<5)|(pipe=1<<3)|queue=0 | 0x80 enable) right after CP_MES_CNTL activate.
const REG_RLC_CP_SCHEDULERS: (u32, usize) = (0x098a, 1);
const REG_CP_MES_IC_BASE_LO: (u32, usize) = (0x5850, 1);
const REG_CP_MES_IC_BASE_HI: (u32, usize) = (0x5851, 1);
const REG_CP_MES_IC_BASE_CNTL: (u32, usize) = (0x5852, 1);
const REG_CP_MES_MDBASE_LO: (u32, usize) = (0x5854, 1);
const REG_CP_MES_MDBASE_HI: (u32, usize) = (0x5855, 1);
const REG_CP_MES_MIBOUND_LO: (u32, usize) = (0x585b, 1);
const REG_CP_MES_MDBOUND_LO: (u32, usize) = (0x585d, 1);

// gfxhub GMC/GPUVM state regs (read-only diagnostic, gc_11_0_0_offset.h). All GC seg0.
const REG_GCMC_VM_FB_LOCATION_BASE: (u32, usize) = (0x1678, 0);
const REG_GCMC_VM_FB_LOCATION_TOP: (u32, usize) = (0x1679, 0);
const REG_GCMC_VM_AGP_BASE: (u32, usize) = (0x167c, 0);
const REG_GCMC_VM_SYSTEM_APERTURE_LOW_ADDR: (u32, usize) = (0x167d, 0);
const REG_GCMC_VM_SYSTEM_APERTURE_HIGH_ADDR: (u32, usize) = (0x167e, 0);
const REG_GCMC_VM_MX_L1_TLB_CNTL: (u32, usize) = (0x167f, 0);
const REG_GCVM_CONTEXT0_CNTL: (u32, usize) = (0x1688, 0);
const REG_GCVM_CONTEXT0_PTB_LO32: (u32, usize) = (0x16f3, 0);
const REG_GCVM_CONTEXT0_PTB_HI32: (u32, usize) = (0x16f4, 0);

// gfxhub GART-build regs (gfxhub_v3_0_gart_enable, gc_11_0_0_offset.h). All GC seg0.
const REG_GCVM_L2_CNTL: (u32, usize) = (0x15bc, 0);
const REG_GCVM_L2_CNTL2: (u32, usize) = (0x15bd, 0);
const REG_GCVM_L2_CNTL3: (u32, usize) = (0x15be, 0);
const REG_GCMC_VM_AGP_TOP: (u32, usize) = (0x167a, 0);
const REG_GCMC_VM_AGP_BOT: (u32, usize) = (0x167b, 0);
const REG_GCMC_VM_SYS_DEFAULT_LSB: (u32, usize) = (0x15a8, 0);
const REG_GCMC_VM_SYS_DEFAULT_MSB: (u32, usize) = (0x15a9, 0);
const REG_GCVM_CONTEXT0_START_LO32: (u32, usize) = (0x1713, 0);
const REG_GCVM_CONTEXT0_START_HI32: (u32, usize) = (0x1714, 0);
const REG_GCVM_CONTEXT0_END_LO32: (u32, usize) = (0x1733, 0);
const REG_GCVM_CONTEXT0_END_HI32: (u32, usize) = (0x1734, 0);
const REG_GCVM_INVALIDATE_ENG0_REQ: (u32, usize) = (0x16ab, 0);
const REG_GCVM_INVALIDATE_ENG0_ACK: (u32, usize) = (0x16bd, 0);

// ── OSSSYS / IH 6.0 (oss/osssys_6_0_0_offset.h) ──────────────────────────────
const REG_IH_RB_BASE: (u32, usize) = (0x0081, 0);
const REG_IH_RB_BASE_HI: (u32, usize) = (0x0082, 0);
const REG_IH_RB_RPTR: (u32, usize) = (0x0083, 0);
const REG_IH_RB_WPTR: (u32, usize) = (0x0084, 0);

// ── MP1 SMU mailbox C2PMSG (mp/mp_13_0_0_offset.h) ───────────────────────────
// BASE_IDX 1: the `mmMP1_SMN_C2PMSG_*` registers live in MP1 SEGMENT 1, not 0 —
// `smu_v13_0_4_ppt.c` defines `mmMP1_SMN_C2PMSG_66_BASE_IDX 1` and resolves them
// with `SOC15_REG_OFFSET(MP1, 0, ...)`. Using seg 0 (Athena MP1 base[0]=0x4000)
// resolved 0x10a08 — a writable-but-WRONG register: on iron the PMFW was silent
// (msg read back our 0x2 but response never posted). Seg 1 (Athena MP1
// base[1]=0x16000) is the real SMN mailbox aperture.
const REG_MP1_C2PMSG_66: (u32, usize) = (0x0282, 1); // message
const REG_MP1_C2PMSG_82: (u32, usize) = (0x0292, 1); // argument
const REG_MP1_C2PMSG_90: (u32, usize) = (0x029a, 1); // response

// ── MP0 PSP mailbox C2PMSG (mp/mp_13_0_4_offset.h) — psp_v13_0 ────────────────
// BASE_IDX **1** (regMP0_SMN_C2PMSG_*_BASE_IDX 1 in mp_13_0_4 — Athena's PSP IP
// version, which is what umr loads: `mp_13_0_4.reg (13.0.4) for mp0`). The PSP
// mailbox lives in MP0 SEGMENT 1 (Athena MP0 base[1]=0x16000 → C2PMSG_81 @ byte
// 0x58244), the SAME segment as the SMU/MP1 mailbox.
//
// NOTE: the *generic* mp_13_0_0 / mp_13_0_6 headers say BASE_IDX 0 (seg 0,
// base[0]=0x4000 → byte 0x10244) — that earlier value pointed at the DEAD seg-0
// mirror, so the sign-of-life read always returned 0 on iron even though the Athena
// Linux oracle (`umr -r *.*.regMP0_SMN_C2PMSG_81`) reads it LIVE + ticking. This is
// VERSION-SPECIFIC: 13.0.4 = seg 1. Confirmed three ways — umr .reg DB seg field,
// mp_13_0_4_offset.h BASE_IDX, and MP1 already working at base_idx 1.
//   C2PMSG_81 = sOS sign-of-life (psp_v13_0_is_sos_alive: !=0 => secure OS up)
//   C2PMSG_58 = sOS fw version    C2PMSG_35 = bootloader status/cmd (bit31=ready)
//   C2PMSG_33 = VMBX ready (bit31) C2PMSG_36 = bootloader address arg
//   C2PMSG_64 = ring cmd          C2PMSG_69/70/71 = ring base lo/hi/size
const REG_MP0_C2PMSG_33: (u32, usize) = (0x0061, 1);
const REG_MP0_C2PMSG_35: (u32, usize) = (0x0063, 1);
const REG_MP0_C2PMSG_36: (u32, usize) = (0x0064, 1);
const REG_MP0_C2PMSG_58: (u32, usize) = (0x007a, 1);
const REG_MP0_C2PMSG_64: (u32, usize) = (0x0080, 1);
const REG_MP0_C2PMSG_67: (u32, usize) = (0x0083, 1); // ring write pointer (dwords)
const REG_MP0_C2PMSG_69: (u32, usize) = (0x0085, 1);
const REG_MP0_C2PMSG_70: (u32, usize) = (0x0086, 1);
const REG_MP0_C2PMSG_71: (u32, usize) = (0x0087, 1);
const REG_MP0_C2PMSG_81: (u32, usize) = (0x0091, 1);

// ── NBIF CONFIG_MEMSIZE (nbio/nbio_4_3_0_offset.h) ───────────────────────────
const REG_CONFIG_MEMSIZE: (u32, usize) = (0x00c3, 2); // regRCC_DEV0_EPF0_RCC_CONFIG_MEMSIZE
                                                      // regRCC_DEV0_EPF0_RCC_DOORBELL_APER_EN (same NBIO block, seg 2): BIF_DOORBELL_APER_EN
                                                      // = bit 0. `nbio_v4_3_enable_doorbell_aperture` sets it so BAR2 doorbell writes route
                                                      // to the engines — the GOP firmware sets up only display, so this is likely off, and
                                                      // without it NO doorbell reaches the MES or the gfx CP.
const REG_RCC_DOORBELL_APER_EN: (u32, usize) = (0x00c0, 2);

/// Resolve `regRCC_DEV0_EPF0_RCC_DOORBELL_APER_EN` (NBIO seg 2) for the doorbell-
/// aperture enable. `None` until discovery resolves the NBIF block.
pub fn rcc_doorbell_aper_en(blocks: &[IpBlock]) -> Option<u32> {
    resolve(blocks, NBIF_HWID, REG_RCC_DOORBELL_APER_EN)
}

/// Resolve `regCP_MEC_DOORBELL_RANGE_LOWER`/`UPPER` (GC seg0) — the compute/MES-class
/// doorbell range the MEC/MES monitors so a doorbell ring WAKES the microengine. Must be
/// programmed before the KIQ doorbell is rung, else the HQD latches the HIT but the MES
/// never services its ring (the gate that left KIQ rptr=0). Returns `(lower, upper)`
/// resolved offsets; `None` until discovery resolves the GC block.
pub fn cp_mec_doorbell_range(blocks: &[IpBlock]) -> Option<(u32, u32)> {
    Some((
        resolve(blocks, GC_HWID, REG_CP_MEC_DOORBELL_RANGE_LOWER)?,
        resolve(blocks, GC_HWID, REG_CP_MEC_DOORBELL_RANGE_UPPER)?,
    ))
}

// ── DCN 3.1.4 display scanout (HUBP0/OTG0, dcn_3_1_4_offset.h) ────────────────
// All in the DMU block, seg 2 (BASE_IDX=2, header-verified). The DCN is firmware-lit
// for the GOP framebuffer, so these read LIVE on a warm boot — the foundation of the
// native display path: read the firmware's scanout state now, later flip the HUBP
// primary surface address to point the panel at an amdgpu-controlled buffer.
const REG_HUBP0_PRIMARY_SURFACE_ADDRESS: (u32, usize) = (0x060a, 2);
const REG_HUBP0_PRIMARY_SURFACE_ADDRESS_HIGH: (u32, usize) = (0x060b, 2);
const REG_HUBP0_SURFACE_CONFIG: (u32, usize) = (0x05e5, 2);
const REG_OTG0_H_TOTAL: (u32, usize) = (0x1b2a, 2);
// DCSURF_SURFACE_INUSE — the surface address the DCN is ACTIVELY scanning out right now
// (vs PRIMARY_SURFACE_ADDRESS, the requested address that latches on vblank). Reading
// this back == the address we wrote is the hardware's own confirmation that the flip
// took and the panel is displaying our buffer — no camera/eyes needed.
const REG_HUBP0_SURFACE_INUSE: (u32, usize) = (0x0621, 2);
const REG_HUBP0_SURFACE_INUSE_HIGH: (u32, usize) = (0x0622, 2);

/// Resolved DCN scanout register byte-offsets (HUBP0 primary surface + OTG0 timing).
pub struct DcnScanout {
    pub primary_surface_addr_lo: u32,
    pub primary_surface_addr_hi: u32,
    pub surface_config: u32,
    pub otg_h_total: u32,
    /// The address the DCN is ACTIVELY scanning (DCSURF_SURFACE_INUSE). Reading this back
    /// == the written primary surface proves the flip latched and the panel shows our buffer.
    pub surface_inuse_lo: u32,
    pub surface_inuse_hi: u32,
}

/// Resolve the DCN HUBP0/OTG0 scanout registers from the DMU block. `None` until
/// discovery resolves DMU (so QEMU — no real DCN — never reads a fabricated offset).
pub fn dcn_scanout(blocks: &[IpBlock]) -> Option<DcnScanout> {
    Some(DcnScanout {
        primary_surface_addr_lo: resolve(blocks, DMU_HWID, REG_HUBP0_PRIMARY_SURFACE_ADDRESS)?,
        primary_surface_addr_hi: resolve(blocks, DMU_HWID, REG_HUBP0_PRIMARY_SURFACE_ADDRESS_HIGH)?,
        surface_config: resolve(blocks, DMU_HWID, REG_HUBP0_SURFACE_CONFIG)?,
        otg_h_total: resolve(blocks, DMU_HWID, REG_OTG0_H_TOTAL)?,
        surface_inuse_lo: resolve(blocks, DMU_HWID, REG_HUBP0_SURFACE_INUSE)?,
        surface_inuse_hi: resolve(blocks, DMU_HWID, REG_HUBP0_SURFACE_INUSE_HIGH)?,
    })
}

// ── SDMA0 QUEUE0 ring (gc/gc_11_0_0_offset.h) ────────────────────────────────
// On gfx11 the SDMA registers live in the GC IP block (sdma_v6_0.c includes
// gc_11_0_0_offset.h; `sdma_v6_0_get_reg_offset` resolves via reg_offset[GC_HWIP]).
// The RB queue registers are OUTSIDE the HYP_DEC range [0x5880,0x589a], so they
// take the seg-0 base — `BASE_IDX 0`, exactly like the CP RB registers.
const REG_SDMA0_QUEUE0_RB_CNTL: (u32, usize) = (0x0080, 0);
const REG_SDMA0_QUEUE0_RB_BASE: (u32, usize) = (0x0081, 0);
const REG_SDMA0_QUEUE0_RB_BASE_HI: (u32, usize) = (0x0082, 0);
const REG_SDMA0_QUEUE0_RB_RPTR: (u32, usize) = (0x0083, 0);
const REG_SDMA0_QUEUE0_RB_RPTR_HI: (u32, usize) = (0x0084, 0);
const REG_SDMA0_QUEUE0_RB_WPTR: (u32, usize) = (0x0085, 0);
const REG_SDMA0_QUEUE0_RB_WPTR_HI: (u32, usize) = (0x0086, 0);
// The F32 firmware polls the WPTR from THIS memory address (not the RB_WPTR
// register) when RB_CNTL.F32_WPTR_POLL_ENABLE is set — the submit path the live
// Athena amdgpu uses (umr: RB_CNTL=0x841817 has F32_WPTR_POLL_ENABLE=1; the
// RB_WPTR register stays 0). BASE_IDX 0, same seg as the ring regs.
const REG_SDMA0_QUEUE0_RB_WPTR_POLL_ADDR_HI: (u32, usize) = (0x00b2, 0);
const REG_SDMA0_QUEUE0_RB_WPTR_POLL_ADDR_LO: (u32, usize) = (0x00b3, 0);
// Doorbell: SDMA0_QUEUE0_DOORBELL.ENABLE (bit 28) routes the queue to the doorbell
// aperture; DOORBELL_OFFSET is the byte offset of this queue's doorbell within that
// BAR. The live Athena amdgpu sets DOORBELL=0x10000000 + DOORBELL_OFFSET=0x800 and
// WAKES the engine via a 64-bit write to doorbell-BAR(0xdc000000)+0x800 (cold trace
// map 113). F32_WPTR_POLL + WPTR register alone did NOT wake it (boot 170257
// RB_RPTR=0). BASE_IDX 0. umr db: DOORBELL 0x92, DOORBELL_OFFSET 0xab.
const REG_SDMA0_QUEUE0_DOORBELL: (u32, usize) = (0x0092, 0);
const REG_SDMA0_QUEUE0_DOORBELL_OFFSET: (u32, usize) = (0x00ab, 0);
// SDMA0_F32_CNTL — the engine halt/enable register. UNLIKE the QUEUE0 ring regs
// (BASE_IDX 0), this lives in GC SEGMENT 1 (BASE_IDX 1) at dword 0x589a — verified
// against the Athena umr db `gc_11_0_0.reg` (`regSDMA0_F32_CNTL 0 0x589a 9 0 1`),
// calibrated against RB_CNTL(0x80,0) and GFX_IMU_CORE_CTRL(0x40b6,1).
const REG_SDMA0_F32_CNTL: (u32, usize) = (0x589a, 1);
// SDMA0_UTCL1_CNTL — the engine's UTC L1 address-translation-cache control. UNLIKE
// F32_CNTL (seg1), this is a QUEUE-side register at GC SEG0 dword 0x3c (umr db
// `regSDMA0_UTCL1_CNTL 0 0x3c`), same seg as the ring regs. sdma_v6_0_gfx_resume
// programs RESP_MODE=3 + REDO_DELAY=9 here so the engine resolves its ring/WPTR/
// fence GPU addresses through VMID0 (else it cannot fetch and RB_RPTR stays 0).
const REG_SDMA0_UTCL1_CNTL: (u32, usize) = (0x003c, 0);
// SDMA RS64 broadcast microcode-load window (GC SEG1, like F32_CNTL). The PSP on
// this Phoenix REJECTS SDMA via the gfx LOAD_IP_FW path (0xffff0010), so AthenaOS
// DIRECT-loads the dual-thread RS64 ucode itself (sdma_v6_0_load_microcode): write
// ADDR then stream the ucode dwords to DATA (auto-increment). Broadcast = all SDMA
// instances at once. umr db: BROADCAST_UCODE_ADDR 0x5886, DATA 0x5887, base_idx 1.
const REG_SDMA0_BROADCAST_UCODE_ADDR: (u32, usize) = (0x5886, 1);
const REG_SDMA0_BROADCAST_UCODE_DATA: (u32, usize) = (0x5887, 1);

/// `RLC_RLCS_BOOTLOAD_STATUS.BOOTLOAD_COMPLETE` (bit 31): the PSP finished the RLC
/// autoload — GFX firmware (IMU/RLC/CP ucode) is loaded. Required before the driver's
/// rlc_resume/cp_resume on a PSP-load ASIC (gfx_v11_0_wait_for_rlc_autoload_complete).
pub const RLC_BOOTLOAD_COMPLETE_MASK: u32 = 0x8000_0000;
/// `GFX_IMU_GFX_RESET_CTRL` low-5-bits all set (`& 0x1f == 0x1f`): the IMU released
/// GFX from reset (imu_v11_0_wait_for_reset_status). With these bits clear, GFX is held
/// in reset and every CP/GFX register write is silently DROPPED — the stage-6
/// readback-0 root cause we are chasing on iron.
pub const GFX_IMU_GFX_RESET_DONE_MASK: u32 = 0x1f;
/// `RLC_SRM_CNTL` SRM_ENABLE | AUTO_INCR_ADDR — the PSP-load rlc_resume enable_srm step.
pub const RLC_SRM_CNTL_ENABLE: u32 = 0x1;
pub const RLC_SRM_CNTL_AUTO_INCR_ADDR: u32 = 0x2;
/// `GFX_IMU_CORE_CTRL.CRESET` (bit 0). `imu_v11_0_start` clears it (`val &= 0xfffffffe`)
/// to release the IMU core from reset so it brings GFX up. Clearing it when the core
/// is already running is a harmless no-op.
pub const GFX_IMU_CORE_CTRL_CRESET: u32 = 0x1;

/// True when the two authoritative gfx11 status registers say GFX is up: the PSP RLC
/// autoload completed (`RLC_RLCS_BOOTLOAD_STATUS.BOOTLOAD_COMPLETE`) AND the IMU has
/// released GFX from reset (`GFX_IMU_GFX_RESET_CTRL & 0x1f == 0x1f`). When this is
/// false, CP/GFX register writes are silently dropped — the stage-6 readback-0 cause.
/// The decision behind stage 6's "GFX is UP/DOWN" verdict, factored out so it is
/// host-KAT-able with no hardware.
pub fn gfx_is_up(bootload_status: u32, gfx_imu_reset_ctrl: u32) -> bool {
    bootload_status & RLC_BOOTLOAD_COMPLETE_MASK != 0
        && gfx_imu_reset_ctrl & GFX_IMU_GFX_RESET_DONE_MASK == GFX_IMU_GFX_RESET_DONE_MASK
}

/// Phoenix (Radeon 760M, 1002:15bf) SMU mailbox — PRE-DISCOVERY bootstrap copies of
/// the discovery-resolved offsets (MP1 seg1 base 0x16000 + C2PMSG_66/82/90, <<2 to
/// bytes). Iron-verified identical to the resolve() output on every Athena boot
/// ("stage 5 SMU mailbox @ msg=0x58a08 arg=0x58a48 resp=0x58a68") and to the live
/// working driver's mailbox in the cold mmiotrace (phys 0xdc558a08). Used ONLY for
/// the probe-time GFXOFF hold, before the discovery blob is readable — reading the
/// discovery blob itself is a VRAM access that can race a PMFW power transition.
pub const PHOENIX_SMU_MB_MSG: u32 = 0x58a08;
pub const PHOENIX_SMU_MB_ARG: u32 = 0x58a48;
pub const PHOENIX_SMU_MB_RESP: u32 = 0x58a68;

/// Phoenix APU PMFW (SMU 13.0.4) message ids
/// (pm/swsmu/inc/pmfw_if/smu_v13_0_4_ppsmc.h). NOTE the APU set differs from the
/// dGPU set — DisallowGfxOff is 0x1A here, NOT the dGPU 0x29.
pub const PPSMC_MSG_DISALLOW_GFXOFF: u32 = 0x1A;
pub const PPSMC_MSG_ALLOW_GFXOFF: u32 = 0x19;
/// `PPSMC_MSG_EnableGfxImu` — tells the PMFW to POWER UP the GFX block via the IMU
/// (`smu_v13_0_set_gfx_power_up_by_imu`, the PSP-load path). The arg is
/// `ENABLE_IMU_ARG_GFXOFF_ENABLE = 1`. DisallowGfxOff only PREVENTS gating; this is
/// what actively wakes a GFX block the firmware power-gated (boot 183253: GFX
/// GATED, ring writes dropped). Verified from smu_v13_0_4_ppsmc.h.
pub const PPSMC_MSG_ENABLE_GFX_IMU: u32 = 0x16;
pub const ENABLE_IMU_ARG_GFXOFF_ENABLE: u32 = 1;
/// `PPSMC_MSG_GfxDeviceDriverReset` — the SMU-driven GFX/compute/SDMA scrub amdgpu
/// issues (`smu_v13_0_4_mode2_reset`) when it loads onto a "dirty" GPU: a GFX block
/// left in a previous-boot state that a cold power-up can't clear. A PCI FLR does NOT
/// clear the APU (only a full power-off or this SMU reset does), so this is the only
/// software path that scrubs a warm GFX domain. Verified from smu_v13_0_4_ppsmc.h
/// (the dGPU set numbers this differently — APU-specific). Arg = the reset mode.
pub const PPSMC_MSG_GFX_DEVICE_DRIVER_RESET: u32 = 0x11;
/// Reset-mode arg for `GfxDeviceDriverReset` (amdgpu `SMU_RESET_MODE_2` = 2): the
/// MODE2 reset scrubs GFX/compute/SDMA without a full chip-off, the mode amdgpu uses
/// for APU GFX recovery.
pub const SMU_RESET_MODE_2: u32 = 2;
/// gfxclk DPM setters (arg = freq in MHz), from the REAL smu_v13_0_4_ppsmc.h (vendored
/// linux-7.0.12) cross-confirmed against the cold mmiotrace's numeric mailbox writes
/// (docs/gpu-oracle/cold_init_named-20260624.txt). The 2026-06-27 kprobe capture only saw
/// the abstract driver enums and the transcription GUESSED 0x1B/0x1C as soft-min/soft-max
/// — wrong: 0x1B is SetSoftMaxGfxClk and 0x1C is SetHardMinGfxClk (real soft-min is 0x09),
/// so the old "pin to 800" sends were actually max-clamps.
pub const PPSMC_MSG_SET_SOFT_MIN_GFXCLK: u32 = 0x09;
pub const PPSMC_MSG_SET_SOFT_MAX_GFXCLK: u32 = 0x1B;
pub const PPSMC_MSG_SET_HARD_MIN_GFXCLK: u32 = 0x1C;
/// SMU driver/metrics DRAM table setup (smu_v13_0_4). amdgpu's cold-init trace sends the
/// trio as 0x0D/0x0E/0x0F (SetDriverDramAddrHigh/Low, TransferTableSmu2Dram — observed
/// live: `msg=0xd arg=0x80`, `msg=0xe arg=0x1ed000`, `msg=0xf arg=0x4`). These were
/// transcribed off-by-one as 0x0C/0x0D/0x0E until 2026-07-01 — and 0x0C on this PMFW is
/// PPSMC_MSG_PrepareMp1ForUnload ("prepare PMFW for GFX driver unload"!), which the
/// gate-rerun-GFX-REGATE-20260629 boot ACKED right before GFX re-gated. Never send it.
pub const PPSMC_MSG_PREPARE_MP1_FOR_UNLOAD: u32 = 0x0C;
pub const PPSMC_MSG_SET_DRIVER_DRAM_ADDR_HIGH: u32 = 0x0D;
pub const PPSMC_MSG_SET_DRIVER_DRAM_ADDR_LOW: u32 = 0x0E;
pub const PPSMC_MSG_TRANSFER_TABLE_SMU2DRAM: u32 = 0x0F;

/// SOC15 absolute MMIO byte offset = `(base_dword + reg_dword) << 2`.
#[inline]
fn soc15(base_dword: u32, reg: (u32, usize)) -> u32 {
    (base_dword.wrapping_add(reg.0)) << 2
}

/// Resolve `reg` against the IP block `hw_id`/instance-0 discovered bases.
#[inline]
fn resolve(blocks: &[IpBlock], hw_id: u16, reg: (u32, usize)) -> Option<u32> {
    ip_base(blocks, hw_id, 0, reg.1).map(|base| soc15(base, reg))
}

/// `RLC_SAFE_MODE` absolute offset for [`crate::bringup::GpuOps::rlc_safe_mode`].
pub fn rlc_safe_mode(blocks: &[IpBlock]) -> Option<RlcSafeMode> {
    resolve(blocks, GC_HWID, REG_RLC_SAFE_MODE).map(|reg| RlcSafeMode { reg })
}

/// IH ring register offsets for [`crate::bringup::GpuOps::ih_ring`].
pub fn ih_ring(blocks: &[IpBlock]) -> Option<IhRing> {
    Some(IhRing {
        rb_base: resolve(blocks, OSSSYS_HWID, REG_IH_RB_BASE)?,
        rb_base_hi: resolve(blocks, OSSSYS_HWID, REG_IH_RB_BASE_HI)?,
        rb_rptr: resolve(blocks, OSSSYS_HWID, REG_IH_RB_RPTR)?,
        rb_wptr: resolve(blocks, OSSSYS_HWID, REG_IH_RB_WPTR)?,
    })
}

/// SMU mailbox register offsets for [`crate::bringup::GpuOps::smu_mailbox`].
pub fn smu_mailbox(blocks: &[IpBlock]) -> Option<SmuMailbox> {
    Some(SmuMailbox {
        msg_reg: resolve(blocks, MP1_HWID, REG_MP1_C2PMSG_66)?,
        arg_reg: resolve(blocks, MP1_HWID, REG_MP1_C2PMSG_82)?,
        resp_reg: resolve(blocks, MP1_HWID, REG_MP1_C2PMSG_90)?,
    })
}

/// PSP (MP0) mailbox + ring register offsets for [`crate::bringup::GpuOps::psp_regs`].
/// The PSP firmware-load path (psp_v13_0) is the only way to cold-start GFX on this
/// PSP-load APU (boot 041507 proved no host/SMU power-up exists). `None` if the MP0
/// block isn't in the discovery table.
pub fn psp_regs(blocks: &[IpBlock]) -> Option<PspRegs> {
    Some(PspRegs {
        sol: resolve(blocks, MP0_HWID, REG_MP0_C2PMSG_81)?,
        sos_fw_version: resolve(blocks, MP0_HWID, REG_MP0_C2PMSG_58)?,
        bl_status: resolve(blocks, MP0_HWID, REG_MP0_C2PMSG_35)?,
        vmbx_status: resolve(blocks, MP0_HWID, REG_MP0_C2PMSG_33)?,
        bl_arg: resolve(blocks, MP0_HWID, REG_MP0_C2PMSG_36)?,
        ring_cmd: resolve(blocks, MP0_HWID, REG_MP0_C2PMSG_64)?,
        ring_lo: resolve(blocks, MP0_HWID, REG_MP0_C2PMSG_69)?,
        ring_hi: resolve(blocks, MP0_HWID, REG_MP0_C2PMSG_70)?,
        ring_size: resolve(blocks, MP0_HWID, REG_MP0_C2PMSG_71)?,
        ring_wptr: resolve(blocks, MP0_HWID, REG_MP0_C2PMSG_67)?,
    })
}

/// `CONFIG_MEMSIZE` register offset for [`crate::bringup::GpuOps::config_memsize`].
pub fn config_memsize_reg(blocks: &[IpBlock]) -> Option<u32> {
    resolve(blocks, NBIF_HWID, REG_CONFIG_MEMSIZE)
}

/// SOC15-correct gfx11 CP/GRBM offsets for [`crate::bringup::GpuOps::gfx_regs`]
/// (the values stage 6 uses instead of gc11's legacy ones). `None` if the GC
/// block isn't in the discovery table.
pub fn gfx_regs(blocks: &[IpBlock]) -> Option<GfxRegs> {
    Some(GfxRegs {
        grbm_status: resolve(blocks, GC_HWID, REG_GRBM_STATUS)?,
        cp_rb0_base: resolve(blocks, GC_HWID, REG_CP_RB0_BASE)?,
        cp_rb0_base_hi: resolve(blocks, GC_HWID, REG_CP_RB0_BASE_HI)?,
        cp_rb0_cntl: resolve(blocks, GC_HWID, REG_CP_RB0_CNTL)?,
        cp_rb0_rptr: resolve(blocks, GC_HWID, REG_CP_RB0_RPTR)?,
        cp_rb0_wptr: resolve(blocks, GC_HWID, REG_CP_RB0_WPTR)?,
        cp_me_cntl: resolve(blocks, GC_HWID, REG_CP_ME_CNTL)?,
        rlc_bootload_status: resolve(blocks, GC_HWID, REG_RLC_RLCS_BOOTLOAD_STATUS)?,
        gfx_imu_reset_ctrl: resolve(blocks, GC_HWID, REG_GFX_IMU_GFX_RESET_CTRL)?,
        gfx_imu_core_ctrl: resolve(blocks, GC_HWID, REG_GFX_IMU_CORE_CTRL)?,
        gfx_imu_i_ram_addr: resolve(blocks, GC_HWID, REG_GFX_IMU_I_RAM_ADDR)?,
        gfx_imu_i_ram_data: resolve(blocks, GC_HWID, REG_GFX_IMU_I_RAM_DATA)?,
        gfx_imu_d_ram_addr: resolve(blocks, GC_HWID, REG_GFX_IMU_D_RAM_ADDR)?,
        gfx_imu_d_ram_data: resolve(blocks, GC_HWID, REG_GFX_IMU_D_RAM_DATA)?,
        rlc_bootloader_addr_lo: resolve(blocks, GC_HWID, REG_GFX_IMU_RLC_BOOTLOADER_ADDR_LO)?,
        rlc_bootloader_addr_hi: resolve(blocks, GC_HWID, REG_GFX_IMU_RLC_BOOTLOADER_ADDR_HI)?,
        rlc_bootloader_size: resolve(blocks, GC_HWID, REG_GFX_IMU_RLC_BOOTLOADER_SIZE)?,
        imu_access_ctrl0: resolve(blocks, GC_HWID, REG_GFX_IMU_ACCESS_CTRL0)?,
        imu_access_ctrl1: resolve(blocks, GC_HWID, REG_GFX_IMU_ACCESS_CTRL1)?,
        imu_scratch_10: resolve(blocks, GC_HWID, REG_GFX_IMU_SCRATCH_10)?,
        rlc_srm_cntl: resolve(blocks, GC_HWID, REG_RLC_SRM_CNTL)?,
        rlc_pg_cntl: resolve(blocks, GC_HWID, REG_RLC_PG_CNTL)?,
        rlc_csib_addr_lo: resolve(blocks, GC_HWID, REG_RLC_CSIB_ADDR_LO)?,
        rlc_csib_addr_hi: resolve(blocks, GC_HWID, REG_RLC_CSIB_ADDR_HI)?,
        rlc_csib_length: resolve(blocks, GC_HWID, REG_RLC_CSIB_LENGTH)?,
    })
}

/// gfxhub GPUVM state register offsets for [`crate::bringup::GpuOps::gmc_vm_regs`]
/// (read-only GART-inheritance diagnostic). GC block. `None` if GC isn't present.
pub fn gmc_vm_regs(blocks: &[IpBlock]) -> Option<GmcVmRegs> {
    Some(GmcVmRegs {
        fb_location_base: resolve(blocks, GC_HWID, REG_GCMC_VM_FB_LOCATION_BASE)?,
        fb_location_top: resolve(blocks, GC_HWID, REG_GCMC_VM_FB_LOCATION_TOP)?,
        agp_base: resolve(blocks, GC_HWID, REG_GCMC_VM_AGP_BASE)?,
        sys_aperture_low: resolve(blocks, GC_HWID, REG_GCMC_VM_SYSTEM_APERTURE_LOW_ADDR)?,
        sys_aperture_high: resolve(blocks, GC_HWID, REG_GCMC_VM_SYSTEM_APERTURE_HIGH_ADDR)?,
        mx_l1_tlb_cntl: resolve(blocks, GC_HWID, REG_GCMC_VM_MX_L1_TLB_CNTL)?,
        context0_cntl: resolve(blocks, GC_HWID, REG_GCVM_CONTEXT0_CNTL)?,
        context0_ptb_lo32: resolve(blocks, GC_HWID, REG_GCVM_CONTEXT0_PTB_LO32)?,
        context0_ptb_hi32: resolve(blocks, GC_HWID, REG_GCVM_CONTEXT0_PTB_HI32)?,
    })
}

/// gfxhub GART-build register offsets for [`crate::gart::build_gart_enable_sequence`].
/// GC block. `None` if GC isn't in the discovery table.
pub fn gfxhub_gart_regs(blocks: &[IpBlock]) -> Option<GfxhubGartRegs> {
    Some(GfxhubGartRegs {
        l2_cntl: resolve(blocks, GC_HWID, REG_GCVM_L2_CNTL)?,
        l2_cntl2: resolve(blocks, GC_HWID, REG_GCVM_L2_CNTL2)?,
        l2_cntl3: resolve(blocks, GC_HWID, REG_GCVM_L2_CNTL3)?,
        fb_location_base: resolve(blocks, GC_HWID, REG_GCMC_VM_FB_LOCATION_BASE)?,
        fb_location_top: resolve(blocks, GC_HWID, REG_GCMC_VM_FB_LOCATION_TOP)?,
        agp_base: resolve(blocks, GC_HWID, REG_GCMC_VM_AGP_BASE)?,
        agp_bot: resolve(blocks, GC_HWID, REG_GCMC_VM_AGP_BOT)?,
        agp_top: resolve(blocks, GC_HWID, REG_GCMC_VM_AGP_TOP)?,
        sys_aperture_low: resolve(blocks, GC_HWID, REG_GCMC_VM_SYSTEM_APERTURE_LOW_ADDR)?,
        sys_aperture_high: resolve(blocks, GC_HWID, REG_GCMC_VM_SYSTEM_APERTURE_HIGH_ADDR)?,
        sys_default_lsb: resolve(blocks, GC_HWID, REG_GCMC_VM_SYS_DEFAULT_LSB)?,
        sys_default_msb: resolve(blocks, GC_HWID, REG_GCMC_VM_SYS_DEFAULT_MSB)?,
        mx_l1_tlb_cntl: resolve(blocks, GC_HWID, REG_GCMC_VM_MX_L1_TLB_CNTL)?,
        context0_cntl: resolve(blocks, GC_HWID, REG_GCVM_CONTEXT0_CNTL)?,
        context0_ptb_lo32: resolve(blocks, GC_HWID, REG_GCVM_CONTEXT0_PTB_LO32)?,
        context0_ptb_hi32: resolve(blocks, GC_HWID, REG_GCVM_CONTEXT0_PTB_HI32)?,
        context0_start_lo32: resolve(blocks, GC_HWID, REG_GCVM_CONTEXT0_START_LO32)?,
        context0_start_hi32: resolve(blocks, GC_HWID, REG_GCVM_CONTEXT0_START_HI32)?,
        context0_end_lo32: resolve(blocks, GC_HWID, REG_GCVM_CONTEXT0_END_LO32)?,
        context0_end_hi32: resolve(blocks, GC_HWID, REG_GCVM_CONTEXT0_END_HI32)?,
        invalidate_eng0_req: resolve(blocks, GC_HWID, REG_GCVM_INVALIDATE_ENG0_REQ)?,
        invalidate_eng0_ack: resolve(blocks, GC_HWID, REG_GCVM_INVALIDATE_ENG0_ACK)?,
    })
}

/// RS64 CP startup register offsets for [`crate::bringup::GpuOps::rs64_cp_regs`]
/// (`gfx_v11_0_config_gfx_rs64`). GC block. `None` if GC isn't in discovery.
pub fn rs64_cp_regs(blocks: &[IpBlock]) -> Option<Rs64CpRegs> {
    Some(Rs64CpRegs {
        grbm_gfx_cntl: resolve(blocks, GC_HWID, REG_GRBM_GFX_CNTL)?,
        me_cntl: resolve(blocks, GC_HWID, REG_CP_ME_CNTL)?,
        pfp_start: resolve(blocks, GC_HWID, REG_CP_PFP_PRGRM_CNTR_START)?,
        pfp_start_hi: resolve(blocks, GC_HWID, REG_CP_PFP_PRGRM_CNTR_START_HI)?,
        me_start: resolve(blocks, GC_HWID, REG_CP_ME_PRGRM_CNTR_START)?,
        me_start_hi: resolve(blocks, GC_HWID, REG_CP_ME_PRGRM_CNTR_START_HI)?,
        mec_start: resolve(blocks, GC_HWID, REG_CP_MEC_RS64_PRGRM_CNTR_START)?,
        mec_start_hi: resolve(blocks, GC_HWID, REG_CP_MEC_RS64_PRGRM_CNTR_START_HI)?,
        mec_cntl: resolve(blocks, GC_HWID, REG_CP_MEC_RS64_CNTL)?,
    })
}

/// MES engine-enable register offsets for [`crate::bringup::GpuOps::mes_enable_regs`]
/// (`mes_v11_0_enable`). GC block. `None` if GC isn't in discovery.
pub fn mes_enable_regs(blocks: &[IpBlock]) -> Option<crate::mes::MesEnableRegs> {
    Some(crate::mes::MesEnableRegs {
        grbm_gfx_cntl: resolve(blocks, GC_HWID, REG_GRBM_GFX_CNTL)?,
        cp_mes_cntl: resolve(blocks, GC_HWID, REG_CP_MES_CNTL)?,
        prgrm_cntr_start: resolve(blocks, GC_HWID, REG_CP_MES_PRGRM_CNTR_START)?,
        prgrm_cntr_start_hi: resolve(blocks, GC_HWID, REG_CP_MES_PRGRM_CNTR_START_HI)?,
        cp_mes_gp3_lo: resolve(blocks, GC_HWID, REG_CP_MES_GP3_LO)?,
    })
}

/// MES instruction/data-cache base register offsets for
/// [`crate::bringup::GpuOps::mes_load_regs`] (`mes_v11_0_load_microcode`). GC block.
pub fn mes_load_regs(blocks: &[IpBlock]) -> Option<crate::mes::MesLoadRegs> {
    Some(crate::mes::MesLoadRegs {
        grbm_gfx_cntl: resolve(blocks, GC_HWID, REG_GRBM_GFX_CNTL)?,
        prgrm_cntr_start: resolve(blocks, GC_HWID, REG_CP_MES_PRGRM_CNTR_START)?,
        prgrm_cntr_start_hi: resolve(blocks, GC_HWID, REG_CP_MES_PRGRM_CNTR_START_HI)?,
        ic_base_lo: resolve(blocks, GC_HWID, REG_CP_MES_IC_BASE_LO)?,
        ic_base_hi: resolve(blocks, GC_HWID, REG_CP_MES_IC_BASE_HI)?,
        ic_base_cntl: resolve(blocks, GC_HWID, REG_CP_MES_IC_BASE_CNTL)?,
        mibound_lo: resolve(blocks, GC_HWID, REG_CP_MES_MIBOUND_LO)?,
        mdbase_lo: resolve(blocks, GC_HWID, REG_CP_MES_MDBASE_LO)?,
        mdbase_hi: resolve(blocks, GC_HWID, REG_CP_MES_MDBASE_HI)?,
        mdbound_lo: resolve(blocks, GC_HWID, REG_CP_MES_MDBOUND_LO)?,
        ic_op_cntl: resolve(blocks, GC_HWID, REG_CP_MES_IC_OP_CNTL)?,
    })
}

/// CP_HQD register offsets for [`crate::bringup::GpuOps::mes_hqd_regs`]
/// (`mes_v11_0_queue_init_register`). GC seg0.
pub fn mes_hqd_regs(blocks: &[IpBlock]) -> Option<crate::mes::MesHqdRegs> {
    Some(crate::mes::MesHqdRegs {
        grbm_gfx_cntl: resolve(blocks, GC_HWID, REG_GRBM_GFX_CNTL)?,
        cp_hqd_vmid: resolve(blocks, GC_HWID, REG_CP_HQD_VMID)?,
        cp_mqd_base_addr: resolve(blocks, GC_HWID, REG_CP_MQD_BASE_ADDR)?,
        cp_mqd_base_addr_hi: resolve(blocks, GC_HWID, REG_CP_MQD_BASE_ADDR_HI)?,
        cp_mqd_control: resolve(blocks, GC_HWID, REG_CP_MQD_CONTROL)?,
        cp_hqd_pq_base: resolve(blocks, GC_HWID, REG_CP_HQD_PQ_BASE)?,
        cp_hqd_pq_base_hi: resolve(blocks, GC_HWID, REG_CP_HQD_PQ_BASE_HI)?,
        cp_hqd_pq_rptr_report_addr: resolve(blocks, GC_HWID, REG_CP_HQD_PQ_RPTR_REPORT_ADDR)?,
        cp_hqd_pq_rptr_report_addr_hi: resolve(blocks, GC_HWID, REG_CP_HQD_PQ_RPTR_REPORT_ADDR_HI)?,
        cp_hqd_pq_wptr_poll_addr: resolve(blocks, GC_HWID, REG_CP_HQD_PQ_WPTR_POLL_ADDR)?,
        cp_hqd_pq_wptr_poll_addr_hi: resolve(blocks, GC_HWID, REG_CP_HQD_PQ_WPTR_POLL_ADDR_HI)?,
        cp_hqd_pq_doorbell_control: resolve(blocks, GC_HWID, REG_CP_HQD_PQ_DOORBELL_CONTROL)?,
        cp_hqd_pq_control: resolve(blocks, GC_HWID, REG_CP_HQD_PQ_CONTROL)?,
        cp_hqd_persistent_state: resolve(blocks, GC_HWID, REG_CP_HQD_PERSISTENT_STATE)?,
        cp_hqd_active: resolve(blocks, GC_HWID, REG_CP_HQD_ACTIVE)?,
        rlc_cp_schedulers: resolve(blocks, GC_HWID, REG_RLC_CP_SCHEDULERS)?,
    })
}

/// CP gfx-ring completion registers for [`crate::bringup::GpuOps::cp_gfx_ring_regs`]
/// — the rest of `gfx_v11_0_cp_gfx_resume` (incl. `CP_RB_ACTIVE`). GC block.
pub fn cp_gfx_ring_regs(blocks: &[IpBlock]) -> Option<CpGfxRingRegs> {
    Some(CpGfxRingRegs {
        rb_active: resolve(blocks, GC_HWID, REG_CP_RB_ACTIVE)?,
        rb_vmid: resolve(blocks, GC_HWID, REG_CP_RB_VMID)?,
        rb0_rptr_addr: resolve(blocks, GC_HWID, REG_CP_RB0_RPTR_ADDR)?,
        rb0_rptr_addr_hi: resolve(blocks, GC_HWID, REG_CP_RB0_RPTR_ADDR_HI)?,
        rb_wptr_poll_addr_lo: resolve(blocks, GC_HWID, REG_CP_RB_WPTR_POLL_ADDR_LO)?,
        rb_wptr_poll_addr_hi: resolve(blocks, GC_HWID, REG_CP_RB_WPTR_POLL_ADDR_HI)?,
        doorbell_control: resolve(blocks, GC_HWID, REG_CP_RB_DOORBELL_CONTROL)?,
        doorbell_range_lower: resolve(blocks, GC_HWID, REG_CP_RB_DOORBELL_RANGE_LOWER)?,
        doorbell_range_upper: resolve(blocks, GC_HWID, REG_CP_RB_DOORBELL_RANGE_UPPER)?,
        max_context: resolve(blocks, GC_HWID, REG_CP_MAX_CONTEXT)?,
        device_id: resolve(blocks, GC_HWID, REG_CP_DEVICE_ID)?,
    })
}

/// SDMA0 QUEUE0 ring register offsets for [`crate::bringup::GpuOps::sdma_regs`]
/// (used by [`crate::bringup::program_sdma_ring`]). Resolved via the GC block —
/// the SDMA regs are part of GC IP on gfx11 (`BASE_IDX 0`). `None` if the GC
/// block isn't in the discovery table.
pub fn sdma_regs(blocks: &[IpBlock]) -> Option<SdmaRegs> {
    Some(SdmaRegs {
        rb_cntl: resolve(blocks, GC_HWID, REG_SDMA0_QUEUE0_RB_CNTL)?,
        rb_base: resolve(blocks, GC_HWID, REG_SDMA0_QUEUE0_RB_BASE)?,
        rb_base_hi: resolve(blocks, GC_HWID, REG_SDMA0_QUEUE0_RB_BASE_HI)?,
        rb_rptr: resolve(blocks, GC_HWID, REG_SDMA0_QUEUE0_RB_RPTR)?,
        rb_rptr_hi: resolve(blocks, GC_HWID, REG_SDMA0_QUEUE0_RB_RPTR_HI)?,
        rb_wptr: resolve(blocks, GC_HWID, REG_SDMA0_QUEUE0_RB_WPTR)?,
        rb_wptr_hi: resolve(blocks, GC_HWID, REG_SDMA0_QUEUE0_RB_WPTR_HI)?,
        rb_wptr_poll_addr_hi: resolve(blocks, GC_HWID, REG_SDMA0_QUEUE0_RB_WPTR_POLL_ADDR_HI)?,
        rb_wptr_poll_addr_lo: resolve(blocks, GC_HWID, REG_SDMA0_QUEUE0_RB_WPTR_POLL_ADDR_LO)?,
        doorbell: resolve(blocks, GC_HWID, REG_SDMA0_QUEUE0_DOORBELL)?,
        doorbell_offset: resolve(blocks, GC_HWID, REG_SDMA0_QUEUE0_DOORBELL_OFFSET)?,
        broadcast_ucode_addr: resolve(blocks, GC_HWID, REG_SDMA0_BROADCAST_UCODE_ADDR)?,
        broadcast_ucode_data: resolve(blocks, GC_HWID, REG_SDMA0_BROADCAST_UCODE_DATA)?,
        f32_cntl: resolve(blocks, GC_HWID, REG_SDMA0_F32_CNTL)?,
        utcl1_cntl: resolve(blocks, GC_HWID, REG_SDMA0_UTCL1_CNTL)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn blocks() -> alloc::vec::Vec<IpBlock> {
        vec![
            // GC: seg0 base 0x1000, seg1 base 0x2000
            IpBlock {
                hw_id: GC_HWID,
                instance: 0,
                bases: vec![0x1000, 0x2000],
            },
            // MP1: seg0 base 0x3000, seg1 base 0x3800 (the C2PMSG mailbox aperture)
            IpBlock {
                hw_id: MP1_HWID,
                instance: 0,
                bases: vec![0x3000, 0x3800],
            },
            // OSSSYS: seg0 base 0x4000
            IpBlock {
                hw_id: OSSSYS_HWID,
                instance: 0,
                bases: vec![0x4000],
            },
            // NBIF: seg0..seg2, seg2 base 0x5000
            IpBlock {
                hw_id: NBIF_HWID,
                instance: 0,
                bases: vec![0, 0, 0x5000],
            },
            // MP0 (PSP): the PSP C2PMSG mailbox is seg 1 (mp_13_0_4 BASE_IDX=1),
            // base[1]=0x6800; base[0]=0x6000 is the dead seg-0 mirror.
            IpBlock {
                hw_id: MP0_HWID,
                instance: 0,
                bases: vec![0x6000, 0x6800],
            },
        ]
    }

    #[test]
    fn soc15_resolution() {
        let b = blocks();
        // RLC_SAFE_MODE: GC seg1 -> (0x2000 + 0x980) << 2
        assert_eq!(rlc_safe_mode(&b).unwrap().reg, (0x2000 + 0x980) << 2);
        // SMU mailbox: MP1 SEG1 (C2PMSG_*_BASE_IDX = 1), not seg0
        let smu = smu_mailbox(&b).unwrap();
        assert_eq!(smu.msg_reg, (0x3800 + 0x282) << 2);
        assert_eq!(smu.arg_reg, (0x3800 + 0x292) << 2);
        assert_eq!(smu.resp_reg, (0x3800 + 0x29a) << 2);
        // PSP mailbox: MP0 SEG1 (C2PMSG_*_BASE_IDX = 1 in mp_13_0_4 — Athena's PSP
        // version) — sign-of-life + ring regs. Seg 0 (0x6000) was the dead mirror.
        let psp = psp_regs(&b).unwrap();
        assert_eq!(psp.sol, (0x6800 + 0x91) << 2, "C2PMSG_81 sign-of-life");
        assert_eq!(psp.sos_fw_version, (0x6800 + 0x7a) << 2);
        assert_eq!(psp.bl_status, (0x6800 + 0x63) << 2);
        assert_eq!(psp.vmbx_status, (0x6800 + 0x61) << 2);
        assert_eq!(psp.ring_cmd, (0x6800 + 0x80) << 2);
        assert_eq!(psp.ring_lo, (0x6800 + 0x85) << 2);
        assert_eq!(psp.ring_size, (0x6800 + 0x87) << 2);
        assert_eq!(psp.ring_wptr, (0x6800 + 0x83) << 2, "C2PMSG_67 ring wptr");
        // IH ring: OSSSYS seg0
        let ih = ih_ring(&b).unwrap();
        assert_eq!(ih.rb_base, (0x4000 + 0x81) << 2);
        assert_eq!(ih.rb_wptr, (0x4000 + 0x84) << 2);
        // CONFIG_MEMSIZE: NBIF seg2
        assert_eq!(config_memsize_reg(&b).unwrap(), (0x5000 + 0xc3) << 2);
        // GFX regs: GC seg0 (all CP/GRBM regs are base_idx 0)
        let g = gfx_regs(&b).unwrap();
        assert_eq!(g.grbm_status, (0x1000 + 0xda4) << 2);
        assert_eq!(g.cp_rb0_base, (0x1000 + 0x1de0) << 2);
        assert_eq!(g.cp_rb0_base_hi, (0x1000 + 0x1e51) << 2);
        assert_eq!(g.cp_rb0_cntl, (0x1000 + 0x1de1) << 2);
        assert_eq!(g.cp_rb0_rptr, (0x1000 + 0xf60) << 2);
        assert_eq!(g.cp_rb0_wptr, (0x1000 + 0x1df4) << 2);
        // CP_ME_CNTL is base_idx 1 (GC seg1 = 0x2000), unlike the RB0 regs (seg0).
        assert_eq!(g.cp_me_cntl, (0x2000 + 0x803) << 2);
        // SDMA0 QUEUE0 ring regs: GC seg0 (base_idx 0), like the CP RB regs.
        let s = sdma_regs(&b).unwrap();
        assert_eq!(s.rb_cntl, (0x1000 + 0x80) << 2);
        assert_eq!(s.rb_base, (0x1000 + 0x81) << 2);
        assert_eq!(s.rb_base_hi, (0x1000 + 0x82) << 2);
        assert_eq!(s.rb_wptr, (0x1000 + 0x85) << 2);
        // CP gfx-ring completion regs: GC seg0 (base_idx 0), incl. CP_RB_ACTIVE.
        let cpr = cp_gfx_ring_regs(&b).unwrap();
        assert_eq!(cpr.rb_active, (0x1000 + 0x1f40) << 2);
        assert_eq!(cpr.rb_vmid, (0x1000 + 0x1df1) << 2);
        assert_eq!(cpr.rb0_rptr_addr, (0x1000 + 0x1de3) << 2);
        assert_eq!(cpr.rb_wptr_poll_addr_lo, (0x1000 + 0x1e8b) << 2);
        // RS64 CP startup: PFP/ME starts are GC seg0; GRBM_GFX_CNTL + MEC are seg1.
        let rs = rs64_cp_regs(&b).unwrap();
        assert_eq!(rs.pfp_start, (0x1000 + 0x1e44) << 2);
        assert_eq!(rs.me_start, (0x1000 + 0x1e45) << 2);
        assert_eq!(rs.grbm_gfx_cntl, (0x2000 + 0x900) << 2);
        assert_eq!(rs.mec_start, (0x2000 + 0x2900) << 2);
        assert_eq!(rs.mec_cntl, (0x2000 + 0x2904) << 2);
        // gfxhub VM-state diagnostic regs: all GC seg0.
        let vm = gmc_vm_regs(&b).unwrap();
        assert_eq!(vm.sys_aperture_low, (0x1000 + 0x167d) << 2);
        assert_eq!(vm.context0_cntl, (0x1000 + 0x1688) << 2);
        assert_eq!(vm.context0_ptb_lo32, (0x1000 + 0x16f3) << 2);
        // GFX-up status regs + IMU core + SRM enable: all GC seg1 (base_idx 1 = 0x2000).
        assert_eq!(g.rlc_bootload_status, (0x2000 + 0x4e7e) << 2);
        assert_eq!(g.gfx_imu_reset_ctrl, (0x2000 + 0x40bc) << 2);
        assert_eq!(g.gfx_imu_core_ctrl, (0x2000 + 0x40b6) << 2);
        assert_eq!(g.gfx_imu_i_ram_addr, (0x2000 + 0x5f90) << 2);
        assert_eq!(g.gfx_imu_i_ram_data, (0x2000 + 0x5f91) << 2);
        assert_eq!(g.gfx_imu_d_ram_addr, (0x2000 + 0x40fc) << 2);
        assert_eq!(g.gfx_imu_d_ram_data, (0x2000 + 0x40fd) << 2);
        assert_eq!(g.rlc_bootloader_addr_lo, (0x2000 + 0x5f82) << 2);
        assert_eq!(g.imu_access_ctrl0, (0x2000 + 0x4040) << 2);
        assert_eq!(g.imu_scratch_10, (0x2000 + 0x4072) << 2);
        assert_eq!(g.rlc_srm_cntl, (0x2000 + 0x4c80) << 2);
        assert_eq!(g.rlc_pg_cntl, (0x2000 + 0x4c43) << 2);
    }

    #[test]
    fn gfx_up_verdict() {
        // Both signals set -> GFX up. BOOTLOAD_COMPLETE is bit 31; reset-done is the
        // low 5 bits all set. Mixed/partial values must read as DOWN (writes dropped).
        assert!(gfx_is_up(0x8000_0000, 0x0000_001f));
        assert!(gfx_is_up(0xC000_0000, 0xFFFF_FF1f)); // extra bits don't matter
        assert!(!gfx_is_up(0x0000_0000, 0x0000_001f)); // autoload not complete
        assert!(!gfx_is_up(0x8000_0000, 0x0000_001e)); // one reset bit short
        assert!(!gfx_is_up(0x7fff_ffff, 0x0000_001f)); // bit 31 clear
        assert!(!gfx_is_up(0x0000_0000, 0x0000_0000)); // cold: both dropped
    }

    #[test]
    fn absent_block_yields_none() {
        // No GC block -> rlc/gfx None (never fabricate an offset)
        let only_mp1 = vec![IpBlock {
            hw_id: MP1_HWID,
            instance: 0,
            bases: vec![0x3000, 0x3800], // seg0 + seg1 (C2PMSG lives in seg1)
        }];
        assert!(rlc_safe_mode(&only_mp1).is_none());
        assert!(gfx_regs(&only_mp1).is_none());
        assert!(config_memsize_reg(&only_mp1).is_none());
        // MP1 present (with seg1) -> smu resolves
        assert!(smu_mailbox(&only_mp1).is_some());
    }
}
