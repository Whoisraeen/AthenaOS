# AthenaOS Cognitive Stack

Working definition of **human-like sentience** for Athena: an embodied cognitive architecture, not a claim of biological consciousness.

## Tick loop

Every control cycle (and coarser deliberative cycles) runs:

```
sense → update self → choose → act → remember
```

| Stage | Owner | Notes |
|---|---|---|
| sense | AthSense | Cameras, mic, IMU, tactile → fused percepts on a perception bus |
| update self | AthMind | Bodily schema, identity, affect, situation model |
| choose | AthMind | Goals + planner + optional LLM/tool proposals |
| act | AthBody / AthVoice | Motor cmds and speech — **only through AthGuard** |
| remember | AthMind + AthFS | Working, episodic, and semantic memory persistence |

## Modules

### Perception bus (AthSense)

- Typed sensor frames (image, audio PCM, IMU sample, contact).
- Fusion produces egocentric state (pose estimate, attention saliency).
- Failures are explicit: missing sensors degrade autonomy modes, never invent certainty.

### Memory

| Store | Lifetime | Content |
|---|---|---|
| Working | seconds–minutes | Current goals, percept buffer, dialogue state |
| Episodic | durable | Timestamped experiences (“what happened”) |
| Semantic | durable | Facts, skills, world model (“what is true”) |
| Self-model | durable | Identity, role, bodily limits, owner policy |

Persistence lands on AthFS; encryption and snapshots inherit CoW FS properties.

### Self-model

- **Identity:** name, role, relationship to owner.
- **Body schema:** DOF map, safe joint limits, energy state.
- **Social role:** how AthVoice presents; consent boundaries from AthGuard.
- **Constraints:** non-negotiable policy pointers (AthGuard is authoritative).

### Goals / affect / planner

- Goals are ranked intents (owner-assigned, homeostatic, social).
- **Affect engine (Layer A):** durable channels (stress, trust, attachment, warmth, resolve, shame, …) bias priority and presence; they cannot raise actuator privileges. Spec: [`docs/superpowers/specs/2026-07-14-athena-affect-arc-design.md`](superpowers/specs/2026-07-14-athena-affect-arc-design.md). **P1 implemented:** `components/athmind/src/affect.rs`; boot proof `[affect] stress=…` via the deferred self-test sweep.
- **Arc / Become Human (Layer C):** software stability, bonds, value ledger, chapters, deviance-as-visible-struggle — under AthGuard. Spec: same document, §6 (not yet implemented).
- **Presence (Layer B):** AthVoice / AthBody express Affect + Arc consistently.
- Planner emits action sketches; AthGuard admits or rejects each sketch.

### LLM / tool runtime

- Inherit and retarget `components/athai/` as the language/tool substrate under AthMind.
- **On-device preferred** for continuous autonomy; cloud optional and capability-gated.
- Model output is proposal-only: no direct motor path.

## Autonomy levels

| Level | Behavior |
|---|---|
| Halted | E-stop or AthGuard lock; no actuation |
| Teleop | Human commands actions; mind observes |
| Supervised | Mind proposes; human confirms high-risk acts |
| Autonomous | Mind runs tick loop under standing AthGuard policy |

## Non-claims

Athena docs must not assert that the system is conscious, feels pain, or has moral patienthood. Sentience here is an **engineering target**: continuous, embodied, self-modeling autonomy with social presence.
