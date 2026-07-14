# Working amdgpu GFX/IMU register + firmware targets — Athena Phoenix 2026-06-27

Read from the LIVE working amdgpu (idle desktop) via umr + amdgpu_firmware_info.
These reframe the GFX cold-start diagnosis.

## Working register values (idle / GFXOFF-gated)
```
gfx1101.regGFX_IMU_GFX_RESET_CTRL  = 0x00000000   # NOT 0x1f — our "==0x1f => up" check is WRONG
gfx1101.regRLC_RLCS_BOOTLOAD_STATUS = 0x00000000  # our autoload poll expects non-zero; working reads 0 too
gfx1101.regGFX_IMU_CORE_CTRL       = 0x00000008   # identical to ours
gfx1101.regGFX_IMU_CORE_STATUS     = 0x00000003   # CBUSY=1 + PWAIT_MODE=1 = IMU RUNNING (idle/power-wait)
```
**Takeaway:** `GFX_IMU_GFX_RESET_CTRL` and `RLC_RLCS_BOOTLOAD_STATUS` read 0 on a
working-but-idle (GFXOFF) GPU, so they are NOT reliable "GFX is up" indicators.
The reliable one is `GFX_IMU_CORE_STATUS.CBUSY` (the IMU core busy bit). Our
bring-up declares "GFX is DOWN" off the wrong registers; the CP-ring-write-reads-0
symptom is GFXOFF power-gating, not a dead engine.

## Working firmware set (amdgpu_firmware_info)
```
RLC   fw 0x8b      RLCP fw 0xf
IMU   fw 0x0b012d00   (== the IMU fw_version we load — same firmware)
SDMA0 fw 0x18
MES_KIQ fw 0x109   MES fw 0x88
```
**vs our LOAD_IP_FW set** (amdgpud::build_gfx_fw_blobs): IMU_I/IMU_D, RLC_IRAM/
DRAM/P/G, CP_PFP/ME/MEC = 9 blobs. **MISSING: MES + MES_KIQ** (the working driver
loads both); SDMA omitted (PSP rejects type 9/71). The gfx11 RLC autoload chain
brings up RLC->CP->MES, so a missing MES is a candidate reason the autoload
(RLC_BOOTLOAD) never reports complete on our side.

## Next code increments (now safely iterable — EnableGfxImu wedge is gated off)
1. Add MES + MES_KIQ (GFX_FW_TYPE_RS64_MES=76 + the KIQ type) to the PSP
   LOAD_IP_FW set in amdgpud::build_gfx_fw_blobs (extract the mes_v11 ucode pieces).
2. Fix the GFX-up detection: read GFX_IMU_CORE_STATUS.CBUSY (IMU alive) instead of
   GFX_IMU_GFX_RESET_CTRL==0x1f / RLC_BOOTLOAD-complete. Probe CORE_STATUS in our
   bring-up to confirm whether OUR IMU is running.
3. The CP-ring-writes-read-0 is GFXOFF gating — hold DisallowGfxOff (un-gate) across
   the CP ring programming (re-validate now that the wedge is gone).
