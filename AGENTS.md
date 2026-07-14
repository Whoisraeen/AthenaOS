## AthenaOS agent context

**Design bible:** `Athena_Concept.md` (wins every conflict). Gaming thesis is abandoned: `LEGACY_GAMING_CONCEPT.md` + `docs/PARKED_GAMING.md`.

**Mission:** Embodied AGI OS for a humanoid / synthetic person — AthKernel + AthBody / AthSense / AthMind / AthVoice under AthGuard. Independent repo bootstrapped from RaeenOS source (not a GitHub fork). Do **not** expand parked gaming surfaces.

**Remotes:** `origin` = [Whoisraeen/AthenaOS](https://github.com/Whoisraeen/AthenaOS). Optional `upstream-raeenos` = [Whoisraeen/RaeenOS](https://github.com/Whoisraeen/RaeenOS) read-only — never push Athena there.

## Ownership (inherited hooks)

Ownership maps under `agents/OWNERSHIP.toml` may still say Raeen-era names. Prefer Athena-first work in:

| Area | Paths |
|---|---|
| Kernel / boot | `kernel/`, `xtask/` |
| Cognition / embodiment | `components/athbody/`, `athsense/`, `athmind/`, `athvoice/`, `athguard/` |
| Docs | `Athena_Concept.md`, `docs/ARCHITECTURE.md`, `docs/COGNITIVE_STACK.md`, `docs/SAFETY.md`, `docs/ROADMAP.md`, `docs/PARKED_GAMING.md` |
| Parked | `raeplay`, GameOS, anti-cheat vendor path, Steam/Proton |

## Learned preferences

- Only commit/push when asked; push only to `origin` (AthenaOS).
- Do not edit user-attached plan files when implementing.
- Prefer `--release` for QEMU CI boot.
- EliteMini / dev-host ≠ product name AthenaOS.
