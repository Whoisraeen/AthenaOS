# M5 on-path stub audit — what must become *real* for a functional Phoenix init

**Status (2026-06-30):** M4c reached — the amdgpu Phoenix bring-up graph LINKS
(`m4c-link.sh`: 53 real objects + ath_linuxkpi + 569 auto-stubs →
`amdgpu-bringup.o`, 0 unresolved). This doc audits which of those **569 stubs are
on the real init path** (the ones a live `amdgpu_device_init` dereferences or
executes), and therefore must be replaced with real compiled `.c` before an M5 run
on Athena is meaningful. The rest stay stubbed.

Reproduce the live counts: `bash linuxkpi-drm/m4c-link.sh` →
`[m4c] M5-readiness — stubs ON the Phoenix init path: 113; OFF-path: 456`.

> **Update 2026-07-01:** the current build reaches **0 on-path stubs** (`FREESTANDING=1
> bash linuxkpi-drm/m4c-link.sh` → `stubs ON the Phoenix init path: 0`) — `ih_v6_0`
> and `smu_v13_0` are now compiled real (rows below updated ✅). The `113` figure above
> is the 2026-06-30 snapshot. Also: the SKU/IP-version identification was corrected
> (GC **11.0.1**, IH **v6.0**) — see the corrected list below. The off-path (456) rows
> were NOT re-verified in this pass; treat their ❌ status as of the 06-30 snapshot.

Target SKU: **Athena = Phoenix1 APU (PCI 1002:15BF), GFX 11.0.1** (IP versions,
verified against the live `ip_discovery` on the reference silicon 2026-07-01: GC
**11.0.1** (gfx_v11_0/gmc_v11_0), SDMA **6.0.1** (sdma_v6_0), MMHUB/ATHUB **3.0.1**
(mmhub_v3_0_1), OSS/IH **6.0.1** (**ih_v6_0**, NOT ih_v6_1), MP0/PSP **13.0.4**
(psp_v13_0_4), MP1/SMU **13.0.4** (smu_v13_0), MES **11.0.0** (mes_v11_0), VCN 4.0.0,
JPEG 4.0.0, DCN/DMU 3.1.4).

> **Correction:** earlier drafts said "GFX/GC 11.0.3" — that is wrong. The chip
> reports **GC 11.0.1** (`/sys/.../ip_discovery/die/0/GC/0` → 11.0.1). This is why
> `amdgpu_discovery.c` sets `AMD_IS_APU` (11.0.1 is in its APU list; 11.0.3 is not)
> and why the bundled `gc_11_0_1` firmware is the correct blob set. Hardcode 11.0.1,
> not 11.0.3, anywhere an IP version is assumed off-discovery.

---

## Why _ip_block stubs are dangerous

`amdgpu_device_ip_init()` walks `adev->ip_blocks[]` and calls each
`block->version->funcs->hw_init(...)`. The IP-block descriptors are **data**
symbols; the auto-stub emits them as a zeroed `char[8192]`, so `->funcs` is NULL —
a real init would null-deref on the first on-path block. So every IP block that
**Phoenix's `amdgpu_discovery_set_ip_blocks` actually adds** must be a *real*
compiled descriptor (its IP `.c`), or be excluded from the add list.

### Phoenix IP-block set: real vs stubbed

| IP block | backing .c | status |
|---|---|---|
| `soc21_common_ip_block` | soc21.c | ✅ real (compiled) |
| `gmc_v11_0_ip_block` | gmc_v11_0.c | ✅ real |
| `gfx_v11_0_ip_block` | gfx_v11_0.c | ✅ real |
| `sdma_v6_0_ip_block` | sdma_v6_0.c | ✅ real |
| `mes_v11_0_ip_block` | mes_v11_0.c | ✅ real |
| `psp_v13_0_ip_block` | amdgpu_psp.c/psp_v13_0.c | ✅ real |
| `ih_v6_0_ip_block` | ih_v6_0.c | ✅ real (compiled) — Phoenix uses **ih_v6_0** (OSS/IH 6.0.1, dmesg `ih_v6_0_0`), **NOT ih_v6_1** |
| `smu_v13_0_ip_block` | swsmu/smu_v13_0*.c (+ yellow_carp_ppt) | ✅ real (compiled) — MP1/SMU 13.0.4 |
| `vcn_v4_0_ip_block` | vcn_v4_0.c | ❌ stubbed — discovery adds it; **compile OR exclude** |
| `jpeg_v4_0_ip_block` | jpeg_v4_0.c | ❌ stubbed — discovery adds it; **compile OR exclude** |
| `umsch_mm_v4_0_ip_block` | umsch_mm_v4_0.c | ❌ stubbed — **compile OR exclude** |
| `dm_ip_block` (display) | display/amdgpu_dm | shadow-stubbed; add only if DC enabled — keep **disabled** (headless bring-up) |

All other `*_ip_block` stubs (gfx_v9/v10/v12, gmc_v9/v10/v12, ih_v6_1/v7_0,
jpeg_v2/3/5, mes_v12, sdma_v4/5/7, smu_v11/12/14/15, nv_common, vkms, the
other-ASIC reg_base) are **off-path** — Phoenix never adds them. Safe to stub.

---

## The 113 on-path stubs, by subsystem (the real M5 work)

| Subsystem | stub count | what it does on the init path | plan |
|---|---|---|---|
| **TTM** (`ttm_*`) | 44 | BO allocation/binding — the MES MQD, GFX/compute ring buffers, PSP firmware staging are all TTM BOs | **real** — vendor `drm/ttm/*.c` (or a minimal real BO allocator over the page facade). Biggest piece. Needs the broader page/pfn map. |
| **Power** (`amdgpu_dpm*` 32 + `smu_v13`/`amdgpu_smu`/`smu_cmn` + `smu_v13_0_ip_block`) | ~40 | brings up GFX/SoC clocks + voltage; without it GFX stays gated and the MES microengine has no clock | **real** — compile the swsmu stack (`pm/swsmu/smu_v13_0*.c` + `amdgpu_smu.c` + `pm/swsmu/smu_cmn.c`) + `amdgpu_dpm.c`. Large. |
| **DRM scheduler** (`drm_sched_*`) | 24 | the GPU job scheduler the amdgpu rings sit on; ring init/`amdgpu_fence`/MES queue submit touch it | **real** — vendor `drm/scheduler/*.c` (sched_main/entity/fence, ~3 files) into the island. Medium. |
| **IH v6.0** (`ih_v6_0*` + `ih_v6_0_ip_block`) | ~2 | the Phoenix interrupt-handler IP (OSS 6.0.1) — ring/fence completion IRQs | **real** — compile `ih_v6_0.c` (ih_v6_1 is a different SKU). Small. |
| **VCN/JPEG/UMSCH** (`vcn_v4_0*`, `jpeg_v4_0*`, `umsch_mm_v4_0*` + their ip_blocks) | ~8 | video decode + user-mode scheduler — **NOT needed for MES graphics bring-up** | **exclude** — patch the discovery add-list to skip them (cleaner than compiling), or compile real if cheap. |

### Not in the 113 but still required real (already flagged)
- **Page / pfn map** — `alloc_pages`/`virt_to_page` are a minimal facade today
  (descriptor-before-data); TTM needs a real page allocator + `page_to_pfn`/
  `pfn_to_page` round-trip. Backs the TTM work above.

---

## Recommended order (smallest-blast-radius first)

1. **Exclude** VCN/JPEG/UMSCH/DM from the Phoenix ip_blocks add — removes ~8
   on-path stubs with zero new code (a discovery add-list guard).
2. **ih_v6_0.c** — compile real (small; unblocks interrupt delivery). Phoenix is IH v6.0 (OSS 6.0.1), not v6.1.
3. **drm_sched** — vendor `drm/scheduler/*.c` into the island (3-4 files).
4. **Page facade → real** + **TTM** — the BO allocator the MES MQD/rings need.
5. **SMU/dpm** — the swsmu stack for clocks (largest; GFX needs it powered).
6. Then **M5**: wire `amdgpud` → `amdgpu_device_init` on Athena and watch the
   `0x7654` pipe0 halt.

The off-path 456 stay stubbed — they are other ASICs, SR-IOV (`amdgpu_virt`/`xgpu`),
KFD (`amdgpu_amdkfd`), user-queues (`amdgpu_userq`), partitioning (`amdgpu_xcp`),
multi-GPU (`amdgpu_xgmi`), the DC display core, and the other-version IP drivers.
None are reached on a headless Phoenix MES bring-up.
