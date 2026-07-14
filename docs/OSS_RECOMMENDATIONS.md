# Open-Source Recommendations — Evaluation vs RaeenOS Concept Doc

Evaluated against `RaeenOS_Concept.md` and existing tree state. Updated 2026-06-16 (browser · Win32/DirectX · accessibility · native-GPU · bootloader sections appended at the end).

---

## Status Legend
- ✅ **Already incorporated** — in tree and working
- ➕ **Add now** — strong alignment, actionable today
- 📖 **Reference only** — use architecture/patterns, not code
- ❌ **Do not use** — misaligns with Concept doc or licensing

---

## ✅ Already Incorporated (don't add again)

| Project | What we use | Where |
|---------|-------------|-------|
| Phil Opp "Writing an OS in Rust" | bootloader_api, GDT/IDT, paging, heap | Entire kernel foundation |
| RedoxFS | CoW FS design, extent system, snapshot model | `redox_reference/`, `components/raefs/vendor/redoxfs/` |
| smoltcp | L2/L3 TCP/IP stack | `kernel/Cargo.toml`, `net_drivers.rs`, `virtio_net.rs` |
| wgpu | 3D GPU effects in compositor (behind `gpu_userspace` feature) | `components/raegfx/`, `components/raeui/` |
| Skia (skia-safe) | 2D rendering, text, vectors | `components/raeui/` (behind `gpu_userspace`) |

---

## ➕ Add Now — Audited Crypto Crates

### Problem
`kernel/src/crypto.rs` has home-grown X25519 (Montgomery ladder) and Blake2s implementations.
Both algorithms are correct in structure but are **unaudited** and untested against known vectors.
For WireGuard (`wireguard.rs`) to be cryptographically valid, we need battle-tested crypto.

### Solution: Replace with audited no_std crates

| Crate | Replaces | License | no_std? |
|-------|----------|---------|---------|
| `x25519-dalek` | `X25519Context::scalarmult` | BSD-3 | ✅ with `default-features = false` |
| `blake2` | `Blake2s256Context` | MIT/Apache-2 | ✅ with `default-features = false` |
| `chacha20poly1305` | Any ChaCha20-Poly1305 stub | MIT/Apache-2 | ✅ with `default-features = false` |

**Cargo.toml additions:**
```toml
x25519-dalek     = { version = "2", default-features = false, features = ["static_secrets"] }
blake2           = { version = "0.10", default-features = false }
chacha20poly1305 = { version = "0.10", default-features = false }
```

**What changes:**
- `crypto.rs`: `X25519Context::generate_public_key` and `compute_shared_secret` delegate to `x25519_dalek::StaticSecret`
- `crypto.rs`: `Blake2s256Context` replaced with `blake2::Blake2s256` wrapper
- `wireguard.rs`: unchanged (same function signatures)
- All WireGuard Noise IK handshake logic is preserved; only the math primitives swap

**MasterChecklist items this closes:**
- `[ ] Real X25519 (currently stub)` → [x] (audited dalek implementation)
- `[ ] Real Blake2s (currently SHA-256 placeholder)` → [x] (blake2 crate)

---

## ➕ Add Now — zstd for RaeFS compression

### Problem
RaeFS currently uses a custom LZ4-style compressor (Phase 5.4). The Concept doc specifies
**Zstd by default**. The `ruzstd` crate is a pure-Rust Zstd decompressor that works in `no_std`.

### Solution
```toml
# Decompression (read path) — pure Rust, no_std compatible
ruzstd = { version = "0.7", default-features = false }
```

For the write path (compression), use `zstd-safe` in a userspace daemon (it wraps libzstd which
needs std). The kernel handles decompress; a userspace compression daemon handles compress.

**Alternative:** `lzzzz` crate provides LZ4 compression + `zstd` via bindings. For now,
the read path is the critical one (loading compressed files from RaeFS).

---

## ➕ Reference Architecture — boringtun (Cloudflare WireGuard)

**Concept alignment:** `RaeenOS_Concept.md §RaeNet`: "Built-in WireGuard, QUIC priority, gaming traffic shaping."

**What boringtun provides:** Production WireGuard with peer management, keepalives, handshake
rekeying, and transport encryption. Uses `x25519-dalek` + `blake2` internally (same crates above).

**What to do:** Don't use boringtun as a dependency (requires `std`, too heavy for kernel).
Instead, use it as the **reference implementation** when extending `wireguard.rs` beyond the
current Noise IK phase to add: peer table, keepalive timer, handshake re-initiation at 180s,
transport packet format. boringtun's `src/noise/` directory is the exact spec to follow.

**GitHub:** `cloudflare/boringtun` — Apache 2.0 licensed (safe to reference).

---

## 📖 Reference Architecture — Iced (reactive GUI)

**Concept alignment:** `RaeenOS_Concept.md §RaeKit`: "SwiftUI-style ergonomics, no GC."

Iced uses the **Elm architecture**: unidirectional data flow — `State → view() → Message → update(state)`.
This is exactly the right model for RaeKit. However, Iced uses its own renderer, not Skia+wgpu.

**What to study in Iced:**
- `iced_core/src/application.rs` — the `update()` / `view()` contract
- `iced_core/src/widget.rs` — trait surface for widgets
- `iced_core/src/layout.rs` — constraint-based layout engine
- `iced_style/src/` — theming model

**What NOT to do:** Don't add Iced as a dependency. RaeKit must sit on our Skia+wgpu stack.
The API ergonomics of `iced` are what to replicate; the rendering pipeline is our own.

**Alternative to study:** Google's **Xilem** (Apache 2.0) — even closer to SwiftUI, uses wgpu.
More actively developed, better aligned with our stack. https://github.com/linebender/xilem

---

## 📖 Reference Architecture — Slint (declarative UI)

**Concept alignment:** `RaeenOS_Concept.md §RaeUI`: sits on Skia + wgpu.
Slint supports Skia and wgpu backends and has `.slint` declarative syntax.

**Licensing issue:** Slint is **GPL v3** for open-source use, commercial license required for
proprietary products. The Concept doc describes a commercial `RaeenOS Pro` tier at $30/$5-mo.
Slint's commercial license costs would apply, making it impractical.

**What to do:** Study Slint's `.slint` widget file format and compiler architecture for
inspiration on RaeKit's `.rae` widget format. The compiler-based approach (compiling declarative
UI to Rust at build time) is exactly right for zero-runtime-overhead RaeKit components.

**Reference:** `slint-ui/slint` on GitHub — GPL v3 / commercial.

---

## 📖 Reference Patterns — cpal (audio buffer management)

**Concept alignment:** `RaeenOS_Concept.md §RaeAudio`: "sub-3ms round-trip, no ASIO mess."

cpal is NOT usable on RaeenOS bare metal — it abstracts CoreAudio/WASAPI/ALSA. On RaeenOS we
write directly to HDA DMA rings. But cpal's `StreamConfig` (sample rate, channels, buffer size)
and ring buffer fill/drain logic show exactly how to structure `audio.rs`'s SCHED_GAME thread.

**What to study:**
- `cpal/src/traits.rs` — `StreamConfig`, `SampleFormat` as the model for `RaeAudioConfig`
- `cpal/src/host/alsa/stream.rs` — period-based DMA fill loop (adapt for HDA)
- Buffer latency calculation: `period_frames / sample_rate * 1000 = ms latency`

---

## 📖 Reference Patterns — cap-std (capability-based stdlib)

**Concept alignment:** `RaeenOS_Concept.md §RaeShield`: "apps request capabilities, user grants, OS enforces at syscall layer."

cap-std provides a capability-based `std::fs` / `std::net` wrapper. On POSIX, it uses file-descriptor
inheritance to enforce directory boundaries. On RaeenOS, the capability enforcement is kernel-level
(our `Cap` enum + syscall gating), but the **userspace API model** of cap-std is exactly right
for `RaeKit` apps — a `Dir` capability grants access to a directory subtree, nothing more.

**What to implement (Phase 9 / RaeShield):**
- RaeKit apps receive `RaeDir`, `RaeNetSocket`, `RaeMic` capability tokens from the OS
- Each token is a file-descriptor-equivalent that limits what the app can do
- Study cap-std's `Dir::open_file()`, `Dir::create_dir()` API for RaeKit's `Cap<File>` model

**GitHub:** `bytecodealliance/cap-std` — Apache 2.0.

---

## ❌ Do Not Use

### Sprout (thin bootloader)
Cloud-native, VM-focused, no UEFI/GOP/Secure Boot support. Our `bootloader_api` already works
and gives us GOP framebuffer + UEFI memory map. Sprout's sub-50ms is achieved by skipping firmware
entirely — not applicable to desktop hardware (Athena uses UEFI). No action needed.

**Study instead:** Look at Sprout's deferred-init pattern for speeding up our Tier 2 boot phase.

### Kerla (Linux ABI Rust OS)
**Concept doc §R7 explicitly rejects Linux clones.** Kerla reimplements Linux semantics top-to-bottom
including `epoll`, `inotify`, `seccomp`, and `bpf`. Our `linux_syscall.rs` already handles
the Linux ELF ABI compatibility layer (~80 syscalls) without cloning the Linux architecture.

**Licensing:** Kerla appears to be MIT-licensed but is effectively unmaintained. Even as reference,
it would lead us toward Linux-clone patterns the Concept doc prohibits.

**Use instead:** FreeBSD's Linux compatibility layer (`compat/linux/`) — MIT, production-grade,
shows how to handle Linux ABI without being Linux.

---

---

## ➕ Add Now — gilrs (gamepad input library)

**Concept alignment:** `RaeenOS_Concept.md §Gaming`: "DualSense + Xbox + every controller with full feature parity (haptics, adaptive triggers, gyro)."

**What gilrs provides:** Pure-Rust gamepad state machine — button/axis normalization, hotplug, force feedback. Apache-2.0/MIT. Handles the XInput, HID, and evdev backends. Abstracts DualSense adaptive triggers, Xbox rumble, and generic HID gamepads.

**Constraint:** gilrs requires `std` (uses threads and platform I/O). It **cannot** run in the kernel.

**Architecture:** The kernel passes raw USB HID reports (already done in `usb_hid.rs`) up to a sandboxed userspace `raeinput` daemon. That daemon links gilrs, normalizes all gamepad state, and pushes `RaeGamepadEvent` structs back to the kernel via IPC for SCHED_GAME dispatch.

```toml
# In components/raeinput/Cargo.toml (userspace daemon — NOT the kernel)
gilrs = { version = "0.10", default-features = false, features = ["serde-serialize"] }
```

**What to build (Phase 12.2):**
- `components/raeinput/` — userspace daemon: reads USB HID from kernel IPC, feeds into gilrs, pushes `RaeGamepadEvent` to SCHED_GAME queue
- Kernel IPC channel: `SYS_GAMEPAD_EVENT_PUSH` syscall (add to `docs/SYSCALL_TABLE.md`)
- gilrs handles: DualSense haptics via HID output reports, Xbox GIP protocol, generic mapping

**GitHub:** `gilrs-project/gilrs` — Apache-2.0/MIT.

**MasterChecklist items this closes:** Phase 12.2 Universal Controller Support (Xbox/DualSense).

---

## ➕ Add Now (userspace) — symphonia (audio decoding)

**Concept alignment:** `RaeenOS_Concept.md §RaeAudio`: "no ASIO mess, no PulseAudio mess." Native decoding without bundling FFmpeg.

**What symphonia provides:** 100% pure Rust audio decoder — WAV, FLAC, MP3 (via feature), AAC, OGG/Vorbis, ALAC. MPL-2.0 license. Has a `no_std` core (`symphonia-core`) but the full codec suite needs `alloc` and requires std for I/O.

**Constraint:** `symphonia-core` + codec crates require `arrayvec` which doesn't compile in a no_std kernel context (confirmed — attempting to add to kernel builds failed). Must run in userspace.

**Architecture:** RaeAudio userspace daemon decodes via symphonia, then pushes raw PCM to the kernel's DMA ring buffer via `SYS_AUDIO_WRITE` syscall.

```toml
# In components/raeaudio/Cargo.toml (userspace daemon — NOT the kernel)
symphonia = { version = "0.5", features = ["wav", "flac", "ogg", "mp3"] }
```

**What to build (Phase 7.2):**
- `components/raeaudio/` — extends existing component with symphonia decode path
- `Decoder::new(format)` → `DecodeResult::Samples` → convert to f32 → push via IPC to kernel ring
- Kernel ring buffer (`AUDIO_RING` in `audio.rs`) already wired

**GitHub:** `pdeljanov/Symphonia` — MPL-2.0.

**MasterChecklist items this unblocks:** Phase 7.2 per-app volume, EQ, routing matrix.

---

## 📖 Reference Architecture — cosmic-text (System76, text shaping)

**Concept alignment:** `RaeenOS_Concept.md §RaeUI`: "native UI framework, Skia + wgpu, premium feel." Text shaping for ligatures, BiDi, and font fallback is notoriously complex.

**What cosmic-text provides:** Pure-Rust text layout on top of `rustybuzz` (HarfBuzz port) + `swash` (font rasterization). System76 uses it for their Cosmic Rust desktop. MIT licensed.

**What to study:**
- `cosmic-text/src/layout.rs` — `Buffer`, `Attrs`, `LayoutLine` for glyph positioning fed to wgpu
- `cosmic-text/src/font.rs` — `FontSystem` for font discovery and fallback chains
- `cosmic-text/src/shaping.rs` — HarfBuzz shaping producing glyph runs

**What this means for RaeUI:** When implementing `components/raeui/src/text.rs` (Phase 8.1), use cosmic-text's layout engine rather than implementing BiDi/ligature shaping from scratch. cosmic-text sits cleanly on top of wgpu via its `Canvas` API.

```toml
# In components/raeui/Cargo.toml (behind gpu_userspace feature)
cosmic-text = { version = "0.12", optional = true }
```

**GitHub:** `pop-os/cosmic-text` — MIT licensed. ✅ Safe for RaeenOS commercial tier.

**MasterChecklist items:** Phase 8.1 Skia integration (text quality), Phase 14.1 RaeShell shell polish.

---

## 📖 Reference Architecture — embassy (bare-metal async executor)

**Concept alignment:** `RaeenOS_Concept.md §RaeKernel`: "hybrid kernel, low latency, driver crashes can't take down the kernel."

**What embassy provides:** The gold standard for no_std bare-metal async in Rust. Uses static task allocation (zero heap fragmentation), interrupt-driven wakeups, and compile-time task sizing. MIT/Apache-2.0.

**What to study:**
- `embassy-executor/src/raw/` — static task arena + waker implementation (adapt for kernel driver tasks)
- `embassy-executor/src/spawner.rs` — `Spawner` as the model for `kernel::userspace_driver` task launch
- `embassy-time/src/` — `Timer::after()` pattern for timeout in NVMe/xHCI polling loops

**What this is NOT:** A replacement for RaeKernel's scheduler (SCHED_GAME, SCHED_NORMAL). Embassy's executor handles cooperative async tasks; our kernel has preemptive real-time scheduling. They serve different layers.

**What to adapt (Phase 2.1 / driver infrastructure):**
- Use embassy's static task arena pattern for userspace driver IPC message handlers
- Use embassy's timer abstraction as a model for deadline timers in `hpet.rs`
- Study embassy's interrupt-to-waker bridge for replacing busy-poll loops in `nvme.rs`

**GitHub:** `embassy-rs/embassy` — MIT / Apache-2.0. ✅ Safe for commercial use.

**MasterChecklist items:** Phase 4.9 24h soak stability, Phase 2 driver improvements.

---

## ❌ Do Not Use — gstreamer-rs / rust-ffmpeg

**Concept doc alignment:** The Concept doc explicitly calls out "no bloated legacy stacks" and treats RaeBridge as the compatibility layer for non-native software.

**Why not:** Both are thin Rust bindings over GStreamer (C) and FFmpeg (C), respectively. Integrating C codecs into the kernel or native stack:
1. Breaks memory-safety guarantees — GStreamer/FFmpeg CVEs would become RaeenOS kernel CVEs
2. Violates the Rust-first architecture at the "hot path" level
3. Pulls in millions of lines of C through linkage (FFmpeg is ~1.2M lines)
4. Contradicts the "no museum of acquisitions" design principle

**What to do instead:**
- Native media: use **symphonia** (pure Rust, userspace RaeAudio daemon) for WAV/FLAC/MP3/OGG
- Legacy media compatibility (MKV, H.264, H.265): run inside **RaeBridge** sandbox with FFmpeg or GStreamer as a win32/Linux compatibility app — isolated, can't touch kernel
- The RaeBridge path means: `ffplay.exe` or VLC run perfectly fine via RaeBridge; they just don't get native OS privileges

---

## Summary Action Plan (updated 2026-06-01)

| Priority | Action | Closes checklist items |
|----------|--------|------------------------|
| **High** | `kernel/Cargo.toml`: `x25519-dalek`, `blake2`, `chacha20poly1305`, `ruzstd` ✅ done | Phase 10: X25519, Blake2s |
| **High** | `components/raeaudio/Cargo.toml`: add symphonia for userspace decode | Phase 7.2: audio routing |
| **High** | `components/raeinput/Cargo.toml`: add gilrs for gamepad daemon | Phase 12.2: controller support |
| **Medium** | Study boringtun for `wireguard.rs` peer mgmt + rekeying | Phase 10: full WireGuard |
| **Medium** | Study Iced + Xilem for RaeKit state model | Phase 8: RaeKit API |
| **Medium** | Study cosmic-text for `raeui/text.rs` layout engine | Phase 8.1: text quality |
| **Medium** | Study embassy executor for static task arena in driver IPC | Phase 2 driver stability |
| **Low** | Study cap-std for RaeKit capability token API | Phase 9: RaeShield |
| **Low** | Study Slint compiler for `.rae` widget format design | Phase 8: RaeKit widget files |
| **None** | Sprout, Kerla, gstreamer-rs, rust-ffmpeg — skip | N/A |

---

## ➕ RaeUI rendering + "feel" stack (Phase 8 / docs/RAEUI_COMPOSITOR_PLAN.md)

Researched 2026-06-11. RaeUI already has the LOGIC (retained `tree.rs`,
`animation.rs`, flexbox `layout.rs`, reactive `binding.rs`) but renders
immediate-mode through the software `raegfx::Canvas` with a placeholder font
rasterizer. These crates supply the *content rendering* + *layout* so the
existing logic can be re-wired into the Core-Animation model. The *feel*
(layer backing + off-thread compositor animation) is RaeenOS code, not a crate.

All are userspace-only (the `gpu_userspace` feature already gates std deps); the
in-kernel compositor's animation driver stays pure-math no_std.

| Crate | Role | Maturity (mid-2026) | License | When |
|---|---|---|---|---|
| **tiny-skia** 0.12 | CPU 2D raster (Skia algorithms: AA paths, gradients, blends, clips) | **Stable**; CPU-only; **no text** (pair w/ cosmic-text) | BSD-3 | **NOW** — replace the placeholder software Canvas; real AA before any GPU |
| **taffy** | Flexbox + CSS-Grid layout (Bevy/Zed/Dioxus use it) | **Stable**, no_std+alloc | MIT | **NOW** — replace `layout.rs` internals |
| **cosmic-text** 0.12 | text shaping/BiDi/ligatures/fallback → glyph runs | **Stable** (System76 COSMIC) | MIT | **NOW** — feeds tiny-skia/Skia raster (already in OSS doc) |
| **swash** | font scaling + glyph rasterization (under cosmic-text/Vello) | Stable | MIT/Apache | with cosmic-text |
| **palette** | color spaces sRGB↔linear↔Display-P3, HDR-correct blends | Stable, no_std+alloc | MIT/Apache | D2 color management |
| **kurbo** | 2D bezier/affine geometry primitives (Linebender) | Stable, no_std+alloc | MIT/Apache | align RaeUI geometry types to these |
| **peniko** | brush/gradient/blend paint model (Linebender) | Stable, no_std+alloc | MIT/Apache | align RaeUI paint types to these |
| **skia-safe** | GPU 2D (Graphite: Vulkan/Metal/D3D) | **Production** (Chrome/Flutter lineage) | BSD-3 | Phase 6 GPU path — the safe production renderer |
| **vello** / **vello_cpu** | GPU-compute 2D, all-Rust (Linebender) | **ALPHA** — not production-ready (verified 2026) | MIT/Apache | **Prototype/watch only** — the all-Rust future, don't ship-block on it |

**Strategy:** ship `tiny-skia` + `cosmic-text` now (stable, CPU, no GPU
needed); use `skia-safe` for the GPU path when Phase 6 lands (production-grade);
**align RaeUI's geometry/paint types to `kurbo`/`peniko`** so a later swap to
Vello is a backend change, not a rewrite. Vello/Xilem are alpha as of mid-2026 —
the all-Rust GPU-compute future, worth prototyping, NOT worth betting the
product on yet.

**Architecture references to STUDY (not deps):** Xilem (Linebender, reactive on
Vello — closest to the SwiftUI+CoreAnimation target), Floem (fine-grained
reactive), Makepad (shader-driven "stunning" visuals). The full **Linebender
stack** (tiny-skia → vello, kurbo, peniko, parley, taffy, xilem) is effectively
the all-Rust version of exactly what `RaeenOS_Concept.md §RaeUI` describes —
align with it.

Sources: linebender/tiny-skia, linebender/vello, linebender/xilem,
pop-os/cosmic-text, DioxusLabs/taffy (crates.io / lib.rs, verified 2026-06).

---

## ➕📖 Net-new gap mapping (2026-06-16): browser · Win32/DirectX · accessibility · native GPU · bootloader

Researched 2026-06-16 (repos verified reachable via `git ls-remote`). These cover
gaps the sections above don't: the **browser** (Milestone-A "load a page" long
pole), **RaeBridge** Win32/DirectX, **accessibility** (the largest *unowned*
parity gap in `PRODUCTION_CHECKLIST.md` Part XVI), the *permissive* framing of
the **GPU** path, and the **bootloader** EFI write gap. The RaeUI 2D / layout /
text crates (Vello, taffy, cosmic-text, swash, rustybuzz, tiny-skia, skia-safe,
kurbo, peniko) are already covered in the "RaeUI rendering + feel stack" section
above — not repeated here.

**The decision rule for a paid/proprietary OS:** permissive (MIT/Apache/BSD/zlib/
MPL) = *vendor it* (a genuine speed-up); GPL = *study patterns only*; LGPL =
dynamic-link friction, isolate it. The notable wins below: **DXVK is zlib** and
**Mesa / Stylo / Blitz are permissive** — so the browser and DirectX paths, which
look like "years of work," have *vendorable* foundations, not just GPL ones.

### Browser — the Milestone-A "load a page" gap (Phase 14.2)

| Project | License | Status | Use |
|---|---|---|---|
| **Blitz** (`DioxusLabs/blitz`) | MIT/Apache | ➕ vendor (eval) | Native HTML/CSS renderer on **Stylo + Vello/wgpu** — the exact stack RaeUI already uses. Cheapest path to a *native web view*: embed on the existing wgpu compositor instead of porting a whole browser. **Young / pre-1.0** — assess maturity first. HTML/CSS render, no JS engine. |
| **Stylo** (`servo/stylo`) | MPL-2.0 | ➕ vendor | Mozilla's Rust CSS engine (Servo + ex-Firefox). Standalone-usable for the CSS layer if building the web view from parts. |
| **html5ever** (`servo/html5ever`) | MIT/Apache | ➕ vendor | Spec-compliant HTML5 parsing in Rust. |
| **Servo** (`servo/servo`) | MPL-2.0 | 📖→➕ | Full engine incl. JS (SpiderMonkey), embeddable via `libservo`. The "real browser" path if a native view isn't enough. Heavy. |
| **Ladybird** (`LadybirdBrowser/ladybird`) | BSD-2 | 📖 | Independent C++ engine (LibWeb/LibJS); permissive + fast-moving, but C++ not Rust, so less stack-aligned. Watch. |

**Recommendation:** prototype **Blitz** (or Stylo + html5ever + Vello directly)
as the native web view for G1; reserve Servo for a full browser later.
RaeBridge+Chromium is not a near-term option (years out).

### RaeBridge — Win32 + DirectX (Phase 11.2)

| Project | License | Status | Use |
|---|---|---|---|
| **DXVK** (`doitsujin/dxvk`) | **zlib** | ➕ vendor | D3D9/10/11 → Vulkan. Permissive (not GPL) — the Concept's "DXVK lineage" is *vendorable*, not just referenceable. The DirectX-11 translation path. |
| **VKD3D-Proton** | LGPL-2.1 | 📖 link | D3D12 → Vulkan. LGPL = dynamic-link friction; usable, isolate it. |
| **windows-rs** (`microsoft/windows-rs`) | MIT/Apache | ➕ generate | Microsoft's own Win32 API metadata + bindings. RaeBridge hand-maintains a 16k-name registry + thunk signatures — windows-rs's `win32metadata` can *generate* those. Cuts the most tedious RaeBridge work. |
| **Wine** | LGPL | 📖 lineage | The Win32 lineage (already the planned RaeBridge heritage). Study; don't vendor verbatim into the proprietary tree. |

### Accessibility — the largest unowned parity gap (`PRODUCTION_CHECKLIST.md` Part XVI)

| Project | License | Status | Use |
|---|---|---|---|
| **AccessKit** (`AccessKit/accesskit`) | MIT/Apache | ➕ vendor | The Rust accessibility-tree standard (egui, Bevy, Zed, Druid). Cross-platform a11y node tree + actions; seeds RaeUI/RaeShell accessibility (screen reader, etc.) instead of building from zero. Userspace (`alloc`). **Highest-leverage find for the a11y gap.** |

### Native GPU — the *permissive* framing (Phase 6 / `NATIVE_DRIVER_PLAN.md`)

| Project | License | Status | Use |
|---|---|---|---|
| **Mesa** | **MIT** | 📖→➕ | radeonsi/radv 3D. **MIT, not GPL** — the LinuxKPI+Mesa 3D path is *permissive* to vendor (C; needs the kernel submit interface). AMD register headers in Mesa's tree (MIT) are reusable for the native scanout register defs. |
| **Asahi** (`AsahiLinux/m1n1` + drivers) | MIT / GPL mix | 📖 study | Best **native-Rust GPU/display** reference (modeset, ring, firmware-handshake patterns). Apple GPU/ARM → patterns not code, but the gold standard for what the native AMD scanout/modeset driver reaches for. |
| Linux **amdgpu** (DC/DCN) | GPL | 📖 study | DCN modeset / PLL / link-training register sequences — study-only (GPL); pair with AMD's published register specs. |

### Bootloader + misc (gaps from earlier discussions)

| Project | License | Status | Use |
|---|---|---|---|
| **uefi-rs** (`rust-osdev/uefi-rs`) | MPL-2.0 | ➕ eval | EFI Boot/Runtime Services in Rust — the pre-exit-boot-services `SetVariable` + ESP write the **dual-boot bootability gap** needs (`MasterChecklist 16.1`). |
| **image** / **zune-png** | MIT/Apache | ➕ vendor (userspace) | PNG/JPEG decode for the Photos app + wallpapers (Phase 14.2). |

**2026-06-16 priority additions to the action plan:**
- **High** — prototype **Blitz** (browser / native web view): the gap with the least existing plan.
- **High** — **AccessKit**: opens the accessibility gap (currently unowned, no MasterChecklist phase).
- **Medium** — **DXVK** (zlib) + **windows-rs** for the RaeBridge DirectX/Win32 surface.
- **Medium** — record in the GPU plan that **Mesa is MIT** (vendorable, not just GPL-referenceable).
- **Low** — **uefi-rs** for the dual-boot `SetVariable` half; **image/zune-png** for Photos/wallpaper.

**Check-before-add reminder (workspace rule):** these are evaluations, not yet in
the tree. Per CLAUDE.md, confirm license + `no_std`/userspace fit and add to the
relevant `Cargo.toml` deliberately; GPL/LGPL projects stay study-only or isolated
behind RaeBridge — never vendored into the first-party proprietary tree.
