# ADR 0001 — Independent AthenaOS repository

## Status

Accepted. Not a GitHub fork. Full `rae*` → `ath*` crate rename applied.

## Decision

1. **[Whoisraeen/AthenaOS](https://github.com/Whoisraeen/AthenaOS)** is an independent public repository.
2. Bootstrapped from [Whoisraeen/RaeenOS](https://github.com/Whoisraeen/RaeenOS) source, then renamed end-to-end (`Ath*` / `ath*`, `SCHED_BODY`, `/proc/athena`, `SYS_ATHENA_*`).
3. Never push Athena commits to RaeenOS. Optional remote name: `upstream-raeenos`.
4. Gaming thesis abandoned — see `LEGACY_GAMING_CONCEPT.md` and `docs/PARKED_GAMING.md`.
