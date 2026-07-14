# ADR 0001 — Independent AthenaOS repository

## Status

Accepted (2026-07-13); amended same day: **not a GitHub fork**.

## Context

AthenaOS needs its own product identity and GitHub presence. RaeenOS is a separate gaming-first OS. Sharing git history or a GitHub fork relationship would couple the projects and confuse ownership.

## Decision

1. **[Whoisraeen/AthenaOS](https://github.com/Whoisraeen/AthenaOS)** is a **new, independent** public repository — not a GitHub fork of RaeenOS.
2. The initial tree was **bootstrapped from RaeenOS source** (hybrid Rust kernel, xtask, capability stack) then retargeted at embodied AGI. Git history on AthenaOS starts at Athena’s own root commit (no RaeenOS commit graph).
3. **Never push Athena commits to RaeenOS.** Optional local remote `upstream-raeenos` may exist only to read/port patches by hand — it is not a parent fork.
4. Park gaming-first goals for Athena v0; product names are `Ath*`.

## Consequences

- Clear separate repos and release lines.
- No GitHub “forked from” banner or shared network graph with RaeenOS.
- Porting fixes from RaeenOS is manual cherry-pick/adapt, not `git pull` from a fork parent.
- `Athena_Concept.md` is the design bible.
