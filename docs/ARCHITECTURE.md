# AthenaOS Architecture

How [`Athena_Concept.md`](../Athena_Concept.md) maps to this repository.

Independent repo bootstrapped from [RaeenOS](https://github.com/Whoisraeen/RaeenOS) source (not a GitHub fork). Product names are Athena (`Ath*`). Many inherited crate paths still use `rae*` until path renames complete; see [ADR 0001](decisions/0001-fork-from-raeenos.md) and [PARKED_GAMING.md](PARKED_GAMING.md).

## Layering

```
┌──────────────────────────────────────────────────────────────┐
│  Presence / apps (AthVoice, tools; desktop apps parked)      │
├──────────────────────────────────────────────────────────────┤
│  AthMind — self, memory, goals, planner, LLM/tool runtime    │
├──────────────────────────────────────────────────────────────┤
│  AthSense — perception bus     AthBody — motors / kinematics │
├──────────────────────────────────────────────────────────────┤
│  AthGuard — capabilities, E-stop, attestation, consent       │
├──────────────────────────────────────────────────────────────┤
│  AthFS / AthNet / drivers (IOMMU-sandboxed)                  │
├──────────────────────────────────────────────────────────────┤
│  AthKernel — hybrid RT: scheduler, MM, IPC, control fast path│
└──────────────────────────────────────────────────────────────┘
```

## Product → crate mapping (v0)

| Product name | Role | Crate path | Stage |
|---|---|---|---|
| AthKernel | Hybrid real-time kernel | `kernel/` | bootable |
| AthFS | CoW FS, durable memory/identity | `components/raefs/` | inherited |
| AthGuard | Caps, E-stop, attestation | `components/athguard/` + `raeshield/` | stub + inherited |
| AthNet | Networking above L3 | `components/raenet/` | inherited |
| AthBody | Motors, kinematics, balance | `components/athbody/` | stub |
| AthSense | Sensors + fusion | `components/athsense/` | stub |
| AthMind | Cognitive loop | `components/athmind/` + `raeai/` | stub |
| AthVoice | Speech / presence | `components/athvoice/` | stub |

## Parked (non-goals)

See [PARKED_GAMING.md](PARKED_GAMING.md). Do **not** expand AthPlay, Steam/Proton, anti-cheat partnerships, or GameOS as Athena product work.

## Build system

- Cargo workspace at repo root.
- `kernel/` targets `x86_64-unknown-none`; build via `xtask`.
- Prefer `--release` for CI boot (debug `raenet` can hit an LLVM bug).

## Boot path (today)

```
QEMU BIOS / UEFI
  → bootloader stub
  → AthKernel `kernel_main`
  → serial banner (AthenaOS / AthKernel)
  → `[ OS ] System successfully booted.`
```

See [BOOT_STATUS.md](BOOT_STATUS.md).

## Remotes

| Remote | Repo | Use |
|---|---|---|
| `origin` | [Whoisraeen/AthenaOS](https://github.com/Whoisraeen/AthenaOS) | Athena pushes only |
| `upstream-raeenos` | [Whoisraeen/RaeenOS](https://github.com/Whoisraeen/RaeenOS) | Optional reference; never push Athena here |
