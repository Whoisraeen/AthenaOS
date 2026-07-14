#!/usr/bin/env bash
cd ~/raeenos || exit 1
VDIR=./linuxkpi-drm/vendor/linux-7.0.12
echo "===== find UEFI_ACPI_VFCT / VFCT_IMAGE_HEADER struct defs (whole vendor tree) ====="
grep -rn "UEFI_ACPI_VFCT\|VFCT_IMAGE_HEADER\|VBIOSImageOffset\|GOP_VBIOS_CONTENT" "$VDIR" 2>/dev/null | grep -iE "struct|typedef|offset|\{|u32|UINT32" | head -30
echo
echo "===== the actual header file with the struct ====="
f=$(grep -rln "VBIOSImageOffset" "$VDIR" 2>/dev/null | head -1); echo "FILE: $f"
[ -n "$f" ] && sed -n '/UEFI_ACPI_VFCT/,/} UEFI_ACPI_VFCT/p;/VFCT_IMAGE_HEADER/,/} VFCT_IMAGE_HEADER/p' "$f" | head -60
echo
echo "===== VFCT.dat: bytes at offset 36, 52, 76 and image header @76 ====="
D=firmware/acpi/athena-beelink-elitemini/VFCT.dat
echo "-- @36 (u32 LE) VBIOSImageOffset-if-std --"; xxd -s 36 -l 4 -e "$D"
echo "-- @52 (u32 LE) RaeenOS image-list-off --"; xxd -s 52 -l 4 -e "$D"
echo "-- @76 image header (PCIBus@+0,PCIDevice@+4,PCIFunction@+8,Vendor@+12,Device@+14,ImgLen@+24) --"
xxd -s 76 -l 28 "$D"
echo "-- @76 as LE u32s --"; xxd -s 76 -l 28 -e "$D"
