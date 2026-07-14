# RaeenOS Production Checklist — Consumer-OS Feature Parity

**What this document is:** the *top-down, consumer-OS* readiness lens for RaeenOS. It is organized the way Windows and macOS are actually **experienced and shipped** — by user-facing surface area — and for each surface it names the Windows equivalent, the macOS equivalent, the RaeenOS answer, an honest parity status, and the owning `MasterChecklist.md` phase.

**What this document is NOT:** the live engineering source of truth. That is **`MasterChecklist.md`**, always. This file is a *parity index* — a way to ask "would a person switching from Windows or a Mac find this OS complete?" When this file and `MasterChecklist.md` disagree on status, **`MasterChecklist.md` wins** and this file is stale — fix it. Precedence overall: `RaeenOS_Concept.md` > `MasterChecklist.md` > this file.

> Anti-stale contract: do not turn this into a second backlog. It mirrors status *from* the MasterChecklist and the Concept doc; it does not own it. Every row links the phase that does.

> **Reconciliation note (2026-06-22):** rows across Parts I, III, V, X, XI, XII, XIV were
> synced to the MasterChecklist "Lead-orchestrated non-UI subsystem wave (2026-06-21)" work-log
> (the ~29 dated entries / 986 host KATs across 20 crates). These additions are decode/parse/
> protocol **libraries** proven by host KAT — almost all `[~]` (NOT yet wired into apps or proven
> on iron). Source of truth: `MasterChecklist.md` (top work-log). Nothing here was promoted to
> `[x]` — iron proof is paused this wave. When in doubt, downgrade.

---

## Status ladder (identical to MasterChecklist)

- `[x]` — Done and measurable on the bar that surface demands. For anything touching hardware, `[x]` means **proven on Athena/iron**, not QEMU.
- `[~]` — Partial: infrastructure exists, end-to-end consumer experience not proven.
- `[ ]` — Not started.
- `[N/A]` — Deliberately out of scope for RaeenOS by Concept design (e.g. forced telemetry, ads, registry archaeology).

"Compiles" is never a status. When in doubt, downgrade.

---

## What "production" means for a consumer OS

Windows and macOS are not "done" because the kernel boots — they are done because a non-technical person can **unbox, set up, and live in them for a year** without a terminal. RaeenOS reaches consumer production when it clears three escalating gates, mapped to the Concept's *Three User Experiences*:

| Gate | Persona (Concept §Three User Experiences) | The bar | Today |
|---|---|---|---|
| **G1 — Daily Driver** | Average User | Install, log in, browse files, get on Wi-Fi, change a setting, run an app, sleep/wake, update, shut down. No terminal. | `[ ]` — blocked on real GPU present, live input on iron end-to-end, browser, installer-to-installed-disk |
| **G2 — Switcher** | Custom-PC Builder | Everything in G1 + run their existing Windows apps (RaeBridge), see hardware telemetry, theme the system, manage drivers. | `[ ]` |
| **G3 — Gamer** | Game Station | Everything in G2 + Steam works, a AAA title runs, controller-only living-room flow, VRR/HDR, per-game profiles. | `[ ]` |

These three gates are the consumer-facing restatement of the engineering **Ship Gate** at the bottom of `MasterChecklist.md`. They do not replace it; they translate it into "what a person can do."

Legend for the **Phase** column: links to the owning section in `MasterChecklist.md`.

---

## Part I — Out-of-box experience (first run)

The first 15 minutes. Windows = OOBE; macOS = Setup Assistant.

| Capability | Windows | macOS | RaeenOS answer | Status | Phase |
|---|---|---|---|---|---|
| Bootable install media | USB / ISO | USB / Recovery | `--safe`/UEFI image, `flash-usb.ps1` | `[~]` | 16.2 |
| Graphical setup wizard | OOBE | Setup Assistant | `installer_ui.rs` 7-screen flow | `[~]` | 16.1 |
| Language / region / keyboard pick | ✓ | ✓ | locale stage exists, no graphical picker | `[~]` | 16.1 |
| Network setup during install | ✓ | ✓ | DHCP runs at boot; no install screen | `[ ]` | 16.1 |
| Local account creation | ✓ | ✓ | `create_local_account` + Argon2 hash, iron-proven | `[x]` | 16.1 |
| Cloud account sign-in | Microsoft account | Apple ID | RaeID (passkeys-first, **optional**); WebAuthn EdDSA+ES256 ceremony core host-KAT'd; sign-in UX + server + iron pending | `[~]` | 15.2 |
| Privacy/permission disclosure screen | ✓ | ✓ | telemetry off by default (Concept §Core 2) | `[ ]` | 16.1 |
| Migration from old machine | (weak) | Migration Assistant | not planned for 1.0 | `[ ]` | — |
| Install-to-internal-disk completes & reboots into OS | ✓ | ✓ | full-disk plan bootable in sim; iron end-to-end pending | `[~]` | 3.5 / 16.1 |
| Dual-boot alongside existing OS | ✓ | (Boot Camp, legacy) | GPT carve + boot-entry encoder done; NVRAM `SetVariable` is a bootloader-phase gap | `[~]` | 16.1 |
| First-boot welcome / OOBE on installed system | ✓ | ✓ | `setup_ui` account screen | `[~]` | 16.1 |

---

## Part II — Desktop shell & everyday UX

The thing you stare at all day. Windows = Shell/Explorer (taskbar, Start); macOS = Finder/Dock/menu bar.

| Capability | Windows | macOS | RaeenOS answer | Status | Phase |
|---|---|---|---|---|---|
| Desktop + wallpaper | ✓ | ✓ | compositor desktop; rendered on iron | `[~]` | 14.1 |
| Taskbar / Dock | Taskbar | Dock | RaeShell taskbar | `[~]` | 14.1 |
| App launcher / Start | Start menu | Launchpad | RaeShell start menu | `[~]` | 14.1 |
| System tray / menu bar status | tray | menu bar | tray w/ live RTC clock | `[~]` | 14.1 |
| Clock reads real wall time | ✓ | ✓ | `tray_clock_string` ← `sys_wall_clock`, QEMU-proven | `[~]` | 14.1 |
| Window move/resize/min/max | ✓ | ✓ | compositor multi-window | `[~]` | 14.1 / 8.1 |
| Window snapping / tiling | Snap Layouts | Stage Mgr / tiling | swappable WM (tile/stack/float) | `[~]` | 13.2 |
| Virtual desktops | Virtual Desktops | Spaces | not yet | `[ ]` | 13.2 |
| App switcher | Alt+Tab / Task View | Cmd+Tab / Mission Control | `cycle_alt_tab` | `[~]` | 14.1 |
| Global search | Windows Search | Spotlight | command palette LIVE (`command_palette.rs` `0b026f1`); kernel index still dead-wired | `[~]` | 14.1 |
| Command palette (run actions) | (PowerToys Run) | Spotlight actions | `command_palette.rs` fuzzy cmd/app/settings palette, QEMU-booted | `[~]` | 14.1 |
| Notifications + center | Action Center | Notification Center | `notify.rs` HISTORY_CAP=64 ring + `toggle_center` glass panel (`e92d00c`) | `[~]` | 14.1 |
| Quick settings / Control Center | Quick Settings | Control Center | `notify::quick_settings` 5-toggle strip + DND/Focus (`e92d00c`) | `[~]` | 14.1 |
| Clipboard history + pin | Win+V | (Universal Clipboard) | kernel ring (syscalls 268-273 `bbb3276`) + Super+C glass panel (`4f7fea8`); text-only, RAM-local | `[~]` | 14.1 |
| Widgets | Widgets board | Widgets | Rainmeter-class system (Concept §Customization) | `[ ]` | 13.1 |
| Lock screen + login UI | ✓ | ✓ | `login_ui` renders on iron | `[x]` | 14.1 |
| Login: password / PIN | ✓ | ✓ | password auth round-trips (Argon2) | `[~]` | 9 / 16.1 |
| Login: biometrics | Windows Hello | Touch ID / Face ID | not planned for 1.0 | `[ ]` | — |
| Fast user switching / multi-user | ✓ | ✓ | sessions exist; switching UX pending | `[~]` | 14.1 |
| Glassmorphic / themed compositor | Acrylic/Mica | Vibrancy | glassmorphism + Vibe Mode (Concept §Customization) | `[~]` | 13.1 |
| Live wallpapers | (3rd party) | (limited) | GPU live wallpapers, paused when occluded | `[ ]` | 13.1 |

---

## Part III — File management

Windows = File Explorer; macOS = Finder.

| Capability | Windows | macOS | RaeenOS answer | Status | Phase |
|---|---|---|---|---|---|
| File manager app | Explorer | Finder | `Files` (basic) | `[~]` | 14.2 |
| Tabs / split panes | ✓ | ✓ (tabs) | `apps/files` tabs LIVE (`rae_files::TabSet`); no split yet | `[~]` | 14.2 |
| Fuzzy search in-folder | ✓ | ✓ | kernel `search_index.rs` live (syscalls wired); NOT populated/queried | `[~]` | 14.1 |
| Batch rename | (PowerToys) | ✓ | `apps/files` batch-rename (host-KAT'd `rae_files`) | `[~]` | 14.2 |
| Trash / Recycle Bin | ✓ | ✓ | `apps/files` Trash = CoW move to `.Trash` + restore/empty | `[~]` | 14.2 |
| Removable media mount/eject | ✓ | ✓ | USB-MSC enumerate still blocked on iron | `[~]` | 2.1 |
| Network shares (SMB) | ✓ | ✓ | not planned for 1.0 | `[ ]` | 10 |
| Cloud drive integration | OneDrive | iCloud Drive | `raesync` E2E core (device enroll, wrapped group key, AEAD SyncBlob, LWW-CRDT convergence) host-KAT'd; server + drive UX + iron pending | `[~]` | 15.3 |
| Quick preview | Preview pane | Quick Look | Files Quick Look renders real PNG pixels (`8550989`); BMP/GIF/PNG/JPEG/WebP decoders + `rae_image` dispatcher host-KAT'd, broader-format Quick Look wiring pending | `[~]` | 14.2 |
| Zip / unzip | ✓ | ✓ | lz4/zstd in kernel; `rae_zip` reader+ZipWriter + `rae_tar` .tar.gz writer host-KAT'd; UI wiring pending | `[~]` | 5.4 |
| File associations / default apps | ✓ | ✓ | not yet | `[ ]` | 14.2 |
| Per-app data isolation | (none) | (containers) | RaeFS per-app buckets (Concept §RaeFS) | `[~]` | 5.6 |

---

## Part IV — Settings & system management

Windows = Settings + (legacy) Control Panel; macOS = System Settings. Concept §Windows Pain Points explicitly demands **one** unified, searchable Settings — no split.

| Capability | Windows | macOS | RaeenOS answer | Status | Phase |
|---|---|---|---|---|---|
| Unified, searchable Settings app | Settings (split w/ Control Panel) | System Settings | `Settings` (basic); catalog in `docs/SETTINGS_CATALOG.md` | `[~]` | 14.2 |
| Display: resolution / scaling | ✓ | ✓ | EDID parse `[~]`; no settings UI | `[~]` | 2.3 |
| Display: multi-monitor | ✓ | ✓ | single FB today | `[ ]` | 1.2 / 2.3 |
| Display: HDR | ✓ | ✓ | Concept §RaeGFX first-class; not built | `[ ]` | 6.4 |
| Display: refresh / VRR | ✓ | ✓ (ProMotion) | compositor VRR target | `[ ]` | 6.4 / 12.1 |
| Display: night light | ✓ | Night Shift | not yet | `[ ]` | 13.1 |
| Sound device + per-app volume | ✓ | ✓ | HDA playback iron-proven; no mixer UI | `[~]` | 2.6 / 7 |
| Network settings UI | ✓ | ✓ | DHCP works; no UI; Wi-Fi unproven | `[~]` | 10 / 2.2 |
| Bluetooth + pairing | ✓ | ✓ | not built | `[ ]` | — |
| Power & battery | ✓ | ✓ | fuel-gauge iron-proven; no plans UI | `[~]` | 2.4 / 4.7 |
| Printers & scanners | ✓ | ✓ | not planned for 1.0 | `[ ]` | — |
| Keyboard / mouse / trackpad | ✓ | ✓ | per-device profiles (Concept §Customization) | `[ ]` | 13.3 |
| Date/time / language / locale | ✓ | ✓ | RTC + wall clock `[x]`; locale UI pending | `[~]` | 1.6 / 16.1 |
| Users & accounts | ✓ | ✓ | account create `[x]`; mgmt UI pending | `[~]` | 16.1 |
| Privacy & per-app permissions | ✓ | ✓ | capability syscalls `[x]`; permission UI `[~]` | `[~]` | 9.2 |
| Update settings + history | ✓ | ✓ | atomic A/B slots `[~]`; no UI | `[~]` | 3.6 |
| Storage management | ✓ | ✓ | RaeFS exists; no UI | `[~]` | 5 |
| About / system info | ✓ | ✓ | `/proc/raeen/*` exists; no UI | `[~]` | 0 |
| Backup & restore | Windows Backup | Time Machine | RaeFS snapshots + one-click rollback iron-proven | `[~]` | 5.1 |
| Recovery / reset | Reset PC / WinRE | Recovery | safe mode exists; recovery env `[ ]` | `[~]` | 4.10 / 16 |
| RGB / fan / overclock unified | (vendor sprawl) | (none) | one RGB API + fan curves (Concept §Customization) | `[~]` | 13.3 |
| **No ads / no forced telemetry** | (present) | (some) | forbidden by design (Concept §Core 2) | `[N/A]` | 18 |

---

## Part V — Built-in apps (the default install)

What ships in the box. Concept §Windows Pain Points: "Default install is OS only" — *no bloatware*, but the **essentials** still have to exist.

| App | Windows | macOS | RaeenOS answer | Status | Phase |
|---|---|---|---|---|---|
| Web browser | Edge | Safari | raeweb HTML/CSS + a from-scratch JS engine (`rae_js`: parse+execute+collections+RegExp+Promise/event-loop) host-KAT'd; layout/render + DOM bindings + app-wiring pending | `[~]` | 14.2 |
| File manager | Explorer | Finder | Files (basic) | `[~]` | 14.2 |
| Settings | Settings | System Settings | Settings (basic) | `[~]` | 14.2 |
| Terminal | Terminal / PowerShell | Terminal.app | full VT100/xterm emulator, SGR-color fixed | `[~]` | 14.2 |
| Text editor | Notepad | TextEdit | exists (basic) | `[~]` | 14.2 |
| Calculator | ✓ | ✓ | `apps/calculator` over `rae_calc` (17 KATs) | `[~]` | 14.2 |
| Photos / viewer | Photos | Photos | `apps/photos`; full decode stack BMP/GIF/PNG/JPEG/WebP-VP8L + PNG/JPEG encode + `rae_image` dispatcher, all host-KAT'd; app-wiring (beyond PNG-in-Files) + iron pending | `[~]` | 14.2 |
| Media player | Media Player | Music / QuickTime | `apps/music`; WAV/FLAC/MP3/AAC/Opus decoders host-KAT'd (MP3+AAC audible on host), rae_mp4 demux; player open/decode path done; SCHED_GAME GameMixer; iron HDA + app-wiring pending | `[~]` | 7 / 14.2 |
| Screenshot / screen record | Snipping Tool | Screenshot | compositor capture (Concept §Gaming) | `[ ]` | 12.2 |
| System monitor | Task Manager | Activity Monitor | `/proc/raeen/*` data exists; no app | `[~]` | 14.2 |
| Disk utility | Disk Management | Disk Utility | installer GPT/RaeFS logic exists; no app | `[~]` | 3 |
| App store client | Microsoft Store | App Store | RaeStore (shell only) | `[~]` | 15.1 |
| Clock / alarms | ✓ | ✓ | `apps/clock` (clock/alarms/timer/stopwatch/calendar) | `[~]` | 14.2 |
| Notes | ✓ | Notes | `apps/notes` (md edit + live rae_markdown preview) | `[~]` | 14.2 |
| Office docs (Word/Excel/PDF) | Office / WordPad | Pages/Numbers/Preview | `rae_docx`/`rae_xlsx`/`rae_pdf` read + DOCX/XLSX write + XLSX formula compute + `rae_print` PDF-1.7 generator, all host-KAT'd; app UI + iron pending | `[~]` | 14.2 |
| Mail / Calendar | ✓ | ✓ | `rae_mail` (SMTP/IMAP/POP3 + RFC822/MIME) + `rae_pim` (iCal/vCard parse + RRULE + POSIX TZ) host-KAT'd; live TLS/TCP wiring + app UI + iron pending | `[~]` | 14.2 |
| Camera | ✓ | Photo Booth | no webcam stack | `[ ]` | — |

---

## Part VI — Graphics & display stack

Windows = DWM + WDDM; macOS = WindowServer + Metal. This is RaeenOS's biggest open Year-1 deliverable.

| Capability | Windows | macOS | RaeenOS answer | Status | Phase |
|---|---|---|---|---|---|
| GPU driver (real submit) | WDDM | Metal/IOKit | LinuxKPI userspace (amdgpud/i915d) + Mesa | `[ ]` | 6 |
| Hardware-accelerated compositor | DWM | WindowServer | compositor on SW raster today | `[~]` | 6 |
| `vkQueueSubmit`-equivalent path | ✓ | ✓ | **software raster only** — the open Year-1 gap | `[ ]` | 6.3 |
| Direct-to-display scanout | ✓ | ✓ | planned | `[ ]` | 6.3 |
| Multi-monitor | ✓ | ✓ | single FB | `[ ]` | 2.3 |
| HDR | ✓ | ✓ | Concept first-class; not built | `[ ]` | 6.4 |
| VRR (FreeSync/G-Sync) | ✓ | ProMotion | compositor VRR target | `[ ]` | 12.1 |
| DPI / fractional scaling | ✓ | ✓ (Retina) | not built | `[ ]` | 8.1 |
| Color management (ICC) | ✓ | ColorSync | not built | `[ ]` | — |
| Compositor capture/record | ✓ | ✓ | zero-cost capture (Concept §Gaming) | `[ ]` | 12.2 |
| Skia + wgpu UI render | (D2D/DWrite) | (CoreGraphics) | RaeUI on Skia+wgpu (Concept §Language Stack) | `[~]` | 8.1 |

---

## Part VII — Audio

Windows = WASAPI/Audio Engine; macOS = CoreAudio. Concept §RaeAudio: sub-3ms round-trip.

| Capability | Windows | macOS | RaeenOS answer | Status | Phase |
|---|---|---|---|---|---|
| Hardware codec playback | ✓ | ✓ | real HDA PCM iron-proven (`hda_playback=1`) | `[x]` | 2.6 |
| Full codec topology walk | ✓ | ✓ | output-pin widget detection fails on Athena codec | `[~]` | 2.6 |
| Output/input device mgmt | ✓ | ✓ | not built | `[ ]` | 7.2 |
| Per-app volume + routing | ✓ | ✓ | VoiceMeeter-class native (Concept §Pro Gaming) | `[ ]` | 7.2 |
| Low-latency / pro path | ASIO | CoreAudio | SCHED_GAME mix thread, <3ms target | `[ ]` | 7.2 |
| Bluetooth audio | ✓ | ✓ | no BT stack | `[ ]` | — |
| System sounds | ✓ | ✓ | not built | `[ ]` | 7 |
| Hotplug (jack detect) | ✓ | ✓ | not built | `[ ]` | 2.6 |

---

## Part VIII — Input & devices

| Capability | Windows | macOS | RaeenOS answer | Status | Phase |
|---|---|---|---|---|---|
| USB keyboard | ✓ | ✓ | HID keyboard armed on iron (Razer); live-typing test pending | `[~]` | 2.1 |
| USB mouse / pointer | ✓ | ✓ | HID wired; iron cursor pending next flash | `[~]` | 2.1 |
| Keyboard layouts / IME | ✓ | ✓ | not built | `[ ]` | 16.1 |
| Emoji / special char picker | ✓ | ✓ | not built | `[ ]` | — |
| Trackpad gestures | ✓ | ✓ (best-in-class) | not built | `[ ]` | 13.3 |
| Game controllers (full feature) | ✓ | (partial) | DualSense/Xbox parse `[x]`; haptics/gyro/adaptive triggers (Concept §Gaming) | `[~]` | 12.2 |
| USB device hotplug | ✓ | ✓ | xHCI enumerates; hub-child probes time out on iron | `[~]` | 2.1 |
| USB mass storage | ✓ | ✓ | never enumerates on iron (open) | `[~]` | 2.1 |
| Bluetooth devices | ✓ | ✓ | no BT stack | `[ ]` | — |
| Printers / scanners | ✓ | ✓ | out of scope | `[ ]` | — |
| Webcam | ✓ | ✓ | out of scope | `[ ]` | — |
| Pen / touch | ✓ | (iPad) | out of scope | `[ ]` | — |

---

## Part IX — Networking

Windows = Network stack + WLAN AutoConfig; macOS = Network framework. Concept §RaeNet.

| Capability | Windows | macOS | RaeenOS answer | Status | Phase |
|---|---|---|---|---|---|
| Wired Ethernet + DHCP | ✓ | ✓ | RTL8125 TX+RX iron-proven; DHCP→Bound pending capture | `[~]` | 2.2 / 10 |
| Wi-Fi (scan/connect/WPA2-3) | ✓ | ✓ | AX210 via LinuxKPI; not built | `[ ]` | 2.2 |
| Static IP / DNS config | ✓ | ✓ | DNS resolve wired (`SYS_NET_DNS`) | `[~]` | 10.2 |
| TCP/UDP sockets API | ✓ | ✓ | socket syscalls 121–125 | `[~]` | 10.2 |
| TLS 1.3 | ✓ | ✓ | full RFC 8446 client handshake + cert-chain validation + CertVerify + hostname binding, fail-closed (MITM-safe), host-KAT'd 105/105; live socket integration + iron pending | `[~]` | 10.2 |
| VPN | built-in + 3rd party | ✓ | WireGuard Noise handshake `[x]`; full tunnel pending | `[~]` | 10.2 |
| Firewall | Defender Firewall | App Firewall | firewall smoketest 7/7 iron | `[~]` | 10.2 |
| File/printer sharing | SMB | SMB / AirDrop | not planned for 1.0 | `[ ]` | — |
| Captive portal handling | ✓ | ✓ | not built | `[ ]` | 10 |
| Hotspot / tethering | ✓ | ✓ | not built | `[ ]` | — |
| Gaming traffic shaping / QUIC priority | (none) | (none) | QUIC + shaper smoketests iron (Concept §RaeNet) | `[~]` | 10.2 |

---

## Part X — Security & privacy

Windows = Defender + BitLocker + SmartScreen + Hello; macOS = Gatekeeper + FileVault + XProtect + Keychain. Concept §Security Model: "iOS-grade security without iOS-grade lockdown."

| Capability | Windows | macOS | RaeenOS answer | Status | Phase |
|---|---|---|---|---|---|
| Secure boot chain | ✓ | ✓ | boot-chain verify + replay iron | `[~]` | 3.7 |
| Full-disk encryption | BitLocker | FileVault | RaeFS FDE; TPM-backed where available | `[~]` | 3.8 |
| TPM-backed keys | ✓ | (Secure Enclave) | not wired | `[ ]` | 3.8 |
| Capability-based permissions | (limited) | (TCC) | `Cap` enum enforced at syscall layer `[x]` | `[x]` | 9 |
| App sandboxing by default | (partial) | (App Sandbox) | sandbox smoketests iron; fail-open for bring-up | `[~]` | 9.2 |
| Per-app permission prompts | ✓ | ✓ | permission UI `[~]` | `[~]` | 9.2 |
| Code signing / notarization | SmartScreen | Gatekeeper | Ed25519 bundle signing; "unverified dev" UX | `[~]` | 9.3 |
| Malware protection | Defender | XProtect | structural (caps) vs scanner; design choice | `[~]` | 9 |
| Credential / password manager | Credential Mgr | Keychain | `rae_keychain` (argon2id master-key KDF + chacha20poly1305 AEAD, fail-closed, zeroized) host-KAT'd; OS-integration + UI + iron pending | `[~]` | 15.2 |
| Passkeys / 2FA | Windows Hello / Authenticator | Passkeys | `raeid` WebAuthn (EdDSA + ES256) + `rae_otp` HOTP/TOTP host-KAT'd vs RFC vectors; authenticator UX + iron pending | `[~]` | 15.2 |
| Ransomware resistance | (partial) | (partial) | per-app FS buckets (Concept §Security) | `[~]` | 5.6 |
| Driver sandboxing (IOMMU) | (partial) | (DriverKit) | IOMMU tables iron; **enforcement** pending | `[~]` | 4.2 |
| Parental controls / screen time | ✓ | ✓ | out of 1.0 scope | `[ ]` | — |
| Memory tagging | (coming) | (coming) | tracked as CPUs ship it | `[ ]` | — |

---

## Part XI — Updates & lifecycle

Concept §Core 2 + §Windows Pain Points: **user controls update timing, always**; atomic CoW updates with one-click rollback; no forced restarts.

| Capability | Windows | macOS | RaeenOS answer | Status | Phase |
|---|---|---|---|---|---|
| OS update mechanism | Windows Update | Software Update | atomic A/B kernel slots | `[~]` | 3.6 |
| Atomic / never-half-apply | (no — can brick) | (better) | CoW atomic; one-click rollback iron | `[~]` | 3.6 / 5.1 |
| User-controlled timing (no forced reboot) | (forced) | (nags) | by design (Concept §Core 2) | `[~]` | 3.6 |
| Rollback after bad update | (System Restore) | (limited) | RaeFS snapshot rollback iron | `[~]` | 5.1 |
| App updates | Store | App Store | RaeStore auto-update w/ consent; `RaeUpdate` verified transactional A/B delta engine (SHA-256+Ed25519 verify-before-apply, atomic flip, auto-rollback) host-KAT'd; UI + iron pending | `[~]` | 15.1 / 3.6 |
| Driver updates | Windows Update | (bundled) | signed driver pipeline | `[ ]` | — |
| Update channels | Insider | beta | stable/beta/nightly | `[ ]` | 16.2 |
| Delta / efficient updates | ✓ | ✓ | not built | `[ ]` | 3.6 |
| Telemetry (opt-in only) | (on by default) | (some) | off by default (Concept §Core 2) | `[N/A]` | 18 |

---

## Part XII — App ecosystem & compatibility

The actual moat. Concept §Compatibility Strategy: this is "how to actually win."

| Capability | Windows | macOS | RaeenOS answer | Status | Phase |
|---|---|---|---|---|---|
| Native app SDK | Win32/WinUI | Cocoa/SwiftUI | RaeKit (Rust, declarative) | `[~]` | 8.2 |
| App package format | MSIX/exe | .app/.pkg | `.raepkg` bounds-checked TLV codec + fail-closed Ed25519 `parse_and_verify` host-KAT'd; hashed deps | `[~]` | 15.1 |
| App store | Microsoft Store | App Store | RaeStore (12% share, sideload OK); transactional dependency-correct install/uninstall+GC host-KAT'd; client UI + iron pending | `[~]` | 15.1 |
| Sideloading | ✓ | (Gatekeeper) | allowed + supported (Concept) | `[ ]` | 15.1 |
| **Windows app compatibility** | (native) | (none) | **RaeBridge** (Wine/Proton lineage) | `[~]` | 11 |
| Win32 ABI / loader | ✓ | — | LDR/TEB/PEB host-KAT'd 33/33; x64 marshaling/SEH/TLS pending | `[~]` | 11.2 |
| DirectX → native translation | ✓ | — | DXVK/VKD3D lineage → RaeGFX | `[ ]` | 11.2 |
| **Steam day one** | ✓ | (limited) | **non-negotiable** (Concept §Gaming); not yet | `[ ]` | 11.3 |
| Linux app support | WSL | (none) | POSIX layer; relibc native apps run | `[~]` | 0 / 11 |
| Web apps / PWA | ✓ | ✓ | `rae_pwa` W3C manifest parse + URL resolution + InstallDescriptor host-KAT'd; render through RaeUI + launch wiring pending | `[~]` | — |
| Default-app / uninstall mgmt | ✓ | ✓ | not built | `[ ]` | 14.2 |

---

## Part XIII — Power & thermal

| Capability | Windows | macOS | RaeenOS answer | Status | Phase |
|---|---|---|---|---|---|
| Sleep (S3) + wake | ✓ | ✓ | not built | `[ ]` | 2.4 |
| Hibernate | ✓ | ✓ | not built | `[ ]` | 2.4 |
| Wake under 1s (Concept §Core 3) | (varies) | ✓ | target; gated on S3 | `[ ]` | 2.4 |
| Battery management + health | ✓ | ✓ | fuel-gauge iron-proven; no UI | `[~]` | 2.4 |
| Power plans / modes | ✓ | ✓ | CPPC read on iron; no plans | `[~]` | 4.7 |
| Thermal throttling | ✓ | ✓ | not proven on iron | `[ ]` | 4.7 |
| Fan curves | (vendor) | (auto) | OS-level (Concept §Customization) | `[~]` | 13.3 |
| Lid-close / idle behavior | ✓ | ✓ | not built | `[ ]` | 2.4 |

---

## Part XIV — Performance & gaming (the wedge)

This is where RaeenOS must *beat* Windows/macOS, not match them. Concept §Gaming-First: "where RaeenOS wins or doesn't ship."

| Capability | Windows | macOS | RaeenOS answer | Status | Phase |
|---|---|---|---|---|---|
| Boot < 6s on NVMe (target 3s) | (slow) | ✓ | iron T0→userspace ~3.1s; **total ~11s, over budget — open** | `[~]` | 1 |
| Real-time game scheduling | Game Mode (weak) | (none) | **SCHED_GAME** hard deadlines; EDF + deadline-miss telemetry (`/proc/raeen/perf`, verifier-PASS); iron game-thread proof pending | `[~]` | 12.1 |
| Background throttling in-game | (weak) | (n/a) | "nothing else gets meaningful CPU" (Concept) | `[ ]` | 12.1 |
| Sub-frame input latency | (varies) | (varies) | OS adds <1 frame; not yet instrumented | `[ ]` | 12.1 |
| Game bar / overlay | Xbox Game Bar | (none) | native overlay: FPS, frametime, temps | `[ ]` | 12.2 |
| Per-game profiles | (3rd party) | (none) | res/refresh/audio/GPU-limit per game | `[~]` | 12.2 |
| GameOS / Big Picture mode | (none) | (none) | GameOS couch shell; F11 toggle | `[~]` | 14.3 |
| Library aggregator (Steam/Epic/GOG) | (none) | (none) | RaePlay unified | `[ ]` | 14.3 |
| Direct-to-GPU fullscreen | ✓ | ✓ | RaeGFX exclusive path | `[ ]` | 6 / 12.1 |
| Shader cache (OS-level) | (per-driver) | (none) | shared, persistent across reinstalls | `[ ]` | 12.1 |
| Capture & stream (zero-cost) | (Game Bar) | (none) | at compositor | `[ ]` | 12.2 |
| Anti-cheat (EAC/BattlEye) | kernel AC | (limited) | RaeShield attestation + sanctioned per-game kernel AC | `[ ]` | 9 / 18 |
| NULL_LATENCY competitive mode | (Reflex, app) | (none) | pure direct-input pipeline (Concept §Pro) | `[ ]` | 12.3 |

---

## Part XV — Reliability & serviceability

A consumer OS survives a year of abuse. Windows = WinRE/SFC/Reliability Monitor; macOS = Recovery/safe boot.

| Capability | Windows | macOS | RaeenOS answer | Status | Phase |
|---|---|---|---|---|---|
| Graceful crash handling | BSOD + recover | panic + reboot | driver crash ≠ kernel crash (Concept) | `[~]` | 4 |
| Crash dump + report | ✓ | ✓ | crash-dump region iron; reporting `[ ]` | `[~]` | 4.5 |
| Safe mode | ✓ | ✓ | `--safe` boot path | `[~]` | 4.10 |
| Recovery environment | WinRE | Recovery | not built | `[ ]` | 16 |
| System restore points | ✓ | Time Machine | RaeFS snapshots | `[~]` | 5.1 |
| Event / log viewer | Event Viewer | Console | `/proc/raeen/*` + bootlog | `[~]` | 0 |
| 24h soak stability | (assumed) | (assumed) | not run | `[ ]` | 4.9 |
| Machine-check / hardware-error handling | ✓ | ✓ | AER caps + SMCA banks iron; real handlers `[ ]` | `[~]` | 4.3 / 4.4 |
| Watchdog | ✓ | ✓ | AMD-EFCH watchdog proven iron | `[~]` | 4.6 |

---

## Part XVI — Localization & accessibility

Ship-blocking for a mass-market OS; both Windows and macOS treat these as non-optional.

> **STALE accessibility rows (flag, 2026-06-21):** the three a11y rows below ("Screen reader",
> "Magnifier / zoom", "High contrast / color filters") are marked "not built" / `[ ]` — that is
> NO LONGER TRUE. Verified in source: the screen-reader announce core, magnifier engine, color
> filters, and a LIVE high-contrast palette swap are built, with user on-switches (hotkeys +
> Control Center). Current state: `[~]` (QEMU-proven, iron-unproven). Authoritative:
> `docs/research/accessibility-audit-2026-06-21.md` + `Audit.md` (Accessibility section). Status
> cells left for the MasterChecklist owner to re-stamp (this checklist mirrors MasterChecklist);
> do not read these three rows as current.

| Capability | Windows | macOS | RaeenOS answer | Status | Phase |
|---|---|---|---|---|---|
| Multiple UI languages | ✓ | ✓ | not built | `[ ]` | 14 |
| RTL layout | ✓ | ✓ | cosmic-text BiDi available; not wired | `[ ]` | 8.1 |
| CJK input methods (IME) | ✓ | ✓ | not built | `[ ]` | 16.1 |
| Region/locale number/date formats | ✓ | ✓ | not built | `[ ]` | 16.1 |
| Screen reader | Narrator | VoiceOver | not built | `[ ]` | 14 |
| Magnifier / zoom | ✓ | ✓ | not built | `[ ]` | 14 |
| High contrast / color filters | ✓ | ✓ | not built | `[ ]` | 13.1 |
| Captions | ✓ | ✓ | not built | `[ ]` | — |
| Sticky/slow keys, key repeat | ✓ | ✓ | not built | `[ ]` | 14 |
| Voice control | ✓ | ✓ | not built | `[ ]` | — |

> Accessibility was the **largest unowned gap** versus Windows/macOS parity. It now has a home: **MasterChecklist Phase 19 — Accessibility** (founded on AccessKit). Localization/IME remains its sibling gap, tracked alongside Phase 19.

---

## Part XVII — Enterprise & management (post-1.0)

Windows and macOS both ship this; RaeenOS 1.0 is consumer-first and may defer most of it. Listed for completeness so it isn't mistaken for parity.

| Capability | Windows | macOS | RaeenOS answer | Status | Phase |
|---|---|---|---|---|---|
| Device management (MDM) | Intune | MDM | out of 1.0 scope | `[ ]` | 18 |
| Directory / domain join | AD/Entra | (limited) | out of scope | `[ ]` | — |
| Policy / config profiles | Group Policy | Profiles | versioned config exists (Concept §RaeFS) | `[~]` | 5.7 |
| Remote desktop | RDP | Screen Sharing | out of 1.0 scope | `[ ]` | — |
| Enterprise licensing | ✓ | ✓ | business model defined (Concept) | `[ ]` | 18 |

---

## Consumer Production Gates — the actual ship criteria

These translate the engineering Ship Gate (`MasterChecklist.md`) into "what a person can do." Each gate is **all-or-nothing**: every line must be `[x]` (iron-proven where hardware is involved) to declare the gate met.

### G1 — Daily Driver (Average User can live in it)
- [ ] Flash USB → boot Athena → graphical installer → install to internal disk → reboots into installed OS (Part I)
- [ ] Log in with password at the lock screen on iron (Part II, live HID)
- [ ] Desktop composites on **real GPU** (not SW raster) with a live mouse cursor (Part VI)
- [ ] Get on Wi-Fi or Ethernet, resolve DNS, load a page (Part IX + browser, Part V)
- [ ] Open Files, browse RaeFS, open Settings, change wallpaper — no terminal (Parts III–IV)
- [ ] Run a native app and a built-in app; notifications appear (Parts II, V, XII)
- [ ] Sleep and wake under 1s; battery reports correctly (Part XIII)
- [ ] Take an OS update and roll it back (Part XI)
- [ ] Shut down cleanly; reboot; no data loss; 24h soak passes (Part XV)

### G2 — Switcher (Custom-PC Builder brings their software)
- [ ] Everything in G1
- [ ] Install and run a real Windows app via RaeBridge (Part XII)
- [ ] See live CPU/GPU temps, fan speed, RGB control in one place (Parts IV, XIV)
- [ ] Apply a theme / Vibe Mode; swap window manager (Part II)
- [ ] Manage drivers and permissions from Settings (Parts IV, X)

### G3 — Gamer (Game Station)
- [ ] Everything in G2
- [ ] Steam installs and runs (Part XII) — **non-negotiable**
- [ ] One AAA title runs at playable parity with VRR/HDR (Parts VI, XIV)
- [ ] Controller-only living-room flow: power on → GameOS → launch a game (Part XIV)
- [ ] Game bar overlay + per-game profile + capture work (Part XIV)
- [ ] EAC/BattlEye path validated on at least one title (Part XIV)

### Cross-cutting release blockers (apply to all gates)
- [ ] Boot total ≤ 6s on iron (Concept §Core 3 — currently ~11s, **open**)
- [ ] No forced telemetry, no ads, no bundled junk — verified in the shipped image (Concept §Core 2)
- [ ] At least 4 SKUs on the RaeReady certified list (MasterChecklist Phase 17)
- [ ] Accessibility baseline: screen reader + magnifier + high contrast (Part XVI) — *currently the largest unowned gap*
- [ ] Localization: at least one non-English UI language end-to-end (Part XVI)

> When all three gates are `[x]` and the cross-cutting blockers clear, the consumer-facing OS is at parity for its target personas. The version number still follows the engineering Ship Gate in `MasterChecklist.md` — this document just tells you whether a *human* would call it finished.

---

## How to use this file

1. **Don't update status here first.** Land it in `MasterChecklist.md` (with the phase's own boot/iron proof), then mirror the row here.
2. **Read it as a gap-finder.** Rows that are `[ ]` with no MasterChecklist phase are *unowned parity gaps* — accessibility, localization, browser, Bluetooth, printing, color management are the current standouts. Raise them into the MasterChecklist before they become 1.0 surprises.
3. **The Concept doc still wins.** Where Windows/macOS do something RaeenOS deliberately refuses (ads, forced telemetry, registry sprawl, walled garden), the cell is `[N/A]` — that's a feature, not a gap.

---

*"Built for people who care about how things feel." — parity is the floor; the wedge in Part XIV is how it wins.*
