# ADR 0005 — UI implementation wave & merge-safe sequencing (2026-06-21)

- Status: accepted
- Date: 2026-06-21
- Owner: raeen-lead (autonomous)

## Context
Green baseline confirmed at HEAD 544a696 (raeen-verifier: build exit 0, marker present,
7/7 HEALTHY, 8/8 smoketests, 0 panics; only the standing >6s total-boot WARN is red against
an absolute target). With a known-green tree, the marquee UI loop (charter Step 3.1) can fan
out.

raeen-design-researcher audited docs/design/ (already mature: 16 specs; login/OOBE and
window-chrome are essentially done and were deliberately NOT re-specced) and wrote three new
specs for the highest-gap surfaces:
1. docs/design/files.md — Files app re-skin (kills hardcoded FM_* palette + 8x8 block glyphs).
2. docs/design/material-and-shadow.md — the #1 "looks basic" defect: drop shadow renders as a
   hard, opaque, blue, offset block instead of a soft ambient shadow. Systemic — fix once in
   the compositor, every elevated surface improves.
3. docs/design/control-center.md — Quick Settings flyout (expand-in-place, media card, gaming
   fast-lane row). Depends on (2) landing first.

File locations (confirmed by grep, drives merge-safety):
- (2) -> kernel/src/compositor.rs::render_drop_shadow  = KERNEL CORE (raeen-gfx)
- (1) -> components/raeshell/src/file_manager.rs        = raeshell crate (raeen-shell-apps)
- (3) -> components/raeshell + raeui                    = (raeen-shell-apps + raeen-ui)

## Decision
- **Priority order:** (2) Material & Shadow first (systemic, highest fan-out — every elevated
  surface inherits the fix), then (1) Files (table-stakes switcher surface), then (3) Control
  Center (gated on (2)).
- **Sequencing for merge safety:** raeen-perf is currently doing a full `xtask run` to measure
  the boot-time gate; mutating kernel/src/compositor.rs during that build would corrupt it and
  pollute the measurement. Hold the kernel-core shadow work (2) until raeen-perf clears. Then
  run (2) raeen-gfx [kernel] and (1) raeen-shell-apps [raeshell] in PARALLEL — different
  crates, merge-safe. (3) is deferred until (2) verifies.
- **Proof path caveat:** the live desktop is currently unreachable in headless CI (noted by
  the design-researcher and ADR 0004), so full visual-QA pixel proof is itself blocked. Until
  that is cleared, each UI item is proven by its boot-log smoketest assertions (each spec
  defines them) + host-render; visual-QA screenshot proof is a follow-up gated on the
  desktop-in-CI unblock (candidate raeen-debugger investigation, tracked separately).

## Rationale
Tie-breaker: (1) Concept says UI is the differentiator; (2) the shadow defect is the most
visible "basic" tell and fixing it in the compositor lifts every surface at once; (3) crate
separation keeps the wave merge-safe; (4) doc-specced + smoketest-gated work is cheaply
reversible.

## How to reverse
Re-order or drop any of the three by editing this ADR's priority list; each spec is
independent and its implementer change is a single-crate diff.
