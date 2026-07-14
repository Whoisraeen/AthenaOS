#!/usr/bin/env bash
cd ~/athenaos || exit 1
f=$(find . -name amdgpu_bios.c 2>/dev/null | head -1)
echo "FOUND: $f"
if [ -n "$f" ]; then
  echo "===== amdgpu_acpi_vfct_bios ====="
  awk '/amdgpu_acpi_vfct_bios/{p=1} p{print} p&&/^}/{c++; if(c>=1 && /^}/) exit}' "$f" | head -90
  echo "===== struct VFCT_IMAGE_HEADER / UEFI_ACPI_VFCT ====="
  grep -rnA14 "VFCT_IMAGE_HEADER {" $(dirname "$f")/../include 2>/dev/null | head -40
  grep -rnA14 "} VFCT_IMAGE_HEADER" "$f" 2>/dev/null | head
fi
