## AthenaOS agent context

**Design bible:** `Athena_Concept.md` (wins every conflict). Upstream gaming thesis is historical only: `RaeenOS_Concept.md`.

**Mission:** Embodied AGI OS for a humanoid / synthetic person — AthKernel + AthBody / AthSense / AthMind / AthVoice under AthGuard. Independent repo bootstrapped from RaeenOS source (not a GitHub fork); do not treat gaming-first checklist items as Athena priorities unless they unlock the kernel spine.

**Remotes:** `origin` = [Whoisraeen/AthenaOS](https://github.com/Whoisraeen/AthenaOS) (independent repo, not a fork). Optional `upstream-raeenos` is read-only reference — never push Athena commits there.

## Three-agent parallel development (inherited from RaeenOS)

Ownership hooks and crate maps from the RaeenOS bootstrap still exist under `agents/OWNERSHIP.toml`. Prefer Athena-first work in:

| Area | Paths |
|---|---|
| Kernel / boot brand | `kernel/`, `xtask/` |
| Cognition / embodiment stubs | `components/athbody/`, `athsense/`, `athmind/`, `athvoice/`, `athguard/` |
| Docs | `Athena_Concept.md`, `docs/ARCHITECTURE.md`, `docs/COGNITIVE_STACK.md`, `docs/SAFETY.md`, `docs/ROADMAP.md`, `docs/BOOT_STATUS.md`, `docs/decisions/` |
| Parked (do not expand for Athena v0) | `raeplay`, consumer store surfaces, anti-cheat / Steam day-one paths |

If you keep using the three-agent hooks: install with `bash scripts/install-hooks.sh`, set `RAEEN_AGENT`, and stage only owned paths. Interface/`rae_abi` rules still apply when touching syscalls.

## Learned User Preferences

- Only create git commits or push when the user explicitly asks.
- Do not edit user-attached plan files when implementing a plan; update Athena docs instead.
- When working a multi-item plan/checklist, keep going until all to-dos are done.
- Rebuild the kernel before relying on serial markers after kernel source changes.
- Hardware nicknamed “Athena” in old RaeenOS logs is the **EliteMini / dev-host** — not this OS.

## Learned Workspace Facts

- Boot proof: `cargo run -p xtask -- run` (or release build + `target\boot.ps1`); expect AthenaOS/AthKernel banner and `[ OS ] System successfully booted.` with no `[PANIC]`.
- Serial often mirrors to GOP framebuffer after `console::init()`.
- Mass `rae*` → `ath*` crate renames are deferred; product names map in `Athena_Concept.md` and `docs/ARCHITECTURE.md`.
- Cherry-pick useful kernel fixes from `upstream-raeenos`; Athena diverges on cognition/embodiment.
