# gfx11 (Phoenix/Radeon 780M) GFX engine bring-up — research + roadmap

Status anchor: boot 150355. SMU mailbox is **working** (GetSmuVersion acked,
DisallowGfxOff acked, RLC safe mode entered) after the MP1-seg-1 fix. All 7
staged stages run end-to-end. The remaining gap: the **GFX command processor (CP)
ring** — `CP_RB0_BASE/CNTL/WPTR` written then read back `0`. This doc maps
*everything* still needed to make the GFX (and SDMA) engines actually execute our
commands, harvested from upstream `gfx_v11_0.c` / `imu_v11_0.c` / `gc_11_0_0_*`.

## 1. The authoritative PSP-load bring-up order (`gfx_v11_0_hw_init`)

Phoenix is `AMDGPU_FW_LOAD_PSP`. The order for that path:

1. **(skip)** IMU load/setup/start — only the `AMDGPU_FW_LOAD_DIRECT` branch drives
   the IMU. On PSP load the **PSP autoloads + starts the IMU and all GFX ucode**.
2. `gfx_v11_0_wait_for_rlc_autoload_complete` — wait for the PSP to finish.
3. **`gfx_v11_0_config_gfx_rs64`** — start the RS64 CP cores (PFP/ME/MEC). ← the
   step we're missing; detailed in §3.
4. `gfx_v11_0_gfxhub_enable` — **GFX memory hub / GART / GPUVM**. ← the big blocker; §4.
5. `gfx_v11_0_constants_init` — GRBM/GB config.
6. `nbio gc_doorbell_init` — doorbell aperture.
7. `gfx_v11_0_rlc_resume` — RLC enable.
8. `gfx_v11_0_cp_resume` → `cp_gfx_enable` + **`cp_gfx_resume`** (ring program) +
   `cp_compute_resume` (MEC/MES). ← our stage-6 is an incomplete `cp_gfx_resume`; §5.

## 2. KEY FINDING — the IMU is NOT our job on Phoenix

The `/goal` said "research the CP/IMU startup first." Result: **on PSP load we do
NOT drive the IMU.** The PSP starts it; `wait_for_rlc_autoload_complete` confirms.
That the GOP display is already up confirms the IMU/GFX power domain is live. So
"IMU startup" is a non-task for us — the CP not running is NOT an IMU problem.
What we DO need is everything the driver does *after* autoload (§3–§5).

## 3. `config_gfx_rs64` — start the RS64 CP (the immediate next slice)

The gfx11 CP is RS64 (RISC cores), PSP-loaded. Clearing the `CP_ME_CNTL` halt bits
(what stage-6 does today) is NOT enough — the cores need their program-counter
START address set + a pipe reset. Sequence (per `gfx_v11_0_config_gfx_rs64`):

- PFP pipes 0,1: `grbm_select(me=0,pipe,queue=0,vmid=0)`, write
  `CP_PFP_PRGRM_CNTR_START`/`_HI` = ucode start addr (from the fw header).
- Reset PFP pipes: `CP_ME_CNTL` PFP_PIPE0/1_RESET 1→0.
- ME pipes 0,1: same with `CP_ME_PRGRM_CNTR_START`/`_HI`, then ME_PIPE0/1_RESET 1→0.
- MEC pipes 0–3: `grbm_select(me=1,pipe,…)`, `CP_MEC_RS64_PRGRM_CNTR_START`/`_HI`,
  then `CP_MEC_RS64_CNTL` MEC_PIPE0–3_RESET 1→0.

Register dword offsets + BASE_IDX (gc_11_0_0_offset.h), `(base[seg]+off)<<2`:

| reg | off | seg |
|---|---|---|
| GRBM_GFX_CNTL (grbm_select) | 0x0900 | 1 |
| CP_PFP_PRGRM_CNTR_START | 0x1e44 | 0 |
| CP_PFP_PRGRM_CNTR_START_HI | 0x1e59 | 0 |
| CP_ME_PRGRM_CNTR_START | 0x1e45 | 0 |
| CP_ME_PRGRM_CNTR_START_HI | 0x1e79 | 0 |
| CP_MEC_RS64_PRGRM_CNTR_START | 0x2900 | 1 |
| CP_MEC_RS64_PRGRM_CNTR_START_HI | 0x2938 | 1 |
| CP_MEC_RS64_CNTL | 0x2904 | 1 |
| CP_RB_ACTIVE | 0x1f40 | 0 |
| CP_RB0_RPTR_ADDR | 0x1de3 | 0 |
| CP_RB_WPTR_POLL_ADDR_LO | 0x1e8b | 0 |
| CP_RB_VMID | 0x1df1 | 0 |

`CP_ME_CNTL` field shifts (gc_11_0_0_sh_mask.h): PFP_PIPE0_RESET=18, PFP_PIPE1=19,
ME_PIPE0=20, ME_PIPE1=21, PFP_HALT=26, ME_HALT=28. `CP_MEC_RS64_CNTL` MEC_PIPE0_RESET=16.

`grbm_select` = write GRBM_GFX_CNTL with {MEID:pipe:queue:vmid} (soc21_grbm_select).
The ucode START addr comes from `gfx_firmware_header_v2_0.ucode_start_addr_{lo,hi}`
in the pfp/me/mec blobs (we already load them) — parsed daemon-side, supplied to
the gated bring-up so the crate writes no guessed address.

## 4. THE REAL END-TO-END BLOCKER — GART / GPUVM (`gfxhub_enable`)

`cp_gfx_resume` programs `CP_RB0_BASE = ring->gpu_addr >> 8`, where `ring->gpu_addr`
is a **GART (GPUVM) address** — the ring lives in `AMDGPU_GEM_DOMAIN_GTT`. The CP
fetches the ring through GPUVM. **Without GART set up, no ring base is valid and no
engine (GFX *or* SDMA) can fetch its ring** — this is why even a perfect CP startup
won't make the ring execute. `gfx_v11_0_gfxhub_enable` → `gfxhub_v3_0_gart_enable`:
program the GART table base + aperture (MC_VM_FB_LOCATION, GMC page-table base,
VM_CONTEXT0 page-table base/start/end), set VMID0 to the system aperture. This is a
**major subsystem** we have not built. SDMA needs the equivalent via MMHUB.

**Honest consequence:** the GFX ring "just working on the next flash" is NOT
physically achievable in one slice — GART/gfxhub is weeks-class work. The realistic
path is the ordered roadmap in §7, several iron flashes, each gated + self-diagnosing.

Possible shortcut to investigate: the BIOS/GOP already configured *some* VM (the
display scans out). Inheriting/reusing the firmware GART (read the existing
VM_CONTEXT0 base, map our ring pages into it) MAY avoid building GART from scratch.
Unproven; flagged for §7. The read-only `log_gmc_vm_state` diagnostic (commit
66c9171, stage 3) dumps CONTEXT0_CNTL + SYSTEM_APERTURE + the page-table base so
one flash decides inherit-vs-build.

### 4.1 `gfxhub_v3_0_gart_enable` register sequence (harvested, for the build path)

The PURE half (PTE encoding + flat-table build) is implemented + host-KAT'd in
`gart.rs` (PTE = sys-page-addr | VALID|SYSTEM|SNOOPED|R|W|X|MTYPE_UC(bits 50:48)).
The risky half — programming these gfxhub GMC regs on the live (display-driving)
GPU — stays GATED until the §4 inherit-vs-build flash decides. Sequence:

1. `GCMC_VM_FB_LOCATION_BASE/TOP` = fb_start>>24 / fb_end>>24.
2. `GCMC_VM_AGP_BASE`=0, `GCMC_VM_AGP_BOT`=agp_start>>24, `GCMC_VM_AGP_TOP`=agp_end>>24.
3. `GCMC_VM_SYSTEM_APERTURE_LOW`=min(fb_start,agp_start)>>18, `_HIGH`=max(fb_end,agp_end)>>18.
4. `GCMC_VM_SYSTEM_APERTURE_DEFAULT_ADDR_LSB/MSB` = the default page.
5. `GCVM_CONTEXT0_PAGE_TABLE_START/END_ADDR_LO32/HI32` = the GART VA range.
6. `GCVM_CONTEXT0_PAGE_TABLE_BASE_ADDR_LO32/HI32` = the GART table phys addr.
7. `GCVM_CONTEXT0_CNTL`: ENABLE_CONTEXT=1, PAGE_TABLE_DEPTH=0 (flat) + fault fields.
8. `GCMC_VM_MX_L1_TLB_CNTL`: ENABLE_L1_TLB + system-aperture default; then TLB invalidate.

Offsets already resolved (regs.rs gmc_vm_regs + the discovery resolver); fb/agp
ranges come from the GMC carve-out (CONFIG_MEMSIZE) + the BAR layout. Wiring this
is the gated iron step once inherit is ruled out.

## 5. `cp_gfx_resume` — what our stage-6 ring program is missing

Beyond `CP_RB0_BASE/CNTL/WPTR` we currently write, the full sequence adds:
`CP_RB_WPTR_DELAY=0`, `CP_RB_VMID=0`, two-phase `CP_RB0_CNTL` (bufsz/blksz, then
re-write with RPTR_WR enable), `CP_RB0_RPTR_ADDR(_HI)` (GPU writeback addr),
`CP_RB_WPTR_POLL_ADDR_LO/HI`, and crucially **`CP_RB_ACTIVE=1`** (activates the
ring — we never write it). All GPU-addr fields again require GART (§4).

## 6. SDMA (the simpler parallel engine) — also VM-gated

SDMA 6.0 legacy ring (`sdma_v6_0_gfx_resume`) programs `SDMA0_QUEUE0_RB_*` directly
(no MES/RS64), so it's simpler than GFX — BUT it also fetches its ring via the
MMHUB GPUVM, so it shares the §4 blocker. Our "SDMA submitted; fence not posted" is
the same root: no VM ⇒ engine can't read the ring. SDMA is the cheapest "the GPU
executed our command" proof *once VM exists*.

## 7. Roadmap to "it works" (ordered; each slice gated + self-diagnosing)

1. **`config_gfx_rs64`** (§3) — RS64 CP start + complete `cp_gfx_resume` (§5). [this slice]
2. **`wait_for_rlc_autoload_complete`** diagnostic — read RLC bootload status to
   confirm the PSP autoloaded GFX ucode (cheap, informative).
3. **GART/gfxhub** (§4) — the big one. First investigate inheriting the firmware
   GART; else build `gfxhub_v3_0_gart_enable`. THE gate for any ring executing.
4. **GFX ring smoke** — submit a NOP+FENCE on RB0, confirm RPTR advances + fence posts.
5. **SDMA ring** (§6) — CONSTANT_FILL + FENCE, confirm fence posts (simplest engine).
6. **MES** — for the real Mesa/Vulkan queue path (user/kernel queues).
7. **Scanout** — present a buffer WE filled (vs the GOP's) → the Year-1 demo.

## 8. Verification

Every slice keeps the gated/host-KAT discipline: SOC15 offsets from discovery,
`None` until resolved (no guessed MMIO), reaction-mock host tests, and on-iron a
register read-back diagnostic (the pattern that cracked the UC-read, the
flush-window, and the SMU-seg bugs). Boot, wait ~45 s, read the `stage 6 …` lines.

## 9. POST-185829 REFRAME — "is GFX even up?" + the two-branch playbook

Boot 185829 disproved the SMU `EnableGfxImu` wake (`resp None`, no ack). Source
confirms why: in `imu_v11_0_start`, `amdgpu_dpm_set_gfx_power_up_by_imu` (the
EnableGfxImu message) is called **only when `load_type != PSP`** — a PSP-load APU
like Phoenix never sends it. The PSP autoloads GFX ucode and the IMU brings GFX out
of reset; the GOP framebuffer is the **DCN display** block, which lights up WITHOUT
the GFX/compute engine. So the real question is binary: *did the PSP bring GFX up?*

**The fork-resolver (landed, stage 6):** read the two authoritative status regs —
`RLC_RLCS_BOOTLOAD_STATUS` bit31 (`BOOTLOAD_COMPLETE`, gc_11_0_1 off `0x4e7e` seg1)
and `GFX_IMU_GFX_RESET_CTRL & 0x1f == 0x1f` (`0x40bc` seg1) → `regs::gfx_is_up()`.
Logs `VERDICT: GFX is UP|DOWN`.

### Branch A — VERDICT: GFX is UP  (remaining work, in order)
The CP-write drops were just the skipped PSP-path `rlc_resume` (now: `enable_srm`
landed). Complete `cp_gfx_resume` to upstream exactly (all GC seg0 unless noted):
1. `CP_RB_WPTR_DELAY = 0`
2. `CP_RB_VMID = 0`  *(done)*
3. pipe-switch to PIPE_ID0 (srbm/`GRBM_GFX_CNTL`, seg1) before touching RB0
4. `CP_RB0_CNTL = BUFSZ | (BUFSZ-2)<<8`  *(done)*
5. `CP_RB0_WPTR = 0`, `CP_RB0_WPTR_HI = 0`  **(init to 0 first; kick later)**
6. `CP_RB0_RPTR_ADDR(_HI)` + `CP_RB_WPTR_POLL_ADDR_LO/HI` writeback  *(done)*
7. **`mdelay(1)` then RE-WRITE `CP_RB0_CNTL = tmp`** (the documented double-write)
8. `CP_RB0_BASE/BASE_HI = ring>>8`  *(done)*  then `CP_RB_ACTIVE = 1`  *(done)*
9. `cp_gfx_set_doorbell` (doorbell index/enable) — for the real submit path
Then submit NOP+RELEASE_MEM fence, confirm RPTR advances + fence posts.

### Branch B — VERDICT: GFX is DOWN  (landed: the IMU-core wake attempt)
The ucode is already PSP-loaded, so the wake is just `imu_v11_0_start`: clear
`GFX_IMU_CORE_CTRL.CRESET` (bit0, off `0x40b6` seg1) to release the IMU core, then
poll `GFX_IMU_GFX_RESET_CTRL & 0x1f == 0x1f` (`try_imu_core_start`, host-KAT'd). If
it comes up → fall through to Branch A. If it TIMES OUT → the ucode is NOT loaded
and we need the full DIRECT-load path: `imu_v11_0_load_microcode` (IMU_I/D-RAM
upload) + `imu_v11_0_setup_imu` + `start_imu` — the larger next slice, scoped but
not yet built. The current image ATTEMPTS Branch B automatically on a DOWN verdict,
so the next flash both resolves the fork AND tries the most-likely wake in one shot.
