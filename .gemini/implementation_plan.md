# Gemini Slice: MasterChecklist Items I Can & Should Complete

## My Ownership (from `agents/OWNERSHIP.toml`)

| Crate | Component |
|-------|-----------|
| `components/athfs` | AthFS filesystem |
| `components/athfat` | FAT filesystem |
| `components/athaudio` | AthAudio engine |
| `components/athmedia` | Media framework |
| `components/athnet` | AthNet networking |
| `components/athvpn` | WireGuard VPN daemon |
| `components/athgfx` | AthGFX graphics API |
| `components/athui` | AthUI framework |
| `components/athkit` | AthKit SDK |
| `components/athfont` | Font subsystem |
| `components/athshell` | AthShell desktop |
| `components/athlocale` | Locale/i18n |
| `components/athaccessibility` | Accessibility |

> [!IMPORTANT]
> I do **not** touch `kernel/`, `xtask/`, `ath_abi/`, `ath_driver_api/` (Opus), nor drivers, installer, apps, tests, LinuxKPI shim (Composer). Items below are filtered to my slice only.

---

## Phase 5 — AthFS Year-2 Deep Features (HIGH PRIORITY)

These are core Concept §AthFS promises. Most infrastructure exists (`[~]`), but end-to-end flows are unfinished.

### Can complete now (no cross-slice blockers)

| Item | Status | What remains | Concept alignment |
|------|--------|-------------|-------------------|
| **§5.1 Snapshot syscall surface** | `[~]` | Wire `sys_athfs_snapshot(name)` userspace API in `components/athfs` | §AthFS CoW snapshots |
| **§5.1 Snapshot restore userspace API** | `[~]` | Userspace snapshot syscall/UI flow | §"Atomic CoW updates with one-click rollback" |
| **§5.1 Time-machine UX retention policy** | `[ ]` | Hourly + daily + weekly retention logic | §AthFS snapshots |
| **§5.1 Snapshot disk quota** | `[ ]` | Quota enforcement so snapshots can't fill the drive | §AthFS |
| **§5.4 zstd encoder/decoder** | `[ ]` | In-component encoder/decoder (ruzstd already in deps) | §AthFS "Zstd by default" |
| **§5.4 Per-extent compression flag** | `[ ]` | Extent metadata flag for compression | §AthFS compression |
| **§5.5 Sequential prefetch** | `[ ]` | Read-ahead for game asset patterns | §"Game-aware extents" |
| **§5.6 Bucket-level encryption keys** | `[ ]` | Per-bucket crypto keys | §"Per-app data buckets" |
| **§5.7 Per-key config restore** | `[~]` | `sys_config_restore(key, version)` granularity | §"Versioned config" |
| **§5.8 `athfsck` userspace utility** | `[ ]` | Full userspace fsck tool | §AthFS reliability |

### Needs interface from Opus (file `NEEDS-INTERFACE:` note)

| Item | What I need |
|------|-------------|
| Snapshot syscall number | New syscall nr in `SYSCALL_TABLE.md` for `sys_athfs_snapshot` |
| Bucket encryption key syscall | Syscall for key provisioning per bucket |

---

## Phase 6 — AthGFX (MEDIUM-HIGH PRIORITY)

### Can complete now

| Item | Status | What remains | Concept alignment |
|------|--------|-------------|-------------------|
| **§6.2 AthGFX public API polish** | `[~]` | Expand Vulkan-equivalent API surface (textures, buffers, pipelines) in `components/athgfx` | §AthGFX "Vulkan-equivalent capabilities" |
| **§6.4 HDR pipeline (10/12-bit)** | `[ ]` | Implement in `components/athgfx` | §"Compositor-level HDR" |
| **§6.4 Color management** | `[ ]` | ICC profile handling in athgfx | §AthGFX |

### Blocked (needs GPU driver from Composer + kernel interface from Opus)

| Item | Blocker |
|------|---------|
| wgpu backend on virtio-gpu | Needs kernel virtio-gpu 3D (Opus) + driver (Composer) |
| VRR pacing | Needs real GPU driver |
| Glassmorphism compositor | Needs wgpu backend live |
| Drop shadows | Needs wgpu backend live |

---

## Phase 7 — AthAudio Engine (HIGH PRIORITY)

### Can complete now

| Item | Status | What remains | Concept alignment |
|------|--------|-------------|-------------------|
| **§7.2 Audio mixer** | `[ ]` | In-component mixer with SCHED_BODY priority path | §AthAudio "sub-3ms" |
| **§7.2 Per-app volume + EQ** | `[ ]` | Per-stream volume/EQ in `components/athaudio` | §AthAudio |
| **§7.2 Routing matrix** | `[ ]` | VoiceMeeter-class input→effects→output routing | §"VoiceMeeter-class functionality built in" |
| **§7.2 Loopback monitoring** | `[ ]` | Monitor path in audio engine | §AthAudio |
| **§7.2 Latency measurement** | `[ ]` | Measurement harness for sub-3ms proof | §"Sub-3ms round-trip on certified hardware" |

### Blocked

| Item | Blocker |
|------|---------|
| HDA full init on Athena | Composer (driver HW) + needs real hardware |
| USB Audio Class | Composer (driver) |
| Bluetooth audio | Composer (BT stack) |

---

## Phase 8 — AthUI and AthKit (MEDIUM PRIORITY)

### Can complete now

| Item | Status | What remains | Concept alignment |
|------|--------|-------------|-------------------|
| **§8.1 Skia integration** | `[ ]` | Wire Skia under `gpu_userspace` feature in `components/athui` | §AthUI "Skia + wgpu" |
| **§8.1 wgpu integration** | `[ ]` | Wire wgpu in `components/athui` | §AthUI |
| **§8.1 Glassmorphic surfaces** | `[ ]` | Blur + transparency at compositor level | §"Glassmorphic by default" |
| **§8.1 Live wallpapers** | `[ ]` | GPU-accelerated, paused when occluded | §"Live wallpapers" |
| **§8.1 Window animation curves** | `[ ]` | User-editable animation curves | §"Window animations curve-editable" |
| **§8.2 Declarative widget tree** | `[ ]` | `view!` macro or similar in `components/athkit` | §AthKit "SwiftUI-style" |
| **§8.2 State/binding system** | `[ ]` | Reactive state in `components/athkit` | §AthKit |
| **§8.2 Layout engine** | `[ ]` | Constraint/flexbox layout in `components/athkit` | §AthKit |
| **§8.2 Theming hook** | `[ ]` | Theme engine integration | §"Theme engine at the compositor level" |
| **§8.2 App bundle packager** | `[ ]` | `athkit bundle` tool | §AthKit |
| **§8.2 Hot reload** | `[ ]` | Dev-time hot reload | §AthKit |

---

## Phase 10 — AthNet (MEDIUM-HIGH PRIORITY)

### Can complete now

| Item | Status | What remains | Concept alignment |
|------|--------|-------------|-------------------|
| **§10.2 Real X25519** | `[ ]` | Replace stub with real x25519-dalek (already in deps) | §AthNet WireGuard |
| **§10.2 Real Blake2s** | `[ ]` | Replace SHA-256 placeholder with blake2 (already in deps) | §AthNet WireGuard |
| **§10.2 QUIC implementation** | `[ ]` | QUIC protocol in `components/athnet` | §"QUIC priority" |
| **§10.2 WireGuard daemon** | `[ ]` | `athvpn` userspace daemon in `components/athvpn` | §"Built-in WireGuard" |
| **§10.2 Gaming traffic shaping** | `[ ]` | Prioritize foreground game's traffic | §"Gaming traffic shaping" |
| **§10.2 Firewall rulesets** | `[ ]` | Per-app firewall rules | §AthNet |
| **§10.2 mDNS / DNS-SD** | `[ ]` | LAN discovery | §AthNet |
| **§10.2 IPv6 dual-stack** | `[ ]` | IPv6 support | §AthNet |
| **§10.2 DoT / DoH** | `[ ]` | DNS encryption | §AthNet |

---

## Phase 13 — Customization Engine (LOWER PRIORITY)

### Can complete now (in my crates)

| Item | Status | What remains | Concept alignment |
|------|--------|-------------|-------------------|
| **§13.1 Theme bundles signed + sandboxed** | `[ ]` | Theme bundle signing in `components/athshell` or `athui` | §"Themes ship as small declarative bundles, signed and sandboxed" |
| **§13.1 Vibe Mode presets** | `[ ]` | System-wide visual personalities | §"Vibe Mode" |
| **§13.1 Vibe Mode components** | `[ ]` | Wallpaper + accents + sounds + fonts + cursor + animations | §Vibe Mode |
| **§13.2 Swappable WM API** | `[ ]` | Tile/stack/float/hybrid WM API in `components/athshell` | §"Swappable window managers" |
| **§13.2 Swappable shell** | `[ ]` | Shell replacement API | §"Swappable shells" |
| **§13.2 Widget system** | `[ ]` | Rainmeter-equivalent in `components/athui` | §"Widget system" |

---

## Phase 14 — AthShell (LOWER PRIORITY)

### Can complete now

| Item | Status | What remains | Concept alignment |
|------|--------|-------------|-------------------|
| **§14.1 System tray clock** | `[ ]` | Read `sys_wall_clock` in shell | §AthShell |
| **§14.1 Notifications surface** | `[ ]` | Notification rendering in `components/athshell` | §AthShell |
| **§14.1 Search bar** | `[ ]` | Sub-100ms search UI (kernel index ready) | §"Search is broken → sub-100ms results" |
| **§14.3 GameOS Mode couch UI** | `[ ]` | Large fonts, controller-driven in `components/athshell` | §"GameOS Mode" |
| **§14.3 Library aggregator** | `[ ]` | Unified game library view | §AthPlay |

---

## Items I Should NOT Touch

These are common sources of confusion — they mention my subsystems but live in other slices:

| Item | Why not mine |
|------|-------------|
| HDA controller init / codec / PCM HW | **Composer** (driver hardware) — my slice is the audio *engine* above HW |
| Wi-Fi / iwlwifi daemon | **Composer** (LinuxKPI + driver) |
| NIC drivers (e1000, igc, i219, RTL) | **Composer** (drivers) |
| GPU drivers (amdgpu, i915) | **Composer** (LinuxKPI driver daemons) |
| Kernel-side audio.rs / net.rs / syscall.rs | **Opus** (kernel/) |
| installer / athinstaller | **Composer** |
| AthBridge / Win32 compat | **OWNERLESS** — do not touch |
| Syscall numbers / ath_abi | **Opus** — file NEEDS-INTERFACE notes |

---

## Proposed Execution Order

Ordered by **fan-out** (how many downstream items each unblocks) and **Concept doc priority**:

### Tier 1 — Highest impact, fewest blockers

1. **AthFS snapshot userspace API** (§5.1) — unblocks time-machine UX, installer rollback, update rollback
2. **AthNet real crypto** (§10.2: X25519 + Blake2s) — unblocks WireGuard being cryptographically valid
3. **AthAudio mixer + routing** (§7.2) — unblocks per-app audio, game mode audio, pro audio

### Tier 2 — Core framework

4. **AthFS zstd compression** (§5.4) — Concept says "Zstd by default"
5. **AthKit declarative widget + state** (§8.2) — unblocks all app development
6. **AthGFX API expansion** (§6.2) — unblocks GPU-accelerated rendering path
7. **AthNet WireGuard daemon** (§10.2) — unblocks VPN feature

### Tier 3 — Polish + completeness

8. **AthUI Skia/wgpu integration** (§8.1) — unblocks glassmorphism, live wallpapers
9. **AthShell notifications + search** (§14.1) — user-facing polish
10. **Customization engine** (§13) — theme bundles, Vibe Mode, swappable WM
11. **AthFS per-app bucket encryption** (§5.6) — security hardening
12. **AthNet QUIC + traffic shaping** (§10.2) — gaming network features

---

## Open Questions

> [!IMPORTANT]
> **Which phase should I start with?** The highest-fan-out unblocked items span multiple phases. I'd recommend starting with **Tier 1** (AthFS snapshots → AthNet crypto → AthAudio mixer) since they're all in my crates, have no cross-slice blockers, and each unblocks significant downstream work. But if you want me to focus on a specific phase or component, let me know.

> [!IMPORTANT]
> **NEEDS-INTERFACE items:** Several AthFS and AthNet features need new syscall numbers from Opus. Should I file those notes in the MasterChecklist now so Opus can land them, or should I work around them initially with in-component APIs?

> [!WARNING]
> **Skia + wgpu integration (§8.1)** requires the GPU userspace path to be functional. The `gpu_userspace` feature gate exists but the actual GPU driver path (Composer's amdgpu/i915 daemons + Opus's virtio-gpu kernel support) isn't live yet. I can build the *framework* side but can't prove it end-to-end until those land. Is building the framework now (with software fallback) acceptable?

## Verification Plan

### Automated Tests
- Each component has its own `cargo test` suite
- `cargo run -p xtask -- build --release` must still pass after changes
- Boot smoketests: any new `run_boot_smoketest()` function must emit a PASS/FAIL line

### Manual Verification
- QEMU boot: `target/boot.ps1` must reach `[ OS ] System successfully booted.` with no regressions
- New `/proc/athena/*` endpoints for any new subsystem features
- Athena hardware verification deferred to when hardware is available
