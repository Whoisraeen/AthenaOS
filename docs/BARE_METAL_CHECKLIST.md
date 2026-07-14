# Bare-Metal Testing Checklist

A pass/fail acceptance list for running AthenaOS on real hardware. Strict criteria — no "kind of works" passes here. Every line either has a measurable Boot artifact or it doesn't ship.

We hold this checklist to one curated machine first: **Beelink Athena (AMD Ryzen 7 8845HS, Zen 4 Phoenix, Radeon 780M iGPU, NVMe M.2)** because the hardware-profile dispatcher (`kernel/src/hardware_profile.rs`) already matches it and applies the right quirks. Once Athena is green, we extend to a second SKU (Framework 13 AMD), then a third (Minisforum UM790 Pro), then a desktop (Intel 12th/13th gen reference).

---

## Tier 0 — First boot. Without these, nothing happens.

| # | Test | Pass criterion | Status |
|---|------|----------------|--------|
| 0.1 | UEFI image boots from USB on Athena | Serial console shows BOOT-BENCH line | ❌ |
| 0.2 | Secure Boot disabled in firmware | Firmware setup screen confirmed | ❌ |
| 0.3 | Boot reaches kernel_main on real iron | Serial line `[ OK ] Serial (COM1 16550 UART)` | ❌ |
| 0.4 | GPT partition table parsed (or raw boot tolerated) | Doesn't panic on disk discovery | ❌ |
| 0.5 | GOP framebuffer accepted, mode logged | `[ OK ] Framebuffer: WxH @ Nbpp` | ❌ |
| 0.6 | Kernel triangle visible on screen | Photo of screen shows the red/green/blue triangle | ❌ |
| 0.7 | SMBIOS parses Athena's real DMI tables | `/proc/raeen/hardware` reports `match: beelink-athena` | ❌ |
| 0.8 | Hardware profile applies Zen 4 quirks at boot | `[hwprof] applied: QUIRK_AMD_TSC_DEADLINE_UNRELIABLE \| QUIRK_IGPU_ONLY \| QUIRK_AMD_ZEN4_SMCA` | ❌ |
| 0.9 | ACPI tables parse without panic | `[ OK ] ACPI tables parsed` | ❌ |
| 0.10 | MADT discovers all 16 logical CPUs of 8845HS | `[smp] bringing up 15 Application Processor(s)...` | ❌ |
| 0.11 | All 16 CPUs heartbeating | `/proc/raeen/smp` shows 16/16 cpu ticks > 0 | ❌ |
| 0.12 | CMOS RTC reads correct year | `[ OK ] RTC wall-clock: 2026-...` | ❌ |
| 0.13 | TSC calibration succeeds | `[apic] Calibrated TSC: <correct MHz>` matching the chip's nominal | ❌ |
| 0.14 | LAPIC timer fires on BSP | `cpu0: ticks > 0` after 1 second | ❌ |
| 0.15 | LAPIC timer fires on all APs | `cpu1..15: ticks > 0` after 1 second | ❌ |
| 0.16 | Kernel reaches "[ OS ] System successfully booted" | Serial last line confirms | ❌ |

**Known kernel gaps blocking Tier 0:**
- `_PIC(1)` AML call not made — some real boards stay in PIC mode and IRQs land at wrong vectors
- `_OSI("Windows 2020")` returns 0 — some DSDTs branch on this for fan control + battery
- x2APIC not implemented (xAPIC only) — fine up to 256 APIC IDs but blocks future server SKUs
- USB HID driver missing — Athena has no PS/2; UEFI keyboard emulation goes away after `ExitBootServices`. Once panic or login appears, you can't interact.

---

## Tier 1 — Useful boot. Tier 0 + ability to do anything.

| # | Test | Pass criterion | Status |
|---|------|----------------|--------|
| 1.1 | USB HID class driver works | Keyboard typing reaches serial echo via xHCI → USB-HID → input | ❌ |
| 1.2 | xHCI controller enumerated | `[xhci] controller online at <BDF>` (not "no controller found") | ❌ |
| 1.3 | NVMe driver detects real Samsung 980 / WD SN770 / Crucial P3 | `[nvme] controller: <real model name> serial=<real>` | ❌ |
| 1.4 | NVMe sector 0 round-trip | `[nvme] smoketest: read sector 0 (Ncycles) marker=<MBR signature or AthFS magic>` | ❌ |
| 1.5 | EDID parsing accepts monitor | `[gop] EDID: <vendor> <model> <native mode>` | ❌ |
| 1.6 | Battery presence detected via `_BIF`/`_BST` | `/proc/raeen/power` reports battery percentage | ❌ |
| 1.7 | AC adapter state detected | `[power] AC: present` (or `absent` if unplugged) | ❌ |
| 1.8 | Lid switch event handled | Closing lid logs `[acpi] lid closed` | ❌ |
| 1.9 | Thermal zone read | `/proc/raeen/thermal` reports CPU temp ≥ 0 | ❌ |
| 1.10 | Real network driver (e1000e or igc for Athena's I225) | `[net] e1000e/igc online, MAC=<real MAC>` | ❌ |
| 1.11 | DHCP DORA on real network | `state: Bound, lease_ip: <real DHCP-assigned IP>` | ❌ |
| 1.12 | HDA codec detected | `[audio] HDA codec at <BDF> vendor=<v>` | ❌ |
| 1.13 | S3 suspend → resume cycle | Kernel comes back, scheduler heartbeats resume | ❌ |
| 1.14 | Watchdog fires on infinite loop | Kernel panics with watchdog message instead of hard-hanging | ❌ |
| 1.15 | OOM handler doesn't just halt | `[mm] OOM: killing pid <N>` (no panic) | ❌ |
| 1.16 | Panic prints to screen, not just serial | Photo shows panic text on framebuffer | ❌ |

**Known gaps blocking Tier 1:**
- e1000e / igc / r8169 drivers don't actually receive frames (only virtio-net works)
- xHCI completed for QEMU emulated controller only; real Intel/AMD xHCI variants unverified
- ACPI battery polling thread doesn't exist (the parser does — never called)
- No OOM killer at all; running out of RAM hard-hangs

---

## Tier 2 — Installable. Tier 1 + persistent install + repeatable boot.

| # | Test | Pass criterion | Status |
|---|------|----------------|--------|
| 2.1 | Live USB boots kernel + ramdisk-only init | Serial log to `OS booted` from USB | ❌ |
| 2.2 | Installer userspace process discovers target NVMe | `installer: target=/dev/nvme0n1 size=<GB>` | ❌ |
| 2.3 | GPT partition table written to target | `parted -l` from a recovery USB shows ESP + AthFS partitions | ❌ |
| 2.4 | ESP formatted FAT32 | `mlabel`/`mdir` from recovery shows EFI/BOOT/BOOTX64.EFI | ❌ |
| 2.5 | AthFS formatted on target | AthFS magic + journal sequence visible in sector 0 of target partition | ❌ |
| 2.6 | Kernel written into ESP | First post-install boot finds the kernel | ❌ |
| 2.7 | Second power-on boots from NVMe (no USB inserted) | Serial confirms boot from NVMe | ❌ |
| 2.8 | AthFS root mounts from PARTUUID | `[ OK ] AthFS root mounted from <UUID>` | ❌ |
| 2.9 | Bootloader → kernel signature verified | `[secboot] kernel signature verified` (or `[secboot] disabled`) | ❌ |
| 2.10 | First-run setup completes | Userspace prompts username, locale, timezone; persists to AthFS | ❌ |
| 2.11 | Reboot preserves AthFS state | Second boot reads back what first boot wrote | ❌ |
| 2.12 | Atomic kernel update | Slot A → Slot B switch, boot fallback if new kernel doesn't reach userspace | ❌ |

**Known gaps blocking Tier 2:**
- No installer at all (userspace doesn't exist)
- No GPT writer in kernel
- No AthFS `mkfs` callable from anywhere
- No bootloader signing infrastructure
- No A/B slot management

---

## Tier 3 — Reliable on real iron. Tier 2 + stays up under load.

| # | Test | Pass criterion | Status |
|---|------|----------------|--------|
| 3.1 | 24 hour soak: kernel doesn't panic | `last_panic_at == None` after 24h | ❌ |
| 3.2 | 24 hour soak: no kernel OOM | Free pages within 10% of start | ❌ |
| 3.3 | 1000-cycle suspend/resume soak | All cycles complete, no degradation | ❌ |
| 3.4 | Stress-ng equivalent on all CPUs | All 16 CPUs at 100% for 1h, no scheduler stall | ❌ |
| 3.5 | Memory pressure test | Allocate until OOM kicks in, gracefully reclaim, system survives | ❌ |
| 3.6 | DMA from rogue driver bounded | Userspace driver attempting bad DMA blocked by IOMMU log entry | ❌ |
| 3.7 | PCIe AER correctable error logged | Inject via `setpci` from recovery, kernel logs `[aer] correctable`, continues | ❌ |
| 3.8 | PCIe AER uncorrectable handled | Inject, kernel logs `[aer] uncorrectable, isolating device`, doesn't panic | ❌ |
| 3.9 | Machine Check Exception survived | Inject MCE, kernel logs + continues (degrade rather than panic) | ❌ |
| 3.10 | Thermal throttling activates | CPU temp > T_target → frequency clamp, no shutdown | ❌ |
| 3.11 | Battery depletion → safe shutdown | At 5% remaining: warn; at 2%: clean shutdown to AthFS | ❌ |
| 3.12 | Crash dump written to disk | Forced panic produces parseable dump in AthFS `/var/crash` | ❌ |
| 3.13 | Watchdog reboots a wedged kernel | Hung kernel reboots within 30s instead of staying down | ❌ |
| 3.14 | Network: 10 Gbps stress to localhost | Sustained throughput with no packet loss in `/proc/raeen/network` | ❌ |
| 3.15 | Storage: 1 GB/s sustained reads via NVMe | `dd if=/dev/nvme0n1 of=/dev/null bs=1M count=10000` near nominal | ❌ |

**Known gaps blocking Tier 3:**
- IOMMU parser exists but enforcement is stubbed — any driver can DMA anywhere
- AER module is a stub; no actual error injection has ever been tested
- MCE handler panics on any MCE — no degradation path
- Watchdog module exists; not wired to any heartbeat
- Crash dump module exists; never written to disk

---

## Hardware coverage matrix

Each row is "what's our credible boot status on this SKU". Update after each test session.

| SKU | CPU | iGPU | NVMe | Wi-Fi | Status | Notes |
|-----|-----|------|------|-------|--------|-------|
| Beelink Athena | AMD Ryzen 7 8845HS | Radeon 780M | Samsung 980 / WD SN770 | Intel AX210 | ❌ untested | Primary target. Hardware profile dispatcher matches. |
| Framework 13 AMD | AMD Ryzen 7 7840U | Radeon 780M | varies | Intel AX210 | ❌ untested | Second SKU for portfolio. |
| Minisforum UM790 Pro | AMD Ryzen 9 7940HS | Radeon 780M | varies | varies | ❌ untested | Third desktop-class SKU. |
| Intel 12/13 gen reference | Alder/Raptor Lake | UHD 770 | varies | varies | ❌ untested | Intel coverage. Hybrid p/e-core scheduling untested. |

---

## How to run Tier 0 today

```powershell
# 1. Build UEFI image
cargo run -p xtask --release -- build --release

# 2. Write to USB stick (use diskpart to find the stick number; ↓ destroys all data)
$stick = "\\.\PhysicalDriveN"   # find via `Get-Disk`
$image = "C:\path\to\target\x86_64-unknown-none\release\kernel.uefi.img"
diskpart # clean + create partition + format FAT32
Copy-Item $image (Get-PSDrive U:).Root\EFI\BOOT\BOOTX64.EFI  # adjust

# 3. On Athena: F7 at boot → select USB → kernel boots
# 4. Capture serial output via USB-C UART dongle to host machine
# 5. Photo the screen at the first triangle frame
# 6. Compare serial log against this checklist line by line
```

---

## Acceptance gate

We do **NOT** declare bare-metal-ready until:
- Tier 0: 16/16 PASS on at least one SKU
- Tier 1: 16/16 PASS on at least one SKU
- Tier 2: 12/12 PASS on at least one SKU
- Tier 3: ≥ 12/15 PASS on at least one SKU (some items are nice-to-have for 1.0)

When that's true, we freeze the kernel and start the "perfect kernel for production" phase. Until then, every commit to `kernel/src/*` should ideally green up one more line on this checklist or eliminate a known gap.
