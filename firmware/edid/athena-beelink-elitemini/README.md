# Athena dev-setup panel EDID (real capture)

Captured 2026-06-12 from Windows on the Athena box itself
(`HKLM:\SYSTEM\CurrentControlSet\Enum\DISPLAY\SAM76E0\...\Device Parameters\EDID`).

| File | What |
|---|---|
| `sam76e0-256.bin` | Full EDID: base block + CTA-861 extension (modes, VRR range) |
| `sam76e0-128.bin` | Base block only (as exposed on the second display UID) |

Panel: Samsung SAM76E0, native 1920x1080 @ 180 Hz (Windows-confirmed current
mode). Block-0 checksum verified (sum mod 256 == 0).

Use exactly like `firmware/acpi/athena-beelink-elitemini/`: host-KAT the
kernel's `edid.rs` parser against these bytes (MasterChecklist Phase 2.3 —
"EDID + display modes" on a real monitor) before burning an iron flash on it.
The CTA extension in the 256-byte blob is the VRR-range source for the
compositor's `negotiate_vrr` (Phase 2.5 / 6.4).
