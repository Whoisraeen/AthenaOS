# ACPI table dump — Athena (Beelink EliteMini)

Raw ACPI tables captured from the RaeenOS **target hardware**. These are the
byte-for-byte firmware tables the RaeKernel ACPI parser (`kernel/src/acpi/`)
must consume without panicking on real silicon — see MasterChecklist Phase 1.4
("Athena DSDT without panic", `_PRT` packages, GPE `_Lxx`/`_Exx`, EC at 0x62/0x66).

## Provenance

| Field | Value |
|---|---|
| System | Beelink EliteMini Series (Micro Computer (HK) Tech Limited) |
| Board | Shenzhen Meigao Electronic Equipment Co. **F7BSI** |
| BIOS | AMI 1.08, 2024-11-04 |
| CPU | AMD Ryzen 5 7640HS w/ Radeon 760M (Phoenix) |
| Captured on | Windows 11 Pro 10.0.26200 |
| Captured | 2026-06-11 |

## Capture method

Pure userland, no kernel driver, two complementary sources merged:

1. **Windows firmware API** (`EnumSystemFirmwareTables`/`GetSystemFirmwareTable`,
   provider four-CC `0x41435049`) — yields every table listed in the XSDT
   (APIC, FACP, HPET, IVRS, MCFG, …) but **cannot** disambiguate same-signature
   tables, so it returns only one SSDT.
2. **`HKLM\HARDWARE\ACPI` registry hive** — Windows caches each table here and
   uniquely renames the duplicate SSDTs (`SSDT`, `SSD1`…`SSDL`), so this is the
   source of the 22 distinct SSDTs plus DSDT/FACS/FADT/XSDT.

These blobs are byte-identical to what `acpidump`/the linuxhw/ACPI project
collect on Linux.

## Contents (40 tables)

- **Core:** `DSDT`, `FACP` (FADT), `FACS`, `XSDT`, `APIC` (MADT), `MCFG`, `HPET`
- **AMD platform:** `IVRS` (AMD-Vi / IOMMU), `CRAT` + `CDIT` (NUMA topology),
  `VFCT` (AMD GPU video BIOS image), `FIDT`, `FPDT`
- **Security/boot:** `TPM2`, `WSMT`, `UEFI`, `BGRT`, `MSDM`
- **SSDTs (22, distinct):** `SSDT`, `SSD1`–`SSDL`

> Note: `XSDT.dat` came from the registry key labelled `RSDT`, but its header
> signature is `XSDT` (64-bit pointers) — renamed accordingly. The firmware-API
> `FACP.dat` and the registry `FADT.dat` were identical; the duplicate was dropped.

## Decoding

```powershell
# Disassemble AML to .dsl (needs ACPICA iasl)
iasl -d DSDT.dat
iasl -d SSD7.dat        # largest SSDT on this board (~39 KB)

# Decode static tables (MADT, FADT, IVRS, …)
iasl -d APIC.dat IVRS.dat FACP.dat
```
