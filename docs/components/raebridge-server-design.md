# raebridge_server — Cross-Process Sync Broker (Design)

**Status:** design / interface decision (2026-06-28). Implements strategy-doc
[`raebridge-wine-strategy.md`](raebridge-wine-strategy.md) §6.1 "the wineserver gap."
Builds on the committed in-process object model (`lib.rs` `SyncObject`, commit
`e9be347`). Owner-assigned via /goal; interface-steward = Opus.

**Concept line served** (`RaeenOS_Concept.md` §Compatibility): "apps run naturally."
A multi-process Windows app — and Steam itself — needs a `Global\Name` mutex/event
to be *shared across processes* and `WaitForSingleObject` to *actually block*. The
in-process model (slice 1) handles a single `.exe`'s own threads; this broker is the
cross-process half.

---

## 1. Interface decision (the load-bearing call)

**The broker needs ~zero new `rae_abi` surface.** It rides two existing primitives:

| Primitive | Syscall | Why it's sufficient |
|---|---|---|
| Cross-process shared page | `SYS_CHANNEL_SHMEM_MAP` (119) | maps one channel's shared region into multiple processes' address spaces — the object-state page |
| True blocking wait/wake | `SYS_FUTEX` (258) | **keys on PHYSICAL address** (`kernel/src/sync.rs` `FutexManager`: *"Using physical address prevents issues with aliased virtual memory"*) → a futex on a shared page is inherently cross-process, like Linux `MAP_SHARED` futexes |

This is the **Wine fast-synchronization / Linux `ntsync` model**: object state lives in
shared memory, the steady-state wait/signal is a direct futex on that memory with **no
broker round-trip per operation**. The broker is consulted only on *open/create* (the
namespace lookup), not on every wait.

**The one open ABI question** (resolve in Slice 1, NOT pre-emptively): can the broker
hand a freshly-created channel capability to an arbitrary client process (cap transfer
across the broker)? If existing cap/channel plumbing already supports a daemon granting
a channel cap to a client, **no ABI change at all**. If not, that is the *single*
justified addition — one of the reserved native-sync slots (`rae_abi` 259–263, already
earmarked) for "open/create a named broker object → channel cap", landed as one
deliberate `[interface]` commit (`RAEEN_AGENT=opus`, `ABI_VERSION` bump, `SYSCALL_TABLE.md`
in the same commit). No speculative widening before Slice 1 proves whether it's needed.

---

## 2. Architecture

```
  guest .exe A                          guest .exe B
  kernel32::CreateMutexW("Global\Foo")  kernel32::OpenMutexW("Global\Foo")
        │  (named → broker)                    │  (named → broker)
        ▼                                       ▼
  ┌───────────────────── raebridge_server (userspace daemon) ─────────────────┐
  │  owns ONLY the namespace:  name "Global\Foo" → object-id → shared page     │
  │  create-on-first-open, refcount, free on last close                        │
  └───────────────────────────────────────────────────────────────────────────┘
        │  hands back: channel cap → SYS_CHANNEL_SHMEM_MAP → shared page
        ▼                                       ▼
  shared object-state page (same physical frame in both processes):
     [ state_word:AtomicU32 | owner_tid:AtomicU32 | recursion | sem_count | … ]
        │                                       │
   WaitForSingleObject = futex_wait(&state_word)   SetEvent = store + futex_wake(&state_word)
   (SYS_FUTEX 258, physical-keyed → cross-process, NO broker round-trip)
```

- **Unnamed / local objects** keep the existing in-process `SyncObject` store — zero
  IPC, fast path, already proven. Only `Global\`/`Local\`-*named* objects involve the
  broker. (The current in-process map already collapses the namespace; this splits the
  *named* ones out to the broker.)
- **The shared-page state layout** is a fixed C-ABI struct (one per object kind), the
  futex word at offset 0. This struct is the real cross-process contract — version it.

---

## 3. State machine (shared-memory, cross-process)

Mirror the in-process semantics already host-KAT'd in `run_sync_self_test`, but on
shared atomics with futex blocking:

- **Event (auto)**: `SetEvent` = `state.store(1); futex_wake(1)`. A waiter CAS-es 1→0
  (consumes) and returns; losers re-wait. **Manual**: `futex_wake(all)`, no consume;
  `ResetEvent` stores 0.
- **Mutex**: `state` = owner tid (0 = free). Acquire = CAS 0→tid; on fail, `futex_wait`;
  recursion counter for same-tid re-entry. `ReleaseMutex` from non-owner → FALSE +
  `ERROR_NOT_OWNER`; on full release store 0 + `futex_wake(1)`. Abandoned-mutex on
  owner death = broker reaps (it holds the refcount/owner table).
- **Semaphore**: `count` atomic; wait = decrement-if-positive else `futex_wait`;
  `ReleaseSemaphore(n)` = add n (clamp to max, else `ERROR_TOO_MANY_POSTS`) +
  `futex_wake(n)`.
- **WaitForMultipleObjects(all)**: the hard case — must avoid partial-acquire deadlock.
  Slice 4: acquire in a canonical (object-id) order with backoff, or a broker-arbitrated
  wait list. `WaitForMultipleObjects(any)` = wait on the first ready, else register on
  all and futex-wait a shared "any" word the broker pokes.

---

## 4. Slice plan (each is independently build+boot+KAT verifiable)

1. **Broker daemon skeleton + namespace** — `raebridge_server` process: `name → object`
   map, create/open/close + refcount, FAIL-able boot smoketest (two synthetic clients
   share one named object id). **Resolves the open ABI question** (cap handoff). Pure
   logic host-KAT first.
2. **Shared-page event** — auto/manual event state struct on a `SYS_CHANNEL_SHMEM_MAP`
   page; `SetEvent`/`ResetEvent`/`WaitForSingleObject` via futex across two real guest
   processes. Boot proof: process A waits, process B signals, A wakes (the thing the
   in-process model *cannot* do).
3. **Shared-page mutex + semaphore** — owner/recursion/abandoned; count/max. Reuse the
   §6.1 semantic tests, now cross-process.
4. **WaitForMultipleObjects (all/any) cross-process** + timeout plumbing (futex with
   deadline). Owner-death reaping.
5. **Wire kernel32 named-object shims** to route named → broker, unnamed → in-process;
   `DuplicateHandle` across processes via the broker. Update `run_sync_self_test` to add
   a cross-process leg.

R10 for the daemon: `init` (spawned at boot), `run_boot_smoketest` (FAIL-able cross-proc
rendezvous), `/proc/raeen/raebridge_server` line, Concept docstring.

## 5. Proof markers

- Host KAT: namespace refcount + each state machine on a simulated shared page (FAIL-able).
- Boot: `[raebridge_server] cross-proc event: A waited, B signaled, A woke -> PASS`,
  `[raebridge_server] named mutex shared across 2 procs -> PASS`, 7/7 HEALTHY, 0 panic.
- Iron: real Athena sweep (futex physical-keying + shmem map on real hardware).

## 6. Non-goals / guardrails

- Not a `wineserver` port — native daemon over RaeenOS primitives ([no Linux clones]).
- No per-wait broker round-trip (that would tank latency — futex direct-path is the point).
- Cap/sandbox: each broker object is capability-scoped; a guest only reaches names its
  manifest permits (ties into RaeShield Phase 9, deferred — fail-open for bring-up, noted).

---

## 7. Sign-off packet (for Opus / interface steward) — 2026-06-30

This section promotes §1's *decision* to a concrete, stampable interface. Nothing
here is landed by Composer; it is the exact proposal for a single deliberate
`[interface]` commit. All syscall facts below were re-verified against
`components/rae_abi/src/lib.rs` on 2026-06-30:

| Fact | Value | Source |
|---|---|---|
| Cross-process shared page | `SYS_CHANNEL_SHMEM_MAP = 119` | `rae_abi` line 699 |
| Blocking wait/wake | `SYS_FUTEX = 258` (op 0=WAIT,1=WAKE; WAIT: rdx=expected; returns 0 woke / 1 EAGAIN / 2 fault) | `rae_abi` lines 751–761 |
| Free reserved sync slots | `259..=263` (`RESERVED_SYNC_LO=258`, `..HI=263`; 258 taken) | `rae_abi` lines 775–776 |

`SYS_FUTEX` keys on the **physical** frame (`kernel/src/sync.rs` `FutexManager`),
so a futex on a `SYS_CHANNEL_SHMEM_MAP` page is cross-process for free — no
per-wait broker hop, matching Wine `ntsync`/Linux `MAP_SHARED` futex.

### 7.1 The shared-page C-ABI struct (the real cross-process contract)

One fixed `#[repr(C)]` frame per named object, futex word at **offset 0** so
`SYS_FUTEX` addresses the struct base directly. Version-gated; the broker rejects
a mismatched `version` at open. Proposed layout (all fields `AtomicU32`, natural
8-byte tail alignment; fits one page with room for future kinds):

```rust
// raebridge shared-sync object page — CROSS-PROCESS ABI. Bump VERSION on any
// field move/resize; broker validates it at open and refuses on mismatch.
pub const RAEBRIDGE_SYNC_ABI_VERSION: u32 = 1;

#[repr(u32)]
pub enum RaeSyncKind { Event = 1, Mutex = 2, Semaphore = 3 }

#[repr(C, align(8))]
pub struct RaeSyncObject {
    state:      AtomicU32, // OFFSET 0 = futex word. Event: 0/1 signaled.
                           //   Mutex: owner tid (0=free). Semaphore: live count.
    version:    AtomicU32, // == RAEBRIDGE_SYNC_ABI_VERSION
    kind:       AtomicU32, // RaeSyncKind
    flags:      AtomicU32, // bit0 = manual-reset (event); bit1 = initially-owned
    owner_tid:  AtomicU32, // mutex: current owner (mirror of state for clarity)
    recursion:  AtomicU32, // mutex: re-entrant acquire depth
    max_count:  AtomicU32, // semaphore: ceiling (ReleaseSemaphore clamp)
    _reserved:  AtomicU32, // pad to 32 bytes; future: abandoned flag, waiters hint
}
```

The broker owns the **namespace + refcount only** (name → object-id → frame,
create-on-first-open, free on last close, abandoned-owner reap). Steady-state
`WaitForSingleObject`/`SetEvent`/`ReleaseMutex` never call the broker — they are
`SYS_FUTEX` on the page. This struct — not any RPC schema — is the versioned
surface Opus signs.

### 7.2 The ONE open ABI question, made binary

The broker must hand a freshly-created channel capability to an arbitrary client
process (broker→client cap transfer). Exactly one of these is the outcome; Opus
picks:

- **Outcome A — NO ABI change (preferred).** If existing channel/cap plumbing
  already lets a daemon grant a channel cap to a connecting client (the same way
  other RaeenOS services hand back a channel), the broker uses it verbatim.
  Deliverable: zero `rae_abi` edits; Slice 1 proves cap handoff with a boot KAT.
- **Outcome B — ONE reserved slot.** If not, add a single syscall in the reserved
  sync block, e.g.:

  ```
  SYS_RAEBRIDGE_SYNC_OPEN = 259   // rdi=name_ptr, rsi=name_len, rdx=kind,
                                  //   r10=flags, r8=init_count/arg
                                  // -> returns channel cap id (or 0 + errno)
  ```

  Landed as one `[interface]` commit: `RAEEN_AGENT=opus`, additive so **no
  `ABI_VERSION` bump** (matches the `SYS_FUTEX` precedent), `docs/SYSCALL_TABLE.md`
  updated in the same commit, number drawn from `259..=263`.

No other slot is requested. No speculative widening. Composer files this as a
`NEEDS-INTERFACE:` line in `MasterChecklist.md`; the decision is Slice-1-scoped
(the skeleton is what reveals whether Outcome A already works).

### 7.3 Acceptance checklist for sign-off

1. Approve `RaeSyncObject` layout + `RAEBRIDGE_SYNC_ABI_VERSION` as the versioned
   cross-process contract (or request field changes).
2. Rule Outcome A vs B on cap handoff. If B, approve `SYS_RAEBRIDGE_SYNC_OPEN=259`
   (additive, no version bump).
3. Confirm no per-wait broker round-trip (futex direct-path) — latency guardrail.
4. Confirm capability scoping story (named object = cap-scoped; RaeShield Phase 9
   enforcement deferred, fail-open noted).

On sign-off, Slice 1 (§4) proceeds: broker skeleton + namespace + FAIL-able
cross-process rendezvous boot smoketest.
