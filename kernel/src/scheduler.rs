//! CFS-style scheduler with SCHED_BODY hard-priority class and per-CPU runqueues.
//!
//! Concept §Scheduling: "per-CPU runqueues (10k context switches/s per core)".
//!
//! Design:
//!   * Each core has its own `Runqueue` with three priority tiers:
//!     1. Deadline Game tasks (EDF).
//!     2. Regular Game tasks (Round-robin).
//!     3. Normal tasks (CFS-style vruntime).
//!   * Tasks are enqueued to a specific core's runqueue based on affinity.
//!   * Work-stealing logic balances load when a core becomes idle.

use crate::arch::VirtAddr;
use crate::task::{CpuAffinity, DeadlineTask, Task, TaskId, TaskPriority, TaskState};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use lazy_static::lazy_static;
use spin::Mutex;

pub static BOOT_COMPLETE: AtomicBool = AtomicBool::new(false);

/// Work-stealing load-balancer toggle. ON since 2026-07-01 (history below —
/// read it before ever trusting a short green streak on this path).
///
/// CORRECTED DIAGNOSIS (owner, 2026-06-16): this is an intermittent
/// steal-resume RACE, NOT the earlier "unrelated stack-content corruption"
/// theory. It was briefly (and wrongly) marked done after two lucky green boots
/// (bicz136cu / bvj8q5iks, 1740 steals); a later clean single boot (b7o42j0kk)
/// double-faulted: `DOUBLE FAULT cpu=1 rsp=0x0 cr2=0xff..f8` — a stolen task
/// resumed with rsp=0 (kernel push to NULL). So the steal path can pick a task
/// whose saved `stack_ptr` is *transiently* invalid — a window between enqueue
/// and the `switch_context` that saves rsp, likely widened by the futex
/// block/yield path. INTERMITTENT (passed twice, failed once), which is exactly
/// why two green boots are not proof.
///
/// What's in place (this commit): an explicit `stack_ptr != 0` in the steal
/// filter; `saved_stack_is_sane()` rejects 0 explicitly; and a FINAL pre-switch
/// tripwire on BOTH switch paths (`yield_task` + `block_current_task_with`)
/// that quarantines an insane `next` and counts `SWITCH_ABORTS` instead of
/// double-faulting. These turn the silent #DF into a recoverable, OBSERVABLE
/// event — the next iron boot reads `SWITCH_ABORTS` via /proc/raeen/sched_stats.
///
/// RE-ENABLED 2026-07-01 after the QEMU multi-boot soak the paragraph above
/// demands (adapted to QEMU while iron flashing is paused): >=10 CI boots
/// across RAEEN_SMP=2 / =4 / =1, every boot reaching the success marker with
/// 0 panics, 0 double faults, `switch_aborts: 0`, and real steals recorded
/// (PER_CPU_STEALS > 0 at SMP>=2) — see MasterChecklist "Latent kernel bugs"
/// for the per-boot tally. The failure mode is also no longer a #DF: pick_next
/// sane-checks every candidate (running-elsewhere + in-transit + affinity +
/// `saved_stack_is_sane`) and BOTH switch paths carry the final rsp-sanity
/// tripwire that quarantines an insane task and counts `SWITCH_ABORTS`
/// instead of switching into it. Iron re-verify pending (row stays [~]):
/// >=5 KVM/iron boots at SMP=1 and =2 with `switch_aborts: 0` still required
/// before the row can claim [x].
pub static WORK_STEALING_ENABLED: AtomicBool = AtomicBool::new(true);
const MIN_GRANULARITY_VNS: u64 = 1_000_000;
const TICK_PERIOD_US: u64 = 1_000;

/// Wall-time monotonic microseconds since boot, from the TSC — the EDF deadline
/// clock. This REPLACES the old `tick * TICK_PERIOD_US` base, which advanced one
/// "µs-of-1000" per `yield_task` call (timers + cooperative yields), so it ran at
/// ~1/10 real rate AND was paced by voluntary yields rather than time — degrading
/// SCHED_BODY deadline accounting for short periods (MasterChecklist ~L435, the
/// "EDF-clock 1/10 defect"). Absolute deadlines (`now + relative`) and miss checks
/// (`now > absolute_deadline`) are all computed in these real µs, so sub-6 ms
/// (input) and sub-3 ms (audio) periods are now accounted against true wall time.
///
/// Clock source: `fast_boot::tsc_mhz()` (TSC ticks per microsecond), populated
/// from `apic::TSC_FREQ_MHZ` during boot's APIC calibration — the SAME calibrated
/// frequency `/proc/raeen/perf` (`record_frame_present`) already uses. Critically
/// it requires NO spin/PIT calibration here, so it is safe to read on the IF=0
/// SCHEDULER-lock hot path AND on the boot-tail idle context. (An earlier version
/// PIT-spun `TscCalibration::calibrate()` from the boot tail; a timer tick could
/// preempt CPU 0 mid-spin and the idle/boot context never resumed to print the
/// success marker — root-caused 2026-06-22. Reusing the already-calibrated value
/// removes that hazard entirely.)
///
/// `tsc_mhz()` is nonzero from APIC calibration onward (long before BOOT_COMPLETE),
/// so the `fallback_tick` legacy base is used only in the vanishingly small pre-
/// calibration window; it keeps deadline ordering self-consistent there.
///
/// TSC-sync caveat (CLAUDE §10, MasterChecklist 4.8 "TSC sync WARN across CPUs"):
/// the EDF path here is BSP-pinned in practice (APs `hlt`-idle post-boot and the
/// SCHED_BODY service threads + sched_proof are affinity-masked to CPU 0), so the
/// clock is read on ONE CPU and per-boot monotonicity holds even before cross-CPU
/// TSC sync lands — exactly how the compositor's `monotonic_us` is used today.
#[inline]
fn edf_monotonic_us(fallback_tick: u64) -> u64 {
    let mhz = crate::fast_boot::tsc_mhz(); // TSC ticks per microsecond
    if mhz == 0 {
        // Pre-APIC-calibration only: fall back to the legacy tick base so deadline
        // ordering stays self-consistent. No spin — never block the IF=0 hot path.
        return fallback_tick.saturating_mul(TICK_PERIOD_US);
    }
    let tsc = crate::timers::TscCalibration::read_tsc();
    tsc / mhz
}

/// No-op retained for call-site clarity / R10 init ergonomics: the EDF wall-clock
/// needs NO separate calibration — it reuses `fast_boot::tsc_mhz()`, already set
/// during boot's APIC calibration. We log the live frequency here (from
/// `sched_proof::init`, post-BOOT_COMPLETE, interrupts ENABLED) so the boot log
/// records that the deadline clock is wall-time µs. Must NOT spin (it runs on the
/// fragile boot-tail idle context — see `edf_monotonic_us`).
pub fn prewarm_edf_clock() {
    let mhz = crate::fast_boot::tsc_mhz();
    crate::serial_println!(
        "[ OK ] EDF wall-clock active: tsc={} MHz (ticks/µs) — deadline clock is wall-time µs, not tick*1000",
        mhz
    );
}
/// Minimum ticks (≈ms) a task must have been NOT running before another CPU may
/// steal it. A task that ran within this window is "hot" and stays on its core;
/// only genuinely-waiting backlog is stolen. Without this, two bursty service
/// threads (e.g. the xHCI HID servicer) ping-pong between cores every tick — the
/// steal-thrash livelock observed as 1550 steals of just 2 tasks in one boot.
const STEAL_MIN_COLD_TICKS: u64 = 3;
/// Defense-in-depth (see `Scheduler::dl_starve_streak`): after this many
/// back-to-back `pick_next` selections of a deadline task on one CPU while that
/// CPU's Normal queue had runnable work, force exactly one Normal pick so a
/// misbehaving / halted-while-current deadline task can never wedge the
/// lower-class queue forever. A correct periodic EDF task `finish()`es and
/// falls through to Normal on its own, so it never reaches this; the limit is
/// comfortably above the few back-to-back picks a healthy mix produces.
const DL_STARVATION_LIMIT: u32 = 64;

const COMPOSITOR_PERIOD_US: u64 = 16_667;
const COMPOSITOR_DEADLINE_US: u64 = 14_000;
const COMPOSITOR_RUNTIME_US: u64 = 8_000;

const AUDIO_PERIOD_US: u64 = 2_667;
const AUDIO_DEADLINE_US: u64 = 2_000;
const AUDIO_RUNTIME_US: u64 = 1_500;

const THROTTLE_RATIO: u64 = 20;

// ─── Game Mode State ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct GameModeState {
    active: bool,
    game_ticks: u64,
    bg_ticks_allowed: u64,
    bg_ticks_used: u64,
}

impl GameModeState {
    const fn new() -> Self {
        Self {
            active: false,
            game_ticks: 0,
            bg_ticks_allowed: 0,
            bg_ticks_used: 0,
        }
    }
    fn tick(&mut self) {
        if !self.active {
            return;
        }
        self.game_ticks += 1;
        self.bg_ticks_allowed = self.game_ticks / THROTTLE_RATIO;
    }
    fn can_run_background(&self) -> bool {
        !self.active || self.bg_ticks_used < self.bg_ticks_allowed
    }
    fn charge_background(&mut self) {
        self.bg_ticks_used += 1;
    }
    fn reset_counters(&mut self) {
        self.game_ticks = 0;
        self.bg_ticks_allowed = 0;
        self.bg_ticks_used = 0;
    }
}

// ─── NULL_LATENCY Mode ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
struct NullLatencyState {
    active: bool,
    game_task_id: Option<TaskId>,
    dedicated_cores: u64,
    non_game_cores: u64,
    saved_affinity: u64,
}

impl NullLatencyState {
    const fn new() -> Self {
        Self {
            active: false,
            game_task_id: None,
            dedicated_cores: 0,
            non_game_cores: 0,
            saved_affinity: u64::MAX,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GameProfile {
    pub affinity: CpuAffinity,
    pub priority: TaskPriority,
    pub deadline: Option<DeadlineTask>,
}

impl GameProfile {
    pub fn default_profile() -> Self {
        Self {
            affinity: CpuAffinity::performance_cores(),
            priority: TaskPriority::Game,
            deadline: None,
        }
    }
    pub fn with_deadline(period_us: u64, deadline_us: u64, runtime_us: u64) -> Self {
        Self {
            affinity: CpuAffinity::performance_cores(),
            priority: TaskPriority::Game,
            deadline: Some(DeadlineTask::new(period_us, deadline_us, runtime_us)),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DeadlineStats {
    pub total_tasks: u64,
    pub total_invocations: u64,
    pub total_misses: u64,
    pub worst_miss_us: u64,
}

lazy_static! {
    static ref SCHEDULER: Mutex<Scheduler> = Mutex::new(Scheduler::new());
}

/// Force-release the scheduler lock.  Called from the double-fault handler
/// to prevent a system-wide deadlock when one CPU crashes while holding it.
///
/// # Safety
/// Only safe when the caller is certain the lock holder will never resume
/// (i.e., the faulting CPU is about to enter `hlt_loop()`).
pub unsafe fn force_unlock_scheduler() {
    SCHEDULER.force_unlock();
}

struct Runqueue {
    tasks_deadline: VecDeque<Task>,
    tasks_game: VecDeque<Task>,
    tasks_normal: VecDeque<Task>,
    cpu_id: usize,
}

impl Runqueue {
    fn new(cpu_id: usize) -> Self {
        Self {
            tasks_deadline: VecDeque::new(),
            tasks_game: VecDeque::new(),
            tasks_normal: VecDeque::new(),
            cpu_id,
        }
    }
    fn is_empty(&self) -> bool {
        self.tasks_deadline.is_empty() && self.tasks_game.is_empty() && self.tasks_normal.is_empty()
    }
    /// Total runnable tasks queued on this core (all three classes). Read by the
    /// perf telemetry max-runqueue-depth observer at pick time.
    fn len(&self) -> usize {
        self.tasks_deadline.len() + self.tasks_game.len() + self.tasks_normal.len()
    }
}

struct Scheduler {
    runqueues: [Runqueue; crate::gdt::MAX_CPUS],
    /// Stable storage for the outgoing task while `switch_context` runs with the
    /// scheduler lock dropped (yield path). Must be empty when not switching.
    switch_stash: [Option<Task>; crate::gdt::MAX_CPUS],
    /// `Box` keeps each blocked task at a stable address across vec growth.
    blocked_tasks: alloc::vec::Vec<alloc::boxed::Box<Task>>,
    /// Dead tasks awaiting cleanup. Freeing a task (kernel stack unmap +
    /// page-table teardown + frame dealloc) must NOT happen inline during reap,
    /// exit, or a context switch: those run with the SCHEDULER lock held and/or
    /// while another CPU may still hold a reference, so freeing-then-reusing the
    /// frames corrupts a live task (observed 2026-06-10 as an NX instruction
    /// fetch at a kernel-stack address after a child reap). Instead the corpse
    /// is parked here and dropped by `drain_dead_tasks()` from the idle thread,
    /// on the idle stack, with no lock held — by then nothing references it.
    dead_tasks: alloc::vec::Vec<alloc::boxed::Box<Task>>,
    current_task: [Option<Task>; crate::gdt::MAX_CPUS],
    idle_task: [Option<Task>; crate::gdt::MAX_CPUS],
    tick: u64,
    min_vruntime: u64,
    game_mode: GameModeState,
    null_latency: NullLatencyState,
    deadline_stats: DeadlineStats,
    /// Defense-in-depth anti-starvation counter: consecutive `pick_next`
    /// selections of a DEADLINE task on each CPU while that CPU's Normal queue
    /// was non-empty. A well-behaved periodic EDF task `finish()`es its work
    /// and `pick_next` then falls through to Normal on its own, so this never
    /// climbs for it. It only climbs for a misbehaving deadline task that stays
    /// runnable without finishing (e.g. the 360b862 xHCI idle `hlt()` that left
    /// the HID EDF task `current` + un-`finish()`ed → it re-won every period and
    /// starved all Normal tasks). At `DL_STARVATION_LIMIT` we force ONE Normal
    /// pick so the lower class can never be wedged indefinitely. Reset whenever
    /// a non-deadline task is picked or no Normal task is waiting.
    dl_starve_streak: [u32; crate::gdt::MAX_CPUS],
}

impl Scheduler {
    fn new() -> Self {
        Scheduler {
            runqueues: core::array::from_fn(|i| Runqueue::new(i)),
            switch_stash: core::array::from_fn(|_| None),
            blocked_tasks: Vec::new(),
            dead_tasks: Vec::new(),
            current_task: core::array::from_fn(|_| None),
            idle_task: core::array::from_fn(|_| None),
            tick: 0,
            min_vruntime: 0,
            game_mode: GameModeState::new(),
            null_latency: NullLatencyState::new(),
            deadline_stats: DeadlineStats::default(),
            dl_starve_streak: [0; crate::gdt::MAX_CPUS],
        }
    }

    fn insert_normal_sorted(&mut self, cpu_id: usize, task: Task) {
        let vr = task.vruntime;
        let rq = &mut self.runqueues[cpu_id];
        let pos = rq.tasks_normal.iter().position(|t| t.vruntime > vr);
        match pos {
            Some(i) => rq.tasks_normal.insert(i, task),
            None => rq.tasks_normal.push_back(task),
        }
    }

    fn insert_deadline_sorted(&mut self, cpu_id: usize, task: Task) {
        let dl = task
            .deadline
            .as_ref()
            .map_or(u64::MAX, |d| d.absolute_deadline);
        let rq = &mut self.runqueues[cpu_id];
        let pos = rq.tasks_deadline.iter().position(|t| {
            t.deadline
                .as_ref()
                .map_or(u64::MAX, |d| d.absolute_deadline)
                > dl
        });
        match pos {
            Some(i) => rq.tasks_deadline.insert(i, task),
            None => rq.tasks_deadline.push_back(task),
        }
    }

    fn pick_next(&mut self, cpu_id: usize) -> Option<Task> {
        // EDF deadline clock = TSC wall-time µs (not the per-yield tick counter).
        // `self.tick` is passed only as the never-PIT-spin fallback for the (early)
        // uncalibrated window; once prewarmed this is real wall-time microseconds.
        let now_us = edf_monotonic_us(self.tick);
        let tick = self.tick;

        // 1. Deadline (EDF) — but ONLY a task that actually has work to do
        //    THIS period. A periodic SCHED_BODY task (compositor @16ms,
        //    audio @2.67ms) that already finished its work for the current
        //    period must yield the CPU to lower scheduling classes until
        //    its next period begins. Otherwise it stays runnable in
        //    `tasks_deadline` and step 1 returns it on every single tick,
        //    starving ALL normal tasks forever — the bug that prevented
        //    user_init (and thus every userspace process) from ever
        //    running. A task is runnable this period if a new period has
        //    started (needs_new_period → fresh budget) or it hasn't yet
        //    finished the current period's work (last_finish < last_wake).
        //    tasks_deadline is kept absolute-deadline-sorted, so the first
        //    runnable entry we find is the earliest-deadline ready task.
        //
        //    DEFENSE-IN-DEPTH (anti-starvation): the per-period `finish()`
        //    fairness above only works if the deadline task actually reaches
        //    `yield_task` to call `finish()`. A deadline task that halts while
        //    `current` (the 360b862 xHCI idle `hlt()` regression) never does,
        //    stays runnable, and re-wins here every period — starving Normal.
        //    `dl_starve_streak` counts consecutive deadline picks made while
        //    Normal had runnable work; once it crosses DL_STARVATION_LIMIT we
        //    skip the deadline class for ONE pick and let Normal run. A healthy
        //    periodic EDF task `finish()`es and falls through on its own, so it
        //    never accumulates the streak — this only ever fires for a
        //    misbehaving never-finishing deadline task.
        let normal_runnable = !self.runqueues[cpu_id].tasks_normal.is_empty();
        let force_normal = normal_runnable && self.dl_starve_streak[cpu_id] >= DL_STARVATION_LIMIT;
        if force_normal {
            crate::serial_println!(
                "[sched] CPU{} deadline-starvation guard tripped (streak={}) — forcing one Normal pick",
                cpu_id,
                self.dl_starve_streak[cpu_id]
            );
            self.dl_starve_streak[cpu_id] = 0;
        }
        while !force_normal {
            let dl_idx = self.runqueues[cpu_id].tasks_deadline.iter().position(|t| {
                t.deadline.as_ref().map_or(true, |dl| {
                    dl.needs_new_period(now_us) || dl.last_finish < dl.last_wake
                })
            });
            let Some(idx) = dl_idx else { break };
            let Some(mut t) = self.runqueues[cpu_id].tasks_deadline.remove(idx) else {
                break;
            };
            if let Some(ref mut dl) = t.deadline {
                if dl.needs_new_period(now_us) {
                    dl.wake(now_us);
                    // Global SCHED_BODY telemetry: one deadline period started.
                    // This is the miss-RATE denominator read by
                    // /proc/raeen/gaming + sys_deadline_stats. Without it the
                    // aggregate's `total_invocations` stayed 0 forever, so the
                    // "missed N frames out of M" indicator had no M (rate
                    // permanently 0.00% even with misses). Bumped in lockstep
                    // with the per-task `dl.total_invocations` above.
                    self.deadline_stats.total_invocations += 1;
                }
            }
            // Refuse to switch into a task whose saved kernel SP is corrupt
            // (would resume on rsp=0 → #DF); quarantine it and try the next.
            if !t.saved_stack_is_sane() {
                self.quarantine_insane_stack(t, cpu_id, "deadline");
                continue;
            }
            // A deadline pick made while Normal had runnable work bumps the
            // anti-starvation streak; if Normal was empty there is nothing to
            // starve, so reset.
            if normal_runnable {
                self.dl_starve_streak[cpu_id] = self.dl_starve_streak[cpu_id].saturating_add(1);
            } else {
                self.dl_starve_streak[cpu_id] = 0;
            }
            // SCHED_BODY deadline-miss telemetry (Concept "Gaming isn't a mode",
            // CLAUDE §1 North Star table). THIS is the dispatch point: the
            // earliest-deadline ready EDF task is about to get the CPU. If the
            // monotonic clock (`now_us`, same µs base as `absolute_deadline`)
            // is already past its deadline, this period is provably late —
            // record it lock-free (the perf atomics; we hold SCHEDULER, so no
            // new lock is taken). `dl` is in scope from the wake/finish block
            // above; read its absolute deadline for the comparison.
            let abs_dl = t
                .deadline
                .as_ref()
                .map_or(u64::MAX, |d| d.absolute_deadline);
            crate::perf::record_game_dispatch(now_us, abs_dl);
            crate::perf::observe_runq_depth(self.runqueues[cpu_id].len() as u64);
            t.mark_scheduled(cpu_id, tick);
            return Some(t);
        }
        // else: every deadline task already finished its period's work (or the
        // anti-starvation guard forced us here) — fall through to game / normal
        // so they get the reserved bandwidth.

        // 2. Game
        loop {
            let Some(mut t) = self.runqueues[cpu_id].tasks_game.pop_front() else {
                break;
            };
            if !t.saved_stack_is_sane() {
                self.quarantine_insane_stack(t, cpu_id, "game");
                continue;
            }
            // A lower-class pick means the deadline class is no longer
            // monopolizing the CPU — clear the streak.
            self.dl_starve_streak[cpu_id] = 0;
            t.mark_scheduled(cpu_id, tick);
            return Some(t);
        }

        // 3. Normal (CFS)
        if !self.game_mode.active || self.game_mode.can_run_background() {
            loop {
                let Some(mut t) = self.runqueues[cpu_id].tasks_normal.pop_front() else {
                    break;
                };
                if !t.saved_stack_is_sane() {
                    self.quarantine_insane_stack(t, cpu_id, "normal");
                    continue;
                }
                if self.game_mode.active {
                    self.game_mode.charge_background();
                }
                self.dl_starve_streak[cpu_id] = 0;
                t.mark_scheduled(cpu_id, tick);
                return Some(t);
            }
        }

        // 4. Work Stealing (runtime-gated by WORK_STEALING_ENABLED).
        //
        // The per-CPU SYSCALL-stack omission in block_current_task_with (fixed
        // 2026-06-11) cured the resume corruption on the non-steal paths. The
        // steal-resume #DF (cpu=1 rsp=0) is now closed by the guards below
        // (running-elsewhere + in-transit + affinity + saved_stack_is_sane) and
        // the pick_next-wide sane-stack gate; the runtime toggle stays so it can
        // be flipped without recompiling the surrounding logic.
        if !WORK_STEALING_ENABLED.load(Ordering::Relaxed) {
            return None;
        }
        let online = crate::smp::ONLINE_CPUS.load(Ordering::Relaxed) as usize;
        let online_limit = core::cmp::min(online, crate::gdt::MAX_CPUS);
        // Snapshot the set of task ids currently RUNNING on some CPU. A task
        // that is already a `current_task` somewhere must never be stolen and
        // switched-to here: a wake/stash race can briefly leave a duplicate
        // entry of a running task in a runqueue, and switching to it would
        // execute on a kernel stack / page table the running copy is actively
        // using → memory corruption → double fault (observed as the cpu=1 #DF
        // when a faulting user task is killed while another CPU steals it).
        // We hold the global SCHEDULER lock, so this snapshot is consistent.
        // MasterChecklist Phase 4.8 — SMP scheduler stability.
        let mut running_ids = [None::<crate::task::TaskId>; crate::gdt::MAX_CPUS];
        for (i, ct) in self.current_task.iter().enumerate() {
            if let Some(t) = ct {
                running_ids[i] = Some(t.id);
            }
        }
        // Also snapshot ids IN TRANSIT in any CPU's switch_stash — a task that
        // is mid-context-switch (block/yield dropped the SCHEDULER lock before
        // switch_context saved its SP). It isn't in a runqueue, but if a
        // wake/stash race ever left a DUPLICATE of it in one, stealing that
        // copy would run a 2nd context on the in-flight task's kernel stack →
        // corruption → the cpu=1 rsp=0 #DF. Never steal an in-transit id.
        let mut intransit_ids = [None::<crate::task::TaskId>; crate::gdt::MAX_CPUS];
        for (i, s) in self.switch_stash.iter().enumerate() {
            if let Some(t) = s {
                intransit_ids[i] = Some(t.id);
            }
        }
        let is_running_elsewhere =
            |id: crate::task::TaskId| running_ids.iter().flatten().any(|&r| r == id);
        let is_in_transit =
            |id: crate::task::TaskId| intransit_ids.iter().flatten().any(|&r| r == id);
        for i in 0..online_limit {
            if i == cpu_id {
                continue;
            }
            // Honor affinity: only steal a task that is allowed to run on the
            // stealing CPU. The old code unconditionally pop_back()'d any task,
            // which silently migrated CPU0-pinned tasks onto other cores — the
            // root of the user_init steal-thrash livelock. Scan from the back
            // (coldest first) for the first task whose affinity allows cpu_id,
            // is not running or in-transit elsewhere, AND whose saved kernel
            // stack pointer is sane (points into its own stack — else switching
            // to it resumes on a garbage RSP, the rsp=0 #DF).
            let candidate = self.runqueues[i].tasks_normal.iter().rposition(|t| {
                t.affinity.is_allowed(cpu_id as u32)
                    && !is_running_elsewhere(t.id)
                    && !is_in_transit(t.id)
                    // Explicit rsp!=0 BEFORE the range test: the intermittent
                    // steal-resume #DF (b7o42j0kk) was a stolen task resumed on
                    // rsp=0. saved_stack_is_sane rejects 0, but stating it here
                    // documents the invariant at the exact decision point and
                    // is cheap insurance against a degenerate-base window.
                    && t.stack_ptr.as_u64() != 0
                    && t.saved_stack_is_sane()
                    // Only steal a COLD task — one not run for the last
                    // STEAL_MIN_COLD_TICKS. A hot task that just ran stays on its
                    // core; this kills the CPU0↔CPU1 ping-pong of bursty service
                    // threads (the 1550-steals-of-2-tasks thrash) while still
                    // migrating genuinely-waiting backlog.
                    && tick.saturating_sub(t.last_ran_tick) >= STEAL_MIN_COLD_TICKS
            });
            if let Some(pos) = candidate {
                if let Some(mut t) = self.runqueues[i].tasks_normal.remove(pos) {
                    if cpu_id < crate::gdt::MAX_CPUS {
                        PER_CPU_STEALS[cpu_id].fetch_add(1, Ordering::Relaxed);
                    }
                    t.mark_scheduled(cpu_id, tick);
                    return Some(t);
                }
            }
        }

        None
    }

    /// A task whose saved kernel SP is outside its own stack would resume on a
    /// garbage RSP (`switch_context` does `mov rsp,<sp>` then pops/`ret`; sp=0
    /// faults at ~-8 → `cr2=0xff..f8`) — the intermittent SMP work-stealing
    /// #DF. Rather than crash, drop it out of the run path and park it FROZEN
    /// in `blocked_tasks` (only the by-id `is_blocked()`-gated wake paths pull
    /// from there, and a Ready-state task with no waker is never matched), with
    /// a loud, greppable diagnostic. We do NOT free it (its stack may be shared
    /// with a live duplicate; freeing could unmap a stack another CPU is on).
    /// On the happy path `saved_stack_is_sane()` is always true, so this never
    /// runs (MasterChecklist 4.8 — SMP scheduler stability).
    fn quarantine_insane_stack(&mut self, t: Task, cpu_id: usize, queue: &str) {
        crate::serial_println!(
            "[sched] DROP Task {:?} ({} queue, CPU{}): saved SP {:#x} not inside its kstack (end {:#x}) — refusing to switch (would rsp=0 #DF)",
            t.id,
            queue,
            cpu_id,
            t.stack_ptr.as_u64(),
            t.kernel_stack_end().as_u64(),
        );
        self.blocked_tasks.push(alloc::boxed::Box::new(t));
    }

    fn enqueue(&mut self, task: Task) {
        // Last-line defense: a Zombie must NEVER reach a runqueue. Running a
        // corpse switches into a stale saved SP whose kernel stack the exit
        // path already overwrote → garbage context → #DF (MasterChecklist
        // 4.8). The wake paths now filter dead tasks at the source; this
        // catches any path we missed and preserves the corpse for wait().
        if matches!(task.state, TaskState::Zombie(_)) {
            crate::serial_println!(
                "[sched] BUG: refused to enqueue Zombie Task {:?} — parked for wait()",
                task.id
            );
            self.blocked_tasks.push(alloc::boxed::Box::new(task));
            return;
        }
        let cpu_id = self.select_cpu(&task);
        if task.priority == TaskPriority::Game && task.deadline.is_some() {
            self.insert_deadline_sorted(cpu_id, task);
        } else if task.priority == TaskPriority::Game {
            self.runqueues[cpu_id].tasks_game.push_back(task);
        } else {
            self.insert_normal_sorted(cpu_id, task);
        }
    }

    fn select_cpu(&self, task: &Task) -> usize {
        let mask = task.affinity.mask;
        let online = crate::smp::ONLINE_CPUS.load(Ordering::Relaxed) as usize;
        let online_limit = core::cmp::min(online, crate::gdt::MAX_CPUS);
        // Cache affinity: prefer the core this task last ran on, if it is online
        // and allowed. This keeps a periodic task (the xHCI HID servicer, a
        // daemon) on ONE core across sleep/wake cycles instead of defaulting
        // back to CPU0 every wake and being re-stolen by an idle AP — the
        // steal-thrash livelock (491 steals / 491 picks in one boot). `last_cpu`
        // is u32::MAX until the task has run once; `home < online_limit`
        // short-circuits before the shift so the sentinel can't overflow it.
        let home = task.last_cpu as usize;
        if home < online_limit && (mask & (1 << home)) != 0 {
            return home;
        }
        // No valid home yet: first allowed CPU that is currently idle.
        for i in 0..online_limit {
            if (mask & (1 << i)) != 0 && self.current_task[i].is_none() {
                return i;
            }
        }
        // Fallback: first allowed.
        for i in 0..online_limit {
            if (mask & (1 << i)) != 0 {
                return i;
            }
        }
        // If the task mask specifies exclusively offline CPUs, fallback to CPU 0
        0
    }

    fn check_admission(&self, cpu_id: usize, task: &Task) -> bool {
        let Some(ref dl) = task.deadline else {
            return true;
        };

        let mut total_util_mc = (dl.runtime_us * 1000) / dl.period_us;
        let rq = &self.runqueues[cpu_id];

        for t in &rq.tasks_deadline {
            if let Some(ref d) = t.deadline {
                total_util_mc += (d.runtime_us * 1000) / d.period_us;
            }
        }

        // Threshold: 80% (800 milli-cores)
        total_util_mc <= 800
    }

    fn check_deadline_misses(&mut self) {
        let now_us = edf_monotonic_us(self.tick);
        for i in 0..self.current_task.len() {
            if let Some(ref mut current) = self.current_task[i] {
                if let Some(ref mut dl) = current.deadline {
                    if dl.check_miss(now_us) {
                        let overshoot = now_us.saturating_sub(dl.absolute_deadline);
                        self.deadline_stats.total_misses += 1;
                        self.deadline_stats.worst_miss_us =
                            self.deadline_stats.worst_miss_us.max(overshoot);
                        dl.runtime_us = dl.runtime_us.saturating_add(dl.runtime_us / 4);
                    }
                }
            }
        }
    }

    fn apply_throttle_flags(&mut self, throttled: bool) {
        for rq in self.runqueues.iter_mut() {
            for t in rq.tasks_normal.iter_mut() {
                t.throttled = throttled;
            }
        }
        for t in self.blocked_tasks.iter_mut() {
            if t.priority != TaskPriority::Game {
                t.throttled = throttled;
            }
        }
    }
}

pub fn spawn(task: Task) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        let mut task = task;
        if task.priority == TaskPriority::Normal {
            task.vruntime = sched.min_vruntime;
            if sched.game_mode.active {
                task.throttled = true;
            }
        }
        if let Some(ref mut dl) = task.deadline {
            // First period anchored to the SAME wall-time µs base pick_next uses.
            dl.wake(edf_monotonic_us(sched.tick));
            // Register the SCHED_BODY task and its first deadline period in the
            // global aggregate (see pick_next for why the denominator matters).
            sched.deadline_stats.total_tasks += 1;
            sched.deadline_stats.total_invocations += 1;
        }
        sched.enqueue(task);
    });
}

/// Spawn a kernel thread PINNED to the BSP (CPU 0).
///
/// KEYSTONE (root-caused from iron bootlog 2026-06-15T1037): the AP cores enter
/// a bare `loop { hlt }` after coming online (`ap_enter_idle`) and never pull
/// from their runqueues post-boot. A kernel thread's DEFAULT affinity is
/// `performance_cores()` (mask 0x0F = CPUs 0-3), so `select_cpu` scatters
/// post-boot service threads onto those halted APs where they SILENTLY never
/// run — that was the single cause of: desktop never coming up (auto_advance),
/// dead mouse (hid_input drain), no DHCP (net poll), and the missing late
/// flush. CPU 0 runs the full preemptive scheduler (user_init, affinity 0x1,
/// runs there), so every critical post-boot service thread must be pinned here
/// until AP scheduling is implemented (separate SMP work). The boot-time SMP
/// smoketest is unaffected — it sets its own affinities and runs in the active
/// boot window, not via these post-boot spawns.
pub fn spawn_on_bsp(mut task: Task) {
    task.affinity = crate::task::CpuAffinity::from_mask(1);
    spawn(task);
}

pub fn with_current_task<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&Task) -> R,
{
    x86_64::instructions::interrupts::without_interrupts(|| {
        let cpu_id = crate::gdt::current_cpu_id();
        SCHEDULER.lock().current_task[cpu_id].as_ref().map(f)
    })
}

pub fn with_current_task_mut<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut Task) -> R,
{
    x86_64::instructions::interrupts::without_interrupts(|| {
        let cpu_id = crate::gdt::current_cpu_id();
        SCHEDULER.lock().current_task[cpu_id].as_mut().map(f)
    })
}

pub fn current_task_id() -> Option<TaskId> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let cpu_id = crate::gdt::current_cpu_id();
        SCHEDULER.lock().current_task[cpu_id].as_ref().map(|t| t.id)
    })
}

pub fn yield_task() {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let cpu_id = crate::gdt::current_cpu_id();
        if cpu_id == 0 && !BOOT_COMPLETE.load(Ordering::Relaxed) {
            return;
        }

        let mut sched = SCHEDULER.lock();
        sched.tick += 1;
        sched.game_mode.tick();
        sched.check_deadline_misses();

        let current = sched.current_task[cpu_id].take().unwrap_or_else(|| {
            sched.idle_task[cpu_id].take().unwrap_or_else(|| {
                let mut t = Task::new(idle_thread, None);
                t.is_idle = true;
                t
            })
        });

        let current_id = current.id;
        let is_idle = current.is_idle;
        let mut current = current;

        if let Some(ref mut dl) = current.deadline {
            // finish-time in the SAME wall-time µs base as wake/now (EDF clock).
            dl.finish(edf_monotonic_us(sched.tick));
        }
        if !is_idle && current.priority == TaskPriority::Normal {
            current.vruntime = current.vruntime.saturating_add(MIN_GRANULARITY_VNS);
        }

        // Bracket the scheduler decision/scan (PERFORMANCE_TARGETS §5): pick_next
        // is the runqueue + EDF earliest-deadline scan, the depth-sensitive part
        // of a context switch. rdtsc + a lock-free perf record only (no lock, no
        // scheduler reentrancy) — safe under the held SCHEDULER lock.
        let pick_t0 = unsafe { core::arch::x86_64::_rdtsc() };
        let picked = sched.pick_next(cpu_id);
        crate::perf::record_pick_ticks(
            unsafe { core::arch::x86_64::_rdtsc() }.saturating_sub(pick_t0),
        );
        let next = match picked {
            Some(n) => {
                if cpu_id < crate::gdt::MAX_CPUS {
                    PER_CPU_PICKS[cpu_id].fetch_add(1, Ordering::Relaxed);
                }
                n
            }
            None => {
                sched.current_task[cpu_id] = Some(current);
                return;
            }
        };

        if next.id == current_id {
            sched.current_task[cpu_id] = Some(next);
            return;
        }

        // FINAL rsp-sanity tripwire (MasterChecklist 4.8 — the intermittent SMP
        // steal-resume #DF). pick_next already sane-checks every candidate, so
        // on the happy path this NEVER fires. It exists to convert any racing /
        // future insane resume into a logged, recoverable abort instead of a
        // silent `mov rsp, 0` → push-to-NULL double fault: we quarantine the
        // bad task and keep `current` running this round (a missed switch is
        // harmless; a DF is not). The counter is the iron tripwire — far more
        // trustworthy than "it didn't crash this boot".
        if !next.saved_stack_is_sane() {
            SWITCH_ABORTS.fetch_add(1, Ordering::Relaxed);
            crate::serial_println!(
                "[sched] ABORT switch CPU{}: next {:?} saved rsp {:#x} insane — quarantine + keep current (rsp=0 #DF tripwire)",
                cpu_id,
                next.id,
                next.stack_ptr.as_u64()
            );
            sched.quarantine_insane_stack(next, cpu_id, "switch-guard");
            sched.current_task[cpu_id] = Some(current);
            return;
        }

        // x86_64 TLS (Linux ABI): the incoming task's FS base must be live in
        // IA32_FS_BASE before it resumes. Both fields are 0 unless a task ran
        // arch_prctl(ARCH_SET_FS) — see Task::fs_base — so native workloads
        // never pay the wrmsr.
        let old_fs = current.fs_base;
        let new_fs = next.fs_base;
        // Win32 TEB (GS base) for AthBridge guests — see Task::gs_base and
        // syscall arm 282. Restored via the ACTIVE GsBase (NOT KernelGsBase):
        // context switches run from Rust kernel code, and the value left in the
        // active GS base here is the one that survives the syscall handler's
        // swapgs pairs to become active at sysret (same parity argument as arm
        // 282). KernelGsBase must keep holding the per-CPU pointer for the next
        // syscall entry's first swapgs — writing the TEB there would destroy
        // it. While the kernel then runs with active GS = TEB (large),
        // current_cpu_id() reads the per-CPU id from the kernel-GS block, so
        // SMP stays correct. Both fields are 0 for native/Linux tasks, so the
        // wrmsr is skipped unless scheduling to/from a AthBridge guest.
        let old_gs = current.gs_base;
        let new_gs = next.gs_base;

        // Telemetry: a real context switch (next != current, sane stack) is
        // about to commit. Record it + which class won the CPU for
        // /proc/raeen/perf. Lock-free relaxed atomics; SCHEDULER is held but no
        // new lock is taken (perf has none).
        crate::perf::record_context_switch(next.priority == TaskPriority::Game);

        crate::gdt::set_rsp0(next.kernel_stack_end());
        crate::syscall::set_syscall_kernel_stack(cpu_id, next.kernel_stack_end().as_u64());
        // Slice 1.5g: source the CR3-switch token through the arch::mmu seam.
        // On x86 `user_root_token(root)` returns `root.start_address().as_u64()`
        // — the IDENTICAL PML4 phys base this asm loads into CR3 (zero behavior
        // change). aarch64 will return the TTBR0_EL1 value behind the same seam.
        // The `mov cr3, rdx` asm is UNTOUCHED; only how `new_cr3` is COMPUTED moves.
        let new_cr3 = if let Some(pml4) = next.pml4 {
            crate::arch::mmu::user_root_token(pml4)
        } else {
            crate::arch::mmu::user_root_token(*crate::memory::KERNEL_PML4.get().unwrap())
        };

        sched.current_task[cpu_id] = Some(next);
        let new_stack_ptr = sched.current_task[cpu_id]
            .as_ref()
            .unwrap()
            .stack_ptr
            .as_u64() as usize;

        let (old_ptr, old_fpu_buf) = if is_idle {
            sched.idle_task[cpu_id] = Some(current);
            let t = sched.idle_task[cpu_id].as_mut().unwrap();
            (
                &mut t.stack_ptr as *mut VirtAddr as *mut usize,
                t.fpu_state.data.as_mut_ptr(),
            )
        } else {
            sched.switch_stash[cpu_id] = Some(current);
            let t = sched.switch_stash[cpu_id].as_mut().unwrap();
            (
                &mut t.stack_ptr as *mut VirtAddr as *mut usize,
                t.fpu_state.data.as_mut_ptr(),
            )
        };
        let new_fpu_buf = sched.current_task[cpu_id]
            .as_mut()
            .unwrap()
            .fpu_state
            .data
            .as_mut_ptr();
        drop(sched);

        if new_fs != old_fs {
            x86_64::registers::model_specific::FsBase::write(VirtAddr::new(new_fs));
        }
        if new_gs != old_gs {
            // A guest task (gs_base != 0) installs its TEB; a native/Linux task
            // (gs_base == 0) restores the legacy active-GS value — the
            // small-integer cpu_id this core's gdt::init parked there — so
            // current_cpu_id()'s active-GS fast path keeps working for native
            // tasks. See restore_active_gs_base.
            restore_active_gs_base(new_gs, cpu_id);
        }

        unsafe {
            crate::context::switch_context(
                old_ptr,
                new_stack_ptr,
                new_cr3,
                old_fpu_buf,
                new_fpu_buf,
            );
        }

        crate::scheduler::finish_task_switch();
    });
}

/// Install the incoming task's user-visible GS base during a context switch.
///
/// `gs_base != 0` → a AthBridge guest's Win32 TEB (set via SYS_SET_GS_BASE).
/// `gs_base == 0` → a native/Linux task: restore the legacy small-integer
/// `cpu_id` so `gdt::current_cpu_id()`'s active-GS fast path returns the right
/// core (writing 0 unconditionally would mis-report cpu_id as 0 on every AP).
/// Writes the ACTIVE GsBase, not KernelGsBase — see the GS save/restore comment
/// in `yield_task` and syscall arm 282 for the swapgs-parity reasoning.
#[inline]
fn restore_active_gs_base(gs_base: u64, cpu_id: usize) {
    let value = if gs_base != 0 { gs_base } else { cpu_id as u64 };
    x86_64::registers::model_specific::GsBase::write(VirtAddr::new(value));
}

pub fn block_current_task_with<F: FnOnce()>(state: TaskState, f: F) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        let cpu_id = crate::gdt::current_cpu_id();

        let mut current = sched.current_task[cpu_id]
            .take()
            .expect("No current task to block!");
        current.state = state;

        let next = sched
            .pick_next(cpu_id)
            .or_else(|| sched.idle_task[cpu_id].take())
            .expect("No ready tasks and no idle task!");

        // FINAL rsp-sanity tripwire (see yield_task). The block path MUST hand
        // control to some task, so an insane `next` is quarantined and we fall
        // back to the (always-sane, never-stolen) idle task rather than resume a
        // garbage frame. Idle itself is trusted, so skip the check for it.
        let next = if next.is_idle || next.saved_stack_is_sane() {
            next
        } else {
            SWITCH_ABORTS.fetch_add(1, Ordering::Relaxed);
            crate::serial_println!(
                "[sched] ABORT block-switch CPU{}: next {:?} saved rsp {:#x} insane — quarantine + idle (rsp=0 #DF tripwire)",
                cpu_id,
                next.id,
                next.stack_ptr.as_u64()
            );
            sched.quarantine_insane_stack(next, cpu_id, "block-switch-guard");
            sched.idle_task[cpu_id]
                .take()
                .expect("block: no idle task to fall back to after quarantine")
        };

        // x86_64 TLS restore — see the yield path above for the invariant.
        let old_fs = current.fs_base;
        let new_fs = next.fs_base;
        // Win32 TEB (GS base) restore — see yield_task / restore_active_gs_base.
        let old_gs = current.gs_base;
        let new_gs = next.gs_base;

        let new_stack_ptr = next.stack_ptr.as_u64() as usize;
        crate::gdt::set_rsp0(next.kernel_stack_end());
        // CRITICAL: also update the per-CPU SYSCALL entry stack (gs:[0x08],
        // loaded by syscall_handler's `mov rsp, gs:[0x08]`). yield_task and
        // exit_current_task do this; block_current_task_with historically did
        // NOT — so a task that blocked here (e.g. in sys_wait) handed `next` a
        // STALE syscall stack pointing at a DIFFERENT task's kernel stack. The
        // moment `next` made a syscall it ran on that other task's stack →
        // two tasks sharing one kernel stack → saved switch frame clobbered →
        // resume into garbage RIP (the user_init 0xca2e / kernel-NX faults,
        // root-caused 2026-06-11). Both stacks must track `next`.
        crate::syscall::set_syscall_kernel_stack(cpu_id, next.kernel_stack_end().as_u64());

        // Slice 1.5g: source the CR3-switch token through the arch::mmu seam
        // (`user_root_token` returns the identical PML4 phys base on x86 — zero
        // behavior change; the `mov cr3, rdx` asm is UNTOUCHED).
        let new_cr3 = if let Some(pml4) = next.pml4 {
            crate::arch::mmu::user_root_token(pml4)
        } else {
            crate::arch::mmu::user_root_token(*crate::memory::KERNEL_PML4.get().unwrap())
        };

        sched.current_task[cpu_id] = Some(next);

        // SMP race fix: do NOT push to `blocked_tasks` here. Between the
        // upcoming `drop(sched)` and `switch_context`, another CPU could
        // observe a wake condition installed by `f()`, take SCHEDULER, scan
        // `blocked_tasks`, find this task, and `remove(i)` — freeing the Box
        // and dangling `old_ptr`. Instead, hand the task to the per-CPU
        // `switch_stash[cpu_id]` slot. `switch_stash` is the same transient
        // holding cell `yield_task` uses; the wake paths
        // (`unblock_receivers` / `unblock_senders` / `unblock_irq_waiters` /
        // `unblock_virtio_waiters` / `wake_thread*`) below scan this slot
        // too and mark `state → Ready` IN PLACE rather than moving the Box.
        // After `switch_context` completes and the SP is saved, the next
        // task's `finish_task_switch` drains the slot: a state that's still
        // `is_blocked()` routes the Box into `blocked_tasks`; a state that
        // was raced to `Ready` is enqueued. Either path keeps the Task heap
        // allocation stable across the SP save in `switch_context`.
        sched.switch_stash[cpu_id] = Some(current);
        let t = sched.switch_stash[cpu_id].as_mut().unwrap();
        let old_ptr = &mut t.stack_ptr as *mut VirtAddr as *mut usize;
        let old_fpu_buf = t.fpu_state.data.as_mut_ptr();
        let new_fpu_buf = sched.current_task[cpu_id]
            .as_mut()
            .unwrap()
            .fpu_state
            .data
            .as_mut_ptr();
        drop(sched);

        f();

        if new_fs != old_fs {
            x86_64::registers::model_specific::FsBase::write(VirtAddr::new(new_fs));
        }
        if new_gs != old_gs {
            restore_active_gs_base(new_gs, cpu_id);
        }

        unsafe {
            crate::context::switch_context(
                old_ptr,
                new_stack_ptr,
                new_cr3,
                old_fpu_buf,
                new_fpu_buf,
            );
        }

        crate::scheduler::finish_task_switch();
    })
}

pub fn block_current_task(state: TaskState, _stack_ptr: usize) {
    block_current_task_with(state, || {});
}

pub fn unblock_receivers(chan_id: usize) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        // Race-window catch: a sibling CPU may have just put a task
        // matching this wake into switch_stash via block_current_task_with,
        // dropped the SCHEDULER lock, and is somewhere between `f()` and
        // `switch_context`. Marking state→Ready in the stash now means
        // finish_task_switch on that CPU will route the task straight back
        // to a runqueue instead of into blocked_tasks — no lost wakeup.
        mark_stash_ready_if(&mut sched, |t| {
            t.state == TaskState::BlockedOnReceive(chan_id)
        });
        let mut i = 0;
        while i < sched.blocked_tasks.len() {
            if sched.blocked_tasks[i].state == TaskState::BlockedOnReceive(chan_id) {
                let mut task = *sched.blocked_tasks.remove(i);
                task.state = TaskState::Ready;
                sched.enqueue(task);
            } else {
                i += 1;
            }
        }
    });
}

pub fn unblock_senders(chan_id: usize) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        mark_stash_ready_if(&mut sched, |t| t.state == TaskState::BlockedOnSend(chan_id));
        let mut i = 0;
        while i < sched.blocked_tasks.len() {
            if sched.blocked_tasks[i].state == TaskState::BlockedOnSend(chan_id) {
                let mut task = *sched.blocked_tasks.remove(i);
                task.state = TaskState::Ready;
                sched.enqueue(task);
            } else {
                i += 1;
            }
        }
    });
}

pub fn unblock_irq_waiters(vector: u8) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        mark_stash_ready_if(&mut sched, |t| t.state == TaskState::BlockedOnIrq(vector));
        let mut i = 0;
        while i < sched.blocked_tasks.len() {
            if sched.blocked_tasks[i].state == TaskState::BlockedOnIrq(vector) {
                let mut task = *sched.blocked_tasks.remove(i);
                task.state = TaskState::Ready;
                sched.enqueue(task);
            } else {
                i += 1;
            }
        }
    });
}

// ── Per-CPU heartbeat counters ─────────────────────────────────────────
//
// Each LAPIC timer IRQ increments PER_CPU_TICKS[cpu_id]. If an AP's
// counter never advances, it's not getting timer IRQs — its LAPIC is
// dead, its IDT isn't installed, or it's stuck in a non-interruptible
// section. /proc/raeen/smp reports per-CPU tick rates so we can spot
// silent CPUs before they cost us throughput.

use core::sync::atomic::AtomicU64;

pub static PER_CPU_TICKS: [AtomicU64; crate::gdt::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::gdt::MAX_CPUS];

/// Tasks picked from this CPU's runqueue (incremented in yield_task when
/// pick_next returns Some). Heartbeat alone proves "IRQs arrive"; this
/// proves "the CPU does scheduler work".
pub static PER_CPU_PICKS: [AtomicU64; crate::gdt::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::gdt::MAX_CPUS];

/// Per-CPU count of tasks STOLEN from another core's runqueue (work-stealing
/// load balancing). Replaces the old per-steal `serial_println!` that flooded
/// ~30% of the boot log and slowed boot; surfaced via /proc/raeen/sched.
pub static PER_CPU_STEALS: [AtomicU64; crate::gdt::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::gdt::MAX_CPUS];

/// Times the final pre-switch tripwire refused to context-switch into a task
/// whose saved kernel RSP was insane (0 / out of its kstack). On the happy path
/// this is ALWAYS 0 — pick_next already sane-checks every candidate. A non-zero
/// value is the smoking gun for the intermittent SMP steal-resume #DF
/// (MasterChecklist 4.8) being CAUGHT instead of double-faulting: the next iron
/// boot can read it via /proc/raeen/sched_stats. Worth more than a green run.
pub static SWITCH_ABORTS: AtomicU64 = AtomicU64::new(0);

static POST_BOOT_TICKS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static POST_BOOT_DIAG_DONE: AtomicBool = AtomicBool::new(false);

/// One-shot post-boot scheduler-state dump, fired from the timer IRQ — the ONLY
/// context guaranteed to run when post-boot kernel threads don't (the timer
/// preempts whatever monopolizes the CPU). Dumps each CPU's current task + the
/// full contents of each runqueue (task id / state / affinity / last_cpu), then
/// flushes the ring to BOOTLOG.TXT so it survives off-target. This answers the
/// open question definitively: are the service threads READY in CPU 0's
/// runqueue but never picked (a pick/preempt bug), absent (spawn/enqueue
/// failed), or stuck on another CPU? `try_lock` only — never blocks the timer.
fn dump_post_boot_diag() {
    let online = core::cmp::min(
        crate::smp::ONLINE_CPUS.load(Ordering::Relaxed) as usize,
        crate::gdt::MAX_CPUS,
    );
    crate::serial_println!(
        "[sched-diag] ===== post-boot scheduler state ({} cpus online) =====",
        online
    );
    if let Some(sched) = SCHEDULER.try_lock() {
        for cpu in 0..online {
            let cur = sched.current_task[cpu].as_ref().map(|t| (t.id, t.state));
            let rq = &sched.runqueues[cpu];
            crate::serial_println!(
                "[sched-diag] cpu{} current={:?} rq: normal={} game={} deadline={}",
                cpu,
                cur,
                rq.tasks_normal.len(),
                rq.tasks_game.len(),
                rq.tasks_deadline.len(),
            );
            for t in rq.tasks_normal.iter().take(10) {
                crate::serial_println!(
                    "[sched-diag]   cpu{} normal Task {:?} state={:?} aff={:#x} last_cpu={} vrt={}",
                    cpu,
                    t.id,
                    t.state,
                    t.affinity.mask,
                    t.last_cpu,
                    t.vruntime,
                );
            }
        }
        crate::serial_println!(
            "[sched-diag] blocked_tasks={} min_vruntime={}",
            sched.blocked_tasks.len(),
            sched.min_vruntime,
        );
        drop(sched);
    } else {
        crate::serial_println!("[sched-diag] SCHEDULER try_lock FAILED (held by another CPU)");
    }
    crate::bootlog_persist::flush();
}

#[no_mangle]
pub extern "C" fn timer_handler_inner(_stack_ptr: usize) {
    let cpu = crate::gdt::current_cpu_id();
    if cpu < crate::gdt::MAX_CPUS {
        PER_CPU_TICKS[cpu].fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    // One-shot post-boot scheduler diagnostic (~a few seconds after boot
    // completes). Runs from the timer IRQ so it fires even when no post-boot
    // kernel thread is being scheduled (the bug under investigation).
    if cpu == 0 && BOOT_COMPLETE.load(Ordering::Relaxed) {
        let t = POST_BOOT_TICKS.fetch_add(1, Ordering::Relaxed);
        // ~600 ticks (a few seconds) so the dump + flush land even if the
        // machine is powered off quickly (the T1145 boot ended before the old
        // 3000-tick threshold fired).
        if t == 600 && !POST_BOOT_DIAG_DONE.swap(true, Ordering::SeqCst) {
            dump_post_boot_diag();
        }
    }
    crate::timers::tick();
    crate::linux_compat::on_timer_tick();
    crate::power::on_timer_tick();
    // EOI must be sent BEFORE yield_task. If yield_task switches to a newly
    // created task, it will NOT return here, and the LAPIC will never get the EOI.
    crate::arch::interrupt_controller::eoi();
    yield_task();
}

/// Check if there is a current task running on this CPU.
/// Used by page fault handler to guard against early-boot faults before task spawning.
pub fn has_current_task() -> bool {
    let sched = SCHEDULER.lock();
    let cpu_id = crate::gdt::current_cpu_id();
    sched.current_task[cpu_id].is_some()
}

/// Re-ready every task blocked in `sys_wait(child)`. Same stash-then-
/// blocked_tasks protocol as `unblock_receivers`: the parent may still sit
/// in a CPU's `switch_stash` mid-block (see `block_current_task_with`), in
/// which case marking it Ready in place lets `finish_task_switch` route it
/// to a runqueue; a parent already filed in `blocked_tasks` is moved to a
/// runqueue here. The woken parent re-executes sys_wait (`rcx -= 2` retry)
/// and reaps the zombie — or gets NotFound if the child was force-killed.
/// Caller holds the SCHEDULER lock. Returns the number of waiters woken.
fn wake_wait_parents(sched: &mut Scheduler, child: TaskId) -> usize {
    let mut woken = mark_stash_ready_if(sched, |t| t.state == TaskState::BlockedOnWait(child));
    let mut i = 0;
    while i < sched.blocked_tasks.len() {
        if sched.blocked_tasks[i].state == TaskState::BlockedOnWait(child) {
            let mut parent = *sched.blocked_tasks.remove(i);
            parent.state = TaskState::Ready;
            sched.enqueue(parent);
            woken += 1;
        } else {
            i += 1;
        }
    }
    woken
}

#[no_mangle]
pub extern "C" fn exit_current_task(exit_code: u64) -> ! {
    x86_64::instructions::interrupts::disable();
    let mut sched = SCHEDULER.lock();
    let cpu_id = crate::gdt::current_cpu_id();
    let mut task = sched.current_task[cpu_id]
        .take()
        .expect("exit_current_task with no current task");
    let dying_fs = task.fs_base;
    let dying_gs = task.gs_base;
    task.state = TaskState::Zombie(exit_code);
    // Release every resource held OUTSIDE Task::drop through the single shared
    // reclaim path: compositor surfaces (the window framebuffer), per-app data
    // buckets, event-bus subscriptions, SOCKET_TABLE entries (BUG-32), the
    // sandbox-table entry (Phase 9), and owned IPC channels (BUG-23). These
    // used to be split — sockets/sandbox/IPC inline here and surfaces/buckets
    // only in kill_task — which leaked sockets + the sandbox entry on the
    // force-kill path (a force-killed networked app: the "app misbehaves ->
    // kill it" desktop case). `reclaim_task_resources` now owns the FULL set so
    // both exit paths reclaim identically. Before this consolidation, a
    // self-exiting windowed app (open an app, press Esc) left the compositor
    // compositing a surface backed by the about-to-be-freed user pages.
    //
    // `reclaim_task_resources` -> `cleanup_task_surfaces` removes the Surface
    // from the compositor's Vec; the ~1.5 MiB framebuffer frames themselves are
    // now returned to the buddy allocator by `Surface::drop` (compositor.rs) when
    // that Vec entry is dropped — `cleanup_task_surfaces` alone used to leak them
    // (no compositor free path existed). Safe here: SCHEDULER held + IF=0 +
    // single scheduling CPU, so the compositor thread is not running concurrently
    // (same context as kill_task, which already does the SCHEDULER->COMPOSITOR
    // acquire).
    reclaim_task_resources(task.id);

    // CRITICAL: do NOT file the zombie into `blocked_tasks` here, and do NOT
    // drop(task). We are still executing on this task's kernel stack:
    //  * drop(task) → Task::drop → free_kernel_stack unmaps the pages under
    //    our feet → #PF → #DF (the post-boot cpu=0 #DF when smp_worker_N
    //    exits).
    //  * filing into blocked_tasks + waking the parent NOW lets the parent's
    //    sys_wait retry reap on a sibling CPU the moment we drop the lock —
    //    `try_wait_task` drops the Box, freeing this stack mid-exit. Observed
    //    2026-06-10 as non-canonical VirtAddr panics on string bytes
    //    (0x6f52_6fe5_7473_7953) when user_init reaped raebridge_host.
    //
    // Instead, ALWAYS hand the dying task to switch_stash[cpu_id]. After
    // switch_context flips to a DIFFERENT task's stack, finish_task_switch
    // on that stack files a parented zombie into blocked_tasks and wakes the
    // waiting parent (or frees an orphan) — by then nothing executes on the
    // dead stack.
    sched.switch_stash[cpu_id] = Some(task);

    let next = sched
        .pick_next(cpu_id)
        .or_else(|| sched.idle_task[cpu_id].take())
        .expect("no next task");
    let new_stack_ptr = next.stack_ptr.as_u64() as usize;
    // Slice 1.5g: source the CR3-switch token through the arch::mmu seam
    // (`user_root_token` returns the identical PML4 phys base on x86 — zero
    // behavior change; the `mov cr3, rdx` asm is UNTOUCHED).
    let new_cr3 = if let Some(pml4) = next.pml4 {
        crate::arch::mmu::user_root_token(pml4)
    } else {
        crate::arch::mmu::user_root_token(*crate::memory::KERNEL_PML4.get().unwrap())
    };
    crate::gdt::set_rsp0(next.kernel_stack_end());
    crate::syscall::set_syscall_kernel_stack(cpu_id, next.kernel_stack_end().as_u64());
    // x86_64 TLS: restore the incoming task's FS base, field-vs-field
    // conditional like the yield/block paths. NEVER write unconditionally:
    // Task::fs_base is inherited from the live MSR at creation, so an
    // unconditional write of a "default" would clobber pre-TLS state that
    // userspace depends on (the raebridge child stall, 2026-06-10).
    let new_fs = next.fs_base;
    let new_gs = next.gs_base;
    sched.current_task[cpu_id] = Some(next);
    let new_fpu_buf = sched.current_task[cpu_id]
        .as_mut()
        .unwrap()
        .fpu_state
        .data
        .as_mut_ptr();
    drop(sched);
    if new_fs != dying_fs {
        x86_64::registers::model_specific::FsBase::write(VirtAddr::new(new_fs));
    }
    // Win32 TEB (GS base) restore for the incoming task — see
    // restore_active_gs_base. dying_gs is the exiting task's field.
    if new_gs != dying_gs {
        restore_active_gs_base(new_gs, cpu_id);
    }
    unsafe {
        crate::context::switch_context(
            core::ptr::null_mut(),
            new_stack_ptr,
            new_cr3,
            core::ptr::null_mut(),
            new_fpu_buf,
        );
    }
    unreachable!();
}

/// Free corpses queued in `dead_tasks`. Called from the idle thread on the
/// idle stack with no scheduler lock held: each `Box<Task>` is moved OUT under
/// a brief lock, then dropped (kernel-stack unmap + page-table teardown +
/// frame dealloc) with the lock released and IRQs on. By the time the idle
/// thread runs, every CPU has switched away from the dead task, so nothing
/// references its stack or page tables — the free is finally safe.
pub fn drain_dead_tasks() {
    loop {
        let corpse = x86_64::instructions::interrupts::without_interrupts(|| {
            SCHEDULER.lock().dead_tasks.pop()
        });
        match corpse {
            Some(task) => drop(task), // Task::drop frees stack + page tables here
            None => break,
        }
    }
}

extern "C" fn idle_thread() {
    x86_64::instructions::interrupts::enable();
    loop {
        drain_dead_tasks();
        x86_64::instructions::hlt();
    }
}

// ── Boot-time SMP smoketest ────────────────────────────────────────────
//
// Spawns 4 short-lived kernel threads, each pinned to a different CPU
// via affinity. Each worker increments a per-CPU counter, then exits.
// Combined with PER_CPU_PICKS, this proves end-to-end:
//   1. Scheduler accepts the task and runqueue placement honors affinity
//   2. The pinned AP's timer IRQ triggers yield_task
//   3. yield_task picks the task and context-switches into it
//   4. The kernel thread runs to completion and exits cleanly

use core::sync::atomic::AtomicUsize;

pub static SMP_WORKERS_RAN: [AtomicUsize; 4] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];

extern "C" fn smp_worker_0() {
    smp_worker(0)
}
extern "C" fn smp_worker_1() {
    smp_worker(1)
}
extern "C" fn smp_worker_2() {
    smp_worker(2)
}
extern "C" fn smp_worker_3() {
    smp_worker(3)
}

fn smp_worker(slot: usize) {
    let cpu = crate::gdt::current_cpu_id();
    SMP_WORKERS_RAN[slot].store(cpu.saturating_add(1), Ordering::SeqCst);
    // Burn a few cycles so the task is visibly "in-flight" if anyone
    // catches it mid-execution.
    for _ in 0..10_000 {
        core::hint::spin_loop();
    }
    crate::serial_println!("[sched] smp_worker slot={} ran on cpu={}", slot, cpu);
}

// ── Work-steal probe workers (MasterChecklist 4.8) ────────────────────
//
// Unpinned twins of the smp_workers above, run ONLY via a cross-CPU steal
// (see the steal probe in run_boot_smoketest). Separate counters so the
// affinity-pinned test and the steal test can't mask each other.

pub static STEAL_WORKERS_RAN: [AtomicUsize; 4] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];

extern "C" fn steal_worker_0() {
    steal_worker(0)
}
extern "C" fn steal_worker_1() {
    steal_worker(1)
}
extern "C" fn steal_worker_2() {
    steal_worker(2)
}
extern "C" fn steal_worker_3() {
    steal_worker(3)
}

fn steal_worker(slot: usize) {
    let cpu = crate::gdt::current_cpu_id();
    STEAL_WORKERS_RAN[slot].store(cpu.saturating_add(1), Ordering::SeqCst);
    for _ in 0..10_000 {
        core::hint::spin_loop();
    }
}

pub fn run_boot_smoketest() {
    let entries: [extern "C" fn(); 4] = [smp_worker_0, smp_worker_1, smp_worker_2, smp_worker_3];
    for (slot, &entry) in entries.iter().enumerate() {
        let mut t = crate::task::Task::new(entry, None);
        // Pin slot N to CPU N so we can see distribution.
        t.affinity = crate::task::CpuAffinity::from_mask(1u64 << slot);
        spawn(t);
    }
    // Give the AP workers a chance to run (their timer-IRQ yield picks them
    // up). The BSP's pinned worker (slot 0) CANNOT run here: yield_task is a
    // deliberate no-op on cpu 0 until BOOT_COMPLETE so the boot sequence is
    // never preempted (scheduler.rs yield_task gate). Slot 0 runs at the
    // first post-BOOT_COMPLETE tick and prints its own proof line then —
    // the old summary counted it as a failure ("3/4") which was a false
    // alarm by construction. Expect 3 pre-boot workers, report slot 0 as
    // deferred.
    for _ in 0..200 {
        let aps_ran = (1..4).all(|s| SMP_WORKERS_RAN[s].load(Ordering::SeqCst) > 0);
        if aps_ran {
            break;
        }
        for _ in 0..10_000 {
            core::hint::spin_loop();
        }
    }

    let mut ran_cpus = [false; crate::gdt::MAX_CPUS];
    let mut completed = 0;
    for slot in 0..4 {
        let v = SMP_WORKERS_RAN[slot].load(Ordering::SeqCst);
        if v > 0 {
            completed += 1;
            let cpu = v - 1;
            if cpu < ran_cpus.len() {
                ran_cpus[cpu] = true;
            }
        }
    }
    let distinct_cpus = ran_cpus.iter().filter(|x| **x).count();
    let slot0_deferred = SMP_WORKERS_RAN[0].load(Ordering::SeqCst) == 0;
    // Workers are pinned to CPU == slot. Slot 0 (BSP) is deferred until
    // BOOT_COMPLETE (yield_task gate); slots >= online_cpus have no CPU to run
    // on. The meaningful proof is "every AP worker that COULD run during boot,
    // did" — expected = online_cpus-1, capped at the 3 AP slots. The old
    // `completed >= 3` hard-coded a 4-CPU box and spuriously FAILed at -smp 1/2.
    let online = crate::smp::ONLINE_CPUS.load(Ordering::Relaxed).max(1) as usize;
    let expected_ap = online.saturating_sub(1).min(3);
    let ap_ran = (1..4)
        .filter(|&s| SMP_WORKERS_RAN[s].load(Ordering::SeqCst) > 0)
        .count();
    let verdict = if ap_ran >= expected_ap {
        "PASS"
    } else {
        "FAIL"
    };
    crate::serial_println!(
        "[sched] smp smoketest: {}/4 workers ran across {} distinct CPU(s){} -> {}",
        completed,
        distinct_cpus,
        if slot0_deferred {
            " (slot 0 deferred until BOOT_COMPLETE unblocks BSP scheduling)"
        } else {
            ""
        },
        verdict,
    );

    // ── Regression probe: a stale wake must not resurrect a dead task ──
    // (MasterChecklist 4.8 — the intermittent smp>=4 duplicate-task #DF.)
    // Manufacture the hazard deterministically instead of waiting for the
    // race: park a synthetic Zombie in blocked_tasks (exactly where
    // exit_current_task leaves a killed task whose parent is alive), fire
    // every wake-by-id path at its id, then verify the corpse was neither
    // woken nor enqueued and is still reapable by wait(). Pre-guard kernels
    // enqueue the corpse here (reaped=false enqueued=true -> FAIL) and would
    // later switch_context into its stale SP — the #DF.
    let corpse_id = x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        let mut corpse = Task::new(idle_thread, None);
        corpse.state = TaskState::Zombie(42);
        let id = corpse.id;
        sched.blocked_tasks.push(alloc::boxed::Box::new(corpse));
        id
    });
    wake_thread(corpse_id);
    wake_thread_direct(corpse_id);
    unblock_futex_waiter(corpse_id);
    let reaped = matches!(try_wait_task(corpse_id), WaitResult::Reaped(42));
    let enqueued = x86_64::instructions::interrupts::without_interrupts(|| {
        let sched = SCHEDULER.lock();
        sched.runqueues.iter().any(|rq| {
            rq.tasks_normal.iter().any(|t| t.id == corpse_id)
                || rq.tasks_game.iter().any(|t| t.id == corpse_id)
                || rq.tasks_deadline.iter().any(|t| t.id == corpse_id)
        })
    });
    crate::serial_println!(
        "[sched] zombie-wake guard: reaped={} enqueued={} -> {}",
        reaped,
        enqueued,
        if reaped && !enqueued { "PASS" } else { "FAIL" },
    );

    // ── SCHED_BODY deadline telemetry (Concept "Gaming isn't a mode") ──
    // Two-part proof the deadline-miss telemetry is REAL (CLAUDE §1 North
    // Star table: "EDF exists; deadline-miss telemetry missing").
    //
    // Part 1 — pure accounting logic: drive a synthetic DeadlineTask through
    // the exact wake/miss path pick_next + check_deadline_misses use, and
    // verify the per-task counters + the /proc/raeen/gaming miss-rate formula.
    let logic_ok = {
        use crate::task::DeadlineTask;
        let mut dl = DeadlineTask::new(1000, 1000, 500);
        // Period 1: wake (non-zero — check_miss ignores last_wake==0), never
        // finish, let the deadline pass -> a real miss.
        dl.wake(1000); // invocation 1, absolute_deadline = 2000
        let missed1 = dl.check_miss(3000); // now > deadline, unfinished -> miss
                                           // Period 2: wake, finish in time -> NOT a miss.
        dl.wake(2000); // invocation 2, absolute_deadline = 3000
        dl.finish(2200); // last_finish >= last_wake
        let missed2 = dl.check_miss(3500); // finished this period -> no miss
        let rate_x10000 = dl
            .deadline_misses
            .saturating_mul(10_000)
            .checked_div(dl.total_invocations)
            .unwrap_or(0);
        // 1 miss / 2 invocations = 5000 (=> 50.00%). A zero-denominator guard
        // must yield 0, never a divide fault.
        let zero_den = 5u64.saturating_mul(10_000).checked_div(0).unwrap_or(0);
        missed1
            && !missed2
            && dl.total_invocations == 2
            && dl.deadline_misses == 1
            && rate_x10000 == 5000
            && zero_den == 0
    };

    // Part 2 — live wiring: the global aggregate read by every telemetry
    // surface must actually advance when a SCHED_BODY deadline task is
    // spawned via the real `spawn` path. Pre-fix `total_invocations`/
    // `total_tasks` were declared but never incremented, so the aggregate sat
    // at 0 forever (the bug). We spawn a probe to exercise that path, then
    // immediately retire it: a deadline task that actually runs + exits inside
    // the boot-completion window perturbs scheduling, so we pull it back out of
    // the deadline runqueue and park it in the dead-task graveyard (freed by
    // the idle thread later — never inline under the lock; see `dead_tasks`).
    let before = deadline_stats();
    let mut probe = Task::new(deadline_probe_worker, None);
    probe.priority = TaskPriority::Game;
    probe.deadline = Some(crate::task::DeadlineTask::new(8_000, 8_000, 1_000));
    let probe_id = probe.id;
    spawn(probe);
    let after = deadline_stats();
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        for cpu in 0..sched.runqueues.len() {
            if let Some(pos) = sched.runqueues[cpu]
                .tasks_deadline
                .iter()
                .position(|t| t.id == probe_id)
            {
                if let Some(t) = sched.runqueues[cpu].tasks_deadline.remove(pos) {
                    sched.dead_tasks.push(alloc::boxed::Box::new(t));
                }
                break;
            }
        }
    });
    let wiring_ok = after.total_tasks > before.total_tasks
        && after.total_invocations > before.total_invocations;

    crate::serial_println!(
        "[sched] deadline telemetry: logic={} wiring(tasks {}->{}, invocations {}->{})={} -> {}",
        logic_ok,
        before.total_tasks,
        after.total_tasks,
        before.total_invocations,
        after.total_invocations,
        wiring_ok,
        if logic_ok && wiring_ok {
            "PASS"
        } else {
            "FAIL"
        },
    );

    // ── rsp=0 #DF tripwire invariant (MasterChecklist 4.8) ──
    // The whole defense rests on `saved_stack_is_sane()` rejecting an insane
    // saved RSP. Prove it: a freshly-built kernel task has a sane in-kstack RSP;
    // rsp=0 (the steal-resume #DF signature) and a low out-of-stack RSP are both
    // rejected; the real value is sane again. The throwaway task is dropped here
    // (never scheduled → Task::drop just frees its kstack). FAIL-able.
    let mut probe2 = Task::new(deadline_probe_worker, None);
    let real_sp = probe2.stack_ptr;
    let sane_real = probe2.saved_stack_is_sane();
    probe2.stack_ptr = VirtAddr::new(0);
    let reject_zero = !probe2.saved_stack_is_sane();
    probe2.stack_ptr = VirtAddr::new(0x1000);
    let reject_low = !probe2.saved_stack_is_sane();
    probe2.stack_ptr = real_sp;
    let sane_again = probe2.saved_stack_is_sane();
    let aborts = SWITCH_ABORTS.load(Ordering::Relaxed);
    drop(probe2);
    let stack_guard_ok = sane_real && reject_zero && reject_low && sane_again;
    crate::serial_println!(
        "[sched] stack-sanity guard: sane_real={} reject_rsp0={} reject_low={} sane_again={} live_switch_aborts={} -> {}",
        sane_real,
        reject_zero,
        reject_low,
        sane_again,
        aborts,
        if stack_guard_ok { "PASS" } else { "FAIL" },
    );

    // ── Work-steal probe (MasterChecklist 4.8 — prove the STEAL path) ──
    // Concept §"Fast is a feature": an idle core must pick up waiting work.
    // The affinity-pinned workers above never exercise cross-CPU stealing
    // (pinning excludes the steal filter by construction), so a green boot
    // with stealing enabled proved nothing about the once-#DF-prone path.
    // Manufacture a deterministic steal instead: home 4 UNPINNED workers to
    // CPU 0's normal queue (`last_cpu = 0`; select_cpu honors the home).
    // CPU 0 cannot run them here — yield_task is a deliberate no-op on the
    // BSP until BOOT_COMPLETE — so the ONLY way they run before the marker
    // is an idle AP's pick_next stealing them cross-CPU. Asserts the whole
    // chain end to end: steal filter accepts a legitimate cold candidate →
    // cross-CPU context switch resumes it cleanly → worker completes →
    // zero SWITCH_ABORTS. FAIL-able three ways: workers unstolen (timeout),
    // workers ran but only on the BSP (steal never happened), or any abort.
    let online = crate::smp::ONLINE_CPUS.load(Ordering::Relaxed).max(1) as usize;
    let stealing_on = WORK_STEALING_ENABLED.load(Ordering::Relaxed);
    if online < 2 || !stealing_on {
        crate::serial_println!(
            "[sched] steal probe: skipped (online={} stealing_enabled={}) — no second CPU to steal to -> PASS",
            online,
            stealing_on,
        );
    } else {
        let steals_before: u64 = PER_CPU_STEALS
            .iter()
            .map(|c| c.load(Ordering::Relaxed))
            .sum();
        let aborts_before = SWITCH_ABORTS.load(Ordering::Relaxed);
        let entries: [extern "C" fn(); 4] = [
            steal_worker_0,
            steal_worker_1,
            steal_worker_2,
            steal_worker_3,
        ];
        for &entry in entries.iter() {
            let mut t = crate::task::Task::new(entry, None);
            t.affinity = crate::task::CpuAffinity::from_mask(u64::MAX);
            t.last_cpu = 0; // home = CPU 0's queue; only a steal can run it now
            spawn(t);
        }
        // Bounded wait: each AP tick (or each worker-exit pick_next) steals
        // one; generous headroom, early-out on completion, FAIL on timeout.
        for _ in 0..2_000 {
            if (0..4).all(|s| STEAL_WORKERS_RAN[s].load(Ordering::SeqCst) > 0) {
                break;
            }
            for _ in 0..10_000 {
                core::hint::spin_loop();
            }
        }
        let ran = (0..4)
            .filter(|&s| STEAL_WORKERS_RAN[s].load(Ordering::SeqCst) > 0)
            .count();
        // stored value is cpu+1, so >1 means it ran on an AP (a real steal).
        let off_bsp = (0..4)
            .filter(|&s| STEAL_WORKERS_RAN[s].load(Ordering::SeqCst) > 1)
            .count();
        let steals_after: u64 = PER_CPU_STEALS
            .iter()
            .map(|c| c.load(Ordering::Relaxed))
            .sum();
        let stolen = steals_after.saturating_sub(steals_before);
        let aborts_delta = SWITCH_ABORTS
            .load(Ordering::Relaxed)
            .saturating_sub(aborts_before);
        let steal_ok = ran == 4 && off_bsp >= 1 && stolen >= 1 && aborts_delta == 0;
        crate::serial_println!(
            "[sched] steal probe: workers={}/4 off_bsp={} stolen={} aborts_delta={} -> {}",
            ran,
            off_bsp,
            stolen,
            aborts_delta,
            if steal_ok { "PASS" } else { "FAIL" },
        );
    }
}

/// Kill-reclaim leak guard (goal #6: zero leaks). FAIL-able R10 proof for the
/// CONFIRMED HIGH leak the reviewer localized: `kill_task`'s not-current
/// branches reclaimed only surfaces/buckets/event-bus and NEVER swept the
/// task's sockets or its sandbox entry, so a force-killed networked/sandboxed
/// app (the "app misbehaves -> kill it" desktop case) leaked its SOCKET_TABLE
/// entries + sandbox-table entry forever — same class as BUG-32, on the other
/// exit path. The fix folds those sweeps into `reclaim_task_resources`, the
/// single path BOTH `exit_current_task` and every `kill_task` branch share.
///
/// Proof: plant a sandbox entry and — if the net stack is up — a socket for a
/// synthetic pid, confirm PRESENT, run `reclaim_task_resources` (exactly what
/// every kill_task branch now calls), then assert both return to baseline. A
/// pre-fix kernel leaves the entries -> after > baseline -> FAIL.
///
/// Runs AFTER `sandbox::init()` (the sandbox TABLE is `None`/no-op before that,
/// which would falsely fail the plant) — hence a separate entry point from
/// `run_boot_smoketest`, called from `kernel_main` once sandbox is online.
pub fn run_kill_reclaim_smoketest() {
    let probe_pid = u64::MAX - 7; // synthetic, never a live task id
    let probe_tid = TaskId::from_raw(probe_pid);

    // Sandbox: baseline = no entry (level_of -> Trusted). Plant Strict, confirm
    // present, reclaim, confirm gone again.
    let sandbox_baseline_present =
        crate::sandbox::level_of(probe_pid) != crate::sandbox::SandboxLevel::Trusted;
    crate::sandbox::set_task_level(probe_pid, crate::sandbox::SandboxLevel::Strict);
    let sandbox_planted =
        crate::sandbox::level_of(probe_pid) == crate::sandbox::SandboxLevel::Strict;

    // Sockets: plant only if the net stack is initialized (headless CI may have
    // no NET_STACK -> sys_net_socket returns u64::MAX; treat as "not exercised"
    // rather than a false FAIL). UDP (proto=1). Presence probed via the per-pid
    // SOCKET_TABLE lookup inside sys_net_status (u64::MAX == absent).
    let sock_fd = crate::net::sys_net_socket(1, probe_pid);
    let sock_exercised = sock_fd != u64::MAX;
    let sock_planted = sock_exercised && crate::net::sys_net_status(sock_fd, probe_pid) != u64::MAX;

    // Reclaim — the exact call every kill_task branch performs for a not-current
    // force-kill. Must clear sandbox + socket entries (and is idempotent).
    reclaim_task_resources(probe_tid);

    let sandbox_after_present =
        crate::sandbox::level_of(probe_pid) != crate::sandbox::SandboxLevel::Trusted;
    let sock_after_present =
        sock_exercised && crate::net::sys_net_status(sock_fd, probe_pid) != u64::MAX;

    // PASS conditions: baseline had no leak, the plant took, and reclaim cleared
    // it. The socket leg only gates the verdict when actually exercised.
    let sandbox_ok = !sandbox_baseline_present && sandbox_planted && !sandbox_after_present;
    let sock_ok = if sock_exercised {
        sock_planted && !sock_after_present
    } else {
        true
    };
    let kill_reclaim_ok = sandbox_ok && sock_ok;
    // Report counts in baseline/after form (entries for the probe pid: 0 or 1).
    let (sock_base, sock_aft) = if sock_exercised {
        (0u32, if sock_after_present { 1 } else { 0 })
    } else {
        (0u32, 0u32)
    };
    let sandbox_base = if sandbox_baseline_present { 1u32 } else { 0 };
    let sandbox_aft = if sandbox_after_present { 1u32 } else { 0 };
    crate::serial_println!(
        "[sched] kill-reclaim smoketest: sockets baseline={} after={}{} sandbox baseline={} after={} -> {}",
        sock_base,
        sock_aft,
        if sock_exercised { "" } else { " (net stack down; not exercised)" },
        sandbox_base,
        sandbox_aft,
        if kill_reclaim_ok { "PASS" } else { "FAIL" },
    );
}

/// Smoketest probe: a trivial SCHED_BODY deadline task. It exists only so
/// `spawn` runs the deadline-accounting path (which bumps the global aggregate);
/// once BOOT_COMPLETE unblocks BSP scheduling it gets a slice, runs, and exits.
extern "C" fn deadline_probe_worker() {
    for _ in 0..1_000 {
        core::hint::spin_loop();
    }
}

pub fn init() {
    let mut sched = SCHEDULER.lock();
    for i in 0..crate::gdt::MAX_CPUS {
        let mut idle = Task::new(idle_thread, None);
        idle.is_idle = true;
        sched.idle_task[i] = Some(idle);
    }

    // BSP (CPU 0) is running the boot sequence. It never calls smp::ap_init().
    // We must manually assign its current_task so the first yield_task sees
    // is_idle=true and correctly saves the boot sequence into idle_task[0]
    // instead of stashing it and leaving idle_task[0] as None.
    let idle0 = sched.idle_task[0].take().unwrap();
    sched.current_task[0] = Some(idle0);

    crate::serial_println!("[ OK ] Scheduler initialized (Per-CPU runqueues + Work Stealing)");
}

pub fn list_task_summaries() -> Vec<TaskSummary> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let sched = SCHEDULER.lock();
        let mut out = Vec::new();
        for t in sched.current_task.iter().flatten() {
            out.push(TaskSummary::from_task(t));
        }
        for rq in &sched.runqueues {
            for t in &rq.tasks_deadline {
                out.push(TaskSummary::from_task(t));
            }
            for t in &rq.tasks_game {
                out.push(TaskSummary::from_task(t));
            }
            for t in &rq.tasks_normal {
                out.push(TaskSummary::from_task(t));
            }
        }
        for t in &sched.blocked_tasks {
            out.push(TaskSummary::from_task(t));
        }
        out
    })
}

#[no_mangle]
pub extern "C" fn finish_task_switch() {
    let cpu_id = crate::gdt::current_cpu_id();
    let mut sched = SCHEDULER.lock();
    if let Some(task) = sched.switch_stash[cpu_id].take() {
        // Route on the stashed task's state at drain time.
        // `yield_task` stashes a `Ready` task → enqueue.
        // `block_current_task_with` stashes a `BlockedOn*` task: if no
        // wake raced our block window it lands in `blocked_tasks` (where
        // `unblock_*` will find it); if a wake raced (sibling CPU's
        // `unblock_*` / `wake_thread*` flipped state to Ready in place via
        // `mark_stash_ready_if`), it goes back to a runqueue — no lost
        // wakeup. This closes the SMP race where pushing to
        // `blocked_tasks` BEFORE `switch_context` could dangle `old_ptr`.
        // `exit_current_task` stashes a `Zombie` task whose kernel stack the
        // dying CPU was still executing on. We're now on a DIFFERENT task's
        // stack, so it's finally safe to act on it: a zombie with a live
        // parent is filed into `blocked_tasks` for the parent's sys_wait to
        // reap, and the parent (blocked as BlockedOnWait) is woken; an
        // orphan is dropped — Task::drop frees the dead kernel stack.
        if matches!(task.state, TaskState::Zombie(_)) {
            let parent_alive = if let Some(pid) = task.parent_id {
                sched.current_task.iter().flatten().any(|t| t.id == pid)
                    || sched.blocked_tasks.iter().any(|t| t.id == pid)
                    || sched
                        .switch_stash
                        .iter()
                        .flatten()
                        .any(|t| t.id == pid && !matches!(t.state, TaskState::Zombie(_)))
                    || sched.runqueues.iter().any(|rq| {
                        rq.tasks_deadline.iter().any(|t| t.id == pid)
                            || rq.tasks_game.iter().any(|t| t.id == pid)
                            || rq.tasks_normal.iter().any(|t| t.id == pid)
                    })
            } else {
                false
            };
            if parent_alive {
                let child_id = task.id;
                sched.blocked_tasks.push(alloc::boxed::Box::new(task));
                let woken = wake_wait_parents(&mut sched, child_id);
                crate::serial_println!(
                    "[sched] reap-file: zombie {:?} filed, {} waiter(s) woken",
                    child_id,
                    woken
                );
            } else {
                // Orphan: defer the free to the idle thread (see `dead_tasks`).
                // Dropping here would unmap/free frames while we run on another
                // task's stack under the SCHEDULER lock, and the freed frames
                // could be reused before all references drain.
                let id = task.id;
                sched.dead_tasks.push(alloc::boxed::Box::new(task));
                crate::serial_println!("[sched] reap-file: zombie {:?} orphaned, queued", id);
            }
        } else if task.state.is_blocked() {
            sched.blocked_tasks.push(alloc::boxed::Box::new(task));
        } else {
            sched.enqueue(task);
        }
    }
}

/// Walk `switch_stash[*]` looking for an in-transit blocked task that
/// matches `predicate`. If found, set its state to `Ready` IN PLACE — the
/// Box stays in the stash slot so the CPU that owns the slot can still
/// complete its `switch_context` save. `finish_task_switch` on that CPU
/// will then route the now-Ready task into a runqueue instead of
/// `blocked_tasks`. Returns the number of slots whose state was rewritten.
///
/// Caller must hold `SCHEDULER`.
fn mark_stash_ready_if<F: Fn(&Task) -> bool>(sched: &mut Scheduler, predicate: F) -> usize {
    let mut hits = 0usize;
    for slot in sched.switch_stash.iter_mut() {
        if let Some(task) = slot.as_mut() {
            if task.state.is_blocked() && predicate(task) {
                task.state = TaskState::Ready;
                hits += 1;
            }
        }
    }
    hits
}

#[derive(Clone, Copy)]
pub struct TaskSummary {
    pub id: u64,
    pub state: u8,
    pub priority: u8,
    pub vruntime: u64,
}
impl TaskSummary {
    fn from_task(t: &Task) -> Self {
        Self {
            id: t.id.raw(),
            state: match t.state {
                TaskState::Ready => 1,
                TaskState::Zombie(_) => 3,
                _ => 2,
            },
            priority: if t.priority == TaskPriority::Game {
                1
            } else {
                0
            },
            vruntime: t.vruntime,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitResult {
    Reaped(u64),
    NotFound,
    Blocked,
}

pub fn spawn_elf_task(elf_data: &[u8], parent_id: Option<TaskId>) -> Result<TaskId, &'static str> {
    let task = Task::new_elf(elf_data, parent_id)?;
    let id = task.id;
    spawn(task);
    Ok(id)
}

pub fn spawn_elf_task_with_pty(
    elf_data: &[u8],
    parent_id: Option<TaskId>,
    pty_id: Option<u32>,
) -> Result<TaskId, &'static str> {
    let task = Task::new_elf_with_pty(elf_data, parent_id, pty_id)?;
    let id = task.id;
    spawn(task);
    Ok(id)
}

pub fn with_task_by_id<F, R>(task_id: TaskId, mut f: F) -> Option<R>
where
    F: FnMut(&mut Task) -> R,
{
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        for t in sched.current_task.iter_mut().flatten() {
            if t.id == task_id {
                return Some(f(t));
            }
        }
        // A sibling CPU may have selected the freshly spawned task while the
        // spawner is delegating its capabilities. During the context-switch
        // handoff the task lives in switch_stash, not current/runqueue/blocked;
        // omitting this transient owner made first-party driver authority
        // seeding race and intermittently leave amdgpud unable to claim PCI.
        for t in sched.switch_stash.iter_mut().flatten() {
            if t.id == task_id {
                return Some(f(t));
            }
        }
        for rq in sched.runqueues.iter_mut() {
            for t in rq.tasks_deadline.iter_mut() {
                if t.id == task_id {
                    return Some(f(t));
                }
            }
            for t in rq.tasks_game.iter_mut() {
                if t.id == task_id {
                    return Some(f(t));
                }
            }
            for t in rq.tasks_normal.iter_mut() {
                if t.id == task_id {
                    return Some(f(t));
                }
            }
        }
        for t in sched.blocked_tasks.iter_mut() {
            if t.id == task_id {
                return Some(f(t));
            }
        }
        None
    })
}

pub fn set_priority(task_id: TaskId, prio: TaskPriority) -> Result<(), ()> {
    with_task_by_id(task_id, |t| t.priority = prio)
        .map(|_| ())
        .ok_or(())
}

/// Release the resources a finished task holds OUTSIDE of `Task::drop`:
/// compositor surfaces (the window framebuffer), per-app data buckets,
/// event-bus subscriptions, global SOCKET_TABLE entries (BUG-32), the
/// sandbox-table entry (Phase 9), and the IPC channels it owned (BUG-23).
/// `Task::drop` only frees the kernel stack and page tables, so everything
/// else MUST be released here — this is the SINGLE reclaim path shared by BOTH
/// exits (`exit_current_task` self-exit and `kill_task` force-kill), so a
/// force-killed networked/sandboxed app (the "app misbehaves -> kill it"
/// desktop case) no longer leaks its sockets/ports + sandbox entry the way it
/// did when only the self-exit path swept them.
///
/// Lock safety (CLAUDE.md pitfall #6 + the IF=0/COMPOSITOR rule): this runs
/// with the SCHEDULER lock held and IF=0. Each foldee is self-contained — it
/// only removes table entries and never re-enters the scheduler (no
/// with_current_task / yield / block) nor acquires a lock that is ever held
/// while waiting on SCHEDULER:
///   * `sandbox::forget_task`        — locks only SANDBOX TABLE + GRANTS.
///   * `net::cleanup_task_sockets`   — locks only SOCKET_TABLE + NET_STACK
///                                     (net.rs never touches SCHEDULER; see its
///                                     own docstring).
///   * `ipc cleanup_task_channels`   — operates under IPC.lock(); destroy is
///                                     idempotent by id and only frees frames.
///   * `secure_ipc::cleanup_task`    — operates under SECURE_IPC.lock(); pure
///                                     table edits, never calls verify_cap /
///                                     re-enters SCHEDULER (so the reverse
///                                     SECURE_IPC->SCHEDULER edge is not walked).
/// `exit_current_task` already exercised this exact `SCHEDULER -> {SANDBOX,
/// SOCKET_TABLE, NET_STACK, IPC, memory}` order before this fold, so it is a
/// proven-safe acquisition order.
fn reclaim_task_resources(task_id: TaskId) {
    crate::compositor::cleanup_task_surfaces(task_id);
    crate::data_buckets::on_task_exit(task_id);
    crate::event_bus::cleanup_task(task_id);
    // Phase 9: drop any sandbox-table entry for the finished task.
    crate::sandbox::forget_task(task_id.raw());
    // BUG-32: sweep any sockets the task left in the global SOCKET_TABLE
    // (otherwise an unbounded socket/port leak per finished networked process).
    crate::net::cleanup_task_sockets(task_id.raw());
    // BUG-23: reclaim any IPC channels the task owned (buffer + shared frame).
    crate::ipc::IPC.lock().cleanup_task_channels(task_id.raw());
    // SEV-2 leak fix: reclaim the task's SECURE_IPC resources (secure channels +
    // their message queues, namespace names, broadcast subs/pubs) and clear any
    // dangling owner_a/owner_b so a reused TaskId can't be handed a stale
    // endpoint. SECURE_IPC-only table edits; never re-enters the scheduler, so
    // it's safe under the SCHEDULER lock / IF=0 just like the siblings above.
    crate::secure_ipc::cleanup_task(task_id);
    // Audio (SYS_AUDIO_SUBMIT 267): drop the task's per-PID Pcm mixer voice so a
    // finished app's source doesn't linger (bounded by MAX_PCM_SOURCES, but free
    // it promptly — same reclaim discipline as the sockets/sandbox sweep above).
    crate::audio::remove_task_sources(task_id.raw());
    // Screen capture (SYS_CAPTURE_START 274): drop any compositor capture
    // session the task owned so a crashed/exited capturer (screenshot tool,
    // Game Bar) doesn't leak a session — same reclaim discipline as the
    // socket/audio-voice sweep above.
    crate::compositor::cleanup_task_captures(task_id.raw());
    crate::gpu_render::cleanup_task(task_id.raw());
}

pub fn kill_task(task_id: TaskId) -> Result<(), ()> {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        let cpu_id = crate::gdt::current_cpu_id();
        let mut kill_current = false;

        for (i, current_opt) in sched.current_task.iter_mut().enumerate() {
            if let Some(current) = current_opt {
                if current.id == task_id {
                    current.state = TaskState::Zombie(0);
                    reclaim_task_resources(task_id);
                    if i == cpu_id {
                        kill_current = true;
                    }
                    if !kill_current {
                        wake_wait_parents(&mut sched, task_id);
                        return Ok(());
                    }
                }
            }
        }

        if kill_current {
            drop(sched);
            crate::scheduler::exit_current_task(0);
        }

        for rq in sched.runqueues.iter_mut() {
            if let Some(i) = rq.tasks_deadline.iter().position(|t| t.id == task_id) {
                let _ = rq.tasks_deadline.remove(i);
                reclaim_task_resources(task_id);
                wake_wait_parents(&mut sched, task_id);
                return Ok(());
            }
            if let Some(i) = rq.tasks_game.iter().position(|t| t.id == task_id) {
                let _ = rq.tasks_game.remove(i);
                reclaim_task_resources(task_id);
                wake_wait_parents(&mut sched, task_id);
                return Ok(());
            }
            if let Some(i) = rq.tasks_normal.iter().position(|t| t.id == task_id) {
                let _ = rq.tasks_normal.remove(i);
                reclaim_task_resources(task_id);
                wake_wait_parents(&mut sched, task_id);
                return Ok(());
            }
        }
        if let Some(i) = sched.blocked_tasks.iter().position(|t| t.id == task_id) {
            let _ = sched.blocked_tasks.remove(i);
            reclaim_task_resources(task_id);
            wake_wait_parents(&mut sched, task_id);
            return Ok(());
        }
        Err(())
    })
}

pub fn try_wait_task(task_id: TaskId) -> WaitResult {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        if let Some(i) = sched.blocked_tasks.iter().position(|t| t.id == task_id) {
            match sched.blocked_tasks[i].state {
                TaskState::Zombie(code) => {
                    // Defer the actual free: move the corpse to `dead_tasks`
                    // instead of dropping the Box here. Dropping inline runs
                    // Task::drop (kernel-stack unmap + page-table teardown +
                    // frame dealloc) while holding the SCHEDULER lock with IRQs
                    // off, and frees frames that may still be referenced by an
                    // in-flight switch on another CPU. The idle thread frees it
                    // later via drain_dead_tasks().
                    let corpse = sched.blocked_tasks.remove(i);
                    sched.dead_tasks.push(corpse);
                    WaitResult::Reaped(code)
                }
                _ => WaitResult::Blocked,
            }
        } else if sched.current_task.iter().flatten().any(|t| t.id == task_id)
            || sched.switch_stash.iter().flatten().any(|t| t.id == task_id)
            || sched.runqueues.iter().any(|rq| {
                rq.tasks_deadline.iter().any(|t| t.id == task_id)
                    || rq.tasks_game.iter().any(|t| t.id == task_id)
                    || rq.tasks_normal.iter().any(|t| t.id == task_id)
            })
        {
            // Child is alive — running, queued, or mid-switch (a stash zombie
            // is also fine to block on: its finish_task_switch files it into
            // blocked_tasks and wake_wait_parents re-readies us for the reap
            // retry). Historically this fell through to NotFound, so a wait
            // on a still-running child returned MAX immediately and parents
            // raced ahead of their children.
            WaitResult::Blocked
        } else {
            WaitResult::NotFound
        }
    })
}

pub fn find_cap_children(
    owner: TaskId,
    handle: crate::capability::CapHandle,
) -> Vec<(TaskId, crate::capability::CapHandle)> {
    let mut children = Vec::new();
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        for t in sched.current_task.iter_mut().flatten() {
            for (h, parent) in t.cap_table.iter_parents() {
                if parent.granter_task == owner && parent.granter_handle == handle {
                    children.push((t.id, h));
                }
            }
        }
        for rq in sched.runqueues.iter_mut() {
            for t in rq.tasks_deadline.iter_mut() {
                for (h, parent) in t.cap_table.iter_parents() {
                    if parent.granter_task == owner && parent.granter_handle == handle {
                        children.push((t.id, h));
                    }
                }
            }
            for t in rq.tasks_game.iter_mut() {
                for (h, parent) in t.cap_table.iter_parents() {
                    if parent.granter_task == owner && parent.granter_handle == handle {
                        children.push((t.id, h));
                    }
                }
            }
            for t in rq.tasks_normal.iter_mut() {
                for (h, parent) in t.cap_table.iter_parents() {
                    if parent.granter_task == owner && parent.granter_handle == handle {
                        children.push((t.id, h));
                    }
                }
            }
        }
        for t in sched.blocked_tasks.iter_mut() {
            for (h, parent) in t.cap_table.iter_parents() {
                if parent.granter_task == owner && parent.granter_handle == handle {
                    children.push((t.id, h));
                }
            }
        }
    });
    children
}

/// SCHED_BODY deadline-miss telemetry for `/proc/raeen/perf`: returns
/// `(total_misses, worst_miss_us)`. Uses `try_lock` so a telemetry read can
/// never deadlock the dump path; returns `(0, 0)` if the scheduler is busy.
pub fn deadline_miss_stats() -> (u64, u64) {
    match SCHEDULER.try_lock() {
        Some(sched) => (
            sched.deadline_stats.total_misses,
            sched.deadline_stats.worst_miss_us,
        ),
        None => (0, 0),
    }
}

pub fn configure_compositor_deadline(task_id: TaskId) -> Result<(), ()> {
    with_task_by_id(task_id, |t| {
        t.priority = TaskPriority::Game;
        t.deadline = Some(DeadlineTask::new(
            COMPOSITOR_PERIOD_US,
            COMPOSITOR_DEADLINE_US,
            COMPOSITOR_RUNTIME_US,
        ));
    })
    .map(|_| ())
    .ok_or(())
}

pub fn configure_audio_deadline(task_id: TaskId) -> Result<(), ()> {
    with_task_by_id(task_id, |t| {
        t.priority = TaskPriority::Game;
        t.deadline = Some(DeadlineTask::new(
            AUDIO_PERIOD_US,
            AUDIO_DEADLINE_US,
            AUDIO_RUNTIME_US,
        ));
    })
    .map(|_| ())
    .ok_or(())
}

pub fn deadline_stats() -> DeadlineStats {
    SCHEDULER.lock().deadline_stats
}

/// Public read of the EDF wall-clock (TSC monotonic microseconds since boot) —
/// the SAME base `pick_next` uses to order deadlines and `record_game_dispatch`
/// uses to detect misses. Exposed so the `sched_proof` short-period workers can
/// measure their OWN per-period dispatch lateness against true wall time (the
/// global perf miss counter cannot attribute a miss to a specific period class).
/// Returns 0 only before APIC TSC calibration (never in the post-boot proof
/// window); callers treat 0 as "skip this sample". Lock-free, no spin — uses the
/// already-calibrated `fast_boot::tsc_mhz()` (ticks/µs), same as `edf_monotonic_us`.
pub fn edf_now_us() -> u64 {
    let mhz = crate::fast_boot::tsc_mhz();
    if mhz == 0 {
        return 0;
    }
    crate::timers::TscCalibration::read_tsc() / mhz
}

/// Current scheduler tick (the monotonic clock that drives the EDF µs base:
/// `now_us = tick * 1000`). Read by `sched_proof` to bound its measurement
/// window in the same clock the scheduler schedules against. Uses a brief
/// bounded `try_lock` retry so a single mid-switch contention never silently
/// returns a bogus 0 (which would corrupt the window's start reference); after
/// the retries it returns 0 only if the lock is genuinely wedged — the caller
/// treats 0 as "skip this sample" since a real post-boot tick is never 0.
pub fn current_tick() -> u64 {
    for _ in 0..1000 {
        if let Some(sched) = SCHEDULER.try_lock() {
            return sched.tick;
        }
        core::hint::spin_loop();
    }
    0
}

pub fn game_mode_active() -> bool {
    SCHEDULER.lock().game_mode.active
}

pub fn null_latency_active() -> bool {
    SCHEDULER.lock().null_latency.active
}

pub fn enter_game_mode() {
    let mut sched = SCHEDULER.lock();
    sched.game_mode.active = true;
    sched.game_mode.reset_counters();
    sched.apply_throttle_flags(true);
}

pub fn exit_game_mode() {
    let mut sched = SCHEDULER.lock();
    sched.game_mode.active = false;
    sched.apply_throttle_flags(false);
}

pub fn enable_null_latency(task_id: TaskId) -> Result<(), ()> {
    let mut sched = SCHEDULER.lock();
    let nl = &mut sched.null_latency;
    nl.active = true;
    nl.game_task_id = Some(task_id);
    nl.dedicated_cores = 0b1111;
    let online_cpus = crate::smp::APS_ONLINE.load(core::sync::atomic::Ordering::Relaxed);
    let online_mask = if online_cpus >= 64 {
        !0
    } else {
        (1u64 << online_cpus) - 1
    };
    nl.non_game_cores = online_mask & !nl.dedicated_cores;
    Ok(())
}

pub fn wake_thread(task_id: TaskId) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        // Race-window catch (see unblock_receivers): a sibling CPU's
        // block_current_task_with may have stashed this exact task with a
        // BlockedOn* state and dropped SCHEDULER. Mark it Ready in the stash
        // — finish_task_switch will route it to a runqueue.
        mark_stash_ready_if(&mut sched, |t| t.id == task_id);
        if let Some(i) = sched.blocked_tasks.iter().position(|t| t.id == task_id) {
            // Only a task in a BlockedOn* state may be woken. blocked_tasks
            // also parks Zombies awaiting parent wait(); a stale wake for a
            // task that has since died (late IPC reply / AER notification /
            // futex wake for a killed waiter) must NOT resurrect the corpse:
            // enqueueing it makes some CPU switch_context into a stale saved
            // SP whose stack the kill path already overwrote → garbage
            // registers → wild jump → the intermittent smp>=4 #DF
            // (MasterChecklist 4.8 duplicate-task race).
            if sched.blocked_tasks[i].state.is_blocked() {
                let mut task = *sched.blocked_tasks.remove(i);
                task.state = TaskState::Ready;
                sched.enqueue(task);
            } else {
                crate::serial_println!(
                    "[sched] stale wake_thread for dead Task {:?} ignored",
                    task_id
                );
            }
        }
    });
}

pub fn wake_thread_direct(task_id: TaskId) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        // Stash race-window catch. wake_thread_direct's normal path bumps
        // priority to Game and pushes to the front of the game runqueue.
        // We can't do the priority + queue-front bit in the stash (the
        // task is still owned by another CPU mid-switch), but we can mark
        // Ready so finish_task_switch routes it; a follow-up
        // wake_thread_direct call after it lands in a runqueue would do the
        // priority bump cleanly. This is rare enough (Game-priority tasks
        // typically aren't on the IPC fast path) that the simplification
        // is worth it.
        mark_stash_ready_if(&mut sched, |t| t.id == task_id);
        if let Some(i) = sched.blocked_tasks.iter().position(|t| t.id == task_id) {
            // Same dead-task guard as wake_thread — never resurrect a Zombie
            // (here it would even jump the queue at Game priority).
            if sched.blocked_tasks[i].state.is_blocked() {
                let mut task = *sched.blocked_tasks.remove(i);
                task.state = TaskState::Ready;
                task.priority = TaskPriority::Game;
                let cpu = sched.select_cpu(&task);
                sched.runqueues[cpu].tasks_game.push_front(task);
            } else {
                crate::serial_println!(
                    "[sched] stale wake_thread_direct for dead Task {:?} ignored",
                    task_id
                );
            }
        }
    });
}

pub fn ap_enter_idle(_cpu_id: usize) {
    x86_64::instructions::interrupts::enable();
    loop {
        // S3 quiesce (see smp::park_aps_for_sleep): the idle-loop top is the
        // AP's proven lock-free point (no scheduler/heap lock can be held
        // here), so this is the ONLY place an AP parks itself for sleep. An
        // AP running stolen work reaches here as soon as its queue drains —
        // a tick or two on the post-boot system.
        if crate::smp::sleep_park_requested() {
            crate::smp::sleep_park_ack_and_halt();
        }
        x86_64::instructions::hlt();
    }
}

/// S3 sleep prep: the APs have been INIT-parked (`smp::park_aps_for_sleep`),
/// so their queued work must not strand and their placeholder contexts are
/// dead. Migrate every task queued on CPUs >= 1 to CPU 0 (the only CPU that
/// resumes) and retire the parked CPUs' `current_task` placeholders into the
/// dead-task graveyard (their saved contexts died with the INIT; the idle
/// thread frees the Boxes safely later). Caller runs this AFTER
/// `park_aps_for_sleep` so `select_cpu` already sees `online == 1`. Returns
/// the number of tasks migrated. Concept §"Fast is a feature" (S3 resume must
/// not lose runnable work).
pub fn offline_aps_for_sleep() -> usize {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        let mut orphans: Vec<Task> = Vec::new();
        for cpu in 1..crate::gdt::MAX_CPUS {
            if let Some(t) = sched.current_task[cpu].take() {
                sched.dead_tasks.push(alloc::boxed::Box::new(t));
            }
            while let Some(t) = sched.runqueues[cpu].tasks_deadline.pop_front() {
                orphans.push(t);
            }
            while let Some(t) = sched.runqueues[cpu].tasks_game.pop_front() {
                orphans.push(t);
            }
            while let Some(t) = sched.runqueues[cpu].tasks_normal.pop_front() {
                orphans.push(t);
            }
        }
        let migrated = orphans.len();
        for t in orphans {
            // enqueue() re-runs select_cpu, which now sees ONLINE_CPUS == 1
            // and homes everything to CPU 0.
            sched.enqueue(t);
        }
        migrated
    })
}

pub fn unblock_virtio_waiters(head: u16) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        // Race-window catch — see unblock_receivers.
        mark_stash_ready_if(&mut sched, |t| {
            matches!(t.state, TaskState::BlockedOnVirtio(_))
        });
        let mut i = 0;
        while i < sched.blocked_tasks.len() {
            if sched.blocked_tasks[i].state == TaskState::BlockedOnVirtio(head) {
                let mut task = *sched.blocked_tasks.remove(i);
                task.state = TaskState::Ready;
                sched.enqueue(task);
            } else {
                i += 1;
            }
        }
    });
}

pub fn unblock_futex_waiter(task_id: TaskId) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut sched = SCHEDULER.lock();
        // Stash race-window catch (see unblock_receivers / wake_thread): the
        // waiter may be mid-block in a sibling CPU's switch_stash (lock
        // dropped between `f()` and `switch_context`). Every other wake path
        // marks it Ready in place so finish_task_switch routes it to a
        // runqueue; this one historically did NOT — a futex wake racing that
        // window was silently LOST (wakes are one-shot: the waiter got filed
        // into blocked_tasks with no second wake coming). Same by-id
        // semantics as wake_thread.
        mark_stash_ready_if(&mut sched, |t| t.id == task_id);
        if let Some(i) = sched.blocked_tasks.iter().position(|t| t.id == task_id) {
            // Dead-task guard (see wake_thread): futex wait-lists hold raw
            // task ids and a killed waiter is never deregistered, so a wake
            // can target a Zombie. Leave the corpse for wait() to reap.
            if sched.blocked_tasks[i].state.is_blocked() {
                let mut task = *sched.blocked_tasks.remove(i);
                task.state = TaskState::Ready;
                sched.enqueue(task);
            } else {
                crate::serial_println!(
                    "[sched] stale futex wake for dead Task {:?} ignored",
                    task_id
                );
            }
        }
    });
}
