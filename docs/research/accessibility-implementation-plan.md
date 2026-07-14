# Phase 19 — Accessibility Implementation Plan (the shovel-ready wave plan)

> **SUPERSEDED (status only) — 2026-06-21.** This was a PLAN-ONLY dispatch doc written before the
> work landed. The **build order, owners, seam map, and proof-tier guidance below are still
> useful**, but the status framing ("ready to dispatch", "every §J row is `[ ]`", "stub",
> "build this") is **STALE** — most of this plan has since shipped. **The authoritative current
> state is `docs/research/accessibility-audit-2026-06-21.md`** (verified against live source).
> Live status ladder: `MasterChecklist.md` + `docs/PARITY_MATRIX.md §J`. Do not resurrect this
> doc's `[ ]`s.
>
> What actually shipped (verified in source 2026-06-21; file evidence in the audit):
> - §1 tree: `kernel/src/a11y.rs` (1236 lines), R10-complete, ABI 277/278 (sec 7 interface landed).
> - §2 screen reader: announce core + `SpeechSink`/`LogSpeechSink` built (TTS->AthAudio still gated).
> - §3 magnifier: compositor source-sampled upscale + focus-follows + FAIL-able smoketest — built.
> - §4 color filters + reduced-motion hook + athui role inference (`role_from_widget_kind`) — built.
> - §5 contrast math (`ath_tokens::contrast_ratio` + HC palette) — built/unit-tested.
> - User on-switches (the §4.3-style hotkeys Super+Alt+M/H/C/R + Super+=/-, Control Center
>   Accessibility tile) AND high-contrast LIVE palette swap (`active_palette() == HIGH_CONTRAST`)
>   shipped AFTER the audit — proven by `a11y::run_onswitch_smoketest`.
>
> Remaining live gaps (the work that is NOT done — see audit Top-5): the widget-provider WIRING
> (`provider_nodes_for_window` has no caller; #1 P0), unified desktop keyboard focus order +
> modal trap + "no-mouse" audit (#4), the FAIL-able boot-time WCAG audit over the full palette
> (#5), real TTS sink, text scaling, full reduced-motion coverage.

**Status:** SUPERSEDED dispatch plan (most slices BUILT; status above) — kept for build order + seams.
**Owner of this doc:** athena-accessibility
**Builds on:** `docs/research/phase19-accessibility-foundation.md` (19.1 tree foundation spec,
already banked). This doc is the *whole-Phase-19* plan: it takes 19.1 as the load-bearing first
slice and lays out 19.2 (screen reader, magnifier, high-contrast/reduced-motion) and 19.3
(keyboard-only nav + focus) on top, with owners, files, proof tiers, and a build order so the
lead can schedule non-overlapping waves.

**Why this is a ship gate (athena-parity #7, flagged SHIP GATE):** a UI that rivals macOS and
Windows has to be usable by keyboard-only, screen-reader, and low-vision users.
~~Every row in `docs/PARITY_MATRIX.md §J` is currently `[ ]`.~~ (STALE — §J now has `[~]` rows;
the screen-reader/magnifier engines are built, high-contrast went live, and the on-switches
shipped. See the SUPERSEDED banner.) This was the single biggest unowned parity gap; it is now
OWNED and largely built — the remaining gap is INTEGRATION + user-reach + audits, not greenfield.

**Concept lines served (every module's `//!` must quote one):**
- Security: "Capability-based permissions — apps request capabilities ... OS enforces at the
  syscall layer." (the AT API is its own privilege class)
- Security: a system "where you can run untrusted software without fear" (an AT client must not
  read another app's UI tree unprompted)
- "Built for people who care about how things feel." (a11y is a feel-of-the-OS property, not a
  bolt-on)
- Phase 19 mandate: parity with Windows Narrator/Magnifier + macOS VoiceOver/Zoom (all shipped
  non-optional).

---

## 0. Best-of-breed scan (what each piece must beat)

| Piece | Windows | macOS | Linux | AthenaOS target (beat them) |
|---|---|---|---|---|
| A11y tree | UI Automation provider tree (AutomationElement) | NSAccessibility / AXUIElement | AT-SPI2 over D-Bus | AccessKit-shaped node arena, kernel-owned at window tier + AthUI widget provider; cap-gated read like macOS TCC. **No D-Bus round-trip tax** — a syscall + procfs snapshot. |
| Screen reader | Narrator | VoiceOver (best-in-class) | Orca | Tree-walk + focus-announce **logic** host-KAT'd & deterministic; speech is a pluggable sink (log/braille now, TTS iron-gated). Other apps feed a stable focus-event stream. |
| Magnifier | Magnifier (full/lens/docked) | Zoom (smooth, focus-follow) | gnome magnifier (heavy) | **Compositor post-process upscale of the already-composited `scanout_ready` buffer** — one extra sampled blit, near-zero frames (reuse the exact overview/snapshot scaled-blit math). Focus-follows from the a11y focus event. |
| Keyboard nav | Tab/arrow + access keys | Full Keyboard Access | inconsistent | One consistent focus ring + Tab order across shell AND apps, driven by the a11y tree's `focus_next/prev`; focus trapping in modals; "no mouse required" as a FAIL-able audit. |
| High-contrast / reduced-motion | HC themes + "show animations" off | Increase contrast + Reduce motion | per-DE | Token-level palette swap via `ath_tokens` + a global mode flag every animation already reads (`MOTION_INSTANT` / `set_reduce_motion`). Honored everywhere, machine-audited. |
| Contrast compliance | manual | manual | manual | **`ath_tokens::contrast_ratio` wired into a FAIL-able boot audit** — the OS proves WCAG AA on its own palette at boot. Nobody else does this. |

---

## 1. ACCESSIBILITY TREE (foundation — 19.1)

**Owner:** athena-accessibility (kernel tree) + athena-ui (widget provider). **Interface:**
athena-architect lands the NEEDS-INTERFACE block FIRST.

### Data model
AccessKit vocabulary, AthenaOS-native `no_std` types (do NOT vendor the std-heavy AccessKit
crate into the kernel; mirror its shape so a future userspace AccessKit *adapter* maps 1:1).
Full type spec is in `phase19-accessibility-foundation.md §3` — `Role(u16)`, `NodeState`
bitflags, `Actions` bitflags, `AccessNode { id, parent, role, name, state, bounds, actions,
z_order }`.

### Where it attaches (verify-before-implement — these already exist)
- **Window tier (build this):** `kernel/src/compositor.rs` already owns the authoritative
  `Vec<Surface>` with `id/title/bounds/z_order/visible/minimized` + `focused_surface_id()` +
  `exclusive_surface_id()`. `kernel/src/a11y.rs::build_tree()` maps each `Surface` -> one
  `AccessNode(Role::Window)` under a root `Desktop` node. **No new compositor state needed.**
- **Widget tier (athena-ui populates the seam):** `components/athui/src/accessibility.rs`
  ALREADY has the full userspace model (`AccessibilityRole` 24 roles, `AccessibilityTree`,
  `build_from_widget_tree`, `focus_next/prev`, `describe_focused`, `HighContrastPalette`,
  `FocusRing`). ~~Its `infer_role_from_widget_id` / `infer_label` are stubs returning
  `Group`/`""`~~ (STALE 2026-06-21 — role inference is now REAL: `role_from_widget_kind` +
  `provider_nodes_for_window`). **The #1 real gap is now the PROVIDER WIRING — both ends of the
  seam exist but `provider_nodes_for_window` has zero callers, so the live tree is window-tier
  only and every app is one anonymous "Window" node.** athena-ui makes
  widgets implement the existing `Accessible` trait and feeds real role/label through the
  `a11y.rs::set_widget_provider(fn(window_id)->Vec<AccessNode>)` seam (foundation §6).

### Sync discipline
Tree is built on demand (per snapshot / per focus-change), NOT a maintained shadow copy — it
reads the live surface list under `lock_compositor()` (IF=0 syscall context), copies out a plain
`Vec<AccessNode>`, drops the guard, then serializes. **Never hold COMPOSITOR across copy-to-user**
(CLAUDE.md §10.6). Widget nodes are pulled the same way: `build_tree()` calls the provider per
window and parents results under that window's `Surface.id`.

### Which crate owns it
`kernel/src/a11y.rs` (NEW) owns the window-tier tree + cap-gated API + procfs. `athui` owns the
widget-tier provider data. They meet at the `set_widget_provider` seam + the `ath_abi` `A11yNode`
repr(C).

### Files touched
- NEW `kernel/src/a11y.rs`; `kernel/src/main.rs` (init after `compositor::init`);
  `kernel/src/procfs.rs` (register `/proc/athena/a11y`); `kernel/src/syscall.rs` (2 dispatch arms).
- `components/ath_abi/src/lib.rs` (Cap variant + 2 syscall numbers + `A11yNode` repr) —
  **athena-architect only, `[interface]` commit, bump `ABI_VERSION`, update
  `docs/SYSCALL_TABLE.md` same commit**.
- `components/athui/src/accessibility.rs` (athena-ui fills the `infer_*` stubs + `Accessible`
  impls) — additive, does NOT touch compositor.

### Proof tier
- **Host-KAT-able (do first, house rule 15):** build a tree from synthetic `Surface`-like
  inputs; assert roles/states/bounds/`cap_denies_uncapped`. Pure logic, `cargo test` on dev box.
- **QEMU smoketest:** R10 `a11y::run_boot_smoketest` (foundation §4).
- **iron-gated:** none for the tree itself (no HW); `[~]` on QEMU pass, `[x]` only on Athena.

### FAIL-able boot-log line (R10, exact)
```
[a11y] tree smoketest: seeded_window_found=true role=Window name_ok=true bounds_ok=true focused_state_ok=true cap_denies_uncapped=true action_focus_ok=true -> PASS
```
Init line: `[a11y] accessibility tree online (AccessKit-compatible, window tier)`

### Serialization note (scheduling)
`a11y.rs::build_tree()` reads the compositor surface list but adds **no compositor state** and
does not modify `compositor.rs`. It only *calls* existing `pub fn`s (`focused_surface_id`,
`create_kernel_surface`, `focus_surface`, `close_surface`). So 19.1 does **not** serialize
against the WM/screenshot waves — it can land in parallel with them. (Contrast with the
magnifier below, which DOES touch `compositor.rs`.)

---

## 2. SCREEN READER (19.2)

**Owner:** athena-accessibility (engine) + athena-ui (widget labels via §1 provider) + athena-kernel
(focus-change event). **Audio tail:** athena-audio (iron-gated).

### Architecture — split the QEMU-provable core from the iron-gated tail
1. **Tree-walk + focus-announce core (host-KAT-able, build this):** given an `AccessNode` tree +
   a focus target, produce the spoken string. The logic ALREADY EXISTS as
   `athui::accessibility::AccessibilityTree::describe_focused()` ("label, role, value, state,
   hint"). Promote/mirror that as the kernel-side announcer over `a11y.rs` nodes so it works for
   the live (kernel) tree, not just the dead athui tree. This is pure string logic — deterministic
   and FAIL-able on the host.
2. **Focus-change event stream (kernel, QEMU-provable):** the screen reader must be told when
   focus moves. Add a lightweight focus-change hook: when `compositor::focus_surface` changes the
   focused surface (or the widget provider reports a new focused widget), push a
   `(node_id, generation)` event the reader drains. Keep it allocation-free (an `AtomicU64` focus
   generation + node id is enough; the reader re-snapshots the tree on change). **This is the
   stable interface other apps feed** — an app raising focus calls the same `focus_surface` /
   widget-focus path; it does not talk to the reader directly.
3. **Speech sink (pluggable; log/braille now, TTS iron-gated):** the announcer writes to a
   `SpeechSink` trait. The default sink is the **serial/announce log** (QEMU-provable: the spoken
   string appears in the boot log). A braille sink and a real **TTS sink are iron/audio-gated**
   (TTS needs the AthAudio PCM path proven on iron, Phase 7) — identify them as the tail, ship
   the log/braille sink now, mark TTS pending.

### Why the split matters
The thing that makes a screen reader *correct* (does it announce the right element with the
right role/state when focus moves?) is the tree-walk + focus-announce logic — and that is 100%
host-KAT-able and QEMU-smoketestable with zero audio hardware. The audio is a sink swap. We do
NOT block the reader on iron TTS.

### Files touched
- NEW `kernel/src/screen_reader.rs` (announcer + `SpeechSink` trait + log sink + focus drain).
- `kernel/src/a11y.rs` (expose tree-walk + a `focus_generation()` accessor).
- `kernel/src/compositor.rs` — **MINIMAL, serializes against WM/screenshot waves:** bump a focus
  generation atomic inside the existing `focus_surface`. One line; coordinate so it lands between
  WM waves, not concurrently. (If athena-kernel prefers, expose it as a no-arg `note_focus_change()`
  that `focus_surface` calls — same one-line touch.)
- `components/athui/src/accessibility.rs` (athena-ui: real labels so the announcer says something).

### Proof tier
- **Host-KAT:** `describe_focused` over synthetic trees — assert "Settings, button" etc., assert
  state suffixes (disabled/selected/expanded). FAIL-able, dev box.
- **QEMU smoketest:** seed two kernel surfaces, focus one, assert the announcer's string for the
  focused node is correct AND that a focus change to the other surface re-announces.
- **iron-gated:** real TTS audio output (AthAudio PCM). Mark pending; report QEMU evidence.

### FAIL-able boot-log line (R10, exact)
```
[a11y-sr] reader smoketest: announce_focused="Settings, window" focus_changed_reannounced=true sink=log empty_tree_says_none=true -> PASS
```
Init line: `[a11y-sr] screen reader online (sink=log; tts pending iron audio)`

### Serialization
The one-line focus-generation bump in `compositor::focus_surface` MUST serialize against the WM
wave (which is in flight and edits `compositor.rs`). Everything else (`screen_reader.rs`,
`a11y.rs`) is additive and parallel-safe.

---

## 3. SCREEN MAGNIFIER (19.2)

**Owner:** athena-gfx (compositor seam) + athena-accessibility (focus-follows policy).

### The exact compositor seam (verified by reading compositor.rs)
The magnifier is a **post-process upscale of the already-composited frame** — it costs one extra
sampled blit, not a re-render. The seam is `recomposite()` Step 4 (the scanout flush,
`compositor.rs` ~lines 3601-3622): after the frame is swapped into `scanout_ready` and the lock
is dropped, the scanout loop copies `ready[y*cw+x]` 1:1 into the framebuffer
(`GpuFb` / `VirtioGpu` / `Gop` arms). The magnifier replaces that 1:1 read with a
**source-sampled read** centered on the zoom focus point:
`src = ready[ (oy/zoom + origin_y)*cw + (ox/zoom + origin_x) ]`.

This reuses machinery that ALREADY EXISTS and is proven:
- `overview_cell_dst` + `blit_thumbnail_into_comp` (compositor.rs ~1803-1875) already do
  aspect-correct integer downscale sampling of a buffer — the magnifier is the **upscale** twin
  of the same nearest/box sample math.
- `snapshot_surface` (compositor.rs ~1896) already proves the lock-disciplined "sample a buffer
  into a destination" pattern.
- The magnifier reads `ready` (compositor-OWNED, persistent) — NOT `surf.kernel_ptr` user pages —
  so it is automatically UAF-safe and runs *outside* the lock, exactly like the existing scanout
  (no new IF=0 window, no new frame cost beyond the sampled copy).

### Mechanism
- Global magnifier state (atomics, like `OVERVIEW_ACTIVE` / `WALLPAPER_ALPHA`):
  `MAG_ENABLED: AtomicBool`, `MAG_ZOOM_X16: AtomicU32` (zoom * 16 fixed-point, e.g. 32 = 2.0x),
  `MAG_FOCUS_X/Y: AtomicU32`. Read lock-free by the scanout step (same pattern as overview).
- `recomposite` scanout step: `if MAG_ENABLED` -> sampled upscale into the FB instead of 1:1.
  Clamp the source window so edges don't read out of `ready`.
- **Focus-follows (athena-accessibility policy):** on a focus-change event (§2), set
  `MAG_FOCUS_X/Y` to the focused node's bounds center. **Smooth-pan:** step the focus point
  toward the target a fraction per frame (honoring reduced-motion: jump instantly when reduced).
- Lens vs full-screen: ship full-screen zoom first (cheapest, matches Win Magnifier default);
  lens mode is a later destination rect.

### Files touched
- `kernel/src/compositor.rs` — **the magnifier sampled-blit in the scanout step + the atomics +
  a `pub fn magnifier_set(enabled, zoom, focus)`. THIS SERIALIZES HARD against the WM + screenshot
  waves** (all three edit `recomposite`/scanout). The lead MUST schedule this in its own wave
  after WM/overview/screenshot settle, or hand athena-gfx a merge window. Call this out explicitly.
- `kernel/src/a11y.rs` or `screen_reader.rs` (focus-follows policy calls `magnifier_set`).

### Proof tier
- **Host-KAT-able:** the sample-coordinate math (given zoom + focus + output pixel -> source
  pixel, with clamping) is pure integer logic — KAT it standalone (assert center maps to focus,
  corners clamp in-bounds, 1.0x == identity).
- **QEMU smoketest:** enable 2x at a known focus, drive one `recomposite`, read the FB top-left
  and center pixels vs the known `ready` content; assert the center sample equals the focus-area
  source pixel and 1.0x is byte-identical to the normal path (the overview smoketest at
  compositor.rs ~2025 is the exact template — copy its before/after pixel-probe structure).
- **iron-gated:** "smooth at 60fps on a real panel" feel — QEMU proves correctness, iron proves
  perf. Mark perf pending.

### FAIL-able boot-log line (R10, exact)
```
[a11y-mag] magnifier smoketest: identity_1x_byte_match=true zoom2x_center_samples_focus=true edge_clamped_in_bounds=true focus_follows_set=true -> PASS
```
Init line: `[a11y-mag] screen magnifier online (compositor post-process, full-screen zoom)`

---

## 4. KEYBOARD-ONLY NAVIGATION + FOCUS MANAGEMENT (19.3)

**Owner:** athena-accessibility (focus order + ring contract) + athena-ui (per-widget focusability)
+ athena-shell-apps (Tab order in each surface) + athena-kernel (input toggles).

### Focus model — reuse what exists
- `athui::accessibility::AccessibilityTree` ALREADY has `focus_next` (Tab), `focus_previous`
  (Shift+Tab), `focus_node`, `focusable_nodes()` (filters `focusable && !hidden && !disabled`),
  `focused_widget_id()`. This IS the focus engine — it is just not driven by live key events.
- `FocusRing::from_node` ALREADY computes a 2px ring inset around the focused node's bounds.
- The fresh keyboard-driven shell surfaces (command palette `0b026f1`, clipboard panel `4f7fea8`)
  are the *pattern* for a surface that owns a selected-index + handles key-down: every focusable
  surface follows that shape.

### What to build
1. **A consistent focus-ring contract** drawn from tokens: the ring color/glow = `ath_tokens`
   `elev_focus(accent.glow)` (already exists; foundation already references the
   `elev_focus`/focus-ring token) and the high-contrast override =
   `HighContrastPalette.focus_ring` (cyan `0xFF00FFFF`, already in athui). Focus is **never
   color-only** (the command-palette spec §8 already states this) — always the ring shape too.
2. **Tab order across shell + apps:** each surface exposes its focusables in DOM/tree order via
   the §1 widget provider; `focus_next/prev` walks them; Tab/Shift+Tab/arrows route through the
   focused surface. Arrow keys for lists/grids; Tab for field-to-field.
3. **Focus trapping in modals:** when a node has `traits.modal` (already a field) / a surface is a
   `Dialog`/`Alert`, Tab wraps within the dialog's subtree (don't escape to the desktop). The
   modal flag already exists; wire the wrap.
4. **"No mouse required":** every flow reachable by keyboard — this is the single most TESTABLE
   a11y property; make it the audit (below).
5. **Input-layer toggles (athena-kernel):** sticky keys / slow keys / key-repeat live in
   `kernel/src/input.rs` (foundation §9 names them). Additive, host-KAT-able state machines.

### Modes (tie to theme engine / tokens)
- **Reduced-motion:** the hook ALREADY EXISTS — `athshell::animations` has `reduce_motion` +
  `set_reduce_motion`, and `ath_tokens::REDUCED_MOTION_DURATION_MS = 0` / `MOTION_INSTANT`. Add a
  **single global flag in `kernel/src/theme_engine.rs`** (an `AtomicBool`, mirrored to userspace
  like `active_accent` is) that every animation site reads. The audit asserts each animation
  honors it.
- **High-contrast:** `athui::HighContrastPalette` exists; promote it to a `ath_tokens`
  high-contrast `Palette` variant so the swap is one token lookup the whole stack already reads
  (every surface already pulls `ath_tokens::DARK`/`LIGHT`). theme_engine holds the
  `HIGH_CONTRAST: AtomicBool`; `active_palette()` returns the HC palette when set.
- **Scalable text:** a global text-scale factor (e.g. 1.0/1.25/1.5/2.0) in theme_engine; layout
  reads it. Must not break layout — the audit checks a scaled pass still fits (no clipped/
  overlapping bounds in the tree). athui layout already computes bounds, so this is checkable.

### Files touched
- `components/athui/src/accessibility.rs` (focusability/labels — additive).
- `kernel/src/theme_engine.rs` (3 mode flags + `active_palette()` + text-scale; mirror to
  userspace like accent). **Coordinate with whoever owns theme_engine; additive atomics.**
- `kernel/src/input.rs` (sticky/slow keys — additive, host-KAT-able).
- `components/athshell/*` + `apps/*` (Tab order per surface) — **shell/app surfaces; serialize
  against the shell wave in flight.**
- `components/ath_tokens/src/lib.rs` (HC palette variant + a `MOTION_INSTANT`/reduced helper if
  not already exposed) — additive.

### Proof tier
- **Host-KAT-able:** `focus_next/prev` wrap-around; modal trap (focus never leaves the modal
  subtree); sticky/slow-key state machines; reduced-motion -> duration 0; HC palette swap;
  text-scale layout-fit checker. All pure logic, dev box.
- **QEMU smoketest:** seed a surface tree with N focusables incl. a modal; assert Tab advances
  through exactly the focusables, wraps, and is trapped in the modal; assert reduced-motion makes
  a sampled animation duration 0; assert HC palette is active when the flag is set.
- **iron-gated:** live key events from a real keyboard (HID typing is iron-pending per the board).
  QEMU proves the focus *logic*; iron proves the *input path*. Mark the live-typing leg pending.

### FAIL-able boot-log line (R10, exact)
```
[a11y-kbd] keyboard-nav smoketest: tab_advances=true tab_wraps=true shift_tab_reverses=true modal_traps_focus=true reduced_motion_zeroes_duration=true high_contrast_palette_active=true text_scale_2x_fits=true -> PASS
```
Init line: `[a11y-kbd] keyboard navigation + focus online (ring=elev_focus; modes: HC/reduced-motion/text-scale)`

---

## 5. COLOR-CONTRAST COMPLIANCE (the audit nobody else ships)

**Owner:** athena-accessibility + athena-visual-qa (measured values from real screenshots).

### Mechanism
`ath_tokens::contrast_ratio(fg_argb, bg_argb) -> f32` ALREADY EXISTS (ath_tokens/src/lib.rs:340,
WCAG relative-luminance formula) and is already used by `derive_accent` and unit-tested
(`contrast_ratio(RAEBLUE, DARK.bg_base) >= 4.5`). The plan WIRES IT INTO A FAIL-ABLE BOOT AUDIT:
- Enumerate the load-bearing token pairs the OS actually paints: `DARK.text_primary` vs
  `DARK.bg_base`, `LIGHT.text_primary` vs `LIGHT.bg_base`, accent-on-bg, secondary text,
  focus-ring vs bg, and the high-contrast palette pairs.
- Assert each ≥ the WCAG bar: **4.5:1 for body text, 3.0:1 for large text / UI affordances /
  focus rings** (WCAG AA). Any pair below the bar -> **FAIL** with the offending pair + measured
  ratio printed (so it's actionable, not just red).
- This is a kernel/host audit over the static token palette. athena-visual-qa supplies the
  *measured* counterpart from real screenshots (rendered pixels can differ from token intent due
  to blur/blend); file mismatches back to athui/shell-apps with the exact widget + measured ratio.

### Files touched
- NEW host-KAT + boot smoketest: `kernel/src/a11y.rs::run_contrast_audit()` (or a small
  `a11y_contrast` submodule) calling `ath_tokens::contrast_ratio` over the palette pair list.
- `components/ath_tokens/src/lib.rs` already has the fn + tests (no change needed beyond possibly
  exposing the HC palette from §4).

### Proof tier
- **Host-KAT-able (primary):** the whole audit is pure logic over static palettes — it runs as a
  `cargo test` and as a boot smoketest. This is the cheapest, most complete proof.
- **QEMU smoketest:** the same audit at boot prints the PASS/FAIL line.
- **iron-gated:** none for token math; athena-visual-qa's *measured-from-screenshot* ratios are the
  rendered-pixel counterpart (QEMU screendump has the known stripe artifact — measure on
  host-render / iron per MEMORY).

### FAIL-able boot-log line (R10, exact)
```
[a11y-contrast] contrast audit: pairs=12 min_ratio=4.83 body_text>=4.5=true ui>=3.0=true hc_palette>=7.0=true worst_pair="secondary_text/bg_base=4.83" -> PASS
```
(On failure: `... worst_pair="accent/bg=2.91" -> FAIL` — names the pair + ratio so it's fixable.)

---

## 6. Owner / serialization summary (for the lead's scheduler)

| Piece | Owner(s) | Touches `compositor.rs`? | Touches `athui`? | Serialize against |
|---|---|---|---|---|
| §1 A11y tree | accessibility (+ architect interface, + ui provider) | **No** (calls existing pub fns only) | yes (additive: fill `infer_*` stubs) | nothing — parallel-safe |
| §2 Screen reader | accessibility (+ kernel 1-line focus bump, + ui labels) | **1 line** in `focus_surface` | yes (labels) | the WM wave (the 1-line bump) |
| §3 Magnifier | gfx (+ accessibility focus-follows) | **YES, in `recomposite` scanout** | no | **WM + screenshot waves (HARD — same scanout/recomposite)** |
| §4 Keyboard nav + modes | accessibility + ui + shell-apps + kernel | no | yes | shell wave + theme_engine owner |
| §5 Contrast audit | accessibility + visual-qa | no | no (ath_tokens) | nothing — parallel-safe |

**Hard serialization callouts:**
- §3 magnifier edits `recomposite`'s scanout step — the SAME code the WM/overview and screenshot
  waves edit. Schedule it in its own gfx wave AFTER those settle, or grant a merge window.
- §2's one-line focus-generation bump in `compositor::focus_surface` must land between WM waves.
- §1, §5 touch no locked files — they can land NOW (pending the architect interface for §1).

---

## 7. NEEDS-INTERFACE (athena-architect, land FIRST in an `[interface]` commit)

Verbatim from `phase19-accessibility-foundation.md §7` (do not duplicate the work here):
1. `Cap::Accessibility { rights: Rights }` (READ = read tree, WRITE = dispatch actions) in
   `ath_abi` + `kernel/src/capability.rs`. Additive enum -> **bump `ABI_VERSION`** + update
   `docs/SYSCALL_TABLE.md` same commit.
2. `SYS_A11Y_SNAPSHOT` + `SYS_A11Y_ACTION` (architect assigns next free numbers; last allocated
   was 273 per the clipboard `[interface]` commit `bbb3276` — confirm against
   `docs/SYSCALL_TABLE.md`). Number in `ath_abi` + dispatch arm in `syscall.rs` same commit.
3. `repr(C) A11yNode { id, parent, role, state, x, y, w, h, actions, name[48] }` in `ath_abi`
   (~80 bytes, inline name like `ThemeInfo`/`SurfaceHdr`). Copy-to-user via validated
   `copy_to_user` (the net/theme syscall fix pattern — no raw deref).

The screen-reader focus event, magnifier control, and mode flags do NOT need new ABI for the
*kernel-internal* smoketests; they only need ABI if exposed to userspace AT clients (a later
batch — keep it out of the first interface commit).

---

## 8. RECOMMENDED BUILD ORDER (highest fan-out first)

1. **§1 Accessibility tree (kernel `a11y.rs` window tier).** *First — everything depends on it.*
   The reader walks it, the magnifier focus-follows off its focus, keyboard nav drives its
   `focus_next/prev`, the contrast audit is in the same module. **Gated on the architect's
   interface commit (§7) landing first.** Touches no locked files. Fan-out: maximal.
   - Sub-slice 1a (parallel, athena-ui): fill `athui::accessibility::infer_role/infer_label`
     stubs + `Accessible` impls so the tree NAMES things (the reader is worthless until this).

2. **§5 Contrast audit.** *Cheapest real win, zero locked-file risk, ships a ship-gate proof
   immediately.* Pure `contrast_ratio` over the token palette as a host-KAT + boot smoketest. Can
   land in parallel with §1. Proves a parity property no competitor ships.

3. **§4 Keyboard nav + modes (focus order, ring, HC/reduced-motion/text-scale).** *Most testable
   a11y property; reuses the existing `focus_next/prev` + `FocusRing` + animation `reduce_motion`
   hooks.* Host-KAT the focus/modal/mode logic now; the live-typing leg is iron-pending. Serializes
   against the shell wave for per-surface Tab order, but the engine + modes land independently.

4. **§2 Screen reader (announcer + focus event + log sink).** *Depends on §1 (tree) + 1a (labels).*
   The one-line compositor focus-bump serializes against the WM wave; everything else is additive.
   Log/braille sink ships now; TTS is the iron/audio tail.

5. **§3 Magnifier.** *LAST of the build pieces because it HARD-serializes against the in-flight
   WM + screenshot waves (same `recomposite` scanout).* Correctness is host-KAT + QEMU-provable
   the moment the gfx merge window opens; smooth-pan feel is iron-perf-pending.

**Recommended FIRST implementation slice for the dedicated wave:** **§1 the kernel `a11y.rs`
window-tier tree** (after athena-architect lands the §7 interface), with **§5 the contrast audit
as the parallel companion** (no interface needed, ships a ship-gate proof on its own). Together
they (a) stand up the data model every later piece consumes and (b) put two FAIL-able a11y
proofs in the boot log immediately — converting `PARITY_MATRIX §J` from all-`[ ]` to its first
`[~]` rows. athena-ui's `infer_*` fill (1a) runs alongside so the tree names real widgets.
