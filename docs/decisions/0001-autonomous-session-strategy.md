# ADR 0001 — Autonomous session orientation & strategy

- Status: accepted
- Date: 2026-06-17
- Owner: raeen-lead (autonomous)

## Context
A standing `/goal` directive points at `docs/AUTONOMY_CHARTER.md`. Iron testing is paused
(production-polish push). The agent roster (20) exists in `.claude/agents/`. The MasterChecklist
work log shows the current front line: desktop bring-up landed in QEMU (compositor starvation +
IF=0 deadlock fixed), glassmorphic OOBE iron-verified, several leak fixes, log-spam cleanup, and a
recent userspace-build break (raegfx VkPhysicalDeviceFeatures) that was repaired. Many backlog
items the owner lists are already built (verify-before-implement).

## Decision
Run the charter loop with this attention order, all QEMU-verifiable:
1. Establish a verified-green baseline first. Before delegating feature work, raeen-verifier does
   an independent build + QEMU boot + log-parse. The tree has silently gone red before behind a
   `| tail` mask, so everything gates on a known-green tree.
2. UI marquee loop (highest fan-out + visibility): design-researcher refreshes per-surface specs
   in docs/design/; raeen-ui / raeen-gfx / raeen-shell-apps implement; raeen-visual-qa critiques.
   Extend existing shadows/glass/HDR; do not rebuild.
3. Parallelize non-conflicting hardening: reviewer leak-hunt, perf boot-budget, fs hardening.
4. Multi-arch reach (arch-abstraction layer) once the UI loop has momentum; large, spec-first.

## Rationale
Tie-breaker: (1) Concept says UI is the differentiator; (2) UI is where we most visibly beat
Win/macOS; (3) verifying green first is the simplest correct prerequisite; (4) doc-first specs are
cheaply reversible. Merge-safety: implementers run in parallel only across different crates.

## How to reverse
Re-prioritize by editing the charter Step 3 order; this ADR is advisory for one session.
