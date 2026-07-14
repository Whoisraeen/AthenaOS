# ADR 0010 — Re-enable scheduler work stealing on QEMU multi-boot evidence (iron re-verify pending)

Date: 2026-07-01
Status: accepted
Owner: opus (kernel slice)

## Context

`WORK_STEALING_ENABLED` (kernel/src/scheduler.rs) has been `false` since the
intermittent steal-resume race (MasterChecklist "Latent kernel bugs", Phase 4.8):
a stolen task once resumed with `rsp=0` → push-to-NULL → `DOUBLE FAULT cpu=1
rsp=0x0 cr2=0xff..f8` (boot b7o42j0kk). Cross-CPU load balancing has been off
ever since — every runnable task stays on its home CPU, which wastes idle APs
under real multi-task load and directly contradicts Concept §"Fast is a feature"
(the scheduler exists to use the machine's cores).

Since the disable, four independent guards landed in the steal/switch path:

1. `pick_next` steal filter: affinity + running-elsewhere + in-transit
   (switch_stash snapshot) + explicit `stack_ptr != 0` + `saved_stack_is_sane()`
   + cold-task (`STEAL_MIN_COLD_TICKS`) checks, all under the SCHEDULER lock.
2. `saved_stack_is_sane()` gates EVERY pick (deadline/game/normal queues too),
   with `quarantine_insane_stack` parking a bad task instead of running it.
3. A FINAL pre-switch rsp-sanity tripwire on BOTH switch paths (`yield_task` +
   `block_current_task_with`) that quarantines and counts `SWITCH_ABORTS`
   instead of switching into an insane frame.
4. Wake-path hardening: dead-task (Zombie) guards on every `unblock_*`/
   `wake_thread*`, and the switch-stash Ready-in-place protocol
   (`mark_stash_ready_if`) that closes the enqueue-before-rsp-save window.

The docstring's re-enable bar was "iron multi-boot (>=5, SMP=2) showing
SWITCH_ABORTS staying 0 or firing with NO #DF". Iron flashing is human-gated
and paused this session.

Additionally, review found `unblock_futex_waiter` was the ONLY wake path
missing the `mark_stash_ready_if` stash catch — a futex wake racing a waiter's
block window was silently lost (wakes are one-shot). Fixed in the same commit.
Classification: defense-in-depth today (no current caller actually
futex-blocks; NVMe registers a waiter but keeps polling), live the moment any
path really parks on a futex.

## Options

1. Keep stealing off until iron resumes. Zero risk, but the latent-bug row
   stagnates and APs stay useless for load balancing; no new information is
   produced.
2. Re-enable now on QEMU multi-boot evidence, keep the row honest at `[~]`
   with the iron gate explicitly still open.
3. Re-enable only behind a non-default env/boot knob. Adds a config surface
   nobody flips; the tested artifact would not be the shipped artifact.

## Decision

Option 2. Run the soak the docstring demands, adapted to QEMU while iron is
paused: >=10 CI boots across `RAEEN_SMP=2` (x5), `=4` (x3), `=1` (x2), each
required to reach `[ OS ] System successfully booted.` with 0 KERNEL PANIC,
0 DOUBLE FAULT, 0 `[sched] ABORT` events, `switch_aborts: 0`, and (at SMP>=2)
nonzero `PER_CPU_STEALS` proving the steal path actually exercised. Land
`WORK_STEALING_ENABLED = true` only if ALL boots pass; otherwise keep it off
and commit the repro + findings instead.

Tie-breakers (AUTONOMY_CHARTER §2): the Concept demands multi-core use (1);
the tripwire turns the historical failure mode into a logged, recoverable
quarantine rather than a #DF, so the risk profile is bounded (2, 3); and the
change is a one-word AtomicBool flip — trivially reversible (4).

## Reversal

Set `WORK_STEALING_ENABLED` back to `AtomicBool::new(false)`. All guards and
counters stay regardless. If any iron boot ever shows `switch_aborts > 0`,
treat it as the race CAUGHT (not crashed): keep stealing on, use the abort's
serial diagnostic to root-cause which wake path produced the insane frame.

## Verification artifacts

- Soak tally: MasterChecklist "Latent kernel bugs" row (per-boot table).
- Live counters: `/proc/raeen/sched` (`switch_aborts`, per-CPU `steals=`),
  `[sched] stack-sanity guard ... live_switch_aborts=0 -> PASS` smoketest.
- Iron gate for `[x]`: >=5 KVM/iron boots at SMP=1 and =2, `switch_aborts: 0`.
