# AthenaOS Architecture

How [`Athena_Concept.md`](../Athena_Concept.md) maps to this repository.

Independent repo bootstrapped from RaeenOS source (not a GitHub fork). Product names are Athena (`Ath*`). Many inherited crate paths still use `rae*` until the rename pass completes; see the mapping table and [ADR 0001](decisions/0001-fork-from-raeenos.md).

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

## Parked (Athena v0 — status: parked)

Do **not** expand these as Athena product goals. Crates may remain so the workspace builds.

| Inherited surface | Why parked |
|---|---|
| RaePlay (`raeplay`) | Gaming launcher — not embodiment |
| Consumer RaeStore (`raestore`) | App-store economics — non-goal |
| RaeBridge Steam/Proton path | Windows gaming compat — non-goal |
| Anti-cheat partnerships | Gaming threat model — non-goal |
| Desktop shell polish as north star | Replaced by AthMind/AthBody loops |

## Build system

- Cargo workspace at repo root; shared `Cargo.lock`.
- `kernel/` targets `x86_64-unknown-none`; build via `xtask`.
- `xtask` builds the kernel, packs a disk image, launches QEMU.
- `rust-toolchain.toml` pins nightly + `x86_64-unknown-none` (and aarch64 softfloat for future robot SoCs).

## Boot path (today)

```
QEMU BIOS / UEFI
  → bootloader stub
  → AthKernel `kernel_main`
  → serial banner (AthenaOS / AthKernel)
  → init stack → `[ OS ] System successfully booted.`
```

See [BOOT_STATUS.md](BOOT_STATUS.md) for the latest boot proof.

## Remotes

| Remote | Repo | Use |
|---|---|---|
| `origin` | [Whoisraeen/AthenaOS](https://github.com/Whoisraeen/AthenaOS) | Athena pushes only |
| `upstream-raeenos` | [Whoisraeen/RaeenOS](https://github.com/Whoisraeen/RaeenOS) | Optional reference for porting patches; **not a fork parent — never push Athena here** |
