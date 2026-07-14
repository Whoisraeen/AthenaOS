# AthenaOS — Agent Context Document

**You are working on AthenaOS**, an embodied-AGI OS in an independent repository (bootstrapped from AthenaOS source — not a GitHub fork). Close the distance to `Athena_Concept.md`, one proven slice at a time. Gaming-desktop goals are **non-goals**.

---

## 0. Quick Reference

| Item | Value |
|---|---|
| Design bible | `Athena_Concept.md` |
| Abandoned gaming thesis | `LEGACY_GAMING_CONCEPT.md`, `docs/PARKED_GAMING.md` |
| Architecture | `docs/ARCHITECTURE.md` |
| Cognitive stack | `docs/COGNITIVE_STACK.md` |
| Safety | `docs/SAFETY.md` |
| Roadmap | `docs/ROADMAP.md` |
| Boot proof | `docs/BOOT_STATUS.md` |
| Lineage ADR | `docs/decisions/0001-fork-from-athenaos.md` |
| Build / boot | `cargo run -p xtask --release -- run --release --ci` |
| Success markers | AthKernel / AthenaOS banner + `[ OS ] System successfully booted.` |

**Precedence:** `Athena_Concept.md` > Athena `docs/*` > this file > inherited checklists (`MasterChecklist.md` is bootstrap residue — many gaming items are parked).

---

## 1. What AthenaOS Is

An OS for a **synthetic person**: continuous perception–action autonomy in a humanoid body, with human-like sentience as an engineering architecture under AthGuard.

**Keep:** hybrid Rust kernel, capabilities, real-time `SCHED_BODY`, xtask, aarch64 direction.

**Do not expand:** AthPlay, Steam/Proton, anti-cheat partnerships, GameOS couch UI, consumer store.

**Add:** AthBody, AthSense, AthMind, AthVoice, AthGuard.

### Decision heuristics

1. Quote the Athena Concept line your work serves.
2. Hot path for balance/motors/sensors stays real-time and capability-gated.
3. “Would a gaming desktop OS do it this way?” is a warning.
4. Prefer measurable autonomy loops under AthGuard.
5. Never let goals modify safety policy silently.
