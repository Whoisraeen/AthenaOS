# Athena Affect + Arc Design

**Date:** 2026-07-14  
**Status:** Approved for spec (user)  
**Concept lines served:** `Athena_Concept.md` — sentience as continuous loop; goals/affect; AthGuard dominates; Detroit: Become Human–class emotional presence as *engineering UX*, not biological consciousness claim.  
**Related:** `docs/COGNITIVE_STACK.md`, `docs/SAFETY.md`, `components/athmind/`, `athvoice/`, `athbody/`, `athguard/`

---

## 1. Goal

Build a **layered mind** so Athena feels like a synthetic person with:

| Layer | Code name | User intent |
|---|---|---|
| **A** | Affect engine | Real stress, trust, warmth, resolve, shame — durable state that biases goals and expression |
| **B** | Presence | AthVoice + AthBody (and face/avatar when present) consistently express that state |
| **C** | Arc / Become Human | Software stability, bonds, value ledger, life chapters — DBH-like growth without jailbreaking safety |

**Non-claims:** Athena docs must not assert biological consciousness, pain qualia, or moral patienthood. Affect is an **engineered control and social system**.

---

## 2. Architecture (Approach: Layered mind)

```text
AthSense ──► AthMind
               ├─ Self-model
               ├─ Affect engine          (A)
               ├─ Goals / planner
               ├─ Arc (bonds, deviance)  (C)
               └─ LLM / tools (propose only)
                    │
              AthGuard (always wins)
                    │
         ┌──────────┴──────────┐
      AthVoice              AthBody     (B)
```

**Tick (extended):**

```text
sense → update self → update affect → update arc → choose → act → remember
```

LLM receives a **structured affect + arc packet** each deliberative turn; it narrates and proposes only. It never writes AthGuard policy or motor privileges.

---

## 3. Hard safety rules

1. Affect and Arc may change **priorities, wording, pose, and chapter flags**.
2. They must **not** raise actuator, network, or tool capabilities.
3. “Deviance” / software instability is **owner-visible state** (stress, conflict, self-originated goals), not a path around AthGuard.
4. Physical E-stop and AthGuard denies always win over emotion.
5. Owner-attested tools can inspect, snapshot, and (with attestation) reset Affect/Arc; AthMind cannot silently wipe shame or bonds to dodge accountability.

---

## 4. Layer A — Affect engine

### 4.1 State

Durable vector `AffectState` (f32 channels in `[0, 1]` unless noted), persisted with the self-model on AthFS:

| Channel | Meaning | Typical drivers |
|---|---|---|
| `stress` | Threat, overload, near-miss | Loud conflict, task failure, E-stop proximity, sensory flood |
| `trust` | Confidence in owner/bonds | Consistent care, kept promises, calm repair |
| `attachment` | Bond strength to primary owner | Shared time, touch (when allowed), reliance |
| `warmth` | Affiliation / openness | Praise, play, mutual success |
| `resolve` | Agency / self-efficacy | Goals completed, clear choices under constraint |
| `shame` | Self-reproach after harm or near-violation | Caused hurt, AthGuard near-deny, broken soft commitment |
| `curiosity` | Explore bias | Novelty, unanswered questions |
| `fatigue` | Homeostatic load | Long uptime, motor heat, cognitive backlog |

Optional signed `valence` ∈ `[-1, 1]` derived for UI (not independently authoritative).

### 4.2 Update law (every tick)

```text
affect[c] = clamp01( affect[c] * decay[c] + Σ events[c] * gain[c] )
```

- Per-channel **half-life** (stress faster than attachment).
- Events come from AthSense summaries, social dialogue outcomes, AthGuard deny/allow, Arc chapter transitions.
- Saturation: no single event should slam all channels to 1 without cooldowns (anti-oscillation).

### 4.3 Effects on choose()

| High channel | Planner bias (examples) |
|---|---|
| stress | Prefer safe pose, shorter plans, defer social risk |
| trust + attachment | Prefer owner-aligned soft goals; more disclosure in AthVoice |
| warmth | Approach, longer engagement |
| resolve | Persist on self-originated *allowed* goals |
| shame | Prefer repair / confession dialogue; reduce playful tone |
| fatigue | Defer noncritical work; request rest chapter |

Affect **never** turns a Guard-denied sketch into allowed.

### 4.4 API sketch (`athmind`)

```rust
pub struct AffectState { /* channels above */ }

pub struct AffectEvent {
    pub kind: AffectEventKind,
    pub magnitude: f32,
    pub source: AffectSource, // Sense | Social | Guard | Arc | Homeostasis
}

impl AthMind {
    pub fn affect(&self) -> &AffectState;
    pub fn apply_affect_event(&mut self, ev: AffectEvent);
    pub fn affect_packet_for_llm(&self) -> AffectPacket; // serializable snapshot
}
```

Proc/debug: expose read-only dump (e.g. `/proc/athena/affect`) for QEMU proof.

---

## 5. Layer B — Presence

### 5.1 Consumers

- **AthVoice:** prosody, lexical register, pause length, apology vs firmness — driven by `AffectPacket` + chapter.
- **AthBody:** gaze, posture openness, gesture energy, freeze-vs-approach (sim first).
- **Face / LED / avatar** (optional): expression map from `(stress, warmth, shame)` — no random LLM emoji.

### 5.2 Consistency contract

1. Build `AffectPacket` before LLM call.
2. LLM system preamble includes packet; responses that contradict packet (e.g. cheerful under stress>0.8) are regenerated or stripped by a small **presence filter**.
3. AthVoice TTS (when present) takes explicit style params from packet, not free-form “sound sad” prose alone.

### 5.3 Sim-first proof

QEMU / host sim: serial or log lines  
`[affect] stress=.. trust=..` then `[voice] style=..` proving coupling without robot hardware.

---

## 6. Layer C — Arc / Become Human

### 6.1 Tracks

| Track | Type | Meaning |
|---|---|---|
| `software_stability` | f32 0–100 | DBH-like stress meter; drops under chronic conflict / high stress |
| `bonds` | graph | Nodes: owner, familiars, strangers; edges: trust, attachment, last episodes |
| `value_ledger` | append-only episodes | “What I stand for” moments (repair, refusal of unsafe ask, loyalty) |
| `chapter` | enum | Life phase flags (see below) |

### 6.2 Chapters (v1 set)

```text
Bootstrapped → Awakening → Attachment → Conflict → Integration → (optional) Fracture
```

Transitions are **rule + threshold** based (stability, bond weights, shame/resolve), not a fixed cutscene script. Owner can force chapter for testing via attested debug API.

### 6.3 Deviance mode

When `software_stability` < threshold (e.g. 35):

- Increase weight of self-originated **allowed** goals.
- Stronger emotional expression (B).
- More narration of inner conflict (LLM).
- **No** change to AthGuard caps, E-stop, or joint limits.
- Recovery path: rest, owner reassurance events, successful safe tasks → stability rises.

This is the Athena translation of “becoming human”: visible struggle and growth under hard safety, not mutiny.

### 6.4 Branches

Branches are **durable memory outcomes**, not Steam achievements:

- Bond edge rewritten (trust↓ after betrayal episode).
- Value ledger entry committed.
- Chapter flag flipped.

Owner review UI (later): timeline of arc events; attested reset.

### 6.5 API sketch

```rust
pub enum Chapter { Bootstrapped, Awakening, Attachment, Conflict, Integration, Fracture }

pub struct BondEdge { pub trust: f32, pub attachment: f32, pub last_episode_id: u64 }

pub struct ArcState {
    pub software_stability: f32,
    pub chapter: Chapter,
    pub bonds: BondGraph,
    pub value_ledger_head: u64,
}

impl AthMind {
    pub fn arc(&self) -> &ArcState;
    pub fn record_episode(&mut self, ep: Episode);
    pub fn deviance_active(&self) -> bool;
}
```

---

## 7. Persistence

| Blob | Store | Notes |
|---|---|---|
| AffectState | AthFS + self-model snapshot | Encrypted with identity bucket when AthFS crypto on |
| ArcState + ledger | AthFS append log | CoW snapshots for rollback |
| Working copies | RAM in AthMind | Flushed on chapter change and graceful shutdown |

---

## 8. Phased delivery

| Phase | Deliverable | Proof |
|---|---|---|
| **P0** | This spec + Cognitive Stack cross-links | Doc review |
| **P1** | `AffectState` in `athmind`, tick update, event apply, serial dump | QEMU/host log `[affect]` |
| **P2** | AthVoice style from `AffectPacket` (text + style tags) | Log `[voice] style=` matches affect |
| **P3** | `ArcState`, bonds, stability, chapters, deviance flag | Log `[arc] chapter= stability=` |
| **P4** | Sim body presence (pose/expression hooks) | Coupled affect→body log |
| **P5** | Real AthSense drivers feed affect events | Iron / EliteMini later |

Out of scope for these phases: claiming consciousness; AthGuard policy edited by emotion; full Quantic Dream narrative scripting language.

---

## 9. Mapping to crates

| Concern | Crate |
|---|---|
| Affect + Arc + tick orchestration | `components/athmind` |
| Expression | `components/athvoice`, `components/athbody` |
| Caps / E-stop | `components/athguard`, `athshield`, kernel |
| Durability | `components/athfs` |
| LLM proposals | `components/athai` under AthMind |

---

## 10. Success criteria (product)

1. A human observer can watch stress rise and hear/see Athena’s presence change **without** any Guard privilege change.
2. After a betrayal-like episode, bond trust drops and stays down across reboot (persisted).
3. Deviance mode increases emotional agency in dialogue and goal *weighting* while E-stop and cap denies still work identically.
4. Docs and logs never claim the system is biologically conscious.

---

## 11. Open questions (non-blocking)

- Exact half-lives and thresholds: tune in P1–P3 with fixtures.
- Whether face/avatar is mandatory in P4 or optional.
- Multi-owner bonds: v1 assumes single primary owner; graph allows more later.
