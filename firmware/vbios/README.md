# Captured VBIOS images (device-keyed: `<vvvv>-<dddd>.bin`)

APUs have **no PCI expansion ROM** — firmware publishes the VBIOS through the
ACPI **VFCT** table instead (Linux: `amdgpu_acpi_vfct_bios`). Until the daemon
can read ACPI tables at runtime, the VFCT-extracted image is vendored here and
served through the normal `request_firmware` path; `bringup::read_vbios` falls
back to `vbios/<vendor>-<device>.bin` when the expansion-ROM map fails.

| File | Source | Validated |
|---|---|---|
| `1002-15bf.bin` | Extracted from Athena's real ACPI dump (`firmware/acpi/athena-beelink-elitemini/VFCT.dat`, captured on iron) — Radeon 760M (Phoenix1), 16896 bytes, AMD build date 09/15/23 | `55 AA` ROM sig, `ATOM` sig at hdr 0x194, byte-identity asserted by the `vfct_parses_real_athena_table` host KAT in `raeen_amdgpu::atombios` |

This is **machine-captured platform data** (like the ACPI and EDID dumps), not
linux-firmware content; AMD's VBIOS ships on the device/board itself and this
copy exists for the bring-up of that same machine.

Re-extract on any SKU: parse VFCT at offset 52 (u32 first-image offset), each
entry = 28-byte image header (`+12` u16 vendor, `+14` u16 device, `+24` u32
length) followed by the image bytes — or just run `atombios::parse_vfct`.
