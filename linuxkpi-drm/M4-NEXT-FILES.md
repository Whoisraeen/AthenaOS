# M4 — the amdgpu `.c` files to compile next (close the 259 internal-symbol gap)

The `m4-link.sh` gauge splits its undefined-symbol gap into (B) the LinuxKPI gap
(`ath_linuxkpi` must export — **now closed**, see `SYMBOL-GAP.md`) and (A) the
**amdgpu/drm-internal** symbols — functions defined in driver `.c` files **not yet in
the gauge's `BRINGUP` list**. This maps those internal symbols to their defining file so
they can be added in priority order instead of discovered one link-error at a time.

**Method:** for each internal symbol in `~/m4-obj/gap.txt`, grep the vendored amd source
for the definition (col-0 / `type name(` form), excluding wrong-ASIC generations
(gfx11/Phoenix is `gfx_v11` / `sdma_v6` / `ih_v6` / `psp_v13` / `smu_v13` / `soc21` /
`*_v3`). 210 of 259 mapped to these **core, ASIC-independent** files.

## Add to `BRINGUP` (count = internal symbols each resolves)

```
 25  amdgpu_device          ← top priority: the core device/IP-block infrastructure
 13  amdgpu_ring
 12  amdgpu_object          (amdgpu_bo_*)
 11  amdgpu_ras
 10  amdgpu_vm
  7  amdgpu_ucode
  5  amdgpu_irq
  4  amdgpu_xgmi   amdgpu_virt   amdgpu_sa   amdgpu_rlc
  3  amdgpu_mca   amdgpu_gart   amdgpu_amdkfd
  2  amdgpu_xcp  amdgpu_userq  amdgpu_umc  amdgpu_sdma  amdgpu_psp  amdgpu_job
     amdgpu_ids  amdgpu_doorbell_mgr
  1  amdgpu_ttm  amdgpu_sync  amdgpu_nbio  amdgpu_mmhub  amdgpu_ip  amdgpu_hdp
     amdgpu_gtt_mgr  amdgpu_gem  amdgpu_discovery  amdgpu_csa  amdgpu_bios
     amdgpu_atomfirmware
```

(`soc21` also appears — note the current `BRINGUP` lists **`soc15`**, but gfx11/Phoenix
is **SOC21**; `soc21.c` is likely the correct one to compile.)

## The ~49 not in the list above (ASIC-specific — add the gfx11/Phoenix variant)

These resolve by compiling the **gfx11.0.1 / Phoenix** IP-version files (the gauge's
wrong-ASIC matches were filtered out): `psp_v13_0`, `smu_v13_0_4` (+ the swsmu core under
`pm/swsmu/`), `sdma_v6_1`, `ih_v6_0`, `gfxhub_v3_0`, `mmhub_v3_0_1`, `athub_v3_0`,
`hdp_v6_0`, `umc_v8_10`. Check `gap.txt` after each add — the precise set falls out.

## It's iterative (but converges fast)

Compiling a new `.c` resolves its symbols **and** may pull in a few new ones it calls —
so re-run `m4-link.sh` after each batch. But the core files above are the bulk; the gap
shrinks monotonically toward the small set of genuine leaf dependencies. The LinuxKPI
side is already complete, so every remaining symbol is either driver-internal (compile
its file) or a `#if 0`-able subsystem (display/VCN/JPEG — already cut per SCOPE.md).

## Prerequisite header work before the core `.c` compile (probed 2026-06-30)

Trial-compiling `amdgpu_device.c` / `amdgpu_ring.c` / `amdgpu_object.c` against the
current shim shows they're gated on two header items (header-shim lane, not LinuxKPI
exports — the export side is closed):

1. **`linux/export.h`** (pulled in by `amdgpu_object.c`) — trivial: `EXPORT_SYMBOL*`,
   `THIS_MODULE`, `MODULE_IMPORT_NS` → no-op macros (these symbols don't need real
   defs; the macros just need to expand to nothing).
2. **`amdgpu_mode.h:45` → `modules/inc/mod_freesync.h`** (a Display-Core header) — the
   MES bring-up subset doesn't use DC, so either stub `modules/inc/mod_freesync.h`
   (+ siblings) empty, or guard the DC includes out of `amdgpu_mode.h`. Same DC-cut
   strategy already applied for the M2/M3 display type graph (SCOPE.md).

Once these resolve, the core files compile and any **new** LinuxKPI symbols they pull in
fall out of `m4-link.sh`'s `kpigap.txt` — ping me and I'll export them immediately
(LinuxKPI lane).

## Regenerate

```bash
# gap.txt is left in ~/m4-obj by m4-link.sh; then per symbol:
grep -rlE "^([a-zA-Z_].*[ *])?<sym>\(" --include=*.c <amd>/amdgpu <scheduler> <ttm>
```
