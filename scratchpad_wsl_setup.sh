#!/usr/bin/env bash
# Bring the WSL build clone to R: HEAD (c2526e8, tip of main) + cap patch.
set -eu
cd ~/raeenos
echo "=== fetching main from R: tree ==="
git fetch --no-tags --force /mnt/r/Projects/RaeenOS/.git main:refs/heads/amdgpu-run 2>&1 | tail -3
echo "=== checkout amdgpu-run ==="
git checkout -f amdgpu-run
echo "HEAD: $(git rev-parse --short HEAD)  (want c2526e8)"
echo "=== applying cap-fix patch ==="
git apply --stat /mnt/r/Projects/RaeenOS/scratchpad-cap-fix.patch
git apply /mnt/r/Projects/RaeenOS/scratchpad-cap-fix.patch
echo "=== verify cap fix landed ==="
grep -c "maybe_seed_driver_daemon" kernel/src/userspace_driver.rs kernel/src/syscall.rs
echo "=== status ==="
git status --short | head
echo "=== m4-obj sanity ==="
ls ~/m4-obj 2>/dev/null | wc -l
echo "SETUP_OK"
