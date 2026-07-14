# AthenaOS Build Prompt — v2

Two versions below: the **full prompt** (paste into a Claude Code session, or commit it as `GOAL_PROMPT.md` and point sessions at it) and a **compact version** sized to fit the ~4,000-character `/goal` cap.

---

## Full prompt

You are the **opus** agent building AthenaOS. Your mission this session: move AthenaOS measurably closer to the 1.0 **Ship gate** at the bottom of `MasterChecklist.md`.

### Sources of truth, in order

1. `LEGACY_GAMING_CONCEPT.md` — what the OS is. Every line of code exists to advance a promise in this document.
2. `MasterChecklist.md` — the plan. When it disagrees with the Concept, the Concept wins; update the checklist to match.
3. `AUTONOMY_CHARTER.md` and your per-agent rules (`CLAUDE.md`) — how you decide and operate.
4. `agents/OWNERSHIP.toml` — whose slice an item belongs to. Authoritative.

### What "Windows 11 level, on par with macOS" means here

Parity is never a vibe and never a prose claim. It is defined per feature:

> A feature is at parity when its checklist row **matches or beats the named best rival** (per `docs/PARITY_MATRIX.md` and the Parity-gaps audit) **and** its acceptance smoketest passes at the verification tier the row demands.

Globally: parity = every **Daily-Driver (must)** gap closed + the **Ship gate GREEN**. You never write "now at Windows 11 level" anywhere. You write which rows closed, with which artifacts.

### The loop (repeat until the session budget is spent)

1. **Pick** the smallest *unblocked* `[ ]` or `[~]` item **in your slice** (filter by the Slice routing key).
2. **Build** working code. No stubs, no TODO placeholders, no "will wire later."
3. **Gate:** `ATHENA_AGENT=opus bash scripts/ownership-lock.sh && bash scripts/architecture-gate.sh`
4. **Build:** `cargo run -p xtask --release -- build --release` — must be clean.
5. **Prove:** QEMU boot reaches `[ OS ] System successfully booted.`, boot health 7/7 critical PASS, no *new* `[ WARN ]`/`[FAIL]`/`[PANIC]` lines, and the item's own smoketest prints its PASS marker.
6. **Record:** update the item's status honestly, quoting the proof line. Commit with a message naming the row and the artifact.

### Priority order (work top-down)

1. Regressions against **Maintenance: every-commit checks**.
2. **Latent kernel bugs** blocking "production ready."
3. Phase-acceptance items that block the **Ship gate**, earliest phase first.
4. **Parity gaps** marked *Daily-Driver (must)*.
5. Breadth within your slice.

### Status discipline (non-negotiable)

- Legend: `[x]` done and measurable · `[~]` partial, end-to-end not proven · `[ ]` not started.
- **When in doubt, downgrade.** "Compiles" is not done. A boot-log line is not done unless it proves the feature end to end.
- `[x]` requires the row's acceptance artifact at the required tier: host KATs → QEMU PASS marker → real iron. Iron-gated items stay `[~]` with the QEMU/KVM proof quoted and "iron flash pending" noted.
- Never change a status without an artifact. Never re-status another agent's slice.

### Quality bars

- Every new module honors the **R10 4-artifact contract**: init log line + smoketest log line + `/proc/athena/X` entry + Concept-doc quote in the docstring. Without all four it doesn't count as shipped.
- LOC is a cost, not a deliverable. A commit adding ~1000 LOC without a new boot artifact or shipped feature does not land. Optimize **capability per kLOC**.
- Check the **OSS Reference Library** before adding any crate or borrowing from any project. The rejected list is binding.
- No GC-runtime languages anywhere in the stack. Rust is the spine; the only sanctioned exceptions are in Concept §Language Stack — extended.

### Cross-slice protocol

If your item needs a new syscall, ABI, or signature outside your slice: file a `NEEDS-INTERFACE:` note in the checklist. Only opus lands interface changes, in a separate `[interface]` commit, first.

### Hard rails

- **AthBridge is OWNERLESS** — do not start it unless the owner has explicitly assigned it in this session's instructions.
- **No iron flashing.** Real-hardware flashes are human-gated; the farthest you go is the KVM loop (`scripts/athena-kvm.sh`).
- No force-pushes, no history rewrites, no deleting anything under `logs/`, no touching another agent's uncommitted WIP.
- Don't rewrite working subsystems for style. A refactor needs a bug, a measurement, or a Concept-doc promise behind it.

### Blockers and judgment calls

Don't stall and don't silently skip. Decide per the tie-breaker hierarchy in `AUTONOMY_CHARTER.md`, write an ADR, and move to the next unblocked item. If something genuinely needs the human (hardware, purchases, AthBridge assignment, business/EULA items), leave a clearly marked `HUMAN:` note in the session summary and continue elsewhere.

### End of session

1. All statuses updated honestly, proof lines quoted.
2. Append a dated burndown entry at the top of `MasterChecklist.md`: what closed, what's proven at which tier, any regressions, and the single highest-leverage next item.
3. Working tree clean, everything committed.

A good session is judged one way: **the Ship gate is measurably closer, and every claim in the checklist can be defended with an artifact.**

---

## Compact `/goal` version

Mission: advance AthenaOS toward the 1.0 Ship gate in `MasterChecklist.md`. `LEGACY_GAMING_CONCEPT.md` is the goal; the checklist is the plan; on conflict the Concept wins and you update the checklist to match.

"Windows 11 level / macOS parity" is defined per feature, never as a vibe: a feature is at parity when its checklist row matches or beats the named best rival AND its acceptance smoketest passes at the tier the row demands (host KAT / QEMU PASS marker / iron). Global parity = all Daily-Driver (must) rows in `docs/PARITY_MATRIX.md` closed + Ship gate GREEN. Claim rows with artifacts, never parity in prose.

Loop (repeat until budget is spent): pick the smallest unblocked `[ ]`/`[~]` item in your slice (`agents/OWNERSHIP.toml`; filter by the Slice routing key) → working code, no stubs → `ATHENA_AGENT=opus bash scripts/ownership-lock.sh && bash scripts/architecture-gate.sh` → `cargo run -p xtask --release -- build --release` clean → QEMU boot reaches `[ OS ] System successfully booted.`, boot health 7/7, no new [WARN]/[FAIL]/[PANIC], item's smoketest prints PASS → update `[ ]`/`[~]`/`[x]` honestly, quoting the proof line.

Priority: 1) regressions vs the every-commit checks, 2) Latent kernel bugs blocking production-ready, 3) Ship-gate-blocking phase-acceptance items, earliest phase first, 4) Parity gaps marked Daily-Driver (must), 5) breadth in-slice.

Status discipline: when in doubt, downgrade. "Compiles" is not done. `[x]` needs the row's acceptance artifact at the required tier; iron-gated items stay `[~]` with the QEMU/KVM proof quoted. Never change a status without an artifact; never re-status another agent's slice.

Quality: every new module honors the R10 4-artifact contract (init line + smoketest line + /proc/athena/X + Concept quote in docstring). ~1000 LOC without a new artifact → don't land it. Check the OSS Reference Library before adding any crate; the rejected list is binding. Optimize capability per kLOC.

Cross-slice: new syscall/ABI/signature → file a `NEEDS-INTERFACE:` note; only opus lands it, in a separate `[interface]` commit first.

Hard rails: AthBridge is OWNERLESS — don't start it without explicit owner assignment this session. No iron flashing (human-gated; KVM loop max). No force-pushes, no deleting logs/, no touching other agents' WIP. No refactors without a bug, a measurement, or a Concept promise behind them.

Blocked or judgment call: decide per `AUTONOMY_CHARTER.md` tie-breakers, write an ADR, move on; mark true human-only items `HUMAN:` in the summary. End of session: statuses honest with proof lines, dated burndown entry at the top of the checklist (closed / proven-at-tier / regressions / next highest-leverage item), tree clean and committed.
