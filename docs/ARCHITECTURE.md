# AthenaOS Architecture

How [`Athena_Concept.md`](../Athena_Concept.md) maps to this repository.

Independent repo bootstrapped from [RaeenOS](https://github.com/Whoisraeen/RaeenOS) source (not a GitHub fork). Crates and product names use `Ath*` / `ath*`. See [ADR 0001](decisions/0001-fork-from-raeenos.md) and [PARKED_GAMING.md](PARKED_GAMING.md).

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

## Product → crate mapping

| Product | Crate path |
|---|---|
| AthKernel | `kernel/` |
| AthFS | `components/athfs/` |
| AthGuard | `components/athguard/` + `athshield/` |
| AthNet | `components/athnet/` |
| AthBody | `components/athbody/` |
| AthSense | `components/athsense/` |
| AthMind | `components/athmind/` + `athai/` |
| AthVoice | `components/athvoice/` |
| AthBridge (parked) | `components/athbridge/` |
| AthPlay (parked) | `components/athplay/` |

## Remotes

| Remote | Repo | Use |
|---|---|---|
| `origin` | [Whoisraeen/AthenaOS](https://github.com/Whoisraeen/AthenaOS) | Athena pushes only |
| `upstream-raeenos` | [Whoisraeen/RaeenOS](https://github.com/Whoisraeen/RaeenOS) | Optional reference only |
