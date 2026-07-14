# Accessibility Audit + Prioritized Plan — 2026-06-21

**Author:** raeen-accessibility (read-only audit; only this doc was written)
**Scope:** the 7 a11y ship-gate dimensions vs Windows (Narrator/UIA, High Contrast, Magnifier,
Color Filters) and macOS (VoiceOver/AX, Full Keyboard Access, Zoom, Reduce Motion, Increase
Contrast). a11y is **ship gate #7** and was the largest unowned parity gap.
**Method:** verified against live source, not the planning docs. Several prior planning docs
(`accessibility-implementation-plan.md`, `phase19-accessibility-foundation.md`) describe the
work as not-yet-started; **that is stale** — most of the kernel-side foundation has since
landed. This audit supersedes their status claims.

**Concept lines served:** "Built for people who care about how things feel." (a11y is a
feel-of-the-OS property) · Security: "OS enforces capabilities at the syscall layer" (the AT
API is its own privilege class).

---

## TL;DR — what is already built (do NOT re-spec)

The kernel a11y stack is **much further along than the planning docs imply.** Verified live:

- **`kernel/src/a11y.rs` (1236 lines) is real and wired:** accessibility tree built from the
  live compositor surface list (`build_tree`), R10 contract complete (init line, 4 FAIL-able
  smoketests, `/proc/raeen/a11y`, Concept docstring), `init()` called from `kernel_main` after
  `compositor::init` (main.rs:560), smoketest run (main.rs:1522).
- **AT ABI is live & cap-gated:** `SYS_A11Y_SNAPSHOT` (277) + `SYS_A11Y_ACTION` (278) dispatched
  in `syscall.rs`, both gated on `Cap::Accessibility{READ/WRITE}` (fails CLOSED), validated
  `copy_to_user`. `A11yNode`/`A11ySnapshotHeader` wire reprs in `rae_abi`.
- **Screen-reader core is real:** `announce_node`/`describe_focused` produce conventional
  VoiceOver/Narrator phrasing ("Save, button", "Search, text field, editing", "Wi-Fi, switch,
  on"), focus-generation polling counter, pluggable `SpeechSink` with a QEMU-provable
  `LogSpeechSink` installed at boot.
- **Magnifier is real:** `kernel/src/compositor.rs` does a source-sampled scanout **upscale**
  (`MAG_ENABLED`/`MAG_ZOOM_X256`/`magnifier_set_*`), with a FAIL-able smoketest
  (`run_magnifier_smoketest`) and **focus-follows pan** wired through `a11y::follow_focus_in`.
- **Color filters are real:** compositor `a11y_filter_set` does invert / grayscale /
  high-contrast as a per-pixel scanout post-process (`run_a11y_filter_smoketest`).
- **raeui widget semantics exist:** `components/raeui/src/accessibility.rs` has real role
  inference (`role_from_widget_kind`, NOT the old `Group` stub), `provider_nodes_for_window`,
  `focus_next/prev`, `describe_focused`, `HighContrastPalette`, `FocusRing`, with host KATs.
- **Contrast math exists & is tested:** `rae_tokens::contrast_ratio` (WCAG luminance) +
  `HighContrastPalette` mirror, unit-tested at >= 4.5 / >= 7.0.
- **Reduced-motion is honored (partially):** `shell_runner::reduced_motion()` reads
  `config_registry "/a11y/reduced_motion"` and gates an animation; `raeshell::animations` has
  `set_reduce_motion`; `rae_tokens` has `MOTION_INSTANT`/`REDUCED_MOTION_DURATION_MS`.
- **Standalone a11y library:** `components/raeaccessibility` (2268 lines) — rich AT data models
  (TTS engine, braille, switch access, eye tracking, voice control, on-screen keyboard). **See
  the BLOAT FLAG below — it is a workspace member that NOTHING imports.**

**The headline gaps are integration & user-reach, not greenfield engines:**
1. The widget-tier provider seam exists in BOTH the kernel (`publish_window_widgets`) and raeui
   (`provider_nodes_for_window`) but **nothing connects them** — so to a screen reader every app
   is ONE anonymous "Window" node with zero internal buttons/fields. This is the #1 gap.
2. **No user-facing on-switch:** magnifier, color filters, screen reader, reduced-motion have
   no hotkey, no Settings toggle, no Control-Center tile. A blind user cannot turn on the
   screen reader without an AT client that doesn't exist.
3. **No high-contrast / forced-colors *mode*** (the palette exists; nothing swaps the live UI to
   it). `[ ]` in PARITY_MATRIX §J.
4. **No global text-scaling.**
5. **Main desktop shell keyboard nav is bespoke per-surface, not a unified Tab focus order**
   with a consistent visible focus ring across taskbar/start/tray/notifications.

---

## Dimension-by-dimension

Severity scale: **P0** = blocks a disabled user from basic use / ship-gate red ·
**P1** = parity gap a reviewer will catch · **P2** = polish.

### (a) Accessibility tree / screen-reader support
**EXISTS:** kernel tree over compositor surfaces (`a11y.rs:build_tree`); cap-gated AT ABI
(277/278); screen-reader announce core + `SpeechSink` + `LogSpeechSink`; focus-generation poll;
raeui widget role inference + `provider_nodes_for_window`. Status `[~]` in PARITY_MATRIX §J.

**GAP vs Win/macOS:**
- **Widget provider not connected (P0).** `publish_window_widgets` / `provider_nodes_for_window`
  exist but have **zero callers** — the live tree is window-tier only. VoiceOver/Narrator name
  every control; AthenaOS names only window titles. A reader is near-useless until apps publish
  their widget subtrees.
- **No real TTS (P1).** `LogSpeechSink` proves logic in QEMU; there is no `AudioSpeechSink` to
  AthAudio PCM, so nothing is actually *spoken*. Iron/audio-gated (Phase 7).
- **No reader navigation commands / gestures (P1):** no "next heading/link/form-field" verbs
  driven by live keys (the verbs exist in the unused `raeaccessibility` crate, not the live path).

**Owner:** widget provider wiring = **raeen-ui** (publish) + **raeen-shell-apps** (per-app
nodes); TTS sink = **raeen-accessibility** + **raeen-audio** (iron). 
**Acceptance:** focusing a control in a real app makes `/proc/raeen/a11y` show a named child
node under that window with correct role/bounds; `describe_focused()` returns
"`<label>, <role>`" for it; FAIL-able boot/host KAT proves a published widget round-trips.

### (b) Full keyboard-only navigation + visible focus
**EXISTS:** raeui `focus_next/prev` + `focusable_nodes()` + `FocusRing::from_node`; per-surface
key handlers (command palette, clipboard panel, webview, capture overlay, gameos/couch all
handle Tab/arrows); `gameos.rs` draws an explicit `draw_focus_ring` (ring + glow + scale + top
cue = four redundant signals — exemplary). Token `elev.focus` + HC `focus_ring` cyan exist.

**GAP vs Win/macOS:**
- **No unified desktop focus order (P0).** The main desktop shell (taskbar, start, system tray,
  notification center, control center) has **no Tab traversal across chrome elements** and no
  consistent visible focus ring on them — keyboard nav is ad-hoc per overlay. macOS Full
  Keyboard Access / Windows Tab+arrows reach every chrome affordance; AthenaOS does not.
- **No focus trapping contract in modals (P1):** modal flag exists in raeui traits; the
  Tab-wrap-within-dialog behavior is not enforced at the shell level.
- **"No mouse required" is unproven (P1):** no audit asserts every flow is keyboard-reachable.
  This is the single most testable a11y property and there is no test for it.
- Live key delivery is iron-gated (HID typing pending on the board) — QEMU proves logic only.

**Owner:** **raeen-accessibility** (focus-order contract + ring spec + the "no-mouse" audit) +
**raeen-shell-apps** (Tab order per surface) + **raeen-ui** (per-widget focusability).
**Acceptance:** a host-KAT/boot smoketest seeds a surface tree incl. a modal and asserts Tab
advances through exactly the focusables, wraps, Shift+Tab reverses, and focus is TRAPPED in the
modal subtree; every shell chrome element exposes a focus ring (ring shape, never color-only).

### (c) Screen magnifier
**EXISTS:** **the engine is done.** Compositor source-sampled scanout upscale
(`magnifier_set_enabled/zoom/center`, zoom 1x-8x), FAIL-able `run_magnifier_smoketest`
(identity-at-1x byte-match, center-samples-focus, edge-clamp), focus-follows pan via
`a11y::follow_focus_in` + its own smoketest. Composes correctly with color filters (sample then
filter). Status `[~]`.

**GAP vs Win/macOS:**
- **No toggle hotkey and no Settings/Control-Center switch (P0 for reach).** PARITY_MATRIX §J
  line 132 confirms: "no toggle HOTKEY wired." The magnifier physically cannot be turned on by a
  user. Win+`=` / macOS Cmd-Opt-`=` are the parity baseline.
- **No smooth-pan animation (P2):** center snaps; macOS Zoom eases. (Honor reduced-motion: snap
  when reduced.)
- **Lens/docked modes absent (P2):** full-screen zoom only.
- iron perf (60fps on a real panel) unproven.

**Owner:** hotkey + Control-Center tile = **raeen-shell-apps**; smooth-pan/lens =
**raeen-gfx** + **raeen-accessibility** policy. 
**Acceptance:** a global shortcut toggles `magnifier_set_enabled` and steps zoom; a
Control-Center "Magnifier" tile reflects/sets the state; reduced-motion makes the pan instant.

### (d) High-contrast / forced-colors mode
**EXISTS:** `rae_tokens` `HighContrastPalette` mirror + HC cyan focus ring + the compositor
`A11Y_FILTER_HIGH_CONTRAST` per-pixel filter; raeui `HighContrastPalette` + `high_contrast_colors()`.

**GAP vs Win/macOS:**
- **No live forced-colors mode (P0).** Status `[ ]` (PARITY_MATRIX §J line 133: "tokens exist;
  no a11y palette" swap). The HC palette is defined but **nothing makes the running UI render in
  it** — there is no global `HIGH_CONTRAST` flag that `active_palette()` honors so every surface
  repaints in HC. Windows High Contrast / macOS Increase Contrast both restyle the whole UI.
- The compositor HC *filter* is a blunt post-process (good as a fallback) but is NOT the
  token-level palette swap that yields readable, intentional HC chrome.

**Owner:** **raeen-accessibility** (the flag + `active_palette()` contract) + **raeen-ui** /
**raeen-shell-apps** (read the active palette). theme_engine currently has **no** HC flag.
**Acceptance:** setting a global HC flag makes shell + apps pull the HC palette (one token
lookup); a boot smoketest asserts `active_palette()` returns HC when the flag is set; the HC
contrast audit (dimension f) passes at >= 7:1.

### (e) Reduced-motion
**EXISTS:** `shell_runner::reduced_motion()` reads `config_registry "/a11y/reduced_motion"` and
gates an animation (shell_runner:3949); `raeshell::animations::set_reduce_motion`; `rae_tokens`
`MOTION_INSTANT` / `REDUCED_MOTION_DURATION_MS = 0`; raeui `animation.rs` reference.

**GAP vs Win/macOS:**
- **Not honored everywhere (P1).** The hook exists but only ONE shell_runner animation site is
  verified to read it. Compositor window/overview/genie animations, toast slide-ins, glassmorphic
  transitions, gameos motion — none are audited to honor the flag. macOS Reduce Motion is
  comprehensive.
- **No user toggle (P1):** `/a11y/reduced_motion` is a config key with no Settings/Control-Center
  switch to flip it.
- **No audit (P1):** nothing asserts each animation site collapses to instant when set.

**Owner:** **raeen-accessibility** (the audit + single source-of-truth flag) + **raeen-gfx**
(compositor animation sites) + **raeen-shell-apps** (shell/app animations).
**Acceptance:** a single global reduced-motion flag; every animation entry point reads it; a
FAIL-able audit/smoketest samples representative animations and asserts duration 0 when set.

### (f) Color-contrast compliance (WCAG AA 4.5:1 text / 3:1 UI)
**EXISTS:** `rae_tokens::contrast_ratio` (WCAG relative-luminance) + unit tests asserting
`RAEBLUE/bg >= 4.5`, `text_primary/bg`, HC palette "exemplary" >= 7.0.

**GAP vs Win/macOS (this is a parity WIN opportunity — nobody ships this):**
- **No FAIL-able boot/CI contrast audit over the full palette (P1).** The fn + a few tests
  exist, but there is no enumerated audit of every load-bearing painted pair (text_primary,
  secondary text, accent-on-bg, focus-ring-on-bg, glass-overlaid text, HC pairs) that prints a
  PASS/FAIL line naming the worst pair + ratio. The OS could *prove* WCAG AA on its own palette
  at boot — a parity property no competitor ships.
- **No measured-from-screenshot counterpart (P2):** rendered pixels differ from token intent
  (blur/blend over glass); needs **raeen-visual-qa** measured ratios (heed the QEMU screendump
  stripe artifact — measure on host-render / iron).

**Owner:** **raeen-accessibility** (token audit) + **raeen-visual-qa** (measured).
**Acceptance:** a host-KAT + boot smoketest enumerates >= 12 painted pairs, asserts body text
>= 4.5, UI/large/focus >= 3.0, HC >= 7.0, and on failure prints `worst_pair="<a/b>=<ratio>"`.

### (g) Text scaling
**EXISTS:** raeui layout computes bounds; rae_tokens type ramp exists.

**GAP vs Win/macOS:**
- **No global text-scale factor (P1).** No 1.0/1.25/1.5/2.0 scale that layout reads; no Settings
  toggle. Windows display scaling + macOS Larger Text are baseline. Must not break layout.
- **No layout-fit audit (P2):** a scaled pass must not clip/overlap (checkable since raeui
  computes bounds).

**Owner:** **raeen-ui** (layout reads the factor) + **raeen-accessibility** (the flag + fit
audit) + **raeen-shell-apps** (settings toggle).
**Acceptance:** a global scale factor; a boot smoketest asserts a 2x pass produces no
clipped/overlapping bounds in a seeded layout.

---

## BLOAT FLAG — `components/raeaccessibility` (anti-bloat house rule)

`components/raeaccessibility` is a **2268-line workspace member that NOTHING imports** (no
Cargo dependency, no `use`, no `init()` caller, no R10 contract, no procfs). It duplicates the
data shape now live in `kernel/src/a11y.rs` + `raeui::accessibility` and adds speculative
engines (eye tracking, voice control, braille, SSML TTS) with no boot artifact and no consumer.
Per the anti-bloat rule (~1000 LOC with no new shipped capability), recommend ONE of:
- **(preferred)** demote to a clearly-marked `[experimental]`/reference module and stop carrying
  it as a built member, OR
- harvest the few genuinely-needed pieces (e.g. the braille cell map, the reader navigation
  verbs) into the LIVE `kernel/src/a11y.rs` path with R10 artifacts, and delete the rest.
Do not invest further in it as-is — the live path is the kernel a11y.rs + raeui seam.

---

## TOP 5 — ranked by leverage (route the next a11y wave from this)

### 1. [P0] Connect the widget-tier provider (apps name their controls)
The single highest-leverage item: both ends of the seam exist
(`a11y::publish_window_widgets` / `raeui::provider_nodes_for_window`) with **zero callers**, so
every app is one anonymous window to a screen reader. Wire raeui to publish each window's widget
subtree (real role/label/bounds) into the kernel tree, starting with one real app (Settings or
Files).
- **Owner:** raeen-ui (publish path) + raeen-shell-apps (per-app nodes).
- **Acceptance:** focusing a control in a live app makes `/proc/raeen/a11y` list a NAMED child
  node under that window with correct role/bounds; `describe_focused()` returns
  "`<label>, <role>`"; a FAIL-able KAT proves a published widget round-trips through the tree.

### 2. [P0] User-facing a11y on-switches (hotkeys + Control-Center + Settings)
Every engine is built but **unreachable by a user.** Wire global hotkeys + a Control-Center
"Accessibility" group + Settings toggles for: magnifier (`magnifier_set_enabled`/zoom), color
filters (`a11y_filter_set`), screen reader on/off, reduced-motion, high-contrast. Without this
the shipped engines are dead.
- **Owner:** raeen-shell-apps (shortcuts + Control-Center tile + Settings) — calls existing
  kernel fns; coordinate the keybind slot with shell_runner.
- **Acceptance:** a documented shortcut toggles the magnifier and steps zoom; a Control-Center
  Accessibility group reflects/sets magnifier + filter + reduced-motion; reduced-motion toggle
  flips `/a11y/reduced_motion`.

### 3. [P0] High-contrast / forced-colors LIVE mode (palette swap, not just a filter)
Promote the existing `HighContrastPalette` to a global `HIGH_CONTRAST` flag that
`active_palette()` honors, so shell + apps repaint in HC via one token lookup (the
already-universal `rae_tokens::DARK/LIGHT` read path). This closes the only fully-`[ ]` row in
PARITY_MATRIX §J.
- **Owner:** raeen-accessibility (flag + `active_palette()` contract) + raeen-ui /
  raeen-shell-apps (read it).
- **Acceptance:** setting the flag makes a seeded shell surface pull the HC palette; boot
  smoketest asserts `active_palette()==HC` when set; HC pairs pass the >= 7:1 contrast audit.

### 4. [P1] Unified desktop keyboard focus order + visible ring + modal trap + "no-mouse" audit
The most TESTABLE a11y property. Build ONE focus-order contract across the desktop shell chrome
(taskbar/start/tray/notifications/control-center) driven by raeui `focus_next/prev`, a
consistent visible focus ring (`elev.focus`, HC override cyan; never color-only), Tab-trapping
in modals, and a FAIL-able "every flow keyboard-reachable" audit.
- **Owner:** raeen-accessibility (contract + audit) + raeen-shell-apps (per-surface Tab order) +
  raeen-ui (focusability).
- **Acceptance:** host-KAT/boot smoketest: Tab advances through exactly the focusables, wraps,
  Shift+Tab reverses, focus TRAPPED in a modal subtree; every chrome element shows a focus ring.

### 5. [P1] FAIL-able WCAG contrast audit at boot (the parity WIN nobody ships)
Wire `rae_tokens::contrast_ratio` into an enumerated boot/host audit over every load-bearing
painted pair; print PASS/FAIL naming the worst pair + ratio. Cheapest real win, zero
locked-file risk, ships a ship-gate proof no competitor ships. Pair with raeen-visual-qa for
the measured-from-screenshot counterpart.
- **Owner:** raeen-accessibility + raeen-visual-qa (measured).
- **Acceptance:** audit over >= 12 pairs asserts body >= 4.5, UI/focus >= 3.0, HC >= 7.0; on
  failure prints `worst_pair="<a/b>=<ratio>" -> FAIL`.

---

## Already partially built — do NOT re-spec these from scratch

| Dimension | Status | Live evidence |
|---|---|---|
| Accessibility tree | `[~]` built | `kernel/src/a11y.rs` (tree, R10, procfs, cap-gated ABI 277/278) |
| Screen-reader announce core | `[~]` built | `a11y.rs:announce_node/describe_focused` + `SpeechSink`/`LogSpeechSink` |
| Magnifier engine | `[~]` built | `compositor.rs` scanout upscale + focus-follows + smoketest |
| Color filters (invert/grayscale/HC) | `[~]` built | `compositor.rs:a11y_filter_set` + smoketest |
| Widget role inference | built (host-KAT) | `raeui/src/accessibility.rs:role_from_widget_kind`, `provider_nodes_for_window` |
| Contrast math | built (unit-tested) | `rae_tokens::contrast_ratio` + `HighContrastPalette` |
| Reduced-motion hook | partial | `shell_runner::reduced_motion()` + `animations::set_reduce_motion` + `MOTION_INSTANT` |
| Focus ring (drawing) | partial (gameos only) | `gameos.rs:draw_focus_ring` (4-signal); `raeui FocusRing` |

The remaining work is **integration, user-reach, and audits** — not new engines. The one place
to be careful of duplication is `components/raeaccessibility` (see Bloat Flag): build on the live
`kernel/src/a11y.rs` + `raeui` seam, not that orphaned crate.
