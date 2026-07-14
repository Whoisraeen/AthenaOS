# ADR 0001 — Independent AthenaOS repository

## Status

Accepted (2026-07-13); amended: not a GitHub fork; gaming thesis abandoned.

## Context

AthenaOS needs its own product identity. RaeenOS remains a separate gaming-oriented OS. Sharing a GitHub fork relationship or treating gaming as Athena’s north star would confuse ownership and roadmap.

## Decision

1. **[Whoisraeen/AthenaOS](https://github.com/Whoisraeen/AthenaOS)** is a **new, independent** public repository — not a GitHub fork of [RaeenOS](https://github.com/Whoisraeen/RaeenOS).
2. The tree was **bootstrapped from RaeenOS source**, then retargeted at embodied AGI. Git history starts at Athena’s own root commit.
3. **Never push Athena commits to RaeenOS.** Optional `upstream-raeenos` is read-only reference only.
4. **Abandon** gaming-first goals for Athena (AthPlay, Steam day-one, anti-cheat partnerships, GameOS). See [LEGACY_GAMING_CONCEPT.md](../../LEGACY_GAMING_CONCEPT.md).
5. Product names are `Ath*`; inherited `rae*` paths rename incrementally.

## Consequences

- Separate repos and release lines.
- No GitHub “forked from” relationship.
- Gaming docs/code may linger as parked residue until removed.
- `Athena_Concept.md` is the only design bible.
