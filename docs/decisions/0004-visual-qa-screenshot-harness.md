# ADR 0004 — Visual-QA screenshot capture path (the marquee's missing proof)

- Status: accepted (plan); implementation queued
- Date: 2026-06-17
- Owner: raeen-lead (autonomous)

## Context
Goal #1 requires the UI be "proven by raeen-visual-qa screenshots judged against
current macOS/Windows 11 references." Today there is NO way to produce that image
automatically:
- xtask runs QEMU with `-display none` and has no screendump/screenshot path.
- Subagent Bash is sandboxed (ADR 0002), so raeen-visual-qa cannot boot QEMU — the
  lead must capture and hand it the image.
- Project memory (`ui-glass-design-system`): headless QEMU `screendump`→PPM→PNG has
  produced striping artifacts (a capture-pipeline bug, not the real render — iron +
  host-render were clean). So a naive screendump may be unusable.
- The live desktop only appears AFTER OOBE/login. A fresh boot parks on the
  FirstBootSetup wizard and (by design, ADR-era fix) does NOT auto-advance out of
  FirstBootSetup — it needs keyboard input. So an automated capture lands on the
  OOBE wizard, not the re-skinned taskbar/desktop, unless a dev knob bypasses OOBE.

## Decision
Build the screenshot harness in xtask (lead-owned) as a dedicated, incremental
capability, in this order:
1. `xtask screenshot` subcommand: boot the existing image with a QMP monitor
   (`-qmp tcp:127.0.0.1:PORT,server,nowait`, keep `-display none` — the framebuffer
   device still renders internally so `screendump` works), tail the serial log for
   a capture sentinel, issue QMP `screendump`, convert the result to PNG, and verify
   the image is NOT striped (correct stride/bpp — the documented artifact to fix).
2. First target = the OOBE first-boot wizard (what a fresh boot actually shows) —
   validates the capture pipeline AND gives visual-qa a real, high-visibility glass
   surface to critique immediately.
3. Then a dev-only boot knob (`RAEEN_SCREENSHOT`/feature) that marks first-boot
   complete + logs in guest so the boot lands directly on the desktop, enabling
   capture of the re-skinned taskbar/Start/chrome/toasts.

Until (1) lands, the UI loop proceeds on boot-log-provable token wiring (every
re-skinned surface prints a FAIL-able `[shell]/[chrome]/[notify] ... -> PASS` line
asserting it consumes rae_tokens), and visual critique is deferred — explicitly
tracked, not silently skipped.

## Rationale
Tie-breaker: (1) the Concept/goal demand visual proof, so the harness is required,
not optional; (3) incremental (OOBE first, desktop next) is the simplest correct
path that de-risks the striping unknown early; (4) reversible — a pure additive
xtask subcommand + a dev-only boot knob, no production-path change. Doing it in
xtask (Rust) rather than ad-hoc shell makes the QMP handshake robust and reusable.

## Findings (2026-06-17 investigation)
- **BIOS capture works but stripes:** `screendump format=png` produces a 1280×720
  PNG, but the content tiles ~5× horizontally with a moiré — a **24bpp-read-as-32bpp**
  mismatch. Confirmed: BIOS/VBE framebuffer is 24bpp.
- **UEFI is the clean path:** an `xtask run --uefi --ci` boot logs
  `[gop] verify OK: 1280x800 ... bpp=32` — a true 32bpp GOP framebuffer (and how
  Athena actually boots). A `screendump` of that will be clean. UEFI boots fine
  via xtask (60 KB serial, post-boot threads running), so clean capture IS
  achievable.
- **The standalone PS harness can't boot UEFI yet:** even mirroring xtask's
  pflash-first + dummy-virtio + isa-debug-exit args, the harness's QEMU produces
  no serial (Start-Process hides QEMU stderr, so the launch failure is opaque).
- **A fresh boot parks on the OOBE wizard** (`first-boot setup wizard ready`,
  1280×800) — it does NOT auto-advance out of FirstBootSetup, so capturing the
  re-skinned *desktop* additionally needs a dev OOBE-bypass + guest-login knob.

## Recommended next step (focused follow-up)
Integrate QMP screendump into xtask's `run_qemu` (which provably boots UEFI):
a `--screenshot=<path>` flag that runs QEMU non-blocking with `-qmp`, polls the
serial marker, screendumps PNG, and exits. This reuses the known-good arg set
instead of re-deriving it in PowerShell. Then add the OOBE-bypass dev knob for
desktop capture. Until then, UI correctness is gated by the boot-log token-proof
lines + the `[theme] accent-cohesion` dynamic test (strong functional proof);
pixel-critique is the deferred refinement.

## RESOLVED (2026-06-17) — working capture path
`xtask run --uefi --production --ci --screenshot=PATH` produces a **clean 32bpp
PNG** of the live desktop/OOBE. The pieces that made it work:
- QMP `screendump format=png` issued from xtask's known-good run path (not a
  standalone harness) — `qmp_screendump()` in xtask, a hand-rolled QMP client.
- `--uefi` → 32bpp GOP framebuffer (no BIOS 24bpp striping).
- `--production` → smoketests gated + console mirror OFF → a clean desktop
  (no scrolling-log scribble) AND a faster boot that fits the window.
- A **desktop-up sentinel** (`kernel surface 1 created ... (z=0, desktop)`) as
  the capture trigger, because `--production` suppresses the boot marker's serial
  line. Fires exactly when the framebuffer is composited.
- Screenshot-mode CI timeout bumped to 560s (UEFI/OVMF + ~72MB initramfs under
  TCG is slow; WHPX is blocked by the kernel high-PCI-BAR bug).
First captured surface: the OOBE wizard (`docs/design/screenshots/oobe-uefi-2026-06-17.png`)
— glass card, gradient + depth blobs, accent button. raeen-visual-qa now has a
real image to critique vs macOS/Win11. Desktop (post-login) capture still needs
an OOBE-bypass dev knob (the first boot parks on OOBE); tracked.

## How to reverse
Delete the `--screenshot` flag + `qmp_screendump()` from xtask; nothing else
depends on them. The boot-log token-proof lines stand on their own as the interim
gate.
