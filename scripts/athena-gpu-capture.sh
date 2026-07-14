#!/usr/bin/env bash
# athena-gpu-capture.sh — READ-ONLY GPU/PSP ground-truth capture for the AthenaOS
# amdgpu bring-up, run on Athena (Arch Linux, amdgpu loaded, Radeon 760M/780M).
#
# Everything here is a READ. No register writes, nothing destructive. It dumps the
# state of a *working* amdgpu so the AthenaOS PSP firmware-load path can be built
# against ground truth instead of reverse-engineered blind.
#
#   sudo bash scripts/athena-gpu-capture.sh
#
# Then send the printed output file back. umr is the power tool (pacman -S umr, or
# the AUR umr-git); the dmesg/sysfs/lspci sections need nothing.
set -u
OUT="${SUDO_USER:+/home/$SUDO_USER}"; OUT="${OUT:-$HOME}/athena-gpu-capture-$(date +%Y%m%d_%H%M%S).txt"
exec > >(tee "$OUT") 2>&1
sec() { printf '\n========== %s ==========\n' "$1"; }

if [ "$(id -u)" -ne 0 ]; then echo "run with sudo (umr + debugfs need root)"; exit 1; fi

sec "SYSTEM"
uname -a
echo "cmdline: $(cat /proc/cmdline)"

sec "GPU PCI (expect 1002:15bf Phoenix1 / 760M)"
GPU=$(lspci -D -d 1002: | grep -iE 'VGA|Display' | awk '{print $1}' | head -1)
echo "GPU bdf = $GPU"
lspci -nnk -d 1002:
echo "--- full config + BAR sizes ---"
lspci -vvv -s "$GPU" 2>/dev/null | sed -n '1,40p'
echo "--- GPU iomem (BAR bases/sizes; confirms BAR5 register-aperture size) ---"
grep -iE "${GPU}|amdgpu" /proc/iomem 2>/dev/null || true

sec "AMDGPU DMESG (the real bring-up sequence + versions — the PSP blueprint)"
dmesg | grep -iE 'amdgpu|\[drm\]|\bpsp\b|\bimu\b|\bgfx\b|\bsmu\b|\brlc\b|\bmes\b|discovery|rmmio|BAR|doorbell|TMR|tmr|ucode|firmware' || true

# locate this GPU's debugfs dir
DRI=""
for d in /sys/kernel/debug/dri/*; do
  [ -e "$d/name" ] && grep -qi amdgpu "$d/name" 2>/dev/null && DRI="$d" && break
done
echo
echo "debugfs dri dir = ${DRI:-<none found>}"

sec "FIRMWARE VERSIONS (PSP sOS / IMU / RLC / ME / MEC / MES / SDMA / SMU / DMCUB)"
[ -n "$DRI" ] && cat "$DRI/amdgpu_firmware_info" 2>/dev/null || echo "no amdgpu_firmware_info"

sec "RINGS amdgpu actually created"
ls "$DRI"/amdgpu_ring_* 2>/dev/null | sed 's#.*/##' || echo "no ring files"

sec "FW ATTESTATION / DISCOVERY (if exposed)"
cat "$DRI/amdgpu_fw_attestation" 2>/dev/null | head -40 || echo "no fw_attestation"
ls "$DRI" 2>/dev/null | grep -iE 'discovery|vbios|psp' || true

# ---- umr: the money shots (best-effort; skips cleanly if umr/reg-name absent) ----
sec "UMR REGISTER READS (state of a WORKING GFX/PSP — confirms AthenaOS offsets)"
if ! command -v umr >/dev/null 2>&1; then
  echo "umr NOT installed.  Install:  sudo pacman -S umr   (or: yay -S umr-git)"
else
  umr --version 2>/dev/null | head -1
  # Read each register by name with bitfield decode. Wildcards let umr match the IP.
  for R in \
    regMP0_SMN_C2PMSG_81 regMP0_SMN_C2PMSG_58 regMP0_SMN_C2PMSG_35 regMP0_SMN_C2PMSG_33 \
    regRLC_RLCS_BOOTLOAD_STATUS regGFX_IMU_GFX_RESET_CTRL regGFX_IMU_CORE_CTRL \
    regGFX_IMU_I_RAM_ADDR regGFX_IMU_I_RAM_DATA regGFX_IMU_RLC_BOOTLOADER_ADDR_LO \
    regGRBM_STATUS regCP_ME_CNTL regCP_RB0_BASE regCP_RB0_CNTL regCP_RB0_WPTR \
    regRLC_SRM_CNTL regGRBM_SEC_CNTL ; do
    echo "--- $R ---"
    umr -i 0 -O bits -r "*.*.$R" 2>&1 | head -8 || true
  done
  echo "--- ring snapshot (gfx) ---"
  umr -i 0 -RS gfx_0.0.0 2>&1 | head -20 || true
fi

sec "DONE"
echo "Capture written to: $OUT"
echo "Send that file back."
