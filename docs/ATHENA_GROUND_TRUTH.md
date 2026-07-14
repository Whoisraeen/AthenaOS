# Athena ground truth (read from the live hardware, 2026-06-12)

Hard facts harvested from Windows running on the Athena box itself (Beelink
EliteMini, Ryzen 5 7640HS). These are the *measured* values the bring-up code
currently guesses at — use them instead of defaults, and prefer extending this
file over re-deriving any of it (each fact here saves an iron round-trip).

## GPU — AMD Radeon 760M (Phoenix1)

| Fact | Value | Consumer |
|---|---|---|
| PCI ID | `1002:15BF` rev `C3`, subsys `1F4C:B016` | `amdgpud` probe / match-mode |
| BDF | bus 196 (0xC4), dev 0, func 0 → `c4:00.0` | probe BDF list |
| IP versions (15BF = Phoenix1) | GC 11.0.1 · PSP 13.0.4 · SDMA 6.0.1 · DCN 3.1.4 · VCN 4.0.2 | `FW_PHOENIX` (confirmed correct) |
| BAR0 (VRAM aperture, 64-bit prefetch) | `0x7C_0000_0000`, 256 MiB (+ 8 MiB doorbell BAR adjacent → 264 MiB window) | stage 2 `map_register_bar` / GMC |
| BAR5 (register aperture) | `0xDC50_0000`, 512 KiB | `reg_read`/`reg_write` base |
| VRAM carve-out (BIOS UMA) | **2048 MiB** (stage 3 currently defaults to 512 MiB — undersized 4×) | `init_gmc` |
| Line IRQs | 36, 39 (plus MSI vectors Windows shows as negatives) | IRQ wiring |

## IP-discovery blob — CAPTURED + off-target-validated (2026-06-20)

The GPU's per-IP register bases (GC / MP1 / OSSSYS / NBIF) are published ONLY in
the on-chip IP-discovery binary — gfx11 has **no** hardcoded base table in amdgpu
(`amdgpu_discovery.c` hardcodes only Vega/Raven/Arcturus/Aldebaran). The daemon
reads it from `firmware/amdgpu/ip_discovery.bin` FIRST (amdgpu's
`amdgpu_discovery_read_binary_from_file` path, `bringup::discovery_from_firmware`)
and only falls back to the VRAM `MM_INDEX` read — which has wedged CPU 0 on every
Athena boot — if the file is absent.

**DONE:** the real Athena 780M blob was captured (Ubuntu live-USB,
`/sys/kernel/debug/dri/0/amdgpu_discovery`, 10240 B, sig `0x28211407`) and
vendored at `firmware/amdgpu/ip_discovery.bin`. The whole `firmware/` tree
auto-bundles into the initramfs, so the daemon serves it via request_firmware.
Validated OFF-TARGET on the dev box with the kernel's exact parser
(`cargo run --manifest-path tools/discovery_probe/Cargo.toml`): all four required
blocks PRESENT and every SOC15 resolver returns Some. **Measured offsets (ground
truth — what the driver writes on iron):**

| resolver | value |
|---|---|
| `gfx_regs.cp_rb0_base` | `0xc100` (real SOC15 — NOT the legacy `0x3040` that was bricking the CP readback) |
| `gfx_regs.grbm_status` | `0x8010` (coincidentally == legacy, which is why the reg-probe looked plausible) |
| `gfx_regs.cp_rb0_wptr` | `0xc150` |
| `gfx_regs.cp_me_cntl` | `0x2a00c` |
| `sdma_regs.rb_cntl` | `0x4b80` |
| `smu_mailbox` (msg/arg/resp) | `0x10a08 / 0x10a48 / 0x10a68` |
| `ih_ring.rb_base` | `0x4484` |
| `rlc_safe_mode` | `0x2a600` |
| `config_memsize` | `0x378c` (== the daemon's hand-derived `0xde3<<2` — independent confirmation) |

`parse_checked` (sig `0x28211407`) rejects a bad/wrong blob before any offset is
trusted. To re-capture for a different SKU, repeat the live-USB `cat` and re-run
`discovery_probe`.

## Display

| Fact | Value | Consumer |
|---|---|---|
| Panel | Samsung `SAM76E0` external monitor | EDID parser KAT |
| Native mode | **1920x1080 @ 180 Hz** (stage 7's 1080p choice is right; refresh is 180, not 60) | `init_display`, VRR pacer |
| EDID | captured: `firmware/edid/athena-beelink-elitemini/sam76e0-{128,256}.bin` (256-byte blob has the CTA ext) | Phase 2.3 host KAT |
| VBIOS | extracted from the captured VFCT ACPI table → `firmware/vbios/1002-15bf.bin` (16896 B, `ATOM` sig @ 0x194, build 09/15/23); APUs have no expansion ROM — VFCT is the only VBIOS source | stage 2 `read_vbios` fallback + `atombios::parse_vfct` real-data KAT |

## Network

| Fact | Value | Consumer |
|---|---|---|
| Ethernet | Realtek RTL8125 `10EC:8125` rev 05 (= 8125B), subsys `1F4C:B016` | `rtl8125.rs` (RX is live-fix item) |
| Wi-Fi | **MediaTek MT7902** `14C3:7902` — NO linux-firmware blob, NO mainline Linux driver | kills the iwlwifi-on-Athena plan; see `docs/FIRMWARE.md` Wi-Fi section |

## How this was collected (repeatable on any Windows target SKU)

```powershell
Get-WmiObject Win32_VideoController | Select Name, PNPDeviceID, AdapterRAM,
  CurrentHorizontalResolution, CurrentVerticalResolution, CurrentRefreshRate
Get-WmiObject Win32_PnPAllocatedResource   # filter Dependent by the GPU DeviceID,
  # join Win32_DeviceMemoryAddress on StartingAddress for BAR ranges
Get-ChildItem HKLM:\SYSTEM\CurrentControlSet\Enum\DISPLAY -Recurse  # EDID bytes
```

The same session also explains why nothing here came from a AthenaOS boot: the
Athena box has **no toolchain** — collection from the installed OS is the
zero-flash way to ground-truth a new SKU before its first AthenaOS boot.
