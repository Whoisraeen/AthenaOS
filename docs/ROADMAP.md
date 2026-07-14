# AthenaOS Roadmap

Milestone philosophy: prove AthKernel, then close the sense→act loop in simulation, then real sensors, then full humanoid integration — always under AthGuard.

## M1 — Brand, boot, docs (done)

- [x] Independent AthenaOS repo (not a GitHub fork of RaeenOS)
- [x] Remotes: `origin` → AthenaOS; optional `upstream-raeenos` reference only
- [x] Athena manifesto + AthKernel banner
- [x] QEMU boot proof (`BOOT_STATUS.md`)
- [x] Architecture / cognitive / safety docs + ADR 0001
- [x] AthBody / AthSense / AthMind / AthVoice / AthGuard stubs
- [x] Product string rename to AthenaOS / Ath*; gaming thesis → `LEGACY_GAMING_CONCEPT.md`

## M2 — Simulated body

- Virtual AthBody (kinematics stub + command bus)
- Synthetic AthSense streams
- AthMind tick loop with AthGuard denies proven on serial

## M3 — SBC / sensors

- aarch64 bring-up for robot-class SoC
- Real camera / IMU / mic behind AthSense
- On-device or edge LLM policy, capability-gated

## M4 — Humanoid integration

- Real actuators via AthBody with E-stop on iron
- Balance / locomotion under `SCHED_BODY`
- Autonomy levels: halted → teleop → supervised → autonomous
- AthVoice social presence with consent flags

## Explicit non-goals

- Gaming desktop (AthPlay, Steam day-one, anti-cheat vendors, GameOS)
- Claiming AGI/consciousness as scientific fact
- Pushing Athena commits into RaeenOS
