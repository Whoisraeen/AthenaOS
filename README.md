# AthenaOS

> An operating system for a synthetic person — embodied AGI on a humanoid platform.

**Independent repository:** [Whoisraeen/AthenaOS](https://github.com/Whoisraeen/AthenaOS) — not a GitHub fork. Bootstrapped from the separate [RaeenOS](https://github.com/Whoisraeen/RaeenOS) codebase (hybrid Rust kernel spine), then retargeted at continuous perception–action autonomy.

See [Athena_Concept.md](Athena_Concept.md) for the manifesto.  
See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for layering.  
See [docs/COGNITIVE_STACK.md](docs/COGNITIVE_STACK.md) for the mind loop.  
See [docs/SAFETY.md](docs/SAFETY.md) for AthGuard.  
See [docs/ROADMAP.md](docs/ROADMAP.md) for what ships when.  
See [docs/BOOT_STATUS.md](docs/BOOT_STATUS.md) for QEMU boot proof.  
Abandoned gaming thesis: [LEGACY_GAMING_CONCEPT.md](LEGACY_GAMING_CONCEPT.md).

## Status

Pre-alpha: **independent AthenaOS repo + AthKernel QEMU boot + cognition/safety docs + Ath\* stubs.**

Gaming crates (`raeplay`, consumer store, Steam/Proton bridge path, etc.) remain in-tree so the workspace builds; they are **parked / non-goals**.

## Repo layout

```
Athena/
├── kernel/             AthKernel — bare-metal Rust, boots in QEMU
├── components/
│   ├── athbody/        AthBody — motors, kinematics, balance, E-stop surface
│   ├── athsense/       AthSense — sensors + fusion API
│   ├── athmind/        AthMind — cognitive loop + memory interfaces
│   ├── athvoice/       AthVoice — speech I/O / presence
│   ├── athguard/       AthGuard — capability + safety policy face
│   ├── raefs/          AthFS mapping (path rename deferred)
│   ├── raeshield/      AthGuard backend (path rename deferred)
│   ├── raeai/          LLM/tool runtime (feeds AthMind)
│   └── …               other inherited crates (many parked)
├── xtask/              Build/run → disk image + QEMU
└── docs/
```

## Quickstart

```powershell
cargo run -p xtask --release -- run --release --ci
```

Expect **AthenaOS / AthKernel** on serial and `[ OS ] System successfully booted.`

## Remotes

| Remote | Purpose |
|---|---|
| `origin` | [Whoisraeen/AthenaOS](https://github.com/Whoisraeen/AthenaOS) — Athena pushes only |
| `upstream-raeenos` | Optional read-only reference to [RaeenOS](https://github.com/Whoisraeen/RaeenOS) — **not a fork parent; never push Athena here** |

## Hardware naming

The Beelink EliteMini bring-up box is **EliteMini / dev-host** in Athena docs (not “Athena”).
