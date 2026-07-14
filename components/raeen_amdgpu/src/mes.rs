//! gfx11 MES (Micro Engine Scheduler) bring-up — the ROOT CAUSE of the CP-fetch gate.
//!
//! amdgpu's gfx ring (`amdgpu_async_gfx_ring=1`, the DEFAULT) is a MES-scheduled
//! Kernel Gfx Queue, NOT a raw CP_RB0 ring: the CP only services the queue once MES
//! maps + schedules it. Proven on iron 2026-06-27 — booting amdgpu with
//! `async_gfx_ring=0` (the legacy raw-CP_RB0 path RaeenOS had been programming) makes
//! amdgpu FAIL to probe (`-110`, "MES failed to respond to msg=MISC"). So amdgpu
//! depends on MES even in legacy gfx mode. RaeenOS loads the MES ucode (mes_2.bin via
//! PSP autoload) but never STARTS the MES microengine, so the gfx queue is never
//! scheduled and the CP never fetches (every CP_RB0/GART/doorbell register matched
//! amdgpu exactly, yet RPTR stayed 0 — because the gate was never a register).
//!
//! This module is the PURE, host-provable bring-up vocabulary. The rungs, in the
//! ORDER `mes_v11_0_kiq_hw_init` + `mes_v11_0_hw_init` perform them:
//!   1. ENABLE both MES pipes (`mes_v11_0_enable`) — pipe0=scheduler, pipe1=KIQ.
//!   2. `kiq_setting` (RLC_CP_SCHEDULERS) + bring up the **KIQ ring** (pipe1) by
//!      DIRECT CP_HQD register writes (`mes_v11_0_queue_init_register`). The KIQ is
//!      the ONLY queue brought up directly.
//!   3. The KIQ maps the **SCHED ring** (pipe0) via a `PACKET3_MAP_QUEUES`
//!      (`gfx11_kiq_map_queues`) — the SCHED ring is NEVER direct-register-written
//!      (`mes_v11_0_queue_init` → `kiq_enable_queue`). THIS was the missing layer:
//!      direct-writing the SCHED HQD + ringing its doorbell hit the HQD but the MES
//!      never serviced a queue the KIQ hadn't mapped.
//!   4. `set_hw_resources` on the SCHED ring (doorbell/aperture layout to MES pipe0).
//!   5. `MAP_QUEUES` for the gfx queue (`amdgpu_mes_map_legacy_queue`), after which
//!      the CP finally fetches.
//! Kept PURE (returns the ordered `(reg, value)` writes a caller applies via MMIO) so
//! every field encoding is host-KAT'able and this file never touches the live GPU.

use alloc::vec::Vec;

/// Parse `mes_firmware_header_v1_0.mes_uc_start_addr_{lo,hi}` (blob offsets 56/60,
/// after the 32-byte common header + the six u32 ucode/data fields) — the MES
/// microengine entry point [`build_mes_enable_sequence`] loads into
/// `CP_MES_PRGRM_CNTR_START`. `None` unless the header is v1 (`header_version_major`
/// == 1 at offset 8) and long enough, so a truncated / wrong blob never yields a
/// bogus program counter. (mes_2.bin = scheduler/pipe0, mes1.bin = KIQ/pipe1.)
pub fn parse_mes_uc_start_addr(blob: &[u8]) -> Option<u64> {
    if blob.len() < 0x40 {
        return None;
    }
    let rd16 = |o: usize| u16::from_le_bytes([blob[o], blob[o + 1]]);
    let rd32 = |o: usize| u32::from_le_bytes([blob[o], blob[o + 1], blob[o + 2], blob[o + 3]]);
    if rd16(8) != 1 {
        return None; // header_version_major must be 1 (mes_firmware_header_v1_0)
    }
    let lo = rd32(56) as u64;
    let hi = rd32(60) as u64;
    Some((hi << 32) | lo)
}

// CP_MES_CNTL field bits (gc_11_0_0_sh_mask.h). These are architectural constants;
// the register OFFSET is discovery-resolved (GC seg1) into [`MesEnableRegs`].
const CP_MES_CNTL_PIPE0_RESET: u32 = 1 << 0x10; // bit 16
const CP_MES_CNTL_PIPE1_RESET: u32 = 1 << 0x11; // bit 17
const CP_MES_CNTL_PIPE0_ACTIVE: u32 = 1 << 0x1a; // bit 26
const CP_MES_CNTL_PIPE1_ACTIVE: u32 = 1 << 0x1b; // bit 27

/// The MES engine is ME index 3 in `GRBM_GFX_CNTL` — `mes_v11_0_enable` does
/// `soc21_grbm_select(adev, 3, pipe, 0, 0)` before each pipe's PC-start write.
const MES_ME_INDEX: u32 = 3;

/// MES-enable register offsets, discovery-resolved (all GC seg1):
/// `CP_MES_CNTL` (0x2807), `CP_MES_PRGRM_CNTR_START` (0x2800) + `_HI` (0x289d), and
/// `GRBM_GFX_CNTL` (the me/pipe selector). Supplied by the daemon once IP discovery
/// resolves them — so the enable sequence never pokes a guessed offset pre-iron.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MesEnableRegs {
    pub grbm_gfx_cntl: u32,
    pub cp_mes_cntl: u32,
    pub prgrm_cntr_start: u32,
    pub prgrm_cntr_start_hi: u32,
    /// `CP_MES_GP3_LO` (GC seg1 0x2849): the MES microengine writes its scheduler/KIQ
    /// firmware version here once it actually boots its ucode (`mes_v11_0_get_fw_version`
    /// reads it per pipe). Non-zero ⇒ the microengine is EXECUTING; 0 ⇒ ACTIVE bits are
    /// set but the engine isn't running its code (gated / ucode not loaded / faulted).
    pub cp_mes_gp3_lo: u32,
}

/// `GRBM_GFX_CNTL` field packing: `PIPEID[1:0] | MEID[3:2] | VMID[7:4] | QUEUEID[10:8]`.
fn grbm_select_value(me: u32, pipe: u32) -> u32 {
    (pipe & 0x3) | ((me & 0x3) << 2)
}

/// The RS64 MES program-counter is a DWORD address: `mes_v11_0_enable` writes
/// `lower/upper_32_bits(uc_start_addr >> 2)`. (Same `>>2` form as the gfx RS64 CP.)
fn mes_pc_start(addr: u64) -> (u32, u32) {
    let pc = addr >> 2;
    (pc as u32, (pc >> 32) as u32)
}

/// Build the `mes_v11_0_enable` register sequence: pulse the pipe reset(s), set each
/// MES pipe's program-counter start (from the autoloaded MES ucode entry), then
/// unhalt + ACTIVATE the pipe(s). `pipe0_start` is the scheduler pipe (mes_2.bin);
/// `pipe1_start` is the KIQ pipe (mes1.bin) when the Kernel Interface Queue is used.
/// The caller applies these via MMIO AFTER the PSP autoload has loaded the MES ucode.
pub fn build_mes_enable_sequence(
    r: &MesEnableRegs,
    pipe0_start: u64,
    pipe1_start: Option<u64>,
) -> Vec<(u32, u32)> {
    let kiq = pipe1_start.is_some();
    let mut w: Vec<(u32, u32)> = Vec::new();
    // Pulse the pipe reset(s) before loading the PC (mes_v11_0_enable resets first).
    let reset = CP_MES_CNTL_PIPE0_RESET | if kiq { CP_MES_CNTL_PIPE1_RESET } else { 0 };
    w.push((r.cp_mes_cntl, reset));
    // Pipe 0 (scheduler) program-counter start.
    w.push((r.grbm_gfx_cntl, grbm_select_value(MES_ME_INDEX, 0)));
    let (lo0, hi0) = mes_pc_start(pipe0_start);
    w.push((r.prgrm_cntr_start, lo0));
    w.push((r.prgrm_cntr_start_hi, hi0));
    // Pipe 1 (KIQ) program-counter start, when used.
    if let Some(s1) = pipe1_start {
        w.push((r.grbm_gfx_cntl, grbm_select_value(MES_ME_INDEX, 1)));
        let (lo1, hi1) = mes_pc_start(s1);
        w.push((r.prgrm_cntr_start, lo1));
        w.push((r.prgrm_cntr_start_hi, hi1));
    }
    // Restore the default GRBM selection (me0/pipe0/queue0/vmid0).
    w.push((r.grbm_gfx_cntl, 0));
    // Unhalt + activate the pipe(s) — the reset bits clear (not set) here, so this
    // write both releases the reset and sets ACTIVE.
    let active = CP_MES_CNTL_PIPE0_ACTIVE | if kiq { CP_MES_CNTL_PIPE1_ACTIVE } else { 0 };
    w.push((r.cp_mes_cntl, active));
    w
}

// CP_MES_IC_OP_CNTL field bits (gc_11_0_0_sh_mask.h).
const CP_MES_IC_OP_INVALIDATE_CACHE: u32 = 1 << 0;
const CP_MES_IC_OP_PRIME_ICACHE: u32 = 1 << 4;

/// MES instruction/data-cache base register offsets (`mes_v11_0_load_microcode`),
/// discovery-resolved (all GC seg1): IC base lo/hi/cntl (0x5850/0x5851/0x5852), the
/// MD (data) base lo/hi (0x5854/0x5855), the IC/MD bounds (0x585b/0x585d), and the
/// IC op-cntl (0x2820). Plus `GRBM_GFX_CNTL` + the PC-start regs (shared with enable).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MesLoadRegs {
    pub grbm_gfx_cntl: u32,
    pub prgrm_cntr_start: u32,
    pub prgrm_cntr_start_hi: u32,
    pub ic_base_lo: u32,
    pub ic_base_hi: u32,
    pub ic_base_cntl: u32,
    pub mibound_lo: u32,
    pub mdbase_lo: u32,
    pub mdbase_hi: u32,
    pub mdbound_lo: u32,
    pub ic_op_cntl: u32,
}

/// Build the `mes_v11_0_load_microcode` register sequence for one MES pipe — point
/// the MES instruction cache at the (direct-loaded) ucode + the data cache at the
/// data blob, set the 2M/512K bounds, and prime the I-cache. The MES ucode is
/// PSP-autoloaded into a PSP-managed location we can't address, so RaeenOS must
/// DIRECT-load it (allocate + copy + GART-map the ucode/data blobs, like the SDMA
/// direct-load) and pass their GART VAs here. WITHOUT this the MES has no code to run
/// on activation → it faults → `CP_MES_CNTL` ACTIVE clears (iron: reads 0). Run this
/// (per pipe) BEFORE [`build_mes_enable_sequence`]. `start_addr` is the ucode entry
/// ([`parse_mes_uc_start_addr`]); `ucode_va`/`data_va` are the GART VAs of the blobs.
pub fn build_mes_load_sequence(
    r: &MesLoadRegs,
    pipe: u32,
    ucode_va: u64,
    data_va: u64,
    start_addr: u64,
    prime_icache: bool,
) -> Vec<(u32, u32)> {
    let mut w: Vec<(u32, u32)> = Vec::new();
    w.push((r.grbm_gfx_cntl, grbm_select_value(MES_ME_INDEX, pipe)));
    w.push((r.ic_base_cntl, 0));
    let (pc_lo, pc_hi) = mes_pc_start(start_addr);
    w.push((r.prgrm_cntr_start, pc_lo));
    w.push((r.prgrm_cntr_start_hi, pc_hi));
    w.push((r.ic_base_lo, ucode_va as u32));
    w.push((r.ic_base_hi, (ucode_va >> 32) as u32));
    w.push((r.mibound_lo, 0x1F_FFFF)); // instruction cache boundary = 2M-1
    w.push((r.mdbase_lo, data_va as u32));
    w.push((r.mdbase_hi, (data_va >> 32) as u32));
    w.push((r.mdbound_lo, 0x7_FFFF)); // data boundary = 512K-1
                                      // Invalidate + prime the I-cache (the `if prime_icache` block in
                                      // mes_v11_0_load_microcode wraps BOTH writes). amdgpu primes ONCE — on
                                      // the LAST pipe loaded (KIQ), after both pipes' IC bases are set — so the
                                      // SCHED pipe passes prime=false and the KIQ pipe prime=true.
    if prime_icache {
        w.push((r.ic_op_cntl, CP_MES_IC_OP_INVALIDATE_CACHE));
        w.push((r.ic_op_cntl, CP_MES_IC_OP_PRIME_ICACHE));
    }
    // Restore the default GRBM selection.
    w.push((r.grbm_gfx_cntl, 0));
    w
}

/// The `v11_compute_mqd` is 512 dwords (2 KiB). The CP/MES reads each field at a
/// FIXED dword offset, so we build the whole 512-dword image and write the specific
/// fields `mes_v11_0_mqd_init` sets at their exact indices (from v11_structs.h).
pub const MQD_DWORDS: usize = 512;

// v11_compute_mqd field dword indices (from include/v11_structs.h).
const MQD_HEADER: usize = 0;
const MQD_PIPELINESTAT_ENABLE: usize = 11;
const MQD_STATIC_THREAD_MGMT_SE0: usize = 23;
const MQD_STATIC_THREAD_MGMT_SE1: usize = 24;
const MQD_STATIC_THREAD_MGMT_SE2: usize = 26;
const MQD_STATIC_THREAD_MGMT_SE3: usize = 27;
const MQD_MISC_RESERVED: usize = 32;
const MQD_CP_MQD_BASE_ADDR_LO: usize = 128;
const MQD_CP_MQD_BASE_ADDR_HI: usize = 129;
const MQD_CP_HQD_ACTIVE: usize = 130;
const MQD_CP_HQD_PERSISTENT_STATE: usize = 132;
const MQD_CP_HQD_PQ_BASE_LO: usize = 136;
const MQD_CP_HQD_PQ_BASE_HI: usize = 137;
const MQD_CP_HQD_PQ_RPTR_REPORT_ADDR_LO: usize = 139;
const MQD_CP_HQD_PQ_RPTR_REPORT_ADDR_HI: usize = 140;
const MQD_CP_HQD_PQ_WPTR_POLL_ADDR_LO: usize = 141;
const MQD_CP_HQD_PQ_WPTR_POLL_ADDR_HI: usize = 142;
const MQD_CP_HQD_PQ_DOORBELL_CONTROL: usize = 143;
const MQD_CP_HQD_PQ_CONTROL: usize = 145;
const MQD_CP_HQD_IB_CONTROL: usize = 149;
const MQD_CP_MQD_CONTROL: usize = 162;
const MQD_CP_HQD_EOP_BASE_ADDR_LO: usize = 165;
const MQD_CP_HQD_EOP_BASE_ADDR_HI: usize = 166;
const MQD_CP_HQD_EOP_CONTROL: usize = 167;

// Register reset DEFAULTs (gc_11_0_0_default.h) + MES_EOP_SIZE (mes_v11_0.c).
const CP_HQD_PQ_CONTROL_DEFAULT: u32 = 0x0030_8509;
const CP_HQD_PERSISTENT_STATE_DEFAULT: u32 = 0x0be0_5501; // PRELOAD_SIZE already 0x55
const CP_HQD_IB_CONTROL_DEFAULT: u32 = 0x0030_0000;
const CP_MQD_CONTROL_DEFAULT: u32 = 0x0000_0100; // VMID field = 0
const MES_EOP_SIZE: u32 = 2048;

/// `order_base_2(x)` = ceil(log2(x)) (amdgpu's helper). For our power-of-two inputs
/// it's just `log2`.
fn order_base_2(x: u32) -> u32 {
    if x <= 1 {
        0
    } else {
        32 - (x - 1).leading_zeros()
    }
}

/// Build the MES scheduler queue descriptor (`mes_v11_0_mqd_init`) as a 512-dword
/// image. `mqd_va`/`ring_va`/`rptr_va`/`wptr_va`/`eop_va` are the GART VAs of the MQD
/// itself, the command ring (HQD/PQ), the rptr-report + wptr-poll writebacks, and the
/// EOP buffer; `ring_bytes` sizes the queue; `doorbell_index` is the ring's doorbell.
///
/// NOTE the `RPTR_BLOCK_SIZE` quirk: amdgpu passes `(order_base_2(PAGE/4)-1) << 8` to
/// REG_SET_FIELD, whose own shift is ALSO 8, so the value shifts out of the field mask
/// and RPTR_BLOCK_SIZE ends up cleared to 0. We replicate that EXACTLY (matching the
/// working driver) — flagged for iron re-verification, since a host KAT can't catch a
/// misread of the hardware's intent here.
pub fn build_mes_mqd(
    mqd_va: u64,
    ring_va: u64,
    rptr_va: u64,
    wptr_va: u64,
    eop_va: u64,
    ring_bytes: u32,
    doorbell_index: u32,
) -> Vec<u32> {
    let mut m = alloc::vec![0u32; MQD_DWORDS];
    m[MQD_HEADER] = 0xC031_0800;
    m[MQD_PIPELINESTAT_ENABLE] = 0x0000_0001;
    m[MQD_STATIC_THREAD_MGMT_SE0] = 0xffff_ffff;
    m[MQD_STATIC_THREAD_MGMT_SE1] = 0xffff_ffff;
    m[MQD_STATIC_THREAD_MGMT_SE2] = 0xffff_ffff;
    m[MQD_STATIC_THREAD_MGMT_SE3] = 0xffff_ffff;
    m[MQD_MISC_RESERVED] = 0x0000_0007;

    // EOP: base = eop>>8; control = DEFAULT with EOP_SIZE = order_base_2(EOP/4)-1.
    let eop_base = eop_va >> 8;
    m[MQD_CP_HQD_EOP_BASE_ADDR_LO] = eop_base as u32;
    m[MQD_CP_HQD_EOP_BASE_ADDR_HI] = (eop_base >> 32) as u32;
    let eop_size = order_base_2(MES_EOP_SIZE / 4) - 1; // = 8
    m[MQD_CP_HQD_EOP_CONTROL] = eop_size & 0x3f; // EOP_CONTROL_DEFAULT=0x6, EOP_SIZE[5:0] replaced

    // MQD self-pointer + control (VMID=0).
    m[MQD_CP_MQD_BASE_ADDR_LO] = (mqd_va & 0xffff_fffc) as u32;
    m[MQD_CP_MQD_BASE_ADDR_HI] = (mqd_va >> 32) as u32;
    m[MQD_CP_MQD_CONTROL] = CP_MQD_CONTROL_DEFAULT; // VMID field = 0

    // PQ (command ring) base — like CP_RB0_BASE/_HI (>>8).
    let hqd = ring_va >> 8;
    m[MQD_CP_HQD_PQ_BASE_LO] = hqd as u32;
    m[MQD_CP_HQD_PQ_BASE_HI] = (hqd >> 32) as u32;
    // rptr-report + wptr-poll writeback addresses.
    m[MQD_CP_HQD_PQ_RPTR_REPORT_ADDR_LO] = (rptr_va & 0xffff_fffc) as u32;
    m[MQD_CP_HQD_PQ_RPTR_REPORT_ADDR_HI] = ((rptr_va >> 32) & 0xffff) as u32;
    m[MQD_CP_HQD_PQ_WPTR_POLL_ADDR_LO] = (wptr_va & 0xffff_fff8) as u32;
    m[MQD_CP_HQD_PQ_WPTR_POLL_ADDR_HI] = ((wptr_va >> 32) & 0xffff) as u32;

    // PQ control: QUEUE_SIZE=log2(bytes/4)-1, RPTR_BLOCK_SIZE quirk (cleared), then
    // UNORD_DISPATCH|PRIV_STATE|KMD_QUEUE|NO_UPDATE_RPTR (TUNNEL_DISPATCH=0).
    let mut pq = CP_HQD_PQ_CONTROL_DEFAULT;
    let queue_size = order_base_2(ring_bytes / 4) - 1;
    pq = (pq & !0x3f) | (queue_size & 0x3f); // QUEUE_SIZE[5:0]
    pq &= !0x0000_3f00; // RPTR_BLOCK_SIZE[13:8] cleared (the amdgpu double-shift quirk)
    pq |= 1 << 0x1b; // NO_UPDATE_RPTR
    pq |= 1 << 0x1c; // UNORD_DISPATCH
    pq |= 1 << 0x1e; // PRIV_STATE
    pq |= 1 << 0x1f; // KMD_QUEUE
    m[MQD_CP_HQD_PQ_CONTROL] = pq;

    // Doorbell: OFFSET (bit 2) + EN (bit 30).
    m[MQD_CP_HQD_PQ_DOORBELL_CONTROL] = (doorbell_index << 2) | (1 << 0x1e);

    m[MQD_CP_HQD_PERSISTENT_STATE] = CP_HQD_PERSISTENT_STATE_DEFAULT; // PRELOAD_SIZE=0x55
    m[MQD_CP_HQD_IB_CONTROL] = CP_HQD_IB_CONTROL_DEFAULT;
    m[MQD_CP_HQD_ACTIVE] = 1; // activate the queue
    m
}

/// CP_HQD register offsets (`mes_v11_0_queue_init_register`), discovery-resolved
/// (all GC seg0). These receive the MQD fields so the hardware queue goes live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MesHqdRegs {
    pub grbm_gfx_cntl: u32,
    pub cp_hqd_vmid: u32,
    pub cp_mqd_base_addr: u32,
    pub cp_mqd_base_addr_hi: u32,
    pub cp_mqd_control: u32,
    pub cp_hqd_pq_base: u32,
    pub cp_hqd_pq_base_hi: u32,
    pub cp_hqd_pq_rptr_report_addr: u32,
    pub cp_hqd_pq_rptr_report_addr_hi: u32,
    pub cp_hqd_pq_wptr_poll_addr: u32,
    pub cp_hqd_pq_wptr_poll_addr_hi: u32,
    pub cp_hqd_pq_doorbell_control: u32,
    pub cp_hqd_pq_control: u32,
    pub cp_hqd_persistent_state: u32,
    pub cp_hqd_active: u32,
    /// `RLC_CP_SCHEDULERS` (GC **seg1** 0x098a — BASE_IDX=1 per gc_11_0_0_offset.h) —
    /// `mes_v11_0_kiq_setting` writes it to tell the RLC which me/pipe/queue is the KIQ.
    pub rlc_cp_schedulers: u32,
}

/// `mes_v11_0_kiq_setting`: the low byte of `RLC_CP_SCHEDULERS` becomes
/// `(me << 5) | (pipe << 3) | queue`, then bit7 (0x80, "scheduler0 valid") is set.
/// For the MES KIQ ring (me=3, pipe=1, queue=0) → `0x68 | 0x80 = 0xE8`. Returns the
/// new full register value given the previous read (upper 24 bits preserved).
pub fn kiq_setting_value(prev: u32, me: u32, pipe: u32, queue: u32) -> u32 {
    let low = ((me << 5) | (pipe << 3) | queue) & 0xff;
    (prev & 0xffff_ff00) | low | 0x80
}

/// Build `mes_v11_0_queue_init_register`: select the MES pipe, then copy the MQD's
/// fields into the live CP_HQD registers (VMID, MQD base/control, PQ base/rptr/wptr/
/// control, doorbell, persistent-state, ACTIVE), restore the GRBM selection. This is
/// what makes the already-alive MES adopt its command ring. `mqd` is the image from
/// [`build_mes_mqd`]; reads its fields by the same dword indices.
pub fn build_mes_queue_init_register(r: &MesHqdRegs, mqd: &[u32], pipe: u32) -> Vec<(u32, u32)> {
    let f = |i: usize| mqd.get(i).copied().unwrap_or(0);
    alloc::vec![
        (r.grbm_gfx_cntl, grbm_select_value(MES_ME_INDEX, pipe)),
        // disable the doorbell first (clear), then program from the MQD.
        (r.cp_hqd_pq_doorbell_control, 0),
        (r.cp_hqd_vmid, 0),
        (r.cp_mqd_base_addr, f(MQD_CP_MQD_BASE_ADDR_LO)),
        (r.cp_mqd_base_addr_hi, f(MQD_CP_MQD_BASE_ADDR_HI)),
        (r.cp_mqd_control, f(MQD_CP_MQD_CONTROL)),
        (r.cp_hqd_pq_base, f(MQD_CP_HQD_PQ_BASE_LO)),
        (r.cp_hqd_pq_base_hi, f(MQD_CP_HQD_PQ_BASE_HI)),
        (
            r.cp_hqd_pq_rptr_report_addr,
            f(MQD_CP_HQD_PQ_RPTR_REPORT_ADDR_LO)
        ),
        (
            r.cp_hqd_pq_rptr_report_addr_hi,
            f(MQD_CP_HQD_PQ_RPTR_REPORT_ADDR_HI)
        ),
        (r.cp_hqd_pq_control, f(MQD_CP_HQD_PQ_CONTROL)),
        (
            r.cp_hqd_pq_wptr_poll_addr,
            f(MQD_CP_HQD_PQ_WPTR_POLL_ADDR_LO)
        ),
        (
            r.cp_hqd_pq_wptr_poll_addr_hi,
            f(MQD_CP_HQD_PQ_WPTR_POLL_ADDR_HI)
        ),
        (
            r.cp_hqd_pq_doorbell_control,
            f(MQD_CP_HQD_PQ_DOORBELL_CONTROL)
        ),
        (r.cp_hqd_persistent_state, f(MQD_CP_HQD_PERSISTENT_STATE)),
        (r.cp_hqd_active, f(MQD_CP_HQD_ACTIVE)),
        (r.grbm_gfx_cntl, 0),
    ]
}

/// The MES SET_HW_RESOURCES packet is a 64-dword (`API_FRAME_SIZE_IN_DWORDS`) frame.
/// Field dword offsets in `union MESAPI_SET_HW_RESOURCES` (mes_v11_api_def.h), all
/// naturally aligned (verified: every u64 lands on an 8-byte boundary):
const HWRES_FRAME_DWORDS: usize = 64;
// header @0; vmid_mask_mmhub@1, gfxhub@2; gds_size@3; paging_vmid@4;
// compute_hqd_mask[8]@5..13; gfx_hqd_mask[2]@13..15; sdma_hqd_mask[2]@15..17;
// aggregated_doorbells[5]@17..22; g_sch_ctx@22(u64); query_status_fence@24(u64);
// gc_base[8]@26..34; mmhub_base[8]@34..42; osssys_base[8]@42..50;
// api_completion_fence_addr@50(u64); api_completion_fence_value@52(u64);
// flags@54; oversubscription_timer@55.

/// The values the daemon computes (from MES state + IP discovery) for the
/// SET_HW_RESOURCES packet. `gc_base`/`mmhub_base`/`osssys_base` are the 8-segment IP
/// register bases (`reg_offset[HWIP][0][0..8]`); the masks/addrs come from MES init.
#[derive(Clone, Debug, Default)]
pub struct MesHwResources {
    pub vmid_mask_mmhub: u32,
    pub vmid_mask_gfxhub: u32,
    pub gds_size: u32,
    pub compute_hqd_mask: [u32; 8],
    pub gfx_hqd_mask: [u32; 2],
    pub sdma_hqd_mask: [u32; 2],
    pub sch_ctx_va: u64,
    pub query_fence_va: u64,
    pub gc_base: [u32; 8],
    pub mmhub_base: [u32; 8],
    pub osssys_base: [u32; 8],
    pub api_fence_addr: u64,
    pub api_fence_value: u64,
}

/// Build the `mes_v11_0_set_hw_resources` packet (the first command the alive MES
/// processes — it describes the doorbell/aperture/VMID layout + the IP register bases
/// so MES can drive the hardware). Returns the 64-dword frame to submit on the MES
/// command ring. header = TYPE_SCHEDULER(1) | SET_HW_RSRC(0) | dwsize 64; the flags
/// word = disable_reset|use_different_vmid_compute|disable_mes_log|apply_mmhub_pgvm_
/// invalidate_ack_loss_wa|enable_level_process_quantum_check|enable_reg_active_poll
/// (= 0x44f; the working amdgpu ships 0x447, bit3 added for the RaeenOS ack-loss env);
/// oversubscription_timer 50.
pub fn build_mes_set_hw_resources(r: &MesHwResources) -> Vec<u32> {
    let mut p = alloc::vec![0u32; HWRES_FRAME_DWORDS];
    // header: type[3:0]=1, opcode[11:4]=0, dwsize[19:12]=64.
    p[0] = 1 | (64u32 << 12);
    p[1] = r.vmid_mask_mmhub;
    p[2] = r.vmid_mask_gfxhub;
    p[3] = r.gds_size;
    // p[4] paging_vmid = 0
    p[5..13].copy_from_slice(&r.compute_hqd_mask);
    p[13..15].copy_from_slice(&r.gfx_hqd_mask);
    p[15..17].copy_from_slice(&r.sdma_hqd_mask);
    // aggregated_doorbells[5] — per-priority-level aggregated doorbell indices. The
    // WORKING amdgpu MES SCHED ring (Athena live debugfs dump, 2026-06-29) sends
    // 0x800,0x802,0x804,0x806,0x808 here. RaeenOS sent 0 — and the iron diagnostic
    // showed the MES READ set_hw_resources (HQD_RPTR advanced) but never ACKed it:
    // with no aggregated doorbells it can't set up scheduling, so it aborts the frame
    // without writing the api_completion_fence. These are doorbell-aperture indices.
    p[17..22].copy_from_slice(&[0x800, 0x802, 0x804, 0x806, 0x808]);
    p[22] = r.sch_ctx_va as u32;
    p[23] = (r.sch_ctx_va >> 32) as u32;
    p[24] = r.query_fence_va as u32;
    p[25] = (r.query_fence_va >> 32) as u32;
    p[26..34].copy_from_slice(&r.gc_base);
    p[34..42].copy_from_slice(&r.mmhub_base);
    p[42..50].copy_from_slice(&r.osssys_base);
    p[50] = r.api_fence_addr as u32;
    p[51] = (r.api_fence_addr >> 32) as u32;
    p[52] = r.api_fence_value as u32;
    p[53] = (r.api_fence_value >> 32) as u32;
    // flags: disable_reset(0)|use_different_vmid_compute(1)|disable_mes_log(2)|
    // apply_mmhub_pgvm_invalidate_ack_loss_wa(3)|enable_level_process_quantum_check(6)|
    // enable_reg_active_poll(10) = 0x44f. Bit positions VERIFIED against
    // mes_v11_api_def.h (union MESAPI_SET_HW_RESOURCES); the working amdgpu ships 0x447.
    // NOTE: clearing enable_reg_active_poll(10) did NOT change the MES stall at INSTR
    // 0x7656 (iron 2026-06-29) — the stall is NOT that flag.
    //
    // bit3 = apply_mmhub_pgvm_invalidate_ack_loss_wa: a MES-firmware workaround for a
    // LOST MMHUB page-VM invalidate ACK. The working driver ships it OFF (its MMHUB acks
    // properly), but RaeenOS's INVALIDATE-STALL capture shows the MES issues the full-
    // flush invalidate (req=0x2f80000) and the ACK never returns on EITHER hub (MMVM/GCVM
    // ack=0) — the microengine then freezes mid-set_hw_resources (INSTR 0x7656, clock
    // running, no fault) waiting on an ack our environment never delivers. Setting the WA
    // tells the MES to tolerate the ack loss instead of hanging on it. See
    // docs/gpu-oracle/netlog-MES-INVALIDATE-STALL-20260629.txt.
    p[54] = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3) | (1 << 6) | (1 << 10);
    p[55] = 50; // oversubscription_timer
    p
}

/// Build a `MESAPI_SET_HW_RESOURCES_1` frame (opcode 0x13). amdgpu sends this RIGHT
/// after `set_hw_resources` (the live working SCHED ring carries SET_HW_RSRC @0x000,
/// QUERY @0x100, SET_HW_RSRC_1 @0x200, QUERY @0x300). Layout byte-diffed from the Athena
/// `umr -RS mes_3.0.0` dump (2026-06-29): header@0 (type1|op0x13|dwsize64 = 0x40131),
/// api_completion_fence_addr@1-2(u64), value@3-4(u64), timestamp@5-6(u64)=0,
/// enable_mes_info_ctx@7=1, mes_info_ctx_mc_addr@8-9(u64)=0, mes_info_ctx_size@10=0,
/// reserved1@11-12(u64)=0, cleaner_shader_fence_mc_addr@13-14(u64). The MES may not
/// finalize its scheduler context (GP0/GP2) until it processes this second packet.
pub fn build_mes_set_hw_resources_1(
    api_fence_addr: u64,
    api_fence_value: u64,
    cleaner_shader_fence_mc_addr: u64,
) -> Vec<u32> {
    let mut p = alloc::vec![0u32; HWRES_FRAME_DWORDS];
    p[0] = 1 | (0x13 << 4) | (64 << 12); // = 0x40131
    p[1] = api_fence_addr as u32;
    p[2] = (api_fence_addr >> 32) as u32;
    p[3] = api_fence_value as u32;
    p[4] = (api_fence_value >> 32) as u32;
    // p[5..7] timestamp = 0
    p[7] = 1; // enable_mes_info_ctx (working = 1)
              // p[8..10] mes_info_ctx_mc_addr = 0 (working = 0)
              // p[10] mes_info_ctx_size = 0; p[11..13] reserved1 = 0
    p[13] = cleaner_shader_fence_mc_addr as u32;
    p[14] = (cleaner_shader_fence_mc_addr >> 32) as u32;
    p
}

/// Build a `MESAPI__QUERY_MES_STATUS` frame (opcode 11). amdgpu appends this to EVERY
/// MES submission (`mes_v11_0_submit_pkt_and_poll_completion`) and polls ITS completion
/// fence rather than the preceding packet's — the MES processes the ring up to the QUERY,
/// which flushes the batch and signals done. Layout read from the live working amdgpu
/// SCHED ring (Athena 2026-06-29, debugfs amdgpu_ring_mes_3.0.0): header@0,
/// api_status@dword2 (api_completion_fence_addr@2-3, api_completion_fence_value@4-5).
pub fn build_mes_query_scheduler_status(fence_addr: u64, fence_value: u64) -> Vec<u32> {
    let mut p = alloc::vec![0u32; HWRES_FRAME_DWORDS]; // 64-dword API frame
                                                       // header: type=SCHEDULER(1), opcode=QUERY_SCHEDULER_STATUS(11), dwsize=64.
    p[0] = 1 | (11 << 4) | (64 << 12);
    p[2] = fence_addr as u32;
    p[3] = (fence_addr >> 32) as u32;
    p[4] = fence_value as u32;
    p[5] = (fence_value >> 32) as u32;
    p
}

/// Build `mes_v11_0_map_legacy_queue` — the `MESAPI__ADD_QUEUE` packet (opcode 2)
/// that tells the alive, configured MES to SCHEDULE a legacy kernel queue (our gfx
/// ring). THIS is the packet that finally makes the CP fetch. Only the legacy-queue
/// subset of ADD_QUEUE is set: doorbell_offset, mqd_addr (the gfx MQD), wptr_addr,
/// queue_type=GFX(0), map_legacy_kq=1, pipe/queue id. Field dword offsets computed
/// for `#pragma pack(4)` (u64s are 4-byte packed — no 8-byte padding):
/// header@0, doorbell_offset@18, mqd_addr@19(u64), wptr_addr@21(u64), queue_type@27,
/// the flags bitfield@36 (map_legacy_kq = bit 11), pipe_id@48, queue_id@49.
pub fn build_mes_map_legacy_queue(
    doorbell_offset: u32,
    mqd_va: u64,
    wptr_va: u64,
    pipe_id: u32,
    queue_id: u32,
) -> Vec<u32> {
    let mut p = alloc::vec![0u32; HWRES_FRAME_DWORDS]; // 64-dword API frame
                                                       // header: type=SCHEDULER(1), opcode=ADD_QUEUE(2), dwsize=64.
    p[0] = 1 | (2u32 << 4) | (64u32 << 12);
    p[18] = doorbell_offset;
    p[19] = mqd_va as u32;
    p[20] = (mqd_va >> 32) as u32;
    p[21] = wptr_va as u32;
    p[22] = (wptr_va >> 32) as u32;
    p[27] = 0; // queue_type = MES_QUEUE_TYPE_GFX
    p[36] = 1 << 11; // map_legacy_kq
    p[48] = pipe_id;
    p[49] = queue_id;
    p
}

/// Build the KIQ `PACKET3_MAP_QUEUES` (gfx11) that maps the **MES scheduler ring**
/// (`gfx11_kiq_map_queues` for `AMDGPU_RING_TYPE_MES` → me=2, eng_sel=5). The KIQ
/// (MES pipe 1) consumes this from ITS ring and activates the scheduler ring (MES
/// pipe 0) — the SCHED ring is NEVER brought up by direct CP_HQD writes, only by the
/// KIQ via this packet (`mes_v11_0_queue_init` → `kiq_enable_queue`). 7 dwords: a
/// type-3 header (count=5 → 6 body dwords) + the queue-select word + doorbell offset
/// + MQD address (u64) + WPTR address (u64). All addresses are GART VAs.
pub fn build_kiq_map_queues_mes(
    doorbell_index: u32, // the SCHED ring doorbell (mes_ring0<<1 = 0x16)
    mqd_va: u64,         // GART VA of the SCHED ring MQD
    wptr_va: u64,        // GART VA of the SCHED ring wptr writeback
    pipe: u32,           // SCHED ring pipe = 0
    queue: u32,          // SCHED ring queue = 0
) -> Vec<u32> {
    // op = 0xA2 — the HWS PACKET3_MAP_QUEUES (soc15d.h). NOT 0x26 (that was wrong and is
    // why the SCHED ring never activated). Iron-proven 2026-06-28: the working amdgpu's
    // mes_kiq ring header reads 0xc005a200 (op=0xA2) and its body word is 0x34080000 —
    // byte-identical to what this builder emits once the opcode is right.
    const PACKET3_MAP_QUEUES: u32 = 0xA2;
    const ME_MES: u32 = 2;
    const ENG_SEL_MES: u32 = 5;
    // PACKET3(op, n): (3<<30) | ((op&0xff)<<8) | ((n&0x3fff)<<16). n=5 ⇒ 6 body dwords.
    // COUNT lives in bits[29:16] (CP_PACKET_GET_COUNT = (h>>16)&0x3fff), NOT the low bits.
    let header = (3u32 << 30) | ((PACKET3_MAP_QUEUES & 0xff) << 8) | ((5 & 0x3fff) << 16);
    // PACKET3_MAP_QUEUES bitfields (soc15d.h): QUEUE_SEL<<4, VMID<<8, QUEUE<<13,
    // PIPE<<16, ME<<18, QUEUE_TYPE<<21, ALLOC_FORMAT<<24, ENGINE_SEL<<26, NUM_QUEUES<<29.
    let sel = (0u32 << 4)       // QUEUE_SEL = 0 (map by mqd)
        | (0u32 << 8)           // VMID = 0
        | ((queue & 0x7) << 13) // QUEUE
        | ((pipe & 0x3) << 16)  // PIPE
        | (ME_MES << 18)        // ME = 2
        | (0u32 << 21)          // QUEUE_TYPE = 0
        | (0u32 << 24)          // ALLOC_FORMAT = all_on_one_pipe
        | (ENG_SEL_MES << 26)   // ENGINE_SEL = 5 (MES)
        | (1u32 << 29); // NUM_QUEUES = 1
    let doorbell = doorbell_index << 2; // PACKET3_MAP_QUEUES_DOORBELL_OFFSET(x) = x<<2
    alloc::vec![
        header,
        sel,
        doorbell,
        mqd_va as u32,
        (mqd_va >> 32) as u32,
        wptr_va as u32,
        (wptr_va >> 32) as u32,
    ]
}

// v11_gfx_mqd field dword indices (include/v11_structs.h) — gfx_v11_0_gfx_mqd_init.
const GMQD_CP_MQD_BASE_ADDR: usize = 128;
const GMQD_CP_MQD_BASE_ADDR_HI: usize = 129;
const GMQD_CP_GFX_HQD_ACTIVE: usize = 130;
const GMQD_CP_GFX_HQD_VMID: usize = 131;
const GMQD_CP_GFX_HQD_QUANTUM: usize = 135;
const GMQD_CP_GFX_HQD_BASE: usize = 136;
const GMQD_CP_GFX_HQD_BASE_HI: usize = 137;
const GMQD_CP_GFX_HQD_RPTR: usize = 138;
const GMQD_CP_GFX_HQD_RPTR_ADDR: usize = 139;
const GMQD_CP_GFX_HQD_RPTR_ADDR_HI: usize = 140;
const GMQD_CP_RB_WPTR_POLL_ADDR_LO: usize = 141;
const GMQD_CP_RB_WPTR_POLL_ADDR_HI: usize = 142;
const GMQD_CP_RB_DOORBELL_CONTROL: usize = 143;
const GMQD_CP_GFX_HQD_CNTL: usize = 145;
const GMQD_CP_GFX_MQD_CONTROL: usize = 162;
// gfx HQD register reset DEFAULTs (gc_11_0_0_default.h).
const CP_GFX_HQD_QUANTUM_DEFAULT: u32 = 0x0000_0a01;
const CP_GFX_HQD_CNTL_DEFAULT: u32 = 0x00a0_0000;
const CP_GFX_MQD_CONTROL_DEFAULT: u32 = 0x0000_0100;

/// Build the GFX queue MQD (`gfx_v11_0_gfx_mqd_init`, `v11_gfx_mqd`, 512 dwords) — the
/// descriptor the MES `map_legacy_queue` packet points at (`mqd_addr`). Describes the
/// gfx ring the alive MES schedules: ring base (HQD), rptr/wptr writebacks, the CNTL
/// (RB_BUFSZ/BLKSZ from the ring size), and the doorbell. All addresses are GART VAs.
pub fn build_gfx_mqd(
    mqd_va: u64,
    ring_va: u64,
    rptr_va: u64,
    wptr_va: u64,
    ring_bytes: u32,
    doorbell_index: u32,
) -> Vec<u32> {
    let mut m = alloc::vec![0u32; MQD_DWORDS];
    m[GMQD_CP_MQD_BASE_ADDR] = (mqd_va & 0xffff_fffc) as u32;
    m[GMQD_CP_MQD_BASE_ADDR_HI] = (mqd_va >> 32) as u32;
    m[GMQD_CP_GFX_MQD_CONTROL] = CP_GFX_MQD_CONTROL_DEFAULT; // VMID=0
    m[GMQD_CP_GFX_HQD_VMID] = 0;
    m[GMQD_CP_GFX_HQD_QUANTUM] = CP_GFX_HQD_QUANTUM_DEFAULT;
    let hqd = ring_va >> 8;
    m[GMQD_CP_GFX_HQD_BASE] = hqd as u32;
    m[GMQD_CP_GFX_HQD_BASE_HI] = (hqd >> 32) as u32;
    m[GMQD_CP_GFX_HQD_RPTR] = 0; // RPTR_DEFAULT
    m[GMQD_CP_GFX_HQD_RPTR_ADDR] = (rptr_va & 0xffff_fffc) as u32;
    m[GMQD_CP_GFX_HQD_RPTR_ADDR_HI] = (rptr_va >> 32) as u32;
    m[GMQD_CP_RB_WPTR_POLL_ADDR_LO] = (wptr_va & 0xffff_fff8) as u32;
    m[GMQD_CP_RB_WPTR_POLL_ADDR_HI] = (wptr_va >> 32) as u32;
    // CNTL = DEFAULT with RB_BUFSZ[5:0] = log2(bytes/4)-1, RB_BLKSZ[13:8] = bufsz-2.
    let bufsz = order_base_2(ring_bytes / 4) - 1;
    let mut cntl = CP_GFX_HQD_CNTL_DEFAULT;
    cntl = (cntl & !0x3f) | (bufsz & 0x3f);
    cntl = (cntl & !0x3f00) | ((bufsz.wrapping_sub(2) & 0x3f) << 8);
    m[GMQD_CP_GFX_HQD_CNTL] = cntl;
    // Doorbell: OFFSET (bit 2) + EN (bit 30) — matches gfx_v11_0 CP_RB_DOORBELL_CONTROL.
    m[GMQD_CP_RB_DOORBELL_CONTROL] = (doorbell_index << 2) | (1 << 0x1e);
    m[GMQD_CP_GFX_HQD_ACTIVE] = 1;
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regs() -> MesEnableRegs {
        MesEnableRegs {
            grbm_gfx_cntl: 0x900,
            cp_mes_cntl: 0x2807,
            prgrm_cntr_start: 0x2800,
            prgrm_cntr_start_hi: 0x289d,
            cp_mes_gp3_lo: 0x2849,
        }
    }

    #[test]
    fn parse_mes_uc_start_reads_v1_header() {
        // Minimal mes_firmware_header_v1_0: version_major=1 @8, start_addr @56/60.
        let mut blob = alloc::vec![0u8; 0x44];
        blob[8] = 1; // header_version_major (u16 LE) = 1
        blob[56..60].copy_from_slice(&0x0040_1234u32.to_le_bytes());
        blob[60..64].copy_from_slice(&0x0000_0007u32.to_le_bytes());
        assert_eq!(parse_mes_uc_start_addr(&blob), Some(0x0000_0007_0040_1234));
        // A v2/F32-style header (major != 1) is rejected.
        let mut wrong = blob.clone();
        wrong[8] = 2;
        assert_eq!(parse_mes_uc_start_addr(&wrong), None);
        // Too-short blob -> None.
        assert_eq!(parse_mes_uc_start_addr(&[0u8; 8]), None);
    }

    #[test]
    fn pc_start_is_dword_address() {
        // Sub-4GiB entry: pure >>2, hi = 0.
        assert_eq!(mes_pc_start(0x1234_5678), (0x1234_5678u32 >> 2, 0));
        // hi carry: addr >> 2 split into lo/hi 32-bit halves.
        let (lo, hi) = mes_pc_start(0x0000_000a_0000_0008);
        assert_eq!(lo, ((0x0000_000a_0000_0008u64 >> 2) & 0xFFFF_FFFF) as u32);
        assert_eq!(hi, ((0x0000_000a_0000_0008u64 >> 2) >> 32) as u32);
    }

    #[test]
    fn enable_scheduler_only_activates_pipe0() {
        let r = regs();
        let seq = build_mes_enable_sequence(&r, 0x4000, None);
        // First write resets pipe0 (bit 16), not pipe1.
        assert_eq!(seq[0], (r.cp_mes_cntl, 1 << 0x10));
        // PC start for pipe 0 was written (dword address).
        let find = |reg: u32| seq.iter().find(|(rr, _)| *rr == reg).map(|(_, v)| *v);
        assert_eq!(find(r.prgrm_cntr_start), Some(0x4000u32 >> 2));
        // GRBM selected MES (me=3) pipe 0 at some point, then restored to 0.
        assert!(seq
            .iter()
            .any(|&(rr, v)| rr == r.grbm_gfx_cntl && v == (3 << 2)));
        assert_eq!(seq.last(), Some(&(r.cp_mes_cntl, CP_MES_CNTL_PIPE0_ACTIVE)));
    }

    #[test]
    fn order_base_2_is_ceil_log2() {
        assert_eq!(order_base_2(512), 9); // EOP: 2048/4
        assert_eq!(order_base_2(16384), 14); // 64KiB/4
        assert_eq!(order_base_2(1), 0);
    }

    #[test]
    fn gfx_mqd_sets_ring_fields() {
        let m = build_gfx_mqd(
            0x7fff_0050_0000,
            0x7fff_0011_0000,
            0x7fff_0012_0000,
            0x7fff_0012_0100,
            64 * 1024,
            0x116,
        );
        assert_eq!(m.len(), MQD_DWORDS);
        assert_eq!(m[GMQD_CP_GFX_HQD_BASE], (0x7fff_0011_0000u64 >> 8) as u32);
        assert_eq!(m[GMQD_CP_GFX_HQD_ACTIVE], 1);
        assert_eq!(m[GMQD_CP_GFX_HQD_QUANTUM], 0xa01);
        // CNTL: BUFSZ=13 (64K), BLKSZ=11, on the 0xa00000 default.
        let cntl = m[GMQD_CP_GFX_HQD_CNTL];
        assert_eq!(cntl & 0x3f, 13);
        assert_eq!((cntl >> 8) & 0x3f, 11);
        assert_eq!(cntl & 0xa00000, 0xa00000, "default high bits kept");
        // Doorbell EN + offset (= 0x40000458 for index 0x116).
        assert_eq!(m[GMQD_CP_RB_DOORBELL_CONTROL], (0x116 << 2) | (1 << 0x1e));
    }

    #[test]
    fn map_legacy_queue_packet_layout() {
        let p = build_mes_map_legacy_queue(0x458, 0x7fff_0050_0000, 0x7fff_0011_fff0, 0, 0);
        assert_eq!(p.len(), HWRES_FRAME_DWORDS);
        // header: type=1, opcode=ADD_QUEUE(2), dwsize=64.
        assert_eq!(p[0] & 0xf, 1);
        assert_eq!((p[0] >> 4) & 0xff, 2, "opcode=ADD_QUEUE");
        assert_eq!((p[0] >> 12) & 0xff, 64);
        assert_eq!(p[18], 0x458, "doorbell_offset@18");
        assert_eq!(p[19], 0x0050_0000, "mqd_addr lo@19");
        assert_eq!(p[20], 0x7fff, "mqd_addr hi@20");
        assert_eq!(p[21], 0x0011_fff0, "wptr_addr lo@21");
        assert_eq!(p[27], 0, "queue_type=GFX");
        assert_eq!(p[36], 1 << 11, "map_legacy_kq bit");
    }

    #[test]
    fn kiq_map_queues_mes_packet_layout() {
        // SCHED ring: doorbell idx 0x16, pipe 0, queue 0.
        let p = build_kiq_map_queues_mes(0x16, 0x7fff_0050_0000, 0x7fff_0011_fff0, 0, 0);
        assert_eq!(p.len(), 7, "header + 6 body dwords");
        // type-3 header, opcode MAP_QUEUES(0xA2), count=5. Iron-anchored: the working
        // amdgpu's mes_kiq ring header reads EXACTLY 0xc005a200 (hexdump 2026-06-28).
        assert_eq!(p[0] >> 30, 3, "PACKET3 type");
        assert_eq!((p[0] >> 8) & 0xff, 0xA2, "opcode MAP_QUEUES");
        assert_eq!((p[0] >> 16) & 0x3fff, 5, "count=5 (bits[29:16])");
        assert_eq!(
            p[0], 0xc005a200,
            "header byte-identical to the working KIQ ring"
        );
        // selector word must equal the working ring's body word 0x34080000.
        assert_eq!(
            p[1], 0x3408_0000,
            "sel word byte-identical to the working KIQ ring"
        );
        // selector: ME=2 (MES), ENGINE_SEL=5, NUM_QUEUES=1, pipe/queue 0.
        assert_eq!((p[1] >> 18) & 0x7, 2, "ME=2");
        assert_eq!((p[1] >> 26) & 0x7, 5, "ENGINE_SEL=5 (MES)");
        assert_eq!((p[1] >> 29) & 0x7, 1, "NUM_QUEUES=1");
        assert_eq!((p[1] >> 16) & 0x3, 0, "PIPE=0");
        assert_eq!((p[1] >> 13) & 0x7, 0, "QUEUE=0");
        // doorbell offset = idx << 2.
        assert_eq!(p[2], 0x16 << 2, "doorbell offset");
        // mqd + wptr addresses split lo/hi.
        assert_eq!(p[3], 0x0050_0000, "mqd lo");
        assert_eq!(p[4], 0x7fff, "mqd hi");
        assert_eq!(p[5], 0x0011_fff0, "wptr lo");
        assert_eq!(p[6], 0x7fff, "wptr hi");
    }

    #[test]
    fn kiq_setting_packs_me_pipe_queue() {
        // MES KIQ ring: me=3, pipe=1, queue=0 ⇒ low byte 0x68, bit7 set ⇒ 0xE8.
        let v = kiq_setting_value(0xabcd_ef00, 3, 1, 0);
        assert_eq!(v & 0xff, 0xE8, "low byte = (3<<5)|(1<<3)|0x80");
        assert_eq!(v & 0xffff_ff00, 0xabcd_ef00, "upper 24 bits preserved");
    }

    #[test]
    fn set_hw_resources_packet_layout() {
        let r = MesHwResources {
            vmid_mask_gfxhub: 0x0000_ff00,
            gc_base: [0x10, 0x20, 0x30, 0x40, 0x50, 0, 0, 0],
            sch_ctx_va: 0x7fff_0040_0000,
            api_fence_addr: 0x7fff_0041_0000,
            api_fence_value: 1,
            compute_hqd_mask: [0xffff_fffe, 0, 0, 0, 0, 0, 0, 0],
            ..Default::default()
        };
        let p = build_mes_set_hw_resources(&r);
        assert_eq!(p.len(), HWRES_FRAME_DWORDS);
        // header: type=1, opcode=0, dwsize=64.
        assert_eq!(p[0] & 0xf, 1, "type=SCHEDULER");
        assert_eq!((p[0] >> 4) & 0xff, 0, "opcode=SET_HW_RSRC");
        assert_eq!((p[0] >> 12) & 0xff, 64, "dwsize");
        // fields land at the right dword offsets.
        assert_eq!(p[2], 0x0000_ff00, "vmid_mask_gfxhub@2");
        assert_eq!(p[5], 0xffff_fffe, "compute_hqd_mask[0]@5");
        // aggregated_doorbells[5]@17..22 — the working amdgpu MES SCHED ring values.
        assert_eq!(p[17], 0x800, "aggregated_doorbells[0]@17");
        assert_eq!(p[21], 0x808, "aggregated_doorbells[4]@21");
        assert_eq!(p[22], 0x0040_0000, "sch_ctx lo@22");
        assert_eq!(p[23], 0x7fff, "sch_ctx hi@23");
        assert_eq!(p[26], 0x10, "gc_base[0]@26");
        assert_eq!(p[30], 0x50, "gc_base[4]@30");
        assert_eq!(p[50], 0x0041_0000, "api_fence_addr lo@50");
        assert_eq!(p[52], 1, "api_fence_value lo@52");
        // flags + oversubscription timer.
        assert_eq!(
            p[54], 0x44f,
            "flags (0x447 + bit3 mmhub_pgvm_invalidate_ack_loss_wa)"
        );
        assert_eq!(p[55], 50, "oversubscription_timer");
    }

    #[test]
    fn queue_init_register_copies_mqd_to_hqd_regs() {
        let mqd = build_mes_mqd(
            0x7fff_0010_0000,
            0x7fff_0011_0000,
            0x7fff_0012_0000,
            0x7fff_0012_0100,
            0x7fff_0013_0000,
            64 * 1024,
            0x116,
        );
        let r = MesHqdRegs {
            grbm_gfx_cntl: 0x900,
            cp_hqd_vmid: 0x1fac,
            cp_mqd_base_addr: 0x1fa9,
            cp_mqd_base_addr_hi: 0x1faa,
            cp_mqd_control: 0x1fcb,
            cp_hqd_pq_base: 0x1fb1,
            cp_hqd_pq_base_hi: 0x1fb2,
            cp_hqd_pq_rptr_report_addr: 0x1fb4,
            cp_hqd_pq_rptr_report_addr_hi: 0x1fb5,
            cp_hqd_pq_wptr_poll_addr: 0x1fb6,
            cp_hqd_pq_wptr_poll_addr_hi: 0x1fb7,
            cp_hqd_pq_doorbell_control: 0x1fb8,
            cp_hqd_pq_control: 0x1fba,
            cp_hqd_persistent_state: 0x1fad,
            cp_hqd_active: 0x1fab,
            rlc_cp_schedulers: 0x098a,
        };
        let seq = build_mes_queue_init_register(&r, &mqd, 0);
        let find = |reg: u32| seq.iter().rev().find(|(rr, _)| *rr == reg).map(|(_, v)| *v);
        // The HQD regs got the MQD's PQ base + control + ACTIVE.
        assert_eq!(
            find(r.cp_hqd_pq_base),
            Some((0x7fff_0011_0000u64 >> 8) as u32)
        );
        assert_eq!(find(r.cp_hqd_pq_control), Some(mqd[MQD_CP_HQD_PQ_CONTROL]));
        assert_eq!(find(r.cp_hqd_active), Some(1));
        // Selects MES (me=3) first, restores GRBM (0) last.
        assert_eq!(seq.first(), Some(&(r.grbm_gfx_cntl, 3 << 2)));
        assert_eq!(seq.last(), Some(&(r.grbm_gfx_cntl, 0)));
    }

    #[test]
    fn mes_mqd_sets_fields_at_correct_offsets() {
        let m = build_mes_mqd(
            0x7fff_0010_0000, // mqd
            0x7fff_0011_0000, // ring (HQD/PQ)
            0x7fff_0012_0000, // rptr report
            0x7fff_0012_0100, // wptr poll
            0x7fff_0013_0000, // eop
            64 * 1024,        // ring bytes
            0x116,            // doorbell index
        );
        assert_eq!(m.len(), MQD_DWORDS);
        assert_eq!(m[MQD_HEADER], 0xC031_0800);
        assert_eq!(m[MQD_STATIC_THREAD_MGMT_SE0], 0xffff_ffff);
        assert_eq!(m[MQD_MISC_RESERVED], 7);
        assert_eq!(m[MQD_CP_HQD_ACTIVE], 1);
        // PQ base = ring >> 8.
        assert_eq!(m[MQD_CP_HQD_PQ_BASE_LO], (0x7fff_0011_0000u64 >> 8) as u32);
        assert_eq!(
            m[MQD_CP_HQD_PQ_BASE_HI],
            ((0x7fff_0011_0000u64 >> 8) >> 32) as u32
        );
        // EOP_SIZE = 8 (order_base_2(2048/4)-1).
        assert_eq!(m[MQD_CP_HQD_EOP_CONTROL], 8);
        // PQ control: QUEUE_SIZE=13 (64K), RPTR_BLOCK cleared, and the high control bits.
        let pq = m[MQD_CP_HQD_PQ_CONTROL];
        assert_eq!(pq & 0x3f, 13, "QUEUE_SIZE");
        assert_eq!(pq & 0x3f00, 0, "RPTR_BLOCK_SIZE cleared (amdgpu quirk)");
        assert_ne!(pq & (1 << 0x1f), 0, "KMD_QUEUE");
        assert_ne!(pq & (1 << 0x1e), 0, "PRIV_STATE");
        // Doorbell EN + offset.
        assert_eq!(
            m[MQD_CP_HQD_PQ_DOORBELL_CONTROL],
            (0x116 << 2) | (1 << 0x1e)
        );
        // persistent_state keeps PRELOAD_SIZE=0x55 (default already has it).
        assert_eq!(m[MQD_CP_HQD_PERSISTENT_STATE], 0x0be0_5501);
    }

    #[test]
    fn load_sequence_sets_ic_md_bases_and_primes() {
        let r = MesLoadRegs {
            grbm_gfx_cntl: 0x900,
            prgrm_cntr_start: 0x2800,
            prgrm_cntr_start_hi: 0x289d,
            ic_base_lo: 0x5850,
            ic_base_hi: 0x5851,
            ic_base_cntl: 0x5852,
            mibound_lo: 0x585b,
            mdbase_lo: 0x5854,
            mdbase_hi: 0x5855,
            mdbound_lo: 0x585d,
            ic_op_cntl: 0x2820,
        };
        let seq = build_mes_load_sequence(&r, 0, 0x7fff_0011_2000, 0x7fff_0013_4000, 0x4000, true);
        let find = |reg: u32| seq.iter().find(|(rr, _)| *rr == reg).map(|(_, v)| *v);
        // prime=false omits BOTH the invalidate + prime writes (the `if prime_icache`).
        let noprime =
            build_mes_load_sequence(&r, 1, 0x7fff_0050_0000, 0x7fff_0060_0000, 0x4000, false);
        assert!(
            !noprime.iter().any(|(rr, _)| *rr == r.ic_op_cntl),
            "prime=false => no IC_OP_CNTL writes"
        );
        // IC base = the ucode GART VA lo/hi; MD base = the data GART VA.
        assert_eq!(find(r.ic_base_lo), Some(0x0011_2000));
        assert_eq!(find(r.ic_base_hi), Some(0x7fff));
        assert_eq!(find(r.mdbase_lo), Some(0x0013_4000));
        // PC start is the dword entry, bounds are the 2M/512K constants.
        assert_eq!(find(r.prgrm_cntr_start), Some(0x4000u32 >> 2));
        assert_eq!(find(r.mibound_lo), Some(0x1F_FFFF));
        assert_eq!(find(r.mdbound_lo), Some(0x7_FFFF));
        // The I-cache is invalidated THEN primed (two IC_OP_CNTL writes, in order).
        let ops: Vec<u32> = seq
            .iter()
            .filter(|(rr, _)| *rr == r.ic_op_cntl)
            .map(|(_, v)| *v)
            .collect();
        assert_eq!(
            ops,
            alloc::vec![CP_MES_IC_OP_INVALIDATE_CACHE, CP_MES_IC_OP_PRIME_ICACHE]
        );
        // GRBM selected MES (me=3) then restored to 0 last.
        assert_eq!(seq.last(), Some(&(r.grbm_gfx_cntl, 0)));
    }

    #[test]
    fn enable_with_kiq_activates_both_pipes() {
        let r = regs();
        let seq = build_mes_enable_sequence(&r, 0x4000, Some(0x8000));
        // Reset both pipes.
        assert_eq!(seq[0], (r.cp_mes_cntl, (1 << 0x10) | (1 << 0x11)));
        // Pipe 1 (KIQ) was selected (me=3, pipe=1) and its PC set.
        assert!(seq
            .iter()
            .any(|&(rr, v)| rr == r.grbm_gfx_cntl && v == ((3 << 2) | 1)));
        // Final write activates BOTH pipes.
        assert_eq!(
            seq.last(),
            Some(&(
                r.cp_mes_cntl,
                CP_MES_CNTL_PIPE0_ACTIVE | CP_MES_CNTL_PIPE1_ACTIVE
            ))
        );
    }
}
