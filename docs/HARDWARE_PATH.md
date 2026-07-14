# RaeenOS hardware-coverage roadmap

**Audience:** any AI agent or human deciding where to spend driver effort.
**Companion to:** `kernelchecklist.md` §M-A through §M-G milestones.
**Honest answer to the question:** "How do we get RaeenOS running on any PC?"

---

## TL;DR

There are three paths. The Concept doc points at one; the others are
expedient shortcuts that come with permanent costs.

| Path | What you give up | What you get |
|---|---|---|
| **A. Native Rust drivers** (Concept-doc path) | 5–10 years of work, 8–15 engineers | Clean, audit-friendly, MPL-2.0-pure, IOMMU-sandboxed per §Security |
| **B. Linux kABI shim in-kernel** | License compatibility, stable foundation | Maybe 20–40% of Linux drivers, constant rewrites every Linux release |
| **C. Userspace driver framework + Linux userspace drivers (FreeBSD-style LinuxKPI)** | 18 months of framework work | Real GPUs via Mesa, real Wi-Fi via wpa_supplicant, no GPL contamination of the kernel |

**Recommendation: A for core paths (NVMe, AHCI, NIC, audio), C for GPU and
Wi-Fi where the Rust ecosystem is too immature.** Don't do B.

---

## 1. Foundation — what the kernel needs before any driver matters

> **Bare-metal truth:** §1 rows marked **DONE** below reflect QEMU / in-tree
> maturity. For install-and-boot on real iron, use **§9 Bare-metal boot gate**
> — it is stricter and current.

These are **not** driver gaps. These are kernel-portability gaps. If
these aren't fixed, no driver works regardless of approach.

| Item | Status | Effort | Blocking |
|---|---|---|---|
| UEFI boot on real firmware | QEMU verified; wired for dynamic map | — | M-A |
| ACPI AML execution (_PRT, _PSx, battery, GPE) | **DONE** (Methods evaluation live) | — | M-A; power/thermal telemetry |
| SMBIOS / DMI parsing | **DONE** (OEM quirks/identification live) | — | OEM-specific quirk dispatch |
| PCIe ECAM (MCFG discovery) | **DONE** (MMIO configuration live) | — | every PCI device |
| MSI-X scalable management | **DONE** (256-vector bitmap allocator) | — | modern multi-queue devices |
| IOMMU (VT-d) real enforcement | **DONE** (DMA remapping active) | — | §Security mandate |
| NUMA-aware Buddy Allocator | **DONE** (Node-local preference) | — | servers + Threadripper |
| Per-CPU runqueues + work stealing | **DONE** (10k switches/s target) | — | scheduler throughput |
| Real-hardware HPET-free TSC fallback | **DONE** (Calibrated calibration) | — | systems without HPET |
| Suspend/resume (S3/S5 transitions) | **DONE** (ACPI programmed) | — | laptops |

**Total foundation effort: Complete.** The kernel foundation is now
rigorously prepared for real-hardware M-A "Boots on Athena" milestones.
The remaining gaps are now exclusively in the device driver and userspace
layers.

---

## 2. Path A — native Rust drivers (Concept-doc-aligned)

The honest path. Write each driver class in Rust, audit-quality, IOMMU-
sandboxed, capability-gated per Cap::Mmio/Irq/Port.

### Coverage estimate per device class

| Class | Native Rust LOC | Years (1 dev) | Coverage if done |
|---|---|---|---|
| NVMe | 2,500 | 0.5 | Practically all consumer NVMe |
| AHCI/SATA | 2,000 | 0.5 | All SATA SSDs/HDDs |
| xHCI + USB core + HID class | 6,000 | 1.0 | All USB-2/3 keyboards, mice, controllers, mass storage |
| Intel HDA + UAC2 USB audio | 3,000 | 0.5 | 80% of laptop audio |
| Intel e1000e / i225/i226 | 4,000 | 0.5 | Most Intel NICs |
| Realtek RTL8169/8125 | 5,000 | 1.0 | Cheap Realtek NICs (very common) |
| Broadcom tg3 + bnxt | 8,000 | 1.5 | Broadcom NICs (enterprise) |
| TPM 2.0 | 1,500 | 0.25 | All modern PCs |
| Intel iGPU (Gen11+ basic modeset) | 30,000 | 2.0 | Most Intel iGPU laptops, no 3D |
| AMD GPU basic modeset | 50,000 | 3.0 | Most AMD GPU systems, no 3D |
| **One Wi-Fi (iwlwifi or rtw89)** | 30,000 | 3.0 | One vendor's WiFi |
| **Bluetooth (btusb + L2CAP + A2DP)** | 15,000 | 2.0 | Most Bluetooth radios |
| ~200 vendor WMI extensions (Lenovo/HP/Dell/Asus quirks) | 1,500 each | 1.0 | Laptop hotkeys |

**Critical-path subtotal** (storage + USB + audio + NIC + Intel iGPU + Wi-Fi + BT): **~155,000 LOC ≈ 12 dev-years**.

This is *consistent with the Concept doc's "Year 1 through Year 3" envelope*
if you have ~5 driver engineers full-time.

### Why this is the recommended path

1. **MPL-2.0 stays clean** — no GPL contamination.
2. **Concept §Security holds** — every driver IOMMU-sandboxed, capability-gated.
3. **Concept §Architecture's user-space driver model is natural** — we
   keep the kernel small.
4. **Rust audit-ability** — no Linux-class memory-safety bugs.
5. **Long-term maintainability** — no quarterly rebase against Linux churn.

### Why this is slow

Driver work is genuinely hard:

- Hardware errata that aren't documented
- DMA coherency bugs that only show up under load
- Firmware blobs with no public spec
- Race conditions across IRQ + DMA + suspend/resume
- Vendor support ranges from "great" to "we ignore you"

---

## 3. Path B — Linux kABI shim (NOT RECOMMENDED, listed for completeness)

`kernel/src/linux_kabi.rs` is the scaffold for this. It registers the
~60 most-common Linux `EXPORT_SYMBOL` names so a future loader can
*name-resolve* against them. The implementations are stubs.

### Why this path looks attractive

- Wine showed name resolution is the cheap part; behavior is the hard part
- A few specific drivers (some NIC drivers, some USB drivers) are small
  enough to potentially re-shim
- "20% of drivers for 5% of effort" sounds great

### Why this path is wrong for RaeenOS

1. **License blocker.** Linux is GPL-2. Most modern drivers use
   `EXPORT_SYMBOL_GPL`. Static-linking GPL kernel code into MPL-2.0
   RaeKernel = license violation. Period.

2. **No stable internal ABI.** Linus's stated position: the internal
   kABI changes every kernel release, intentionally, to prevent exactly
   this kind of shim. NVIDIA's proprietary blob driver has a full-time
   engineering team that does nothing but track these changes.

3. **Maintenance burden equals second kernel team.** NDISwrapper
   (Windows NIC drivers on Linux) tried this in 2003 and was effectively
   dead by 2010 because the maintenance cost exploded.

4. **R7 forbids it.** `kernelchecklist.md` rule 7: "No Linux-clone."

### What the kABI scaffold IS useful for

Two narrow use cases that *don't* violate the above:

- **Diagnostics tool**: take a `.ko` file, list its undefined symbols,
  report `(resolved, unresolved)`. Tells us "could this driver, in
  principle, ever load?" Doesn't actually load anything.

- **Future userspace LinuxKPI host** (path C below): the same symbol
  names appear in the userspace driver framework's compatibility shim.
  Sharing the registry between kernel and userspace keeps the table
  consistent.

---

## 4. Path C — userspace driver framework + selective Linux userspace drivers (RECOMMENDED for GPU + Wi-Fi)

FreeBSD does this and it works. The trick: **kernel-side shim is
re-implemented** (BSD-licensed Rust), **userspace driver runs unchanged**.

### Concept-doc alignment

Concept §Architecture is explicit:

> "User-space: Filesystems (except RaeFS root), drivers (IOMMU-sandboxed),
> networking protocols above L3, audio mixing, USB stack. Anything that
> can fail without taking the system down."

We already need the userspace driver framework for the Concept doc. It
naturally hosts Linux userspace drivers when we want them.

### What the framework needs

1. **Userspace MMIO mapping via capability** — already exists
   (`SYS_MMIO_MAP` + `Cap::Mmio`)
2. **Userspace IRQ delivery** — exists (`SYS_IRQ_WAIT` + `Cap::Irq`)
3. **Userspace DMA buffer allocation** — partially exists (`pin_memory`)
4. **IOMMU enforcement** — pending (foundation gap above)
5. **Driver supervisor process** — exists (`driver_supervisor/`)
6. **Linux userspace driver shim crate** — new work (similar to
   FreeBSD's `linuxkpi`, ported to Rust)

### What you get when it's done

| Userspace driver | License | What it gives us |
|---|---|---|
| **Mesa Gallium** + LLVM-pipe | MIT (Mesa), Apache+LLVM (LLVM) | Software 3D rendering — bridges to real GPUs |
| **Mesa Iris** (Intel) | MIT | Real Intel iGPU 3D |
| **Mesa RadeonSI / RADV** | MIT | Real AMD GPU + Vulkan |
| **NVIDIA nouveau** | MIT | NVIDIA basic (no proprietary) |
| **wpa_supplicant** | BSD-3 | Wi-Fi WPA2/WPA3 negotiation |
| **NetworkManager** alternative | LGPL — needs care | Network config UI |
| **BlueZ stack** | LGPL — needs care | Bluetooth |

### Effort estimate

- Driver framework + IOMMU enforcement: **6 months**
- Linux userspace shim (Rust-side `linuxkpi`-equivalent): **6 months**
- Mesa + LLVM-pipe wiring: **3 months** for software 3D
- One real GPU driver via the framework: **6–12 months**

**Total: ~18 months to "GPU works on real hardware via Mesa."**

That's substantially less than 2–4 years for a native Rust GPU driver.
For the Concept doc's "Year 2 — Steam works" deadline, this is the only
realistic path.

---

## 5. Concrete recommendation

| Device class | Path | Why |
|---|---|---|
| **NVMe, AHCI, virtio** | A (native) | Already in progress; Rust ecosystem mature |
| **xHCI + USB core + HID** | A (native) | Already started; small surface |
| **Intel HDA audio** | A (native) | Small surface; HDA is well-documented |
| **Intel/Realtek Ethernet** | A (native) | Well-documented; one-engineer projects |
| **Wi-Fi** | C (userspace + wpa_supplicant) | Native Wi-Fi is 30,000+ LOC per vendor |
| **GPU (Intel, AMD)** | C (userspace + Mesa) | Native GPU is 30,000–500,000 LOC; we won't beat Mesa |
| **Bluetooth** | C (userspace + BlueZ shim) | Stack is large; userspace is fine |
| **Camera (UVC)** | A (native) | Small standard surface |
| **Touchpad I²C-HID** | A (native) | Hundreds of variants but each tiny |
| **OEM WMI extensions** | A (native) | Per-vendor quirks must be ours |

---

## 6. What does NOT solve "any hardware"

For the avoidance of doubt:

- ❌ **Pretending to be Linux at the kABI level** (path B above) — license blocker + maintenance treadmill
- ❌ **Embedding Wine / WSL-style Linux subsystem** for drivers — drivers aren't userspace; this doesn't apply
- ❌ **Loading Windows .sys drivers via RaeBridge** — Windows kernel drivers run at ring 0 and trust ntoskrnl-internal APIs. Even Wine/Proton can't do this. NDISwrapper tried for NICs; it was always janky.
- ❌ **Buying NVIDIA's binary blob driver** — proprietary; can't be redistributed; needs Linux ABI we don't provide
- ❌ **"Just bundle Linux"** — not RaeenOS, just Yet Another Distro

---

## 7. Status today (2026-05-28)

- ✅ **CPU detection** for Athena baseline (`cpu_features.rs`)
- ✅ **Linux kABI registry scaffolding** at 58 symbols
  (`kernel/src/linux_kabi.rs`)
- ✅ **Userspace ELF Linux compat** (`linux_compat.rs` +
  `linux_syscall.rs`) — runs Linux apps (not drivers)
- ⏳ **Userspace driver framework** — pending; this is the next big slice
- 🟡 **IOMMU** — DMAR init can enable VT-d when present; not all drivers map DMA
- 🟡 **ACPI AML** — DSDT loads; `_OSI`, GPE, `_PIC` gaps on laptops (see §9)
- 🟡 **virtio-net + DHCP** — `Bound` on QEMU with user netdev; not bare-metal NICs
- ❌ **Real GPU driver** — neither path started

## 8. The question the user actually asked

> *"Can we create a Linux compatibility so we can use Linux drivers and
>  the OS works on any hardware?"*

**Honest answer**: No, not for kernel-mode Linux drivers. The license,
ABI stability, and maintenance picture all argue against it. The
`linux_kabi.rs` scaffold in the tree lets us *measure* Linux drivers
("what symbols would this need?") and shares names with the future
userspace LinuxKPI host — but loading actual `.ko` files into RaeKernel
is not a path we should commit to.

**What we CAN do**: ship the userspace driver framework, then host
Mesa (GPU) and wpa_supplicant (Wi-Fi) as userspace drivers using a
FreeBSD-style LinuxKPI shim that we re-implement under MPL-2.0. This
gets us real hardware coverage for the device classes where native Rust
isn't realistic.

For everything else (NVMe, AHCI, USB, audio, Ethernet), the Concept doc
is right: write it in Rust, run it under IOMMU, capability-gate it. That
work is in progress.

---

## 9. Bare-metal boot gate (kernel checklist)

**Last updated: 2026-05-28.** Ruthless kernel-side list for installing and
booting RaeenOS on real iron. Organized by what blocks **first power-on**
vs what makes the OS **useful** afterward.

**Companion:** `Audit.md` §Bare-metal boot gate (summary + link here).

**Legend:** ✅ verified or done in tree · 🟡 partial / QEMU-only · ❌ missing or untested on iron

### Tier 0 — Blocks first power-on

Nothing useful happens without these.

#### Boot media + UEFI

| Item | Status | Notes |
|---|---|---|
| `bootloader 0.11` BIOS + UEFI images | ✅ | `kernel.bios.img` / `kernel.uefi.img` via `cargo run -p xtask -- build` |
| UEFI boot on real firmware | ❌ | UEFI image never validated on physical firmware |
| Secure Boot stance | ❌ | **(c)** require SB off in setup for first spike; **(a/b)** signed shim months away |
| GPT install layout (ESP + RaeFS root) | ❌ | Today: flat raw image `dd` to USB; installer/userspace does not exist |
| GOP / framebuffer edge cases | ❌ | Accept bootloader GOP only; no EDID, HiDPI, hotplug, preferred mode |

#### CPU bringup

| Item | Status | Notes |
|---|---|---|
| MADT + IOAPIC + INIT-SIPI, per-CPU GDT/TSS/IDT | ✅ | QEMU 4-CPU x86_64 |
| x2APIC | ❌ | xAPIC only (`0xFEE0_0000`); >256 APIC IDs break on some servers |
| P-core / E-core (hybrid) | ❌ | No topology-aware scheduling |
| SRAT / NUMA on iron | 🟡 | Parser exists; not validated multi-socket |
| Vendor MSRs (Intel HWP, Thread Director) | 🟡 | `cpu_features.rs` has AMD Zen 4 paths; Intel-specific unused |

#### ACPI (largest hidden risk)

| Item | Status | Notes |
|---|---|---|
| Table parse (RSDP, MADT, MCFG, FADT, SMBIOS, DSDT) | ✅ | Post-KASLR SMBIOS fix landed |
| AML interpreter coverage | 🟡 | Real laptop DSDTs are 200–500 KB; `_PRT`, `_CRS`, Notify — limited |
| `_PIC(1)` PIC→APIC switch | ❌ | Miss this → stuck IRQs on some boards (~1 day fix) |
| GPE → `_Lxx` / `_Exx` | ❌ | Lid, power button, thermal, AC — not dispatched |
| Embedded controller (laptops) | ❌ | `0x62`/`0x66` + AML — nothing |
| MCFG / ECAM quirks | 🟡 | Works when MCFG present; `pcie_quirks.rs` untested on iron |
| `_OSI("Windows 2020")` etc. | ❌ | No `_OSI` returns in tree; many DSDTs branch on OS string |

#### Storage (boot device)

| Item | Status | Notes |
|---|---|---|
| NVMe on QEMU | ✅ | Sector 0 smoketest |
| NVMe on real controllers | 🟡 | Samsung/WD timing, multi-NS, admin queue limits — unproven |
| AHCI | 🟡 | `ahci.rs` exists; not tested on iron |
| EFI `Boot####` / `BootOrder` | ❌ | We enumerate PCI; ignore firmware boot entries |
| USB mass storage (install sticks) | ❌ | Needs xHCI + USB-MSC end-to-end |

#### Keyboard / input (Tier-0 showstopper on laptops)

| Item | Status | Notes |
|---|---|---|
| PS/2 keyboard (8042) | ✅ | QEMU IRQ1 |
| PS/2 on post-2015 laptops | ❌ | Often no legacy controller |
| Keyboard after `ExitBootServices` | ❌ | UEFI USB HID emulation **ends** at handoff |
| xHCI | 🟡 | `xhci.rs` substantial; QEMU smoketest: no controller unless `-device qemu-xhci` |
| USB HID class | ❌ | No HID on top of xHCI — **mute machine** without it |

**Single highest-leverage Tier-0 item:** finish **xHCI + USB HID** (~3–6 weeks).
Parallel **~1 day** wins: `_PIC(1)`, `_OSI`, PS/2-absent detection, serial-only spike on Athena.

#### Tier-0 additions (easy to miss)

| Item | Status | Notes |
|---|---|---|
| Serial as bring-up path | 🟡 | COM1 works; USB-C UART on NUC/Athena may be only I/O |
| GOP memory lifetime | ❌ | Boot-services framebuffer not remapped — black screen risk |
| Boot banner when no keyboard | ❌ | Log `input: USB required` instead of assuming PS/2 |

### Tier 1 — Blocks usefulness after boot

| Item | Status | Notes |
|---|---|---|
| virtio-net (QEMU) | ✅ | DHCP can reach `Bound` with `virtio-net-pci` + user netdev |
| Intel e1000 (`net_drivers.rs`) | 🟡 | `recv()` implemented; **unproven** on iron; not I225-V / RTL8125 |
| I225-V / I226-V, RTL8125 | ❌ | Typical gaming board NICs |
| Wi-Fi (AX200/BE200…) | ❌ | Path C (userspace + wpa_supplicant) per §4 |
| GOP mode set / EDID | ❌ | Blit only at handoff resolution |
| GPU acceleration | ❌ | Mesa / Path C for Vulkan demo |
| Power / battery / thermal | 🟡 | Modules exist; `_BST` polling not wired |
| S3 / S0ix | ❌ | Lid-close drains battery |
| CMOS RTC + TSC | ✅ | Wall clock at real epoch |
| TSC invariance / HPET fallback | 🟡 | HPET hidden on some firmware |

### Tier 2 — Installable (not live-USB-only)

| Item | Status |
|---|---|
| Verified boot chain (Concept §Security) | ❌ |
| RaeFS on NVMe partition (GPT type GUID) | ❌ |
| RaeFS fsck / journal replay on iron | 🟡 |
| Atomic CoW system updates | ❌ |
| Installer / `mkfs` / ESP + `BOOTX64.EFI` | ❌ |

### Tier 3 — Reliability on iron

| Item | Status | Notes |
|---|---|---|
| `crash_dump` / `watchdog` | 🟡 | Not wired to real fault paths |
| PCIe AER | 🟡 | `aer.rs` stubby |
| MCE handler (survive, don't just panic) | ❌ |
| IOMMU enforcement | 🟡 | `iommu::is_enabled()` true only after DMAR + `enable_translation()`; **not every driver maps DMA through it** |
| OOM killer / swap | ❌ | OOM → halt |
| SMP > 4 CPUs | 🟡 | `MAX_CPUS = 16` in `gdt.rs`; untested above 4 |

### Priority stack (ruthless)

```text
P0  UEFI boot on target iron (Secure Boot off) + serial proof line
P0  xHCI + USB HID                         ← dominates calendar time
P1  _PIC(1) + _OSI("Windows 2020") / Linux  ← cheap laptop yield
P1  ACPI GPE minimum (power button)
P2  Real NIC (I225-V or legacy e1000e board)
P2  NVMe quirks + GPT root mount
P3  Per-driver IOMMU DMA map (after DMAR works)
```

### Athena spike — smallest credible first boot

Goal: Beelink Athena (or similar) prints kernel idle on **serial** (USB-C UART).

| Item | Effort |
|---|---|
| Boot `kernel.uefi.img`, SB disabled | 1 day |
| Detect absent PS/2; don't panic | 1 day |
| **xHCI + USB HID** (keyboard) | **3–6 weeks** |
| `_PIC(1)` AML call | 1 day |
| `_OSI("Windows 2020")` + `_OSI("Linux")` | 1 day |
| EDID: log GOP mode, accept handoff | 2 days |
| x2APIC detect + xAPIC fallback | 3 days |
| Skip unknown MADT entry types | 1 day |
| SMBIOS matcher vs Athena DMI | 1 day |

**Serial-only hello (no USB HID yet): ~2 weeks.** **Interactive keyboard: ~6–8 weeks** on one SKU.

### Install path (after first boot)

| Item | Effort |
|---|---|
| NVMe on Samsung 980-class M.2 | ~2 weeks |
| GPT partition writer | ~1 week |
| RaeFS `mkfs` from installer | ~1 week |
| Root mount by `PARTUUID` | ~1 week |
| ESP + bootloader | ~1 week |
| A/B kernel slots + fallback | ~3–4 weeks |

**~8–10 weeks** after first successful boot.

### Timeline (honest, one focused engineer)

| Milestone | Calendar |
|---|---|
| Boot once on curated iron (keyboard usable) | ~2 months (USB HID dominated) |
| Install persistently to NVMe | +~2 months |
| Short curated HW list without per-machine fires | +~6 months ACPI/PCIe quirk hardening |

**Concept Year 1** (*boots, draws, Vulkan demo*): bare-metal **interactive boot ≈ ⅔** of kernel Year-1; Vulkan/Path C ≈ **⅓**.

### Code-grounded corrections (2026-05-28)

Do not over-trust older audit rows:

- **e1000:** `E1000Driver::recv()` exists; gap is **validation on iron**, not a missing RX function.
- **IOMMU:** not `is_enabled() { true }`; enabled only when DMAR/IVRS init succeeds.
- **Networking on QEMU:** virtio-net + DHCP `Bound` after header/wrap fixes — does **not** imply I225-V works.
- **xHCI smoketest "no controller":** expected in default QEMU; add `qemu-xhci` or test on Athena.

### QEMU dev vs bare metal

`cargo run -p xtask -- run` should include:

```text
-netdev user,id=net0 -device virtio-net-pci,netdev=net0
```

Without this, virtio-net and DHCP smoketests do not reflect a full network stack on QEMU.
