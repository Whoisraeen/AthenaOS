# amdgpu PSP-load enable-only path — the post-first-light refactor reference

**Date:** 2026-06-29 · **ASIC:** Phoenix1 (gfx_11_0_1, Radeon 760M), `AMDGPU_FW_LOAD_PSP`, CP = **F32** (v1 ucode).

**Context:** First light is achieved + committed (`95e60fb`): `EnableGfxImu(0x16,1)` sent right after `AUTOLOAD_RLC` → `RLC_BOOTLOAD_STATUS=0xc000001f`, `GFX_IMU_GFX_RESET_CTRL=0x1f`. The blocker now is that GFX goes back *down* (`RESET_CTRL 0x1f→0x10`) during the post-first-light ring setup, because RaeenOS redundantly driver-loads firmware the PSP already loaded. This doc is the source-of-truth for the cleanup.

## Ground truth — amdgpu `gfx_v11_0_hw_init`, the `AMDGPU_FW_LOAD_PSP` branch

After `gfx_v11_0_wait_for_rlc_autoload_complete()` (= our first light), the driver does **zero ucode loads**:

```
wait_for_rlc_autoload_complete()          # = first light (done)
get_gb_addr_config()
if rs64_enable: config_gfx_rs64()          # F32 Phoenix SKIPS
gfx_v11_0_gfxhub_enable()                  # GART / VMID0
gfx_v11_0_init_golden_registers()
# (DIRECT only: amdgpu_pm_load_smu_firmware) — SKIP on PSP
gfx_v11_0_constants_init()
# (non-PSP only: select_cp_fw_arch) — SKIP on PSP
gc_doorbell_init()
gfx_v11_0_rlc_resume()
gfx_v11_0_tcp_harvest()
gfx_v11_0_cp_resume():
    if load_type == DIRECT:                # SKIP on PSP — NO cp ucode load
        cp_gfx_load_microcode(); cp_compute_load_microcode[_rs64]()
    cp_set_doorbell_range()
    if async_gfx_ring: cp_compute_enable(true); cp_gfx_enable(true)
    amdgpu_mes_kiq_hw_init()               # KIQ — see mes_v11_0 below
    kcq_resume()
    cp_gfx_resume() / cp_async_gfx_ring_resume()   # program the gfx ring (NO ucode)
get IMU version; irq setup
```

## `mes_v11_0_enable` — the key: NO IC-base load on PSP

`mes_v11_0_enable(enable=true)` (mes_v11_0.c ~951):
1. reset MES pipes: `CP_MES_CNTL` `MES_PIPE0_RESET=1`, `MES_PIPE1_RESET=enable_mes_kiq`
2. for each pipe: `soc21_grbm_select(3,pipe,0,0)` then
   `CP_MES_PRGRM_CNTR_START[_HI] = adev->mes.uc_start_addr[pipe] >> 2`
   — **the ucode entry point from the fw header**, NOT an IC-base copy
3. unhalt + activate pipe0 (`CP_MES_CNTL` active bits)

It does **not** set `CP_MES_IC_BASE`. On PSP load the MES IMEM is already populated and `IC_BASE` set **by the PSP**; the driver only points the PC at the entry and unhalts. `mes_v11_0_load_microcode` (the IC-base streamer) is **`AMDGPU_FW_LOAD_DIRECT` only**.

## RaeenOS divergences (what to remove on the first-light / PSP path)

All in `components/raeen_amdgpu/src/bringup.rs::init_rings` (stage 6):

1. **Backdoor IMU bring-up** (`program_rlc_ram` + autoload buffer + `imu_load_microcode` + `setup_imu` + `try_imu_core_start`) — **already gated off** on `first_light` (commit 95e60fb). amdgpu runs none of it on PSP.
2. **MES IC-base direct-load** (~2845 `mes_load`, ~2874 `mes_kiq_load`, ~3543 `build_mes_load_sequence` writing `CP_MES_IC_BASE/MDBASE`). The `psp_loaded` flag at **3578 is hardcoded `false`** from the stale H2 conclusion ("PSP rejects MES, 0xffff0006") — which NIGHT 5 FIXED (types 33/34/81/82 now accepted, no rejects in the netlog). The dead `if psp_loaded {…}` ENABLE-ONLY branch (3583) is the correct path.
3. **CP F32 ucode** — verify RaeenOS isn't direct-loading CP ucode on PSP either (amdgpu skips it).

## Refactor plan (incremental — do NOT big-bang; first light is fragile)

- **Step 0 (diagnostic, cheap):** instrument `RESET_CTRL` after each step in the 2720→3110 window (HOLD-GFX-AWAKE → MES dma-write → KIQ → SDMA → queue alloc → GART → probe) to pinpoint the EXACT op that drops GFX `0x1f→0x10`. The drop is currently *before* the IC-base write, so the enable-only flip alone may not fix it. Also fix the `SetSoftMin` gfxclk that returned `None` (min clock unpinned → GFX may idle-gate).
- **Step 1:** flip the MES to ENABLE-ONLY (`psp_loaded` = "CP_MES_IC_BASE non-zero" i.e. PSP set it), taking the dormant 3583 branch — no `IC_BASE` overwrite.
- **Step 2:** decouple the queue setup (`set_hw_resources`, MQDs, rings) from `mes_load.is_some()` so it runs on the PSP-loaded MES (not just the direct-loaded one).
- **Step 3:** drop the redundant MES/KIQ ucode dma-writes entirely once enable-only is proven.
- **Verify each step on iron:** `GFX-up probe` must read `reset_done=true` after the MES setup (not 0x10), then chase `set_hw_resources` ack on a GFX that stays up.

## Iron loop reminders

netlog python reassembler now writes per-boot files (`<out>.boot<id>.txt`). Deploy = scp safe.img → losetup -fP → cp `kernel-x86_64` to BOTH `/boot/` and `/boot/EFI/RAEEN/` → `efibootmgr --bootnext 0003` → cold cycle; 480s auto-return brings it back (self-recovers unless a stuck-MMIO hard-hang). Sources fetched to Athena `/tmp` (gfx_v11_0.c, mes_v11_0.c, imu_v11_0.c).
