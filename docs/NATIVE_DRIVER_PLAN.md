# Native Rust Driver Plan (AthenaOS)

**Status:** Plan / framework design. Companion to `docs/LINUX_DRIVER_STRATEGY.md`.
That doc covers *borrowing* Linux drivers via the LinuxKPI shim (breadth).
**This doc covers writing drivers from scratch in Rust (depth + control), and the
selection layer that lets you choose which backend services a device.**

---

## 0. The two-sentence thesis

Every device gets a **ranked list of candidate drivers**, not a single assignment.
A **native Rust driver is the default where one exists and is stable**; LinuxKPI is
the fallback for the chips it is infeasible to rewrite (GPU, Wi-Fi). A **policy
(global + per-device override) chooses**, so "default drivers you can choose" is a
real, inspectable setting — not a hardcode.

---

## 1. Principles (what we will and won't rewrite)

| Class | Native Rust from scratch? | Why |
|---|---|---|
| Storage (NVMe, AHCI, virtio-blk) | **Yes — already done** | Stable, public specs; hot path; small surface. |
| NIC (e1000/igc, virtio-net, RTL8169) | **Yes — partly done** | Public datasheets; native already beats a shim here. |
| USB (xHCI + MSC/HID/hub) | **Yes — in progress** | Spec is public (xHCI/USB); class drivers are tractable. |
| Input (PS/2, HID, gamepad) | **Yes** | Tiny, well-documented; latency-critical (SCHED_BODY). |
| Audio (HD Audio codec walk + PCM) | **Yes** | HDA spec is public; native fits AthAudio's sub-3ms goal. |
| Platform (ACPI EC, RTC, GPIO, I²C/SMBus, thermal/fan) | **Yes** | Board glue; ACPI describes it; no vendor secrets. |
| **GPU (AMD/Intel/NVIDIA 3D)** | **No — keep LinuxKPI** | Command ISA, shader compiler, display engine, PM are undocumented/enormous. A from-scratch GPU driver is a multi-year effort even for one vendor. Native applies only to **modeset/scanout/cursor** (small, doable), not 3D. |
| **Wi-Fi (iwlwifi, etc.)** | **No — keep LinuxKPI** | Opaque firmware command interfaces; huge regulatory + MLME surface. |

**Rule of thumb:** if the spec is public and the surface is bounded, write it native
(control + no GPL + crash-isolatable + testable on the host). If the device is a
black box gated behind vendor firmware, borrow the Linux driver via LinuxKPI.

---

## 2. The selection layer — "default drivers you can choose"

### 2.1 Today (single assignment)
`kernel/src/driver_manifest.rs::match_pci()` returns **one** `DriverMatch
{ package, kind }`, where `kind ∈ {Builtin, LinuxKpi, None}`. One device → one driver.

### 2.2 Target (ranked candidates + policy)
Evolve the matcher to return a **ranked candidate list** and add a selection policy:

```rust
pub enum DriverBackend { Native, LinuxKpi }      // who wrote the logic
pub enum DriverResidence { InKernel, Userspace } // hot-path vs IOMMU-sandboxed
pub enum DriverStatus { Stable, Experimental, Stub }

pub struct DriverCandidate {
    pub name: &'static str,        // "nvme", "amdgpud", ...
    pub backend: DriverBackend,
    pub residence: DriverResidence,
    pub status: DriverStatus,
    pub rank: u8,                  // lower = more preferred by default
}

/// Ordered best-first. e.g. AMD iGPU -> [amd_modeset(Native,Experimental),
/// amdgpud(LinuxKpi,Stable)]; NVMe -> [nvme(Native,Stable)].
pub fn candidates(vendor,u16, device,u16, class,u8, subclass,u8) -> Vec<DriverCandidate>;
```

`DriverKind::Builtin` is just `Native + InKernel` in the new taxonomy — the change is
additive and backward-mappable, so the existing `/proc/raeen/drivers` +
`required_linuxkpi_packages()` keep working.

### 2.3 The policy (how you choose)
A `DriverPolicy` resolved at boot/install, in priority order:

1. **Per-device pin** — `RaeManifest`/registry entry: `pci:1002:15bf = amd_modeset`
   (force this device onto a named driver).
2. **Per-class preference** — `prefer = native | linuxkpi` for a whole class
   (e.g. "native for storage, linuxkpi for gpu").
3. **Global default** — `native-first` (ship default) vs `linuxkpi-first`.
4. **Fallback chain** — if the chosen candidate's `attach()` fails, fall to the next
   in rank (native experimental fails → LinuxKPI stable picks up), logged loudly.

Surfaced read-only at `/proc/raeen/drivers` (current pick + alternatives + why) and
settable via the config registry / installer UI later. This is the inspectable
"choose your driver" control.

### 2.4 Acceptance for the selection layer
- `[drvman] candidates: <dev> -> native:nvme(stable)* linuxkpi:- ...` per device.
- A boot smoketest that pins a device to a non-default backend and proves the pick.
- Fallback proof: a forced-fail native `attach()` falls through to the next candidate.

---

## 3. The native-driver authoring framework

### 3.1 One lifecycle trait above the class traits
We already have the *data-plane* traits: `BlockDevice` (`block_io.rs`),
`NetDriver` (`net_drivers.rs`), `CharDevice` (`tty.rs`). Add a thin *lifecycle/probe*
trait they plug into so every native driver has the same shape:

```rust
pub trait RaeDriver: Send {
    fn name(&self) -> &'static str;
    fn probe(dev: &PciDevice) -> bool where Self: Sized; // does this driver bind?
    fn capabilities() -> &'static [Cap] where Self: Sized; // what it must be granted
    fn attach(&mut self, dev: &PciDevice) -> Result<(), DriverError>; // bring up
    fn detach(&mut self) -> Result<(), DriverError>;
    fn run_boot_smoketest(&mut self) -> bool;              // R10
}
```

`attach()` returns the data-plane handle (a `Box<dyn BlockDevice>` / `NetDriver` /
…) registered with the relevant subsystem. R10 contract (init + smoketest + procfs +
Concept docstring) stays mandatory.

### 3.2 A probe registry
A static table of `(probe_fn, factory_fn)` the boot/PCI scan walks. The
`driver_manifest` selection layer (§2) decides *which* candidate to instantiate; the
registry knows *how*. New driver = add one row, no edits to the scan loop.

### 3.3 Residence: in-kernel vs userspace (Concept-mandated isolation)
- **In-kernel** for frame/audio/storage hot paths where profiling proves residency
  (NVMe, NIC, xHCI, input). Uses `crate::capability` for privileged ops.
- **Userspace, IOMMU-sandboxed** for anything that can fail without taking the box
  down (the Concept default). The driver is an ELF daemon; it `sys_claim_device`s,
  gets an IOMMU domain (`iommu.rs`) so its DMA can't escape, and talks to the kernel
  over capability IPC. `userspace_driver::sys_claim_device` already exists.
- The **same `RaeDriver` source can target either** residence — start a new driver
  in userspace (safe to crash while developing), promote to in-kernel only if the
  hot path demands it.

### 3.4 Capability + IOMMU posture
Every native driver declares `capabilities()` (e.g. `Cap::Dma`, `Cap::PciConfig`,
`Cap::Mmio(range)`). The kernel grants exactly those; a userspace driver additionally
gets a per-device IOMMU domain so a buggy/hostile driver can't DMA over kernel memory.
This is what makes "write a driver from scratch" safe to iterate.

---

## 4. Host-first test discipline (the thing that makes this tractable)

The hardest part of a from-scratch driver is the **bring-up register sequence**, and
you can validate most of it **without QEMU or iron** using the pattern already proven
in `tools/linuxkpi_harness` (mock-GPU) and the rae_crypto/amdgpu host KATs:

1. Put the driver's pure logic (register offsets, init state machine, ring/queue
   math, descriptor formats) in a `#![no_std]` module with **no direct hardware
   access** — all MMIO goes through a `Mmio` trait.
2. **Host harness** implements `Mmio` as a **mock register file** that models the
   device's response (doorbell → completion, reset → ready bit, link-up, etc.), and
   asserts the driver drives it through the correct sequence. Runs as a normal
   `cargo test` / standalone host binary — instant, deterministic, no emulation.
3. **In-kernel**, the same logic runs over a real `Mmio` backed by the BAR mapping.
4. Boot smoketest + QEMU device (where QEMU emulates the part) + finally iron.

A native driver should reach "host-KAT green" before it ever touches QEMU. This is
how we caught the HMAC and terminal-color bugs early, and how the mock-GPU bring-up
got tested before Athena.

---

## 5. Per-driver authoring playbook (repeatable checklist)

For each driver, in order:

1. **Spec + IDs** — datasheet / class spec; the `(vendor, device)` or `(class,
   subclass)` it binds; add the candidate row in `driver_manifest`.
2. **Register map** — offsets/bitfields as `const`s in a pure module.
3. **Host KAT** — mock-MMIO model + a test that walks reset → init → ready → one
   data operation (one block read / one packet / one report). Green on host first.
4. **Lifecycle** — implement `RaeDriver` + the right data-plane trait
   (`BlockDevice`/`NetDriver`/`CharDevice`/…); declare `capabilities()`.
5. **Residence** — userspace ELF daemon (default, sandboxed) or in-kernel (hot path).
6. **R10** — `init()` wired, `run_boot_smoketest()`, `/proc/raeen/<driver>`, Concept
   docstring.
7. **QEMU** — if QEMU emulates the device, prove a real transaction in the serial log.
8. **Iron** — the final `[x]`; everything before is `[~]`.

---

## 6. Prioritized roadmap (native, from scratch)

Difficulty: 🟢 small / 🟡 medium / 🔴 large. Status from the current tree.

| # | Driver | Diff | Status | Notes |
|---|---|---|---|---|
| 1 | **ACPI EC** (0x62/0x66) | 🟢 | open (Phase 1.4) | Battery/thermal/lid; needs the AML namespace populated first. |
| 2 | **RTC / CMOS** | 🟢 | partial | Wall clock; trivial, good warm-up. |
| 3 | **PS/2 (i8042) kbd+mouse** | 🟢 | partial | Fallback input when no USB; tiny. |
| 4 | **USB HID (boot+report)** | 🟡 | partial | Keyboard/mouse/gamepad over xHCI; SCHED_BODY latency. |
| 5 | **USB MSC** | 🟡 | open (Phase 2.1) | Install media; bulk-only transport over xHCI. |
| 6 | **USB hub** | 🟡 | open | Needed for real-world port trees (Athena HID debug). |
| 7 | **HD Audio codec walk + PCM** | 🔴 | open (Phase 7) | Widget graph + ring-buffer DMA; native fits AthAudio. |
| 8 | **I²C/SMBus + DDC/EDID** | 🟡 | open (Phase 2.3) | Monitor detect; also RGB/fan controllers. |
| 9 | **NIC: RTL8125/8169 native** | 🟡 | LinuxKPI today | Public datasheet; native would drop the shim for Realtek. |
| 10 | **GPU modeset/scanout (per vendor)** | 🔴 | LinuxKPI/SW today | *Only* display path (set a mode, flip a buffer, move cursor) — NOT 3D. Doable native; unlocks a real framebuffer before the full LinuxKPI 3D stack. |

**Explicitly NOT on the native list:** GPU 3D/compute and Wi-Fi MLME — those stay on
LinuxKPI. Writing them from scratch is a multi-year effort with no payoff over
borrowing Linux's, and the Concept already blesses the shim for exactly these.

---

## 7. What I need from you, per driver

One of these unblocks a from-scratch driver:
- **A public datasheet / class spec** (NVMe, xHCI, HDA, I²C, Realtek — all public), **or**
- **A QEMU-emulated version** of the device (so I can prove it pre-iron), **or**
- **The real part on Athena** + a serial log (for parts QEMU doesn't emulate).

For platform parts (EC, RTC, PS/2, I²C) I need nothing but the go-ahead — the specs
are public and QEMU/ACPI exercise them.

---

## 8. First concrete step when you say "go"

1. Land the **selection layer** (§2): extend `driver_manifest` to ranked candidates +
   `DriverPolicy` + the `/proc/raeen/drivers` "current pick + alternatives" view +
   a pin/fallback boot smoketest. This is the "choose your driver" mechanism and is
   pure-QEMU-provable.
2. Then pick driver #2 or #3 (RTC or PS/2) as the **reference native driver** that
   exercises the whole `RaeDriver` + host-KAT + R10 path end to end — small enough to
   finish in one pass, establishing the template every later driver copies.
```
