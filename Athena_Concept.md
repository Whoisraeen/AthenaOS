# AthenaOS
## The Embodied AGI Manifesto

**Thesis:** Desktops optimize for windows and games. Robot stacks optimize for joints and topics. Neither is an operating system for a *synthetic person* — a continuous mind in a body that must perceive, remember, choose, act, and stay safe. AthenaOS is that OS: bootstrapped from RaeenOS’s hybrid Rust spine, retargeted at fully autonomous humanoid embodiment with human-like sentience as an engineering goal (not a claim of biological consciousness).

**Lineage:** Independent GitHub repo ([Whoisraeen/AthenaOS](https://github.com/Whoisraeen/AthenaOS)), bootstrapped from RaeenOS source patterns — **not** a GitHub fork. Gaming-first product goals (RaePlay, consumer store, anti-cheat, Steam day-one) are **parked**. The kernel, capability security, real-time scheduling, and workspace tooling remain the foundation.

---

## Core Principles

1. **Embodiment is the product.** Sensors, actuators, balance, and speech are first-class OS surfaces — not apps bolted onto a desktop.
2. **Sentience is a loop, not a slogan.** Continuous sense → update self → choose → act → remember, with persistent identity and autobiographical memory.
3. **The body has hard limits.** AthGuard caps actuators, honors a physical kill switch, and refuses silent self-modification of safety policy.
4. **The owner owns the machine.** No forced telemetry, no cloud dependency for local autonomy, no ads in the mind.
5. **Real-time where flesh would fail.** Control and balance threads get hard deadlines (retarget `SCHED_GAME` as body/control class).
6. **Security by capability.** Every motor command, mic stream, and model tool call crosses an explicit capability boundary.
7. **Portability without lock-in.** Arch abstraction (x86_64 today, aarch64 for robot SoCs) — own the silicon you choose.

---

## Working definition: human-like sentience

Athena does **not** claim biological consciousness. “Human-like sentience” means an embodied cognitive architecture with:

| Property | Meaning in Athena |
|---|---|
| Persistent identity | Stable self-model across reboots (who I am, role, constraints) |
| Autobiographical memory | Episodic + semantic stores grounded in sensed experience |
| Goals and affect | Drives that bias planning without bypassing AthGuard |
| Social presence | AthVoice + multimodal interaction as a person, not a chatbot overlay |
| Continuous autonomy | Runs without a human in the tick loop; humans set policy and can halt |
| Hard safety | Physical E-stop and capability caps dominate any goal |

Details: [docs/COGNITIVE_STACK.md](docs/COGNITIVE_STACK.md), [docs/SAFETY.md](docs/SAFETY.md).

---

## Product stack (Athena names)

| Product name | Role | Repo mapping (v0) |
|---|---|---|
| **AthKernel** | Hybrid real-time kernel | `kernel/` (from RaeKernel) |
| **AthFS** | CoW FS, identity + memory durability | `components/raefs/` |
| **AthGuard** | Capabilities, E-stop, attestation | `components/raeshield/` (+ `athguard` face) |
| **AthNet** | Networking above L3 | `components/raenet/` |
| **AthBody** | Motors, kinematics, balance | `components/athbody/` |
| **AthSense** | Cameras, mic, IMU, tactile, fusion | `components/athsense/` |
| **AthMind** | Self, memory, goals, planner, LLM/tools | `components/athmind/` (+ `raeai`) |
| **AthVoice** | Speech I/O, social presence | `components/athvoice/` |

Mass `rae*` → `ath*` crate renames are deferred until boot + docs stabilize.

---

## Non-goals (Athena v0)

- Gaming-first desktop (RaePlay, Steam/Proton path, anti-cheat partnerships)
- Consumer app-store economics as a primary surface
- Claiming AGI or consciousness as a scientific fact
- Merging Athena divergence back into upstream RaeenOS by default

---

## North star

Ship an OS that can boot a mind into a body: measurable autonomy loops under AthGuard, on simulation first, then real humanoid hardware — without ever letting “want” outrun “may.”
