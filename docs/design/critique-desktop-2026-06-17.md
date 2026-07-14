# Visual-QA Critique — Desktop/Shell iteration — 2026-06-17

> raeen-visual-qa. Boots it, screenshots it, compares it to the spec and to the
> best desktops in the world, says precisely what's wrong. No UI code written —
> see, compare, report.

## TL;DR

- **The live desktop could NOT be reached in headless CI** — boot lands and stays
  on the OOBE first-boot wizard. The `desktop_autologin` cargo feature (compiled
  in, confirmed) did NOT bring the desktop up: its auto-advance thread produced
  **zero serial output** (not even its unconditional log line). So the
  taskbar / Start / tray / window-chrome critique the brief asked for is
  **BLOCKED** and handed to raeen-verifier + raeen-debugger.
- I captured the furthest clean frame (the OOBE card) and critiqued every chrome
  surface it shares with the desktop: AA Inter type, glass card material, accent
  button, input fields, drop shadow, corner radius, accent usage.
- **Headline finding (highest visual impact):** the drop shadow is not a soft
  shadow at all — it is a **hard, opaque, blue, offset duplicate rectangle**.
  This is the single biggest "looks basic, not premium" signal now that the
  blocky font is gone.
- **The AA Inter rollout is a real win** — text is genuinely crisp and reads as
  Inter at every size in the frame. The remaining type issues are color/weight/
  spacing polish, not the old "blocky bitmap" problem.

---

## Boot / capture

- **Booted to:** OOBE first-boot setup wizard (`SessionPhase::FirstBootSetup`),
  1280×800, UEFI + `--production`, TCG, SMP=2. No PANIC. AA-text + chrome
  smoketests PASS in the serial log.
- **Build:** `RAEEN_KERNEL_FEATURES=kernel/desktop_autologin cargo run -p xtask
  --release -- build --release --uefi --production` (feature confirmed compiled
  in — `[xtask] extra kernel features: kernel/desktop_autologin` +
  `[xtask] Building kernel (production)`).
- **Capture:** `... run ... --ci --screenshot=...` (xtask QMP `screendump
  format=png` on the desktop sentinel + 8 s settle). 32bpp UEFI capture is clean
  (no PPM striping).
- **Screenshots saved:**
  - `docs/design/screenshots/desktop-uefi-2026-06-17.png` — full frame (the
    capture; **shows OOBE, not desktop** — see Blocking).
  - `docs/design/screenshots/BLOCKED-oobe-not-desktop-2026-06-17.png` — same
    frame, named honestly for the record.
  - `docs/design/screenshots/zoom-title-3x.png` — title/subtitle AA @ 3×.
  - `docs/design/screenshots/zoom-body-2x.png` — labels/fields AA @ 2×.
  - `docs/design/screenshots/zoom-button-3x.png` — accent button text @ 3×.
  - `docs/design/screenshots/zoom-shadow-corner-3x.png` — the shadow defect @ 3×.
- **Against spec:** `docs/design/design-language.md` (tokens), `desktop-shell.md`,
  `typography-rendering.md`.

---

## BLOCKING — desktop unreachable in headless CI (hand to verifier + debugger)

The brief asked for the **live desktop** (taskbar/Start/tray/window). It cannot
be screenshotted headlessly today:

- Fresh boot has `/setup/first_boot_done` unset → `initial_phase =
  FirstBootSetup`. There is **no config knob** to pre-complete OOBE, and the
  capture sentinel (`kernel surface 1 created … z=0, desktop`) fires on the first
  compositor surface regardless of phase, so xtask captures the OOBE wizard.
- `kernel/src/shell_runner.rs::auto_advance_thread_entry` has a
  `#[cfg(feature = "desktop_autologin")]` block that should `login_guest()` +
  `activate_desktop()`. With the feature compiled in, **none of its serial
  markers appeared** — not the `desktop_autologin: guest desktop up` line, and
  not even the function's *unconditional* first `serial_println!`
  ("guest auto-advance removed …"). `grep -c "auto-advance"` on the serial log =
  **0**.
- Meanwhile a sibling kernel thread (`[xhci] HID input thread`) runs 3600+
  iterations on CPU0 in the same window, so CPU0 *is* scheduling — the
  auto-advance thread specifically is never reached. Suspect: the thread spawned
  via `scheduler::spawn_on_bsp` at the tail of `shell_runner::init` is not landing
  on the runqueue, or faults before its first instruction. (There is also a
  `[PAGE FAULT] Killing faulting task T1` — user_init — in the same boot;
  unclear if related.)

**Owner: raeen-verifier (reproduce: feature compiled, thread silent) +
raeen-debugger (root-cause why the BSP-pinned auto-advance thread never executes
while the HID thread does).** Until this is fixed, no desktop-shell screenshot
exists and the taskbar/Start/tray polish cannot be visually verified — only the
boot-log smoketests assert it.

---

## What landed well (call it out — it is real progress)

- **Crisp AA Inter is in and it works.** `[gfx] draw_text_aa smoketest:
  families=2 face=Inter cov=53401 … PASS`; `[chrome] titlebar … glyphs=real
  text=aa controls=left … PASS`. The 3× zooms confirm: outlines, grayscale AA,
  legible at 22 px title and 13–14 px body. This kills the #1 "basic" signal from
  the last iteration (the 8×8 block font). Everything below is now polish, not a
  rebuild.
- **Accent fidelity is exact** where it's used as a fill: the "Create account"
  button measures `(78,156,255)` = `0x4E_9C_FF` = `accent.base` RaeBlue, to the
  byte. The cohesion engine is feeding the real token.
- **Button text** is well-centered, crisp, correct contrast (white on accent).
- **Field placeholder** "Enter a password" is correctly `text.tertiary` gray;
  the focused field has the accent ring.

---

## Findings (ranked by visual impact — each actionable)

### 1. Drop shadow is a hard opaque offset block, not a soft shadow — owner: raeen-gfx
- **Surface:** OOBE card (and, by shared code, every `elev.*` surface — windows,
  flyouts, Start, toasts).
- **Observed:** below the card bottom edge (y=647) the pixel snaps to a flat
  `(78,123,200)` and holds ~12 px, then hard-cuts back to wallpaper at y=661 — a
  **solid down-right offset duplicate of the card**, with sharp inner AND outer
  edges and a hard-rounded corner (see `zoom-shadow-corner-3x.png`). Right edge:
  same — abrupt `(120,152,205)` band ~6 px then a step. There is **zero
  feathering / penumbra**, and the color is **blue**, not near-black.
- **Should be:** `elev.4` (OOBE card) = `offset_y 12, radius 40, color
  0x66_00_00_00` with the documented **quadratic falloff** — a wide, soft,
  near-black-at-~40%-alpha penumbra. `compositor::render_drop_shadow` is supposed
  to do quadratic falloff; either it isn't being invoked for this surface (the
  card may be drawing its own flat offset rect) or the falloff term is collapsing
  to a constant and the color is inheriting the blue wallpaper instead of
  near-black.
- **Reference gap:** macOS Sequoia and Win11 dialog/card shadows are wide soft
  ambient shadows (large blur radius, low alpha, neutral/near-black). This frame
  reads as a flat sticker with a colored ledge — the dominant "not premium"
  signal now that type is fixed.

### 2. Headings/labels rendered in blue instead of neutral text tokens — owner: raeen-ui (+ raeen-shell-apps for the surface)
- **Surface:** OOBE card title, subtitle, and field labels (shared chrome
  pattern — likely affects Settings group headers and other labels too).
- **Observed:** title "Welcome to AthenaOS" stems measure `(115,134,167)` — a
  desaturated navy; subtitle "Let's set up your account" and the "Username" /
  "Password" labels render in **accent blue** (see zoom crops).
- **Should be:** on the light palette, `text.primary = 0xFF_14_18_22` (near-black)
  for the title, `text.secondary = 0xFF_45_4C_5E` (neutral gray) for subtitle and
  labels. **Chrome stays neutral; accent is reserved for interactive/focus**
  (design-language §1 "restraint in chrome color", §4.3). Coloring static labels
  with the accent is exactly the macOS/Win11 lesson the spec warns against, and
  it also risks the AA `text.secondary` ≥4.5:1 contrast target
  (flag raeen-accessibility).

### 3. Focused field has a ring but no focus glow — owner: raeen-gfx (glow) / raeen-ui (token wiring)
- **Surface:** focused username field (and every focusable control by extension).
- **Observed:** the focused field shows a ~1px accent border only.
- **Should be:** `elev.focus` = additive `accent.glow` (`0x66_4E_9C_FF`, radius
  10) **plus** a 2px `accent.base` ring (design-language §8 / §5.3). Focus must be
  a glow + ring, never a bare 1px color change — this is both a premium cue and an
  a11y requirement. Currently indistinguishable-at-a-glance from a plain border.

### 4. Title weight reads heavy and letter-spacing is uneven — owner: raeen-ui
- **Surface:** title "Welcome to AthenaOS" (`type.title` = 22px / weight 600).
- **Observed (3× zoom):** stems look a touch bold/dark for 600 at 22px, and the
  caps run "AthenaOS" shows uneven inter-letter spacing; the word "to" sits a hair
  tight. Reads as either a slightly-off gamma in the AA blend (stems over-dark) or
  the wrong static weight instance / missing kerning pair application.
- **Should be:** gamma-correct grayscale blend (typography-rendering spec), the
  600 instance (not a faux-bolded 500), and `kern`-table pair kerning applied
  (raefont parses `kern`). Letter-spacing 0 for titles (design-language §6).
  Verify the AA gamma constant and that the shaper is applying kerning.

### 5. Card corner radius slightly tight + reads faintly polygonal — owner: raeen-gfx
- **Surface:** OOBE card corners.
- **Observed:** top-left corner curve spans ~18px (x 364→346 over y 152→170);
  the curve is acceptable but a touch faceted at 3× and below the spec value.
- **Should be:** `radius.xl` = 24px for the OOBE / full-screen modal card
  (design-language §3). Bump to 24 and confirm `Canvas` rounded-rect produces a
  continuous (squircle-leaning) edge, not a low-segment polyline, at this radius.

---

## Reference comparison (macOS Sequoia / Windows 11)

- **Shadow (vs both):** macOS and Win11 cards/dialogs use a wide, soft,
  low-alpha, neutral ambient shadow. AthenaOS's current hard blue offset block is
  the clearest gap — name it the top fix. (Spec target: `elev.4`, quadratic
  falloff, `0x66_00_00_00`.)
- **Chrome color (vs macOS):** macOS keeps chrome text neutral (label grays /
  near-black) and tints only controls/selection with the system accent. AthenaOS
  is currently tinting static headings+labels blue — off the spec's own
  "restraint in chrome color" rule.
- **Type (vs Win11 DirectWrite / macOS Core Text):** the new Inter AA is in the
  right ballpark; remaining gap is gamma/weight/kerning fine-tuning, not the
  category gap that existed last iteration.
- *(No public reference screenshots were embeddable from the searches; the
  comparison leans on the spec's own prior-art distillation in
  design-language §1 and established macOS/Win11 behavior. Sources below.)*

## Consistency issues

- Cannot run the full §6 cohesion test (taskbar + Start + titlebar same
  `accent.base`, concentric radii) — **desktop is unreachable** (see Blocking).
  The boot log asserts chrome accent `0xFF4E9CFF`; visual cohesion across shell
  surfaces is unverified this iteration.
- Within the captured frame: accent fill is consistent and exact; the
  shadow + label-color defects are systemic (shared chrome code), so fixing them
  once should propagate to all `elev.*` / labeled surfaces.

## Confidence

- **Medium-high** on the per-surface findings (shadow, label color, focus glow,
  type, radius) — all backed by pixel measurements + zoom crops on a clean frame.
- **High** on the blocking finding — feature confirmed compiled, thread output
  confirmed absent, sibling thread confirmed scheduling.
- **N/A** on the desktop-shell findings the brief actually wanted — the surface
  never rendered; do not trust any desktop critique until the block is cleared
  and a real desktop frame is captured.

## Sources
- [Windows 11 OOBE overview (Yahoo/Tech)](https://tech.yahoo.com/general/articles/whats-box-experience-oobe-windows-095618868.html)
- [macOS Sequoia Setup Assistant (Mac Install Guide)](https://mac.install.guide/mac-setup/)
