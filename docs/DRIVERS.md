# RaeenOS Driver Registry — every driver we need, what it does, how to build it

**Purpose:** the single enumerated list of every device driver RaeenOS needs to be a
daily-driver gaming OS, with the development info required to build each one. This is
the *catalogue*; the *method* lives in its companions:

| Companion | Owns |
|---|---|
| `docs/NATIVE_DRIVER_PLAN.md` | How to write a driver from scratch in Rust (the `RaeDriver` trait, host-KAT discipline, the per-driver playbook, the selection layer) |
| `docs/LINUX_DRIVER_STRATEGY.md` | The LinuxKPI userspace path (Path C) for GPU + Wi-Fi, and the GPL/license boundary |
| `docs/HARDWARE_PATH.md` | Coverage roadmap, effort/LOC estimates, the bare-metal boot gate |
| `docs/FIRMWARE.md` | Which microcode blobs each driver loads via `request_firmware` (syscall 142) |
| `kernel/src/driver_manifest.rs` | The live HWID→driver matcher (`/proc/raeen/drivers`) |
| `MasterChecklist.md` | **Authoritative live status.** When a status here and there disagree, the MasterChecklist wins — re-verify before relying on a row below. |

> **Status is a snapshot (2026-06-15).** Every `[x]` claim must trace to an Athena
> bootlog (`logs/`); `[~]` is QEMU/host-KAT only. The MasterChecklist "IRON
> VERIFICATION" section is fresher than this file.

---

## 0. How to read this file

**Backend** — who wrote the device logic:
- **Native** — first-party Rust, from a public spec. Default where the spec is public and the surface is bounded (Concept-aligned: MPL-clean, audit-friendly, crash-isolatable).
- **LinuxKPI** — an unmodified Linux *userspace* driver hosted over the re-implemented MPL-2.0 `raeen_linuxkpi` shim (Path C). Reserved for black-box-firmware devices (GPU 3D, Wi-Fi MLME) where a from-scratch rewrite is a multi-year effort with no payoff.
- **Hybrid** — native modeset/scanout + LinuxKPI for 3D, or native data-plane + firmware blob.

**Residence** — where it runs:
- **Kernel** — frame/audio/storage/input hot paths only (profiling-justified). Uses `crate::capability`.
- **User** — IOMMU-sandboxed ELF daemon (the Concept default). `sys_claim_device` (syscall 111) + a per-device IOMMU domain. Anything that can fail without taking the box down.

**Status ladder** (strict — CLAUDE.md §14): `[x]` proven on Athena iron · `[~]` QEMU- or host-KAT-proven only · `[ ]` not started. "Compiles" is never a status.

---

## 1. Master matrix (at a glance)

| # | Driver | Class | Backend | Resid. | Status | Code / package |
|--:|---|---|---|---|:--:|---|
| **Bus & platform infrastructure** |
| 1 | PCIe ECAM / config space | bus | Native | Kernel | `[x]` | `pci.rs`, `pcie.rs` |
| 2 | MSI / MSI-X interrupts | bus | Native | Kernel | `[x]` | `pci_irq.rs`, `pcie.rs` |
| 3 | PCIe AER (error reporting) | bus | Native | Kernel | `[x]` | `pcie_aer.rs` |
| 4 | PCI power management (D-states/ASPM) | bus | Native | Kernel | `[~]` | `pci_pm.rs` |
| 5 | IOMMU (VT-d / AMD-Vi) | bus | Native | Kernel | `[~]` | `iommu.rs` |
| 6 | ACPI core (RSDP/MADT/FADT/AML) | platform | Native | Kernel | `[x]` | `acpi_full.rs`, `acpi.rs` |
| 7 | SMBIOS / DMI | platform | Native | Kernel | `[x]` | `smbios.rs` |
| 8 | ACPI Embedded Controller (0x62/0x66) | platform | Native | Kernel | `[~]` | `acpi_full.rs` (EC) |
| 9 | RTC / CMOS clock | platform | Native | Kernel | `[x]` | (in `acpi`/time path) |
| 10 | I²C / SMBus controller | platform | Native | Kernel | `[~]` | `i2c_spi.rs` |
| 11 | GPIO / pinctrl | platform | Native | Kernel | `[ ]` | (in `i2c_spi`/platform) |
| 12 | Watchdog (AMD-EFCH / Intel TCO) | platform | Native | Kernel | `[x]` | `watchdog.rs` |
| 13 | MCE / machine-check | platform | Native | Kernel | `[~]` | `mce.rs` |
| 14 | TPM 2.0 (TIS/CRB) | security | Native | Kernel | `[~]` | `tpm.rs` |
| **Storage** |
| 15 | NVMe | storage | Native | Kernel | `[x]` | `nvme.rs` |
| 16 | AHCI / SATA | storage | Native | Kernel | `[~]` | `ahci.rs` |
| 17 | virtio-blk | storage | Native | Kernel | `[~]` | `virtio.rs` |
| 18 | SD / eMMC (SDHCI) | storage | Native | User | `[ ]` | — |
| 19 | USB Mass Storage (BOT/UASP) | storage | Native | Kernel | `[ ]` | `xhci.rs` (MSC class) |
| **USB** |
| 20 | xHCI host controller | usb | Native | Kernel | `[x]` | `xhci.rs` |
| 21 | USB hub (2.0 + 3.x) | usb | Native | Kernel | `[~]` | `xhci.rs` (hub) |
| 22 | USB HID (keyboard/mouse) | usb/input | Native | Kernel | `[x]` | `xhci.rs`, `input.rs` |
| 23 | USB HID gamepad | usb/input | Native | Kernel | `[~]` | `hid_gamepad.rs`, `input.rs` |
| 24 | USB Audio Class (UAC1/2) | usb/audio | Native | User | `[~]` | `audio.rs` (usb_audio) |
| 25 | USB Video Class (UVC webcam) | usb/media | Native | User | `[ ]` | — |
| 26 | USB-Serial (CDC-ACM/FTDI) | usb | Native | User | `[ ]` | — |
| **Networking — wired** |
| 27 | virtio-net | net | Native | Kernel | `[~]` | `virtio_net.rs`, `net_drivers.rs` |
| 28 | Intel e1000 / e1000e (I219) | net | Native | Kernel | `[~]` | `net_drivers.rs` |
| 29 | Intel igc (I225-V / I226-V 2.5G) | net | Native | Kernel | `[~]` | `igc.rs` |
| 30 | Realtek RTL8125 (2.5G) | net | Native | Kernel | `[x]` | `net_drivers.rs` (rtl8125) |
| 31 | Realtek RTL8168/8169 (1G) | net | Native | Kernel | `[ ]` | — (planned native) |
| 32 | Broadcom tg3 / bnxt | net | Native/LinuxKPI | User | `[ ]` | — |
| 33 | Aquantia/Marvell AQC (10G) | net | LinuxKPI | User | `[ ]` | — |
| **Networking — wireless** |
| 34 | Intel iwlwifi (AX2xx/BE2xx) | wifi | LinuxKPI | User | `[ ]` | `iwlwifi.elf` |
| 35 | MediaTek mt76 (7921/7922/7925) | wifi | LinuxKPI | User | `[ ]` | `mt7921.elf` |
| 36 | MediaTek MT7902 (Athena's chip) | wifi | — | User | `[ ]` | **no driver exists** — see §10 |
| 37 | Realtek rtw89 | wifi | LinuxKPI | User | `[ ]` | `rtw89.elf` |
| 38 | Broadcom brcmfmac | wifi | LinuxKPI | User | `[ ]` | `brcmfmac.elf` |
| **GPU / display** |
| 39 | AMD GPU (amdgpud — RDNA, Phoenix iGPU) | gpu | Hybrid | User | `[~]` | `amdgpud/` (daemon), `components/raeen_amdgpu` (lib) |
| 40 | Intel GPU (i915d / Xe) | gpu | Hybrid | User | `[~]` | `i915d/` (daemon) |
| 41 | NVIDIA (nouveau, open) | gpu | LinuxKPI | User | `[ ]` | `nouveau.elf` |
| 42 | Native modeset/scanout (per vendor) | gpu | Native | Kernel | `[~]` | `gpu.rs`, `display.rs` |
| 43 | virtio-gpu | gpu | Native | Kernel | `[~]` | `virtio_gpu.rs` |
| 44 | Generic VGA / Bochs VBE / GOP fb | gpu | Native | Kernel | `[x]` | `gpu.rs`, framebuffer |
| 45 | EDID / DDC-I²C monitor detect | display | Native | Kernel | `[~]` | `edid.rs`, `display.rs` |
| 46 | Backlight / brightness | display | Native | Kernel | `[~]` | `display.rs` |
| **Audio** |
| 47 | Intel HD Audio (HDA codec + PCM) | audio | Native | Kernel | `[x]` | `audio.rs` |
| 48 | USB Audio (UAC2) | audio | Native | User | `[~]` | `audio.rs` |
| 49 | Bluetooth audio (A2DP/HFP) | audio | Native | User | `[ ]` | `bluetooth.rs` (+ A2DP) |
| 50 | AMD/Realtek codec topology quirks | audio | Native | Kernel | `[~]` | `audio.rs` |
| **Input / HID** |
| 51 | PS/2 i8042 keyboard + mouse | input | Native | Kernel | `[~]` | `input.rs` |
| 52 | I²C-HID touchpad/touchscreen | input | Native | Kernel | `[ ]` | `i2c_spi.rs` (+ HID) |
| 53 | DualSense (PS5) full feature | input | Native | Kernel | `[~]` | `input.rs` |
| 54 | Xbox (XInput + GIP) | input | Native | Kernel | `[~]` | `input.rs` |
| 55 | Generic HID gamepad (report-descriptor) | input | Native | Kernel | `[~]` | `hid_gamepad.rs` |
| **Platform / sensors / power** |
| 56 | Thermal zones + fan control | platform | Native | Kernel | `[~]` | `thermal.rs` |
| 57 | Battery / AC (fuel gauge, _BST) | platform | Native | Kernel | `[~]` | `battery.rs`, `power_supply.rs` |
| 58 | Power button / lid / GPE events | platform | Native | Kernel | `[~]` | `power_events.rs`, `power.rs` |
| 59 | S3 / S0ix suspend-resume | platform | Native | Kernel | `[ ]` | `power.rs` |
| 60 | RGB unified (mobo/fan/peripheral) | platform | Native | Kernel | `[~]` | `rgb.rs` |
| **Connectivity / peripherals** |
| 61 | Bluetooth (btusb + HCI + L2CAP) | bt | Hybrid | User | `[~]` | `bluetooth.rs` |
| 62 | Card reader (USB/PCIe) | misc | Native | User | `[ ]` | — |
| 63 | Fingerprint reader | security | LinuxKPI | User | `[ ]` | — |
| 64 | Printers / scanners | misc | (userspace IPP) | User | `[ ]` | — (not a kernel driver) |

---

## 2. Bus & platform infrastructure

These aren't device drivers in the user sense — they're what every *other* driver stands on. If these are wrong, nothing binds.

### 1. PCIe ECAM / config space — `pci.rs`, `pcie.rs` — `[x]`
- **Does:** enumerates the PCI(e) bus, reads config space (legacy 0xCF8/0xCFC and MMIO ECAM via the ACPI MCFG table), exposes `PciDevice {vendor,device,class,subclass,bars}` to every driver and to `driver_manifest`.
- **Spec:** PCI Local Bus 3.0, PCIe Base 5.0, ACPI MCFG.
- **Dev info:** BAR sizing (write all-ones, read back mask), capability-list walk (cap id 0x10 = PCIe, 0x05 = MSI, 0x11 = MSI-X). Proof: `/proc/raeen/pci`; iron shows full device tree.
- **Gotcha:** ECAM base comes from MCFG; some firmware hides it — fall back to 0xCF8.

### 2. MSI / MSI-X — `pci_irq.rs`, `pcie.rs` — `[x]`
- **Does:** allocates interrupt vectors, programs the MSI/MSI-X capability (message address/data), routes to the right CPU's IDT vector. 256-vector bitmap allocator.
- **Dev info:** every modern multi-queue device (NVMe, igc, xHCI) needs this. MSI-X table lives in a BAR at the cap's table-offset. Legacy INTx + `_PRT` routing is the fallback (84 `_PRT` entries parsed on Athena).

### 3. PCIe AER — `pcie_aer.rs` — `[x]`
- **Does:** walks the Advanced Error Reporting extended cap, logs correctable/uncorrectable errors instead of silently corrupting. Iron: `aer_devices=4 scanned=45 corr_bits=4`.
- **Spec:** PCIe Base §6.2, AER ext-cap 0x0001.

### 4. PCI power management — `pci_pm.rs` — `[~]`
- **Does:** D0–D3 device power states, ASPM link power. Needed for laptop battery life.
- **Open:** real runtime PM (D3cold, L1.2) untested on iron.

### 5. IOMMU (VT-d / AMD-Vi) — `iommu.rs` — `[~]`
- **Does:** per-device DMA remapping domains — the Concept §Security mandate that makes userspace drivers safe (a buggy/hostile driver can't DMA over kernel memory).
- **Spec:** Intel VT-d, AMD I/O Virtualization (IOMMUv2). DMAR/IVRS ACPI tables.
- **Status:** DTE/devtab/cmdbuf selftests PASS on iron (AMD-Vi); **enforcement per-driver is Phase 4** — not every driver maps DMA through a domain yet.
- **Gotcha (CLAUDE.md §10.12):** no `intel-iommu` device in xtask's default QEMU args — it often breaks boot; add only for explicit VT-d testing.

### 6. ACPI core — `acpi_full.rs`, `acpi.rs` — `[x]`
- **Does:** RSDP→XSDT→{MADT, FADT, MCFG, HPET, SRAT, VFCT…}, the AML interpreter (vendored `components/vendored/aml`), `_PRT` IRQ routing, GPE, EC discovery, `_PIC`, `_OSI`.
- **Iron:** `tables=37 devices=159 values=2818`, 84 `_PRT` entries, `\_PIC -> Integer(0)`.
- **Dev info:** AML parser bugs go in `components/vendored/aml`, repro'd via `tools/aml_probe` against `firmware/acpi/athena-beelink-elitemini/` (ms iteration, no flash). Open: method-runtime opcodes (Acquire/Release/Sleep/Stall) for some method bodies.

### 7. SMBIOS / DMI — `smbios.rs` — `[x]`
- **Does:** parses the SMBIOS entry point (from the EFI config table post-KASLR), exposes board/BIOS/OEM strings for per-machine quirk dispatch (`beelink-athena` profile).

### 8. ACPI Embedded Controller — `acpi_full.rs` (EC) — `[~]`
- **Does:** the 0x62/0x66 EC interface laptops use for battery, thermal, lid, hotkeys, fan. EC discovered on iron; AML `_Qxx` query dispatch is the open part.
- **Spec:** ACPI §12 (Embedded Controller).

### 9. RTC / CMOS — time path — `[x]`
- **Does:** wall-clock at the real epoch; TSC-anchored. Trivial but load-bearing for TLS/file timestamps.

### 10. I²C / SMBus — `i2c_spi.rs` — `[~]`
- **Does:** the board's I²C/SMBus controller — the transport under DDC/EDID, SMBus RGB/fan controllers, I²C-HID touchpads, battery fuel gauges.
- **Spec:** SMBus 3.0; controller is often the AMD FCH or Intel PCH SMBus function.

### 11. GPIO / pinctrl — `[ ]`
- **Does:** general-purpose pins for board glue (some touchpad interrupts, some LED/fan lines). Mostly described by ACPI `_CRS`. Low priority until a board needs it.

### 12. Watchdog — `watchdog.rs` — `[x]`
- **Does:** hardware watchdog that reboots a hung box. Iron-proven: `amd-efch ... countdown 300 -> 297` (WDT at FED80B00, SMBus 00:14.0). Intel TCO is the equivalent on Intel boards.

### 13. MCE / machine-check — `mce.rs` — `[~]`
- **Does:** reads the 32 MCA banks (Athena), classifies correctable vs fatal, logs instead of triple-faulting. Real fault-path wiring (survive, don't panic) is Phase 4.

### 14. TPM 2.0 — `tpm.rs` — `[~]`
- **Does:** the TPM the Concept §Security wants for hardware-backed encryption (TIS or CRB interface, MMIO at 0xFED40000). PCR extend, measured boot.
- **Spec:** TCG TPM 2.0, PC Client Platform TIS/CRB.

---

## 3. Storage

### 15. NVMe — `nvme.rs` — `[x]` (iron)
- **Does:** the primary boot/root device on every modern gaming PC. Admin + I/O submission/completion queues, namespace enumeration, block read/write.
- **Spec:** NVMe 1.4 / 2.0 (public). ~2,500 LOC native.
- **Iron:** LBA0 read on the real Samsung-class SSD; ESP writes for `BOOTLOG.TXT`.
- **Gotcha (CLAUDE.md §10.8):** credit `cpl.sq_head` back to your ring or the controller silently wedges after N commands (only showed on iron). Don't read SMART while holding `NVME_CONTROLLERS.lock` (boot hang — `nvme-smart-boot-hang` memory).
- **Open:** multi-namespace, more vendor admin quirks; one non-fatal `SC=12` on Athena.

### 16. AHCI / SATA — `ahci.rs` — `[~]`
- **Does:** SATA SSD/HDD on the AMD FCH / Intel PCH SATA controller. Command list + FIS, port multiplier.
- **Spec:** AHCI 1.3.1, SATA 3.x.
- **Status:** works on QEMU ICH9; **AHCI on AMD SATA is unproven on iron**. Use `--disk=smoketest` (BIOS boot + nvme/ahci present) to exercise the code path in QEMU.

### 17. virtio-blk — `virtio.rs` — `[~]`
- **Does:** the QEMU/VM block device — the default dev disk. Not a real-hardware target but the cheapest data-plane proof.

### 18. SD / eMMC (SDHCI) — `[ ]`
- **Does:** card readers / eMMC on handhelds (Steam-Deck-class, the Rae Station). SDHCI standard host controller.
- **Spec:** SD Host Controller Standard 4.x.

### 19. USB Mass Storage (BOT + UASP) — `xhci.rs` MSC class — `[ ]` (Phase 2.1)
- **Does:** install media + external drives. Bulk-Only Transport (and UASP for speed) over xHCI bulk endpoints, wrapping SCSI READ/WRITE(10).
- **Status:** blocked on USB3-hub child-probe timeouts on Athena (`GetPortStatus` timeouts). **Every new block write path MUST route through `block_io::safe_mode_guard_write`** (CLAUDE.md §9 — a non-safe image once wiped a Windows partition; commit 4d228c8 closed the USB-MSC hole).

---

## 4. USB

### 20. xHCI — `xhci.rs` — `[x]` (iron)
- **Does:** the USB 3.x host controller — the root of all USB. Command/event/transfer rings, slot/endpoint contexts, port enumeration.
- **Spec:** xHCI 1.2, USB 2.0/3.2.
- **Iron:** 4/4 controllers bound, HID keyboard armed, hubs enumerated.
- **Gotchas:** the **live** driver is `kernel/src/xhci.rs`; `kernel/src/usb/xhci.rs` is dead scaffold — never wire it (CLAUDE.md §10.4). Credit transfer-ring consumption back (phantom-full, §10.8). Max-ESIT-Payload fix (9c86947) cleared the Razer ConfigureEndpoint ParameterError. HCE 5ms-grace fix avoids the 100ms-per-dead-probe grind on QEMU's empty USB3 hub ports.

### 21. USB hub (2.0 + 3.x) — `xhci.rs` hub — `[~]`
- **Does:** port trees behind a hub — required for real-world machines (Athena's HID sits behind hubs). `bPwrOn2PwrGood`-honoring power settle, SET_HUB_DEPTH, per-port enumeration.
- **Open:** USB3-hub child probes still time out on some hubs (worked-around by skipping a hub that fails SET_HUB_DEPTH; a real HCRST recovery is a deeper follow-up).

### 22. USB HID keyboard/mouse — `xhci.rs` + `input.rs` — `[x]` (iron)
- **Does:** the Tier-0 input path — without it a post-2015 laptop is a mute machine (no PS/2). Boot + report protocol, interrupt-IN endpoint → `service_hid_reports` → `usb_hid` → `shell_runner::handle_key`. SCHED_GAME latency.
- **Iron:** keyboard armed (5th boot); live typing test pending. Open: LowSpeed keyboard 18-byte `GET_DESCRIPTOR` stall (8-byte-header fallback fix, iron pending — `xhci-lowspeed-kbd-fallback-order` memory).

### 23. USB HID gamepad — `hid_gamepad.rs` + `input.rs` — `[~]`
- **Does:** arbitrary pads via HID report-descriptor parsing (learns axis/button bit layout from the device). Feeds `GamepadState::Generic`.
- **Status:** parser host-KAT'd + iron report-parse proven; **live interrupt-IN binding pending iron**.

### 24. USB Audio Class — `audio.rs` (usb_audio) — `[~]`
- **Does:** USB headsets/DACs. UAC1/2 isochronous transfers. Sub-3ms RaeAudio path candidate.
- **Spec:** USB Audio Class 2.0. Isoch endpoint scheduling over xHCI.

### 25. USB Video Class (UVC webcam) — `[ ]`
- **Does:** webcams (built-in laptop cams are usually UVC over USB). Small standard surface (Path A per HARDWARE_PATH).
- **Spec:** USB Video Class 1.5.

### 26. USB-Serial (CDC-ACM / FTDI / CP210x) — `[ ]`
- **Does:** dev boards, some peripherals; a bring-up convenience (USB-C UART). Low priority.

---

## 5. Networking — wired

The gateway to *all* networking is one working NIC reaching DHCP `Bound`.

### 27. virtio-net — `virtio_net.rs`, `net_drivers.rs` — `[~]`
- **Does:** the QEMU NIC — full DORA on QEMU (10.0.2.15 lease). Dev path only.
- **Gotcha:** `virtio_net_hdr` is 10 bytes (the fix that unblocked RX). DHCP OFFER routing must call `dhcp::handle_eth_frame` in *both* the VirtioNet and NetDriver branches (the iron bug — `rtl8125-rx-and-net-poll` / the 2026-06-15 OFFER-routing fix).

### 28. Intel e1000 / e1000e (I219) — `net_drivers.rs` — `[~]`
- **Does:** very common Intel 1G NICs (I219 on many laptops/desktops). `recv()` implemented.
- **Status:** unproven on iron. ~4,000 LOC native covers most Intel NICs.
- **Spec:** Intel 8254x/8257x SDM (public).

### 29. Intel igc (I225-V / I226-V 2.5G) — `igc.rs` — `[~]`
- **Does:** the 2.5G NIC on many gaming boards. Multi-queue + MSI-X.
- **Spec:** Intel Foxville I225/I226 datasheet.

### 30. Realtek RTL8125 (2.5G) — `net_drivers.rs` (rtl8125) — `[x]` (iron)
- **Does:** the cheap-but-ubiquitous 2.5G Realtek NIC — **Athena's wired link**. TX + RX live on iron.
- **Gotcha (the saga):** posted-write-needs-readback — read `RTL_RCR`/`RTL_CR` after RX-enable to flush the receiver-on write. Nothing drove `net::poll_full()` post-boot → added `net::spawn_poll_thread()`. DHCP lifecycle: use hpet-monotonic seconds for lease renewal, install the lease from `dhcp::tick` after `poll()` frees `NET_STACK`. (`rtl8125-rx-and-net-poll` memory.)

### 31. Realtek RTL8168/8169 (1G) — `[ ]`
- **Does:** the most common cheap 1G NIC on Earth. Public datasheet → native (~5,000 LOC). Today `driver_manifest` routes it to a LinuxKPI `r8169` package as a placeholder.

### 32. Broadcom tg3 / bnxt — `[ ]`
- **Does:** enterprise/workstation NICs. ~8,000 LOC native, or LinuxKPI userspace.

### 33. Aquantia / Marvell AQC (10G) — `[ ]`
- **Does:** 10G on high-end boards. LinuxKPI userspace candidate.

---

## 6. Networking — wireless (all LinuxKPI / Path C)

Native Wi-Fi is 30,000+ LOC per vendor (opaque firmware + huge MLME/regulatory surface) — the Concept blesses the userspace LinuxKPI shim here. Each runs as an ELF daemon over `raeen_linuxkpi`, talking to `wpa_supplicant`-class WPA2/WPA3 negotiation above it.

### 34. Intel iwlwifi (AX200/AX210/BE200) — `iwlwifi.elf` — `[ ]`
- **Does:** the most common gaming-laptop Wi-Fi. The **recommended path for Athena** is to swap the M.2 2230 for an **Intel AX210**, then drop `iwlwifi-ty-a0-gf-a0-<NN>.ucode` into `firmware/`.
- **Firmware:** per-device ucode from linux-firmware.

### 35. MediaTek mt76 (7921/7922/7925) — `mt7921.elf` — `[ ]`
- **Does:** common MediaTek Wi-Fi 6/6E. mt76 driver family.

### 36. MediaTek MT7902 (`14C3:7902`) — **no driver exists** — `[ ]`
- **Reality (Athena ground truth, `docs/ATHENA_GROUND_TRUTH.md`):** Athena's actual Wi-Fi is an MT7902, which has **no linux-firmware blob and no mainline Linux driver** (mt76 doesn't cover 7902). The userspace-LinuxKPI plan cannot apply to this exact chip. **Mitigation: Ethernet-first (RTL8125), or swap to AX210, or a supported USB Wi-Fi dongle.**

### 37. Realtek rtw89 — `rtw89.elf` — `[ ]`  ·  ### 38. Broadcom brcmfmac — `brcmfmac.elf` — `[ ]`
- Common Realtek / Broadcom Wi-Fi; LinuxKPI userspace + firmware blob.

---

## 7. GPU / display

The single biggest open Concept Year-1 deliverable: a **real GPU submit path** (`vkQueueSubmit`-equivalent) instead of the current software raster. Strategy: **native modeset/scanout** (small, doable — set a mode, flip a buffer, move the cursor) + **LinuxKPI userspace Mesa** for 3D/Vulkan (RADV/RadeonSI/Iris). Never a from-scratch 3D driver.

### 39. AMD GPU (amdgpud) — `amdgpud/` (daemon, repo root), `components/raeen_amdgpu` (host-testable bring-up lib) — `[~]`
- **Does:** the Athena iGPU (Radeon 780M, Phoenix, GC 11.0.1) and discrete RDNA. Userspace LinuxKPI daemon: GMC/IH/SMU/GFX bring-up, then Mesa RADV on top. Supervised by `driver_supervisor/` (repo root).
- **Firmware (`docs/FIRMWARE.md`, vendored at `firmware/amdgpu/`):** PSP TOC/TA, GC 11.0.1 IMU/PFP/ME/MEC/RLC/MES, SDMA 6.0.1, DCN 3.1.4 DMCUB, VCN 4.0.2. **No `smu_13_0_4.bin`** — APU SMU is embedded in the system BIOS, loaded by PSP. VBIOS comes from the ACPI **VFCT** table (no PCI ROM on APUs), vendored at `firmware/vbios/1002-15bf.bin`.
- **Status:** real-Athena-blob host-KAT'd bring-up sequence; **iron BAR5 bring-up failure currently reproduces deterministically** (recent `logs/` captures) — active debug. The `iron-pending-offset` pattern: no guessed MMIO pre-iron (`amdgpu-bringup-host-kat` memory).
- **Dev info:** `tools/linuxkpi_harness` replays the real bring-up against a mock register file on the host; `raeen_amdgpu::bringup` over a `GpuOps` trait is the testable lib.

### 40. Intel GPU (i915d / Xe) — `i915d` — `[~]`
- **Does:** Intel UHD/Iris/Xe iGPU. LinuxKPI daemon + Mesa Iris. GuC/HuC firmware (`firmware/i915/`).
- **Status:** scaffold; firmware preflight wiring shared with amdgpud.

### 41. NVIDIA nouveau (open) — `nouveau.elf` — `[ ]`
- **Does:** NVIDIA basic via the open nouveau driver (no proprietary blob — that needs a Linux ABI we don't provide). Modeset + limited accel.

### 42. Native modeset / scanout — `gpu.rs`, `display.rs` — `[~]`
- **Does:** the small native display path: set a mode, flip a framebuffer, move a hardware cursor — a real framebuffer *before* the full LinuxKPI 3D stack lands. NOT 3D.
- **Gotcha (CLAUDE.md §10.7):** multi-page GPU buffers need `allocate_contiguous_frames(order)` — a loop of `allocate_frame()` is non-contiguous and trampled the heap (the `create_scanout` wild-write that looked like an SMP bug). Probe iGPU config-space only at boot — reading BAR0 MMIO data-fabric-stalls on the AMD APU (the Tier-8 freeze fix).

### 43. virtio-gpu — `virtio_gpu.rs` — `[~]`  ·  ### 44. Generic VGA / Bochs VBE / GOP — `gpu.rs` — `[x]`
- virtio-gpu for VMs; GOP framebuffer is the guaranteed fallback (the boot login screen renders into it). Software raster today.

### 45. EDID / DDC-I²C — `edid.rs`, `display.rs` — `[~]`
- **Does:** read the monitor's EDID over DDC/I²C for native resolution/refresh/HDR caps + VRR ranges. Parser host-KAT passed; **real-monitor read pending iron** (Phase 2.3).

### 46. Backlight / brightness — `display.rs` — `[~]`
- **Does:** panel brightness (ACPI `_BCL`/`_BCM` or GPU native). Laptop essential.

---

## 8. Audio

### 47. Intel HD Audio (HDA) — `audio.rs` — `[x]` (iron, partial)
- **Does:** the analog/HDMI audio on virtually every desktop/laptop. CORB/RIRB command rings, codec widget-graph walk, ring-buffer DMA PCM. The RaeAudio sub-3ms target path.
- **Spec:** Intel High Definition Audio 1.0a (public). ~3,000 LOC.
- **Iron:** real codec playback (`wrote_samples=960 hda_playback=1`). **Open:** `codec-walk: dac=true output_pin=false -> FAIL` — output-pin widget detection on this codec topology.

### 48. USB Audio (UAC2) — `audio.rs` — `[~]` — see §4.24.
### 49. Bluetooth audio (A2DP/HFP) — `bluetooth.rs` — `[ ]` — A2DP sink/source + SBC/AAC over the BT stack (§9.61).
### 50. Codec topology quirks — `audio.rs` — `[~]` — per-board pin-config/verb quirks (AMD ACP, Realtek ALC codecs).

---

## 9. Input / HID, platform/sensors, Bluetooth

### 51. PS/2 i8042 — `input.rs` — `[~]`
- **Does:** the legacy keyboard/mouse fallback when there's no USB HID. Tiny (IRQ1/IRQ12). Many post-2015 laptops omit it — detect-absent-and-don't-panic is the rule.

### 52. I²C-HID touchpad/touchscreen — `i2c_spi.rs` + HID — `[ ]`
- **Does:** modern laptop touchpads/touchscreens (HID-over-I²C, ACPI-described). Hundreds of variants but each tiny; precision-touchpad gestures feed the compositor.
- **Spec:** Microsoft HID-over-I²C + HID precision touchpad.

### 53. DualSense (PS5) — `input.rs` — `[~]`
- **Does:** full feature parity (Concept §Gaming): sticks/triggers/buttons/gyro/accel/touchpad/battery in; rumble/LED/7 adaptive-trigger modes/player-LEDs/mic-LED/haptics out. Parser + output-report build regression-fenced + iron-proven; **live USB isoch haptics/triggers pending iron**.

### 54. Xbox (XInput + GIP) — `input.rs` — `[~]` — GIP report parser + rumble packet builder; parser iron-proven, live GIP transfer pending.
### 55. Generic HID gamepad — `hid_gamepad.rs` — `[~]` — see §4.23.

### 56. Thermal + fan — `thermal.rs` — `[~]`
- **Does:** thermal zones (ACPI `_TMP`/`_TZ` and CPU/GPU sensors) + fan curves at the OS layer (Concept §Customization — no MSI Afterburner/Armoury Crate sprawl). EC/PWM fan control.

### 57. Battery / AC — `battery.rs`, `power_supply.rs` — `[~]`
- **Does:** fuel-gauge percentage, charge/discharge, AC presence (ACPI `_BST`/`_BIF` via the EC). Fuel-gauge math host-KAT'd 10/10; **`_BST` polling on iron is the open wiring**.

### 58. Power button / lid / GPE — `power_events.rs`, `power.rs` — `[~]`
- **Does:** dispatch ACPI General Purpose Events (`_Lxx`/`_Exx`) for power button, lid close, AC plug. Power button found on iron.

### 59. S3 / S0ix suspend-resume — `power.rs` — `[ ]`
- **Does:** sleep/resume (Concept Phase 2.4). Without it, lid-close drains the battery. ACPI S3 programming + device save/restore — a cross-cutting effort touching every driver's `suspend()/resume()`.

### 60. RGB unified — `rgb.rs` — `[~]`
- **Does:** the Concept §Customization promise — every mobo/fan/keyboard RGB through one API (SMBus/USB/I²C vendor protocols). "RGB hell is a Windows problem; RaeenOS solves it."

### 61. Bluetooth (btusb + HCI + L2CAP + GAP/A2DP) — `bluetooth.rs` — `[~]`
- **Does:** BT radios (usually USB btusb, HCI over USB bulk/interrupt). Pairing, controllers, audio (§8.49). ~15,000 LOC native or a BlueZ-class userspace stack (Path C).
- **Spec:** Bluetooth Core 5.x, HCI USB transport.

### 62–64. Card reader / fingerprint / printers — `[ ]`
- Long tail. Card readers (USB/PCIe), fingerprint (LinuxKPI/libfprint-class), printing (userspace IPP — not a kernel driver).

---

## 10. Firmware requirements (`request_firmware` / syscall 142)

xtask bundles **everything under `firmware/`** recursively (drop a blob, it ships). A driver asks by the same name Linux uses; the kernel serves it from the initramfs and maps it read-only into the daemon. Full ledger: `docs/FIRMWARE.md`, `docs/THIRD_PARTY_LICENSES.md`.

| Driver | Firmware | Source | In-tree? |
|---|---|---|---|
| amdgpud (Phoenix GC 11.0.1) | PSP TOC/TA, GC IMU/PFP/ME/MEC/RLC/MES, SDMA 6.0.1, DMCUB, VCN 4.0.2 | linux-firmware (GitLab mirror) | ✅ `firmware/amdgpu/` |
| amdgpud VBIOS | `1002-15bf.bin` (from ACPI VFCT, machine-captured) | Athena capture | ✅ `firmware/vbios/` |
| i915d (Intel Xe) | GuC + HuC `i915/<platform>_*.bin` | linux-firmware | ❌ (when targeting Intel iron) |
| iwlwifi (AX210) | `iwlwifi-ty-a0-gf-a0-<NN>.ucode` | linux-firmware | ❌ (licensed; drop in) |
| RTL8125 | none required (in-kernel native runs without `rtl8125b-2.fw`) | — | n/a |
| MT7902 | **none exists** | — | ❌ blocked |
| self-test | `firmware/raeen-selftest.bin` | in-tree | ✅ keep |

> APUs have no `smu_*.bin` (SMU/PMFW is in the system BIOS, loaded by PSP) and no PCI expansion ROM (VBIOS via VFCT). Don't preflight a `smu_13_0_4.bin` — it will report `11/12 absent` forever.

---

## 11. The selection layer — "default drivers you can choose"

Every device should resolve to a **ranked candidate list**, not one hardcoded assignment, with a `DriverPolicy` (per-device pin → per-class preference → global `native-first`/`linuxkpi-first` → fallback chain). Today `driver_manifest::match_pci()` returns one `DriverMatch{package,kind}`; the target taxonomy (`DriverBackend × DriverResidence × DriverStatus + rank`) is additive. This is the inspectable "choose your driver" control surfaced at `/proc/raeen/drivers`. Full design + acceptance: `docs/NATIVE_DRIVER_PLAN.md §2`.

---

## 12. Authoring a new driver (the repeatable path)

Per `docs/NATIVE_DRIVER_PLAN.md §3–5`, every native driver has the same shape:

1. **Spec + IDs** — datasheet/class spec; add the `(vendor,device)`/`(class,subclass)` candidate row in `driver_manifest`.
2. **Register map** — offsets/bitfields as `const`s in a pure `#![no_std]` module; all MMIO behind an `Mmio` trait.
3. **Host KAT first** — a mock register file models the device's responses; assert the driver walks reset→init→ready→one data op. Green on host **before** QEMU (CLAUDE.md §15 — the cheapest real proof, caught the most bugs).
4. **Lifecycle** — implement `RaeDriver` (probe/attach/detach) + the data-plane trait (`BlockDevice`/`NetDriver`/`CharDevice`); declare `capabilities()` (`Cap::Dma`/`Mmio`/`PciConfig`).
5. **Residence** — userspace ELF daemon (default, sandboxed, safe to crash) → promote to in-kernel only if the hot path demands it.
6. **R10 4-artifact contract** — `init()` from `kernel_main` + `run_boot_smoketest()` (must be able to print FAIL) + `/proc/raeen/<driver>` + Concept docstring.
7. **QEMU** — prove a real transaction if QEMU emulates the part.
8. **Iron** — the only thing that earns `[x]`.

---

## 13. Target hardware matrix

### Curated SKU — Beelink Athena (the first RaeReady box)
Ryzen 5 7640HS (Phoenix, 12 logical CPUs) · Radeon 780M iGPU (`1002:15BF`, GC 11.0.1) · RTL8125 2.5G Ethernet (`10EC:8125`) · MediaTek MT7902 Wi-Fi (`14C3:7902`, **no driver**) · NVMe SSD · 4× xHCI · HDA audio · AMD-EFCH watchdog. Ground truth: `docs/ATHENA_GROUND_TRUTH.md`. Real ACPI tables: `firmware/acpi/athena-beelink-elitemini/`.

### General gaming PC (the breadth target)
AMD/Intel/NVIDIA discrete GPU (amdgpud/i915d/nouveau) · NVMe + SATA · Intel igc/Realtek 2.5G + Intel/MediaTek/Realtek Wi-Fi · HDA + USB audio · xHCI HID + DualSense/Xbox · TPM 2.0 · per-vendor OEM WMI hotkey quirks (must be first-party — Path A).

---

## 14. Effort reality (from `docs/HARDWARE_PATH.md`)

Critical-path native subtotal (storage + USB + audio + NIC + Intel iGPU modeset + one Wi-Fi + BT): **~155,000 LOC ≈ 12 dev-years** at audit quality. The strategy that makes this tractable: **Path A native** for the public-spec/bounded-surface classes (storage, USB, audio, Ethernet, input, platform), **Path C LinuxKPI userspace** for the black-box-firmware classes (GPU 3D, Wi-Fi MLME, Bluetooth stack), **never Path B** (in-kernel Linux kABI shim — license blocker + maintenance treadmill + violates R7).
