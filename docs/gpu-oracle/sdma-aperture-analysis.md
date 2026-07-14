# SDMA bring-up root-cause — the GC vs LSDMA register-aperture split (2026-06-25)

**Method:** the real Linux amdgpu is running live on Athena (Phoenix1 `1002:15bf`,
`c4:00.0`, bound to `amdgpu`, NOT vfio). `umr` (v1.0.11) over SSH reads the WORKING
engine's registers directly. This is the authoritative reference for "what a
running SDMA looks like" — the comparison the SDMA rung-1 work needed.

## The finding: SDMA registers exist in TWO distinct, non-aliased apertures

umr exposes the Phoenix SDMA registers under two IP blocks with DIFFERENT MMIO
offsets, and they are **separate register instances** (proven: same logical
register reads different values in each):

| logical reg | `gfx1101` (GC aperture) | `lsdma601` (LSDMA aperture) |
|---|---|---|
| QUEUE0_RB_CNTL | off 0x12e0, **live = 0x00000000** | off 0x11480, **live = 0x00040015** |
| F32_CNTL | off 0xf89a, **live = 0x00000000** | (no such register) |
| UCODE_ADDR / DATA | off 0xf880 / 0xf881 | (no such register) |
| (engine control) | — | regLSDMA_CNTL off 0x11473, live = 0x00000a80 |
| UTCL1_CNTL | — | off 0x1143c |

The working amdgpu programs the **LSDMA aperture (base 0x11400)** for the ring and
leaves the **GC-aperture SDMA0_* registers at 0** (unused). The LSDMA block exposes
NO `F32_CNTL` and NO `UCODE_ADDR/DATA` — SDMA firmware on Phoenix is loaded by the
**PSP/RLC autoload**, not by MMIO streaming (corroborated by the cold wreg oracle:
only 3226 total `amdgpu_device_wreg` entries — a direct ucode stream would be
~10,600 writes for TH0+TH1 alone; the top offset appears just 195×).

## CAVEAT — reconciled with the prior oracle pass (do not over-read the "=0")

The GC-aperture `regSDMA0_F32_CNTL = 0` and `regSDMA0_QUEUE0_RB_CNTL = 0` reads do
**NOT** prove the GC aperture is the "wrong" engine-control aperture. SDMA
**power-gates when idle, deeper than GFXOFF** (umr can't wake it), so the GC
SDMA0 registers read 0 *because the block is gated*, not because they're unused.
A prior pass caught the live RUNNING value `F32_CNTL = 0x08084600` (vs our
0x08084400 — only TH0_RESET differs), confirming the **GC-aperture F32_CNTL is
the correct engine-enable register** and our value is right. So the engine-enable
aperture is NOT a second bug.

The genuinely useful NEW datum here is diagnostic: the **LSDMA aperture
(0x11400) retains the ring config even while SDMA is power-gated** — `lsdma601.
regLSDMA_QUEUE0_RB_CNTL = 0x00040015` reads back when the GC view reads 0. So to
inspect a gated SDMA's ring state, read the `lsdma601` block, not `gfx1101`.

## Why RB_RPTR stays 0 (the established root cause)

The ring regs resolve correctly to the LSDMA aperture (our 0x80 + base 0x11400 =
0x11480 = `regLSDMA_QUEUE0_RB_CNTL`), and the F32_CNTL/UTCL1/threads are all
correct. The real blocker is **firmware**: the RS64 SDMA F32 has no working
firmware, so it never executes (no ring fetch → RB_RPTR = 0). Confirmed across
many iron runs (see memory `amdgpu-iron-hang-uc-firmware-read`):
- amdgpu loads ALL gfx11 firmware (RLC/CP/SDMA/MES) via **PSP/RLC-backdoor DMA
  autoload**, NOT MMIO streaming (the cold mmiotrace has no ~10k-write ucode
  burst; this analysis's own finding — only 3226 total wreg, top offset 195×).
- This Phoenix's PSP **rejects** SDMA via individual `LOAD_IP_FW` (type 71/72 →
  status 0xffff0010 / 0x11), so SDMA is never in the TMR the autoload distributes.
- Our MMIO BROADCAST direct-load (the only SDMA load that runs) does not boot the
  RS64 on this PSP-autoload part. Gating it off leaves RB_RPTR=0 either way →
  the engine has no firmware regardless.

## Working reference values (live, idle engine)

```
lsdma601.regLSDMA_QUEUE0_RB_CNTL = 0x00040015   # RB_ENABLE | RB_SIZE=10; NO F32_WPTR_POLL(bit11), NO RB_PRIV(bit23)
lsdma601.regLSDMA_CNTL           = 0x00000a80   # engine-side control (bits 7,9,11 set)
gfx1101.regSDMA0_F32_CNTL        = 0x00000000   # GC-aperture F32_CNTL UNUSED by the working driver
gfx1101.regSDMA0_QUEUE0_RB_CNTL  = 0x00000000   # GC-aperture ring UNUSED
```

Note our code targets `RB_CNTL = RB_ENABLE | size | F32_WPTR_POLL | RB_PRIV` (the
"0x841817" from a prior capture). The current live IDLE value is 0x00040015 (no
poll/priv) — reconcile: 0x841817 may be an active-submission or older-driver
snapshot. RB_SIZE matches (10).

## The fix (already scoped in memory): the (C) RLC-backdoor autoload path

The decisive fix is to load SDMA firmware the way amdgpu does on this part:
switch RaeenOS's firmware load from the PSP individual-`LOAD_IP_FW` path to
**AMDGPU_FW_LOAD_RLC_BACKDOOR_AUTO**, where the IMU/RLC loads RLC+CP+SDMA+MES from
the autoload buffer (which already includes SDMA — `rlc_autoload.rs::
build_autoload_buffer` is host-KAT'd with TH0/TH1). The toggle is staged as
`docs/gpu-oracle/backdoor-toggle.patch` (a 1-const `FW_LOAD_RLC_BACKDOOR=true` in
`amdgpud/main.rs` + gating the PSP fw-load step 3b). Test via the cold-vfio KVM
loop (`athena-kvm-vfio-noflash-loop`), watching IMU-core wake / RLC_BOOTLOAD /
RB_RPTR.

Secondary cleanups worth folding in when the engine runs (amdgpu does them in
`gfx_resume` and we skip): **RB_RPTR_ADDR writeback** + **IB_CNTL.IB_ENABLE**.
Neither boots the engine, but both are real gfx_resume steps.

## Reproduce (no flash; amdgpu live on Athena)

```
ssh whoisraeen@192.168.1.244
sudo umr -i 1 -lr phoenix.lsdma601                 # list LSDMA regs + offsets
sudo umr -i 1 -O bits -r phoenix.lsdma601.regLSDMA_QUEUE0_RB_CNTL
sudo umr -i 1 -r phoenix.gfx1101.regSDMA0_F32_CNTL # GC-aperture (unused, =0)
```
(If amdgpu is later re-blacklisted for cold-vfio bring-up, revert the cmdline to
let amdgpu bind before umr can read the live engine — see the vfio memory.)
