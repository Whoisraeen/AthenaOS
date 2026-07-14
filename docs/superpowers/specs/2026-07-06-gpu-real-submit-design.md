# GPU Real-Submit Campaign — Design Spec

**Date:** 2026-07-06
**Author:** opus (lead), design approved by owner in-session
**Status:** APPROVED — hand-off to implementation plan
**Concept line served:** §AthGFX ("looks like Metal, performs like Vulkan") + §Gaming ("gaming isn't a mode") — the Year-1 "Vulkan demo on a real GPU" deliverable and the substrate for "Steam day one."

---

## 1. Goal and acceptance bar

Deliver **real GPU command submission on Athena** (Radeon 760M, Phoenix1 `1002:15bf`), culminating in an **open-source Linux-native Vulkan game (vkQuake-class) rendering and playable on the panel**.

Owner decisions locked in-session (2026-07-06):

| Question | Decision |
|---|---|
| Proof target | Real-iron submit (not the QEMU virtio-gpu detour) |
| Finish line | A real game renders — vkQuake-class, open-source, Linux-native Vulkan |
| Architecture appetite | Whatever renders a game fastest, decided by evidence (gate experiment) |

**Directive pivot recorded:** the 2026-07-01 owner directive ("native Rust on bare metal, NOT linuxkpi") is superseded by the owner's 2026-07-06 request to pursue whichever route clears the MES wall fastest, explicitly including LinuxKPI (Strategy B) and a driver-VM. This spec is the record of that pivot.

**Explicitly out of scope for this campaign's milestones** (but designed-for NOW — see §10): Steam, AthBridge/Windows games, anti-cheat, compute/video workloads, perf tuning beyond 30 fps, NVIDIA/Intel, multi-monitor. Owner directive 2026-07-06: the GPU stack must be ready for Windows games and "anything we throw at the GPU" — §10 encodes that as forward design constraints on the seam so the follow-on campaigns are additive, not rework.

## 2. Gate experiment — results (2026-07-06)

Run against the live Athena box (Arch, kernel 7.0.12-arch1-1, amdgpu 3.64.0) over SSH. **Zero flashes, zero reboots** — the needed boot pair already existed in the journal.

**Q1 — does stock amdgpu init from the exact AthenaOS handoff state?**
Boot `-1` (Jul 05 01:07) ran the vfio/blacklisted UKI — GPU dark all boot. Boot `0` (01:09:28) warm-rebooted into amdgpu-enabled Arch — the *identical* handoff AthenaOS's iron loop receives ("Arch reboot hands us a dark GPU").

- ✅ `SMU is initialized successfully!`
- ✅ MES v11 configured (`gfx_hqd_mask 0x2, compute_hqd_mask 0xc, sdma_hqd_mask 0xfc`), **no `set_hw_resources` timeout, no "MES failed to respond" anywhere**
- ✅ Ring roster mapped: `gfx_0.0.0`, `comp_1.[0-3].[0-1]` (8), `sdma0`, `vcn_unified_0`, `jpeg_dec`, `mes_kiq_3.1.0`
- ✅ `[drm] Initialized amdgpu 3.64.0`, `fbcon: amdgpudrmfb is primary` — driving the display 1.6 days on that init
- Only blemish: benign `optc314_disable_crtc` REG_WAIT (known-noisy DCN wait)

**Firmware-version hypothesis — ELIMINATED.** All 10 blobs in `firmware/amdgpu/` are **byte-identical** (sha256) to what the working driver loads from `linux-firmware 20260519-1`, including all three MES ucodes (`gc_11_0_1_mes.bin` `4844bb38…`, `mes1.bin` `8f2c0249…`, `mes_2.bin` `31796311…`). The host's `psp_13_0_4_toc.bin.zst` is a symlink to `psp_13_0_11_toc.bin.zst`; its target hash `11e3529d…` matches our vendored TOC. Live versions: **MES 0x88, MES_KIQ 0x109**, ME 0x67, PFP 0x6d, MEC 0x46, RLC 0x8b, IMU 0x0b012d00, SDMA0 0x18, SMC 76.87.0, VBIOS `113-PHXGENERIC-001`.

**Verdict:** the cold handoff is NOT poisoned and the firmware is NOT the cause. The `0x7656` MES pipe0 halt is a **behavior/environment mismatch in our init**. Route A is GO.

**Q2 (stock amdgpu inside a KVM/VFIO guest — gates the driver-VM fallback): DEFERRED.** Costs two remote reboots + APU-wedge risk and only matters if Route A's KMD halts. Bought if and when needed (see §8 fallback).

## 3. Route decision

**Route A — Strategy B KMD + Mesa-under-Linux-ABI UMD.** Chosen because it has the shortest path through already-proven AthenaOS assets:

- Real amdgpu C already **links** into a bare-metal `amdgpud` (M5, `df8c4b3`/`ad7b1de`, `RAEEN_AMDGPU_REAL=1`).
- Dynamically-linked multithreaded glibc Linux ELFs already run on iron (`[x]`: ld.so, clone, futex, prlimit64).
- The DRM uAPI seam is already started (`raeen_amdgpu::uapi`: `AMDGPU_INFO` done; GEM/CS/WAIT_CS named next).
- The DCN scanout path (display) is already `[x]` iron (2026-07-02 milestone) — rendered frames have a proven road to the panel.

**Route B — driver-VM (thin SVM/NPT hypervisor + pinned Linux guest owning the GPU via VFIO + virtio-gpu-class transport): the pre-agreed fallback**, triggered per §8. Route C (continue native-Rust MES register archaeology) is folded into Route A as diagnostics, not a standalone plan; the native driver remains the DCN scanout owner and the halt-dump tooling.

## 4. Architecture (3 layers)

```
┌─ L3: GAME ─────────────────────────────────────────────────────┐
│ vkQuake (Linux glibc ELF) under the Linux-ABI layer.           │
│ SDL2 video/input via RaeWSI shim: swapchain = N shared BOs,    │
│ present → compositor surface → DCN scanout (already [x] iron). │
│ Input: xHCI HID → raeshell events → SDL2 event shim.           │
├─ L2: UMD ──────────────────────────────────────────────────────┤
│ Mesa RADV compiled for Linux, run as a glibc ELF.              │
│ Seam: libdrm /dev/dri/renderD* ioctls → IPC → amdgpud.         │
│ Build out raeen_amdgpu::uapi: GEM_CREATE, GEM_MMAP, VM_OP,     │
│ CTX, CS, WAIT_CS, INFO (done), plus DRM core (version, auth).  │
├─ L1: KMD ──────────────────────────────────────────────────────┤
│ Strategy B: real upstream amdgpu C against the LinuxKPI shim,  │
│ inside userspace amdgpud (IOMMU-sandbox lineage, caps 109–118).│
│ Its REAL init clears MES (possible from this handoff per §2).  │
│ Native-Rust driver: retained for DCN scanout + halt forensics. │
└────────────────────────────────────────────────────────────────┘
```

Constraints honored: no Linux clones in-kernel (Mesa/vkQuake run as *guests* of the Linux-ABI translation layer, which exists precisely for this); every privileged op through `crate::capability`; new syscalls/IPC surfaces only via `[interface]` commits with `docs/SYSCALL_TABLE.md` updated in the same commit; no new block-device write paths; all iron boots `--safe`.

## 5. Data flow (one frame)

```
game logic → SDL2 → Vulkan
 → RADV builds PM4 command buffers in GEM BOs
 → libdrm shim: DRM_IOCTL_AMDGPU_CS ──IPC──► amdgpud
 → amdgpud (real amdgpu): validate → MES-managed GFX queue
   → doorbell → GPU executes → fence signals
 → WAIT_CS / fence poll back over IPC → RADV
 → RaeWSI present: rendered BO → compositor surface
 → compositor → DCN scanout
```

BO sharing uses the same physical-carveout + `memory::map_phys_wb` machinery the DCN scanout landed (ownership-gated: firmware-reserved pages only, never usable RAM — same security model as `MAP_PHYS`). The libdrm shim is a **drop-in libdrm replacement built into the Mesa cross-build** (not `LD_PRELOAD` — we control the toolchain, so link-time substitution is deterministic); ioctls marshal to amdgpud request/reply messages. Where an ioctl is hot (CS submit, fence wait), the message carries indices into a shared ring rather than copies.

## 6. Error handling

- **KMD halt (the 0x7656 scenario):** amdgpud self-dumps (HALT-DUMP block + PMFW-liveness probe + netlog), exits clean; kernel and desktop stay alive (proven 2026-07-02). The machine is never taken down by the driver — Concept: "driver crash ≠ system crash."
- **GPU hang mid-game:** fence timeout in amdgpud → kill guest ctx → error reply over IPC → RADV surfaces `VK_ERROR_DEVICE_LOST` → game exits; compositor falls back to SW-raster/GOP presentation.
- **Missing Linux-ABI syscall:** fail LOUD with the syscall number (existing strace-oracle gap-filling loop consumes these). Never silently stubbed.
- **Safe-mode:** `--safe` images for every test boot; this campaign adds no disk-write paths.

## 7. Proof ladder (per docs/TESTING_STRATEGY.md)

1. **Host KATs:** uapi struct layouts vs libdrm headers; CS marshal round-trip; RaeWSI geometry/acquire-present state machine. `cargo test -p raeen_amdgpu` (115 KATs today) grows with each seam.
2. **QEMU CI green every slice:** no Radeon in QEMU → real-amdgpu path self-skips (existing behavior). Linux-ABI syscall additions get QEMU smoketests.
3. **Iron loop:** existing no-flash deploy (ESP `kernel-x86_64`, netlog UDP 51514, `scripts/athena-kvm.sh` for the KVM variants). FAIL-able serial/netlog markers per milestone.
4. **Fresh oracle:** today's captured stock-driver state (dmesg init sequence, ring roster, firmware table) is committed as `docs/gpu-oracle/stock-init-20260706.txt` and every iron run diffs against it.

## 8. Milestones

| # | Milestone | Proof | Closes |
|---|---|---|---|
| **M1** | Real amdgpu init clears MES on iron (`RAEEN_AMDGPU_REAL=1` image, one deploy) | `set_hw_resources ACK` + gfx ring alive in netlog | the wall |
| **M2** | DRM seam: GEM/CS/WAIT_CS/CTX over IPC — built under the §10 constraints (multi-client, ring-agnostic, export/syncobj surfaces reserved) | host KATs + hand-built PM4 triangle submitted via CS from a native test client, pixels verified in a readback BO | 6.3 "Submits to GPU" |
| **M3** | RADV renders: vkcube-class triangle on the Athena panel | screenshot + frame-fence markers | 6.3 complete + Year-1 Vulkan demo |
| **M4** | vkQuake playable: renders, takes input, ≥30 fps sustained through the demo loop | timing marker + owner plays it | campaign DONE |

M1 is deliberately one-deploy small; it decides the universe within a day.

**Fallback trigger (pre-agreed, not a future debate):** if M1 halts at `0x7656`, run the shim-behavior diff against the §2 oracle (workqueue/timing/ordering, the MM_INDEX pipe0-stack read at the halt). If **two** fix-arcs fail to move the halt, STOP Route A KMD work: run gate Q2 (stock amdgpu in a KVM/VFIO guest) and write the Route B (driver-VM) spec. L2/L3 work (uapi seam, RaeWSI, Linux-ABI gaps) transfers to Route B mostly intact — it is deliberately KMD-agnostic.

## 9. Risks (ranked)

1. **Real amdgpu also halts under the shim** (moderate) — mitigated by the pre-agreed §8 fallback and by L2/L3 being KMD-agnostic.
2. **Mesa's syscall surface** (high certainty, low unit cost) — memfd/epoll/eventfd/sysfs-read gaps; ground each with the strace oracle on Athena-Linux, fix in `linux_syscall.rs`, never guess.
3. **Futex phys-rekey on the critical path** (possible) — RADV internal threading may exercise cross-process-grade sync; the design keeps game+RADV single-process to dodge it. If it becomes load-bearing: it is HUMAN-GATED `scheduler.rs` work (AthBridge sync-gate item) — surface to owner, do not start unilaterally.
4. **RaeWSI is bespoke** (certain, bounded) — no X11/Wayland (banned clones); shim scope is N shared BOs + acquire/present semantics + SDL2 video/input backend.
5. **IPC-per-ioctl performance** (accepted) — M4 bar is 30 fps; shared-ring hot paths if needed; real perf work is a later campaign against `docs/PERFORMANCE_TARGETS.md`.

## 10. Forward design constraints — Windows games + general GPU workloads (owner directive 2026-07-06)

The seam (L2) is designed from day one so the follow-on campaigns bolt on without reworking it. These are **design constraints on M2, not new milestones**:

1. **Multi-client from the first line.** amdgpud serves N concurrent clients (game + compositor + overlay + AthBridge guest later), each with its own DRM ctx and GPU VM address space — a client table keyed by connection, never a singleton pipe. MES user-queue scheduling is exactly what Phoenix's hardware is for; the seam must not assume one submitter.
2. **BO export/import (dma-buf equivalent).** Every GEM handle can be exported to a cross-client handle and imported by another client (compositor consuming a game's render target = zero-copy; DXVK swapchains need this). Reserved in the uapi message space at M2, implemented when the compositor consumes its first client BO (M3's present path).
3. **Syncobj / timeline semaphores.** DXVK and VKD3D-Proton require `VK_KHR_timeline_semaphore`-grade sync; RADV provides it if the seam speaks drm-syncobj. M2 implements basic fences but **reserves the syncobj surface** (message opcodes + handle namespace) so it's additive. Cross-process sync ties into the AthBridge sync-gate work (host-half done in `sync_engine.rs`); the kernel futex phys-rekey half stays HUMAN-GATED.
4. **Ring-agnostic submit.** The CS message carries an IP type (GFX / compute / SDMA / VCN-decode / VCN-encode) + queue priority, not a hardcoded GFX target. Compute rides the same path (the 8 `comp_1.*` rings are already in the oracle roster); video decode (VCN 4.0.2) becomes a client of the same seam when the media campaign wants it. "Anything we throw at the GPU" = a new IP type enum value, not a new seam.
5. **Queue priority → SCHED_BODY.** The priority field maps GameOS/SCHED_BODY intent onto MES high-priority queues (the hardware anti-jank mechanism), so "gaming isn't a mode" reaches the GPU scheduler too.
6. **Windows-games checkpoint (post-M4 campaign C2).** AthBridge D3D path = DXVK/VKD3D-Proton on this same Vulkan substrate; DXGI presents through RaeWSI. Already in-tree: `dxbc_spirv.rs` (D3D9/11 shaders, spirv-val-clean). Still Phase 11 items: DXIL→SPIR-V (D3D12), the runtime ports, and AthBridge guest execution (human-gated). C2's *GPU-side* prerequisites are exactly items 1–3 above — which is why they're constraints now.
7. **Compute/media checkpoint (post-M4 campaign C3).** Headless compute contexts (no WSI), VCN decode for the media stack, encode for capture/streaming — all IP-type additions per item 4.

## 11. Open items intentionally deferred

- Q2 VFIO-guest oracle (buys Route B feasibility data) — run only on §8 trigger.
- Exclusive-fullscreen direct-to-GPU + frame-time <16.6 ms (checklist 6.5) — next campaign, on this substrate.
- `MMMC_VM_FB_OFFSET` live read (replace the Athena `0x3E0000000` scanout const) — hardening follow-up rides M2/M3.
- Steam / AthBridge / DXVK — Phase 11, separate campaign, human-gated.

## 12. Hand-off

Next step: `superpowers:writing-plans` produces the implementation plan for **M1 first** (smallest decisive step), then M2–M4 as separately landable slices, each with build + QEMU CI + iron verification per CLAUDE.md §11.
