#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────────────────
# RaeenOS no-flash KVM test loop.
#
# Builds on the dev box, ships the ~20 MB UEFI image to the Athena Mini PC
# (real Ryzen 5 7640HS, Zen4 family 0x19), KVM-boots it there with `-cpu host`
# so the AMD-family code paths that dev-box QEMU TCG (Family 0xF) skips actually
# run, then pulls the serial log back. Replaces the flash+reboot+human loop for
# everything except real-silicon driver work (which uses VFIO passthrough or a
# real flash). See memory: athena-kvm-noflash-loop.
#
# Usage:
#   scripts/athena-kvm.sh [minimal|full] [smp] [--no-build]
#     minimal  disk+serial only (fast, deterministic; default)
#     full     + virtio-net/gpu, qemu-xhci (kbd/mouse/hub/MSC), intel-hda
#     smp      vCPU count (default 1 — avoids the work-stealing race)
#     --no-build  skip the cargo build, ship the existing image
#
# Calibration boundaries (need iron, not KVM): AMD CPPC/SMCA MSR *values*,
# live SMU/SMN thermal, RTL8125 RX (the NIC is Athena's SSH lifeline).
# ──────────────────────────────────────────────────────────────────────────
set -euo pipefail

H="${RAEEN_ATHENA:-whoisraeen@192.168.1.244}"
DEVSET="${1:-minimal}"
SMP="${2:-1}"
NOBUILD="${3:-}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMG="$ROOT/target/x86_64-unknown-none/release/kernel.uefi.img"

if [ "$NOBUILD" != "--no-build" ]; then
  echo "[athena-kvm] building release image on dev box..."
  ( cd "$ROOT" && cargo run -p xtask --release -- build --release )
fi
[ -f "$IMG" ] || { echo "[athena-kvm] ERROR: $IMG not found (build first)"; exit 1; }

echo "[athena-kvm] shipping image -> $H:~/raeen-vm/ ($(du -h "$IMG" | cut -f1))"
scp -C "$IMG" "$H:raeen-vm/kernel.uefi.img"
# Ship the USB-MSC backing images once (only if absent on Athena).
for f in usb-msc.img usb-msc2.img; do
  if [ -f "$ROOT/target/$f" ]; then
    ssh "$H" "test -f raeen-vm/$f" || scp -C "$ROOT/target/$f" "$H:raeen-vm/$f"
  fi
done

echo "[athena-kvm] KVM boot on real Ryzen (DEVSET=$DEVSET SMP=$SMP)..."
ssh "$H" "cd raeen-vm && DEVSET=$DEVSET SMP=$SMP CPU=host TIMEOUT=120 ./run.sh"

OUT="$ROOT/target/athena-serial.log"
ssh "$H" 'cat /tmp/raeen-serial.log' > "$OUT"
echo "[athena-kvm] serial -> target/athena-serial.log ($(wc -l < "$OUT") lines)"
echo "[athena-kvm] marker: $(grep -c 'System successfully booted' "$OUT" || true)   panics: $(grep -cE '\[PANIC\]|\[EXCEPTION\].*FAULT' "$OUT" || true)"
