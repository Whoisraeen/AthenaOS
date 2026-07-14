# GFX/IMU root cause — the working driver loads the IMU via PSP, NOT MMIO (2026-06-27)

**Major strategic redirect, proven by a comparative hardware trace on the live working
amdgpu (Athena Arch host, Phoenix1 `1002:15bf`).** This invalidates the entire
"find the missing GFX_IMU init register" line of attack — there isn't one.

## Method (reproducible)
1. Toggle Athena to **amdgpu-reference mode**: `/etc/kernel/cmdline` drop
   `module_blacklist=amdgpu vfio-pci.ids=1002:15bf`; add modprobe.d `blacklist amdgpu`
   (SOFT — unbound at boot, still `modprobe`-able); disable `raeen-vfio.conf`;
   `mkinitcpio -p linux`; reboot. (Backed up as `cmdline.vfio.bak` +
   `raeen-vfio.conf.disabled`; restore = reverse + reboot.)
   **NEVER `modprobe -r/-modprobe` rebind the APU — it wedges. Soft-blacklist + a
   single cold `modprobe amdgpu` gives a TRUE cold init without the wedge.**
2. Arm a deferred kprobe on every MMIO write, then cold-bind:
   ```
   echo 'p:wreg amdgpu:amdgpu_device_wreg reg=%si:x32 val=%dx:x32' > /sys/kernel/tracing/kprobe_events
   echo 1 > /sys/kernel/tracing/events/kprobes/wreg/enable
   modprobe amdgpu            # the cold bind — full init captured
   cat /sys/kernel/tracing/trace > /tmp/linux_wreg_init.log
   ```
3. Warm reference (GFXOFF defeated): `umr -go 0 -r '*.*.regGFX_IMU_*'`.

Artifacts committed: `linux_wreg_init-20260627.log` (the 6858-write cold trace),
`linux_gfx_imu_warm-20260627.txt`.

## The warm reference (working IMU, the target state)
| reg | working value | note |
|---|---|---|
| `GFX_IMU_CORE_STATUS` | **0x03** | CBUSY=1 RUNNING (AthenaOS gets 0x02 = PWAIT, not running) |
| `GFX_IMU_CORE_CTRL` | 0x08 | bit3 set, CRESET(bit0) clear |
| `GFX_IMU_GFX_RESET_CTRL` | 0x1f | GFX fully out of reset |
| `GFX_IMU_I_RAM_ADDR` | 0xffffffff | **reset default — the I-RAM was NEVER MMIO-written** |
| `GFX_IMU_D_RAM_ADDR` | 0x163c | |

## The decisive evidence
- **Total MMIO writes during a full cold amdgpu init = 6858.** The IMU ucode alone is
  132352 B = ~33k dwords. If amdgpu streamed it via `GFX_IMU_I_RAM_DATA` (as AthenaOS
  does), ONE offset would show ~33k writes. The **max writes to any single offset = 780**.
  There is no ucode-streaming burst anywhere → **amdgpu does not MMIO-load any firmware.**
- **amdgpu writes ZERO times to EVERY GFX_IMU control register** — verified per raw seg1
  offset: CORE_CTRL `0x40b6`=0, GFX_RESET_CTRL `0x40bc`=0, D_RAM `0x40fc/0x40fd`=0,
  I_RAM `0x5f90/0x5f91`=0, RLC_BOOTLOADER `0x5f81/0x5f82/0x5f83`=0. The final GFX-up
  writes are all GC ring regs (`0x5023/0x4ff*/0x5049/0x3ae0`), never GFX_IMU.
- The PSP mailbox (MP0 `C2PMSG`) **is** in the trace at dword `0x16080+` (byte `0x58200`,
  matching AthenaOS's `cmd@0x58200`). So the firmware load is the PSP doing it.

## Conclusion → the real gate
The PSP **loads and starts** the IMU (and RLC/CP/MES/SDMA) via autoload. amdgpu never
touches GFX_IMU over MMIO. **AthenaOS's `bringup.rs` IMU MMIO direct-load (I-RAM/D-RAM
streaming + CORE_CTRL/RESET pokes) is a dead-end workaround that fights the hardware.**
The IMU sits at CORE_STATUS=0x02 (PWAIT) because AthenaOS's **PSP autoload is broken** —
the `imu-not-executing` trace shows `PSP LOAD_IP_FW(type 76-79) REJECTED status 0xffff0006`
and `RLC_BOOTLOAD_STATUS` never leaving 0 — so the PSP never brings GFX up, and the MMIO
fallback can't substitute for it.

## Next step (precise, no more guessing)
Diff **AthenaOS's PSP `LOAD_IP_FW` sequence vs amdgpu's** — both are MMIO-visible in this
trace (the `C2PMSG` writes at dword `0x16080+`). Specifically:
1. Map amdgpu's PSP command sequence from the trace: every write to the C2PMSG block
   (`0x16080..0x162xx`), with values, in order = the working LOAD_TOC → SETUP_TMR →
   LOAD_IP_FW ×N → AUTOLOAD_RLC handshake.
2. Find which fw types amdgpu's PSP loads and in what order, the exact cmd-buffer layout,
   and what makes `RLC_BOOTLOAD_STATUS` go `0xc000001f`.
3. Fix AthenaOS's PSP autoload (`amdgpud`/`bringup.rs` PSP path) to match — the 0xffff0006
   rejection is the first thing to resolve (likely a TMR/cmd-buffer/fw-type mismatch).
4. Once the PSP autoload completes, the IMU comes up on its own (CORE_STATUS→0x03) and the
   GFX_IMU MMIO code can be deleted. THEN re-test the CP/MES ring (which was the prior gate).

The GFX_IMU MMIO direct-load code in `bringup.rs` should be retired once the PSP path
works — it is not how this ASIC starts GFX.

## PINPOINTED (impl session, 2026-06-27) — the gate is MES/SDMA LOAD_IP_FW auth failure

Code-level diff of AthenaOS's PSP path against amdgpu + the trace, narrowed to the exact bug:

- **The PSP ring wptr stride is CORRECT** — AthenaOS `PSP_RB_FRAME_DWORDS=16` (`0x10`),
  matching the trace's `C2PMSG_67` advancing `0x10..0x1b0` (27 frames). Not the bug.
- **The `LOAD_IP_FW` cmd structure is amdgpu-byte-exact** — `psp_gpcom_cmd` puts
  fw_addr_lo/hi/size/type at dwords 7/8/9/10 (byte +28/+32/+36/+40), = `struct
  psp_gfx_cmd_load_ip_fw`. Not the bug.
- **The response-status read is amdgpu-byte-exact** — `PSP_RESP_STATUS_DWORD=216`
  (byte 864), `tmr_size@220` (byte 880), = `psp_gfx_cmd_resp.resp` @ +864 in
  psp_gfx_if.h. So the `0xffff0006` (MES 76-79) / `0xffff0010` (SDMA 71) statuses are
  **REAL PSP responses, not misreads.**
- **Therefore the rejection is a PSP firmware-AUTHENTICATION failure**: IMU/RLC/CP blobs
  authenticate (status 0), MES/SDMA blobs do NOT. The bytes AthenaOS hands `LOAD_IP_FW`
  for MES (`extract_mes_ucode_data`) and SDMA are the wrong signed region — the PSP
  signs/validates a specific span and AthenaOS extracts a different one.
- amdgpu's autoload **requires** MES + SDMA in the set (gfx_v11_0 ucode list +
  WebFetch), so the autoload (`RLC_BOOTLOAD_STATUS`) cannot reach `0xc000001f` while
  those two are rejected.

**FIVE candidate bugs ELIMINATED with code-level proof (all amdgpu-byte-exact):**
1. PSP ring wptr stride — `PSP_RB_FRAME_DWORDS=16` = trace `0x10`/frame. ✓
2. `LOAD_IP_FW` cmd struct — addr_lo/hi/size/type @ dwords 7/8/9/10. ✓
3. Response-status read — `status@dword216`/`tmr@220` = `psp_gfx_cmd_resp.resp@+864`. ✓
4. Blob span — `extract_mes_ucode_data` reads offsets 36/40/48/52 = `mes_firmware_header_v1_0`
   `mes_ucode_{size,offset}` + `mes_ucode_data_{size,offset}`. ✓
5. fw_type IDs — RS64_MES/STACK/KIQ/KIQ_STACK=76/77/78/79, SDMA_TH0/TH1=71/72,
   IMU_I/D=68/69, RLC_G=8, CP=1/2/4 — all verified against current psp_gfx_if.h. ✓

So the MES(0xffff0006)/SDMA(0xffff0010) rejection is **NOT a static encoding bug.** The
PSP authenticates IMU/RLC/CP (same machinery) but rejects MES/SDMA. The remaining,
iron-disambiguable causes:
- **Staging address** — where AthenaOS stages each blob before `LOAD_IP_FW` (`fw_pri` MC
  addr / alignment); MES is large — verify the staging buffer + that the MC addr the PSP
  authenticates from is correct for the bigger blobs.
- **Load phase / order** — amdgpu's `mes_v11_0_hw_init` runs AFTER `gfx_v11_0_hw_init`;
  the PSP may only accept MES once GFX is up. Test: load IMU+RLC+CP, AUTOLOAD_RLC to first
  light, THEN load MES/SDMA.
- **PSP TA / sub-state** — 0xffff0006 vs 0xffff0010 are distinct PSP statuses; decode them
  from the PSP TA error space.

**Next = the iron patch→boot→read loop (Phase 3), one hypothesis per boot**, starting with
the load-phase reorder (highest-leverage, matches amdgpu's IP-init ordering). The five
verified-correct items above do NOT need re-checking.
