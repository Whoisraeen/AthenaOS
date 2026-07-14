# RaeenOS Master Build Checklist

Status key: `[x]` done, `[~]` partial/structural, `[ ]` not started

---

## Phase 0: Real Hardware I/O (Critical Path)

These block everything — without them the OS only runs in QEMU.

- [x] **NVMe driver** — DMA via GlobalFrameAllocator, no hardcoded addresses
- [x] **AHCI driver** — DMA via GlobalFrameAllocator for command lists, FIS, command tables
- [x] **Block device trait** — `BlockDevice` trait in block_io.rs, RaeFS wired through it
- [x] **MSI/MSI-X interrupt support** — pci.rs capability parsing + msi.rs enable API + dynamic dispatch
- [x] **PCIe ECAM** — MCFG ACPI parsing, ECAM MMIO + legacy fallback, extended caps (AER, SR-IOV, LTR)
- [x] **xHCI USB host controller** — real MMIO volatile reads/writes throughout
- [x] **e1000 NIC** — PCI probe wiring, BAR0 MMIO, RX/TX ring setup, smoltcp Device
- [x] **IOMMU (VT-d/AMD-Vi)** — DMAR parsing, VT-d registers, root/context tables, DMA domains, IOTLB invalidation, cap-gated
- [x] **Un-quarantine init() calls** — 8-tier boot ordering, all modules wired

## Phase 1: Year-1 Completion (Kernel + RaeFS + RaeGFX + RaeUI)

### RaeFS — CoW Filesystem
- [x] Basic mount/format/read/write (on virtio-blk)
- [x] Copy-on-Write journal
- [x] Snapshot mechanism (instant rollback)
- [x] Atomic system updates (CoW journal)
- [x] Native encryption (FDE, TPM 2.0 key sealing)
- [x] Tiered storage (NVMe/SATA hot-data promotion)
- [x] Game-aware extents (large sequential-read optimization)
- [x] Zstd transparent compression
- [x] Per-app data buckets (FS-layer isolation)
- [x] Versioned config files

### RaeGFX — Native Graphics API
- [x] Software rasterizer (Canvas, draw_triangle, barycentric)
- [x] Vulkan-equivalent command submission API
- [x] GPU memory management (VRAM allocator, PCI discovery, BAR mapping, command ring, VirtIO-GPU, display scanout)
- [x] Shader compiler (SPIR-V or custom IR)
- [x] HDR output pipeline
- [x] VRR (Variable Refresh Rate) frame pacing (compositor-level, adaptive/fixed)
- [x] Direct-to-GPU exclusive fullscreen (page flip, VRR controller, LFC, mode switch, hotplug recovery)
- [x] OS-level shader cache (persistent across reinstalls)

### RaeUI — Native UI Framework
- [x] Label + Button + Frame widgets
- [x] Theme palette (hardcoded)
- [x] Layout engine (flexbox/grid — taffy or custom)
- [x] Text rendering (font rasterizer, glyph cache)
- [x] Theme engine at compositor level (Vibe Mode, 12 presets, compositor-integrated)
- [x] Animation engine (curve-editable)
- [x] Accessibility tree
- [x] Data binding
- [x] Rich text editor widget
- [x] i18n / localization

### Compositor
- [x] Multi-window surface management
- [x] Z-ordered compositing
- [x] VRR-aware frame pacing
- [x] HDR tone mapping
- [x] Glassmorphism / blur effects
- [x] Live wallpapers (GPU-accelerated, paused when occluded)
- [x] Zero-cost screen capture at compositor level

## Phase 2: Year-2 Desktop Experience

### RaeShell — Desktop Shell
- [x] Window manager (wired to compositor via desktop.rs)
- [x] Taskbar (renders on own surface, window buttons, clock)
- [x] Start menu (toggle, app list, click-to-launch)
- [x] File manager (two-panel, navigation, file ops)
- [x] Terminal emulator (VT100, ANSI colors, character grid)
- [x] System tray (battery, network, volume icons + popups)
- [x] Notification daemon (toast rendering, notification center)
- [x] Screenshot / screen recorder (compositor zero-cost capture)
- [x] Calculator (expression parsing, button grid, history)
- [x] Image viewer (BMP/TGA/PPM/QOI decoders, zoom, pan, slideshow, bilinear scaling)
- [x] Media player (WAV/PCM decoder, waveform display, playlist, volume, transport controls)
- [x] Settings app (unified, 5 categories, searchable)
- [x] File dialog (system-wide, open/save/folder, breadcrumbs, preview, filters)
- [x] Lock screen (secure overlay, blur, PIN/passkey, idle/lid/hotkey triggers, multi-monitor)
- [x] Search (local-first, inverted index, TF-IDF, fuzzy Levenshtein, sub-100ms, filesystem watcher)

### RaeBridge — Windows App Compatibility
- [x] Win32 type definitions (HANDLE, BOOL, DWORD)
- [x] 30 DLL shim modules declared (kernel32, user32, gdi32, etc.)
- [x] kernel32 API (CreateFile A/W, ReadFile, WriteFile, CreateProcess, VirtualAlloc, HeapAlloc, TLS, etc.)
- [x] user32 API (CreateWindow, DefWindowProc, message loop, input, timer, dialogs, A/W variants)
- [x] gdi32 API (DC, bitmap, text output, shapes, regions, clipping, text metrics, A/W variants)
- [x] ntdll API (NtCreateFile, NtQueryInformationFile, NtQueryInformationProcess, Rtl*, heap, registry)
- [x] PE loader (parse PE32/PE32+, section mapping, base relocations, import resolution, DllRegistry)
- [x] Win32 threading model (CreateThread, TLS, critical sections)
- [x] Error translation (RaeenOS → Win32 error codes)
- [x] DirectX 11/12 -> RaeGFX translation layer (D3D11/12 state mapping, DXGI, DXBC parsing, compat DB)
- [ ] Steam client compatibility test

### RaeNet — Networking Stack
- [x] virtio-net L2 driver
- [x] smoltcp L3/L4 stack (wired, poll_full integration)
- [x] DHCP client (full state machine, auto-configure, renewal)
- [x] DNS resolver (cache, TTL, negative caching, static hosts, multi-server failover)
- [x] WireGuard VPN (built-in, Noise IK handshake, ChaCha20-Poly1305 transport)
- [x] QUIC protocol support (connection mgmt, stream mux, loss detection, flow control)
- [x] Gaming traffic shaping (strict-priority queue: Game/Interactive/Bulk)
- [x] Firewall (capability-gated, per-app, conntrack, rate limiting, 3 profiles)
- [x] e1000 NIC PCI probe wiring

### RaeAudio — Low-Latency Audio
- [x] HDA register map + codec structures
- [x] Real MMIO I/O to HDA controller (GCAP, GCTL, CORB/RIRB, stream descriptors)
- [x] Lockless ring buffer (SPSC, AtomicUsize, 4096 samples)
- [x] SCHED_GAME audio thread (128 frames @ 48kHz = 2.67ms)
- [x] Mixer (N-stream additive mix, f32→i16, master gain)
- [x] Audio routing (VoiceMeeter-class, built-in)
- [x] Capture / recording
- [x] Spatial audio (HRTF, ITD/ILD, 72-direction table)
- [x] Audio effects chain (EQ, compressor, limiter, gate, reverb, delay)
- [x] Audio graph (node-based processing, topological sort)
- [x] Device manager (hot-plug, per-app device routing)

### SCHED_GAME — Real-Time Scheduling
- [x] Game priority queue (preempts Normal)
- [x] Hard deadline enforcement (not just priority — actual deadline miss detection)
- [x] Compositor thread on SCHED_GAME
- [x] Audio engine thread on SCHED_GAME
- [x] Background process throttling (when in-game, nothing else gets meaningful CPU)
- [x] Per-game CPU affinity / core pinning

## Phase 3: Gaming Features

### RaeShield — Security + Anti-Cheat
- [x] Capability system (14 flavors, derivation, grant/revoke)
- [x] Secure boot chain (6-stage measured boot into TPM PCRs)
- [x] Attestation service (TPM2_Quote, HMAC-SHA256 signatures, W^X check)
- [x] TPM 2.0 integration (CRB MMIO driver, software fallback, PCR extend/read/quote)
- [x] Measured boot output for EAC/BattlEye (signed attestation blob with PCR values)
- [x] W^X enforcement (page table scanner, flag helpers, violation counting)
- [~] Memory tagging on supported CPUs (interface defined, stub on x86_64)
- [x] IOMMU driver sandboxing (mandatory for all drivers)

### Gaming Experience
- [x] GameOS Mode (couch UI, carousel, library grid, detail view, controller nav)
- [x] Per-game profiles (display, GPU, audio, input, scheduler, compat)
- [x] Game Bar overlay (FPS/frametime/temps, quick actions, compact+full modes)
- [x] DualSense full support (haptics, adaptive triggers, gyro, touchpad, LED)
- [x] Xbox controller full support (impulse triggers, 4-motor rumble)
- [x] NULL_LATENCY mode (disable all smoothing/queuing)
- [x] Memory pinning API (pin/unpin, 50% RAM limit, cap-gated)
- [x] RaePlay launcher (Steam/Epic/GOG/RaeStore, VDF/JSON parsers, per-game profiles, playtime, achievements)

## Phase 4: Polish + Ecosystem

### RaeKit — App Development SDK
- [x] Declarative UI API (ViewNode 24 variants, 15 builders, SwiftUI-style chaining)
- [x] App lifecycle management (RaeApp trait, AppRunner, event loop)
- [x] Sandboxed app model (capability-gated syscall wrappers)
- [x] App bundle format (via RaeStore package manifest)
- [x] State management (State<T>, Binding<T>, ObservableObject)
- [x] Navigation (NavigationStack, TabView, Router with URL-style history)

### Customization Engine
- [x] Vibe Mode (system-wide visual personalities — 12 presets, user profiles, time-based auto-switch, smooth transitions)
- [x] Window animation curve editor (cubic bezier, 7 presets, per-action assignment, animation manager)
- [x] Swappable window managers (TilingWm master/stack, FloatingWm snap zones/resize, HybridWm tile groups, per-workspace mode, hotkeys)
- [x] Swappable shells (ShellRegistry, switch_shell, IPC protocol, RaeShell + GameOS built-in)
- [x] Widget system (Rainmeter-style, sandboxed — Clock, Weather, SystemMonitor, NowPlaying, Calendar, QuickNotes, drag placement, bundles)
- [x] RGB unified API (RgbDevice trait, 10 effects, RgbManager discovery, profiles, game integration)
- [x] Fan curve / power management at OS level (FanCurve with hysteresis, 4 PowerProfiles, AC/battery auto-switch, emergency throttle)

### RaeStore / RaeID / RaeSync
- [x] App store (package format, dependency resolver, repository, sandboxed install, delta updates, 12% revenue)
- [x] Account system (passkey auth, session management, multi-user, guest mode, audit log)
- [x] Cross-device sync (E2E encrypted, X25519+ChaCha20, conflict resolution, device trust, 8 sync types)

---

## Line Count Targets

| Phase | Target Lines | Current |
|-------|-------------|---------|
| Phase 0 (HW I/O) | +20K | ~5K structural |
| Phase 1 (Year-1) | +80K | ~3K functional |
| Phase 2 (Year-2) | +300K | ~150K structural |
| Phase 3 (Gaming) | +200K | ~5K structural |
| Phase 4 (Polish) | +400K | 0 |
| **Total target** | **~1M+ quality lines** | **~247K** |

---

## Phase 5: Deep Systems (Wave 9+)

### POSIX / Linux Compatibility
- [x] POSIX syscall layer (file I/O, fork/exec/wait, signals, mmap, sockets)
- [x] Linux x86_64 syscall translation (dispatch table, clone, futex, epoll, prctl)
- [x] ELF loader Linux detection (OSABI, aux vector, vDSO, stack layout)
- [x] Dynamic linker (ld-linux.so equivalent, lazy binding, NEEDED resolution)
- [x] /proc + /sys full Linux-compatible virtual filesystems
- [x] tmpfs in-memory filesystem (mounted at /tmp and /dev/shm)
- [ ] PWA support (web apps rendered through RaeUI)

### RaeBridge Deep
- [x] advapi32 (registry, security, crypto, services)
- [x] ole32 / COM (CoInitialize, IUnknown, CLSIDFromString, CoTaskMemAlloc)
- [x] ws2_32 / Winsock (socket, send/recv, WSA*, getaddrinfo)
- [x] Registry hive emulator (tree-based, Windows 10 defaults)
- [x] Shell32 (SHGetFolderPath, SHFileOperation, ShellExecute, drag-drop)
- [x] XInput / DirectInput (gamepad for Win32 games — xinput_deep + dinput)
- [x] winmm (multimedia timers, waveOut, MIDI, joystick)
- [x] version.dll (GetFileVersionInfo)

### RaeShield Deep
- [x] Sandbox policy engine (builder pattern, 5 profiles, enforcer)
- [x] Code signing (Ed25519/RSA, cert chain, CRL)
- [x] Mandatory Access Control (labels, policies, dominance hierarchy)
- [x] Security audit log (ring buffer, 19 event types, alert rules)
- [x] Process attestation API (nonce-based, EAC/BattlEye format)
- [x] Runtime permission prompts (kernel request queue, approve/deny API, timeout, UI-ready)
- [x] Secure IPC (authenticated channels with capability checks)

### Kernel Deep
- [x] SMP (multi-core scheduler, per-CPU run queues, load balancing, per-CPU syscall stacks via SWAPGS)
- [x] FPU/SSE/AVX state save/restore on context switch (fxsave64/fxrstor64)
- [x] NUMA-aware memory allocation (topology, policies, per-node allocator)
- [x] Kernel module hot-loading (dynamic driver load/unload)
- [x] Watchdog timer (hardware + software)
- [x] Kernel crash dump (panic → save state → reboot)
- [x] Fast boot path optimization (parallel init, deferred driver probing)
- [x] Overclocking API (CPU/GPU frequency, voltage, OS-level)
