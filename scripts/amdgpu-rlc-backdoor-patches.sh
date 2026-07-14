#!/usr/bin/env bash
# amdgpu-rlc-backdoor-patches.sh — re-apply the AthenaOS bring-up adaptations to
# the VENDORED (git-ignored) amdgpu C tree. Firmware-loader selection remains
# upstream-auto so Phoenix uses the same PSP/SMU path as native Linux; failures
# must propagate instead of being converted into successful initialization.
#
# The vendored tree (linuxkpi-drm/vendor/linux-7.0.12) is NOT tracked in git and
# is wiped by fetch-source.sh / hostrun rebuilds, so these patches must be
# re-applied after the tree is (re)materialised. Idempotent: safe to run twice.
#
# Implementation: the fully-patched files are checked in under
# linuxkpi-drm/patches/rae-wall-files/ (they track the exact vendored 7.0.12
# version); we copy them into place. This is more robust than fragile in-place
# perl against a large upstream file. If the vendored version ever changes,
# regenerate the backups from a known-good patched tree.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
V="$ROOT/linuxkpi-drm/vendor/linux-7.0.12/drivers/gpu/drm/amd/amdgpu"
B="$ROOT/linuxkpi-drm/patches/rae-wall-files"

if [ ! -d "$V" ]; then
	echo "[walls] ERROR: vendored tree missing ($V). Run fetch-source.sh first." >&2
	exit 1
fi

apply() { # src dst label
	if [ ! -f "$1" ]; then echo "[walls] MISSING backup $1" >&2; exit 1; fi
	cp -f "$1" "$2"
	echo "[walls] applied: $3"
}

apply "$B/amdgpu/gfx_v11_0.c" "$V/gfx_v11_0.c" "wall-7 (gfx_v11_0.c: RLC TOC \$PS1 skip)"
apply "$B/amdgpu/imu_v11_0.c" "$V/imu_v11_0.c" "wall-5 (imu_v11_0.c: 11.0.1 RLC RAM case)"
apply "$B/amdgpu/amdgpu_psp.c" "$V/amdgpu_psp.c" "wall-3 (amdgpu_psp.c: non-fatal PSP)"
apply "$B/bringup_params.c" "$ROOT/linuxkpi-drm/bringup_params.c" "firmware loader (bringup_params.c: upstream auto)"

# Wall 6: the substituted RLC TOC firmware (Phoenix ships no gc_11_0_1_toc.bin).
cp -f "$ROOT/firmware/amdgpu/psp_13_0_4_toc.bin" "$ROOT/firmware/amdgpu/gc_11_0_1_toc.bin"
echo "[walls] applied: wall-6 (firmware/amdgpu/gc_11_0_1_toc.bin)"

# Sanity: confirm the wall markers landed.
grep -q "gfx_v11_0_rae_find_toc_entries" "$V/gfx_v11_0.c" && echo "[walls] verify: wall-7 marker present" || { echo "[walls] wall-7 MISSING" >&2; exit 1; }
grep -q "IP_VERSION(11, 0, 1)" "$V/imu_v11_0.c" && echo "[walls] verify: wall-5 marker present" || { echo "[walls] wall-5 MISSING" >&2; exit 1; }
grep -q "amdgpu_fw_load_type = -1" "$ROOT/linuxkpi-drm/bringup_params.c" && echo "[walls] verify: upstream firmware auto marker present" || { echo "[walls] firmware auto marker MISSING" >&2; exit 1; }
echo "[walls] all walls 3-7 applied. Rebuild: FREESTANDING=1 bash linuxkpi-drm/m4c-link.sh"
