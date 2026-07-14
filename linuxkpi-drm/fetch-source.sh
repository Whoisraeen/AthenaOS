#!/usr/bin/env bash
# Fetch the upstream amdgpu source subtree into vendor/ (git-ignored).
#
# We pin kernel 7.0.12 because that is what the Athena reference machine runs
# (uname -r = 7.0.12-arch1-1) — the driver C must match the firmware blobs in
# firmware/amdgpu/ and the live register state we diff against in docs/gpu-oracle/.
#
# GPL boundary: this source is NEVER committed (see README.md). It lands in vendor/.
set -euo pipefail

VER="${AMDGPU_KVER:-7.0.12}"
MAJ="${VER%%.*}"
HERE="$(cd "$(dirname "$0")" && pwd)"
VENDOR="$HERE/vendor"
SRC="$VENDOR/linux-$VER"

if [ -f "$SRC/drivers/gpu/drm/amd/amdgpu/mes_v11_0.c" ]; then
    echo "[fetch] already present: $SRC"
    exit 0
fi

mkdir -p "$VENDOR"
cd "$VENDOR"
TARBALL="linux-$VER.tar.xz"
if [ ! -f "$TARBALL" ]; then
    echo "[fetch] downloading linux-$VER source from kernel.org ..."
    curl -fSL -o "$TARBALL" "https://cdn.kernel.org/pub/linux/kernel/v${MAJ}.x/linux-$VER.tar.xz"
fi

echo "[fetch] extracting the amd subtree + drm core (ttm/scheduler/allocators) + headers ..."
tar -xf "$TARBALL" \
    "linux-$VER/drivers/gpu/drm/amd" \
    "linux-$VER/drivers/gpu/drm/ttm" \
    "linux-$VER/drivers/gpu/drm/scheduler" \
    "linux-$VER/drivers/gpu/drm/drm_buddy.c" \
    "linux-$VER/drivers/gpu/drm/drm_exec.c" \
    "linux-$VER/drivers/gpu/drm/drm_file.c" \
    "linux-$VER/drivers/gpu/drm/drm_gem.c" \
    "linux-$VER/drivers/gpu/drm/drm_gem_ttm_helper.c" \
    "linux-$VER/drivers/gpu/drm/drm_ioctl.c" \
    "linux-$VER/drivers/gpu/drm/drm_managed.c" \
    "linux-$VER/drivers/gpu/drm/drm_auth.c" \
    "linux-$VER/drivers/gpu/drm/drm_prime.c" \
    "linux-$VER/drivers/gpu/drm/drm_syncobj.c" \
    "linux-$VER/drivers/gpu/drm/drm_internal.h" \
    "linux-$VER/drivers/gpu/drm/drm_crtc_internal.h" \
    "linux-$VER/drivers/dma-buf/dma-fence-unwrap.c" \
    "linux-$VER/drivers/gpu/drm/drm_mm.c" \
    "linux-$VER/drivers/gpu/drm/drm_vma_manager.c" \
    "linux-$VER/include/drm" \
    "linux-$VER/include/uapi/drm"

# Apply local bring-up patches over the pristine tree. These live in patches/
# (committed) because the vendored source itself is git-ignored; they fix upstream
# behaviour that our headless/constrained-BAR bring-up depends on. Only runs on a
# fresh fetch (the "already present" guard above exits before extraction on re-run).
if [ -d "$HERE/patches" ]; then
    for p in "$HERE"/patches/*.patch; do
        [ -f "$p" ] || continue
        echo "[fetch] applying patch $(basename "$p")"
        ( cd "$SRC" && patch -p1 < "$p" ) || { echo "[fetch] PATCH FAILED: $p"; exit 1; }
    done
fi

echo "[fetch] done -> $SRC"
ls -la "$SRC/drivers/gpu/drm/amd/amdgpu/mes_v11_0.c"
