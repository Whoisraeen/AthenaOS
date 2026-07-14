#!/usr/bin/env bash
# One-shot VFIO amdgpu test — cap-delegation image (Claude, run: cap1).
# Boots the AthenaOS real-amdgpu image with the AMD GPU (c4:00.0) passed through,
# preserves serial CONTINUOUSLY, then reboots back to the default desktop entry.
set -u
cd /home/whoisraeen/raeen-vm
STAMP=cap11
IMG=kernel-real-amdgpu-cap.img
rm -f .vfio-test-armed                       # disarm FIRST — never loop
systemctl disable raeen-vfio-test.service 2>/dev/null || true
{
  echo "[autotest] $(date -Is) kernel=$(uname -r) run=$STAMP img=$IMG"
  echo "[autotest] GPU state: $(lspci -ks c4:00.0 | tr '\n' ' | ')"
  if ! lspci -ks c4:00.0 | grep -q "vfio-pci"; then
    echo "[autotest] GPU NOT bound to vfio-pci — wrong boot entry, skipping test"
  else
    ( while :; do cp -f /tmp/raeen-serial.log "serial-vfio-${STAMP}.log" 2>/dev/null; sync -f .; sleep 5; done ) &
    COPIER=$!
    systemd-inhibit --what=handle-power-key --who=raeen-vfio-test \
      --why="AthenaOS GPU test in progress — auto-reboots when done" \
      env IMG="$IMG" TIMEOUT=300 timeout 420 ./run-vfio.sh
    echo "[autotest] run-vfio.sh exit=$?"
    kill $COPIER 2>/dev/null
    cp -f /tmp/raeen-serial.log "serial-vfio-${STAMP}.log" 2>/dev/null || true
    echo "[autotest] serial saved: serial-vfio-${STAMP}.log ($(wc -l < "serial-vfio-${STAMP}.log" 2>/dev/null || echo 0) lines)"
    grep -acE "\[PANIC\]" "serial-vfio-${STAMP}.log" 2>/dev/null | sed 's/^/[autotest] panic lines: /'
    echo "[autotest] --- claim/seed trail ---"
    grep -aE "usdriver|ERR_NO_AUTHORITY|f50e|seeded driver authority|no AMD GPU" "serial-vfio-${STAMP}.log" 2>/dev/null | tail -12 | sed 's/^/[autotest] /'
    echo "[autotest] --- amdgpu init trail ---"
    grep -aE "amdgpu\]|msg: 90|CKPT|discovery|ATOM|VBIOS|PSP|SMU|MES|GART|gmc" "serial-vfio-${STAMP}.log" 2>/dev/null | tail -15 | sed 's/^/[autotest] /'
    echo "[autotest] --- DCN scanout / display trail (the display proof) ---"
    grep -aiE "page.?flip|scanout|DCN|HUBP|magenta|BYPASS|CRC|first.?light|register_scanout|panel|surface addr" "serial-vfio-${STAMP}.log" 2>/dev/null | tail -20 | sed 's/^/[autotest] /'
  fi
  echo "[autotest] done $(date -Is) — rebooting to default entry"
} > "autotest-${STAMP}.out" 2>&1
chown whoisraeen:whoisraeen "autotest-${STAMP}.out" "serial-vfio-${STAMP}.log" 2>/dev/null || true
sync
systemctl reboot
