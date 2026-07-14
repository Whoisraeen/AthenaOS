# AthenaOS Settings Catalog & UI Map

The complete settings surface for the userspace **Settings** app — every toggle,
slider, and picker AthenaOS should expose, at parity with Windows 11 Settings,
macOS System Settings, and GNOME/KDE, plus the AthenaOS-native categories
(Gaming, RGB/Vibe, AthGuard, AthBridge, Drivers).

This is both a **product spec** (what the UI ships) and a **build map** (each
setting names the subsystem that backs it + honest status).

---

## 0. Architecture — how Settings works

- **Runs entirely in userspace** (AthUI/AthKit app, `apps/settings/`). No setting
  reaches hardware directly; it goes through one of three backends below.
- **Three backends:**
  1. **`config_registry`** (`kernel/src/config_registry.rs`) — the versioned,
     hierarchical, snapshotted key store. Persistent prefs live here as namespaced
     keys (`display.night_light.enabled`, `gaming.sched_game.enabled`, …). Snapshots
     mean **every settings change is rollback-able** ("undo my last change" / restore
     a known-good profile) for free.
  2. **Live subsystem syscalls** — for things applied immediately to running state
     (set volume, change resolution, pin a driver, set a fan curve). The registry
     stores the *desired* value; a syscall pushes it to the live subsystem.
  3. **`/proc/raeen/*`** — read-only live state the UI displays (current driver pick,
     thermals, link status, battery health, sandbox denials).
- **Every write is capability-gated** via AthGuard: changing security/driver/network
  settings requires the matching `Cap`, and sensitive changes route through
  `perm_prompt` (the consent dialog). A sandboxed app cannot silently flip system
  settings.
- **Search across all panels** (Windows/mac-style "search settings") via the
  `search_index` over this catalog's keys + labels + synonyms.
- **Sync** optional per-category via AthSync/AthID (like iCloud/Microsoft-account
  settings sync), end-to-end encrypted.

**Status legend:** ✅ backing exists & wired · 🟡 backing partial · ⬜ planned (no
backing yet). The Settings *UI itself* is ⬜ today — this doc is its blueprint.

---

## 1. System

| Setting | Notes | Backing | Status |
|---|---|---|---|
| About (device name, CPU/RAM/GPU, OS build, serial) | mirrors Win "About" / mac "About This Mac" | cpu_features, acpi, `/proc/raeen/*` | 🟡 |
| Rename device | hostname | config_registry | ⬜ |
| OS updates (check, download, schedule, history) | Win Update / Software Update | raeupdate + A/B slots | 🟡 |
| Active hours / update deferral | | config_registry | ⬜ |
| Atomic update slots (A/B), rollback to previous build | AthenaOS-native | Phase 3.6 update slots | 🟡 |
| Storage overview + Storage Sense (auto-clean) | Win Storage Sense / mac Storage | raefs, tiered storage | 🟡 |
| Multitasking (snap layouts, alt-tab, virtual desktops) | | compositor | 🟡 |
| Clipboard history + sync | Win clipboard | clipboard (`/proc/raeen/clipboard`) | 🟡 |
| Recovery / Reset this PC (keep files / wipe) | | installer, raefs snapshots | 🟡 |
| Activation / licensing | | AthID | ⬜ |
| Telemetry / diagnostics level (Off/Basic/Full) | privacy-first default Off | audit, config_registry | 🟡 |

## 2. Display & Graphics

| Setting | Notes | Backing | Status |
|---|---|---|---|
| Resolution (per monitor) | | compositor, EDID/DDC (Phase 2.3) | 🟡 |
| Refresh rate + **VRR / adaptive sync** | embodiment-first | compositor VRR pacer | 🟡 |
| **HDR** on/off, peak brightness, tone-map | 10/12-bit BT.2020 | compositor HDR pipeline | 🟡 |
| Scaling / DPI (100–400%, fractional) | | compositor | 🟡 |
| Multi-monitor: arrange, primary, extend/mirror | | compositor | 🟡 |
| Orientation / rotation | | compositor | ⬜ |
| Night Light / color temperature schedule | Win Night Light / mac Night Shift | compositor, config_registry | ⬜ |
| Color profiles / ICC, calibration | | compositor | ⬜ |
| Brightness (panel + auto/ambient) | | power_supply, acpi | 🟡 |
| **GPU selection per app** (iGPU/dGPU) | Win "Graphics preference" | driver_manifest selection layer | ⬜ |
| Graphics driver backend (native vs LinuxKPI) | → see §13 Drivers | driver_manifest | 🟡 |

## 3. Sound & Audio

| Setting | Notes | Backing | Status |
|---|---|---|---|
| Output device + per-app volume mixer | Win Volume Mixer | raeaudio | 🟡 |
| Input device, mic level, mic boost | | raeaudio | 🟡 |
| Master volume / mute / balance | | raeaudio | 🟡 |
| Sample rate / bit depth / **latency (sub-3ms)** | pro/gaming audio | raeaudio | 🟡 |
| Spatial audio / surround | | raeaudio | ⬜ |
| **Routing matrix** (VoiceMeeter-class input→fx→output) | AthenaOS-native | raeaudio AudioRouter | 🟡 |
| Per-app audio device routing | | raeaudio | ⬜ |
| Sound effects / system sounds theme | | theme_engine | ⬜ |
| Mono audio (accessibility) | → §11 | raeaudio | ⬜ |

## 4. Bluetooth & Devices

| Setting | Notes | Backing | Status |
|---|---|---|---|
| Bluetooth on/off, pair, manage | | bluetooth.rs | 🟡 |
| Mice / trackpads (speed, scroll, natural scroll, gestures) | | input, hid | 🟡 |
| Keyboards (repeat rate, layout, shortcuts) | | input, hid | 🟡 |
| Pens / touch / tablet | | input | ⬜ |
| **Game controllers** (DualSense/Xbox: test, remap, deadzones, rumble, LED) | embodiment-first | input (DualSense/Xbox) | 🟡 |
| Printers & scanners | | (LinuxKPI / IPP) | ⬜ |
| USB devices tree + per-port power | | xhci, usb_core | 🟡 |
| AutoPlay / removable-media defaults | | config_registry | ⬜ |
| Mobile device / phone link | | AthSync | ⬜ |

## 5. Network & Internet

| Setting | Notes | Backing | Status |
|---|---|---|---|
| Wi-Fi (scan, connect, known networks, metered) | | net_drivers, LinuxKPI iwlwifi | 🟡 |
| Ethernet (IP, DHCP/static, DNS) | | net_drivers, smoltcp, DHCP | 🟡 |
| **VPN / WireGuard** (tunnels, keys, on-demand) | native WireGuard | tunnel.rs, WireGuard Noise | 🟡 |
| Proxy (manual/auto/PAC) | | raenet | ⬜ |
| DNS (servers, DoH/DoT) | | raenet, TLS 1.3 | 🟡 |
| **Firewall** (per-app, inbound/outbound rules, zones) | | AthGuard, sandbox net classes | 🟡 |
| Mobile hotspot / Internet sharing | | net_drivers | ⬜ |
| Airplane mode | | net_drivers, bluetooth | ⬜ |
| Data usage / per-app network stats | | raenet | ⬜ |

## 6. Personalization

| Setting | Notes | Backing | Status |
|---|---|---|---|
| **Themes** (8 built-in, custom) | | theme_engine | 🟡 |
| **Vibe Mode presets** (Cyberpunk Night, Ghibli Morning, Bauhaus…) | AthenaOS-native: wallpaper+colors+sound+fonts+cursor+anim as one | theme_engine | ⬜ |
| Accent color / auto-from-wallpaper | | theme_engine, compositor | 🟡 |
| Light / Dark / Auto-by-schedule | | theme_engine | 🟡 |
| Wallpaper (static, slideshow) | | compositor | 🟡 |
| **Live wallpaper** (GPU, pause when occluded) | AthenaOS-native | live_wallpaper | 🟡 |
| **Glassmorphism** (blur radius, transparency, vibrancy) | AthenaOS-native | compositor | 🟡 |
| Window animations (speed, curve editor, reduce-motion) | | compositor | 🟡 |
| Fonts (system font, size, install/manage) | | raeui, font engine | 🟡 |
| Cursor (theme, size, trail, color) | | compositor | 🟡 |
| Lock screen (wallpaper, widgets, clock) | | compositor | ⬜ |
| Start/Taskbar/Dock (position, size, behavior, pinned) | | raeshell | 🟡 |
| Sounds (system sound scheme) | | theme_engine, raeaudio | ⬜ |

## 7. Apps

| Setting | Notes | Backing | Status |
|---|---|---|---|
| Installed apps (list, uninstall, repair, move) | | app_bundle, raestore | 🟡 |
| Default apps (by type / by protocol) | | config_registry | ⬜ |
| Startup apps (enable/disable, impact) | | shell_runner | 🟡 |
| **App permissions** (per-app: camera/mic/location/files/…) | → §12 | AthGuard, rae_manifest | ✅ |
| Optional features / components | | app_bundle | ⬜ |
| **App bundle dependency view** (hashed deps) | AthenaOS-native | app_bundle (SYS_BUNDLE_VERIFY) | 🟡 |
| Offline maps / per-app storage / app data buckets | | raefs data buckets | 🟡 |
| App store settings (AthStore: auto-update, sources) | | raestore | ⬜ |

## 8. Accounts & Users

| Setting | Notes | Backing | Status |
|---|---|---|---|
| **AthID** account (sign in, profile) | like MS account / Apple ID | AthID | 🟡 |
| Local user accounts (add, type, admin) | | AthID, capability | ⬜ |
| Sign-in options: password, PIN, **biometrics**, security key | | AthID, AthGuard, TPM | ⬜ |
| Auto-login / lock policy / dynamic lock | | AthGuard | ⬜ |
| Family / parental controls / screen time | | AthID | ⬜ |
| **Settings sync** (which categories sync) | | AthSync | ⬜ |
| Work/school / enterprise enrollment (MDM) | | AthID | ⬜ |

## 9. Time & Language

| Setting | Notes | Backing | Status |
|---|---|---|---|
| Date & time (auto/manual, NTP server) | | rtc, raenet NTP | 🟡 |
| Time zone (auto-by-location/manual) | | config_registry | ⬜ |
| Region / formats (number, currency, first day) | | config_registry | ⬜ |
| Language (display, preferred order) | | i18n | ⬜ |
| Keyboard layouts / input methods (IME) | | input | ⬜ |
| Speech (TTS voice, recognition language) | | raeaudio | ⬜ |

## 10. Gaming (AthenaOS flagship)

| Setting | Notes | Backing | Status |
|---|---|---|---|
| **Game Mode / SCHED_BODY** (auto-prioritize foreground game) | hard real-time class | scheduler SCHED_BODY | 🟡 |
| Game Bar / in-game overlay (FPS, perf, chat) | | compositor | ⬜ |
| **Captures** (clips, screenshots, hotkeys, bitrate) | | compositor, raeplay | ⬜ |
| **GameOS / Couch mode** (big-picture UI) | AthenaOS-native | raeplay GameOS | 🟡 |
| Per-game profiles (power, RGB, resolution, fan) | AthenaOS-native | overclock, rgb, power_supply | 🟡 |
| **Anti-cheat — Tier 1 attestation** (per-game status) | userspace attestation, no ring-0 | anticheat | ✅ |
| **Anti-cheat — Tier 2 kernel AC consent** (per-game allow/revoke, "games allowed to load kernel AC", audit view, global hard-off) | for titles that require it (EAC/BattlEye/Vanguard); load-on-launch, signed, audited — see `ANTICHEAT_STRATEGY.md` | anticheat Tier 2, perm_prompt, rae_manifest | ⬜ |
| **AthBridge / Windows-game compatibility** (per-title) | Proton-lineage | raebridge | 🟡 |
| Shader cache management | | raegfx | ⬜ |
| VRR/low-latency mode per game | | compositor | 🟡 |

## 11. Accessibility

| Group | Settings | Backing | Status |
|---|---|---|---|
| Vision | text size, magnifier, high contrast, color filters (color-blind), cursor size, reduce transparency/motion | compositor, theme_engine | 🟡 |
| Hearing | mono audio, visual alerts, live captions | raeaudio, compositor | ⬜ |
| Interaction | sticky/filter/toggle keys, mouse keys, on-screen keyboard, dwell, voice control | input | ⬜ |
| Narrator / screen reader | speech, verbosity, braille | raeaudio, raeui a11y tree | ⬜ |

## 12. Privacy & Security (AthGuard)

| Setting | Notes | Backing | Status |
|---|---|---|---|
| **Per-app permissions** (camera, mic, location, files, network, devices) | mac/iOS-style consent | AthGuard, rae_manifest, perm_prompt | ✅ |
| **App sandbox level** (Trusted/AppSandbox/Strict) per app | AthenaOS-native | sandbox.rs | 🟡 |
| **Code signing / app trust** (verified developer, sideload warning) | | rae_manifest, secure_boot | 🟡 |
| Permission activity / which app used the mic | | audit, cap_audit | 🟡 |
| **Full-disk encryption (FDE)** on/off, recovery key | LUKS-equiv | raefs, rae_crypto | ⬜ |
| **Secure Boot** status, key enrollment | | secure_boot | 🟡 |
| **TPM** / measured boot / sealing | | tpm, security.rs | 🟡 |
| Firewall (→ §5) | | AthGuard | 🟡 |
| Find my device / remote wipe | | AthID, AthSync | ⬜ |
| Activity history / diagnostics data view + delete | | audit | 🟡 |
| Anti-theft / lockdown mode | | AthGuard | ⬜ |

## 13. Drivers & Hardware  *(the one you flagged)*

| Setting | Notes | Backing | Status |
|---|---|---|---|
| **Device Manager** (tree of all devices, status, conflicts) | Win Device Manager | driver_manifest, pci, `/proc/raeen/drivers` | 🟡 |
| **Driver backend per device** (Native Rust ⇄ LinuxKPI) | the choosable default — see `NATIVE_DRIVER_PLAN.md` | driver_manifest selection layer | 🟡 |
| Per-device **pin** to a specific driver | | driver_manifest DriverPolicy | ⬜ |
| Per-class preference (native-first / linuxkpi-first) | | driver_manifest DriverPolicy | ⬜ |
| Driver status (stable/experimental/stub), fallback log | | driver_manifest | 🟡 |
| **Firmware** (installed blobs, versions, update) | | firmware.rs, docs/FIRMWARE.md | 🟡 |
| Roll back / disable a driver | | driver_supervisor | ⬜ |
| **IOMMU sandboxing** per device (on/off, domains) | | iommu.rs | 🟡 |
| Hardware diagnostics (PCI/USB tree, IRQs, resources) | | pci, acpi, procfs | 🟡 |
| Driver auto-scan & install during setup | | driver_manifest required_linuxkpi_packages | 🟡 |

## 14. Power & Performance

| Setting | Notes | Backing | Status |
|---|---|---|---|
| Power plan (Balanced/Performance/Efficiency/Custom) | | power_supply, scheduler | 🟡 |
| **CPU power limits per game/app** | AthenaOS-native | overclock | 🟡 |
| **Overclock / undervolt** (HWP, CPPC, P/E core) | enthusiast | overclock, cpu_features | 🟡 |
| **Fan curves** (per-zone, custom points) | | overclock, i2c_spi | ⬜ |
| Thermal limits / throttle behavior | | power_supply, mce | 🟡 |
| Battery health, charge limit (80% cap), battery saver | | power_supply | 🟡 |
| Sleep / hibernate / **S3 suspend** behavior, wake timers | | acpi GPE, power_supply (Phase 2.4) | ⬜ |
| Screen/sleep timeouts (plugged/battery) | | power_supply, config_registry | 🟡 |
| USB selective suspend | | xhci, usb_core | ⬜ |

## 15. Storage & Filesystem (AthFS)

| Setting | Notes | Backing | Status |
|---|---|---|---|
| Disks & volumes (manage, format, mount) | | raefs, gpt, block_io | 🟡 |
| **Snapshots** (list, create, **one-click rollback**) | AthenaOS-native | raefs snapshots/CoW | ✅ |
| **Tiered storage** (hot/cold placement policy) | | raefs tiered storage | 🟡 |
| **Per-app data buckets** (quota, isolation) | AthenaOS-native | raefs data buckets | 🟡 |
| Trash / recycle bin (size, auto-empty) | | raefs | ⬜ |
| Drive optimization / TRIM | | nvme, raefs | ⬜ |
| Network drives / file shares | | raenet | ⬜ |
| Versioned config history (the registry itself) | AthenaOS-native | config_registry snapshots | ✅ |

## 16. Notifications & Focus

| Setting | Notes | Backing | Status |
|---|---|---|---|
| Notification center on/off, per-app | | compositor, raeshell | ⬜ |
| **Focus / Do Not Disturb** (schedules, priority, game auto-DND) | | compositor, scheduler | ⬜ |
| Banners, sounds, badges per app | | compositor | ⬜ |
| Notification history | | raeshell | ⬜ |

## 17. Search & Indexing

| Setting | Notes | Backing | Status |
|---|---|---|---|
| Search scope (folders included/excluded) | | search_index | 🟡 |
| Index now / rebuild | | search_index | 🟡 |
| Web results in search on/off (privacy) | | search_index | ⬜ |
| Settings search synonyms | | search_index over this catalog | ⬜ |

## 18. Compatibility (AthBridge)

| Setting | Notes | Backing | Status |
|---|---|---|---|
| Windows-app compatibility layer on/off | | raebridge | 🟡 |
| Per-app compat profile (Windows version, DLL overrides) | | raebridge | 🟡 |
| Proton/Wine-lineage runtime selection | | raebridge | 🟡 |
| POSIX/Linux-binary compatibility | | linux_syscall, linux_compat | 🟡 |
| DXVK/D3D→Vulkan translation toggles | | raebridge, raegfx | ⬜ |

## 19. Developer & Advanced

| Setting | Notes | Backing | Status |
|---|---|---|---|
| **Developer mode** (sideload unsigned, verbose logs) | | rae_manifest, AthGuard | 🟡 |
| Unverified-developer sideload UX (warn, not block) | AthenaOS-native | rae_manifest | 🟡 |
| Terminal / shell defaults (AthShell) | | raeshell terminal | 🟡 |
| SSH / remote access | | raenet | ⬜ |
| **Scripting layer** (Swift-like scripts) enable | | scripting.rs | ⬜ |
| Kernel/boot diagnostics (serial log, bootlog, procfs browser) | | bootlog, procfs | ✅ |
| Window manager API (tile/stack/float/hybrid swap) | AthenaOS-native | compositor | ⬜ |
| Swappable shell (replace AthShell) | AthenaOS-native | raeshell | ⬜ |

---

## 20. Parity notes

- **Covered beyond the big-3:** Gaming (§10), RGB/Vibe (§6), per-app sandbox + two-tier
  anti-cheat (userspace attestation + consented per-game kernel AC, §10/§12), driver
  backend choice (§13), FS snapshots/buckets (§15),
  versioned settings with rollback (§0) — these are AthenaOS-native and have **no direct
  Windows/mac/Linux equivalent**.
- **Windows-specific we map but rename:** Device Manager → §13; BitLocker → FDE §12;
  Storage Sense → §1; Game Bar → §10; Phone Link → §4/§8.
- **macOS-specific we map:** Night Shift → Night Light §2; True Tone/Display calibration
  → §2; Time Machine → Snapshots §15; Gatekeeper → Code signing §12; Spotlight → §17;
  Focus → §16; Continuity/Handoff → AthSync §4/§8.
- **Linux/GNOME-KDE we map:** Online Accounts → AthID §8; Color profiles → §2; Power
  profiles → §14; Wayland/X knobs → N/A (AthGFX/compositor own this).

## 21. Build order (when the Settings app is built)

1. **Settings backend service** — a userspace daemon over `config_registry` (read/write +
   snapshot/rollback) + a typed schema generated from this catalog (key, type, range,
   default, `Cap` required, live-apply syscall). One source of truth for every panel.
2. **Shell of the app** (AthUI): left nav = these categories, search bar over the schema,
   per-panel capability gating via AthGuard.
3. **Wire the ✅/🟡 backings first** (they already have live state) — Privacy/Permissions,
   Drivers, Storage/Snapshots, Gaming/Anti-cheat, Power — so the app is genuinely useful
   on day one, then fill the ⬜ rows as their subsystems land.
```
