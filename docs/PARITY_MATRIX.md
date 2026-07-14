# RaeenOS Parity Matrix — per-capability deep table

Owner: raeen-parity. This is the EXTENSIVE per-capability companion to
`PRODUCTION_CHECKLIST.md` (which holds the 3 Consumer Production Gates + the
top-down summary). This file holds the deep "best-of-breed + improve" target per
capability and the honest RaeenOS status.

Precedence on any conflict: `RaeenOS_Concept.md` > `MasterChecklist.md` >
`PRODUCTION_CHECKLIST.md` > this file. Status here MIRRORS MasterChecklist; it
never invents a greener status than the code can prove.

Status ladder (identical to MasterChecklist): `[x]` iron-proven / `[~]` partial
or QEMU-only / `[ ]` not started / `[N/A]` out of scope by Concept design.

"Best" = the platform that does it best today + why. "Target" = best-of-breed
then the RaeenOS improvement. "Status" = have / partial / missing with the code
evidence (crate or file) that justifies it.

Last full pass: 2026-06-17 (PRODUCTION POLISH PUSH waves 3-7; iron paused, QEMU == [~]).
Closed waves 1-2 (QEMU-only `[~]`): command palette (B, `0b026f1`), clipboard
history ABI + panel (E, `bbb3276`+`4f7fea8`), Files Quick Look real PNG pixels (C,
`8550989`), Notification Center + Quick Settings (D, `e92d00c`).
Closed waves 3-7 (all QEMU/host-KAT `[~]`, iron paused):
- Media pillar: raemedia from-scratch PNG/JPEG/WAV decoders + EXIF (165 KATs,
  `461985e`/`720e99e`/`f836954`/`89cce24`) -> Photos app (`720e99e`) + Music app
  (`f836954`); PNG encoder -> screenshots save as real `.png` (`89cce24`).
- Accessibility SUITE: a11y tree -> screen-reader announce (`1569ed6`, Phase 19.2)
  -> compositor magnifier (`8b7b2f4`, §3) -> focus-follows pan (`30a577e`, 19.3),
  banked wave 7 (`4a91c43`); a11y ABI 277-278 (`Cap::Accessibility`-gated).
- Screen-capture ABI 274-276 (`Cap::ScreenCapture`-gated, safe-mode-refused).
- Web/PWA foundation (all host-KAT'd pure libs): raenet HTTP/1.1 client (`1a4be7c`)
  + live TCP/DNS transport (`253467a`); rae_json RFC 8259 (`b559025`); rae_pwa W3C
  manifest (`1b67e01`); rae_markdown CommonMark subset (`f900146`).
In flight (concurrent session owns; do NOT plan into): GPU SDMA submit
(`4c773c6`), GameOS couch + Game Bar (`af7c925`/`08279c6`), screenshot tool
(`0c4abb2`), settings redesign (`86d36c2`/`d190f8c`), raeweb engine (`0cd527e`).
Overview/Spaces (A): `virtual_desktops.rs` module still NOT instantiated in live
`DesktopShell`; stays honest `[~]`/`[ ]`.

---

## A. Window management / multitasking

| Capability | Best today + why | Target (best-of-breed + improve) | RaeenOS status | Evidence |
|---|---|---|---|---|
| Snap / tile layouts | Win11 Snap Layouts (hover-to-zone, named) | zone editor + tile/stack/float swap as POLICY over one compositor | `[~]` partial | `kernel/src/wm_policy.rs` computes origins; NO client resize (windows placed at cell origin, keep size); no zone editor, no hover UI |
| Snap Groups / restore | Win11 Snap Groups | restore a saved window set per-display | `[ ]` missing | none |
| Overview / expose | macOS Mission Control | live thumbnails + drag-between-spaces, GPU-cheap | `[ ]` missing (IN FLIGHT) | `raeshell/animations.rs` has a `DesktopOverview` action enum but no live thumbnail/compositor mechanism yet |
| Virtual desktops / Spaces | macOS Spaces, Win Virtual Desktops | per-monitor workspaces + per-space wallpaper | `[ ]` missing (IN FLIGHT) | `raeshell/virtual_desktops.rs` (1334 lines) exists as a module + `WorkspaceSwitch` animation, but is NOT instantiated in live `DesktopShell` (declared `pub mod` only, same dead-code pattern as the search index); compositor has no per-space scanout/workspace concept |
| Stage-Manager-style grouping | macOS Stage Manager | optional auto-grouping, off by default | `[ ]` missing | none |
| App switcher | macOS Cmd-Tab / Win Alt-Tab | live previews, type-to-filter | `[~]` partial | `raeshell` cycle_alt_tab; lead added live-thumbnail switcher chrome (concurrent-owned shell render); no type-to-filter |

## B. Search & launch

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| Instant local search | macOS Spotlight (indexed, instant, math/units) | sub-100ms index across files+apps+settings | `[~]` partial | TWO indexes: (1) DEAD raeshell `search_indexer` (TF-IDF/trigram/calc, never instantiated); (2) LIVE kernel `search_index.rs` w/ full syscall surface (add/remove/query/stats, wired+smoketested) but NOTHING POPULATES IT and nothing QUERIES it yet. Live Start search = app-name substring only. THE GAP IS WIRING, not engine |
| Command palette (run actions) | rofi / VSCode palette / mac Spotlight actions | one palette runs ACTIONS, not just finds files | `[~]` partial | `raeshell/command_palette.rs` LIVE (instantiated in `DesktopShell` line 1597, rendered line 1787; fuzzy match + command/app registry + seeded settings-actions). QEMU-booted `0b026f1`. GAP: not wired to `search_indexer` for file content; iron unproven |
| Web/answer in search | Spotlight, Win Search | optional, privacy-respecting | `[ ]` missing | transport now exists (raenet HTTP/1.1 + live TCP/DNS, `1a4be7c`/`253467a`) but no search-bar web action wired |

## C. File management

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| Tabs / split panes | Win11 Explorer tabs + mac Finder tabs | tabs + split + dual-pane | `[~]` partial | TABS LIVE: `apps/files` uses `rae_files::TabSet` (host-KAT'd tab/history model; up to 8 tabs, +/close, per-tab back/forward, line ~650/744/1870). GAP: no split / dual-pane yet; iron-unproven |
| Fuzzy in-folder search | Finder / Explorer | wired to the shared index | `[~]` partial | TWO indexes exist: raeshell `search_indexer` is DEAD (declared, never instantiated) AND a LIVE kernel `kernel/src/search_index.rs` (init+smoketest in main.rs 1095; syscalls add/remove/query/stats wired in `syscall.rs` 1908-1926; procfs stats). GAP: nothing POPULATES the kernel index (no RaeFS crawler) and Files/palette don't QUERY it |
| Quick Look / preview | macOS Quick Look (spacebar) | spacebar preview for img/text/pdf/media | `[~]` partial | `apps/files` Quick Look renders REAL PNG pixels via `raemedia` (`8550989`); raemedia now also decodes JPEG (`461985e`) + EXIF orientation (`720e99e`) + WAV, so a wired QuickLook can show jpeg too. GAP: Files app not yet calling jpeg/EXIF path; no text/pdf/video; no spacebar gesture; iron-unproven |
| Batch rename | macOS Finder rename | pattern + counter + find/replace | `[~]` partial | LIVE: `apps/files` batch-rename dialog over host-KAT'd `rae_files::batch_rename_target` (pattern + counter, line 1735/1768/1796). GAP: no find/replace mode; iron-unproven |
| Trash / Recycle Bin | both | undoable delete + restore | `[~]` partial | LIVE: `apps/files` Trash = CoW move into `<home>/.Trash` bucket via `SYS_RENAME` (trash/restore/empty over host-KAT'd `rae_files::{trash_target,restore_target}`, line 995-1038). GAP: no global undo, no auto-purge policy; iron-unproven |
| Tags / smart folders | macOS Finder | saved searches + tags | `[ ]` missing | none |
| File associations / default apps | both | per-type default + open-with | `[ ]` missing | confirmed: no default-app/open-with registry in `apps/`, `raeshell`, `raestore`, or kernel. Files app has no "open with" path; double-click has no MIME->app resolution. Daily-driver blocker (can't open a downloaded file by clicking it) |
| Removable media mount/eject | both | auto-mount + eject | `[~]` partial | USB-MSC enumerate blocked on iron |
| Per-app data buckets | RaeFS (Concept differentiator) | isolation by default | `[~]` partial | `kernel/src/raefs.rs` buckets |

## D. Notifications & focus

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| Toasts | both | grouped, glass, urgency | `[~]` partial | `kernel/src/notify.rs` (TTL toasts, MAX_VISIBLE=3) |
| Notification center / history | macOS Notification Center | persistent grouped history panel | `[~]` partial | `notify.rs` now has a `HISTORY_CAP=64` retained ring (`record_history`), grouped, with `toggle_center` glass pull-down (dismiss-one / clear-all). QEMU-booted `e92d00c`. GAP: iron-unproven, no scroll/grouping-by-app UI polish |
| Actionable notifications | both (reply/snooze in-toast) | inline actions | `[ ]` missing | history retained but no inline reply/snooze actions |
| Focus / DND modes | macOS Focus modes | scheduled DND + per-mode allow-list | `[~]` partial | `notify::quick_settings::dnd_enabled()` suppresses toast surface (keeps history) except Critical; `e92d00c`. GAP: no scheduled/per-mode allow-list |
| Quick Settings / Control Center | Win Quick Settings + mac Control Center | one panel: wifi/audio/brightness/focus/theme | `[~]` partial | `notify::quick_settings` strip — 5 toggles (DND + theme/accent flip via `theme_engine`, etc.) over the Notification Center; `e92d00c`. GAP: wifi/audio/brightness toggles not all backend-wired; iron-unproven |

## E. Clipboard

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| Cross-app text copy/paste | both | session-wide text clipboard | `[~]` partial | `kernel/src/clipboard.rs` (64KiB text, syscalls 107/108) |
| Clipboard history + pin | Win11 Win+V | history ring + pin + clear, privacy-aware | `[~]` partial | kernel `clipboard.rs` bounded newest-first history ring (CLIP_HIST_MAX_ENTRIES=64, pinned-safe eviction, de-dup) via `[interface]` syscalls 268-273 (`bbb3276`); `raeshell` Super+C glass flyout panel (pin/delete/clear/paste-on-select, `4f7fea8`). RAM-only/local by design. QEMU-booted. GAP: text-only (Image/Files/Url reserved), iron-unproven |
| Rich/binary formats | both | image/file formats | `[ ]` missing | text-only by design today |

## F. Screenshots / recording / annotation

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| Region/window/scroll capture | macOS screenshot + Flameshot | region+window+scroll, hotkey | `[~]` partial | Capture ABI 274-276 LANDED (`Cap::ScreenCapture`-gated, safe-mode-refused; `CaptureHeader` + pixels); raemedia PNG encoder saves real `.png` (`89cce24`); raeshell region-capture overlay Super+Shift+S (`0c4abb2`, concurrent-owned shell). GAP: no scroll capture, markup, or iron proof |
| Markup / annotate | macOS + Flameshot | arrows/text/blur post-capture | `[ ]` missing | none |
| Screen recording + GIF | macOS + Game Bar | compositor-level zero-cost capture | `[~]` partial | capture ABI supports `CAPTURE_FLAG_*` continuous; no encoder/GIF/video container yet |

## G. Continuity / cross-device

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| Handoff / file beam | macOS Handoff/AirDrop | RaeSync-backed handoff + beam | `[~]` partial | `components/raesync` compiles (E2E); no live transport |
| Universal clipboard | macOS | cross-device clipboard | `[ ]` missing | none |

## H. Backup & recovery

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| Snapshots + rollback | macOS Time Machine | CoW snapshots + one-click rollback | `[~]` partial (iron-proven engine, no UI) | `raefs.rs` snapshot/rollback iron-proven; no Settings UI |
| Scheduled versioned backup | Time Machine | scheduled + external target | `[ ]` missing | none |
| Recovery environment | WinRE / mac Recovery | bootable recovery + reset | `[~]` partial | `--safe` boot path; no recovery env |

## I. Security & privacy

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| Capability permissions | macOS TCC | Cap enum at syscall layer | `[x]` (enforced, iron) | `kernel/src/capability.rs` |
| Per-app sandbox by default | macOS App Sandbox | manifest-driven, fail-closed | `[~]` partial | sandbox fail-OPEN for bring-up (Phase 9) |
| Per-app permission prompts | both | consent prompt + real grant | `[~]` partial | `perm_prompt.rs` real grant; UI partial |
| FDE | BitLocker / FileVault | TPM-backed FDE | `[~]` partial | RaeFS FDE; TPM not wired |
| Code signing / notarization | Gatekeeper / SmartScreen | Ed25519 bundle sign + unverified-dev UX | `[~]` partial | `raeshield` Ed25519 signing |
| Driver claim/DMA cap-gate | DriverKit | syscalls 109-118 gated on held Cap | `[ ]` missing (HIGH) | flagged in work-log; needs daemon cap-plumbing |

## J. Accessibility (the parity gate)

> **Status note (2026-06-21):** two rows below are now STALE — the on-switches and the live
> high-contrast palette swap SHIPPED after this table was written. Verified in source:
> `shell_runner` a11y hotkeys (Super+Alt+M/H/C/R, Super+=/-) + Control Center Accessibility tile,
> and `rae_tokens::active_palette() == HIGH_CONTRAST` via `a11y::set_high_contrast` (proven by
> `a11y::run_onswitch_smoketest` + host KAT `active_palette_swaps_under_high_contrast`).
> So the "Magnifier" GAP "no toggle HOTKEY wired" is CLOSED, and "High contrast" is no longer
> `[ ]` — it is `[~]` (live palette swap, iron-unproven). Authoritative state:
> `docs/research/accessibility-audit-2026-06-21.md` + `Audit.md` (Accessibility section). Leaving
> the ladder cells for the lead to re-stamp; do not read the two flagged rows as current.

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| Screen reader | macOS VoiceOver | AccessKit tree + TTS | `[~]` partial | LIVE a11y tree (`kernel/src/a11y.rs`) + widget-tier names + screen-reader core: `announce_node`/`announce_focus` over a pluggable `SpeechSink` (`LogSpeechSink` QEMU-provable, `1569ed6` Phase 19.2); a11y ABI 277-278 (`SYS_A11Y_SNAPSHOT`/`ACTION`). GAP: no real TTS->RaeAudio `AudioSpeechSink`; no nav gestures; iron-unproven |
| Magnifier / zoom | both | compositor zoom + smooth follow | `[~]` partial | compositor magnifier = source-sampled scanout upscale (`magnifier_set_enabled`, `8b7b2f4` §3) + focus-follows pan to focused node (`30a577e` Phase 19.3). **UPDATE 2026-06-21: toggle HOTKEY now wired** (Super+Alt+M + Super+=/- in `shell_runner`, `a11y::toggle_magnifier`/`magnifier_zoom_in/out`). GAP: no smooth animation, iron-unproven |
| High contrast / color filters | both | palette swap via tokens | `[~]` partial (was `[ ]`; SHIPPED 2026-06-21) | LIVE forced-colors swap: `a11y::toggle_high_contrast`/`set_high_contrast` -> `rae_tokens::active_palette() == HIGH_CONTRAST`; on-switch Super+Alt+H + Control Center tile; compositor color filters (`a11y_filter_set`, Super+Alt+C). Proven by `a11y::run_onswitch_smoketest` + host KAT. GAP: broaden surface coverage; iron-unproven |
| Sticky/slow keys, key repeat | both | input-layer toggles | `[ ]` missing | none |
| Live captions / voice control | both | post-1.0 | `[ ]` missing | none |

## K. Dev tools

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| Terminal + shell | mac Terminal / Win Terminal | VT100/xterm, tabs, true-color | `[~]` partial | `apps/terminal` VT100, SGR color |
| Package manager | brew / winget / apt | RaeStore CLI + repos | `[~]` partial | `raestore` compiles |
| Containers / VM | WSL / Hyper-V / Parallels | POSIX layer + a VM story | `[~]` partial | Linux syscall ABI runs relibc apps |

## L. Settings & management

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| Unified searchable Settings | mac System Settings | one searchable app, no split | `[~]` partial | `apps/settings` + `control_panel.rs` model (2446 lines), tokenized; search/IA pending |
| Storage management | both | per-app usage + cleanup | `[ ]` missing | RaeFS data exists; no UI |
| System info / About | both | one panel from /proc/raeen | `[~]` partial | data exists; no UI |
| Display/sound/network/power UIs | both | per-domain panels | `[~]/[ ]` mixed | backends exist; most UIs missing |

## M. Power & battery

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| Battery health + reporting | both | fuel-gauge + health + per-app energy | `[~]` partial | fuel-gauge iron-proven; no UI |
| Low-power / power modes | both | plans + auto | `[~]` partial | CPPC read iron; no plans UI |
| Sleep (S3) + wake <1s | both | S3 + fast wake | `[~]` partial (iron-gated) | NOT "not built": `kernel/src/suspend.rs` has a real S3 entry path (FADT PM1a/PM1b_CNT, SLP_TYP from `_S3` | SLP_EN, FACS waking-vector handling) + S4/S5, wired+smoketested (main.rs 1010). FACS resume trampoline explicitly skipped on QEMU; needs iron to prove resume. NOT a QEMU-verifiable target |

## N. Gaming (the wedge)

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| Real-time game scheduling | none (Game Mode weak) | SCHED_GAME hard deadlines | `[~]` partial | EDF exists; deadline-miss telemetry missing |
| Game bar / overlay | Win Game Bar | FPS/frametime/temps overlay | `[ ]` missing | none |
| Per-game profiles | 3rd party | res/refresh/audio/GPU per game | `[~]` partial | profile struct exists |
| GameOS / Big Picture | none | couch shell, controller-only | `[~]` partial | `raeshell/gameos.rs` |
| Capture/stream | Game Bar | zero-cost at compositor | `[ ]` missing | (= F. screen recording) |

## O. Input

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| Keyboard / mouse | both | live HID end-to-end | `[~]` partial | HID armed iron; live-typing pending |
| Keyboard remapping | Win PowerToys / kanata | system remap + layers | `[~]` partial | `components/kanata_daemon` vendored; wiring pending |
| Trackpad gestures | macOS (best-in-class) | swipe spaces/expose/zoom | `[ ]` missing | none |
| IME / layouts | both | CJK + layouts | `[ ]` missing | confirmed: `usb_hid.rs::bridge_scancode` ships raw scancodes; the scancode->char map is fixed US-QWERTY (no AZERTY/QWERTZ/Dvorak), `raelocale` has NO keyboard-layout data, no IME framework. Switcher blocker for non-US users; QEMU-verifiable for alt-LAYOUTS (CJK IME is bigger, defer) |
| Emoji / symbol picker | both | hotkey picker | `[ ]` missing | none |

## P. Automation & scripting

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| Visual + scriptable automation | mac Shortcuts / Win Power Automate | visual flows + scriptable | `[ ]` missing | none |

## Q. App distribution

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| App store + sideload + PWA | App Store / Store / Flatpak | RaeStore + sideload + PWA | `[~]` partial | `raestore` shell; rae_pwa parses W3C Web App Manifest (`1b67e01`, install-to-desktop foundation) but no install flow wired; sideload missing |
| Windows app compat | none (the moat) | RaeBridge (Wine/Proton lineage) | `[~]` partial | `raebridge` LDR/SEH host-KAT'd; GPU/Steam blocked |

## R. Customization (Vibe Mode differentiator)

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| Themes / accent / one-tap reskin | none unified | one seed reskins whole desktop + running apps | `[~]` partial (proven) | `theme_engine::active_accent` + `SYS_THEME_GET` (266) reach apps; cohesion smoketest PASS |
| Live wallpapers | 3rd party | GPU live wallpaper, paused when occluded | `[ ]` missing | none |
| Widgets | both | Rainmeter-class | `[ ]` missing | `widgets.rs` kit only |
| RGB / fan curves | vendor sprawl | one API | `[~]` partial | `rgb.rs` + fan curves; HW iron-gated |

## S. Networking

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| Wired + DHCP | both | DHCP to Bound | `[~]` partial | RTL8125 TX+RX iron; DHCP Bound pending |
| Wi-Fi | both | scan/connect/WPA3 | `[ ]` missing | AX210 via LinuxKPI; not built |
| VPN (WireGuard) | both | built-in tunnel | `[~]` partial | Noise handshake; tunnel pending |
| Firewall | both | per-app rules + UI | `[~]` partial | smoketest iron; no UI |
| File/printer sharing | SMB / AirDrop | post-1.0 | `[ ]` missing | none |

## T. Quick utilities

| Capability | Best today + why | Target | RaeenOS status | Evidence |
|---|---|---|---|---|
| Calculator | both | real f64 calc | `[~]` partial | `apps/calculator` + `rae_calc` (17 KATs) |
| Notes | both | quick notes | `[~]` partial | LIVE: `apps/notes` real app — sidebar `.md`/`.txt` list in `<home>/Notes`, edit buffer + live `rae_markdown` preview (Tab toggle, Ctrl+S save, New/Delete), themed via `SYS_THEME_GET`. GAP: iron-unproven, no sync |
| Clock / timers | both | alarms/timers | `[~]` partial | LIVE: `apps/clock` real app — 5 tabs (Clock w/ analog face, Alarms, Timer, Stopwatch, Calendar month grid) off the system wall clock. GAP: alarms fire only while app open (no bg daemon); iron-unproven |
| Photos / image viewer | both | viewer + basic edit | `[~]` partial | `apps/photos` LIVE over raemedia PNG/JPEG + EXIF orientation (`720e99e`); QEMU-booted. GAP: no edit, no zoom/pan UI proven, iron-unproven |
| Media player | both | symphonia-backed player | `[~]` partial | `apps/music` LIVE over raemedia WAV decoder -> mixer PCM (`f836954`); QEMU-booted. GAP: WAV-only (no mp3/flac/aac), no video, no playlist UI depth |
| System monitor | Task Mgr / Activity Monitor | live CPU/mem/proc/net | `[~]` partial | `apps/task_mgr` exists; depth TBD |
| Emoji / color / character picker | both | pickers | `[ ]` missing | none |

---

## Integration-wave plan (ranked, 2026-06-17 waves 3-7)

The foundations are now LANDED (media decoders, a11y suite, capture/a11y ABI,
web/PWA pure libs). The next value is WIRING them into shippable user-facing
features. Codeable = not blocked on iron/Mesa/Steam/external HW.

CLOSED waves 1-7 (now `[~]`, QEMU/host-KAT only): command palette (B), clipboard
history + pin (E), Notification Center + Quick Settings (D), Files Quick Look PNG
(C), media pillar Photos/Music + decoders (T/C), accessibility suite tree/reader/
magnifier/focus-follows (J), screen-capture + a11y ABIs (F/J), web/PWA pure libs.

LANE DISCIPLINE: a concurrent opus session owns raeshell (launcher/switcher
chrome), shell_runner (hotkeys), raeweb (web engine), gameos, and the GPU stack.
Anything touching those files must SEQUENCE-AFTER their in-flight commit lands.
My disjoint lanes: `apps/*`, the new `components/*` libs, `kernel/src/a11y.rs`,
raemedia, raenet client surface.

### MY-LANE wave (disjoint — runnable once image is green):

1. [DAILY] **Notes app** (`apps/notes`) on rae_markdown. NEW crate, zero
   collision. Live edit + CommonMark preview. Boot-verifiable (launch + render
   marker). Highest value/risk ratio; pure my-lane.
2. [DAILY] **Clock/Calendar utility** (`apps/clock`). NEW crate over `SYS_TIME`/
   tray clock data; alarms/timers/world-clock + month grid. Boot-verifiable.
   Zero collision. (Calendar can be same app or `apps/calendar`.)
3. [SWITCHER] **Files Quick Look: JPEG + EXIF + thumbnails** (`apps/files`). Wire
   raemedia jpeg/EXIF (already decoding) into the existing PNG QuickLook path +
   a thumbnail grid. My-lane (`apps/files` is mine). Boot-verifiable.
4. [DAILY/SWITCHER] **GIF + WebP decoder, then Video player** (raemedia +
   `apps/music`->media). GIF/WebP are host-KAT-only pure decoders (extend
   raemedia, 165-KAT pattern); video player app is boot-verifiable. My-lane.
   Sequence: decoders first (host-KAT), then player UI.
5. [SWITCHER] **Web-fetch / PWA-install demo** on raenet HTTP + rae_pwa +
   rae_json + rae_markdown. A small `apps/` demo: fetch a URL, parse a manifest,
   install-to-launcher stub, render markdown. Pure my-lane libs. Web FETCH is
   host-KAT-able; the launcher-install hook touches raeshell -> SEQUENCE-AFTER
   concurrent (stub the registration locally, hand the wire-up to raeshell).
6. [SHIP GATE] **a11y: high-contrast palette + AudioSpeechSink** (`kernel/src/
   a11y.rs` + raemedia/raeaudio). High-contrast = token palette swap (my-lane,
   boot-verifiable smoketest). Real TTS sink is bigger; host-KAT the text->PCM
   path first. `a11y.rs` is my-lane.

### SEQUENCE-AFTER concurrent session (collide — hand off / wait):

- Wire Photos/Music/PWA-install into the LAUNCHER (raeshell) — concurrent owns
  raeshell render; provide the app-registry rows, let them wire the tiles.
- Magnifier toggle HOTKEY (shell_runner) — mechanism is mine and DONE
  (`magnifier_set_enabled`); the keybind is in shell_runner (concurrent). Hand
  off the one-line bind, or sequence after their hotkey-table commit.
- Search index -> palette/Start (raeshell `search_indexer` still dead) —
  raeshell-owned; flag for sequencing.

### NEEDS THE OWNER (not codeable disjoint):
- Iron UNPAUSE — every `[~]` above is QEMU/host-KAT; NONE can reach `[x]` until
  Athena flashing resumes. This is the single biggest blocker to the ship gates.
- Two-session de-confliction — the shared image is red from concurrent GameBar
  WIP; my-lane waves wait on their fix-commit + a green build. The raeshell/
  shell_runner/raeweb/gameos/gpu lane boundary needs the owner's coordination so
  the launcher/hotkey wire-ups don't double-commit (per the shared-worktree
  hazard memory).

### STILL BLOCKED (external/iron — track separately):
- Real-GPU submit / Mesa scanout (Phase 6) — concurrent session pushing SDMA.
- Steam / Proton / DXVK (Phase 11) — external + GPU-blocked.
- Wi-Fi (AX210), DHCP->Bound, USB-MSC enumerate, S3 suspend, live HID typing,
  TPM, thermal — pure-iron, paused.
