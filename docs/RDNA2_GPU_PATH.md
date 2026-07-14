# RDNA2 (RX 6600) GPU path — the fast road to real 3D

**Status:** Plan / not started. Hardware-gated (needs a discrete RDNA2 card + a host
to seat it). This is the recommended route to the Concept **Phase 6 / Year-1 "real
GPU submit"** deliverable, in parallel with — or instead of — grinding the Phoenix
APU. See [`linuxkpi-drm/M5-BAREMETAL-PLAN.md`](../linuxkpi-drm/M5-BAREMETAL-PLAN.md)
for the Strategy-B (real upstream `amdgpu` via LinuxKPI) foundation this builds on.

---

## 1. Why RDNA2, and why now

The Phoenix APU (Athena's integrated Radeon 760M, gfx11) is where the real `amdgpu`
driver bring-up currently stands. As of 2026-07-04 it runs **deep** on real silicon
(iron-proven): past early-init, through VBIOS parse, GMC, VRAM/TTM, GART, and **every
ring's fence driver**, stalling only at **PSP firmware load**. But three things about
the APU make it a punishing dev target — and **a discrete RDNA2 card deletes all three
by architecture, not by us out-engineering them:**

| Wall on the Phoenix APU | On a discrete RX 6600 (RDNA2) |
|---|---|
| **SoC self-resets at GPU power-up** — the GPU shares the CPU's power/reset domain; powering up GFX can reboot the whole machine. Not fixable in software. | **Gone.** A discrete card is its own power domain. GPU power-up does nothing to the CPU. |
| **APU won't cold-reset over the wire** — once a boot reaches PSP, the APU is left dirty and *warm* reboots don't clear it; the daemon then can't even re-claim it (`usdriver claim failed 0x503`). Only a physical power-cycle resets it. (Hit live 2026-07-04.) | **Clean FLR / bus reset.** Discrete cards reset properly every run → a fast, repeatable, *remote* test loop. No power-cycle roulette. |
| **UMA carveout VRAM** — VRAM is stolen from system RAM; CPU-written buffers and the GPU's DMA view must be kept coherent by the still-in-progress TTM/`mm.rs` path. **This is the current PSP blocker** (the `LOAD_TOC` command buffer isn't coherent between the CPU write and the PSP's DMA read). | **Dedicated VRAM + a standard BAR aperture.** No carveout-coherency puzzle — the exact thing blocking PSP today simply isn't present. |
| **gfx11 MES firmware scheduler halts** (the original `0x7654` wall the whole C-driver strategy exists to bypass). | **KIQ-based queue mapping** (RDNA2's `gfx10.3` path), which AthenaOS has *already proven*. The finicky MES scheduler isn't on the critical path. |

**Bottom line:** on the APU we're defusing a bomb wired into the CPU. On a discrete
card the bomb isn't there. Proving 3D on *any* AMD GPU unblocks the entire AthGFX
submit pipeline (compositor → wgpu/Vulkan-equivalent → games); the APU then becomes a
"make the fragile one behave later" problem instead of a total blocker.

---

## 2. What carries over from the Phoenix work (most of the hard part is done)

The multi-week investment in Strategy-B is **largely GPU-family-agnostic** and comes
along for free:

- **The LinuxKPI shim** (`components/ath_linuxkpi/`, ~1400+ C-ABI symbols; the real
  upstream `amdgpu` compiles + links against it) — the genuinely hard, done part.
- **The reloc fix** (`c0e350d`: `-fvisibility=hidden` + `-Bsymbolic`) — link-level,
  totally family-independent. Free.
- **`SYS_LINUXKPI_MAP_PHYS` (144)** and the device-access facade (PCI cfg / `ioremap`
  / `dma_alloc_coherent` / BAR claim) — all family-independent.
- **The daemon entry** (`amdgpud` → `rae_amdgpu_device_init` → real `amdgpu_device_init`)
  and the `ATHENA_AMDGPU_REAL=1` build path in `xtask`.
- **The proof loop** — VFIO passthrough capture + persistent serial (works today), and
  on a discrete card it becomes *repeatable* (clean FLR).

What is **new work** is bounded and compile-shaped, not reverse-engineering.

---

## 3. Hardware shopping list

**GPU — any RDNA2 (gfx10.3.x). The RX 6600 is the sweet spot:**

| Card | Chip | IP (gfx) | Notes |
|---|---|---|---|
| **RX 6600 / 6600 XT** (recommended) | Navi 23 | gfx10.3.4 | cheap, common, low power (no aux connector on plain 6600), well-documented |
| RX 6700 / 6700 XT | Navi 22 | gfx10.3.1 | more VRAM/perf if you want headroom |
| RX 6800 / 6900 | Navi 21 | gfx10.3.0 | overkill for bring-up |
| RX 6400 / 6500 XT | Navi 24 | gfx10.3.6 | smallest/cheapest; fine for first-light |

Used RX 6600: **~$150–200**.

**A host to seat it (the card needs a real PCIe x16 slot — the UM760 mini-PC has none):**

1. **A cheap used desktop tower with a PCIe x16 slot + a ≥400 W PSU** — *recommended*.
   Most reliable dev vehicle; clean VFIO + FLR; ~$100–200 used. Run Arch (mirror the
   Athena loop) or bare-metal AthenaOS directly.
2. **An eGPU enclosure over USB4/Thunderbolt** — keeps the UM760 as the host (the
   7640HS has USB4), but adds TB/USB4 passthrough quirks and lower bandwidth. Workable,
   not preferred for first bring-up.

**Total: ~$300–400** for card + a tower. One-time.

---

## 4. Software work (bounded, ordered)

Milestones mirror the Strategy-B M-series. Each is compile-and-verify against the
**done** shim, not new invention.

- **R1 — Retarget the object set to gfx10.3.** In `linuxkpi-drm/m4-link.sh`, swap the
  Phoenix IP `.c` files for the Navi 23 set: `gfx_v10_0.c`, `gmc_v10_0.c`,
  `sdma_v5_2.c`, `nbio_v2_3.c`, `mmhub_v2_0.c`, `athub_v2_1.c`, `psp_v11_0.c` (Navi 2x
  PSP), `smu_v11_0` / `sienna_cichlid_ppt`, `ih_v5_0` / `navi10_ih`, plus `nv.c`
  (the Navi `soc`-equivalent of `soc21.c`). Let the on-chip IP-discovery table select
  them (it already drives block selection). Expect a fresh but *small* per-file shim
  delta — the shim is mature.
- **R2 — Confirm the daemon claims + probes the card.** BDF will differ (a real x16
  slot, not `c4:00.0`); the match-by-class probe already handles that. Verify BAR
  sizing + `sys_ioremap` on the discrete BARs (BAR0 = VRAM aperture, BAR2 = doorbell,
  BAR5 = regs).
- **R3 — Run `amdgpu_device_init` to first light.** With dedicated VRAM the GMC/TTM/PSP
  path should clear the carveout-coherency wall that blocks Phoenix today. Watch PSP
  `LOAD_TOC` / `SETUP_TMR` succeed (the discrete VRAM MC addresses are BAR-coherent).
- **R4 — KIQ ring bring-up + a GFX submit.** RDNA2 maps queues via the KIQ/MEC (proven
  in AthenaOS), not the gfx11 MES. Bring up the GFX/compute rings, submit a trivial
  packet, confirm the fence signals.
- **R5 — First triangle → scanout.** A `vkQueueSubmit`-equivalent draw → direct scanout
  on a real panel (modeset + EDID) → the Concept Year-1 "Vulkan demo on real GPU."
- **R6 — Wire to AthGFX / Mesa (RADV lineage) → Proton/Steam.** The gaming payoff.

---

## 5. Acceptance criteria

- **First light (R3):** `amdgpu_device_init` returns 0 on the RX 6600 — a thing the
  Phoenix path has *never* reached (blocked at PSP). Captured via the VFIO/bare-metal
  serial loop.
- **First submit (R4):** a GFX ring packet completes + its fence signals.
- **Year-1 deliverable (R5):** a rendered triangle on the panel via the real submit
  path — closes the open Concept Phase-6 gap (currently a software raster).

Status ladder discipline: `[~]` for QEMU/proxy, `[x]` only with real-card iron
evidence — same bar as everywhere else.

---

## 6. Risks / open questions

- **VFIO on the discrete card:** should be textbook (clean FLR, own reset domain) —
  the opposite of the APU. Verify the tower's IOMMU groups isolate the GPU.
- **Per-file shim delta (R1):** a handful of Navi-specific types/registers may need
  shimming; bounded, given the mature `linux/*.h` + real-`drm/*.h` model.
- **PSP on Navi 2x** uses `psp_v11_0` (not `psp_v13_0`); the *coherency* problem should
  vanish with dedicated VRAM, but the PSP command/ring sequence still needs to work —
  the [[amdgpu-iron-hang-uc-firmware-read]] Rust reimpl + oracle notes transfer.
- **Two GPUs, one box** (if using the tower as a workstation): standard VFIO
  host-GPU/guest-GPU split; or dedicate the tower to bring-up.

---

## 7. First step when a card is in hand

1. Seat the RX 6600 in a tower; boot Arch; confirm `lspci` shows the Navi 23 + set up
   the VFIO loop (mirror `scripts/athena-*` / `~/athena-vm/run-vfio-persist.sh`).
2. Do **R1** (swap the IP `.c` set in `m4-link.sh`) + `FREESTANDING=1 ATHENA_AMDGPU_REAL=1`
   build; run the daemon; iterate the per-file shim delta to a clean link.
3. Boot it → read the serial log → chase `amdgpu_device_init` to **first light (R3)**.

The reloc fix and syscall-144 are already on `main` and apply unchanged. The only
gating input is the hardware.

---

*Cross-refs:* [`linuxkpi-drm/M5-BAREMETAL-PLAN.md`](../linuxkpi-drm/M5-BAREMETAL-PLAN.md),
[`linuxkpi-drm/M5-ONPATH-AUDIT.md`](../linuxkpi-drm/M5-ONPATH-AUDIT.md),
[`docs/LINUX_DRIVER_STRATEGY.md`](LINUX_DRIVER_STRATEGY.md),
[`docs/NATIVE_DRIVER_PLAN.md`](NATIVE_DRIVER_PLAN.md).
