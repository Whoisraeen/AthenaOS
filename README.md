# AthenaOS

> An operating system for a synthetic person — embodied AGI on a humanoid platform.

**Independent repository:** [Whoisraeen/AthenaOS](https://github.com/Whoisraeen/AthenaOS) — not a GitHub fork. The codebase was bootstrapped from [RaeenOS](https://github.com/Whoisraeen/RaeenOS) patterns (hybrid Rust kernel spine), then retargeted at continuous perception–action autonomy with human-like sentience (engineering definition — see the concept doc).

See [Athena_Concept.md](Athena_Concept.md) for the manifesto.  
See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for layering.  
See [docs/COGNITIVE_STACK.md](docs/COGNITIVE_STACK.md) for the mind loop.  
See [docs/SAFETY.md](docs/SAFETY.md) for AthGuard.  
See [docs/ROADMAP.md](docs/ROADMAP.md) for what ships when.  
See [docs/BOOT_STATUS.md](docs/BOOT_STATUS.md) for QEMU boot proof.  
Historical gaming thesis kept for lineage notes: [RaeenOS_Concept.md](RaeenOS_Concept.md).

## Status

Pre-alpha Milestone 1: **independent AthenaOS repo + AthKernel QEMU boot + cognition/safety docs + Ath\* stubs.**

Gaming-first crates (`raeplay`, consumer `raestore`, `raebridge` Steam path, etc.) remain in-tree so the workspace builds; they are **parked** for Athena v0.

## Repo layout

```
Athena/
├── kernel/             AthKernel — bare-metal Rust, boots in QEMU
├── components/
│   ├── athbody/        AthBody — motors, kinematics, balance, E-stop surface
│   ├── athsense/       AthSense — sensors + fusion API
│   ├── athmind/        AthMind — cognitive loop + memory interfaces
│   ├── athvoice/       AthVoice — speech I/O / presence
│   ├── athguard/       AthGuard — Athena face of capability + safety policy
│   ├── raefs/          AthFS mapping (crate name deferred)
│   ├── raeshield/      AthGuard backend (crate name deferred)
│   ├── raeai/          LLM/tool runtime (feeds AthMind)
│   └── …               other inherited crates (many parked for desktop/gaming)
├── xtask/              Build/run automation → disk image + QEMU
└── docs/
    ├── ARCHITECTURE.md
    ├── COGNITIVE_STACK.md
    ├── SAFETY.md
    ├── ROADMAP.md
    ├── BOOT_STATUS.md
    └── decisions/      ADRs (0001 = independent repo)
```

## Quickstart

Prereqs: Rust 1.94+ (stable), QEMU. On this dev machine QEMU often lives at
`C:\Program Files\qemu\qemu-system-x86_64.exe`; xtask finds it automatically.

```powershell
# from the repo root
cargo run -p xtask -- run
```

Expect an **AthenaOS / AthKernel** banner and eventually `[ OS ] System successfully booted.`

```powershell
cargo run -p xtask -- build
cargo build -p kernel --target x86_64-unknown-none
```

## Remotes

| Remote | Purpose |
|---|---|
| `origin` | **[Whoisraeen/AthenaOS](https://github.com/Whoisraeen/AthenaOS)** — Athena pushes only |
| `upstream-raeenos` | Optional read-only reference to RaeenOS for porting patches — **not a fork parent; never push Athena here** |

## Hardware naming note

Older RaeenOS docs call the Beelink EliteMini bring-up box “Athena.” In **AthenaOS** docs, that machine is **EliteMini** / **dev-host** to avoid colliding with this OS’s name.

## License / lineage

Same license terms as the RaeenOS tree this codebase was bootstrapped from. AthenaOS is a separate product and a separate GitHub repository.
