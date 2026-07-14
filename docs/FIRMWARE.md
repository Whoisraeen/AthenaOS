# Firmware blobs (request_firmware / syscall 142)

The kernel serves device microcode to userspace driver daemons through
`SYS_LINUXKPI_REQUEST_FIRMWARE` (142). `linuxkpi_host::lkpi_request_firmware`
looks the blob up **by its path under `firmware/`** in the initramfs and maps it
read-only into the requesting daemon. xtask bundles **every file under
`firmware/`** automatically (recursive walk — drop a blob in, it ships; no
xtask edit needed).

A driver asks for the same name Linux uses, e.g. amdgpu calls
`request_firmware(&fw, "amdgpu/gc_11_0_1_pfp.bin", dev)` → the kernel serves
`firmware/amdgpu/gc_11_0_1_pfp.bin`.

## Status

The plumbing is DONE and tested (the C shim `request_firmware` in
`components/ath_linuxkpi/src/lib.rs` → syscall 142 → host loader). The amdgpu
Phoenix blobs are **vendored in-tree** (2026-06-12) at `firmware/amdgpu/`
together with `LICENSE.amdgpu`, whose redistribution grant covers exactly this
(binary-form copies). Source of truth for refreshes:
<https://gitlab.com/kernel-firmware/linux-firmware> (the kernel.org cgit
frontend bot-blocks automated downloads — use the GitLab mirror). Ledger entry:
`docs/THIRD_PARTY_LICENSES.md`.

`firmware/athena-selftest.bin` is a tiny in-tree test blob proving the path
end-to-end; keep it.

## Athena target: AMD Radeon 780M (Phoenix, GC 11.0.1) — `firmware/amdgpu/`

amdgpu loads these at init (PSP → SMU → GFX/SDMA/VCN). Minimum set for modeset +
GFX on Phoenix (names as amdgpu requests them):

| File | Stage |
|------|-------|
| `amdgpu/psp_13_0_4_toc.bin` | PSP TOC |
| `amdgpu/psp_13_0_4_ta.bin` | PSP trusted apps |
| `amdgpu/gc_11_0_1_imu.bin` | GFX IMU (required on GFX11 APUs) |
| `amdgpu/gc_11_0_1_pfp.bin` | GFX prefetch parser |
| `amdgpu/gc_11_0_1_me.bin` | GFX micro-engine |
| `amdgpu/gc_11_0_1_mec.bin` | GFX compute |
| `amdgpu/gc_11_0_1_rlc.bin` | GFX RLC |
| `amdgpu/gc_11_0_1_mes_2.bin` | MES scheduler pipe 0 |
| `amdgpu/gc_11_0_1_mes1.bin` | MES scheduler pipe 1 |
| `amdgpu/sdma_6_0_1.bin` | SDMA copy engine |
| `amdgpu/dcn_3_1_4_dmcub.bin` | display DMCUB |
| `amdgpu/vcn_4_0_2.bin` | video (optional for modeset) |

There is **no `smu_13_0_4.bin`** — on APUs the SMU/PMFW image is embedded in the
system BIOS and the PSP bootloader loads it from there; linux-firmware only
ships SMU blobs for dGPUs. (An earlier revision of `FW_PHOENIX` listed it and
would have preflighted `11/12 absent` forever.)

## VBIOS — `firmware/vbios/<vvvv>-<dddd>.bin` (machine-captured, not linux-firmware)

APUs have no PCI expansion ROM; the VBIOS comes from the ACPI **VFCT** table.
Athena's real image is extracted from the captured table and vendored at
`firmware/vbios/1002-15bf.bin`; `bringup::read_vbios` falls back to it via
`request_firmware` when the ROM map fails. See `firmware/vbios/README.md` and
`atombios::parse_vfct` (with a real-table host KAT).

(Exact GC/PSP/SMU version suffixes track the silicon stepping — confirm against
`dmesg | grep amdgpu` on a Linux boot of the same Athena box, which prints every
`Loading firmware ... amdgpu/<file>` line. That dmesg list is the authoritative
per-machine set.)

## Wi-Fi — Athena reality check (2026-06-12)

**Athena's actual Wi-Fi chip is a MediaTek MT7902 (`14C3:7902`)** — verified
from Windows on the box itself (`docs/ATHENA_GROUND_TRUTH.md`). The MT7902 has
**no blob in linux-firmware and no mainline Linux driver** (mt76 covers
7921/7922/7925, not 7902), so the userspace-LinuxKPI Wi-Fi plan cannot apply to
this SKU as built. Options, cheapest first:

1. **Ethernet-first** (current Phase 2.2 path): RTL8125 is the link; no blob
   required. (linux-firmware has optional `rtl_nic/rtl8125b-2.fw` patch
   microcode, but our in-kernel `rtl8125.rs` has no firmware loader and the
   NIC runs without it — only relevant if a quirk hunt ever points there.)
2. **Swap the M.2 2230 card for an Intel AX210** — makes the iwlwifi plan real:
   then drop the newest `iwlwifi-ty-a0-gf-a0-<NN>.ucode` from linux-firmware
   into `firmware/`.
3. A supported USB Wi-Fi dongle once the USB driver path matures.

## Intel GPU (UHD/Xe) — `firmware/i915/` (if targeting Intel iron)

i915/Xe GuC + HuC, e.g. `i915/<platform>_guc_<ver>.bin`, `i915/<platform>_huc.bin`.

## Verifying

After dropping blobs and `cargo run -p xtask --release -- build --release`:
- xtask prints `[xtask] Firmware blobs bundled: N`.
- `tar -tf kernel/src/initramfs.tar | grep firmware/` lists them.
- On boot, a daemon's `request_firmware("amdgpu/...")` returns the blob instead
  of `absent` — amdgpud's `firmware_preflight` flips from `amdgpu_blobs=0/12` to
  `12/12 present` (serial sentinel `9062` = 9050 + present count).

NOTE: end-to-end firmware load on hardware is currently blocked upstream by the
daemon-chain resume bug (user_init must survive its spawn loop to launch
amdgpud) — see the scheduler resume issue. The firmware *wiring* is ready now;
it activates the moment the chain runs.
