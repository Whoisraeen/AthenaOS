# AthenaOS — Agent Context Document

**You are working on AthenaOS**, an embodied-AGI operating system in an independent repository (bootstrapped from RaeenOS source — not a GitHub fork). Read this file before writing code. Close the distance between the repository and `Athena_Concept.md`, one proven slice at a time.

---

## 0. Quick Reference

| Item | Value |
|---|---|
| Design bible | `Athena_Concept.md` — wins every conflict |
| Upstream thesis (parked gaming) | `RaeenOS_Concept.md` — lineage only |
| Architecture | `docs/ARCHITECTURE.md` |
| Cognitive stack | `docs/COGNITIVE_STACK.md` |
| Safety | `docs/SAFETY.md` |
| Roadmap | `docs/ROADMAP.md` |
| Boot proof | `docs/BOOT_STATUS.md` |
| Lineage ADR | `docs/decisions/0001-fork-from-raeenos.md` |
| Learned preferences | `AGENTS.md` |
| Build | `cargo run -p xtask -- build` |
| Boot (QEMU) | `cargo run -p xtask -- run` |
| Success markers | AthKernel / AthenaOS banner + `[ OS ] System successfully booted.` |

**Precedence:** `Athena_Concept.md` > Athena `docs/*` > this file > inherited RaeenOS checklists (`MasterChecklist.md`, etc.).

---

## 1. What AthenaOS Is

An OS for a **synthetic person**: continuous perception–action autonomy in a humanoid body, with human-like sentience as an *engineering* architecture (persistent self, memory, goals/affect, social presence) under hard AthGuard limits.

**Keep from RaeenOS:** hybrid Rust kernel, capability security, real-time scheduling class, Cargo workspace + xtask, aarch64 direction.

**Park for Athena v0:** RaePlay, consumer RaeStore, Steam/Proton day-one, anti-cheat partnerships, gaming compositor polish as the north star.

**Add:** AthBody, AthSense, AthMind, AthVoice, AthGuard (product names; crate renames deferred).

### Decision heuristics

1. Quote the Athena Concept line your work serves.
2. Hot path for balance/motors/sensors stays real-time and capability-gated.
3. “Would a desktop OS do it this way?” is a warning for embodiment work.
4. Prefer changes that make the autonomy loop measurable under AthGuard.
5. Never let goals modify safety policy silently.

---

## 2. Inherited RaeenOS machinery

Much of the tree (LinuxKPI, GPU bring-up, RaeBridge, MasterChecklist) is still RaeenOS-shaped. Use it when it strengthens AthKernel; do not expand gaming product surfaces unless required for boot. EliteMini bare-metal notes in old docs refer to the **dev-host**, not this product name.

For deep RaeenOS subsystem rules (syscall tables, ownership hooks, iron flash), see git history / `upstream-raeenos` docs — but Athena roadmap phases beat RaeenOS Year-1 gaming milestones.
