# Redox Extraction Map

**Purpose:** Prioritized guide for what to copy/adapt from `redox_reference/` (Redox cookbook) and its upstream GitLab repos into AthenaOS.

**Rules:** `.cursor/rules/redox-reference.mdc`, `.cursor/rules/redox-migration.mdc`  
**License tracking:** `docs/THIRD_PARTY_LICENSES.md`  
**Last scanned:** 2026-05-29 (full upstream cross-check vs live tree — see "Verification pass" at end; **extraction effort CLOSED**)

---

## Harvest — 2026-06-15: HID report-descriptor parser (`hidreport`)

Redox decodes non-boot-protocol keyboards/mice via `usbhidd` → `rehid` →
**`hidreport`** (MIT, `#![no_std]`, github.com/hidutils/hidreport). `rehid`
itself is a thin wrapper (orbclient/redox_syscall glue we don't want); the
reusable parser is `hidreport`. Harvested as **`components/raehid`** — a
`#![no_std]`+alloc wrapper that depends on `hidreport` (`default-features=false`,
so no `std`/`hut`) and exposes `HidDevice::{parse, extract_mouse,
extract_keyboard}` against raw report bytes (matches raw u16 usage codes:
GenericDesktop X/Y/Wheel, Button page, Keyboard page + 0xE0..E7 modifiers).
Host-KAT'd 8/8 (`cargo test -p raehid`: boot-mouse + boot-keyboard descriptors,
signed deltas, modifier/keycode-array, idle, `kind()` classification, boot-report
bridges). **WIRED INTO THE KERNEL (2026-06-16):** `xhci::bring_up_hid_keyboard_with_config`
now gates on `boot_capable` (Boot Interface Subclass 1 + boot protocol) — boot
devices keep the exact iron-proven boot path (`hid_device == None`), while
report-only HID devices (gaming mice/keyboards that skip boot subclass) get their
report descriptor fetched (`get_hid_report_descriptor`, interface GET_DESCRIPTOR
0x22) + parsed by `raehid`, stored in `DeviceSlot::hid_device`. `service_hid_reports`
decodes those via raehid → boot-report bridge → existing dispatch (no new input
plumbing). QEMU-proven: `armed 3 HID` (no regression) + `report-protocol device —
parsed 74-byte report descriptor, kind=Mouse` (QEMU usb-tablet) on a live boot.
License rows added to `docs/THIRD_PARTY_LICENSES.md`.

## Re-verification — 2026-06-13 (post-"closed" tree check)

Re-audited the live tree (the upstream clones are gitignored/off-disk, so this
checks what actually landed since the 2026-05-29 close).

**Conclusion stands:** the core driver/FS/USB/net/audio/ACPI/dynamic-linker
surface is natively implemented and exceeds Redox — nothing non-duplicative left
to bulk-port. Three appendix tools were pulled as **ports** since the close, and
the build list is now corrected to only what actually builds:

| Item | State (2026-06-13) |
|------|--------------------|
| **rustysd** (init daemon, "no systemd archaeology") | `[x]` builds + ships in initramfs (`x86_64-unknown-redox` → native osabi stamp) |
| **ripgrep** / `rg` (Concept "search is broken") | `[x]` builds + ships in initramfs |
| **gptman** (GPT CLI) | **removed from build list** — recipe needs `cookbook_cargo --features cli` (xtask build_port runs plain cargo, default `nix` feature) so it only emitted "No binary found". GPT is covered natively: `block_io::{parse_gpt,detect_partition_table,from_gpt_guid}` (read) + `installer`/`fatfs_esp::seed_minimal_gpt_with_esp` (write). Recipe kept in `ports/gptman/` for reference. |
| **helix** (modal editor) | recipe-only in `ports/helix/`; needs the full Redox **cookbook** toolchain (bash `cookbook_cargo`, tree-sitter grammar `.so` build, runtime dirs) which xtask's simple `build_port` cannot run. Deferred — heavy; not a simple git+cargo port. |

**Remaining Concept-aligned items are third-party Rust crates Redox merely
*packages*, not Redox-authored code** — pulling them is upstream-crate
integration (future phase), not "porting Redox":
- Skia + wgpu/naga → AthGFX (Phase 6, the open Year-1 GPU-submit gap)
- fd / lsd / zoxide → AthShell CLI polish (Phase 8)
- gameroy/melonds/etc. emulators → GameOS defaults (Phase 6/7 verification)
- Innernet (WireGuard mesh) → AthNet (we have `wireguard.rs` natively)
- yara-x, mkisofs-rs → AthGuard quarantine / installer ISO (Phase 9/16)

No remaining Redox-authored, Concept-aligned code is left unported. The
extraction effort remains **closed**; the above are tracked under their owning
MasterChecklist phases, not here.

---

## Git policy (mandatory)

| Do | Do not |
|----|--------|
| Commit and push **only** to the **private AthenaOS** GitHub (`origin` of this repo) | Push, PR, or fork-publish to `github.com/redox-os/*` or `gitlab.redox-os.org/redox-os/*` |
| Keep `redox_reference/` as a **local read-only mirror** (`git pull` to refresh) | Commit AthenaOS-specific changes inside `redox_reference/` |
| Copy source into `kernel/`, `components/`, `xtask/` with MIT headers preserved | Strip `Copyright … Redox OS Developers` lines |

**Note:** `redox_reference/` is in `.gitignore`. Recipe paths below are relative to that clone on disk.

---

## How this clone is organized

`redox_reference/` is the **Redox build system (cookbook)**, not a monorepo of all sources. Each `recipes/**/recipe.toml` points at a separate GitLab repo built during `make` / cookbook.

| Path in clone | What it is |
|---------------|------------|
| `recipes/core/` | Kernel, bootloader, relibc, redoxfs, base (drivers bundle), installer |
| `recipes/libs/` | Third-party ports (mesa, sdl, fatfs, …) |
| `recipes/net/`, `recipes/gui/` | Apps and networking utilities |
| `recipes/wip/` | Experimental ports (AML, libusb, pciutils, …) |
| `config/*.toml` | Image recipes (packages + init scripts) |
| `mk/qemu.mk` | QEMU flags (e.g. `qemu-xhci` on x86_64) |

**Deep source for drivers:** almost all hardware daemons live in **`base.git`**, not in individual cookbook folders. Clone for reading:

`https://gitlab.redox-os.org/redox-os/base.git` (MIT) — recipe: `recipes/core/base/recipe.toml`

---

## Status legend

| Status | Meaning |
|--------|---------|
| `[ ]` | Not started |
| `[~]` | AthenaOS has partial own impl; Redox can inform or replace a slice |
| `[x]` | Migrated or consciously not needed |
| **P0** | Unblocks current MasterChecklist phase (bare metal / USB / net) |
| **P1** | High value next quarter |
| **P2** | Later / selective snippets only |
| **SKIP** | Conflicts with Concept (wrong arch or duplicate proprietary stack) |

---

## P0 — Boot, storage, USB, net (current checklist)

| ID | Redox upstream | Cookbook pointer | AthenaOS destination | Checklist tie-in | Status | Notes |
|----|----------------|------------------|---------------------|------------------|--------|-------|
| R01 | `bootloader.git` | `recipes/core/bootloader/` | `boot/` or xtask disk image | Phase 1.8 boot artifacts | `[~]` | `xtask build` emits BIOS+UEFI `.img`; OVMF probe; `docs/BOOT_PACKAGING.md` � keep rust-osdev 0.11 |
| R02 | `kernel.git` | `recipes/core/kernel/` | **SKIP core** | — | **SKIP** | Microkernel + scheme IPC — do **not** replace hybrid `kernel/`. Cherry-pick: ACPI tables, panic formatting, low-level helpers only |
| R03 | `base.git` → **pcid** | `recipes/core/base/` | `components/pcid` + `kernel/src/pci.rs` | Phase 1 PCIe enumeration | `[x]` | `components/pcid` curated ID table; kernel enum logs `pcid::describe()`. Full `pciids.git` DB deferred (R34). MSI-X in `pci.rs`. |
| R04 | `base.git` → **xhcid** | `recipes/core/base/` | userspace USB host + `kernel/src/xhci.rs` IPC | Phase 2.1 USB | `[x]` | Full HID path on QEMU: config descriptor → Configure Endpoint → SET_CONFIGURATION → interrupt IN at correct DCI. Fixed off-by-one in `xhci_ep_index` (was programming DCI 4 = EP2-OUT → StallError; now DCI 3 = EP1-IN). Live servicing thread drains interrupt-IN completions. Serial: `[xhci] HID report (slot 2): [00,00,04..07,..]` → `input::push_event`. |
| R05 | `base.git` → **inputd** / **ps2d** | `recipes/core/base/`, `config/minimal.toml` init | `kernel/src/usb_hid.rs` + `kernel/src/input.rs` | Phase 2.1 HID | `[x]` | Parsers + smoketest PASS; live keystrokes (sendkey a/b/c/d) reach `usb_hid::dispatch_boot_report` → `input::push_event` via the xHCI servicing thread (R04). |
| R06 | `base.git` → **ahcid** | `recipes/core/base/` | userspace AHCI daemon | Phase 1 storage / AthFS mount | `[~]` | In-kernel `ahci.rs`: identify + DMA read; QEMU `ahci` + `ide-hd` smoketest LBA0 (`[ahci] smoketest PASS`); `/proc/raeen/ahci`. Userspace `ahcid` deferred. |
| R07 | `base.git` → **nvmed** (or nvme in base) | `recipes/core/base/` | userspace NVMe daemon | Phase 1 NVMe / AthFS | `[~]` | Identify: ONCS@520, sqes/cqes, tnvmcap log; `sector_size` from NS; `/proc/raeen/nvme`; smoketest PASS line — compare `drivers/nvmed` when cloned |
| R08 | `redoxfs.git` | `recipes/core/redoxfs/` | `components/raefs/` (fork) | Phase 1 GPT/AthFS, storage tiers | `[x]` | `redoxfs_adapter/{tree,header,disk}.rs` + vendor LICENSE; disk trait and superblock probe completed |
| R09 | `redox-fatfs.git` | `recipes/libs/redox-fatfs/` | `kernel/src/fatfs_esp.rs` (native) | Phase 16 installer | `[~]` | Upstream `fatfs 0.3.6` depends on the abandoned `core_io` crate and won't build no_std on current nightly. Shipped a native FAT32 BPB + root-dir cluster parser instead (read-only ESP locator, 8.3 + LFN decode). Smoketest mounts sector 0 of active block dev, dumps to `/proc/raeen/fatfs_esp`. Write support + GPT-partition view follow when install lane copies files. |
| R10 | `base.git` → **e1000d** / **rtl8139d** | `recipes/core/base/` | userspace NIC daemons | Phase 2.2 real NICs | `[x]` | **Conscious in-kernel** (`virtio_net`, e1000/igc paths). Userspace NIC daemons deferred until IOMMU + caps IPC model exists. |
| R11 | `relibc.git` | `recipes/core/relibc/` | `components/raebridge/relibc/` | Phase 11 AthBridge | `[~]` | `raeenOS_syscall.rs` + build-std; `user_init` spawns `hello_relibc` first (`msg:811x`, `[hello_relibc]`); caps + R19/R20 remain |
| R12 | `aml` (any1/aml) | `recipes/wip/libs/other/aml/`, crates.io `aml` | `kernel` via `acpi_full.rs` | Phase 1.4 ACPI | `[x]` | `aml = "0.16"` in kernel; bring-up audit log + SSDT count at boot |
| R13 | Cookbook `mk/qemu.mk` | `mk/qemu.mk` | `target/boot.ps1`, `xtask` | QEMU smoketest | `[x]` | Already mirrored: `qemu-xhci`, virtio-net; **intel-iommu** left off (breaks boot) |

---

## P1 — Platform, installer, dev loop

| ID | Redox upstream | Cookbook pointer | AthenaOS destination | Checklist tie-in | Status | Notes |
|----|----------------|------------------|---------------------|------------------|--------|-------|
| R14 | `installer.git` | `recipes/core/installer/` | `installer/` / Phase 16 | Phase 16 | `[ ]` | Partitioning + image laydown patterns |
| R15 | `pkgar.git` / `pkgutils.git` | `recipes/core/pkgar`, `pkgutils` | packaging research | Phase 16 atomic updates | `[ ]` | Ideas for artifact format — not wholesale pkgar unless checklist says |
| R16 | `base.git` → **acpid** | `recipes/core/base/` | `kernel/src/acpi_full.rs`, `battery.rs` | Phase 1.4 | `[~]` | Daemon model for _BST, GPE — we poll in-kernel today |
| R17 | `base.git` → **vesad** / **bgad** | `recipes/core/base/` | **SKIP** display server | Phase 3 AthGFX | **SKIP** | Use AthGFX + GOP/EDID path — not Orbital/VESA daemons |
| R18 | `netdb.git` / `netutils.git` | `recipes/core/netdb`, `netutils` | `components/raenet/` DNS stub | Phase 10 AthNet | `[x]` | `netdb` is config data only; DNS resolver lives in already-added `relibc`. No standalone extraction. |
| R19 | `openposixtestsuite.git` | `recipes/tests/openposixtestsuite/` | CI / AthBridge tests | Phase 11 | `[ ]` | POSIX conformance when relibc lands |
| R20 | `redox-posix-tests.git` | `recipes/tests/redox-posix-tests/` | CI | Phase 11 | `[ ]` | Smaller than full OPATS |
| R21 | `redoxer.git` | `recipes/dev/redoxer/` | `xtask` cross-build ideas | DevEx | `[ ]` | Optional: guest testing harness — do not require Redoxer for main loop |
| R22 | Cookbook `src/bin/repo.rs` | `redox_reference/src/` | `xtask` | Build loop | `[ ]` | Recipe dependency graph / incremental cook ideas |
| R23 | `.gitlab-ci.yml` + `config/*ci.toml` | root, `config/x86_64/ci.toml` | `.github/workflows/` | CI | `[ ]` | QEMU headless test patterns only |
| R29 | `userutils.git` | `recipes/core/userutils` | `components/raeshield/` | Phase 9 AthGuard | `[x]` | Covered natively: `components/raeid` (1795 L) already implements the account system; `userutils` would duplicate. |
| R30 | `dynamic-example.git` | `recipes/demos/dynamic-example` | `components/raebridge/loader` | Phase 11 AthBridge | `[x]` | Covered + exceeded: `kernel/src/dynamic_linker.rs` (1211 L) + `elf_loader.rs` (1401 L) + `raebridge` full Win32 layer (kernel32/ntdll/d3d/pe_dll_registry). |
| R33 | `gitoxide` / `binutils` | `recipes/dev/*` | `target/` rootfs | DevEx | `[ ]` | Pure-Rust git and development tools for self-hosting |
| R34 | `pciids.git` | `recipes/libs/pciids` | `components/pcid` (expand) | Phase 1.7 Hardware | `[~]` | Subset in `pcid::KNOWN_DEVICES`; full DB vendored when installer/device-manager phase needs it |

---

## P2 — GUI, audio, media (mostly SKIP for Concept)

| ID | Redox upstream | Cookbook pointer | AthenaOS destination | Status | Notes |
|----|----------------|------------------|---------------------|--------|-------|
| R24 | `orbital.git`, `liborbital` | `recipes/gui/orbital`, `libs/liborbital` | **SKIP** | **SKIP** | AthUI / AthGFX — not Orbital |
| R25 | `mesa.git` | `recipes/libs/mesa` | **SKIP** | **SKIP** | Linux DRM stack — Concept forbids |
| R26 | `cpal`, `timidity`, etc. | `recipes/sound/*`, `demos/cpal` | **SKIP** | **SKIP** | AthAudio is proprietary path |
| R27 | `ion.git` | `recipes/core/ion` | optional shell research | `[ ]` | Reference only for CLI UX — Rae shell is separate |
| R28 | `contain.git` | `recipes/core/contain` | AthGuard research | `[ ]` | Capability/container ideas — adapt to `crate::capability` |
| R31 | `cosmic-*` | `recipes/cosmic/*` | `apps/` or `components/` | `[ ]` | System76 COSMIC desktop apps built in Rust; adapt `cosmic-files` as default file explorer |
| R32 | `procedural-wallpapers-rs` | `recipes/gui/procedural-wallpapers-rs` | `components/raeui/` | `[ ]` | Concept §Live wallpapers that don't murder battery (GPU-accelerated procedural rendering) |

---

## `base.git` driver inventory (expected layout)

When you clone `base.git` locally (read-only), look for these crates — names are stable in Redox docs and init:

| Daemon / crate | Hardware | AthenaOS strategy |
|----------------|----------|------------------|
| `pcid` | PCIe config space | Compare with `pci.rs`; MSI-X setup |
| `xhcid` | USB 3 host | Complement in-kernel `xhci.rs`; HID enumeration |
| `inputd` | Input multiplexer | Feed `input::keyboard_event` |
| `ps2d` | PS/2 kbd/mouse | Athena fallback when USB fails |
| `ahcid` | SATA/AHCI | DMA rings vs `ahci.rs` |
| `nvmed` | NVMe | Admin/IO queue vs `nvme.rs` |
| `e1000d`, `rtl8139d`, … | Ethernet | Userspace + IOMMU after in-kernel P0 works |
| `acpid` | ACPI events | Battery, lid, AC — align with `acpi_full.rs` |
| `vesad`, `bgad` | Framebuffer | **Do not port** — AthGFX path |

---

## Config / QEMU snippets worth mirroring

From `config/base.toml` + `mk/qemu.mk` (already partially in AthenaOS):

```text
# Redox x86_64 QEMU (mk/qemu.mk)
-device qemu-xhci          # USB host — mirrored in boot.ps1 / xtask
QEMU_MACHINE=q35
QEMU_SMP=4
```

From `config/minimal.toml` init:

```text
inputd -A 2
getty …
```

→ Future AthenaOS initramfs: start input service before getty when USB HID is `[x]`.

---

## Recommended clone commands (local only, never push)

```powershell
# Refresh cookbook (already at redox_reference/)
cd C:\Users\woisr\OneDrive\Documents\AthenaOS\redox_reference
git pull

# Optional deep-read clones (sibling dir, gitignored or separate folder)
git clone https://gitlab.redox-os.org/redox-os/base.git redox_reference_upstream/base
git clone https://gitlab.redox-os.org/redox-os/redoxfs.git redox_reference_upstream/redoxfs
git clone https://gitlab.redox-os.org/redox-os/relibc.git redox_reference_upstream/relibc
```

Add `redox_reference_upstream/` to `.gitignore` if created — keeps MIT sources local without bloating AthenaOS commits.

---

### Storage Utilities & Installation
- [ ] **FAT32 Formatter** (`https://gitlab.redox-os.org/redox-os/redox-fatfs.git`)
  - **Mapping:** *Phase 3.3 ESP*
  - **Extract:** ESPs must be FAT32. This provides a battle-tested FAT32 implementation so we don't have to write one from scratch for the `AthenaOS` installer.

### DNS & Network Resolution
- [ ] **NetDB** (`https://gitlab.redox-os.org/redox-os/netdb.git`)
  - **Mapping:** *Phase 10 AthNet*
  - **Extract:** DNS resolution abstractions and `/etc/resolv.conf` parsing utilities in safe Rust.

### Modern Rust CLI & AthShell Enhancements
Redox OS ships with recipes for the best modern Rust CLI tools. By including these in the base install, we immediately deliver on the Concept promise of a modern, fast environment.
- [ ] **Ripgrep** (`https://github.com/jackpot51/ripgrep.git`) & **FD** (`https://github.com/sharkdp/fd.git`)
  - **Mapping:** *Phase 8 AthShell / Search*
  - **Extract:** Fixes the "Search is broken" problem with sub-100ms local-first search logic.
- [ ] **LSD** (`https://github.com/lsd-rs/lsd`) & **Zoxide** (`https://github.com/ajeetdsouza/zoxide`)
  - **Mapping:** *Phase 8 AthShell*
  - **Extract:** Modern replacements for `ls` and `cd` that feel like 2026, not 1995.
- [ ] **Helix** (`https://github.com/greyshaman/helix.git`)
  - **Mapping:** *Phase 0 DevEx*
  - **Extract:** A modern modal text editor written in Rust to serve as the ultimate built-in terminal editor.

### Container & Sandbox Runtimes
- [ ] **Youki** (`https://github.com/containers/youki`)
  - **Mapping:** *Phase 9 AthGuard (Sandboxing)*
  - **Extract:** An OCI-compliant container runtime written entirely in Rust. While we use capabilities instead of standard Linux cgroups, Youki's namespace isolation logic is top-tier reference material for our app sandboxes.

### OS Init, Partitioning & Security (Rust Native)
- [ ] **RustySD** (`https://github.com/willnode/rustysd`)
  - **Mapping:** *Phase 2 Init Daemon*
  - **Extract:** Concept dictates "no systemd archaeology". This is a pure-Rust systemd-compatible service manager that can act as the backbone for AthenaOS service bring-up.
- [ ] **Gptman** (`https://github.com/rust-disk-partition-management/gptman`)
  - **Mapping:** *Phase 3 Installer / Storage*
  - **Extract:** A library to manage GUID Partition Tables (GPT) natively in Rust. Essential for the AthenaOS bare-metal installer.
- [ ] **Lemurs** (`https://github.com/coastalwhite/lemurs`)
  - **Mapping:** *Phase 9 AthGuard (Login)*
  - **Extract:** A tiny, customizable TUI Display/Login Manager in Rust. Perfect for the fallback TTY login interface before `AthUI` fully initializes.
- [ ] **Yara-X** (`https://github.com/VirusTotal/yara-x`)
  - **Mapping:** *Phase 9 AthGuard (Security)*
  - **Extract:** Pure-Rust reimplementation of the YARA malware scanner, perfect for integrating into the quarantine framework.

### GameOS & Graphics Verification
To prove out Phase 6 (`AthGFX`) and Phase 7 (`AthAudio`), we need complex workloads. The Redox cookbook contains recipes for these pure-Rust game engines and emulators, meaning they are already proven to compile in `no_std` or customized environments.
- [ ] **Naga & wgpu** (`https://github.com/gfx-rs/wgpu`)
  - **Mapping:** *Phase 6 AthGFX*
  - **Extract:** The core shader translation and WebGPU native implementations required to power the `AthUI` Skia backend.
- [ ] **Rust-Doom & Unvanquished** (`https://github.com/hovinen/rust-doom`, `https://github.com/DaemonEngine/Daemon`)
  - **Mapping:** *Phase 6/7 Verification*
  - **Extract:** Run these natively to prove the GPU buffer mapping and low-latency audio routing works.
- [ ] **Gameroy, Dolphin, MelonDS, ShadPS4** (Assorted recipes)
  - **Mapping:** *GameOS Default Features*
  - **Extract:** Concept states "Gaming isn't a mode. It's the default." Shipping built-in, native emulators for Gameboy, DS, and Gamecube demonstrates this instantly out-of-the-box.

### The Ultimate TUI App Ecosystem
Redox has ported a suite of modern terminal apps that look and feel like desktop apps. These should be built into `AthShell`'s default PATH.
- [ ] **GitUI** (`https://github.com/extrawurst/gitui`) — Blazing fast git client.
- [ ] **Process-Viewer** (`https://github.com/GuillaumeGomez/process-viewer`) & **Battop** (`https://github.com/svartalf/rust-battop`) — Modern Task Manager and battery visualizer in the terminal.
- [ ] **Twitch-TUI & Youtube-TUI** (`https://github.com/Xithrius/twitch-tui`, `https://github.com/Siriusmart/youtube-tui`) — Media consumption directly in `AthShell` to prove networking and media decoding.

### Graphics APIs & Window Management (AthGFX)
- [ ] **libskia recipe** (`https://skia.googlesource.com/skia`)
  - **Mapping:** *Phase 6 AthGFX*
  - **Extract:** The Concept explicitly specifies `AthUI` is built on Skia. Redox already has a working recipe to cross-compile Skia into a `no_std` / custom OS environment!
- [ ] **Wayland-rs & LeftWM** (`https://github.com/jackpot51/wayland-rs`, `https://github.com/leftwm/leftwm`)
  - **Mapping:** *Phase 6 Compositor*
  - **Extract:** Pure-Rust Wayland bindings and a Rust window manager. If `AthGFX` decides to implement Wayland protocol compatibility, this is the exact reference to use.

### Network Mesh & Security (AthNet)
- [ ] **Innernet** (`https://github.com/tonarino/innernet`)
  - **Mapping:** *Phase 10 AthNet*
  - **Extract:** A WireGuard-based mesh network written in Rust. AthenaOS wants native, fast networking. Integrating a WireGuard mesh securely into the OS configuration natively is a killer feature.
- [ ] **Sniffnet** (`https://github.com/GyulyVGC/sniffnet`)
  - **Mapping:** *Phase 10 AthNet*
  - **Extract:** A network analyzer in Rust to verify packet routing and firewall rules.

### Core Kernel Tweaks & ISO Generation
- [ ] **tikv-jemallocator** (`https://gitlab.redox-os.org/njskalski/jemallocator.git`)
  - **Mapping:** *Phase 4 Kernel Polish*
  - **Extract:** A high-performance memory allocator mapped for Rust. Can be analyzed for replacing the kernel's default heap allocator under heavy multicore loads.
- [ ] **mkisofs-rs** (`https://github.com/marysaka/mkisofs-rs`)
  - **Mapping:** *Phase 16 Installer*
  - **Extract:** Pure-Rust ISO 9660 image generator. Essential for the `xtask` build loop to generate bootable `AthenaOS.iso` files without relying on legacy C tools like `xorriso`.
- [x] **Kanata** (`https://github.com/jtroo/kanata`)
  - **Mapping:** *Phase 2.1 Input*
  - **Extract:** An advanced keyboard remapper in Rust. Great reference for intercepting raw HID events and applying software macros before passing them to `AthUI`.

### Developer Workflows & Build Scripts (xtask inspiration)
The root of the `redox_reference` cookbook itself contains several excellent scripts and CI patterns we can extract to make AthenaOS development faster:
- [ ] **`.gitlab-ci.yml` & `mk/ci.mk`**
  - **Mapping:** *Phase 0 DevEx / CI Pipeline*
  - **Extract:** Redox uses a headless `qemu-system-x86_64` runner with `gpu=no` in their CI to boot the OS, mount the disk, extract the `os-test.json` conformance logs, and assert success. We can replicate this exact flow in our `.github/workflows/` using `xtask`.
- [ ] **`mk/qemu.mk`**
  - **Mapping:** *Phase 0 DevEx*
  - **Extract:** An incredibly exhaustive list of QEMU flags spanning x86_64, aarch64, and RISC-V. Useful for cross-compilation testing and simulating raw NVMe vs ATA disks.
- [ ] **`scripts/backtrace.sh` & `scripts/ventoy.sh`**
  - **Mapping:** *Phase 0 DevEx & Phase 16 Installer*
  - **Extract:** A script that translates raw hex panic stack traces back to Rust code lines, and a script for generating Ventoy-compatible multiboot USB images for bare-metal testing.
- [ ] **`src/recipe.rs` & `src/cook.rs`**
  - **Mapping:** *Phase 0 xtask architecture*
  - **Extract:** The pure-Rust logic that Redox uses to parse dependency graphs and build `.toml` recipes into an OS disk image. We can port portions of this into our `xtask` rather than relying on Makefiles.

### OS Image Profiles & Bootstrapping
- [ ] **`config/base.toml` & `config/desktop.toml`**
  - **Mapping:** *Phase 16 OS Assembly & Installer*
  - **Extract:** Redox uses declarative TOML files to assemble its final `.img` files. These files dictate which packages are included, create default users (`root`, `user`), and configure exactly which microkernel schemes each user has access to (e.g., locking down the `display*` or `tcp` schemes). This is a masterclass in how to declaratively build `AthenaOS.iso` profiles (e.g., `gaming.toml`, `server.toml`).
- [ ] **Avoid `native_bootstrap.sh`**
  - **Mapping:** *Phase 0 DevEx*
  - **Extract:** The `native_bootstrap.sh` file is 1,180 lines of bash installing legacy C-tools (`bison`, `flex`, `genisoimage`, `m4`) across every Linux distro. **This proves why your Concept's `xtask` approach is superior.** We will entirely skip this headache by utilizing the pure-Rust equivalents we mapped earlier (like `mkisofs-rs`).

---

## Extraction workflow (per item)

1. Pick row **ID** with status `[ ]` or `[~]` and **P0** priority matching current `MasterChecklist.md` phase.
2. Read upstream repo; copy into destination with `LICENSE-MIT` + copyrights.
3. Adapt: capabilities, IOMMU DMA, hybrid syscall boundary (no scheme-IPC in kernel hot path).
4. `cargo run -p xtask -- build --release` + `target\boot.ps1` — serial proof.
5. Update **Status** column here and row in `docs/THIRD_PARTY_LICENSES.md`.
6. Auto-commit to **private AthenaOS GitHub only**.

---

## Do-not-port list (Concept conflicts)

- Redox **microkernel** + **scheme IPC** as AthenaOS kernel core  
- **Orbital**, **mesa**, **libdrm** as display stack → **AthGFX**  
- **ALSA/Pulse/cpal** paths → **AthAudio**  
- **Linux netfilter / full Wi-Fi kernel port** → **AthNet** + LinuxKPI Path C per `docs/LINUX_DRIVER_STRATEGY.md`  
- Whole **cookbook** as AthenaOS build system → keep **xtask** + small batches  

---

## Landed in AthenaOS tree (2026-05-28)

| Path | Extraction IDs | Notes |
|------|----------------|-------|
| `components/pcid/` | R03, R34 (subset) | `no_std` PCI name lookup |
| `components/raefat/` | R09 (slice 1) | FAT BPB / boot-sector probe |
| `components/raefs/src/redoxfs_adapter/` | R08 | `tree.rs`, `header.rs`, MIT vendor tree |
| `kernel/src/{xhci,xhci_desc,usb_hid,input,pci}.rs` | R04, R05, R03 | In-kernel P0 stack |
| `kernel/src/ahci.rs` + `target/boot.ps1` AHCI disk | R06 (slice) | LBA0 smoketest + `/proc/raeen/ahci` |
| `components/raebridge/relibc/` | R11 (slice) | Syscall adapter, crt0, hello_relibc |
| `components/kanata_daemon/vendor/` | Kanata appendix | LGPL reference only |

## Deferred backlog (not P0 — do not block boot)

The appendix sections below (ripgrep, Helix, emulators, Innernet, Skia recipe, Wayland, etc.) are **research / Phase 8+** items. They stay `[ ]` until the matching `MasterChecklist.md` phase opens. Do not bulk-port cookbook recipes.

## Next suggested extractions (2026-05-28)

1. ~~**R04 + R05** — Finish QEMU HID → `input::push_event`.~~ **Done 2026-05-28** (DCI fix + live servicing thread; serial shows HID reports for sendkey a/b/c/d).  
2. **R08** — `redoxfs_adapter` disk trait + read-only superblock probe on NVMe block dev.  
3. **R01** — Compare Redox bootloader packaging with `xtask` / `boot.ps1` (artifacts only).  
4. Clone **`base.git`** locally (Windows: watch `aux.rs` path) and cross-check `xhcid` / `inputd` file paths for any remaining literal ports.

---

## Verification pass — 2026-05-29 (extraction effort CLOSED)

Upstream sources were cloned read-only into `redox_reference_upstream/` (gitignored):
`base` (full driver tree), `redoxfs`, `relibc`, `aml`. The entire Redox driver /
subsystem surface was then cross-checked against the live AthenaOS tree.

> [!IMPORTANT]
> **Conclusion: AthenaOS already implements — and in most cases *exceeds* — Redox's
> entire extractable surface.** There is no remaining non-duplicative Redox code
> to bulk-migrate. Forcing further copies would only create duplicate, conflicting,
> or microkernel-scheme-mismatched code. The extraction initiative is **closed**;
> future work is native feature completion + driver depth, not porting.

**Evidence (Redox component → already-native AthenaOS equivalent):**

| Redox | AthenaOS (verified present) |
|-------|----------------------------|
| `partitionlib` (GPT/MBR) | `block_io.rs`: `detect_partition_table`, `parse_gpt`, `from_gpt_guid` |
| `usbscsid` (SCSI + Bulk-Only Transport) | `usb_core.rs`: `scsi_read10/write10/inquiry/read_capacity10/request_sense`, `CommandBlockWrapper`, `CBW_SIGNATURE` |
| `usbhubd` | `usb_core.rs`: `UsbHub` |
| `xhcid` / `usbhidd` / `inputd` / `ps2d` | `xhci.rs` (3270 L) / `usb_hid.rs` / `input.rs` |
| `nvmed` / `ahcid` / `virtio-blkd` | `nvme.rs` / `ahci.rs` / `virtio.rs` |
| `e1000d` / `ixgbed` / `rtl*` / `virtio-netd` | `net_drivers.rs` (2228 L, full E1000 regs) / `igc.rs` / `virtio_net` |
| `audio/ihdad` (Intel HD Audio) / `ac97d` | `audio.rs` (2338 L, HDA controller) |
| `acpid` / `amlserde` / `rtcd` | `acpi_full.rs` / `aml` crate (0.16) / `rtc.rs` |
| `dynamic-example` (R30, dynamic ELF) | `dynamic_linker.rs` (1211 L) + `elf_loader.rs` (1401 L) + `raebridge` (full Win32 layer) |
| `userutils` (R29, login/passwd) | `raeid` (1795 L account system) |
| `netdb` (R18, DNS) | resolver lives in the already-added `relibc` |
| `orbital` / `mesa` / `vesad` / scheme IPC | **SKIP** — AthGFX/AthUI + hybrid kernel (Concept) |

**Status upgrades:** R18, R29, R30 → `[x]` (covered natively, exceeds upstream).

**Windows note:** `base.git` cannot be fully checked out on Windows —
`drivers/graphics/ihdgd/src/device/aux.rs` uses the reserved name `aux`. Read its
files via `git show HEAD:<path>` instead of a working-tree checkout.

**Remaining `[ ]` items** are all either SKIP-per-Concept or future-phase
(R14 installer / R15 pkgar → Phase 16; R19/R20 POSIX tests → Phase 11 CI;
R31 COSMIC apps / R32 procedural wallpapers → Phase 3+ GUI). None block current work.
