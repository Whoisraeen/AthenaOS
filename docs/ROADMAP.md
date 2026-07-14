# AthenaOS Roadmap

Milestone philosophy: prove AthKernel, then close the sense→act loop in simulation, then real sensors, then full humanoid integration — always under AthGuard.

## M1 — Brand, boot, docs (current)

- [x] Bootstrap independent AthenaOS repo from RaeenOS source (not a GitHub fork)
- [x] Remotes: `origin` → [AthenaOS](https://github.com/Whoisraeen/AthenaOS); optional `upstream-raeenos` reference only
- [x] Athena manifesto + thin AthKernel banner rebrand
- [x] QEMU boot proof documented in `BOOT_STATUS.md`
- [x] Architecture / cognitive / safety docs + ADR 0001
- [x] AthBody / AthSense / AthMind / AthVoice / AthGuard stubs
- [x] Product identity rename (banner, DMI, system name, mDNS, installer labels, xtask); inherited `rae*` crate paths kept temporarily
- [x] Replace inherited git history with AthenaOS root commit (independent repo)

## M2 — Simulated body

- Virtual AthBody (kinematics stub + command bus)
- Synthetic AthSense streams (scripted or recorded)
- AthMind tick loop in userspace/QEMU with AthGuard denies proven on serial
- No real robot hardware required

## M3 — SBC / sensors

- aarch64 bring-up path for a robot-class SoC (or companion board)
- Real camera / IMU / mic drivers behind AthSense
- On-device or edge LLM policy documented and capability-gated

## M4 — Humanoid integration

- Real actuators via AthBody with E-stop on iron
- Balance / locomotion under RT scheduling class
- Continuous autonomy levels (halted → teleop → supervised → autonomous)
- Social presence via AthVoice with consent flags

## Explicit non-goals until later

- Gaming-first desktop (RaePlay, Steam day-one, anti-cheat)
- Mass claim of AGI/consciousness
- Merging Athena divergence back into RaeenOS by default
