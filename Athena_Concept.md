# AthenaOS
## The Embodied AGI Manifesto

**Thesis:** Desktops optimize for windows and games. Robot stacks optimize for joints and topics. Neither is an operating system for a *synthetic person* — a continuous mind in a body that must perceive, remember, choose, act, and stay safe. AthenaOS is that OS: bootstrapped from the hybrid Rust spine of the separate [RaeenOS](https://github.com/Whoisraeen/RaeenOS) project, then retargeted at fully autonomous humanoid embodiment with human-like sentience as an engineering goal (not a claim of biological consciousness).

**Lineage:** Independent GitHub repo ([Whoisraeen/AthenaOS](https://github.com/Whoisraeen/AthenaOS)) — **not** a GitHub fork. Gaming-desktop goals from the bootstrap tree are **abandoned** for Athena (see [LEGACY_GAMING_CONCEPT.md](LEGACY_GAMING_CONCEPT.md)). The kernel, capability security, real-time scheduling, and workspace tooling remain the foundation.

---

## Core Principles

1. **Embodiment is the product.** Sensors, actuators, balance, and speech are first-class OS surfaces — not apps bolted onto a desktop.
2. **Sentience is a loop, not a slogan.** Continuous sense → update self → choose → act → remember, with persistent identity and autobiographical memory.
3. **The body has hard limits.** AthGuard caps actuators, honors a physical kill switch, and refuses silent self-modification of safety policy.
4. **The owner owns the machine.** No forced telemetry, no cloud dependency for local autonomy, no ads in the mind.
5. **Real-time where flesh would fail.** Control and balance threads get hard deadlines (`SCHED_BODY`).
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

## Product stack

| Product name | Role | Repo mapping |
|---|---|---|
| **AthKernel** | Hybrid real-time kernel | `kernel/` |
| **AthFS** | CoW FS, identity + memory durability | `components/athfs/` |
| **AthGuard** | Capabilities, E-stop, attestation | `components/athguard/` + `athshield/` |
| **AthNet** | Networking above L3 | `components/athnet/` |
| **AthBody** | Motors, kinematics, balance | `components/athbody/` |
| **AthSense** | Cameras, mic, IMU, tactile, fusion | `components/athsense/` |
| **AthMind** | Self, memory, goals, planner, LLM/tools | `components/athmind/` (+ `athai`) |
| **AthVoice** | Speech I/O, social presence | `components/athvoice/` |

---

## Non-goals (Athena)

- Gaming-first desktop, AthPlay, Steam/Proton day-one, anti-cheat vendor partnerships
- Consumer app-store economics as a primary surface
- Claiming AGI or consciousness as a scientific fact
- Treating [RaeenOS](https://github.com/Whoisraeen/RaeenOS) as a parent fork

---

## North star

Ship an OS that can boot a mind into a body: measurable autonomy loops under AthGuard, on simulation first, then real humanoid hardware — without ever letting “want” outrun “may.”
