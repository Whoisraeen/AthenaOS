# Real amdgpu on RaeenOS — two walls broken, PSP is a passthrough limit (2026-07-08)

**Session result:** the real upstream amdgpu C (Strategy B, `RAEEN_AMDGPU_REAL=1`),
running under the LinuxKPI shim in a VFIO GPU-passthrough KVM guest on Athena
(Phoenix1 `1002:15bf`), was driven **from "dies at device claim" all the way to
PSP `hw_init`** — the deepest the real driver has ever run on RaeenOS. Two walls
were broken; the third (PSP) is proven to be a VFIO-APU-passthrough limitation,
not a code bug.

Harness: `~/raeen-vm/run-vfio.sh` on Athena (Arch host, GPU cold-bound to
vfio-pci via the `arch-linux-vfio` UKI), auto-run by the `raeen-vfio-test`
systemd oneshot, serial captured to `/tmp/raeen-serial.log`. Iron transcripts:
`logs/` (`serial-vfio-cap4` = the furthest run).

## Wall 1 — capability gate (BROKEN)

Commit `5731136` (2026-07-07) gated `sys_claim_device` behind `Cap::System{WRITE}`
to close a sandbox escape, but it also locked out the legitimate driver daemons:
`amdgpud` spawns with an empty CapTable, so the 2026-07-08 run got
`sys_claim_device denied: caller lacks Cap::System{WRITE}` (`ERR_NO_AUTHORITY`,
`0xf50e`) and exited "no AMD GPU found" (a red herring — printed on ANY claim
failure).

**Fix** (`kernel/src/userspace_driver.rs` + `syscall.rs`): `maybe_seed_driver_daemon()`
delegates a `Cap::System{WRITE}` to a freshly-spawned child IFF (a) it is an
allowlisted first-party driver binary (`amdgpud`/`i915d`/`nvidiad`) AND (b) the
spawning parent already holds `Cap::System{WRITE}`. Capability *delegation*, not
amplification — safe under the (still-ungated) SYS_SPAWN. FAIL-able policy
smoketest asserts the truth table.

**Proof:** `[usdriver] claim granted: driver #6 took device 0x300 (mmio=0x380000000000+0x10000000, 1 IRQs)`.

## Wall 2 — VBIOS acquisition (BROKEN)

Past claim, the real init parsed IP discovery (39 blocks, all CLKA instances —
the old M1 CLKA-#2 wall is gone), brought SMU up, entered `amdgpu_device_init`,
then died at VBIOS: `ACPI VFCT table present but broken (too short #2)` ->
`Unable to locate a BIOS ROM` -> `Fatal error during GPU init`.

Root cause: `amdgpu_acpi_vfct_bios()` (`amdgpu_bios.c`) accepts the VFCT VBIOS
image only when its `VFCT_IMAGE_HEADER` PCIBus/PCIDevice/PCIFunction equal the
live device's `pdev` BDF. The bundled `VFCT.dat` was captured on Athena where the
iGPU is `c4:00.0`, so the image records `PCIBus=0xc4`. Under passthrough the
guest device is `00:03.0`, so the location never matched and the loop fell
through to "too short #2".

Two attempts, one decisive result:
- **cap2/cap3 (failed):** serve a `kmalloc`'d VFCT copy with the image header BDF
  rewritten to the guest BDF (`0:3.0`). Instrumentation confirmed the write
  landed (`realign img_off=76 wrote bus=0 dev=3 func=0 (was PCIBus=0xc4)`), yet
  the match still missed — **amdgpu reads the ORIGINAL firmware mapping, not our
  served copy** (`request_firmware_blob` returns a cached pointer amdgpu also
  holds).
- **cap4 (WORKED):** leave the VFCT unmodified and instead set `pdev`'s BDF to the
  VFCT's *native* recorded location (`vfct_native_bdf()` reads it straight from
  the table → `c4:00.0`). Now the original VFCT and `pdev` agree regardless of
  which buffer amdgpu reads. `pdev`'s BDF is provenance-only (real MMIO/config
  route via the claimed handle + BARs), so a non-guest bus number is safe.

**Proof:** `[drm] ATOM BIOS: 113-PHXGENERIC-001` (the oracle value) — amdgpu
accepted the ATOMBIOS.

Fix files: `components/raeen_linuxkpi/src/lib.rs` (`vfct_native_bdf`),
`amdgpud/src/main.rs` (pass native BDF to bring-up),
`components/raeen_linuxkpi/src/drm_bringup.rs` (served-copy alignment +
`set_vfct_bdf` belt-and-suspenders).

## How far cap4 reached (deepest ever)

```
Detected VRAM RAM=2048M, BAR=2048M / RAM width 64bits DDR5
2048M of VRAM ready / 4096M of GTT ready / PCIE GART of 512M enabled
fence driver on ring gfx_0.0.0 / comp_1.0.0..comp_1.3.1 / sdma0 / mes_kiq_3.1.0 / mes_3.0.0
MES: vmid_mask... gfx_hqd_mask 0x2 compute_hqd_mask 0xc sdma_hqd_mask 0xfc
```
GMC, VRAM, GART, and every GFX/compute/SDMA/MES ring fence driver — far past the
old MES `0x7654` halt.

## Wall 3 — PSP hw_init (a passthrough limitation, NOT a code bug)

```
psp gfx command UNKNOWN CMD(0x0) failed and response status is (0x0)
PSP tmr init failed! / psp reg (0x16080) wait timed out, read: 30000 exp: 80000000
Fail to stop psp ring / PSP firmware loading failed
hw_init of IP block <psp> failed -22 -> amdgpu_device_ip_init failed -> Fatal
```

`psp reg 0x16080` = `regMP0_SMN_C2PMSG_64` (byte `0x58200`, matches the live host
amdgpu wreg trace in `IMU-ROOT-CAUSE-PSP-not-MMIO-2026-06-27.md`), so the
register offset is correct. `psp_v13_0_ring_stop` writes `DESTROY_RINGS` and
waits for response bit 31; it reads `0x30000` — the PSP *receives* but never
*acknowledges*.

**Why it can't work under passthrough (decisive, from the PCI topology):**
```
c4:00.0 VGA — Phoenix1 GPU           <- the ONLY function VFIO passes to the guest
c4:00.2 Encryption controller — AMD Phoenix CCP/PSP 3.0 Device  <- the PSP, host-owned
c4:00.1/.5/.6 audio, c4:00.3/.4 USB  <- host-owned siblings
```
- The PSP is a **separate PCI function (`c4:00.2`)** that is not passed to the
  guest; the SoC PSP is host-owned and shared.
- The GPU **has no FLR** (`FLReset-`) — only a *bus* reset, which would also reset
  the host's USB (keyboard), audio, and the CCP. VFIO therefore can't hand the
  guest a cleanly-reset, owned PSP, and a manual bus reset is unsafe.

The host Linux amdgpu inits this exact PSP fine **because it owns all of
`c4:00.*`**. A guest that owns only `c4:00.0` never can. This is an APU-VFIO
limitation, independent of RaeenOS.

## Next step — bare-metal RaeenOS (the real PSP test)

Boot the same `--safe` `RAEEN_AMDGPU_REAL=1` image on Athena **bare metal** (USB
per §9): RaeenOS then owns the whole SoC, the firmware cold-inits the PSP (SOS
loaded), and amdgpu inits its rings against a PSP it fully controls — the same
ownership the working host driver has. The cap-gate and VBIOS fixes carry over
unchanged (bare metal the GPU IS at `c4:00.0`, which is exactly what
`vfct_native_bdf` aligns `pdev` to). Capture via netlog (UDP 51514) + on-ESP
`BOOTLOG.TXT`.

Everything up to and including GMC/GART/rings is proven on real silicon; the PSP
is the one wall that the passthrough harness structurally cannot break.
