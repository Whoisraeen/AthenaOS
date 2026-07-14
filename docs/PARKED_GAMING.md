# Parked gaming surfaces (Athena)

These bootstrap leftovers must **not** drive Athena work. Prefer deletion or
AthGuard/AthBody retargeting over expanding them.

| Path / surface | Former purpose | Athena action |
|---|---|---|
| `components/athplay` | Game launcher | Parked |
| `docs/design/gameos-mode.md` | Couch gaming UI | Parked |
| `docs/ANTICHEAT_STRATEGY.md` | EAC/BattlEye | Parked (attestation may feed body safety later) |
| `kernel/src/anticheat.rs` | Game AC syscalls | Parked residue |
| `components/athbridge` Steam/Proton path | Windows games | Parked |
| Consumer `athstore` | App store | Parked |
| Game profiles / RGB “gamer” paths | Desktop gaming | Parked |

Authoritative direction: [`Athena_Concept.md`](../Athena_Concept.md),
[`LEGACY_GAMING_CONCEPT.md`](../LEGACY_GAMING_CONCEPT.md).
