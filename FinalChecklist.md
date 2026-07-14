# RaeenOS — Final Completion Checklist

**Generated:** 2026-06-26  
**Authority:** `RaeenOS_Concept.md` > `MasterChecklist.md` > this file  
**Status legend:** `[x]` = iron-proven · `[~]` = QEMU-only / host-KAT / partial · `[ ]` = not built  
**Source of truth for live status:** `MasterChecklist.md`. This file is a *completion snapshot* — update MasterChecklist first, then this.

---

## Iron-Proven Baseline (what we know works on real Athena Ryzen 5 7640HS)

As of 2026-06-25 KVM session: `System successfully booted`, 0 panics, `boot health: 7/7 critical PASS → HEALTHY`.

| Area | Iron-proven |
|---|---|
| SMP | 12/12 logical CPUs online, x2APIC, AMD CPPC/SVM/SMCA |
| Memory | 15.8 GiB managed via real UEFI memory map |
| ACPI | 37 tables, 159 devices / 2818 values, 84 _PRT IRQ-routing entries |
| Storage | NVMe real SSD (incl. ESP BOOTLOG.TXT writes), AHCI, full RaeFS suite |
| USB | 4/4 xHCI controllers bound, HID keyboard armed, hubs enumerated |
| Network | RTL8125 link up + TX proven (DHCPDISCOVER emitted) |
| Audio | Real HDA codec playback (`hda_playback=1`) |
| Security | AMD-EFCH watchdog, PCIe AER, full crypto KAT suite, secure-boot chain replay |
| Userspace | Dynamic linker (glibc), Linux clone()/threads, pthread_join, login UI |
| Boot time | ~11.1 s total on iron — **over the 6 s budget (open blocker)** |

---

## §1 — Live Fix Queue (go first — every item compounds the next iron boot)

- [ ] **Boot time: 11.1 s → < 6 s on iron** (Concept §Core 3 "boot under 6 s") — biggest single miss; root-cause profiling on iron needed; QEMU TCG is ~1.9 s (not representative)
- [ ] **RTL8125 RX proven on iron** — DHCP currently stuck at `Selecting`; RX path structurally wired but no iron DHCP `Bound` capture yet
- [ ] **USB-MSC boot stick enumerates on iron** — USB3-hub child probes time out (`GetPortStatus` / `GET_DESCRIPTOR`); SET_HUB_DEPTH fix is in code (8cfe00a) but **PENDING iron verification**
- [ ] **USB-C UART serial console on Athena** — currently relies on BOOTLOG.TXT + netlog; USB-C serial gives real-time debug without a flash cycle

---

## §2 — Phase 2: Bare-metal Useful (current focus)

### 2.1 USB

- [~] USB-MSC class driver end-to-end on iron (blocked: hub-child probe timeouts — see live fix)
- [~] USB HID keyboard: live typing reaches the OS on iron (armed, iron typing test still pending)
- [~] USB HID mouse: cursor updates on iron (wired, iron cursor pending next flash)
- [ ] USB power management (suspend / resume)
- [ ] USB 3.x SuperSpeed handling distinct from 2.x

### 2.2 NICs

- [~] RTL8125 **RX** proven on iron — DHCP must reach `Bound` (TX proven, RX blocked)
- [ ] Intel I225-V / I219 driver on iron
- [ ] Wi-Fi: Intel AX210 via LinuxKPI — not built
- [ ] Wi-Fi: scan / connect / WPA2-3 (dead-code stubs in WifiDriver — **not functional**)

### 2.3 Display / EDID

- [~] EDID / DDC-I2C parser (QEMU-only smoketest passed on iron; live monitor not tested)
- [ ] Hotplug detection (monitor unplug + replug)
- [ ] Multi-monitor enumeration

### 2.4 Power / Thermal

- [~] S3 suspend + resume, thermal throttling — `[~]` QEMU only
- [ ] Fan curve at OS level validated on Athena hardware
- [ ] Battery percentage updates over time (laptop-gated)
- [ ] Lid close logs `[acpi] lid closed` event

### 2.5 Reliability

- [~] Watchdog armed-mode test — structural, iron armed-mode test pending

### 2.6 HDA Audio

- [~] Full HDA codec topology walk (output-pin widget detection fails on Athena codec currently)
- [ ] Audio plays a 1-second test tone on iron (explicit iron test)
- [ ] Records from built-in mic on iron
- [ ] Bluetooth audio (requires BT stack — see Phase 17)
- [ ] System sounds
- [ ] Hotplug jack detect

---

## §3 — Phase 3: Installable

- [~] Graphical installer (`installer_ui.rs` 7-screen flow) — QEMU-verified, iron pending
- [~] GPT formatter + FAT32 ESP writer — QEMU-proven
- [~] RaeFS mkfs (on-disk format) — QEMU-proven
- [~] Atomic A/B kernel update slots — QEMU-proven; **persisted `RAESLOT.CFG` + real staged-kernel reboot on Athena pending**
- [~] Secure-boot chain verified end-to-end — replay proven; TPM-sealing path pending
- [~] FDE (RaeFS encryption) — QEMU-proven
- [ ] **Live USB boots, installer formats Athena's NVMe, copies kernel, reboots without USB into working RaeenOS** (the full install-to-installed-disk path — the iron gate for Phase 3)
- [ ] User account + password persists across reboot on installed system
- [ ] Network setup screen during install (DHCP already runs at boot; no installer network UI)
- [ ] Language / region / keyboard graphical picker
- [ ] Privacy/permission disclosure screen
- [ ] Passphrase entry from USB HID at boot (FDE unlock UX)
- [ ] TPM 2.0 unsealing path for FDE keys
- [ ] Dual-boot NVRAM `SetVariable` for boot-entry (GPT carve done; UEFI `SetVariable` gap)
- [ ] ISO build pipeline

---

## §4 — Phase 4: Production-Ready Kernel

- [ ] **24-hour soak on Athena: no panic, no OOM, no scheduler stall** (Concept §Core 3 reliability gate)
- [ ] IOMMU enforcement live (VT-d / AMD-Vi tables parsed; **enforcement** — drivers don't all map DMA through IOMMU yet)
- [ ] TSC sync confirmed clean across all 12 Athena CPUs (warp-test fix landed; multi-boot verification needed)
- [ ] Real MCE handler (SMCA 32-bank structure parsed; handler just logs today)
- [ ] Real AER handler (caps walked; correctable/uncorrectable actions pending)
- [ ] Crash dump to NVMe (minidump on panic)
- [ ] Cross-CPU IPI for scheduler rebalance
- [ ] Hybrid p-core / e-core awareness with priority routing (Alder Lake+)
- [ ] Work-stealing root-cause resolved (currently `WORK_STEALING_ENABLED=false` — intermittent steal-resume race `stack_ptr==0`)
- [ ] x2APIC for >256 logical CPU IDs (server-class, future)
- [ ] KCFI: kernel CFI via `-Z sanitizer=kcfi`
- [ ] Network throughput sustained under 24h load
- [ ] NVMe throughput sustained under 24h load
- [ ] `[BOOT-BENCH]` < 6000 ms (machine-enforced gate — currently failing on iron)

---

## §5 — Phase 5: RaeFS Deep Features

- [~] Tiered storage (NVMe/SATA/HDD tiers) — structural, not iron-exercised
- [~] Game-aware extents / large sequential read optimization — structural
- [~] Zstd compression (transparent) — `[~]`
- [~] Per-app data buckets — `[~]`
- [~] Versioned config (every config file auto-versioned, roll back one click) — structural
- [~] Snapshot rollback (one-click) — QEMU-proven; iron end-to-end pending
- [ ] **TPM 2.0 sealing of root keys** (Concept §RaeFS encryption)
- [ ] Hardware AES-NI fast path (software AES only today; `aes_ni` capability `false` under QEMU TCG)
- [ ] Performance: < 5% throughput penalty vs unencrypted on NVMe
- [ ] Encrypted root mounts with TPM-sealed key end-to-end
- [ ] fsck / repair tool (`rae_fsck`)
- [ ] RaeFS on Athena NVMe root partition (currently ESPwrite-only on iron; RaeFS volume on NVMe unproven)
- [ ] Network shares (SMB — not planned for 1.0)
- [ ] File associations / default apps

---

## §6 — Phase 6: RaeGFX / Vulkan Demo (Year-1 open deliverable)

**This is the largest open Year-1 gap.** The Concept promises "Kernel + RaeFS + RaeGFX + RaeUI hello world. Boots, draws, plays a single Vulkan demo." The demo today is software raster — `vkQueueSubmit`-equivalent on real silicon is not built.

- [~] WGSL → SPIR-V toolchain (6 effect shaders host-KAT'd; live GPU submit pending)
- [~] amdgpu bring-up (discovery, VBIOS, GMC, IH, SMU mailbox, GFX first-light, GART, modeset all running on VFIO KVM; SDMA RS64 F32 microcontroller not yet executing — rung-1 still `[ ]`)
- [ ] **`vkQueueSubmit`-equivalent path on real AMD/Intel GPU** (the Year-1 Vulkan demo target)
- [ ] SDMA ring execution on Athena (RS64 F32 wakeup / DEC_START / PROGRAM registers)
- [ ] GEM / command-submission / wait-fence ioctl handlers (Mesa seam)
- [ ] virtio-gpu 3D commands (QEMU path for the GPU submit test)
- [ ] wgpu backend wired and tested on virtio-gpu
- [ ] Direct scanout from GPU buffer for exclusive fullscreen (skip CPU blit)
- [ ] Frame time < 16.6 ms at 1080p on Athena
- [ ] Multi-monitor: independent framebuffers
- [ ] HDR (Concept §RaeGFX first-class; not built)
- [ ] VRR (FreeSync/G-Sync) end-to-end on a real panel
- [ ] DPI / fractional scaling
- [ ] Color management (ICC profiles)
- [ ] Shader cache shared across Vulkan + RaeGFX (struct exists; no GPU pipeline to populate)
- [ ] Shader cache persistent across reinstalls
- [ ] Compositor capture / zero-cost recording (Concept §Gaming)

---

## §7 — Phase 7: Audio Engine

- [~] HDA full codec walk — output-pin widget detection incomplete on Athena codec
- [~] SCHED_GAME audio thread (128-frame/2.67ms period) — structural
- [ ] **Sub-3ms round-trip verified on Athena** (Concept §RaeAudio hard target — not yet measured)
- [ ] Per-app volume control + routing matrix (VoiceMeeter-class, Concept §Pro Gaming)
- [ ] Low-latency / pro audio path end-to-end (ASIO equivalent)
- [ ] Sound Open Firmware (SoF) DSP loading for newer Intel chips
- [ ] Bluetooth audio (requires BT stack)
- [ ] USB audio class (UAC1 / UAC2 via xHCI isochronous)
- [ ] Reports round-trip latency measurement to `/proc/raeen/perf`

---

## §8 — RaeUI / RaeKit

- [~] RaeUI (Label + Button + Frame + theme palette over software Canvas) — functional on QEMU
- [~] RaeKit declarative SDK (`view!` macro, reactive state, app lifecycle, NavigationStack/TabView) — compiled
- [ ] **Skia integration wired** (Concept §Language Stack: "RaeUI sits on Skia for 2D") — not wired
- [ ] **wgpu integration wired** (Concept §Language Stack: "wgpu for 3D and GPU effects") — not wired
- [ ] Text rendering at native quality on Athena (Skia-backed; SW raster today)
- [ ] Live wallpapers GPU-accelerated, paused when occluded (Concept §Customization)
- [ ] Window animations curve-editable by users (Concept §Customization)
- [ ] DPI / fractional scaling in layout engine
- [ ] Global text scale factor honored by layout
- [ ] cosmic-text layout engine integration for correct glyph positioning
- [ ] `zig cc` hermetic C/C++ cross-compile toolchain for Mesa/DXVK/Skia/LinuxKPI dependencies

---

## §9 — RaeShield (Security Model)

- [~] Capability-based syscall enforcement — core SOUND; fail-open for untracked PIDs (bring-up mode)
- [~] App sandboxing (sandbox smoketests pass; **fail-open for untracked PIDs — KNOWN**)
- [~] Per-app permission prompts (`perm_prompt.rs`) — wired; UI polish pending
- [~] Code signing / Ed25519 bundle signing — structural; "unverified dev" UX pending
- [~] `RaeManifest.toml` per-app permission manifests — compiled; not enforced end-to-end
- [ ] **Default-Deny sandbox** for untracked PIDs (flip from fail-open — requires per-app manifests Phase 9 complete)
- [ ] Per-app manifest enforcement at launch (not at policy-check time)
- [ ] IOMMU enforcement for userspace drivers (tables parsed; DMA domains not enforced)
- [ ] TPM 2.0 sealing of root keys (Concept §Security: "hardware-backed encryption on certified hardware")
- [ ] Memory tagging (MTE on ARMv8.5 / LAM on Intel — tracked as CPUs ship it)
- [ ] EAC/BattlEye partnership conversation ready (Concept §Compatibility: "partnerships day one")
- [ ] Parental controls / screen time (out of 1.0 scope)

---

## §10 — RaeNet

- [~] TCP/UDP socket syscalls 121–125 — structural
- [~] TLS 1.3 full RFC 8446 client handshake — 105/105 host KATs; **live socket integration + iron pending**
- [~] WireGuard Noise IKpsk2 — handshake `[x]`; full tunnel (data plane) pending
- [~] DNS resolve (`SYS_NET_DNS`) — wired; live upstream resolve iron-gated on DHCP
- [~] QUIC priority + traffic shaping — smoketests pass; iron-gated
- [ ] **`curl https://example.com` from userspace returns HTTP/2 response on Athena** (the e2e net proof)
- [ ] **WireGuard tunnel to a real remote endpoint passes ping**
- [ ] DHCP reaches `Bound` on iron (blocked by RTL8125 RX — see live fix)
- [ ] Captive portal handling
- [ ] Network settings UI (DHCP works; no user-facing UI)
- [ ] Wi-Fi (scan/connect/WPA2-3) — Intel AX210 via LinuxKPI; not built
- [ ] Hotspot / tethering (not planned for 1.0)
- [ ] SMB file sharing (not planned for 1.0)
- [ ] `SYS_NET_STATUS` for blocking client

---

## §11 — RaeBridge (Windows Compatibility) — HUMAN-GATED

> **Do not start net-new RaeBridge guest-execution work without explicit owner assignment.**  
> Design, host-KATs, and ABI surface are fair game; guest-execution is human-gated.

- [~] 36 Win32 DLL shims (kernel32/user32/gdi32/ntdll/ws2_32/msvcrt) — compiled + dispatch wired
- [~] PE loader + IAT patching — compiled
- [~] D3D9/11/12/DXGI translation — compiled
- [~] SEH engine (.pdata/.xdata unwind, `__C_specific_handler`) — host-KAT'd 14/14
- [~] Registry hive (Steam + DirectX + VC++ keys) — structural
- [ ] **DXBC → SPIR-V shader compilation** (the actual DX11/12 blocker for game rendering)
- [ ] **Machine-code trampoline emitter** (16-byte IAT stubs that bounce into Rust handlers)
- [ ] x64 Microsoft calling convention marshaling layer
- [ ] LDR structures: PEB, TEB, in-process module list
- [ ] TLS (thread-local storage) for Windows threads
- [ ] Win32 → POSIX semantic gaps (file handles, `\` paths, registry-equivalent)
- [ ] Phase C: 200 most-imported kernel32/ntdll names with real implementations
- [ ] Phase D: user32 + gdi32 — windowing + GDI primitives
- [ ] DXVK port: DirectX 9/10/11 → Vulkan translation (zlib-licensable)
- [ ] VKD3D-Proton port: DirectX 12 → Vulkan
- [ ] **`notepad.exe` runs, types text, saves a file** (the first RaeBridge iron milestone)
- [ ] **Steam launcher opens** (the non-negotiable gaming-OS milestone)
- [ ] **One AAA game runs at native parity vs Windows**
- [ ] Anti-cheat Tier 1: EAC + BattlEye user-mode paths (kernel side proven; userspace ceremony pending)

---

## §12 — Gaming-First Features

- [~] SCHED_GAME hard real-time class — EDF exists; deadline telemetry wired to `/proc/raeen/perf`
- [~] GameOS Mode (couch UI, 1633 lines, controller nav) — QEMU-proven; iron interactive pending
- [~] Per-game CPU power cap — iron-proven via KVM session
- [~] Per-game refresh rate switch — iron-proven via KVM session
- [~] VRR pacing — iron-proven via KVM session
- [~] DualSense / Xbox controller parse — `[x]` HID; haptics/gyro/adaptive triggers pending
- [~] Memory pinning API for games (`MemoryPinManager`) — structural
- [~] NULL_LATENCY mode — structural
- [~] Background process throttling in-game — structural
- [ ] **Direct-to-GPU exclusive fullscreen** (skip compositor — zero overhead; requires real GPU submit)
- [ ] Per-game shader cache on disk + persistent across reinstalls
- [ ] In-game overlay: FPS, frametime graph, CPU/GPU temps, voice chat, screenshots (the "Game Bar that doesn't suck" Concept promise)
- [ ] Steam works (Concept: "Steam day one or there is no gaming OS") — blocked on RaeBridge + GPU
- [ ] Top-20 games run via RaeBridge + Steam at native parity
- [ ] GameOS Mode booted from controller-only on iron
- [ ] Polling-rate control for mice (1000 Hz / 8000 Hz)
- [ ] Background YouTube + foreground game: no measurable FPS drop
- [ ] Compositor capture + stream (zero-cost recording, no OBS overhead)
- [ ] DualSense haptics + adaptive triggers end-to-end
- [ ] Game controller gyro input

---

## §13 — Customization Engine

- [~] Vibe Mode (12 theme presets, smooth ARGB transitions, time-based auto-switch) — QEMU-proven
- [~] Theme engine at compositor level (WGSL → SPIR-V shaders) — host-KAT'd; live GPU submit pending
- [~] raelang scripting interpreter (sandboxed Swift-flavored automation) — structural
- [~] RGB unified API (`RgbManager`, `RgbDevice`, `RgbEffect`) — structural
- [~] Fan curve / power management at OS level — structural
- [~] Swappable window managers (tile/stack/float/hybrid) — tiling WM and resize iron-proven
- [ ] **Live wallpapers** GPU-accelerated, paused when occluded (Concept §Customization)
- [ ] Window animations curve-editable in user-facing UI
- [ ] Desktop widget system (Rainmeter-class, sandboxed — Concept §Customization)
- [ ] Drive a real USB-attached LED strip via OpenRGB-compatible protocol
- [ ] Drive a real RGB keyboard
- [ ] Per-device peripheral profile persistence
- [ ] RGB strip changes color when a game enters foreground
- [ ] Virtual desktops (Spaces / Task View equivalent)
- [ ] Swappable shell (replace RaeShell with a competing one via first-class API)
- [ ] Overclocking utilities at OS level (CPU/GPU frequency + voltage API — structural, not exposed as UX)
- [ ] Display night light / blue-light filter

---

## §14 — Shell & First-Party Apps

### Shell

- [~] RaeShell default desktop (taskbar, start menu, tray w/ live RTC clock, notifications) — QEMU-proven
- [~] Window move/resize/min/max, compositor multi-window — QEMU-proven
- [~] App switcher (Alt+Tab `cycle_alt_tab`) — QEMU-proven
- [~] Command palette (fuzzy cmd/app/settings) — QEMU-proven
- [~] Notifications + center (HISTORY_CAP=64 ring + glass panel) — QEMU-proven
- [~] Quick settings / Control Center (5-toggle strip + DND/Focus) — QEMU-proven
- [~] Clipboard history + pin (kernel ring, syscalls 268–273) — QEMU-proven
- [ ] **App launch from start menu** on iron (click → app opens — last shell interactive gap)
- [ ] Virtual desktops (Spaces equivalent)
- [ ] Desktop widgets board
- [ ] Fast user switching end-to-end

### Built-in Apps

- [~] **Files** (tabs, batch rename, Trash, Quick Look PNG) — QEMU-proven; broader format wiring pending
- [~] **Terminal** (full VT100/xterm emulator, SGR-color) — QEMU-proven
- [~] **Settings** (basic) — QEMU-proven; full catalog in `docs/SETTINGS_CATALOG.md`
- [~] **Text Editor** — QEMU-proven
- [~] **Calculator** (`rae_calc` 17 KATs) — QEMU-proven
- [~] **Photos** (full decode stack BMP/GIF/PNG/JPEG/WebP, encode PNG/JPEG) — host-KAT'd; app-wiring + iron pending
- [~] **Music player** (WAV/FLAC/MP3/AAC/Opus decoders, `rae_mp4` demux, SCHED_GAME GameMixer) — host-KAT'd; iron HDA + app-wiring pending
- [~] **System Monitor** (`/proc/raeen/*` data exists; no app)
- [~] **Clock / Alarms** — QEMU-proven
- [~] **Notes** (md edit + live `rae_markdown` preview) — QEMU-proven
- [~] **Office** (`rae_docx`/`rae_xlsx`/`rae_pdf` + XLSX formula compute + `rae_print` PDF-1.7) — host-KAT'd; app UI + iron pending
- [~] **Mail / Calendar** (`rae_mail` SMTP/IMAP/POP3 + `rae_pim` iCal/vCard/RRULE) — host-KAT'd; live TLS/TCP wiring + app UI + iron pending
- [~] **Web browser** (`rae_js` parse+execute+Map/Set/RegExp+Promise/event-loop + raeweb HTML/CSS) — host-KAT'd; layout/render + DOM bindings + app-wiring pending
- [~] **PWA install** (`rae_pwa` URL resolution + InstallDescriptor) — host-KAT'd; render + launch wiring pending
- [~] **RaePlay** (Steam/Epic/GOG/RaeStore unified launcher, playtime, achievements) — compiled; no live game launching
- [ ] Screenshot / screen record app (compositor capture — `[ ]`)
- [ ] File associations / default apps
- [ ] Camera (out of scope for 1.0)
- [ ] Web browser: H.264 video decode (codec not built)
- [ ] Web browser: async/await suspension (Promise event-loop exists; suspension not wired)

---

## §15 — Services (RaeStore, RaeID, RaeSync)

- [~] RaeStore `.raepkg` bounds-checked TLV codec + fail-closed Ed25519 verify + transactional dep-correct install/GC — host-KAT'd; client UI + iron pending
- [~] RaeID WebAuthn EdDSA+ES256 ceremony core + sessions + guest — host-KAT'd; authenticator UX + iron pending
- [~] RaeSync E2E (device enroll, wrapped group key, AEAD SyncBlob, LWW-CRDT convergence) — host-KAT'd; **server + drive UX + iron pending**
- [~] `rae_keychain` (Argon2id KDF + ChaCha20Poly1305 AEAD, fail-closed, zeroized) — host-KAT'd; OS integration + UI pending
- [~] `rae_otp` HOTP/TOTP — host-KAT'd; authenticator UX pending
- [ ] RaeStore server backend (API, CDN, payment processor — out of scope for kernel work; in scope for OS shipping)
- [ ] 12% revenue share infrastructure
- [ ] Sideloading UX (allowed + supported by Concept; no UX built)
- [ ] Update channels: stable / beta / nightly
- [ ] Delta / efficient OS updates
- [ ] Driver update pipeline (signed)
- [ ] Sign up with RaeID → install app on device A → suggested on device B (the sync proof)
- [ ] `rae_otp` authenticator UX wired to system login

---

## §16 — Installer / Distribution

- [~] `scripts/flash-usb.ps1` (refuses internal drives) — `[x]`
- [~] `--safe` image + `safe_mode_guard_write` — `[x]` iron-proven
- [~] Graphical installer (7-screen flow, dry-runs in safe mode) — QEMU-verified
- [~] Bootable UEFI image — `[x]` built; UEFI boot on real firmware confirmed
- [ ] **Non-developer can flash USB, boot Athena, install RaeenOS, log in, browse files — without reading a wiki** (the consumer install gate)
- [ ] Dual-boot NVRAM `SetVariable` for boot-entry (bootloader gap)
- [ ] Microsoft-3rd-party-CA-signed shim or self-signed key enrollment (Secure Boot compatibility)
- [ ] Update channels: stable / beta / nightly with user opt-in
- [ ] ISO build pipeline
- [ ] Release notes per build
- [ ] RaeenOS Pro / Studio / Enterprise pricing tiers (business model)

---

## §17 — Hardware Coverage Matrix

- [ ] **Bluetooth stack** (no Bluetooth driver at all; blocks BT audio, BT keyboard/mouse, BT controllers)
- [ ] Intel I225-V / I219 NIC driver on iron
- [ ] NVIDIA GPU — **out of scope for 1.0** (nouveau / proprietary blob both expensive)
- [ ] Intel GPU (i915 via LinuxKPI userspace) — structural harness; real submit not built
- [ ] AMD GPU (amdgpu via LinuxKPI userspace) — bring-up proven end-to-end in VFIO KVM; SDMA execute still blocked
- [ ] S0ix modern standby (Intel firmware-driven) — out of scope for initial release
- [ ] VT-d (Intel) DMAR table parsed and **active** (AMD-Vi structural; VT-d not tested on Intel SKU)
- [ ] Keyboard layouts / IME (input method editors for CJK and other complex scripts)
- [ ] Trackpad gestures
- [ ] Printer / scanner support — out of scope for 1.0
- [ ] Webcam — out of scope for 1.0
- [ ] Pen / touch — out of scope for 1.0
- [ ] AX210 Wi-Fi via LinuxKPI — not built

---

## §18 — Business & Community (non-engineering)

- [ ] Dev community formed
- [ ] RaeKit tools free for first-year developers
- [ ] Free signing for first-year developers
- [ ] World-class developer docs
- [ ] Swift app-development onramp (toolchain + RaeKit bindings so a macOS app ports in a week)
- [ ] App store at 12% revenue share
- [ ] Hardware OEM RaeReady certification program
- [ ] First-party Rae Station hardware shipped
- [ ] EAC / BattlEye partnerships closed
- [ ] Five-year roadmap public
- [ ] RaeenOS Pro at $30 / $5-mo pricing tier
- [ ] RaeenOS Studio pricing tier
- [ ] Enterprise licensing

---

## §19 — Accessibility

Most of the a11y stack is **built but not wired.** The remaining work is integration + user-reach, not new engines.

- [~] Accessibility tree + AT ABI (`kernel/src/a11y.rs` 1236 lines, cap-gated, R10-complete) — `[~]`; widget-provider wiring has **ZERO callers** (P0 gap)
- [~] Screen reader announce core (`announce_node` / `describe_focused`) — `[~]`; no real TTS→RaeAudio sink
- [~] Magnifier (compositor source-sampled 1×–8×, focus-follows pan) — `[~]`
- [~] Color filters (invert/grayscale/HC) — `[~]` engine complete
- [~] High-contrast forced-colors live mode — **SHIPPED** (`[x]` for mechanism; iron-unproven)
- [~] On-switches: Super+Alt+M/H/C/R hotkeys + Control Center Accessibility tile — **SHIPPED**
- [~] Keyboard-only nav + visible focus ring — partial (`focusable_nodes()` + `FocusRing` exist)
- [~] Sticky/slow/bounce keys + repeat filters — iron-proven via KVM session
- [ ] **Widget-provider wiring** — `a11y::publish_window_widgets` + `raeui::provider_nodes_for_window` exist but have ZERO callers; every app is an anonymous "Window" node to a screen reader (P0)
- [ ] Unified desktop keyboard focus order across shell chrome (taskbar/start/tray/notifications)
- [ ] Modal focus-trap contract
- [ ] FAIL-able "no-mouse" audit: a complete task done with keyboard only
- [ ] FAIL-able WCAG contrast audit at boot over the full painted palette
- [ ] Real TTS→RaeAudio `AudioSpeechSink` (iron/Phase 7 gated)
- [ ] Global text scaling (type ramp exists; no global scale factor honored by layout)
- [ ] Per-app screen-reader nav verbs on live keys
- [ ] Demote / harvest `components/raeaccessibility` (2268 lines, nothing imports it — duplicates live `a11y.rs`) — **do not invest in it as-is**

---

## §20 — Multi-Arch (aarch64 — #3 Concept promise)

> The Concept: "the same OS boots x86_64, aarch64, and i686." Current: x86_64 only.

- [x] Arch-neutral address types (`arch::PhysAddr/VirtAddr` — transparent aliases on x86_64)
- [x] `arch::mmu` HAL (map/unmap/translate/mmap/mprotect/kernel-stack/PML4-create/CR3-switch — seam surface complete)
- [x] `arch::interrupts` / `arch::cpu` / `arch::interrupt_controller` / `arch::timer` seams
- [~] `components/aarch64_logic` — MMU descriptor encoder + ESR decode + GICv2 + DTB walk — 33/33 host KATs; no boot yet
- [ ] aarch64 target triple + toolchain setup (`aarch64-unknown-none`)
- [ ] aarch64 boot entry (EL2→EL1 transition, MMU on, early UART)
- [ ] aarch64 MMU bring-up (MAIR_EL1, TCR_EL1, TTBR0/TTBR1, stage-1 tables)
- [ ] GIC v2/v3 driver (GICD/GICC/ITS)
- [ ] PSCI SMP bring-up (CPU_ON, secondary boot)
- [ ] DTB parsing for QEMU-virt (memory, UART, GIC, timer bases)
- [ ] `qemu-system-aarch64 -M virt` boots to `System successfully booted`
- [ ] i686 port (32-bit x86) — deferred after aarch64

---

## §21 — Ship Gate (1.0 Definition)

All of the following must be `[x]` (iron-proven or consumer-proven):

- [ ] Phase 1 + 2 + 3 GREEN on at least 2 SKUs
- [ ] Phase 4 GREEN on at least 1 SKU (24 h soak passes)
- [ ] Phase 5 GREEN
- [ ] Phase 6 GREEN (Vulkan triangle visible on real GPU)
- [ ] Phase 7 GREEN
- [ ] Phase 8 GREEN
- [ ] Phase 9 GREEN
- [ ] Phase 10 GREEN
- [ ] Phase 11 GREEN with Steam + 1 AAA game working
- [ ] Phase 12 GREEN with controller-only living-room flow
- [ ] Phase 13 GREEN
- [ ] Phase 14 GREEN with Three User Experiences demonstrable
- [ ] Phase 15 GREEN
- [ ] Phase 16 GREEN — non-developer can install and use without a wiki
- [ ] Phase 19 GREEN (accessibility — ship gate)
- [ ] Boot time < 6 s on NVMe (Concept §Core 3 — **currently failing on iron at ~11.1 s**)
- [ ] G1 Daily Driver gate: install → log in → browse files → get on Wi-Fi → run an app → sleep/wake → update → shut down, no terminal
- [ ] G2 Switcher gate: G1 + run existing Windows apps via RaeBridge + hardware telemetry + theme + driver management
- [ ] G3 Gamer gate: G2 + Steam works + 1 AAA title runs + controller-only living-room + VRR/HDR + per-game profiles

---

## Completion Snapshot (2026-06-26)

| Layer | Concept Promise | Honest Status |
|---|---|---|
| **Kernel core** | Hybrid, SCHED_GAME, capability-gated | `[x]` iron-proven |
| **SMP / ACPI / APIC** | 12 CPUs, full ACPI AML | `[x]` iron-proven |
| **RaeFS** | CoW, snapshots, FDE, buckets | `[~]` QEMU-proven; iron NVMe root unproven |
| **RaeGFX** | "Looks like Metal, performs like Vulkan" | `[ ]` SW raster only; real GPU submit not built |
| **RaeAudio** | Sub-3ms round-trip | `[~]` HDA playback `[x]`; sub-3ms unverified |
| **RaeUI / RaeKit** | SwiftUI-style, GPU-accelerated, Skia+wgpu | `[~]` Skia/wgpu not wired; SW canvas works |
| **RaeShield** | Capability sandbox, attestation | `[~]` Core sound; fail-open sandbox (bring-up) |
| **RaeNet** | WireGuard, TLS 1.3, QUIC | `[~]` TLS/WG host-KAT'd; iron net blocked on RX |
| **RaeBridge** | Steam day one | `[~]` Compiled; DXBC→SPIR-V and real GPU needed |
| **RaePlay** | Steam/Epic/GOG unified | `[~]` Compiled; no live game launching |
| **Customization** | Vibe Mode, themes, RGB | `[~]` QEMU-proven; GPU shaders pending |
| **Shell / Apps** | Desktop rivals Windows/macOS | `[~]` QEMU-proven; app launch on iron pending |
| **Installer** | Non-dev can install, no wiki | `[~]` QEMU-proven; full iron install pending |
| **Accessibility** | Screen reader, magnifier, HC | `[~]` Mostly built; widget provider has 0 callers |
| **aarch64** | "Runs on any silicon" | `[ ]` Seam built; no aarch64 boot yet |
| **Boot time** | < 6 s (target 3 s) | `FAILING` — ~11.1 s on iron |

**Rough completion estimate vs 1.0:** ~40–45% of the consumer-grade deliverables are iron-proven or host-KAT'd. The remaining ~55–60% spans real GPU work (Year-1 open gap), RaeBridge guest execution (human-gated), sub-3ms audio proof, full installer flow, 24 h soak, and the full app/shell wiring that takes libraries from host-KAT to usable on iron.
