# AthenaOS Gap Analysis: Concept Doc vs Reality + Real Hardware Path

**Last updated: 2026-06-22** (non-UI subsystem wave reconciled — image/audio/office/mail/PIM/auth/sync/JS-engine/LinuxKPI libraries closed many app-layer gaps at `[~]`; see the dedicated section below. Prior: accessibility gap section; security/boot rows per prior dates)

## What's DONE and RUNNING (verified at boot)

All 9 initialization tiers complete. Boot log confirms every module initializes successfully.

| Concept-doc component | Implementation | Status |
|---|---|---|
| **AthKernel hybrid architecture** | Boots, runs userspace ELFs, capability-gated syscalls | **Year-1 complete** |
| **SCHED_BODY priority class** | Separate queue, preempts Normal, EDF deadlines, per-CPU affinity | **Functional** |
| **Capability-based sandboxing** | 14 cap flavors, derivation, grant/revoke syscalls | **Functional** |
| **AthFS** | Mount/format/read/write via BlockDevice trait, CoW writes, journal WAL, snapshots, refcounting | **Functional (QEMU)** |
| **AthUI (minimal)** | Label + Button + Frame + theme palette over software Canvas | **Functional** |
| **Compositor** | Multi-window, z-ordered, VRR pacer, HDR pipeline, glassmorphism blur, GPU scanout (32bpp BGRA), hardware cursor | **Functional** |
| **SMP** | INIT-SIPI-SIPI, 4 cores online, per-CPU GDT+TSS, per-CPU scheduler | **Functional (real HW)** |
| **Networking (full stack)** | smoltcp L2+L3 via unified NIC (e1000/virtio-net), DHCP, DNS, firewall, traffic shaper, QUIC, tunnel | **Running** |
| **IPC ring buffers** | Bounded channels with flow control + secure IPC (authenticated) | **Functional** |
| **Security stack** | TPM 2.0, encryption, crypto API, hardening, anticheat, audit, MAC, sandbox | **Running** |
| **Process model** | Process table, init system, signals, namespaces, POSIX compat | **Running** |
| **Linux syscall compat** | 80+ Linux x86_64 syscalls: file I/O, fork/exec, mmap, sockets, futex, epoll, clone, prctl | **Running** |
| **Dynamic linker** | ld-linux.so equivalent, lazy binding, NEEDED resolution | **Running** |
| **Virtual filesystems** | /proc (Linux-compat), /sys (kobject-based sysfs), tmpfs (/tmp, /dev/shm) | **Running** |
| **USB/Input** | USB core framework, DualSense/Xbox/HID/RGB, PS/2 mouse (cursor + click dispatch to shell) | **Running** |
| **Audio** | HDA controller (PCI-discovered, bus-mastered), SCHED_BODY audio thread (128-frame/2.67ms period) | **Running** |
| **Power/Thermal** | Power management, thermal, CPUfreq, suspend/resume, power supply | **Running** |
| **ACPI full** | AML parser: DSDT/SSDT loaded, 53 namespace devices, scopes/devices/methods/processors | **Running** |
| **Platform** | PCIe (legacy I/O + ECAM-ready), EFI runtime, firmware interface, TTY, D-Bus kernel bus, Bluetooth | **Running** |
| **Reliability** | Crash dump, watchdog timer, fast boot profiler, hardware diagnostics | **Running** |
| **Kernel modules** | Hot-loading driver framework | **Running** |
| **Overclocking** | CPU/GPU frequency + voltage API | **Running** |
| **NUMA** | Topology discovery, per-node allocator | **Running** |
| **Permission prompts** | Kernel request queue, approve/deny API, timeout, UI-ready | **Running** |
| **Shell API** | AthShell + GameOS registered, desktop shell rendered into compositor surface at boot | **Running** |
| **Kernel code purge** | Deleted ~36k LOC of dead/stub technical debt (`sched_advanced`, `vmm`, `ipc_advanced`, etc.); kernel now ~114k LOC | **Complete** |
| **AthBridge (Win32 + DirectX)** | 36 DLL dispatch, PE loader, D3D9/11/12/DXGI translation, registry hive w/ Steam+DirectX+VC++ keys, full kernel32/user32/gdi32/ntdll/ws2_32/msvcrt surface | **Compiled + wired** |
| **AthKit SDK** | Declarative UI, reactive state, app lifecycle, NavigationStack/TabView, IPC | **Compiled** |
| **AthStore/AthID/AthSync** | App store + passkey auth + E2E encrypted cross-device sync | **Compiled** |
| **AthPlay** | Steam/Epic/GOG/AthStore unified launcher, playtime, achievements | **Compiled** |
| **Vibe Mode + GameOS** | 12 theme presets with smooth transitions; couch UI with controller nav | **Compiled** |

## Known Issues

| Issue | Severity | Details |
|---|---|---|
| **MCFG on QEMU** | Expected | SeaBIOS doesn't emit MCFG, so PCIe uses legacy I/O. Real hardware with UEFI will have MCFG and ECAM will activate automatically. |
| **GPU (real HW)** | Expected | No real GPU driver (Intel/AMD/NVIDIA). Bochs VBE driver active for QEMU stdvga with LFB modesetting. |

## Security enforcement gaps (audited 2026-06-09; re-audited 2026-06-11)

Four of the five gaps recorded on 2026-06-09 are **FIXED with boot-verified
regression fences as of 2026-06-11** (each now has a smoketest that proves the
enforcement on every boot and would catch a regression). The crypto-primitive
holes of this same class were fixed earlier (Ed25519 real + KAT;
RSA/ECDSA/DH/ECDH/AEAD fail-closed) — commits 79ad97d, 0ec53e8.

| Severity | Location | Status |
|---|---|---|
| ~~CRITICAL~~ FIXED 2026-06-11 | `secure_ipc.rs` `verify_cap_static` | Replaced by `verify_cap(sender, token, required)`: anti-spoof (`token.holder_task == sender`), the handle must exist in the sender's REAL CapTable at send time (revocation-safe), and policy rights are checked against the TABLE's cap — the token's self-declared rights are ignored for authorization. Boot proof: `[secure-ipc] smoketest: forged_denied=true real_ok=true spoof_denied=true open_ok=true -> PASS`. |
| ~~CRITICAL~~ RESOLVED (by design) 2026-06-11 | `EndpointPolicy::open()` + `Rights::NONE` | Decided: `Rights::NONE` is the documented public-channel contract — the policy value comes from kernel-mediated channel CREATION (the creator opts out for that side), never from the sender, so it is not sender-forgeable. Documented at `verify_cap`. |
| ~~CRITICAL~~ FIXED 2026-06-11 | `perm_prompt.rs` `resolve()` | Approval now performs the REAL grant: the full requested `Cap` is carried in the request and `insert_root`-ed into the requester's CapTable (user consent = granting authority); the granted handle is returned via `poll_verdict`. Approval for a dead task degrades to Denied. Syscalls 71/72 are now gated on a held `Cap::System` (previously ANY task could approve its own request — with the grant wired that would have been a one-syscall sandbox escape, so the gate landed in the same commit). user_init is seeded the System cap. Boot proof: `[perm] smoketest: approve=Approved handle=N in_table=true deny_clean=true -> PASS`. |
| ~~HIGH~~ FIXED 2026-06-11 | `security.rs` `verify_chain()` | Tautology replaced with TCG-style event-log replay: PCR baseline captured pre-measurement, the log is folded over it (`PCR = SHA-256(old ‖ hash)`), and verification requires the replay to reproduce the live (TPM-synced) PCR values per touched PCR. Tampering with the log or PCR divergence now fails verification, making `generate_attestation_quote` coherent. Boot proof: `[secboot-chain] smoketest: intact=true tamper_detected=true restored=true -> PASS`. |
| ~~CRITICAL~~ FIXED 2026-07-04 ([~] host-KAT, DEAD path) | `athshield::verify_signature` (code-signing gate) | Did structural validation ONLY (digest match, sig size/non-zero, pubkey size, expiry, CRL, trust level) then returned `Valid` **without ever verifying the Ed25519 signature** — any random non-zero blob over the target's own hash + a well-formed signer cert was accepted (complete code-signing bypass). Now calls `ath_crypto::ed25519::verify(pubkey, binary_hash, sig)`; `Rsa4096` fails closed (no RSA verifier). Currently DEAD (Phase 9 primitive, not yet kernel-wired) but a latent auth-bypass landmine. FAIL-able host KAT (`cargo test -p athshield codesign`), mutation-verified. |
| ~~HIGH~~ FIXED 2026-07-04 | `security.rs` `generate_attestation_quote` (BUG-33 software-path leak) | The 2026-06-11 fail-closed fix caught only the `Err` arm; `TpmDevice::quote` returns `Ok(unsigned blob)` for a `SoftTpm`, so on every TPM-less platform the attestation `tpm_quote` field carried a forgeable, publicly-computable blob (`RAEQ ‖ nonce ‖ SHA256(pcrs‖nonce)`) — the exact hole BUG-33 claimed to close. Now gated on `TpmDevice::is_hardware()`: only a real hardware TPM's AK-signed quote escapes; software falls through to the empty fail-closed quote and the caller relies on the Tier-1 platform HMAC. FAIL-able fence added: `quote_fails_closed = have_hw || quote.is_empty()`. Iron proof (`tpm_available=false`): `[secboot-chain] smoketest: … quote_fails_closed=true -> PASS`. |
| HIGH (live, intentional) — **still open** | `sandbox.rs:108` `level_of` / `:183` `check_syscall` | Untracked PIDs default to `SandboxLevel::Trusted` (allow-all); `check_syscall` is live at `syscall.rs:506`. **Load-bearing fail-open for bring-up** — flipping to default-Deny kills every untracked task and breaks boot. Real per-app sandbox policy (manifests) is the Phase 9 fix, not a default flip. |

## Accessibility (ship gate #7 — gap is now INTEGRATION, not greenfield) — audited 2026-06-21

**Authoritative current state:** `docs/research/accessibility-audit-2026-06-21.md` (verified
against live source). Live status ladder: `MasterChecklist.md` + `docs/PARITY_MATRIX.md §J`.
The older planning docs (`docs/research/accessibility-implementation-plan.md`,
`docs/research/phase19-accessibility-foundation.md`) are **SUPERSEDED (status only)** — they
describe a11y as not-yet-started; that is stale. Most of the stack is built.

a11y was the largest unowned parity gap; it is now OWNED and largely built. The remaining work
is **integration + user-reach + audits, not new engines.** Do NOT re-spec the built pieces.

| Dimension | Status | Built (live evidence) | Remaining gap |
|---|---|---|---|
| Accessibility tree + AT ABI | **Built, `[~]`** (QEMU; iron-unproven) | `kernel/src/a11y.rs` (1236 lines) builds the tree from the live compositor surfaces; R10-complete (init line, FAIL-able smoketests, `/proc/athena/a11y`, Concept docstring); cap-gated `SYS_A11Y_SNAPSHOT` (277)/`SYS_A11Y_ACTION` (278), `Cap::Accessibility` READ/WRITE, fail-closed | **#1 P0: widget-provider WIRING** — `a11y::publish_window_widgets` + `athui::provider_nodes_for_window` both exist but have ZERO callers, so every app is one anonymous "Window" node to a reader |
| Screen reader (announce core) | **Built, `[~]`** | `a11y.rs` `announce_node`/`describe_focused` (VoiceOver/Narrator phrasing) over a pluggable `SpeechSink`; `LogSpeechSink` QEMU-provable; focus-generation poll; athui role inference `role_from_widget_kind` (NOT the old `Group` stub) | No real TTS->AthAudio `AudioSpeechSink` (iron/Phase 7); no reader nav verbs on live keys |
| Magnifier | **Built, `[~]`** | compositor source-sampled scanout upscale (1x-8x), focus-follows pan via `a11y::follow_focus_in`, FAIL-able `run_magnifier_smoketest`; composes with color filters | No smooth-pan ease; no lens/docked mode; iron 60fps unproven |
| Color filters (invert/grayscale/HC) | **Built, `[~]`** | compositor `a11y_filter_set` per-pixel scanout post-process + smoketest | — (engine complete) |
| High-contrast forced-colors LIVE mode | **SHIPPED** (newer than the 2026-06-21 audit, which listed this `[ ]`) | `a11y::toggle_high_contrast`/`set_high_contrast` drive `ath_tokens::set_high_contrast`; `ath_tokens::active_palette()` returns `HIGH_CONTRAST` so every surface repaints in HC on next frame. Proven: `a11y::run_onswitch_smoketest` (`hc_palette_swapped`/`hc_reverts`) + host KAT `active_palette_swaps_under_high_contrast` | Broaden which surfaces honor `active_palette()`; iron-unproven |
| User-facing on-switches (hotkeys + Control Center) | **SHIPPED** (newer than the 2026-06-21 audit, which listed this a P0 gap) | `shell_runner` global hotkeys Super+Alt+M (magnifier), +H (high-contrast), +C (color filter), +R (reduced-motion), Super+=/Super+- (zoom); Control Center Accessibility tile drives the same `a11y` backend; FAIL-able `run_onswitch_smoketest` proves each toggle FLIPS live state | Settings-app toggles; per-engine on-screen affordance; iron live-key (HID typing iron-pending) |
| Contrast math (WCAG) | **Built (unit-tested)** | `ath_tokens::contrast_ratio` (relative luminance) + `HighContrastPalette`, tested >= 4.5 / >= 7.0 | **#5: no FAIL-able BOOT/CI audit over the FULL painted palette** (the parity win nobody ships) |
| Reduced-motion | **Built (partial), `[~]`** | `shell_runner::reduced_motion()` reads `/a11y/reduced_motion`; `a11y::toggle_reduced_motion`; `athshell::animations::set_reduce_motion`; `ath_tokens` `MOTION_INSTANT`/`REDUCED_MOTION_DURATION_MS` | Only some animation sites verified to honor it; no all-sites audit |
| Keyboard-only nav + visible focus | **Partial** | athui `focus_next/prev` + `focusable_nodes()` + `FocusRing`; per-surface key handlers; `gameos.rs:draw_focus_ring` (4-signal) | **#4: no unified desktop focus order** across shell chrome (taskbar/start/tray/notifications), no modal focus-trap contract, no FAIL-able "no-mouse" audit |
| Text scaling | **Missing** | ath_tokens type ramp + athui bounds computation exist | No global text-scale factor read by layout; no layout-fit audit |

**Bloat flag:** `components/athaccessibility` (2268 lines) is a workspace member NOTHING imports
(no dep, no `use`, no `init()`, no R10) — duplicates the live `kernel/src/a11y.rs` + `athui`
shape. Demote to reference/`[experimental]` or harvest the few needed pieces; do not invest in
it as-is (the live path is `a11y.rs` + the athui seam).

**Top remaining a11y items (from the audit, ranked by leverage):** (1) connect the widget
provider so apps name their controls [P0]; (2) unified desktop keyboard focus order + modal trap
+ "no-mouse" audit [P1]; (3) FAIL-able WCAG contrast audit at boot [P1]; (4) real TTS sink
[P1, iron/audio]; (5) global text scaling [P1]. The on-switches and HC-live mode that earlier
plans listed as gaps are DONE.

## Non-UI subsystem wave — app-layer gaps closed/partially closed (2026-06-21 wave, reconciled 2026-06-22)

**Source of truth:** `MasterChecklist.md` top work-log ("Lead-orchestrated non-UI subsystem
wave", ~29 dated entries, 986 host KATs across 20 component crates, 0 failures). Everything
below is **host-KAT-proven (`[~]`)** — real decode/parse/protocol logic, FAIL-ability confirmed,
hostile-input-hardened — but **NOT yet wired into apps and NOT proven on iron**. None of this is
`[x]` (iron proof is paused this wave). When in doubt, downgrade.

| Concept/parity gap | Before | Now (`[~]`, host-KAT) | Remaining for `[x]` |
|---|---|---|---|
| "Show my photos" — image decode | PNG-in-Files only | full **decode** BMP/GIF/PNG/JPEG/WebP-VP8L + **encode** PNG/JPEG + `ath_image` unified dispatcher | app-wiring beyond Files; VP8-lossy / video; iron |
| "Play my music" — audio decode | WAV→mixer only | WAV/FLAC/MP3/AAC/Opus decoders + player open/decode path; MP3+AAC audible on host; SCHED_BODY GameMixer + windowed-sinc SRC | bit-exact external fixtures; iron HDA; app-wiring |
| "Play my movies" — container | none | `ath_mp4` ISO-BMFF demux (sample table + ES extraction) | actual video codec (H.264) decode; iron |
| Office (Word/Excel/PDF) | none | `ath_docx`/`ath_xlsx`/`ath_pdf` read + DOCX/XLSX **write** + XLSX **formula compute** + `rae_print` PDF-1.7 generator | app UI; iron |
| Mail | out of scope | `ath_mail` SMTP/IMAP/POP3 + RFC822/MIME (transport-abstracted) | live TLS/TCP-over-athnet wiring; app UI; iron |
| Calendar / Contacts | none | `ath_pim` iCal/vCard parse + RRULE expander + POSIX timezone engine | app UI; iron |
| Credential manager / Keychain | AthID only | `ath_keychain` (argon2id KDF + chacha20poly1305 AEAD, fail-closed, zeroized) | OS integration; UI; iron |
| Passkeys + 2FA | structural | `athid` WebAuthn EdDSA+ES256 ceremony core + `ath_otp` HOTP/TOTP (RFC vectors) | authenticator UX; iron |
| E2E cross-device sync | "Compiled" | `athsync` device enroll + wrapped group key + AEAD SyncBlob + LWW-CRDT convergence proof | server; drive UX; iron |
| Backup / export | AthFS snapshots | `ath_tar` .tar.gz writer + `ath_zip` ZipWriter + `ath_kv` embedded KV | UI; iron |
| App update engine | A/B slots `[~]` | `RaeUpdate` verified transactional A/B delta (SHA-256+Ed25519 verify-before-apply, atomic flip, auto-rollback, power-loss recoverable) | UI; iron |
| `.athpkg` / store install | hashed deps | bounds-checked TLV codec + fail-closed verify + transactional dependency-correct install/uninstall+GC | client UI; iron |
| Browser (JS) | HTML/CSS, no JS | `ath_js` from-scratch engine: parse+execute+Map/Set/Date/Symbol+RegExp+Promise/event-loop, budget-bounded (never hangs host) | layout/render; DOM bindings; async/await suspension; app-wiring; iron |
| PWA install | manifest parse | `ath_pwa` URL resolution + InstallDescriptor (launchable record) | render + launch wiring; iron |
| TLS 1.3 | "real handshake" | full RFC 8446 client handshake + cert-chain validation + CertVerify + hostname binding, fail-closed (**MITM-safe**) | live socket integration; iron |
| WireGuard VPN | Noise handshake `[x]`-flagged | fixed CRITICAL cleartext-static-key defect; spec-correct Noise_IKpsk2 (tamper/forge rejected) | full tunnel; iron |
| LinuxKPI driver breadth (GPU path) | atomics/MMIO/workqueue | list/hlist/klist/rculist/xarray/llist + ww_mutex/drm_exec + seqlock/completion C-ABI facades | real GPU submit (Mesa); iron |
| SCHED_BODY deadline telemetry | "EDF exists, telemetry missing" | lock-free perf counters + miss-detection hook + `/proc/athena/perf`, verifier-PASS | iron game-thread overrun proof |
| Multi-arch reach (criterion #3) | x86_64-only | `kernel/src/arch/` HAL Slice 0 (x86_64 backend, verifier-PASS) | aarch64 backend actually booting; iron |
| Boot time (live-fix #1) | ~14.2s QEMU TCG | -1.6s (deferred 7 pure-test smoketests), verifier-PASS 6 boots | still >6s; iron total ~11s — **open** |

**Hostile-input posture (#6 "never crash/hang/OOM on hostile input"):** 9 deep security reviews
this wave hardened the three biggest hostile-input surfaces (JS engine, media decoders, network
parsers) plus office readers, all 5 image decoders, and the crypto/auth foundation — 33 real
bugs found+fixed. The full merged OS boots **7/7 HEALTHY** both SMP at clean HEAD.

**Gaps that REMAIN (NOT closed by this wave):** UI polish + light-theme ("Lumen") shoot;
real-GPU `vkQueueSubmit` submit path; all iron proof (input on real HW, GPU on real HW, audible
audio on real HDA, app-wiring end-to-end); aarch64 actually booting; app-wiring of these
libraries into shipped apps; unbuilt codecs (zstd/brotli/VP8-lossy/H.264 — unprovable here
without reference frames).

## What's STRUCTURAL (compiles, types defined, not fully exercised)

| Category | Modules | What exists | What's missing |
|---|---|---|---|
| **AthAudio** | `audio.rs` + component | HDA register map, PCI class discovery, bus-mastered, SCHED_BODY thread | Real codec negotiation, sub-3ms path verification |
| **AthGFX** | `gpu.rs`, `display.rs`, `components/athgfx` | Bochs VBE modesetting (QEMU), VirtIO-GPU, compositor scanout, `vulkan.rs` API surface, SW hello-triangle in `user_init` | No real GPU driver (Intel/AMD/NVIDIA); boot demo is SW raster, not `vkQueueSubmit` |
| **AthBridge** | 36 Win32 DLL shims + DirectX | Full kernel32 (codepage, locale, file mapping, FLS, init-once, paths, disks, IOCP, pipes, SRW locks, condition vars); user32 (monitors, raw input, hooks, window enum, coordinate mapping, layered windows); gdi32 (paths, regions, palettes, gradient/alpha blending, font enumeration, world transforms); ntdll (SEH/VEH, mutants, semaphores, timers, RTL unwind, string conversion, status→DOS error); ws2_32 (WSA async, events, wait, send/recv-from, duplicate socket); msvcrt (low-level file I/O, threading, signals, CRT init, wide string, snprintf) | DXBC→SPIR-V shader compilation, real GPU command submission, Steam compatibility test |
| **AthShell** | 30+ submodules | Window manager, taskbar, file manager, terminal, GameOS couch mode (1633 lines), Vibe Mode (842 lines) | Desktop rendered at boot; keyboard+mouse input routed; cursor visible; click dispatch (start menu toggle, taskbar focus, tray→settings, window focus). App launch from start menu still needed. |

## What DOESN'T EXIST AT ALL

| Promise | Gap |
|---|---|
| ~~AthFS CoW + snapshots~~ | **Implemented** — `cow_write_block`, `create_snapshot`, `replay_journal`, refcount tracking |
| ~~AthFS tiered storage~~ | **Implemented** — `TieredStorage` with NVMe/SATA/HDD tiers, `promote`/`demote` operations |
| ~~AthFS native encryption~~ | **Implemented** — XTS-AES-256, `EncryptionKey`, `encrypt_data_block`/`decrypt_data_block`, KDF salt in superblock |
| **AthGFX native graphics API** | No Vulkan-equivalent surface ("looks like Metal, performs like Vulkan") |
| ~~DirectX 11/12 -> AthGFX translation~~ | **Implemented** — `d3d9`, `d3d11`, `d3d12`, `dxgi`, `d3d_translate` modules in athbridge |
| ~~WireGuard built-in~~ | **Implemented** — Noise IK handshake, ChaCha20Poly1305 transport, `WireGuardInterface`, peer management |
| ~~AthKit SDK~~ | **Implemented** — Declarative UI (`view`, `builders`), reactive state, app lifecycle, navigation (NavigationStack/TabView), IPC |
| ~~AthStore / AthID / AthSync~~ | **Implemented (`[~]` host-KAT)** — AthStore: `.athpkg` fail-closed verify + transactional dep-correct install/GC; AthID: WebAuthn EdDSA+ES256 ceremony core + sessions + guest; AthSync: E2E sync with LWW-CRDT convergence proof. App-wiring + iron pending (2026-06-21 wave) |
| ~~AthPlay~~ | **Implemented** — Steam/Epic/GOG/AthStore unified library, playtime tracking, launch orchestration, achievements |
| ~~Theme engine (compositor-level)~~ | **Implemented** — Vibe Mode: 12 presets (Cyberpunk Night through Sakura Dawn), smooth ARGB transitions, time-based auto-switch |
| ~~GameOS couch mode~~ | **Implemented** — 1633 lines: controller nav, game grid + carousel, quick menu, settings, search, achievements |
| ~~RGB unified API~~ | **Implemented** — `RgbManager`, `RgbDevice`, `RgbEffect` (static/breathing/wave/reactive), sync, brightness |
| ~~Memory pinning API for games~~ | **Implemented** — `MemoryPinManager`, `pin_memory`/`unpin_memory`, 50% cap, cap-gated |
| ~~NULL_LATENCY mode~~ | **Implemented** — Dedicated core pinning, interrupt routing, max freq, background throttling |
| **Shader cache (OS-level)** | `ShaderCache` struct with insert/get/evict, but no GPU pipeline to populate it |

## Bare-metal boot gate (summary)

**Full checklist:** [`docs/HARDWARE_PATH.md` §9 — Bare-metal boot gate](docs/HARDWARE_PATH.md#9-bare-metal-boot-gate-kernel-checklist)

| Tier | What it blocks | Dominant gap |
|---|---|---|
| **0** | First power-on / any input | **xHCI + USB HID** (UEFI keyboard dies after handoff); UEFI untested; `_PIC` / `_OSI` |
| **1** | Useful desktop | Real NIC (I225-V, RTL8125); GPU modeset; power/thermal on iron |
| **2** | Installable OS | GPT/ESP, AthFS on NVMe partition, verified boot chain |
| **3** | Fleet reliability | Quirks, AER/MCE, OOM, per-driver IOMMU DMA |

**Honest calendar (one engineer, one SKU like Beelink Athena):** ~2 months to interactive first boot; +~2 months install; +~6 months short curated list.

**Highest leverage now:** xHCI + USB HID. Parallel 1-day: `_PIC(1)`, `_OSI`, PS/2-absent log, UEFI boot attempt with SB off.

**QEMU note:** virtio-net + DHCP `Bound` on QEMU (2026-05-28) does **not** close bare-metal networking — still need I225-V/RTL8125 or Path C Wi-Fi.

## What's needed to boot on REAL HARDWARE (legacy table — see §9 for truth)

The rows below marked **Done** mean "implemented and exercised in QEMU," not "validated on bare metal."

| Gap | Priority | Effort | Why |
|---|---|---|---|
| **NVMe DMA + I/O queues** | **QEMU** | — | Smoketest on emulated NVMe; real Samsung/WD quirks open |
| **BlockDevice abstraction** | **QEMU** | — | Trait + virtio/NVMe/AHCI adapters |
| **MSI-X support** | **QEMU** | — | Allocator + IDT stubs; iron IRQ storm risk unproven |
| **e1000 NIC probe** | **QEMU** | — | `recv()` exists; not I225-V; not validated on iron |
| **PCIe ECAM** | **QEMU** | — | MCFG when firmware provides it; vendor quirks untested |
| **AHCI DMA** | **Structural** | ~2 weeks iron | Module exists; not iron-tested |
| **xHCI + USB HID** | **Critical** | **3–6 weeks** | **Blocks laptops** — PS/2-only is insufficient |
| **IOMMU (VT-d)** | **Partial** | ongoing | DMAR init can enable translation; drivers don't all map DMA |
| **ACPI AML** | **Partial** | ongoing | Parses DSDT; `_OSI`, GPE, EC, `_PIC` gaps remain |
| **UEFI boot image** | **Open** | 1 day spike | `kernel.uefi.img` never booted on real firmware |

## Where you are on the 5-year roadmap

**Year 1 target**: "Kernel + AthFS + AthGFX + AthUI hello world. Boots, draws, plays a single Vulkan demo."

- Kernel: **done** (hybrid, capability-gated, SMP, preemptive, 28 native + 80+ Linux syscalls)
- AthFS: **done** (mount/format, R/W, CoW, journal, snapshots, refcounting, compression, encryption types)
- AthGFX: **partial** — compositor + `Canvas::draw_triangle` Year-1 demo ships in `user_init`; `vulkan.rs` API exists; GPU/wgpu path not boot-proven
- AthUI hello world: **done** (Label + Button + Frame in a compositor window)
- Vulkan demo: **partial (visual)** — gradient triangle renders to framebuffer via userspace surface; **not** full SPIR-V → GPU submit yet

**You're at roughly Year 1 milestone ~90%.** Kernel, UI, and on-screen triangle demo work in QEMU. Remaining Year-1 graphics gap: wire hello-triangle through `vk_*` / virtio-gpu (or Athena DRM) instead of software raster only.

**Year 2 target**: "Full desktop experience. AthBridge runs 80% of Windows apps. Steam works."

- AthBridge: **comprehensive** (36 DLL modules dispatched, ~300+ Win32/NT functions across kernel32/user32/gdi32/ntdll/ws2_32/msvcrt with real logic, PE loader, D3D9/11/12/DXGI translation, registry hive with Steam/DirectX/VC++/.NET keys)
- AthKit SDK: **done** (declarative UI, state management, navigation, app lifecycle)
- AthStore/AthID/AthSync: **done** (app store, passkey auth, E2E sync)
- AthPlay: **done** (Steam/Epic/GOG/AthStore unified launcher, playtime, achievements)
- Desktop experience: **interactive** (shell rendered, cursor visible, keyboard+mouse input routed, click dispatch for taskbar/start menu/settings/window focus; app launch pending)
- Steam: **not started** (compat test needed — blocker is DXBC→SPIR-V shader translation + real GPU commands)

**Year-2 is ~55% complete.** The Win32 compatibility layer now covers the full core API surface (codepage/locale, file mapping, IOCP, pipes, monitors, raw input, hooks, regions, palettes, SEH/VEH, Winsock async, CRT threading/signals/file I/O). Desktop shell renders at boot with interactive mouse cursor. Remaining: shader compilation, GPU command submission, Steam integration test.
