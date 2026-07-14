#!/usr/bin/env bash
# linuxkpi-drm M4 link gauge — host-first (WSL/gcc + WSL cargo).
#
# Builds raeen_linuxkpi as a host staticlib (the `clib` feature), compiles the
# MES bring-up .c subset to objects against the shim headers, and reports the
# undefined-symbol GAP split into:
#   (A) amdgpu/drm-internal symbols  -> resolved by compiling MORE driver .c
#   (B) the real LinuxKPI/drm gap    -> raeen_linuxkpi must export, or a new shim
# This is M4 step "map every undefined symbol to a raeen_linuxkpi export".
set -euo pipefail

# REPO is derived from this script's location (linuxkpi-drm/..) so the build runs on
# any host — the old hardcoded /mnt/c/... WSL path broke every non-WSL box.
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# fetch-source.sh extracts the pinned tree into linuxkpi-drm/vendor/ — default there
# (overridable) so the compile and the fetch agree on one location.
VENDOR=${VENDOR:-$REPO/linuxkpi-drm/vendor}
SRC=$VENDOR/linux-7.0.12
AMD=$SRC/drivers/gpu/drm/amd
SCHED=$SRC/drivers/gpu/drm/scheduler
TTM=$SRC/drivers/gpu/drm/ttm
DMABUF=$SRC/drivers/dma-buf
PM=$AMD/pm
TGT=${CARGO_TARGET_DIR:-$HOME/m4-target}
OBJ=$HOME/m4-obj
# FREESTANDING=1 -> the bare-metal (AthenaOS daemon) build: freestanding objects +
# the x86_64-unknown-none staticlib. Default -> the host link-test build.
# gcc 14+ promotes implicit-function-declaration and implicit-int from WARNING to a
# hard ERROR. The WSL toolchain that authored this build treated them as warnings —
# the compile loops hide them with `2>/dev/null` and the (~14 alloc-alias, bitmap,
# atomic_long, printk_ratelimit …) symbols resolve at LINK via the auto-stub. Restore
# that behaviour on gcc 16 so the exact same object set builds on this native Linux box.
GCC_COMPAT="-fpermissive -Wno-implicit-function-declaration -Wno-implicit-int"
# RELOC FIX: -fvisibility=hidden makes amdgpu's global symbols non-interposable, so
# every .data.rel.ro vtable slot (`X_funcs.op = x_op`) that references another amdgpu
# global emits R_X86_64_RELATIVE at the final daemon link instead of a symbol-based
# R_X86_64_64. The AthenaOS ELF loader (kernel/src/elf.rs) applies ONLY RELATIVE, so a
# symbol-based slot stays null -> `nbio.funcs->set_reg_remap` jumps through 0 (the
# Athena 0x77 fault). Everything statically merges into the one daemon, so nothing
# needs to stay exported. Belt-and-suspenders with build.rs's -Bsymbolic final link.
VIS="-fvisibility=hidden"
# rae_implicit_fence.h (force-included into EVERY vendored TU): exact-signature
# prototypes for the implicitly-declared functions that return pointers or
# 64-bit values. Without it gcc assumes `int f()` and the caller truncates +
# sign-extends the return (`call ioremap; cltq` corrupted adev->rmmio — found
# off-target 2026-07-08 by tools/amdgpu_hostrun). int-returning implicits stay
# tolerated (the off-path weak-stub strategy depends on them).
FENCE="-include $REPO/linuxkpi-drm/include/linux/rae_implicit_fence.h"
if [ -n "${FREESTANDING:-}" ]; then
  RTARGET=x86_64-unknown-none;        CFLAGS="-ffreestanding -mno-red-zone $VIS $GCC_COMPAT $FENCE"
else
  RTARGET=x86_64-unknown-linux-gnu;   CFLAGS="$VIS $GCC_COMPAT $FENCE"
fi
# CONFIG_ACPI is enabled ONLY for amdgpu_bios.c (scoped in the BRINGUP loop
# below), not globally: Phoenix (APU) has no PCI-ROM VBIOS, so the sole ATOMBIOS
# source is amdgpu_acpi_vfct_bios() (amdgpu_bios.c, #ifdef CONFIG_ACPI), which
# calls acpi_get_table("VFCT") — served by raeen_linuxkpi from the bundled VFCT
# capture. Enabling CONFIG_ACPI GLOBALLY pulled in an ACPI virt-detect path in
# other files that faulted (mutex_lock on the xgpu_nv SR-IOV stub), so we scope it.
LIB=$TGT/$RTARGET/release/libraeen_linuxkpi.a

INC="-I $REPO/linuxkpi-drm/include/amd-stubs -I $REPO/linuxkpi-drm/include \
  -I $AMD/include -I $AMD/include/asic_reg -I $AMD/amdgpu -I $AMD/amdkfd \
  -I $AMD/ras -I $AMD/ras/ras_mgr -I $AMD/ras/rascore \
  -I $AMD/display -I $AMD/display/include -I $AMD/display/dc -I $AMD/display/modules/inc \
  -I $AMD/pm/inc -I $AMD/pm/swsmu -I $AMD/pm/swsmu/inc -I $AMD/pm/swsmu/inc/pmfw_if \
  -I $AMD/pm/powerplay/inc -I $AMD/pm/powerplay/hwmgr -I $AMD/pm/powerplay/smumgr \
  -I $AMD/pm/swsmu/smu11 -I $AMD/pm/swsmu/smu12 -I $AMD/pm/swsmu/smu13 \
  -I $AMD/pm/swsmu/smu14 -I $AMD/pm/swsmu/smu15 \
  -I $SRC/include -I $SRC/include/uapi -I $SCHED -I $TTM"

# the full compiling subset — every .c below typechecks at 0 (M3 + M4b).
BRINGUP="mes_v11_0 gfx_v11_0 amdgpu_mes gmc_v11_0 amdgpu_ih soc15 nbio_v4_3 nbio_v7_7 \
  amdgpu_gfx amdgpu_rlc imu_v11_0 amdgpu_gmc amdgpu_fence amdgpu_ib amdgpu_ring amdgpu_device amdgpu_discovery \
  amdgpu_psp psp_v13_0 psp_v13_0_4 atom amdgpu_atombios amdgpu_atomfirmware amdgpu_ucode amdgpu_bios \
  amdgpu_ras amdgpu_vm amdgpu_object amdgpu_ttm amdgpu_ctx amdgpu_cs amdgpu_gem \
  amdgpu_display amdgpu_dma_buf amdgpu_kms amdgpu_irq amdgpu_pll amdgpu_sync \
  gfxhub_v3_0 mmhub_v3_0 mmhub_v3_0_1 athub_v3_0 hdp_v6_0 sdma_v6_0 \
  amdgpu_gart amdgpu_gtt_mgr amdgpu_vram_mgr amdgpu_preempt_mgr amdgpu_sa amdgpu_sdma amdgpu_reset \
  soc21 gfx_v11_0_3 amdgpu_eviction_fence \
  amdgpu_job amdgpu_seq64 amdgpu_hdp amdgpu_doorbell_mgr amdgpu_vm_pt amdgpu_fru_eeprom \
  amdgpu_ids amdgpu_vm_sdma amdgpu_vm_cpu amdgpu_bo_list amdgpu_csa \
  ih_v6_1 ih_v6_0 amdgpu_ip"

# DRM scheduler (outside amd/ — lives at drivers/gpu/drm/scheduler/, fetched separately
# since fetch-source.sh only pulls the amd/ subtree + drm headers). amdgpu_ring/
# amdgpu_fence's ring init and the MES queue submit path sit on this. M5-ONPATH-AUDIT.md
# item 3 (drm_sched_*, ~24 on-path stubs).
BRINGUP_SCHED="sched_main sched_entity sched_fence"

# TTM memory manager (drivers/gpu/drm/ttm/, fetched from the tarball). The MES
# MQD/ring/PSP-fw BOs are TTM buffer objects — M5-ONPATH-AUDIT.md item 4 (ttm_*,
# ~44 on-path stubs). Core BO/device/resource/pool/tt; skips ttm_bo_vm (mmap
# fault — userspace), ttm_agp_backend (legacy AGP), ttm_backup (swap, out of subset).
BRINGUP_TTM="ttm_module ttm_device ttm_resource ttm_sys_manager ttm_range_manager \
  ttm_tt ttm_bo ttm_bo_util ttm_execbuf_util ttm_pool ttm_bo_vm"

# DRM core allocators (drivers/gpu/drm/, fetched from the tarball like ttm/sched).
# amdgpu_vram_mgr's VRAM heap IS drm_buddy (drm_buddy_alloc_blocks/free/trim), so a
# weak-stubbed drm_buddy makes amdgpu_vram_mgr_init fail ("Failed initializing VRAM
# heap"). drm_mm (the GTT + doorbell interval allocator, augmented-rbtree based)
# similarly leaves GTT at 0M and fails the kernel-doorbell BO alloc when stubbed.
# Pure data-structure code — host-KAT-able, no hardware.
BRINGUP_DRM="drm_buddy drm_mm drm_exec drm_gem drm_managed drm_vma_manager \
  drm_file drm_ioctl drm_auth drm_prime drm_syncobj drm_gem_ttm_helper"
BRINGUP_DMABUF="dma-fence-unwrap"

# SMU / power (drivers/gpu/drm/amd/pm/) — brings up the GFX/SoC clocks + voltage;
# without it the MES microengine has no clock. M5-ONPATH-AUDIT item 2 (~40 on-path
# stubs). Phoenix = SMU 13.0.4. Skips amdgpu_pm (hwmon knobs — off the init path).
# Athena's Phoenix1 discovery reports MP1 v13.0.4 and dispatches to
# smu_v13_0_4_set_ppt_funcs. Keep yellow_carp_ppt in the closure as a compile-time
# dependency/nearby Phoenix-family implementation, but it is not Athena's live path.
BRINGUP_SMU="amdgpu_dpm swsmu/amdgpu_smu swsmu/smu_cmn swsmu/smu13/smu_v13_0 \
  swsmu/smu13/smu_v13_0_4_ppt swsmu/smu13/yellow_carp_ppt"

echo "[m4] building raeen_linuxkpi host staticlib (clib feature)..."
( cd "$REPO" && CARGO_TARGET_DIR="$TGT" cargo rustc -p raeen_linuxkpi \
    --features clib --crate-type staticlib --target $RTARGET --release >/dev/null )
echo "[m4] staticlib: $LIB ($(nm "$LIB" 2>/dev/null | grep -cE ' [TtWR] ') defined symbols)"

mkdir -p "$OBJ"; rm -f "$OBJ"/*.o "$OBJ"/*.err

# A zero-byte vendor file is a successful C translation unit but contributes no
# implementation.  Treat it as a hard source-closure failure instead of
# reporting a misleading "real object" count.
require_source() {
  [ -s "$1" ] || { echo "[m4] ERROR: mandatory source missing/empty: $2 ($1)" >&2; exit 1; }
}
for F in $BRINGUP; do require_source "$AMD/amdgpu/$F.c" "$F"; done
for F in $BRINGUP_SCHED; do require_source "$SCHED/$F.c" "$F"; done
for F in $BRINGUP_TTM; do require_source "$TTM/$F.c" "$F"; done
for F in $BRINGUP_DRM; do require_source "$SRC/drivers/gpu/drm/$F.c" "$F"; done
for F in $BRINGUP_DMABUF; do require_source "$DMABUF/$F.c" "$F"; done
for F in $BRINGUP_SMU; do require_source "$PM/$F.c" "$F"; done

ok=0; n=0; failed=""
for F in $BRINGUP; do
  n=$((n+1))
  # amdgpu_bios.c: the only file that needs CONFIG_ACPI (the VFCT VBIOS path).
  XCF=""; [ "$F" = amdgpu_bios ] && XCF="-DCONFIG_ACPI"
  # gmc_v11_0.c: the APU visible-VRAM override (aper_base = mmhub get_mc_fb_offset,
  # aper_size = real_vram_size) is gated on #ifdef CONFIG_X86_64. AthenaOS IS x86-64;
  # without the define visible_vram_size stays at the 256M PCI BAR while the PSP TMR
  # reserves near the top of real VRAM (~1920M) -> ttm placement fails -EINVAL(-22).
  # Athena is an APU (GC 11.0.1, in amdgpu_discovery's AMD_IS_APU list), so the branch
  # fires at runtime. Scope to gmc_v11_0 (its only on-path CONFIG_X86_64 block) to
  # avoid enabling set_memory_wc paths elsewhere. See docs/gpu-oracle (Linux oracle).
  [ "$F" = gmc_v11_0 ] && XCF="-DCONFIG_X86_64"
  if gcc -std=gnu11 -fno-strict-aliasing -fno-stack-protector $CFLAGS $XCF -c -Wno-unknown-pragmas "$AMD/amdgpu/$F.c" $INC -o "$OBJ/$F.o" 2>"$OBJ/$F.err"; then
    ok=$((ok+1)); rm -f "$OBJ/$F.err"; else failed="$failed $F"; fi
done
for F in $BRINGUP_SCHED; do
  n=$((n+1))
  if gcc -std=gnu11 -fno-strict-aliasing -fno-stack-protector $CFLAGS -c -Wno-unknown-pragmas "$SCHED/$F.c" $INC -o "$OBJ/$F.o" 2>"$OBJ/$F.err"; then
    ok=$((ok+1)); rm -f "$OBJ/$F.err"; else failed="$failed $F"; fi
done
# TTM also gets <linux/highmem.h> force-included: ttm_bo_util/ttm_pool/ttm_tt
# call the kmap family with no prototype in scope, and those are static-inline
# definitions in the curated highmem.h (an extern prototype in the fence header
# would conflict) — same pointer-truncation class as the fence's ioremap case.
for F in $BRINGUP_TTM; do
  n=$((n+1))
  XCF=""; [ "$F" = ttm_bo_vm ] && XCF="-DCONFIG_MMU"
  if gcc -std=gnu11 -fno-strict-aliasing -fno-stack-protector $CFLAGS $XCF -include "$REPO/linuxkpi-drm/include/linux/highmem.h" -c -Wno-unknown-pragmas "$TTM/$F.c" $INC -o "$OBJ/$F.o" 2>"$OBJ/$F.err"; then
    ok=$((ok+1)); rm -f "$OBJ/$F.err"; else failed="$failed $F"; fi
done
for F in $BRINGUP_DRM; do
  n=$((n+1))
  if gcc -std=gnu11 -fno-strict-aliasing -fno-stack-protector $CFLAGS -c -Wno-unknown-pragmas "$SRC/drivers/gpu/drm/$F.c" $INC -o "$OBJ/$F.o" 2>"$OBJ/$F.err"; then
    ok=$((ok+1)); rm -f "$OBJ/$F.err"; else failed="$failed $F"; fi
done
for F in $BRINGUP_DMABUF; do
  n=$((n+1))
  if gcc -std=gnu11 -fno-strict-aliasing -fno-stack-protector $CFLAGS -c -Wno-unknown-pragmas "$DMABUF/$F.c" $INC -o "$OBJ/$F.o" 2>"$OBJ/$F.err"; then
    ok=$((ok+1)); rm -f "$OBJ/$F.err"; else failed="$failed $F"; fi
done
for F in $BRINGUP_SMU; do
  n=$((n+1))
  base=$(basename "$F")
  if gcc -std=gnu11 -fno-strict-aliasing -fno-stack-protector $CFLAGS -c -Wno-unknown-pragmas "$PM/$F.c" $INC -o "$OBJ/$base.o" 2>"$OBJ/$base.err"; then
    ok=$((ok+1)); rm -f "$OBJ/$base.err"; else failed="$failed $F"; fi
done
# MPL no-op IP-block descriptors (VCN/JPEG/UMSCH — Phoenix's discovery adds them
# but they are out of the 3D/gaming scope; valid inert descriptors so ip_init
# doesn't fault on a zeroed stub's NULL .funcs).
n=$((n+1))
if gcc -std=gnu11 -fno-strict-aliasing -fno-stack-protector $CFLAGS -c -Wno-unknown-pragmas "$REPO/linuxkpi-drm/bringup_ip_noop.c" $INC -o "$OBJ/bringup_ip_noop.o" 2>"$OBJ/bringup_ip_noop.err"; then
  ok=$((ok+1)); rm -f "$OBJ/bringup_ip_noop.err"; else failed="$failed bringup_ip_noop"; fi
# amdgpu module parameters as real data (amdgpu_dpm/dc/vm_size/... — defined in the
# uncompiled amdgpu_drv.c; without these they auto-stub as functions read as ints).
n=$((n+1))
if gcc -std=gnu11 -fno-strict-aliasing -fno-stack-protector $CFLAGS -c -Wno-unknown-pragmas "$REPO/linuxkpi-drm/bringup_params.c" $INC -o "$OBJ/bringup_params.o" 2>"$OBJ/bringup_params.err"; then
  ok=$((ok+1)); rm -f "$OBJ/bringup_params.err"; else failed="$failed bringup_params"; fi
# drm_device field setup (vma_offset_manager + anon_inode) the skipped drm_dev_init
# would normally do; ttm_device_init derefs them during amdgpu_ttm_init.
n=$((n+1))
if gcc -std=gnu11 -fno-strict-aliasing -fno-stack-protector $CFLAGS -c -Wno-unknown-pragmas "$REPO/linuxkpi-drm/bringup_drm.c" $INC -o "$OBJ/bringup_drm.o" 2>"$OBJ/bringup_drm.err"; then
  ok=$((ok+1)); rm -f "$OBJ/bringup_drm.err"; else failed="$failed bringup_drm"; fi
# Phoenix has no XCP manager. Compile exact no-manager semantics and fail closed
# for partition operations, rather than linking untyped weak stubs.
n=$((n+1))
if gcc -std=gnu11 -fno-strict-aliasing -fno-stack-protector $CFLAGS -c -Wno-unknown-pragmas "$REPO/linuxkpi-drm/bringup_xcp_none.c" $INC -o "$OBJ/bringup_xcp_none.o" 2>"$OBJ/bringup_xcp_none.err"; then
  ok=$((ok+1)); rm -f "$OBJ/bringup_xcp_none.err"; else failed="$failed bringup_xcp_none"; fi
# the amdgpud daemon entry: rae_amdgpu_device_init -> amdgpu_driver_load_kms ->
# amdgpu_device_init -> the full IP init. Links the whole init path into one object.
n=$((n+1))
if gcc -std=gnu11 -fno-strict-aliasing -fno-stack-protector $CFLAGS -c -Wno-unknown-pragmas "$REPO/linuxkpi-drm/bringup_entry.c" $INC -o "$OBJ/bringup_entry.o" 2>"$OBJ/bringup_entry.err"; then
  ok=$((ok+1)); rm -f "$OBJ/bringup_entry.err"; else failed="$failed bringup_entry"; fi
# Real render-only DRM file lifecycle and ioctl table for the retained device.
n=$((n+1))
if gcc -std=gnu11 -fno-strict-aliasing -fno-stack-protector $CFLAGS -c -Wno-unknown-pragmas "$REPO/linuxkpi-drm/bringup_render.c" $INC -o "$OBJ/bringup_render.o" 2>"$OBJ/bringup_render.err"; then
  ok=$((ok+1)); rm -f "$OBJ/bringup_render.err"; else failed="$failed bringup_render"; fi
echo "[m4] compiled $ok/$n bring-up objects"
if [ -n "$failed" ]; then
  echo "[m4] ERROR: mandatory sources failed:$failed" >&2
  for e in "$OBJ"/*.err; do
    [ -s "$e" ] || continue
    echo "===== $(basename "$e") =====" >&2
    cat "$e" >&2
  done
  exit 1
fi

cd "$OBJ"
nm -u *.o 2>/dev/null | awk '{print $2}' | sort -u > undef.txt
# DEFINED = any symbol with an address whose type is not 'U' (text T/t, weak W/V,
# read-only R, data D/d, bss B/b, ...) — must count DATA symbols, not just text,
# or globals like jiffies/system_state show up as false-positive gaps.
nm "$LIB" 2>/dev/null | awk 'NF==3 && $2!="U" {print $3}' | sort -u > def.txt
comm -23 undef.txt def.txt > gap.txt
# amdgpu/drm-internal prefixes (resolved by compiling more driver .c)
# amdgpu's own + DRM-core (drm_*) + TTM + the DRM scheduler are all "compile/fetch
# more component .c", NOT the LinuxKPI seam raeen_linuxkpi owns.
AMDRE='^(amdgpu|gfx_|gfx9|gfx10|gfx11|gmc|mes_|soc|nbio|nbif|psp|sdma|vcn|jpeg|vpe|umc|umsch|uvd|vce|gfxhub|mmhub|athub|hdp|smu|pp_smu|aqua|aldebaran|arct|navi|sienna|cyan|emu_soc|kgd|kfd|dce|dcn|dm_|amdkfd|cik|si_|vi_|nv_|ih_|isp|vega|polaris|imu|lsdma|mca|aca|ras_v|df_|mgpu|amdgv|amd_|drm_|drmm_|ttm_|xgpu|userq|renoir|vangogh|yellow_carp|is_support|link_speed|_GLOBAL|__stack_chk)'
# amdgpu IP-block descriptors end in _ip_block (defined in their own IP .c file);
# also drop CRT/compiler symbols (__stack_chk_fail, the GOT) and blank lines.
grep -vE "$AMDRE" gap.txt | grep -vE '_ip_block$' | grep -vE '^(__stack_chk|_GLOBAL_OFFSET)' | grep -vE '^$' > kpigap.txt || true
echo "[m4] undefined=$(wc -l < undef.txt)  staticlib-provides=$(wc -l < def.txt)"
echo "[m4] GAP total=$(wc -l < gap.txt)  (amdgpu/drm-internal=$(($(wc -l < gap.txt)-$(wc -l < kpigap.txt))), real LinuxKPI gap=$(wc -l < kpigap.txt))"
echo "[m4] --- real LinuxKPI gap (raeen_linuxkpi must export / new shim) ---"
cat kpigap.txt
