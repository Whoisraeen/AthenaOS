#!/usr/bin/env bash
# Launch the real-amdgpu safe image build in WSL, backgrounded, logging to file.
set -u
cd ~/athenaos
LOG=~/athenaos/build-amdgpu.log
: > "$LOG"
echo "[build] starting $(date -Is) HEAD=$(git rev-parse --short HEAD)" >> "$LOG"
nohup bash -lc 'cd ~/athenaos && ATHENA_AMDGPU_REAL=1 cargo run -p xtask --release -- build --release --safe --uefi; echo "BUILD_EXIT=$?"' >> "$LOG" 2>&1 &
echo "BUILD_PID=$!"
