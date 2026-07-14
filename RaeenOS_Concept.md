# RaeenOS
## The OS Manifesto

> **AthenaOS note:** This file is the **historical RaeenOS** gaming-first thesis, kept for lineage.
> The design bible for *this* repository is [`Athena_Concept.md`](Athena_Concept.md).
> Gaming product goals described below are **parked** in Athena v0. AthenaOS is an **independent** GitHub repo, not a fork of RaeenOS.

**Thesis:** Windows became bloated chasing enterprise. macOS got locked behind a walled garden of Apple silicon. Linux never figured out gaming or design coherence. RaeenOS is the third path — a from-scratch, gaming-first, native-feeling OS that treats power users like adults, gamers like priorities, and creative work like the point.

---

## Core Principles

1. **Native everywhere.** No Electron tax. No web wrappers. Native rendering, native input, native audio — sub-frame latency end to end.
2. **The user owns the machine.** No forced updates. No telemetry without explicit opt-in. No ads in the OS. No bundled junk. Ever.
3. **Fast is a feature.** Boot under 6 seconds on NVMe (target: 3s). Wake under 1 second. Window animations at the display's refresh rate. Input latency measured in milliseconds, not frames.
4. **Security by default, not by friction.** Capability-based sandboxing that's invisible to users and predictable to developers.
5. **Customization is a first-class citizen.** Not a registry hack — a designed extensibility surface.
6. **Gaming isn't a mode. It's the default.**

---

## Architecture

### Kernel: Hybrid (RaeKernel)

Pure microkernels (seL4, Zircon) win on isolation but pay for it in IPC latency. Pure monolithic kernels (Linux) win on raw speed but expand the attack surface every time you add a driver. Neither is right for a gaming-first OS that also wants to be secure.

**RaeKernel is hybrid** in the XNU lineage but modernized:

- **In-kernel:** Scheduler, memory management, IPC, graphics fast path, audio fast path, input subsystem, networking L3 and below. Hot path code that touches every frame.
- **User-space:** Filesystems (except RaeFS root), drivers (IOMMU-sandboxed), networking protocols above L3, audio mixing, USB stack. Anything that can fail without taking the system down.
- **Driver isolation:** Every driver runs in its own protection domain with IOMMU enforcement. A bad GPU driver crashes a service, not the kernel. (Take the lessons from Apple's DriverKit and Microsoft's UMDF, but make it mandatory.)
- **Real-time scheduling class:** A `SCHED_GAME` priority above `SCHED_FIFO` with explicit hard deadlines, used by the compositor, audio engine, and game-claimed threads.

### Architecture Reach: runs anywhere, locked to no silicon

macOS got locked behind a walled garden of Apple silicon. RaeenOS refuses ISA lock-in: the kernel sits on a clean `arch::` abstraction layer (boot, MMU, interrupts, timers, SMP, context switch, syscall entry, firmware discovery) so the same OS boots **x86_64, aarch64 (ARM 64-bit), and i686 (32-bit x86)** — each proven independently. The scheduler, memory manager, IPC, VFS, RaeFS, and the entire Rae* userspace stack are arch-neutral; only the thin per-arch seam differs. The user/syscall ABI is arch-neutral by design. Bring-up order: x86_64 (done) → aarch64 → i686. *Portability is the anti-walled-garden property — you own the machine, on the silicon you choose.*

### Language Stack

| Layer | Language | Why |
|---|---|---|
| Boot + low-level kernel core | Rust + minimal Zig for boot stub | Memory safety where bugs are catastrophic |
| Kernel modules & drivers | Rust | No GC, no UB, full control |
| System services & daemons | Rust | Same |
| App frameworks (RaeUI, RaeKit) | Rust on Skia + wgpu | Memory safety + battle-tested rendering primitives |
| App development | Rust (primary), C/C++, Zig | Declarative RaeKit API; SwiftUI-style ergonomics without the Apple lock-in |
| Compatibility shims (RaeBridge) | Rust + C | Wine/Proton lineage, isolated |

**One language top to bottom.** Rust in the kernel, Rust in services, Rust in the UI framework, Rust as the recommended app language. Kernel devs and app devs speak the same language — genuinely rare, and a real recruiting advantage.

RaeUI doesn't write a renderer from scratch. It sits on **Skia** for 2D (the same library powering Chrome, Flutter, and Firefox) and **wgpu** for 3D and GPU effects (Vulkan/DX12/Metal abstraction). Skia handles text, vectors, and the bread-and-butter widget rendering; wgpu handles compositor effects, glassmorphism, live wallpapers, and shader-driven theming. This is the AppKit-on-Core-Graphics pattern: the proprietary surface is RaeUI, the libraries underneath are implementation detail.

What this buys: a decade of Google and Mozilla rendering work for free, and roughly 5% of the engineering cost of a from-scratch Vulkan renderer at equivalent capability.

### Language Stack — extended

Rust is the spine. The additions below earn their place only where Rust serves the goal poorly, and each is contained to a boundary (FFI, app-layer, or sandbox) so the "one language" coherence holds:

- **raelang** — the automation/scripting layer (§Customization). A small, Swift-flavored, sandboxed (fuel-limited, kill-able) language implemented in Rust, one interpreter shared by the kernel and a userspace daemon. This *is* the "Swift scripts" promise — realized without dragging the Swift runtime into the system.
- **WGSL → SPIR-V** — the shader language for the theme engine and compositor effects (glassmorphism, live wallpapers, Vibe Mode). Authored in WGSL, compiled to SPIR-V for the RaeGFX submit path. This is how the visual identity is *expressed*, not implementation detail.
- **WebAssembly** — the sandboxed substrate for third-party extensions, widgets, and untrusted store apps: "any language in, one capability-gated runtime." The anti-Electron — no DOM, no JS engine, near-native. Scoped to extensions/untrusted apps; the hot path and games stay native.
- **Zig** — the boot stub, plus `zig cc` as the hermetic C/C++ cross-compiler that builds the unavoidable C/C++ dependencies (Mesa, Wine/DXVK/VKD3D, Skia, LinuxKPI glue) reproducibly.
- **C/C++** — imported strictly behind FFI for those same dependencies; never in the kernel proper.
- **Swift** — a first-class *app-development* target (distinct from raelang scripting) so porting a macOS app is a week, not a quarter.

**Rejected as system or kernel languages:** JavaScript/TypeScript (the Electron tax this OS exists to avoid — the web runs via the browser/PWA path, never as an OS component) and Python/Go/C#/.NET/JVM (GC + heavy runtimes fight "fast is a feature" and inflate attack surface — they run as ported apps, not as part of the stack).

### File System: RaeFS

A custom copy-on-write filesystem optimized for the access patterns of a gaming desktop:

- **CoW with snapshots** — instant rollback, time-machine-style backups, atomic system updates that never half-apply
- **Native encryption** — full-disk, hardware-backed where available (TPM 2.0 minimum on certified hardware)
- **Tiered storage** — automatic hot-data promotion between NVMe / SATA / spinning rust, transparent to apps
- **Game-aware extents** — large sequential-read optimization for game assets, with a "game install" hint that pre-allocates contiguous blocks
- **Compression** — Zstd by default, transparent, fast enough to be a net win on most workloads
- **Per-app data buckets** — apps see their own data only, system enforces isolation at the FS layer
- **Versioned config** — every system config file is automatically versioned. Bricked your config? Roll back one click.

---

## The Proprietary Stack

Every layer is owned, controlled, and optimized as a unit. This is what makes Apple's stack feel coherent and Windows' stack feel like a museum of acquisitions.

| Component | What it is |
|---|---|
| **RaeKernel** | Hybrid kernel, Rust |
| **RaeFS** | CoW filesystem with snapshots and tiered storage |
| **RaeGFX** | Native graphics API. Vulkan-equivalent capabilities, friendlier surface, first-class HDR/VRR. |
| **RaeAudio** | Low-latency audio engine. Sub-3ms round-trip on certified hardware. No ASIO mess, no PulseAudio mess. |
| **RaeUI** | Native UI framework, Rust on Skia + wgpu. Compositor-aware, GPU-accelerated, glassmorphic by default. |
| **RaeKit** | App development SDK — Rust-first, declarative (SwiftUI-style ergonomics, no GC) |
| **RaeShield** | Security framework — capabilities, sandboxing, attestation |
| **RaeNet** | Userspace networking above L3, with built-in WireGuard, QUIC priority, gaming traffic shaping |
| **RaeStore** | App store — 12% revenue share, sideloading allowed, no review hostage situations |
| **RaeID** | Account system — passkeys first, optional, never required for local use |
| **RaeSync** | Optional cross-device sync, end-to-end encrypted |
| **RaeBridge** | Windows app compatibility layer (Wine/Proton heritage, tightly integrated) |
| **RaePlay** | Built-in game launcher / library aggregator (Steam, Epic, GOG, RaeStore unified) |

---

## Gaming-First Design

This is where RaeenOS wins or doesn't ship.

### Performance
- **`SCHED_GAME` priority class** — the foreground game's main + render threads get hard real-time guarantees
- **Direct-to-GPU paths** — RaeGFX skips the compositor entirely in exclusive fullscreen, zero overhead
- **Compositor-level VRR + HDR** — handled at the OS layer, every game gets it free
- **Shader cache** at the OS level, shared across Vulkan/RaeGFX, persistent across reinstalls
- **Memory pinning API** — games can request guaranteed-resident pages for hot data
- **Background process throttling** — when you're in-game, nothing else gets meaningful CPU. Windows still hasn't figured this out.

### Compatibility
- **Native ports** — Vulkan and RaeGFX both first-class. RaeGFX has a "looks like Metal, performs like Vulkan" feel.
- **DirectX 11/12 → RaeGFX translation** at the driver level (DXVK/VKD3D-Proton lineage, but integrated and signed)
- **Steam works day one** via RaeBridge — non-negotiable; without Steam there is no PC gaming OS
- **Easy Anti-Cheat / BattlEye** support — partnerships day one. This is the actual hard problem. Solve it or die. (See RaeShield approach below.)

### Features
- **GameOS Mode** — couch UI, big-picture, controller-first. Toggle into it instantly. Same OS, different shell.
- **Capture & stream** at the compositor — zero-cost recording, no OBS overhead
- **Per-game profiles** — resolution, refresh rate, audio device, GPU power limit, all configured per game and auto-applied
- **Game Bar that doesn't suck** — overlay with FPS, frametime graph, CPU/GPU temps, voice chat, screenshots. All native, all fast.
- **DualSense + Xbox + every controller** with full feature parity (haptics, adaptive triggers, gyro)

### Pro Gaming
- **NULL_LATENCY mode** — disables every optional smoothing, every queued frame, every nice-to-have. Pure direct-input pipeline. For competitive players.
- **Audio routing native** — VoiceMeeter-class functionality built in, properly

---

## Security Model

iOS-grade security without iOS-grade lockdown.

- **Capability-based permissions** — apps request capabilities (file access, camera, mic, network), user grants, OS enforces at the syscall layer
- **Mandatory app sandboxing** — every app runs in its own sandbox by default. "Trusted app" mode for legacy software, clearly marked.
- **Code signing** — required for app store distribution, **optional** for sideloaded apps (with clear "unverified developer" UX, not punitive)
- **Secure boot chain** — bootloader → kernel → init → compositor, every step verified
- **Hardware-backed encryption** on certified hardware (TPM 2.0 minimum)
- **Memory tagging** on supported CPUs (ARMv8.5 MTE, Intel/AMD equivalent as it ships)
- **Driver sandboxing** — IOMMU-enforced, no exceptions
- **No kernel-level anti-cheat needed** — RaeShield exposes an attestation API that anti-cheat vendors can use without owning ring 0. The pitch to EAC/BattlEye: "you don't need kernel access on our OS; here's a better, harder-to-bypass primitive."

What this gets you: a system that resists ransomware structurally (apps can't touch other apps' data without explicit permission), where malware infections are bounded, and where you can run untrusted software without fear.

---

## Customization Engine

This is the killer feature for the custom-PC crowd.

### Visual
- **Theme engine at the compositor level** — themes change the actual rendering, not just colors. Frosted glass, holographic, CRT scanlines, neo-noir, brutalist, whatever. Themes ship as small declarative bundles, signed and sandboxed.
- **Vibe Mode** — system-wide visual personalities. "Cyberpunk Night," "Studio Ghibli Morning," "Bauhaus." Changes wallpaper, accent colors, sound design, system font, cursor, even window animations as a coherent set.
- **Live wallpapers** that don't murder battery — GPU-accelerated, paused when occluded
- **Window animations curve-editable** by users who care

### Functional
- **Swappable window managers** — tile (i3-style), stack (macOS-style), float, hybrid. First-class API, not a hack.
- **Swappable shells** — the default RaeShell can be replaced with a competing one. The OS doesn't care.
- **Widget system** — Rainmeter-style but actually fast and sandboxed
- **Scripting layer** — **raelang**, a sandboxed Swift-flavored scripting language for automation (no PowerShell archaeology required). See §Language Stack — extended

### Hardware
- **RGB unified** — every motherboard, every fan, every keyboard, one API, one config. RGB hell is a Windows problem; RaeenOS solves it.
- **Per-device profiles** — keyboard, mouse, controller settings stored in OS, synced across machines
- **Fan curve / power management** at the OS layer, not in a sprawl of vendor utilities

---

## Windows Pain Points → RaeenOS Solutions

| Windows annoyance | RaeenOS answer |
|---|---|
| Forced restarts for updates | User controls update timing. Ever. Always. |
| Update breaks your machine | Atomic CoW updates with one-click rollback |
| Bloatware (Candy Crush, etc.) | Zero. Default install is OS only. |
| Ads in Start menu, Explorer, lock screen | Forbidden by design. Burned into the EULA. |
| Telemetry you can't fully disable | Off by default. Opt-in with clear disclosure. |
| Registry is a graveyard | Versioned, hierarchical, human-readable config with snapshots |
| DLL hell | App bundles with explicit, hashed dependencies |
| Settings vs. Control Panel split | Single unified Settings, every option discoverable via search |
| Search is broken | Local-first, indexed, sub-100ms results |
| Random reboots | Never without explicit consent |
| Slow boot | <6s on NVMe, target 3s |
| Driver Wild West | All drivers signed and IOMMU-sandboxed |
| Pushed Copilot / Cortana / Edge | AI is optional, off by default, fully removable |
| File Explorer is from 2007 | Modern file manager: tabs, split panes, fuzzy search, batch rename — that exists in 2026 |
| WSL is a great idea wrapped in friction | Linux subsystem first-class, but most things are native anyway |

---

## Three User Experiences, One OS

### Average User
- RaeShell default — familiar enough to switch from Windows or Mac in 10 minutes
- App store has what they need, sideload available if they want it
- Updates are quiet, opt-in for major versions
- It just works.

### Custom PC Builder
- Hardware diagnostics built in — temps, voltages, fan curves, RGB
- Overclocking utilities at the OS level, no MSI Afterburner / Armoury Crate / iCUE sprawl
- Per-component driver management, clean and unified
- Theme engine + swappable window manager + tweakable everything

### Game Station
- Boot directly into GameOS Mode if configured
- Couch UI, controller-driven, optimized for living-room use
- Same OS, same library, same saves as the desktop experience
- Competes with SteamOS on its home turf with a better app ecosystem

---

## Compatibility Strategy (a.k.a. How To Actually Win)

The hardest problem isn't the OS — it's getting people to switch. Steam Deck succeeded because it didn't ask users to switch consciously. The plan:

1. **RaeBridge runs Windows apps on day one.** Wine + Proton heritage, tightly integrated. Not a "subsystem" — apps run naturally.
2. **Native Linux app support** via POSIX layer. Get every Linux dev for free.
3. **Web apps via PWA support** that actually feels native (renders through RaeUI).
4. **Developer onramp** — RaeKit is declarative and ergonomic enough that porting a Mac or Windows app is a week, not a quarter. Free tools, free signing for the first year, world-class docs. The pitch to skeptical devs: "your app gets memory safety and a real sandbox model for free."
5. **Hardware partnerships** — RaeReady certification program. OEM-shipped on gaming laptops and prebuilts that hit a quality bar.
6. **First-party Steam Deck competitor — the Rae Station.** Sells the OS to the gaming crowd first.

---

## Distribution & Business Model

- **Free for personal use**, period
- **RaeenOS Pro** — power-user features, advanced theming, dev tools — $30 one-time or $5/mo
- **RaeenOS Studio** — for creators, includes pro audio/video tooling
- **Enterprise licensing** — for business
- **App store** — 12% revenue share, sideloading allowed and supported
- **Hardware certification** — OEMs pay for "RaeReady" badge
- **No ads. No data sales. Ever.** Burned into the EULA.

---

## 5-Year Roadmap

- **Year 1:** Kernel + RaeFS + RaeGFX + RaeUI hello world. Boots, draws, plays a single Vulkan demo. Dev community formed.
- **Year 2:** Full desktop experience. RaeBridge runs 80% of Windows apps. Steam works. Limited public alpha.
- **Year 3:** Public beta. GameOS Mode ships. First RaeReady hardware. Top 100 Windows games at native parity.
- **Year 4:** 1.0. First Rae Station hardware. EAC/BattlEye partnerships closed. Developer ecosystem at critical mass.
- **Year 5:** Mass-market push. OEM design wins. The "third desktop OS" is no longer Linux.

---

## What Makes This Win

Windows has institutional inertia and a captive enterprise audience. Apple has total vertical integration and the world's best industrial design team. Linux has community and freedom.

RaeenOS wins by being the first OS that treats **gamers**, **creative pros**, and **power users** as the *primary* audience — not the residual one. By treating customization not as a hack but as architecture. By being natively fast in a world that gave up on native software. By respecting the user without locking the platform.

It wins because it's the OS the people building it would actually want to use.

That's the only way it ever ships.

---

*"Built for people who care about how things feel."*
