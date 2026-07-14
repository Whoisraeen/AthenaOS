#!/usr/bin/env bash
cd ~/raeenos || exit 1
V=./linuxkpi-drm/vendor/linux-7.0.12/drivers/gpu/drm/amd/amdgpu
echo "===== UEFI_ACPI_VFCT + VFCT_IMAGE_HEADER struct defs ====="
grep -rnA12 "UEFI_ACPI_VFCT {" "$V" 2>/dev/null | head -20
grep -rnA14 "VFCT_IMAGE_HEADER {" "$V" 2>/dev/null | head -20
grep -rnA6  "GOP_VBIOS_CONTENT {" "$V" 2>/dev/null | head -12
echo "===== real VFCT.dat header dump ====="
D=firmware/acpi/athena-beelink-elitemini/VFCT.dat
ls -la "$D"
echo "-- sig(0..4), acpi length(4..8) --"
xxd -l 8 "$D"
echo "-- VBIOSImageOffset @ struct offset 36 (0x24) --"
xxd -s 36 -l 8 "$D"
echo "-- first 64 bytes --"
xxd -l 64 "$D"
