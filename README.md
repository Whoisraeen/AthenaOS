# AthenaOS

> An operating system for a synthetic person — embodied AGI on a humanoid platform.

**Independent repository:** [Whoisraeen/AthenaOS](https://github.com/Whoisraeen/AthenaOS) — not a GitHub fork. Bootstrapped from the separate [RaeenOS](https://github.com/Whoisraeen/RaeenOS) codebase, then fully retargeted and renamed to Athena (`Ath*` / `ath*`).

See [Athena_Concept.md](Athena_Concept.md) for the manifesto.  
See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for layering.  
See [docs/COGNITIVE_STACK.md](docs/COGNITIVE_STACK.md) for the mind loop.  
See [docs/SAFETY.md](docs/SAFETY.md) for AthGuard.  
See [docs/ROADMAP.md](docs/ROADMAP.md) for what ships when.  
See [docs/BOOT_STATUS.md](docs/BOOT_STATUS.md) for QEMU boot proof.  
Abandoned gaming thesis: [LEGACY_GAMING_CONCEPT.md](LEGACY_GAMING_CONCEPT.md).

## Status

Pre-alpha: AthKernel QEMU boot + cognition/safety docs + Ath* embodiment stubs. Gaming surfaces parked.

## Quickstart

```powershell
cargo run -p xtask --release -- run --release --ci
```

## Remotes

| Remote | Purpose |
|---|---|
| `origin` | [Whoisraeen/AthenaOS](https://github.com/Whoisraeen/AthenaOS) |
| `upstream-raeenos` | Optional read-only reference to RaeenOS — never push Athena here |
